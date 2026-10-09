# Octree Viewer

Visor extensible de nubes LiDAR y nubes de puntos 3D, construido en Rust y ejecutable en el
navegador mediante WebAssembly. El proyecto está en una fase temprana: hoy contiene un núcleo de
voxelización en memoria, una importación LAS/LAZ incremental, preparación de datos de escena y una
demo web 2D. Aún no es un visor 3D ni puede abrir y explorar datasets de cientos de GB de extremo a
extremo.

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
el soporte completo se debe fijar si ese tamaño se mide antes o después de la conversión, qué atributos
se incluyen y qué hardware y navegador forman parte del objetivo.

## Principios de arquitectura

- **Lectura fuera de memoria:** no retener el dataset completo en RAM ni copiar la escena completa a
  JavaScript.
- **Acceso espacial:** indexar datos en bloques jerárquicos independientes que puedan leerse,
  descomprimirse y descartarse por separado.
- **Nivel de detalle:** seleccionar detalle según cámara, tamaño proyectado y presupuesto, no solo
  cantidad total de puntos.
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
| `viewer_core` | Tipos de puntos, RGB, lotes, voxelización, AABB y jerarquía validada con consultas AABB/frustum. | `SparseVoxelStore` mantiene voxeles en un `HashMap`; la jerarquía de nodos y sus metadatos también residen en memoria. Aún no se leen bloques ni se selecciona LOD; el filtro de voxeles incluye por centro. |
| `viewer_render_wgpu` | Conversión de voxeles a instancias, layout de vértices y shader WGSL. | No crea dispositivo, superficie, pipeline ni presenta una escena. |
| `viewer_wasm` | Bindings para el visor y sesión incremental LAS/LAZ desde un `Blob`. | No hay lectura espacial del formato propio ni flujo completo de worker, contrapresión y cancelación. |
| `examples/web-host` | Demo de datos sintéticos y proyección de depuración con Canvas 2D. | No es visualización 3D, no abre archivos y copia la escena de instancias al host. |
| Formato propio | Especificación v1, cuantizador `u32`, codec LZ4 por bloque, CRC32, escritor incremental y lector con acceso aleatorio y límites configurables ([`docs/FORMAT_V1.md`](docs/FORMAT_V1.md)). | Fixtures generadas prueban round-trip, lectura selectiva, corrupción, límites y versión; la política v1 exige nueva versión para ampliar el formato. XYZ `u32` conserva 12 bytes por punto, igual que `f32`; el ahorro aplica frente a `f64`. |
| Pruebas y rendimiento | Hay tests pequeños de núcleo e importación. | No demuestran consumo de memoria o navegación con datasets grandes ni comparan con Potree. |
| `viewer_convert` | CLI nativa LAS/LAZ → formato v1 en bloques secuenciales. Conserva intensidad, RGB16, clasificación, retornos, ángulo de escaneo en grados `f32`, user data, source ID y GPS/NIR cuando están disponibles. | Emite un único nodo raíz sin partición espacial/LOD; descarta flags, waveform, Extra Bytes y CRS VLR con aviso. El índice y directorio de bloques permanecen en RAM. |

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

- [ ] Definir si los 500 GB corresponden a LAS/LAZ de origen, al resultado propio o a ambos; registrar
  puntos, atributos, CRS, distribución espacial y tamaño convertido de cada dataset.
- [ ] Definir entornos objetivo: navegador, sistema operativo, memoria RAM/GPU, almacenamiento local o
  remoto y disponibilidad de lectura por rangos.
- [ ] Elegir métricas: tiempo de conversión, tiempo hasta primera imagen, memoria pico, bytes leídos,
  latencia de consulta/navegación, FPS o tiempo de frame, calidad visual y estabilidad.
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

