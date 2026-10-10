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

## Próximos criterios de cierre

El seguimiento activo se mantiene en este README; no se crean Issues/PRs por defecto.

- [ ] **Navegador:** verificar en Chrome y Firefox con WebGPU la apertura gris, asociación de pareja,
  cambio de componente, teclado, archivos erróneos, OPFS/cuota, cancelación durante lectura, picking
  y pérdida real de GPU. Los tests Rust/JS y renders nativos no sustituyen esa interacción.
- [ ] **Escala:** medir hasta una fuente mayor de 500 GB decimales, con índice, spools, RSS, recursos
  GPU, bytes leídos, primera vista y navegación separados. Fijar mínimos oficiales RAM/GPU/SO.
- [ ] **Conversión:** matriz de fuentes <10 GB con al menos tres repeticiones y caché declarada;
  objetivos <5 min (ideal <1 min) y suma de ambos archivos ≤3× fuente (ideal <2×), no garantías.
  Perfilar decodificación, particionado, rangos, compresión, validación y temporal antes de cambiar
  la pipeline o adoptar codecs/representaciones nuevas. Comparar spools y escritura directa con
  igual salida semántica; comprobar cancelación/recuperación y memoria máxima.
- [ ] **Calidad/LOD:** comparar divisiones 2/4/8 con igual presupuesto y revisión visual antes de
  cambiar el valor predeterminado 2. Extender cobertura no solapada a todos los consumidores;
  corpus grilla/plano/línea/clusters/outliers, cámaras, viewport, EPT y adapters diferentes.
- [ ] **Lectura:** medir navegación con bloques retardados; integrar rangos asíncronos y
  tickets/cancelación del scheduler solo si la evidencia lo justifica. El worker cede entre bloques.
- [ ] **Formato y atributos:** evaluar `u16`/coordenadas locales con error y tamaño medidos; acordar
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

La última orden requiere GPU o Vulkan por software. La validación de esta revisión cubre **212 tests Rust**, **2 tests GPU adicionales**, seis suites
Node y tres tests Python, Clippy nativo/WASM, formato y compilación WASM release. Se generó el
paquete local y se renderizó Outer Banks con y sin acompañante a 640×480 (2.381 vóxeles,
12.034 píxeles de contenido; todos grises en el caso sin atributos). No certifica navegación
real en Chrome/Firefox ni escala.
Ver [host web y empaquetado](examples/web-host/README.md), [muestras](data/samples/README.md) y
[corpus de datasets](data/datasets/README.md). El registro histórico conserva referencias a
Potree, COPC, EPT, Arrow, Zarr, LASzip y artículos usados en decisiones anteriores.
