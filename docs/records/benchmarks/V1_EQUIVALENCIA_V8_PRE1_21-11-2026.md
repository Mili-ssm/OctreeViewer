# Equivalencia V1 de v8.1-pre frente a v7 — 21-11-2026

**Estado: medido.** Es la tarea 0 del [plan del formato V8 congelado](../../plans/formato-v8-congelado.md): comprueba el
requisito V1 del [diseño de la V8](../../plans/formato-v8.md) («con la misma entrada el visor dibuja lo mismo que con
v7, en todos los modos de color por defecto y en todos los niveles») a nivel de datos, mientras v7 todavía se puede
escribir.

**Conclusión:** en las cuatro entradas (dos muestras de `data/samples`, Belvedere y Majadas), la salida `v8.1-pre` y la
salida v7 de la misma entrada, convertidas con el mismo binario, **cumplen la precondición (mismo esquema, jerarquía,
bloques, número de puntos y geometría cuantizada) y tienen 0 diferencias**: todas las columnas de las hojas son iguales
byte a byte, el color de pantalla que devuelve el lector coincide con el almacenado en v7, y los nodos LOD coinciden en
geometría, color de pantalla, intensidad y clase. En `v8.1-pre` solo faltan en los LOD las columnas que D2 retira
(ninguna otra). El orden de filas dentro de cada bloque es idéntico en todos los casos (no hizo falta comparar
multiconjuntos). La comprobación es de **datos, no de píxeles**: no ejecuta la GPU ni el visor.

## Condiciones

| Dato | Valor |
| --- | --- |
| Binario | `viewer_convert` `release` compilado desde `9846271` (`main`, rama `feat/formato-v8-final` sin cambios en el conversor); el mismo binario escribe v7 y `v8.1-pre`, ficheros recién convertidos (no se reutilizó `target/render-data`) |
| Comparador | Ejemplo [`v1_equivalence`](../../../crates/viewer_format/examples/v1_equivalence.rs), `release`; abre cada pareja `.oct` + `.octattrs` con `FormatReader` (v7 y v8.1-pre) |
| Orden v7 | `target/release/viewer_convert ENTRADA target/v1-gate/<caso>.v7.oct` (v7 es el formato por defecto) |
| Orden v8.1-pre | `target/release/viewer_convert ENTRADA target/v1-gate/<caso>.v8pre1.oct --format-version v8.1-pre` |
| Orden de comparación | `target/release/examples/v1_equivalence target/v1-gate/<caso>.v7.oct target/v1-gate/<caso>.v8pre1.oct` |
| Entradas | `data/samples/fs_e430_n4008_18.las` y `fs_e432_n4002_18.las`; Belvedere (`belv_2009_digital-aerial_pcd_copc.laz`); Majadas (`SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz`) |
| Equipo | El del propietario (AMD Ryzen AI 9 365, 20 hilos, 30 GiB de RAM). Conversiones una cada vez |
| Datos | [`data/v1-equivalencia-21-11-2026/`](data/v1-equivalencia-21-11-2026/): `<caso>.txt` (salida íntegra del comparador), `<nube>.convert-*.txt` (registro del conversor y tiempo), `mutation-check.txt` |

## Qué compara el ejemplo

1. **Precondición** (se comprueba primero; si falla, sale con error y dice cuál): mismo esquema de atributos, misma raíz
   y mismos ids de bloque; en cada nodo mismo padre, nivel, caja, hijos y bloques; en cada bloque mismo número de puntos
   (y la suma por nodo coincide con el índice) y mismas coordenadas cuantizadas exactas (`read_block_quanta`, `i64`).
2. **Hojas** (nodos sin hijos): cada columna del esquema, byte a byte (`intensity`, `classification`,
   `return_number`, `number_of_returns`, `scan_angle_degrees`, `user_data`, `point_source_id`, `gps_time` si existe,
   `rgb` si existe —RGB16— y `las_flags`), más el color de pantalla de `read_display_colors` (en v8.1-pre derivado
   por el lector de `rgb` o de `intensity`) frente al almacenado en v7. Además, `v8.1-pre` debe almacenar exactamente
   todas las columnas.
