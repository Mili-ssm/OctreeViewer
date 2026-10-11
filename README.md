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
Las mediciones grandes de [Albuera/Majadas](docs/REAL_DATASET_BENCH.md) corresponden a versiones
anteriores y no validan v5. El [protocolo web](docs/WEB_PERFORMANCE_BENCH.md) sigue sin tabla de
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
La base completa sigue abierta: faltan conversiones grandes repetidas con temporales/I/O por fase
y mediciones reales Chrome/Firefox.

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
  representan. Continúan pendientes las conversiones grandes por fase y Chrome/Firefox.

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
2. [ ] **RAM y consultas — `viewer_core`, `viewer_format`, `viewer_render_wgpu`, `viewer_wasm`.**
   Definir presupuestos en bytes para acumulación CPU, caché y recursos GPU, con respuesta explícita
   al agotarlos y medición del pico conjunto. Priorizar lookup de `BlockId` y buffers reutilizables.
   Si el perfil lo justifica, comparar mapa actual con vector ordenado/reducción o claves compactas;
   índice residente plano; cola de prioridad LOD con el mismo desempate; cachés de geometría/atributo
   separadas. Lookup y reutilización acotada de rangos globales ya implementados. Cierre: menor coste
   medido, mismos resultados deterministas,
   cobertura `Replacement`/`Additive`, sin solapamientos ni crecimiento de memoria sin control.
3. [ ] **Picos de conversión — `viewer_convert` y `viewer_format`.** Reducir vectores intermedios
   y medir el lote de compresión completo. Comparar primero con el spool actual; solo después probar
   spool ordenado espacialmente, particiones acotadas o dos pasadas sobre LAZ. Cierre: salida
   semánticamente equivalente, RAM/temporales/I/O y tiempo por fase comparados, recuperación intacta.
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

Mantener IDs y offsets públicos `u64`; usar ordinales menores solo internamente y con límites
validados. Una máscara de ocho hijos sirve para 2³, no para las divisiones 4³/8³ (hasta 512 hijos).
Morton y máscaras son candidatos sobre coordenadas enteras; no reutilizar mantisas, NaN, bits de
atributos válidos ni campos reservados de v5. Evaluar retornos/flags LAS y ángulo escalado como
preservación de datos con round-trip y esquema explícito, no como bits libres. Reutilizar jerarquía
COPC o ampliar EPT queda condicionado al perfil de importación y al alcance de interoperabilidad.

## Próximos criterios de cierre

El seguimiento activo se mantiene en este README; no se crean Issues/PRs por defecto.

- [ ] **Navegador:** verificar en Chrome y Firefox con WebGPU la apertura gris, asociación de pareja,
  cambio de componente, teclado, archivos erróneos, OPFS/cuota, cancelación durante lectura, picking
  y pérdida real de GPU. Los tests Rust/JS y renders nativos no sustituyen esa interacción.
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
