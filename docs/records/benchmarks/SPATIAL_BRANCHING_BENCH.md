# Experimento de ramificación espacial

Estado: experimento exploratorio; la comparativa sintética y las conversiones directas 2/4/8 están
registradas. Los resultados informan la selección de partición, pero no fijan la política de
producción. Cualquier validación futura de cortes o calidad visual se sigue desde el
[README del proyecto](../../../README.md), que es la única hoja de ruta activa.

Repetir con:

```sh
cargo run -p viewer_format --release --example spatial_branching_bench
```

## Método

El benchmark genera 100.000 puntos deterministas en cinco distribuciones: uniforme aleatoria,
90 % concentrada en un cubo pequeño, plano, línea y rejilla regular de 100 × 100 × 10. En el modo
`point_cap`, particiona con 2, 4 y 8 divisiones por eje, crea solo celdas ocupadas y subdivide hasta
que cada hoja tenga como máximo 2.048 puntos. Conserva cada punto exactamente una vez.

También compara resoluciones espaciales equivalentes: dos niveles de 2 divisiones frente a un nivel
de 4 (`same_extent_4`), y tres niveles de 2 frente a un nivel de 8 (`same_extent_8`). En estas dos
comparaciones se fija la profundidad y no se aplica el límite de puntos por hoja. Así se aísla el
coste de jerarquía; algunas hojas densas superan 2.048 puntos.

Cada payload de hoja contiene tres coordenadas `u32` cuantizadas respecto a la celda y una intensidad
`u16` (14 bytes por punto); se usa el codec LZ4 de `viewer_format`. No incluye colores, otros
atributos, índice, cabeceras ni las muestras de LOD de nodos internos. Las consultas son 100 cajas
AABB deterministas de lado 0,1, centradas en puntos reales. El ratio de candidatos suma todos los
puntos de las hojas cuyas celdas intersectan la caja y lo divide por los puntos de origen; la
sobrecarga divide candidatos por los puntos realmente contenidos en la caja. No mide frustum, caché
ni latencia de lectura. El tiempo es construcción y codificación en memoria, no acceso a disco.

## Partición adaptativa por límite de hoja

| Distribución | Divisiones/eje | Nodos | Hojas | Profundidad | Puntos por hoja (mín/media/máx) | Ratio LZ4 | Candidatos media / p95 | Sobrecarga media / p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Uniforme | 2 | 73 | 64 | 2 | 1.492 / 1.562,5 / 1.640 | 1,0000 | 3,30 % / 6,34 % | 33,29 / 70,29 |
| Uniforme | 4 | 65 | 64 | 1 | 1.492 / 1.562,5 / 1.640 | 1,0000 | 3,30 % / 6,34 % | 33,29 / 70,29 |
| Uniforme | 8 | 513 | 512 | 1 | 154 / 195,3 / 234 | 1,0000 | 0,97 % / 1,60 % | 9,78 / 17,88 |
| Concentrada | 2 | 324 | 283 | 6 | 1 / 353,4 / 1.761 | 1,0000 | 52,94 % / 90,03 % | 3,24 / 17,60 |
| Concentrada | 4 | 1.049 | 1.032 | 3 | 1 / 96,9 / 404 | 1,0000 | 46,51 % / 81,90 % | 3,04 / 17,60 |
| Concentrada | 8 | 1.146 | 1.137 | 2 | 1 / 88,0 / 404 | 1,0000 | 46,48 % / 81,89 % | 1,79 / 6,50 |
| Plana | 2 | 169 | 128 | 3 | 697 / 781,2 / 846 | 1,0000 | 4,45 % / 6,36 % | 4,48 / 6,55 |
| Plana | 4 | 545 | 512 | 2 | 151 / 195,3 / 251 | 1,0000 | 2,42 % / 3,56 % | 2,43 / 3,60 |
| Plana | 8 | 129 | 128 | 1 | 697 / 781,2 / 846 | 1,0000 | 4,45 % / 6,36 % | 4,48 / 6,55 |
| Lineal | 2 | 505 | 442 | 6 | 1 / 226,2 / 1.595 | 1,0000 | 11,31 % / 12,47 % | 1,15 / 1,27 |
| Lineal | 4 | 463 | 442 | 3 | 1 / 226,2 / 1.595 | 1,0000 | 11,31 % / 12,47 % | 1,15 / 1,27 |
| Lineal | 8 | 451 | 442 | 2 | 1 / 226,2 / 1.595 | 1,0000 | 11,31 % / 12,47 % | 1,15 / 1,27 |
| Rejilla | 2 | 73 | 64 | 2 | 1.250 / 1.562,5 / 1.875 | 0,4374 | 3,56 % / 7,50 % | 43,44 / 92,59 |
| Rejilla | 4 | 65 | 64 | 1 | 1.250 / 1.562,5 / 1.875 | 0,4374 | 3,56 % / 7,50 % | 43,44 / 92,59 |
| Rejilla | 8 | 513 | 512 | 1 | 144 / 195,3 / 338 | 0,4546 | 0,97 % / 1,87 % | 11,92 / 23,15 |

