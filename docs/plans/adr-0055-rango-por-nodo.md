# ADR 0055 en modo celda: rango por nodo y puerta de residencia

Estado: en curso · 18-11-2026 · plan del agente a partir del mapa de arquitectura; pre-mortem aplicado el 18-11-2026; tramo A hecho (tareas 1 a 4), tramo B en pausa hasta decidir cómo se limita `hi`

**Conclusión.** Doce tareas pequeñas en tres tramos, cada tramo entregable por separado: (A) dividir `cell_pool.rs`,
medir y corregir el extremo grueso `lo` del rango; (B) rango por nodo completo y campos a nivel de nodo; (C) puerta
de residencia en lugar de la retención por nodo, y solo entonces borrar lo que queda sin uso. El modo octante no
cambia: está congelado hasta su retirada ([#67](https://github.com/Mili-ssm/OctreeViewer/issues/67)), y por eso
casi toda la maquinaria por octante compartida se queda; aquí solo se borra lo que usa únicamente el modo celda
(unas 400 líneas). **Salida pequeña:** si la medida de la tarea 2 no da una cota ajustada del extremo fino `hi`, se
entrega solo el tramo A y el resto espera a la validación del propietario en el navegador y a la medida de Majadas
([#69](https://github.com/Mili-ssm/OctreeViewer/issues/69)).

Ejecuta los pasos «Fuera de esta sesión» 1 y 2 del
[replanteamiento](replanteamiento-lod-por-celda-18-11-2026.md#tareas), que desarrollan el
[ADR 0055](../adr/0055-la-gpu-decide-el-nivel-la-cpu-la-residencia.md). El paso 3 (medir Majadas) va aparte
([#69](https://github.com/Mili-ssm/OctreeViewer/issues/69)).

## Decisiones (agente, 18-11-2026)

| # | Decisión | Por qué |
| --- | --- | --- |
| D1 | Dividir `cell_pool.rs` antes de cambiar comportamiento | El paso 1 borra unas 230 de sus 4028 líneas; el resto compartido sigue hasta #67. La puerta «solo se mueve» funciona mejor sin cambios en vuelo, y un trabajador no edita con seguridad un fichero de 4000 líneas |
| D2 | El rango `[lo, hi]` es una cota: contiene todo nivel que el gemelo de la GPU pide dentro de nodo ∩ datos ∩ vista | Es lo que exige el ADR 0055 (rango conservador). El `lo` actual no lo cumple en P4 ([registro](../records/benchmarks/R3_FASE2B_BORDE_P4_18-11-2026.md)) |
| D3 | El oráculo de las simulaciones se independiza de `wanted`/`stage` antes de cambiar lo que significan | Si no, el cambio aflojaría el oráculo sin que nadie lo note (ya pasó: [replanteamiento, «Por qué se atascó»](replanteamiento-lod-por-celda-18-11-2026.md#por-qué-se-atascó)) |
| D4 | En modo celda los campos por octante se rellenan a nivel de nodo: `wanted = hi`; `drawn = ancla`, el nivel residente más fino que no pasa de `hi` (nunca el más grueso: anclar ahí engrosó la imagen en el experimento E-B1); sin `step`; `stage` = «falta por residir algún nivel del tramo `lo..hi`» | Deja funcionar el código compartido con el modo octante, pero **no sin tocarlo**: `finer_drawn` lee la sonda por octante, la ruta en solitario de `eval_refine_hold` corta sin rango y la demo da por alcanzado el objetivo con `poolStages === 0` (`viewer-worker.js`). La tarea 7 lista cada lector |
| D5 | La regla de activos (`ActiveRule`) no se toca aquí | La elige el propietario ([#66](https://github.com/Mili-ssm/OctreeViewer/issues/66)); las tareas valen para las dos |
| D6 | La puerta de residencia (tareas 9 y 10) va antes de borrar semillas y `drawn_ceiling` (tarea 11) | `eval_refine_hold` usa las dos: borrarlas antes deja un estado intermedio que congela al hijo o engrosa la imagen (pre-mortem) |
| D7 | Los niveles del tramo se piden de grueso a fino | Con la regla contigua, `hi` no se activa hasta que llegan los intermedios; pedirlo primero retrasa el refinado visible |

## Restricciones

- El modo octante no cambia de comportamiento: sus tests no se editan y siguen en verde.
- No se añaden reglas por octante al modo celda.
- Puertas de todas las tareas: `cargo test -p viewer_render_wgpu -p viewer_wasm --profile fast --offline --lib`,
  clippy nativo y wasm32, y el test de pose P2 `a_node_holding_the_eye_keeps_contiguous_active_depths_and_no_sub_pixel_cells`.
- Las tareas marcadas **R** añaden las simulaciones reales
  (`R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile fast --offline --lib -- --ignored real_sim`) con
  `unexcused_drops`, `zone_coarsenings` y `lost_samples` a 0, y `p4_border_levels`.
- Antes de fusionar: `FUZZ_CHECK=0 scripts/check.sh` y navegador en las poses P1 a P5.

## Criterios de parada

- La medida de la tarea 2 no da una fórmula cerrada de `hi` a un nivel o menos del máximo que pide el gemelo en los
  nodos que contienen el ojo: parar antes de la tarea 5, entregar el tramo A y replantear `hi` con el propietario (#69).
- La cota de D2 no se puede calcular sin recorrer celdas (coste por nodo y frame no acotado): parar y replantear.
- Las simulaciones reales piden más de 2 × los niveles por frame de hoy o agotan el pool donde hoy no: parar.
- Una simulación real necesita cambiar un umbral o una excusa del oráculo para pasar: parar.
- `poolStages > 0` en reposo en una simulación de celdas: parar la tarea en curso.
- Una tarea necesita cambiar un test del modo octante: parar; la costura compartida está mal elegida.
- El encargo de una tarea necesita más de unas 20 llamadas antes de empezar a escribir: dividirla.

## Tareas

### Tramo A: `lo` correcto

### Task 1: Split `cell_pool.rs` (move only)

- [x] (b67708d) `cell_pool.rs` queda como padre (773 líneas) con la carpeta `cell_pool/`: `cache`, `pages`,
  `residency`, `fit`, `fallback`, `hold`, `active`, `step`, `seed` (solo modo celda, borrable) y `plan`.
  Comprobado: `moved.py` solo ve líneas estructurales y dos firmas reajustadas por `cargo fmt`; 403 y 183 tests.

### Task 2: Measure what the twin wants, and why `far` is 11 at P4

- [x] (52e56e5) `p4_border_levels` imprime el diagnóstico de `far`: la esquina que sondea `node_far_depth` está
  1174 px fuera de pantalla, donde la perspectiva plana estira la celda.
- [x] (52e56e5) Medida en P1 a P5 ([registro](../records/benchmarks/R3_RANGO_GEMELO_18-11-2026.md)): la cota
  candidata contiene lo que pide el gemelo en 46 de 46 casos, con holgura 0 en `lo` y de 0 a 1 en `hi`. Las
  simulaciones s1 y s7 no se midieron (otro arnés).
- [x] (52e56e5) Propiedad `the_node_range_contains_what_the_twin_wants_within_one_level`, ignorada: hoy falla con
  11 infracciones.

**Resultado para el plan (18-11-2026).** El tramo A sigue: `lo` tiene fórmula cerrada sin coste. El tramo B queda
**en pausa**: el `hi` que pide la regla junto al ojo lo fija el recorte cercano en puntos sin datos (nivel 18,
576 páginas estimadas frente a 72 en el nodo 14 en P2). Antes de la tarea 5 hay que decidir con qué se limita `hi`
y medirlo: es la pregunta OQ-16 ([#80](https://github.com/Mili-ssm/OctreeViewer/issues/80)).

### Task 3: `node_view_range`, a pure per-node range (not wired)

- [x] (2f628e9) Función pura `node_view_range(vista, rejilla, origen, región, tramo ∩ construible, objetivo px, lado mínimo) -> Option<(lo, hi)>`,
  con `región` = nodo ∩ datos. Tests unitarios: detrás del ojo → `None`, región vacía, lado mínimo, tope.
  Comprobación: la mitad `lo` de la propiedad de la tarea 2 pasa sobre esta función (la propiedad se parte en dos
  tests: `lo`, que la tarea 4 activa, y `hi`, que sigue ignorado). La función calcula también `hi_c`, sin usarlo
  todavía.
  Comprobado: 6 tests unitarios en `cell_cut_tests.rs`; la propiedad `lo` pasa sobre la función en P1 a P5 (ignorada, 175 s;
  variante de solo P4 sin ignorar).

### Task 4: Cell mode takes `lo` from `node_view_range` (R)

- [x] (2f628e9) `refresh_depths` y `cell_cut_ranged` toman `lo` de la función nueva en modo celda; `node_far_depth` deja de
  usarse ahí. `hi` no cambia en esta tarea.
  Test que falla antes: en P4 el nodo 14 tiene `range.lo <= 10` y ningún par visible difiere en más de un nivel.
  Comprobado: P4 nodo 14 pasa de 11..13 a 10..13 y los 3358 pares visibles tienen diferencia 0; `real_sim` 6 de 6 con
  contadores del oráculo a 0; `node_far_depth` se borra.

**Tramo A cerrado (18-11-2026).** Tras `0e63030` (el cableado pide solo `lo`; test de propiedad de 800 poses
recuperado): 409 y 185 tests, simulaciones reales 6 de 6 con los contadores a cero y el mismo resumen del corte, y
en P4 el nodo 14 pasa de rango 11–13 a 10–13 con 3358 pares visibles sin diferencia de nivel. En el navegador la
línea recta de P4 desaparece; queda la transición normal entre dos niveles (cubos de 4 a 8 px, ADR 0054), las
etapas quedan a 0 en reposo y no hay errores de consola. Sin medir: el coste por frame de la búsqueda de `lo` con
la cámara en movimiento, y el recorte por el plano lejano (hoy `lo` puede quedar algo más grueso de lo necesario).

### Tramo B: rango por nodo completo

### Task 5: Cell mode takes `hi` from `node_view_range` (R)

- [ ] Solo si la tarea 2 dio una cota ajustada. `behind_*` deja de usarse en modo celda.
  Test que falla antes: el test de pose P2 sigue pasando sin la regla de octantes detrás del ojo.
  Comprobación: páginas residentes y peticiones por frame de las simulaciones reales frente a hoy.

### Task 6: The sim oracle stops reading `wanted` and `stage`

- [ ] El oráculo de celdas (`lod_cells_stream_tests.rs`) usa como tope el tramo construible. La excusa es
  específica: el nivel que falta está entre el dibujado y el pedido, y el `fit` lo limitó o lo expulsó en ese
  frame. `stage_excuses`, que comparte el oráculo de octantes, no se toca.
  Comprobación: una mutación que deja de proteger los niveles aplicados da `unexcused_drops > 0`.

### Task 7: Node-level `wanted`/`drawn`/`stage` in cell mode (R)

- [ ] `drawn_levels` y `cell_pool` rellenan los campos según D4. El encargo lista cada lector por octante y qué
  hace con él: `finer_drawn`, la ruta en solitario de `eval_refine_hold`, el tope de `fit`, `range_only`,
  `needed_levels`, `lost_zones`, `stages`, el resumen del corte y el `busy` del driver. No se borra nada todavía.
  Test que falla antes: `cell_mode_keeps_the_stepped_depth_active` reescrito como «en modo celda no hay paso».
  Puerta añadida: `stages == 0` en reposo en una simulación de celdas; P2 y P4 con las dos reglas de activos y un
  caso de alejamiento con `Window`.

### Task 8: Requests from coarse to fine, and no steps in `plan` and `fit` (R)

- [ ] El orden de peticiones recorre `lo..hi` de grueso a fino (D7); `plan` no usa `steps`/`step_waits` en modo
  celda; `fit` limita desde `hi`. Test que falla antes: un test de pool donde el rango cabe bajo el tope sin clave
  de paso. Comprobación: frames hasta completar en s1 y s7, `level_requests` por frame y `pool_coarsened`, frente a hoy.

### Tramo C: puerta de residencia

### Task 9: `run_covers`, a pure residency gate

- [ ] `run_covers(tramo del hijo, nivel más fino del antepasado)`: verdadero cuando el tramo del hijo alcanza ese
  nivel más uno (el nombre evita `covers_in` y `CoverRule::cover`, que ya existen en `fallback.rs`). Tests unitarios.

### Task 10: Cell mode uses the residency gate instead of the per-node hold (R)

- [ ] `eval_hold`/`add_d5_gate` usan `run_covers` en modo celda en lugar de `eval_refine_hold` (R3-Q21).
  Test que falla antes: en un borde, un hijo no tapa a su antepasado antes de que su tramo sea contiguo.

### Task 11: Delete what cell mode no longer uses (R)

- [ ] Se borran `cell_pool/seed.rs` entero, la parte de celda de `drawn_ceiling`, las ramas de sonda de celda,
  `behind_*` y `node_far_depth`, con sus tests. Comprobación: mismas cifras de las simulaciones reales que tras la
  tarea 10.

### Task 12: Whole-change verification

- [ ] `FUZZ_CHECK=0 scripts/check.sh`; navegador en P1 a P5 y zoom en movimiento, con «objetivo alcanzado» en
  reposo; registro fechado con los contadores de las simulaciones antes y después; `docs/reference/` y el ADR 0055
  enlazados donde cambie el contrato.

## Solo tras retirar el modo octante (#67)

Borrar `stepped_depth`, `STEP_WAIT_FRAMES`, `step_waits`, `prev_state`, `step_hops`, `finer_drawn`,
`eval_refine_hold`, las sondas por octante y la búsqueda con pista; los campos `CellCutEntry.{wanted,drawn,stage,step}`
(públicos en `lib.rs`); el `busy` por paso del driver y las ramas `LodMode`.

## Riesgos y su comprobación

Del pre-mortem del 18-11-2026; cada uno tiene su enmienda en las tareas.

| Riesgo | Comprobación más barata | Tarea |
| --- | --- | --- |
| `hi` se va al último nivel en los nodos que contienen el ojo: el gemelo da `+inf` a un cubo con una esquina detrás del plano del ojo | Medida de la tarea 2 antes de diseñar la 3 | 2, 5 |
| La retención congela al hijo o engrosa la imagen si se borran las semillas antes de sustituirla | Orden D6; simulaciones reales en cada tarea R | 10, 11 |
| `poolStages` no llega a 0 en reposo y la demo nunca da el objetivo por alcanzado | Aserción `stages == 0` en reposo | 7 |
| El oráculo se excusa a sí mismo con «el nivel pedido no es residente» | Excusa específica y mutación por expulsión | 6 |
| `hi` se pide antes que los niveles intermedios y el refinado visible se retrasa | Frames hasta completar en s1 y s7 | 8 |
| Quedan lectores por octante sin adaptar | Lista de lectores en el encargo de la tarea 7 | 7 |
| La regla `Window` abre un hueco al separar el ancla de la ventana | P2 y P4 con las dos reglas y un alejamiento | 7 |
| Conflictos al rebasar la pila de PR sobre la división | `git log main..HEAD -- crates/viewer_render_wgpu/src/cell_pool.rs` antes de cada rebase; un arreglo de revisión de la #74 se aplica en el submódulo correspondiente | 1 |
| La propiedad de la tarea 2 falla de forma intermitente por f32 frente a f64 | Tolerancia en px | 2 |
