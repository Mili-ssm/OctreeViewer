# Octree Viewer

Visor de nubes LiDAR en Rust, WebAssembly y WebGPU. Convierte LAS/LAZ o EPT local a una
jerarquía espacial persistente y carga los bloques necesarios para la vista. El objetivo a largo
plazo es explorar fuentes mayores de **500 GB decimales** con memoria acotada. La escala completa
y cualquier superioridad frente a Potree siguen sin demostrarse.

## Estado actual — 25-10-2026

- **V7 es la prioridad exclusiva de evolución**, con objetivo de retirar V5/V6 tras completar
  la transición. La base optativa `v7.1-pre` hereda los contratos de pre4 y añade una mejora
  compatible de buffers del encoder; la identidad por sí sola no reduce disco. El reader nativo
  async ya permite concurrencia con cuota compartida y el viewer web solapa lecturas Blob
  priorizadas por LOD con resultados retenidos bajo cuota; falta agregar la caché del viewer y
  medir navegación concurrente. [Plan y
  criterios](docs/FORMAT_V7_PLAN.md).
- El formato predeterminado durante la transición es **v5 definitivo** (`OCTVFRM1`, revisión 0), separado en `.oct` (coordenadas y jerarquía) y
  `.octattrs` (RGB y atributos numéricos). Se lee y escribe v5; hay que regenerar archivos v1–v4 y v5 provisionales (`OCTVWFRM`) desde sus fuentes.
- Están implementados **`v6.1-pre`, `v6.2-pre`, `v6.3-pre` y `v6.4-pre` optativos**. Pre1 añade deltas; pre2 serializa
  ocupación en arrays por profundidad dentro de cada bloque, con registros de 6 bytes y residuales
  exactos. Se elige el menor payload tras LZ4, con fallback raw/delta. V5 sigue predeterminado;
  pre3 añade constantes/diccionarios exactos por columna; pre4 añade planos de bytes independientes
  y topología de máscaras con hojas/prefijos implícitos, con fallback y lectura selectiva.
  V6 final y pre5 permanecen rechazados.
- La conversión admite LAS/LAZ y EPT local. La demo web admite carga progresiva por bloques,
  color por RGB o componente numérico, cámara orbital y modo pájaro.
- La ruta LOD voxelizada usa celdas diádicas enteras sin solapamiento entre generaciones. El
  objetivo en píxeles guía la selección; no cambia arbitrariamente el tamaño físico de las celdas.
  Los huecos de la fuente no se rellenan. Este invariante todavía necesita cobertura en todos los
  consumidores de bloques.
- La migración a **wgpu/naga 30 y glam 0.34** ya compila en nativo/WASM y pasa
  los checks nativos y readback por software. La validación de navegador sigue parcial por
  pérdida de dispositivo también reproducida con un clear independiente.
  [Cambios de contrato, checks y límites](docs/RENDERER_DEPENDENCIES.md).
- `createViewer` libera el worker y los recursos registrados si falla la inicialización;
  EPT rechaza presupuestos estimados inviables al leer la jerarquía y antes de reservar
  bloques de planificación. Son correcciones locales; no certifican GPU ni un límite RSS.
- El lector de formato predeterminado admite hasta 256 atributos. El escritor permite
  esquemas mayores y su apertura requiere `FormatReader::with_limits`; hay una regresión
  con 257 atributos. La política de compatibilidad predeterminada sigue por decidir.
- Las métricas de conversión, memoria, LOD y navegador son resultados de ejecuciones concretas,
  con límites de interpretación. No equivalen a límites garantizados ni a objetivos ya cumplidos.

## Prioridades pendientes

Esta es la única lista activa. El [plan V7](docs/FORMAT_V7_PLAN.md) desarrolla contratos,
alternativas y verificación. No se desarrollarán nuevas revisiones V5/V6: se conservan durante
la transición como compatibilidad y referencia de comparación.

