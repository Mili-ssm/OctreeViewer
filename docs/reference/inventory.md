# Inventario del proyecto — 21-11-2026

Mapa de referencia del código, manifests, pruebas e informes de este checkout. Estado y
prioridades activas: [estado](../estado.md); índice de documentos: [docs/README.md](../README.md).
«Implementado» indica una ruta de código existente; «medido» remite a una ejecución fechada;
«experimental» no implica integración en el visor. Este documento no ejecuta benchmarks ni
certifica el checkout; los checks se ejecutan con `scripts/check.sh` ([CONTRIBUTING](../../CONTRIBUTING.md)).

## Objetivo y flujo

LAS/LAZ → conversión nativa → pareja versionada `.oct`/`.octattrs` → lectura selectiva →
selección LOD/caché → vóxeles → WebGPU en un worker → host web.
La V8 es el único formato ([FORMAT_V8](format/FORMAT_V8.md)); EPT y checkpoints se retiraron ([ADR 0061](../adr/0061-ept-y-checkpoints-se-retiran-con-la-v8.md)). COPC puede leerse
como LAS/LAZ secuencial y reparticionarse; no se conserva su jerarquía como acceso COPC nativo.
La conversión web LAS/LAZ es una ruta adicional con salida OPFS: un worker sobre `viewer_convert_core` con los mismos bytes que el CLI y spools en segmentos de JS y zonas OPFS paginadas ([ADR 0017](../adr/0017-zonas-opfs-paginadas-para-spools-web.md)). El conversor anterior (dos pasadas) se eliminó en M3b; `engine: "core"` se acepta sin efecto.
Explorar fuentes mayores de 500 GB sigue siendo objetivo sin validación de extremo a extremo.

## Componentes y puntos de entrada

