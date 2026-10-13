# Octree Viewer

Visor de nubes LiDAR en Rust/WebAssembly y WebGPU. Convierte LAS/LAZ o EPT local en una
jerarquía espacial persistente y lee los bloques que necesita la vista. El objetivo es explorar
fuentes mayores de **500.000.000.000 bytes** con memoria acotada; ese soporte completo y la
superioridad frente a Potree **todavía no están demostrados**.

## Uso y formato actual

```sh
cargo run --profile perf -p viewer_convert -- INPUT.laz OUTPUT.oct
# También admite un directorio EPT local laszip o su ept.json.
# --checkpoint conserva fases completas; --resume retoma la misma entrada/opciones.
```

La conversión nativa y la conversión web producen dos archivos **v5**:

| Archivo | Contenido | Uso |
| --- | --- | --- |
| `OUTPUT.oct` | XYZ cuantizado a `u32`, marco de coordenadas, jerarquía e índice de ambos archivos. | Se abre por sí solo y se visualiza en gris. |
| `OUTPUT.octattrs` | RGB de presentación y columnas tipadas, alineadas por bloque/fila. | Opcional; habilita RGB, intensidad, clasificación y selección de cualquier componente numérico. |

El archivo de coordenadas conserva los metadatos **estructurales** necesarios para localizar,
cuantizar y seleccionar bloques, además del esquema/rangos; no duplica los valores de atributos.
El acompañante no duplica coordenadas ni jerarquía. Renombrar la pareja no cambia su asociación.
El lector verifica el cierre, tamaños y checksum del índice común al adjuntar atributos, y el CRC
de cada payload al leerlo. Un acompañante incompatible se rechaza; un atributo corrupto no impide
leer XYZ. CRC protege frente a corrupción accidental, no falsificación deliberada.

En la demo, abrir `.oct` y después elegir su `.octattrs` en el selector opcional, o seleccionar el
acompañante antes de abrir las coordenadas. El selector de color enumera columnas y componentes
cuando están disponibles. Los rangos finitos del dataset mantienen la misma escala entre bloques;
valores no finitos o rangos constantes se muestran grises. Los enteros de 64 bits conservan sus
bits en disco; su proyección de color mediante `f64` puede perder precisión.