## Resolución espacial equivalente

Cada par produjo las mismas celdas hoja, conteos por celda, bytes comprimidos y estadísticas de
consulta. La diferencia está en cuántos nodos intermedios conserva la jerarquía:

| Distribución | Nodos: 2 por eje × 2 niveles / 4 por eje × 1 nivel | Nodos: 2 por eje × 3 niveles / 8 por eje × 1 nivel |
| --- | ---: | ---: |
| Uniforme | 73 / 65 | 585 / 513 |
| Concentrada | 73 / 65 | 585 / 513 |
| Plana | 41 / 33 | 169 / 129 |
| Lineal | 31 / 23 | 81 / 51 |
| Rejilla | 73 / 65 | 585 / 513 |

El factor 4 reduce la profundidad en un nivel y elimina entre 8 nodos intermedios para esta
resolución. El factor 8 elimina entre 30 y 72 nodos respecto a tres niveles binarios. Esa reducción
abarata el índice sintético, mientras que una jerarquía binaria conserva más puntos donde podrían
ubicarse muestras LOD; el benchmark no mide el valor visual ni los bytes de esas muestras.

## Datos reales: jerarquías de resolución equivalente

El ejemplo `viewer_convert/examples/spatial_quality_bench.rs` evalúa los mismos cortes en una
conversión real:

```sh
cargo run --release -p viewer_convert --example spatial_quality_bench -- SOURCE DATASET.oct
```

Para una lectura rápida de nodos y bytes retenidos, sin volver a recorrer la fuente:

```sh
cargo run --release -p viewer_convert --example spatial_quality_bench -- SOURCE DATASET.oct --metadata-only
```

Para analizar una conversión directa 4×4×4 u 8×8×8, indique la misma opción usada al convertir:

```sh
cargo run --release -p viewer_convert --example spatial_quality_bench -- SOURCE DATASET.oct --spatial-divisions 4
```

El factor 2/4/8 por eje agrupa uno, dos o tres niveles binarios del octree existente. Cada candidato
conserva la raíz, nodos internos en múltiplos del nivel agrupado y todas las hojas; los nodos
conservados se conectarían a su ancestro conservado más cercano. No se genera un `.oct` candidato:
los bytes LZ4 reportados son la suma exacta de los payloads que quedarían, mientras que el nuevo
índice/cabecera no se vuelve a serializar. La comparación conserva la partición de hojas 2×2×2 y no
simula la partición adaptativa directa 4×4×4 u 8×8×8.

La calidad se mide sobre puntos fuente seleccionados por un hash estable del índice, hasta un millón
por dataset. Cada punto se asigna a su hoja o muestra LOD en la grilla 8×8×8 de su nodo. El residuo
con signo es `representante - fuente`; RMSE, p95 y máximo son distancias euclidianas observadas en la
muestra. El error declarado es el máximo de los nodos seleccionados en esa misma muestra. Las hojas
se tratan como representación exacta a efectos del residuo; su error de cuantización sigue dentro de
la cota declarada. El salto de transición compara, para los mismos puntos, los representantes de
cortes sucesivos (2→4 y 4→8). Estos máximos son observados, no cotas globales ni una evaluación visual.

Para las dos teselas USGS se convirtió con `--points-per-block 10000` para que datasets menores que
el límite predeterminado de 262.144 puntos generasen niveles LOD. Belvedere usa la conversión por
defecto ya medida en [REAL_DATASET_BENCH.md](REAL_DATASET_BENCH.md). Las coordenadas se expresan en
las unidades de cada fuente. Para repetir una tesela pequeña desde la raíz del repositorio:

```sh
cargo run --release -p viewer_convert -- data/samples/fs_e432_n4002_18.las /tmp/fs_e432.oct --points-per-block 10000
cargo run --release -p viewer_convert --example spatial_quality_bench -- data/samples/fs_e432_n4002_18.las /tmp/fs_e432.oct
```

