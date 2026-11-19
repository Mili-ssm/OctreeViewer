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

- [x] (6a29ea5) `nearest_occupied_depth` (`cell_range.rs`): profundidad de vista mínima de las cajas de página
  recortadas a la vista, con el recorte exacto medido en 5a; `node_view_range_at` evalúa `hi` ahí (con `None` se
  comporta como antes). Siete tests unitarios, entre ellos la caja junto al ojo que está cerca en distancia y lejos
  en profundidad. El diagnóstico de 5a llama ya al código de producción y da las mismas cifras en P2 y P4.
- [x] (6a29ea5, 85d6554) El pool guarda `hi_d` por nodo en la caché de profundidades, sin usarlo en ninguna
  decisión: con la pose exacta sin cambios solo sube (histéresis) y se recalcula al moverse; se vuelve a mirar
  cuando cambia el nivel residente más fino (por `residency_epoch`), sin refrescar la entrada ni tocar las claves
  memorizadas. Tres tests de pool; quitar la histéresis o el disparador los hace fallar. Nada observable cambia:
  goldens intactos y simulaciones reales idénticas. Un nivel residente siempre tiene sus cajas en producción.
  Sin medir: páginas por nodo y coste del recorte por frame. El test de «200 frames sin repetir petición» pasa a la
  tarea 7a, que es donde `hi` se conecta. **La histéresis y el disparador por nivel más fino de este punto se
  sustituyeron en la tarea 7a**, que redefine la cota.

### Task 6: The sim oracle checks the node range, not per-octant fields

- [ ] **Aplazado a la tarea 10** (intentado el 18-11-2026 y parado por su criterio de parada): que el oráculo
  juzgue solo celdas ocupadas y deje de leer `wanted` y `stage` por octante. Con el tramo construible como tope, el
  oráculo marca 90 caídas en celdas ocupadas que no son un defecto nuevo: son la diferencia ya medida entre lo que
  pide el gemelo junto al ojo (niveles 14 a 17) y el deseo del pool (9 a 10), y el gemelo se satura en las celdas
  que cruzan el plano del ojo o tocan el lado mínimo, así que no sirve como único tope. El tope correcto es el
  rango por nodo con la cota por datos, que existirá cuando sustituya a los campos por octante. Mientras tanto la
  tarea 7a limita `wanted` a la cota, y el oráculo actual, que usa `wanted` como tope, sigue valiendo. El trabajo
  a medias (muestreo solo de celdas ocupadas, excusa por expulsión de nivel) está en
  `.superpowers/sdd/retomar-18-11/t6a/t6a-wip.patch`.
