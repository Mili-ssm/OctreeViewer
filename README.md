# Octree Viewer

Visor extensible de nubes LiDAR y nubes de puntos 3D, construido en Rust y ejecutable en el
navegador mediante WebAssembly. El proyecto está en una fase temprana: hoy contiene un núcleo de
voxelización en memoria, una importación LAS/LAZ incremental, un lector espacial por bloques y una
demo WebGPU 3D con worker, navegación y perfiles LOD. Aún no puede explorar datasets de cientos de GB
de extremo a extremo; la demo informa bytes únicos leídos y permite cancelar reiniciando el worker.

## Intención del proyecto

La meta es ofrecer una base de visualización que pueda explorar datasets LiDAR mayores que la
memoria disponible sin cargarlos completos. El visor debe leer progresivamente una estructura
espacial persistente, elegir solo los bloques que necesita la vista y mantener un presupuesto
acotado de memoria, transferencia y renderizado.

La ambición es superar a Potree en dimensiones concretas que podamos medir —por ejemplo tiempo hasta
la primera vista, latencia al navegar, memoria máxima, coste de conversión y facilidad para añadir
nuevos atributos, fuentes o renderizadores— conservando visualización web y una ruta WASM en Rust.
“Más potente y eficiente” es una dirección, no una afirmación demostrada: se tendrá que comparar
contra Potree con los mismos datasets, equipo, configuración y criterios de calidad visual.

Una capacidad requerida es **un formato de almacenamiento propio, versionado y optimizado para el
visor**, pensado para conversión, acceso espacial aleatorio y lectura progresiva de datasets LiDAR
de más de 500 GB. LAS/LAZ seguirán siendo formatos de entrada; el formato del proyecto será una
representación de trabajo optimizada para visualizar. La compatibilidad con COPC/EPT se evaluará
como interoperabilidad, sin sustituir el requisito del formato propio.

El objetivo de escala se refiere a explorar progresivamente una nube cuyo dataset fuente supere los
500 GB. No significa mantener todos sus puntos en RAM o dibujarlos simultáneamente. Antes de declarar
el soporte completo se deben fijar los atributos incluidos y el hardware y navegador que forman parte
del objetivo.

Para las mediciones del proyecto, **500 GB significa el tamaño del archivo fuente LAS/LAZ antes de
convertir, en bytes decimales (`500.000.000.000 B`)**. El tamaño del `.oct` convertido se registra por
separado y no cuenta para alcanzar ese umbral. Cada dataset de benchmark debe documentar ambos tamaños,
conteo de puntos, atributos, CRS y distribución espacial.

## Principios de arquitectura

- **Lectura fuera de memoria:** no retener el dataset completo en RAM ni copiar la escena completa a
  JavaScript.
- **Memoria y representación compactas:** optimizar el uso de memoria en todas las etapas, usando
  los tipos más pequeños que permitan los rangos y la precisión requeridos. Para almacenamiento y
  transferencia, cuantizar coordenadas a enteros cuando sea viable —con `u32` como punto de partida—
  y reconstruirlas a coordenadas locales `f32` solo para los bloques que se van a visualizar. La
  cuantización debe declarar y respetar un error máximo; no debe perderse precisión de forma
  silenciosa.
- **Acceso espacial:** indexar datos en bloques jerárquicos independientes que puedan leerse,
  descomprimirse y descartarse por separado.
- **Nivel de detalle:** seleccionar detalle según cámara, tamaño proyectado y presupuesto, no solo
  cantidad total de puntos.
- **Granularidad visual objetivo:** en condiciones normales, ajustar el LOD dentro de un rango
  orientativo que va de una densidad alta de aproximadamente 2 vóxeles por píxel a una representación
  gruesa donde cada vóxel ocupa alrededor de 16 × 16 píxeles proyectados. Este rango expresa a la vez
  la cantidad de representantes que puede llegar a dibujarse y el tamaño visual de cada celda; no es
  una densidad fija ni una garantía para toda cámara, viewport o dataset. La selección concreta debe
  seguir el error proyectado y los presupuestos de puntos/GPU, y validarse visualmente con datos reales.
- **Contratos pequeños:** separar geometría, almacenamiento, lectura, consulta espacial y
  presentación para poder extenderlos sin acoplar el núcleo a WASM o `wgpu`.
- **Métricas antes de promesas:** toda afirmación de escala o rendimiento debe venir acompañada de
  dataset, hardware, configuración, límites y mediciones reproducibles.
- **Conversión recuperable:** preparar el dataset puede tardar; la conversión debe informar progreso,
  permitir cancelación y evitar perder el trabajo ya completado tras un fallo.

### Flujo objetivo de datos

La preparación de un dataset de cientos de GB y su exploración son trabajos distintos. La conversión
se plantea inicialmente como herramienta Rust nativa con uso de memoria acotado; el navegador abre
el resultado propio y lee solo metadatos y bloques requeridos por la vista. No se considera requisito
convertir 500 GB dentro de WASM.

```text
LAS/LAZ ──► conversor Rust nativo ──► formato propio versionado
                                         │
                               Blob local o lectura remota
                                         │
                 viewer_wasm ──► viewer_core: selección/LOD
                                         │
                               viewer_render_wgpu
                                         │
                                  host web 3D
```

## Estado actual del repositorio

Este inventario describe el árbol de trabajo actual; comprobar el estado de Git antes de asociarlo a
una versión publicada.

| Área | Estado observado | Límite actual |
| --- | --- | --- |
| `viewer_core` | Tipos de puntos, jerarquía AABB/frustum, selección LOD de reemplazo/aditiva, scheduler de bloques, caché LRU acotada y transiciones de cortes disponibles. | Jerarquía, scheduler y caché residen en memoria; el scheduler no ejecuta I/O. El presupuesto contabiliza el payload estimado, no el RSS ni la sobrecarga del allocator/índice. `SparseVoxelStore` mantiene voxeles en un `HashMap` y su filtro incluye por centro. |
| `viewer_render_wgpu` | Conversión a instancias, cámara orbital, modos de color, buffers GPU por bloque, presupuesto configurable y frustum/error proyectado de cámara. | El límite GPU contabiliza buffers de instancias, no VRAM total. |
| `viewer_wasm` | Bindings para el visor, sesiones LAS/LAZ, presentación WebGPU y carga bajo demanda de bloques v1/v2 al corte LOD. | El host aún debe crear el worker, gestionar sus mensajes/estados y presentar la demo con datos propios. |
| `examples/web-host` | Demo WebGPU en worker, con muestra sintética, apertura de formato local, progreso por rangos, cancelación y perfiles LOD. | La cancelación termina y recrea el worker; no se ha validado visualmente con GPU en navegador. |
| Formato propio | Especificaciones v1/v2, cuantizador `u32`, codec LZ4 por bloque, CRC32, escritor incremental y lector aleatorio con límites configurables ([`docs/FORMAT_V1.md`](docs/FORMAT_V1.md), [`docs/FORMAT_V2.md`](docs/FORMAT_V2.md)). v1 significa jerarquía de reemplazo; v2 añade la semántica aditiva y conserva lectura v1. | Fixtures prueban round-trip, bloques selectivos y una lectura ejecutada desde un ticket del scheduler, corrupción, límites, ambas semánticas y compatibilidad. XYZ `u32` conserva 12 bytes por punto, igual que `f32`; el ahorro aplica frente a `f64`. |
| Pruebas y rendimiento | Hay tests pequeños de núcleo e importación. | No demuestran consumo de memoria o navegación con datasets grandes ni comparan con Potree. |
| `viewer_convert` | CLI nativa LAS/LAZ → v1 con particiones LOD 2/4/8 y reanudación por fases; también acepta directorios EPT locales `laszip` y conserva su topología aditiva en v2. | El índice y los nodos viven en RAM con estimación de presupuesto. EPT remoto, gzip de jerarquía y codecs `binary`/`zstandard` no están soportados. La comparación numérica de cortes iguales está registrada; falta evaluación visual. |

