# Octree Viewer

Visor de nubes LiDAR en Rust, WebAssembly y WebGPU. Convierte LAS/LAZ o EPT local a una
jerarquía espacial persistente y carga los bloques necesarios para la vista. El objetivo a largo
plazo es explorar fuentes mayores de **500 GB decimales** con memoria acotada. La escala completa
y cualquier superioridad frente a Potree siguen sin demostrarse. Índice de documentos:
[docs/README.md](docs/README.md); desarrollo y checks: [CONTRIBUTING.md](CONTRIBUTING.md).
Toolchain nightly fijado: [CONTRIBUTING](CONTRIBUTING.md#toolchain).

## Estado actual — 29-10-2026

- **V7 es la única línea de evolución**; V5/V6 se retiran al completarla. `v7.1-pre` hereda
  pre4, `v7.2-pre` reduce la búsqueda de ocupación y `v7.3-pre` limita diccionarios a tuplas de
  1/2/4 B. Son revisiones experimentales: su identidad por sí sola no reduce disco.
  [Plan y criterios](docs/explanation/format-v7-plan.md).
- Formato predeterminado durante la transición: **v5 definitivo** (`OCTVFRM1`, revisión 0), con
  `.oct` (coordenadas y jerarquía) y `.octattrs` (RGB y atributos numéricos) separados. Los
  archivos v1–v4 y v5 provisionales (`OCTVWFRM`) deben regenerarse desde sus fuentes.
- `v6.1-pre`–`v6.4-pre` están implementados y son optativos (comparación y compatibilidad);
  V6 final y pre5 permanecen rechazados. Identidades y qué añade cada una:
  [inventario](docs/reference/inventory.md#contrato-de-almacenamiento-y-atributos).
- Lectura: el reader async nativo admite concurrencia con cuota compartida; el viewer web
  solapa lecturas Blob priorizadas por LOD. Falta la caché del viewer y medir navegación
  concurrente.
- La ruta LOD voxelizada usa celdas diádicas enteras sin solapamiento entre generaciones; los
  huecos de la fuente no se rellenan. El invariante aún necesita cobertura en todos los
  consumidores de bloques ([contrato](docs/explanation/REGIONAL_VOXEL_LOD.md)).
- **wgpu/naga 30 y glam 0.34** compilan en nativo/WASM y pasan checks nativos y readback por
  software; la validación de navegador sigue parcial por pérdida de dispositivo
  ([límites](docs/explanation/RENDERER_DEPENDENCIES.md)).
- `createViewer` libera worker y recursos si falla la inicialización; EPT rechaza presupuestos
  estimados inviables antes de reservar planificación. No certifican GPU ni un límite RSS.
- Límites de lectura por defecto (p. ej. 256 atributos; mayores requieren
  `FormatReader::with_limits`): [inventario](docs/reference/inventory.md#límites). Las métricas
  son resultados de ejecuciones concretas, no límites garantizados ni objetivos cumplidos.

## Prioridades pendientes

Esta es la única lista activa. El [plan V7](docs/explanation/format-v7-plan.md) desarrolla contratos,
alternativas y verificación. No se desarrollarán nuevas revisiones V5/V6: se conservan durante
la transición como compatibilidad y referencia de comparación.

| Fase | Resultado y cierre | Estado |
| --- | --- | --- |
| 1 | Base `v7.1-pre`: reader/writer, conversión, identidad/checkpoints exactos y menor buffer auxiliar sin cambiar payloads. | Implementado; verificación de esta entrega al final del [plan](docs/records/benchmarks/format-v7-evidence.md#evidencia-del-primer-incremento). Sin mejora global de RSS/disco medida. |
| 2 | Plan de rangos y costes por selección/codec; presupuesto agregado de memoria y medición por fase de temporales y RSS. | Planes selectivos y preflight por lectura implementados, con rangos sin I/O y estimación de buffers. La cuota compartida conserva reservas de tareas y resultados; el benchmark verifica bytes leídos. Falta completar diagnóstico de temporales/RSS. |
| 3 | Reader lógico único por dataset en la aplicación, con llamadas concurrentes, prioridad/cancelación y bytes en vuelo; integrar scheduler en WASM y validar navegador. | El worker integra el scheduler WASM: reconcilia bloques por prioridad LOD, limita slots y reserva bytes antes de admitirlos. La cancelación de tickets es lógica y los errores solo se reintentan tras una refinación explícita. Falta validar navegación LOD con GPU y medir caché/memoria concurrente. [Plan y evidencia](docs/records/benchmarks/format-v7-evidence.md#evidencia-del-séptimo-incremento). |
| 4 | Writer con admisión por bytes, pipeline ordenado, EPT y recuperación; reducir pico de spools sin romper checkpoints. | Preparación paralela/pipeline ordenado y checkpoints EPT V7.1/V7.2/V7.3 por bloques implementados; identidad por rutas/tamaño/`mtime`, que puede omitir cambios con metadatos iguales. V7.2 limita la búsqueda a raw/delta/ocupación compacta de 24 bits y sigue experimental. Índice con pares `(u32, u32)` cuando caben y diccionarios de atributos con claves enteras conservan los payloads. Tres pares Belvedere del último cambio bajan 2,6% la preparación mediana y 1,4% el total; RSS pico muestreado 5,4% menor. Falta repetir corpus grandes, contrastar V5/V7 actuales y cerrar presupuestos/picos. Medición completa V5 frente a V7.3-pre del 28-10-2026 (Belvedere y Majadas, 3 rondas): pareja −7,5%/−38,6%, pero conversión ~+18% y RSS +12%/+24% ([registro](docs/records/benchmarks/STORAGE_V7_V73_VS_V5_2026-10-28.md)). [Plan y comparaciones](docs/explanation/format-v7-plan.md#4-writer-y-pico-de-disco), [A/B de atributos](docs/records/benchmarks/data/storage-v7-v72-integer-dictionary-2026-10-25.json). |
| 5 | Contrastar representación XYZ/atributos, índices y páginas con el corpus real, incluidos RAM, disco temporal y latencia selectiva. | [Sonda de atributos](docs/records/benchmarks/data/storage-v7-v72-attribute-candidates-2026-10-25.json) sobre 512 bloques Albuera: conservar diccionarios en tuplas de 1/2/4 B compone +0,185% de bytes y −46,5% de tiempo del encoder aislado. [V7.3-pre](docs/reference/format/FORMAT_V7_PRE3.md) implementado y verificado en nativo/async, EPT y Blob/OPFS WASM; conversión/pareja completas medidas el 28-10-2026: menos disco que V5 pero más tiempo, RSS y lectura completa ([registro](docs/records/benchmarks/STORAGE_V7_V73_VS_V5_2026-10-28.md)). |
| 6 | Promover V7 final/default, regenerar fixtures/datasets activos y eliminar backends/opciones V5/V6. | [Inventario operativo de migración](docs/explanation/format-v7-plan.md#inventario-operativo-para-la-promoción-y-retirada) documentado; suite comparativa admite V7. Pendiente de contratos, comparaciones y validación nativa/WASM/navegador. |
| 7 | Conversor web paralelo con paridad byte a byte con el CLI (núcleo sin I/O, pool de workers, hasta Majadas); después, lectura paralela con spec propia. | Spec aprobada 29-10-2026 con enmiendas del pre-mortem: [conversor web paralelo](docs/explanation/conversor-web-paralelo.md), ADR 0014/0015. Etapa 1 cerrada 29-10-2026: independencia de workers ampliada, LZ4 igual en nativo y wasm32 (S0), referencia CLI (M0) y spike de arenas (S1). Decisiones: trabajos por nodo, Chrome `file://` solo en RAM, medir cuota real (S8). Etapa 2 (29-10-2026): S0 con bloques reales de Belvedere en `check.sh`; cuota OPFS medida (S8: Firefox necesita `persist()` concedido; Chrome no falla hasta 40 GiB aunque declara 10 GiB); modelo por nodos: las arenas por trabajo superan 1,5 × y un archivo por spool de hijo (variante c) queda en ≤ 1,46 ×. Tareas 1–3 y 5 hechas. El spike S9 ([registro](docs/records/benchmarks/OPFS_SPOOL_FILES_2026-10-29.md)) mostró que un archivo OPFS por spool funciona pero con coste fijo por archivo; decisión del propietario 2 (29-10-2026): en la web, híbrido RAM + volcado por spool. M1 ([etapa 2](docs/explanation/conversor-web-paralelo-etapa2.md), Tarea 4) implementado: trabajos por nodo sobre `SpoolStore`/`PointSource`/`Clock` con los SHA de M0 y planificador de cola de listos por defecto tras la puerta R7 ([registro](docs/records/benchmarks/M1_R7_2026-10-29.md)); R7 pasó también en la sesión final sobre el candidato a merge ([sesión final](docs/records/benchmarks/M1_R7_2026-10-29.md#sesión-final-sobre-el-candidato-a-merge)). Etapa 3 ([plan](docs/explanation/conversor-web-paralelo-etapa3.md)): S2 ([registro](docs/records/benchmarks/LAZ_CHUNK_RANGES_2026-10-29.md)) decodifica rangos de chunks LAZ sin `seek` ni parche de `laz` (wasm32 1,74 × el nativo); S10 ([registro](docs/records/benchmarks/HYBRID_SPOOL_RAM_2026-10-29.md)) propone P3 (solo el coordinador vuelca) y, por la decisión del propietario 3, M3b compara P3 con spools de 2 y 4 GiB; M2: crate `viewer_convert_core` (lint y check wasm32, API serie, partición raíz por 1–8 rangos con bytes idénticos; puerta 3b +0,37 % de instrucciones, [registro](docs/records/benchmarks/M2_R7_2026-10-29.md)); M3a: motor `core` en el host web con paridad en Node/`check.sh` y en Chrome/Firefox ([registro](docs/records/benchmarks/M3A_PARITY_2026-10-29.md)); decisión del propietario 4: la norma portátil sustituye a `hypot` ([ADR 0016](docs/adr/0016-norma-portatil-en-el-error-de-cuantizacion.md)), con [nuevas referencias R1](docs/records/benchmarks/R1_REFERENCE_SHA_2026-10-29.md); la salida del CLI ya no depende de glibc. R7 completo (Tarea 6, 30-10-2026): pasa, mediana pareada 0,968 frente a `fb9a3b7` (8 pares; [registro](docs/records/benchmarks/M2_R7_2026-10-29.md#r7-completo-tarea-6)). Etapa 4 cerrada 30-10-2026 ([plan](docs/explanation/conversor-web-paralelo-etapa4.md)): M3b hecho. Belvedere en Chrome y Firefox con SHA = R1 a 2,1–2,9 × nativo y PSS 3,4–4,5 GiB con spools de 2 GiB ([navegadores](docs/records/benchmarks/M3B_BELVEDERE_2026-10-30.md); puerta Node 2,8 ×, [registro](docs/records/benchmarks/M3B_NODE_GATE_2026-10-30.md)); spools en segmentos de JS y zonas OPFS paginadas ([ADR 0017](docs/adr/0017-zonas-opfs-paginadas-para-spools-web.md), [medidas OPFS](docs/records/benchmarks/OPFS_ZONES_2026-10-30.md)); decisión 3 resuelta en 2 GiB; conversor anterior eliminado. Siguiente: M4 (pool), M5, M6. |
| 8 | Render y carga a escala: cientos de GB con vóxeles de 1–8 px y carga continua (sin saltos, huecos ni bloqueos). | [Evaluación](docs/records/archive/RENDER_ASSESSMENT_2026-10-30.md) del 30-10-2026: la voxelización en CPU por vista del corte completo y la pirámide LOD sin densidades intermedias (8³ representantes frente a hojas de 262 144 puntos) dominan; Belvedere no completa 4 px en 600 s. [Plan](docs/explanation/render-y-carga-escala.md): R0 arnés y línea base, R1 mejoras sin decisiones del propietario, R2 decodificación fuera del hilo de render; R3–R5 esperan las [preguntas pendientes](docs/explanation/preguntas-propietario.md) P1–P6. |

<details>
<summary>Pendientes anteriores conservados para después de V7 y como escenarios de regresión</summary>

El [inventario por componente](docs/reference/inventory.md) conserva el mapa del proyecto.
Las entregas cerradas están en [el archivo del 14-10-2026](docs/records/archive/DELIVERIES_2026-10-14.md).
Las filas V6 siguientes describen el punto de partida histórico; su evolución se absorbe en V7.

| Orden | Trabajo abierto y componentes | Siguiente paso / criterio de cierre |
| --- | --- | --- |
| 1 | **Corrección visual y transición LOD** — core, render, WASM. | El planificador admite un hijo directo residente junto al padre Replacement cuando su frontera coincide con la grilla y no supera la resolución de los representantes del padre. Durante una carga, conserva por región el último corte visible con cobertura completa tanto en Replacement como en Additive; al completar el objetivo publica solo el corte nuevo. Los payloads retenidos respetan el presupuesto de caché: si no cabe el bloque nuevo, la inserción se rechaza y la escena queda visible hasta que otro objetivo residente cierre la transición o se reinicie el dataset. El builder omite los puntos del padre dentro de la región del hijo y conserva RGB/atributos propios. Firefox headless presentó un `.oct` real de 55 bloques, conservó el corte visible tras un checksum fallido y recuperó la carga al abrir el archivo sano; [protocolo y límites](docs/explanation/REGIONAL_VOXEL_LOD.md). Faltan zoom, más cámaras/cargas, Chrome/GPU objetivo y memoria/presentación representativa. `sync_visible_blocks` solo vacía la escena en WASM; el corte voxelizado se publica con `upload_voxel_cut`. |
| 2 | **Validación de navegador** — WASM y host. | Firefox 158 y Chrome 156 local con AMD Radeon 880M/RDNA3 presentaron clear y muestra WASM. Chrome completó picking, recentrado, resize, carga del host a 4 px y cambio de vista con `W` en modo pájaro; a 390 px no hubo desbordamiento horizontal. [Entorno, pasos y límites](docs/explanation/RENDERER_DEPENDENCIES.md). Faltan pérdida de dispositivo reproducible y recuperación observada, móvil físico y teclado físico. La limpieza de `createViewer` ante fallo síncrono tiene regresiones Node ([evaluación](docs/records/archive/CODE_ASSESSMENT_2026-10-24.md)); readback/picking no cierran por sí solos la presentación. |
| 3 | **Pico conjunto de memoria** — core, render, WASM. | Repeticiones independientes de Firefox 158 en Belvedere a 8 px y Additive real parcial completan el corte y el cambio de cámara. En tres Belvedere actuales el PSS muestreado llega a 3,47–3,49 GB (RSS 4,28–4,30 GB); el A/B con el planner anterior también llega a 3,56 GB. `smaps` encuentra páginas anónimas, sin identificar su dueño; el pico no se atribuye al cambio LOD. [Medidas, A/B y límites](docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md). Falta explicar esa memoria y capturar asignaciones transitorias con más resolución, repetir Additive ramificado representativo y evaluar acumulación persistente. |
| 4 | **Conversión y escala de extremo a extremo** — convert y host. | Se repitieron las conversiones v5 actuales de Albuera (354→449 s) y Majadas (299→318 s); ambos pares `.oct`/`.octattrs` conservaron hashes idénticos. Firefox 158 repitió Majadas a 4 px: 54,0 s, 239 bloques, 0 pendientes; PSS/RSS observados 0,97/1,77 GB frente a 3,73/4,54 GB antes, variación no explicada que no se atribuye a una mejora. [Conversión y límites](docs/records/benchmarks/V5_MEMORY_BENCH.md#repetición-de-conversión-v5-actual-24-10-2026), [host](docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md). Belvedere mostró primer frame grueso en 28 ms, pero tras 600 s a 4 px aún tenía 783 bloques pendientes y 87 expulsiones; PSS llegó a 2,59 GB. Sigue pendiente completar Belvedere a 4 px y repetir Albuera en host; separar apertura, I/O, temporales y memoria. El rechazo temprano del presupuesto estimado EPT tiene regresiones; falta medir RSS y escala real ([evaluación](docs/records/archive/CODE_ASSESSMENT_2026-10-24.md)). Más de 500 GB y comparación con Potree requieren hardware, calidad y tareas equivalentes. |
| 5 | **Requisitos de producto y soporte** — transversal. | Acordar plataformas y mínimos RAM/GPU, dimensiones LAS/LAZ (CRS, flags, Extra Bytes), licencia/compatibilidad y alcance de medición, clasificación, recorte y exportación. Documentar decisiones antes de prometer soporte. |
| 6 | **Almacenamiento v6: lectura selectiva y transporte** — format, core y WASM. | El trabajo V7 ya conecta el Blob async al worker; la tabla histórica sigue documentando la ruta V6 previa, mientras cuotas, concurrencia y cancelación lógica coherentes quedan en la prioridad 3. No se desarrollarán nuevas revisiones V6. |
| 7 | **Siguientes candidatos v6** — core y format. | Perfilar selector de codecs; comparar grupos RGB exactos, bitpacking por rango y resúmenes/diccionarios con los codecs pre3/pre4 en la misma envoltura. Exigir parejas válidas, lectura selectiva, precisión, corrupción, checkpoints y WASM antes de otra revisión incompatible; mantener v6 final rechazado. |
| 8 | **Grilla física y varias nubes** — core y format. | Fijar CRS, unidades/ejes, ancla y escala diádica; decidir grilla global o transformación explícita por nube. Probar negativos, límites y nubes convertidas por separado, independientemente del orden de carga. La ocupación cuantizada por bloque de pre2/pre4 no resuelve este contrato. |
| 9 | **Avisos upstream de seguridad (backlog)** — las/laz. | `las` `raw/vlr.rs:86` redimensiona los datos EVLR con el u64 de la cabecera; `las` `raw/header.rs:358-359` desborda en debug `puntos × record_len` y la suma `offset_to_point_data + puntos × record_len` (también en LAZ); `laz` `chunk_table.rs:154` hace `with_capacity(number_of_chunks)`, con entradas i32 extendidas con signo y `chunk_size` fijo que dimensionan capacidad/`resize` en `parallel/decompression.rs:70/:149/:196/:234`. Mitigaciones propias: `viewer_core::las::preflight_las` y la comprobación de la tabla de chunks en `viewer_convert::las_source` ([ADR 0012](docs/adr/0012-validacion-previa-de-cabeceras-las-laz.md)). Decisión 28-10-2026: reportar más adelante y mantener las guardas propias. Añadidos 29-10-2026: pánicos del decodificador `laz` con datos corruptos y `LasZipDecompressor::seek` incorrecto con chunks de tamaño variable (`sequential/decompression.rs:~203`); `lz4_flex` 0.14 codifica distinto en wasm32 que en 64 bits (`block/hashtable.rs:26-50`). |

Estos pendientes se conservan; V7 concentra ahora el desarrollo. Los avances de almacenamiento
no cierran la validación visual ni la memoria conjunta. El detalle y los límites están en los
[benchmarks web](docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md), de
[vóxeles](docs/records/benchmarks/VOXEL_BUILD_BENCH.md) y de [almacenamiento](docs/records/benchmarks/STORAGE_BENCH.md).
Caras expuestas/agrupadas, índice exterior compacto y otros añadidos se mantienen como propuestas
condicionadas en el inventario, sin convertirlos en compromisos nuevos.

</details>

## Uso y formato

```sh
cargo run --profile perf -p viewer_convert -- INPUT.laz OUTPUT.oct --format-version v7.3-pre
# v5 es el default si se omite --format-version. También admite un directorio EPT local
# laszip o su ept.json. --checkpoint conserva fases LAS/LAZ y bloques EPT (V7.1/V7.2/V7.3-pre);
# --resume retoma la misma entrada/opciones. v7.2-pre limita ocupación a compact24; v7.3-pre
# además limita diccionarios a tuplas de 1/2/4 B.
```

Cada `v7.N-pre` identifica una revisión exacta de desarrollo, aún provisional; `v7` final y
revisiones posteriores a V7.3 se rechazan, igual que versiones sin backend o checkpoints de otra
identidad. Especificaciones:
[V5](docs/reference/format/FORMAT_V5.md), [V7.1-pre](docs/reference/format/FORMAT_V7_PRE1.md),
[V7.2-pre](docs/reference/format/FORMAT_V7_PRE2.md), [V7.3-pre](docs/reference/format/FORMAT_V7_PRE3.md).

| Archivo | Contenido | Uso |
| --- | --- | --- |
| `OUTPUT.oct` | XYZ cuantizado a `u32`, marco local, jerarquía e índice común. | Se abre sin atributos; se visualiza en gris. |
| `OUTPUT.octattrs` | RGB de presentación y columnas numéricas tipadas, alineadas por bloque/fila. | Opcional; habilita RGB, intensidad, clasificación y componentes numéricos. |

La publicación de los dos archivos no es atómica; el conversor valida candidatos y publica
atributos antes de coordenadas. En la demo, abrir `.oct` y seleccionar después su `.octattrs`, o
asociar el acompañante antes. Para desarrollo, servir el repositorio por HTTP
(`python3 -m http.server 8000 --bind 127.0.0.1`, abrir `/examples/web-host/index.html`); para
doble clic, `artifacts/visor-web-local/index.html`. Contrato, regeneración y pruebas:
[README del host web](examples/web-host/README.md).

La conversión conserva intensidad, clasificación, retornos, ángulo, user data y point source ID;
si existen, GPS time, RGB16 y NIR. Flags LAS, Extra Bytes y CRS requieren trabajo adicional y las
omisiones se notifican. El `.oct` conserva marco local `f64` y error de cuantización acotado; el
render reconstruye posiciones locales `f32`.

## Arquitectura

| Área | Responsabilidad | Límite relevante |
| --- | --- | --- |
| `viewer_core` | Geometría, jerarquía, LOD, caché y contratos compartidos (incluido el contrato LAS en `las`). | El índice residente y la memoria estimada no equivalen a RSS. |
| `viewer_format` | V5, v6 pre1–pre4 y V7.1/V7.2/V7.3-pre; política de versiones, cuantización, codecs, checksums, lectura selectiva. | XYZ lógico ocupa 12 bytes/punto; los enteros de 64 bits conservan bits en disco, pero su proyección de color puede perder precisión. |
| `viewer_convert_core` | Núcleo puro de la conversión (planificador, bloques, partición por rangos, API serie `convert_serial`); compila para wasm32. | Sin `fs`, hilos, reloj ni libm: lo vigila un lint de clippy y `check.sh`. |
| `viewer_convert` | Shell nativo: CLI, ficheros, hilos, spools/checkpoints, EPT local y publicación. | EPT remoto y binary/zstandard no están soportados. Checkpoint EPT en V7.1/V7.2/V7.3-pre; identifica la fuente por metadatos, no por hash. |
| `viewer_render_wgpu` | Cámara, voxelización, buffers GPU, HZB y presentación. | Los presupuestos propios no representan toda la VRAM. |
| `viewer_wasm` | Worker, carga LOD y conversión web (`WasmCoreConverter` sobre `viewer_convert_core`, spools en segmentos de JS y zonas OPFS). | Lecturas por bloque síncronas; convertir cientos de GB en navegador no es requisito. |
| `examples/web-host` | Demo, API embebible, carga y descarga de archivos. | OPFS/cuota, pérdida de GPU, picking y cancelación requieren validación en navegadores objetivo. |

Mapa de código, herramientas y límites por componente: [inventario](docs/reference/inventory.md).
