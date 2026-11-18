# Limpieza y refactor del proyecto

Estado: en curso · 18-11-2026 · encargo del propietario (18-11-2026); rama local `refactor/limpieza`, sin publicar

**Objetivo.** Dejar el repositorio y el código más fáciles de mantener sin cambiar su comportamiento: quitar
basura, poner al día los documentos vivos y partir los módulos más grandes en submódulos cohesivos. Cada tarea
de código es un movimiento mecánico comprobado con los tests de antes y de después.

**Rama.** `refactor/limpieza` parte de `explore/2b-contiguous-active` (fase 2b más el arreglo del conjunto
activo) e integra `origin/main` (lints de Clippy, PR #58). No parte del experimento E-B1
([replanteamiento](replanteamiento-lod-por-celda-18-11-2026.md#experimentos)), para que rechazarlo no obligue a
deshacer la limpieza. Nada se publica ni se fusiona sin el visto bueno del propietario.

## Restricciones

- Sin cambios de comportamiento: ningún test, dorado ni aserción cambia. Si una tarea lo exige, se para y se anota.
- No se toca la lógica de selección de nivel por celda (`cell_cut.rs`, y en `cell_pool.rs` los pasos, la retención
  y el conjunto activo): está pendiente del [ADR 0055](../adr/0055-la-gpu-decide-el-nivel-la-cpu-la-residencia.md).
  Se puede mover de fichero tal cual.
- Los ADR aceptados siguen vigentes (V5 heredado hasta V8, literales de versión solo en `viewer_format`, núcleo de
  conversión sin I/O).
- Los ejemplos y datos que citan los registros de `docs/records/` no se borran.

## Tareas

Comprobación común de las tareas de código: `cargo clippy --workspace --all-targets --offline --profile fast`
sin avisos nuevos y `cargo test -p <crates tocados> --profile fast --offline` con el mismo número de tests en
verde que la línea base.

- [x] **L1 Disco y git.** Borradas las cachés de compilación sin uso (`target/debug`, `o3`, `o3wasm`, `swasm`:
  21 GB), el worktree de la rama ya fusionada `chore/clippy-lints` (8 GB), tres carpetas vacías (`src/`, `256/`,
  `64000/`) y cuatro ramas locales fusionadas. Lo que no es caché queda en «Decisiones del propietario».
- [x] **L2 Documentos vivos.** Estados de planes, página de estado, índice y notas de cambio de R3-Q13 y R3-Q18
  puestos al día (commit efafa3e). Comprobado con `check_docs_links.py` y `check_docs_index.py`.
- [x] **L3 Integrar `origin/main`.** Fusión sin conflictos (93 ficheros).
- [ ] **L4 Línea base.** `scripts/check.sh` completo sobre la rama tras la fusión.
- [ ] **R2 `SpatialBounds::max_side()`.** La misma expresión (lado mayor de una caja) está copiada en 11 sitios
  de 4 crates; pasa a un método de `viewer_core` con el cuerpo idéntico.
- [ ] **R3 `ViewUniform`.** El uniforme de GPU de `gpu.rs` se llama igual que la `ViewProjection` pública de la
  cámara; se renombra el uniforme.
- [ ] **R4 WGSL a ficheros.** Unas 650 líneas de sombreadores viven en cadenas de Rust; pasan tal cual a
  `crates/viewer_render_wgpu/src/shaders/*.wgsl` con `include_str!`.
- [ ] **R5 Partir `gpu.rs`** (2188 líneas) en `gpu/{mod,error,init,pipeline,blocks,cells,occlusion,view,pick,stats,render}.rs`.
- [ ] **R6 Partir `viewer_convert_core/src/spatial.rs`** (2644 líneas) en `spatial/{mod,spool,block,plan,builder,sample,source_points,geometry,memory}.rs`.
- [ ] **R8 Módulos de `viewer_wasm` con ruta previsible.** `lod_dataset::cells` vive en `lod_cells.rs` por un
  `#[path]`; pasa a `lod_dataset/{mod,cells}.rs`. Los ficheros solo de test `*_sim.rs` y `*_bench.rs` pasan a
  llamarse `*_tests.rs`.
- [ ] **R1 Quitar los módulos puente de `viewer_convert`** que solo reexportan `viewer_convert_core` bajo la ruta
  antigua.
- [ ] **R7 Partir `cell_pool.rs`** (3990 líneas) en `cell_pool/{mod,cache,residency,fit,fallback,hold,active,step,plan}.rs`,
  moviendo los elementos tal cual. Las costuras `active`, `step` y `hold` coinciden con lo que el ADR 0055
  borraría. Va la última.
- [ ] **L5 Cierre.** `scripts/check.sh` completo, simulaciones reales, revisión, y portar el experimento E-B1
  sobre la rama partida si R7 se hizo.

## Propuesto, sin hacer en esta sesión

| Qué | Por qué se deja |
| --- | --- |
| Trocear `CellPool::plan` (967 líneas) y `plan_cell_update` (499) | El ADR 0055 borraría parte de ese código; trocearlo antes es trabajo que se tiraría |
| Trocear `parse_index`, `decode_page` y `ept.rs::convert` (736) | Riesgo medio en lectores de formato; `ept.rs` solo escribe la línea heredada |
| Unificar ayudantes de test duplicados | Bajo valor frente al riesgo de tocar muchos tests |
| Reutilizar búferes por frame en `cell_driver.rs` y `cell_render.rs` | Ganancia sin medir en código que el ADR 0055 puede reescribir |
| Activar los lints pedantes en `viewer_render_wgpu` y `viewer_wasm` | El plan de la fase 2b lo sitúa tras fusionarla |
| Tope de tamaño en `check.sh` (fichero ≤ 1500 líneas, función ≤ 300) | Regla nueva del proyecto: decide el propietario |

## Decisiones del propietario

| # | Decisión | Datos |
| --- | --- | --- |
| D1 | Borrar las salidas de experimentos guardadas dentro de `target/` | Unos 215 GB: datasets convertidos del 24 y 25-10-2026 (`v7-writer-comparison*`, `v7-cross-corpus-repeat*`, `albuera-current-v5`, `majadas-current-v5`…, unos 190 GB) y perfiles de Firefox y capturas de octubre (unos 5 GB). Se regeneran con horas de conversión desde `data/datasets/`. `target/render-data` (38 GB) lo usan los tests y se queda |
| D2 | Ramas remotas ya fusionadas, ramas de experimento y el stash `WIP on explore/c4-deferred-copy` | El stash apunta a una rama que ya no existe |
| D3 | Borrar 4 ejemplos que nadie cita (`las_header_count.rs`, `cache_result_layout.rs`, `subdivision_bench.rs`, `layout_report.rs`) | Ni scripts ni documentos ni registros los nombran |
| D4 | Planes cerrados: se quedan en `docs/plans/` o pasan a `docs/records/archive/` | `docs/README.md` dice las dos cosas |
| D5 | Cerrar `format-v7-plan.md` y `render-y-carga-escala.md` (siguen como «activo») | V7 está congelado (ADR 0044) y R0–R5 están hechos; quedan dos casillas abiertas por revisar |
| D6 | `artifacts/` (23 MB, octubre) y `.playwright-mcp/` (5 MB) | Ignorados por git; los documentos citan `artifacts/` como salida local |
