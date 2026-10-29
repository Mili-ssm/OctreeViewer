# Inventario del proyecto — 28-10-2026

Mapa de referencia del código, manifests, pruebas e informes de este checkout. Estado y
prioridades activas: [README](../../README.md); índice de documentos: [docs/README.md](../README.md).
«Implementado» indica una ruta de código existente; «medido» remite a una ejecución fechada;
«experimental» no implica integración en el visor. Este documento no ejecuta benchmarks ni
certifica el checkout; los checks se ejecutan con `scripts/check.sh` ([CONTRIBUTING](../../CONTRIBUTING.md)).

## Objetivo y flujo

LAS/LAZ → conversión nativa → pareja versionada `.oct`/`.octattrs` → lectura selectiva →
selección LOD/caché → vóxeles → WebGPU en un worker → host web.
EPT local `laszip` entra por un importador que conserva jerarquía Additive. COPC puede leerse
como LAS/LAZ secuencial y reparticionarse; no se conserva su jerarquía como acceso COPC nativo.
La conversión web LAS/LAZ es una ruta adicional en dos pasadas con salida OPFS.
Explorar fuentes mayores de 500 GB sigue siendo objetivo sin validación de extremo a extremo.

## Componentes y puntos de entrada

| Componente | Capacidades presentes | Archivos para empezar |
| --- | --- | --- |
| `viewer_core` | Geometría, marco local, atributos tipados, jerarquía Replacement/Additive, frustum, selección LOD, scheduler y caché; octree ocupado e índice celda→filas; contrato LAS compartido (`las_attribute_schema`, `visual_scale_for_bounds`, `preflight_las`, que acota las reservas declaradas en la cabecera LAS/LAZ antes de que las crates `las`/`laz` las procesen; `las` 0.11.2 no lo corrige; la comprobación de las entradas decodificadas de la tabla de chunks LAZ, `check_laz_chunk_table`, vive tras la feature `laz` y la usan la ruta nativa y el conversor web). Con la feature `experimental`: `node_attributes`, `categorical_attribute`, `occupied_point_block`. | [Exports](../../crates/viewer_core/src/lib.rs), [LAS](../../crates/viewer_core/src/las.rs), [LOD/transición](../../crates/viewer_core/src/lod_transition.rs), [prioridad](../../crates/viewer_core/src/spatial_lod_priority.rs). |
| `viewer_format` | Política de versiones (`FormatVersion::DEFAULT_OUTPUT`, `WRITABLE`, `supports_block_checkpoint()`), identidades exactas, cuantización u32, raw/LZ4, CRC, pareja separada, lecturas XYZ/RGB/columna/fila/rangos, lectores independientes con índice compartido y reader async (feature `async-reader`, solo no-wasm). Con `experimental`: `experimental_tuple_dictionary`. | [Versiones](../../crates/viewer_format/src/version.rs), [layouts](../../crates/viewer_format/src/layout.rs), [contratos](../../crates/viewer_format/src/contracts.rs), [lector](../../crates/viewer_format/src/storage_reader.rs), [límites del reader](../../crates/viewer_format/src/reader_limits.rs). Módulos de apoyo: `index_codec`, `checkpoint_record`, `prepare`, `slice_cursor`. |
| `viewer_convert` | CLI y biblioteca LAS/LAZ, particiones 2/4/8, workers acotados, spools, publicación validada, checkpoints/resume y diagnóstico de fases. EPT local con checkpoint por bloques en V7.1/V7.2/V7.3-pre. Pipeline de escritura optativo y apagado por defecto. | [CLI](../../crates/viewer_convert/src/main.rs), [biblioteca](../../crates/viewer_convert/src/lib.rs), [EPT](../../crates/viewer_convert/src/ept.rs). Módulos: `options`, `error`, `report`, `las_source`, `publish`, `fs_util`. |
| `viewer_render_wgpu` | Cámara orbital/pájaro, vóxeles diádicos enteros, color por atributos, instancias, presupuestos, picking, frustum/HZB y diagnóstico headless. | [Grilla](../../crates/viewer_render_wgpu/src/voxel_grid.rs), [GPU](../../crates/viewer_render_wgpu/src/gpu.rs), [sincronización](../../crates/viewer_render_wgpu/src/block_sync.rs). |
| `viewer_wasm` | Bindings, Blob con acompañante, carga progresiva, caché, reutilización de escena, métricas y conversor web OPFS. La actualización progresiva carga como máximo un bloque por llamada. | [Bindings](../../crates/viewer_wasm/src/bindings/mod.rs) (también `points`, `lod`, `camera`, `render`, `js_stats`, `js_marshal`), [dataset LOD](../../crates/viewer_wasm/src/lod_dataset.rs), [I/O Blob](../../crates/viewer_wasm/src/browser_io.rs). |
| `examples/web-host` | Demo, API embebible de viewer/converter, cola de comandos, workers, estadísticas, probes y paquete autocontenido para `file://`. | [Guía de uso/build/API](../../examples/web-host/README.md), [API viewer](../../examples/web-host/viewer-api.mjs), [API converter](../../examples/web-host/converter-api.mjs). `pkg/` y el HTML/ZIP exportado son artefactos que hay que regenerar tras cambios. |

