# Referencia del CLI para el conversor web paralelo (29-10-2026)

Registro fechado e inmutable (hito M0, etapa 1 del conversor web paralelo). Fija los SHA de salida del CLI nativo, contra los que se comprobará la paridad byte a byte del navegador (R1), y los tiempos de Belvedere para la no regresión nativa (R7). Datos crudos: [JSON](data/web-converter-baseline-2026-10-29.json). Evidencia cruda (no versionada): `target/m0/` (logs y ficheros de SHA citados); es efímera (`cargo clean` la borra), así que la evidencia duradera es este registro y el JSON.

## Entorno

AMD Ryzen AI 9 365 (20 hilos), 30 GB RAM, NVMe local, Linux 7.2.11-200.fc44; `rustc 1.101.0-nightly (41089d07a 2026-10-27)`; perfil `perf`; commit `74ef00f` (árbol limpio; incluye el parche LZ4 de wasm32 y los contadores de spools). **Caché del SO caliente, sin purga.** Máquina sin otras cargas pesadas durante las mediciones. Espacio libre previo: 348 GB (Majadas necesita ≥ 40 GB).

## Comandos

```sh
cargo build --profile perf -p viewer_convert --bins --examples
# SHA (CLI, opciones por defecto salvo indicación); tiempos/RSS con /usr/bin/time -v
target/perf/viewer_convert INPUT OUT.oct [--spatial-workers 1|8] [--spatial-divisions 8]
# Rondas Belvedere (5 seguidas) y perfiles
target/perf/examples/real_dataset_bench convert INPUT OUT.oct 4
target/perf/examples/real_dataset_bench convert-profile INPUT OUT.oct 4 [8]
sha256sum OUT.oct OUT.octattrs
```

Entradas: `data/samples/fs_e430_n4008_18.las`; `autzen.copc.laz` (de `las-0.11.2/tests/data`); `data/datasets/belv_2009_digital-aerial_pcd_copc.laz`; `data/datasets/sensoforest/SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz`.

**Opciones del bench.** `real_dataset_bench` usa 1 worker por defecto y el CLI `min(4, hilos disponibles)` = 4 aquí; por eso el bench se lanza con `4` explícito. El resto coincide (V5, 2 divisiones, 262144 puntos/bloque, error 0,01). Comparación bench frente a CLI (lo realmente calculado el 29-10-2026): Belvedere, primera ejecución del bench, SHA-256 completos de `.oct` y `.octattrs` iguales al CLI; las 5 rondas cronometradas, solo los primeros 12 hex de ambos ficheros (iguales); `convert-profile` de Belvedere y de divisiones 8, solo los primeros 20 hex del `.oct` (iguales; `.octattrs` no comparado); Majadas, SHA-256 completos de `convert-profile` y CLI iguales. Los valores completos de las rondas no se conservaron.

## SHA de referencia (V5, 4 workers por defecto)

| Caso | `.oct` SHA-256 | `.octattrs` SHA-256 |
| --- | --- | --- |
| LAS pequeño | `413858a908b4794b…` | `964c89effe7a94ff…` |
| COPC pequeño (autzen) | `5966be999f77397f…` | `3be1310bcf3b3488…` |
| Belvedere (defecto) | `6ca0f86d376c79d0…` | `0afa9c6041a6ae3f…` |
| Belvedere, 1 worker | idéntico al defecto | idéntico al defecto |
| Belvedere, 8 workers | idéntico al defecto | idéntico al defecto |
| Belvedere, divisiones 8 | `fb90a490f9884239…` | `b5124e84f7c4e12a…` |
| Majadas | `742262867c844bb4…` | `d45cd5fd70816d24…` |

SHA completos en el [JSON](data/web-converter-baseline-2026-10-29.json). Belvedere con 1, 4 y 8 workers coincide: confirma a escala real la independencia del número de workers ([plan de la etapa 1](../archive/conversor-web-paralelo-etapa1.md), Tarea 1). Tamaños Belvedere: 1.591.650.185 B (pareja); Majadas: 7.533.781.717 B. El COPC pequeño es un fichero de prueba diminuto (salida de 2,4 kB `.oct`), útil solo como control de forma.

## Tiempos Belvedere (`convert`, 4 workers, 5 rondas seguidas; mediana; mín–máx)

| Métrica | Mediana | Mín–máx |
| --- | --- | --- |
| Total (`conversion_elapsed_ms`, ms) | 26.269 | 25.401–26.413 |
| `PlanningSpatialIndex` (ms) | 10.679 | 10.547–10.814 |
| `BuildingSpatialHierarchy` (ms) | 8.040 | 7.647–8.360 |
| `ReadingAndWritingBlocks` (ms) | 6.123 | 5.912–6.224 |
| `ValidatingOutput` (ms) | 1.250 | 1.235–1.312 |
| Pared `/usr/bin/time` (s) | 26,29 | 25,42–26,43 |
| RSS pico (kB) | 276.076 | 270.616–291.392 |

El modo `convert` no imprime las ventanas `internal_*` (solo `convert-profile`); las fases por etapa (`stage_*`) sí. Los SHA de las rondas se comprobaron solo por prefijo de 12 hex (ver «Opciones del bench»).

**Referencia R7:** `real_dataset_bench convert`, total (`conversion_elapsed_ms`), 4 workers, perfil `perf`, mediana de 5 rondas = 26.269 ms (primera fila de la tabla).

## Perfiles (una ejecución cada uno; contadores de spool para modelar los arenas OPFS)

| Caso | Total (ms) | `internal_spatial_planning` (s) | `block_stream` (s) | Validación (s) | Spool escrito (B) | Pico spool vivo (B) | RSS (kB) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Belvedere | 26.324 | 19,23 | 5,19 | 1,22 | 16.165.156.750 | 6.126.109.459 | 273.896 |
| Belvedere, divisiones 8 | 21.682 | 14,08 | 5,53 | 1,25 | 6.660.894.157 | 3.535.907.518 | 184.504 |
| Majadas | 220.782 | 180,70 | 32,55 | 6,72 | 134.724.002.211 | 20.619.710.640 | 326.760 |

Spool por hijo de la raíz (`root_child_spool_bytes_written`, tal como lo imprime el bench: 4 entradas en Belvedere, 81 en divisiones 8, 7 en Majadas, algunas a 0), fases del spool y demás contadores: en el JSON. RSS de los CLI en las ejecuciones de SHA: Belvedere 286 MB (4 workers), 95 MB (1), 492 MB (8), 194 MB (divisiones 8); Majadas 304 MB. Pared CLI Belvedere: 28,6 s (4), 44,2 s (1), 26,8 s (8), 21,3 s (div. 8); Majadas 221,96 s.

## Límites

- Una sola ejecución por celda en los perfiles, CLI y Majadas; solo Belvedere tiene 5 rondas.
- Las cifras valen para esta revisión, perfil `perf` y este equipo; no trasladarlas (en especial, no comparar con registros anteriores medidos con 1 worker).
- Los pareados con 1 y 8 workers solo comprueban SHA; su pared se cita como orientativa (una ejecución).