La medición se ejecutó el 9 de octubre de 2026 en Linux x86-64, AMD Ryzen AI 9 365, Rust 1.99.0.
El análisis completo relee secuencialmente la fuente; no mide tiempo de conversión ni RSS.

| Dataset / muestra | Divisiones/eje | Nodos conservados | Payload LZ4 retenido | Payload bruto retenido | Ratio LZ4 |
| --- | ---: | ---: | ---: | ---: | ---: |
| USGS `fs_e432_n4002_18` (149.730 puntos) | 2 | 55 | 3.063.713 B | 5.264.735 B | 0,581931 |
|  | 4 | 46 | 3.056.095 B | 5.248.110 B | 0,582323 |
|  | 8 | 48 | 3.059.368 B | 5.255.670 B | 0,582108 |
| USGS `fs_e430_n4008_18` (111.190 puntos) | 2 | 47 | 2.259.661 B | 3.911.880 B | 0,577641 |
|  | 4 | 39 | 2.253.172 B | 3.897.880 B | 0,578051 |
|  | 8 | 41 | 2.256.029 B | 3.904.355 B | 0,577824 |
| Belvedere COPC LAZ (81.682.329 puntos; muestra 995.791) | 2 | 1.343 | 1.596.095.673 B | 3.349.954.651 B | 0,476453 |
|  | 4 | 1.270 | 1.596.010.185 B | 3.349.733.948 B | 0,476459 |
|  | 8 | 1.153 | 1.595.798.740 B | 3.349.186.188 B | 0,476474 |

| Dataset | Divisiones/eje | Sesgo medio XYZ | RMSE | p95 observado | Máximo observado | Error declarado máx. | Violaciones muestreadas | Salto p95 / máximo observado |
| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| USGS `fs_e432_n4002_18` | 2 | 0,105 / −0,237 / 57,644 | 77,127 | 94,993 | 107,485 | 108,251 | 0 | — |
|  | 4 | 0,196 / 0,046 / 25,495 | 36,175 | 46,069 | 53,589 | 54,126 | 0 | 54,126 / 106,024 |
|  | 8 | 0,006 / −0,026 / 6,588 | 13,290 | 21,194 | 26,517 | 27,063 | 0 | 43,158 / 53,589 |
| USGS `fs_e430_n4008_18` | 2 | −0,071 / −0,209 / 58,231 | 77,595 | 95,111 | 107,529 | 108,251 | 0 | — |
|  | 4 | −0,217 / −0,178 / 24,347 | 35,391 | 46,187 | 53,415 | 54,126 | 0 | 77,302 / 107,096 |
|  | 8 | −0,039 / −0,021 / 7,012 | 13,636 | 21,704 | 26,516 | 27,063 | 0 | 42,454 / 53,415 |
| Belvedere COPC LAZ | 2 | 3,169 / −0,400 / −1,125 | 190,205 | 261,881 | 324,585 | 328,199 | 0 | — |
|  | 4 | 0,628 / −0,002 / −0,399 | 94,407 | 130,713 | 163,049 | 164,100 | 0 | 164,100 / 318,499 |
|  | 8 | 0,149 / 0,023 / 0,013 | 46,974 | 65,126 | 81,661 | 82,050 | 0 | 82,050 / 161,162 |

En estas tres fuentes, conservar niveles más profundos reduce el error observado y el sesgo medio,
especialmente en Z para las teselas costeras. El payload retenido y su ratio LZ4 cambian muy poco;
además, 8 divisiones no siempre retienen menos bytes o nodos que 4 porque las hojas adaptativas no
coinciden con los mismos niveles. No se observó una violación de la cota geométrica entre los puntos
muestreados. El resultado apoya evaluar profundidad y calidad por separado de compresión. Sus cifras
describen retención de niveles en una jerarquía binaria y no predicen las salidas directas medidas a
continuación.

## Conversiones adaptativas directas 2/4/8

El conversor ahora crea realmente 2×2×2, 4×4×4 u 8×8×8 hijos por nodo; usa 2 por defecto. Para
repetir la conversión y la evaluación de calidad del COPC Belvedere:

```sh
cargo run --release -p viewer_convert -- data/datasets/belv_2009_digital-aerial_pcd_copc.laz /tmp/belv.oct --spatial-divisions 8
cargo run --release -p viewer_convert --example spatial_quality_bench -- data/datasets/belv_2009_digital-aerial_pcd_copc.laz /tmp/belv.oct --spatial-divisions 8
```

