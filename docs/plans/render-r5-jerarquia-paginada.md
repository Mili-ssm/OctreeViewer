# R5 — Jerarquía paginada: plan de implementación

Estado: borrador · 02-11-2026

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Objetivo:** el coste de abrir una nube deja de crecer con el número total de nodos. El índice se lee por páginas bajo demanda, en local y por HTTP Range. Va en la revisión `v7.5-pre`, que incluye también los flags LAS y los Extra Bytes (RF-24).

**Por qué:** con el índice entero, el primer frame remoto (RQ-04) falla hacia 70 GB, el local (RQ-03) hacia 90 GB y el límite del lector de 256 MiB se alcanza hacia 150 GB. Antes de 1 TB revienta la memoria wasm32 ([ADR 0030](../adr/0030-jerarquia-paginada-por-anclas.md)).

**Spec:** [ADR 0030](../adr/0030-jerarquia-paginada-por-anclas.md), [ADR 0029](../adr/0029-octree-global-por-bits-de-enteros.md), [ADR 0022](../adr/0022-alcance-y-congelacion-de-v7-final.md), [ADR 0023](../adr/0023-fuentes-locales-y-remotas.md); [spec v7.4-pre](../reference/format/FORMAT_V7_PRE4.md); [spike P0](../records/benchmarks/R5_P0_PAGES_02-11-2026.md); requisitos RF-24, RQ-02, RQ-03, RQ-04, RQ-08.

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

## Criterios de parada

- P1: cualquier diferencia de corte LOD o de digest de escena frente a la jerarquía actual → parar.
- P2: índice paginado mayor que el de `v7.4-pre`, o una página con *k* = 1 por encima de 1 MiB → parar.
- P3: regresión de nodos sin cubrir o de píxeles vacíos (RQ-02) → parar.
- P4: la paginación añade más de una petición al primer frame → parar.

## Tarea P1: jerarquía que crece

- [ ] `SpatialHierarchy` detrás de una abstracción que admite subárboles desconocidos («hijos desconocidos») y que puede crecer al llegar una página.
- [ ] La selección de LOD trata un ancla sin página como hoja provisional y pide su página con la prioridad de error proyectado.
- [ ] **Pasa si** el corte LOD y los digests de escena son iguales a los de hoy con todas las páginas cargadas.

## Tarea P2: `v7.5-pre`

- [ ] Spec `FORMAT_V7_PRE5.md`:
  - cabecera de página (nivel del ancla, prefijo, *k*, número de nodos, CRC32);
  - registro reducido y punteros a páginas hijas;
  - página raíz junto al pie;
  - flags LAS y Extra Bytes.
- [ ] Códec, escritor y lector de páginas en `viewer_format`, con un target de fuzz para páginas.
- [ ] El conversor escribe las páginas en anchura después de los datos, eligiendo *k* por página.
- [ ] `viewer_convert verify`, con las comprobaciones globales.
- [ ] **Pasa si** el índice es al menos un 30 % menor que el de `v7.4-pre`, la exportación devuelve los flags y los Extra Bytes, y hay paridad web/CLI.

## Tarea P3: lector perezoso local

- [ ] Fuente de páginas sobre `read_at` / `Blob.slice`, con caché LRU y páginas fijadas mientras sus nodos estén seleccionados o residentes. Se elimina el descriptor JS por nodo.
- [ ] **Pasa si** abrir una jerarquía sintética de 6 M nodos tarda como máximo el doble que una de 24 k nodos, y la matriz de T7 no empeora.

## Tarea P4: HTTP Range

- [ ] Fuente HTTP con peticiones de rango agrupadas (varias páginas hermanas en una).
- [ ] **Pasa si** se cumple RQ-04 con la latencia de RED-REF. Además se miden los bytes por vista a 4 px (ASM-02).

## Fuera de este plan

OQ-07 (profundidad > 24 en el visor; va con R3), la fusión incremental de nubes (RF-21) y la reproyección con PROJ.