- [x] (da47822) Regla de contigüidad en el oráculo ([#83](https://github.com/Mili-ssm/OctreeViewer/issues/83)):
  entre el nivel activo más grueso y el más fino de un nodo, todo nivel residente y usable dentro del tramo del nodo
  está activo; el nivel D5 queda exento. Corre en cada frame de todas las simulaciones, que ahora aplican de verdad
  su regla de activos y corren con las dos. `the_cell_oracle_reports_a_middle_depth_missing_from_a_run` activado.
  **La regla destapó un defecto** ([#87](https://github.com/Mili-ssm/OctreeViewer/issues/87)): con `Window`, un
  nodo que dibuja por debajo de su rango dejaba un hueco (`[10, 12]` con el 11 residente). Arreglado en `3839ab1`
  (el extremo grueso del tramo baja hasta el nivel dibujado más grueso), con test unitario que falla antes;
  simulaciones reales idénticas a la línea base en contadores y resúmenes. Quedan fijados en test, sin arreglar,
  dos huecos más de `Window` que ninguna simulación dispara: activa un nivel al otro lado de uno no residente, y
  deja huecos si lo dibujado cae más de un nivel bajo la ventana. `Contiguous` no tiene ninguno.

### Task 7a: Cap the per-octant wish at the data bound (R)

- [x] (d8e49d8) Fijado lo de hoy: residencia del nodo del ojo en P1 a P5, retraso en dolly, órbitas y zoom
  sintético, y Majadas. Los niveles más finos que la cota no se dibujaban en ninguna muestra ocupada.
- [x] (66f7ac6, 5d46959) `Ctx::depth` limita el deseo por octante a `max(lo, hi_d)`; peticiones, pasos, `stage` y
  `fit` lo siguen sin más cambios ([registro](../records/benchmarks/R3_TOPE_POR_DATOS_19-11-2026.md)).
  **La cota de la tarea 5 no servía conectada**: al depender del nivel residente más fino, el corte final dependía
  del orden de llegada. Ahora es el mínimo, entre los niveles residentes `L`, de `max(b(L), L)`, y un nivel sin
  cajas a la vista aporta `L`; sin histéresis. Tests que fallan antes: el nodo del ojo no pide niveles sobre la cota,
  la cota no depende de los niveles sobre ella, un nivel sin cajas a la vista acota el nodo (por mutación).
  Puertas: Majadas 45 806 → 45 734 y 16 214 → 16 059 páginas; simulaciones reales con contadores a 0; navegador sin
  huecos en zoom y giro, P1 a P4 a ≤ 0,119 % de la compilación anterior.
  **El criterio de parada del dolly saltó y se descartó con evidencia**: las 267 779 muestras más gruesas están
  fuera del recorte de la vista (no se dibujan); dentro, 0 de 703. El medidor las separa ahora (`LAGSPLIT`).
  Sin arreglar, anotado en el registro: la cota de un nodo se queda en la pose anterior mientras la cámara se mueve
  dentro de su margen de deriva, y cada llegada de nivel recalcula la cota de todo el corte.

### Task 7b: Requests come from the node range (R)

- [x] (2fe583a, 3ba8d60) Los niveles que un nodo necesita y pide salen de un único rango `[lo, top]`
  (`CellPool::node_range`), de grueso a fino (D7), sin la clasificación `range_only`. `top` es el mayor entre
  `range.hi` y el deseo más fino de sus octantes: `range.hi` no cuenta los octantes detrás del ojo o que cruzan su
  plano, cuyo deseo (ya limitado por la cota) se dibuja igual. **Se conserva la guarda de espaciado**: más allá del
  nivel almacenado más fino solo se pide lo que un octante quiere exactamente. El primer intento la quitó (pedía
  toda la escalera hasta `top` y un test necesitó un pool 64 veces mayor); la revisión lo detectó y se restauró.
  Comprobación: simulaciones reales idénticas línea a línea a la tarea 7a; frames hasta asentarse iguales (dolly 29,
  órbitas 53 y 37); 0 muestras dibujables más gruesas; Majadas a 4 px idéntica (16 059 páginas, mismo resumen del
  corte); en la muestra, P2 2314 → 2170 y P3 2746 → 2602 páginas; `scripts/check.sh` 23 de 23.

### Tramo C: puerta de residencia, campos por nodo y borrado

### Task 8: `run_covers`, a pure residency gate for both directions

- [x] (2b8a1e1) Tests de caracterización de `eval_refine_hold` en un borde al acercarse (23 casos) y en uno al
  alejarse (28 casos), con cinco mutaciones que los hacen fallar. **Regla observada**, por octante, con `F` = nivel
  activo más fino de lo que dibujaba antes sobre la huella y `w` = deseo del nodo: el nodo se retiene si lo que
  dibujaría queda por debajo de `min(F, w)`. **No hay «más uno»**: un hijo que solo tiene residente `F` se libera
  hoy. El «más uno» que decía este plan era un error de redacción (confundía la retención con el paso de un nivel
  por frame); vale la regla observada, que es la de R3-Q21 (ninguna zona se engrosa).
- [x] (64631ad) `run_covers(run, before, wish)` (`cell_pool/hold.rs`, solo en tests hasta la tarea 9): el nodo
  puede sustituir cuando el extremo fino de su tramo alcanza `min(máximo fino de before, wish)`; `before` es el
  tramo del antepasado al acercarse y los de los descendientes al alejarse. Coincide con `eval_refine_hold` en los
  51 casos, con las dos reglas de activos.
- [x] (0f743bb) **El árbitro es el gemelo de la GPU, no la regla vieja.** La revisión vio que la equivalencia anterior
  era en parte circular (el tramo de prueba salía de lo mismo que compara `eval_refine_hold`). Ahora cada caso
  (35 al acercarse, 39 al alejarse, con las dos reglas de activos y dos anchuras de rango, y con el nodo ya retenido)
  se juzga contra `cell_lod::drawn_depth_at`: liberar es seguro si ninguna celda de la huella dibuja más grueso que
  antes. `run_covers` coincide con el gemelo en todos. `eval_refine_hold` retiene de más en 5 casos mixtos al
  alejarse (`ZOOM_OUT_PARTS`): juzga por octante que se dibujaría el nivel 4 donde se quiere el 5, y la GPU dibuja
  ahí el 6 o el 7, que están activos. Es decir, la tarea 9 libera antes en esos casos sin engrosar nada.

### Task 9: The residency gate replaces the per-node hold (R)

- [x] (70e2624, 5fb44dc) `eval_hold` decide la retención con `run_covers` sobre el tramo activo del nodo (el que
  recibe la GPU); la histéresis, las salidas y los retornos tempranos no cambian. `eval_refine_hold` queda solo
  para tests hasta la tarea 11. **`add_d5_gate` no cambia**: nunca usó la regla por octante (este plan lo daba por
  hecho); un test nuevo la fija y falla si se quita.
  Comprobación: la decisión de producción coincide con el gemelo de la GPU en los 74 casos de la tarea 8 (los 5
  mixtos al alejarse ahora se liberan); con tope de memoria, el conjunto que el plan emite de verdad no engrosa
  ninguna celda (más de 30 casos, las dos direcciones); pose fija 50 frames sin parpadeo del conjunto retenido;
  simulaciones reales idénticas byte a byte con `zone_coarsenings` a 0; cortes de las simulaciones sintéticas sin
  cambio; `scripts/check.sh` 23 de 23; navegador sin huecos ni zonas gruesas al alejarse (muestra y Belvedere, las
  dos reglas).
  Límites: los datos reales no llegan a los 5 casos mixtos, así que ese cambio solo lo cubren las tablas y el
  gemelo; el navegador no ve transitorios de menos de unos 150 ms ni se probó alejarse con el pool a medio cargar;
  un nodo cuyo nivel necesario no llega sigue retenido sin tiempo límite, igual que con la regla anterior.

### Task 10: Node-level range and anchor replace the per-octant fields (R)

- [ ] **10-pre.** Se retira la regla `Window` (propietario, 19-11-2026): `ActiveRule`, `applied_window`, el
  selector de la demo, `setCellActiveRule` y `?cellActive=` (ignorado con aviso). Comprobación: las líneas
  `SimStats` de `Contiguous` y el resumen de la entrada de la GPU, idénticos; navegador.

- [ ] **10a, en este plan** (diseño del agente, 19-11-2026; **enmendado por el pre-mortem**: falta fijar los
  oráculos de 10a-bis y la decisión del propietario sobre la regla `Window` antes de ejecutar 10b).
  Ver [«Diseño de la tarea 10»](#diseño-de-la-tarea-10-campos-por-nodo).
- [ ] **10b.** `CellCutEntry` gana los campos por nodo junto a los viejos y los lectores se pasan de uno en uno,
  un commit cada uno: `busy` del driver, `stages`, el tope de `fit`, `lost_zones`, el resumen del corte.
- [ ] **10c.** Se quitan los campos por octante; los goldens y resúmenes que cambian se rehacen comprobando a mano
  una muestra de entradas, con el motivo escrito.
- [ ] **10d.** Los tests se migran (unas 290 referencias en más de diez ficheros) y el oráculo de pasos de
  `cell_pool_tests` se sustituye por uno del tramo activo.
- [ ] **10e.** El oráculo de las simulaciones juzga solo celdas ocupadas, con el rango por nodo como tope y una
  excusa específica (el nivel que falta fue expulsado o limitado por el `fit` en ese frame); parte del parche
  guardado de la tarea 6.
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

## Diseño de la tarea 10: campos por nodo

**Conclusión.** `CellCutEntry` pasa de cuatro arrays por octante a cuatro valores por nodo: `range`, `anchor`,
`stage` y `step`. La GPU ya solo recibe el conjunto de niveles activos del nodo y la máscara 8³; lo único que
dependía de verdad de un valor por octante es el paso de un nivel por frame (ADR 0053), que pasa a ser por nodo.

Inventario de lectores (19-11-2026, sobre `cefcc04`): de unos 40 usos en producción, los valores por octante solo
son esenciales en la cadena de pasos (`drawn_levels`, `prev_state`, `step_waits`, `step_hops`, semillas). El resto
reduce a máximo, «alguno», recuento o conjunto, o a **presencia** (`drawn[o].is_some()`), que equivale a «octante no
enmascarado y el nodo tiene algún nivel residente en su tramo».

| Campo nuevo | Significado | Sustituye a |
| --- | --- | --- |
| `range: Option<(u8, u8)>` | `[lo, hi]`: `lo` de `node_view_range`; `hi` = el deseo más fino del nodo, ya limitado por la cota por datos y el tramo construible (`node_range` de la tarea 7b) | `wanted[o]` y el `range` actual |
| `anchor: Option<u8>` | Nivel más fino que el nodo puede tener activo este frame. `None`: nada residente en su tramo. Avanza como mucho un nivel por frame hacia `hi` (ADR 0053) | `drawn[o]` |
| `stage: bool` | El ancla no ha llegado al objetivo alcanzable: `anchor < min(hi, nivel almacenado más fino o el construible que se puede pedir, tope del fit)` | `stage[o]` |
| `step: Option<u8>` | Nivel siguiente que el ancla espera (ancla ± 1), si no es residente y usable | `step[o]` |

Reglas:

- **Activos.** `cell_active_depths` toma los niveles residentes y usables de `[lo, anchor]` según la regla de
  activos; ya no recibe niveles «dibujados». El nivel D5 sigue como hoy.
- **Paso.** Con `prev` = ancla aplicada del nodo: si `hi > prev`, el ancla es el nivel usable más fino `≤ prev + 1`;
  si `hi < prev`, baja a `max(hi, prev − 1)`; el salto por espera agotada (`STEP_WAIT_FRAMES`) se conserva, con una
  entrada por nodo en `step_waits`.
- **Entrada al corte** (nodo sin ancla aplicada). Al acercarse, `prev` = nivel activo más fino del antepasado que
  dibujaba (`applied_finest`), recortado al tramo del nodo. Al alejarse, `prev` = el ancla aplicada más gruesa de
  los descendientes que sustituye. La retención (`run_covers`, tarea 9) decide cuándo entra; no cambia.
- **Presencia.** Los lectores de `fallback.rs` (`covers_in`, `uncovered_levels`, `lost_zones`, `octant_covered`,
  `missing_levels`) usan `octant_fully_masked(mask, o)` y `anchor.is_some()`.
- **`stages`.** El contador pasa de octantes a nodos (más los términos `applied_covered` y `too_fine_excluded`, que
  se reescalan a nodos). La demo y las sondas solo comprueban `poolStages === 0`, que no cambia de significado.
- **Resúmenes.** `cell_cut_digest` y `drawn_digest` resumen `range`, `anchor`, `stage`, `step` y la máscara: todos
  los goldens cambian una vez (10c).

Qué deja de existir en la tarea 11: las semillas por octante (`seed.rs`), `PrevState` por octante, `finer_drawn`
por octante (pasa a «niveles sobre el tope que estaban activos») y el juez `PerOctant`.

Qué cambia de comportamiento, y cómo se comprueba:

| Cambio | Riesgo | Comprobación |
| --- | --- | --- |
| Un nodo cuyos octantes hoy dibujan niveles distintos tiene un único ancla | El conjunto activo puede diferir en un nivel durante el paso | La GPU ya elige por celda entre los activos: el oráculo del gemelo de la tarea 8, las simulaciones reales y P1 a P4 en el navegador |
| El paso es por nodo | Un nodo con deseos muy dispares (cerca y lejos) llega a `hi` en `hi − ancla` frames, no antes | Test de la regla `Window` de esta tarea; frames hasta asentarse en dolly y órbitas ≤ 1,5 × |
| `stages` cuenta nodos | Cifras de `poolStages` no comparables con las anteriores | Solo se usa `=== 0`; se anota en `PROBES.md` |

Orden (cada paso, un commit con los tests en verde):

1. **10b-1.** `CellCutEntry` gana `anchor`, `stage_node`, `step_node` calculados junto a los campos viejos, sin
   lectores. Test: en cada simulación, `anchor` = máximo de `drawn`, `stage_node` = algún `stage`, y el conjunto
   activo calculado desde `[lo, anchor]` es igual al de hoy; las discrepancias se listan, no se ocultan.
2. **10b-2 a 10b-6.** Un lector por commit: `busy` del driver, `stages`, tope del `fit` y peticiones
   (`needed_levels`, `requested_levels`, `steps`), presencia en `fallback.rs`, `cell_active_depths`.
3. **10b-7.** El paso por nodo sustituye a `drawn_levels` y a las semillas como fuente del ancla (aquí cambia el
   comportamiento); `applied_drawn` pasa a guardar el ancla.
4. **10c a 10e**, como están descritas arriba.

### Enmiendas del pre-mortem (19-11-2026)

**La regla «Paso» de arriba queda refutada para la regla por defecto.** Con `Contiguous`, el conjunto activo ya se
extiende hoy desde cada nivel dibujado usable hasta `hi`, sin límite de un nivel por frame
(`cell_pool/active.rs`, y ADR 0055: «no hay ventana de un nivel por frame»). Un ancla que avanza un nivel por frame
volvería a poner ese límite. Lo que sigue sustituye a las reglas «Paso», «Entrada al corte» y al «Orden».

| # | Enmienda | Fallo que evita |
| --- | --- | --- |
| A1 | `anchor` = extremo fino del tramo activo **emitido**. Con `Contiguous`: el tramo contiguo usable máximo hasta `hi`, sin límite por frame. Con `Window`: la ventana `applied_window` ± 1 de hoy. El paso por nodo y `STEP_WAIT_FRAMES` solo existen para `Window` | Reintroducir el límite que el ADR 0055 quitó; saltar el criterio de parada en 10b-1 |
| A2 | Una sola función pura `node_run(prev, lo, hi, usables, regla, espera agotada) -> Option<Run>` para el corte, la retención (`eval_hold`), la emisión, `stage` y el oráculo. La retención se cambia en el mismo commit que el paso por nodo | La puerta predice un tramo y el plan emite otro: al alejarse, una zona se engrosa sin que `zone_coarsenings` lo vea |
| A3 | Semilla al alejarse = el extremo fino aplicado **más fino** de los descendientes sustituidos (igual al `before` de la puerta), no el más grueso; al acercarse, `applied_finest` del antepasado. Luego se recorta a un nivel usable | Ídem |
| A4 | Invariante: el ancla es un nivel usable de la lista de residentes que usa el corte (tras retención, tope y `finer_drawn`) dentro del tramo del nodo, o `None` solo si esa lista está vacía. Propiedad de 10b-1: `anchor.is_some()` ⇔ algún `drawn[o]` es `Some`, en cada frame | Ancla en un nivel no usable: hueco, o `poolStages === 0` con la imagen más gruesa; presencia calculada con otra residencia |
| A5 | Una espera agotada nunca cruza un nivel no residente: baja el objetivo y se cuenta. `stage` = extremo emitido < objetivo | Conjunto activo con hueco (`{11, 18}`): celdas subpíxel con todos los contadores a 0 |
| A6 | **10a-bis, antes de 10b**: (i) golden de la entrada de la GPU (máscara y bits activos por nodo) en los goldens de corte y las simulaciones, idéntico byte a byte durante 10b-2 a 10b-6 y 10c; (ii) el oráculo marca los huecos de niveles **no residentes** dentro del tramo activo (los dos huecos conocidos de `Window`, exentos por nombre); (iii) test de mutación: engrosar un octante de un nodo que tiene otro octante en espera debe contar; (iv) en reposo, `stages == 0` ⇒ ninguna muestra más gruesa que su deseo | Aflojar el oráculo sin notarlo (D3) |
| A7 | Orden: 10e (oráculo sobre campos por nodo) **antes** del paso por nodo; 10c y 10d en un solo commit guiado por el compilador con el golden (i) sin cambios; hasta entonces, las peticiones usan la unión de los pasos por octante | Commits intermedios que no pueden estar en verde; el oráculo reescrito dentro de «quitar campos» |
| A8 | Parada medible: por nodo y frame, diferencia del extremo fino, del grueso y cualquier hueco no residente. Parar ante cualquier hueco, cualquier diferencia con `Contiguous`, o más de un nivel con `Window`. En el commit del paso por nodo se repiten las tablas de la tarea 8 (74 casos y los 5 mixtos) contra `node_run` | El criterio anterior no era medible |
| A9 | Lectores que faltaban en el inventario: `hidden_held` (`plan.rs`), `seed_depths` y `required_levels` en `residency.rs`, `pages.rs` (expulsión), `drawn_digest`, `finer_drawn`, `step_hops` | Lectores sin migrar |

**Decisión del propietario (19-11-2026): se retira `Window`** ([#66](https://github.com/Mili-ssm/OctreeViewer/issues/66),
OQ-11). `node_run` se reduce al tramo contiguo usable; desaparecen el paso por nodo, A3, A5 y la mitad de A1. El
diseño se rehace tras la retirada (tarea 10-pre abajo).

**10a-bis hecho** (35f69f1, 19c0c2e): resumen de la entrada de la GPU (bits activos y máscara por nodo) fijado en
P1 a P5 y en las simulaciones (sintéticas: igual a la referencia; reales: determinista en dos ejecuciones); el
oráculo cuenta los huecos no residentes del conjunto activo (`active_gaps`). **Con `Contiguous` aparecen**: 7
frames en la órbita real de semilla 1 (ninguno en el resto). Ejemplo: nodo 77, frame 36, activos {7, 12}, dibujados
7 y 12 por octantes distintos, rango (12, 12), deseo 7 en todos los octantes; 8 a 10 no residentes y 11 recién
subido (aún no usable). Es el hueco de la regla actual: cada nivel dibujado usable abre su propio tramo. Un tramo
único por nodo lo elimina por construcción, así que la aserción dura se activa en la tarea 10, no antes. El rango
(12, 12) con deseo 7 es la misma pregunta abierta de [#87](https://github.com/Mili-ssm/OctreeViewer/issues/87)
(`far` 12 con octantes que desean 7). Un nivel recién subido cuenta como hueco: la GPU aún no lo tiene.

Criterios de parada de la tarea 10 (sustituyen al del diseño):

- 10b-1: en algún frame de una simulación real, los bits activos de la regla nueva difieren de los de hoy.
- En el commit del paso por nodo, algún caso juzgado por el gemelo donde `run_covers` sobre `node_run` discrepa.
- Un hueco no residente llega al oráculo en alguna simulación.
- `poolStages === 0` con una muestra más gruesa que su deseo, o `stages > 0` en reposo.
- Un commit necesita tocar `stage_excuses`, el recuento `!staged` o el tope `wanted` fuera de 10e.
- Frames hasta asentarse en dolly u órbitas por encima de 1,2 × en un commit que solo mueve un lector.

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
