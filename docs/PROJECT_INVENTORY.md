# Inventario del proyecto — 14-10-2026

Revisión documental del código, manifests, pruebas e informes presentes en este checkout.
La única hoja de ruta activa es [Prioridades pendientes del README](../README.md#prioridades-pendientes).
«Implementado» indica una ruta de código existente; «medido» remite a una ejecución fechada;
«experimental» no implica integración en el visor. Esta revisión no ejecuta benchmarks ni
certifica el checkout mediante tests. La actualización posterior completa la [migración de dependencias del renderer](RENDERER_DEPENDENCIES.md),
con sus propios checks y limitaciones de navegador; no actualiza los benchmarks históricos.

## Objetivo y flujo

LAS/LAZ → conversión nativa → pareja versionada `.oct`/`.octattrs` → lectura selectiva →
selección LOD/caché → vóxeles → WebGPU en un worker → host web.
EPT local `laszip` entra por un importador que conserva jerarquía Additive. COPC puede leerse
como LAS/LAZ secuencial y reparticionarse; no se conserva su jerarquía como acceso COPC nativo.
La conversión web LAS/LAZ es una ruta adicional en dos pasadas con salida OPFS.
Explorar fuentes mayores de 500 GB sigue siendo objetivo sin validación de extremo a extremo.

## Componentes y puntos de entrada

| Componente | Capacidades presentes | Archivos para empezar / límites |
| --- | --- | --- |
| `viewer_core` | Geometría, marco local, atributos tipados, jerarquía Replacement/Additive, frustum, selección LOD, scheduler y caché; octree ocupado e índices celda→filas experimentales. | [Exports](../crates/viewer_core/src/lib.rs), [LOD/transición](../crates/viewer_core/src/lod_transition.rs), [prioridad](../crates/viewer_core/src/spatial_lod_priority.rs). El fallback de bloques residentes no demuestra continuidad geométrica del render. |
| `viewer_format` | Identidades exactas, cuantización u32, raw/LZ4, CRC, pareja separada, lecturas XYZ/RGB/columna/fila/rangos y lectores independientes con índice compartido. | [Contratos](../crates/viewer_format/src/contracts.rs), [lector](../crates/viewer_format/src/storage_reader.rs), [layouts](../crates/viewer_format/src/layout.rs). La ocupación codificada es local al bloque, no una grilla física común. |
| `viewer_convert` | CLI y biblioteca LAS/LAZ, particiones 2/4/8, workers acotados, spools, publicación validada, checkpoints/resume y diagnóstico de fases. Pipeline de escritura optativo y apagado por defecto. | [CLI](../crates/viewer_convert/src/main.rs), [biblioteca](../crates/viewer_convert/src/lib.rs), [EPT](../crates/viewer_convert/src/ept.rs). EPT remoto/binary/zstandard y checkpoint EPT no están soportados; publicación de la pareja no es atómica. |
| `viewer_render_wgpu` | Cámara orbital/pájaro, vóxeles diádicos enteros, color por atributos, instancias, presupuestos, picking, frustum/HZB y diagnóstico headless. | [Grilla](../crates/viewer_render_wgpu/src/voxel_grid.rs), [GPU](../crates/viewer_render_wgpu/src/gpu.rs), [sincronización](../crates/viewer_render_wgpu/src/block_sync.rs). Los representantes limitan sus regiones ocupadas; payloads externos con más de ocho regiones por nodo conservan el suelo global; se dibujan cubos, sin extracción de caras expuestas/agrupadas. |
| `viewer_wasm` | Bindings, Blob con acompañante, carga progresiva, caché, reutilización de escena, métricas y conversor web OPFS. La actualización progresiva carga como máximo un bloque por llamada. | [Bindings](../crates/viewer_wasm/src/bindings.rs), [dataset LOD](../crates/viewer_wasm/src/lod_dataset.rs), [I/O Blob](../crates/viewer_wasm/src/browser_io.rs). Lectura síncrona en worker; la lectura BFS paralela del benchmark nativo no está integrada aquí. |
| `examples/web-host` | Demo, API embebible de viewer/converter, cola de comandos, workers, estadísticas, probes y paquete autocontenido para `file://`. | [Guía de uso/build/API](../examples/web-host/README.md), [API viewer](../examples/web-host/viewer-api.mjs), [API converter](../examples/web-host/converter-api.mjs). `pkg/` y el HTML/ZIP exportado son artefactos que hay que regenerar tras cambios. |

El transporte de navegador revisado es Blob local; las interfaces selectivas y el uso de
rangos no acreditan por sí solos un transporte HTTP Range/remoto del producto.

## Contrato de almacenamiento y atributos

| Identidad | Estado / cambio acumulado | Referencia |
| --- | --- | --- |
| `v5` (`5/0`) | Predeterminada y definitiva, magic `OCTVFRM1`; XYZ y jerarquía separados de RGB/columnas. | [Especificación](../crates/viewer_format/FORMAT_V5.md) |
| `v6.1-pre` (`6/1`) | Optativa; coordenadas delta-varint con fallback. | [Pre1](../crates/viewer_format/FORMAT_V6_PRE1.md) |
| `v6.2-pre` (`6/2`) | Optativa; ocupación por profundidad dentro del bloque, filas y residuales exactos, fallback. | [Pre2](../crates/viewer_format/FORMAT_V6_PRE2.md) |
| `v6.3-pre` (`6/3`) | Optativa; constantes/diccionarios exactos e independientes por columna. | [Pre3](../crates/viewer_format/FORMAT_V6_PRE3.md) |
| `v6.4-pre` (`6/4`) | Optativa; planos de bytes y máscaras compactas con hojas/prefijos implícitos. | [Pre4](../crates/viewer_format/FORMAT_V6_PRE4.md) |
| `v6` final / otras revisiones | Identificables por cabecera, sin backend admitido. No se infiere compatibilidad por orden. | [Identidades](../crates/viewer_format/src/version.rs) |
| v1–v4 / provisional `OCTVWFRM` | Reconvertir desde fuente; sin migración automática ni lector legacy. | [Preparación/compatibilidad](STORAGE_V6_PREPARATION.md) |

Ambos archivos deben tener identidad exacta y asociación de dataset compatibles. Abrir solo
`.oct` permite gris. Los codecs conservan filas y bits cuantizados, no el LAS original entero.
Se preservan intensidad, clasificación, retornos, ángulo, user data, point source ID y, cuando
existen, GPS time, RGB16 y NIR. Flags, Extra Bytes y CRS tienen omisiones notificadas y trabajo
pendiente. La conservación de enteros de 64 bits en disco no garantiza precisión en color.

`OccupiedOctree`, `OccupiedPointBlock` y `QuantizedOccupancy` tienen contratos separados del
renderer. Los [resúmenes por nodo y diccionario conjunto](NODE_ATTRIBUTE_SUMMARIES.md) son
contenedores experimentales: no se serializan como una pareja válida del formato vigente ni
están conectados a filtros/LOD. Media de intensidad, RGB lineal, categorías y extremos temporales
tienen políticas explícitas; su mera existencia no acredita herramientas de edición/clasificación.

## Herramientas y evidencia disponible

| Área | Herramientas / documentos | Qué falta para cerrar la prioridad relacionada |
| --- | --- | --- |
| Conversión/memoria | `profile_conversion.py`, `real_dataset_bench`, `voxel_build_bench`; [memoria v5](V5_MEMORY_BENCH.md), [vóxeles](VOXEL_BUILD_BENCH.md). | Pico simultáneo durante carga/cámara, no solo modelos de capacidad o RSS nativo. |
| Formato e I/O | `storage_suite.py`, `compare_storage_revisions.py`, `compare_io_binaries.py`, `storage_bench`, `storage_payload_stats`; [almacenamiento](STORAGE_BENCH.md), [I/O](STORAGE_V6_IO.md). | Consultas selectivas paralelas, caché controlada y transporte real antes de concurrencia en el viewer. |
| Corpus | [Datasets](../data/datasets/README.md), [muestras](../data/samples/README.md), fixture Additive real y [SensOFOREST pre3/pre4](SENSOFOREST_V6_BENCH.md). | El corpus nativo grande no demuestra escala web ni equivalencia entre formatos/escenarios no comparados. |
| Navegador | Probes `probe-cache-camera.mjs`, `probe-real-additive.mjs`, `probe-surface-memory.mjs`, muestreador de procesos y colector; [benchmark web](WEB_PERFORMANCE_BENCH.md). | Presentación Chrome/AMD, interacción y recuperación en plataformas objetivo. |
| Experimentos | Benchmarks de ocupación, diccionarios, resúmenes, anchuras y planos de bytes; [CPU](STORAGE_V6_CPU.md), [localidad](STORAGE_V6_BYTE_PLANES.md). | No integrar un candidato por microbenchmark aislado: medir pareja completa, consultas y costes. |

Resultados que deben conservarse al decidir: delta-varint ahorra solo **0,0054 %** en Belvedere;
pre4 frente a pre3 ahorra **13,62 % / 19,15 %** en Majadas/Albuera, con una conversión por caso
y tres lecturas; el diccionario conjunto penaliza clasificación; tiling pierde en los bloques
habituales y se descartó. Sus informes contienen opciones, hashes, dispersión y contraejemplos.
No sumar porcentajes de series CPU distintas ni convertir readback/SwiftShader en prueba visual.

La [entrega regional Replacement](REGIONAL_VOXEL_LOD.md) permite detalle independiente fuera de
regiones gruesas y contabiliza sus metadatos. Esta instantánea del 14-10 no incluía retención del
corte entre generaciones; la actualización del 24-10 la implementa, aunque la presentación de
navegador sigue pendiente.

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
| Automatizar checks y documentación | No hay workflows `.github` en este checkout. Considerar CI nativa/WASM/host y comprobación de enlaces; GPU/navegador requieren su propio entorno. Evitar un segundo roadmap y extraer módulos solo cuando reduzcan acoplamiento real. |

## Validación y mantenimiento documental

Las suites disponibles incluyen tests Rust unitarios/integración/ejemplos, GPU ignorados por
requisito de adaptador, tests Node `*.test.mjs`, unittest Python del host y runners, y probes
`*.browser.mjs`. Los probes requieren bindings recientes y orquestación de navegador; no los
ejecuta `node --test .../*.test.mjs`. Los comandos habituales están en el
[README](../README.md#desarrollo); los específicos, en cada informe y en el host.

No hay cambios de interfaz ni de implementación en este inventario. Se verifican enlaces locales,
coherencia de identidades con el despacho del código y diff documental. Las entregas archivadas
conservan sus checks originales, sin atribuirlos a revisiones posteriores de dependencias. Los checks de la migración se registran aparte.
Al cerrar trabajo: actualizar la prioridad del README, conservar evidencia en el informe temático
y evitar reintroducir casillas históricas como pendientes activos.
