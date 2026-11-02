# R5 — Jerarquía paginada: plan de implementación

Estado: listo para ejecutar · 02-11-2026

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Objetivo:** el coste de abrir una nube deja de crecer con el número total de nodos. El índice se lee por páginas bajo demanda, en local y por HTTP Range. Va en la revisión `v7.5-pre`, que incluye también los flags LAS y los Extra Bytes (RF-24).

**Por qué:** con el índice entero, el primer frame remoto (RQ-04) falla hacia 70 GB, el local (RQ-03) hacia 90 GB y el límite del lector de 256 MiB se alcanza hacia 150 GB. Antes de 1 TB revienta la memoria wasm32 ([ADR 0030](../adr/0030-jerarquia-paginada-por-anclas.md)).

**Spec:** [ADR 0030](../adr/0030-jerarquia-paginada-por-anclas.md), [ADR 0029](../adr/0029-octree-global-por-bits-de-enteros.md), [ADR 0022](../adr/0022-alcance-y-congelacion-de-v7-final.md), [ADR 0023](../adr/0023-fuentes-locales-y-remotas.md); [spec v7.4-pre](../reference/format/FORMAT_V7_PRE4.md); [spec v7.5-pre](../reference/format/FORMAT_V7_PRE5.md); [spike P0](../records/benchmarks/R5_P0_PAGES_02-11-2026.md); requisitos RF-24, RQ-02, RQ-03, RQ-04, RQ-08.

## Resoluciones del propietario (02-11-2026)

| Id | Resolución |
| --- | --- |
| R5-Q1 | Páginas por niveles del octree desde anclas de prefijo, con nodos direccionados por (profundidad `u8`, camino `u16`) (OQ-06) |
| R5-Q2 | *k* por página (1–5), el mayor que deja la página ≤ 512 KB |
| R5-Q3 | Validación por página al abrir; las comprobaciones globales pasan a `viewer_convert verify` |
| R5-Q4 | `v7.5-pre` incluye los flags LAS y los Extra Bytes (RF-24): una sola regeneración |
| R5-Q5 | Se rechaza la compresión de cadenas (OQ-05) |

## Restricciones globales

- `scripts/check.sh` en verde al cerrar cada tarea.
- V5 sigue siendo la salida por defecto, y v5 y `v7.1–4-pre` se siguen leyendo con sus bytes y dorados intactos.
- Paridad web/CLI byte a byte (RQ-08).
- `FormatVersion::V<n>` solo dentro de `viewer_format`.
- Los cambios en `lod_dataset.rs` (el fichero con más churn) se hacen de uno en uno.
- Los cortes se comparan por (nivel, prefijo), no por `NodeId`: los ids pasan a derivarse.
- v7.5-pre rechaza de forma explícita la entrada EPT, los checkpoints y Additive.

## Criterios de parada

- P1: con todas las páginas cargadas, cualquier diferencia de corte LOD (por nivel y prefijo) o de digest de escena frente a la jerarquía actual → parar.
- P1/P3: la etapa llega al objetivo con páginas pendientes, o `hierarchy.node()` devuelve `None` para un nodo del corte tras una expulsión → parar.
- P2: índice paginado mayor que el de `v7.4-pre`, o una página con *k* = 1 por encima de 1 MiB → parar.
- P2a: la ruta de datos de RF-24 toca más de 2 crates además de `viewer_format` → llevarla a una revisión propia, con el visto bueno del propietario.
- P3: regresión de nodos sin cubrir o de píxeles vacíos (RQ-02), o una reserva de página falla con menos del 10 % del presupuesto fijado → parar y replantear el presupuesto.
- P4: la paginación añade más de una petición al primer frame, o llegar a 4 px en RED-REF necesita más de 3 viajes de página en serie → revisar cómo se elige *k*.

Pre-mortem (02-11-2026) integrado: sus fallos 1–8 tienen tarea o criterio en este plan.

## Tarea P1: jerarquía que crece y encoge

- [x] `SpatialHierarchy` detrás de una abstracción con subárboles desconocidos, que crece al llegar una página y encoge al expulsarla.
- [x] `LodSelection` distingue `unloaded_anchors` de `resolution_limited`. `next_lod_stage` y `resident_target_plan_is_complete` (`lod_dataset.rs`) cuentan las páginas pendientes. Las páginas que faltan se piden con la prioridad de error proyectado.
- [x] Solo se expulsa una página si no tiene páginas descendientes residentes ni nodos en el corte retenido del planner (`lod_transition.rs` sube por `parent()`).
- [x] Tests:
  - con todas las páginas cargadas, el corte y los digests son iguales a los de hoy;
  - con páginas retenidas, la etapa no baja de la inicial y se pide la página;
  - expulsar páginas bajo presión no deja nodos huérfanos.
