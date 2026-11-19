# Limpieza y refactor del proyecto

Estado: completado · 18-11-2026 · encargo del propietario (18-11-2026); rama `refactor/limpieza`, PR [#61](https://github.com/Mili-ssm/OctreeViewer/pull/61) fusionada el 18-11-2026; decisiones del propietario respondidas

**Objetivo.** Dejar el repositorio y el código más fáciles de mantener sin cambiar su comportamiento: quitar
basura, poner al día los documentos vivos y partir los módulos más grandes en submódulos cohesivos. Cada tarea
de código es un movimiento mecánico comprobado con los tests de antes y de después.

**Rama.** `refactor/limpieza` parte de `explore/2b-contiguous-active` (fase 2b más el arreglo del conjunto
activo) e integra `origin/main` (lints de Clippy, PR #58). No parte del experimento E-B1
([replanteamiento](../../plans/replanteamiento-lod-por-celda-18-11-2026.md#experimentos)), para que rechazarlo no obligue a
deshacer la limpieza. Nada se publica ni se fusiona sin el visto bueno del propietario.

## Restricciones

- Sin cambios de comportamiento: ningún test, dorado ni aserción cambia. Si una tarea lo exige, se para y se anota.
- No se toca la lógica de selección de nivel por celda (`cell_cut.rs`, y en `cell_pool.rs` los pasos, la retención
  y el conjunto activo): está pendiente del [ADR 0055](../../adr/0055-la-gpu-decide-el-nivel-la-cpu-la-residencia.md).
  Se puede mover de fichero tal cual.
- Los ADR aceptados siguen vigentes (V5 heredado hasta V8, literales de versión solo en `viewer_format`, núcleo de
  conversión sin I/O).
- Los ejemplos y datos que citan los registros de `docs/records/` no se borran.

## Cómo se ejecuta (pre-mortem aplicado, 18-11-2026)

- **Forma de las divisiones:** el fichero padre se queda (`gpu.rs`) y gana una carpeta hermana (`gpu/`), como ya
  hace `camera.rs` con `camera/`. Así no cambian las rutas de `#[path]`, el historial ni los enlaces de los documentos.
- **Mover, no reescribir:** los elementos se trasladan por rangos de líneas con un script. La comprobación de
  «solo líneas movidas» compara el fichero original con la unión de los nuevos y solo admite diferencias
  estructurales (`mod`, `use`, `impl … {`, llaves, `pub(super)`).
- **Puerta por tarea:** `fmt --check`; clippy con `-D warnings` (por defecto, con todas las features y en wasm32
  para `viewer_wasm` y `viewer_convert_core`); `cargo doc` con `-D warnings`; enlaces e índice de documentos;
  literales de versión; la lista de tests del workspace igual que la línea base; y los tests de los crates tocados.
  Un commit por tarea, solo con la puerta en verde; antes de cada tarea, una etiqueta `pre-<tarea>` a la que volver.
- **Se para y se revierte la tarea** si un test, dorado o aserción tendría que cambiar, si cambia una línea que no
  es estructural, si hay que ampliar la visibilidad de un campo, o tras tres puertas fallidas.
- Las herramientas de la puerta están en `.superpowers/sdd/limpieza/gate/` del checkout principal (ignorado por git).

## Tareas

- [x] **L1 Disco y git.** Borradas las cachés de compilación sin uso (`target/debug`, `o3`, `o3wasm`, `swasm`:
  21 GB; `target/debug` se regenera más pequeña con el siguiente clippy), el worktree de la rama ya fusionada
  `chore/clippy-lints` (8 GB), tres carpetas vacías (`src/`, `256/`, `64000/`) y cuatro ramas locales fusionadas.
  Lo que no es caché queda en «Decisiones del propietario».
- [x] **L2 Documentos vivos.** Estados de planes, página de estado, índice y notas de cambio de R3-Q13 y R3-Q18
  puestos al día (commit efafa3e). Comprobado con `check_docs_links.py` y `check_docs_index.py`.
- [x] **L3 Integrar `origin/main`.** Fusión sin conflictos (93 ficheros).
- [x] **L4 Línea base.** `scripts/check.sh` completo sobre la rama tras la fusión y lista de tests del workspace.
- [x] **R2 `SpatialBounds::max_side()`.** La misma expresión `f64` (lado mayor de una caja) está copiada en varios
  sitios de 4 crates; pasa a un método de `viewer_core` con el cuerpo idéntico. Solo las copias exactas en `f64`
  del código de producción; ni ejemplos ni variantes en `f32`.
- [x] **R3 `ViewUniform`.** El uniforme de GPU de `gpu.rs` se llama igual que la `ViewProjection` pública de la
  cámara; se renombra el uniforme. `camera.rs` no puede aparecer en el diff.
- [x] **R4 WGSL a ficheros.** Los sombreadores que son literales simples pasan tal cual a
  `crates/viewer_render_wgpu/src/shaders/*.wgsl` con `include_str!`. Se queda en Rust el que se construye con
  `format!` y constantes compartidas con el gemelo de CPU (`occlusion_wgsl`). Comprobación extra: el hash de cada
  fuente ensamblada es idéntico antes y después.
- [x] **R5 Partir `gpu.rs`** (2380 líneas con sus tests) en `gpu.rs` más `gpu/{error,init,pipeline,blocks,cells,occlusion,view,pick,stats,render}.rs`,
  en dos pasos: tipos y funciones libres, luego métodos.
- [x] **R6 Partir `viewer_convert_core/src/spatial.rs`** (3035 líneas con sus tests) en `spatial.rs` más
  `spatial/{spool,block,plan,builder,sample,source_points,geometry,memory,partition_tests}.rs`, en dos pasos.
- [x] **R8 Ficheros solo de test con nombre de test** en `viewer_wasm`: `lod_cells_sim.rs`, `lod_cells_glue_sim.rs`
  y `lod_cells_bench.rs` pasan a terminar en `_tests.rs`.
- [x] **R1 Quitar los módulos puente de `viewer_convert`** que solo reexportan `viewer_convert_core` bajo la ruta
  antigua.
- [x] **L5 Cierre.** `scripts/check.sh`: 23 pasos en verde. Simulaciones reales de Belvedere (6 de 6) con los
  mismos contadores y huellas que `feat/cell-lod-2b` (órbita s1: 2166, 0, 2627/17741; órbita s7: 343, 0,
  1857/12267). Navegador: P1–P4 iguales a la compilación previa al refactor (diferencia media de luma ≤ 0,024),
  sin errores de sombreadores. Revisión independiente: sin hallazgos de comportamiento; los 11 `.wgsl` coinciden
  byte a byte con los literales antiguos.

## Resultado de las tareas de código

Cada commit pasó la puerta completa; la lista de tests del workspace (1190) es idéntica a la de la línea base
más el test nuevo de `max_side`.

| Tarea | Commit | Resultado |
| --- | --- | --- |
| L4 | — | `scripts/check.sh`: 23 pasos en verde (829 s), con el dataset real de Belvedere |
| R2 | c93b049 | 13 copias sustituidas en 10 ficheros de 4 crates; un test nuevo |
| R3 | eee2cd1 | 4 ficheros; `camera.rs` y `viewer_wasm` sin tocar |
| R5 | b570f09 | `gpu.rs` de 2380 a 329 líneas más 10 submódulos de 99 a 348; ninguna línea de lógica alterada; 17 elementos y 3 campos pasan a `pub(super)` |
| R6 | ef8d571 | `spatial.rs` de 3031 a 412 líneas más 8 submódulos de 126 a 937; ninguna línea de lógica alterada; los tests siguen en `spatial.rs` |
| R4 | f55b2a4 | 11 ficheros `.wgsl` (unas 740 líneas) en `crates/viewer_render_wgpu/src/shaders/`, copiados byte a byte; son fragmentos que el código concatena, no sombreadores completos |
| R8 | 1cec285 | `lod_cells_sim_tests.rs`, `lod_cells_glue_sim_tests.rs`, `lod_cells_bench_tests.rs` |
| R1 | 4aa00f5 | 8 módulos puente borrados; unas 28 rutas reescritas en 9 ficheros. Quedan dos puentes que además contienen tests (`pipeline/node_job.rs`, `pipeline/point_source.rs`) |

`pub(super)` en campos de una estructura que pasó a un submódulo no amplía nada: el elemento ya era visible en todo
el módulo original.

## Propuesto, sin hacer en esta sesión

| Qué | Por qué se deja |
| --- | --- |
| Partir `cell_pool.rs` (3989 líneas) en `cell_pool/{cache,residency,fit,fallback,hold,active,step,plan}.rs` y portar encima el experimento E-B1 | Es el fichero que más cambia y el que el ADR 0055 reescribiría; tres ramas lo tocan. Se hace con el propietario delante, tras decidir el ADR 0055. Las costuras `active`, `step` y `hold` coinciden con lo que ese ADR borraría |
| `lod_dataset::cells` vive en `lod_cells.rs` por un `#[path]` | Moverlo a una carpeta cambia las rutas de otros cuatro `#[path]`; va junto con la división de `lod_cells.rs` |
| Trocear `CellPool::plan` (967 líneas) y `plan_cell_update` (499) | El ADR 0055 borraría parte de ese código; trocearlo antes es trabajo que se tiraría |
| Trocear `parse_index`, `decode_page` y `ept.rs::convert` (736) | Riesgo medio en lectores de formato; `ept.rs` solo escribe la línea heredada |
| Unificar ayudantes de test duplicados | Bajo valor frente al riesgo de tocar muchos tests |
| Reutilizar búferes por frame en `cell_driver.rs` y `cell_render.rs` | Ganancia sin medir en código que el ADR 0055 puede reescribir |
| Activar los lints pedantes en `viewer_render_wgpu` y `viewer_wasm` | El plan de la fase 2b lo sitúa tras fusionarla |
| Tope de tamaño en `check.sh` (fichero ≤ 1500 líneas, función ≤ 300) | Regla nueva del proyecto: decide el propietario |

## Decisiones del propietario

Respondidas el 18-11-2026: las respuestas y su estado están en
[Cómo retomar](retomar-18-11-2026.md#respuestas-del-propietario-18-11-2026).

| # | Decisión | Datos |
| --- | --- | --- |
| D1 | Borrar las salidas de experimentos guardadas dentro de `target/` | Unos 215 GB: datasets convertidos del 24 y 25-10-2026 (`v7-writer-comparison*`, `v7-cross-corpus-repeat*`, `albuera-current-v5`, `majadas-current-v5`…, unos 190 GB) y perfiles de Firefox y capturas de octubre (unos 5 GB). Se regeneran con horas de conversión desde `data/datasets/`. `target/render-data` (38 GB) lo usan los tests y se queda |
| D2 | Ramas remotas ya fusionadas, ramas de experimento y el stash `WIP on explore/c4-deferred-copy` | El stash apunta a una rama que ya no existe |
| D3 | Borrar 4 ejemplos que nadie cita (`las_header_count.rs`, `cache_result_layout.rs`, `subdivision_bench.rs`, `layout_report.rs`) | Ni scripts ni documentos ni registros los nombran |
| D4 | Planes cerrados: se quedan en `docs/plans/` o pasan a `docs/records/archive/` | `docs/README.md` dice las dos cosas |
| D5 | Cerrar `format-v7-plan.md` y `render-y-carga-escala.md` (siguen como «activo») | V7 está congelado (ADR 0044) y R0–R5 están hechos; quedan dos casillas abiertas por revisar |
| D6 | `artifacts/` (23 MB, octubre) y `.playwright-mcp/` (5 MB) | Ignorados por git; los documentos citan `artifacts/` como salida local |
