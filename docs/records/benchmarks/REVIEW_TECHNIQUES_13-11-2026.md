# Revisión de técnicas: estructuras de datos, algoritmos, carga y render

Fecha: 13-11-2026. Rama: `feat/render-r3-node-cells`, revisión `c473c85`. Estado: **propuestas sin implementar ni medir; pendiente del propietario**.

Contrasta el código con técnicas publicadas y lista puntos de mejora. No se ejecutó código ni
benchmarks: cada propuesta lleva la medida que la confirmaría o la descartaría. Complementa, sin
repetir, la [evaluación de render del 30-10-2026](../archive/RENDER_ASSESSMENT_2026-10-30.md)
(hallazgos B1–B7) y la [evaluación de código del 28-10-2026](../archive/CODE_ASSESSMENT_2026-10-28.md).

Los identificadores de propuesta llevan el prefijo `T` (`TC` conversor, `TW` carga web, `TR` render,
`TF` formato, `TA` arquitectura) para no chocar con las fases R1–R5 ni con las resoluciones `P` y
`W` de los requisitos.

Marcas de evidencia:

| Marca | Significado |
| --- | --- |
| **[V]** | Comprobado en el código de esta revisión al redactar |
| **[L]** | Leído por un agente lector; línea citada sin recomprobar |
| **[F]** | Fuente externa abierta y leída (URL exacta en [fuentes](#fuentes-externas)) |
| **[N]** | Fuente no abierta (solo resumen, fragmento de búsqueda o conocimiento general): tratar como pista |
| **[I]** | Inferencia de esta revisión |

## Resumen

| Orden (valor esperado frente a coste, a juicio de esta revisión) | Propuesta | Área | Por qué primero |
| --- | --- | --- | --- |
| 1 | [TW1](#carga-web-y-api): corregir `outcome` fuera de ámbito en `viewer-worker.js` | Web | Defecto probable: un fallo de lectura deja rechazada la cola de mensajes del worker |
| 2 | [TC3](#conversor): sacar `i128` del bucle por punto (codificación y decodificación) | Conversor, lector | Ataca RQ-07 (prioridad 2 del README) y también la decodificación en wasm32 |
| 3 | [TC2](#conversor): acumulador de muestreo con datos calientes separados o agregación por lotes ordenados | Conversor | Misma prioridad; el diseño actual encadena 2–3 accesos dependientes por punto |
| 4 | [TC7](#conversor): cola de trabajo sin barrera por lote y `par_iter` efectivo | Conversor | Con 1/2/4 workers el plan V7 registra 797/663/811 s en Albuera; el paralelismo no escala |
| 5 | [TW3](#carga-web-y-api)–[TW5](#carga-web-y-api): cancelación real, unión con huecos y reintentos en HTTP Range | Web | Robustez de la fuente remota (ADR 0023) antes de medir contra un servidor real |
| 6 | [TW6](#carga-web-y-api): recuperar la pérdida de dispositivo WebGPU | Web | Hoy es un error fatal; la escena sigue en el worker de construcción |
| 7 | [TF1](#formato): decidir el orden de filas en bloque antes de congelar V7 | Formato | Puerta de una sola vez (ADR 0022) |
| 8 | [TR2](#render-y-lod): jerarquía en arena y derivados incrementales | Núcleo | La selección a 10⁶ nodos sigue en 0,4–0,6 s (R1-T3a) |
| 9 | [TR1](#render-y-lod): oclusión HZB durante el movimiento | Render | Hoy solo actúa con la cámara quieta |

## Límites de esta revisión

- Ninguna cifra de coste es una medida: los «por qué puede costar» son lecturas de código.
- No se abrieron (PDF por encima del límite de descarga o acceso denegado): el artículo de
  PotreeConverter 2 (solo resumen), la tesis de Potree, la charla de Nanite, la especificación OGC
  de 3D Tiles en PDF, Karras 2012, Müller 2015, Balkesen 2013, Laine y Karras 2010, Museth 2013 y
  LASzip 2013. Lo que dependa de ellos va marcado **[N]**.
- No se encontró ninguna medida publicada de estas técnicas en WebGPU, ni de decodificación de
  bloques de puntos en WebAssembly.
- Sin revisar: `ept.rs` en detalle, `pipeline/hybrid/*`, `block_sync.rs`, los probes de navegador.

## Qué no se propone

Ya decidido o medido; aparece aquí para que no se reabra por esta revisión.

| Idea | Dónde se decidió |
| --- | --- |
| *Splats* de pantalla o rasterizado por cómputo a 1 px | [ADR 0019](../../adr/0019-cubos-por-nodo-residentes-en-gpu.md): el propietario exige cubos |
| Cambiar de códec en V7 (zstd, Brotli, codificador aritmético) | [ADR 0025](../../adr/0025-codificador-lz4-propio.md) descarta «cambiar de códec en V7» |
| Páginas de jerarquía tipo COPC, Potree 2 o EPT; compresión de cadenas | [ADR 0030](../../adr/0030-jerarquia-paginada-por-anclas.md) |
| Hilos wasm obligatorios o `wasm-bindgen-rayon` | [ADR 0015](../../adr/0015-pool-de-workers-sin-memoria-compartida.md), [ADR 0024](../../adr/0024-hilos-wasm-opcionales-con-coop-coep.md) |
| Delta-varint de coordenadas, tiling, diccionario conjunto, SIMD portátil en CRC32 y planos de bytes | Medidos y descartados: [inventario](../../reference/inventory.md#herramientas-y-evidencia-disponible), [nightly](nightly-features-2026-10-28.md) |
| Sustituir la voxelización en CPU por vista (`BTreeMap` por punto en `voxel_grid.rs:284` **[V]**) | Lo sustituye R3 ([ADR 0031](../../adr/0031-celdas-por-nodo-en-v7-5.md)); la instancia de 16 B está decidida en ese mismo ADR |

## Conversor

Contexto medido por el proyecto (cifras válidas solo para la revisión, el perfil, el dataset y el
equipo de su registro): Belvedere v7.5/V5 ≈ 1,9–2,0 frente a RQ-07 ≤ 1,3
([registro](EXPLORE_CONVERT_CPU_02-11-2026.md)); mediana de preparación de bloques 186,89 s en V7
frente a 58,23 s en V5 en la comparación Belvedere/Majadas del
[plan V7](../../plans/format-v7-plan.md), que atribuye a esa fase la mayor parte de la diferencia.

| Id | Hoy | Propuesta | Respaldo | Medida que decide |
| --- | --- | --- | --- | --- |
| TC1 | `GlobalCellFrame::cell` recuantiza una posición `f64` ya ajustada a la rejilla y calcula el centro de celda en flotante solo para la distancia (`viewer_convert_core/src/spatial.rs:2376-2447` **[L]**). Listado «sin probar» el 02-11-2026 | Celda y distancia desde los enteros del registro (distancia en cuantos²) | **[I]** | Mediana de 3 en Belvedere, SHA-256 idéntico |
| TC2 | `SparseCells`: tabla abierta de palabras `clave<<32 \| índice`, más `entries: Vec<(u32, SampleAccumulator)>` (~80 B por entrada: acumulador de 72 B y clave) y el arena `NearestRecords` (`pipeline/node_job.rs:160-208` **[V]**). Por punto: sondeo de tabla → `entries[índice]` → escritura en el arena, en orden de spool, no de celda **[V]** | (a) Separar lo caliente: distancia mínima y ranura en un vector paralelo a la tabla; sumas de color aparte. (b) Agregar por lotes: ordenar por radix `(celda, registro)` dentro de cada lote y reducir por tramos | Particionar antes de agregar evita fallos de caché y TLB: Müller 2015, Balkesen 2013 **[N]**. `voracious_radix_sort`: 1 M `u32` en 6,6 ms frente a 24,9 ms de `sort_unstable` (Ryzen 9 3950X) **[F]**. El proyecto ya midió −7,02 % de preparación con radix de pares espaciales | `perf stat -e cache-misses,dTLB-load-misses` por variante; mediana de 3; SHA idéntico |
| TC3 | `quantize_global_positions` hace por punto `contains` en `i128`, `block_cell_origin`, resta en `i128` y `local_position_units` (`viewer_format/src/quantization.rs:243-256` **[V]**). El lector repite aritmética `i128` y comprobación de celda por punto (`reader.rs:918-932` **[L]**) | Mínimo y máximo del bloque en `i64` en una pasada; `contains` y «una sola celda» se comprueban sobre esos dos extremos (la celda es una caja alineada a ejes **[I]**); `i128` solo si el lado del nodo supera 2⁶² | La propuesta *wide-arithmetic* de WebAssembly declara el código de 128 bits 2–7× más lento que nativo **[F]**; afecta a la decodificación en el navegador | Tiempo de fase de bloques nativo; `decodeMs` por bloque en wasm (ya se recoge en `asyncBlockTotals`) |
| TC4 | `PlannedBlock::read` decodifica cada registro a un `SpoolPoint` completo (~180 B) y lo reparte en ~10 vectores; la posición se cuantiza por tercera vez (`spatial.rs:189-300` **[L]**) | Copiar de los bytes del registro a las columnas, sin `SpoolPoint` intermedio | **[I]** | Tiempo de `prepare` por bloque (ya instrumentado) |
| TC5 | Cada nodo no hoja relee su spool y reescribe cada registro en un spool hijo: un punto se reescribe una vez por nivel (`pipeline/node_job.rs:341` **[L]**) | Distribución en dos pasadas (contar por celda de profundidad *k*, luego repartir) y construcción por *chunk* en memoria | PotreeConverter 2: ordenación jerárquica por conteo en *chunks* paralelos; ~6 M puntos/s (ruido azul) y ~9 M (aleatorio) con E/S incluida, solo según el resumen **[N]** | Bytes escritos en spools ÷ bytes de registros de entrada (el `ledger` ya los cuenta); puntos/s de Belvedere para comparar (OQ-02) |
| TC6 | `FileSpoolStore::append` abre el fichero en cada volcado de 32 KiB y toma un `Mutex` global (`viewer_convert/src/file_spool_store.rs:105-138` **[V]**) | Caché LRU de descriptores abiertos con tope fijo | **[I]**; ADR 0017 midió el coste de abrir por spool en OPFS, no en nativo | `strace -c` (cuenta de `openat`) y tiempo de pared |
| TC7 | Los hijos de la raíz se planifican en lotes con barrera (`viewer_convert_core/src/spatial.rs:1503-1523`; `executor.rs:254` ejecuta cada lote **[L]**). `prepare` usa `par_iter` (`las_source.rs:601,613`) pero `process_batches` le pasa rebanadas de un elemento (`las_source.rs:724,789`) **[V]**, así que no paraleliza | Cola única con robo de trabajo entre niveles; quitar el `par_iter` inerte o pasarle el lote entero | Albuera 1/2/4 workers: 796,84 / 662,53 / 811,36 s ([plan V7](../../plans/format-v7-plan.md)) | Curva de escalado 1–8 workers con RSS |
| TC8 | `fingerprint_file` recorre toda la entrada byte a byte con multiplicación `u128` (`checkpoint.rs:282-304` **[V]**), y se repite en `verify_input` (`checkpoint.rs:199`, llamado desde `las_source.rs:211` **[L]**) | Huella por bloques con una función vectorizada, o tamaño + fecha + muestras como el punto de control EPT | **[I]**: a 400–500 GB es una pasada serie completa adicional | Tiempo de la huella sobre el dataset ≥ 50 GB de RQ-11 |
| TC9 | `measure_leaf_spacing` es una pasada aparte por hoja con `Vec<[i128;3]>` (48 B/punto) y ordenación (`pipeline/node_job.rs:967-1052` **[L]**) | Plegarla en la pasada que crea la hoja, con celdas `u64` | **[I]** | Tiempo de la fase en el diagnóstico |

## Carga web y API

| Id | Hoy | Propuesta | Respaldo | Medida que decide |
| --- | --- | --- | --- | --- |
| TW1 | En el `catch` de la lectura asíncrona, `outcome` se declara con `const` dentro de `if (!ticketCompleted)` y se lee fuera (`examples/web-host/viewer-worker.js:519-524` **[V]**). Con la generación vigente lanza `ReferenceError` y la promesa encadenada queda rechazada (patrón reproducido aislado en Node; la ruta real no se ejecutó), así que el error original no se emite y `messageQueue` queda rechazada **[I]**. `build-worker.js` lo declara fuera | Declarar `let outcome` antes del `if` | Semántica de ámbito de bloque de `const` | Test Node que fuerce un fallo de lectura en la ruta sin worker de construcción |
| TW2 | `readBudget ??=` se crea una vez por worker con el `cacheBytes` del primer dataset (`build-worker.js:307` **[V]**); un `load` posterior con otro valor conservaría el presupuesto anterior **[I]** | Recrear el presupuesto al cambiar `cacheBytes` | **[I]** | Test con dos `load` de distinto `cacheBytes` |
| TW3 | La cancelación es lógica: el token se consulta entre lecturas, pero ningún llamador pasa `signal` a `resolveSource` (`viewer-worker.js:783`, `build-worker.js:310` **[V]**), así que el cuerpo HTTP de un bloque obsoleto se descarga entero | Un `AbortController` por ticket, abortado al cancelar | `abort()` cancela la petición y el consumo del cuerpo (MDN) **[F]**; CesiumJS descarta peticiones al mover la cámara (`cullRequestsWhileMoving`) **[F]** | Bytes descargados y no usados durante una órbita en RED-REF |
| TW4 | `maxGap` vale 0 y nadie lo cambia (`http-range-source.mjs:39,91` **[V]**): solo se unen rangos contiguos. Ya listado como candidato en el inventario | Umbral de hueco derivado de la red: unir cuando descargar el hueco cuesta menos que un viaje de ida y vuelta **[I]** | COG coloca cabecera e IFD juntos para una sola petición y cifra 3 peticiones en serie en ~540 ms **[F]**; PMTiles exige el directorio raíz en los primeros 16 KiB **[F]** | Peticiones, bytes extra y tiempo a 4 px en RED-REF para 0, 16 KiB, 64 KiB y 256 KiB |
| TW5 | Sin reintentos ni tiempo límite por petición (`http-range-source.mjs` **[V]**); un fallo marca el bloque hasta `retryFailed` | Reintento con espera creciente para errores de red, 429 y 5xx; tiempo límite con `AbortSignal` | **[I]**; el registro R5_P4 reconoce que no se midió contra un S3 real | Inyección de fallos en el servidor del arnés |
| TW6 | No hay manejo de pérdida de dispositivo: ninguna referencia a `device.lost` en renderer, wasm ni host **[V]**, salvo en dos probes de navegador que solo notifican el evento **[L]**. `CurrentSurfaceTexture::Lost` devuelve error (`gpu.rs:~1200` **[L]**) y la API lo trata como fatal (`viewer-api.mjs:111-117` **[L]**) | Escuchar `device.lost`, pedir adaptador y dispositivo de nuevo, recrear recursos y volver a subir la escena, que sigue en el worker de construcción | MDN: reintentar salvo `destroy()` y recrear todos los recursos **[F]** | Prueba manual con `device.destroy()` simulado; RF de recuperación a definir |
| TW7 | Tres copias por bloque entre JS y wasm: `ArrayBuffer` → `to_vec` → decodificación → `Float32Array` → de vuelta a wasm (`async_format_reader.rs:665-680,846-851`, `bindings/lod.rs:343` **[L]**); de ahí el factor 3× con que el lector estima el pico de memoria de cada lectura (`async_format_reader.rs:592-616` **[L]**) | Decodificar en el buffer de destino o pasar una vista de la memoria wasm | La transferencia de `ArrayBuffer` no depende del tamaño; la copia sí (Surma, 2019) **[F]** | PSS y `decodeMs`/`insertMs` por bloque |
| TW8 | `load` solo acepta `Blob` (`viewer-api.mjs:283` **[V]**); las fuentes `{url}` existen en la demo y en el protocolo del worker | Exponer `{url}` en la API pública | [ADR 0023](../../adr/0023-fuentes-locales-y-remotas.md) decide la fuente remota y RF-11 exige que cada función del visor esté en la API pública: es una carencia, no una decisión abierta | Test de la API con una fuente remota del arnés |
| TW9 | El dispositivo se pide con `Limits::downlevel_defaults().using_resolution(...)` (`gpu.rs:326` **[V]**) y la escena es un único buffer de instancias. Con 40 B por instancia (`scene.rs:10-16` **[V]**), 8 M de vóxeles son 320 MB, por encima de los 256 MiB por defecto de `maxBufferSize` **[F]** **[I]** | Pedir los límites del adaptador que hagan falta, o trocear el buffer; el pool paginado de R3 lo evitaría | MDN: `maxBufferSize` 256 MiB y `maxStorageBufferBindingSize` 128 MiB por defecto, ampliables con `requiredLimits` **[F]** | Comprobar el límite efectivo en Chrome con una escena de 8 M |
| TW10 | Concurrencia fija de 4 lecturas de bloque y 4 de página (`build-worker.js:19,149,197` **[V]**) para local y remoto | Hacerla configurable por tipo de fuente | Potree usa 4 «por experiencia» **[F]**; CesiumJS 18 por servidor y permite subirlo para HTTP/2 y HTTP/3 **[F]**; sin medidas publicadas | Tiempo a 4 px con 4, 8 y 16 en RED-REF |

## Render y LOD

La voxelización por vista y el formato de instancia quedan fuera: los cambia R3.

| Id | Hoy | Propuesta | Respaldo | Medida que decide |
| --- | --- | --- | --- | --- |
| TR1 | El dibujo con oclusión por *buffer* Z jerárquico (HZB) solo se usa si la cámara coincide con la de referencia (`gpu.rs:1173` **[V]**); la referencia se anula en 11 puntos (`gpu.rs:497-1118` **[V]**). En movimiento se dibujan todas las instancias | Oclusión en dos fases: probar contra el HZB del frame anterior, dibujar lo visible, reconstruir y dibujar lo recién descubierto | Técnica de *pipelines* dirigidos por GPU (Haar y Aaltonen, 2015) **[N]** | Instancias dibujadas y p95 de frame durante la órbita de RQ-01 |
| TR2 | `SpatialHierarchy.nodes` es `HashMap<NodeId, IndexedNode>` con el hasher por defecto (`viewer_core/src/spatial.rs:551` **[V]**). `refresh_derived` recorre todos los nodos y ordena los bloques (`spatial.rs:891-907` **[V]**) tras cada página injertada o expulsada **[L]** | Arena por página (`NodeId` ya es `página<<32 \| índice`) con acceso por índice; `block_ids` y `max_level` incrementales | Octrees lineales sin punteros (Gargantini 1982; Warren y Salmon 1993) **[N]**. Medida propia: selección a 10⁶ nodos en 0,4–0,6 s tras R1-T3a | `lod_frontier_bench` a 10⁶ nodos; coste de injertar 100 páginas |
| TR3 | La selección parte de la raíz en cada cambio de cámara y no aplica banda de tolerancia entre refinar y simplificar **[L]**; `BlockRequestScheduler::update_view` usa el `select_lod` lento (`scheduler.rs:125` **[L]**). Relacionado con B3 y B5 | Corte incremental desde el corte anterior, con esa banda de tolerancia | CLOD (Schütz 2019) cambia la densidad de forma gradual **[F, resumen]**; la evaluación del 30-10 cita 10–20 % de histéresis | Nodos visitados por actualización; cambios de etapa en órbita (RQ-02) |
| TR4 | Profundidad `Depth32Float` con `Less`, sin Z invertida (`gpu.rs:1693-1696` **[V]**); razón lejos/cerca limitada a 10⁵ (`camera.rs:12` **[L]**) | Z invertida (borrar a 0, comparar `Greater`); el HZB pasa de máximo a mínimo | Práctica habitual con profundidad en coma flotante **[N]** | *Z-fighting* en la vista lejana de Majadas; subir el tope de la razón |
| TR5 | Sin posproceso; el relieve viene de un tinte por cara y profundidad (`scene.rs:451-458` **[L]**) | *Eye-Dome Lighting* como pase opcional; compatible con cubos | Solo necesita color y profundidad (Boucheny y Ribes 2011, vía fuente secundaria) **[N]** | Coste del pase a resolución de EQ-REF; valoración visual del propietario |
| TR6 | `DecodedBlockCache`: `HashMap` más `BTreeSet<(u64, BlockId)>` de recencia; la expulsión salta los protegidos (`block_cache.rs:71-78` **[V]**, `:187` **[L]**) | Lista de recencia intrusiva o CLOCK si la expulsión aparece en perfiles | **[I]**, prioridad baja | Tiempo de expulsión con muchos protegidos |

## Formato

V7 final «se congela una vez», con una sola regeneración de datasets
([ADR 0022](../../adr/0022-alcance-y-congelacion-de-v7-final.md)); de ahí que un cambio de bytes como
TF1 o TF2 solo quepa antes de esa congelación **[I]**. Ninguno está en el alcance acordado de V7 final:
incluirlos es decisión del propietario.

| Id | Hoy | Propuesta | Respaldo | Medida que decide |
| --- | --- | --- | --- | --- |
| TF1 | El códec de ocupación guarda, por punto, el índice de fila original (varint de diferencias dentro de cada hoja) para conservar el orden de la fuente, además de residuales de `residual_bits` por eje, 24 en los candidatos compactos de `prepare.rs:211` (`occupancy_codec.rs:45-137` **[L]**) | Permitir orden Morton dentro del bloque, con las columnas de atributos en el mismo orden; desaparecen los índices | Potree 2 y G-PCC ordenan por Morton **[N]**; sin cifra publicada de ganancia | `storage_suite` con y sin índices en Belvedere y Majadas; exige cambiar el contrato de orden (OQ-09) |
| TF2 | `BytePlanes` transpone por posición de byte y aplica LZ4, sin predictor entre filas (`attribute_codec.rs:207` **[L]**) | Delta o XOR previo en columnas monótonas (tiempo GPS). No reabre lo descartado: lo medido el 28-10-2026 fue una implementación SIMD de la misma transposición, no un predictor | Blosc, datos sintéticos en un portátil i5-3380M (no son atributos LAS): *shuffle* + LZ4 ratio 3,78 y *bitshuffle* 28,88, con descompresión 8112 frente a 2427 MB/s **[F]**. Precaución: en este proyecto el delta-varint de coordenadas ahorró 0,0054 % en Belvedere | `node_attribute_suite` por columna |
| TF3 | `PagePointer` no lleva el CRC de la página hija (`page_codec.rs:47` **[L]**) | Añadir 4 B al puntero | **[I]**, prioridad baja | — |
| TF4 | `compress_block` sigue llamando a `lz4_flex::block::compress` (`codec.rs:54` **[V]**) aunque el ADR 0025 está aceptado | Ver [incoherencias](#incoherencias-documentales) | — | — |

Contexto, sin propuesta: lzbench (Silesia, i7-9700K) da a LZ4 ratio 2,11 con 4950 MB/s de
descompresión y a zstd -1 ratio 2,90 con 1550 MB/s **[F]**; LAZ declara ficheros del 7–20 % del
LAS **[F, fabricante]**.

## Arquitectura y patrones

El grafo de crates es acíclico y respeta las capas; el renderer no depende del formato y
`viewer_convert` reexporta el núcleo con módulos de 3–4 líneas, sin copiar código **[L]**.

| Id | Hoy | Propuesta | Condición |
| --- | --- | --- | --- |
| TA1 | `viewer_convert_core/src/spatial.rs` (2995 líneas) mezcla registro, plan, constructor (24 campos, `spatial.rs:863-893` **[V]**), muestreo, ingesta, rejilla y memoria **[L]** | Partir en `spatial/{record,plan,builder,sampling,ingest,grid,memory}.rs`; primero solo mover | Un commit sin cambio de comportamiento, con SHA de salida idéntico |
| TA2 | La conversión son funciones únicas largas con ramas `Option`: `ept.rs:118` (~734 líneas) y `las_source.rs:127` (~397) **[L]** | Fases explícitas (preparar, planificar, emitir, finalizar, publicar) y un enum para el destino con o sin punto de control | Tras decidir si los puntos de control EPT sobreviven a V7 final |
| TA3 | `SurfaceRenderer` reúne dispositivo, cámara, HZB (estado repartido en `Option` y `bool`), *picking* y residencia (`gpu.rs:256-292` **[L]**) | Estado HZB como enum dentro de `hzb.rs`; extraer *picking* y residencia; sombreadores a ficheros `.wgsl` con `include_str!` | Antes de que R3 añada el pool |
| TA4 | La política de LOD y de prioridad de carga, neutra de plataforma, vive en `viewer_wasm/src/lod_dataset.rs` **[L]** | Mover las funciones puras a `viewer_core` | Solo si se prevé un visor nativo o sin cabeza (OQ-10) |
| TA5 | Ningún `#[non_exhaustive]` en el workspace **[V]**; `FormatError` tiene 72 variantes **[V]**; al menos 23 `Result<_, String>` en código de producción **[V]** | Marcar enums de error y estructuras de opciones públicas; agrupar `FormatError` por clase | Solo si se publican los crates: el [ADR 0026](../../adr/0026-api-publica-licencia-y-version-1.md) los declara internos y fija el paquete npm como única superficie. Hasta entonces el beneficio es interno (añadir variantes sin tocar otros crates) |
| TA6 | 12 ramas sobre `uses_pages()` fuera de tests **[V]**, 7 fuera de `viewer_format` **[L]** | Operaciones en `viewer_format` que oculten la rama (`open_auto`, `verify_any`) | Al fijar V7 final |
| TA7 | Ciclos de módulos: `index_codec` ↔ `reader`, `ept` ↔ `las_source`, `error` ↔ `options` (este último en `viewer_convert_core`) **[L]** | Extraer el tipo compartido de cada par | Al tocar esos módulos |
| TA8 | Un solo `unsafe` en producción (`allocator.rs:22`) **[L]**; ningún crate declara `#![forbid(unsafe_code)]` **[V]** | Declararlo en los cinco crates que no lo necesitan | Coste nulo |

## Incoherencias documentales

Los registros no se editan; estas requieren acción en documentos vivos o un ADR nuevo.

| Documento | Incoherencia | Evidencia |
| --- | --- | --- |
| [ADR 0008](../../adr/0008-perfiles-release-y-perf.md) | Dice que `release` optimiza tamaño; desde el 02-11-2026 usa `opt-level = 3` | `Cargo.toml` **[V]**, CONTRIBUTING |
| [ADR 0025](../../adr/0025-codificador-lz4-propio.md) | Aceptado, pero el codificador propio no existe y `vendor/lz4_flex` sigue parcheado | `codec.rs:54`, `Cargo.toml` **[V]** |
| [ADR 0027](../../adr/0027-celdas-por-nodo-en-pool-paginado.md) | Sigue como «Propuesta» y el 0031 lo sustituye en parte | Índice de ADR **[V]** |
| [ADR 0030](../../adr/0030-jerarquia-paginada-por-anclas.md) | «≤ 512 KB» para elegir *k* (línea 14) y «1 MiB» para una página con *k* = 1 en la condición de revisión (línea 52): comprobar que ambos umbrales son coherentes | **[L]** |
| [Requisitos](../../requirements.md#preguntas-abiertas) | OQ-06 figura abierta y R5 ya adoptó ese direccionamiento | `page_codec.rs:15-34` **[L]** |

## Preguntas para el propietario

TF1 y TA4 dependen de decisiones del propietario: están en
[requisitos, preguntas abiertas](../../requirements.md#preguntas-abiertas) como OQ-09 y OQ-10.

## Fuentes externas

| Tema | Dato | Estado | URL |
| --- | --- | --- | --- |
| PotreeConverter 2 | Ordenación jerárquica por conteo, *chunks* paralelos, submuestreo ascendente; ~6 M y ~9 M puntos/s | Resumen leído; cuerpo no | https://repositum.tuwien.at/handle/20.500.12708/58254 |
| PotreeConverter 2 | 10–50× más rápido que 1.7; salida en 3 ficheros | README leído | https://github.com/potree/PotreeConverter/ |
| SimLOD | En memoria y en GPU; hasta 580 M puntos/s (RTX 4090); la decodificación LAZ limita la carga a ~22 M puntos/s | Leído | https://ar5iv.labs.arxiv.org/html/2310.03567 |
| COPC | Primeros 589 bytes; páginas de jerarquía de entradas de 32 B | Leído | https://copc.io/ |
| Untwine | Memoria proporcional a hilos; sin límite de memoria ni cifras de velocidad | README leído | https://github.com/hobu/untwine |
| Rasterizado por cómputo | `atomicMin` de 64 bits; Morton barajado más rápido en el 65 % de los casos; 2,25× de media sobre `GL_POINTS` (RTX 3090) | Leído | https://arxiv.org/html/2104.07526 |
| Rasterizado de 2 G de puntos | Lotes de 10 240 puntos, precisión adaptativa de 10 bits | Leído | https://arxiv.org/html/2204.01287 |
| WGSL | Atómicos solo de 32 bits | Leído | https://google.github.io/tour-of-wgsl/types/atomics/atomic-types |
| Límites WebGPU | `maxBufferSize` 256 MiB, `maxStorageBufferBindingSize` 128 MiB | Leído | https://developer.mozilla.org/en-US/docs/Web/API/GPUSupportedLimits |
| Pérdida de dispositivo | Reintentar salvo `destroy()`; recrear recursos | Leído | https://developer.mozilla.org/en-US/docs/Web/API/GPUDevice/lost |
| 3D Tiles | Error geométrico, `ADD` y `REPLACE` | Leído (sin fórmula explícita) | https://github.com/CesiumGS/3d-tiles/blob/main/specification/README.adoc |
| CesiumJS | `maximumScreenSpaceError` 16, error foveado, `cullRequestsWhileMoving`, `cacheBytes` | Leído | https://cesium.com/learn/cesiumjs/ref-doc/Cesium3DTileset.html |
| CesiumJS | 18 peticiones por servidor, ampliable para HTTP/2 y HTTP/3 | Leído | https://cesium.com/learn/cesiumjs/ref-doc/RequestScheduler.html |
| Potree | Prioridad por tamaño proyectado, presupuesto de puntos, LRU, 4 peticiones | Leído (CESCG 2015) | https://www.old.cescg.org/CESCG-2015/papers/Schutz-Rendering_Large_Point_Clouds_in_Web_Browsers.pdf |
| CLOD | LOD continuo, densidad gradual | Resumen | https://www.cg.tuwien.ac.at/research/publications/2019/schuetz-2019-CLOD/ |
| PMTiles v3 | Cabecera de 127 B; directorio raíz en los primeros 16 KiB | Leído | https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md |
| COG | Cabecera e IFD al principio; 3 peticiones en serie ≈ 540 ms | Leído | https://docs.ogc.org/is/21-026/21-026.html |
| Cabecera `Range` | Un solo rango no provoca *preflight* CORS; puede responderse 200 | Leído | https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Range |
| OPFS | Manejadores síncronos solo en workers dedicados | Leído | https://developer.mozilla.org/en-US/docs/Web/API/FileSystemSyncAccessHandle |
| Cuota | Hasta el 60 % del disco por origen en Chrome | Leído | https://web.dev/articles/storage-for-the-web |
| memory64 | Entre 10 % y más de 100 % más lento | Leído | https://spidermonkey.dev/blog/2025/01/15/is-memory64-actually-worth-using.html |
| `postMessage` | Transferir un `ArrayBuffer` no depende del tamaño | Leído (2019) | https://surma.dev/things/is-postmessage-slow/index.html |
| lzbench | LZ4 y zstd en Silesia | Leído | https://github.com/lz4/lz4 y https://github.com/facebook/zstd |
| *Bitshuffle* | Ratios y velocidades con LZ4 en datos sintéticos | Leído (2015) | https://www.blosc.org/posts/new-bitshuffle-filter/ |
| Draco | LAS de 4,48 GB a 0,64 GB con 1 mm; la decodificación en CPU es el cuello en disco local | Leído | https://cesium.com/blog/2019/02/26/draco-point-clouds/ |
| Radix en Rust | `voracious_radix_sort` frente a `sort_unstable` | Leído | https://docs.rs/voracious_radix_sort/latest/voracious_radix_sort/ |
| Morton | Bits mágicos y tablas ~10× sobre el bucle (2013, sin equipo declarado) | Leído | https://www.forceflow.be/2013/10/07/morton-encodingdecoding-through-bit-interleaving-implementations |
| Aritmética ancha en wasm | `i64.add128`, `i64.mul_wide_*`; 128 bits 2–7× más lento que nativo | Leído | https://github.com/WebAssembly/wide-arithmetic |
| Tablas Swiss | Byte de control por entrada, 16 comparaciones por SSE | Leído (sin cifras) | https://abseil.io/about/design/swisstables |
