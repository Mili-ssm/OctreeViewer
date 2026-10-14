# Diccionarios de atributos — experimento 14-10-2026

Hasta este experimento no había diccionario en `.octattrs`: v5 y v6 pre1/pre2 comprimen RGB y
cada columna tipada por separado con raw/LZ4 (`writer.rs`, `append_payload`). La idea estaba
pendiente en la preparación de v6. Este trabajo implementa un **benchmark experimental**, no
un backend publicado ni una revisión pre3. Los archivos existentes y su lectura siguen iguales.

## Qué se ha medido

El ejemplo [attribute_dictionary_bench.rs](../crates/viewer_format/examples/attribute_dictionary_bench.rs)
lee exclusivamente colores/atributos de parejas definitivas pre2 y conserva todos los bits,
componentes y filas, también representantes LOD. Compara tres contenedores materializados:

1. `baseline`: una secuencia de streams raw/LZ4 por bloque y columna, como el escritor actual.
2. `column_adaptive`: diccionario local de valores completos de cada columna (incluidos sus
   componentes), o baseline si éste ocupa menos. Mantiene columnas independientes.
3. `tuple_dictionary`: diccionario **forzado** de la combinación completa de RGB8 y todas las
   columnas, con un ID por fila. Permite ver tanto los beneficios como los contraejemplos;
   no selecciona automáticamente el mejor formato ni conserva independencia entre columnas.

Las claves son bytes little-endian; los floats no se comparan numéricamente ni se convierten
los enteros a `f64`. El ID usa `ceil(log2(cardinalidad))` bits, empaquetados LSB; una combinación
constante no necesita IDs. Se aplica raw/LZ4 por separado a tabla e IDs. Cada stream incluye
32 bytes de cabecera: modo, codecs, bits/ID y siete u32 (filas, ancho, cardinalidad, dos tamaños
raw y dos tamaños almacenados). No hay padding adicional.

Cada stream se escribe y se relee inmediatamente, se decodifica y compara byte a byte con la
entrada en las tres repeticiones. La igualdad del baseline sin sus cabeceras contra el tamaño
de los payloads originales también se verifica. El runner guarda SHA-256 de entradas, binario,
fuentes, contenedores y logs. No se prueba lectura en frío ni durabilidad frente a fallos.

## Tamaños observados

Bytes lógicos de archivo, incluidos diccionario, IDs, compresión y cabeceras experimentales.
La columna vigente son **payloads** originales de `.octattrs`; su archivo completo añade 64 B.
El baseline experimental añade 32 B por columna/bloque, por lo que no coincide con esa columna.

| Muestra | Filas almacenadas / bloques | Payloads vigentes | Baseline experimental | Diccionario por columna adaptativo | Diccionario de combinación completa |
| --- | ---: | ---: | ---: | ---: | ---: |
| Outer Banks e430 | 111.768 / 47 | 914.901 | 928.437 | 810.533 | 1.172.562 |
| Bunny parcial | 40.577 / 19 | 4.150 | 9.014 | 5.149 | 893 |
| Sintética agrupada | 4.096 / 1 | 41.180 | 41.468 | 41.250 | 59.422 |
| Belvedere completo | 81.706.211 / 1.343 | 610.586.795 | 611.016.555 | 439.593.178 | 341.724.528 |

En Belvedere, contando **también** todas las cabeceras experimentales, el ahorro frente a los
payloads vigentes es **28,00 % por columna** y **44,03 % por combinación completa**. Hay
17.578.926 tuplas distintas sumadas por bloque, un 21,51 % de las filas; no son una cardinalidad
global. Los campos constantes contribuyen a que se repitan combinaciones completas. RGB8 y
RGB16 concentran el tamaño: pasan de 243.949.282 y 360.106.626 B de payload a 200.935.014 y
238.287.496 B de streams adaptativos con cabeceras. Los otros ocho campos ensayados son
constantes dentro de cada bloque: sus diccionarios tienen una entrada por bloque.

Si se conservaran el `.oct`, su índice y los 64 B de envoltura actuales, sustituir únicamente los
payloads por estos contenedores daría **una proyección**, no archivos `.octattrs` compatibles:
la pareja Belvedere pasaría de 1.590.761.099 a 1.419.767.482 / 1.321.898.832 B
(10,75 % / 16,90 % menos). Falta definir el contrato de codec/metadatos/CRC y medir su resultado
real en una revisión futura. La proyección no incluye un nuevo índice para tuplas.

Los contraejemplos importan: el diccionario conjunto **aumenta** los bytes un 28,16 % en Outer
Banks y un 44,30 % en la sintética. El adaptativo por columna reduce ambos contenedores frente
al baseline experimental, pero en Bunny y la sintética sus cabeceras hacen que todavía ocupe
más que los payloads vigentes. Elegir un codec siempre debe contar su metadata incremental.

## CPU y acceso selectivo

Equipo: Ryzen AI 9 365, Linux x86_64, perfil Cargo `perf`, NVMe local Btrfs zstd:1. El runner
identifica plataforma, hashes y comandos. Los tamaños son longitudes de archivo, **no** bloques
físicos después de la compresión transparente del filesystem. Caché del SO sin controlar;
relectura inmediata de cada stream. Un proceso por muestra, orden fijo baseline → adaptativo
→ tuplas por bloque. Tres operaciones CPU por stream; se suma la media de cada bloque.