Features Cargo (apagadas por defecto): `experimental` (`viewer_core`, `viewer_format`) y
`async-reader` (`viewer_format`). `scripts/check.sh` ejecuta tests y clippy con y sin ellas.

## Límites

Qué no demuestra lo implementado; consultar antes de afirmar capacidades.

| Área | Límite |
| --- | --- |
| Core | El fallback de bloques residentes no demuestra continuidad geométrica del render. El índice residente y la memoria estimada no equivalen a RSS. |
| Formato | La ocupación codificada es local al bloque, no una grilla física común. El lector predeterminado acota atributos (256), componentes (65 535), cadenas (1 MiB), índice y bloque (256 MiB), nodos y bloques (2 000 000) y rangos de atributo: un total de 2^23 sumados en todos los nodos, no por nodo (`with_max_attribute_ranges`; `FormatReaderLimits::DEFAULT_MAX_ATTRIBUTE_RANGES`) — constantes en `reader_limits.rs`. Un rango `u8` ocupa 6 B en disco y unos 76 B de heap: sin esa cota, un índice de 256 atributos por nodo amplifica ≈12,9× (medido a 4, 32 y 128 MiB con perfil `fast`, `tests/index_amplification.rs`; ~3,3 GB extrapolados a 256 MiB); esquemas mayores requieren `FormatReader::with_limits` y la política por defecto sigue por decidir. Los enteros de 64 bits conservan bits en disco; su proyección a color puede perder precisión. |
| Conversión | EPT remoto/binary/zstandard no soportados. La identidad de checkpoint EPT usa rutas/tamaño/`mtime`, no hash: puede omitir cambios con metadatos iguales. Publicar `.oct`/`.octattrs` no es atómico (atributos primero). El `fsync` del directorio padre al publicar es solo unix; Windows no falla pero no está probado. COPC se lee como LAS/LAZ secuencial; no se conserva su jerarquía. |
| Render | Los representantes limitan sus regiones ocupadas; payloads externos con más de ocho regiones por nodo conservan el suelo global. Se dibujan cubos, sin extracción de caras expuestas/agrupadas. Los presupuestos propios no cubren toda la VRAM. |
| WASM / host | Lectura síncrona en el worker; la lectura BFS paralela del benchmark nativo no está integrada. El transporte revisado es Blob local: las interfaces por rangos no acreditan HTTP Range/remoto. En el navegador, un pánico interno del decodificador LAZ (datos aritméticos corruptos) no se puede capturar y termina el worker de conversión; el host lo informa y lo recrea (en nativo se contiene como error, [ADR 0012](../adr/0012-validacion-previa-de-cabeceras-las-laz.md)). |
| Evidencia | Readback/SwiftShader no es prueba visual. No sumar porcentajes de series CPU distintas. Un corpus nativo grande no demuestra escala web. |

## Contrato de almacenamiento y atributos

| Identidad | Estado / cambio acumulado | Referencia |
| --- | --- | --- |
| `v5` (`5/0`) | Predeterminada y definitiva, magic `OCTVFRM1`; XYZ y jerarquía separados de RGB/columnas. | [Especificación](format/FORMAT_V5.md) |
| `v6.1-pre` (`6/1`) | Optativa; coordenadas delta-varint con fallback. | [Pre1](../records/archive/FORMAT_V6_PRE1.md) |
| `v6.2-pre` (`6/2`) | Optativa; ocupación por profundidad dentro del bloque, filas y residuales exactos, fallback. | [Pre2](../records/archive/FORMAT_V6_PRE2.md) |
| `v6.3-pre` (`6/3`) | Optativa; constantes/diccionarios exactos e independientes por columna. | [Pre3](../records/archive/FORMAT_V6_PRE3.md) |
| `v6.4-pre` (`6/4`) | Optativa; planos de bytes y máscaras compactas con hojas/prefijos implícitos. | [Pre4](../records/archive/FORMAT_V6_PRE4.md) |
| `v7.1-pre` (`7/1`) | Optativa; base V7 que hereda pre4. Magics de índice/footer `OCTVI71P`/`OCTVE71P`. Checkpoint EPT por bloques. | [V7.1-pre](format/FORMAT_V7_PRE1.md) |
| `v7.2-pre` (`7/2`) | Optativa; ocupación restringida a compact24. `OCTVI72P`/`OCTVE72P`. Checkpoint EPT por bloques. | [V7.2-pre](format/FORMAT_V7_PRE2.md) |
| `v7.3-pre` (`7/3`) | Optativa; además limita diccionarios de atributos a tuplas de 1/2/4 B. `OCTVI73P`/`OCTVE73P`. Checkpoint EPT por bloques. | [V7.3-pre](format/FORMAT_V7_PRE3.md) |
| `v7` final / `v6` final / otras revisiones | Identificables por cabecera, sin backend admitido. No se infiere compatibilidad por orden. | [Identidades](../../crates/viewer_format/src/version.rs) |
| v1–v4 / provisional `OCTVWFRM` | Reconvertir desde fuente; sin migración automática ni lector legacy. | [Preparación/compatibilidad](../records/benchmarks/STORAGE_V6_PREPARATION.md) |

