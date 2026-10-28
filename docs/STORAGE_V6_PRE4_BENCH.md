# v6.4-pre: columnas independientes y topología compacta — 14-10-2026

Esta entrega implementa una revisión optativa con dos candidatos físicos exactos:
**planos de bytes por columna antes de LZ4** y **máscaras de ocupación internas con hojas y
prefijos implícitos**. Mantiene los candidatos pre3 y solo reemplaza el ganador cuando ocupa
menos bytes almacenados. V5 sigue predeterminado; no se publica v6 final.
[Especificación completa](../crates/viewer_format/FORMAT_V6_PRE4.md).

## Decisión y alcance

El [experimento conjunto columnar](NODE_ATTRIBUTE_SUMMARIES.md) ahorró muchos bytes de
originales Belvedere frente a raw/LZ4, pero sus IDs compartidos penalizaron consultas de
clasificación y sus resúmenes internos casi no se deduplicaron. Esta entrega conserva la
independencia de las columnas y no serializa resúmenes ni diccionarios conjuntos.

La transposición es una permutación de bytes; no cambia precisión, igualdad, valores ni
orden lógico. La topología compacta elimina datos derivados, sin eliminar puntos o atributos.
Cada bloque/columna sigue teniendo su propio payload, CRC y acceso selectivo. Los límites de
buffers no constituyen un límite conjunto de RSS.

## Protocolo y evidencia

[Comparación de parejas válidas](benchmarks/storage-v6-pre4-comparison-2026-10-14.json),
[atribución de payloads/codecs](benchmarks/storage-v6-pre4-payloads-2026-10-14.json) y
[WASM/OPFS/Blob e interoperabilidad](benchmarks/storage-v6-pre4-browser-2026-10-14.json).

Se reutilizan corpus y opciones del informe pre3: Outer Banks e430, Bunny parcial, sintética
agrupada y Belvedere completo. Tres rondas por muestra, alternando pre3→pre4, pre4→pre3,
pre3→pre4. Cada conversión se sigue de una apertura de jerarquía, reconstrucción residente y
BFS progresiva, en procesos independientes. Los hashes de cada revisión son estables y los
checksums lógicos residente/BFS deben coincidir entre todas las rondas y revisiones.

Cargo `perf`, Linux x86_64, Ryzen AI 9 365, NVMe local Btrfs zstd:1; sin purgar caché. Un
trabajador, división 2, bloques de 10.000 puntos en muestras pequeñas y 262.144 en Belvedere.
Lecturas tras conversión, validación y hashes. Longitudes lógicas, no asignación física tras
compresión del filesystem. Los tiempos de conversión incluyen su pipeline completo; `load_ms`
excluye la verificación posterior de checksums. Tres muestras no prueban una mejora general.

`--measure-rss` usa `/usr/bin/time` para el máximo RSS del proceso nativo de cada conversión/
lectura. Es una medida por proceso, no la suma simultánea de procesos ni memoria JS/GPU. Los
logs individuales y sus comandos se conservan. No confundir RSS con estimaciones de caché
o buffers que también emite el benchmark.

Los reportes y logs del runner serial se conservan en `target/storage-pre4-20261014/`; sus
parejas `.oct`/`.octattrs` generadas se purgaron el 25-10-2026 para liberar espacio y pueden
regenerarse desde la fuente documentada.
El JSON compacto enlaza los reportes con hashes, evitando duplicar miles de líneas de logs.
El nuevo `storage_payload_stats` atribuye bytes/codecs leyendo únicamente la metadata.

## Resultados completos

Se cierran **24 conversiones y 72 procesos de lectura**, con hashes estables por muestra/
revisión y checksums iguales entre revisiones en las tres rondas.

| Muestra | Pareja pre3 (B) | Pareja pre4 (B) | Reducción | Conversión mediana pre3/pre4 (ms) |
|---|---:|---:|---:|---:|
| Outer Banks e430 | 1.905.318 | 1.591.099 | 16,49 % | 152,426 / 136,015 |
| Bunny parcial | 207.320 | 207.320 | 0 % | 42,797 / 48,897 |
| Sintética agrupada | 84.319 | 44.041 | 47,77 % | 14,271 / 14,034 |
| Belvedere completo | 1.419.356.066 | 1.404.306.523 | **1,06 %** | 70.374,119 / 69.878,673 |

