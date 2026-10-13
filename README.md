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
  Avance: `convert-profile` ya informa ventanas monotónicas de planificación espacial, flujo de
  bloques y validación, con límites Unix para correlacionar muestreo externo; separa tiempo de
  preparación por lotes y escritura ordenada/liberación de spools. Un smoke LAS de 111.190 puntos
  completa y el test compara `.oct`/`.octattrs` byte a byte con el perfilador activado/desactivado.
  Avance: tiempos internos agregados de fill LAS directo, lectura de rangos del spool,
  clasificación/particionado/muestreo y escritura LOD; el perfilador los incluye cuando están
  disponibles. Son duraciones de pared sumadas y las ramas paralelas pueden solaparse. Solo `fill`
  directo está incluido: la creación inicial del spool fuente y checkpoints quedan fuera. No son
  CPU exclusiva ni I/O físico. Sigue pendiente medir ingestión de spool, pico del ciclo completo de
  compresión y memoria/RSS de readers y buffers de validación. La RSS y el tamaño temporal
  continúan siendo métricas del proceso, no de
  una estructura individual. [Contrato y límites](docs/V5_MEMORY_BENCH.md#ventanas-internas-de-conversión--13-10-2026).

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
   Diagnóstico interno añadido: cada worker de validación agrega bloques y puntos leídos, tiempo
   acumulado dentro de `read_block` y máxima estimación de memoria del bloque decodificado. No son
   RSS ni incluyen buffers transitorios; al no activar `convert-profile` no se calcula la estimación.
   La planificación ahora expone bytes lógicos escritos en spools source, child y LOD, más bytes
   de registros leídos en escaneos recursivos source/child. El stream cuenta aparte el payload
   lógico leído de bloques fuente y LOD; no mide lectura anticipada, syscalls ni I/O físico. Queda
   El perfilador mide ahora el máximo de bloques preparados retenidos por lote y la capacidad de
   sus buffers de payload comprimido, individual y conjunta. No incluye vectores de metadatos,
   bloques de entrada ni buffers raw temporales de compresión; quedan pendientes el pico completo
   del lote y tiempos separados de ingestión, clasificación y escritura LOD.
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
   Albuera XYZ: tres cargas/cámaras Firefox completan con panel/export coincidentes y 17.085
   vóxeles; recursos GPU solicitados **15.623.424 B** en seis exports finales. PSS pico muestreado
   **1.009,1 / 3.823,2 / 1.005,7 MB**. Captura opcional de mapas por umbral añadida y probada:
   segunda ronda captura a 6,078 s, con **161 mapas anónimos de 1280×720×4 B**; el pico posterior
   no está capturado. Once tests Python, Ruff y smoke CLI pasan. No atribuye propietario ni VRAM.
   [Mapas, protocolo y límites](docs/WEB_PERFORMANCE_BENCH.md#albuera-gpu-snapshots-and-transient-parent-mapping-capture--13-10-2026).
   Clear WebGPU independiente añadido y medido: tres procesos por tamaño, 300 frames + 20 s
   de reposo, CSS constante. A 1280×720 quedan **3/4/5** mapas anónimos de tamaño de imagen;
   a 640×360 no hay mapas del tamaño exacto filtrado. No reproduce los **161** mapas de Albuera
   ni prueba quién los asigna. Firefox completa seis secuencias; Playwright MCP verifica errores
   de parámetros en escritorio/móvil, sin validar GPU Chrome. [Matriz y alcance](docs/WEB_PERFORMANCE_BENCH.md#independent-clear-only-surface-memory-matrix--13-10-2026).
   Modo WASM de muestra fija añadido: seis secuencias Firefox de 300 frames + 20 s de reposo
   con 4.096 vóxeles y cola GPU completada. A 1280×720 hay **5/6/6** mapas de tamaño de imagen;
   no reproduce los 161 mapas/pico multi-GB de Albuera. Una ronda 640×360 conserva **646,7 MB**
   de PSS padre sin mapas de la clase exacta: tamaño de imagen no explica el total. 27 tests Node,
   11 Python y smoke de ambos modos pasan; MCP comprueba modo inválido en escritorio/móvil.
   [Matriz y límites](docs/WEB_PERFORMANCE_BENCH.md#fixed-small-wasm-sample-surface-matrix--13-10-2026).
   Órbita con muestra pequeña medida en seis perfiles: 300 giros producen **903 builds** y
   cero lecturas adicionales; las capturas de mapas se serializan antes de la pantalla. No aparece
   la clase de 161 mapas de Albuera y la PSS sube durante el reposo por causas no atribuidas.
   Corte objetivo residente en cámara: publicar a 4 px solo si no faltan bloques ni hay regiones
   descubiertas/omitidas; conserva el refinamiento progresivo como fallback. Tres rondas Firefox
   de 300 giros mantienen 4.096 vóxeles completos y bajan de **903 a 303 builds**; mediana de
   voxelización **20.617→9.253 ms**, subidas GPU acumuladas **147.115.800→49.643.520 B**, recursos
   GPU solicitados estables en **15.051.868 B**. Comparación indicativa, sin FPS ni memoria
   residente. La carga inicial prueba el camino progresivo sin residencia. Una regresión con
   planes Replacement/Additive verifica elegibilidad multibloque, bloques pendientes y omisiones
   de presupuesto; el fallo real por máximo de vóxeles y su fallback ya están comprobados en la fase 11.
   [Contrato, resultados y límites](docs/WEB_PERFORMANCE_BENCH.md#resident-target-cut-on-camera-invalidation--13-10-2026).
   Majadas multibloque (415,5 M puntos): con 239 bloques/112,3 MB en caché, un arrastre de cámara
   conserva cobertura 4 px de 202 bloques y 6.397 vóxeles, sin nuevas lecturas ni evicciones;
   builds **98→100**. Una ronda funcional, no benchmark FPS. El probe BiDi mantiene render activo
   hasta exportar estadísticas worker actuales.
   [Evidencia y alcance](docs/WEB_PERFORMANCE_BENCH.md#resident-target-cut-on-camera-invalidation--13-10-2026).
   Fallback por máximo de vóxeles cerrado con renderer real (fase 11). Siguiente: repetir cámara
   multibloque con comparador controlado, separar memoria JS/WASM y ampliar corpus Additive real
   antes de diseñar reutilización persistente.
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
   Plan 13-10-2026 — diagnóstico nativo del pico conjunto (`viewer_render_wgpu`):
   - [x] Añadir al benchmark un intervalo RSS opcional (1–1000 ms); sin muestreo por defecto.
     Muestrear durante el build con bloques y escena anterior residentes, detener y unir el hilo
     antes de publicar resultados. Registrar máximo RSS observado, número de muestras e intervalo.
   - [x] Verificar ejecución con/sin muestreo, igualdad de escena y rechazo de intervalos inválidos;
     ejecutar fmt/Clippy y documentar una medición reproducible antes del commit.
   Resultado: fixture pequeña, 2.410 vóxeles y checksum idéntico con intervalos 0/1/1000 ms;
   intervalos inválidos rechazados. Majadas: 202 bloques, 7.015.449 puntos, 6.397 vóxeles;
   RSS observado a 1 ms: 135.254.016 / 135.454.720 / 135.454.720 B en tres builds,
   con 267 / 276 / 272 muestras. Estimación combinada con escena anterior: 147.297.000 B;
   son magnitudes distintas, sin inferir ahorro ni justificar acumulación persistente.
   Compilación optimizada, Clippy del ejemplo, fmt y diff pasan; 60 tests renderer pasan,
   dos opcionales ignorados.
   [Reproducción y alcance](docs/VOXEL_BUILD_BENCH.md#optional-native-rss-sampling--13-10-2026).
   Plan siguiente — capacidad WASM del worker y export de benchmark:
   - [x] Conservar los exports de `init()` y consultar `memory.buffer.byteLength` al emitir
     stats, progreso y `getStats`; releer el buffer tras crecimientos, sin almacenar vistas antiguas.
   - [x] Exportar la última muestra del worker por separado del heap JS del hilo principal.
     Heap JS del worker: desconocido (`null`); no restar WASM a PSS ni sumar métricas solapadas.
   - [x] Verificar crecimiento de memoria con un `WebAssembly.Memory` real en tests JS,
     validar worker real/API/export en Playwright MCP y regenerar el paquete local.
   Resultado: Chrome 155/SwiftShader, 1280×720 y 390×844: export con 2.818.048 B de
   capacidad WASM, 4.096 vóxeles y cobertura completa; API independiente: 1.310.720 B.
   Heap JS del worker desconocido; no es atribución completa ni pico conjunto del navegador.
   28 tests JS y 11 Python pasan; paquete local regenerado y comprobado por `file://`
   con carga, benchmark y export. Sin cambios Rust/bindings.
   [Contrato y reproducción](docs/WEB_PERFORMANCE_BENCH.md#worker-wasm-linear-memory-snapshots--13-10-2026).
   - [ ] Continuar después con atribución de asignaciones JS/WASM y corpus Additive real. El RSS de proceso incluye
     reader, índice y allocator; no atribuye bytes al builder, no mide GPU ni garantiza capturar
     picos menores al intervalo. Las duraciones con muestreo no son comparables con las anteriores.
   - [x] Corregir importación EPT real: aceptar `float` (nombre oficial) conservando el alias
     `floating`. La regresión con ScanAngleRank/GpsTime fallaba antes y pasa tras la corrección;
     41 tests del conversor y Clippy pasan. USGS convierte sin modificar el schema original.
     El bounds extenso requiere `--max-error 0.02` (cota calculada 0,0109732; default 0,01 rechaza).
   Plan — corpus Additive real acotado (`viewer_convert` + navegador):
   - [ ] Preparar tres nodos USGS NC Hurricane Florence con padre presente, LAZ originales
     fijados por SHA-256 y procedencia; conservar bounds/schema, ajustar solo `points` y jerarquía
     a los 8.884 puntos seleccionados. Rechazar directorios existentes y descargas excesivas.
   - [ ] Convertir y validar v5 dos veces; comprobar hashes y semántica Additive preservada.
   - [ ] Medir tres rondas de carga/cámara, prefijos candidatos, trabajo completo y capacidad WASM;
     dejar la muestra marcada como parcial, sin inferir cobertura del dataset original ni ahorros.
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
   Propuesta recibida el 13-10-2026 para compresión de atributos: optimizar lecturas y LOD con una
   representación persistida distinta de la representación expandida/de ejecución. En disco,
   evaluar guardar cada combinación única de atributos en un diccionario del `.octattrs` y que cada
   punto/vóxel del `.oct` refiera su entrada por índice, evitando repetir la tupla completa. Para
   campos de texto, evaluar códigos numéricos y un índice ordenado por campo que permita regenerar
   código→texto (conceptualmente un B-tree exterior por campo y un B-tree interior por valores).
   Sigue siendo una propuesta, no un cambio decidido. Medir cardinalidad y duplicación real de
   tuplas, coste de índices/códigos, tamaño total incluido padding y metadatos, lectura selectiva,
   expansión en memoria y construcción/lectura de LOD; resolver IDs de campo estables, nulos y
   normalización de texto. Mantener compatibilidad v5 explícita y contrastar con el layout actual.
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
   Avance 13-10-2026: el coste objetivo acordado es tamaño persistido. V5 persiste
   jerarquía/bloques y el renderer crea celdas dyádicas `VoxelKey` en memoria;
   `LodTransitionPlanner` transiciona nodos/bloques
   residentes, no máscaras por vóxel. Una máscara completa de ocupación necesita 8/64/512 bits
   para divisiones 2³/4³/8³; el byte propuesto no codifica las posiciones generales. «Iguales» debe
   especificar geometría, RGB y atributos (incluidos intensidad/clasificación cuando existan).
   Prioridad acordada: tamaño persistido del formato. No se implementa codificación ni se infiere
   ahorro: quedan estas subtareas antes de diseñarla: (a) fijar la semántica de igualdad; (b) comparar
   baseline, máscara completa y representación dispersa en corpus/cortes reales, incluyendo índices,
   padding, atributos y metadatos; (c) medir CPU, memoria, lectura selectiva y transiciones, y
   comprobar cobertura y no solapamiento. Evidencia: `viewer_render_wgpu/src/voxel_grid.rs`,
   `viewer_core/src/lod_transition.rs` y `viewer_format/FORMAT_V5.md`.
8. [x] **Refactorizar contratos y propiedad en Rust (primera fase).** Plan ampliado el 13-10-2026 a petición
   del usuario; sustituye la auditoría localizada anterior. Assessment arquitectónico de los cinco
   crates y de sus consumidores nativos/WASM, ejemplos, tests y host web; no es un perfil exhaustivo
   de cada función. Objetivo: intercambiar implementaciones pequeñas sin convertir el workspace en
   un framework ni modificar v5, la API JavaScript o la semántica de vóxeles enteros no solapados.

   | Área | Evaluación y decisión |
   | --- | --- |
   | `viewer_core`: geometría, atributos, jerarquía, LOD, scheduler, caché y voxel store | `DatasetSource` ya separa E/S. `AttributeValues`, `RequestState` y `HierarchySemantics` modelan alternativas cerradas. La lectura de residentes debe depender de un contrato pequeño, mientras la protección/evicción sigue perteneciendo a la caché. |
   | `viewer_format`: reader/writer, codecs, cuantización y layout | Conservar `Read + Seek` / `Write + Seek`, enums de codec/atributos y `Arc` de metadatos/directorios compartidos por `fork_reader`. La segunda fase añade contratos selectivos de reader/writer sobre estas APIs; apertura y transporte siguen concretos. |
   | `viewer_convert`: LAS/LAZ, EPT, planificación espacial, workers y checkpoints | `SpatialInput`, fases y mensajes ya usan enums. Mantener `Arc<AtomicBool>` de cancelación y callbacks prestados. No unificar spools y fuentes detrás de un trait hasta tener operaciones y errores comunes reales. |
   | `viewer_render_wgpu`: escena, grilla, atributos, cámara, buffers y HZB | Separar consulta de bloques de la política LRU en preparación de escena y sincronización GPU. Mantener layouts explícitos de GPU y enums de modos; no introducir `dyn Renderer` con un único backend. |
   | `viewer_wasm`: controller, LOD, Blob, conversión y bindings | `ViewerData` ya evita coexistencia de puntos brutos y store. Mantener metadatos compartidos; migrar consumidores del resultado de caché. Unificar el estado de dataset en bindings es candidato posterior con pruebas de sustitución/cancelación. |
   | Host web, scripts, ejemplos y tests | No son candidatos a traits Rust. Conservar el protocolo JS/worker y adaptar ejemplos/tests que consuman las APIs Rust modificadas. |

   Plan de implementación y cierre:

   - [x] **Contrato de bloques residentes.** Añadir `ResidentBlockSource` con consulta prestada,
     sin E/S, clonación de payload, cambios LRU ni requisitos `Send`/`Sync`. Usar genéricos
     `S: ResidentBlockSource + ?Sized` en preparación LOD, grilla y sincronización GPU: admitir
     implementaciones concretas y `&dyn ResidentBlockSource`. Conservar operaciones mutables
     de protección/evicción en `DecodedBlockCache`. Cierre: misma cobertura, presupuestos y errores
     con caché LRU y proveedor alternativo, también mediante objeto trait.
   - [x] **Resultado de inserción como enum.** Reemplazar `cached: bool` + rechazo opcional por
     variantes de éxito/rechazo que impidan estados contradictorios. Migrar todos los consumidores
     y eliminar el `expect` del rechazo en LOD. Es un cambio de API Rust; documentar accesores.
     Cierre: tests de éxito, oversize, protección, evicción y atomicidad conservados.
     Migración Rust: `result.cached`, `.rejection`, `.evicted_blocks` y `.accounted_bytes`
     pasan a métodos con `()`; para mover el vector sin clonarlo, desestructurar
     `CacheInsertResult::Cached { evicted_blocks, .. }`. `Rejected` contiene motivo y bytes
     intentados, y no puede contener expulsiones. Los consumidores del workspace están migrados.
     Layout medido con rustc 1.99.0 (b940084d7), x86_64, perfil `fast`: estructura anterior
     40 B / alineación 8 B; enum 32 B / alineación 8 B. Reproducir con
     `cargo run -p viewer_core --profile fast --offline --example cache_result_layout`.
     Es el tamaño inline de un resultado temporal, no de las entradas residentes: no demuestra
     reducción de RSS, rendimiento ni un layout estable en otros compiladores o targets.
   - [x] **Validación y commit.** `cargo test-fast --offline`: 248 tests pasan, 3 ignorados
     (benchmarks/pruebas GPU opcionales); fmt, Clippy nativo con `--all-targets -D warnings`,
     Clippy WASM con `-D warnings`, `cargo check-web --offline` y `git diff --check` pasan.
     Cuatro regresiones nuevas comprueban proveedores alternativos en LOD, grilla y planificación
     GPU, y consultas prestadas sin cambios LRU/estadísticas. No se modifica interfaz/protocolo web;
     no se ejecuta navegador ni se afirma una medición de rendimiento end-to-end.
     Cambios agrupados para el commit de esta primera fase.

   Sugerencias condicionadas a evidencia, fuera de esta primera implementación:

   - [ ] **Proyectores genéricos o estrategia de grilla.** Comparar `&dyn Fn` prestado con
     genéricos y una segunda estrategia real usando iguales entradas, cámara, checksum y límites.
     Medir tiempo, tamaño binario/WASM y compilación antes de propagar tipos por todo el builder.
     El contrato actual no asigna un callback en heap.
   - [x] **Estado de dataset en bindings como enum.** Cerrado en fase 10: `Option<LodSession<R>>`
     contiene `External(dataset)` o `Format { dataset, reader }`. El lector no puede existir sin
     su dataset. Las aperturas construyen la respuesta antes del reemplazo; configuración de vista,
     métricas y flags siguen fuera del enum. Regresiones de reemplazo, fallos de apertura y clear
     comprobadas en navegador. No se atribuye ahorro de memoria al enum ni se añade `Box`.
   - [ ] **`Box` y layouts.** Medir `size_of`, alineación, número de instancias y asignaciones
     antes de encajonar variantes grandes de `ViewerData` o estados del converter. Un enum ocupa
     espacio suficiente para su mayor variante; añadir variantes no reduce memoria automáticamente.
     `Box<[BlockId]>` ya es adecuado para IDs finalizados. No encajonar payloads GPU ni puntos
     individuales: la indirección y las asignaciones perjudicarían su localidad. Avance fase 12:
     diagnóstico nativo reproducible para cinco tipos públicos; encajonar el objeto entero añade
     8 B al modelo objeto+handle en x86_64. No extrapolar a variantes/colecciones ni a WASM.
   - [ ] **`Rc` frente a `Arc`.** Mantener `Arc` en metadatos y lectores compartidos por workers
     nativos. Evaluar `Rc<RefCell<BrowserReadStats>>` solo para contadores confinados al worker WASM,
     con medición de instrumentación y comprobación de préstamos/reentrancia; no sustituir todos
     los `Arc` ni añadir sincronización a los nuevos traits.
   - [ ] **Caché de rangos compartida.** Comparar `Arc<Mutex<_>>` con caché por lector bajo
     validación concurrente y lectura de atributos; medir contención, reutilización y memoria total.
     La caché independiente puede duplicar trabajo y memoria.

   Ejemplo de intercambio: implementar `ResidentBlockSource::peek` en un store experimental y
   pasar `&store` a `LodTransitionPlanner::prepare` o `build_voxel_scene_with_limits`; para elegir
   proveedor en ejecución, pasar `&dyn ResidentBlockSource`. El préstamo evita clonar bloques y no
   requiere `Box`, `Rc` ni `Arc`. `commit/update/reset` conservan la caché concreta porque gestionan
   protección. Una segunda política completa de caché necesitará además un contrato de mutación
   con las mismas garantías de presupuesto/atomicidad, antes de sustituirla en `LodDataset`.

   Las sugerencias requieren experimento antes de decidir implementación; no son mejoras de
   rendimiento demostradas ni compromisos de introducir todos estos tipos. Esta fase entrega
   desacoplamiento y estados válidos; cualquier ahorro de layout/tiempo se registra por separado.

9. [x] **Contratos reader/writer del octree (segunda fase, 13-10-2026).** Separar las operaciones
   de consumo del contenedor v5 y de los streams concretos, conservando `DatasetSource` para lectura
   completa y `ResidentBlockSource` para bloques ya decodificados. Alcance solicitado: usar los
   contratos en rutas reales, admitir genéricos/objetos trait y preservar formato, límites y errores.

   - [x] **`OctreeReader: DatasetSource`.** Lecturas selectivas de posiciones, geometría con RGB,
     colores, columnas y filas, disponibilidad de atributos y rangos. `Error` heredado de `DatasetSource`;
     sin `File`, Blob, `Read + Seek`, apertura/fork ni `Send`/`Sync` en el contrato. Implementar para
     `FormatReader<R>` y extraer la carga selectiva de bloques del binding a una función genérica
     comprobable en nativo. Cierre: RGB/intensidad/clasificación/atributo elegido y errores mantienen
     las mismas lecturas; la ruta XYZ no accede al acompañante ni carga columnas completas.
   - [x] **`OctreeWriter`.** Escribir `SourcePointBlock` con XYZ locales `f64`, RGB y columnas,
     errores y salida asociados; cierre consumidor `finish_pair(self)` con `Self: Sized`, conservando
     ambos sinks y el orden de cierre. Apertura y publicación/checkpoints pertenecen al caller.
     Implementar para `FormatWriter<W>` y usar en conversión LAS/EPT/OPFS. Cierre: igual v5 y fallos
     de bloque duplicado, ausente, inválido o stream fallido; no perder `.octattrs` al finalizar.
   - [x] **`PreparedOctreeWriter: OctreeWriter`.** Capacidad opcional para aceptar un tipo asociado
     de bloque preparado; usarla en `stream_blocks` para conservar codificación paralela y append
     ordenado sin exigir `File` como writer. Esa ruta sigue preparando v5 explícitamente; cambiar
     codec/formato requerirá también otra preparación compatible. No forzar esa capacidad sobre
     todos los writers ni mover la codificación al hilo secuencial.
   - [x] **Validar y cerrar.** Pruebas de round-trip y bytes equivalentes mediante traits, proveedor
     alternativo y `dyn`, fallos y lecturas selectivas; workspace, fmt, Clippy nativo/WASM, check-web,
     diff y commit. Resultado: 254 tests pasan, 3 opcionales ignorados; fmt, Clippy nativo/WASM,
     check-web y diff correctos. Prueba de selección v5 instrumentada a través de `dyn OctreeReader`;
     round-trip/salidas simple y split, bytes equivalentes y fallos mediante `OctreeWriter`.
     WASM release/bindings y paquete local regenerados. Playwright MCP, Chrome 155/SwiftShader:
     muestra con RGB/intensidad/clasificación/atributo elegido y conversión OPFS del bunny
     (40.256 puntos), seguida de apertura de ambos archivos, pasan sin errores JavaScript.
     La sesión MCP predeterminada no ofrecía adapter GPU: la prueba válida usa Chrome con
     SwiftShader explícito. El host informa 404 de favicon; no se cambia interfaz/protocolo JS.
     Firefox, GPU física y comparación de rendimiento quedan sin verificar en esta fase.

   Contratos: [`contracts.rs`](crates/viewer_format/src/contracts.rs). Consumidores reales:
   `viewer_convert::stream_blocks`, conversión EPT/OPFS y
   [`block_loading.rs`](crates/viewer_wasm/src/block_loading.rs). Ejemplo de intercambio:
   `fn load<R: OctreeReader + ?Sized>(reader: &mut R, id: BlockId) -> Result<PointBatch, R::Error>`
   puede llamar `reader.read_positions(id)` tanto con una struct como con `&mut dyn OctreeReader`.
   En writer usar `W: OctreeWriter + ?Sized` para append y `OctreeWriter::finish_pair(writer)`
   con propiedad concreta para finalizar; el contrato no finaliza un `dyn` prestado. Las fábricas
   de LAS/EPT/OPFS siguen construyendo `FormatWriter`: intercambiar el formato completo en la CLI
   requeriría también fábrica, preparación y validación compatibles. No se añade selección de formato
   ni despacho dinámico obligatorio. No hay cambio de API Rust existente ni del formato v5.

   **Decisión sobre el parser:** no introducir un trait que mezcle LAS → jerarquía → archivo.
   `las::Reader` 0.11.1 ya delega decodificación y rellena `PointData` reutilizable; el constructor
   espacial conserva enteros/transformaciones LAS y usa spools, mientras EPT agrega múltiples LAS
   y OPFS tiene su propio ciclo. Un wrapper `header/fill_points` sin un punto público de inyección
   apenas permitiría experimentar. Diseño futuro acotado: fuente de lotes con esquema/marco,
   precisión y EOF/error definidos, más una fábrica reabrible para segundas pasadas; el builder
   produce jerarquía/bloques y el writer persiste. Antes de implementarlo, elegir una segunda fuente
   real (decoder alternativo, COPC por chunks o fixture inyectable) y comprobar enteros exactos,
   atributos, reutilización del buffer, límites, cancelación y reapertura/checkpoints. No convertir
   todo a `Point` f32 ni prometer mejoras de LAZ por añadir un trait.

10. [x] **Despejar genéricos en fronteras de coste grueso (tercera fase, 13-10-2026).**
    Revisar cada llamada por su frecuencia, no por la presencia de un `for`: una consulta por bloque
    seguida de decodificación/voxelización tiene un coste distinto de un callback por punto.

    | Contrato/ruta | Decisión |
    | --- | --- |
    | Progreso del conversor | Mantener wrappers públicos genéricos compatibles; `&mut dyn FnMut` en las funciones internas grandes, EPT y planificación espacial. Sin asignar `Box` ni añadir sincronización. |
    | `OctreeReader` en carga de render | `&mut dyn OctreeReader<Error = E>`; conservar genérico solo el error `E: Display`, no cada backend. Llamadas por payload/bloque, no por punto. |
    | `ResidentBlockSource` en grilla/sync GPU | `&dyn ResidentBlockSource` en consumidores por bloque; la iteración por puntos continúa concreta. |
    | `ResidentBlockSource` en resolución LOD | Conservar genéricos: `contains/peek` se invocan repetidamente en recorridos y son consultas pequeñas; comparar antes de borrar ese tipo. |
    | `OctreeWriter` / `PreparedOctreeWriter` | Finalización por valor concreta; `stream_blocks` conserva `W`: borrar el backend exigiría fijar `Output` o parametrizarlo y seguiría monomorfizando por salida. Hoy solo existe `FormatWriter<W>` en producción. No añadir un parámetro sustituto sin beneficio concreto. |
    | `Read + Seek` / `Write + Seek`, visitantes por punto, selección LOD | Conservar genéricos en E/S de payloads y callbacks finos. Los proyectores de la grilla ya son `&dyn Fn`; su comparación pendiente sigue abierta. |

    - [x] Aplicar las fronteras anteriores y conservar pruebas de fuentes alternativas y errores.
    - [x] Comparar reconstrucción nativa y tamaño con baseline `ac7569a`, registrar método y límites;
      no inferir menos assembly, compilación o más FPS únicamente por usar `dyn`.
    - [x] Continuar pendiente 8: sesión LOD con enum `External(dataset)` / `Format { dataset, reader }`
      dentro de `Option`. Expresar los tres estados válidos (ninguno/externo/archivo), impedir reader
      huérfano y reunir destrucción/reemplazo. Construir respuesta de apertura antes del reemplazo.
      Preservar selección, gris, carga manual, fallos de apertura y reinicio del renderer.
    - [x] Validación nativa/WASM, regresión navegador de transiciones y atributos; README y commits
      por cambio coherente antes de avanzar. No implementar simultáneamente parser, compresión,
      nuevo renderer o tipos de propiedad sin un consumidor/experimento definido.

    Evidencia local del despacho, antes del cambio de sesión: baseline `ac7569a`, rustc 1.99.0,
    perfil `fast`, `CARGO_INCREMENTAL=0`, dependencias calientes, tres reconstrucciones forzadas
    de `viewer_convert` tocando solo `src/lib.rs`; grupos antes/después secuenciales, no alternados.
    Mediana/rango: antes 1,101 s [0,998–1,408], después 1,323 s [1,247–1,399]. **No demuestra
    mejora de compilación** ni mide un build limpio del workspace. Release nativo: 1.610.656 →
    1.606.600 B; WASM release: 2.573.845 → 2.573.281 B. Son bytes de archivo, no recuentos de
    instrucciones, RSS o beneficio de caché demostrado.
    Bunny caliente, `--points-per-block 4096 --spatial-workers 1`, tres pares alternando orden:
    antes 55,3 ms [36,2–63,3], después 58,3 ms [53,7–59,3]. Ruido/entrada pequeña: no acredita
    aceleración. Las seis parejas `.oct/.octattrs` tienen hashes idénticos entre versiones:
    `.oct` SHA-256 `636fac86998d04f48bd8bafc594229a3d6f682d061d2809fe75bb4eb635e02ae`;
    `.octattrs` SHA-256 `c18c24a5affa1dc3e6d3a90f48846d66e6185addcddf16257d66595e4e1e39a3`.
    Entrada SHA-256 `6b89d660f4893613f3605ce3a3d22f6354def6defe02d61d0c2b2bb3c02beafb`.
    Comandos: `CARGO_INCREMENTAL=0 cargo build --profile fast --offline -p viewer_convert`,
    `cargo build --release --offline -p viewer_convert` y build release de `viewer_wasm` para
    `wasm32-unknown-unknown`. Logs/JSON/binarios locales: `artifacts/dispatch-refactor/`.
    Despacho validado: 254 tests pasan, 3 opcionales ignorados; fmt, Clippy nativo/WASM,
    check-web y diff correctos.
    Se conserva el cambio por fronteras explícitas y menor duplicación potencial; no por una
    mejora de tiempo medida. Las llamadas normales siguen siendo compatibles; los consumidores
    render ya no aceptan turbofish sobre el tipo de fuente, mientras `prepare` LOD sigue genérico.

    Sesión implementada en [`lod_session.rs`](crates/viewer_wasm/src/lod_session.rs); commit del
    despacho `a189a63`. El cierre de la sesión usa un único `take` y los préstamos de dataset/reader
    proceden de la misma variante. Prueba nativa de liberación del lector al sustituir/limpiar y de
    inserción externa; regresión persistente
    [`lod-session.browser.mjs`](examples/web-host/tests/lod-session.browser.mjs) para ausencia,
    externa, pareja, fallo de coordenadas/acompañante/descriptor y fallo inyectado de respuesta JS,
    cambio a externa, gris, clear, puntos manuales y reapertura.
    Cierre final: 255 tests Rust pasan, 3 opcionales ignorados; 27 tests JS pasan; fmt, Clippy
    nativo/WASM, check-web, sintaxis del probe y diff correctos. WASM/bindings y paquete local
    regenerados. Playwright MCP, Chrome 155.0.8059.39 con SwiftShader: probes de sesión, atributos
    (RGB/intensidad/clasificación/columna y acompañante válido incompatible) y reutilización LOD
    (cámara, viewport, escena/payload/reapertura) pasan. Sin errores JavaScript; un primer arranque
    informó el 404 ya conocido del favicon. Pruebas de backend con OffscreenCanvas, no una auditoría
    visual ni un benchmark de GPU física/Firefox/latencia. No se cambia el protocolo JS ni el formato.

    Investigación adicional: el estado de dataset es una alternativa cerrada y encaja en un enum.
    Un trait de parser sigue necesitando fuente real/reapertura (pendiente 9); un trait de renderer
    o política de caché completa requiere segundo consumidor y semántica de mutación/presupuesto.
    `Rc` para métricas y caché de rangos por lector permanecen experimentos condicionados a perfil.
    Son candidatos, no compromisos ni ahorros demostrados.

    Fuentes: [trait objects y vtables (Rust Reference)](https://doc.rust-lang.org/reference/types/trait-object.html),
    [despacho estático/dinámico (Rust Book)](https://doc.rust-lang.org/book/ch18-02-trait-objects.html) y
    [medición por unidades de compilación (Cargo Book)](https://doc.rust-lang.org/cargo/reference/timings.html).
    La vtable añade indirección y puede limitar inline; la monomorfización puede duplicar código.
    El compilador/LTO puede transformar ambos: ni tamaño ni velocidad quedan garantizados.

11. [x] **Cerrar fallback LOD por presupuesto de construcción.** Pendiente seleccionado tras
    revisar el estado actual: un corte residente no garantiza que su grilla fina quepa en CPU/GPU.
    Añadir `createOffscreenWithVoxelLimits(canvas, voxelSize, maxVoxels, maxEstimatedBytes)` como
    constructor adicional para fijar los límites CPU existentes del renderer, sin modificar el
    constructor/worker predeterminados ni introducir límites mutables sobre escenas reutilizadas.
    Ambos límites son enteros `u32` positivos, bytes estimados independientes de RSS/VRAM; el límite
    efectivo de vóxeles conserva también el máximo GPU. Reutilizar una inicialización privada.
    Cierre: fixture pequeña residente que cabe a 16 px y excede el presupuesto al refinar; verificar
    intento fino + fallback grueso en renderer WebGPU, escena válida conservada ante posteriores
    fallos, error propagado y ausencia de cargas adicionales. Comparar con renderer sin restricción;
    tests/sintaxis, checks nativo/WASM, Playwright, paquete actualizado y commit. Sin afirmaciones
    de rendimiento y sin exponer controles nuevos en el host web.
    Resultado: Chrome 155.0.8059.39 + SwiftShader/Vulkan, canvas 320×240 e invalidación a 321×240.
    Ocho puntos residentes: referencia fina 8 vóxeles; límite 1 vóxel/64 MiB CPU estimados;
    dos intentos de build en la llamada (fino fallido + grueso válido), un vóxel visible y cero
    cargas adicionales. El error de refinamiento explícito se propaga; `renderFrame` sigue
    presentando la escena anterior y volver a 16 px recupera la ejecución. Es una sesión externa:
    el caller conduce el refinamiento, sin afirmar error automático en el panel.
    También pasan límites cero rechazados, sesión, atributos y reutilización LOD. Validación:
    255 tests Rust pasan (3 opcionales ignorados), 27 JS y 11 Python; fmt, Clippy nativo/WASM,
    check-web, sintaxis del probe y diff correctos. WASM/bindings y paquete local regenerados.
    No se midieron FPS/RSS ni se validó GPU física/Firefox o fallos de subida GPU/memoria estimada.
    [Método y reproducción](docs/WEB_PERFORMANCE_BENCH.md#resident-construction-budget-fallback--13-10-2026).

12. [x] **Diagnóstico de layouts públicos antes de añadir `Box`.**
    [`layout_report`](crates/viewer_wasm/examples/layout_report.rs) mide tamaños/alineación de tipos
    reales, handle `Box<T>` y suma handle+objeto. Reproducir:
    `cargo run -p viewer_wasm --profile fast --offline --example layout_report`.
    rustc 1.99.0 (b940084d7), x86_64 de 64 bits:

    | Tipo | Inline (B) | Alineación (B) | Handle Box (B) | Objeto + handle (B) |
    | --- | ---: | ---: | ---: | ---: |
    | `ViewerController` | 144 | 8 | 8 | 152 |
    | `LodDataset` | 752 | 8 | 8 | 760 |
    | `FormatReader<Cursor<Vec<u8>>>` | 184 | 8 | 8 | 192 |
    | `RenderScene` | 48 | 8 | 8 | 56 |
    | `InstanceRaw` | 40 | 4 | 8 | 48 |

    La suma es un modelo inline+payload, excluye buffers propios y overhead/padding del allocator;
    el ejemplo no asigna cajas ni mide RSS, accesos o throughput. No demuestra un beneficio para
    encajonar objetos enteros, y no se altera su propiedad. Encajonar solo variantes grandes en
    arrays o estructuras con otra distribución sigue requiriendo medir el consumidor concreto.
    El layout Rust de estos tipos no constituye una ABI garantizada; `InstanceRaw` conserva su
    layout GPU explícito. Validación: ejecución nativa, fmt, Clippy de `viewer_wasm --all-targets`,
    check del ejemplo para WASM y diff pasan. El ejemplo WASM compila, pero sus tamaños no se
    ejecutaron/registraron: la tabla es exclusivamente nativa. Se reutiliza la suite anterior,
    porque este diagnóstico no cambia lógica de producción.

13. [x] **Prioridad incremental LOD para proyectores estables.** Núcleo implementado, medido
    e integrado en las dos selecciones de cámara WASM con límite de prioridades de 4 MiB. [Evidencia](docs/LOD_FRONTIER_BENCH.md#prioridad-incremental-opt-in--13-10-2026).
    264 tests pasan, tres ignorados; fmt, Clippy nativo/WASM y check-web pasan.
     Conservar `select_lod` y sus
    callbacks con estado sin cambios. Añadir `select_lod_stable_projector` como opt-in: error estable
    por nodo durante toda la llamada, sin garantía de número/orden de callbacks. Heap de candidatos
    + lista de bloqueados, prioridad por mayor error/menor ID; conservar orden visible, estructura,
    semánticas y reintento al liberar presupuesto Replacement. Una cota conservadora de crecimiento
    debe detectar posibles overflows antes de elegir, preservando precedencia mediante fallback.
    Límite explícito sobre capacidades heap/bloqueados, con reservas falibles y estadísticas aparte
    de `LodSelection`; al agotarlo, liberar el intento y ejecutar la ruta original. No limita RSS,
    temporales comunes ni picos internos del allocator. No añadir caché persistente a la jerarquía.
    Cierre: resultados completos/errores equivalentes con presupuestos, frusta, empates e infinito,
    casos de liberación y errores anticipados; matriz de 86 casos + cámara real, tres pares alternados
    y capacidad de prioridades registrada. Adoptar en la cámara solo tras medir y validar estabilidad,
    repetir navegador, README y commits (completado). Playwright MCP Chrome 155/SwiftShader pasa
    sesiones, atributos, reutilización y fallback real por presupuesto. Dataset Bunny ramificado:
    35 bloques, 40.256 puntos fuente y 21.969 vóxeles, cobertura completa, sin sobredimensionados,
    geometría conservada en RGB/intensidad/clasificación/columna. Sin errores JS no capturados;
    son pruebas funcionales, no evidencia de presentación visual ni FPS.
    Los splices/búsquedas y ciertos reintentos siguen siendo
    lineales: no prometer complejidad global logarítmica ni mejoras de FPS.

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