Mediciones del 9 de octubre de 2026 en Linux x86-64, AMD Ryzen AI 9 365, Rust 1.99.0, misma fuente y
opciones de conversión salvo divisiones, una ejecución por variante. `/usr/bin/time -v` midió el
tiempo total y RSS. El análisis de calidad muestreó 995.791 de los 81.682.329 puntos fuente, con el
mismo hash e intervalo de muestreo en las tres conversiones.

| Divisiones/eje | Tiempo | Archivo `.oct` | Bloques | Payload LZ4 | Ratio payload | Memoria estimada | RSS pico |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2 | 99,68 s | 1.596.356.685 B | 1.343 | 1.596.095.673 B | 0,476453 | 60 MiB | 36,3 MiB |
| 4 | 83,91 s | 1.592.927.212 B | 4.435 | 1.592.066.352 B | 0,475281 | 63 MiB | 39,5 MiB |
| 8 | 68,67 s | 1.592.150.607 B | 5.140 | 1.591.152.977 B | 0,475086 | 67 MiB | 39,8 MiB |

El factor 2 nuevo conservó exactamente el número de bloques y los bytes del payload de la salida
histórica; el archivo total cambió en 11 bytes por la descripción de la política de muestreo. Los
factores 4 y 8 redujeron el tamaño total en aproximadamente 0,21 % y 0,26 %, pero multiplicaron el
número de bloques por 3,3× y 3,8×. En esta ejecución, el tiempo bajó aproximadamente 16 % y 31 %;
son mediciones de una sola pasada, no una garantía de rendimiento. No se midió el pico de spools en
disco de las variantes directas.

| División/eje | Nivel medido | Celdas nominales/eje | RMSE | p95 | Máximo observado | Máximo de error declarado | Violaciones |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2 | 1 | 2 | 190,205 | 261,881 | 324,585 | 328,199 | 0 |
| 2 | 2 | 4 | 94,407 | 130,713 | 163,049 | 164,100 | 0 |
| 2 | 3 | 8 | 46,974 | 65,126 | 81,661 | 82,050 | 0 |
| 4 | 1 | 4 | 94,407 | 130,713 | 163,049 | 164,100 | 0 |
| 4 | 2 | 16 | 21,694 | 32,287 | 40,871 | 41,025 | 0 |
| 4 | 3 | 64 | 0,000 | 0,000 | 0,000 | 0,000 | 0 |
| 8 | 1 | 8 | 46,974 | 65,126 | 81,661 | 82,050 | 0 |
| 8 | 2 | 64 | 0,000 | 0,000 | 0,000 | 0,000 | 0 |

Los niveles mostrados no representan cortes de igual tamaño físico entre todas las variantes. Los
cortes de nivel 3 para factor 4 y nivel 2 para factor 8 ya llegan a hojas completas en esta nube, por
eso su residuo muestreado es cero salvo la cuantización de coordenadas. En los primeros niveles, los
errores bajan al aumentar las divisiones, aunque el número de bloques y la complejidad del índice
suben. El estudio previo de jerarquías equivalentes conserva las mismas hojas y no se debe confundir
con esta conversión directa.

La salida directa 2×2×2 generada en esta medición tiene payload idéntico a la referencia guardada.
La prueba de equivalencia de workers también cubre partición 4×4×4; la opción de división se guarda
en los checkpoints y no se puede cambiar al reanudar. Los manifiestos anteriores a V3 se rechazan: sus registros de spool no permiten reanudar
con el layout compacto actual.

## Cortes directos con presupuestos iguales

El ejemplo acepta varios `--point-budget` en una pasada. Se generaron las salidas directas 4×4×4 y
8×8×8 indicadas arriba y se compararon con el `.oct` 2×2×2 guardado:

```sh
cargo run --release -p viewer_convert -- data/datasets/belv_2009_digital-aerial_pcd_copc.laz /tmp/belv-direct-4.oct --spatial-divisions 4
cargo run --release -p viewer_convert -- data/datasets/belv_2009_digital-aerial_pcd_copc.laz /tmp/belv-direct-8.oct --spatial-divisions 8
cargo run --release -p viewer_convert --example spatial_quality_bench -- data/datasets/belv_2009_digital-aerial_pcd_copc.laz data/datasets/belv_2009_digital-aerial_pcd_copc.oct --point-budget 1000000 --point-budget 4000000
cargo run --release -p viewer_convert --example spatial_quality_bench -- data/datasets/belv_2009_digital-aerial_pcd_copc.laz /tmp/belv-direct-4.oct --spatial-divisions 4 --point-budget 1000000 --point-budget 4000000
cargo run --release -p viewer_convert --example spatial_quality_bench -- data/datasets/belv_2009_digital-aerial_pcd_copc.laz /tmp/belv-direct-8.oct --spatial-divisions 8 --point-budget 1000000 --point-budget 4000000
```