| Componente | Capacidades presentes | Archivos para empezar |
| --- | --- | --- |
| `viewer_core` | Geometría, marco local, atributos tipados, jerarquía Replacement/Additive, frustum, selección LOD, scheduler y caché; octree ocupado e índice celda→filas; contrato LAS compartido (`las_attribute_schema`, `visual_scale_for_bounds`, Extra Bytes). Sin dependencia de `laz`: el preflight vive en `viewer_convert_core` ([ADR 0062](../adr/0062-el-preflight-las-vive-en-el-nucleo-de-conversion.md)). Con la feature `experimental`: `node_attributes`, `categorical_attribute`, `occupied_point_block`. | [Exports](../../crates/viewer_core/src/lib.rs), [LAS](../../crates/viewer_core/src/las.rs), [LOD/transición](../../crates/viewer_core/src/lod_transition.rs), [prioridad](../../crates/viewer_core/src/spatial_lod_priority.rs). |
| `viewer_format` | Política de versiones (`FormatVersion::DEFAULT_OUTPUT`, `WRITABLE`), identidades exactas, cuantización u32, raw/LZ4, CRC, pareja separada, lecturas XYZ/RGB/columna/fila/rangos, lectores independientes con índice compartido y reader async (feature `async-reader`, solo no-wasm). Jerarquía paginada ([ADR 0030](../adr/0030-jerarquia-paginada-por-anclas.md)) por niveles desde anclas de prefijo, con *k* por página (`paged_index.rs`), `PagedReader` perezoso (lectura de cola: pie, metadatos y página raíz en una petición; las demás páginas bajo demanda sobre cualquier fuente de lectura por rangos) y `FormatReader` eager, que las lee todas y ejecuta las comprobaciones globales. Con `experimental`: `experimental_tuple_dictionary`. | [Versiones](../../crates/viewer_format/src/version.rs), [layouts](../../crates/viewer_format/src/layout.rs), [contratos](../../crates/viewer_format/src/contracts.rs), [lector](../../crates/viewer_format/src/storage_reader.rs), [límites del reader](../../crates/viewer_format/src/reader_limits.rs), [páginas](../../crates/viewer_format/src/paged_index.rs). Módulos de apoyo: `index_codec`, `retired` (error de reconvertir ante una familia retirada), `prepare`, `slice_cursor`. |
| `viewer_convert_core` | Núcleo puro de la conversión, compartido por el CLI y el navegador: opciones, errores, inspección de fuente, preflight LAS/LAZ (`las_preflight`: `preflight_las`, que acota las reservas declaradas en la cabecera antes de que las crates `las`/`laz` las procesen, pues `las` 0.11.2 no lo corrige, y `check_laz_chunk_table`, que comprueba las entradas decodificadas de la tabla de chunks; lo usan la ruta nativa y el conversor web), planificador espacial, bloques, traits `SpoolStore`/`SourceFactory`/`Clock`, partición raíz por 1–8 rangos (`with_root_partition_ranges`), `LazChunkPointSource` (rangos de chunks LAZ sin `seek`), el almacén híbrido `HybridSpoolStore` (`pipeline/hybrid/`: tiers de RAM y de páginas con claves `u32`, volcado P3 con pistas `hint_retained`/`hint_consume_rank`, lector anclado, estimación de zonas) y la API serie `convert_serial`/`validate_output_serial`. Compila para wasm32. Sin `std::fs`, hilos, `Instant`/`SystemTime`, `process`, `rayon`, `laz-parallel` ni `ctrlc`: `clippy.toml` (`disallowed-methods`/`types`, denegado) lo impone y `check.sh` comprueba wasm32 y `cargo tree`. El mismo `clippy.toml` prohíbe las funciones float respaldadas por libm (`hypot`, `powf`, `exp`, `ln`, `sin`, `mul_add`…) porque su resultado difiere entre glibc y wasm32 ([ADR 0016](../adr/0016-norma-portatil-en-el-error-de-cuantizacion.md)); la guarda existe también en `viewer_format` y en el shell. | [Exports](../../crates/viewer_convert_core/src/lib.rs), [serie](../../crates/viewer_convert_core/src/serial.rs), [preflight LAS](../../crates/viewer_convert_core/src/las_preflight.rs), [chunks LAZ](../../crates/viewer_convert_core/src/pipeline/laz_chunks.rs), [almacén híbrido](../../crates/viewer_convert_core/src/pipeline/hybrid/store.rs), [`clippy.toml`](../../crates/viewer_convert_core/clippy.toml). |
| `viewer_convert` | Shell nativo sobre `viewer_convert_core`: CLI y biblioteca LAS/LAZ, hilos y rayon, ficheros de spool, particiones 2/4/8, workers acotados, publicación validada y diagnóstico de fases; sin EPT ni checkpoints desde la V8 (ADR 0061). Siempre escribe `v8`, que fija 64 000 puntos por bloque, 2 divisiones espaciales y orden híbrido: `--format-version`, `--row-order`, `--points-per-block`, `--spatial-divisions`, `--attr-codec`, `--checkpoint` y `--resume` se retiraron y se rechazan con un error explícito (no se ignoran). Pipeline de escritura optativo y apagado por defecto; la opción oculta `--page-target-bytes N` fuerza páginas pequeñas para pruebas y benchmarks. El LAS conserva flags (`las_flags`) y Extra Bytes de hasta 64 B por punto (RF-24). Subcomando `viewer_convert verify FILE.oct`: lee todas las páginas y ejecuta las comprobaciones globales (y el pie del `.octattrs` si existe). | [CLI](../../crates/viewer_convert/src/main.rs), [biblioteca](../../crates/viewer_convert/src/lib.rs), [verify](../../crates/viewer_convert/src/verify.rs). Módulos: `las_source`, `publish`, `fs_util`, `file_spool_store`, `executor`; `options`, `error` y `report` re-exportan los del núcleo. |
| `viewer_render_wgpu` | Cámara orbital/pájaro, vóxeles diádicos enteros, color por atributos, instancias, presupuestos, picking, frustum/HZB y diagnóstico headless; ruta de celdas por nodo en un pool paginado de GPU (`cells/pool.rs`, `cells/cut.rs`, `cells/render.rs`) y decisión de nivel por celda, el único modo (decidido el 18-11-2026, en `main` desde el 19-11-2026) (`cells/lod.rs`, [plan](../records/archive/fase-2a-lod-por-celda-en-gpu.md), [ADR 0056](../adr/0056-se-retira-el-modo-octante.md)). | [Grilla](../../crates/viewer_render_wgpu/src/voxel_grid.rs), [GPU](../../crates/viewer_render_wgpu/src/gpu.rs), [sincronización](../../crates/viewer_render_wgpu/src/block_sync.rs). |
| `viewer_wasm` | Bindings, Blob con acompañante, carga progresiva, caché, reutilización de escena, métricas y conversor web OPFS (`core_convert.rs`: `WasmCoreConverter` sobre `viewer_convert_core`; `spool_host.rs`: import estructural del host de spools con bytes por llamada; `opfs.rs`: `OpfsWriter`/`OpfsReader`, donde una escritura corta da `StorageFull`). La actualización progresiva carga como máximo un bloque por llamada. Lectura perezosa de V8: las vistas leen e injertan páginas (`update_lod_view_paged`, `lod_driver.rs`) con un presupuesto propio de páginas de 64 MiB (`DEFAULT_PAGE_BUDGET_BYTES`, `lod_pages.rs`), separado del de bloques; si se agota, la vista queda degradada sin bucle de reintentos (`lod.pageBudgetExhausted`). | [Bindings](../../crates/viewer_wasm/src/bindings/mod.rs) (también `points`, `lod`, `camera`, `render`, `js_stats`, `js_marshal`), [dataset LOD](../../crates/viewer_wasm/src/lod_dataset.rs), [I/O Blob](../../crates/viewer_wasm/src/browser_io.rs), [conversor](../../crates/viewer_wasm/src/core_convert.rs), [host de spools](../../crates/viewer_wasm/src/spool_host.rs), [OPFS](../../crates/viewer_wasm/src/opfs.rs). |
| `examples/web-host` | Demo, API embebible de viewer/converter, cola de comandos, workers, estadísticas, probes y paquete autocontenido para `file://`. | [Guía de uso/build/API](../../examples/web-host/README.md), [API viewer](../../examples/web-host/viewer-api.mjs), [API converter](../../examples/web-host/converter-api.mjs), [tiers de spools](../../examples/web-host/spool-tiers.mjs) (segmentos de JS por clases de tamaño, zonas OPFS, `removeWithRetry`), [worker](../../examples/web-host/converter-worker.js). Fuente remota: [`http-range-source.mjs`](../../examples/web-host/http-range-source.mjs) (`?url=FILE.oct` en el visor y `load({ url })` en la API, [D4](../records/archive/d4-fuente-url-en-la-api.md); imita un `Blob`, abre con una petición de sufijo y lee rangos simples con `If-Range`; un 200 ante `Range` es error), [`red-ref-server.mjs`](../../examples/web-host/probes/red-ref-server.mjs) (servidor de pruebas con Range, CORS y la red RED-REF simulada) y [`probe-remote-open.mjs`](../../examples/web-host/probes/probe-remote-open.mjs) (sonda de apertura remota; medidas en el [registro R5-P4](../records/benchmarks/R5_P4_HTTP_RANGE_02-11-2026.md)). El paquete local inserta la fuente de rangos en línea (`package-local.py`). `pkg/` y el HTML/ZIP exportado son artefactos que hay que regenerar tras cambios. |

