# Tests ignorados sobre datos reales en V8 — 21-11-2026

**Estado: medido.** Es la tarea 2b (A6) del [plan del formato V8 congelado](../../plans/formato-v8-congelado.md): los tests
ignorados de `crates/viewer_wasm` (`lod_cells_*_tests.rs`) leen ahora `belvedere-v8.oct` y `majadas-v8.oct`, y se han
ejecutado con tres juegos de datos.

**Conclusión:** los tests ignorados que leen datos reales dan **los mismos valores y el mismo veredicto con los datos v7
antiguos (a), con v7 recién convertido (b) y con V8 (c)**; no hay ningún valor fijado que rehacer. Los ficheros v7 antiguos son
**idénticos byte a byte** a los recién convertidos con el binario actual (mismo SHA-256 en los cuatro), así que el conversor no
cambió su salida v7 desde que se generaron. Dos tests ignorados fallan igual en (a), (b) y (c) porque no leen datos
reales (usan la muestra incorporada); fallaban ya antes de la tarea 2a (ver más abajo).

## Condiciones

| Dato | Valor |
| --- | --- |
| Binario | `viewer_convert` `release` compilado desde `ced31fe` (rama `feat/formato-v8-final`); mismo binario para v7 y V8 |
| Entradas | `data/datasets/belv_2009_digital-aerial_pcd_copc.laz` y `data/datasets/sensoforest/SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz` (las de `MANIFEST.txt`) |
| Orden V8 | `viewer_convert ENTRADA.laz $RENDER_DATA_DIR/<nube>-v8.oct` (V8 por defecto) → ficheros nuevos en `target/render-data` del checkout principal, anotados al final de su `MANIFEST.txt` |
| Orden v7 | `viewer_convert ENTRADA.laz target/render-data-check/<nube>-v7.oct --format-version v7` |
| (a) | `RENDER_DATA_DIR` con enlaces `<nube>-v8.*` → los `*-v7.*` antiguos de `target/render-data`, que a su vez son enlaces a `~/.cache/codex-worktrees/octree-r3-v75/target/render-data/` |
| (b) | `RENDER_DATA_DIR` con enlaces `<nube>-v8.*` → los v7 recién convertidos en `target/render-data-check/` del worktree (borrados después de la prueba) |
| (c) | `RENDER_DATA_DIR=<checkout principal>/target/render-data` (los V8 nuevos) |
| Tests | binario `perf` de `viewer_wasm`, `R3_REQUIRE_REAL_DATA=1`, una invocación por módulo o filtro de [`run.sh`](data/render-data-v8-21-11-2026/run.sh) con `--ignored --nocapture --test-threads=1` ([`run.sh`](data/render-data-v8-21-11-2026/run.sh)); (a), (b) y (c) se lanzaron a la vez, así que los tiempos no valen |
| Equipo | El del propietario (AMD Ryzen AI 9 365, 20 hilos, 30 GiB de RAM) |
| Datos | [`data/render-data-v8-21-11-2026/`](data/render-data-v8-21-11-2026/): registros de conversión, SHA-256, `test-{a,b,c}/` con la salida íntegra de cada test |

## Ficheros

| Fichero | Bytes | SHA-256 (prefijo) |
| --- | --- | --- |
| `belvedere-v7.oct` antiguo = recién convertido | 530 962 031 | `da206a65` |
| `belvedere-v7.octattrs` antiguo = recién convertido | 889 640 140 | `e2f587ab` |
| `majadas-v7.oct` antiguo = recién convertido | 2 120 782 616 | `a213eb23` |
| `majadas-v7.octattrs` antiguo = recién convertido | 5 567 762 310 | `0cffd527` |
| `belvedere-v8.oct` / `.octattrs` | 530 791 954 / 395 208 123 | `8518efc0` / `20d08de0` |
| `majadas-v8.oct` / `.octattrs` | 2 119 903 414 / 2 455 932 155 | `d2d95238` / `285d7575` |

(SHA completos en [`sha.txt`](data/render-data-v8-21-11-2026/sha.txt) y `sha-old-*.txt`.)

## Resultados

