# ADR 0055 en modo celda: rango por nodo y puerta de residencia

Estado: completado · 19-11-2026 · salida reducida por decisión del propietario: hechas las tareas 1 a 9, la retirada de `Window`, el arreglo de #87 y un tramo activo por nodo; el cambio de comportamiento de la tarea 10 y las tareas 11 y 12 no se hacen (experimento aparcado en la PR en borrador [#88](https://github.com/Mili-ssm/OctreeViewer/pull/88)); se archiva al fusionar #86

**Cierre (19-11-2026).** El rango por nodo manda en lo que se pide (cota por datos, peticiones desde el rango) y
la retención la decide el tramo activo del nodo juzgado contra el gemelo de la GPU; cada nodo emite un solo tramo
activo. La maquinaria por octante (pasos, semillas, juez por octante) **se queda**: al conectar el tramo único por
nodo como fuente de la emisión aparecieron reglas que el diseño no tenía y 28 tests en rojo, para un beneficio
visible pequeño; el propietario eligió el arreglo mínimo. Evidencia en el
[registro del cierre](../records/benchmarks/R3_CIERRE_RANGO_POR_NODO_19-11-2026.md).

**Conclusión (texto original).** Tres tramos, cada uno entregable por separado. El **A** está hecho: `cell_pool.rs` dividido y el
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

- [x] **10-pre.** Se retira la regla `Window` (propietario, 19-11-2026): `ActiveRule`, `applied_window`, el
  selector de la demo, `setCellActiveRule` y `?cellActive=` (ignorado con aviso). Comprobación: las líneas
  `SimStats` de `Contiguous` y el resumen de la entrada de la GPU, idénticos; navegador.
  Código en d108004 (crates) y f09344a (demo y binding). Navegador: selector ausente sin hueco en el panel, orden
  de tabulación intacto, P1 a P4 a ≤ 0,098 % de la ejecución anterior, `?cellActive=` con un solo aviso, zoom, giro
  y Belvedere sin huecos. Páginas en reposo iguales antes y después con el mismo script (A/B, tres ejecuciones:
  P1 1143, P2 2792, P3 3152); P4 varía entre ejecuciones en las dos compilaciones (unas 1600 a 1960), así que no
  sirve como puerta. Las cifras del navegador de la tarea 9 (P1 962…) no se reproducen y se descartan.