Belvedere conserva 81.682.329 puntos fuente, 81.706.211 filas con LOD y 1.343 bloques. Su
`.oct` pasa de **980.174.240 a 979.686.544 B** (−487.696 B, 0,0498 %) y `.octattrs` de
**439.181.826 a 424.619.979 B** (−14.561.847 B, 3,32 %). El ahorro total es 15.049.543 B:
medido en parejas válidas, no extrapolado desde los prototipos de diccionario conjunto.

La geometría compacta gana en **212 bloques** (pre3 usaba ocupación en 80); 260 mantienen
deltas y 871 raw. BytePlanes gana en **558 columnas RGB16** y un payload pequeño RGB8.
Clasificación sigue usando 1.343 payloads constantes de **un byte**, exactamente igual en
ambas revisiones: una consulta de clasificación no lee planos RGB ni IDs compartidos.
Outer Banks reduce `.octattrs` de 797.855 a 483.636 B, sin cambiar la longitud de `.oct`.
En Bunny ganan todos los fallbacks anteriores y no hay ahorro; construir candidatos cuesta CPU.

| Belvedere / operación | Pre3 mediana [mín., máx.] | Pre4 mediana [mín., máx.] |
|---|---:|---:|
| Conversión completa | 70,374 [68,633; 71,609] s | 69,879 [69,505; 71,232] s |
| Reconstrucción residente `load_ms` | 9,602 [9,443; 9,616] s | 9,203 [9,202; 9,377] s |
| BFS progresiva `load_ms` | 9,418 [8,758; 9,566] s | 9,000 [8,756; 9,240] s |

Las medianas de lectura bajan 4,16 % / 4,44 % en esta serie caliente. La mediana de conversión
baja 0,70 %, con rangos solapados: no demuestra una aceleración general. Las muestras pequeñas
también tienen dispersión y algún coste observado sin ahorro de bytes. La decisión de esta
entrega prioriza el ahorro exacto con fallback, manteniendo pre4 optativo y publicando sus costes.

| Belvedere / máximo RSS del proceso, mediana de tres ejecuciones | Pre3 | Pre4 |
|---|---:|---:|
| Conversión | 106,42 MiB | 107,34 MiB |
| Reconstrucción residente | 3.285,80 MiB | 3.286,18 MiB |
| BFS progresiva | 19,03 MiB | 20,30 MiB |

Son máximos de procesos nativos; no justifican afirmar ahorro de memoria. La residente retiene
todos los bloques, mientras BFS libera cada bloque. No miden el pico de caché/escena/renderer
del navegador ni GPU. Checksums Belvedere: residente `36909ff142199a4e`, BFS `0737ca650c6081b4`.

Menos bytes pueden ayudar cuando domina transferencia, pero no se han medido HDD ni blobs
remotos. El modelo ideal `tiempo_de_transferencia_ahorrado = bytes_ahorrados / caudal_efectivo`
debe añadir CPU, seeks/solicitudes, latencia y caché para predecir tiempo total. Esta revisión
conserva la granularidad de payloads: no demuestra que reduzca el número de seeks o solicitudes.

## WASM y navegador

Playwright MCP con HeadlessChrome 155 convierte una LAS sintética de 4.096 puntos con GPS time,
vía encoder WASM y OPFS. Comprueba apertura Blob sin/con acompañante y rechazo de provisional,
pre5 y acompañante incorrecto. La misma fuente se convierte con pre3 y pre4:

| Versión | `.oct` (B) | `.octattrs` (B) | Filas con LOD | Checksum residente nativo |
|---|---:|---:|---:|---|
| pre3 | 56.376 | 65.636 | 4.160 | `ad776b7ebf478c7a` |
| pre4 | 54.060 | 29.261 | 4.160 | `ad776b7ebf478c7a` |