## Qué significará “feature complete”

El proyecto se podrá considerar completo para su objetivo inicial cuando se pueda:

1. Convertir una fuente LAS/LAZ LiDAR de más de 500 GB al formato propio, con progreso, cancelación,
   recuperación y errores comprensibles.
2. Abrir el resultado sin cargar todos sus puntos y navegarlo desde un navegador compatible mediante
   WASM y renderizado 3D GPU.
3. Cargar solo los bloques requeridos por la vista, cambiar de nivel de detalle y mantener los límites
   acordados de RAM, GPU y tiempo de respuesta.
4. Recuperar metadatos y atributos definidos por el formato, y permitir añadir al menos una nueva
   fuente de datos o modo de color sin reescribir el núcleo ni la interfaz de render.
5. Repetir y publicar los benchmarks, pruebas de integridad del formato y comprobaciones visuales que
   respaldan las afirmaciones de compatibilidad y rendimiento.

La selección de clases, medición, recorte, edición, anotación, exportación, autenticación y trabajo
colaborativo no se consideran alcance inicial por defecto. Se pueden añadir si se convierten en
requisitos explícitos.

## Plan de trabajo

Las tareas están ordenadas por dependencias. Los nombres de crates futuros son propuestas de
organización; pueden cambiar durante la implementación si la separación real del código lo justifica.

### Fase 0 — Contrato de escala y comparación

**Característica:** fijar qué significa soporte de más de 500 GB y cómo se comprobará la ambición
frente a Potree.

- [x] Definir los 500 GB como tamaño del archivo fuente LAS/LAZ antes de convertir, en bytes decimales;
  registrar el tamaño `.oct` por separado. El registro existente de Belvedere incluye ambos tamaños,
  puntos, atributos, CRS y distribución; ampliar la ficha al añadir datasets de benchmark.
- [x] Elegir Chrome y Firefox con WebGPU como navegadores objetivo; el host necesita también
  `OffscreenCanvas` para ejecutar el visor en el worker.
