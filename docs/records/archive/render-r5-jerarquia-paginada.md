# R5 — Jerarquía paginada: plan de implementación

Estado: completado · 02-11-2026

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Objetivo:** el coste de abrir una nube deja de crecer con el número total de nodos. El índice se lee por páginas bajo demanda, en local y por HTTP Range. Va en la revisión `v7.5-pre`, que incluye también los flags LAS y los Extra Bytes (RF-24).

**Por qué:** con el índice entero, el primer frame remoto (RQ-04) falla hacia 70 GB, el local (RQ-03) hacia 90 GB y el límite del lector de 256 MiB se alcanza hacia 150 GB. Antes de 1 TB revienta la memoria wasm32 ([ADR 0030](../../adr/0030-jerarquia-paginada-por-anclas.md)).

**Spec:** [ADR 0030](../../adr/0030-jerarquia-paginada-por-anclas.md), [ADR 0029](../../adr/0029-octree-global-por-bits-de-enteros.md), [ADR 0022](../../adr/0022-alcance-y-congelacion-de-v7-final.md), [ADR 0023](../../adr/0023-fuentes-locales-y-remotas.md); [spec v7.4-pre](../../reference/format/FORMAT_V7_PRE4.md); [spec v7.5-pre](../../reference/format/FORMAT_V7_PRE5.md); [spike P0](../benchmarks/R5_P0_PAGES_02-11-2026.md); requisitos RF-24, RQ-02, RQ-03, RQ-04, RQ-08.

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

- [x] Spec [`FORMAT_V7_PRE5.md`](../../reference/format/FORMAT_V7_PRE5.md):
  - cabecera de página (nivel del ancla, prefijo, *k*, número de nodos, CRC32);
  - registro reducido y punteros a páginas hijas;
  - página raíz junto al pie, con una lectura de cola que traiga pie y raíz en una sola petición;
  - derivación de `NodeId` (página, índice), `BlockId` y del orden de los hijos.
- [x] Validación por página: el ancla de la página hija coincide con el nodo que la apunta. Límites `max_page_bytes` y `max_resident_pages` en `FormatReaderLimits`. Ningún `BlockId` se repite entre páginas.
- [x] Códec, escritor y lector de páginas en `viewer_format`; fuzz con varias páginas encadenadas (`fuzz_paged_reader`, [semillas](../benchmarks/R5_V75_REFERENCE_SHA_02-11-2026.md)).
Resultado parcial (02-11-2026): `FormatVersion::V7_PRE5`, `PagedReader` (lectura de cola: pie, metadatos y raíz en una petición) y escritor con *k* por página en `viewer_format`; con el mismo contenido el índice mide el 39 % del de `v7.4-pre` en el fixture de los tests. Queda la segunda mitad.
- [x] El conversor escribe las páginas en anchura después de los datos, eligiendo *k* por página.
- [x] Subcomando `viewer_convert verify` con las comprobaciones globales (hoy el CLI solo tiene flags sueltos, `main.rs:28-80`).
- [x] Generador de un `.oct` sintético `v7.5-pre` con 6 M nodos.
- [x] **Pasa si** el índice es al menos un 30 % menor que el de `v7.4-pre` y hay paridad web/CLI.
Resultado (02-11-2026, revisado el mismo día por dos decisiones del propietario): `--format-version v7.5-pre` escribe la pareja `.oct` + `.octattrs` como v7.4 ([ADR 0001](../../adr/0001-pareja-separada-oct-octattrs.md); la geometría, las páginas y el índice van en el `.oct`, que abre solo en gris) y el id de nodo es `(página, índice)` sin límite de profundidad. EPT, checkpoints y Additive dan error explícito. Índice (release, `perf` no usado):

| Dataset | Índice v7.4 | Índice v7.5 | Reducción | Páginas | Página máx. | Conversión | RSS |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Belvedere | 2 405 003 B | 1 116 491 B | 53,6 % | 152 (todas *k* = 5) | 77 540 B | 46 s | 104 MiB |
| Majadas | 13 515 427 B | 6 065 569 B | 55,1 % | 810 (todas *k* = 5) | 272 898 B | 6 min 52 s (con `check.sh` en paralelo) | 230 MiB |

`verify` pasa en ambos (también comprueba el pie del `.octattrs`). Generador: `cargo run --release -p viewer_format --example synthetic_paged -- OUT.oct` (6 M nodos, `.oct` de 761 914 737 B más `.octattrs`, 63 s, 15 GB RAM; `verify` pasa). Paridad web/nativo: casos `small-v75` (el LAS que cruza el bit de signo, raíz en nivel 0 y cadena de ~40 niveles, ya convierte y verifica) y `synthetic-v75` en `tools/convert_parity`. Hallazgos para P3: (1) `FormatReaderLimits::max_nodes` por defecto (2 M) rechaza al abrir un fichero de 6 M nodos; (2) los ids de bloque y de nodo del fichero dependen del plan de páginas, no del plan del conversor; (3) la decodificación de la celda de bloque desbordaba `i64` con raíces de nivel 0 (corregido en `page_codec`).

