# R3 — Cubos por nodo residentes en GPU (v7.5-pre): plan de implementación

Estado: en curso · 03-11-2026, enmendado el 05-11-2026 por el ADR 0032 (sustituye el plan del 30-10-2026, que sigue en el historial de git)

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Objetivo:** sustituir la voxelización en CPU por vista (B1) por celdas por nodo residentes en GPU, sobre `v7.4/v7.5-pre` paginado.

**Por qué:** con 256³ por nodo, el diseño de 0027 desborda el pool y dibuja lo lejano con detalle de más ([ADR 0031](../adr/0031-celdas-por-nodo-en-v7-5.md)).

**Spec:**
- ADR: [0019](../adr/0019-cubos-por-nodo-residentes-en-gpu.md), [0027](../adr/0027-celdas-por-nodo-en-pool-paginado.md) (en lo vigente), [0032](../adr/0032-hojas-adaptativas-e-instancia-de-12-b.md), [0033](../adr/0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md), [0034](../adr/0034-nivel-por-octante-con-arista-real.md), [0035](../adr/0035-instancia-de-16-b-con-w-16.md), [0036](../adr/0036-hi-z-por-celda.md), [0029](../adr/0029-octree-global-por-bits-de-enteros.md), [0030](../adr/0030-jerarquia-paginada-por-anclas.md) y [0031](../adr/0031-celdas-por-nodo-en-v7-5.md).
- Requisitos: RF-03 a RF-08, RQ-01, RQ-02, RQ-05 y RQ-09.
- [Contrato de celdas](../explanation/REGIONAL_VOXEL_LOD.md).

## Resoluciones del propietario (03-11-2026)

