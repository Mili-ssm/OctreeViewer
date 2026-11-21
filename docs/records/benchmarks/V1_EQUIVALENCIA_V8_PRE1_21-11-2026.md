# Equivalencia V1 de v8.1-pre frente a v7 — 21-11-2026

**Estado: medido.** Es la tarea 0 del [plan del formato V8 congelado](../archive/formato-v8-congelado.md): comprueba el
requisito V1 del [diseño de la V8](../archive/formato-v8.md) («con la misma entrada el visor dibuja lo mismo que con
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
4. Si el orden de filas de un bloque difiere, compara el multiconjunto ordenado de filas completas (geometría, color de pantalla y columnas) y lo avisa
   (`row_order=multiset`); con los datos reales no ocurrió, y la rama se ejecutó solo con las mutaciones `reorder` y
   `reorder-corrupt` (véase más abajo).
5. **Marco global** (parte de la precondición): `format_metadata().global_frame()` de ambos ficheros debe ser igual
   (celda raíz, CRS, escala y desplazamiento de la fuente, celdas de nodo); si no, la precondición falla.
6. **Comprobación inversa:** ninguna columna almacenada en un bloque de `v8.1-pre` puede faltar en el mismo bloque de v7.

## Resultados

| Entrada | Nodos | Bloques (hojas / LOD) | Puntos hojas | Puntos LOD | Puntos, todos los bloques | Comparaciones de columna | Orden de filas | Diferencias |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: |
| `fs_e430_n4008_18` | 75 | 76 (5 / 71) | 111 190 | 12 344 | 123 534 | 339 | idéntico | **0** |
| `fs_e432_n4002_18` | 79 | 80 (8 / 72) | 149 730 | 37 820 | 187 550 | 376 | idéntico | **0** |
| Belvedere | 3 941 | 3 941 (3 174 / 767) | 81 682 329 | 51 140 224 | 132 822 553 | 41 156 | idéntico | **0** |
| Majadas | 24 091 | 24 091 (19 526 / 4 565) | 415 465 882 | 414 013 425 | 829 479 307 | 233 046 | idéntico | **0** |

- Los puntos de hojas y de LOD salen de la línea `summary` de cada `<caso>.txt` (`points=TODOS (leaf H, LOD L)`). Los
  LOD repiten puntos de las hojas con otra geometría; los de las hojas son los de la entrada (conversor: 81 682 329 en
  Belvedere y 415 465 882 en Majadas).
- Una «comparación de columna» es una columna de un bloque (geometría y color cuentan cada una como una).
- Esquemas: las muestras y Belvedere llevan `gps_time`; Belvedere y Majadas llevan `rgb` (RGB16), las muestras no, así
  que su color de pantalla se deriva de `intensity` (por la regla D2; el ejemplo no distingue el origen, solo compara el resultado). Majadas no tiene `gps_time`. Todas las comparaciones de hojas
  cubren las 9 o 10 columnas del esquema de cada entrada.
- Las salidas por conversión coinciden en tamaño con el [registro del 20-11-2026](FORMATO_V8_PRE1_20-11-2026.md):
  Belvedere 1 420 602 171 B (v7) y 926 000 077 B (v8.1-pre); Majadas 7 688 544 926 B y 4 575 835 569 B.

| Conversión | v7 | v8.1-pre |
| --- | --- | --- |
| Belvedere | 52,3 s, RSS 106 324 kB | 53,2 s, RSS 105 192 kB |
| Majadas | 352,3 s, RSS 243 440 kB | 354,6 s, RSS 239 100 kB |

Son una sola ejecución por caso (con la máquina en uso) y no sustituyen la medida de coste del
[registro del 20-11-2026](FORMATO_V8_PRE1_20-11-2026.md) (V4). El tiempo de la comparación no se registró.

## Prueba de que el comparador puede fallar

`V1_EQUIVALENCE_MUTATE=<modo>` altera en memoria el lado `v8.1-pre` (los ficheros no se tocan; un byte corrompido en
el fichero daría un error de CRC al leer, no una diferencia). Se ejecutó cada modo sobre `fs_e430_n4008_18` (salida
íntegra en [`mutation-check.txt`](data/v1-equivalencia-21-11-2026/mutation-check.txt)):

| Modo | Qué altera | Resultado |
| --- | --- | --- |
| `leaf-column` | invierte un bit de un byte de columna de una hoja | `column 0 (intensity) differs`, 1 diferencia, salida 1 |
| `display` | invierte un bit del color de pantalla de una hoja | `display colour differs`, 1 diferencia, salida 1 |
| `lod-column` | invierte un bit de una columna de un bloque LOD | `column 0 (intensity) differs`, 1 diferencia, salida 1 |
| `stored-set` | quita una columna almacenada de una hoja | `v8.1-pre stores columns [...], expected [...]` y `column 8 missing`, 2 diferencias, salida 1 |
| `reorder` | intercambia las filas 0 y 1 de un bloque de hoja | rama multiconjunto (`row_order=multiset`, `resorted_blocks=1`), **0 diferencias**, salida 0 |
| `reorder-corrupt` | intercambia las filas 0 y 1 e invierte un bit | rama multiconjunto, `sorted row multisets differ`, 1 diferencia, salida 1 |

`reorder` muestra que la rama del multiconjunto se ejecuta y tolera un orden distinto; `reorder-corrupt`, que esa rama
compara filas completas y detecta un valor corrompido. La rama del multiconjunto **solo** se ejecutó con estas
mutaciones, no con datos reales.

## Lo que no se midió

- **Píxeles:** no se renderiza nada; V1 queda probado en los datos que el visor lee, no en la imagen (GPU, shaders y
  modos de color del visor no intervienen). Los modos de color por defecto dependen de `rgb`, `intensity` y
  `classification`, que sí se comparan.
- **Orden de filas distinto en datos reales:** nunca ocurrió (el orden fue idéntico siempre); la rama del
  multiconjunto solo se probó con las mutaciones `reorder` y `reorder-corrupt`.
- **Otras entradas:** Albuera y los datos sintéticos no se repitieron aquí; los tests de `viewer_format`
  (`tests/v8_display.rs`) cubren la equivalencia de lectura con nubes sintéticas.
- **V4 (coste de conversión):** no se repite; el plan lo enlaza al registro del 20-11-2026 (mismo binario).
