# Evaluación de carga y render a escala (30-10-2026)

Registro fechado. Objetivo del propietario (30-10-2026): cargar nubes de **cientos de GB** y
dibujarlas con un tamaño de vóxel en pantalla de **1–8 px**, con una carga que se perciba continua y
natural (sin saltos, huecos ni bloqueos). Fuentes: revisión de arquitectura del código (líneas de
`c38cf47`), registros ya mergeados e investigación externa. **[E]** = evidencia en código o registro;
**[I]** = inferencia. Plan derivado: [render y carga a escala](../../explanation/render-y-carga-escala.md).

## Conclusión

La cadena funciona y es robusta ante fallos (retiene el corte visible, respeta presupuestos), pero
está lejos del objetivo: con datasets de 1–4 GB tarda 29–157 s en llegar a 4–8 px y Belvedere no
completa 4 px en 600–900 s. Dos causas estructurales dominan: la **voxelización en CPU dependiente
de la vista** sobre el corte completo y la **pirámide LOD sin densidades intermedias**. Hay mejoras
pequeñas que quitan saltos y bloqueos sin decisiones del propietario; las estructurales sí las piden.

## Lo que ya está medido

| Escenario (Firefox headless, WebGPU) | Resultado | Fuente |
| --- | --- | --- |
| Majadas XYZ, 4 px | objetivo en 54–62 s, 239 bloques, 0 pendientes; PSS 0,97–3,73 GB sin explicar | [web](../benchmarks/WEB_PERFORMANCE_BENCH.md) |
| Albuera RGB, 4 px | objetivo en 115–140 s, 746 bloques | ídem |
| Belvedere RGB, 8 px | objetivo en 122–251 s; 94 de 122 s en construcción de vóxeles | ídem |
| Belvedere, 4 px, 600 s | **sin completar**: 783 pendientes, 87 expulsiones | ídem |
| Belvedere, caché 1 GiB, 900 s | 357 pendientes; 837 de 900 s en construcción | ídem |

Sin medir: FPS de GPU y de pantalla (el «FPS» registrado es cadencia de respuestas del worker),
heap JS/wasm, VRAM, lecturas en frío, zoom/movimiento visual, Chrome con GPU real en carga larga.

## Mapa de la cadena

```mermaid
flowchart LR
  F[.oct/.octattrs local] -->|Blob.slice| R[reader async: índice residente completo]
  R -->|descriptor JS por nodo| H[SpatialHierarchy]
  H --> S[selección LOD] --> P[plan de transición] --> Q[prioridad de pendientes]
  Q --> SCH[scheduler: 4 lecturas] -->|lectura + decodificación en el mismo worker| C[caché decodificada LRU]
  C --> V[voxelización CPU del corte completo] --> U[subida completa de instancias] --> D[cubos instanciados; HZB solo con cámara quieta]
```

Todo corre en un único worker serializado (`viewer-worker.js:530-533`) [E].

## Hallazgos

| # | Gravedad | Problema | Evidencia |
| --- | --- | --- | --- |
| B1 | crítica | Voxelización en CPU, por vista, del corte completo y síncrona dentro de `renderFrame`; además se reconstruye por cada bloque que llega (98 builds para 239 cargas en Majadas) | `voxel_grid.rs:543-631`, `bindings/render.rs:131`, `viewer-worker.js:255-260` [E] |
| B2 | crítica | Pirámide LOD con 8³ representantes en nodos internos frente a hojas de hasta 262 144 puntos: ~4 niveles de densidad colapsados; 1 100–2 700 puntos leídos por vóxel dibujado | `node_job.rs:46`, `options.rs:5`; bench Belvedere/Majadas [E] |
| B3 | mayor | Selección, plan y prioridad sin estado y supralineales (`position`/`splice` en la frontera; recorrido de todos los nodos visibles), ejecutados 2–3 veces por actualización | `spatial_lod_priority.rs:274-299`, `lod_dataset.rs:767-787`, `bindings/lod.rs:447-460` [E] |
| B4 | mayor | Un solo hilo para decodificación, LOD, construcción y dibujo | `viewer-worker.js:530`, `async_format_reader.rs:462` [E] |
| B5 | mayor | Cada órbita, desplazamiento o zoom reinicia la etapa a 16 px: salto visible a grueso y re-refinado | `bindings/camera.rs:20-63`, `bindings/lod.rs:447-460` [E] |
| B6 | mayor | Índice monolítico con triple copia (reader → objeto JS por nodo → HashMap); límite de 2 M nodos | `async_format_reader.rs:264-301`, `reader_limits.rs:8-9` [E] |
| B7 | menor | Tres métricas de error distintas (selección por esfera con `lado/8`, arista proyectada del builder, `geometric_error` del fichero, 0 en hojas); plano cercano ≥ 0,001·radio; el README dice lecturas «síncronas» | `lod_dataset.rs:320-323`, `voxel_grid.rs:170-176`, `camera.rs:454-456` [E] |

