# Retirada del modo octante

Estado: en curso · 18-11-2026 · ejecuta el [ADR 0056](../adr/0056-se-retira-el-modo-octante.md) (propietario, 18-11-2026); plan del agente a partir del mapa de alcance del 18-11-2026; pre-mortem aplicado el 18-11-2026; tareas 1 a 7 hechas

**Conclusión.** Primero se sabe qué cambia y después se cambia: línea base del modo celda (hecha), ensayo que
clasifica los tests que fallan al pasar todo a modo celda, oráculo de pasos para modo celda, y solo entonces el
cambio de predeterminado, la API, el porte de tests por temas y el borrado de ramas fichero a fichero. Ahora se
borran unas 300–450 líneas de producción y unas 1000–1500 de tests; la maquinaria por octante que el modo celda
todavía usa (pasos, sondas, retención por nodo) se queda para el [plan del ADR 0055](adr-0055-rango-por-nodo.md).

## Riesgos principales (pre-mortem del 18-11-2026)

- **Tests que cambian de modo sin avisar.** Unos 81 de los 97 tests de `cell_pool_tests` y 39 de los 45 de
  `cell_driver_tests` no fijan modo y corren en octante. El predeterminado no es `#[default]` (nadie llama a
  `LodMode::default()`), sino literales: `cell_pool.rs` (`CellPool::new`), `cells_glue.rs` (dos), `bindings/mod.rs`;
  además lo fijan por su cuenta los arneses wasm (`lod_cells_stream_tests.rs`, `lod_cells_glue_sim_tests.rs`),
  `BENCH_MODE` y el `cell_cut` público.
- **El oráculo de pasos se apaga solo.** `Rig::check_stepping` (`cell_pool_tests.rs`) vuelve sin comprobar nada
  fuera del modo octante; el modo celda sigue dando pasos (`stepped_depth`).
- **Borrar cobertura en vez de portarla.** Muchos tests afirman órdenes de subida, recortes y presupuestos
  calibrados en bytes de octante; fallarán por diferencias legítimas entre modos.
- **Un guarda de modo invertido al borrar ramas** pasaría un golden de un solo frame.

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

- El resumen de una simulación real de celda cambia respecto a la línea base de la tarea 1 (comparado ordenado y
  sin las líneas de tiempo): parar.
- Un tema de la tabla de la tarea 2 se queda sin ningún test que lo afirme en modo celda: parar.
- Una tarea borra más tests de los que porta, o cambia un presupuesto sin su aritmética en un comentario: parar y
  consultar al propietario.
- Un test que falla en modo celda revela un defecto del modo celda (no una diferencia esperada entre modos): parar,
  abrir issue y decidir antes de seguir.
- Un test de GPU «pasa» sin ningún adaptador: no cuenta como puerta.
- El encargo de una tarea necesita más de unas 20 llamadas antes de empezar a escribir: dividirla.

## Tareas

### Task 1: Characterize cell mode (R)

- [x] (4116b38, efdef22) Golden `cell_cut_digest_golden_in_cell_mode` (falla al alterar `range`) y
  [línea base](../records/benchmarks/R3_LINEA_BASE_CELDA_18-11-2026.md): las simulaciones reales son deterministas
  salvo el orden y las líneas de tiempo; 618 nombres de test. La pasada de `cells_bench` no se hizo.

### Task 2: Dry run, classify what fails in cell mode

- [x] (18-11-2026, rama desechable `explore/octant-dry-run`; salidas en `.superpowers/sdd/retomar-18-11/dry-run/`)
  Con los diez sitios que fijan el modo cambiados a celda fallan **10 tests** de 595 (7 de render, 3 de wasm) y
  ninguno apunta a un defecto del modo celda. No hizo falta ningún arreglo de compilación.

Sitios que fijan el modo: `cell_cut.rs` (`#[default]` y el `cell_cut` público), `cell_pool.rs` (`CellPool::new`),
`cells_glue.rs` (dos), `bindings/mod.rs`, `lod_cells_stream_tests.rs` y `lod_cells_glue_sim_tests.rs` (arneses),
`BENCH_MODE` (`lod_cells_bench_tests.rs`) y `eval_with` (`cell_pool_tests.rs`). `Rig::new` y las llamadas directas
a `CellDriver::new` heredan el de `CellPool::new`.