Features Cargo (apagadas por defecto): `experimental` (`viewer_core`, `viewer_format`) y
`async-reader` (`viewer_format`). `scripts/check.sh` ejecuta tests y clippy con y sin ellas.

Herramientas de paridad wasm32 (miembros del workspace, sin uso en producción; se ejecutan bajo Node 22 desde `check.sh`):

| Herramienta | Qué hace | Límite |
| --- | --- | --- |
| [`tools/convert_parity`](../../tools/convert_parity/) | Convierte fixtures con `convert_serial` en wasm32 y con el CLI nativo (`native_convert`) y compara el SHA-256 de `.oct`/`.octattrs`. [`run-bindings.mjs`](../../tools/convert_parity/run-bindings.mjs) ejecuta los bindings reales de `viewer_wasm` en Node (shim de `FileReaderSync`, handles sobre `fs`, volcado forzado) y, con `--input`, es la puerta Node de M3b. [`native_hybrid`](../../tools/convert_parity/src/bin/native_hybrid.rs) es la referencia nativa de tiempo (`convert_serial` con `HybridSpoolStore` y páginas en fichero). | Rutas serie; no cubre el pool ni Majadas. Autzen solo si el registro de cargo lo tiene. `run-bindings.mjs` no certifica navegadores. |
| [`tools/laz_chunk_probe`](../../tools/laz_chunk_probe/) | Sonda S2: decodifica rangos de chunks LAZ sin `seek`; genera `fixtures/synthetic_variable_chunks.laz` (chunks de tamaño variable) y sirve de oráculo de tests del núcleo. | Sonda y oráculo, no parte del pipeline. |
| [`tools/format_parity`](../../tools/format_parity/) | Igualdad nativo/wasm32 de la codificación de bloques (S0). | Los bloques reales de Belvedere requieren el dataset local. |