Ambos archivos deben tener identidad exacta y asociación de dataset compatibles. Abrir solo
`.oct` permite gris. Los codecs conservan filas y bits cuantizados, no el LAS original entero.
Se preservan intensidad, clasificación, retornos, ángulo, user data, point source ID y, cuando
existen, GPS time, RGB16 y NIR. Flags, Extra Bytes y CRS tienen omisiones notificadas y trabajo
pendiente.

`OccupiedOctree`, `OccupiedPointBlock` y `QuantizedOccupancy` tienen contratos separados del
renderer. Los [resúmenes por nodo y diccionario conjunto](../explanation/NODE_ATTRIBUTE_SUMMARIES.md) son
contenedores experimentales: no se serializan como una pareja válida del formato vigente ni
están conectados a filtros/LOD. Media de intensidad, RGB lineal, categorías y extremos temporales
tienen políticas explícitas; su mera existencia no acredita herramientas de edición/clasificación.

## Herramientas y evidencia disponible

| Área | Herramientas / documentos | Qué falta para cerrar la prioridad relacionada |
| --- | --- | --- |
| Conversión/memoria | `profile_conversion.py`, `real_dataset_bench`, `voxel_build_bench`; [memoria v5](../records/benchmarks/V5_MEMORY_BENCH.md), [vóxeles](../records/benchmarks/VOXEL_BUILD_BENCH.md). | Pico simultáneo durante carga/cámara, no solo modelos de capacidad o RSS nativo. |
| Formato e I/O | `storage_suite.py`, `compare_storage_revisions.py`, `compare_io_binaries.py`, `storage_bench`, `storage_payload_stats`; [almacenamiento](../records/benchmarks/STORAGE_BENCH.md), [I/O](../records/benchmarks/STORAGE_V6_IO.md). | Consultas selectivas paralelas, caché controlada y transporte real antes de concurrencia en el viewer. |
| Corpus | [Datasets](../../data/datasets/README.md), [muestras](../../data/samples/README.md), fixture Additive real y [SensOFOREST pre3/pre4](../records/benchmarks/SENSOFOREST_V6_BENCH.md). | El corpus nativo grande no demuestra escala web ni equivalencia entre formatos/escenarios no comparados. |
| Navegador | Probes `probe-cache-camera.mjs`, `probe-real-additive.mjs`, `probe-surface-memory.mjs`, muestreador de procesos y colector; [benchmark web](../records/benchmarks/WEB_PERFORMANCE_BENCH.md). | Firefox 158 y Chrome 156 con AMD físico presentan la muestra; siguen pendientes pérdida/recuperación y móvil físico ([evidencia](../explanation/RENDERER_DEPENDENCIES.md)). |
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
| Índice exterior compacto / niveles paginados | Medir coste residente y consultas antes de cambiar layout. Un array global obliga a resolver paginación, raíces/grilla y compatibilidad; no es requisito para optimizar bloques actuales. |
| Transporte remoto / coalescing / prefetch | Contar solicitudes, latencia, bytes extra y datos en vuelo; definir cancelación y fallos. No introducir async o concurrencia solo por extrapolación desde NVMe. |
| Interoperabilidad y herramientas | Priorizar con requisitos de producto: CRS/flags/Extra Bytes, COPC espacial, EPT remoto, filtros, medición, recorte y exportación. El soporte parcial actual no las convierte en features completas. |
| CI de GPU/navegador | La CI (`.github/workflows/ci.yml`) ejecuta `scripts/check.sh`, sin GPU ni navegador; esos checks requieren su propio entorno. Extraer módulos solo cuando reduzcan acoplamiento real; evitar un segundo roadmap. |

## Validación y mantenimiento documental

Suites, alcance y lo que `scripts/check.sh` no cubre: [CONTRIBUTING](../../CONTRIBUTING.md#checks).
Las entregas archivadas conservan sus checks originales, sin atribuirlos a revisiones posteriores.
Al cerrar trabajo: actualizar la prioridad del README, conservar evidencia en el informe temático
y no reintroducir casillas históricas como pendientes activos.
