# R3 — Cubos por nodo residentes en GPU (v7.5-pre): plan de implementación

Estado: en curso · 03-11-2026, enmendado el 05-11-2026 por el ADR 0032 (sustituye el plan del 30-10-2026, que sigue en el historial de git)

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Objetivo:** sustituir la voxelización en CPU por vista (B1) por celdas por nodo residentes en GPU, sobre `v7.4/v7.5-pre` paginado.

**Por qué:** con 256³ por nodo, el diseño de 0027 desborda el pool y dibuja lo lejano con detalle de más ([ADR 0031](../adr/0031-celdas-por-nodo-en-v7-5.md)).

**Spec:**
- ADR: [0019](../adr/0019-cubos-por-nodo-residentes-en-gpu.md), [0027](../adr/0027-celdas-por-nodo-en-pool-paginado.md) (en lo vigente), [0032](../adr/0032-hojas-adaptativas-e-instancia-de-12-b.md), [0029](../adr/0029-octree-global-por-bits-de-enteros.md), [0030](../adr/0030-jerarquia-paginada-por-anclas.md) y [0031](../adr/0031-celdas-por-nodo-en-v7-5.md).
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
| R3-Q8 | (05-11-2026) Instancia de 12 B: 10 bits por eje relativos a la página y flags en `PageEntry`; W = 16 (16 B) queda como control en T5 |
| R3-Q9 | (05-11-2026) Una hoja con todas sus posiciones separadas no se refina más: cubos mayores que el objetivo de px en primeros planos caen en la excepción «densidad de la fuente» de RQ-09 |
| R3-Q10 | (05-11-2026) Una actualización de corte con `DescendantTooFine` se rechaza y se retiene el corte visible hasta que el nuevo esté completo (ADR 0027); la congelación se mide en la Tarea 11 |
| R3-Q11 | (05-11-2026) Si el corte deseado no cabe en el pool, los nodos más lejanos bajan a un nivel más grueso, como etapa contada, hasta que quepa. GPU mínima de 4 GB ([requisitos](../requirements.md#resoluciones-del-05-11-2026)) |

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
- **T4:** el corte ocupa más de 256 MiB, o hay más del doble de instancias en pantalla que en la ruta CPU en la pose rasante → replantear (nivel por octante) y volver al propietario.
- **T4/T9:** `DescendantTooFine` en más del 5 % de las actualizaciones de corte → rediseñar la máscara.
- **T5:** más de 16,7 ms de GPU con los recuentos de T4 → revisar ADR 0019.
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

- [ ] Majadas y Belvedere en órbita, en pose rasante y desde dentro de una hoja, a 1, 2, 4 y 8 px. Comparar la pirámide con 256³ solo.
- [ ] Registrar:
  - instancias totales y en pantalla;
  - píxeles vacíos;
  - bytes del corte;
  - frecuencia de `DescendantTooFine`.
- [ ] **Pasa si** no salta ningún criterio de parada de T4.

## Tarea 5: spike de GPU

- [ ] Con los recuentos de T4, comparar un `draw` por nodo con un `draw` único.
- [ ] Comparar la instancia de 12 B (zancada de 3 `u32`) con el control de 16 B (W = 16), por R3-Q8.
- [ ] **Pasa si** la GPU tarda 8 ms o menos con Belvedere a 4 px.

## Tarea 6: instancia y tabla de páginas

- [ ] Instancia de 12 B: un `u32` con 10 bits por eje relativos a la página, rgba8, intensidad `u16` y clase `i16`. Las páginas (8 KiB, 682 instancias) se parten en orden Morton si el rango supera 2¹⁰ o se llenan. `PageEntry` lleva el origen relativo a la cámara, el desplazamiento dentro del nodo y los flags `has_intensity` y `has_class`.
- [ ] **Pasa si** la decodificación es igual bit a bit en Rust y en WGSL, con raíz en nivel 0 y un origen a 10⁶ m.

## Tarea 7: pool en `SurfaceRenderer`

- [ ] Residencia todo o nada por (nodo, nivel), con presupuesto por frame. `evict_page` emite `evict`.
- [ ] **Pasa si** pasan `overflow_keeps_previous_complete_cut` y `regraft_mid_cut_keeps_coverage`.

## Tarea 8: picking sobre el pool

- [ ] **Pasa si** pasa `pick_on_pool_hits_same_cell_as_cpu_path`.

## Tarea 9 ★: `LodDataset` con celdas

- [ ] Una hoja es un nodo sin `has_children`.
- [ ] Los nodos fijados incluyen el corte confirmado por `poolAck`.
- [ ] v5, `v7.1–3` y Additive devuelven `CellPathUnsupported`.
- [ ] **Pasa si** expulsar y volver a injertar una página a mitad del corte no deja huecos y la etapa nunca sube.

## Tarea 10 ★: protocolo de workers

- [ ] `nodeCells` y `evict` con la clave del pool, sin coalescer. `poolAck` y `poolResync`.
- [ ] **Pasa si** con el worker activado y desactivado se obtiene el mismo `cutDigestAtRest`.

## Tarea 11: humo en navegador

- [ ] «Objetivo alcanzado» exige `CutApply::Applied`, `nodesWithoutLevel` = 0 y ningún nodo dibujado con un nivel más grueso que el deseado (R3-Q3).
- [ ] **Pasa si** no salta ningún criterio de T11. Verificación con Playwright.

## Tarea 12 ★: modos de color

- [ ] **Pasa si:**
  - el color por atributo coincide bit a bit con la ruta CPU;
  - los modos de shader no reconstruyen celdas;
  - las generaciones de color viejas se descartan.

## Tarea 13 ★: continuidad entre nubes (RF-05) con paginación

- [ ] Usar el espaciado de referencia de T0b.
- [ ] **Pasa si** un mismo espaciado por nube no cambia al injertar páginas.

## Tarea 14: modos de px (antes R1-T1b)

- [ ] Como en el plan anterior (antes R1-T1b).

## Tarea 15 (condicional): nivel por octante

- [ ] Solo si T4 da más del doble de instancias que la ruta CPU con la pirámide. Riesgo: celdas lejanas demasiado finas. Lleva su propio pre-mortem.

## Tarea 16 (condicional): compactación y HZB

- [ ] Como en el plan anterior.

## Tarea 17: medida y ruta por defecto

- [ ] Matriz de R0 más RED-REF/HTTP Range. «Alcanzado» se define como en T11. Registrar GPU y GTT.
- [ ] **Pasa si** se cumplen RQ-01, RQ-02 y RQ-05. Entonces `renderPath: "cells"` pasa a ser la ruta por defecto, y se confirman las cifras de ADR 0027 y 0031.

## Fuera de este plan

Rediseño del acumulador de conversión (RQ-07), exportador LAS y multinube (RF-01).
