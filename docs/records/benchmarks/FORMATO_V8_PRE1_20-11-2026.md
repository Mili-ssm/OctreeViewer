# Formato v8.1-pre frente a v7 — 20-11-2026

**Estado: medido.** Es la tarea 8 del [plan de v8.1-pre](../../plans/formato-v8-pre1.md), dentro del
[diseño de la V8](../../plans/formato-v8.md) ([ADR 0058](../../adr/0058-formato-v8-experimental.md)). Parte de la
[proyección de la tarea 0](../../plans/formato-v8-pre1.md#proyección) y del
[reparto de bytes de v7](TAMANO_SALIDA_V7_20-11-2026.md).

**Conclusión:** con el mismo binario, `--format-version v8.1-pre` reduce la salida a **−40,6 % (Albuera), −40,5 %
(Majadas) y −34,8 % (Belvedere)** respecto a v7, igual que la proyección (diferencia ≤ 0,01 puntos), de modo que el
criterio V2 (Albuera ≥ 35 % más pequeña) se cumple. El tiempo de conversión no sube (−3,3 % / −4,7 % / +3,6 %), la
carga en el navegador de Belvedere tampoco empeora (mediana del objetivo de 4 px 798 ms frente a 918 ms, con 4
ejecuciones por formato y mucha dispersión) y las tres salidas pasan `viewer_convert verify`. Ningún umbral de parada
de la tarea 8 se activa. Las cifras valen solo para esta revisión, el perfil `release`, estos datasets y este equipo.

## Condiciones

| Dato | Valor |
| --- | --- |
| Binario | `release` de `viewer_convert` compilado desde `7a57c0b` (rama `feat/temporales-fase-a`); el mismo binario escribe v7 y v8.1-pre. Después se añadió solo el ejemplo `distance_quantiles` (no toca el conversor) |
| Equipo | Equipo del propietario (AMD Ryzen AI 9 365, 20 hilos, 30 GiB de RAM); salida en `/home`: `btrfs rw,relatime,seclabel,compress=zstd:1,ssd,discard=async,space_cache=v2` |
| Orden de las conversiones | Una cada vez, sin otra carga: v7 y v8.1-pre de Belvedere, de Majadas y de Albuera, en ese orden por nube (v7 primero). Una sola medición por caso (el tamaño es determinista; el tiempo no se repitió) |
| Orden v7 | `scripts/measure_temp_disk.sh target/release/viewer_convert ENTRADA DIR`, igual que en el [registro de la fase B](TEMPORALES_CLI_FASE_B_20-11-2026.md) (opciones por defecto: v7, 4 workers espaciales) |
| Orden v8.1-pre | La misma orden con `-- --format-version v8.1-pre`; para poder pasar `verify` y leer la cabecera se usó una copia del script sin su último `rm` de `.oct` y `.octattrs` (el resto es idéntico), y se borraron las salidas después |
| Cabecera | `cargo run --release -p viewer_format --example distance_quantiles -- FICHERO.oct` (ejemplo añadido en esta tarea) |
| Navegador | `node examples/web-host/probe-orbit-load.mjs belvedere.oct SALIDA.json --url=http://127.0.0.1:8791/examples/web-host/index.html --attrs --rest-first --max-load-s=120`, con la demo servida por `PORT=8791 scripts/serve-demo.sh --no-build` (wasm reconstruido de esta rama, SHA-256 `1769e27ba46c…`); Chrome headless con adaptador WebGPU de hardware (`amd`, `rdna-3`), 4 px, `mode` = `timing` en las 8 ejecuciones |
| Datos | [`data/formato-v8-pre1-20-11-2026/`](data/formato-v8-pre1-20-11-2026/): `<caso>.phases.txt` (segundo en que empieza cada fase y línea de resultado), `<caso>.tsv` (muestras de `.work`), `<nube>.quantiles.txt`, `.verify.txt`, `.sizes.txt` y `browser_run<N>_<formato>.json` con `browser_summary.txt` |

## Salida y conversión

Entradas: Belvedere (LAZ de 618 390 994 B), Albuera (4 305 484 789 B) y Majadas (1 343 266 470 B). SHA de v7 iguales a
los del [registro de la fase B](TEMPORALES_CLI_FASE_B_20-11-2026.md) en las tres nubes: la rama no cambia los bytes de
v7. Del tamaño de v7 solo se guardó el total que imprime el conversor (el script borra los ficheros) y, para Belvedere,
la pareja de ficheros de la prueba del navegador.

| Nube | Formato | Total (B) | Frente a v7 | Tiempo | RSS máx. | Pico aparente de temporales |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Albuera | v7 | 15 751 915 636 | — | 710,8 s | 256 280 kB | 14 997 684 940 B |
| | **v8.1-pre** | **9 362 079 659** (`.oct` 4 562 037 055 + `.octattrs` 4 800 042 604) | **−40,57 %** | 687,2 s (−3,3 %) | 230 160 kB | 14 992 347 911 B |
| Majadas | v7 | 7 688 544 926 | — | 348,3 s | 243 452 kB | 6 492 825 173 B |
| | **v8.1-pre** | **4 575 835 569** (`.oct` 2 119 903 414 + `.octattrs` 2 455 932 155) | **−40,49 %** | 331,9 s (−4,7 %) | 245 568 kB | 6 478 909 922 B |
| Belvedere | v7 | 1 420 602 171 (`.oct` 530 962 031 + `.octattrs` 889 640 140) | — | 52,7 s | 100 896 kB | 1 490 685 887 B |
| | **v8.1-pre** | **926 000 077** (`.oct` 530 791 954 + `.octattrs` 395 208 123) | **−34,82 %** | 54,6 s (+3,6 %) | 105 004 kB | 1 501 250 976 B |

SHA-256 (`.oct` / `.octattrs`):

| Nube | v7 | v8.1-pre |
| --- | --- | --- |
| Albuera | `5c15df5e…8bb6` / `b76636f4…1076` | `4e0dc764…0530` / `3497d451…57bc` |
| Majadas | `a213eb23…0783` / `0cffd527…a36c` | `f57c913d…31ab` / `a20544bd…ccac` |
| Belvedere | `da206a65…f0c4` / `e2f587ab…2323` | `5bf6ebf0…86ee` / `968d453f…b88f` |

Los hashes completos están en `<caso>.phases.txt`.

- **Dónde está el ahorro:** el `.oct` casi no cambia (Belvedere −0,03 %); el ahorro está en el `.octattrs` (−55,6 % en
  Belvedere), que es donde viven las columnas omitidas y el color de pantalla de las hojas.
- **Frente a la proyección** de la tarea 0 (v7 − columnas LOD omitidas − color de hoja): Albuera 9 363 244 869 B
  proyectados frente a 9 362 079 659 B reales; Majadas 4 576 714 771 frente a 4 575 835 569; Belvedere 926 170 154
  frente a 926 000 077. La proyección acertó a menos de 1,2 MB por nube.
- **Tiempo:** en Albuera y Majadas baja algo y en Belvedere sube 1,9 s. Con una sola ejecución por caso, ±5 % cae
  dentro de la variación entre ejecuciones: Albuera con el mismo código de spools tardó 695,6 s en la
  [fase B](TEMPORALES_CLI_FASE_B_20-11-2026.md) y 710,8 s aquí (+2,2 %).
- **Temporales y RSS:** los temporales no cambian (la fase de spools es anterior a la omisión de columnas, y su pico
  es el mismo ±0,7 %); el RSS cambia menos de 4,1 % salvo Albuera (−10,2 %).
- **Fases (segundo de inicio en el log):** Albuera v7 0 / 91 / 570 / 680 / 709 y v8.1-pre 0 / 92 / 578 / 664 / 687
  (partición / división de nodos / bloques / validación / publicación); la fase de bloques dura 110 s en v7 y 86 s en
  v8.1-pre (Majadas: 79 s y 67 s) porque se escriben menos bytes; el resto en `phases.txt`.

## Cuantiles de distancia y espaciado de referencia

Leídos de la cabecera de cada salida v8.1-pre; unidades del dataset.

| Nube | p1 | p5 | p50 | Espaciado de referencia | p5 / espaciado |
| --- | ---: | ---: | ---: | ---: | ---: |
| Belvedere | 0,169 983 | 0,170 306 | 0,412 436 | 0,25 | 0,68 |
| Majadas | 0,000 976 563 | 0,000 976 563 | 0,002 429 552 | 0,001 953 125 | 0,50 |
| Albuera | 0,004 486 292 | 0,007 874 777 | 0,020 804 171 | 0,015 625 | 0,50 |

- El espaciado de referencia es una potencia de 2 (la mediana de la parada de llenado de las hojas, redondeada), y
  p5 queda entre 0,50 y 0,68 veces ese valor: el tamaño recomendado (p5) es menor que el espaciado en las tres nubes.
- En Majadas p1 = p5 = 2⁻¹⁰, un valor redondo (observado; no se investigó si viene de la cuantización de las
  coordenadas o de puntos repetidos).
- Los cuantiles salen de una muestra con semilla fija en las hojas (borde de hoja sobreestima algo, ver el
  [diseño](../../plans/formato-v8.md#distancia-mínima-entre-puntos-d5)); no se comparó con un cálculo exacto sobre
  todos los puntos.

## Lectura en el navegador (Belvedere)

Cuatro ejecuciones por formato, alternando el orden (v7, v8, v8, v7, v7, v8, v8, v7), cada una con el fichero local
y su `.octattrs` (`--attrs`; sin él v8.1-pre no tendría color) y sin órbita durante la carga (`--rest-first`). Carga del
equipo al empezar (`loadavg` a 1 min): 1,3 a 3,7. `mode` = `timing` en las ocho (adaptador `amd`/`rdna-3`, ningún
`cargo`/`rustc` ni otro navegador en marcha): los tiempos son válidos como medida de esta máquina.

| Medida (ms) | Formato | Ejecuciones | Mediana | Mínimo–máximo |
| --- | --- | --- | ---: | ---: |
| Objetivo de 4 px alcanzado (`openTimings.targetLodMs`) | v7 | 968, 897, 918, 918 | **918** | 897–968 |
| | v8.1-pre | 943, 677, 919, 654 | **798 (−13,1 %)** | 654–943 |
| Primer fotograma grueso (`firstCoarseFrameMs`) | v7 | 108, 103, 88, 90 | 96,5 | 88–108 |
| | v8.1-pre | 116, 100, 108, 94 | 104 | 94–116 |
| Sondeo hasta «objetivo» (`timeToReachedMs`, paso de 250 ms) | v7 | 1352, 1349, 1349, 1352 | 1350,5 | 1349–1352 |
| | v8.1-pre | 1344, 1088, 1363, 1069 | 1216 (−10,0 %) | 1069–1363 |

- **Sin empeoramiento:** el peor caso de v8.1-pre (943 ms) queda por debajo del peor de v7 (968 ms) y la mediana baja;
  el umbral del plan (+10 %) no se acerca. No se afirma una mejora: la serie de v8.1-pre es bimodal (654–677 ms y
  919–943 ms) y con 4 ejecuciones no se puede separar ese salto del ruido; la causa no se investigó.
- Los ficheros eran locales (sin red): no mide el efecto de leer 35 % menos bytes por HTTP Range.
- Las ocho ejecuciones registran los mismos dos errores de consola `404 (File not found)` (también en v7; no se
  identificó el recurso); la carga termina con «target reached». La captura final de cada formato está junto a su JSON
  (`browser_run1_v7.json.png`, `browser_run3_v8.json.png`).

## Lo que no se midió

- Una segunda ejecución de la conversión (el tiempo se midió una vez por caso).
- El tamaño de `.oct` y `.octattrs` de v7 en Albuera y Majadas por separado (solo el total).
- La carga por HTTP Range con la red RED-REF, Majadas y Albuera en el navegador, Firefox, y la fidelidad visual del
  color derivado más allá de la captura final de Belvedere.
- Uso real de disco en btrfs: el delta de `df` no es fiable aquí (ver la fase B); el pico de temporales es el aparente.
- Compresión por fichero (D3, abierta) y la comparación de los cuantiles con un cálculo exacto.
