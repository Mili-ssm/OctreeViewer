# ADR 0055 en modo celda: rango por nodo y puerta de residencia

Estado: pendiente · 18-11-2026 · plan del agente a partir del mapa de arquitectura del 18-11-2026; pre-mortem pendiente antes de la tarea 1

**Conclusión.** Diez tareas pequeñas: dividir `cell_pool.rs`, un diagnóstico, cinco para que el modo celda tome su
rango de un cálculo por nodo en vez de las sondas por octante, y dos para sustituir la retención por nodo por una
puerta de residencia; después, verificación de conjunto. El modo octante no cambia: está congelado hasta su
retirada ([#67](https://github.com/Mili-ssm/OctreeViewer/issues/67)), y por eso casi toda la maquinaria por
octante compartida se queda; aquí solo se borra lo que usa únicamente el modo celda (unas 400 líneas).

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
| D4 | En modo celda los campos por octante se rellenan a nivel de nodo (`wanted = hi`, `drawn = ancla`, sin `step`) | Así `fit`, peticiones, retención y zonas perdidas, compartidos con el modo octante, siguen funcionando sin tocarlos |
| D5 | La regla de activos (`ActiveRule`) no se toca aquí | La elige el propietario ([#66](https://github.com/Mili-ssm/OctreeViewer/issues/66)); las tareas valen para las dos |

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

- La cota de D2 no se puede calcular sin recorrer celdas (coste por nodo y frame no acotado): parar y replantear.
- Tras la tarea 4 las simulaciones reales piden más de 2 × los niveles por frame de hoy o agotan el pool donde hoy
  no: parar; el rango es demasiado ancho o falta un orden de peticiones.
- Una tarea necesita cambiar un test del modo octante: parar; la costura compartida está mal elegida.

## Tareas

### Task 1: Split `cell_pool.rs` (move only)

- [ ] `cell_pool.rs` queda como padre con la carpeta `cell_pool/`
  (`cache`, `residency`, `fit`, `fallback`, `hold`, `active`, `step`, `plan`; las semillas, `far` y la ventana en su
  propio submódulo), con `split.py` y sin líneas alteradas.
  Comprobación: `moved.py` sin diferencias no estructurales; `gate.sh run viewer_render_wgpu viewer_wasm` en verde.

### Task 2: Diagnose `far` at P4 and add the coverage property test

- [ ] `p4_border_levels` imprime, para el nodo 14, `node_depth`, `buildable`, y el px de la hija más pequeña en la
  esquina lejana y en el centro del borde: queda escrito por qué `far` vale 11.
- [ ] Test de propiedad (ignorado mientras falle): en poses aleatorias, todo nivel que el gemelo pide dentro de
  nodo ∩ datos ∩ vista cae en `[lo, hi]`. Debe fallar con `node_far_depth`.

### Task 3: `node_view_range`, a pure per-node range (not wired)

- [ ] Función pura `node_view_range(vista, rejilla, origen, región, tramo ∩ construible, objetivo px, lado mínimo) -> Option<(lo, hi)>`,
  con `región` = nodo ∩ datos. Tests unitarios: detrás del ojo → `None`, región vacía, lado mínimo, tope.
  Comprobación: el test de propiedad de la tarea 2 pasa sobre esta función.

### Task 4: Cell mode takes its range from `node_view_range` (R)

- [ ] `refresh_depths` y `cell_cut_ranged` usan la función nueva en modo celda; `behind_*` y `node_far_depth` dejan
  de usarse ahí.
  Test que falla antes: en P4 el nodo 14 tiene `range.lo <= 10` y ningún par visible difiere en más de un nivel.

### Task 5: The sim oracle stops reading `wanted` and `stage`

- [ ] El oráculo de celdas (`lod_cells_stream_tests.rs`) usa como tope el tramo construible y como excusa «el
  nivel pedido no es residente». Comprobación: un test de mutación como `the_oracle_reports_…` sigue detectando la
  mutación.

### Task 6: Node-level `wanted`/`drawn` in cell mode; delete the seeds (R)

- [ ] `drawn_levels` y `cell_pool` rellenan los campos a nivel de nodo (D4); se borran las semillas,
  `prev_state_seeded`, la parte de celda de `drawn_ceiling` y las ramas de sonda de celda.
  Test que falla antes: `cell_mode_keeps_the_stepped_depth_active` reescrito como «en modo celda no hay paso».

### Task 7: Cell mode without steps in `plan` and `fit` (R)

- [ ] `plan` no usa `steps`/`step_waits` en modo celda; `fit` limita desde `hi`; el orden de peticiones no depende
  de `Need.step`. Test que falla antes: un test de pool donde el rango cabe bajo el tope sin clave de paso.
  Comprobación: `level_requests` por frame y `pool_coarsened` de las simulaciones reales frente a hoy.

### Task 8: `covers`, a pure residency gate

- [ ] `covers(tramo del hijo, nivel más fino del antepasado)`: verdadero cuando el tramo del hijo alcanza ese nivel
  más uno. Tests unitarios.

### Task 9: Cell mode uses the residency gate instead of the per-node hold (R)

- [ ] `eval_hold`/`add_d5_gate` usan `covers` en modo celda en lugar de `eval_refine_hold` (R3-Q21).
  Test que falla antes: en un borde, un hijo no tapa a su antepasado antes de que su tramo sea contiguo.

### Task 10: Whole-change verification

- [ ] `FUZZ_CHECK=0 scripts/check.sh`; navegador en P1 a P5 y zoom en movimiento; registro fechado con los
  contadores de las simulaciones antes y después; `docs/reference/` y el ADR 0055 enlazados donde cambie el contrato.

## Solo tras retirar el modo octante (#67)

Borrar `stepped_depth`, `STEP_WAIT_FRAMES`, `step_waits`, `prev_state`, `step_hops`, `finer_drawn`,
`eval_refine_hold`, las sondas por octante y la búsqueda con pista; los campos `CellCutEntry.{wanted,drawn,stage,step}`
(públicos en `lib.rs`); el `busy` por paso del driver y las ramas `LodMode`.

## Riesgos y su comprobación

| Riesgo | Comprobación más barata |
| --- | --- |
| Sin prioridad de paso, todos los niveles de `lo` a `hi` se piden a la vez y el `fit` sigue limitando por octante | Contadores de las simulaciones reales (peticiones por frame, `pool_coarsened`) frente a hoy; después Majadas |
| Huecos o engrosamiento en bordes mientras cargan niveles (la semilla se borra en la tarea 6 y la retención sigue hasta la 9) | Tests P2 y P4, partición en GPU, `zone_coarsenings` y `lost_samples` |
| El oráculo se afloja al cambiar `wanted`/`stage` | Tarea 5 antes de la 6, con test de mutación |