## Tarea P2a: flags LAS y Extra Bytes (RF-24)

Decisión (02-11-2026): el criterio de parada se activó (toca `viewer_convert_core`, `viewer_convert`, `viewer_core` y `viewer_wasm`). El propietario autoriza hacerlo dentro de `v7.5-pre` para mantener una sola regeneración, **después de P3**. Diseño: columna `las_flags` u8 (bit 0 synthetic, 1 key-point, 2 withheld, 3 overlap) y una columna tipada por Extra Byte; descriptores en la extensión de metadatos; tipos obsoletos o que no caben → error explícito.

- [x] Llevar los flags y los Extra Bytes al spool (`SpoolLayout`), al esquema (≤ 256 atributos) y a la conversión web (la web usa el mismo núcleo serie de `viewer_convert_core`).
- [x] **Pasa si** una ida y vuelta conversor → lector devuelve los flags y los Extra Bytes exactos. Mientras no exista exportador LAS, esta es la verificación.

Resultado (02-11-2026): implementado según el diseño; formato en [FORMAT_V7_PRE5](../../reference/format/FORMAT_V7_PRE5.md#flags-las-y-extra-bytes-rf-24). Extra Bytes de hasta 64 B por punto (el registro del spool los lleva en línea). Pruebas de ida y vuelta en `viewer_convert` (`las_extra_tests.rs`: formatos 3 y 6, escalado, no escalado, array, tipos rechazados, más de 256 atributos, formatos anteriores sin cambios) y paridad núcleo serie (ruta web) frente a CLI. Pendiente: caso de paridad en wasm32 real (`tools/convert_parity`, necesita un LAS generado) y verificación en navegador.

## Tarea P3: lector perezoso local

- [x] Fuente de páginas sobre `read_at` / `Blob.slice`, con un **presupuesto propio para páginas** (64 MiB, separado del de bloques), y caché LRU con las páginas fijadas según P1.
- [x] Migrar las rutas de apertura (`open_format` síncrono incluido: lee `v7.5-pre` en perezoso y las vistas leen y injertan páginas con `update_lod_view_paged`) y los usos de `nodeCount` / `maxLevel`. Eliminar el descriptor JS por nodo.
- [x] Los rangos globales de atributos van en el bloque de metadatos (cambia los bytes de `v7.5-pre`: los ficheros anteriores se regeneran); `verify` y el lector completo los comparan con las páginas.
- [x] **Pasa si** el tiempo de apertura está acotado por el tamaño de la página raíz (≤ 512 KB) y no crece con el total de nodos, y la matriz de T7 no empeora (verificación con Playwright incluida). Criterio reformulado por el propietario el 02-11-2026: el original («6 M nodos ≤ 2 × 24 k nodos») daba 2,3 × contra Majadas y 1,25 × contra Belvedere porque compara páginas raíz de distinto tamaño.

Resultado (02-11-2026, release, Chrome headless, equipo de T6b):

- Ficheros con páginas pequeñas (`viewer_convert --page-target-bytes N`, opción oculta para pruebas y benchmarks): Belvedere con *k* = 1 (767 páginas, raíz de 17 nodos) y con 16 KB; Majadas con 16 KB (1 652 páginas, raíz de 37 nodos).
- Belvedere *k* = 1: a 8 px se injertan 7 páginas, a 4 px 11; objetivo alcanzado, 0 nodos sin cubrir, vacíos 0,33 (8 px) y 0,31 (4 px) frente a 0,29–0,36 y 0,30–0,38 de T6b; el valor final en reposo (0,5177 a 8 px) es idéntico al de T6b. Con 16 KB la raíz (26 nodos) basta a 8 px y no se piden páginas. Majadas 16 KB a 4 px: raíz suficiente, 0,868 (T6b 0,85–0,87). Consola sin errores (salvo favicon).
- Presión (presupuesto de páginas de 4 KB, compilación temporal): 19 expulsiones, sin errores ni huérfanos, 0 sin cubrir; pero con un presupuesto menor que el conjunto de trabajo queda una ancla pendiente en reposo y la sonda marca el objetivo como alcanzado (la etapa de la sonda no ve las páginas). Con 64 MiB no ocurre. Corregido (02-11-2026): la sonda y el host cuentan `pendingPageAnchors`, `lod.pageBudgetExhausted` marca el presupuesto agotado (estado degradado, sin objetivo alcanzado ni bucle de reintentos hasta que cambie la vista) y el lector olvida la página rechazada; con 4 KB la sonda deja de dar el objetivo por alcanzado y con 64 MiB (8 px) sigue alcanzándolo.
- Apertura nativa (`lazy_open_time`, mediana): sintético 6 M 1,01 ms; Majadas 0,44 ms (razón 2,3); Belvedere 0,81 ms (1,25); Belvedere *k* = 1 0,10 ms. El coste sigue el tamaño de la página raíz (la del sintético es una página completa de *k* = 5), no el total de nodos; no se ajusta el umbral.
- Playwright (escritorio 1280×720 y móvil 390×844): `sample.oct`, `sample-v7.oct`, v7.4 y v7.5, con cambios de fichero, consola limpia.

## Tarea P4: HTTP Range

- [x] Arnés RED-REF (`examples/web-host/red-ref-server.mjs`): servidor con Range y CORS, fijado por `ETag` / `If-Range`. Tiene que detectar un 200 completo cuando el servidor ignora `Range`, y no usar peticiones multirango, que S3 rechaza.
- [x] Fuente HTTP (`http-range-source.mjs`, `?url=` en el visor) con agrupación de páginas hermanas en un solo rango contiguo.
- [x] **Pasa si** se cumple RQ-04 con la latencia de RED-REF. Además se miden los bytes por vista a 4 px (ASM-02).

Resultado (02-11-2026, release, Chrome headless 1280×720, RED-REF 100 Mbit/s y 50 ms de RTT simulados por `red-ref-server.mjs`, datos y app en orígenes distintos; sonda `probe-remote-open.mjs`):

- Diseño: la fuente HTTP imita un `Blob` (`size`, `slice().arrayBuffer()`) y cada worker abre la suya. Apertura con una sola petición de sufijo (`Range: bytes=-1 MiB`: tamaño, ETag, pie, metadatos y raíz); el `.octattrs` pide su cabecera y su pie en paralelo. Después, un rango simple por petición con `If-Range: <ETag>`; las lecturas del mismo tick cuyos rangos se tocan se unen en una petición (hermanas contiguas). Un 200 ante un `Range`, un ETag distinto o un `Content-Range` incoherente son error (el cuerpo se aborta). Sin multirango.
- Medidas (primer frame / objetivo 4 px / peticiones GET hasta el primer frame / GET totales / bytes de cuerpo):

| Fichero | Primer frame | 4 px | GET hasta 1.er frame | GET totales | Bytes |
| --- | --- | --- | --- | --- | --- |
| belvedere-v75 | 377 ms | 954 ms | 11 | 23 | 2,05 MB |
| belvedere-v75k1 (767 páginas) | 390 ms | 1 253 ms | 11 | 24 | 2,06 MB |
| belvedere-v74 (línea base) | 784 ms | 1 500 ms | 13 | 25 | 4,46 MB |
| majadas-v75 | 370 ms | 861 ms | 7 | 27 | 1,89 MB |
| majadas-v74 (línea base) | 2 195 ms | 2 883 ms | 9 | 29 | 15,4 MB |

- Criterios de parada: la paginación no añade peticiones al primer frame (v7.5 abre con 1 petición `.oct` frente a 3 de v7.4: cola, cabecera e índice). Con las páginas por defecto (*k* = 5) la raíz basta a 4 px (0 lecturas de página); con *k* = 1 hacen falta 4 lecturas en 1 oleada (≤ 3). RQ-04 (≤ 10 s / ≤ 120 s) se cumple con holgura. Bytes por vista a 4 px (ASM-02): 1,9–2,1 MB, de los cuales 1 MiB es la lectura de cola.
- Escritorio y móvil (390×844): se abre `belvedere-v75.oct` por URL, objetivo alcanzado, consola sin errores salvo el 404 de `favicon.ico` del servidor estático; con `--ignore-range` el visor informa `HttpRangeError: the server ignores Range` y aborta.
- Límites: vista fija sin órbita (no es la matriz T7); no se probó un S3 real ni un servidor con compresión; hueco de unión de rangos 0 (solo rangos contiguos).

## Tarea P5: documentación

- [x] Actualizar el inventario (`docs/reference/inventory.md`) y los límites vigentes de v7.5-pre. ADR 0007 no se edita (aceptada): sus límites de nodos y bloques siguen vigentes para el lector eager; los de páginas están en la [spec](../../reference/format/FORMAT_V7_PRE5.md#limits) y el inventario.

Resultado (02-11-2026): inventario, README y registro [R5-P4](../benchmarks/R5_P4_HTTP_RANGE_02-11-2026.md) al día. R5 queda implementado como `v7.5-pre` en la rama `feat/r5-paged-hierarchy`, pendiente de integración.

## Fuera de este plan

OQ-07 (profundidad > 24 en el visor; va con R3), la fusión incremental de nubes (RF-21), la reproyección con PROJ y un exportador LAS.