Pre4 elige 8 bloques de geometría compacta, 48 payloads BytePlanes y 96 constantes. Las parejas
escritas por el navegador se reconstruyen completas con el lector nativo y coinciden con pre3.
Esto prueba encoding WASM, metadata Blob e interoperabilidad con decoding nativo; **no** prueba
decodificación de payloads/render/presentación GPU en el navegador. La consola solo registró
el 404 de `favicon.ico` del listado local, sin errores del conversor/probe.
Se regeneran bindings y paquete local con la instantánea validada.

## Dependencias y validación

El workspace principal tenía cambios previos en `Cargo.toml`, `Cargo.lock` y el manifiesto del
renderer (wgpu 24→30/glam 0,29→0,34). Se preservan. `cargo check --workspace` con ese estado
falla por 39 incompatibilidades del renderer con wgpu 30, entre ellas `SurfaceError`, `Maintain`
y campos de pipelines; esta entrega no modifica ese renderer ni atribuye esos fallos a pre4.

La validación completa y los binarios de la comparación usan `/tmp/octree-v6-pre4-20261014`,
checkout de HEAD con sus manifiestos/lock y las mismas fuentes de la feature. El reporte registra
binarios, fuentes y dependencias de esa instantánea. Las bibliotecas de almacenamiento también
se comprueban directamente con las dependencias actuales; los resultados globales corresponden
a las dependencias de HEAD y no certifican la migración ajena.

Pasan 350 tests nativos `--workspace --all-targets` (3 ignorados: dos GPU y un microbenchmark),
20 tests Python de benchmarks, 28 tests Node del host y 11 tests Python del empaquetado. Hay
pruebas específicas de 33 particiones, duplicados/extremos, máscara/filas/padding corruptos,
bits de atributos, lectura selectiva y rechazo de revisiones/tags/acompañantes incorrectos.
El runner RSS prueba éxito y fallo de operaciones sin alterar métricas de stdout. Ruff no está
instalado; sintaxis/imports/CLI y tests Python se verifican con el intérprete disponible.
Clippy pasa con `-D warnings` en el workspace nativo aislado y los paquetes compatibles con
WASM (`viewer_wasm`, renderer, format y core); la CLI nativa `viewer_convert` no se comprueba
como paquete WASM porque depende de `ctrlc`. En el workspace principal pasan Clippy de
core/format/convert, comprobación WASM de core/format, fmt y `git diff --check`.

## Uso, reproducción y siguientes decisiones

```sh
cargo run --release -p viewer_convert -- INPUT.laz OUTPUT.oct --format-version v6.4-pre
python3 crates/viewer_convert/benchmarks/compare_storage_revisions.py \
  docs/benchmarks/storage-v6-pre3-comparison-2026-10-14.json \
  target/pre4-new-suite target/pre4-new-report.json \
  --convert-binary target/perf/examples/real_dataset_bench \
  --read-binary target/perf/examples/storage_bench
cargo run --offline --profile perf -p viewer_format --example storage_payload_stats -- OUTPUT.oct
```

En el estado actual de dependencias, reproducir los checks globales/WASM desde la instantánea
de HEAD indicada o resolver la migración de wgpu por separado; no sustituir silenciosamente
los manifiestos del workspace principal.

Pendientes: lecturas XYZ/atributos por separado con latencia y caché controladas; modelos
parametrizados y mediciones reales de HDD/remoto, incluyendo seeks/solicitudes, CPU y volumen;
candidatos para planos de coordenadas/índice exterior; bitpacking por
rango y grupos RGB exactos sin acoplar campos de baja cardinalidad; selector de candidatos más
barato. Cualquier mejora debe cerrar bytes, CPU y memoria antes de avanzar otra revisión.

Ampliación completada con [Majadas TLS y Albuera UAV](SENSOFOREST_V6_BENCH.md): fuentes
completas, cuatro conversiones (N=1 por caso) y 48 lecturas, incluyendo XYZ y clasificación.
Su protocolo y resultados se documentan por separado de la serie anterior.
