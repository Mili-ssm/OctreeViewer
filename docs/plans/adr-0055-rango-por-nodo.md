# ADR 0055 en modo celda: rango por nodo y puerta de residencia

Estado: en curso · 18-11-2026 · tramo A hecho y fusionado (#79); tramos B y C replanteados el 18-11-2026 tras retirar el modo octante ([ADR 0056](../adr/0056-se-retira-el-modo-octante.md)) y resolver OQ-16; pre-mortem del replanteo aplicado el 18-11-2026

**Conclusión.** Tres tramos, cada uno entregable por separado. El **A** está hecho: `cell_pool.rs` dividido y el
extremo grueso `lo` del rango calculado con una cota por nodo. El **B** hace que el rango por nodo mande de verdad:
`hi` limitado por datos, peticiones y residencia desde el rango, y los campos por octante sustituidos por rango y
ancla por nodo. El **C** sustituye la retención por nodo por una puerta de residencia y borra la maquinaria por
octante que queda sin uso. Desde el 18-11-2026 no hay modo octante: ya no existe código compartido que mantener
alimentado, y lo que antes solo se podía borrar «tras retirar el modo octante» entra en el tramo C.

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
| D4 | Los campos por octante de `CellCutEntry` (`wanted`, `drawn`, `stage`, `step`) se **sustituyen** por rango y ancla por nodo; no se rellenan a nivel de nodo (replanteada el 18-11-2026) | Rellenarlos era para no tocar el código compartido con el modo octante, que ya no existe. Queda vigente lo que el pre-mortem encontró: cada lector hay que adaptarlo (`finer_drawn`, la ruta en solitario de `eval_refine_hold`, el tope de `fit`, `needed_levels`, `lost_zones`, el resumen del corte, el `busy` del driver) y la demo da por alcanzado el objetivo con `poolStages === 0` |
| D5 | La regla de activos (`ActiveRule`) no se toca aquí | La elige el propietario ([#66](https://github.com/Mili-ssm/OctreeViewer/issues/66)); las tareas valen para las dos |
| D6 | La puerta de residencia (tareas 9 y 10) va antes de borrar semillas y `drawn_ceiling` (tarea 11) | `eval_refine_hold` usa las dos: borrarlas antes deja un estado intermedio que congela al hijo o engrosa la imagen (pre-mortem) |
| D7 | Los niveles del tramo se piden de grueso a fino | Con la regla contigua, `hi` no se activa hasta que llegan los intermedios; pedirlo primero retrasa el refinado visible |
| D8 | `hi` se limita por datos: se calcula a la distancia de la celda ocupada más cercana del nodo, no en el recorte cercano del LOD (propietario, OQ-16, 18-11-2026) | La cota fiel a la regla pide el último nivel en puntos sin datos. Medido: la cota por datos contiene lo pedido en 44 de 44 casos con 0 o 1 niveles de holgura ([registro](../records/benchmarks/R3_EXTREMO_FINO_POR_DATOS_18-11-2026.md)) |
| D9 | La celda ocupada más cercana se estima con las cajas de página del nivel residente más fino, no recorriendo celdas | Recorrer celdas son millones de operaciones por frame. Cuánto afloja la cota hay que medirlo (tarea 5) |

## Restricciones

- No se añaden reglas por octante.
- La regla de activos (`ActiveRule`) sigue con sus dos valores hasta que el propietario elija (#66); cada tarea vale
  para los dos.
- Puertas de todas las tareas: `cargo test -p viewer_render_wgpu -p viewer_wasm --profile fast --offline --lib`,
  clippy nativo y wasm32, los goldens de corte en modo celda y los tests de pose P2 y P4.
- Las tareas marcadas **R** añaden las simulaciones reales
  (`R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile fast --offline --lib -- --ignored real_sim --nocapture`)
  con `unexcused_drops`, `zone_coarsenings` y `lost_samples` a 0. Sus líneas `SimStats` ordenadas se comparan con la
  [línea base](../records/benchmarks/R3_LINEA_BASE_CELDA_18-11-2026.md): un cambio se explica en la tarea o se para.
- Antes de fusionar: `FUZZ_CHECK=0 scripts/check.sh` y navegador en las poses P1 a P4.
- Un trabajo en un worktree aparte compila con su propio `CARGO_TARGET_DIR`.

## Criterios de parada

- La medida de la tarea 2 no da una fórmula cerrada de `hi` a un nivel o menos del máximo que pide el gemelo en los
  nodos que contienen el ojo: parar antes de la tarea 5, entregar el tramo A y replantear `hi` con el propietario (#69).
- La cota de D2 no se puede calcular sin recorrer celdas (coste por nodo y frame no acotado): parar y replantear.
- Las simulaciones reales piden más de 2 × los niveles por frame de hoy o agotan el pool donde hoy no: parar.
- Una simulación real necesita cambiar un umbral o una excusa del oráculo para pasar: parar.
- `poolStages > 0` en reposo en una simulación de celdas: parar la tarea en curso.
- La residencia medida en Majadas a 1 px dentro de una hoja pasa del 90 % de lo usable (51 610 de 57 344 páginas; hoy 45 806): parar y replantear `hi` con el propietario.
- Tarea 5a: la cota con la fuente elegida queda más de un nivel sobre la cota celda a celda en el nodo del ojo:
  cambiar de fuente a las cajas de datos por octante; si tampoco, parar.
- Una clave se pide y se cancela, o se expulsa y se vuelve a pedir, más de dos veces con la pose fija: parar.
- En la simulación de dolly (semilla 3), los frames hasta asentarse o los frames más gruesos que la línea base
  superan 1,5 × los de hoy: parar y enseñárselo al propietario en el navegador.
- Una subtarea de la 10 no termina en dos intentos de un trabajador: entregar hasta la 7a y replantear.
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

### Tramo B: el rango por nodo manda

**Salida menor (decisión del propietario si se llega a ella).** Tareas 5a, 5, 6 y 7a, y parar: `hi` por datos como
tope de lo que se pide junto al ojo, con la maquinaria por octante intacta. Captura las páginas junto al ojo y la
respuesta de residencia de OQ-16; renuncia a borrar la maquinaria por octante (el estado final que pide el ADR 0055).

### Task 5a: Measure the sources of the data bound (no wiring)

- [x] (6416ad5) Cuatro fuentes medidas, con y sin recorte a la vista
  ([registro](../records/benchmarks/R3_FUENTES_COTA_POR_DATOS_18-11-2026.md)). Todas son válidas; sin recorte todas
  vuelven al último nivel. **Fuente elegida: cajas de página del nivel residente más fino, recortadas a la vista**:
  igual a la cota celda a celda en el nodo que contiene el ojo, estable al quitar niveles finos, +36 páginas
  estimadas en Majadas. Límite aceptado: en el nodo contiguo al ojo queda 3 o 4 niveles holgada (la caja de una
  página de 512 celdas es amplia). El criterio de parada hablaba del nodo del ojo, que se cumple; la holgura del
  contiguo se vigila con la residencia real de la tarea 7a. Las cajas de datos por octante no mejoran nada.

### Task 5: `hi` bounded by data, from a source that does not depend on residency

- [ ] `node_view_range` calcula `hi` con las cajas de página del nivel residente más fino, recortadas a la vista
  (el recorte exacto del test de 5a pasa a producción). La fuente depende de la residencia: la sonda de 5a no vio
  oscilación, pero solo podía quitar niveles, así que `hi` solo baja cuando cambia la pose (histéresis). Sin conectar a las peticiones todavía.
  Tests: 0 infracciones frente a lo que pide el gemelo donde hay puntos; con pose fija y el pool por encima del
  umbral de expulsión, 200 frames sin que ninguna clave se pida dos veces.
- [ ] La caché de profundidades por nodo (`refresh_depths`) se invalida cuando cambia el dato del que sale `hi`.

### Task 6: The sim oracle checks the node range, not per-octant fields

- [ ] El oráculo de celdas muestrea solo celdas ocupadas (o la rejilla cortada con las cajas de página), para que
  un `hi` que se ajusta con la cámara quieta no cuente como caída en puntos de aire. Deja de leer `wanted` y
  `stage` por octante. Comprobación: la mutación que deja de proteger los niveles aplicados da
  `unexcused_drops > 0`.
- [ ] Regla de contigüidad del conjunto activo escrita y comprobada
  ([#83](https://github.com/Mili-ssm/OctreeViewer/issues/83)): contiguo respecto de los niveles residentes, con la
  excepción del nivel D5. Se activa `the_cell_oracle_reports_a_middle_depth_missing_from_a_run`.

### Task 7a: Cap the per-octant wish at the data bound (R)

- [ ] Antes de tocar nada, se fija lo de hoy: niveles residentes del nodo del ojo en P2, y frames hasta el nivel
  más fino dibujado en el acercamiento de la simulación de dolly (semilla 3).
- [ ] `cell_cut_ranged` limita el deseo por octante a `max(lo, hi_d)`: peticiones, pasos, `stage` y `fit` lo
  siguen sin más cambios. Hoy los octantes que cruzan el plano del ojo piden el último nivel y los pasos lo traen
  aunque `range.hi` sea menor. Test que falla antes: con el ojo dentro de los datos, el nodo del ojo no pide niveles
  más finos que `hi_d`.
  Puertas: residencia real en Majadas a 1 y 4 px frente a 45 806 y 16 214 páginas; dolly s3 y zoom en el navegador.

### Task 7b: Requests come from the node range (R)

- [ ] `needed_levels` y las peticiones salen de `[lo, hi]`, de grueso a fino (D7), sin el rango `range_only`
  aparte. Comprobación: frames hasta completar en s1, s7 y s3 frente a la tarea 7a.

### Tramo C: puerta de residencia, campos por nodo y borrado

### Task 8: `run_covers`, a pure residency gate for both directions

- [ ] Test de caracterización que fija `eval_refine_hold` en un borde al acercarse (hijo frente a antepasado) y en
  uno al alejarse (nodo frente a descendientes aplicados, la rama `below`).
- [ ] `run_covers` cubre los dos casos: el tramo del hijo alcanza el nivel más fino del antepasado más uno; y, al
  alejarse, el nodo no sustituye a descendientes cuyo tramo dibuja más fino de lo que él alcanza. Tests unitarios.
  (El nombre evita `covers_in` y `CoverRule::cover`, que ya existen en `fallback.rs`.)

### Task 9: The residency gate replaces the per-node hold (R)

- [ ] `eval_hold` y `add_d5_gate` usan `run_covers` en lugar de `eval_refine_hold` (R3-Q21). Los tramos ya existen
  hoy en `cell_active_depths`, así que esta tarea no espera a la 10. Comprobación: el test de caracterización de la
  tarea 8 y `zone_coarsenings` a 0 en la simulación de dolly.

### Task 10: Node-level range and anchor replace the per-octant fields (R)

- [ ] **10a, en este plan.** Se definen «ancla» y `stage` por nodo. `stage` se mide sobre
  `[lo, min(hi, separación, tope del fit)]`, lo que de verdad se puede pedir: si no, `stages` no llega a 0 en reposo.
- [ ] **10b.** `CellCutEntry` gana los campos por nodo junto a los viejos y los lectores se pasan de uno en uno,
  un commit cada uno: `busy` del driver, `stages`, el tope de `fit`, `lost_zones`, el resumen del corte.
- [ ] **10c.** Se quitan los campos por octante; los goldens y resúmenes que cambian se rehacen comprobando a mano
  una muestra de entradas, con el motivo escrito.
- [ ] **10d.** Los tests se migran (unas 290 referencias en más de diez ficheros) y el oráculo de pasos de
  `cell_pool_tests` se sustituye por uno del tramo activo.
- [ ] Test de la regla `Window`: con la cámara quieta, el tramo activo llega a `[lo, hi]` en como mucho `hi − lo`
  frames y el driver sigue ocupado mientras tanto.

### Task 11: Delete the per-octant machinery (R)

- [ ] Se borran `cell_pool/seed.rs`, `stepped_depth`, `STEP_WAIT_FRAMES`, `step_waits`, `prev_state`, `step_hops`,
  `finer_drawn`, `eval_refine_hold`, las sondas por octante y la búsqueda con pista, `behind_*`,
  `MaskUpdate.active` y el `busy` por paso del driver, con sus tests. Guiado por el compilador, como la retirada de
  `LodMode`. Comprobación: mismas cifras de las simulaciones reales que tras la tarea 10.

### Task 12: Whole-change verification

- [ ] `FUZZ_CHECK=0 scripts/check.sh`; navegador en P1 a P4 y zoom en movimiento, con «objetivo alcanzado» en
  reposo; registro fechado con las simulaciones y la residencia en Majadas antes y después; `docs/reference/`,
  `docs/explanation/REGIONAL_VOXEL_LOD.md` (su regla E6) y el ADR 0055 enlazados donde cambie el contrato.

## Riesgos y su comprobación

Del pre-mortem del 18-11-2026 (anterior al replanteo) y de las medidas posteriores.

| Riesgo | Comprobación más barata | Tarea |
| --- | --- | --- |
| La cota de `hi` con cajas de página afloja demasiado o depende de qué nivel es residente y oscila | Comparación con la cota celda a celda; test con un nivel que entra y sale | 5 |
| Al mandar el rango sobre las peticiones, el pool retiene más de lo que cabe en nubes grandes | Residencia real en Majadas a 1 px; criterio de parada del 90 % de lo usable | 7 |
| Todos los niveles de `lo` a `hi` se piden a la vez y el refinado visible se retrasa | Frames hasta completar en s1 y s7; orden de grueso a fino | 7 |
| El oráculo se excusa a sí mismo con «el nivel pedido no es residente» | Excusa específica y mutación por expulsión | 6 |
| `stages` no llega a 0 en reposo y la demo nunca da el objetivo por alcanzado | Aserción `stages == 0` en reposo | 8 |
| Quedan lectores por octante sin adaptar | Lista de lectores en el encargo | 8 |
| La regla `Window` abre un hueco al separar el ancla de la ventana | P2 y P4 con las dos reglas y un alejamiento | 8 |
| La retención congela al hijo o engrosa la imagen si se borra antes de sustituirla | Orden D6; simulaciones reales en cada tarea R | 10, 11 |
| Los tests de pool y driver pasan con rangos de un solo nivel y no ven el cambio | Los ocho tests de rangos anchos de la [retirada del modo octante](retirada-del-modo-octante.md), ampliados en cada tarea | 7, 8 |