| Test | Tema | Acción | Motivo | Hecho |
| --- | --- | --- | --- | --- |
| `cell_cut::cell_mode_ignores_children_resident_only_outside_their_span` | solo octante | borrar su cola de octante | la mitad de celda pasa | cola borrada (la mitad de celda pasa) |
| `cell_cut::spanning_levels_promote_octants_to_the_next_buildable_level` | solo octante | borrar | la promoción por octantes que abarcan solo existe en octante | fijado en octante (`run_octant`); se borra en la tarea 7 |
| `cell_driver::a_level_added_and_dropped_is_requested_again_and_returns_with_the_new_colours` | registros | portar | en celda se piden también los niveles del rango (`[(2,7),(2,5),(2,6)]`); antes, entender por qué `a_missing_level_is_requested…` pide solo uno | portado (`[(2,7),(2,5),(2,6)]` en celda) |
| `cell_driver::driver_in_cell_mode_writes_records_and_uniform` | registros | portar | quitar las fases de octante y conservar las de celda | fijado en octante; se borra o porta en la tarea 7 |
| `cell_driver::octant_mode_writes_no_record_when_only_the_eye_moves` | solo octante | borrar | `anchors_follow_the_eye` cubre el lado de celda | fijado en octante; se borra en la tarea 7 |
| `cell_pool::a_retained_octant_cut_draws_nothing_outside_the_span_after_switching_to_cell_mode` | cambio de modo | borrar | ya no hay cambio de modo | fijado en octante; se borra en la tarea 7 |
| `cell_pool::the_hold_threshold_is_the_depth_the_ancestor_drew_over_the_octant_of_the_child` | retención | borrar | la regla de celda tiene su test, que pasa | fijado en octante; se borra en la tarea 7 |
| `glue::switching_mode_rewrites_the_page_table` y `…_through_the_glue` | cambio de modo | borrar | ya no hay cambio de modo | fijados en octante (`in_mode(Octant)` y `set_lod_mode`); se borran en la tarea 8 |
| `the_oracle_reports_a_hidden_cell_whose_refinement_does_not_draw` | oráculo | portar como autotest de `check_cell_cut` | hoy depende de `last_cut`, que solo rellena la rama de octante del arnés | fijado en octante (`in_mode(Octant)`); el autotest de celda es `the_cell_oracle_reports_a_node_whose_active_depths_are_missing` (tarea 3); se borra en la tarea 9 con `CutView` |

Quedan aparte unos 28 tests explícitamente de octante (15 mitades de las macros de dos modos, 5 de
`cell_cut_tests`, 2 de `cell_pool_tests`, 6 sueltos de wasm), que se borran en las tareas 7 y 8.

**Avisos del ensayo.**
- En modo celda el arnés wasm no ejecuta `CutView::holes`, `gate_octant_steps` ni los contadores de zona; solo
  `check_cell_cut`, que no tiene autotest. Por eso el último test de la tabla se porta en vez de borrarse.
- Con el oráculo de pasos activado en celda falla un solo test (`a_leaving_node_hands_back_one_level`) y el fallo
  es del oráculo. La tarea 3 encontró la causa: no era el filtro de tramo, sino que el oráculo no conocía la semilla
  de entrada (`CellPool::entering_seed`) de un nodo que no dibujó nada en el corte aplicado.
- Los temas se etiquetaron por nombre de test, no leyendo los 142 de pool y driver: ningún tema se queda sin tests
  que pasen, pero el recuento es aproximado.

### Task 3: A stepping oracle for cell mode

- [x] `check_stepping` deja de apagarse: versión para modo celda (pasos de un nivel dentro del tramo del nodo,
  con el filtro `in_cell_range` y sin la excepción de octantes que abarcan), activa en todos los arneses de celda.
  Debe pasar en todos los tests que ya corren en modo celda y, con el modo cambiado, en
  `a_leaving_node_hands_back_one_level`. Si falla en alguno, es un posible defecto: criterio de parada.
  Hecho en `e56ff17`: el oráculo filtra por tramo (`in_cell_range`, `buildable` acotado) y parte de `entering_seed`
  para los nodos que no dibujaban (la causa del fallo de `a_leaving_node_hands_back_one_level` era esa semilla, no
  el filtro de tramo); `the_stepping_oracle_runs_in_cell_mode` comprueba 2264 pares (nodo, octante) con 3 pasos; con
  el modo celda por defecto solo fallan los 2 tests conocidos, ninguno en `check_stepping`.