| Test ignorado | (a) v7 antiguo | (b) v7 nuevo | (c) V8 | Valores que cambian |
| --- | --- | --- | --- | --- |
| `cells_tests::multi_owner_blocks_in_the_v8_datasets` | ok: Majadas 24 091 bloques, 0 con varios dueños, 810 páginas; Belvedere 3 941, 0, 152 | igual | igual | ninguno |
| `bench::cells_bench` (Majadas, `py`, 4 px) | ok: 323 fotogramas, digest `e259294406045622`, 18 123 subidas | igual | igual | solo tiempos (`build_ms`, `frame_ms`), que varían entre ejecuciones |
| `bench::majadas_at_1px_settles_with_the_browser_block_cache` | ok: asienta tras 480, 932 engrosados por CPU, 62 expulsiones | igual | igual | ninguno |
| `real_belvedere_cells_respect_the_minimum_side` | ok: con mínimo 40, 80 octantes, lado mínimo 64; sin mínimo, 104, 0,25 | igual | igual | ninguno |
| `sim::*` (5 `real_sim_*`/`real_lag_*` y 3 `real_glue_*`, 8 tests) | 8 ok | igual | igual | ninguno |
| `data_bounded_hi_vs_node_range`, `eye_node_residency_at_poses`, `p4_border_levels`, `the_node_lo_…`, `twin_wanted_range_vs_node_range` | ok | igual | igual | ninguno (muestra incorporada, no leen los ficheros) |
| `synthetic_zoom_in_lag` | falla (`ilog2` de 0, `lod_cells_stream_tests.rs`) | igual | igual | ninguno; **falla también en `705cf91^`**, antes de la tarea 2a |
| `the_node_hi_contains_what_the_twin_wants_within_one_level` | falla (7 violaciones) | igual | igual | ninguno; test **en pausa** por su `#[ignore]` y falla también en `705cf91^` |

«Igual» es la salida íntegra sin contar tiempos (`diff` de los registros normalizados). La salida de la simulación incluye
bytes retenidos y digests, así que la igualdad no es solo de veredicto.

## Re-baseline

Ningún valor fijado de los tests ignorados cambia: ninguna aserción se ha tocado ni relajado.

**Tests no ignorados con datos reales (añadido tras la revisión):** `real_streaming_ladder_belvedere`,
`real_regraft_belvedere` y `real_pages_with_drawn_nodes_are_not_evicted` (`lod_cells_stream_tests.rs`) también leen
`belvedere-v8.oct`, pero sin datos imprimen SKIP y pasan, así que en el worktree no se habían ejecutado. Con
`RENDER_DATA_DIR` apuntando a los datos reales, `real_pages_with_drawn_nodes_are_not_evicted` fallaba en V8 y pasaba en v7
(`assert!(refused > 0)`). La causa son las páginas de índice de V8, que ocupan menos porque los bloques LOD no llevan
entradas de directorio para las columnas que D2 retira: la región A usa 41 934 bytes de páginas con V8 frente a 47 398 con
v7, y el presupuesto de 60 000 bytes ya no obliga a rechazar injertos. El presupuesto `REGRAFT_PAGE_BUDGET` se escala en la
misma proporción (60 000 × 41 934 / 47 398 ≈ 53 000). Con él, V8 rechaza 30 injertos, igual que v7, y los tres tests pasan.
El digest del dibujo es el mismo en v7 y en V8 (`1880d5816fd50ea5`). `run-r3-b6-matrix.sh` y los tests leen ahora
`*-v8.oct` de `RENDER_DATA_DIR` (por defecto `target/render-data`).

## Lo que no se midió

- `run-r3-b6-matrix.sh`: solo se ha cambiado la ruta; no se ejecuta (necesita Chrome con WebGPU y `pkg/`).
- `bench::cells_bench` solo con Majadas, pose `py`, 4 px (los valores por defecto); otras poses y Belvedere no.
- El resto de poses de `BENCH_POSE` y `data/datasets/*.oct` no se han reconvertido.
- Tiempos: las tres configuraciones corrieron a la vez; no hay comparación de rendimiento.
- Nada de GPU ni de píxeles.