`SpatialHierarchy::select_lod` proyecta el error mediante una función suministrada por el consumidor,
elige un corte visible sin mezclar padre e hijos y refina por mayor error mientras el reemplazo entero
quepa en el presupuesto. El productor debe garantizar que el payload del padre representa el
subárbol y que sus hijos visibles lo refinan; los metadatos por sí solos no permiten validar esa
cobertura. Si el corte visible base excede el presupuesto se devuelve un error; si una mejora no cabe
o no hay resolución disponible, el resultado lo informa. La selección opera sobre conteos de puntos
representantes, no sobre bytes o memoria. El conversor aún debe materializar esta política en bloques
y producir las cotas geométricas para que el selector funcione con datos reales.

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

- [x] Publicar una especificación mínima: magic/versionado, endianness, metadatos, CRS y transformación
  de coordenadas, esquema de atributos, jerarquía, directorio de bloques, offsets/tamaños, compresión y
  reglas de alineación.
- [x] Definir representación de cada nodo: límites espaciales, nivel, conteos, rango de atributos,
  política de muestreo y bloque(s) asociados.
- [x] Diseñar y medir la representación compacta de coordenadas y atributos: definir el origen,
  escala y precisión de cuantización por dataset o bloque, comenzar evaluando coordenadas `u32` y
  reconstruirlas a `f32` local al preparar los datos para renderizado. El error de cuantización debe
  quedar acotado y reflejarse en las cotas de error geométrico.
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
- [ ] Medir RSS pico con datasets reales y añadir paralelismo solo con colas acotadas y contrapresión.
- [x] Informar fases, puntos procesados, bytes escritos y errores en la API/CLI.
- [x] Permitir cancelación cooperativa desde el callback entre bloques y durante validación; limpiar
  el temporal y no publicar una salida parcial.
- [x] Conectar la cancelación de la CLI a Ctrl+C con manejo portable de señales.
- [x] Permitir reconstrucción segura desde la fuente tras cancelación; validar el resultado antes de
  publicar.
- [ ] Añadir checkpoints durables y reanudación que conserve el trabajo ya procesado.
- [x] Comparar particiones 2×2×2, 4×4×4 y 8×8×8 con nubes sintéticas y compresión LZ4; registrar
  metodología, métricas y límites en [SPATIAL_BRANCHING_BENCH.md](docs/SPATIAL_BRANCHING_BENCH.md).
- [ ] Elegir e implementar una estrategia de partición y LOD que preserve representación útil en
  zonas densas y dispersas; validar error geométrico, sesgo, compresión y transiciones con LAS/LAZ
  representativos antes de fijar el branching factor.
- [x] Evitar pérdida silenciosa de dimensiones LAS/LAZ: preservar atributos soportados y reportar de
  forma explícita los que se descartan o transforman.