- [ ] **10a** (diseño rehecho el 19-11-2026 tras retirar `Window`): ver
  [«Diseño de la tarea 10»](#diseño-de-la-tarea-10-campos-por-nodo), con las enmiendas del segundo pre-mortem.
  B7 hecho: #87 diagnosticado y arreglado (5c2f92d, más un retoque): el `lo` de un nodo cuyos octantes con datos
  están todos detrás del ojo salía de la caja unión, que «pasaba» por una esquina que tampoco se ve; ahora `lo` y
  la cota por datos miden solo octantes con datos a la vista (nodo 77: (12, 12) → (7, 7)). El hueco de ese nodo
  sigue: un octante aún dibuja 12 mientras baja nivel a nivel y otro ya dibuja 7; lo quita el tramo único (10e).
- [x] **10b.** `node_run` (función pura) y los campos por nodo, calculados junto a los viejos sin lectores.
  - [x] (ec9bd09) `cell_pool/run.rs`: `node_run` con 11 tests unitarios (B1, B2, B6, B7, B11); el plan rellena
    `node_anchor`, `node_run`, `node_stage`, `node_step` y guarda `applied_run` (lo emitido sin el D5) y la espera
    por nodo; `before_finest` es la única fuente de `before` (B5). Nada observable cambia (simulaciones y resumen
    de la GPU idénticos byte a byte). Para 10c: los campos se calculan al construir la memoria del corte, así que
    la espera agotada debe subir `residency_epoch` como hoy `step_waits`; y `before` aún sale de
    `applied_finest` (con el D5), y así debe seguir para la retención (ver 9b5d027).
  - [x] (4f8a784) Comparación por (nodo, frame) entre el tramo de `node_run` y los bits activos de hoy (B8), en
    todas las simulaciones. Reales: 7 huecos (solo órbita 1), 27 a 34 por encima de `hi`, 10 a 21 nodos que entran,
    0 por `prev` expulsado, 13 a 19 extremo fino distinto, 0 a 4 sin clasificar; 2 frames «más grueso que hoy» por
    simulación, todos de nodos que entran sin `prev`. Tres hallazgos de diseño (C1 a C3 abajo).
  - [x] (5be1a1c, f4fd143, 8a68d17) C1 a C3 y los hallazgos de la revisión. `prev` sale de una sola función
    (`node_prev`): tramo aplicado, lo dibujado por el propio nodo, el `before` de la retención y, si no, la semilla
    de entrada de hoy (los tres frames «más gruesos» eran nodos sin cobertura para la retención, con historia en la
    semilla). Comparación final en las 4 simulaciones reales: 0 sin clasificar; 1 frame más grueso que hoy (órbita 1,
    frame 36, nodo 24): el antepasado dibujaba 8, el tramo único arranca en 8 y pide 10, y hoy salta a 11 porque la
    semilla se recorta al tramo del nodo. **Aceptado por diseño**: no es más grueso que lo que ya se veía (R3-Q21),
    solo refina nivel a nivel; lo vigila el criterio de 1,5 × frames hasta asentarse. Desde aquí, la parada de la
    comparación es «más grueso que `prev`», no «más grueso que hoy».
  - [x] (9b5d027) Corrección tras la revisión: 5be1a1c había cambiado decisiones de la retención en un commit «sin
    cambio» (su `before` dejó de contar el nivel D5, que la GPU sí puede dibujar). La retención vuelve a contar el
    D5 y el ancla no (B3 vale solo para `prev`), con un test que falla si se reintroduce; el D5 suelto sale de los
    niveles usables de `node_run`. Comparación: 0 sin clasificar, 0 frames con el tramo bajo `prev`. Para 10e: un
    nodo cuyo único nivel usable dentro del rango es el D5 suelto se ancla por encima de `hi` (visto en la
    referencia en proceso, frames 31 y 32, nodo 23).
- [ ] **10c.** Lectores a los campos por nodo y, después, **un** commit de comportamiento (B9).
  - [x] (eda80b8) `node_run` toma sus niveles usables de la misma lista de residentes que el corte (vacía para los
    nodos retenidos, sustituida por `finer_drawn` con tope): lo pedía A4; el encargo de 10b decía «todo el tramo».
  - [x] (f59ec3c) Invariante en cada frame de cada simulación: hay ancla ⇔ algún octante dibuja, y por octante,
    dibuja ⇔ no enmascarado y con ancla.
  - [x] (cd83d13) La presencia (`fallback.rs`) y `hidden_held` leen la máscara y el ancla, salvo `covers_in`: la
    retención lo llama antes de que existan las anclas (su `before` alimenta a `node_run`); pasa en el commit de
    comportamiento. Resumen de la GPU idéntico.
  - [x] (86a5052) `stages` cuenta nodos (antes octantes); en reposo sigue en 0. `poolStages` cambia de escala (la
    demo solo comprueba `=== 0`; anotar en `PROBES.md` al cerrar).
  - [ ] ~~**Commit de comportamiento**~~ (no se hace: aparcado en #88, decisión del propietario 19-11-2026; en su lugar, un tramo activo por nodo en `cell_active_depths`, a123fe3, y `active_gaps == 0` obligatorio, af75572; `stages` vuelve a contar octantes, f5d25ef): peticiones, protección, retención (`covers_in` incluido) y emisión desde
    `node_run`; `busy` espera a `node_step` (probado y aplazado: sin este commit nadie pide `node_step` y tres tests
    de GPU del driver no se asientan); la espera agotada sube `residency_epoch`; el caso del D5 suelto como único
    usable del rango; la aserción `active_gaps == 0`.
- [ ] **10d.** Oráculo de las simulaciones sobre los campos por nodo (lo aplazado de la tarea 6): solo celdas
  ocupadas, el rango por nodo como tope, excusa específica; `active_gaps` pasa a aserción (0). Antes de 10e (D3).
- [ ] **10e.** El tramo de `node_run` pasa a ser el emitido (aquí cambia el comportamiento) y se quitan los campos
  por octante; tests migrados (unas 290 referencias) en un commit guiado por el compilador.

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

Rehecho el 19-11-2026 tras retirar `Window`; sustituye al primer diseño y sus enmiendas (en el historial de git:
`5668487`, `94451f3`). De aquellas enmiendas siguen vigentes A2, A4, A6 (hecha en 10a-bis), A7, A8 y A9.

**Conclusión.** Un nodo tiene **un único tramo activo**: el tramo contiguo máximo de niveles usables que contiene su
**ancla**. La GPU ya solo recibe ese conjunto y la máscara; con `Contiguous` no hay límite de un nivel por frame
(ADR 0055), así que no hace falta paso ni semillas por octante.

| Campo | Significado |
| --- | --- |
| `range: Option<(u8, u8)>` | `[lo, hi]`, como hoy (`node_range`: `hi` incluye el deseo más fino, ya limitado por la cota por datos) |
| `anchor: Option<u8>` | El nivel usable del tramo del nodo más cercano a `prev`; empate, el más grueso. `None` solo si no hay ninguno (A4) |
| `run: Option<(u8, u8)>` | Tramo contiguo máximo de niveles usables que contiene el ancla, recortado a `[min(lo, ancla), hi]` |
| `stage: bool` | `run.hi < objetivo`, con objetivo = `min(hi, nivel más fino almacenado o pedible, tope del fit)`; un nivel rechazado baja el objetivo (A5) |
| `step: Option<u8>` | El nivel que extiende el tramo hacia el objetivo (`run.hi + 1`) o hacia `lo` (`run.lo − 1`); es lo que se pide con prioridad |

`prev`, de dónde parte el ancla:

- Nodo que ya dibujaba: el extremo fino de su tramo aplicado.
- Al acercarse (entra bajo un antepasado): `applied_finest` del antepasado, como el `before` de `run_covers`.
- Al alejarse (sustituye a descendientes): el extremo fino aplicado **más fino** de ellos, también el `before`.

Así la puerta de la tarea 9 y el tramo emitido salen de la misma función (`node_run`, A2): el nodo se libera cuando
`run.hi ≥ min(before, deseo)`, y lo que se libera es exactamente ese tramo.

Por qué el ancla más cercana a `prev`: es la que evita saltos. Si hay dos tramos separados por un hueco (el ejemplo
del nodo 77: niveles 7 y 12 usables, 8 a 11 no), se queda el que continúa lo que se veía; el otro se activa cuando
llegan los niveles del hueco. Hoy se activan los dos y la GPU salta entre ellos.

Qué desaparece en la tarea 11: `stepped_depth`, `drawn_levels`, `PrevState`, `STEP_WAIT_FRAMES`, `step_waits`,
`step_hops`, `seed.rs`, el juez `PerOctant` y `finer_drawn` por octante (pasa a «niveles activos sobre el tope»).

### Enmiendas del segundo pre-mortem (19-11-2026)

El tramo único vale (al acercarse es más prudente que hoy), pero el texto de arriba fallaba en tres sitios. Estas
enmiendas mandan sobre la tabla y las reglas anteriores.

| # | Enmienda | Fallo que evita |
| --- | --- | --- |
| B1 | Tramo emitido = tramo usable que contiene el ancla, recortado a `[min(lo, ancla), max(hi, ancla)]` (corregido el 19-11-2026: decía `min(lo, run.lo)`, que no recorta nada por abajo; la intención es la de hoy, `lo.min(dibujado)`): nunca vacío si hay ancla, como hoy `hi.max(dibujado)`. Test: `prev` 12, usables {5, 10, 11, 12}, `hi` 8 → {10, 11, 12} | Al alejarse rápido, tramo vacío (hueco) o salto de 12 a 5 |
| B2 | Ancla: el usable más cercano a `prev` en el sentido del objetivo (`[prev, objetivo]`) y, si no hay, el más cercano en general, como `nearest_to_prev` hoy | Engrosar un nivel cuando `prev` se expulsa |
| B3 | `prev` = extremo fino del tramo aplicado **sin** el nivel D5 (campo nuevo `applied_run`), no `applied_finest` | Anclar en el D5, que es un tramo aparte |
| B4 | Se protege el tramo aplicado entero, no solo lo dibujado (`required_levels` en `plan.rs`). La emisión recalcula `node_run` desde el ancla guardada | Tramo expulsado entre el plan y la emisión; puerta y emisión distintas |
| B5 | `before` sale de una sola función, usada por la puerta y por `prev`. Aserción en cada frame de simulación: extremo fino emitido ≥ el que supuso la puerta | Dos caminos de código para `before` |
| B6 | **Nivel que no llega** (decisión del agente: se conserva lo de hoy): si `run.hi + 1` no es usable en `STEP_WAIT_FRAMES` frames y hay un tramo usable más fino hacia el objetivo, el ancla salta a él (como el salto por espera de hoy; con tramo único no deja hueco). Test sintético: el 8 no llega nunca, 10 a 14 residentes | `stage` y `busy` sin llegar nunca a 0; las simulaciones reales no ejercitan el salto (`hops` = 0) |
| B7 | `stage` también es cierto mientras el deseo más grueso del nodo quede por debajo de `run.lo`. Antes de 10e se resuelve [#87](https://github.com/Mili-ssm/OctreeViewer/issues/87) (rango (12, 12) con deseo 7) | Un nodo en reposo dibujando 5 niveles más fino de lo que se quiere, sin contador que lo vea |
| B8 | 10b compara por (nodo, frame) y clasifica cada diferencia: hueco, por encima de `hi`, nodo que entra, `prev` no usable. Parar ante una sin clasificar. Además, una ejecución en sombra (rama aparte) donde `node_run` manda, con sus `SimStats` | Criterio de parada que salta siempre al alejarse o se afloja |
| B9 | Orden: primero los lectores que solo reducen (`stages`, `busy`, presencia, `hidden_held`), con el resumen de la GPU idéntico; luego **un** commit de comportamiento (peticiones, retención y emisión), con la aserción `active_gaps = 0` en él; 10e solo quita campos, resumen idéntico; la migración de tests, aparte | 10d en rojo antes de 10e; promesas de resumen idéntico imposibles |
| B10 | La tarea 11 conserva un término de deseo por nodo para los octantes detrás del ojo o que cruzan su plano (o prueba que `range` es `Some` en todo nodo que dibuja) antes de borrar las sondas | Nodos junto al plano del ojo gruesos o vacíos |
| B11 | `step` solo hacia arriba; los niveles hacia abajo llegan por `needed`; `busy` lee `stage` | `busy` cierto para siempre por `run.lo − 1` |

### Enmiendas de la comparación (19-11-2026, sobre 4f8a784)

| # | Enmienda | Lo que mostró la comparación |
| --- | --- | --- |
| C1 | `step` también hacia abajo: si el deseo más grueso queda bajo `run.lo` y `run.lo − 1` no es usable, ese nivel se pide (con la misma espera y salto que B6, hacia lo grueso). Sustituye a B11 | Al alejarse, el nodo 34 se quedaba en (11, 12) para siempre: nadie pedía 8 a 10 y `stage` no bajaba a 0. Hoy el paso por octante los pide de uno en uno |
| C2 | `prev` de **todo** nodo que entra = `before` (la función compartida de B5), no solo de los que juzga la retención; `None` solo si nada dibujaba sobre su huella | Los 6 frames «más grueso que hoy» de las simulaciones reales: nodos que entraban sin `prev`, anclados en el objetivo (6) cuando el antepasado dibujaba 8 |
| C3 | La comparación cuenta el D5 que la emisión añade y separa la clase «por debajo de `lo`» (el ancla sigue en `prev` bajo `lo`: un nivel más grueso activo que no se dibuja, porque refina en toda celda visible) | Los «sin clasificar» de las formas 1 y 2 |

Se acepta, por diseño, que un nodo refine nivel a nivel por su extremo fino (con salto tras la espera) aunque hoy
salte dos niveles tras reiniciar el driver (forma 3, solo en una simulación sintética con inyección): no engrosa
nada respecto a lo que ya se veía.

Criterios de parada añadidos: un nodo con ancla que no emite nada, o cuyo extremo emitido baja de `min(prev, deseo)`;
en pose fija tras asentarse, `stages > 0` o `needs_frame()` tras 2 × `STEP_WAIT_FRAMES`; el nodo 77 de la órbita 1 en
reposo dibujando ≥ 2 niveles más fino que el deseo del gemelo (llevar #87 al propietario); dolly s3 > 1,5 × frames
hasta asentarse; Majadas > 90 % de lo usable tras B4; saltos de varios niveles visibles en el navegador al alejarse
rápido (el ADR 0055 dice que entonces vuelve la ventana: decisión del propietario).

Riesgos propios:

| Riesgo | Comprobación |
| --- | --- |
| El tramo único deja fuera un nivel que hoy se dibuja (otro tramo tras un hueco) | Es el cambio buscado; se limita a los frames con `active_gaps > 0` (10b) y se juzga con el gemelo de la GPU |
| `stage` no llega a 0 en reposo por un objetivo inalcanzable | Criterio «`stages > 0` en reposo»; test con un nivel rechazado |
| El ancla cae en un nivel recién subido y el tramo queda vacío | El ancla solo elige niveles usables (A4) |

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