| Fase | Resultado y cierre | Estado |
| --- | --- | --- |
| 1 | Base `v7.1-pre`: reader/writer, conversión, identidad/checkpoints exactos y menor buffer auxiliar sin cambiar payloads. | Implementado; verificación de esta entrega al final del [plan](docs/FORMAT_V7_PLAN.md#evidencia-del-primer-incremento). Sin mejora global de RSS/disco medida. |
| 2 | Plan de rangos y costes por selección/codec; presupuesto agregado de memoria y medición por fase de temporales y RSS. | Planes selectivos y preflight por lectura implementados, con rangos sin I/O y estimación de buffers. La cuota compartida conserva reservas de tareas y resultados; el benchmark verifica bytes leídos. Falta completar diagnóstico de temporales/RSS. |
| 3 | Reader lógico único por dataset en la aplicación, con llamadas concurrentes, prioridad/cancelación y bytes en vuelo; integrar scheduler en WASM y validar navegador. | El worker integra el scheduler WASM: reconcilia bloques por prioridad LOD, limita slots y reserva bytes antes de admitirlos. La cancelación de tickets es lógica y los errores solo se reintentan tras una refinación explícita. Falta validar navegación LOD con GPU y medir caché/memoria concurrente. [Plan y evidencia](docs/FORMAT_V7_PLAN.md#evidencia-del-septimo-incremento). |
| 4 | Writer con admisión por bytes, pipeline ordenado, EPT y recuperación; reducir pico de spools sin romper checkpoints. | EPT: eliminado un buffer RGB temporal (6 B/punto coloreado teóricos), con U16/display intactos; `--pipeline-writes` solapa lectura/preparación con escritura ordenada. Checkpoint V7.1-pre por bloques confirmado, journal durable, `Reader::seek` y publicación recuperable; pruebas de cursor dentro de LAZ, metadatos cambiados y corte antes de publicar. La identidad enumera rutas, tamaños y `mtime`; cambios con los mismos metadatos pueden pasar inadvertidos. Una serie secuencial aislada de Albuera verificó checksum lógico idéntico en V5/V6.4/V7: V7 ocupa lo mismo que V6.4 y 24,48% menos que V5. RSS y temporales muestreados no muestran menor pico V7; los tiempos de una ejecución/formato no estiman dispersión. Falta base multiformato en más workers y datasets, checkpoint on/off, y admisión por bytes. [Medidas y límites](docs/benchmarks/storage-v7-albuera-v5-v64-v71-sequential-2026-10-25.json) y [plan](docs/FORMAT_V7_PLAN.md#4-writer-y-pico-de-disco). |
| 5 | Contrastar representación XYZ/atributos, índices y páginas con el corpus real, incluidos RAM, disco temporal y latencia selectiva. | Pendiente; solo incorporar candidatos con evidencia. |
| 6 | Promover V7 final/default, regenerar fixtures/datasets activos y eliminar backends/opciones V5/V6. | Pendiente de contratos, comparaciones y validación nativa/WASM/navegador. |

<details>
<summary>Pendientes anteriores conservados para después de V7 y como escenarios de regresión</summary>

El [inventario por componente](docs/PROJECT_INVENTORY.md) conserva el mapa del proyecto.
Las entregas cerradas están en [el archivo del 14-10-2026](docs/DELIVERIES_2026-10-14.md).
Las filas V6 siguientes describen el punto de partida histórico; su evolución se absorbe en V7.

| Orden | Trabajo abierto y componentes | Siguiente paso / criterio de cierre |
| --- | --- | --- |
| 1 | **Corrección visual y transición LOD** — core, render, WASM. | El planificador admite un hijo directo residente junto al padre Replacement cuando su frontera coincide con la grilla y no supera la resolución de los representantes del padre. Durante una carga, conserva por región el último corte visible con cobertura completa tanto en Replacement como en Additive; al completar el objetivo publica solo el corte nuevo. Los payloads retenidos respetan el presupuesto de caché: si no cabe el bloque nuevo, la inserción se rechaza y la escena queda visible hasta que otro objetivo residente cierre la transición o se reinicie el dataset. El builder omite los puntos del padre dentro de la región del hijo y conserva RGB/atributos propios. Firefox headless presentó un `.oct` real de 55 bloques, conservó el corte visible tras un checksum fallido y recuperó la carga al abrir el archivo sano; [protocolo y límites](docs/REGIONAL_VOXEL_LOD.md). Faltan zoom, más cámaras/cargas, Chrome/GPU objetivo y memoria/presentación representativa. `sync_visible_blocks` solo vacía la escena en WASM; el corte voxelizado se publica con `upload_voxel_cut`. |
| 2 | **Validación de navegador** — WASM y host. | Firefox 158 y Chrome 156 local con AMD Radeon 880M/RDNA3 presentaron clear y muestra WASM. Chrome completó picking, recentrado, resize, carga del host a 4 px y cambio de vista con `W` en modo pájaro; a 390 px no hubo desbordamiento horizontal. [Entorno, pasos y límites](docs/RENDERER_DEPENDENCIES.md). Faltan pérdida de dispositivo reproducible y recuperación observada, móvil físico y teclado físico. La limpieza de `createViewer` ante fallo síncrono tiene regresiones Node ([evaluación](docs/CODE_ASSESSMENT_2026-10-24.md)); readback/picking no cierran por sí solos la presentación. |
| 3 | **Pico conjunto de memoria** — core, render, WASM. | Repeticiones independientes de Firefox 158 en Belvedere a 8 px y Additive real parcial completan el corte y el cambio de cámara. En tres Belvedere actuales el PSS muestreado llega a 3,47–3,49 GB (RSS 4,28–4,30 GB); el A/B con el planner anterior también llega a 3,56 GB. `smaps` encuentra páginas anónimas, sin identificar su dueño; el pico no se atribuye al cambio LOD. [Medidas, A/B y límites](docs/WEB_PERFORMANCE_BENCH.md). Falta explicar esa memoria y capturar asignaciones transitorias con más resolución, repetir Additive ramificado representativo y evaluar acumulación persistente. |
| 4 | **Conversión y escala de extremo a extremo** — convert y host. | Se repitieron las conversiones v5 actuales de Albuera (354→449 s) y Majadas (299→318 s); ambos pares `.oct`/`.octattrs` conservaron hashes idénticos. Firefox 158 repitió Majadas a 4 px: 54,0 s, 239 bloques, 0 pendientes; PSS/RSS observados 0,97/1,77 GB frente a 3,73/4,54 GB antes, variación no explicada que no se atribuye a una mejora. [Conversión y límites](docs/V5_MEMORY_BENCH.md#repetición-de-conversión-v5-actual-24-10-2026), [host](docs/WEB_PERFORMANCE_BENCH.md). Belvedere mostró primer frame grueso en 28 ms, pero tras 600 s a 4 px aún tenía 783 bloques pendientes y 87 expulsiones; PSS llegó a 2,59 GB. Sigue pendiente completar Belvedere a 4 px y repetir Albuera en host; separar apertura, I/O, temporales y memoria. El rechazo temprano del presupuesto estimado EPT tiene regresiones; falta medir RSS y escala real ([evaluación](docs/CODE_ASSESSMENT_2026-10-24.md)). Más de 500 GB y comparación con Potree requieren hardware, calidad y tareas equivalentes. |
| 5 | **Requisitos de producto y soporte** — transversal. | Acordar plataformas y mínimos RAM/GPU, dimensiones LAS/LAZ (CRS, flags, Extra Bytes), licencia/compatibilidad y alcance de medición, clasificación, recorte y exportación. Documentar decisiones antes de prometer soporte. |
| 6 | **Almacenamiento v6: lectura selectiva y transporte** — format, core y WASM. | El trabajo V7 ya conecta el Blob async al worker; la tabla histórica sigue documentando la ruta V6 previa, mientras cuotas, concurrencia y cancelación lógica coherentes quedan en la prioridad 3. No se desarrollarán nuevas revisiones V6. |
| 7 | **Siguientes candidatos v6** — core y format. | Perfilar selector de codecs; comparar grupos RGB exactos, bitpacking por rango y resúmenes/diccionarios con los codecs pre3/pre4 en la misma envoltura. Exigir parejas válidas, lectura selectiva, precisión, corrupción, checkpoints y WASM antes de otra revisión incompatible; mantener v6 final rechazado. |
| 8 | **Grilla física y varias nubes** — core y format. | Fijar CRS, unidades/ejes, ancla y escala diádica; decidir grilla global o transformación explícita por nube. Probar negativos, límites y nubes convertidas por separado, independientemente del orden de carga. La ocupación cuantizada por bloque de pre2/pre4 no resuelve este contrato. |

Estos pendientes se conservan; V7 concentra ahora el desarrollo. Los avances de almacenamiento
no cierran la validación visual ni la memoria conjunta. El detalle y los límites están en los
[benchmarks web](docs/WEB_PERFORMANCE_BENCH.md), de
[vóxeles](docs/VOXEL_BUILD_BENCH.md) y de [almacenamiento](docs/STORAGE_BENCH.md).
Caras expuestas/agrupadas, índice exterior compacto y otros añadidos se mantienen como propuestas
condicionadas en el inventario, sin convertirlos en compromisos nuevos.

</details>

## Uso y formato

Conversión nativa:

```sh
cargo run --profile perf -p viewer_convert -- INPUT.laz OUTPUT.oct --format-version v7.1-pre
# v5 sigue siendo el default si se omite --format-version durante la transición.
# También admite un directorio EPT local laszip o su ept.json.
# --checkpoint conserva fases LAS/LAZ y bloques EPT V7.1-pre; --resume retoma la misma entrada/opciones.
```

El comando anterior produce una pareja `v7.1-pre`, aún provisional. Las muestras versionadas
siguen siendo V5 y se regenerarán antes de retirar su backend. Los archivos locales provisionales
`OCTVWFRM` deben reconvertirse desde sus fuentes. `v5` y `v6.1-pre`–`v6.4-pre` siguen disponibles
para compatibilidad y comparación durante la transición. Se rechazan versiones sin backend antes
de escribir y checkpoints de otra identidad. `v7.1-pre` identifica una revisión exacta de
desarrollo; `v7` final y revisiones posteriores siguen rechazadas.

La pareja contiene:

| Archivo | Contenido | Uso |
| --- | --- | --- |
| `OUTPUT.oct` | XYZ cuantizado a `u32` (V7.1-pre hereda raw/delta/ocupación compacta de V6 pre4), marco local, jerarquía e índice común. | Se puede abrir sin atributos; se visualiza en gris. |
| `OUTPUT.octattrs` | RGB de presentación y columnas numéricas tipadas, alineadas por bloque/fila. | Opcional; habilita RGB, intensidad, clasificación y selección de componentes numéricos. |

En la demo, abrir `.oct` y seleccionar después su `.octattrs`, o asociar el acompañante antes de
abrir las coordenadas. Para desarrollo, servir el repositorio por HTTP:

```sh
python3 -m http.server 8000 --bind 127.0.0.1
# Abrir http://localhost:8000/examples/web-host/index.html
```

Para doble clic se usa el paquete autocontenido `artifacts/visor-web-local/index.html`. Contrato,
regeneración y pruebas de ambos modos: [README del host web](examples/web-host/README.md).
Especificación de la base nueva: [V7.1-pre](crates/viewer_format/FORMAT_V7_PRE1.md);
[contrato V5](crates/viewer_format/FORMAT_V5.md) durante la transición. La publicación de los dos
archivos no es atómica; el conversor valida candidatos y publica atributos antes de coordenadas.

## Arquitectura y límites conocidos

| Área | Responsabilidad | Límite relevante |
| --- | --- | --- |
| `viewer_core` | Geometría, jerarquía, LOD, caché y contratos compartidos. | El índice residente y la memoria estimada no equivalen a RSS. |
| `viewer_format` | V7.1-pre, V5 y v6 pre1/pre2/pre3/pre4, cuantización, codecs, checksums y lectura selectiva. | XYZ lógico ocupa 12 bytes/punto; v6 puede reducir el stream antes de LZ4; los enteros de 64 bits conservan bits en disco, pero su proyección de color puede perder precisión. |
| `viewer_convert` | Conversión LAS/LAZ con spools/checkpoints y EPT local. | EPT remoto y binary/zstandard no están soportados. Checkpoint EPT solo en V7.1-pre; identifica la fuente por metadatos, no por hash de contenido. |
| `viewer_render_wgpu` | Cámara, voxelización, buffers GPU, HZB y presentación. | Los presupuestos propios no representan toda la VRAM; la GPU puede reservar recursos fuera del control del renderer. |
| `viewer_wasm` | Worker, carga LOD y conversión web en dos pasadas. | Lecturas por bloque síncronas; convertir fuentes de cientos de GB en navegador no es requisito. |
| `examples/web-host` | Demo, API embebible, carga y descarga de archivos. | OPFS/cuota, pérdida de GPU, picking y cancelación requieren validación real en navegadores objetivo. |

La conversión conserva intensidad, clasificación, retornos, ángulo, user data y point source ID; si
existen, GPS time, RGB16 y NIR. Flags LAS, Extra Bytes y CRS requieren trabajo adicional y las
omisiones se notifican. El `.oct` conserva marco local `f64` y error de cuantización acotado; el
render reconstruye posiciones locales `f32`.

## Documentación

Punto de entrada para retomar trabajo: [inventario del proyecto](docs/PROJECT_INVENTORY.md).
Incluye mapa de código, herramientas, cobertura disponible y propuestas de mejora.
La [evaluación del código del 24-10-2026](docs/CODE_ASSESSMENT_2026-10-24.md) contrasta
documentación, criterios de desarrollo, checks y próximos pasos; las prioridades activas
siguen siendo las de este README.

| Documento | Contenido vigente |
| --- | --- |
| [Plan V7](docs/FORMAT_V7_PLAN.md) y [base V7.1-pre](crates/viewer_format/FORMAT_V7_PRE1.md) | Prioridad actual: recursos, async/paralelismo, candidatos de formato y retirada de V5/V6. |
| [Preparación v6](docs/STORAGE_V6_PREPARATION.md) y [pre1](crates/viewer_format/FORMAT_V6_PRE1.md) / [pre2](crates/viewer_format/FORMAT_V6_PRE2.md) / [pre3](crates/viewer_format/FORMAT_V6_PRE3.md) / [pre4](crates/viewer_format/FORMAT_V6_PRE4.md) | Traits, formatos experimentales, compatibilidad y criterios pendientes. |
| [Pre3](docs/STORAGE_V6_PRE3_BENCH.md), [pre4](docs/STORAGE_V6_PRE4_BENCH.md) y [SensOFOREST completo](docs/SENSOFOREST_V6_BENCH.md) | Comparaciones de parejas válidas, tiempos, lectura selectiva y RSS con sus límites. |
| [I/O v6](docs/STORAGE_V6_IO.md), [CPU/CRC/diccionarios](docs/STORAGE_V6_CPU.md) y [planos de bytes](docs/STORAGE_V6_BYTE_PLANES.md) | Pipeline optativo, lectores del benchmark y optimizaciones compatibles; tiling descartado para la ruta actual. |
| [Resúmenes por nodo](docs/NODE_ATTRIBUTE_SUMMARIES.md) y [diccionarios](docs/ATTRIBUTE_DICTIONARY_BENCH.md) | Experimentos; no equivalen a integración en `.oct`/`.octattrs`. |
| [Benchmarks de almacenamiento](docs/STORAGE_BENCH.md) | Suite repetida: tamaños, escritura, reconstrucción y carga secuencial de hijos. |
| [Ocupación cuantizada](docs/QUANTIZED_OCCUPANCY_BENCH.md) | Residuales exactos, coste del índice en memoria y límites del candidato. |
| [Benchmark v5 de memoria y lectura](docs/V5_MEMORY_BENCH.md) | Conversión, acceso a bloques, memoria y perfiles nativos. |
| [Benchmark de construcción de vóxeles](docs/VOXEL_BUILD_BENCH.md) | Construcción, memoria conjunta, acumuladores y experimentos de reutilización. |
| [Resolución regional LOD](docs/REGIONAL_VOXEL_LOD.md) | Contrato de celdas, fallback, regresiones, probe WebGPU y comparación nativa de cortes Belvedere. |
| [Dependencias del renderer](docs/RENDERER_DEPENDENCIES.md) | Migración wgpu 30/glam 0.34, contratos de superficie/readback y validación parcial de navegador. |
| [Benchmark web](docs/WEB_PERFORMANCE_BENCH.md) | Métricas de navegador, flujos, resultados y limitaciones visuales. |
| [Selección LOD](docs/LOD_FRONTIER_BENCH.md) | Contratos y mediciones de selección de frontera y prioridad incremental. |
| [Datos reales](docs/REAL_DATASET_BENCH.md) | Protocolo y resultados históricos; revisar fecha/versión antes de comparar con v5. |
| [Ramificación espacial](docs/SPATIAL_BRANCHING_BENCH.md) | Experimentos de subdivisión y cortes con diferentes ramas. |
| [Anchura de coordenadas](docs/COORDINATE_WIDTH_BENCH.md) | Evaluación del experimento `u16` y sus límites. |
| [Entregas del 14-10-2026](docs/DELIVERIES_2026-10-14.md) | Archivo de implementaciones y experimentos cerrados, con checks históricos. |
| [Historial técnico](docs/DEVELOPMENT_HISTORY.md) | Snapshot anterior a v5; archivo de decisiones y evidencia, no hoja de ruta activa. |
| [Dataset de escala](data/datasets/README.md) y [muestras](data/samples/README.md) | Fuentes, procedencia y preparación reproducible. |

Los documentos de benchmark mantienen los comandos, versiones, dispersión y contraejemplos. Sus
cifras no deben trasladarse a otra versión, navegador, dataset o equipo sin una comparación válida.

## Desarrollo

Comprobaciones habituales del workspace (requieren dependencias ya disponibles para `--offline`):

```sh
cargo test-fast --offline
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo clippy -p viewer_wasm --target wasm32-unknown-unknown --offline -- -D warnings
node --test examples/web-host/tests/*.test.mjs
python3 -m unittest discover -s examples/web-host/tests -p 'test_*.py'
python3 -m unittest discover -s crates/viewer_convert/benchmarks -p 'test_*.py'
```

Las pruebas GPU ignoradas requieren un adaptador compatible o Vulkan por software. Para cambios en
el host web, además de las suites anteriores, comprobar con Playwright los flujos afectados, la
consola y escritorio/móvil cuando corresponda. No considerar tests nativos ni lecturas GPU como
evidencia de presentación visual en todos los navegadores.

Para compilar WASM y generar bindings/paquete local, seguir el
[host web](examples/web-host/README.md). Los perfiles `fast`, `perf` y `release` tienen objetivos
distintos; registrar el perfil al comparar medidas. El inventario inicial no reejecutó
suites ni benchmarks; la [migración posterior](docs/RENDERER_DEPENDENCIES.md) registra los checks
actuales y sus límites. Las mediciones de rendimiento anteriores pertenecen a sus revisiones.