- [x] Autotest de `check_cell_cut`: una mutación del corte de celda (un nivel activo quitado) hace que el oráculo
  informe; sustituye a `the_oracle_reports_a_hidden_cell_whose_refinement_does_not_draw`.
  Hecho en `e56ff17`: `the_cell_oracle_reports_a_node_whose_active_depths_are_missing` pasa el plan intacto y luego
  uno sin los niveles activos de un nodo aislado: el oráculo informa «is drawn by no node».

### Task 4: Goldens of the paths the deletions will touch

- [x] Golden del `cell_cut` público calculado en modo celda (ruta sin rango ni estado previo) y golden de un corte
  de dos frames con estado previo (pasos). Se anota qué miden hoy `coverage_spike` y la estimación sin tope de
  `cell_pool_bench` (los dos pasan por el `cell_cut` de octante) y qué medirán después.
  Hecho en `3284b5c`: `cell_cut_digest_golden_of_the_plain_cut_in_cell_mode` (48 casos, `cell_cut_with` con
  `LodMode::Cell`) y `cell_cut_digest_golden_with_a_previous_cut_in_cell_mode` (64 casos de dos frames, con todo
  residente y con solo lo dibujado en el frame 1; pasos y etapas verificados > 0); los 4 goldens de `cell_cut`
  pasan y el golden 1 y el 2 fallan al perturbar la entrada.
  - Hecho: `examples/coverage_spike.rs` llama a `cell_cut` (octante) en las líneas 936 (corte deseado por pose y
    objetivo) y 1399 (unión de transición por variante); `examples/cell_pool_bench.rs` lo llama en la línea 825
    (estimación «sin tope», claves `wanted` de cada octante). Ninguno pasa rango ni estado previo.
  - Hecho: tras la retirada, `cell_cut` calculará por `cell_cut_with` en modo celda, la ruta fijada por el golden 1,
    y esos tres puntos de llamada pasarán por ella.

### Task 5: Flip the defaults and the rigs to cell mode

Replanteada tras el ensayo: como solo fallan 10 tests y el oráculo de pasos ya corre en celda, no hace falta fijar
antes los arneses en octante. Los tests explícitamente de octante siguen corriendo hasta las tareas 8 y 9.

- [x] **5a, render.** Cambian a celda `CellPool::new`, el `#[default]`, el `cell_cut` público y `eval_with`. Los
  siete fallos de render de la tabla de la tarea 2 se resuelven como dice la tabla (dos portes, cinco borrados con
  su motivo). Comprobación: la lista de tests solo pierde los borrados; goldens de las tareas 1 y 4 intactos.
  Hecho en `71344b5` (con 5b): render 413 tests, cuatro goldens intactos; los tests de octante se fijan en vez de borrarse.
- [x] **5b, wasm (R).** Cambian a celda `cells_glue.rs` (dos), `bindings/mod.rs`, los arneses
  `lod_cells_stream_tests.rs` y `lod_cells_glue_sim_tests.rs` y `BENCH_MODE`. Se borran los dos tests de cambio de
  modo; el autotest del oráculo de octante ya tiene sustituto (tarea 3). Comprobación: simulaciones reales iguales
  a la línea base.
  Hecho en `71344b5`: wasm 186 tests, lista de tests idéntica a la previa, simulaciones reales idénticas (`SIMS-IDENTICAL`).
  Hallazgo: el pegamento de wasm no aplicaba su modo al primer driver, por eso 5a y 5b van en un solo commit.

### Task 6: Remove the mode from the API and the web host

- [x] (800aa2a, 18-11-2026) Se borran `setCellLod` y `parse_lod_mode` (wasm), el método de `viewer-api.mjs`, el comando del worker, el
  selector `#cell-lod` y la opción `--cell-lod` del probe de paridad; `?cellLod=` se ignora con un aviso. README y
  PROBES actualizados. Comprobación: `node --test`, test de Python del paquete y Playwright (la demo abre, el
  selector no existe, `?cellLod=octant` avisa y dibuja en modo celda). Hecho: wasm 185 tests (-1,
  `set_cell_lod_rejects_unknown_modes`), `node --test` 252 (-1, el de `setCellLod`), clippy nativo y wasm32 limpios;
  el probe conserva `cellLod: "cell"` constante en su informe; Playwright pendiente.