Resultado (02-11-2026): API `mark_unloaded` / `graft_page` / `evict_page` en `SpatialHierarchy`; selección y etapas con anclas pendientes; dorados sin cambios. Pasa a P3: `LodDataset` mutable (hoy `Arc<DatasetMetadata>`), política de expulsión, exponer `pending_page_anchors` a JS y descartar anclas sin puntos. La comparación por (nivel, prefijo) pasa a P2.

## Tarea P2: `v7.5-pre`, páginas

- [x] Spec [`FORMAT_V7_PRE5.md`](../reference/format/FORMAT_V7_PRE5.md):
  - cabecera de página (nivel del ancla, prefijo, *k*, número de nodos, CRC32);
  - registro reducido y punteros a páginas hijas;
  - página raíz junto al pie, con una lectura de cola que traiga pie y raíz en una sola petición;
  - derivación de `NodeId`, `BlockId` y del orden de los hijos.
- [x] Validación por página: el ancla de la página hija coincide con el nodo que la apunta. Límites `max_page_bytes` y `max_resident_pages` en `FormatReaderLimits`. Ningún `BlockId` se repite entre páginas.
- [x] Códec, escritor y lector de páginas en `viewer_format`; fuzz con varias páginas encadenadas (`fuzz_paged_reader`, [semillas](../records/benchmarks/R5_V75_REFERENCE_SHA_02-11-2026.md)).
Resultado parcial (02-11-2026): `FormatVersion::V7_PRE5`, `PagedReader` (lectura de cola: pie, metadatos y raíz en una petición) y escritor con *k* por página en `viewer_format`; con el mismo contenido el índice mide el 39 % del de `v7.4-pre` en el fixture de los tests. Queda la segunda mitad.
- [ ] El conversor escribe las páginas en anchura después de los datos, eligiendo *k* por página.
- [ ] Subcomando `viewer_convert verify` con las comprobaciones globales (hoy el CLI solo tiene flags sueltos, `main.rs:28-80`).
- [ ] Generador de un `.oct` sintético `v7.5-pre` con 6 M nodos.
- [ ] **Pasa si** el índice es al menos un 30 % menor que el de `v7.4-pre` y hay paridad web/CLI.

## Tarea P2a: flags LAS y Extra Bytes (RF-24)

- [ ] Llevar los flags y los Extra Bytes al spool (`SpoolLayout`), al esquema (≤ 256 atributos) y a la conversión web. Hoy se descartan (`source.rs:262-276`).
- [ ] **Pasa si** una ida y vuelta conversor → lector devuelve los flags y los Extra Bytes exactos. Mientras no exista exportador LAS, esta es la verificación.

## Tarea P3: lector perezoso local

- [ ] Fuente de páginas sobre `read_at` / `Blob.slice`, con un **presupuesto propio para páginas**, separado del de bloques, y caché LRU con las páginas fijadas según P1.
- [ ] Migrar las tres rutas de apertura (`bindings/lod.rs:118`, `viewer-worker.js:743`, `build-worker.js:266`) y los usos de `nodeCount` / `maxLevel` (`main.js:483-486` y tests de navegador). Eliminar el descriptor JS por nodo.
- [ ] **Pasa si** abrir el sintético de 6 M nodos tarda como máximo el doble que una nube de 24 k nodos, y la matriz de T7 no empeora (verificación con Playwright incluida).

## Tarea P4: HTTP Range

- [ ] Arnés RED-REF: servidor con Range y CORS, fijado por `ETag` / `If-Range`. Tiene que detectar un 200 completo cuando el servidor ignora `Range`, y no usar peticiones multirango, que S3 rechaza.
- [ ] Fuente HTTP con agrupación de páginas hermanas en un solo rango contiguo.
- [ ] **Pasa si** se cumple RQ-04 con la latencia de RED-REF. Además se miden los bytes por vista a 4 px (ASM-02).

## Tarea P5: documentación

- [ ] Actualizar el inventario (`docs/reference/inventory.md`), los límites de ADR 0007 y la spec.

## Fuera de este plan

OQ-07 (profundidad > 24 en el visor; va con R3), la fusión incremental de nubes (RF-21), la reproyección con PROJ y un exportador LAS.