| Belvedere | Codificar CPU | Decodificar CPU |
| --- | ---: | ---: |
| Baseline | 1,102 s | 0,144 s |
| Por columna adaptativo | 12,439 s | 2,737 s |
| Combinación completa | 3,185 s | 0,944 s |

El adaptativo incluye construir **ambos candidatos** y elegir; es su coste de búsqueda real,
no el coste de codificar sólo el ganador. Las tuplas añaden **1,488 s** para intercalar columnas,
medido una vez por bloque y separado de la tabla. No se cronometran lectura del origen,
serialización inicial de atributos tipados, escritura/relectura de archivos, comparación,
hashing ni proyección de tuplas a columnas. La decodificación devuelve bytes; no construye
`AttributeColumn`. No son tiempos de conversión/lectura end-to-end, ni tres procesos alternados.
No se midió RSS ni rendimiento WASM.

La independencia tiene valor: consultar sólo `classification` lee actualmente **335.757 B**
de payload sumados en Belvedere; su stream adaptativo ocupa 44.319 B con cabeceras. El diccionario
conjunto exigiría acceder al stream combinado de **341.724.528 B** para esa consulta con el
layout ensayado, unas **1.018 veces** los bytes vigentes. Se puede proyectar una columna sin
materializar las demás, pero seguirá acoplada a su tabla y sus IDs. Es un conteo de bytes,
no una medición de latencia de consultas selectivas.

## Decisiones y siguientes experimentos

1. **Candidato principal: codecs independientes por columna y bloque.** Probar primero un modo
   constante, después bitpacking para enteros de rango pequeño y diccionario exacto. Usar
   metadata mínima sólo cuando sea necesario y fallback raw/LZ4 contando todos los bytes.
   Los campos constantes de Belvedere justifican medir un modo constante sin tablas/IDs.
2. **Paletas compartidas sólo para grupos elegidos.** Probar RGB8+RGB16 conjuntamente y mantener
   clasificación, retornos y tiempo separados. Medir consulta de RGB frente a atributo numérico;
   el resultado del diccionario de todas las columnas no demuestra el de este grupo.
   Evaluar RGB8 derivable exactamente de RGB16 o intensidad sólo cuando el bloque satisfaga
   esa igualdad; no descartar colores originales ni introducir aproximaciones.
3. **Topología `.oct`: omitir prefijos reconstruibles.** En pre2 `first_child` es el prefijo de
   `popcount(mask)` de los nodos anteriores del nivel. El lector ya descomprime el bloque
   completo, por lo que puede reconstruirlo en una pasada. Omitirlo elimina 4 bytes brutos
   por nodo interno; no demuestra ese ahorro tras LZ4.
4. **Hojas implícitas y máscara como dato fundamental.** El último nivel ya identifica hojas;
   omitir sus registros elimina 6 bytes brutos por hoja. Inferir conteo/octante desde máscaras
   permite ensayar un byte por nodo interno. Medir ambos cambios por separado y juntos contra
   pre2, incluyendo conteos de niveles, mapas de filas, residuales, CPU y fallback.
5. **Residuales/índice exterior, después.** Comparar planos de bits y deltas de residuales, y
   columnas compactas de metadatos. Cualquier reordenación espacial necesita conservar la
   asociación de filas con atributos y contar su permutación. La grilla física/CRS compartida
   sigue siendo un contrato distinto de esta compresión.

Cierre antes de integrar codecs nuevos: revisión física explícita, exactitud de los diez tipos
primitivos y componentes, corrupción/límites/CRC, lectura selectiva, checkpoints, WASM, archivos
realmente válidos, conversiones y lecturas alternadas, CPU/RSS y más corpus (Majadas/Albuera).
El benchmark actual acredita tamaños y roundtrip de streams, no esa integración.

## Reproducción y evidencia

[Resumen JSON](benchmarks/attribute-dictionary-2026-10-14.json) conserva agregados, comandos,
hashes y cardinalidad. El reporte íntegro y los contenedores están en
`target/attribute-dictionary-20261014/` (ignorados por Git; no se redistribuyen fuentes).
Las entradas completas y su conversión están identificadas por el
[benchmark pre2](benchmarks/storage-v6-pre2-comparison-2026-10-14.json).

```sh
cargo build --offline --profile perf -p viewer_format --example attribute_dictionary_bench
python3 crates/viewer_format/benchmarks/attribute_dictionary_suite.py \
  --binary target/perf/examples/attribute_dictionary_bench \
  --output target/dictionary-new-run \
  --sample Belvedere=RUTA/PAREJA.oct --repetitions 3
cargo test --offline -p viewer_format --example attribute_dictionary_bench
```

Para un prefijo usar `--max-blocks N`; el orden es el del directorio del lector, no una selección
espacial ni un muestreo aleatorio. No comparar prefijos con archivos completos.

Validación de esta entrega: 88 tests de `viewer_format --all-targets` pasan (incluidos dos del
nuevo ejemplo), Clippy con `-D warnings`, `cargo fmt`, compilación sintáctica Python, `--help`,
los cuatro benchmarks completos, enlaces locales y `git diff --check`. Ruff no está instalado
en este entorno. La revisión focalizada del experimento confirma la necesidad de distinguir
coste de búsqueda adaptativa, cabeceras experimentales y relectura caliente; son los límites
explicitados arriba. No se modifica interfaz web ni backend WASM.