## Límites

Qué no demuestra lo implementado; consultar antes de afirmar capacidades. Los requisitos y decisiones de producto (aún no implementados) están en [requirements.md](../requirements.md).

| Área | Límite |
| --- | --- |
| Core | El fallback de bloques residentes no demuestra continuidad geométrica del render. El índice residente y la memoria estimada no equivalen a RSS. |
| Formato | Límites de páginas de V8 (`with_page_limits`; [spec](format/FORMAT_V8.md#índice-paginado)): `max_page_bytes` 1 MiB y `max_resident_pages` 4 096; el presupuesto de páginas residentes del visor es aparte (64 MiB, véase WASM / host). En V8, `max_nodes` y `max_blocks` (2 000 000, [ADR 0007](../adr/0007-limites-por-defecto-del-lector.md)) acotan lo que el lector eager lee, no los totales declarados: el lector perezoso abre ficheros de más nodos y el eager los rechaza con los límites por defecto (visto con el sintético de 6 M nodos). La ocupación codificada es local al bloque, no una grilla física común. El lector predeterminado acota atributos (256), componentes (65 535), cadenas (1 MiB), índice y bloque (256 MiB), nodos y bloques (2 000 000) y rangos de atributo: un total de 2^23 sumados en todos los nodos, no por nodo (`with_max_attribute_ranges`; `FormatReaderLimits::DEFAULT_MAX_ATTRIBUTE_RANGES`) — constantes en `reader_limits.rs`. Un rango `u8` ocupa 6 B en disco y unos 76 B de heap: sin esa cota, un índice de 256 atributos por nodo amplifica ≈12,9× (medido a 4, 32 y 128 MiB con perfil `fast`, `tests/index_amplification.rs`; ~3,3 GB extrapolados a 256 MiB); esquemas mayores requieren `FormatReader::with_limits` y la política por defecto sigue por decidir. Los enteros de 64 bits conservan bits en disco; su proyección a color puede perder precisión. |
| Conversión | Sin EPT ni checkpoints (ADR 0061). Publicar `.oct`/`.octattrs` no es atómico (atributos primero). El `fsync` del directorio padre al publicar es solo unix; Windows no falla pero no está probado. COPC se lee como LAS/LAZ secuencial; no se conserva su jerarquía. |
| Render | Los representantes limitan sus regiones ocupadas; payloads externos con más de ocho regiones por nodo conservan el suelo global. Se dibujan cubos, sin extracción de caras expuestas/agrupadas. Los presupuestos propios no cubren toda la VRAM. |
| WASM / host | El worker lee bloques LOD de forma async con scheduler (`WasmAsyncFormatReader`); la lectura de la entrada de conversión sigue siendo síncrona (`FileReaderSync`). La lectura BFS paralela del benchmark nativo no está integrada. Transporte: Blob local, y HTTP Range sobre V8, el único formato. La evidencia remota es RED-REF simulada, con vista fija sin órbita, sin S3 real ni compresión, y solo une rangos contiguos ([registro](../records/benchmarks/R5_P4_HTTP_RANGE_02-11-2026.md)). En el navegador, un pánico interno del decodificador LAZ (datos aritméticos corruptos) no se puede capturar y termina el worker de conversión; el host lo informa y lo recrea (en nativo se contiene como error, [ADR 0012](../adr/0012-validacion-previa-de-cabeceras-las-laz.md)). |
| Evidencia | Readback/SwiftShader no es prueba visual. No sumar porcentajes de series CPU distintas. Un corpus nativo grande no demuestra escala web. |

## Contrato de almacenamiento y atributos

| Identidad | Estado / cambio acumulado | Referencia |
| --- | --- | --- |
| `v8` (`8/0`) | **Vigente y único** ([FORMAT_V8](format/FORMAT_V8.md); [ADR 0058](../adr/0058-formato-v8-experimental.md), [0060](../adr/0060-v8-solo-lz4.md), [0061](../adr/0061-ept-y-checkpoints-se-retiran-con-la-v8.md)). Pareja `.oct`/`.octattrs`, magics `OCTVIDX8`/`OCTVEND8`; 64 000 puntos por bloque, 2 divisiones y orden híbrido fijos; solo LZ4; LOD con solo geometría, color de pantalla, intensidad y clase; color de hojas derivado; cuantiles de distancia en la cabecera; máximo 256 atributos (RF-27). Es la salida del CLI, el wasm y la app web. | [V8](format/FORMAT_V8.md) |
| `v5`, `v6.N-pre`, `v7`, `v7.N-pre`, `v8.1-pre`, `v8.2-pre` | **Retiradas con la V8 (D4, 21-11-2026)**: no se leen ni se escriben. Abrirlas da `RetiredFormat` («reconvierte desde el LAS/LAZ»); las 13 familias y sus magics están en [FORMAT_V8](format/FORMAT_V8.md#identidad-formatos-retirados-y-errores). Sus especificaciones se conservan como históricas: [V5](format/FORMAT_V5.md), [V7](format/FORMAT_V7.md), [V7.1-pre](format/FORMAT_V7_PRE1.md), [V7.2-pre](format/FORMAT_V7_PRE2.md), [V7.3-pre](format/FORMAT_V7_PRE3.md), [V7.4-pre](format/FORMAT_V7_PRE4.md), [V7.5-pre](format/FORMAT_V7_PRE5.md), [V8.1-pre](format/FORMAT_V8_PRE1.md), [V8.2-pre](format/FORMAT_V8_PRE2.md), v6 en [archivo](../records/archive/FORMAT_V6_PRE1.md). | Históricas |
| v1–v4 / provisional `OCTVWFRM` | Reconvertir desde fuente; sin migración automática ni lector legacy. | [Preparación/compatibilidad](../records/benchmarks/STORAGE_V6_PREPARATION.md) |

### Entrada, checkpoints y formato

El formato retirado, la entrada EPT y `--checkpoint`/`--resume` se retiraron con la V8 ([ADR 0061](../adr/0061-ept-y-checkpoints-se-retiran-con-la-v8.md)):
el conversor acepta LAS/LAZ/COPC y rechaza EPT y esas opciones con un error que lo explica. Código: `las_source.rs` de
`viewer_convert`; la ayuda del CLI repite la regla.

Muestras de la demo (`examples/web-host/`): `sample-v8`, `sample-small` y `belvedere-crop-v8`, versionadas por nombre; `belvedere-crop-v8` no está en `examples/web-host/`: lo genera `scripts/make-demo-belvedere.sh` en `target/demo-data` y se publica en gh-pages. Un `.oct` y su `.octattrs` deben tener identidad exacta y asociación de dataset compatibles. Abrir solo
`.oct` permite gris. Los codecs conservan filas y bits cuantizados, no el LAS original entero.
Se preservan intensidad, clasificación, retornos, ángulo, user data, point source ID y, cuando
existen, GPS time, RGB16 y NIR. También flags LAS y Extra Bytes; para el CRS
las omisiones se notifican y el trabajo sigue pendiente.

`OccupiedOctree`, `OccupiedPointBlock` y `QuantizedOccupancy` tienen contratos separados del
renderer. Los [resúmenes por nodo y diccionario conjunto](../explanation/NODE_ATTRIBUTE_SUMMARIES.md) son
contenedores experimentales: no se serializan como una pareja válida del formato vigente ni
están conectados a filtros/LOD. Media de intensidad, RGB lineal, categorías y extremos temporales
tienen políticas explícitas; su mera existencia no acredita herramientas de edición/clasificación.

## Herramientas y evidencia disponible

| Área | Herramientas / documentos | Qué falta para cerrar la prioridad relacionada |
| --- | --- | --- |
| Conversión/memoria | `profile_conversion.py`, `real_dataset_bench`, `voxel_build_bench`; [memoria v5](../records/benchmarks/V5_MEMORY_BENCH.md), [vóxeles](../records/benchmarks/VOXEL_BUILD_BENCH.md). | Pico simultáneo durante carga/cámara, no solo modelos de capacidad o RSS nativo. |
| Formato e I/O | `storage_suite.py`, `compare_io_binaries.py`, `storage_bench`, `storage_payload_stats`; [almacenamiento](../records/benchmarks/STORAGE_BENCH.md), [I/O](../records/benchmarks/STORAGE_V6_IO.md). | Consultas selectivas paralelas, caché controlada y transporte real antes de concurrencia en el viewer. |
| Corpus | [Datasets](../../data/datasets/README.md), [muestras](../../data/samples/README.md), fixture Additive real y [SensOFOREST pre3/pre4](../records/benchmarks/SENSOFOREST_V6_BENCH.md). | El corpus nativo grande no demuestra escala web ni equivalencia entre formatos/escenarios no comparados. |
| Navegador | Probes (`examples/web-host/probes/`) `probe-cache-camera.mjs`, `probe-real-additive.mjs`, `probe-surface-memory.mjs`, muestreador de procesos y colector; [benchmark web](../records/benchmarks/WEB_PERFORMANCE_BENCH.md). | Firefox 158 y Chrome 156 con AMD físico presentan la muestra; siguen pendientes pérdida/recuperación y móvil físico ([evidencia](../records/archive/RENDERER_DEPENDENCIES.md)). |
| Experimentos | Benchmarks de ocupación, diccionarios, resúmenes, anchuras y planos de bytes; [CPU](../records/benchmarks/STORAGE_V6_CPU.md), [localidad](../records/benchmarks/STORAGE_V6_BYTE_PLANES.md). | No integrar un candidato por microbenchmark aislado: medir pareja completa, consultas y costes. |

Resultados que deben conservarse al decidir: delta-varint ahorra solo **0,0054 %** en Belvedere;
pre4 frente a pre3 ahorra **13,62 % / 19,15 %** en Majadas/Albuera, con una conversión por caso
y tres lecturas; el diccionario conjunto penaliza clasificación; tiling pierde en los bloques
habituales y se descartó. Sus informes contienen opciones, hashes, dispersión y contraejemplos.
Las cifras no se trasladan entre revisiones, perfiles (`perf`/`release`) ni equipos.

La [entrega regional Replacement](../explanation/REGIONAL_VOXEL_LOD.md) permite detalle independiente fuera de
regiones gruesas, contabiliza sus metadatos y retiene el corte entre generaciones; la presentación
de navegador sigue pendiente.

## Mejoras y añadidos candidatos

Estas propuestas no amplían los compromisos de la hoja de ruta. Se evaluarán cuando haya una
necesidad concreta y evidencia suficiente; se favorecen cambios locales y contratos simples.

| Candidato | Motivo / condición para abordarlo |
| --- | --- |
| Unificar consumidores de vóxeles | Revisar la ruta genérica por bloque frente a la ruta LOD agregada. Cerrar el mismo invariante entero/sin solapamiento y presupuestos antes de retirar APIs; ausencia de llamadas internas no prueba que no haya consumidores externos. |
| Caras expuestas o agrupadas | Posible reducción de trabajo GPU. Primero medir vecinos y límites entre LOD, coste CPU/buffers y presentación; backface culling/HZB no equivalen a extraer caras internas. |
| Selección de codecs más barata | Perfilar costes por payload y corpus; conservar bytes, precisión y fallbacks. Saltarse candidatos por muestreo exige medir pérdida de compresión y definir ese contrato. |
| RGB agrupado / bitpacking por rango | Comparar contra pre3/pre4 con metadatos e IDs incluidos; conservar lectura independiente de clasificación y otros atributos. No imponer diccionario universal. |
| Transporte remoto: prefetch y unión con huecos | HTTP Range existe para V8 (solo rangos contiguos). Contar solicitudes, latencia, bytes extra y datos en vuelo; definir cancelación y fallos. No introducir async o concurrencia solo por extrapolación desde NVMe. |
| Interoperabilidad y herramientas | Priorizar con requisitos de producto: CRS/flags/Extra Bytes, COPC espacial, filtros, medición, recorte y exportación. El soporte parcial actual no las convierte en features completas. |
| Checks de GPU/navegador | Sin CI (decisión del propietario, 15-11-2026): `scripts/check.sh` se ejecuta en local, sin GPU ni navegador; esos checks requieren su propio entorno. Extraer módulos solo cuando reduzcan acoplamiento real; evitar un segundo roadmap. |
| Propuestas de la revisión de técnicas (13-11-2026) | Conversor, carga web, render, formato y arquitectura, cada propuesta con la medida que la decide: [registro](../records/benchmarks/REVIEW_TECHNIQUES_13-11-2026.md). Estado de cada una en el [plan de mejoras](../plans/mejoras-pendientes-13-11-2026.md). |

## Validación y mantenimiento documental

Suites, alcance y lo que `scripts/check.sh` no cubre: [CONTRIBUTING](../../CONTRIBUTING.md#checks).
Las entregas archivadas conservan sus checks originales, sin atribuirlos a revisiones posteriores.
Al cerrar trabajo: actualizar la prioridad del README, conservar evidencia en el informe temático
y no reintroducir casillas históricas como pendientes activos.