- [x] Aceptar LAS/LAZ y COPC como entrada secuencial interoperable; el fixture COPC de 107 puntos de
  `las 0.11.1` se convierte correctamente. COPC se aplana en una raíz y no conserva su jerarquía;
  consultar la [especificación COPC](https://copc.io/) para conservar acceso espacial en una versión
  futura.
- [x] Evaluar EPT: es un dataset de varios archivos (`ept.json`, jerarquía y datos por nodo), no una
  ruta LAS/LAZ única ([especificación EPT](https://entwine.io/en/latest/entwine-point-tile.html)).
- [ ] Implementar entrada EPT tras definir una abstracción de dataset y cómo preservar su jerarquía
  aditiva en el formato propio.

Uso inicial: `cargo run -p viewer_convert -- INPUT.las|INPUT.laz|INPUT.copc.laz OUTPUT.oct [--points-per-block N] [--max-error E] [--max-memory-mib MIB]`.
Los valores por defecto son 262.144 puntos por bloque y error máximo de cuantización 0,01 en unidades
de la fuente, con un presupuesto estimado de memoria controlada de 512 MiB. La estimación reserva
192 bytes por punto del bloque, 512 bytes por entrada del directorio y 16 MiB fijos; no equivale al
RSS total del proceso ni incluye memoria interna de la biblioteca LAS/LAZ o del allocator. La salida
publica mediante un enlace al archivo temporal validado y falla si el destino ya existe. Esta primera
versión usa un solo nodo raíz y conserva atributos estándar disponibles: intensidad, RGB, clasificación,
retornos, ángulo de escaneo normalizado a grados `f32`, user data, source ID, GPS y NIR. Avisa de flags,
waveform, Extra Bytes y CRS VLR que no conserva. La memoria del directorio todavía crece con el número
de bloques.
La implementación separa inspección de cabecera, planificación de nodo/esquema, lectura y escritura
secuencial, y validación/publicación; la planificación espacial actual solo crea una raíz sin LOD.
La CLI muestra las fases y actualiza puntos/bytes en pasos del 10%; `convert_file_with_progress`
ofrece los mismos eventos y permite cancelar devolviendo `false`. La cancelación cooperativa elimina
el temporal; un cierre abrupto puede dejarlo, pero la siguiente ejecución lo ignora y reconstruye
desde la fuente. Ctrl+C solicita esa cancelación y espera al siguiente límite seguro entre bloques.
Los checkpoints durables y la reanudación siguen pendientes.

**Cierre:** una conversión de dataset de escala acordada termina con memoria acotada, puede cancelarse
y reanudarse, y produce un archivo validado y abierto por el lector del formato.

### Fase 4 — Lectura fuera de memoria y selección (`viewer_core` + `viewer_format`)

**Característica:** pasar de cámara/consulta a bloques concretos, priorizados y acotados.

- [ ] Implementar recorrido jerárquico con poda por frustum y selección de LOD según error/tamaño
  proyectado o política equivalente documentada.
- [ ] Crear planificador de solicitudes con prioridad por visibilidad y distancia, límites de trabajos
  en curso y cancelación de bloques que dejan de ser relevantes.
- [ ] Añadir caché acotada de bloques descomprimidos con política de expulsión, presupuesto de bytes y
  estadísticas observables.
- [ ] Gestionar transiciones LOD sin huecos notorios ni duplicación excesiva de puntos; especificar el
  comportamiento en los bordes de bloques.
- [ ] Hacer que la consulta solo devuelva bloques dentro del presupuesto, y exponer los motivos de
  omisión/atraso para diagnóstico.

**Cierre:** al cambiar la cámara, se solicitan/cancelan los bloques esperados; la memoria de caché no
supera el límite configurado y la selección mantiene cobertura visual aceptable.

### Fase 5 — Presentación 3D (`viewer_render_wgpu`)

**Característica:** transformar bloques seleccionados en una escena 3D visible y eficiente.

- [ ] Inicializar adaptador, dispositivo, superficie, formato de presentación, pipeline y recursos GPU;
  manejar resize, pérdida/restauración de superficie y errores de inicialización.
- [ ] Implementar cámara y controles de navegación adecuados a escalas y coordenadas LiDAR, con
  encuadre inicial al dataset y precisión numérica estable cerca/lejos del origen.
- [ ] Subir solo bloques seleccionados; actualizar y reutilizar buffers en lugar de reconstruir y
  serializar la escena completa en cada cambio.
- [ ] Añadir modos iniciales de color (RGB, intensidad, elevación y clasificación cuando exista) y
  tamaño de punto configurable.
- [ ] Definir presupuesto de puntos/instancias, gestión de recursos y comportamiento si la GPU no
  soporta los límites esperados.
- [ ] Añadir diagnóstico de tiempo de frame, bytes subidos y cantidad visible; el shader actual es un
  punto de partida, no un renderer completo.

**Cierre:** el host presenta una nube real en 3D, permite navegarla y el número de bloques y bytes
subidos corresponde a la selección visible, no al tamaño total del dataset.

### Fase 6 — Orquestación WASM (`viewer_wasm`)

**Característica:** exponer una API estable y asíncrona al host para abrir datasets, gestionar lecturas
y presentar estado.

- [ ] Conectar lector del formato y solicitudes espaciales con el controlador del visor.
- [ ] Mover lectura y decodificación a workers; definir mensajes para progreso, errores, cancelación,
  disponibilidad de bloques y contrapresión.
- [ ] Sustituir las copias completas de arrays/instancias por buffers o lotes acotados, con propiedad
  y ciclo de vida claros entre JS y WASM.
- [ ] Soportar fuente local seleccionada por el usuario; definir por separado si el producto debe
  aceptar URL/range requests, porque CORS, autenticación y consistencia de rangos requieren contrato
  específico.
- [ ] Exponer operaciones útiles sin acoplar el host a estructuras internas: abrir/cerrar, encuadrar,
  mover cámara, definir modo de color, consultar estadísticas y cancelar.
- [ ] Añadir pruebas de bindings, ciclo de vida y concurrencia; comprobar que callbacks/eventos no
  bloqueen la UI y que los errores llegan al usuario.

**Cierre:** la interfaz permanece utilizable durante lectura/navegación, reporta estados y errores y
no realiza copias proporcionales al tamaño total de la nube.

### Fase 7 — Aplicación de referencia y extensibilidad (`examples/web-host`)

**Característica:** demostrar el flujo completo y servir como integración mínima mantenible.

- [ ] Reemplazar la proyección Canvas 2D por el canvas 3D y el renderer real.
- [ ] Añadir selección de archivo, estados de conversión/carga, progreso, cancelar/reintentar y resumen
  de metadatos con acciones accesibles por teclado.
- [ ] Mostrar estadísticas de forma útil: puntos totales, bloques solicitados/visibles, nivel LOD,
  memoria/caché conocida, tiempo de frame y errores recuperables.
- [ ] Documentar el contrato para integrar el WASM en otra aplicación y ofrecer un ejemplo pequeño que
  cargue datos locales reproducibles.
- [ ] Mantener la demo como consumidor de API pública; no introducir lógica de almacenamiento o
  geometría que duplique el núcleo en JavaScript.

**Cierre:** una persona puede seleccionar un archivo convertido, ver progreso, navegar la nube y
entender límites/errores sin editar el código de la demo.

### Fase 8 — Validación de escala, calidad y compatibilidad

**Característica:** demostrar que las prestaciones y el objetivo de escala se sostienen con evidencia.

- [ ] Ejecutar pruebas de integridad sobre formato, índices, bloques y recuperación ante truncamiento.
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
| `viewer_core` | Geometría, atributos, jerarquía, LOD, consulta y presupuesto independiente de plataforma. | Modelo compartido; no depende de WASM, DOM ni `wgpu`. |
| `viewer_format` | Especificación, lectura/escritura, índices y bloques comprimidos del formato propio. | Modelo espacial de `viewer_core`; pruebas de compatibilidad. |
| `viewer_convert` | Conversión nativa secuencial LAS/LAZ por bloques y publicación validada del formato. | `las`, `viewer_core`, `viewer_format`; jerarquía/LOD y procesamiento reanudable pendientes. |
| `viewer_render_wgpu` | Cámara, recursos, pipeline, subida acotada y presentación GPU. | Bloques/selección de `viewer_core`; GPU disponible. |
| `viewer_wasm` | API al host, workers, control de lectura y ciclo de vida del visor en navegador. | `viewer_core`, `viewer_format`, `viewer_render_wgpu`. |
| `examples/web-host` | Integración de referencia, interacción y estado visible al usuario. | API pública WASM; no reimplementa lógica de crates. |

## Decisiones pendientes que afectan el plan

Estas decisiones no bloquean el trabajo inicial sobre modelo, formato y pruebas pequeñas, pero sí
cambian alcance, rendimiento y compatibilidad:

- ¿El objetivo de 500 GB se mide en archivos fuente LAS/LAZ, en formato propio o en ambos?
- ¿La primera versión abre únicamente archivos locales o también datos remotos mediante peticiones
  por rango?
- ¿Qué plataformas, navegadores, RAM y clases de GPU son hardware oficialmente objetivo?
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

Checks existentes para el workspace:

```bash
cargo test --workspace
cargo fmt --all -- --check
cargo check -p viewer_wasm --target wasm32-unknown-unknown
```

La ejecución de los tests y la compilación WASM depende de las dependencias y targets disponibles en
el entorno. Los checks actuales no sustituyen las pruebas de escala del plan: estas necesitarán
datasets grandes, hardware objetivo y un procedimiento de benchmark reproducible.