3. **LOD** (nodos con hijos): geometría, color de pantalla, `intensity` y `classification`; `v8.1-pre` debe almacenar
   exactamente el conjunto LOD de la cabecera (que el ejemplo exige igual a `intensity` + `classification`) y nada más.
4. Si el orden de filas de un bloque difiere, compara el multiconjunto ordenado de filas completas y lo avisa
   (`row_order=multiset`); aquí no ocurrió.

## Resultados

| Entrada | Nodos | Bloques (hojas / LOD) | Puntos comparados | Comparaciones de columna | Orden de filas | Diferencias |
| --- | ---: | ---: | ---: | ---: | --- | ---: |
| `fs_e430_n4008_18` | 75 | 76 (5 / 71 nodos) | 123 534 | 339 | idéntico | **0** |
| `fs_e432_n4002_18` | 79 | 80 (8 / 72 nodos) | 187 550 | 376 | idéntico | **0** |
| Belvedere | 3 941 | 3 941 (3 174 / 767) | 132 822 553 | 41 156 | idéntico | **0** |
| Majadas | 24 091 | 24 091 (19 526 / 4 565) | 829 479 307 | 233 046 | idéntico | **0** |

- «Puntos comparados» suma los puntos de todos los bloques, hojas y LOD (los LOD repiten puntos de las hojas con
  otra geometría): las hojas de Belvedere y Majadas suman 81 682 329 y 415 465 882 puntos (los de la entrada).
- Una «comparación de columna» es una columna de un bloque (geometría y color cuentan cada una como una).
- Esquemas: las muestras y Belvedere llevan `gps_time`; Belvedere y Majadas llevan `rgb` (RGB16), las muestras no, así
  que su color de pantalla se deriva de `intensity` (por la regla D2; el ejemplo no distingue el origen, solo compara el resultado). Majadas no tiene `gps_time`. Todas las comparaciones de hojas
  cubren las 9 o 10 columnas del esquema de cada entrada.
- Las salidas por conversión coinciden en tamaño con el [registro del 20-11-2026](FORMATO_V8_PRE1_20-11-2026.md):
  Belvedere 1 420 602 171 B (v7) y 926 000 077 B (v8.1-pre); Majadas 7 688 544 926 B y 4 575 835 569 B.

| Conversión | v7 | v8.1-pre |
| --- | --- | --- |
| Belvedere | 56,6 s, RSS 106 520 kB | 51,4 s, RSS 104 908 kB |
| Majadas | 338,8 s, RSS 232 856 kB | 334,6 s, RSS 259 564 kB |

La comparación tarda 23,6 s (Belvedere) y 119,3 s (Majadas); es una sola ejecución por caso y no sustituye la medida
de coste del [registro del 20-11-2026](FORMATO_V8_PRE1_20-11-2026.md) (V4).

## Prueba de que el comparador puede fallar

Con `V1_EQUIVALENCE_MUTATE=1` el ejemplo invierte un bit de un byte de una columna de hoja de `v8.1-pre` en memoria
(sin tocar el fichero) y compara `fs_e430_n4008_18`: informa `column 0 (intensity) differs`, `differences=1` y sale con
código 1 ([`mutation-check.txt`](data/v1-equivalencia-21-11-2026/mutation-check.txt)). Un byte corrompido en el
fichero daría un error de CRC al leer, no una diferencia, por eso la mutación es en memoria.

## Lo que no se midió

- **Píxeles:** no se renderiza nada; V1 queda probado en los datos que el visor lee, no en la imagen (GPU, shaders y
  modos de color del visor no intervienen). Los modos de color por defecto dependen de `rgb`, `intensity` y
  `classification`, que sí se comparan.
- **Orden de filas distinto:** la rama del multiconjunto existe pero no se ejecutó (el orden fue idéntico siempre), así
  que solo se probó con la mutación, no con datos reales.
- **Otras entradas:** Albuera y los datos sintéticos no se repitieron aquí; los tests de `viewer_format`
  (`tests/v8_display.rs`) cubren la equivalencia de lectura con nubes sintéticas.
- **V4 (coste de conversión):** no se repite; el plan lo enlaza al registro del 20-11-2026 (mismo binario).
