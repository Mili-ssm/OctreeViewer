# Retirada del modo octante

Estado: en curso · 18-11-2026 · ejecuta el [ADR 0056](../adr/0056-se-retira-el-modo-octante.md) (propietario, 18-11-2026); plan del agente a partir del mapa de alcance del 18-11-2026; pre-mortem pendiente antes de la tarea 2

**Conclusión.** Ocho tareas, cada una deja la compilación y los tests en verde. Primero se fija una línea base del
modo celda; después se cambia el predeterminado, se quita la API, se pasan los tests a modo celda y, al final, se
borran las ramas. Ahora se borran unas 300–450 líneas de producción y unas 1000–1500 de tests; la maquinaria por
octante que el modo celda todavía usa (pasos, sondas, retención por nodo) se queda para el
[plan del ADR 0055](adr-0055-rango-por-nodo.md).

## Riesgo principal

Unos 81 de los 97 tests de `cell_pool_tests` y 39 de los 45 de `cell_driver_tests` no fijan modo: hoy corren en
modo octante por el predeterminado. Al cambiarlo pasarían a modo celda sin avisar. Por eso la tarea 2 fija los
arneses en octante antes de tocar el predeterminado, y las tareas 4 y 5 los pasan a celda test a test.

## Restricciones

- El modo celda no cambia de comportamiento en ninguna tarea. Puerta: el golden de celda de la tarea 1, los tests
  `*_in_cell_mode`, la partición en GPU, los tests de pose P2 y P4, y las simulaciones reales con el mismo resumen
  que la línea base y `unexcused_drops`, `zone_coarsenings` y `lost_samples` a 0.
- Un test de octante que comprueba algo que sigue importando se **porta** a modo celda; solo se borra si lo que
  comprueba desaparece con el modo. Cada borrado lleva su motivo en el informe de la tarea.
- No se debilita ningún umbral ni excusa de oráculo para pasar a verde.
- No se borra lo que el modo celda usa hoy: `stepped_depth`, `PrevState`, `STEP_WAIT_FRAMES`, `step_waits`,
  `prev_state`, `step_hops`, `finer_drawn`, `eval_refine_hold`, las sondas por octante y la búsqueda con pista,
  `CellCutEntry.{wanted,drawn,stage,step}`, el `busy` por paso del driver, `seed.rs` y la palabra `octants` de las
  páginas (el shader la usa como marca de página viva).
- El mensaje `cellLod` de `build-worker.js` lleva estadísticas, no el modo: no se renombra.
- Puertas de cada tarea: `cargo test -p viewer_render_wgpu -p viewer_wasm --profile fast --offline --lib`, clippy
  nativo y wasm32, `cargo fmt`; las tareas con **R** añaden las simulaciones reales; las de la demo, `node --test`
  y Playwright.

## Criterios de parada

- El resumen de una simulación real de celda cambia respecto a la línea base de la tarea 1: parar.
- Al pasar un arnés a modo celda fallan más de unos 15 tests de un fichero: parar y repartir el fichero en más tareas.
- Un test que falla en modo celda revela un defecto del modo celda (no una diferencia esperada entre modos): parar,
  abrir issue y decidir antes de seguir.
- La ruta del shader sin vista de LOD no se puede quitar sin reescribir los tests de render y de selección: se deja
  (tarea 8 lo decide con medida).

## Tareas

### Task 1: Characterize cell mode (R)

- [x] Golden de `cell_cut_ranged` en `LodMode::Cell` sobre los casos del golden actual (`cell_cut_tests.rs`), y
  registro fechado con el resumen de las simulaciones reales de celda y una pasada de `cells_bench` en modo celda.
  Comprobación: el golden nuevo pasa y falla si se altera una profundidad a mano.
  Hecho en `4116b38` (golden `cell_cut_digest_golden_in_cell_mode`, falla al alterar `range`) y en el registro
  [R3_LINEA_BASE_CELDA_18-11-2026](../records/benchmarks/R3_LINEA_BASE_CELDA_18-11-2026.md); la pasada de
  `cells_bench` no se hizo (solo las simulaciones `real_sim`).

### Task 2: Default flip, with the test rigs pinned to octant

- [ ] Los cuatro predeterminados pasan a celda (`cell_cut.rs`, `cell_pool.rs`, `cells_glue.rs` ×2,
  `bindings/mod.rs`); antes, cada arnés de tests que dependía del predeterminado fija `LodMode::Octant` de forma
  explícita. Comprobación: mismos tests y mismo recuento que antes; `grep` de `LodMode::default()` y de arneses sin
  modo queda vacío.

### Task 3: Remove the mode from the API and the web host

- [ ] Se borran `setCellLod` y `parse_lod_mode` (wasm), el método de `viewer-api.mjs`, el comando del worker, el
  selector `#cell-lod` y la opción `--cell-lod` del probe de paridad; `?cellLod=` se ignora con un aviso. README y
  PROBES actualizados. Comprobación: `node --test`, test de Python del paquete y Playwright (la demo abre, el
  selector no existe, `?cellLod=octant` avisa y dibuja en modo celda).

### Task 4: `cell_pool_tests` run in cell mode

- [ ] El arnés de `cell_pool_tests` pasa a celda. Cada test que falle se porta (si lo que comprueba sigue
  importando) o se borra con su motivo. Los tests solo de octante (`check_stepping` como oráculo de octante, los de
  cambio de modo) se borran.

### Task 5: `cell_driver_tests`, `cell_cut_tests`, `cell_pick_tests` run in cell mode

- [ ] Igual que la tarea 4. Se conservan: los tests unitarios de pasos de `cell_cut_tests` (el modo celda aún usa
  `stepped_depth`), `the_driver_draws_like_the_cpu_path_at_a_far_origin_on_every_adapter` y
  `a_digest_is_not_at_rest_right_after_the_pool_was_lost`, portados. El golden de octante se sustituye por el de
  celda de la tarea 1.

### Task 6: wasm sims and glue tests, one mode (R)

- [ ] Se colapsan las macros `sim_both_modes!`, `real_sim_both_modes!`, `glue_both_modes!`, `real_glue_both_modes!`
  y `both_modes!`: queda el test de celda con el nombre simple. Se borra el oráculo de octante. Se portan los tests
  con presupuesto calibrado en bytes de octante y las mitades de celda de los tests de cambio de modo.
  Comprobación: resumen de las simulaciones reales igual que la línea base.

### Task 7: Delete the branches and `LodMode` (R)

- [ ] Se borran las ramas de octante de `cell_cut`, `cell_pool/*`, `cell_driver`, `cells_glue`, la opción
  `--cell-lod` de `cell_pool_bench`, `records_mode`/`mode_changed` del driver y el enum. `cell_cut` público calcula
  en modo celda. Comprobación: puerta de la tarea 1 y simulaciones reales.

### Task 8: Shader path without a LOD view, docs, and closing

- [ ] Se decide con los tests de render y de selección delante si la ruta `lod_mode == 0` del shader se queda como
  ruta solo de máscara para tests o si la vista de LOD pasa a ser obligatoria; se aplica lo más pequeño.
- [ ] `docs/requirements.md`, `docs/explanation/REGIONAL_VOXEL_LOD.md` y `docs/estado.md` dejan de describir el
  modo octante como vigente; se cierra #59 y #67; `FUZZ_CHECK=0 scripts/check.sh`; navegador en P1 a P4.
- [ ] El plan del ADR 0055 se reescribe sin la restricción del modo octante: su D4 deja de hacer falta.