- [x] Registrar la máquina local disponible como referencia de benchmark, sin tratarla como mínimo de
  soporte ([perfil observado](docs/REAL_DATASET_BENCH.md#máquina-de-referencia-local)).
- [ ] Definir sistemas operativos y mínimos de RAM/GPU; decidir si la primera versión admite solo
  archivos locales o también lectura remota por rangos.
- [x] Elegir métricas: tiempo de conversión, apertura y primera vista útil; memoria RAM/GPU; bytes
  solicitados, leídos y subidos; latencia de navegación y frame p50/p95/p99; calidad visual y
  estabilidad. Las mediciones de cada fase indicarán qué métricas puede observar realmente su entorno.
- [ ] Crear una comparación repetible con Potree y datos equivalentes. Registrar versiones, comandos,
  opciones, hardware y resultados; no comparar cifras de entornos distintos.
- [ ] Establecer umbrales de aceptación para cada métrica después de una primera medición de referencia.

**Cierre:** existe un documento de benchmark versionado con datasets reproducibles, hardware y
umbrales acordados. Hasta entonces, “más eficiente que Potree” sigue siendo una meta sin verificar.

### Fase 1 — Modelo espacial y contratos comunes (`viewer_core`)

**Característica:** representar datasets y consultas sin depender de una estructura residente
completa en memoria.

- [x] Revisar el modelo de coordenadas: conservar coordenadas enteras/escala/desplazamiento o una
  precisión equivalente antes de convertir a coordenadas locales de renderizado.
- [x] Definir la semántica base de AABB: validar coordenadas finitas y ordenadas; contención e
  intersección incluyen los bordes, y las cajas inválidas no coinciden en consultas.
- [x] Definir el contrato base del nodo espacial (ID opaco, padre, nivel, límites, conteo del payload,
  error geométrico, referencias a bloques), esquema de atributos y estadísticas sin sumar muestras
  entre niveles. `SpatialHierarchy` valida la estructura y deriva el orden de hijos.
- [x] Definir la unidad del error geométrico: distancia máxima conservadora en las mismas unidades
  que los límites del dataset, calculada a partir de los datos fuente.
- [x] Definir el muestreo LOD: voxel cúbico alineado al origen, representante en el centro y color
  agregado; el error conservador es la semidiagonal más el margen de redondeo del centro a `f32`.
- [x] Hacer explícita la tolerancia en píxeles como parámetro obligatorio de cada selección LOD; no
  hay un valor global por defecto porque depende de cámara y viewport.
- [x] Añadir consulta conservadora por frustum sobre límites de nodos, con poda de subárboles.
- [x] Seleccionar nodos por nivel de detalle y presupuesto; mantener AABB como consulta simple útil
  para recorte.
- [x] Separar voxelización/agregación del camino de puntos: `PointBatch` se puede renderizar sin
  reducción; la voxelización sigue disponible como modo explícito de reducción de densidad.
- [x] Definir límites y errores numéricos: coordenadas y límites finitos/ordenados, conversión local
  fuera de rango y sumas de conteos con overflow explícito.
- [x] Tratar truncamiento LAS/LAZ como error terminal de la sesión de importación; cuando falta RGB,
  usar intensidad en escala de grises como fallback documentado.
- [x] Mantener conteos de fuente y payload en `u64`, con cobertura de valores mayores que `u32`.
- [x] Diseñar contratos pequeños para leer fuentes, solicitar bloques y proporcionar atributos. Evitar
  hacer que `viewer_core` dependa de WASM, DOM o tipos de `wgpu`.

**Progreso:** el índice residente valida raíces, padres, niveles y contención de límites; las consultas
AABB y frustum recorren en profundidad de forma determinista y podan subárboles disjuntos, sin
materializar puntos. La consulta frustum es conservadora y devuelve candidatos. La selección LOD por
error proyectado y presupuesto está implementada sobre metadatos; la jerarquía aún no carga ni cancela
bloques desde almacenamiento y el presupuesto todavía cuenta puntos representantes, no memoria.

El origen del dataset se conserva en `f64` y `CoordinateFrame` resta ese origen antes de convertir
posiciones a `f32` local para renderizado; la importación LAS usa el mismo contrato y las pruebas
cubren coordenadas mundiales grandes con desplazamientos pequeños. La precisión final local sigue
limitada por `f32` y por la extensión del dataset. `SpatialNode::geometric_error` se define como la
distancia máxima conservadora entre puntos fuente representados y sus representantes renderizados,
en las mismas unidades que los límites del dataset. La política base de muestreo usa una grilla de
voxeles cúbicos alineada al origen; cada voxel representa sus puntos con el centro de la celda y el
color agregado, y su error máximo queda acotado por la semidiagonal más el redondeo del centro a
`f32`. Los niveles pueden duplicar el
tamaño de celda para mantener grillas anidadas. El conversor deberá aplicar la política y almacenar
el máximo de esas cotas por nodo. El consumidor debe proporcionar la tolerancia en píxeles para cada
vista; `select_lod` no inventa un umbral global.

`DatasetSource` expone metadatos y lee bloques identificados por `BlockId` bajo demanda, sin acoplar
el núcleo a archivos, red, WASM ni `wgpu`. Cada `PointBlock` lleva un lote de puntos y columnas de
atributos tipadas; las columnas comprueban el tipo y el número de componentes por punto. La lectura
es síncrona en este contrato base, por lo que el consumidor decide si la ejecuta en un worker o hilo.
El formato propio aún debe implementar este contrato y validar que los bloques coinciden con el
esquema y la jerarquía.

`SpatialHierarchy::select_lod` proyecta el error mediante una función suministrada por el consumidor
y refina por mayor error mientras el cambio que dicta la semántica de la jerarquía quepa en el
presupuesto. En v1, los hijos reemplazan al padre; en v2 aditivo, se retienen ancestros y se añaden
hijos. El productor define la cobertura/error, y el selector cuenta puntos, no bytes o memoria. Si la
selección base supera el presupuesto devuelve un error; si una mejora no cabe o no hay más
resolución, lo informa.

`ViewerController` y `WasmVoxelViewer` ahora renderizan puntos sin agregarlos por defecto. La ruta
opcional `replaceVoxelizedPoints`/`appendVoxelizedPoints` conserva la reducción de densidad. Ambos
caminos siguen almacenando la escena actual en memoria; la lectura de bloques persistentes y acotados
pertenece a `viewer_format` y a las fases posteriores.

La importación LAS/LAZ valida límites completos del encabezado antes de aceptar el origen. Un error
de lectura, incluido un archivo truncado, invalida la sesión para evitar continuar desde un cursor
parcial; los conteos espaciales usan `u64` y la selección detecta desbordamiento al sumarlos. La
importación actual conserva RGB cuando existe y, si no, representa intensidad como gris; otras
dimensiones LAS todavía no se exponen como atributos del visor.

**Cierre:** las consultas espaciales devuelven nodos y metadatos sin materializar todos los puntos;
los tests validan límites, precisión, orden LOD y presupuestos.

### Fase 2 — Formato de archivo propio (`viewer_format`, crate propuesto)

**Característica:** persistir una nube organizada en bloques indexados, legibles aleatoriamente y
compatibles entre versiones.

- [x] Añadir v2 aditivo para jerarquías cuyos nodos contienen payloads distintos; el lector conserva
  compatibilidad con v1 y lo interpreta como reemplazo ([`docs/FORMAT_V2.md`](docs/FORMAT_V2.md)).

- [x] Publicar una especificación mínima: magic/versionado, endianness, metadatos, CRS y transformación
  de coordenadas, esquema de atributos, jerarquía, directorio de bloques, offsets/tamaños, compresión y
  reglas de alineación.
- [x] Definir representación de cada nodo: límites espaciales, nivel, conteos, rango de atributos,
  política de muestreo y bloque(s) asociados.
- [x] Diseñar y medir la representación compacta de coordenadas y atributos: definir el origen,
  escala y precisión de cuantización por dataset o bloque, comenzar evaluando coordenadas `u32` y
  reconstruirlas a `f32` local al preparar los datos para renderizado. El error de cuantización debe
  quedar acotado y reflejarse en las cotas de error geométrico.
- [x] Medir un candidato `u16` afín por bloque sobre Belvedere y reservar fallback `u32` por bloque
  cuando exceda el umbral candidato de 0,01. El error incremental máximo fue 0,0349; 19 de 1.343
  bloques requirieron fallback y el ahorro teórico de coordenadas sin comprimir fue 490.227.702 B.
  El ensayo parte de la salida `.oct` actual, no de LAS original, y no mide el efecto de compresión;
  por eso no cambia el formato ni demuestra el error total ([informe](docs/COORDINATE_WIDTH_BENCH.md)).
- [ ] Evaluar coordenadas menores que `u32` relativas al nodo o voxel, reconstruyendo posiciones a
  través de la jerarquía. Repetir con LAS original y varias distribuciones; adoptar solo si se puede
  garantizar el límite total de error y medir la reducción real del payload comprimido.
- [x] Elegir compresión por bloque independiente para poder leer y descartar datos sin descomprimir el
  archivo entero; medir ratio, coste de CPU y coste de lectura.
- [x] Incluir detección de truncamiento/corrupción y mecanismo de integridad por bloque o índice.
- [x] Implementar lector con acceso aleatorio, lectura parcial y validación explícita de versiones y
  tamaños antes de reservar memoria.
- [x] Implementar escritor que genere archivos deterministas cuando sea posible, cierre correctamente
  el índice y no publique como válido un resultado parcial.
- [x] Añadir fixtures pequeños, grandes sintéticos, archivos malformados y pruebas de round-trip,
  lectura selectiva, corrupción, límites de tamaño y compatibilidad de versiones.
- [x] Documentar política de compatibilidad: qué versiones se leen, cómo se migra, qué campos pueden
  ampliarse y cómo se preservan atributos desconocidos.

**Cierre:** el lector recupera un bloque seleccionado sin leer el archivo completo; los datos
conservan coordenadas y atributos tras escribir/leer y las corrupciones fallan con errores claros.

### Fase 3 — Conversor de datasets grandes (`viewer_convert`)

**Característica:** crear el formato propio desde LAS/LAZ sin almacenar toda la nube en RAM.

- [x] Implementar primero un ejecutable nativo de conversión; no exigir a la aplicación WASM leer y
  convertir la fuente completa antes de mostrar la nube.
- [x] Crear un pipeline por etapas: inspección de metadatos, lectura, construcción del índice/LOD,
  codificación y escritura de bloques.
- [x] Imponer límites de puntos/buffers por bloque y un presupuesto para la memoria controlada por
  bloque e índice; reportar la estimación antes de publicar.
- [x] Medir tiempo de conversión por fase, puntos/s, RSS pico, ratio de compresión y lectura de
  bloques con un dataset real de 81,7 millones de puntos; resultados y comandos reproducibles en
  [REAL_DATASET_BENCH.md](docs/REAL_DATASET_BENCH.md).
- [x] Medir el pico de espacio temporal de disco con un dataset real; registrar método, resolución
  del muestreo y límites en [REAL_DATASET_BENCH.md](docs/REAL_DATASET_BENCH.md).
- [x] Repetir la línea de base serial para decidir dónde evaluar paralelismo; registrar tiempo,
  RSS y fases en [REAL_DATASET_BENCH.md](docs/REAL_DATASET_BENCH.md).
- [x] Probar paralelismo acotado en subárboles de la raíz con 2 y 4 trabajadores. Las salidas fueron
  idénticas byte a byte y la fase de jerarquía bajó aproximadamente 30 % y 39 %; el pico temporal
  observado se mantuvo en 8,34 GiB. Mantener 1 trabajador por defecto y registrar límites y muestras
  en [REAL_DATASET_BENCH.md](docs/REAL_DATASET_BENCH.md).
- [x] Medir el coste de memoria de las representaciones e intermedios en un dataset real y eliminar
  las copias de atributos y puntos del spool que no aportaban al formato; registrar mediciones y
  límites en [REAL_DATASET_BENCH.md](docs/REAL_DATASET_BENCH.md). Mantener la estimación conservadora
  hasta repetir el benchmark para confirmar su margen.
- [x] Informar fases, puntos procesados, bytes escritos y errores en la API/CLI.
- [x] Permitir cancelación cooperativa desde el callback entre bloques y durante validación; limpiar
  el temporal y no publicar una salida parcial.
- [x] Conectar la cancelación de la CLI a Ctrl+C con manejo portable de señales.
- [x] Permitir reconstrucción segura desde la fuente tras cancelación; validar el resultado antes de
  publicar.
- [x] Añadir checkpoints durables por fases: conservar el spool de fuente completo y el candidato
  `.oct` completo; tras una interrupción de jerarquía/escritura se repite esa fase desde el spool,
  y tras una interrupción de validación/publicación se reutiliza el candidato. No se reanudan nodos
  ni bloques individuales.
- [x] Comparar particiones 2×2×2, 4×4×4 y 8×8×8 con nubes sintéticas, resoluciones equivalentes,
  compresión LZ4 y consultas ancladas en puntos reales; registrar metodología, métricas y límites en
  [SPATIAL_BRANCHING_BENCH.md](docs/SPATIAL_BRANCHING_BENCH.md).
- [x] Implementar octree adaptativo 2×2×2 con hojas a resolución completa y muestras LOD en grillas
  8×8×8 de voxeles ocupados; agrega color por voxel, toma los demás atributos del punto fuente más
  cercano y declara error geométrico conservador.
- [x] Medir sesgo, error geométrico observado, compresión de payloads y saltos entre niveles sobre
  un COPC LAZ de 81,7 millones de puntos y dos teselas LAS USGS; la herramienta y los límites de la
  comparación de jerarquías equivalentes están en
  [SPATIAL_BRANCHING_BENCH.md](docs/SPATIAL_BRANCHING_BENCH.md).
- [x] Añadir particiones adaptativas directas 2×2×2, 4×4×4 y 8×8×8 al conversor; registrar tamaño,
  bloques, tiempo y error sobre el COPC Belvedere real. El benchmark separa estas ejecuciones del
  estudio anterior, que conservaba una misma partición de hojas.
- [x] Comparar cortes directos con presupuestos iguales de 1 M y 4 M de puntos sobre Belvedere;
  registrar presupuesto usado, error observado y límites de la selección en
  [SPATIAL_BRANCHING_BENCH.md](docs/SPATIAL_BRANCHING_BENCH.md).
- [ ] Revisar visualmente los cortes directos en navegador antes de cambiar el valor predeterminado,
  que sigue en 2×2×2.
- [x] Evitar pérdida silenciosa de dimensiones LAS/LAZ: preservar atributos soportados y reportar de
  forma explícita los que se descartan o transforman.
- [x] Aceptar LAS/LAZ y COPC como entrada secuencial interoperable; el fixture COPC de 107 puntos de
  `las 0.11.1` se convierte correctamente. COPC se aplana en una raíz y no conserva su jerarquía;
  consultar la [especificación COPC](https://copc.io/) para conservar acceso espacial en una versión
  futura.
- [x] Evaluar EPT: es un dataset de varios archivos (`ept.json`, jerarquía y datos por nodo), no una
  ruta LAS/LAZ única ([especificación EPT](https://entwine.io/en/latest/entwine-point-tile.html)).
- [x] Implementar entrada para datasets EPT locales `laszip`: leer `ept.json`, páginas JSON y LAZ
  por nodo, validar conteos y mantener la jerarquía aditiva en v2. Remoto y otros codecs quedan fuera
  de esta iteración.

Uso inicial: `cargo run -p viewer_convert -- INPUT.las|INPUT.laz|INPUT.copc.laz|EPT_DIRECTORY|EPT_DIRECTORY/ept.json OUTPUT.oct [--points-per-block N] [--max-error E] [--max-memory-mib MIB] [--spatial-divisions 2|4|8] [--spatial-workers N] [--checkpoint | --resume]`.
Los valores por defecto son 262.144 puntos por bloque y error máximo de cuantización 0,01 en unidades
de la fuente, con un presupuesto estimado de memoria controlada de 512 MiB. La estimación incluye
192 bytes por punto del bloque, 512 bytes por bloque del índice, 256 bytes por nodo, espacio para una
grilla LOD de 512 celdas, buffers de spool de 8 KiB por hijo posible (64 KiB, 512 KiB o 4 MiB para
2, 4 u 8 divisiones), 16 MiB fijos y 1 MiB por trabajador espacial adicional; no equivale al RSS
total ni incluye el allocator o la biblioteca LAS/LAZ. `--spatial-divisions` elige 2, 4 u 8 divisiones
por eje y conserva 2 por defecto. `--spatial-workers` permite 1–4 trabajadores,
con valor por defecto 1; el modo
paralelo solo distribuye los subárboles directos de la raíz y reúne resultados en orden para mantener
IDs y bytes estables. Usa un canal de progreso acotado y reserva memoria por trabajador. El número de
trabajadores puede cambiar al reanudar un checkpoint; `--spatial-divisions` debe conservarse.
La construcción usa spools temporales en disco
que pueden ocupar más que la fuente;
en modo normal se limpian al completar o cancelar cooperativamente, aunque un cierre abrupto puede
dejarlos. Con `--checkpoint`, el conversor crea `OUTPUT.oct.checkpoint`; vuelve a ejecutar con
`--resume`, la misma entrada y las mismas opciones para continuar. Un checkpoint incompatible o una
salida existente distinta se rechazan sin sobrescribirla. Al completar se elimina el directorio del
checkpoint. El spool y el candidato completo pueden coexistir, así que esta modalidad requiere más
espacio temporal que la conversión normal; no se ha medido aún su pico con el dataset de 81,7 millones
de puntos.

La identidad de fuente se comprueba leyendo todos sus bytes con una huella FNV-1a de 128 bits. Es una
comprobación local de cambios accidentales, no una firma criptográfica. El manifiesto y los archivos
se sincronizan antes de avanzar de fase; la sincronización de directorios para resistencia a pérdida
de alimentación está implementada en Unix y no se promete con el mismo alcance en otras plataformas.

La planificación crea una jerarquía espacial adaptativa con 2, 4 u 8 divisiones por eje: las hojas
conservan todos los puntos fuente y cada nodo interno almacena hasta 512 representantes en centros de
voxel. El color y RGB16 se promedian por voxel; intensidad y demás atributos proceden del punto fuente más cercano. El error declarado incluye
la semidiagonal del voxel, redondeo local a `f32` y cuantización del bloque. El total de bloques incluye
tanto hojas como muestras LOD. La salida se publica mediante un enlace al archivo temporal validado y
falla si el destino ya existe. Se conservan intensidad, RGB, clasificación, retornos, ángulo de escaneo
en grados `f32`, user data, source ID y GPS/NIR cuando están disponibles; se avisa de flags, waveform,
Extra Bytes y CRS VLR que no se conservan.

Para EPT, la entrada es el directorio del dataset o su `ept.json`. La primera versión acepta solo
`dataType: laszip` y `hierarchyType: json` locales; valida páginas, padres, conteos declarados y
cabeceras LAZ antes de publicar. Mantiene payloads propios por nodo y escribe v2 aditivo; no vuelve
a particionar la jerarquía. `--checkpoint`/`--resume`, `--spatial-divisions` distintas de 2 y
`--spatial-workers` distintos de 1 no se aplican a EPT. El error geométrico EPT almacenado es la
diagonal de la celda del nodo y sirve como estimación de resolución espacial.

La CLI muestra las fases y progreso de lectura, planificación espacial, escritura y validación;
`convert_file_with_progress` ofrece los mismos eventos y permite cancelar devolviendo `false`. Ctrl+C
solicita cancelación y espera al siguiente límite seguro. En modo checkpoint se conserva el último
límite completo; la validación vuelve a comprobar el candidato antes de publicar. La comprobación de
identidad vuelve a leer la fuente, pero reanudar desde el spool evita volver a decodificar sus puntos.

#### Registro de rendimiento

Esta tabla conserva la referencia para comparar cambios en el conversor y el lector. No reemplazar
filas anteriores: añadir una fila por versión o cambio relevante, indicando revisión y condiciones.
Los resultados detallados, métricas por fase y estimaciones de memoria están en
[REAL_DATASET_BENCH.md](docs/REAL_DATASET_BENCH.md).

| Fecha / revisión | Dataset y configuración | Conversión | Salida / ratio LAZ | Ratio payload LZ4 | RSS conversión | Pico disco temporal | Lectura de todos los bloques | RSS lectura |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2026-10-09 / `3761ed3` | Belvedere Glacier 2009; 81.682.329 puntos; release; opciones por defecto | 100,12 s; 815.872 pt/s | 1.596.356.674 B; 2,581× | 0,4765 (52,36 % menos) | 37.612 KiB | 8.980.927.112 B (8,37 GiB; muestreo 100 ms) | 9,81 s; índice 2,87 ms | 27.024 KiB |
| 2026-10-09 / escritura directa tras `3761ed3` | Mismo dataset y opciones; una ejecución | 101,16 s; 807.468 pt/s | 1.596.356.674 B; 2,581× | 0,4765 (52,36 % menos) | 36.068 KiB | — | — | — |

La referencia de rendimiento se midió en Linux x86_64, AMD Ryzen AI 9 365 (20 hilos lógicos), Rust
1.99.0. El pico de disco temporal se midió en una conversión adicional de la misma revisión,
configuración y equipo. Son ejecuciones iniciales, con la lectura realizada después de convertir en
el mismo equipo; no representan una lectura con caché fría ni una garantía de rendimiento. Para
comparar un cambio, usa el mismo LAZ,
equipo, opciones y comandos; ejecuta tres veces la versión de referencia y el cambio, y compara las
medianas de conversión, lectura y RSS. Registra también la revisión (`git rev-parse --short HEAD`),
fecha, configuración y cualquier diferencia del entorno. No cambies la referencia aceptada hasta
confirmar una mejora o regresión repetible.

Para repetir la medición, descarga primero el LAZ según [`data/datasets/README.md`](data/datasets/README.md)
y compila el benchmark en release. El conversor no sobrescribe su destino: usa un `.oct` nuevo por
ejecución o elimina el resultado previo.

```sh
cargo build --release -p viewer_convert --example real_dataset_bench
/usr/bin/time -v -o /tmp/belv-convert.time \
  target/release/examples/real_dataset_bench convert \
  data/datasets/belv_2009_digital-aerial_pcd_copc.laz \
  data/datasets/belv_2009_digital-aerial_pcd_copc.oct \
  > /tmp/belv-convert.log 2>&1
/usr/bin/time -v -o /tmp/belv-read.time \
  target/release/examples/real_dataset_bench read \
  data/datasets/belv_2009_digital-aerial_pcd_copc.oct \
  data/datasets/belv_2009_digital-aerial_pcd_copc.laz \
  > /tmp/belv-read.log 2>&1
```

El benchmark informa conversión por fase, puntos/s, tamaños, compresión LZ4 y memoria estimada; el
RSS pico se obtiene de los archivos `time`. El tiempo de lectura incluye I/O, CRC, descompresión y
reconstrucción de todos los bloques. Los GB/s del archivo, compresión interna y ratio `.oct`/LAZ son
métricas distintas y deben registrarse por separado. Las estimaciones de RAM/GPU para render no son
mediciones del renderer: aún no presenta una escena.

**Cierre:** una conversión de dataset de escala acordada termina con memoria acotada, puede cancelarse
y reanudarse, y produce un archivo validado y abierto por el lector del formato.

### Fase 4 — Lectura fuera de memoria y selección (`viewer_core` + `viewer_format`)

**Característica:** pasar de cámara/consulta a bloques concretos, priorizados y acotados.

- [x] Implementar recorrido jerárquico con poda por frustum y selección de LOD según error/tamaño
  proyectado o política equivalente documentada.
- [x] Crear planificador de solicitudes con prioridad por visibilidad y distancia, límites de trabajos
  en curso y cancelación lógica de bloques que dejan de ser relevantes.
- [x] Añadir caché acotada de bloques descomprimidos con política LRU, presupuesto de bytes estimados y
  estadísticas observables.
- [x] Gestionar transiciones LOD por subárbol con cambio atómico, fallbacks disponibles y diagnóstico
  de regiones sin cobertura; especificar el comportamiento en los bordes de bloques.
- [x] Hacer que la consulta solo devuelva bloques dentro del presupuesto y exponer los motivos de
  omisión/atraso para diagnóstico.

**Cierre:** al cambiar la cámara, se solicitan/cancelan los bloques esperados; la memoria de caché no
supera el límite configurado y la selección mantiene cobertura visual aceptable.

**Progreso:** `SpatialHierarchy` ya recorre candidatos con poda de frustum y selecciona LOD para
jerarquías de reemplazo y aditivas. `BlockRequestScheduler` convierte ese corte en solicitudes de
bloques deduplicadas, ordenadas por distancia a la AABB más cercana, con un máximo de lecturas activas.
El consumidor ejecuta los tickets; al cambiar la vista se quitan pendientes obsoletos y se marcan
activas para descarte al completar. Una lectura síncrona no se interrumpe y conserva su plaza hasta
finalizar. `DecodedBlockCache` conserva bloques descomprimidos bajo un presupuesto de bytes estimados,
expulsa por LRU y expone hits, misses, inserciones, expulsiones y rechazos por tamaño o protección.
La estimación incluye capacidades de payload e información inline del bloque; excluye allocator, índice y memoria
del proceso. `LodTransitionPlanner` conserva el corte visible: en jerarquías de reemplazo mantiene
el padre hasta que todos los bloques del corte hijo estén disponibles, y cambia cada subárbol de
forma atómica; al reducir detalle conserva los hijos hasta que llega el padre y señala cobertura no
demostrada si cambia la región visible mientras falta ese padre. En jerarquías aditivas
mantiene los ancestros y agrega cada payload hijo cuando todos sus bloques están disponibles. Las
ramas independientes pueden avanzar por separado. Protege el corte activo frente a expulsiones; si
el presupuesto no permite cargar el nuevo corte, se rechaza la inserción y se conserva el actual.
El plan expone bloques pendientes, fallbacks y nodos sin representación residente. Las AABB se
consultan de forma inclusiva y conservadora; el lector no vuelve a recortar puntos por los límites
de bloque. El conversor asigna planos internos a la celda superior y preserva el máximo exterior,
pero el núcleo no elimina duplicados de datasets arbitrarios. Al procesar `CacheInsertResult`, el
consumidor debe pasar sus bloques expulsados a `BlockRequestScheduler::invalidate_completed` para
permitir recargarlos; para bloques seleccionados que ya estén en caché, debe llamar `mark_cached` y
evitar lecturas duplicadas. `LodSelection` conserva y expone el presupuesto configurado, además de
los nodos limitados por puntos y por resolución. `LodTransitionPlan` vuelve a comprobar el límite con
los puntos reales de los bloques residentes, deduplicados por `BlockId`; admite cada nodo completo
en orden ascendente de `NodeId` o lo omite completo, y expone puntos devueltos, presupuesto, nodos
omitidos, bloques pendientes, fallbacks y cobertura no disponible. Una omisión por presupuesto no se
cuenta como lectura pendiente. Para no encolar bloques de nodos omitidos, el consumidor debe pasar
`pending_blocks()` a `BlockRequestScheduler::update_blocks` con sus prioridades. El scheduler expone
el estado de cada bloque (no seleccionado, cola, activo, cancelado aún activo, entregado o fallido) y
contadores de atraso. Si la selección base de la
jerarquía excede el presupuesto, `select_lod` devuelve `BaseCutExceedsBudget`; el consumidor debe
reportar la consulta fallida y no presentar el corte anterior como si respetara el límite nuevo.
Los fallos admiten reintento explícito. No hay executor multihilo ni integración con cámara/WASM
todavía.

### Fase 5 — Presentación 3D (`viewer_render_wgpu`)

**Característica:** transformar bloques seleccionados en una escena 3D visible y eficiente.

- [x] Inicializar adaptador compatible, dispositivo, formato, pipeline, geometría y recursos GPU para
  una `Surface` creada por el host; presentar escenas instanciadas con profundidad. `SurfaceRenderer`
  devuelve errores de inicialización, suspende frames con tamaño cero y reconfigura ante superficies
  `Lost`/`Outdated`. Quien integra `SurfaceRenderer` directamente crea el `Instance` y la `Surface`;
  la factoría WASM recibe el canvas y los crea internamente. El host reintenta después de una
  reconfiguración y recrea el visor si debe reemplazar la superficie o el dispositivo.
- [x] Conectar el host WASM y el canvas a `SurfaceRenderer`: el host mantiene el canvas, gestiona resize
  y reintento/recreación; Rust crea y presenta en la `Surface` WebGPU.
- [x] Implementar cámara orbital con encuadre explícito de escena/dataset, orbit, pan, zoom y resize.
  Usa Z como eje vertical, calcula bounds y cámara en `f64` y resta un origen local en el shader antes de
  proyectar. En cortes LOD encuadra los bounds del dataset; en la escena completa encuadra instancias.
  Preserva la precisión local disponible; no recupera precisión perdida al convertir coordenadas
  globales a `f32`.
- [x] Añadir `SurfaceRenderer::sync_visible_blocks`: recibe IDs de un corte renderizable y la caché
  CPU, deduplica, conserva buffers compartidos, sube solo bloques nuevos y libera los retirados.
  Repetir el mismo corte no escribe instancias; datos inválidos o ausentes preservan el corte anterior.
  Los IDs son locales al dataset: hay que llamar `clear_dataset` al cambiarlo. Los bloques se tratan
  como inmutables mientras residan en GPU.
- [x] Conectar `LodTransitionPlan` y `DecodedBlockCache` del visor/WASM a esta ruta. `openLodDataset`,
  `insertLodBlock` y `updateLod` admiten metadatos y bloques RGB decodificados; la selección usa
  frustum y error proyectado de cámara. Se prepara el corte CPU, se valida/sube atómicamente a GPU y
  solo después se confirma la protección de caché. Si el corte base excede el presupuesto estricto,
  se vacía el corte anterior y se devuelve el diagnóstico; un rechazo GPU conserva el corte y plan
  previos. La lectura bajo demanda del formato está conectada en WASM; el host de referencia la ejecuta
  en un worker.
- [x] Añadir modos de color RGB, intensidad, elevación y clasificación cuando existe el atributo.
  Intensidad ausente usa luminancia RGB en escala de grises; clasificación ausente conserva RGB.
  El multiplicador de tamaño de las instancias cúbicas admite valores de 0.1 a 10.
- [x] Definir un presupuesto ajustable en `RendererConfig::gpu_budget`: por defecto 4 millones de
  instancias visibles y 256 MiB de capacidad agregada para buffers de instancias. Cada buffer también
  respeta `max_buffer_size` del dispositivo y el límite `u32` de cada draw. La capacidad se redondea
  a potencias de dos y el chequeo de cambios atómicos incluye el pico temporal de buffers antiguos y
  nuevos. El límite solo estima buffers de instancias del renderer; no mide ni garantiza VRAM libre.
  Si se excede, devuelve un error tipado con requerido/límite y conserva la selección anterior; no
  reduce LOD por su cuenta.
- [x] Exponer instancias/bloques visibles y bytes acumulados de instancia en `gpuStats()`. La demo
  muestra además el tiempo síncrono de la llamada JS/WASM a `renderFrame`; es una medida CPU y no
  representa duración de ejecución GPU ni latencia de presentación. No se solicitan GPU timestamps.
  El shader sigue siendo un punto de partida, no un renderer completo.

**Cierre:** el host presenta una nube real en 3D, permite navegarla y el número de bloques y bytes
subidos corresponde a la selección visible, no al tamaño total del dataset.

**Estado actual:** `SurfaceRenderer` presenta escenas con cámara orbital, modos de color y tamaño
configurable y presupuesto estricto de instancias/buffers; conserva buffers por bloque para cortes
LOD. El host WASM expone instancias/bloques visibles y bytes subidos; la demo muestra el tiempo de
llamada CPU al renderer. El puente LOD acepta coordenadas locales `f32` y RGB por bloque; `updateLod`
aplica el perfil y la selección se recalcula antes del siguiente `renderFrame` después de cambios de
cámara o llegada de bloques. Los atributos de intensidad/clasificación aún no se transfieren por este
puente. No se ha verificado presentación visual en un navegador con GPU.

### Fase 6 — Orquestación WASM (`viewer_wasm`)

**Característica:** exponer una API estable y asíncrona al host para abrir datasets, gestionar lecturas
y presentar estado.

- [x] Conectar `FormatReader` v1/v2 al puente LOD: abre el índice, selecciona desde cámara y lee los
  bloques faltantes del corte antes de publicarlo. `openFormatDataset` usa `FileReaderSync`, por lo que
  el host debe ejecutar el visor en un worker; `createOffscreen` permite presentar en un canvas
  transferido al worker. Si falla una lectura, conserva el corte actual y devuelve el error.
- [x] Ejecutar lectura, decodificación, selección y render en un worker con `OffscreenCanvas`; se
  cargan solo los bloques del corte visible y la UI principal conserva controles responsivos.
- [x] Aplicar contrapresión al protocolo de la demo: resize, color, tamaño, perfil y encuadre conservan
  como máximo un comando en vuelo y el último valor pendiente por tipo. Los frames siguen limitados a
  uno en vuelo. La lectura de bloques permanece síncrona dentro del worker; cancelar termina/recrea el
  worker y descarta el dataset abierto.
- [x] Limitar transferencias WASM: `replacePoints`/`appendPoints` y `insertLodBlock` convierten
  arrays tipados en lotes temporales de 16.384 puntos, con un máximo de puntos de escena tomado del
  presupuesto GPU y límite de bloque tomado de `cacheBytes`. Rust conserva los puntos al terminar;
  el host retiene los arrays durante la llamada. `readNext` limita cada respuesta a 65.536 puntos y
  8 MiB de registros fuente. `renderSceneBatch` limita la salida a 16.384 instancias; `renderScene`
  rechaza escenas mayores. No se comparte memoria zero-copy.
- [x] Soportar archivos locales del formato propio en la demo. Definir por separado si el producto
  debe aceptar URL/range requests, porque CORS, autenticación y consistencia de rangos requieren
  contrato específico.
- [x] Exponer apertura de metadatos, inserción de bloques RGB decodificados, actualización LOD,
  encuadre y movimiento de cámara. Los IDs y conteos `u64` se intercambian como strings para evitar
  pérdida de precisión en JavaScript.
- [x] Añadir perfiles `performance` y `quality`: error proyectado de 2 px y 1/16 px, con presupuestos
  de puntos por defecto de 1 millón y 4 millones. El host puede pasar otro presupuesto; ambos perfiles
  siguen limitados por el presupuesto GPU estricto del renderer. Son una aproximación al rango
  acordado de 2:1 a 1:16 voxeles/píxel, no una garantía de densidad exacta. Falta validar el efecto
  visual con datasets reales.
- [x] Integrar selección de archivo local, mensajes de carga/error y resumen de nodos del índice; la
  demo ejecuta visor, lector y renderer en worker.
- [x] Mostrar estadísticas de puntos/bloques visibles, nivel LOD más profundo, cobertura, bloques
  pendientes, uso/presupuesto de caché decodificada, bytes GPU y tiempo CPU del worker.
- [x] Mostrar los bytes únicos leídos del archivo y cancelar una lectura terminando/recreando el
  worker y su canvas. La operación puede reiniciarse seleccionando otra vez el archivo.
- [x] Añadir un `.oct` reproducible y controles accesibles por teclado para cargarlo o elegir otro archivo.
- [x] Probar la cola coalescente de comandos (último valor, tipos separados y reset) con `node:test`;
  los contratos Rust de caché, transiciones LOD e importación LAS tienen tests de ciclo de vida.
- [ ] Probar bindings, cancelación y propagación de errores en navegador real; la validación de sintaxis
  JS y las pruebas unitarias no demuestran que una UI WebGPU permanezca interactiva.

#### Contrato del puente LOD WASM actual

Hay dos entradas. Para datos propios ya decodificados, el host abre el índice residente con
`openLodDataset({ coordinateOrigin, semantics, cacheBytes, sourcePointCount?, nodes })`. Cada nodo
contiene `id`, `parent`, `level`, `min`, `max`, `pointCount`, `geometricError` y `blocks`; los IDs y
conteos `u64` son strings decimales. `min`/`max` están en coordenadas del dataset;
`insertLodBlock(id, positions, colors)` recibe posiciones `f32` locales a `coordinateOrigin` y RGB
intercalado en arrays tipados; los convierte en lotes de hasta 16.384 puntos a una `PointBlock` propia
de Rust y rechaza bloques cuyo payload excede `cacheBytes`. El host conserva los arrays mientras dura
la llamada. `replacePoints`/`appendPoints` y sus variantes voxelizadas reciben los mismos typed arrays;
limitan cada escena de entrada al máximo de instancias GPU configurado y mueven los puntos a Rust.
`readNext` devuelve lotes limitados a 65.536 puntos/8 MiB de registros fuente. Para leer una escena de
diagnóstico desde JS, usa `renderSceneBatch(start, count)` con hasta 16.384 instancias; `renderScene()`
solo funciona hasta ese tamaño. Para archivos del proyecto, `openFormatDataset(file, cacheBytes)` abre el
Blob con el lector v1/v2. Ejecuta `createOffscreen(canvas.transferControlToOffscreen(), voxelSize)` y
las operaciones de archivo en un worker dedicado; así `FileReaderSync` no bloquea la UI y la superficie
WebGPU también vive en ese worker. El lector y el selector comparten la misma jerarquía en memoria.

Después de abrir un dataset, `updateLod("quality" | "performance", pointBudget?)` selecciona y guarda
el perfil. La selección se recalcula antes de `renderFrame` tras cambios de cámara o llegada de
bloques; para encuadrar, llama primero `fitScene()` y después `renderFrame()`. En el camino de formato,
los bloques que faltan se leen antes del cambio atómico del corte. `clear()` cierra el dataset LOD.
Las estadísticas incluyen bloques/puntos visibles, máximo nivel del corte (`maxVisibleLevel`),
presupuesto, uso y límite de caché decodificada (`cacheBytes`/`cacheBudgetBytes` como strings
decimales), IDs de bloques pendientes, cobertura y nodos omitidos por presupuesto. La demo ya ejecuta
lector y renderer en un worker; informa bytes
únicos cubiertos por rangos de 256 KiB. No es un porcentaje de finalización: la lectura busca
cabeceras e índice y luego solo bloques seleccionados. Cancelar termina el worker síncrono,
reemplaza el canvas transferido y vuelve a crear el visor; la apertura debe iniciarse otra vez. La demo
limita comandos repetibles a uno en vuelo y un valor pendiente por tipo, y no envía más de un frame a la
vez; las lecturas siguen siendo síncronas dentro del worker.

**Cierre:** la interfaz permanece utilizable durante lectura/navegación, reporta estados y errores y
no copia el archivo o todos sus puntos a JS. Las transferencias tipadas usan staging Rust acotado y
límites explícitos; la demo de archivo local permanece en la ruta directa `FormatReader`→caché→GPU.

### Fase 7 — Aplicación de referencia y extensibilidad (`examples/web-host`)

**Característica:** demostrar el flujo completo y servir como integración mínima mantenible.

- [x] Reemplazar la proyección Canvas 2D por el canvas 3D y el renderer real para la muestra sintética.
- [x] Añadir selección de archivo, estados de carga/error y resumen de nodos con controles etiquetados.
  Muestra bytes únicos leídos y permite cancelar/reintentar reiniciando el worker y el canvas.
- [x] Mostrar puntos, bloques visibles, nivel LOD más profundo, cobertura, bloques pendientes,
  caché CPU usada/presupuesto, bytes GPU y tiempo CPU del worker.
- [x] Documentar el contrato del worker y WASM, generar un `.oct` sintético reproducible e incluir
  un botón accesible por teclado para abrirlo; el selector local acepta otros archivos.
- [x] Mantener la demo como consumidor de la API pública; el único código de geometría JavaScript
  genera el dataset sintético ya existente.

**Cierre:** una persona puede seleccionar un archivo convertido, ver progreso, navegar la nube y
entender límites/errores sin editar el código de la demo.

**Estado actual:** la demo transfiere el canvas a un worker, crea allí el renderer WebGPU y abre
archivos del formato v1/v2 mediante `FileReaderSync`. `updateLod` lee los bloques visibles bajo demanda
y cambia el corte de forma atómica. Informa bytes únicos leídos en rangos de 256 KiB (sin
interpretarlos como porcentaje de finalización); cancelar termina/recrea el worker y permite reintentar
seleccionando de nuevo el archivo. Muestra estado, resumen del índice, puntos/bloques visibles, nivel
LOD más profundo, cobertura, bytes subidos, caché decodificada usada/límite y tiempo síncrono del
worker; permite navegar, cambiar perfil, color y tamaño. Incluye un archivo sintético `.oct` de 4.096
puntos, generable desde un ejemplo Rust, y un control de teclado para cargarlo. Chromium con WebGPU
cargó la muestra y presentó frames; el redimensionado del canvas y el layout móvil se comprobaron en
viewport de escritorio y de 500 px. La captura headless no permitió confirmar visualmente los puntos,
y Firefox no expuso `navigator.gpu` en este entorno; la validación visual y de compatibilidad siguen
pendientes.

### Fase 8 — Validación de escala, calidad y compatibilidad

**Característica:** demostrar que las prestaciones y el objetivo de escala se sostienen con evidencia.

- [x] Ejecutar pruebas unitarias de integridad de formato/índice/bloques y recuperación de
  checkpoints (`cargo test -p viewer_format` y `cargo test -p viewer_convert`). Cubren corrupción,
  truncamiento y reanudación; no sustituyen pruebas de escala sobre datasets mayores de 500 GB.
- [ ] Medir datasets de varios tamaños y distribuciones hasta superar 500 GB en la definición acordada;
  incluir nubes densas, dispersas, RGB y sin RGB, y declarar tamaño en GB o GiB.
- [ ] Registrar RAM pico, memoria GPU, tiempo de conversión, tiempo a primera imagen, latencia de
  navegación, bytes leídos/subidos, frames perdidos y calidad visual.
- [ ] Comparar contra Potree en la misma máquina, fuente y tarea de visualización; documentar
  diferencias de conversión/formato, mantener calidad visual equivalente y evitar comparar solo FPS
  o solo tamaño convertido.
- [ ] Separar tiempos de conversión, apertura, primera vista útil y navegación; reportar número de
  puntos y atributos, RAM del proceso/conversor, memoria WASM/JS/GPU cuando se pueda medir, bytes
  solicitados y leídos, y latencia de frame p50/p95/p99.
- [ ] Repetir con caché fría y caliente y publicar versiones de software, configuración, equipo,
  almacenamiento/red y recorrido de cámara para que la comparación sea reproducible.
- [ ] Probar navegadores/dispositivos objetivo, archivos inválidos, falta de memoria, pérdida de
  dispositivo/superficie y cancelación durante lectura/conversión.
- [ ] Publicar procedimiento de benchmark, resultados y límites conocidos; ajustar los objetivos si
  la evidencia no los alcanza.

**Cierre:** se puede reproducir la prueba de más de 500 GB dentro de límites acordados y explicar con
datos en qué áreas el proyecto supera, iguala o todavía no alcanza a Potree.

## Mapa por librería

| Componente | Responsabilidad final | Dependencias principales |
| --- | --- | --- |
| `viewer_core` | Geometría, atributos, jerarquía, LOD, transición de cortes, consulta, planificación priorizada y caché de bloques independiente de plataforma. | Modelo compartido; no depende de WASM, DOM, I/O ni `wgpu`; el executor queda fuera. |
| `viewer_format` | Especificación, lectura/escritura, índices y bloques comprimidos del formato propio. | Modelo espacial de `viewer_core`; pruebas de compatibilidad. |
| `viewer_convert` | Conversión LAS/LAZ por bloques y EPT local `laszip` aditivo, publicación validada, checkpoints LAS/LAZ, particiones 2/4/8 y paralelismo acotado. | `las`, `serde_json`, `viewer_core`, `viewer_format`; EPT remoto/otros codecs y evaluación de calidad visual siguen pendientes. |
| `viewer_render_wgpu` | Cámara, recursos, pipeline, subida acotada y presentación GPU. | Bloques/selección de `viewer_core`; GPU disponible. |
| `viewer_wasm` | API al host, workers, control de lectura y ciclo de vida del visor en navegador. | `viewer_core`, `viewer_format`, `viewer_render_wgpu`. |
| `examples/web-host` | Integración de referencia, interacción y estado visible al usuario. | API pública WASM; no reimplementa lógica de crates. |

## Decisiones pendientes que afectan el plan

Estas decisiones no bloquean el trabajo inicial sobre modelo, formato y pruebas pequeñas, pero sí
cambian alcance, rendimiento y compatibilidad:

- ¿La primera versión abre únicamente archivos locales o también datos remotos mediante peticiones
  por rango?
- ¿Qué sistemas operativos, mínimos de RAM y clases de GPU forman el perfil oficial? Chrome y Firefox
  con WebGPU son los navegadores elegidos, pero sus flujos aún no se han verificado en navegador real.
- ¿Qué dimensiones LAS/LAZ se deben preservar en v1 y cuáles pueden convertirse o descartarse con
  aviso explícito?
- ¿La salida es un contenedor único, un conjunto de archivos o ambos? Esta decisión afecta escrituras
  interrumpidas, almacenamiento remoto y lectura aleatoria.
- ¿Qué licencia y política de compatibilidad tendrá la especificación propia, y se publicará para
  permitir lectores externos?
- ¿Qué tareas de análisis (selección, medición, clasificación, recorte, exportación) pertenecen al
  primer producto y cuáles son extensiones posteriores?

## Referencias para el objetivo técnico

- [Potree](https://github.com/potree/potree): referencia WebGL de visualización de grandes nubes; su
  repositorio documenta conversión a formato Potree y trabajos sobre octrees y LOD. La comparación de
  rendimiento debe basarse en pruebas propias reproducibles.
- [COPC 1.0](https://copc.io/copc-specification-1.0.pdf): formato LAZ organizado como octree para
  acceso espacial y lectura por bloques; referencia útil para evaluar interoperabilidad y decisiones
  de acceso aleatorio.
- [Estándar LAS de OGC](https://www.ogc.org/standards/LAS/): referencia de intercambio para datos
  LiDAR que deben poder convertirse a la representación de visualización del proyecto.

## Desarrollo y validación

Perfiles Cargo para separar iteración rápida y mediciones optimizadas:

| Perfil | Uso | Compilación |
| --- | --- | --- |
| `fast` | Tests frecuentes (`cargo test-fast`) | Hereda `dev`, omite símbolos de depuración y mantiene habilitada la compilación incremental. |
| `perf` | Benchmarks nativos (`cargo bench-visibility`, `cargo bench-codec`, `cargo bench-spatial`) | Hereda `release`, usa `opt-level=3`, LTO thin y 16 unidades de código. |
| `release` | Artefactos actuales y referencia de tamaño/WASM | Mantiene `opt-level="s"`, LTO completo y una unidad de código. |

`fast` y `perf` generan artefactos separados en `target/fast` y `target/perf`. `perf` facilita
iteraciones de benchmark y no sustituye las cifras de `release`; registra siempre el perfil, la
revisión, el dataset y el equipo. Los aliases de `.cargo/config.toml` también ofrecen
`cargo check-all` para compilar todos los targets.

Para medir la selección de vóxeles visibles frente a ordenar todos los candidatos y truncar:

```bash
cargo bench-visibility
```

El benchmark sintético genera 200.000 vóxeles y compara medianas de nueve muestras intercaladas
por presupuesto. En Linux x86-64 con AMD Ryzen AI 9 365, tres ejecuciones del perfil `perf` dieron
una mejora de 2,79–2,83× para límite 256, 2,72–2,86× para 4.096 y 1,75× para 65.536. Son resultados
de microbenchmark local, no una predicción del tiempo de frame en navegador. La consulta limitada
usa unos 8 bytes temporales por candidato para conservar el orden de empates y asigna el resultado
final aparte, proporcionalmente al límite solicitado.

Checks del workspace y comandos de iteración:

```bash
cargo test-fast
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check-all
cargo check -p viewer_wasm --target wasm32-unknown-unknown
```

La ejecución de los tests y la compilación WASM depende de las dependencias y targets disponibles en
el entorno. Los checks actuales no sustituyen las pruebas de escala del plan: estas necesitarán
datasets grandes, hardware objetivo y un procedimiento de benchmark reproducible.