Escala [I]: 500 GB a 10–19,5 B/punto son 26–50 G puntos y 0,4–0,8 M bloques; a 1 px en 4 MP la
frontera tendría ~6·10⁴ nodos, inviable con B3.

## Investigación externa (resumen)

| Tema | Hallazgo | Fuente |
| --- | --- | --- |
| Refinado | 3D Tiles: REPLACE dibuja hijos en lugar del padre, ADD además del padre; evitar huecos es comportamiento del cliente, no del formato | [3D Tiles](https://github.com/CesiumGS/3d-tiles/blob/main/specification/README.adoc) |
| Presupuestos | CesiumJS: `maximumScreenSpaceError` 16, `cacheBytes` 512 MiB, error en pantalla dinámico | [CesiumJS 1.133](https://cesium.com/downloads/cesiumjs/releases/1.133/Build/Documentation/Cesium3DTileset.html) |
| Paginación | COPC: jerarquía en páginas (entradas de 32 B, `pointCount = -1` → página hija); Potree 2: `hierarchy.bin` con nodos proxy | [copc.io](https://copc.io/) |
| Continuidad | LOD continuo por punto (Schütz 2019) reduce el pop; histéresis de 10–20 % en motores; geomorphing o fundido | [CLOD](https://www.cg.tuwien.ac.at/research/publications/2019/schuetz-2019-CLOD/) |
| Rasterización | Compute con atómicos de 64 bits: ~2 G puntos a 60 fps en nativo; en WebGPU no hay atómicos de 64 bits en navegadores (vía factible: pase de profundidad de 32 bits + resolución de color) | [Schütz 2022](https://arxiv.org/abs/2204.01287), [gpuweb #5071](https://github.com/gpuweb/gpuweb/issues/5071) |
| Primitiva | `point-list` en WebGPU es siempre 1 px: tamaños mayores requieren quads instanciados o compute | [Babylon.js](https://forum.babylonjs.com/t/webgpu-and-point-size/45212) |
| Límites | Por defecto `maxBufferSize` 256 MiB y `maxStorageBufferBindingSize` 128 MiB; no se puede consultar la VRAM; memoria wasm32 4 GiB (memory64 en Chrome 133 y Firefox 134, no en Safari) | [MDN](https://developer.mozilla.org/en-US/docs/Web/API/GPUSupportedLimits), [W3C](https://www.w3.org/TR/webgpu/#limits) |
| Subidas | Patrón de anillo de staging y presupuesto de tiempo por frame (Unity: *time slice*); `writeBuffer` frente a `mapAsync` sin benchmark público fiable | [Unity](https://docs.unity.com/en-us/engine/6000.3/manual/analysis/graphics-performance-profiling/loading-texture-mesh-data-asynchronously/configure-asynchronous-upload-pipeline.md) |

No se encontraron presupuestos de puntos por frame verificados para visores WebGPU ni márgenes de
histéresis publicados para nubes de puntos.

## Mejoras sin decisión del propietario

| # | Cambio | Impacto | Verificación |
| --- | --- | --- | --- |
| Q1 | No reiniciar la etapa al mover: conservar el corte y refinar desde él | Quita el pop a grueso | `lodStagePixels` nunca sube durante una órbita |
| Q2 | Agrupar llegadas y sacar la reconstrucción de `renderFrame` con presupuesto de tiempo | Quita bloqueos de frame | p95 de la llamada de render en carga < 16 ms (Majadas 4 px) |
| Q3 | Reutilizar la selección de `target_cut_is_resident` | −50 % selección | `refinementWork` |
| Q4 | Prioridad con la métrica de selección y solo sobre pendientes | O(pendientes) | `planningMs` con 1 M nodos sintéticos |
| Q5 | Frontera indexada en lugar de `position`/`splice` | O(F log F) | bench con F = 10⁵ |
| Q6 | Punto más cercano del AABB en lugar de la esfera | Menos sobre-lectura | mismo objetivo de arista con menos puntos |
| Q7 | Jerarquía construida en wasm desde el reader, sin descriptor JS | Memoria y apertura | heap JS y ms de metadatos con 1 M nodos |
| Q8 | Plano cercano según el nodo visible más cercano | Primeros planos | vista pájaro a 1 m |

## Decisiones que requieren al propietario

Recogidas en [preguntas pendientes](../../explanation/preguntas-propietario.md): primitiva de render
(P1), hardware mínimo (P2), reconversión con pirámide densa (P3), huecos frente a continuidad (P4),
calidad dinámica 8 px → 1–2 px (P5), transporte remoto y cabeceras COOP/COEP (P6).

## Límites

Revisión estática más registros previos (una a tres ejecuciones por escenario, Firefox headless);
sin medida nueva en esta evaluación. Fuentes externas de rango 1–5 según el investigador; las cifras
de terceros no se reprodujeron.
