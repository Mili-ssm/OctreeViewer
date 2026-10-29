# 0016 Norma portátil en el error de cuantización

Estado: Aceptada · Fecha: 29-10-2026

## Contexto
`viewer_format::quantization` calculaba el error de cada bloque (`measured_error` y
`arithmetic_margin`) con `f64::hypot`, que llama a la libm de la plataforma: glibc en nativo y la libm
derivada de musl que incluye `compiler-builtins` en wasm32. Difieren en 1 ulp para algunas entradas,
y ese `f64` se guarda en el índice del `.oct` (con su CRC). Resultado: el conversor web y el CLI
producían ficheros distintos para el mismo LAS (`small-4096-d2`: 91 bloques, 1 byte de un `f64` del
índice más el CRC). Además, el `hypot` de glibc no está correctamente redondeado (615 de 200.000
entradas aleatorias difieren del redondeo exacto), así que la salida del CLI dependía de la versión de
glibc del equipo. La paridad byte a byte (R1) exige salida idéntica en todos los destinos.

## Decisión
Decisión 4 del propietario (29-10-2026): sustituir `hypot` por `norm3`, una norma euclídea escalada que
solo usa operaciones IEEE-754 correctamente redondeadas (`+ − × ÷` y `sqrt`): `m = max(|x|,|y|,|z|)`;
`0` si `m == 0`; si no, `m · sqrt((x/m)² + (y/m)² + (z/m)²)`, con multiplicaciones explícitas y suma en
orden de ejes. El resultado se aproxima a la norma exacta en pocos ulp; la cota sigue siendo válida
porque `arithmetic_margin` (8·EPSILON·(|p|+|d|+1) por eje) es órdenes de magnitud mayor. Como guarda, `viewer_format` y
`viewer_convert_core` prohíben por clippy (`disallowed-methods`) las funciones de `f32`/`f64` respaldadas
por libm (`hypot`, `powf`, `exp`, `ln`, `log*`, `cbrt`, `sin`, `cos`, `tan`, `atan2`, `mul_add`...).
`powi` queda permitido: se reduce a multiplicaciones de compiler-rt, iguales en ambos destinos.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Crate `libm` en nativo y wasm32 | Añade una dependencia y una versión que fijar para obtener lo que `sqrt` y las cuatro operaciones ya dan; el resultado seguiría atado a esa implementación. |
| Mantener glibc y aceptar la divergencia web | Incumple R1 y deja la salida del CLI dependiente de la versión de glibc. |
| Portar el algoritmo de glibc | Licencia LGPL y atado a una versión concreta de glibc; mucho más código que la norma escalada. |

## Consecuencias
- Los SHA nativos de referencia cambian solo en índice y pie (bytes de payload idénticos); las nuevas
  referencias están en [R1_REFERENCE_SHA_2026-10-29](../records/benchmarks/R1_REFERENCE_SHA_2026-10-29.md),
  y sustituyen a las de M0.
- La salida del CLI deja de depender de la glibc del equipo y coincide con wasm32 (comprobado por
  `tools/convert_parity`, sin casos exentos).
- La guarda de clippy impide reintroducir libm en la ruta de salida; usos legítimos ajenos a ella
  (ejemplos) llevan `#[expect]` con motivo.
- La diagonal de nodo EPT (`viewer_convert/src/ept.rs`, `node_diagonal`), que entra en el error
  geométrico, usa la misma norma: la salida EPT del CLI tampoco depende de glibc. El shell
  `viewer_convert` tiene la misma guarda de clippy.

## Revisar si
Aparece una función libm necesaria en la ruta de salida, o un destino cuya `sqrt` o aritmética básica
no sea IEEE-754 correctamente redondeada.

## Evidencia
[Referencias R1](../records/benchmarks/R1_REFERENCE_SHA_2026-10-29.md) ·
[Paridad M3a](../records/benchmarks/M3A_PARITY_2026-10-29.md) ·
[ADR 0014](0014-nucleo-de-conversion-sin-io.md)