**Demo web y CORS:** para abrir con doble clic, usar el visor autocontenido generado en
`artifacts/visor-web-local/index.html`. El `examples/web-host/index.html` de desarrollo necesita
localhost/HTTPS: `python3 -m http.server 8000 --bind 127.0.0.1` desde la raíz, y abrir
`http://localhost:8000/examples/web-host/index.html`. Su arranque detecta `file://` y ofrece el
enlace local sin solicitar módulos externos. Regenerar el paquete tras cambiar host/WASM;
[comandos, contrato de carga y tests contra regresiones](examples/web-host/README.md#contrato-de-carga-y-prevención-de-regresiones-cors).

**Compatibilidad:** se lee/escribe solo v5. Reconstruir archivos v1–v4 desde su fuente; no se
reinterpretan silenciosamente. Los checkpoints anteriores tampoco se reanudan. Las muestras
versionadas están regeneradas. [Especificación completa](crates/viewer_format/FORMAT_V5.md).

```rust,ignore
let mut reader = FormatReader::new(coordinates)?;
let gray = reader.read_positions(block_id)?; // solo XYZ, incluso con acompañante adjunto
reader.attach_attributes(attributes)?;
let colors = reader.read_display_colors(block_id)?;
let column = reader.read_attribute(block_id, schema_index)?;
```

`read_geometry` mantiene la conveniencia XYZ+RGB cuando hay atributos, y gris si faltan.
`DatasetSource::read_block` añade todas las columnas disponibles. `FormatWriter::new_split`
escribe la pareja directamente, sin separar después un archivo completo. `new` mantiene el modo
monolítico de streams para integraciones; usa el mismo índice y codecs, también v5. La publicación
nativa valida los dos candidatos, publica atributos primero y coordenadas después mediante enlaces
sin sobrescritura. **No existe una transacción atómica de dos archivos**: un corte puede dejar solo
el acompañante. Los checkpoints verifican ambos candidatos y permiten retomar esa publicación;
sin checkpoint, conservar la fuente y retirar únicamente el acompañante incompleto que se sepa propio.
La conversión informa el tamaño final combinado.

## Arquitectura y límites

| Componente | Responsabilidad | Límite relevante |
| --- | --- | --- |
| `viewer_core` | Geometría, columnas tipadas, jerarquía, LOD, caché, scheduler y subdivisión compartida. | Índice residente en RAM; scheduler sin executor I/O; memoria estimada no equivale a RSS. |
| `viewer_format` | Formato v5, cuantización, LZ4 por payload, CRC, límites y lectura selectiva. | XYZ ocupa 12 bytes/punto antes de compresión, igual que `f32`; ahorra frente a `f64`. |
| `viewer_convert` | LAS/LAZ con spools y checkpoints, particiones 2/4/8, workers acotados; EPT aditivo local. | Presupuesto heurístico; EPT remoto/binary/zstandard y checkpoints EPT pendientes. |
| `viewer_render_wgpu` | Cámara, voxelización, color, buffers GPU, HZB y presentación. | Presupuesto de recursos administrados, no VRAM total. HZB conservador para vistas estáticas. |
| `viewer_wasm` | Worker, Blob, carga LOD progresiva y conversión OPFS en dos pasadas. | Lecturas síncronas por bloque; cede entre bloques. Conversión web de cientos de GB no es requisito. |
| `examples/web-host` | Demo, cámara, API embebible, conversión y descarga de la pareja. | OPFS/cuota, pérdida de GPU, picking y cancelación necesitan pruebas reales en navegadores objetivo. |

La conversión preserva intensidad, clasificación, retornos, ángulo, user data, point source ID y,
cuando existen, GPS time, RGB16 y NIR. Flags LAS, Extra Bytes y CRS requieren trabajo adicional;
el conversor informa las omisiones. El `.oct` guarda su propio marco local en `f64` y error de
cuantización acotado; posiciones de render se reconstruyen en `f32` local.

Los vóxeles pertenecen a celdas enteras alineadas, con reemplazo de ancestros por hijos y sin
solapamiento en la ruta LOD voxelizada. El objetivo proyectado de 0,5–16 px es un máximo de arista,
no garantía de densidad visible ni igualdad de tamaño. Los huecos reales no se rellenan. La consulta
simple de `SparseVoxelStore` incluye por centro; no es una prueba conservadora de intersección de
caras. El API genérico de buffers por bloque todavía requiere validar el mismo invariante visual.

## Revisión aplicada el 10-10-2026

- [x] Separar coordenadas, RGB y atributos con lecturas y checksums independientes. Ambos conversores
  producen la pareja; el visor acepta coordenadas solas y componentes numéricos seleccionables.
- [x] Comprobar asociación, truncamientos, corrupción selectiva, fallos de escritura, conflicto de
  nombres y recuperación de publicación parcial. Conservar validación paralela nativa acotada.
- [x] Sustituir divisiones/módulos enteros de direccionamiento 2/4/8 por campos de bits, con prueba de
  cada celda y plano interno. No cambiar la cuantización flotante ni los IDs persistentes del árbol.
- [x] Evitar reservas/rellenos GPS/RGB/NIR ausentes: hasta 16 bytes temporales por punto en bloques
  nativos sin esas dimensiones. Es una eliminación de buffers, no una medición de reducción de RSS.
- [x] Corregir la afirmación obsoleta de que intensidad/clasificación no llegaban a WASM y resolver
  la decisión de contenedor. El estado actual y las tareas están aquí; las mediciones y decisiones
  previas se conservan en el [registro técnico histórico](docs/DEVELOPMENT_HISTORY.md).

No se añaden flags por vóxel sin un significado/consumidor concreto: empaquetar estados inventados
no reduce datos existentes. Los flags del archivo distinguen modo separado/monolítico/acompañante
y rechazan combinaciones desconocidas. Tampoco se duplica toda la cuantización para ahorrar cálculo
mediante buffers adicionales sin medir el coste de memoria.

### Evidencia local y reproducción

Misma fuente y opciones, un worker, muestras de Outer Banks; tamaños exactos en bytes:

| Fuente | Archivo v4 anterior | Coordenadas v5 | Atributos v5 | Total v5 |
| --- | ---: | ---: | ---: | ---: |
| `fs_e430_n4008_18.las` (111.190 puntos) | 2.247.972 | 1.335.303 | 907.255 | 2.242.558 |
| `fs_e432_n4002_18.las` (149.730 puntos) | 3.046.648 | 1.797.783 | 1.241.351 | 3.039.134 |

La pareja apenas reduce el total en estas muestras (~0,25 %); distribuir solo coordenadas reduce
el archivo necesario aproximadamente un 41 %. No es una predicción para otros datasets.
El microbenchmark de direccionamiento, perfil `perf`, mediana de nueve pares alternados de dos
millones de operaciones, Rust 1.99.0, Linux x86_64 y AMD Ryzen AI 9 365, dio 7,96 ms con divisiones frente a 1,59 ms con bits (~5×) en este entorno.
Aísla operaciones enteras con divisores en tiempo de ejecución; excluye clasificación espacial
flotante, descompresión, construcción del árbol e I/O. No demuestra una aceleración de conversión.

```sh
cargo run --profile perf -p viewer_core --example subdivision_bench
cargo run --profile perf -p viewer_convert --example real_dataset_bench -- read OUTPUT.oct
cargo run --profile perf -p viewer_convert --example real_dataset_bench -- read-geometry OUTPUT.oct
cargo run --profile perf -p viewer_render_wgpu --example render_dataset -- OUTPUT.oct /tmp/preview 4 640 480
```

El benchmark de lectura completa adjunta el acompañante y contabiliza ambos archivos; la lectura
XYZ no lo necesita. `render_dataset` adjunta el `.octattrs` vecino si existe y usa gris si no.
Las cifras históricas de Belvedere (81,7 M puntos; 100,12 s; 8,37 GiB temporales observados) pertenecen
a versiones/configuraciones anteriores: ver el registro, no usarlas como resultado actual de v5.

### Revisión de la auditoría de memoria y almacenamiento

La inspección de código respalda priorizar memoria CPU, consultas del índice y temporales antes
de cambiar precisión o codec. Identifica costes potenciales; **no mide qué fase domina** en v5.
Las mejoras se contrastan contra una base comparable sin perder calidad, determinismo ni
mantenibilidad; las tareas abiertas se detallan al final de esta sección.

| Hallazgo contrastado | Implicación y matiz |
| --- | --- |
| [`VoxelSceneBuilder`](crates/viewer_render_wgpu/src/voxel_grid.rs) acumula en `BTreeMap` y mantiene una caché de proyección. | Desde el 11-10, la ruta WASM y el diagnóstico nativo usan un presupuesto CPU estimado de 256 MiB, independiente del límite de vóxeles. No limita RSS ni incluye bloques fuente, escena anterior o GPU. |
| [`FormatReader`](crates/viewer_format/src/reader.rs) lee el índice completo y construye directorios, bounds y jerarquía. | Los 256 MiB predeterminados limitan el índice serializado, no la memoria de las estructuras decodificadas ni el pico durante apertura. |
| [`references_block`](crates/viewer_core/src/spatial.rs) se usa al cargar bloques. | Desde el 11-10 usa un catálogo ordenado con búsqueda binaria. Añade 8 B por ID único; aún duplica IDs del directorio y las referencias por nodo. |
| [`attribute_range`](crates/viewer_format/src/reader.rs) agrega rangos por nodo para colorear con una escala global. | Desde el 11-10 memoriza las últimas 64 consultas de componente, compartidas por lectores derivados. La primera consulta sigue recorriendo metadatos; se conservan los rangos regionales y el formato v5. |
| [`prepare_point_block`](crates/viewer_format/src/writer.rs) conserva fuente, un buffer raw reutilizado y payloads preparados. | Desde el 11-10 reconstruye bounds sin vector intermedio y libera los `u32` tras codificarlos. Falta medir el pico del lote completo de workers. |
| [`estimated_memory_bytes`](crates/viewer_core/src/dataset.rs) cuenta capacidades de payload, sin sobrecarga del asignador/índice de caché. | Separar presupuesto de payload, memoria residente estimada y RSS observado. Ningún límite individual representa todo el proceso. |
| [`set_color_mode`](crates/viewer_wasm/src/bindings.rs) vacía bloques al cambiar modo o atributo seleccionado. | Aunque la lectura de columnas es selectiva, la recarga vuelve a leer geometría; medir ese coste antes de separar cachés. |

El [ensayo `u16`](docs/COORDINATE_WIDTH_BENCH.md) encontró 620 puntos fuera de tolerancia 0,01
en 19 de 1.343 bloques, con error máximo incremental 0,034913176. Descarta `u16` uniforme bajo
ese criterio, no un esquema con fallback a `u32`. Los 490.227.702 B de ahorro son teóricos y sin
comprimir; faltan error desde LAS/LAZ original y tamaño final con metadatos/compresión.
Las mediciones históricas de [Albuera/Majadas](docs/REAL_DATASET_BENCH.md) corresponden a versiones
anteriores y no validan v5; las bases v5 repetidas de Majadas y Albuera se documentan más abajo.
El [protocolo web](docs/WEB_PERFORMANCE_BENCH.md) sigue sin tabla de
resultados reales; sus tamaños de ejemplo históricos deben actualizarse al medir la pareja v5.

### Primera implementación de la auditoría — 11-10-2026

- [x] **Lookup de bloques:** catálogo inmutable ordenado/deduplicado en `SpatialHierarchy`, sin
  nuevo mapa ni cambios de IDs persistentes. Pruebas de equivalencia con el escaneo, IDs `u64`
  extremos, referencias compartidas/duplicadas y semánticas `Replacement`/`Additive`. El catálogo
  también sirve a fuentes sin `FormatReader`; su coste residente sigue siendo un compromiso explícito.
- [x] **Temporales del writer:** eliminar el vector reconstruido de 12 B/punto y liberar el vector
  cuantizado de otros 12 B/punto antes de comprimir. Conservar aritmética, error y bounds de fuente
  más reconstrucción. Ambas muestras Outer Banks producen la misma pareja byte a byte antes/después.
- [x] **Instrumentación nativa v5:** separar apertura del índice y asociación de atributos, exponer
  bytes del índice serializado, contar I/O de cada archivo y desglosar payloads; latencias por bloque
  p50/p95 en lectura secuencial con CRC/decodificación. `open_index_ms` conserva el total anterior;
  los nuevos campos distinguen ambas fases. No mide lectura aleatoria, navegador ni GPU.
- [x] **Primera medición:** tres pares de conversiones pequeñas y tres lecturas por modo sobre
  Belvedere v5; resultados, RSS externo, dispersión, limitaciones y comandos en
  [mediciones de memoria y lectura v5](docs/V5_MEMORY_BENCH.md). El benchmark sintético de lookup
  compara nueve pares alternados; su resultado no predice una aceleración del visor completo.

Validación: **215 tests del workspace y 2 del ejemplo de benchmark**, formato, Clippy nativo y
WASM. Comprobación de igualdad de archivos y de que cada modo lee solo los payloads solicitados.
La base completa sigue abierta: las conversiones grandes por fase y primeras rondas Firefox se
registran abajo; faltan lote de workers, matriz web completa y condiciones frías/red/OPFS.

### Presupuesto CPU de vóxeles — 11-10-2026

- [x] `VoxelBuildLimits` separa máximo de vóxeles y bytes estimados: 256 MiB por defecto en
  `RendererConfig`. Contabiliza acumuladores, reserva de caché de proyección y capacidad final
  de instancias; reduce la caché con presupuestos pequeños. Las APIs anteriores conservan su
  comportamiento sin límite de bytes; las rutas integradas usan `with_limits`/`try_finish`.
- [x] Liberar la caché de proyección antes de reservar las instancias con `try_reserve_exact`.
  Un error de acumulación invalida el builder: no se publica una escena parcial. WASM construye
  antes de subir/confirmar el nuevo corte, conservando el visible si falla el presupuesto CPU.
- [x] Exponer presupuesto, pico estimado, capacidad de instancias y máximo de entradas de caché
  en diagnóstico nativo y estadísticas web. No son medidas de RSS; las asignaciones de `BTreeMap`
  siguen siendo infalibles en la biblioteca estándar. Falta calibrar el modelo con procesos grandes.

Validación: **223 tests del workspace**, formato y Clippy nativo/WASM. El PNG Outer Banks a
640×480 y 4 px es idéntico byte a byte al anterior: 1.849 vóxeles y 73.960 B de instancias.
El rechazo con presupuesto cero conserva el PNG previo; es una prueba del diagnóstico nativo,
no una validación del navegador. Fórmula, alcance y comandos en
[mediciones v5](docs/V5_MEMORY_BENCH.md#presupuesto-cpu-del-builder-de-vóxeles).

### Rangos globales reutilizables — 11-10-2026

- [x] Caché LRU lazy de 64 consultas `(atributo, componente)`, incluyendo rangos ausentes,
  compartida por `fork_reader`. Validar índices antes de consultarla; mantener las conversiones
  numéricas y los rangos por nodo. No precalcula todos los componentes al abrir ni cambia v5.
- [x] Pruebas de evicción/reconsulta, extremos `u64`, componentes múltiples, rangos agregados,
  errores, ausencia de I/O y caché compartida. **225 tests del workspace**, formato y Clippy
  nativo/WASM. Benchmark reproducible en `viewer_format/examples/attribute_range_bench.rs`.

En Belvedere v5, tres pares alternados con 11 componentes y caché caliente: mediana de consultas
repetidas **9.481 → 14 ns/consulta**. La primera pasada sigue costando aproximadamente 105 µs
para los 11 componentes. Es un microbenchmark de metadatos, no una medida de cambio de color,
LOD o FPS; [método y dispersión](docs/V5_MEMORY_BENCH.md#consultas-repetidas-de-rangos-globales).

### Base específica de selección LOD — 11-10-2026

- [x] `lod_frontier_bench` mide selección de metadatos en fronteras amplias, árboles binarios y
  cadenas; 86 casos con ambas semánticas, presupuesto parcial/completo, visibilidad y callbacks
  declarados/sintéticos. Tres procesos, nueve muestras por caso; verifica resultados completos
  dentro de cada proceso y conserva checksums entre ejecuciones. **226 tests del workspace**,
  formato y Clippy nativo; la nueva regresión cubre desempates finitos/infinitos y orden de entrada.
- [x] Identificar repetición: 3.073 nodos sintéticos requieren 1.576.448 llamadas de proyección y
  unos 68 ms para un corte completo; una cadena de 1.024 nodos requiere 1.025 llamadas. No son
  tiempos de frame ni datos reales. [Método, dispersión y contratos](docs/LOD_FRONTIER_BENCH.md).
- [ ] Prototipo de prioridad incremental con memoria contabilizada y equivalencia frente al
  algoritmo actual: menor ID en empate, orden visible original, diagnósticos y reintento tras
  sustituciones que liberan presupuesto. Resolver estabilidad del callback antes de memorizar
  prioridades. La reutilización de temporales ya está medida; la cola incremental sigue pendiente.

### Reutilización de temporales LOD — 11-10-2026

- [x] Reutilizar buffers de recorrido, hijos y diagnósticos dentro de cada selección, evitando
  asignaciones y clones repetidos. Se conserva la API, el orden y número de callbacks, incluso
  con estado; no se memorizan prioridades. Las capacidades se liberan al terminar la llamada.
- [x] Cinco regresiones nuevas: equivalencia con el algoritmo anterior en 192 combinaciones,
  errores/overflow y reutilización sin residuos. **231 tests del workspace**, formato, Clippy
  y comprobaciones nativas/WASM. La imagen nativa de referencia coincide byte a byte.
- [x] Tres pares antes/después: 86 casos sintéticos y nueve cortes de Belvedere con cámara real.
  Frontera amplia: **68,42 → 61,12 ms**; Belvedere a 1 px sin límite de representantes:
  **10,25 → 9,42 ms**. RSS mediano sin cambio; son tiempos de selección CPU, no FPS.
  [Resultados, dispersión y comandos](docs/LOD_FRONTIER_BENCH.md#reutilización-de-temporales--11-10-2026).
- [ ] Reducir recorridos repetidos de la frontera: siguen presentes. Resolver el contrato del
  callback y contabilizar memoria antes de introducir prioridades incrementales.

### Lectura aleatoria reproducible — 11-10-2026

- [x] `real_dataset_bench read-random` permuta todos los bloques una vez, con semilla y checksum
  del recorrido. Reutiliza los modos XYZ/completo/columna, contadores de I/O y latencias p50/p95.
  Los comandos anteriores se conservan. Tres tests del ejemplo comprueban percentiles, contadores
  y permutación reproducible; formato y Clippy de `viewer_convert` pasan.
- [x] Tres pares secuencial/aleatorio sobre Belvedere v5, caché del SO caliente: mismos bytes y
  filas por modo, sin penalización clara de latencia. XYZ aleatorio muestra unos 2 MiB más de RSS,
  causa sin perfilar. [Resultados y comandos](docs/V5_MEMORY_BENCH.md#lectura-de-bloques-en-orden-aleatorio-caché-caliente).
- [x] Solicitudes parciales con `read-sample`: semilla/cantidad explícitas, selección sin duplicados,
  contadores y percentiles de los bloques solicitados. Tres repeticiones de 0/1/16/128 bloques
  en Belvedere, XYZ/completo/columna 0. Apertura de ~7,6–7,9 ms frente a 0,245 ms de lectura
  de columna para un bloque (medianas); no es una medida de mejora ni de navegación.
  [Método, dispersión y comandos](docs/V5_MEMORY_BENCH.md#solicitudes-parciales-reproducibles).
  Cuatro tests del ejemplo, equivalencia CLI con lectura completa, formato y Clippy pasan.
- [ ] Medir condiciones frías/red/OPFS y solicitudes de cámara; las muestras calientes no las
  representan. Conversiones grandes por fase y primeras rondas Firefox registradas abajo;
  falta completar Chrome y la matriz de interacción.

### Perfil de conversión nativa — 11-10-2026

- [x] Completar y verificar la instrumentación local `convert-profile`: tiempos entre notificaciones
  y contadores Linux por intervalo; muestreador externo de RSS/VmHWM y temporales con logs e informe
  de ejecución parcial. Rechazar éxito aparente si falta un archivo de la pareja o está vacío.
- [x] Belvedere v5: calentamiento y tres conversiones con un worker/divisiones 2; mediana
  **46,286 s**, validación **6,191 s**, RSS pico observado **94,254 MiB** y temporales máximos
  muestreados **4.941.891.420 B**. Ambos archivos coinciden byte a byte en las cuatro ejecuciones.
  El mayor intervalo es construir la jerarquía (**18,216 s**); no se ha aislado CPU frente a I/O.
  [Método, dispersión, amplificación de bytes y límites](docs/V5_MEMORY_BENCH.md#perfil-de-conversión-nativa-por-fase).
- [x] Tests del workspace, siete del ejemplo y cinco Python; formato, Clippy nativo/WASM y
  compilación Python. Ruff no está instalado. No es una comparación de mejora ni cierra la base
  de escala: faltan Albuera/Majadas repetidas y pruebas reales de navegador.

### Base nativa de Majadas TLS — 12-10-2026

- [x] Tres conversiones v5 `perf`, un worker/divisiones 2, fuente verificada y calentada antes
  de cada proceso. Mediana **347,277 s** [345,468–356,203]; jerarquía **197,574 s** y validación
  **32,317 s**. RSS pico observado **77,086 MiB**; temporales máximos muestreados **21.250.289.683 B**.
- [x] Validación profunda y SHA-256 coincidentes de ambas salidas en las tres repeticiones:
  **7.533.781.717 B**, **5,608553×** la fuente. Esta configuración no cumple <5 min ni ≤3×.
  [Método, dispersión, I/O y siguientes preguntas](docs/V5_MEMORY_BENCH.md#conversión-v5-de-majadas-tls--12-10-2026).
  Priorizar perfilar jerarquía/spools; no se ha aislado CPU frente a I/O ni demostrado una mejora.

### Base nativa de Albuera UAV — 12-10-2026

- [x] Tres conversiones v5 `perf`, un worker/divisiones 2, MD5 previo y fuente leída antes de
  cada proceso. Mediana **400,158 [397,782–453,562] s**, jerarquía **179,764 s**.
  RSS pico observado **101,246 MiB**;
  temporales máximos muestreados **34.537.564.482 B**.
- [x] Validación profunda e igualdad SHA-256 de ambas salidas: **13.666.843.763 B**,
  **3,174287×** fuente. Tampoco cumple <5 min ni ≤3× en esta configuración.
  [Método, dispersión e I/O](docs/V5_MEMORY_BENCH.md#conversión-v5-de-albuera-uav--12-10-2026).

### Concurrencia nativa medida — 12-10-2026

- [x] Tres pares alternados Belvedere, uno/dos workers, mismo binario `perf`, divisiones 2,
  fuente leída antes de cada proceso y muestreo 100 ms: mediana **46,037 → 34,928 s**.
  Validación profunda y SHA-256 iguales de `.oct` y `.octattrs` en los seis procesos.
- [x] Coste asociado: RSS pico muestreado **93,785 → 165,355 MiB**, temporales máximos
  **4,334 → 5,459 GiB**. Lote y workers de validación efectivos 1/2; pool Rayon 20 threads.
  [Dispersión y fases](docs/V5_MEMORY_BENCH.md#uno-frente-a-dos-workers-en-belvedere--12-10-2026).
- [ ] Repetir en Majadas/Albuera y medir internamente el lote y readers: el RSS mayor aparece
  en validación, pero los intervalos no atribuyen el coste a una estructura concreta.

### Temporales consumidos del conversor — 12-10-2026

- [x] Liberar hojas/LOD después de escribir su último rango, tras terminar las lecturas del lote.
  Conservar el spool durable del checkpoint y la recuperación tras cancelación.
- [x] Tres pares alternados de Belvedere v5: pico temporal lógico muestreado de
  **4.941.891.420 B** a **4.647.826.988 B** (**6,0 %** menos en la mediana).
  Ambas salidas tienen SHA-256 iguales en los seis procesos. No se extrapola a fuentes mayores.
  [Método, tiempos, RSS e I/O](docs/V5_MEMORY_BENCH.md#liberación-de-spools-consumidos--12-10-2026).
- [x] Tests de rangos compartidos/LOD, workers y reanudación; workspace, Clippy nativo/WASM y formato.
  Ronda posterior en Albuera/Majadas documentada abajo; falta contraste emparejado de la mejora.

### Picos por intervalo del conversor — 12-10-2026

- [x] Atribuir muestras RSS/temporales a intervalos de progreso con ordinales y límites
  explícitos. Excluir capturas que cruzan una transición; conservar fases repetidas y picos
  `null` cuando no hay muestras. VmHWM sigue siendo global. Metadatos inválidos/ausentes dejan
  el informe incompleto; la agregación recorre las muestras sin cargarlas todas en memoria.
- [x] Tres conversiones Belvedere v5, un worker/divisiones 2, muestreo 100 ms: mayor pico temporal
  en jerarquía, mediana **4.633.019.920 B**; mayor RSS muestreado en validación, **93,023 MiB**.
  Ambas salidas coinciden con la referencia v5 en las tres ejecuciones. No mide una mejora;
  los máximos breves y fases sin muestras siguen sin resolverse.
  [Método, dispersión y límites](docs/V5_MEMORY_BENCH.md#rss-y-temporales-por-intervalo-de-conversión--12-10-2026).
- [x] Ocho tests Python, siete del ejemplo Rust, Clippy de `viewer_convert` con todos sus targets,
  formato, compilación Python y diff. Ruff no está instalado. Albuera/Majadas por fase medidas
  abajo; pendientes lote de workers y matriz web. No se aíslan operaciones internas CPU/I/O.

### Primera base Firefox y ventana del benchmark — 12-10-2026

- [x] Reproducir y corregir el rechazo de rondas completas: separar los 10 s de observación
  del intervalo entre primer/último frame. Mantener el cálculo FPS y exportar ambos tiempos.
  La regresión falla antes y pasa después; siete suites Node (25 tests), cuatro tests Python
  y empaquetado local pasan. Visor autocontenido regenerado.
- [x] Tres rondas válidas de Outer Banks XYZ gris en Firefox 157.0.1 headless, WebGPU activado
  en perfil temporal, 1280×720 y 4 px: mediana **251 ms** hasta LOD objetivo, **60,00 FPS** de
  respuestas del worker, **2.410 vóxeles**. Adaptador y heap JS no identificables en los exports;
  no certifica FPS de GPU/pantalla ni interacción. [Método y dispersión](docs/WEB_PERFORMANCE_BENCH.md).
- [ ] Chrome 155 con SwiftShader pasa muestra RGB/clasificación, cámara, móvil y siete flujos API;
  falta matriz grande y GPU física. Belvedere RGB Firefox completa 3/3 a 8 px; falta 4 px y
  OPFS/picking/pérdida de GPU.
  Pareja y columnas Majadas en Firefox verificadas abajo. El primer timeout Belvedere por texto de estado era inconcluyente; las nuevas rondas
  usan la finalización del benchmark y conservan estados/fallos como se detalla abajo.

### Albuera y Majadas por fase — 12-10-2026

- [x] Tres conversiones por fuente con binario `perf` fijo, un worker/divisiones 2, MD5 previo
  y muestreo 100 ms; informes completos, validación profunda y hashes de ambas salidas iguales
  a las referencias v5. Medianas: **349,051 s** Majadas y **513,502 s** Albuera.
- [x] Mayor pico temporal muestreado en jerarquía: **18.804.783.558 B** Majadas,
  **27.429.926.943 B** Albuera. Mayor RSS muestreado en validación: **98,703 / 110,988 MiB**.
  [Dispersión, I/O, invariantes y reproducción](docs/V5_MEMORY_BENCH.md#albuera-y-majadas-picos-por-fase--12-10-2026).
- [ ] Perfilar internamente jerarquía/spools para tiempo/disco y lote/reader de validación para
  RSS; falta varios workers. Frente a rondas históricas hay menos temporales, más RSS y mayor
  tiempo en Albuera: no es contraste emparejado ni prueba de mejora. Ambas fuentes siguen sin
  cumplir <5 min y pareja ≤3×. No rebajar la validación profunda para alcanzar esos objetivos.

### Exploración web de datasets grandes — 12-10-2026

- [x] Repetir Belvedere XYZ tras reutilizar escenas: un intento limitado a **300 s**, sin
  benchmark completo; primera vista **24 ms**, corte 4 px, **782 bloques pendientes**,
  cobertura completa y caché **255,8 / 256 MiB**. No demuestra rechazo por presupuesto
  ni permite atribuir el estancamiento a la caché. Conservar diagnóstico y medir rechazos/
  progreso antes de cambiar presupuestos o el planificador.

- [x] Firefox headless, XYZ gris, 1280×720 y 4 px: Majadas produce **dos exports válidos de
  tres intentos**. Primera vista **61–76 ms**, LOD objetivo **152.983–161.454 ms**, luego
  ~**60 FPS** de respuestas. Tercer intento sin respuesta BiDi >30 s, sin export; no es una
  base cerrada de tres rondas válidas. GPU concreta y heap JS/WASM/RSS siguen sin identificar.
- [x] Conservar intentos incompletos y capturas: Belvedere sin benchmark completo a **180 s**
  (corte 8 px, 64 bloques pendientes); Albuera en Firefox nuevo a **300 s** (corte 4 px,
  187 pendientes). Ambos muestran nube y cobertura del corte actual, que no cierra el objetivo.
  [Métricas, condiciones y límites](docs/WEB_PERFORMANCE_BENCH.md#large-file-experiments--12-10-2026).
- [x] Instrumentar lectura/decodificación conjunta, selección, planificación, voxelización y
  preparación/subida CPU durante el refinamiento, con contadores de bloques y reconstrucciones.
  `lodStats().refinementWork` y el export web conservan el desglose acumulado; no mide GPU ni RSS.
  [Contrato de las fases](docs/WEB_PERFORMANCE_BENCH.md#refinement-phase-accounting).
- [x] Verificar fases en Firefox: tres rondas sintéticas y una Majadas exploratoria. Majadas:
  239 reconstrucciones, 522 M puntos de entrada acumulados; voxelización **157,4 s de 159,4 s**
  entre fases medidas. Mismo corte final de 6.397 vóxeles que las rondas anteriores; no es una
  comparación de velocidad ni prueba de que todas las reconstrucciones sean evitables.
- [x] Reutilizar la última celda terminal dentro del constructor, verificando la misma clave
  entera antes de omitir el recorrido desde la raíz. Memoria fija; mismo acumulador y presupuestos.
  Tres pares nativos × tres reconstrucciones: **3,73× Majadas**, **1,95× Albuera**, **2,22× Outer
  Banks** en construcción del corte final, con misma huella de escena, atributos y límites.
  No son ratios de FPS ni de carga web. Belvedere rechazado por el límite del diagnóstico.
  [Protocolo y resultados](docs/VOXEL_BUILD_BENCH.md).
- [x] Validar WASM optimizado en Majadas: **3/3 rondas Firefox**, objetivo mediano **59,7 s**
  [58,4–65,9], mismos contadores finales y captura idéntica a la anterior. Las sesiones web
  previas no están emparejadas; los ratios de mejora anteriores corresponden solo a CPU nativa.
- [x] Medir reconstrucciones sin cambios: una ronda Majadas registra **143 cortes repetidos**, de
  los que **141** mantienen también cámara exacta, viewport, etapa y payload visible. Consumen
  **41,0 de 59,1 s** de voxelización; 355 M puntos acumulados reutilizables. Es una medición
  exploratoria, no un ahorro prometido. [Contrato y evidencia](docs/WEB_PERFORMANCE_BENCH.md#unchanged-cut-instrumentation--12-10-2026).
- [x] Omitir construcción/subida de escenas idénticas, conservando commit del plan y protección
  de caché. Prueba Firefox de siete flujos: vista idéntica, cámara, viewport, etapa, payload,
  atributos y reapertura. Majadas **3/3**: **98 construcciones y 141 reutilizaciones**, mismos
  6.397 vóxeles finales; objetivo mediano **29,5 s [21,7–30,8]**. Sesiones no emparejadas:
  no se presenta un ratio de aceleración. [Resultados](docs/WEB_PERFORMANCE_BENCH.md#identical-scene-reuse--12-10-2026).
- [x] Completar Albuera XYZ gris tras la reutilización: **3/3 rondas Firefox**, primera vista
  mediana **61 ms [55–89]**, objetivo **140,3 s [119,9–151,4]**; mismos 17.030 vóxeles,
  568 bloques visibles y cobertura completa. 233 construcciones/513 reutilizaciones por ronda.
  Voxelización sigue dominando; faltan color/atributos y memoria real de navegador.
  [Condiciones y desglose](docs/WEB_PERFORMANCE_BENCH.md#albuera-repeated-gray-load-after-reuse--12-10-2026).
- [x] Separar lectura de rangos y trabajo restante del callback LOD, compartiendo contadores
  XYZ/atributos y excluyendo metadatos/consultas externas. Majadas exploratoria: **275 ms** de
  lectura/copia y **900 ms** restantes, 542 rellenos/142 MB con relecturas. Prueba Firefox de
  metadatos, vista repetida y reapertura; Clippy nativo/WASM, build release y 25 tests pasan.
  El residual incluye CRC, conversión y copias: no es descompresión pura ni I/O físico.
- [x] Validar Majadas RGB con `.octattrs`: **3/3 Firefox**, primera vista **65 ms [43–79]**,
  objetivo **28,9 s [20,5–31,3]**, misma geometría y captura con color. El export identifica el
  sidecar; el recolector separa atributos ausentes/presentes/desconocidos y muestra modo/color.
  Regresión reproducida: antes mezclaba cuatro configuraciones en una fila; ahora quedan separadas.
  [Resultados y límites](docs/WEB_PERFORMANCE_BENCH.md#majadas-rgb-sidecar--12-10-2026).
- [x] Añadir columnas tipadas al fixture reproducible (intensidad `u16` y clasificación `u8`)
  y probe browser para selección, columna inválida y sidecar incompatible. Firefox pasa fixture
  pequeño y Majadas con intensidad/componente RGB: mismos contadores finales, 6.397 vóxeles y
  caché de 112 MB. Rechazar `.octattrs` de Albuera conserva la escena anterior. Es validación
  API/worker; falta benchmark repetido del flujo de cambio de columna en el host.
- [x] Corregir tiempos de cambio de columna/detalle: `load` conserva apertura RGB y
  `refinement` identifica la petición actual. Color/detalle se coalescen juntos; FPS espera el
  último objetivo. Firefox pasa carga, columna, error y recuperación con cambios rápidos.
  Corregido también cierre prematuro de ventana (9.999 ms observado); 26 tests Node/6 Python
  y paquete local pasan. [Contrato y regresiones](docs/WEB_PERFORMANCE_BENCH.md#scoped-host-refinement-timing--12-10-2026).
- [x] Medir recarga de intensidad seleccionada en Majadas desde RGB estable: **3/3 Firefox**,
  primer frame mediano **6 ms [6–7]**, objetivo **21,0 s [20,3–21,7]**, mismos 6.397 vóxeles.
  Lectura/copia **185–380 ms**, residual **660–702 ms**, voxelización **16,0–17,2 s**.
  [Protocolo de recarga y límites](docs/WEB_PERFORMANCE_BENCH.md#repeated-majadas-intensity-reload--12-10-2026).
- [x] Validar paquete autocontenido en Firefox desde `file://`, con servidor HTTP detenido:
  muestra/sidecar embebidos, selección de clasificación mediante teclado y export de 10 s
  pasan; 4.096 vóxeles, cobertura completa. No valida conversión/OPFS ni todos los controles.
- [x] Validar una sola búsqueda del acumulador por punto con `BTreeMap::entry`: misma escena,
  atributos y errores de presupuesto; 85 tests renderer/WASM, formato y Clippy nativo/WASM pasan.
  Reducción mediana de acumulación nativa de 10,1–16,2% en los tres casos medidos.
  [Mediciones y límites](docs/VOXEL_BUILD_BENCH.md#single-accumulator-lookup--13-10-2026).
- [x] Repetir cámara tras presión de caché a 256 MiB hasta una nueva pausa estable, con
  export fresco y contadores nuevos del worker. Belvedere conserva escena y cobertura, pero
  quedan 780 bloques del objetivo pendientes; capacidad y caso a 1 GiB siguen abiertos.
- [x] Reproducir y corregir el panic HZB de Chrome software al abrir la muestra: buffer de
  argumentos indirectos creado sin mapeo; `cull` inicializa sus cinco campos antes del draw.
  Playwright MCP pasa RGB/clasificación, cámara, ajuste y móvil, más siete flujos API;
  test GPU verifica argumentos completos. Capturas Chrome/SwiftShader muestran canvas blanco:
  validación visual Chrome pendiente. 85 tests renderer/WASM, formato y Clippy pasan.
  [Causa y validación](docs/WEB_PERFORMANCE_BENCH.md#chrome-software-webgpu-hzb-buffer-fix--13-10-2026).
- [x] Añadir diagnóstico independiente de presentación y picking: lectura GPU roja, muestra RGB,
  frames directo/indirecto, hit central en coordenadas del dataset, miss fuera del viewport y
  recentrado. Firefox muestra ambos canvas; Chrome/AMD lee y selecciona correctamente pero
  tampoco presenta el clear WebGPU sin Rust. Mantener la salida visual Chrome pendiente;
  no atribuirla a HZB. [Evidencia](docs/WEB_PERFORMANCE_BENCH.md#independent-presentation-and-picking-diagnostic--13-10-2026).
- [x] Repetir observación acotada a 1 GiB con export fresco y auditar el contrato incremental:
  cámara confirma 219 cargas nuevas en 600 s; quedan 207 pendientes, sin evicciones ni pausa.
  Medición de prefijos en Majadas/Albuera; ampliar corpus y medir pico conjunto siguen pendientes;
  no se da por completado el objetivo de 4 px ni se retiene un acumulador todavía.
- [x] Instrumentar prefijos estrictos del recorrido y medir Majadas en Firefox, tres rondas:
  3/98 reconstrucciones elegibles, 1.870 puntos retenibles y 125 añadidos; 3–4 ms de trabajo
  completo candidato frente a 11.128–12.155 ms totales. Cámara: cero oportunidades nuevas.
  86 tests renderer/WASM y Clippy nativo/WASM pasan. No justifica retener acumulador aquí.
  [Contadores y límites](docs/VOXEL_BUILD_BENCH.md#strict-prefix-opportunity-measurement--13-10-2026).
- [x] Ampliar prefijos a Albuera XYZ en tres rondas: 3/233 reconstrucciones, 24–33 ms
  candidatos de 75.280–78.387 ms totales; cámara sin prefijos nuevos. Seis fases completan
  objetivo, 17.085 vóxeles y cero errores. No justifica acumulador persistente en estos casos.
- [x] Ampliar prefijos a Belvedere XYZ, tres rondas a 256 MiB: 4/156 reconstrucciones
  elegibles, 18–21 ms candidatos; ningún prefijo nuevo tras el giro. Las seis fases pausan
  con 782/780 bloques pendientes: no cierra 4 px ni justifica acumulador persistente.
- [x] Añadir corpus EPT `Additive` determinista: 73 nodos, 37.376 puntos; dos generaciones
  idénticas y tres rondas Firefox con cobertura completa. Solo 2/75 prefijos, menos del 0,8%
  de trabajo candidato; cámara sin oportunidades nuevas. Playwright MCP Chrome parcial:
  canvas blanco y panel atrasado; no se da por validada su presentación.
- [ ] Evaluar reutilización incremental adicional solo después de medir el trabajo restante;
  conservar equivalencia, límites y ausencia de solapamientos.
  Albuera RGB pasa 3/3 en Firefox; Belvedere RGB pasa 3/3 a 8 px, pero 4 px sigue detenido por
  presión de caché. Faltan otros navegadores, componentes adicionales y memoria. Usar `refinement`
  para recarga y mantener `load` como apertura inicial.
  Reproducir el fallo del tercer intento y obtener tres rondas válidas comparables por celda,
  contabilizando fallos. Mantener presupuestos, escena visible y validación de pareja/color.

### Orden de trabajo y aceptación

1. [ ] **Base v5 — conversor, lector y host web.** Muestras pequeñas, Belvedere y Albuera/Majadas;
   al menos tres ejecuciones comparables por caso, revisión, opciones, perfil, hardware y estado de
   caché declarados. Registrar mediana y dispersión; separar apertura, conversión, validación,
   voxelización y subida GPU. Medir RSS pico, temporales máximos y acumulados, bytes leídos/escritos
   y amplificación de I/O; desglosar `.oct` + `.octattrs`, índice y payloads sin contar el índice dos
   veces. En navegador: primera vista, LOD objetivo, latencias de bloque p50/p95, puntos/vóxeles/bloques,
   ocupación de caché y rechazos por presupuesto. Distinguir CPU nativa, heap JS/WASM y buffers GPU,
   declarando qué memoria no se puede observar. Cierre: resultados reproducibles y cuello de botella
   identificado; las métricas todavía no instrumentadas son parte de esta tarea.
   Avance: perfilador nativo Linux `convert-profile` y muestreo externo de RSS/temporales;
   tres conversiones por dataset con tiempos por fase de Belvedere, Majadas y Albuera en
   [mediciones v5](docs/V5_MEMORY_BENCH.md#perfil-de-conversión-nativa-por-fase).
   Atribución de RSS y temporales por intervalo implementada; medición repetida en Belvedere
   en [mediciones v5](docs/V5_MEMORY_BENCH.md#rss-y-temporales-por-intervalo-de-conversión--12-10-2026).
   Albuera/Majadas por fase medidas con tres repeticiones; matriz de workers 1/2 completa para
   los tres datasets; matriz web parcial. PSS externo Firefox medido en tres cargas; heap JS y VRAM
   siguen sin lectura directa.
   El benchmark ya informa lote efectivo de preparación, límite de threads Rayon, workers de
   validación y estimaciones por worker; contraste 1/2 completado en Belvedere, Majadas y
   Albuera. La matriz web continúa parcial; queda validar más navegadores y medir memoria GPU/heap JS.
   Base pequeña Firefox y exploración grande con intentos fallidos documentadas; los cuellos
   internos CPU/I/O siguen sin aislar. El refinamiento web ya expone selección, planificación,
   carga conjunta, lectura/copia de rangos, residual de decode/otras tareas, voxelización y subida
   CPU. Albuera XYZ/RGB/intensidad, Majadas RGB/intensidad y Belvedere RGB a 8 px tienen tres
   rondas válidas; Albuera intensidad registra un timeout BiDi en cuatro intentos. Falta cerrar Belvedere a 4 px, medir más componentes/datasets y otros navegadores. PSS externo
   del árbol Firefox en tres cargas fue **0,98/6,58/0,99 GiB**; dos cargas nuevas no reprodujeron
   el pico. Albuera RGB: objetivo
   mediano **139,6 s [137,6–142,5]**, primer frame **110 ms [72–114]**, 17.030 vóxeles y cobertura
   completa; ~60 respuestas/s de worker. Heap JS y memoria GPU no están disponibles en Firefox.
   [Albuera RGB](docs/WEB_PERFORMANCE_BENCH.md#albuera-rgb-sidecar--12-10-2026),
   [Albuera intensidad](docs/WEB_PERFORMANCE_BENCH.md#albuera-intensity-column-reload--12-10-2026) y
   [Belvedere RGB a 8 px](docs/WEB_PERFORMANCE_BENCH.md#belvedere-rgb-at-8-px--12-10-2026).
2. [ ] **RAM y consultas — `viewer_core`, `viewer_format`, `viewer_render_wgpu`, `viewer_wasm`.**
   Definir presupuestos en bytes para acumulación CPU, caché y recursos GPU, con respuesta explícita
   al agotarlos y medición del pico conjunto. Priorizar lookup de `BlockId` y buffers reutilizables.
   Si el perfil lo justifica, comparar mapa actual con vector ordenado/reducción o claves compactas;
   índice residente plano; cola de prioridad LOD con el mismo desempate; cachés de geometría/atributo
   separadas. Lookup y reutilización acotada de rangos globales ya implementados. Cierre: menor coste
   medido, mismos resultados deterministas,
   cobertura `Replacement`/`Additive`, sin solapamientos ni crecimiento de memoria sin control.
   Tres cargas Albuera medidas con PSS externo del árbol Firefox: **0,98/6,58/0,99 GiB**;
   una exportación mostró un pico transitorio de 6,58 GiB en el proceso principal. Dos cargas
   frescas posteriores quedaron en **0,98/0,99 GiB** y no cruzaron el umbral de captura de VMA.
   La causa sigue sin aislar; GPU VRAM/heap JS siguen fuera de la medida.
   [Muestreo y límites](docs/WEB_PERFORMANCE_BENCH.md#external-firefox-process-tree-pss--12-10-2026).
   Descubrimiento reproducido: con un padre protegido y espacio para un solo hijo, las
   inserciones alternan hijos pendientes sin rechazos. Prueba de regresión añadida;
   `cacheActivity` y `refinementWork.evictedPendingBlocks` permiten medirlo en navegador.
   Belvedere confirma el ciclo. El detector Brent de un checkpoint pausa tras repetir estado
   LRU con vista/corte constantes y conserva la escena. Firefox reanuda tras cambiar detalle:
   cobertura completa a 8 px en 48,6 s. También reanuda tras orbitar la cámara: 782→68 pendientes
   en la captura temprana. Una repetición prolongada con cinco órbitas cargó 727 bloques más, pero
   volvió a pausar en cada ciclo: quedaron 805 pendientes y `targetLodMs` nulo. `coverageComplete`
   describe el último corte visible retenido, no el objetivo solicitado. Microbenchmark nativo
   release con 500 bloques: comprobación coincidente **4,45 μs**, discrepancia de recorrido completo
   **8,03 μs** y creación del checkpoint **8,89 μs**; no incluye navegador ni actualización LOD.
   Un probe sin movimiento a 512 MiB llegó a 765 bloques cargados y pausó a los 480 s con 559
   pendientes, 211 evicciones (98 objetivos); duplicar 256 MiB retrasó la presión pero no completó
   4 px. A 1 GiB, 900 s dejaron 357 pendientes sin evicciones (732 MiB residentes); `voxelBuildMs`
   acumuló 837 s. El export inmediato tras orbitar quedó atrasado respecto a las estadísticas DOM
   (corte nuevo a 4 px, 368 pendientes) y la ventana de 10 s fue demasiado corta para concluir.
   Perfil nativo cerrado: acumulación mediana **12,74 s** a 4 px frente a **5,52 ms** de
   finalización; a 8 px, **1,435 s / 1,298 ms**. El corte ajustado de 4 px suma 1.004 bloques,
   74,19 M puntos y al menos **1.132 MiB** decodificados, por encima de 1 GiB. El banco aplica ya
   la escala visual del archivo (antes normalizada a 1); conserva cortes y checksums de Belvedere.
   No mide el working set progresivo ni cierra la observación de cámara en Firefox.
   [Fases, capacidad y reproducción](docs/VOXEL_BUILD_BENCH.md#belvedere-phase-and-capacity-diagnostic--13-10-2026).
   Acumulador con una sola búsqueda por punto validado: tres parejas alternadas, nueve muestras
   por versión/caso, misma escena y presupuestos. Acumulación nativa mediana **12,739→11,454 s**
   en Belvedere 4 px (**−10,1%**), **1,439→1,239 s** a 8 px (**−13,9%**) y
   **0,342→0,287 s** en Majadas (**−16,2%**); no demuestra mejora WASM ni menor RSS.
   [Comparación y serie excluida](docs/VOXEL_BUILD_BENCH.md#single-accumulator-lookup--13-10-2026).
   Cámara repetida a 256 MiB con export fresco: pausa inicial a 120,2 s, 782 pendientes;
   `ArrowRight` provoca 179 cargas adicionales y nueva pausa a 50,1 s, 780 pendientes.
   Ambos estados finales coinciden con el panel y no hay errores de consola; objetivo incompleto.
   Probe BiDi reproducible añadido, con 120 s sin progreso y cap de 600 s por fase.
   [Observación y límites](docs/WEB_PERFORMANCE_BENCH.md#fresh-export-camera-recovery-at-256-mib--13-10-2026).
   Caso de 1 GiB repetido con dos ventanas de 600 s: 796→1.015 cargas y 429→207 pendientes
   después del giro; 658→890 MiB residentes, sin evicciones ni pausa, objetivo incompleto.
   Exports frescos y actividad nueva confirmados; el panel puede ir atrasado. Hubo checks/Chrome
   simultáneos: no es comparación de tiempos ni cierre de capacidad.
   [Observación acotada](docs/WEB_PERFORMANCE_BENCH.md#fresh-1-gib-camera-observation-after-lookup-change--13-10-2026).
   Diagnóstico nativo conjunto añadido: payload decodificado + modelo del constructor +
   instancias de la primera escena retenida; RSS por límites de construcción y HWM de proceso
   Linux separados. Tres procesos × tres builds por caso conservan checksums: Majadas suma
   estimada **147,3 MB**, HWM **134,9–135,2 MB**; Belvedere 8 px **246,6 MB** y
   **225,1–225,4 MB**. Son bytes decimales; reservas estimadas no equivalen a RSS y el HWM
   incluye lectura/decode. [Alcance y reproducción](docs/VOXEL_BUILD_BENCH.md#joint-native-construction-memory-diagnostic--13-10-2026).
   Muestreador Linux reproducible por PID añadido; tres perfiles Firefox nuevos de Majadas XYZ
   completan carga y cámara con cero errores. PSS pico muestreado **1.001,6 / 1.849,2 / 985,0 MB**;
   el exceso de la segunda ronda se concentra en el padre. El panel queda atrasado en los seis
   exports; no son ventanas FPS ni latencia independiente de cámara. Nueve tests Python y Ruff
   pasan. [Protocolo y límites](docs/WEB_PERFORMANCE_BENCH.md#reproducible-progressive-process-tree-memory--13-10-2026).
   Contabilidad de recursos GPU solicitados implementada en renderer y `gpuStats()` WASM:
   buffers actuales por categoría y texturas lógicas; tráfico acumulado sigue separado. Firefox
   pasa muestra RGB, picking y resize: **2.269.956→686.576 B** al reducir viewport, sin cambiar
   capacidad de instancias. 86 tests Rust, 26 Node y 9 Python pasan; Clippy nativo/WASM y rebuild
   release/paquete pasan. Playwright MCP Chrome falla también en clear independiente por pérdida
   de dispositivo: no se valida Chrome ni VRAM física. [Contrato y evidencia](docs/WEB_PERFORMANCE_BENCH.md#requested-gpu-resources-and-resize-accounting--13-10-2026).
   Corregido panel/export GPU atrasados al parar: progreso LOD incluye las estadísticas actuales
   del mismo worker y actualiza ambos consumidores sin contar progreso como frames. Regresión
   Node roja/verde y Playwright MCP del host con worker determinista pasan en escritorio/móvil;
   Firefox Majadas real completa tres cargas/cámaras con seis paneles/export coincidentes.
   Recursos GPU solicitados finales **15.153.112 / 15.153.108 B** antes/después de cámara;
   PSS pico de proceso **967,5 / 1.028,7 / 1.889,1 MB**. 27 tests Node y 9 Python pasan;
   paquete regenerado. [Reproducción y límites](docs/WEB_PERFORMANCE_BENCH.md#progress-only-panel-and-gpu-export-synchronization--13-10-2026).
   Siguiente: ampliar capturas de recursos GPU a cortes mayores, separar memoria JS/WASM,
   capturar picos transitorios y ampliar corpus Additive real antes de diseñar reutilización.
   Belvedere: tres rondas, 4/156 prefijos y menos de 0,03% de trabajo candidato; corpus
   Additive sintético: tres rondas, 2/75 y menos del 0,8%. El cierre del objetivo
   a 1 GiB sigue pendiente. Falta medir el efecto WASM/end-to-end del detector con pares comparables.
   Auditoría local incremental cerrada: comparar secuencia real de bloques, conservar orden de
   clasificación/sumas y descartar en reemplazos o cambios de contexto; la invalidación de payload
   actual ya existe. Majadas completa tres rondas con solo 3/98 prefijos y 3–4 ms candidatos;
   Albuera también completa tres rondas con 3/233 y 24–33 ms; ningún giro añade prefijos.
   Ampliar corpus y medir pico conjunto acumulador/instancias/escena
   anterior antes de introducir estado persistente; el caso medido no justifica ese prototipo.
   [Contrato y subtareas](docs/VOXEL_BUILD_BENCH.md#progressive-accumulator-reuse-local-contract-audit--13-10-2026).
3. [ ] **Picos de conversión — `viewer_convert` y `viewer_format`.** Reducir vectores intermedios
   y medir el lote de compresión completo. Comparar primero con el spool actual; solo después probar
   spool ordenado espacialmente, particiones acotadas o dos pasadas sobre LAZ. Cierre: salida
   semánticamente equivalente, RAM/temporales/I/O y tiempo por fase comparados, recuperación intacta.
   Avance: liberación de spools consumidos comparada en Belvedere y ronda posterior por fase
   en Albuera/Majadas; tres pares 1/2 workers Belvedere medidos. Los tres pares 1/2 por dataset
   ya están medidos: dos workers aumentan RSS en los tres; el tiempo mediano baja en
   Belvedere/Albuera y sube en Majadas. En Albuera: **604,5/560,7 s**,
   pero RSS **100/213,7 MiB** y dispersión amplia. Salidas validadas e idénticas por hashes.
   No cambiar el valor predeterminado. RSS por fase, incluida validación, ya se observó externamente;
   falta aislar internamente jerarquía, spools y buffers de validación.
   [Majadas](docs/V5_MEMORY_BENCH.md#uno-frente-a-dos-workers-en-majadas--12-10-2026) ·
   [Albuera](docs/V5_MEMORY_BENCH.md#uno-frente-a-dos-workers-en-albuera--12-10-2026).
   Separar el coste de validación; no sustituir la comprobación profunda por CRC sin demostrar qué
   corrupción, errores de descompresión y cuantización detectaría cada modo.
4. [ ] **Representaciones experimentales — formato y render.** Un cambio por experimento:
   `u16` con fallback, delta/shuffle u orden Morton alineado con todas las columnas, y codecs por
   payload. Medir tamaño combinado, error local/mundo desde fuente, CPU, memoria temporal y lectura
   aleatoria p50/p95 sobre grillas, planos, líneas, clusters y outliers. En GPU, evaluar compactar
   `InstanceRaw` (40 B actuales) por modo conservando color/sombreado; medir buffers/draws por bloque
   en la API que los usa antes de proponer una arena. No extrapolar ese coste a toda la ruta WASM.
5. [ ] **Evolución estructural, condicionada a resultados.** Diseñar una nueva versión solo si
   hacen falta cambios persistentes: índice paginado, subchunks de atributos para consultas por fila,
   anchos de coordenadas/codec nuevos o metadatos reducidos. Compactar estructuras residentes no
   exige por sí solo cambiar v5. Evaluar derivar RGB/bounds o reducir rangos únicamente conservando
   sus consumidores y jerarquías arbitrarias. Cierre: beneficio medido, especificación y política
   de compatibilidad acordadas antes de implementar; V6 no es una decisión tomada.
6. [ ] **Evaluar portar el renderizador a Bevy — prioridad baja.** Comparar un prototipo acotado
   con `viewer_render_wgpu` antes de decidir la migración. Condición de avance: reducir complejidad
   y código propio de cámara, escena e interacción, sin duplicar motores ni perder presupuestos,
   streaming/LOD, grilla no solapada o soporte WebGPU en Chrome/Firefox. Medir también tamaño WASM,
   arranque, memoria y rendimiento con la misma nube/cámara/calidad. Facilitar como objetivos futuros:
   varias nubes simultáneas con marcos de coordenadas explícitos; mediciones con unidades y precisión
   conocidas; puntos etiquetados con información persistente y navegación directa. Cierre de la
   evaluación: evidencia y decisión documentadas; portar solo si el balance de simplicidad y
   extensibilidad resulta favorable. Estas funciones y la migración todavía no están implementadas.
7. [ ] **Estudiar metadatos compactos por vóxel, compresión y transiciones LOD.** Evaluar la
   propuesta de un byte: 1 bit para hijos iguales, 3 para el número de hijos directos y 4 para
   posiciones ocupadas/vacías. Definir qué significa «iguales» (geometría y atributos) y comprobar
   su capacidad: 4 bits no representan todas las máscaras de 8 posiciones y 3 bits requieren una
   convención adicional para distinguir los recuentos de 0 a 8; considerar también divisiones 4³/8³.
   Comparar alternativas como máscaras, valores heredados y eliminación de subárboles redundantes,
   además de mecanismos que suavicen transiciones LOD conservando vóxeles enteros sin solapamiento.
   Cierre: viabilidad documentada y comparación de tamaño neto comprimido (incluidos metadatos),
   CPU, memoria, lectura selectiva y calidad de transición; no asumir ahorro por añadir el byte.
8. [ ] **Revisar abstracciones y propiedad en Rust.** Identificar lugares concretos donde `enum`,
   `Box`, `dyn Trait`, genéricos, `Arc`, `Rc` o préstamos puedan mejorar rendimiento, acoplamiento
   y mantenibilidad. Revisar despacho estático/dinámico, asignaciones, copias, propiedad compartida
   y requisitos `Send`/`Sync` en nativo y WASM; comparar con la solución actual antes de cambiarla.
   Cierre: propuestas localizadas con beneficio justificado, mediciones cuando se alegue rendimiento
   y checks pertinentes; priorizar simplicidad y buenas prácticas, evitando abstracciones sin uso.

Mantener IDs y offsets públicos `u64`; usar ordinales menores solo internamente y con límites
validados. Una máscara de ocho hijos sirve para 2³, no para las divisiones 4³/8³ (hasta 512 hijos).
Morton y máscaras son candidatos sobre coordenadas enteras; no reutilizar mantisas, NaN, bits de
atributos válidos ni campos reservados de v5. Evaluar retornos/flags LAS y ángulo escalado como
preservación de datos con round-trip y esquema explícito, no como bits libres. Reutilizar jerarquía
COPC o ampliar EPT queda condicionado al perfil de importación y al alcance de interoperabilidad.

## Próximos criterios de cierre

El seguimiento activo se mantiene en este README; no se crean Issues/PRs por defecto.

- [ ] **Navegador:** apertura gris, pareja y componentes comprobados en Firefox; paquete `file://`
  y teclado del selector también. Chrome 155 con SwiftShader pasa muestra, clasificación, cámara
  y viewport móvil a nivel funcional. Picking WASM pequeño pasa en Chrome/Firefox; interacción
  CSS/API y recuperación de GPU siguen pendientes. Clear WebGPU sin Rust también queda invisible
  en Chrome/AMD pese a readback rojo: falta salida visual Chrome, matriz grande/GPU física,
  resto de teclado, corrupción de payloads,
  OPFS/cuota, cancelación durante lectura, interacción/latencia de picking y recuperación tras pérdida de GPU. Los tests Rust/JS y renders nativos no sustituyen esa interacción.
- [ ] **Escala:** medir hasta una fuente mayor de 500 GB decimales, con índice, spools, RSS, recursos
  GPU, bytes leídos, primera vista y navegación separados. Fijar mínimos oficiales RAM/GPU/SO.
- [ ] **Conversión:** matriz de fuentes <10 GB con al menos tres repeticiones y caché declarada;
  objetivos <5 min (ideal <1 min) y suma de ambos archivos ≤3× fuente (ideal <2×), no garantías.
  Aplicar la base v5 y el orden de optimización anterior; comprobar cancelación/recuperación.
- [ ] **Calidad/LOD:** comparar divisiones 2/4/8 con igual presupuesto y revisión visual antes de
  cambiar el valor predeterminado 2. Extender cobertura no solapada a todos los consumidores;
  corpus grilla/plano/línea/clusters/outliers, cámaras, viewport, EPT y adapters diferentes.
- [ ] **Lectura:** medir navegación con bloques retardados; integrar rangos asíncronos y
  tickets/cancelación del scheduler solo si la evidencia lo justifica. El worker cede entre bloques.
- [ ] **Formato y atributos:** resolver los experimentos anteriores con error y tamaño medidos; acordar
  preservación de flags/Extra Bytes/CRS, licencia y política de compatibilidad pública.
- [ ] **Comparación:** Potree con misma fuente, equipo, configuración y calidad; registrar caché
  fría/caliente, versiones y percentiles. Acordar umbrales después de obtener una base reproducible.
- [ ] **Web y extensibilidad:** medir conversión de Albuera, coste de dos pasadas y cuotas OPFS;
  definir alcance de medición, selección, clasificación, recorte/exportación e interoperabilidad.

## Desarrollo

`fast` reduce coste de compilación para tests; `perf` optimiza CPU nativa para benchmarks;
`release` conserva la referencia de tamaño/WASM. Registrar siempre perfil y equipo.

```sh
cargo test-fast --offline
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo clippy -p viewer_wasm --target wasm32-unknown-unknown --offline -- -D warnings
node --test examples/web-host/tests/*.test.mjs
python3 -m unittest discover -s examples/web-host/tests -p 'test_*.py'
cargo test -p viewer_render_wgpu --profile fast --offline -- --ignored
```

La última orden requiere GPU o Vulkan por software. La validación de la implementación v5 anterior
a esta revisión documental cubrió **212 tests Rust**, **2 tests GPU adicionales**, seis suites
Node y tres tests Python, Clippy nativo/WASM, formato y compilación WASM release. Se generó el
paquete local y se renderizó Outer Banks con y sin acompañante a 640×480 (2.381 vóxeles,
12.034 píxeles de contenido; todos grises en el caso sin atributos). No certifica navegación
real en Chrome/Firefox ni escala.
Ver [host web y empaquetado](examples/web-host/README.md), [muestras](data/samples/README.md) y
[corpus de datasets](data/datasets/README.md). El registro histórico conserva referencias a
Potree, COPC, EPT, Arrow, Zarr, LASzip y artículos usados en decisiones anteriores.