Cada corte usa la selección LOD del núcleo, un frustum que cubre el dataset completo y presupuesto
estricto. El selector refina primero el nodo con mayor error geométrico declarado, usando tolerancia
cero; al exceder el presupuesto conserva el corte previo en esa rama. El error se mide en la misma
muestra estable de 995.791 puntos fuente (hash e intervalo idénticos en las variantes). El frustum
completo y la prioridad por error mundial sirven para comparar de forma reproducible, pero no
representan una cámara ni equivalen a un objetivo de error en píxeles. La salida contabiliza
presupuesto usado, nodos seleccionados y nodos cuya refinación quedó limitada por el presupuesto.

Medición del 9 de octubre de 2026 con el mismo hardware y versiones de la sección anterior:

| Divisiones/eje | Presupuesto | Puntos seleccionados | Uso del presupuesto | Nodos | Nodos limitados | RMSE | p95 | Máximo observado | Violaciones de error declarado |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2 | 1 M | 997.985 | 99,80 % | 100 | 82 | 45,246 | 64,680 | 81,661 | 0 |
| 4 | 1 M | 971.199 | 97,12 % | 41 | 22 | 92,552 | 130,258 | 163,049 | 0 |
| 8 | 1 M | 915.526 | 91,55 % | 81 | 69 | 46,974 | 65,126 | 81,661 | 0 |
| 2 | 4 M | 3.993.767 | 99,84 % | 146 | 97 | 41,721 | 63,643 | 81,661 | 0 |
| 4 | 4 M | 3.903.596 | 97,59 % | 89 | 44 | 85,512 | 128,703 | 163,049 | 0 |
| 8 | 4 M | 3.914.315 | 97,86 % | 295 | 66 | 46,112 | 64,971 | 81,661 | 0 |

El factor 4 da mayor error observado en ambos presupuestos para esta política de selección; 2 y 8
quedan cerca, con ventaja pequeña de 2 en RMSE. El selector añade hijos por rama como unidad, por lo
que puede quedar capacidad sin usar. No hay violaciones en los puntos muestreados; esto no prueba una
cota global. La comparación numérica queda cerrada, pero la revisión visual en navegador sigue
pendiente y el valor predeterminado permanece en 2×2×2.

## Lectura y límites

- En los datos uniformes y la rejilla, el modo adaptativo con 8 mejora la selección espacial frente
  a 2/4, con ocho veces más hojas. La compresión de la rejilla empeora de 0,4374 a 0,4546.
- En datos concentrados, el límite de hoja obliga a subdividir de forma adaptativa. El factor 8
  reduce la sobrecarga de candidatos, pero no gana en cantidad de hojas ni en todas las distribuciones.
- No aparece un ganador universal. Los resultados muestran el coste de los factores y de la
  jerarquía; todavía falta comparar calidad de LOD y memoria fuera de núcleo.
- COPC, EPT y PCL usan octrees con hasta ocho hijos al bisecar los ejes ([COPC](https://copc.io/),
  [EPT](https://entwine.io/en/latest/entwine-point-tile.html),
  [PCL](https://pointclouds.org/documentation/tutorials/octree.html)). Schütz et al. estudian
  generación LOD fuera de memoria con octrees adaptativos y discuten los efectos de densidad desigual
  y localidad de entrada ([Computer Graphics Forum, 2020](https://doi.org/10.1111/cgf.14134)).

Medición sintética en Linux x86-64, AMD Ryzen AI 9 365, Rust 1.99.0, perfil `release`. La construcción y
codificación en memoria tarda pocos milisegundos y no sirve para comparar rendimiento de producción.
El benchmark no usa LAS/LAZ reales, no incluye lectura/escritura en disco, RSS, índice del formato,
muestras internas, error geométrico, transiciones LOD ni calidad visual. La elección de producción
queda abierta hasta medir fixtures representativos, cobertura y error de representantes, bytes
totales, puntos leídos por consulta y memoria pico.