| Id | Resolución |
| --- | --- |
| R3-Q1 | Pirámide por nodo, de 8³ a 256³, podada. La Tarea 4 mide 256³ solo como control |
| R3-Q2 | Residencia todo o nada por (nodo, nivel) |
| R3-Q3 | Mientras falta el nivel deseado se dibuja el residente más grueso como etapa, y se cuenta. «Objetivo alcanzado» exige que no quede ninguno |
| R3-Q4 | RF-05: espaciado de referencia por nube en los metadatos de `v7.5-pre` (hay que regenerar) |
| R3-Q5 | La ruta CPU (Additive, v5 y `v7.1–3`) se mantiene hasta retirar V5/V6 |
| R3-Q6 | ~~Límite de 21 bits relativos por eje; si se supera, se para~~ Sustituida el 05-11-2026 por R3-Q7 y R3-Q8 ([ADR 0032](../adr/0032-hojas-adaptativas-e-instancia-de-12-b.md)) |
| R3-Q7 | (05-11-2026) La cadena de cada hoja baja hasta separar las posiciones finitas distintas; más allá de 21 bits la clave es regional; el límite es la profundidad absoluta 63 |
| R3-Q8 | (05-11-2026) ~~Instancia de 12 B: 10 bits por eje relativos a la página y flags en `PageEntry`; W = 16 (16 B) queda como control en T5~~ Sustituida por R3-Q14 |
| R3-Q9 | (05-11-2026) Una hoja con todas sus posiciones separadas no se refina más: cubos mayores que el objetivo de px en primeros planos caen en la excepción «densidad de la fuente» de RQ-09 |
| R3-Q10 | (05-11-2026) Una actualización de corte con `DescendantTooFine` se rechaza y se retiene el corte visible hasta que el nuevo esté completo ([ADR 0033](../adr/0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md)) |
| R3-Q11 | (05-11-2026) Si el corte deseado no cabe en el pool, los nodos más lejanos bajan a un nivel más grueso, como etapa contada, hasta que quepa ([ADR 0033](../adr/0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md)) |
| R3-Q12 | (05-11-2026) Nivel por octante con la arista proyectada real ([ADR 0034](../adr/0034-nivel-por-octante-con-arista-real.md)); en la pose rasante, ≤ 2× las instancias de la CPU a 2–8 px y ≤ 2,5× a 1 px |
| R3-Q13 | (05-11-2026) RQ-09 admite ≤ 0,5 % de celdas por encima del objetivo con la cámara dentro del dataset (ADR 0034) |
| R3-Q14 | (05-11-2026) Instancia de 16 B con W = 16 y 512 huecos por página, en lugar de 12 B con W = 10 ([ADR 0035](../adr/0035-instancia-de-16-b-con-w-16.md)) |
| R3-Q15 | (05-11-2026) La Tarea 16 (compactación y Hi-Z por página) es obligatoria y va antes de la 6; raymarching solo si no basta (ADR 0035). Técnica precisada por R3-Q16 |
| R3-Q16 | (05-11-2026) Hi-Z por celda en dos fases (`kpc` de la Tarea 16a), con caja ajustada por página en `PageEntry` ([ADR 0036](../adr/0036-hi-z-por-celda.md)) |
| R3-Q17 | (06-11-2026) La regla de 2 ms de CPU por frame de T7c/T7d decide con los casos a 4 px; Majadas dentro de una hoja a 1 px queda como prueba de carga y se vuelve a medir en el visor en la Tarea 11 |
| R3-Q18 | (06-11-2026) Con la cámara en movimiento, el nivel de cada octante se recalcula solo al pasar un umbral, y una celda puede proyectarse hasta ~5 % por encima del objetivo de px; con la cámara quieta, el resultado es exacto. La Tarea 11 lo comprueba en el navegador |
| R3-Q19 | (06-11-2026) Un corte de celdas se aplica si no deja huecos: cada nodo sin nivel en GPU queda cubierto por un antepasado dibujado, que sigue en el corte hasta que el nodo tiene nivel, y lo que falta en una zona no bloquea las demás. Se carga de grueso a fino: no se pide el nivel de un nodo antes que el de su antepasado. Sustituye la regla de aplicar de T7a («ningún nodo sin nivel»), que se acercaba a la alternativa que el [ADR 0033](../adr/0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md) recuerda rechazada en 0031; R3-Q10 sigue igual para `DescendantTooFine` |
| R3-Q20 | (06-11-2026) Un nodo del corte sin nivel en GPU y sin antepasado que dibuje no entra en el corte aplicado: su nivel más grueso se precarga con prioridad y entra cuando es residente. No bloquea el resto del corte. Si aplicar el corte dejaría de dibujar una zona que el corte anterior dibujaba, se retiene el anterior. Precisa R3-Q19 (hallazgo de T9b2b: la imagen se congelaba mientras llegaban nodos) Precisión del 06-11-2026: al alejar la cámara, los descendientes que ya se dibujaban siguen dibujando la zona del nodo grueso nuevo hasta que es residente; solo se retiene todo el corte si la zona no se puede cubrir así |
| R3-Q21 | (06-11-2026) La imagen solo se refina: el antepasado sigue dibujando la zona de un hijo hasta que el hijo tiene ahí un nivel al menos tan fino como el que dibujaba el antepasado. Precisa R3-Q19 (hallazgo de T9b2b: al sustituir un nodo se veía un instante un nivel más grueso) Precisión del 06-11-2026: el umbral es el más grueso entre el nivel que dibujaba el antepasado y el que se pide ahora (deseado y tope del ajuste), y se decide por nodo entero (la máscara no distingue octantes del hijo). Dentro de un nodo, si falta el nivel deseado, se dibuja el residente más cercano a él |
| R3-Q22 | (06-11-2026) Padres primero por nodo: antes de construir las celdas de un nodo se construyen las de su padre, aunque el padre no esté en el corte. La escalera de px de celdas solo arranca al abrir el dataset o tras un salto de cámara (ningún nodo del corte nuevo tiene celdas ni un antepasado con celdas); al orbitar no se reinicia. Sustituye la nota «reiniciar la escalera en cada movimiento de cámara» (pre-mortem de la Tarea 10: volvía más tosca toda la imagen en cada paso) |
| R3-Q23 | (06-11-2026) Hasta la Tarea 12, la ruta de celdas solo dibuja RGB: otro modo de color vuelve a la ruta CPU, y al volver a RGB las celdas se reactivan solas (siguen en la memoria del driver; solo se vuelven a subir a la GPU) |
| R3-Q24 | (07-11-2026) La ruta de celdas solo funciona con el worker de construcción. Sin él (`buildWorker=0`, `file://` o un navegador sin workers anidados), `cells=1` se ignora, se usa la ruta CPU y las estadísticas dan `cellPath: "needsBuildWorker"`. Motivo: construir las celdas en el worker de render llevó el p95 del frame a 100 ms mientras llegaban nodos, frente a 18 ms con el worker (T10c2). El camino en proceso sigue para los tests nativos |
| R3-Q25 | (13-11-2026) Tarea 12: en los modos de intensidad y elevación, el color final de la ruta de celdas puede diferir en ≤ 1 nivel de 8 bits (1 LSB) por canal de la ruta CPU; RGB y el resto de modos siguen bit a bit. Motivo: la instancia de 16 B (ADR 0035) guarda la intensidad como media `u16` redondeada y la elevación sin `center.z` absoluto ([consecuencia en la Tarea 6](#tarea-6-instancia-y-tabla-de-páginas)) |
| R3-Q26 | (13-11-2026) Pérdida de dispositivo WebGPU (TW6 de la [revisión](../records/benchmarks/REVIEW_TECHNIQUES_13-11-2026.md#carga-web-y-api)): salvo `destroy()`, el visor pide adaptador y dispositivo nuevos, recrea los recursos y vuelve a subir la escena desde el worker de construcción, sin recargar el dataset y conservando la cámara. Se prueba con la caída del proceso GPU de Chrome |
| R3-Q27 | (13-11-2026) Olvido en bucle hallado en la Tarea 11 (Majadas a 1 px dentro de una hoja: las celdas llenan los 512 MiB del presupuesto de CPU en wasm; en nativo, la hoja +x a 1 px ocupa 824 MiB según T9): el corte deseado se ajusta al presupuesto de CPU engrosando los nodos lejanos, como hace el [ADR 0033](../adr/0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md) con el pool de GPU, y el driver nunca olvida nodos del corte vigente. La histéresis sola no basta porque el conjunto de trabajo no cabe. Se recoge en el [ADR 0037](../adr/0037-corte-ajustado-al-presupuesto-de-cpu.md) |
| R3-Q28 | (14-11-2026) La causa del bucle de R3-Q27 es la caché de bloques (256 MiB en el navegador): el plan exige los bloques decodificados de cada nodo del corte. En la ruta de celdas, un nodo con celdas cuenta como residente aunque sus bloques salgan de la caché ([ADR 0038](../adr/0038-residencia-por-celdas.md)) |
| R3-Q29 | (14-11-2026) Corte que no cabe en el pool de GPU ni con los niveles más gruesos (Belvedere a 1 px tras un salto): pool de 512 MiB y corte de nodos ajustado también al pool ([ADR 0040](../adr/0040-pool-de-512-mib-y-corte-ajustado-al-pool.md)) |
| R3-Q30 | (14-11-2026) Se confirman las reglas de B1c que el ADR 0037 no detallaba: hijos en curso protegidos y contados, bloqueo de todos los candidatos y bloqueo retenido mientras no cambie la selección ([ADR 0039](../adr/0039-hijos-en-curso-y-bloqueo-retenido.md)) |
| R3-Q31 | (14-11-2026) El criterio de ≤ 16,7 ms de la Tarea 16b en el visor wasm se mide con el tiempo de GPU del frame (consultas de timestamp de WebGPU), p95 ≤ 16,7 ms; el intervalo entre frames está atado al vsync y no sirve para ese criterio |
| R3-Q32 | (14-11-2026) Paridad de color de la Tarea 12: con las mismas entradas por celda o vóxel (RGB, intensidad, clase, z) y los mismos rangos, el shader de celdas da el mismo color que el de la ruta CPU, bit a bit en RGB y clasificación y con ≤ 1 LSB en intensidad y elevación (R3-Q25). No se exige la misma imagen: las dos rutas no voxelizan con la misma rejilla |
| R3-Q33 | (14-11-2026) Los modos `attribute:N:M` no caben en la instancia de 16 B: al elegirlos, las celdas se reconstruyen con el RGB pintado por el atributo, cada cambio sube `color_gen` y las celdas de generaciones anteriores se descartan; mientras llegan se dibuja de grueso a fino sin huecos. Sustituye para estos modos la vuelta a la ruta CPU de R3-Q23 |

## Restricciones globales

- `scripts/check.sh` en verde al cerrar cada tarea.
- Solo cambia el formato en lo que exige R3-Q4.
- La ruta CPU se conserva: es el oráculo, y la que usan Additive, v5 y `v7.1–3`.
- Las tareas de la serie ★ tocan `lod_dataset.rs` y `bindings/{lod,builder}.rs`: se hacen de una en una.
- Los cortes se comparan por (nivel, prefijo). La clave del pool es (sesión, nodo, profundidad, generación de color).
- La línea base CPU sale de la misma compilación.
- Cambios en `examples/web-host`: verificación con Playwright.

## Criterios de parada

- **T1:** alguna hoja con posiciones finitas distintas sin separar en la profundidad absoluta 63 → parar (R3-Q7).
- **T4:** el corte ocupa más de 256 MiB, o en la pose rasante hay más de 2× las instancias en pantalla de la ruta CPU a 2–8 px, o más de 2,5× a 1 px (R3-Q12) → replantear y volver al propietario.
- **T4/T9:** `DescendantTooFine` en más del 5 % de las actualizaciones de corte → rediseñar la máscara. Revisado el 05-11-2026 ([ADR 0033](../adr/0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md)): 16,7 % solo en saltos de pose; se retiene el corte (R3-Q10). Vuelve a aplicar si aparece con navegación continua.
- **T5:** más de 16,7 ms de GPU en Majadas dentro de una hoja a 4 px, con el `draw` por rango → revisar ADR 0019.
- **T11:** cualquiera de estos → parar:
  - racha de `RetainedPrevious` de más de 1 s;
  - `poolOverflow` > 0;
  - zoom de 100 ms o más;
  - más de 1 GiB de GPU o GTT.

## Tarea 0: línea base y fixtures

- [x] Nueva línea base de la ruta CPU sobre v7.5: Majadas a 4 px y Belvedere a 4 y 8 px, 3 rondas. [Matriz independiente](../records/benchmarks/R3_T0_CPU_04-11-2026.md): 9/9 muestras; no mezcla el [registro parcial anterior](../records/benchmarks/R3_T0_PARTIAL_04-11-2026.md), capturado en otro entorno.
- [x] Fixtures en ambas versiones, v7.4 y v7.5, con estos casos ([evidencia y límites](../records/benchmarks/R3_T0_PARTIAL_04-11-2026.md)):
  - nube con CRS y raíz en nivel 0;
  - nube local;
  - hoja de nivel 2 con profundidad 16.
- [x] **Pasa si** las medidas quedan en un registro fechado y todos los fixtures se abren en nativo y en la web real. En v7.5 se comprueban la carga diferida de jerarquía y el injerto de páginas; v7.4 conserva la ruta de compatibilidad con índice completo (criterio confirmado el 04-11-2026). Evidencia: [matriz CPU](../records/benchmarks/R3_T0_CPU_04-11-2026.md), [anfitrión web real](../records/benchmarks/R3_T0_WEB_REAL_04-11-2026.md) y [prueba nativa](../records/benchmarks/R3_T0_PARTIAL_04-11-2026.md).

## Tarea 0b: espaciado de referencia en v7.5 (R3-Q4)

- [x] Nuevo campo en el bloque de metadatos de `v7.5-pre` (spec, códec, conversor y `verify`), con [semillas de fuzz y SHA nuevos](../records/benchmarks/R5_V75_REFERENCE_SHA_03-11-2026.md).
- [x] Regenerar `target/render-data/*-v75`.
- [x] **Pasa si** hay ida y vuelta, paridad web/CLI y el valor es estable al injertar páginas.

**Evidencia (04-11-2026, rama `feat/r3-v75`):** tests de formato y jerarquía; para el mismo fixture, el lector nativo obtiene `0,03125` y el visor WebAssembly en Chromium obtiene el mismo valor antes y después de un injerto ([probe y comandos](../../examples/web-host/tests/reference-spacing.browser.mjs)). Belvedere y Majadas se regeneraron y pasaron `viewer_convert verify` en `target/render-data/` del worktree aislado; los originales del checkout principal se conservaron. La expulsión solo se probó en tests nativos; no se verificó GPU. `scripts/check.sh --offline` pasó los 24 pasos sin omisiones, incluidos fuzz, bindings y paridad con Belvedere real ([registro](../records/benchmarks/R3_T0_PARTIAL_04-11-2026.md)).

## Tarea 1 (reabierta): celdas por nodo

**Hecha (05-11-2026):** [aceptación en Belvedere y Majadas](../records/benchmarks/R3_T1_ACCEPT_05-11-2026.md): ninguna hoja sin separar.

**Parada (04-11-2026):** Majadas presenta 16 hojas con colisiones entre posiciones `f32` distintas en el límite de profundidad; resuelta por el [ADR 0032](../adr/0032-hojas-adaptativas-e-instancia-de-12-b.md) (05-11-2026); T1 continúa con el código de `experiment/r3-layout-narrow`. [Diagnóstico, reproducción y límites](../records/benchmarks/R3_T1_STOP_04-11-2026.md); [salida completa del benchmark repetido](../records/benchmarks/R3_T1_BENCH_04-11-2026.md).

- [x] Continuación adaptativa de hojas y clave regional más allá de 21 bits (R3-Q7).
- [x] Claves `u64`, Morton de 64 bits relativo al nodo hasta 21 bits, firma `&[PointBlock]`, `intensity_count` y clase `i16` (−1 si falta).
- [x] Pirámide interna de +3 a +8, podada. Las hojas se reconocen por `has_children` = 0: lo decide quien llama (`is_leaf`); hoy el bench, y `LodDataset` en la Tarea 9.
- [x] Bench sobre todos los nodos: celdas por nivel, bytes y hojas sin separar.
- [x] **Pasa si:**
  - ninguna hoja queda con posiciones distintas sin separar;
  - las sumas coinciden con `accumulate_block`;
  - el color es igual al de la ruta CPU.

## Tarea 2: oráculo de refinamiento

**Hecha (05-11-2026):** `refinement_tests.rs` pasa 32 casos × 2 elecciones de nivel (43.682 vóxeles CPU; 0 con error ∞; 23.179 por densidad de la fuente, R3-Q9) y los 48 dorados no cambian.

- [x] `refinement_relation_holds` con las precondiciones de A8, ahora con pirámide:
  - casos: fixtures de `scene_digest_tests.rs`, cortes Replacement sin mezcla (raíz e hijos) × 4 cámaras × {1, 2, 4, 8} px; los cortes mixtos esperan a la máscara de T3 y Additive no usa la ruta de celdas;
  - por nodo, el nivel guardado más grueso ≥ la profundidad CPU (RQ-09) y también el más fino de la pirámide;
  - relaciones: toda celda nueva cae en una sola celda CPU; toda celda CPU contiene alguna; por celda CPU, Σ rgb, Σ recuentos y Σ `intensity_count` exactos; sin solapes;
  - precondiciones (A8): solo celdas dentro del frustum y cámaras fuera de los nodos. Divergencias conocidas, lista cerrada, que se excluyen y se cuentan: vóxeles CPU con una esquina detrás de la cámara (error ∞, `camera.rs:470`) junto con las celdas nuevas que contienen, y celdas del nivel más fino de una hoja separada que contienen exactamente un vóxel CPU con sus mismas sumas (R3-Q9). Al sumar, cada vóxel CPU visible cuenta todas las celdas nuevas que contiene, también las que quedan fuera del frustum.
- [x] **Pasa si** se cumple sin excepciones nuevas y los dorados existentes no cambian.

## Tarea 3: `cell_cut`

**Hecha (05-11-2026):** `cell_cut.rs`. El volumen visible coincide con el cubierto, sin solapes, en 156 casos (fixtures dorados, incluidos cortes mixtos y un hijo sin residencia, y casos de T0: origen CRS a 10⁶, nube local y hoja de nivel 2 a profundidad 16), contra un oráculo independiente que se comprobó que falla sin máscara. 48 dorados de `cell_cut_digest`. Una etapa es un nivel dibujado más grueso que el deseado.

- [x] Elegir el nivel de cada nodo (`wanted_depth`): el primer nivel guardado cuyo error proyectado cumple el objetivo de px; si ninguno lo cumple, el más fino (R3-Q9). Error infinito (cámara dentro del nodo) → el más fino, sin NaN.
- [x] Nivel dibujado (`resident_depth`, R3-Q2 y R3-Q3): el deseado si es residente; si no, el residente más grueso como etapa; sin ninguno, el nodo no se dibuja.
- [x] Máscara de 8³ por nodo, construida solo con descendientes residentes. `DescendantTooFine` para descendientes más allá de +3.
- [x] `cell_cut_digest` con dorados sobre los fixtures de `scene_digest_tests.rs`. Los modos de px y de huecos (`GapMode`) quedan para la Tarea 14.
- [x] Una profundidad podada se resuelve con la menor guardada ≥ la deseada.
- [x] **Pasa si** el volumen visible es igual al cubierto, sin solapes, en los fixtures de T0.

## Tarea 4: spike de cobertura (v7.5)

**Parada (05-11-2026):** saltan los criterios de T4 y el disparador de `DescendantTooFine` ([registro](../records/benchmarks/R3_T4_COVERAGE_05-11-2026.md)): Majadas pasa de 256 MiB desde dentro de una hoja a 1 px, la pirámide da 2–4,6× las instancias de la CPU en la pose rasante de Majadas, y el 16,7 % de las transiciones (todas saltos de pose) dan `DescendantTooFine`. Causa principal: `projected_error_pixels` es una cota superior. Vuelve al propietario. Medidas nativas hechas; los píxeles vacíos quedan para después de la Tarea 7.

**Alternativas (05-11-2026):** con métrica ajustada y nivel por octante, el corte cabe en 256 MiB y la razón frente a la CPU baja a 1,00–1,01× salvo 2,27× a 1 px en Majadas ([registro](../records/benchmarks/R3_T4_METRIC_05-11-2026.md)). Decidido el 05-11-2026: R3-Q12 y R3-Q13 ([ADR 0034](../adr/0034-nivel-por-octante-con-arista-real.md); [celdas por encima del objetivo](../records/benchmarks/R3_T4_OFFAXIS_05-11-2026.md)).

- [x] Majadas y Belvedere en órbita, en pose rasante y desde dentro de una hoja, a 1, 2, 4 y 8 px. Comparar la pirámide con 256³ solo.
- [x] Registrar (píxeles vacíos pendientes del render de la Tarea 7):
  - instancias totales y en pantalla;
  - píxeles vacíos (pendiente, abajo);
  - bytes del corte;
  - frecuencia de `DescendantTooFine`.
- [x] **Pasa si** no salta ningún criterio de parada de T4: pasa con el nivel por octante y la arista real (R3-Q12) en los dos datasets. El de `DescendantTooFine` (16,7 %, solo en saltos de pose) se revisó en el [ADR 0033](../adr/0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md) (R3-Q10).
- [ ] Píxeles vacíos, cuando exista el render (Tarea 7).

## Tarea 5: spike de GPU

**Parada (05-11-2026), decidida en el [ADR 0035](../adr/0035-instancia-de-16-b-con-w-16.md) (16 B con W = 16 y Tarea 16 antes de la 6):** [registro](../records/benchmarks/R3_GPU_SPIKE_05-11-2026.md). Belvedere dentro de una hoja a 4 px pasa (7,66 ms), pero Majadas a 4 px tarda 27,96 ms (> 16,7 ms) y la instancia de 12 B cuesta un 34 % más que la de 16 B. Dentro de una hoja solo se ve el 0,75–4 % de las celdas dibujadas.

Mide en la GPU de referencia (Radeon 880M, Chrome con WebGPU) el coste de dibujar los cortes reales de la Tarea 4 con la instancia de 12 B. Especificación del 05-11-2026, revisada con su pre-mortem. El código es desechable: la decodificación exacta es de la Tarea 6 y el pool de la Tarea 7.

**T5a, tiempos (decide):**

- [x] **Exportación** (`coverage_spike --export DIR`), para la variante `octant_tight`:
  - por (nodo, nivel) de `required_levels()`, el nivel **completo**, empaquetado como el pool: páginas fijas de 8 KiB y reparto voraz en orden Morton (página nueva si el rango supera 2^W en algún eje o la página se llena);
  - dos empaquetados: W = 10 con instancia de 12 B (682 huecos por página) y el control W = 16 con 16 B (512 huecos);
  - rangos de dibujo = `octant_ranges` sobre esos niveles, en el espacio de huecos;
  - el shader de vértices descarta las celdas tapadas por la máscara 8³ y las de octantes que dibujan otro nivel;
  - un control con la `InstanceRaw` de 40 B de la ruta actual, con las mismas celdas;
  - en el registro: instancias por variante, fracción de huecos vacíos y suma de rangos frente a celdas dibujadas;
  - error máximo en píxeles de origen de página `f32` + coordenada relativa × lado frente a `f64`, en celdas de muestra que incluyan las hojas más profundas.
- [x] **Página** `examples/web-host/spike-cube-pool.{html,mjs}`, WebGPU sin wasm:
  - mismo pipeline que `gpu.rs`: cubo indexado con `CUBE_INDICES`, `Depth32Float` con `Less`, culling de caras traseras, mezcla alfa y el sombreado de fragmentos de `scene.rs`;
  - destino fuera de pantalla de 1280×720, cuyo tamaño se comprueba;
  - `requiredLimits` explícitos para los buffers grandes; se registran los límites concedidos y se falla con un mensaje claro si no bastan;
  - variantes: `draw` por rango; `draw` único sobre todos los huecos usados («cota inferior compactada»); 12 B; 16 B; 40 B;
  - tiempo de GPU con `timestamp-query`, guardando los ticks crudos y su máximo común divisor, contrastado con `onSubmittedWorkDone`; tiempo de CPU de codificar y enviar por frame y por `draw`;
  - calentamiento de al menos 2 s, variantes intercaladas dentro de cada ronda y la serie completa por frame, no solo la mediana;
  - una variante con la cámara en movimiento leve y el corte fijo, que reescribe la tabla de páginas en cada frame y mide el `writeBuffer`.
- [x] **Sonda** `examples/web-host/probe-cube-pool.mjs`: lanza Chrome como `probe-orbit-load.mjs`, añadiendo `--enable-webgpu-developer-features` y rechazando adaptadores de software. Sirve el repositorio por HTTP, hace 3 rondas y registra antes y después la alimentación, el perfil de energía, `pp_dpm_sclk`, la temperatura y `gpu_busy_percent`.
- [x] **Registro** `R3_GPU_SPIKE_dd-mm-aaaa.md` con mediana y rango por caso y variante.
- [x] **Casos:** Belvedere y Majadas a 1 y 4 px en órbita, rasante y dentro de una hoja. Órbita y rasante dibujan pocas instancias y sirven solo de comprobación; deciden los casos «dentro de una hoja».
- [x] **Reglas** (sobre el `draw` por rango, fiel al pool), evaluadas: la de Belvedere pasa; la de Majadas y la de 12 B fallan (decidido en el ADR 0035):
  - Belvedere dentro de una hoja a 4 px: ≤ 8 ms → se queda el `draw` por nodo; si solo el `draw` único cumple → la Tarea 16 pasa a obligatoria;
  - Majadas dentro de una hoja a 4 px: > 16,7 ms → criterio de parada de T5 (revisar ADR 0019). Majadas dentro de una hoja a 1 px queda como prueba de carga, porque roza el pool;
  - 12 B frente a 16 B: se mantiene la de 12 B si no cuesta más de un 10 % de GPU. La regla solo se aplica a casos de ≥ 2 ms y si la variación A/A de una variante es ≤ 5 %;
  - CPU de codificación > 2 ms por frame con unos 1.000 rangos → revisar el `draw` por nodo antes de la Tarea 7.
- **Parar si:**
  - los ticks salen cuantizados a ~100 µs incluso con las funciones de desarrollador; entonces no se aplica la regla del 10 % y se vuelve al propietario;
  - la variación A/A supera el 5 %; antes de dar veredicto hay que fijar el estado de energía;
  - el spike necesita la decodificación de la Tarea 6 o la residencia de la Tarea 7 para dar cifras; entonces se replantea su alcance.

**T5b, diagnóstico para OQ-08 (no decide):**

- [x] *Overdraw* con consultas de oclusión (muestras que pasan) en el pase con prueba de profundidad, y una cuenta sin prueba de profundidad. Se registra el orden de dibujo.
- [x] Fracción de celdas visibles con un pase de identificadores de hueco global en `r32uint` (comprobando que `instance_index` incluye `firstInstance`). Límite documentado: como mucho 921.600 identificadores distintos, y los cubos de menos de un píxel se pierden.

## Tarea 6: instancia y tabla de páginas

Instancia de 16 B con W = 16 (R3-Q14, [ADR 0035](../adr/0035-instancia-de-16-b-con-w-16.md)) y tabla de páginas con caja ajustada (R3-Q16, [ADR 0036](../adr/0036-hi-z-por-celda.md)). Especificación del 05-11-2026, enmendada con su pre-mortem. Solo empaqueta y decodifica: subir el pool, la residencia y los pases son de la Tarea 7.

**Hecha (05-11-2026):** [aceptación](../records/benchmarks/R3_T6_ACCEPT_05-11-2026.md). Decodificación bit a bit en la Radeon 880M (Vulkan y GL) y en llvmpipe; mismas páginas que el spike en los 22 casos; error ≤ 0,0006 px con el origen relativo al ojo.

- [x] **Módulo** `crates/viewer_render_wgpu/src/cell_pages.rs`, privado y reexportado desde `lib.rs` como `node_cells`, para que lo use el spike, con sus tests en `cell_pages_tests.rs`. Entrada: un `NodeCellLevel` de `node_cells.rs`, con su nodo, su `CellGrid` y el rango del nodo (`node_range`). Salida: las instancias, ya repartidas en páginas de 512 huecos con los vacíos rellenos, y los datos de cada página en la CPU.
- [x] **Instancia** (`CellInstance`, 4 × `u32`, `Pod`):

  | Palabra | Contenido |
  | --- | --- |
  | 0 | x `u16` (bits 0–15) e y `u16` (16–31), relativos al origen de la página |
  | 1 | z `u16` (0–15) y flags `u16` (16–31) |
  | 2 | rgba8 en sRGB, r en el byte bajo y a = 255: media redondeada, `(suma + n/2) / n` |
  | 3 | intensidad `u16` (0–15): `intensity_sum / intensity_count` pasada a `f32` como la ruta CPU y después redondeada y limitada a 0–65 534; 0xFFFF si falta o si la media es negativa, como la ruta CPU (`scene.rs:433`); clase `i16` (16–31), −1 si falta |

  - Hueco vacío: palabras 0 y 1 a `u32::MAX`, 2 y 3 a 0. El bit 15 de los flags siempre es 0, así que una celda nunca coincide con el centinela.
- [x] **Flags de la instancia:**
  - bits 0–8: la celda 8³ de la máscara del nodo (`x + 8y + 64z`), calculada en la CPU con aritmética entera desde la coordenada absoluta. Por eje, `m = min((coord − node_range(d).lo) >> (d − nodo − 3), 7)` si `d ≥ nodo + 3`, y `(coord − lo) << (nodo + 3 − d)`, también limitado a 7, si no. Es el mismo límite que `octant_offsets` (`node_cells.rs:253`). Sirve para cualquier profundidad, también las que pasan de 2³² celdas por eje con la clave regional ([ADR 0032](../adr/0032-hojas-adaptativas-e-instancia-de-12-b.md));
  - los niveles con `d < nodo + 3` solo existen en hojas, porque los nodos internos guardan de nodo + 3 a nodo + 8 (`node_cells.rs:181-196`). Una hoja no tiene descendientes en el corte y su máscara es 0. La Tarea 7 da error si un nodo con máscara ≠ 0 tiene uno de esos niveles;
  - bit 9: la celda abarca más de un octante (nivel con profundidad ≤ la del nodo). El shader no hace entonces las pruebas de octante ni de máscara;
  - bits 10–15: reservados, a 0.
  - El octante sale del índice: `4·(mx >> 2) + 2·(my >> 2) + (mz >> 2)`, el orden de la Tarea 15.
  - Con esto, la página ya no necesita el origen de celda relativo al nodo ni el desplazamiento de máscara del spike.
- [x] **Reparto en páginas:** voraz, en el orden de `cells`, que es Morton también con la clave regional (`node_cells.rs:409-416`). Se abre una página nueva si la actual tiene 512 celdas o si el rango de algún eje pasaría de 2¹⁶. La regla del ADR 0035 de partir al cambiar los flags de página no llega a actuar, porque son constantes por nivel (siguiente punto). El origen de la página es el mínimo por eje de sus celdas, y la caja ajustada es su máximo relativo (`u16` × 3).
- [x] **Flags de página:** `has_intensity` y `has_class`, el OR de las celdas del nivel. Son constantes por (nodo, nivel), así que nunca parten páginas; cada celda conserva su centinela (0xFFFF, −1).
- [x] **`PageEntry` en GPU** (8 × `u32`, 32 B, `Pod`):

  | Palabra | Contenido | Se reescribe |
  | --- | --- | --- |
  | 0–2 | origen `f32` de la esquina mínima de la página, **relativo al ojo**: `(root_min + coord × lado − coordinate_origin) × visual_scale − ojo`, con el ojo escalado en `f64` de la cámara. Relativo al objetivo, como `scene.rs`, el error llegaba a 0,079 px en Majadas dentro de una hoja mirando a −z (ver «Parar si») | con la cámara |
  | 3 | lado de celda × `visual_scale`, `f32` | con la cámara, si cambia la escala |
  | 4 | índice del nodo en la tabla de máscaras (16 × `u32` por nodo, bit `x + 8y + 64z`) | con la residencia (T7) |
  | 5 | bits de los octantes que dibujan este nivel (0–7) | con el corte (T7) |
  | 6 | caja: máximo relativo x (0–15) e y (16–31) | nunca |
  | 7 | caja: máximo relativo z (0–15); flags de página (16–31: bit 0 `has_intensity`, bit 1 `has_class`) | nunca |

  - La CPU guarda por página el nodo, la profundidad, el origen absoluto (`u64` × 3), el número de celdas y la caja, para el picking y para reescribir el origen.
- [x] **Decodificación**, idéntica en Rust y en WGSL, en una constante WGSL compartida (`CELL_PAGES_WGSL`) que usará la Tarea 7:
  - esquinas `lo = fma(vec3<f32>(rel), lado, origen)` y `hi = fma(vec3<f32>(rel) + 1.0, lado, origen)`, con `f32::mul_add` en Rust. El `fma` explícito es obligatorio: en la Radeon 880M la expresión sin él sale fusionada y en llvmpipe no (sonda del pre-mortem, 100.000 casos: `fma()` = `mul_add` en los dos adaptadores);
  - el vértice del cubo elige `lo` o `hi` por eje, y el pase de huecos de la Tarea 7 prueba el frustum y el Hi-Z con las mismas `lo` y `hi`. Así la caja que se prueba es la que se rasteriza;
  - color: sRGB a lineal con una tabla de 256 `f32`, generada desde `ColorRgb::to_linear_rgba`, en un uniform. Es exacta y no ocupa búfer de almacenamiento.
- [x] **Tests:**
  - ida y vuelta: cada celda del nivel aparece en un solo hueco; origen de página + relativo = coordenada de la celda; color, intensidad y clase como sus medias;
  - reparto: corte a 512, corte por un rango de 2¹⁶ + 1 en un eje, y los flags de página no parten páginas;
  - flags: el índice de máscara y el octante coinciden con un oráculo que parte de la coordenada absoluta y de `node_range`, en niveles con profundidad nodo + 1, nodo + 2, nodo + 3, nodo + 8 y una más allá de 2³² celdas por eje; y el bit 9 en los niveles que abarcan octantes;
  - marco: para las mismas celdas, `lo` y `hi` más `ojo − objetivo` coinciden (en `f64`) con la caja que da la fórmula de `scene.rs` a una `InstanceRaw` de esa celda;
  - precisión: raíz en nivel 0, dataset con origen a 10⁶ m y `visual_scale` ≠ 1. El error de `lo` y `hi` en `f32` frente a `f64` no pasa de 4 ulp de `max(|origen|, (caja + 1) × lado)`. Además, ≤ 0,01 px con la geometría de la pose dentro de una hoja de la Tarea 4 (ojo dentro de la hoja más profunda y objetivo a `root_side / 16`), proyectando con una matriz de vista con el ojo en el origen;
  - octante: en cada celda, el octante que sale del índice coincide con el de `octant_offsets`;
  - WGSL: `naga` valida la constante. Un test de cómputo decodifica ≥ 10.000 instancias (con los extremos: relativo 65 535, clases −1, `i16::MIN` y `i16::MAX`, intensidad 0, 65 534 y ausente) y compara con Rust en cada adaptador disponible, real y de software. Muestra el nombre de cada adaptador. Sin adaptador real, lo dice y no falla, salvo con `CELL_PAGES_REQUIRE_GPU=1`;
  - datos reales: `coverage_spike --check-cell-pages` empaqueta con el módulo los niveles de los cortes exportados de las Tareas 5 y 16. Exige las mismas páginas y los mismos huecos que la exportación del spike, y las mismas palabras 0, 2 y 3 y la misma z. Registra la fracción de huecos vacíos.
- [x] **Pasa si:**
  - los campos enteros, `lo`, `hi` y el color lineal salen iguales bit a bit en Rust y en WGSL, en la Radeon 880M (`CELL_PAGES_REQUIRE_GPU=1`) y en llvmpipe. Chrome (Tint) queda fuera: lo comprueba el humo de la Tarea 11;
  - se cumplen los demás tests y `check.sh`.
- **Parar si:**
  - `fma()` no coincide con `mul_add` bit a bit en la Radeon 880M → vuelve al propietario;
  - el octante del índice no coincide con `octant_offsets` en alguna celda;
  - con los cortes reales de más de 1.000 páginas, más del 2 % de huecos vacíos, o más de 1,05 × las páginas del spike → replantear el reparto. En los cortes pequeños, el último hueco parcial de cada (nodo, nivel) pesa más en porcentaje, pero ocupa poco: el peor, Majadas en órbita a 4 px, tiene 12 páginas (96 KiB) con un 33,7 % vacío, unos 32 KiB;
  - más de 0,01 px de error en el marco del objetivo → volver a orígenes relativos al ojo, lo que cambia la matriz de vista en la Tarea 7. **Saltó el 05-11-2026:** relativo al objetivo, 0,051 px en Majadas dentro de una hoja (celdas a 0,35 unidades del ojo) y 0,079 px mirando a −z (0,23 unidades); Belvedere, 0,012–0,023 px. Se pasa al ojo: la Tarea 7 dibuja las celdas con una matriz de vista con el ojo en el origen;
  - más de 0,01 px también relativo al ojo → vuelve al propietario.
- **Consecuencia para la Tarea 12:** la intensidad pasa a una media `u16` redondeada (hasta 0,5 de diferencia con la `f32` de la ruta CPU, `voxel_grid.rs:1068-1073`), y la elevación ya no tiene el `center.z` absoluto (`scene.rs:436`). Choca con el «bit a bit con la ruta CPU» de la Tarea 12 en los modos de intensidad y elevación. **Pregunta para el propietario** antes de la Tarea 12: tolerancia de 1 LSB de 8 bits (propuesta), o intensidad en `f32`, que no cabe en la instancia de 16 B.

## Tarea 7: pool en `SurfaceRenderer`

Residencia de celdas en GPU y su dibujo con compactación y Hi-Z por celda (R3-Q2, R3-Q3, R3-Q10, R3-Q11, R3-Q16), sobre la instancia y las páginas de la Tarea 6. Incluye la Tarea 16b. Especificación del 05-11-2026, enmendada con su pre-mortem. Se hace en cuatro partes, en este orden, y cada una se commitea por separado. Nadie produce aún `NodeCells` fuera de tests y ejemplos: la carga desde el dataset es de la Tarea 9.

**T7b1, pool estático en GPU y dibujo sin oclusión** (`gpu.rs`, `camera.rs` y un módulo nuevo `cell_render.rs`):

**Hecha (05-11-2026).** 9 tests en `cell_render_tests.rs`, en la Radeon 880M (RADV) y en llvmpipe. El adaptador GL se salta porque no puede leer la profundidad. El oráculo por píxel da 0 diferencias fuera de los bordes y el mismo número de píxeles cubiertos; en la escena de cubos grandes, el 90 % de los píxeles es interior. Tres mutaciones (cubos 0,5 px más grandes, sin prueba de octante y sin la guarda de página inerte) hacen fallar los tests. Diferencias con lo especificado:
- la máscara se reserva por (nodo, nivel), y la asignación por nodo queda para T7a;
- el pase de páginas recorre los huecos hasta el más alto usado, en vez de una lista de activas;
- llvmpipe no da los 256 MiB por binding, así que su test usa un pool de 64 páginas;
- mientras existe el pool, el picking devuelve un error explícito hasta la Tarea 8.
- **las dos rutas son excluyentes** (segunda revisión): solo `upload_cell_level` crea el pool; si esa primera subida falla, el pool se descarta y la escena CPU sigue intacta; si sale bien, se libera la escena CPU. Subir una escena CPU libera el pool;
- sin test sobre un `SurfaceRenderer` real, que necesita una superficie: que tras la primera subida la escena CPU quede liberada y que las llamadas sin pool den `NoPool`. Los cubren dos tests de las funciones auxiliares y la compilación; se probarán en el humo de la Tarea 11.

- [x] **Soporte:** `SurfaceRenderer::new` pide, sin pasar de lo que da el adaptador, `max_storage_buffers_per_shader_stage` ≥ 8 y `max_storage_buffer_binding_size` y `max_buffer_size` ≥ los bytes del pool ([ADR 0036](../adr/0036-hi-z-por-celda.md)). Además exige cómputo, dibujo indirecto, almacenamiento en la etapa de vértices y texturas de almacenamiento `r32float` (`DownlevelFlags`). Si algo falta, `cells_supported() = false` (R3-Q5).
- [x] **La ruta CPU no cambia:** sus comprobaciones de tamaño (`gpu.rs:1364`, `hzb.rs:263-265`) siguen usando los límites de `downlevel_defaults`. Los límites se pueden inyectar en `RendererConfig`, para el test.
- [x] **Pool perezoso:** se crea al recibir el primer nivel, no en `new()`, y se libera al volver a la ruta CPU. Así sus bytes no cuentan contra el presupuesto de las escenas CPU (`budget.rs:17`).
- [x] **Cámara:** `OrbitCamera` da una vista-proyección con el ojo en el origen (`look_at` desde 0 hacia `−offset`) y el ojo escalado en `f64`, para `PageFrame` (Tarea 6).
- [x] **Búferes:**
  - instancias (páginas × 512 × 16 B, con `COPY_SRC` para el picking de la Tarea 8);
  - tabla de páginas (32 B por página);
  - máscaras (16 × `u32` por nodo, con su propia lista libre);
  - tabla sRGB en un uniform;
  - índices visibles (un `u32` por hueco) y argumentos indirectos.
  - Todo cuenta en `allocated_gpu_bytes` y en `GpuResourceStats`.
- [x] **API mínima:** subir un `PackedLevel` a unas páginas dadas, liberar páginas, fijar la máscara de un nodo, y por frame la lista de páginas activas con sus bits de octante. Todas las escrituras van con `queue.write_buffer`. Orden en cada frame: expulsiones de lo que no se dibuja → subidas → tabla de páginas → envío.
- [x] **Pase `k`** (como la Tarea 16a): páginas con frustum; huecos con vacíos, octante, máscara y frustum; un `drawIndexedIndirect`. El frustum prueba las mismas esquinas `lo` y `hi` que dibuja el vértice, con la misma multiplicación por la matriz.
- [x] **Un solo codificador** `encode_cells(encoder, color, depth)`, que comparten `render()` y un arnés headless. Así los tests ejercitan el mismo código que el visor.
- [x] **Convivencia:** el renderer dibuja la ruta CPU o la de celdas, según lo que se le haya subido. El HZB de la ruta CPU no se usa con celdas.
- [x] **Oráculo por píxel:** cada celda con un color único. La imagen de celdas se compara con esas mismas celdas convertidas a `InstanceRaw` y dibujadas por la ruta CPU: mismo identificador en cada píxel salvo en los bordes, con profundidad dentro de una tolerancia derivada y 0 píxeles más profundos. Corre en la 880M y en llvmpipe, y falla si no corre en ninguno (`CELL_POOL_REQUIRE_GPU=1` exige la 880M).
- [x] Con los límites de `downlevel_defaults` inyectados, `cells_supported()` es falso y la ruta CPU dibuja igual que antes.

**T7a, residencia en CPU** (`crates/viewer_render_wgpu/src/cell_pool.rs`, sin GPU):

**Hecha (06-11-2026).** 16 tests con un cargador y una GPU falsos. La GPU falsa falla si se escribe un hueco en uso en el frame actual o en el anterior, si un nivel activo no tiene todas sus páginas o si una máscara no coincide con la de `cell_cut`. Cuatro mutaciones (contadores fijos a 0, sin orden de prioridad, histéresis al 90 % y sin bloqueo por prioridad) hacen fallar los tests. Pendiente de medir con cortes reales en T7c: si la estimación de tamaño recorta de más con la cámara quieta, porque los niveles por encima del límite no se piden y conservan su tamaño estimado.

- [x] **Pool de páginas:** capacidad fija en páginas de 8 KiB (256 MiB = 32.768 páginas, [ADR 0033](../adr/0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md)), con lista libre. Clave de un nivel: (sesión, nodo, profundidad, generación de color).
- [x] **Estados por (nodo, nivel):** pedido → subiendo → residente (R3-Q2: solo cuenta como residente con todas sus páginas). La entrada de datos es asíncrona, `offer(clave, PackedLevel)`, como la entregarán los workers (Tarea 10). Un nivel puede subirse en varios frames, con un presupuesto de bytes por frame. Si sale del corte, se cancela y se liberan sus páginas.
- [x] **El nivel más grueso de cada nodo del corte está siempre residente y fijado**, también el de los nodos con todos los octantes tapados por la máscara. Si un descendiente pierde residencia y la máscara se abre, el antepasado tiene qué dibujar. Se reserva 1/8 del pool para las transiciones.
- [x] **Prioridad de subida:** primero el nivel más grueso de los nodos que no tienen ningún nivel dibujable; después, los deseados, del nodo más cercano al más lejano.
- [x] **Ajuste al pool** (R3-Q11), calculado sobre el **corte deseado**, no sobre la residencia actual: `cell_cut` con `resident = stored`, y la suma de páginas de sus niveles `wanted`.
  - El tamaño de un nivel que aún no ha llegado se estima: `ceil(min(puntos del nodo, 8^(d − nodo)) / 512)` páginas. No es una cota estricta, porque `pack_level` abre página nueva si el rango pasa de 2¹⁶. Al recibirlo con `offer` se usa su tamaño exacto, y el ajuste se corrige en el frame siguiente; la reserva de 1/8 absorbe la diferencia.
  - Si no cabe, se recortan en bloque los nodos más lejanos (distancia del ojo a su caja, con la ordenación hecha una sola vez): se truncan su `stored` y su `resident` por debajo de su `max(wanted)` actual, y se repite hasta que quepa. `cell_cut` no cambia.
  - Histéresis: se recorta por encima del 90 % de la capacidad y se deshace por debajo del 80 %.
  - Cada nodo recortado cuenta en `pool_coarsened`, que impide «objetivo alcanzado» como una etapa.
- [x] **Desbordamiento:** si ni con todos los nodos en su nivel más grueso cabe, se mantiene el último corte completo y se cuenta `pool_overflow`.
- [x] **`DescendantTooFine`** (R3-Q10): `CutApply { Applied, RetainedPrevious }`. Los descendientes culpables solo deciden qué se pide: no se piden niveles que harían el corte `DescendantTooFine`. Se aplica según el corte real, con la residencia actual: cuando `cell_cut` da `Ok` y no queda ningún nodo sin nivel (`nodes_without_level = 0`), o cuando el corte aplicado tampoco estaba completo. Un corte cuyo `cell_cut` real da `DescendantTooFine` se retiene y se cuenta; corregirlo en el selector es de la Tarea 9, y la Tarea 11 para si la retención dura más de 1 s.
- [x] **Expulsión:** LRU entre los niveles que no están fijados ni los usa el corte aplicado o el deseado. Cada expulsión emite `CellPoolEvent::Evict(clave)`, y cada nivel cancelado antes de ser residente, `CellPoolEvent::Cancel(clave)`, para que los workers suelten sus datos (Tarea 10). Las páginas expulsadas vuelven a la lista libre dos frames después del último que las dibujó.
- [x] **Tests:**
  - `overflow_keeps_previous_complete_cut`;
  - `regraft_mid_cut_keeps_coverage`: expulsar y volver a subir un nivel a mitad del corte. Mientras falta, el oráculo de cobertura de la Tarea 15 cubre cada celda fina una sola vez, y la etapa nunca sube;
  - todo o nada, subida repartida en varios frames y cancelación;
  - ajuste por los nodos más lejanos, con los bytes residentes bajando, e histéresis sin oscilar con la cámara quieta;
  - salida de `RetainedPrevious`;
  - orden LRU y eventos de expulsión;
  - `nodes_without_level = 0` tras la primera subida en todos los tests.

**T7b2, oclusión** (Tarea 16b; módulo nuevo `cell_cull.rs`, que sustituye el plan de llevarla a `hzb.rs`):

**Hecha (06-11-2026).** 10 tests de oclusión en la Radeon 880M (RADV) y en llvmpipe, sobre una escena con una envoltura cerrada alrededor de 6.000 celdas ocultas. La profundidad sale idéntica bit a bit con y sin oclusión en cada frame: cámara quieta, 10 frames en movimiento desde frío, una órbita que desocluye más de 100 celdas en la fase 2, la cámara dentro de la envoltura cruzando el plano cercano, reutilización de huecos y redimensionado. El color también es idéntico, salvo en 8 píxeles de empate de profundidad que llvmpipe resuelve distinto en cada ejecución. Con la cámara quieta se dibujan 434 de 7.342 celdas. El margen de profundidad es de 8 u (u = 2⁻²⁴). Medido en la GPU con la misma `cell_clip` del shader y cámaras reales, giradas y oblicuas, sobre más de un millón de píxeles, ningún fragmento queda más de 2,00 u por delante de la esquina más cercana de su celda. El pase de cierre usa los 8 búferes de almacenamiento, así que no queda holgura. Siete mutaciones hacen fallar los tests.

- [x] Pases de `kpc` sobre `k`: Hi-Z sobre la caja ajustada en el pase de páginas, Hi-Z por celda en la fase 2, dos fases y un `drawIndexedIndirect` por fase. Los bits de visibilidad, en doble búfer y por hueco del pool, se ponen a cero cuando se reutiliza un hueco.
- [x] **Hi-Z propio:** cada texel guarda el máximo de todos sus píxeles. No usa la escala de `hzb.rs`, que no es conservadora fuera de potencias de dos. Se rehace al redimensionar y en `Outdated`.
- [x] **Margen de profundidad**, derivado y documentado: el pase y el vértice calculan los mismos `clip`, así que solo queda el redondeo de la interpolación del rasterizador. Si no se puede acotar, se mide el peor caso en los tests y se usa el doble.
- [ ] **Dispositivo perdido:** se vacía la residencia y se emite `Evict` de todo. Pasa a la integración de la Tarea 9, que une `cell_pool` con el renderer; los bits y el Hi-Z son estado derivado que se rehace con el renderer.
- [x] **Tests:**
  - profundidad idéntica con y sin oclusión, en un frame quieto y en 10 frames en movimiento desde un arranque en frío;
  - la fase 1 dibuja algo tras el frame 0 y la fase 2 descarta algo;
  - a mitad de secuencia se expulsa un nivel y otro reutiliza sus huecos.

**T7c, medida nativa** (no decide; referencia para la Tarea 11):

**Hecha (06-11-2026), salta la regla de CPU:** [registro](../records/benchmarks/R3_T7C_NATIVE_06-11-2026.md). En la GPU, `kpc` tarda 3,28 ms en Majadas mirando a +y a 4 px (spike en Chrome: 4,39 ms), y todas las poses quietas convergen en ≤ 15 frames. Pero `CellPool::frame` cuesta hasta 8,75 ms de CPU por frame (Majadas a 1 px), por encima de los 2 ms. Además, la estimación de tamaños, unas 2 veces el tamaño real, recorta 29 nodos sin necesidad con la cámara quieta. Se corrige en T7d.

- [x] `coverage_spike --export-pool DIR` escribe los niveles de los cortes exportados ya empaquetados con `cell_pages`. Un ejemplo nuevo, `cell_pool_bench`, los ofrece al pool, en la 880M y con `timestamp-query`:
  - el corte quieto (los casos de la Tarea 16a);
  - la cámara en movimiento, con expulsiones y subidas;
  - el coste de CPU por frame del corte, el ajuste y la reescritura de la tabla de páginas.
- **Parar si:**
  - `kpc` en nativo tarda más de 2 × el spike (4,39 ms en Majadas mirando a +y): algo no se parece al spike;
  - el corte, el ajuste y la tabla de páginas pasan de 2 ms de CPU por frame en `release` → actualización incremental o en GPU antes de la Tarea 9;
  - con la pose quieta no converge en 120 frames, o siguen las subidas y expulsiones → más histéresis antes de la Tarea 9.

**T7d, coste de CPU y tamaños exactos** (antes de la Tarea 9; especificación del 06-11-2026):

**Hecha (06-11-2026):** [registro](../records/benchmarks/R3_T7D_CPU_06-11-2026.md). Con R3-Q17 y R3-Q18, pasa: ≤ 1,38 ms de CPU por frame en los casos a 4 px, y ya no hay recorte falso. Majadas a 1 px en movimiento, que es la prueba de carga, sigue en 2,76 / 3,34 ms. La GPU de Majadas +y sube de 3,28 a 3,90 ms porque ahora dibuja el corte sin recortar, así que el criterio del 5 % no se aplica ahí.

- [x] **Tamaños exactos sin subir:** `CellPool::set_level_sizes(nodo, &[(profundidad, páginas)])`. En el visor, todos los niveles de un nodo se construyen a la vez desde sus puntos, así que sus tamaños se conocen aunque no se suban. El ajuste usa el tamaño exacto en cuanto lo tiene; la estimación solo queda para los nodos cuyos puntos no han llegado. El banco los entrega al cargar cada nodo.
- [x] **Coste por frame:** como mucho una llamada a `cell_cut` por frame en régimen estable. El corte deseado y el ajuste solo se recalculan si cambia el corte de nodos, si llegan tamaños o residencia nuevos, o si la cámara se mueve más de un umbral documentado; si no, se reutilizan. Antes de cambiar nada, se perfila `cell_cut` con el corte de Majadas a 1 px y se mide cuánto cuesta cada parte.
- [x] **Pasa si:**
  - `cell_pool_bench` da ≤ 2 ms de CPU por frame (mediana y p95), quieta y en movimiento, en los 22 casos, Majadas a 1 px incluida;
  - con la cámara quieta, `pool_coarsened` es 0 donde el corte deseado exacto cabe (Majadas +y a 4 px), y mayor que 0 donde no cabe (Majadas a 1 px);
  - los tests de T7a siguen pasando y se añade uno con tamaños exactos que levanta un recorte falso;
  - los tiempos de GPU de T7c no empeoran más de un 5 %.
- **Parar si** con una sola llamada a `cell_cut` por frame sigue pasando de 2 ms en Majadas a 1 px → `cell_cut` incremental, o en GPU, y vuelve al propietario.

- **Pasa si** se cumplen los tests de las cinco partes, `check.sh` y las medidas de T7c y T7d. **Hecha (06-11-2026)**, salvo la expulsión al perder el dispositivo, que pasa a la Tarea 9. El criterio de la Tarea 16b en el visor wasm (≤ 16,7 ms) se comprueba en la Tarea 11, cuando el visor ya cargue celdas.

## Tarea 8: picking sobre el pool

**Hecha el 06-11-2026.** Spec del 06-11-2026, enmendada tras el pre-mortem. Mantiene la API asíncrona de la ruta CPU (`prepare_pick` copia a un búfer de lectura y `resolve_pick` resuelve sobre los bytes mapeados), así que los bindings no cambian.

- [x] **Dónde vive:** un módulo `cell_pick.rs` con la preparación (candidatas y copias) y la resolución, que toman el rayo en el marco de las entradas. `SurfaceRenderer::prepare_pick` y `resolve_pick` solo lo envuelven. Motivo: `SurfaceRenderer::new` exige una superficie y ningún test la crea.
- [x] **Estado en la CPU:**
  - `CellRenderer` guarda una copia de cada máscara, igual que la GPU: se pone a cero solo al tomar el hueco (`alloc_mask_slot`); liberarlo no la toca.
  - `set_active_cell_pages` recibe además el `PageFrame` de las entradas. Si su `coordinate_origin` o su `visual_scale` no coinciden con los del renderer, da error. El pool guarda el marco de sus páginas (ojo, escala y origen). Si la escala o el origen del renderer cambian antes de reescribirlas, `prepare_pick` da error en lugar de mezclar marcos (hallazgo de la revisión).
  - *Límite conocido:* `render` dibuja con el ojo de la cámara actual. Si la cámara se mueve sin reescribir las páginas, el dibujo se desplaza, pero el picking sigue siendo correcto en el mundo. El host reescribe las páginas en cada frame.
- [x] **`prepare_pick` en la ruta de celdas:**
  - rayo de la cámara actual, pasado al marco de las entradas (origen − ojo guardado);
  - candidatas: páginas activas con algún octante cuya caja ajustada (origen + [0, `box_max` + 1] × lado, ensanchada un ulp) corta el rayo dentro de [near, far];
  - orden por distancia de entrada (desempate por hueco); se copian como mucho 128 páginas (8 KiB cada una; 65.536 huecos, el mismo tope que la ruta CPU). El presupuesto se comprueba con `allocated_gpu_bytes()`, que incluye el pool, más el búfer de lectura;
  - cada copia guarda su hueco, su entrada y la máscara del nodo en ese momento. Si quedan candidatas sin copiar, se guarda la distancia de entrada de la primera (`cutoff`);
  - `PickReadback` pasa a ser un enum (CPU | pool). La variante del pool guarda el ojo, `visual_scale` y `coordinate_origin`, y `resolve_pick` decide por la variante, no por el modo actual, porque entre las dos llamadas hay un `await` en el host.
- [x] **`resolve_pick`:**
  - por cada celda no vacía, los mismos filtros que el pase de huecos: si no abarca octantes, su octante debe estar en los bits de la página y su bit de máscara a cero; sin frustum ni Hi-Z;
  - caja con `decode_cell` (el mismo `fma`), intersección en `f64`; gana la más cercana, y a igual distancia, el hueco global menor;
  - con `cutoff` puesto, error si no hay ganadora o si su distancia es ≥ `cutoff` («el rayo cruza más de 128 páginas antes del primer impacto»): el resultado no sería seguro;
  - `PickHit`: `instance_index` = hueco de página × 512 + hueco; posición, centro, lado y distancia en unidades del dataset, como la ruta CPU.
- [x] **Pasa si** pasa `pick_on_pool_hits_same_cell_as_cpu_path` (GPU real; falla sin adaptador con `CELL_POOL_REQUIRE_GPU=1`, como `cell_render_tests`):
  - un fixture con dos niveles de un nodo, un octante al nivel fino y la máscara del grueso puesta (como la escena de `cell_render_tests`);
  - el oráculo es el picking de la ruta CPU: `build_pick_chunks` y `resolve_pick_instances` (`pub(crate)`) sobre las celdas que dibuja el pool, como `InstanceRaw` (`cpu_instances`);
  - una rejilla de ≥ 32 × 18 píxeles, más píxeles a ±0,1 px de las aristas proyectadas;
  - **misma celda:** el índice del oráculo se traduce a hueco de página × 512 + hueco, con el mismo lado, el centro a ≤ 10⁻³ × lado (las vecinas distan un lado entero) y la distancia a ≤ 2⁻²⁰ · (d + |c|);
  - se excluyen, y se cuentan (≤ 2 %), los píxeles en que las dos mejores distancias del oráculo difieren menos de 10⁻⁵ en relativo;
  - no es vacío: impactos en ≥ 50 % de la rejilla, y alguno en que una celda enmascarada o de otro octante queda delante;
  - mutaciones, que se informan: quitar el filtro de máscara, el de octante, el de huecos vacíos o el atajo de «abarca octantes», usar el ojo actual en lugar del guardado y quitar la comprobación de `cutoff`. Cada una debe hacer fallar algún test.
- [x] Tests de `cutoff` (error si el impacto queda tras la última página copiada o si no hay impacto con páginas sin copiar; acierto si cae en las copiadas) y de mover la cámara sin reescribir las páginas (mismo punto del mundo → misma celda).
- [x] **Parar si**, tras estas tolerancias, el oráculo discrepa en más del 2 % de los píxeles no excluidos: se revisan los filtros o la decodificación antes de tocar tolerancias.
- **Resultado** (GPU real: Radeon 880M con RADV y llvmpipe, cifras idénticas):
  - `pick_on_pool_hits_same_cell_as_cpu_path`: 852 píxeles, 822 impactos, 0 discrepancias y 0 excluidos. Error de centro máximo de 2,6 · 10⁻⁷ × lado, y de distancia, 0,16 de la tolerancia. En 719 píxeles, sin los filtros ganaría otra celda;
  - con la cámara movida sin reescribir las páginas: 812 píxeles, 0 discrepancias;
  - mutaciones detectadas (filtros de máscara, octante, huecos vacíos y atajo de octantes; ojo actual; `cutoff`; índice de corte; comprobación del marco); según el implementador;
  - `check.sh`: 24/24.
- **Sin probar:** el envoltorio de `SurfaceRenderer` (necesita superficie); su lógica está en funciones probadas aparte.

## Tarea 9 ★: `LodDataset` con celdas

**Hecha (06-11-2026).** Spec del 06-11-2026, enmendada tras el pre-mortem y con R3-Q19. Une el pool (Tarea 7) con el dataset en Rust nativo. **El visor recibe las celdas por el worker en la Tarea 10** (decisión del propietario del 06-11-2026): construir un nodo interno cuesta 26–39 ms en nativo ([T1](../records/benchmarks/R3_T1_ACCEPT_05-11-2026.md)), demasiado para el hilo principal. Esta tarea no toca los bindings. Dos partes, cada una con su commit.

**Hechos de partida** (comprobados en el código):
- El plan visible de `LodTransitionPlanner` no es una antichain: mientras cargan los hijos, contiene al padre y a sus hijos directos alineados ya cargados (`resolve_replacement`; test `partial.node_ids() == [1, 2]` en `viewer_core/tests/lod_transition.rs`). El salto es siempre 1, y la máscara de 8³ lo cubre. Por construcción no hay pares `DescendantTooFine`; se comprueba un salto ≤ 3 en cada actualización y se cuentan los `DescendantTooFine` (deben ser 0).
- `offer` rechaza niveles sin páginas, y `CellPool::frame` rechaza nodos sin niveles (`NoStoredLevels`).
- `SurfaceRenderer` suelta el pool en `sync_visible_blocks`, `upload_voxel_cut`, `clear_dataset` y al cambiar el modo de color.

**T9a, pool y driver** (`viewer_render_wgpu`):
- [x] **R3-Q19 en `CellPool`** (hecho el 06-11-2026, T9a1; límite conocido: si un antepasado del corte dibuja solo algunos octantes, el nodo que falta debajo retiene el corte en vez de buscar un respaldo más arriba; no deja huecos, y no se da mientras el nivel más grueso, que cubre todo el nodo, siga fijado):
  - el corte se aplica si cada nodo sin nivel residente tiene un antepasado en el corte que dibuja; si no, se retiene el anterior (R3-Q10);
  - un nodo sin nivel mantiene en el corte su antepasado del último corte aplicado, con salto ≤ 3, hasta que el nodo es residente. Es una etapa contada (`stages`). Así la retención es local;
  - las peticiones y las subidas van de grueso a fino: primero los nodos que no dibujan nada, después la profundidad absoluta y después la distancia. El primer nivel de un hijo va detrás del de su padre, y un nodo vacío no espera tras los refinamientos de otras zonas;
  - los tests de T7a que fijan la regla anterior se cambian. Uno nuevo: con un nodo sin nivel en una zona, otra zona sin relación se aplica en ese mismo frame.
- [x] **`cell_driver.rs`, `CellDriver`** (hecho el 06-11-2026, T9a2):
  - posee el `CellPool` y un `PackedLevel` por (nodo, nivel), compartido con el pool sin copiarlo. Empaqueta al recibir las celdas de un nodo, pasa los tamaños exactos (`set_level_sizes`) y no guarda `NodeCell`. Quita los niveles vacíos de `stored`;
  - tiene un presupuesto de bytes de CPU: olvida, primero los más lejanos, los nodos que no están en el corte ni tienen niveles residentes o pedidos;
  - generaliza el `step` de `cell_pool_bench`: eventos, liberaciones, subidas, máscaras, tabla de páginas con su `PageFrame`. El banco pasa a usar el driver;
  - el renderer va detrás de un trait `CellTarget`, implementado por `SurfaceRenderer` y `CellRenderer`, más el doble de GPU estricto de T7a;
  - `reset()`: residencia vacía (API nueva del pool) y `Evict` de todo. Se llama al perder el dispositivo y siempre que el renderer ya no tenga pool;
  - `needs_frame()`: hay peticiones, subidas o retención pendientes. El visor (Tarea 10) dibuja mientras sea cierto y reescribe la tabla de páginas en cada movimiento de cámara;
  - sin `Instant` (rompe en wasm32).
- [x] **Pasa si:**
  - en GPU real, con el oráculo por píxel de T7b1, el driver dibuja bien con `coordinate_origin` en 10⁶ y `visual_scale` 0,0788;
  - con el doble estricto, tras `reset()` converge al mismo corte y los mismos contadores, sin huecos en ningún frame;
  - el banco da los mismos contadores que antes de pasar al driver, en un caso de T7d.
- **Resultado de T9a** (06-11-2026; según el implementador salvo lo marcado):
  - oráculo por píxel con origen en 10⁶ y escala 0,0788: cobertura exacta en RADV y llvmpipe. El ancla de la rejilla es diádica (`[0,125, −0,25, 0,0625]`). Con anclas no diádicas, la cobertura sigue exacta, pero falla un píxel de borde con una sola muestra de referencia (comprobado);
  - el banco da los mismos contadores antes y después de pasar al driver en Majadas +y a 4 px, y conserva las claves JSON de T7c/T7d. La columna nueva `drv` (el driver entero, con las escrituras en GPU) da 0,50 / 1,10 ms de mediana (quieta / en movimiento) y 2,05 ms de p95 en movimiento: se vigila en la Tarea 11;
  - un error del renderer o una oferta rechazada hacen fallar el frame: el renderer suelta su pool y el siguiente frame empieza de cero.

**T9b, `LodDataset`** (★: `lod_dataset.rs`; los bindings no cambian):
- [x] (T9b1) En la apertura, `LodDataset` sabe si admite celdas: v7.4 y v7.5 con reemplazo, a partir de `uses_global_frame()` y sin literales de versión. v5, `v7.1–3` y Additive dan `CellPathUnsupported` (R3-Q5).
- [x] `update_cells(view, target_px)` (T9b1 y T9b2a; el tope de construcciones mantiene la vista sucia mientras queden nodos aplazados, de forma preventiva: ningún test encontró un caso que se atascara sin ello; en Majadas y Belvedere v7.5 ningún bloque tiene más de un dueño):
  - [x] (T9b2a) con la escalera de px de la ruta CPU (`lod_driver`), compartida con ella, de grueso a fino (R3-Q19, 06-11-2026: no se carga el nivel de un nodo antes que el de sus padres). Cada etapa del corte queda como respaldo retenido de la siguiente, y la etapa avanza cuando su corte tiene las celdas construidas. Sustituye «al px objetivo, sin escalera», que saltaba a los nodos finos sin construir antes sus padres;
  - el plan ve los bloques de un nodo como residentes solo cuando sus celdas existen. Lo hace un `ResidentBlockSource` que envuelve la caché, así que el plan cubre con el respaldo los nodos sin construir y el pool nunca recibe un nodo sin niveles;
  - se construyen como mucho N nodos por actualización, primero los más cercanos;
  - las celdas se construyen en el marco local de los puntos, y el pool y `PageFrame` usan el del mundo, con el `coordinate_origin` del renderer;
  - hoja = `is_leaf(id) && !is_unloaded(id)`, contrastado con `has_children` de la página en un test;
  - un `NodeCellError` es un error de la ruta de celdas.
- [x] (T9b1) Los nodos fijados incluyen los nodos que el pool dibuja, también durante `RetainedPrevious` (`poolAck` en la Tarea 10).
- [x] **Pasa si:**
  - expulsar y volver a injertar una página no deja huecos (el oráculo de cobertura de T15 sobre celdas finas), sobre un fixture v7.5 de T0. La cámara se aleja, la página se expulsa y la cámara vuelve. Se vuelve a injertar con los mismos `NodeId`, y el `cell_cut_digest` es el de antes, tras k frames aplicados sin subidas, con un pool holgado;
  - una página con nodos que el pool dibuja no se expulsa;
  - mientras llegan los bloques (bloque a bloque), la racha más larga de `RetainedPrevious` es ≤ 60 frames y la etapa del pool no sube en ninguna zona;
  - salto ≤ 3 y 0 `DescendantTooFine` en todas las actualizaciones del test;
  - en nativo, en Majadas a 4 px y a 1 px, se miden la memoria de CPU de las celdas y el tiempo de construcción por actualización.
- **Parar si** (no saltó ninguno):
  - hay un salto > 1 en un plan real;
  - hay un `NoStoredLevels` o un `UnknownNode`;
  - las celdas pasan de 512 MiB de memoria en Majadas a 4 px → cambiar el almacenamiento antes de la Tarea 10.

**T9c, precarga y solo refinar** (R3-Q20 y R3-Q21 con sus precisiones; `cell_pool.rs`, `cell_cut.rs`, `cell_driver.rs`). Spec enmendada tras el pre-mortem del 06-11-2026. Dos partes, cada una con su commit:

**T9c1, solo refinar (R3-Q21):**
- [x] `resident_depth`: si el nivel deseado no es residente, el residente más cercano a él (el más fino de los más gruesos, o el más grueso de los más finos si no hay ninguno más grueso). Los dorados de `cell_cut_digest` que cambien se listan con su motivo.
- [x] Un hijo «retenido» mientras no dibuje, en todos sus octantes, un nivel ≥ `min(nivel que dibujaba el antepasado ahí, deseado del hijo, tope del ajuste)`, según lo que `cell_cut` pondría en `drawn`. Un hijo retenido no dibuja ni enmascara al antepasado (`resident = []` en el corte real) y cuenta como sin nivel en el respaldo.
- [x] Memorias: claves con un digest de las profundidades dibujadas y del conjunto retenido; `reset()` lo vacía todo.
- [x] Contadores: los hijos retenidos cuentan en `nodes_without_level` y sus octantes en `stages`; páginas de los antepasados de respaldo en un contador nuevo.
- **Hecha (06-11-2026).** Ningún dorado de `cell_cut_digest` cambia (todos con todos los niveles residentes). Límite: un hijo bajo un antepasado a más de 3 niveles de otro nodo residente no se puede retener (la máscara no llega) y dibuja un instante su nivel más grueso, sin huecos. Si la búsqueda de retenidos no se estabiliza en 3 pasadas, se retiene de más y se cuenta (`hold_unsettled`).

**T9c2, precarga (R3-Q20):**
- [x] Un nodo sin nivel y sin antepasado que dibuje sale del corte aplicado; su nivel más grueso se pide y sube con prioridad; entra al ser residente. Cuenta en `nodes_without_level`.
- [x] Respaldo por descendientes: si ese nodo tiene descendientes en el último corte aplicado (salto ≤ 3) que dibujaban, siguen en el corte real hasta que el nodo es residente.
- [x] Se retiene el corte anterior solo si una zona que dibujaba no se puede cubrir así.
- [x] Contador de la retención más larga de un hijo (frames seguidos retenido con su nivel umbral pedido o en vuelo), para el criterio de parada de 120 frames. Un nodo que salió del corte del planificador (fuera de vista) nunca cuenta como perdido.
- **Hecha la parte del pool (T9c2a, 06-11-2026).** Al alejar, el nodo grueso espera con la misma regla de T9c1 frente a los descendientes que dibujaban. Un nodo excluido que sería demasiado fino para un antepasado del corte no se pide: su antepasado dibuja la zona y cuenta como etapa, no como nodo sin nivel. En el banco de T7d, los contadores son idénticos y la CPU a 4 px sigue por debajo de 2 ms; Majadas a 1 px (carga) pasa de 2,76 a 3,11 ms de mediana en movimiento. Falta T9c2b: el oráculo de zonas perdidas en el banco de `LodDataset` y la medida en Majadas.

**Pasa si** (T9c1 y T9c2) — **pasa** ([registro](../records/benchmarks/R3_T9_CELLS_06-11-2026.md)):
- tests sintéticos de alejar, salto de pose y bajada del tope, además de los de T9a1 y T9b2a;
- el oráculo de huecos también mira las zonas de los nodos excluidos o retenidos (`lost_samples`: un punto que se dibujaba y ya no, y sigue en el corte), y el banco lo ejecuta de verdad;
- un engrosado entre nodos solo cuenta si `después < min(antes, deseado ahora)`; debe ser 0 en todas las poses del banco, también en las órbitas;
- el banco de T9b2b llega a la etapa final en Majadas dentro de una hoja (4 px; 1 px como carga), con la racha de `RetainedPrevious` ≤ 60 frames y 0 huecos; se miden la memoria de celdas, el tiempo de construcción y las páginas de respaldo (criterio de parada de T9b: 512 MiB a 4 px).

**Parar si:** `lost_samples` > 0 al alejar o en un salto de pose; un antepasado retenido más de 120 frames sin que el nivel umbral del hijo esté pedido o en vuelo; `pool_coarsened` o `pool_overflow` por encima de T9b2b durante las retenciones; `nodes_without_level` = 0 con nodos excluidos o retenidos.

**Tarea 9 hecha (06-11-2026)** ([registro](../records/benchmarks/R3_T9_CELLS_06-11-2026.md)): los 9 casos del banco nativo se asientan sin huecos, sin zonas perdidas, sin retener la imagen y con un salto máximo de 1; dentro de una hoja de Majadas a 4 px, las celdas ocupan como mucho 303 MiB. Incluye T9c3: con un recorte del ajuste, el nodo conserva su nivel fino hasta que el recortado está en GPU. La GPU y el navegador se miden en la Tarea 11.

## Tarea 10 ★: protocolo de workers

**Hecha (07-11-2026).** Spec del 06-11-2026, enmendada tras el pre-mortem y con R3-Q22 y R3-Q23. El visor dibuja celdas que construye el worker de construcción (decisión del propietario del 06-11-2026; construir un nodo cuesta hasta 69 ms en p95, [T9](../records/benchmarks/R3_T9_CELLS_06-11-2026.md)). La ruta de celdas es opt-in; la ruta por defecto la decide la Tarea 17. Cinco partes, cada una con su commit.

**Hechos de partida:**
- El wasm vive en dos workers: el de render (`viewer-worker.js`, con `WasmVoxelViewer`, el `SurfaceRenderer` y el lienzo) y el de construcción (`build-worker.js`, con `WasmSceneBuilder` y el `LodDataset`). El hilo principal no carga wasm. Sin worker de construcción, el de render tiene el dataset. Los dos workers no importan módulos (los incrusta `package-local.py`).
- Hoy `update_cells_at_stage` llama al `CellDriver` de forma síncrona (`add_node`, `frame`, `has_node`) y fija los nodos que dibuja el pool (`pool_drawn`). No hay códec para `PackedLevel` ni bindings para la ruta de celdas.
- `CellDriver::reset()` vacía el pool pero conserva los niveles empaquetados; `forget` olvida nodos que no están en el corte actual; `upload_voxel_cut`, `sync_visible_blocks` y el cambio de modo de color sueltan el pool del renderer.
- Ya hay mecanismos de «gana el último» (vista, escenas por época, `CoalescedCommandQueue`); `nodeCells` y `evict` no pueden usarlos.

**Diseño:**
- **Tres pasos, separables:**
  1. el dataset elige, planifica, carga, construye y empaqueta, y da el corte y los nodos nuevos;
  2. el driver dibuja ese corte en el renderer;
  3. el dataset recibe el acuse.
  En proceso, los tres se llaman seguidos; con worker, el 1 y el 3 van en el de construcción y el 2 en el de render.
- **Mensajes** (todos con la generación del dataset y la sesión del pool):
  - construcción → render:
    - `nodeCells`: un nodo con todos sus niveles empaquetados, búferes transferidos, en orden y sin fusionar. Un nodo repetido con el mismo id y la misma generación se ignora;
    - `cellCut`: el corte de la etapa actual, con su `cutEpoch` y su `support` (los antepasados ya construidos de nodos que aún esperan celdas; el driver no los olvida, para que «padres primero» no los reconstruya una y otra vez). Aquí sí gana el último.
  - render → construcción:
    - `poolAck`: el `cutEpoch` aplicado, los nodos del corte aplicado o retenido y `missing` (los nodos que el corte nombraba y el driver no tenía, para reconstruirlos). Gana el último;
    - `evict`: los nodos que el driver olvidó por el presupuesto de CPU, con la clave del pool, en orden y sin fusionar;
    - `poolReset`: el pool se vació (se perdió el dispositivo o el renderer soltó el pool), pero el driver conserva los nodos. El dataset solo vacía `pool_drawn`;
    - `driverReplaced`: el driver es nuevo (cambio de dataset o de worker). El dataset da por perdidos todos los nodos enviados y los vuelve a mandar.
- **El lado de render filtra el corte** por los nodos que tiene (`has_node`), como hoy en proceso. `forget` no olvida nodos que llegaron después del último corte aplicado.
- **Fijado con retraso:** el dataset fija la unión del último `poolAck` y de todos los cortes enviados desde entonces (por `cutEpoch`).
- **Padres primero (R3-Q22):** antes de construir un nodo se construye su padre. La escalera solo arranca al abrir el dataset o tras un salto de cámara.
- **Dibujo:** el worker de render dibuja mientras `CellDriver::needs_frame()` sea cierto o haya un `cutEpoch` enviado sin aplicar (`cellsBusy`, en las estadísticas de LOD y en `lodStillLoading`). Reescribe la tabla de páginas en cada movimiento de cámara. Se aplica como mucho un tope de bytes de `nodeCells` por frame; el resto espera en cola.
- **En modo de celdas, el constructor no emite escenas**, y el worker de render comprueba que no recibe ninguna (si no, la escena soltaría el pool).
- **`cutDigestAtRest`:** el `cell_cut_digest` del corte aplicado cuando el constructor está en reposo en la época E, el render aplicó E, `needs_frame()` es falso y el digest se repite en dos lecturas seguidas. Se publica en las estadísticas de LOD de los dos modos.
- **Modos de color (R3-Q23):** otro modo vuelve a la ruta CPU; al volver a RGB, las celdas se reactivan.
- **Memoria:** `cpu_budget_bytes` del driver en wasm, 512 MiB (el criterio de parada de T9b). Se registra `wasmLinearMemoryBytes` en los dos workers.

**T10a1, códec y empaquetado** (`viewer_render_wgpu`):
- [x] Códec de `PackedLevel` (cabecera, `PageInfo` de tamaño fijo, instancias `Pod`), con ida y vuelta exacta y errores en datos truncados o mal formados.
- [x] `pack_node`: empaqueta las celdas de un nodo fuera del driver (lo que hoy hace `CellDriver::add_node`), para hacerlo en el worker de construcción. `add_packed_generation` ignora el duplicado con la misma generación (`AlreadyStored`); con otra generación es `Duplicate` y se resuelve con `driverReplaced`.
- [x] **Pasa si** el códec da exactamente los mismos bytes de GPU que el empaquetado directo, y los datos malformados dan error, nunca pánico.

**T10a2, tres pasos, padres primero y simulación** (`lod_cells.rs`, `lod_driver.rs`, `lod_dataset.rs`):
- [x] `update_cells_at_stage` separado en los tres pasos, con un acuse que puede llegar con retraso; el fijado, los olvidados, `missing`, `poolReset` y `driverReplaced` como en el diseño.
- [x] Padres primero (R3-Q22) y la escalera que solo arranca al abrir o tras un salto.
- [x] **Pasa si**, en un test nativo que simula los workers, el `cutDigestAtRest` es el mismo que en proceso:
  - intercalado aleatorio con semilla de pasos de construcción y de render, presupuestos de CPU y de páginas pequeños, en órbita y en acercamiento, sobre Belvedere v7.5 (con `R3_REQUIRE_REAL_DATA`) y en sintético;
  - con un `poolReset` y un `driverReplaced` a mitad de carga;
  - sin `Duplicate`, `UnknownNode` ni `NoStoredLevels`;
  - una página expulsada durante el retraso del acuse no deja huecos;
  - al orbitar, ninguna muestra se vuelve más gruesa que antes y que lo pedido (R3-Q21), y la etapa no pasa más de 1 s por encima del objetivo.
- **Hechas T10a1 y T10a2 (07-11-2026).** En la simulación de los dos workers, con intercalado aleatorio, el `cutDigestAtRest` es igual al del camino en proceso:
  - en sintético, con 4 semillas, en órbita y en acercamiento, con `poolReset` y `driverReplaced`;
  - en Belvedere v7.5, con las semillas 1 y 7 en órbita, la 3 en acercamiento y la 5 con reinicios;
  - sin huecos, zonas perdidas, engrosados ni violaciones de «padres primero», y sin ningún `Duplicate`, `UnknownNode` ni `NoStoredLevels`.
  Las simulaciones reales tardan minutos: son `#[ignore]` y se lanzan con `R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile perf -- --ignored real_sim`.
  Dos fallos de la simulación resultaron ser del oráculo y se corrigieron sin aflojarlo:
  - muestras en caras compartidas por dos nodos;
  - comparar con un plan que el render aún no había recibido.
- **Límites conocidos:**
  - los antepasados de «padres primero» (`support`) no se olvidan y pueden sobrepasar el presupuesto de CPU sin aviso;
  - leer primero el antepasado más alto e intercalar con la selección es preventivo: ningún test reprodujo la inanición;
  - un `add_cell_nodes` fallido vacía los nodos fijados del pool;
  - por debajo del conjunto de trabajo, el olvido y la reconstrucción se repiten en bucle (ver Tarea 11).

**T10b, bindings** (★ `bindings/lod.rs`, `bindings/builder.rs`, `bindings/render.rs`):
- [x] Apertura con celdas opt-in. Si el formato no las admite, `CellPathUnsupported` y se sigue por la ruta CPU.
- [x] `WasmSceneBuilder` da los `nodeCells` y el `cellCut`, y recibe `poolAck`, `evict`, `poolReset` y `driverReplaced`. `WasmVoxelViewer` tiene `addCellNode` y `setCellCut`, y en `renderFrame` ejecuta el frame del driver. En modo sin worker usa el camino en proceso.
- [x] `cutDigestAtRest`, `cellsBusy`, `cutEpoch`, los contadores del pool y `wasmLinearMemoryBytes` en las estadísticas.
- [x] **Pasa si:**
  - un test nativo de los bindings da el mismo `cutDigestAtRest` con constructor separado y en proceso;
  - clippy wasm32.
- **Hecha (07-11-2026).** La lógica vive en `cells_glue.rs` (`BuilderCells`, `ViewerCells`, `InProcessCells`), sin tipos de wasm-bindgen, y los bindings (`bindings/cells.rs`) solo convierten tipos. API para el host:
  - constructor: `enableCells`, `setViewEpoch`, `updateCells`, `applyPoolAck`, `applyEvict`, `poolReset` y `driverReplaced`;
  - visor: `enableCells`, `beginRemoteCells`, `addCellNode`, `setCellCut` (con `viewEpoch`), `takeCellMessages` y `viewEpoch`;
  - estadísticas: `cutDigestAtRest`, `cellsBusy`, `viewEpoch` y `wasmLinearMemoryBytes`.
  El corte lleva el `viewEpoch` de la vista con que se planificó, y el reposo exige que sea el último. Así no se publica un digest de una vista anterior tras mover la cámara.
  En las pruebas nativas, el pegamento da el mismo digest que el camino en proceso, también en Belvedere v7.5. Los bindings solo se han compilado (wasm32); se ejercitan en T10c.

**T10c1, host web** (`examples/web-host`):
- [x] Los dos workers manejan los mensajes nuevos: `nodeCells` y `evict` sin fusionar y fuera del filtro de época de las escenas, el tope de bytes por frame y el parámetro `cells=1`. Las ayudas nuevas van dentro de cada worker.
- [x] **Pasa si** los tests de Node, con los workers en `vm` y wasm falso, cubren el orden y la no fusión de `nodeCells`/`evict`, el gana el último en `cellCut`/`poolAck`, `poolReset`, `driverReplaced`, `cellsBusy` y que en modo de celdas no llegan escenas.
- **Hecha (07-11-2026).** `?cells=1` o `createViewer({ cells: true })`. Mensajes nuevos: `nodeCells`, `cellCut`, `cellLod`, `poolAck`, `evict`, `poolReset` y `driverReplaced`; la vista lleva `viewEpoch`. 148 tests de Node (32 de celdas), con los workers en `vm` y dobles que fallan como la API real.
  - Con las celdas apagadas, la ruta CPU no envía mensajes de más y las métricas conservan su forma.
  - Tras 600 frames ocupados sin progreso, el worker de render da `cellsBusy: false` y `cellsStuck: true`.
  - Para T10c2: `cellsBusy: false` no significa reposo; el probe espera un `cutDigestAtRest` no nulo.

**T10c2, navegador:**
- [x] **Pasa si** con el worker activado y desactivado se obtiene el mismo `cutDigestAtRest`, en Chrome con WebGPU, sobre Belvedere v7.5, servido por http (no `file://`) y con un probe como `probe-orbit-load.mjs`.
- [x] Verificación con Playwright: consola sin errores, escritorio y móvil (emulado), órbita, cambio de modo de color y vuelta a RGB, y picking.
- [x] Se miden el p95 del frame del worker de render mientras llegan nodos y `wasmLinearMemoryBytes`.
- **Hecha (07-11-2026)** ([registro](../records/benchmarks/R3_T10_WORKERS_07-11-2026.md)): mismo `cutDigestAtRest` con el worker de construcción y sin él en Chrome con WebGPU, y verificación con Playwright en escritorio y en móvil emulado. Sin worker de construcción, el frame llegaba a un p95 de 100 ms, y de ahí R3-Q24: las celdas solo van con el worker de construcción.

**Tarea 10 hecha (07-11-2026).**

**Parar si:**
- hay algún `Duplicate`, `UnknownNode` o `NoStoredLevels` en una simulación o en el navegador;
- la sesión del pool sube sin que se pierda el dispositivo ni cambie el dataset (un bucle de reinicios);
- los digests de la simulación aleatoria difieren del camino en proceso tras los arreglos (la convergencia depende del camino) → al propietario;
- el p95 del frame del worker de render pasa de 2 × el de la ruta CPU mientras llegan nodos, o la memoria wasm pasa del presupuesto;
- al orbitar, la etapa pasa más de 1 s por encima del objetivo.

## Tarea 11: humo en navegador

**Medida (13-11-2026), salta un criterio:** [registro](../records/benchmarks/R3_T11_SMOKE_13-11-2026.md). Se asienta en todas las poses a 4 px; Majadas a 1 px dentro de una hoja entra en bucle de olvido con el presupuesto de CPU de wasm lleno. Corregido el 14-11-2026 (R3-Q27, R3-Q28; [registro](../records/benchmarks/R3_B1C_CPU_FIT_14-11-2026.md)): Majadas a 1 px dentro de una hoja se asienta engrosado. Belvedere a 1 px tras un salto, que desbordaba el pool de GPU, se asienta con el pool de 512 MiB (R3-Q29; [registro](../records/benchmarks/R3_B1D_GPU_POOL_14-11-2026.md)): no salta ningún criterio propio de la Tarea 11; queda el de ≤ 16,7 ms de la Tarea 16b.

- [x] «Objetivo alcanzado» exige `CutApply::Applied`, `nodesWithoutLevel` = 0, `poolCoarsened` = 0 y ningún nodo dibujado con un nivel más grueso que el deseado (R3-Q3).
- [x] Medir la congelación por `DescendantTooFine` (R3-Q10): duración y frecuencia en saltos de pose.
- [x] Con un presupuesto de CPU del driver por debajo del conjunto de trabajo de la vista, los nodos fuera del corte se olvidan y se reconstruyen en cada frame (hallazgo de T10a2: en Belvedere, el acercamiento no se asienta por debajo del 90 % de su conjunto de trabajo, 54,8 MB). En wasm el presupuesto es de 512 MiB y Majadas a 1 px ocupa 824 MiB ([T9](../records/benchmarks/R3_T9_CELLS_06-11-2026.md)): medirlo en el navegador y, si se da, añadir histéresis al olvido.
- [x] **Pasa si** no salta ningún criterio de T11. Verificación con Playwright. **Pasa (14-11-2026):** se asientan todas las poses ([B1d](../records/benchmarks/R3_B1D_GPU_POOL_14-11-2026.md)) y el tiempo de GPU de la Tarea 16b es p95 7,38 ms ([registro de B1b](../records/benchmarks/R3_B1B_HIZ_14-11-2026.md)).

## Tarea 12 ★: modos de color

**Hecha (14-11-2026):** [registro](../records/benchmarks/R3_B2_COLOR_14-11-2026.md). Todos los modos se dibujan con celdas (R3-Q32, R3-Q33); la vuelta a la ruta CPU de R3-Q23 desaparece.

- [x] **Pasa si:** (reabierta y cerrada el 14-11-2026: la ruta asíncrona no leía intensidad ni clase en modo RGB; corregido en [B2b](../records/benchmarks/R3_B2B_ASYNC_ATTRS_14-11-2026.md))
  - el color por atributo coincide bit a bit con la ruta CPU, salvo intensidad y elevación, con ≤ 1 LSB (R3-Q25);
  - los modos de shader no reconstruyen celdas;
  - las generaciones de color viejas se descartan.

## Tarea 13 ★: continuidad entre nubes (RF-05) con paginación

- [ ] Usar el espaciado de referencia de T0b.
- [ ] **Pasa si** un mismo espaciado por nube no cambia al injertar páginas.

## Tarea 14: modos de px (antes R1-T1b)

- [ ] Como en el plan anterior (antes R1-T1b).

## Tarea 15: nivel por octante (obligatoria desde el 05-11-2026)

Regla en el [ADR 0034](../adr/0034-nivel-por-octante-con-arista-real.md). Se hace antes de la Tarea 5. Especificación revisada con el pre-mortem del 05-11-2026.

**Hecha (05-11-2026):** [aceptación](../records/benchmarks/R3_T15_ACCEPT_05-11-2026.md): 0 diferencias por octante con el oráculo del spike y cifras idénticas a las registradas. El criterio de rangos de dibujo salta solo en cortes de un nodo; la Tarea 5 medirá con estos recuentos.

- [x] **Octantes:** los 8 hijos del nodo, partido por la mitad en cada eje. El índice es `o = 4x + 2y + z`, el mismo orden que el Morton de `node_cells.rs` (x es el bit más alto), así que las celdas de un octante quedan contiguas en cada nivel. La relación con la máscara 8³ (bit `x + 8y + 64z`) se documenta y se prueba en un fixture asimétrico.
- [x] **Desplazamientos por octante:** cada `NodeCellLevel` guarda 9 desplazamientos, con el inicio de cada octante y el final, calculados al construir el nivel. Solo hacen falta en los niveles con profundidad ≥ nodo + 1.
- [x] **Marco de coordenadas:** las cajas de octante se pasan al marco local (`caja − origen`), el ojo es `posición / visual_scale` y la prueba es `projected_voxel_edge_pixels(cubo, [0; 3])`, el mismo marco que el spike. Un test de invariancia desplaza rejilla, cajas y origen 10⁶ con `visual_scale` = 0,0788 y exige los mismos niveles por octante.
- [x] **Nivel deseado del octante:** el primer nivel guardado `d` (ascendente) cuya celda de lado `root_side / 2^d`, colocada en el punto del octante más cercano al ojo (ajustada para quedar dentro), cumple el objetivo con `projected_voxel_edge_pixels`. Si ninguno lo cumple, el más fino (R3-Q9); con error infinito o NaN, también el más fino.
  - Un octante tapado entero por la máscara no pide nivel: `wanted = None`, no exige residencia y no cuenta como etapa.
  - Los niveles de hoja con profundidad ≤ nodo abarcan varios octantes. Si un octante elegiría uno de ellos, pasa al siguiente nivel guardado ≥ nodo + 1, que es más fino y respeta RQ-09. Si no hay ninguno, todo el nodo usa ese nivel.
- [x] **Nivel dibujado del octante:** `resident_depth` por octante (R3-Q3): el deseado si es residente; si no, el residente más grueso, que es etapa solo si es más grueso que el deseado. Las etapas se cuentan por (nodo, octante).
- [x] **`CellCutEntry`:** `wanted: [Option<u8>; 8]`, `drawn: [Option<u8>; 8]`, `stage: [bool; 8]` y la misma máscara 8³.
  - `required_levels()` devuelve las profundidades distintas que se deben tener residentes.
  - `draws()` devuelve, por profundidad dibujada, los rangos contiguos de instancias de los octantes que la usan, a partir de los desplazamientos. Es lo que dibujan las Tareas 6 y 7.
- [x] **Dorados:** `cell_cut_digest` añade los campos por octante solo cuando difieren entre octantes. Un primer commit de refactorización, con todos los octantes de un nodo en el nivel de la regla anterior, deja los dorados idénticos. El commit de la regla nueva cambia solo los que debe.
- [x] **Oráculos:**
  - por octante, por fuerza bruta y con su propia colocación del cubo: todos los niveles guardados más gruesos fallan y el deseado pasa;
  - de cobertura sobre `draws()`: cada celda de la rejilla más fina queda cubierta exactamente una vez, con el octante calculado desde coordenadas enteras;
  - con una cámara dentro del dataset y `visual_scale` ≠ 1.
- [x] **Spike de cobertura:** `level_ok` sigue siendo un oráculo independiente. En cada caso debe haber 0 diferencias por (nodo, octante) con la librería, y los totales deben coincidir con las filas registradas de `octant_tight`. Se añaden los bytes residentes reales (niveles completos × 12 B) y el número de rangos de dibujo.
- [x] **Pasa si** se cumplen los oráculos y los tests de volumen de la Tarea 3, el spike no da diferencias, y los bytes residentes de Majadas desde dentro de una hoja caben en 256 MiB.
- **Parar si:**
  - los bytes residentes superan 256 MiB (vuelve al propietario por el ADR 0034);
  - hay diferencias entre la librería y el oráculo, o el test de invariancia falla;
  - los rangos de dibujo superan 3× los nodos del corte en Belvedere a 4 px (repetir la Tarea 5 antes de la 6);
  - un dataset real elige un nivel que abarca varios octantes y no tiene uno más fino guardado (replantear la cadena de la Tarea 1).

## Tarea 16: compactación y oclusión sobre el pool (obligatoria antes de la Tarea 6, R3-Q15)

Decidida en el [ADR 0035](../adr/0035-instancia-de-16-b-con-w-16.md): compactación con Hi-Z por página (precisada por el [ADR 0036](../adr/0036-hi-z-por-celda.md): Hi-Z por celda); raymarching (OQ-08) solo si con ella Majadas dentro de una hoja a 4 px no baja de 16,7 ms. Especificación del 05-11-2026.

El pool de producción aún no existe (Tarea 7), así que la técnica se mide primero sobre la página del spike de GPU, con los mismos casos y la misma sonda. El resultado fija qué campos de página añade la Tarea 6 y qué pases lleva la Tarea 7 a `SurfaceRenderer`. Referencias: la Tarea 5 ([registro](../records/benchmarks/R3_GPU_SPIKE_05-11-2026.md)) y las [opciones de oclusión](../records/benchmarks/R3_OCCLUSION_OPTIONS_05-11-2026.md).

**T16a hecha (05-11-2026):** [registro](../records/benchmarks/R3_T16_OCCLUSION_05-11-2026.md). Pasa con `kpc`. En la peor dirección (Majadas dentro de una hoja mirando a +y, 4 px), 4,39 ms con la cámara quieta y p95 de 8,27 ms en movimiento, frente a 21,66 ms del `draw` por rango; 0 agujeros en los 21 casos con algo en pantalla. Entra el Hi-Z por celda (−42 % y −3,23 ms frente a `k`); el Hi-Z por página no aporta (+0,9 %). `PageEntry` lleva la caja ajustada de la página. El propietario lo aceptó en el [ADR 0036](../adr/0036-hi-z-por-celda.md) (R3-Q16).

**T16a, spike (decide):** amplía `examples/web-host/spike-cube-pool.*`, solo con la instancia de 16 B (W = 16). Sigue siendo código desechable. Enmendada con su pre-mortem del 05-11-2026.

- [x] **Seis direcciones:** el corte de nodos depende del frustum (`coverage_spike.rs:528`), pero cada nodo se dibuja entero. Según el pre-mortem, el 72,9 % de las celdas dibujadas en Majadas dentro de una hoja a 4 px cae fuera del frustum. Por eso `coverage_spike --export` añade, solo a 4 px y en ambos datasets, la pose dentro de una hoja mirando a −x, ±y y ±z, con el mismo ojo; la actual mira a +x. Para ±z, `set_pose` limita el pitch a ±89° y recoloca el ojo, lo que se registra. Las filas ya registradas no cambian. **Decide la peor dirección.**
- [x] **Caja de página ajustada:** al cargar el caso, el mínimo y el máximo de las coordenadas relativas de los huecos usados de cada página, calculados en la página desde `w16_instances.bin`. En producción serían campos de `PageEntry`.
- [x] **Pase de páginas** (cómputo, un hilo por página): descarta la página si su caja queda fuera del frustum y, en las variantes con oclusión, si queda tapada en el Hi-Z. Escribe la lista de páginas que siguen y los argumentos de un despacho indirecto.
- [x] **Pase de huecos** (cómputo con despacho indirecto, **un grupo de trabajo por página** que sigue):
  - descarta los huecos vacíos, los de octantes que dibujan otro nivel, los tapados por la máscara 8³ y los que caen fuera del frustum; en la variante por celda, también los tapados en el Hi-Z;
  - compacta con una suma de prefijos local y **un solo `atomicAdd` global por grupo**, que conserva el orden Morton dentro de la página;
  - escribe índices **absolutos** de hueco y los argumentos de `drawIndexedIndirect`. Con índices absolutos no hace falta la feature `indirect-first-instance`;
  - se comprueba en la CPU que el despacho no supera `maxComputeWorkgroupsPerDimension`. Si lo supera, se usa un despacho 2D; nunca se deja un despacho que no hace nada.
- [x] **Dibujo:** un `drawIndexedIndirect` por fase. El shader de vértices toma el hueco de `visible_indices[instance_index]` y decodifica como el spike, sin las pruebas que ya hizo el pase de huecos.
- [x] **Oclusión en dos fases** (Haar y Aaltonen, 2015), con bits de visibilidad por página y por hueco **en doble búfer**: cada fase lee `prev` y escribe `next`, y se ponen a cero al cambiar de variante o de caso.
  - fase 1: compacta y dibuja lo que fue visible en el frame anterior y sigue dentro del frustum;
  - Hi-Z propio, sin reutilizar las constantes de `hzb.rs`:
    - la profundidad se copia a la escala de la pirámide, de forma que cada texel cubra todos sus píxeles con el máximo;
    - el margen de profundidad es de unos pocos ulp, no el `depth_epsilon = 0.001` de NDC;
  - fase 2: prueba contra el Hi-Z todas las páginas y huecos candidatos, escribe sus bits `next` y dibuja, sobre la misma profundidad, los que pasan y no se dibujaron en la fase 1.
  - El primer frame, sin bits, dibuja todo en la fase 2.
- [x] **Variantes**, intercaladas por ronda como en la Tarea 5:

  | Variante | Qué hace |
  | --- | --- |
  | `w16-ranges` | Referencia: el `draw` por rango de la Tarea 5 |
  | `k` | Pases de páginas y huecos sin oclusión (frustum, vacíos, octante, máscara) |
  | `kp` | `k` + dos fases con Hi-Z solo por página |
  | `kpc` | `kp` + Hi-Z por celda en el pase de huecos |
  | `k-moving`, `kp-moving`, `kpc-moving` | Las anteriores durante 120 frames con un giro de 0,5° por frame y una traslación de una celda por frame, con el corte fijo |

- [x] **Medida:**
  - tiempo de GPU del frame entero, desde el inicio del primer pase hasta el final del último, más el de cada pase;
  - con la cámara quieta, mediana; en movimiento, mediana y p95, el frame frío tras cambiar de variante y cuántos frames tarda la fase 2 en estabilizarse;
  - fuera de los frames medidos, recuentos leídos de la GPU: páginas que siguen, celdas compactadas y celdas dibujadas en cada fase.
- [x] **Recuentos correctos:** una variante de comprobación de `k` sin frustum debe compactar exactamente `drawn_cells` de `case.json`. Si no coincide, las cifras no valen.
- [x] **Imagen correcta:** con `depthCompare: "less"`, la profundidad final es el mínimo de los fragmentos y no depende del orden de dibujo. Por eso la profundidad leída de cada variante debe ser **idéntica** a la de `w16-ranges`, bit a bit:
  - con la cámara quieta, en cada caso;
  - en movimiento, en un modo de comprobación sin medir tiempos, en todos los frames, incluidos los 10 primeros tras un arranque en frío.
  - Un píxel más profundo que en la referencia es un agujero: la oclusión no es conservadora. Se registra además la fracción de píxeles con distinto color, que puede venir de caras coplanares dibujadas en otro orden.
- [x] **Registro** `R3_T16_OCCLUSION_dd-mm-aaaa.md` con la misma identidad y reproducción que la Tarea 5.
- **Reglas** (peor dirección, a 4 px):
  - **pasa** si alguna variante deja Majadas dentro de una hoja en ≤ 16,7 ms, con variación A/A ≤ 5 %, sin agujeros y con el p95 en movimiento también ≤ 16,7 ms. Belvedere dentro de una hoja sigue en ≤ 8 ms. La Tarea 7 implementa la variante más simple que cumpla;
  - el Hi-Z (`kp`, `kpc`) solo entra en la Tarea 7, y la caja de página solo entra en `PageEntry` (Tarea 6), si mejora a `k` en al menos un 10 % y 1 ms en el caso que decide. Si solo `k` cumple, se registra que el Hi-Z no está probado;
  - `kp` a menos de un 5 % de `k` → se descarta el Hi-Z por página;
  - en órbita y rasante, si los pases cuestan más de 1 ms sobre `w16-ranges`, la Tarea 7 solo los activa por encima de un número mínimo de huecos, como `MIN_OCCLUSION_CANDIDATES` en `hzb.rs`;
  - Majadas dentro de una hoja a 1 px queda como prueba de carga: se registra frente a 33 ms y no decide.
- **Parar si:**
  - ninguna variante baja de 16,7 ms en la peor dirección de Majadas dentro de una hoja a 4 px → vuelve al propietario por el raymarching (OQ-08, ADR 0035);
  - pasa con la cámara quieta, pero el p95 en movimiento o el frame frío superan 16,7 ms → vuelve al propietario;
  - hay agujeros que no se explican por un error de la implementación;
  - los recuentos de la GPU no coinciden con los de `case.json`;
  - la variación A/A supera el 5 % dos veces seguidas: antes de dar veredicto hay que fijar el estado de energía.

**T16b, producción:** los pases van a un módulo nuevo, `cell_cull.rs` (Tarea 7b2), dentro de la Tarea 7, sobre la instancia y la tabla de páginas de la Tarea 6. El orden respeta el ADR 0035: la Tarea 16a decide antes de la 6 qué campos tiene la página, y el código de producción solo puede ir con el pool, que llega en la 7.

- [x] El margen de profundidad del Hi-Z sale de una cota del error entre la caja proyectada y el rasterizador; en el spike, 4 ulp solo están validados empíricamente. **Hecho (14-11-2026, `3bb1d1b`):** el margen fijo de 8 u no tenía derivación que lo cubriera (el pase por página prueba la caja de la página, no la de la celda). Pasa a `u·(25ρ + 24)`, con ρ = máx. |p|/w en las esquinas, derivado del redondeo del recorte, la división del shader (2,5 ulp, WGSL «Floating Point Accuracy») y un error de rasterizado supuesto de 4 u (el doble de los 2,00 u medidos; WebGPU y Vulkan no lo acotan). Con ρ por encima de 65.536 o NaN no se oculta. En 6 poses extremas el peor error medido es 2,00 u y la holgura mínima 45,6 u (RADV y llvmpipe).
- [x] (Corregido el 14-11-2026, `ace9d84`, con test a 1280×720.) La pirámide de `hzb.rs` se crea con tamaño potencia de dos, pero `copy_depth` copia 1:1 y la búsqueda escala por `pyramid_size / viewport`, de modo que a 1280×720 lee texeles de otra zona (pre-mortem del 05-11-2026; *inferido* leyendo el código, sin reproducir). El pase nuevo usa la convención del spike: cada texel guarda el máximo de todos sus píxeles.
- [x] **Bindings frente a los límites de producción** (05-11-2026). `gpu.rs:326` pide `downlevel_defaults`: 4 búferes de almacenamiento por etapa, 128 MiB por binding y 256 MiB por búfer. Búferes de almacenamiento de `kpc` en el spike:

  | Pase | Búferes | Con la caja en `PageEntry` |
  | --- | ---: | ---: |
  | Páginas (cómputo) | ≤ 6 | ≤ 5 |
  | Huecos (cómputo): instancias, páginas, máscaras, cajas, lista de páginas, visibles, argumentos, bits | 8 | 7 |
  | Cierre (cómputo) | 3 | 3 |
  | Dibujo (vértices): instancias, páginas, visibles | 3 | 3 |

  Ni el pase de huecos ni el pool de 256 MiB (ADR 0033) caben en `downlevel_defaults`. Por eso, según el ADR 0036, la Tarea 7 pide `max_storage_buffers_per_shader_stage` ≥ 8, que es el mínimo del estándar WebGPU, y un `max_storage_buffer_binding_size` y un `max_buffer_size` que alojen el pool, sin pasar de lo que da el adaptador. Si el adaptador no llega, la ruta de celdas no se activa y se usa la ruta CPU (R3-Q5).
- [x] **Pasa si** la profundidad es idéntica con y sin oclusión (Tarea 7b), y Majadas dentro de una hoja a 4 px en el visor wasm, con los límites de producción, sigue en ≤ 16,7 ms (se mide en la Tarea 11). **Pasa (14-11-2026):** tiempo de GPU p95 7,38 ms en reposo (R3-Q31; [registro de B1b](../records/benchmarks/R3_B1B_HIZ_14-11-2026.md)).

## Tarea 17: medida y ruta por defecto

- [ ] Matriz de R0 más RED-REF/HTTP Range. «Alcanzado» se define como en T11. Registrar GPU y GTT.
- [ ] **Pasa si** se cumplen RQ-01, RQ-02 y RQ-05. Entonces `renderPath: "cells"` pasa a ser la ruta por defecto, y se confirman las cifras de ADR 0027 y 0031.

## Fuera de este plan

Rediseño del acumulador de conversión (RQ-07), exportador LAS y multinube (RF-01).