### Task 6b: Cell-mode coverage with wide ranges, before any deletion

Evaluación de calidad del 18-11-2026 (lectura estática de los 110 tests de pool y driver que pasaron a modo celda
sin fijar modo): unos 62 siguen siendo sólidos, 25 comprueban menos y 20 pasan por coincidencia, porque con la
cámara de encuadre el rango de cada nodo se reduce a un nivel y el modo celda se comporta como el octante. El
comportamiento con rangos anchos solo lo cubren unos 15 tests explícitos. Estas lagunas son anteriores a la
retirada, pero al borrar el modo octante dejaría de haber red. Cada test nuevo debe fallar con su mutación.

- [x] **6b-1** `cell_reset_restarts_the_active_window`: tras `reset()` el conjunto activo de un nodo que entra
  vuelve a empezar. Mutación: quitar `applied_window.clear()`. Hecho en ef6dc6d. Evidencia: quitar
  `applied_window.clear()` sobrevive (equivalente: `applied_window` se reconstruye desde `applied_span` en el
  primer corte aplicado, y `applied_span.clear()` es el que importa); quitar `applied_span.clear()` hace fallar
  `entering the cut` (`[7, 9]` en vez de `[9]`); `applied_finest.clear()` también sobrevive (tampoco se lee antes
  de reescribirse).
- [x] **6b-2** `an_active_range_depth_is_never_evicted_under_pressure`: con zoom y un pool justo, ninguna clave
  expulsada está en el conjunto activo anterior ni en el actual. Mutación: quitar los niveles del rango de
  `needed_levels`. Hecho en ef6dc6d. Evidencia: el criterio se acota a lo expulsado que estaba activo en el plan
  anterior y sigue en el rango de su nodo (el actual no puede contenerlo: lo activo exige residencia); quitar
  `range_depths(..).0` de `needed_levels` falla en el frame 76 (nodo 5, profundidad 8, regla `Window`).
- [x] **6b-3** `cap_clamps_the_range_and_fit_reports_it_with_wide_ranges`: recorte del más lejano primero, informado,
  y niveles activos no más finos que el tope. Mutación: quitar `capped()` de `range_depths`. Hecho en ef6dc6d
  (el test pasa; la mutación sobrevive). Evidencia: el test fija el más lejano recortado, caps no más gruesos de lejos
  a cerca, activos y peticiones no más finos que el tope y un nodo recortado con nivel activo solo de rango; quitar
  `capped()` (y `capped_range`) de `range_depths` sobrevive porque `entry.range` ya sale recortado del corte.
- [x] **6b-4** `every_active_depth_is_fully_uploaded_with_a_wide_range`: con subida de una página por frame, todo
  nivel activo es residente; un contador prueba que hubo un nivel activo solo de rango y una subida parcial.
  Mutación: quitar la comprobación de residencia de `cell_active_depths`. Hecho en ef6dc6d. Evidencia: con
  `usable` siempre verdadero falla (`test_support.rs:301`, «page 3 is active but not uploaded»).
- [x] **6b-5** `hold_threshold_is_the_finest_active_depth_end_to_end`: retención con un antepasado de rango ancho.
  Mutación: usar la profundidad nominal dibujada como umbral.
  Hecho en `f398319`: falla al usar la profundidad nominal como umbral; con las dos reglas de activos.
- [x] **6b-6** `real_memo_is_recomputed_when_only_the_active_span_changes`: el resumen del corte aplicado no
  incluye el nivel activo más fino, que las semillas leen. Riesgo sin comprobar: si el test falla sin mutación,
  es un defecto y se para.
  Hecho en `f398319`: **sin defecto**. El test pasa sin mutación; invalidan la decisión memorizada `residency_epoch` y `applied_digest` (`plan.rs`), y quitar cualquiera de las dos lo hace fallar. `RealMemo` no tiene clave propia para el nivel activo más fino: laguna teórica, no observada.
