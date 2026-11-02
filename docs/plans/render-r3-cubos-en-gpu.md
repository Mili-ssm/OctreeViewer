# R3 — Cubos por nodo residentes en GPU (v7.5-pre): plan de implementación

Estado: listo para ejecutar · 03-11-2026 (sustituye el plan del 30-10-2026, que sigue en el historial de git)

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Objetivo:** sustituir la voxelización en CPU por vista (B1) por celdas por nodo residentes en GPU, sobre `v7.4/v7.5-pre` paginado.

**Por qué:** con 256³ por nodo, el diseño de 0027 desborda el pool y dibuja lo lejano con detalle de más ([ADR 0031](../adr/0031-celdas-por-nodo-en-v7-5.md)).

**Spec:**
- ADR: [0019](../adr/0019-cubos-por-nodo-residentes-en-gpu.md), [0027](../adr/0027-celdas-por-nodo-en-pool-paginado.md) (en lo vigente), [0029](../adr/0029-octree-global-por-bits-de-enteros.md), [0030](../adr/0030-jerarquia-paginada-por-anclas.md) y [0031](../adr/0031-celdas-por-nodo-en-v7-5.md).
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
| R3-Q6 | Límite de 21 bits relativos por eje; si se supera, se para (propuesta por defecto, sin objeción del propietario) |

## Restricciones globales

- `scripts/check.sh` en verde al cerrar cada tarea.
- Solo cambia el formato en lo que exige R3-Q4.
- La ruta CPU se conserva: es el oráculo, y la que usan Additive, v5 y `v7.1–3`.
- Las tareas de la serie ★ tocan `lod_dataset.rs` y `bindings/{lod,builder}.rs`: se hacen de una en una.
- Los cortes se comparan por (nivel, prefijo). La clave del pool es (sesión, nodo, profundidad, generación de color).
- La línea base CPU sale de la misma compilación.
- Cambios en `examples/web-host`: verificación con Playwright.

## Criterios de parada

- **T1:** algún nodo recortado en profundidad, o con más de 21 bits relativos → parar.
- **T4:** el corte ocupa más de 256 MiB, o hay más del doble de instancias en pantalla que en la ruta CPU en la pose rasante → replantear (nivel por octante) y volver al propietario.
- **T4/T9:** `DescendantTooFine` en más del 5 % de las actualizaciones de corte → rediseñar la máscara.
- **T5:** más de 16,7 ms de GPU con los recuentos de T4 → revisar ADR 0019.
- **T11:** cualquiera de estos → parar:
  - racha de `RetainedPrevious` de más de 1 s;
  - `poolOverflow` > 0;
  - zoom de 100 ms o más;
  - más de 1 GiB de GPU o GTT.

## Tarea 0: línea base y fixtures

- [ ] Nueva línea base de la ruta CPU sobre v7.5: Majadas a 4 px y Belvedere a 4 y 8 px, 3 rondas.
- [ ] Fixtures paginados v7.4/v7.5 con estos casos:
  - nube con CRS y raíz en nivel 0;
  - nube local;
  - hoja de nivel 2 con profundidad 16.
- [ ] **Pasa si** las medidas quedan en un registro fechado y los fixtures se abren por la ruta paginada.

## Tarea 0b: espaciado de referencia en v7.5 (R3-Q4)

- [ ] Nuevo campo en el bloque de metadatos de `v7.5-pre` (spec, códec, conversor y `verify`), con semillas de fuzz y SHA nuevos.
- [ ] Regenerar `target/render-data/*-v75`.
- [ ] **Pasa si** hay ida y vuelta, paridad web/CLI y el valor es estable al injertar páginas.

## Tarea 1 (reabierta): celdas por nodo

- [ ] Claves `u64`, Morton de 64 bits relativo al nodo, firma `&[PointBlock]`, `intensity_count` y clase `i16` (−1 si falta).
- [ ] Pirámide interna de +3 a +8, podada. Las hojas se reconocen por `has_children` = 0.
- [ ] Bench sobre todos los nodos: celdas por nivel, bytes y nodos recortados.
- [ ] **Pasa si:**
  - no hay ningún nodo recortado;
  - las sumas coinciden con `accumulate_block`;
  - el color es igual al de la ruta CPU.

## Tarea 2: oráculo de refinamiento

- [ ] `refinement_relation_holds` con las precondiciones de A8, ahora con pirámide.
- [ ] **Pasa si** se cumple sin excepciones nuevas y los dorados existentes no cambian.

## Tarea 3: `cell_cut`

- [ ] Elegir el nivel de cada nodo.
- [ ] Máscara de 8³ por nodo, construida solo con descendientes residentes. `DescendantTooFine` para descendientes más allá de +3.
- [ ] Una profundidad podada se resuelve con la menor guardada ≥ la deseada.
- [ ] **Pasa si** el volumen visible es igual al cubierto, sin solapes, en los fixtures de T0.

## Tarea 4: spike de cobertura (v7.5)

- [ ] Majadas y Belvedere en órbita, en pose rasante y desde dentro de una hoja, a 1, 2, 4 y 8 px. Comparar la pirámide con 256³ solo.
- [ ] Registrar:
  - instancias totales y en pantalla;
  - píxeles vacíos;
  - bytes del corte;
  - frecuencia de `DescendantTooFine`.
- [ ] **Pasa si** no salta ningún criterio de parada de T4.

## Tarea 5: spike de GPU

- [ ] Con los recuentos de T4, comparar un `draw` por nodo con un `draw` único.
- [ ] **Pasa si** la GPU tarda 8 ms o menos con Belvedere a 4 px.

## Tarea 6: instancia y tabla de páginas

- [ ] 10 bits por eje relativos a la página. Las páginas se parten en orden Morton si el rango supera 2¹⁰. `PageEntry` lleva el origen relativo a la cámara y el desplazamiento dentro del nodo.
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