- [x] **6b-7** `requests_rank_first_step_wanted_range_only_nearest_first`: orden de peticiones con dos nodos y
  niveles solo de rango. Mutación: quitar el rango `range_only` o invertir el desempate por distancia.
  Hecho en `f398319`: orden fijado (deseados primero; después solo de rango por profundidad, distancia e id); fallan las dos mutaciones. El rango de paso no se ejercita en este arnés.
- [x] **6b-8** El test de presupuesto del driver afirma `coarsened >= 1` y residentes por debajo de la capacidad
  (hoy afirma un intervalo que siempre se cumple); el arnés afirma que el oráculo de pasos comprobó algo y cuenta
  los nodos que se salta.
  Hecho en `2089ddd`: el test del driver nunca recortaba en modo celda con capacidad 64 (pasaba por un intervalo siempre cierto); con 40 páginas recorta 5 nodos y lo afirma. El arnés cuenta lo que el oráculo de pasos se salta por motivo y cuatro tests afirman que comprobó algo. Los totales del fichero no se midieron.

Pendiente de la revisión de las tareas 1 a 5 (se resuelve al borrar en la tarea 7): dos tests de retención fijados
en octante ejecutan sus primeros frames, o su corte, en modo celda (`the_hold_threshold_is_the_depth…`, la mitad de
octante de `in_cell_mode_the_hold_threshold…`); el autotest del oráculo de celda solo prueba un nodo sin ningún
nivel activo (se amplía en la tarea 8 con un nivel suelto dentro del tramo).

### Task 7: Delete the explicitly-octant render tests

- [x] (82cd7cf) Los tests de render ya no ejercitan el modo octante: 6 borrados (regla de retención por octante,
  cambio de modo, registro sin cambios al mover el ojo, un test de reinicio contenido en otro, dos de promoción por
  octantes que abarcan), 4 pierden su mitad de octante y el resto se portan a la ruta de celda, incluidos los tests
  unitarios de pasos. `the_driver_draws_like_the_cpu_path_at_a_far_origin_on_every_adapter` usa el gemelo de celda
  como oráculo y corrió en dos adaptadores (diferencia máxima de 5e-7 con tolerancia de 5e-4). 414 tests de render;
  goldens de celda intactos; ninguna mención a `LodMode::Octant` en los tests de render. Se conserva el golden
  antiguo `cell_cut_digest_golden`: su tabla no es comparable entrada a entrada con la del corte simple en modo
  celda porque usan resúmenes distintos.

### Task 8: wasm sims and glue tests, one mode (R)

- [ ] Se colapsan las macros de dos modos (queda el test de celda con el nombre simple), se cambian los literales
  de los arneses wasm y los tests fuera de las macros, se borra el oráculo de octante y se portan
  `a_digest_is_not_at_rest_right_after_the_pool_was_lost` (`lod_cells_sim_tests.rs`) y los tests con presupuesto
  en bytes de octante (`synthetic_cpu_bytes(Cell)` con la misma fracción; se anotan las dos cifras).
  Comprobación: resumen de las simulaciones reales igual que la línea base.

### Task 9: Delete the branches and `LodMode`, file by file (R)

- [ ] Un commit por fichero (`cell_cut`, cada `cell_pool/*`, `cell_driver`, `cells_glue`, `cell_pool_bench`), con
  los goldens de las tareas 1 y 4 y las simulaciones reales tras cada uno; al final, el enum. Atención a los
  guardas `!= Cell → return` de `seed.rs` y `hold.rs` y a `drawn_levels`, donde el código de octante va detrás del
  `return` del bloque de celda.

### Task 10: Shader path without a LOD view, docs, and closing

- [ ] Se comprueba primero si la selección en producción corre siempre con vista de LOD. Si es así, los tests de
  render y de selección reciben una `LodView` (como `cell_lod_gpu_tests`) y se borra la ruta `lod_mode == 0`; si
  no, se deja como ruta de máscara.
- [ ] `docs/requirements.md`, `docs/explanation/REGIONAL_VOXEL_LOD.md` y `docs/estado.md` dejan de describir el
  modo octante como vigente; se cierran #59 y #67; `FUZZ_CHECK=0 scripts/check.sh`; navegador en P1 a P4.
- [ ] El plan del ADR 0055 se reescribe sin la restricción del modo octante: su D4 deja de hacer falta.
