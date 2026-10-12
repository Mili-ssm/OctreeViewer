# Memoria y lectura v5 — 11-10-2026

Primera medición de la auditoría, con formato, precisión y codecs v5 conservados.
Base: `ddb3076`; candidato: esa revisión más los cambios de lookup, temporales e instrumentación
de esta entrega. No constituye el cierre de la matriz de escala del README.

## Entorno y método

- Linux `7.2.9-200.fc44.x86_64`, AMD Ryzen AI 9 365, 31.885.120 KiB de RAM total observable.
- Rust `1.99.0 (b940084d7 2026-09-28)`, perfil `perf`; disco local del checkout.
- RSS pico externo de cada proceso mediante `/usr/bin/time -v`, incluyendo resumen del benchmark.
  No equivale al presupuesto heurístico ni a memoria exclusiva de una fase.
- Conversión: un worker, subdivisión 2, opciones restantes predeterminadas, sin checkpoint;
  un calentamiento de cada binario seguido de tres pares alternando orden antes/después por fuente.
- Lectura: tres procesos por modo sobre la pareja Belvedere existente, ya v5. Caché del SO caliente
  por lecturas previas, sin vaciarla; sin compilaciones/checks concurrentes en la ronda final.
  No se fijaron frecuencia CPU, modo de energía ni carga externa del equipo.
- Tablas: mediana y rango mínimo–máximo. Los logs de esta sesión están en
  `target/audit-v5-2026-10-11/` (directorio local excluido de Git).

## Conversión pequeña: comparación de la entrega completa

Se eliminó el vector de posiciones reconstruidas (`12 × puntos` bytes de payload temporal) y se
liberó el vector cuantizado (`12 × puntos`) tras codificar XYZ, antes de comprimir. Durante compresión
dejan de coexistir esos dos vectores. No es una fórmula del pico RSS: siguen presentes fuente, buffer
raw reutilizado, bytes preparados, asignador y estructuras del conversor. El candidato también añade
el catálogo de IDs; la comparación no aísla cada cambio por separado.

| Fuente | Versión del código | Conversión (ms) | RSS pico (KiB) | Pareja de salida (B) |
| --- | --- | ---: | ---: | ---: |
| Outer Banks `fs_e430_n4008_18`, 111.190 puntos | Base v5 | 63,337 [61,076–68,335] | 22.444 [22.292–22.764] | 2.242.558 |
| Misma fuente | Candidato v5 | 61,072 [59,085–63,211] | 15.564 [15.012–15.592] | 2.242.558 |
| Outer Banks `fs_e432_n4002_18`, 149.730 puntos | Base v5 | 88,321 [82,621–99,576] | 29.108 [28.792–29.376] | 3.039.134 |
| Misma fuente | Candidato v5 | 81,929 [80,706–84,066] | 19.224 [19.196–19.336] | 3.039.134 |

Reducción de RSS mediano observada: 30,7 % y 34,0 %. Los intervalos de tiempo se solapan; no se
concluye una aceleración general. Se compararon **ambos archivos byte a byte** en los tres pares
de cada muestra, además de los calentamientos. No se reconvirtieron Belvedere, Albuera o Majadas
en esta entrega ni se midió el pico temporal de esas conversiones.

## Lectura secuencial de Belvedere v5

Fuente documentada en [el corpus](../data/datasets/README.md): 81.682.329 puntos fuente y
81.706.211 filas almacenadas incluyendo LOD, repartidas en 1.343 bloques.
Pareja existente: `.oct` de **981.063.326 B** y `.octattrs` de **610.586.859 B**;
total **1.591.650.185 B**. Índice serializado común: **807.558 B**, almacenado una sola vez.
El catálogo nuevo ocupa **10.744 B** de IDs, además de estructuras previas del índice.

| Modo | Apertura de índice (ms) | Lectura + CRC + decodificación (ms) | RSS pico (KiB) |
| --- | ---: | ---: | ---: |
| Solo XYZ | 7,544 [6,237–7,823] | 2.731,480 [2.725,247–2.732,253] | 13.836 [13.788–13.976] |
| Todos los payloads | 7,665 [6,127–7,819] | 6.207,340 [6.181,616–6.211,939] | 18.368 [18.324–18.644] |
| Columna 0 | 7,676 [7,632–7,811] | 201,176 [197,140–202,635] | 8.572 [8.416–8.796] |

Adjuntar atributos tardó 0,013 [0,009–0,014] ms en lectura completa y
0,011 [0,009–0,011] ms para la columna 0; no se adjuntaron para XYZ.

| Modo | Mediana del p50 de bloques (ms) | Mediana del p95 de bloques (ms) | Payload XYZ leído (B) | Payload de atributos leído (B) |
| --- | ---: | ---: | ---: | ---: |
| Solo XYZ | 1,948 [1,935–1,974] | 4,717 [4,715–4,797] | 980.255.704 | 0 |
| Todos los payloads | 4,316 [4,293–4,330] | 11,110 [11,068–11,141] | 980.255.704 | 610.586.795 |
| Columna 0 | 0,141 [0,141–0,141] | 0,347 [0,339–0,350] | 0 | 656.109 |

Los percentiles usan nearest-rank sobre los tiempos de cada bloque y se agregan entre tres procesos;
no son percentiles de una muestra combinada. Los bloques tienen tamaños distintos y se recorren en
orden de ID. Son latencias secuenciales del modo completo solicitado, incluyendo asignación y descarte
del resultado; no representan acceso aleatorio, red, primer LOD visible ni frames del navegador.

Metadatos leídos al abrir: `807.558 + 64 = 807.622 B` del archivo de coordenadas y otros `64 B` al
adjuntar el acompañante. La sobrecarga total en disco es índice + `128 B` de cabeceras/footers de
la pareja. Los contadores verificaron que XYZ no toca atributos y que leer columna 0 no toca XYZ
ni las demás columnas. Solo los contadores de bytes son exactos; el benchmark conserva estadísticas
y un `u64` de tiempo por bloque que también contribuyen al RSS del proceso.

## Lookup sintético

`SpatialHierarchy::references_block` cambia el recorrido de nodos/referencias por búsqueda binaria
en IDs ordenados y únicos. Añade 8 B por ID único y el coste de construir el catálogo al abrir.
No añade otro `HashMap`; tampoco elimina las claves ya presentes en el directorio del lector.

Nueve pares alternados tras calentamiento; cada muestra consulta 256 IDs, mitad presentes y mitad
ausentes. Equivalencia comprobada fuera del cronómetro. El baseline usa vectores por nodo y
**no reproduce el recorrido del antiguo `HashMap`**: compara escalabilidad de algoritmos.

| Nodos / IDs únicos | Catálogo (B) | Construcción jerarquía + catálogo (µs, una observación) | Escaneo (ns/consulta) | Búsqueda binaria (ns/consulta) |
| ---: | ---: | ---: | ---: | ---: |
| 64 | 512 | 43 | 46 | 7 |
| 512 | 4.096 | 161 | 358 | 12 |
| 4.096 | 32.768 | 1.261 | 1.256 | 9 |
| 16.384 | 131.072 | 3.861 | 5.244 | 18 |

Los tiempos por consulta derivan de la mediana del lote y están truncados a ns enteros. No justifican
un factor de aceleración del lector/visor. Quedan por perfilar apertura y navegación con muchos más
IDs; compactar o compartir el directorio puede reducir duplicación si la memoria medida lo exige.

## Reproducción

```sh
cargo build --profile perf -p viewer_convert --example real_dataset_bench --offline
cargo run --profile perf -p viewer_core --example block_lookup_bench --offline
mkdir -p target/v5-memory-repro
```

Para cada repetición usar un nombre de salida nuevo y conservar ambos archivos para compararlos;
el conversor rechaza sobrescrituras. Ejecutar los dos binarios de revisiones comparadas alternando
su orden y con iguales opciones. `OUTPUT` representa una ruta nueva bajo el directorio anterior:

```sh
/usr/bin/time -v target/perf/examples/real_dataset_bench convert data/samples/fs_e430_n4008_18.las OUTPUT.oct 1 2
/usr/bin/time -v target/perf/examples/real_dataset_bench convert data/samples/fs_e432_n4002_18.las OTHER_OUTPUT.oct 1 2
/usr/bin/time -v target/perf/examples/real_dataset_bench read-geometry data/datasets/belv_2009_digital-aerial_pcd_copc.oct
/usr/bin/time -v target/perf/examples/real_dataset_bench read data/datasets/belv_2009_digital-aerial_pcd_copc.oct
/usr/bin/time -v target/perf/examples/real_dataset_bench read-attribute data/datasets/belv_2009_digital-aerial_pcd_copc.oct 0
```

`open_index_ms` conserva su significado histórico: apertura más asociación cuando procede.
`open_primary_index_ms` y `attach_attributes_ms` lo desglosan. `serialized_index_bytes` mide disco,
no RAM decodificada. Los contadores distinguen metadatos/payload por archivo y el resumen agrupa
geometría, RGB de presentación y cada columna por índice de esquema.

## Presupuesto CPU del builder de vóxeles

Segunda entrega, comparada con `4e78489`, sin cambiar grilla, color ni orden de instancias.
`RendererConfig` usa por defecto 256 MiB de **memoria estimada de construcción**, además del
límite independiente de vóxeles renderizables. El ejemplo nativo acepta un sexto argumento
opcional con ese presupuesto en bytes. Las APIs antiguas sin presupuesto de bytes se conservan.

Para `n` acumuladores, el modelo carga `n × A + max(C, capacidad_instancias × 40)` bytes.
`A` es dos veces el tamaño de `(VoxelKey, VoxelAccumulator)` más seis palabras de puntero;
`C` reserva hasta `min(262.144, presupuesto / 8 / P)` entradas de proyección. `P` carga dos veces
`(VoxelKey, f64)`, seis palabras de puntero y cuatro claves para la capacidad de la cola FIFO.
En esta compilación nativa, `A = 192 B` y `P = 160 B`; dependen de la arquitectura. La reserva
se contabiliza completa aunque la caché no se llene. Se descarta antes de reservar el vector final
con `try_reserve_exact`, evitando el crecimiento geométrico durante la finalización.

Es un modelo conservador explícito, **no una cota garantizada del asignador ni de RSS**. Excluye
índice, bloques fuente/caché, escena anterior, picking, heap JS/WASM y buffers GPU. Las reservas
de vector devuelven errores; las asignaciones internas de `BTreeMap` no ofrecen esa recuperación.
Al exceder el modelo o el máximo de vóxeles, el builder queda invalidado y `try_finish` devuelve
el error. En WASM, la construcción precede a la subida y confirmación del corte visible.

Diagnóstico Outer Banks `fs_e430_n4008_18`, 640×480, objetivo 4 px, llvmpipe/Vulkan:
PNG idéntico byte a byte antes/después, 111.190 puntos fuente, 1.849 vóxeles, 9.790 píxeles con
geometría, cero vóxeles sobredimensionados. Capacidad final **73.960 B**; pico estimado
**33.909.408 B**, incluyendo **33.554.400 B** de reserva de caché, que solo alcanzó 2.552 entradas.
No se interpreta este valor como memoria residente observada ni como mejora de rendimiento.
Una ejecución con cero bytes devuelve `EstimatedMemoryLimitExceeded` y conserva el PNG anterior.
El estado visible del navegador sigue pendiente de prueba con Chrome/Firefox.

```sh
cargo build --profile perf -p viewer_render_wgpu --example render_dataset --offline
target/perf/examples/render_dataset data/samples/fs_e430_n4008_18.oct target/voxel-cpu-repro 4 640 480
# Debe fallar sin sobrescribir el PNG de la ejecución anterior:
target/perf/examples/render_dataset data/samples/fs_e430_n4008_18.oct target/voxel-cpu-repro 4 640 480 0
```

Cobertura: frontera exacta de bytes, límite de vóxeles independiente, muchos puntos en una celda,
caché desactivada/evicción con igual escena, error persistente, celdas no renderizables y overflow
antes de reservar.

## Consultas repetidas de rangos globales

Tercera entrega: `FormatReader::attribute_range` memoriza las últimas 64 consultas de atributo y
componente en una cola LRU compartida por lectores derivados. Conserva `None`, valida antes del
lookup y calcula un miss recorriendo los metadatos existentes. Los rangos por nodo permanecen
intactos: no se modifica v5, no se altera la escala de color ni se leen payloads. El máximo de
entradas es fijo, independiente del total de atributos/componentes permitido por el esquema.

Comparación con `d8c354a`, compilando **el mismo ejemplo** contra ambas revisiones, perfil `perf`,
entorno nativo anterior y Belvedere v5 de 1.343 nodos/11 componentes. Tres pares de procesos
alternando orden, sin compilaciones/checks concurrentes. Cada proceso consulta primero todos los
componentes y luego mide nueve lotes de 100.000 consultas cíclicas al conjunto completo. Se usa
`black_box` y se comprueba que los checksums de resultados coinciden entre revisiones. La primera
pasada incluye el cálculo; los lotes miden reutilización con metadatos/caché calientes.

| Ruta | Primera pasada, 11 componentes (µs) | Consultas repetidas (ns/consulta) |
| --- | ---: | ---: |
| Escaneo anterior | 105,969 [102,211–155,793] | 9.480,974 [9.321,823–9.665,739] |
| Caché LRU | 104,596 [103,404–135,655] | 13,945 [13,926–14,118] |

Mediana y mínimo–máximo de tres procesos; cada dato de consultas repetidas es la mediana de
nueve lotes. Un calentamiento exploratorio con 10.000 consultas mostró más dispersión en el
candidato (13,901–29,629 ns), por lo que se aumentó el lote. No se concluye una mejora de apertura,
cambio de color completo, LOD o FPS. El caso medido cabe en la caché; un patrón de más de 64
componentes puede provocar evicción y repetir escaneos. La sincronización serializa misses entre
forks y añade coste a índices pequeños; no se midió contención entre hilos. Logs locales ignorados:
`target/audit-attribute-ranges/repeated-100k/`.

```sh
cargo run --profile perf -p viewer_format --example attribute_range_bench --offline -- data/datasets/belv_2009_digital-aerial_pcd_copc.oct 100000
```

## Perfil de conversión nativa por fase

Entrega sobre `71d5fb9`, incluyendo la instrumentación local de `convert-profile` y el
muestreador `crates/viewer_convert/benchmarks/profile_conversion.py`. Este último exige un
directorio nuevo, conserva logs/pareja/samples y escribe `report.json` también al fallar o
interrumpirse. Un código cero sin ambos archivos no vacíos se considera incompleto; esto no
sustituye la validación profunda que ya realiza el conversor antes de publicar.

`convert` conserva los campos anteriores sin depender de `/proc`; `convert-profile` añade
contadores Linux de `/proc/self/io`. Los intervalos `phase_N` van de una notificación de progreso
a la siguiente y conservan etapas repetidas. No son cronómetros internos de cada función:
`PlanningMetadata` puede incluir trabajo inicial anterior a la primera notificación espacial;
`ReadingAndWritingBlocks` incluye cierre del writer y preparación para validar. `Complete`
mide el intervalo hasta que vuelve el conversor, incluyendo destrucción de sus temporales.
`conversion_elapsed_ms` excluye el resumen posterior de almacenamiento.

El muestreador externo observa RSS y VmHWM del hijo y tamaños lógicos de `.tmp` y archivos bajo
directorios `.work` cada 100 ms. Excluye logs y salidas finales; deduplica enlaces duros por inode.
No mide espacio físico asignado ni garantiza capturar picos breves. El crecimiento acumulado
suma deltas positivos por ruta entre muestras: omite reescrituras y archivos efímeros, y **no es
I/O exacto**. RSS/VmHWM observados tampoco identifican por sí solos la fase del máximo real.

`rchar/wchar` son bytes de llamadas de lectura/escritura del proceso, incluyendo buffers/spools
y las pequeñas lecturas propias de `/proc/self/io`. `read_bytes/write_bytes` son contadores de
almacenamiento Linux, afectados por caché/writeback; no son los anteriores ni transferencia
física medida en el dispositivo. `cancelled_write_bytes` se conserva por separado. El proceso
muestreador no entra en esos contadores del conversor.

### Belvedere: tres conversiones v5

Mismo equipo/Rust/Linux descritos al principio, perfil `perf`, un worker, divisiones 2 y opciones
restantes predeterminadas. Fuente de 618.390.994 B y 81.682.329 puntos. Una conversión de
calentamiento seguida de tres procesos secuenciales (`belvedere-run-2/3/4`), sin compilaciones
ni otros benchmarks concurrentes. Caché del SO previamente calentada y sin vaciar; no se fija su
residencia: `read_bytes` fue 0, 0 y 88.432.640 B respectivamente. No se fijaron frecuencia CPU,
modo de energía o writeback del sistema. El muestreo externo puede introducir overhead, que no
se ha aislado con una ronda sin muestreador. Mediana y mínimo–máximo entre tres procesos:

| Intervalo entre notificaciones | Tiempo (s) |
| --- | ---: |
| Total hasta retorno, antes del resumen | 46,286 [45,625–46,460] |
| `PlanningMetadata` | 0,044 [0,043–0,052] |
| `PlanningSpatialIndex` | 10,353 [10,295–10,400] |
| `BuildingSpatialHierarchy` | 18,216 [17,684–18,392] |
| `ReadingAndWritingBlocks` | 11,343 [11,108–11,349] |
| `ValidatingOutput` | 6,191 [6,140–6,203] |
| `PublishingOutput` | 0,016 [0,006–0,032] |
| `Complete` hasta retorno | 0,169 [0,157–0,170] |

La jerarquía es el mayor intervalo observado (~39 % del total), seguido de escritura/compresión
y planificación espacial. La validación ocupa ~13 %: esta base no justifica sustituirla por
una comprobación más débil. Tampoco compara dos implementaciones ni demuestra una aceleración.
Queda por separar CPU/descompresión/I/O dentro de cada intervalo antes de optimizar los spools.

| Métrica del proceso completo | Mediana [mínimo–máximo] |
| --- | ---: |
| RSS pico muestreado / VmHWM máximo observado (MiB) | 94,254 [93,445–95,277] |
| Temporales: pico lógico muestreado (B) | 4.941.891.420 [igual en las tres] |
| Temporales: crecimiento positivo observado (B) | 17.112.157.133 [17.018.786.107–17.158.453.922] |
| `conversion_io_rchar` (B) | 18.375.221.641 [18.375.221.641–18.375.221.682] |
| `conversion_io_wchar` (B) | 17.756.806.959 [igual en las tres] |
| `conversion_io_write_bytes` (B) | 17.768.312.832 [17.763.926.016–17.768.574.976] |
| `conversion_io_cancelled_write_bytes` (B) | 0 [igual en las tres] |

Las llamadas del conversor solicitan leer ~29,71× y escribir ~28,71× el tamaño de la fuente LAZ
comprimida (o ~11,54×/~11,16× la pareja final). Es amplificación de bytes de llamadas del proceso,
incluye spools sin comprimir y lectura de validación; no es amplificación física del SSD.
Los máximos RSS/VmHWM observados coinciden en estas tres ejecuciones; no equivalen a una medida
externa completa del pico ni al presupuesto controlado estimado de 62.940.928 B del conversor.

Cada ejecución produce 1.343 bloques, 81.706.211 filas incluyendo LOD y la misma pareja:
981.063.326 B de coordenadas + 610.586.859 B de atributos = **1.591.650.185 B**, ratio **2,574×**
la fuente. Se verificó igualdad byte a byte mediante SHA-256 de cada archivo entre las tres
repeticiones y el calentamiento:

```text
oct      2faa89194c199dba65955d0442b129d0c5f339a1f8ddf89d1181920f51f82d0d
octattrs 013b8fe420ba82aee1775f08800d0c2dcd94796dc8f096b0824e4d80310fa294
```

Logs, muestras y resumen local: `target/audit-v5-conversion-phases/` (`summary.json`).
Validación: tests del workspace, siete tests del ejemplo, cinco tests Python, compilación
Python, formato y Clippy nativo/WASM pasan. Ruff no está instalado en este entorno.

Reproducción desde la raíz, sin compilaciones ni otros benchmarks concurrentes durante la medida:

```sh
cargo build --profile perf -p viewer_convert --example real_dataset_bench --offline
python3 -m unittest discover -s crates/viewer_convert/benchmarks -p 'test_*.py'
cargo test -p viewer_convert --example real_dataset_bench --profile fast --offline
python3 crates/viewer_convert/benchmarks/profile_conversion.py \
  data/datasets/belv_2009_digital-aerial_pcd_copc.laz \
  target/conversion-profile-repro/warmup --workers 1 --divisions 2 --interval-ms 100
# Repetir con directorios nuevos run-1, run-2 y run-3 tras el calentamiento.
```

Las salidas quedan para inspección; no se eliminan automáticamente ni se reutiliza un destino
existente. La interrupción del muestreador termina al hijo y marca el informe incompleto;
puede conservar temporales, pues no equivale a la cancelación cooperativa del CLI principal.
El perfilador mide conversiones LAS/LAZ locales sin checkpoint; no valida recuperación,
HTTP/OPFS, Chrome/Firefox ni fuentes de más de 500 GB. La ampliación a Majadas se documenta
a continuación; faltan Albuera repetida y calibración de los presupuestos CPU con el pico conjunto.

## Conversión v5 de Majadas TLS — 12-10-2026

Base `fe230af`, perfil `perf`, Linux 7.2.9-200.fc44.x86_64, Rust 1.99.0 y Ryzen AI 9 365;
31.885.120 KiB de RAM total. Fuente local de **1.343.266.470 B**, **415.465.882 puntos**,
descrita en [el corpus](../data/datasets/README.md#sensoforest-albuera-uav-y-majadas-tls).
Tres conversiones seriales independientes, un worker, divisiones 2 y opciones restantes
predeterminadas: 262.144 puntos/bloque, error máximo 0,01 y presupuesto controlado estimado
de 512 MiB. Sin `RUSTFLAGS` ni `target-cpu=native`; se comprobó que el ejecutable corresponde
al artefacto de Cargo sin esos flags. Antes de cada conversión se lee toda la fuente y se comprueba su MD5
`fd0b4433e805246f2442df037dfadb6b`. No se vacía caché ni se garantiza residencia durante la
conversión; no se fijan frecuencia CPU, modo de energía o writeback del SO.

No hay compilaciones ni otros benchmarks concurrentes durante las medidas. Se reutiliza
`convert-profile` y el muestreador externo cada 100 ms, con las mismas definiciones y límites
de intervalos, I/O lógico/de almacenamiento y máximos observados descritos arriba.
Los logs, parejas, muestras e información de entorno quedan localmente en
`target/audit-v5-majadas-2026-10-12/`.

### Resultados y equivalencia

Las tres ejecuciones finalizaron con código cero y sin errores de observación. Mediana y
mínimo–máximo entre procesos; los intervalos se delimitan por notificaciones, no por funciones:

| Intervalo | Tiempo (s) |
| --- | ---: |
| Total hasta retorno, antes del resumen | **347,277 [345,468–356,203]** |
| `PlanningMetadata` | 0,042 [0,032–0,053] |
| `PlanningSpatialIndex` | 43,169 [42,214–44,246] |
| `BuildingSpatialHierarchy` | **197,574 [196,446–208,956]** |
| `ReadingAndWritingBlocks` | 72,285 [72,141–72,748] |
| `ValidatingOutput` | 32,317 [32,316–32,595] |
| `PublishingOutput` | 0,012 [0,002–0,017] |
| `Complete` hasta retorno | 0,530 [0,511–0,739] |

| Métrica del proceso | Mediana [mínimo–máximo] |
| --- | ---: |
| RSS pico muestreado / VmHWM máximo observado (MiB) | **77,086 [77,062–83,418]** |
| Temporales: pico lógico muestreado (B) | **21.250.289.683 [igual en las tres]** |
| Temporales: crecimiento positivo observado (B) | 138.297.177.192 [138.176.106.156–138.360.218.084] |
| `conversion_io_rchar` (B) | 143.601.067.903 [143.601.067.898–143.601.067.903] |
| `conversion_io_wchar` (B) | 142.257.783.952 [igual en las tres] |
| `conversion_io_read_bytes` (B) | 26.358.759.424 [21.506.580.480–31.224.238.080] |
| `conversion_io_write_bytes` (B) | 142.333.284.352 [142.324.559.872–142.337.003.520] |
| `conversion_io_cancelled_write_bytes` (B) | 0 [igual en las tres] |

RSS/VmHWM describen el proceso completo, incluido el resumen posterior; tiempos y contadores
de conversión excluyen ese resumen. No constituyen el pico conjunto por fase ni una cota de RSS.
La estimación controlada del conversor es **72.308.992 B**, distinta de la memoria observada.
El pico temporal incluye candidatos de salida aún no publicados mientras coexisten con spools;
el tamaño lógico muestreado no equivale a ocupación física. El calentamiento de la fuente no
impide lecturas de almacenamiento posteriores de los spools; la variación de `read_bytes`
confirma que no es una prueba con toda la conversión residente en caché.

Lecturas/escrituras lógicas equivalen aproximadamente a **106,90× / 105,90×** la fuente LAZ
comprimida, o **19,06× / 18,88×** la pareja final. Incluyen pasos temporales y validación;
no son amplificación física del dispositivo. El crecimiento observado por muestreo no debe
usarse como sustituto de `wchar` ni de `write_bytes`.

Las tres salidas coinciden en puntos fuente (**415.465.882**), filas almacenadas con LOD
(**415.602.190**), bloques (**6.259**) e índice serializado (**3.455.366 B**, contado una vez).
Coordenadas: **4.214.687.109 B**; atributos: **3.319.094.608 B**; total **7.533.781.717 B**,
ratio **5,608553×** la fuente. Los SHA-256 de cada archivo coinciden entre las tres ejecuciones:

```text
oct      f17646889c058a49262034ac28cdd744ffdcdc920069579c63e062d827180b39
octattrs 24ea479471b7f3641d8f86879e252fffc77a049434b02f56c683754e2074ce0b
```

En esta configuración no se alcanzan los objetivos de conversión <5 min ni pareja ≤3× fuente.
La jerarquía representa aproximadamente el **57 %** del tiempo frente al **9 %** de validación:
priorizar el perfil de clasificación espacial, profundidad y reescritura de spools antes de
rediseñarlos. El índice ocupa solo el **0,046 %** de la pareja: reducir metadatos no resolvería
por sí solo el objetivo de tamaño. Los experimentos de payload deben conservar precisión y
atributos, conforme al orden de trabajo del README. Estos resultados no comparan revisiones,
no prueban una mejora ni sustituyen una medida de navegador/GPU.

Validación de esta entrega: integridad MD5 previa a cada proceso, validación profunda del
conversor, estado completo de los tres informes, invariantes comunes y SHA-256 de ambas salidas.
`summary.json` conserva los valores agregados y de cada ejecución; `environment.json` registra
revisión, entorno, opciones y hash del binario. Los checks de código de `fe230af` siguen vigentes:
esta entrega cambia documentación, no implementación. Albuera repetida y host web siguen pendientes.

Reproducción desde la raíz, compilando antes de iniciar las medidas:

```sh
cargo build --profile perf -p viewer_convert --example real_dataset_bench --offline
# Calentar/verificar antes de CADA repetición; comprobar el MD5 indicado arriba:
md5sum data/datasets/sensoforest/SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz
python3 crates/viewer_convert/benchmarks/profile_conversion.py \
  data/datasets/sensoforest/SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz \
  target/majadas-v5-repro/run-1 --workers 1 --divisions 2 --interval-ms 100
# Repetir calentamiento y conversión con directorios nuevos run-2 y run-3.
```

## Conversión v5 de Albuera UAV — 12-10-2026

Base `d2af4b3` (misma implementación que `fe230af`), perfil `perf` sin `RUSTFLAGS`, un worker,
divisiones 2, 262.144 puntos/bloque, error máximo 0,01 y presupuesto estimado 512 MiB.
Mismo Linux/Rust/equipo que Majadas;
fuente local de **4.305.484.789 B** y **508.809.556 puntos**. Tres procesos seriales, fuente
leída y verificada mediante MD5 `71ff22c046d6eb83c371791f680bea13` antes de cada uno.
No se vacía caché ni se garantiza residencia; frecuencia CPU, energía y writeback sin fijar.

El ejecutable de referencia permanece fijo durante las tres medidas, sin compilaciones ni
otros benchmarks concurrentes. La preparación del cambio de liberación de spools en fuentes
no modifica ese binario ni estos resultados. Muestreo externo cada 100 ms, intervalos de
notificaciones y contadores Linux con las mismas definiciones/limitaciones descritas arriba.
Logs, parejas, muestras, entorno y resumen local: `target/audit-v5-albuera-2026-10-12/`.

Las tres conversiones finalizaron con código cero, validación profunda y sin errores de
observación. Mediana y mínimo–máximo entre procesos:

| Intervalo | Tiempo (s) |
| --- | ---: |
| Total hasta retorno, antes del resumen | 400,158 [397,782–453,562] |
| PlanningMetadata | 0,050 [0,048–0,050] |
| PlanningSpatialIndex | 75,004 [74,405–78,685] |
| BuildingSpatialHierarchy | 179,764 [178,552–213,306] |
| ReadingAndWritingBlocks | 91,984 [91,741–113,756] |
| ValidatingOutput | 50,579 [50,110–50,966] |
| PublishingOutput | 0,003 [0,003–0,003] |
| Complete hasta retorno | 0,852 [0,774–0,865] |

| Métrica del proceso | Mediana [mínimo–máximo] |
| --- | ---: |
| RSS pico muestreado (MiB) | 101,246 [101,238–106,449] |
| VmHWM máximo observado (MiB) | 101,246 [101,238–106,449] |
| Temporales: pico lógico muestreado (B) | 34.537.564.482 [igual en las tres] |
| Temporales: crecimiento positivo observado (B) | 143.983.844.793 [143.609.752.071–144.096.277.837] |
| `conversion_io_rchar` (B) | 152.901.387.041 [152.901.387.026–152.901.387.051] |
| `conversion_io_wchar` (B) | 148.595.885.658 [igual en las tres] |
| `conversion_io_read_bytes` (B) | 57.192.673.280 [50.362.712.064–62.517.932.032] |
| `conversion_io_write_bytes` (B) | 148.679.835.648 [148.670.361.600–148.696.592.384] |
| `conversion_io_cancelled_write_bytes` (B) | 0 [igual en las tres] |

RSS/VmHWM incluyen el resumen posterior; tiempos e I/O de conversión lo excluyen. La estimación
controlada es **71.020.864 B**; no es RSS ni su límite garantizado.
El pico temporal cuenta candidatos aún no publicados junto a spools, no espacio físico de disco;
el crecimiento muestreado no sustituye I/O exacto. Los contadores lógicos medianos equivalen a
**35,51× lectura / 34,51× escritura** de la fuente comprimida;
la variación de almacenamiento refleja caché/writeback y no se interpreta como tráfico físico.

Invariantes comunes: **508.809.556 puntos fuente**, **508.989.347 filas con LOD**,
**8.437 bloques**, índice serializado **5.071.052 B** contado una vez.
Geometría: **6.105.617.040 B**; atributos: **7.561.226.723 B**;
pareja: **13.666.843.763 B**, ratio **3,174287×** la fuente.
SHA-256 iguales para cada archivo en las tres repeticiones:

```text
oct      8bca49854c48af8a976e6e37b3c569f047dcd2d1f6c6b5d57f89356f22b3be02
octattrs 7b16a940b28eb57350dd3021f1ecaf1505947f8570b9abc48fa711707246abed
```

Esta configuración tampoco alcanza <5 min ni pareja ≤3× fuente. El mayor intervalo es construir
la jerarquía; estos tiempos no separan CPU de I/O. La siguiente mejora local evaluará liberar spools ya
consumidos durante escritura, con una comparación independiente: esta base no demuestra su efecto.
Siguen pendientes pico conjunto por fase, navegador/OPFS y mediciones de presentación GPU.

Validación: MD5 antes de cada proceso, informes completos, invariantes y SHA-256 de ambas salidas.
`summary.json` conserva datos agregados y crudos; `environment.json`, revisión, opciones, entorno y
hash del ejecutable. Este registro no cambia la implementación medida.

```sh
cargo build --profile perf -p viewer_convert --example real_dataset_bench --offline
# Antes de CADA repetición, comprobar el MD5 indicado arriba:
md5sum data/datasets/sensoforest/SensOFOREST_2026_02_22_Albuera_Pointcloud_UAV_LiDAR_20260503.laz
python3 crates/viewer_convert/benchmarks/profile_conversion.py \
  data/datasets/sensoforest/SensOFOREST_2026_02_22_Albuera_Pointcloud_UAV_LiDAR_20260503.laz \
  target/albuera-v5-repro/run-1 --workers 1 --divisions 2 --interval-ms 100
# Repetir con directorios nuevos run-2 y run-3.
```

## Liberación de spools consumidos — 12-10-2026

Comparación con `85fd324` (misma implementación nativa que `fe230af`): se elimina cada spool
hoja/LOD después de escribir su último rango, en lugar de conservarlo hasta destruir el plan.
Los rangos compartidos mantienen el orden de construcción; primero terminan todas las lecturas
paralelas del lote, después se escribe y libera. No añade mapas ni contadores globales de archivos.
Un fallo al eliminar aborta antes de publicar; la limpieza de workspace sigue cubriendo restos.
El spool fuente durable del checkpoint es independiente del enlace desechable del plan: no se borra.
Reanudar reconstruye el plan, incluidos sus spools.

Mismo Linux/Rust/equipo, perfil `perf` sin `RUSTFLAGS`, Belvedere local, un worker/divisiones 2,
262.144 puntos/bloque, error 0,01, presupuesto estimado 512 MiB, sin checkpoint.
Tres pares seriales con orden base/candidato, candidato/base, base/candidato. Antes de **cada**
proceso se lee la fuente completa y se comprueba MD5 `197534e677ae2521a97a114bef005ec9`.
No se vacía caché ni se garantiza residencia; frecuencia CPU, energía y writeback sin fijar.
Binarios copiados antes de las medidas y hashes registrados; sin compilaciones ni otros benchmarks
concurrentes. Muestreo externo a 100 ms y definiciones de fases/RSS/temporales/I/O anteriores.
Logs, binarios, patch, entorno y resumen crudo/agregado: `target/audit-spool-release-2026-10-12/`.
Mediana y rango mínimo–máximo de tres procesos **por variante**:

| Métrica | Base | Liberación temprana |
| --- | ---: | ---: |
| Total conversión (s) | 50,723 [49,946–50,734] | 50,672 [49,631–53,568] |
| Planificación espacial (s) | 10,959 [10,846–11,031] | 11,176 [10,812–12,707] |
| Jerarquía (s) | 20,760 [20,121–20,785] | 20,622 [20,195–23,182] |
| Lectura/escritura (s) | 12,500 [12,489–12,556] | 12,314 [11,459–12,530] |
| Validación (s) | 6,279 [6,224–6,317] | 6,261 [6,166–6,295] |
| Complete hasta retorno (s) | 0,121 [0,113–0,126] | 0,001 [0,001–0,001] |
| RSS pico muestreado (MiB) | 92,898 [92,246–93,180] | 94,582 [93,500–94,867] |
| VmHWM máximo observado (MiB) | 92,898 [92,246–93,180] | 94,582 [93,500–94,867] |
| Temporales: pico lógico muestreado (B) | 4.941.891.420 [4.941.891.420–4.941.891.420] | 4.647.826.988 [4.645.646.520–4.671.709.980] |
| Temporales: crecimiento positivo observado (B) | 17.176.521.262 [17.141.706.233–17.246.082.072] | 17.256.957.166 [17.096.447.692–17.281.479.186] |
| `conversion_io_rchar` (B) | 18.375.221.691 [18.375.221.686–18.375.221.709] | 18.375.221.704 [18.375.221.696–18.375.221.711] |
| `conversion_io_wchar` (B) | 17.756.806.959 [17.756.806.959–17.756.806.959] | 17.756.806.959 [17.756.806.959–17.756.806.959] |
| `conversion_io_read_bytes` (B) | 5.556.133.888 [4.908.154.880–6.082.396.160] | 4.246.888.448 [1.893.785.600–4.754.579.456] |
| `conversion_io_write_bytes` (B) | 17.772.195.840 [17.770.496.000–17.775.779.840] | 17.772.691.456 [17.769.046.016–17.773.826.048] |
| `conversion_io_cancelled_write_bytes` (B) | 0 [0–0] | 0 [0–0] |

Reducción del pico temporal lógico mediano observado: **6,0 %**. En las tres muestras candidatas, el máximo
se observa dentro del intervalo de construcción de jerarquía, antes de consumir los bloques;
no se extrapola a Albuera/Majadas. Los tiempos se solapan y no prueban una aceleración general.
El RSS mediano observado sube de 92,898 a 94,582 MiB; no se concluye ahorro de RAM ni se ha
aislado la causa de esa diferencia.
Liberar spools no reduce los bytes lógicos que se escribieron para construirlos; crecimiento
muestreado e I/O Linux se presentan separados, sin tratarlos como ocupación física del dispositivo.

Los seis procesos finalizan con validación profunda e informes completos sin errores de observación.
Invariantes: 81.682.329 puntos fuente, 81.706.211 filas con LOD, 1.343 bloques, índice 807.558 B;
geometría 981.063.326 B + atributos 610.586.859 B = **1.591.650.185 B**. SHA-256 comunes a las seis:

```text
oct      2faa89194c199dba65955d0442b129d0c5f339a1f8ddf89d1181920f51f82d0d
octattrs 013b8fe420ba82aee1775f08800d0c2dcd94796dc8f096b0824e4d80310fa294
```

Tests: spools compartidos conservados hasta su último rango con 1/2/3 workers; hojas y LOD ausentes
antes de validar; cancelación tras consumir el plan conserva bytes del spool fuente del checkpoint,
reanuda sin releer LAS y produce ambas salidas idénticas a conversión fresca. Pasan tests del workspace,
Clippy nativo con todos los targets, Clippy WASM y formato. Esta medida no valida navegador/OPFS.

Para reproducir, compilar/copiar el ejecutable de `85fd324` como `BASE` y el de esta entrega como
`CANDIDATE`, ambos con el comando `cargo build --profile perf -p viewer_convert --example real_dataset_bench --offline`.
Antes de cada proceso verificar/calentar la fuente con `md5sum`; alternar orden en tres pares,
con directorios nuevos y sin otras compilaciones/benchmarks:

```sh
md5sum data/datasets/belv_2009_digital-aerial_pcd_copc.laz
python3 crates/viewer_convert/benchmarks/profile_conversion.py \
  data/datasets/belv_2009_digital-aerial_pcd_copc.laz target/spool-repro/base-1 \
  --binary BASE --workers 1 --divisions 2 --interval-ms 100
# Repetir con --binary CANDIDATE, luego CANDIDATE/BASE y BASE/CANDIDATE.
# Comparar SHA-256 de ambos archivos en las seis salidas.
```

## Lectura de bloques en orden aleatorio, caché caliente

Entrega sobre `90202f1`: el ejemplo existente añade `read-random INPUT.oct SEED MODE [INDEX]`.
Permuta la lista de bloques con Fisher-Yates/SplitMix64 fijo, sin otra copia del directorio, y
lee **cada bloque una vez**. Imprime orden, semilla y checksum de IDs para reproducir el recorrido.
Los modos y comandos secuenciales conservan su comportamiento; se reutilizan lector, contadores,
CRC/decodificación y percentiles. Estadísticas/permutación quedan fuera de la ventana de lectura.

Misma pareja Belvedere v5, perfil `perf` y entorno anterior. Un calentamiento completo de ambos
archivos, seguido de tres pares alternando orden secuencial/aleatorio **por modo**; semillas 1/2/3.
Caché del SO caliente, sin vaciarla, sin compilaciones/checks concurrentes. Un proceso por muestra;
RSS externo con `/usr/bin/time -v`. Esta prueba mide una permutación completa sin repetición,
no una muestra pequeña de bloques ni una distribución de solicitudes de cámara/red.

| Modo / orden | Lectura + CRC + decode (ms) | p50 por bloque (ms) | p95 por bloque (ms) |
| --- | ---: | ---: | ---: |
| XYZ / secuencial | 2.729,633 [2.711,974–2.758,282] | 1,947 [1,932–1,954] | 4,732 [4,719–4,815] |
| XYZ / aleatorio | 2.723,885 [2.714,736–2.733,697] | 1,947 [1,933–1,955] | 4,717 [4,717–4,752] |
| Completo / secuencial | 6.223,845 [6.154,694–6.265,173] | 4,340 [4,296–4,355] | 11,155 [10,978–11,240] |
| Completo / aleatorio | 6.231,545 [6.207,612–6.292,019] | 4,405 [4,371–4,449] | 11,233 [11,105–11,375] |
| Columna 0 / secuencial | 204,405 [200,455–207,309] | 0,145 [0,142–0,146] | 0,355 [0,347–0,361] |
| Columna 0 / aleatorio | 204,783 [200,411–205,401] | 0,145 [0,140–0,145] | 0,358 [0,348–0,359] |

Mediana y rango entre tres procesos; p50/p95 nearest-rank calculados dentro de cada proceso,
sin combinar muestras. Se verificó igual cantidad de bloques/filas y bytes por archivo entre los
dos órdenes: 1.343 bloques, 81.706.211 filas. XYZ lee 980.255.704 B de payload geométrico y cero
atributos; columna 0 lee 656.109 B de atributos y cero geometría; completo lee 980.255.704 B y
610.586.795 B respectivamente. Con cada semilla, el checksum de orden coincide entre modos.

| Modo | RSS pico secuencial (KiB) | RSS pico aleatorio (KiB) |
| --- | ---: | ---: |
| XYZ | 13.800 [13.772–14.052] | 15.876 [15.872–16.088] |
| Completo | 18.632 [18.580–18.644] | 18.412 [18.248–18.816] |
| Columna 0 | 8.592 [8.328–8.808] | 8.488 [8.428–8.616] |

No se observa una penalización clara de latencia por orden en esta condición caliente. XYZ sí
presenta unos 2 MiB más de RSS en orden aleatorio; la causa no está perfilada y no se atribuye a
la memoria del índice. RSS incluye overhead del benchmark y asignador. No extrapolar a disco
frío, HTTP Range, OPFS, latencia de cámara ni Chrome/Firefox. Logs locales ignorados:
`target/audit-v5-random-read/`.

```sh
cargo build --profile perf -p viewer_convert --example real_dataset_bench --offline
target/perf/examples/real_dataset_bench read-random data/datasets/belv_2009_digital-aerial_pcd_copc.oct 1 geometry
target/perf/examples/real_dataset_bench read-random data/datasets/belv_2009_digital-aerial_pcd_copc.oct 1 all
target/perf/examples/real_dataset_bench read-random data/datasets/belv_2009_digital-aerial_pcd_copc.oct 1 attribute 0
cargo test -p viewer_convert --example real_dataset_bench --profile fast --offline
```

Pendiente: calibrar el modelo/RSS conjunto del builder, conversiones v5 grandes repetidas,
temporales máximos/acumulados e I/O por fase, lectura con caché fría/red/OPFS y Chrome/Firefox
con WebGPU; optimización LOD conforme a su base específica.


## Solicitudes parciales reproducibles

Entrega sobre `b2056cf`: `read-sample INPUT.oct SEED COUNT MODE [INDEX]` selecciona
un prefijo de la misma permutación de `read-random`, sin reemplazo. A igual semilla,
las muestras son anidadas y coinciden entre modos. Cero bloques mide apertura sin payloads;
una cantidad mayor que el total devuelve error antes de leer payloads. Con todos los bloques,
el recorrido coincide con `read-random`. No representa una trayectoria de cámara.

`requested_block_count`, `decoded_payload_points`, checksum, contadores y percentiles
describen únicamente la selección. `read_decode_crc_selected_blocks_ms` mide su lectura.
Los comandos completos conservan también `read_decode_crc_all_blocks_ms`.
`storage_summary_scope=whole_dataset` identifica que los tamaños, filas almacenadas y
estimaciones finales siguen describiendo el dataset completo: no se atribuyen a la muestra.
`selection_ms` mide permutación/selección, excluyendo materializar estadísticas y checksum.
El benchmark conserva estadísticas de todos los bloques y el lector abre todo el índice;
solicitar pocos bloques no hace su memoria proporcional a COUNT.

Mismo Belvedere v5 y equipo de las tablas anteriores, verificados en esta entrega: Linux
7.2.9-200.fc44.x86_64, Ryzen AI 9 365, Rust 1.99.0, perfil perf, 31.885.120 KiB RAM.
Una lectura completa de calentamiento; tres procesos por combinación de 0/1/16/128 bloques
y XYZ/completo/columna 0. Semilla **42 fija** para separar dispersión temporal de cambios de
bloques; la segunda ronda invierte el orden de casos. Caché del SO caliente, sin vaciar,
sin compilaciones concurrentes. RSS externo incluye apertura, lectura y resumen.
Mediana y mínimo–máximo; no son mediciones de mejora frente a otra implementación.

| Bloques solicitados | Modo | Apertura + asociación (ms) | Lectura + CRC + decode (ms) | RSS pico (KiB) | Payload leído (B) |
| ---: | --- | ---: | ---: | ---: | ---: |
| 0 | XYZ | 6.155 [5.018–7.879] | 0.000 [0.000–0.000] | 8400 [8212–8432] | 0 |
| 0 | Completo | 7.669 [4.223–7.903] | 0.000 [0.000–0.000] | 8652 [8412–8740] | 0 |
| 0 | Columna 0 | 6.125 [5.217–7.800] | 0.000 [0.000–0.000] | 8456 [8380–8784] | 0 |
| 1 | XYZ | 7.669 [7.662–7.730] | 2.863 [2.858–2.877] | 8788 [8600–8856] | 430116 |
| 1 | Completo | 7.906 [7.532–7.957] | 6.524 [3.903–6.589] | 8476 [8440–8648] | 706719 |
| 1 | Columna 0 | 7.614 [7.337–7.680] | 0.245 [0.118–0.245] | 8668 [8600–8704] | 293 |
| 16 | XYZ | 7.768 [7.490–7.814] | 45.365 [36.314–52.967] | 11292 [11264–11532] | 12020391 |
| 16 | Completo | 7.729 [7.685–7.815] | 88.361 [87.730–89.283] | 14464 [14384–14744] | 19434865 |
| 16 | Columna 0 | 7.744 [7.676–7.888] | 5.819 [4.977–5.819] | 8760 [8760–8812] | 8039 |
| 128 | XYZ | 7.727 [7.721–7.739] | 275.768 [272.078–278.404] | 14048 [13896–14092] | 95284530 |
| 128 | Completo | 7.645 [4.876–7.697] | 632.118 [630.365–635.879] | 18340 [18236–18348] | 154909087 |
| 128 | Columna 0 | 7.586 [4.746–7.727] | 24.555 [20.343–33.072] | 8596 [8380–8600] | 63748 |

Para un bloque, la apertura cuesta más que leer la columna seleccionada: permite identificar
el coste fijo del índice en esta solicitud, pero no basta para justificar paginarlo. Los bloques
no tienen tamaños iguales y una semilla no caracteriza todas las solicitudes. Los percentiles
por bloque están en los logs; con un bloque p50 y p95 son la misma observación.

En las tres rondas se comprobó igualdad de checksum/filas entre modos y repeticiones por COUNT,
cero lectura de atributos en XYZ, cero geometría en columna 0 y cero payloads con COUNT 0.
Las pruebas unitarias cubren prefijos de varios tamaños, ausencia de duplicados, vacío y exceso;
la comprobación CLI de equivalencia total usa la muestra Outer Banks de un bloque.
Formato, cuatro tests del ejemplo y Clippy de viewer_convert con todos sus targets pasan.
No cambia el lector ni el formato. No valida caché fría, HTTP, OPFS, LOD ni navegación web.

Logs locales ignorados: `target/audit-v5-sampled-read/` (36 procesos, calentamiento y
`results.json`). Reproducir cada comando tres veces en un proceso nuevo; usar COUNT
0/1/16/128 y modos geometry/all/attribute 0, invirtiendo el orden en la segunda ronda:

```sh
cargo build --profile perf -p viewer_convert --example real_dataset_bench --offline
# Calentamiento:
target/perf/examples/real_dataset_bench read data/datasets/belv_2009_digital-aerial_pcd_copc.oct
/usr/bin/time -v target/perf/examples/real_dataset_bench read-sample data/datasets/belv_2009_digital-aerial_pcd_copc.oct 42 16 geometry
/usr/bin/time -v target/perf/examples/real_dataset_bench read-sample data/datasets/belv_2009_digital-aerial_pcd_copc.oct 42 16 all
/usr/bin/time -v target/perf/examples/real_dataset_bench read-sample data/datasets/belv_2009_digital-aerial_pcd_copc.oct 42 16 attribute 0
```

## RSS y temporales por intervalo de conversión — 12-10-2026

Entrega sobre `390fb88`: `convert-profile` conserva tiempos/I/O y añade límites Unix en
nanosegundos por ordinal. El muestreador registra inicio y fin de cada observación y atribuye
RSS y tamaño lógico temporal solo si **toda** la observación cae en un intervalo. No depende
de cuándo se vacía stdout; las fases repetidas siguen separadas. `report.json` añade `phases`
(nombre, ordinal, límites, muestras y dos picos) y `phase_unassigned_sample_count`.

Las muestras que cruzan una transición o caen fuera de la conversión cuentan como no asignadas;
siguen participando en las métricas globales. Una fase sin muestras tiene picos `null`, no cero.
VmHWM permanece global: atribuir su valor a una fase posterior confundiría el máximo histórico
del proceso con memoria de esa fase. Los dos picos por fase pueden ocurrir en instantes diferentes;
no se suman RAM y disco ni máximos de distintas fases. Los JSONL conservan las observaciones
para analizar coincidencias. La agregación recorre el JSONL sin cargar todas las muestras en RAM.

Se requiere reloj de pared estable entre ambos procesos. Se rechazan límites invertidos,
discontinuos, duplicados o ausentes, ordinales incompletos y muestras que retroceden en el tiempo;
eso no detecta todos los ajustes de reloj posibles. Ante metadatos ausentes/inválidos, incluso con
exit cero y pareja publicada, el informe queda incompleto. Recompilar el ejemplo antes de usar
este muestreador; los binarios anteriores carecen de estos límites. Las duraciones existentes
siguen usando `Instant`. Una conversión fallida conserva logs/muestras y no se presenta como éxito.

### Belvedere: tres procesos con atribución por fase

Mismo Linux/Rust/CPU del apartado inicial, disco del checkout, perfil `perf`, un worker,
divisiones 2 y opciones restantes predeterminadas, sin checkpoint. Fuente de 618.390.994 B,
MD5 `197534e677ae2521a97a114bef005ec9` leído antes de cada proceso para verificar/calentar;
no se vació la caché ni se controlaron frecuencia CPU, writeback o carga externa. No hubo
compilaciones ni otros benchmarks concurrentes. Muestreo de 100 ms, sin medir su overhead.
Mediana [mínimo–máximo] de los **picos muestreados por proceso**:

| Intervalo | RSS (MiB) | Temporales lógicos (B) |
| --- | ---: | ---: |
| `PlanningSpatialIndex` | 67,746 [67,305–68,293] | 3.334.178.261 [3.329.329.929–3.335.848.970] |
| `BuildingSpatialHierarchy` | 68,938 [68,281–69,027] | 4.633.019.920 [4.625.878.458–4.651.870.730] |
| `ReadingAndWritingBlocks` | 86,785 [85,082–86,938] | 3.346.811.976 [3.341.551.805–3.350.223.351] |
| `ValidatingOutput` | 93,023 [92,551–93,176] | 1.591.650.185 [1.591.650.185–1.591.650.185] |

Las otras cinco fases no tuvieron muestras enteramente contenidas en ninguna ejecución;
no se infiere su consumo. Totales de muestras 442/500/463, de las que 1/1/2 no fueron asignadas.
Tiempo de conversión monotónico: 46,231 [44,183–49,965] s. Esta ronda no compara una mejora.
El mayor temporal observado pertenece a jerarquía y el mayor RSS muestreado a validación,
en los tres procesos. No identifica qué función interna causa esos máximos ni mide memoria
exclusiva de cada fase: RSS incluye los recursos del proceso que siguen vivos.

Validación profunda del conversor, SHA-256 y `cmp`: ambos archivos coinciden en los tres
procesos y con la pareja v5 de referencia. Coordenadas **981.063.326 B**, atributos
**610.586.859 B**; SHA-256 respectivos:

```text
oct      2faa89194c199dba65955d0442b129d0c5f339a1f8ddf89d1181920f51f82d0d
octattrs 013b8fe420ba82aee1775f08800d0c2dcd94796dc8f096b0824e4d80310fa294
```

Logs/informes/muestras locales ignorados en `target/phase-peaks-belvedere/run-{1,2,3}/`.
Pasan ocho tests Python (transiciones, fases repetidas/vacías, metadatos inválidos,
reloj invertido y éxito aparente), siete tests del ejemplo Rust, Clippy de `viewer_convert`
con todos sus targets, formato, compilación Python y diff. Ruff no está instalado.
Pendientes otras fuentes y lotes con varios workers; tampoco valida navegador ni GPU.

Reproducir tres veces, con directorios nuevos, sin otras compilaciones/benchmarks concurrentes:

```sh
cargo build --profile perf -p viewer_convert --example real_dataset_bench --offline
md5sum data/datasets/belv_2009_digital-aerial_pcd_copc.laz
python3 crates/viewer_convert/benchmarks/profile_conversion.py \
  data/datasets/belv_2009_digital-aerial_pcd_copc.laz target/phase-peaks-belvedere/run-1 \
  --workers 1 --divisions 2 --interval-ms 100
# Repetir MD5 y perfilado para run-2 y run-3; comparar ambas salidas byte a byte.
```

## Albuera y Majadas: picos por fase — 12-10-2026

Seis procesos seriales, tres por fuente, con el conversor de `66ea003` (ya libera spools
consumidos). Binario `perf` fijo, SHA-256
`c4013e7330f97715e0518f4ac072ef4a42442646b2c239a8abfbd4a107ae054e`.
Mismo Linux/Rust/equipo del apartado inicial; un worker, divisiones 2, opciones restantes
predeterminadas, sin checkpoint y sin `RUSTFLAGS`. No hubo conversiones simultáneas,
compilaciones ni benchmarks de navegador durante las medidas. La corrección JS del benchmark
web no cambia este ejecutable nativo. No se fijaron energía, frecuencia, carga externa o writeback.

Antes de cada proceso se leyó/verificó la fuente completa: Majadas **1.343.266.470 B**, MD5
`fd0b4433e805246f2442df037dfadb6b`; Albuera **4.305.484.789 B**, MD5
`71ff22c046d6eb83c371791f680bea13`. Caché sin vaciar; el calentamiento no garantiza residencia
de la fuente ni de spools. Muestreo solicitado cada 100 ms; escaneo y planificación del SO pueden
alargar el intervalo real. No se aisló overhead del muestreador. Se aplican los límites de
atribución descritos arriba: solo capturas enteramente contenidas, VmHWM global y picos breves
posiblemente omitidos. Los picos RAM/disco pueden ocurrir en instantes diferentes.

Mediana [mínimo–máximo] de tres procesos por fuente; tiempos entre notificaciones, RSS del
proceso completo y tamaños lógicos muestreados. No sumar medianas de fases para reconstruir
la mediana total: pueden pertenecer a procesos distintos.

### Majadas TLS

| Intervalo | Tiempo (s) | RSS pico muestreado (MiB) | Temporales pico muestreado (B) |
| --- | ---: | ---: | ---: |
| PlanningSpatialIndex | 43,769 [42,069–44,450] | 45,141 [42,941–51,254] | 13.685.415.744 [13.685.415.744–13.701.751.008] |
| BuildingSpatialHierarchy | 194,680 [186,634–201,831] | 50,664 [48,477–56,812] | 18.804.783.558 [18.799.578.534–18.824.457.894] |
| ReadingAndWritingBlocks | 84,642 [68,819–97,265] | 84,855 [82,844–91,094] | 13.714.267.023 [13.713.615.390–13.721.170.377] |
| ValidatingOutput | 32,922 [32,644–33,265] | 98,703 [96,691–105,004] | 7.533.781.717 [igual en las tres] |

Total hasta retorno **349,051 [347,401–366,716] s**. Muestras totales 3.342/3.631/3.451;
dos no asignadas en cada proceso. Las otras cinco fases no tienen muestras contenidas: picos
`null`, sin inferir consumo cero. VmHWM observado coincide con el RSS pico global en los tres.

### Albuera UAV

| Intervalo | Tiempo (s) | RSS pico muestreado (MiB) | Temporales pico muestreado (B) |
| --- | ---: | ---: | ---: |
| PlanningSpatialIndex | 77,563 [77,220–77,658] | 44,898 [40,461–47,004] | 20.858.408.757 [20.836.460.227–20.858.408.757] |
| BuildingSpatialHierarchy | 232,137 [198,049–283,789] | 50,309 [45,926–52,473] | 27.429.926.943 [27.419.646.264–27.463.839.467] |
| ReadingAndWritingBlocks | 128,001 [124,299–184,744] | 96,750 [91,332–97,879] | 20.870.870.399 [20.869.779.384–20.872.554.105] |
| ValidatingOutput | 52,986 [52,131–54,088] | 110,988 [105,570–112,117] | 13.666.843.763 [igual en las tres] |

Total hasta retorno **513,502 [489,904–539,482] s**. Muestras totales 4.707/5.057/5.004;
dos no asignadas en cada proceso. Las otras cinco fases tampoco tienen muestras contenidas.
VmHWM observado coincide con el RSS pico global en los tres.

### I/O y verificación

Bytes por proceso, mediana [mínimo–máximo]; mismas definiciones Linux anteriores:

| Contador | Majadas | Albuera |
| --- | ---: | ---: |
| `rchar` | 143.601.067.898 [143.601.067.893–143.601.067.908] | 152.901.387.047 [152.901.387.046–152.901.387.052] |
| `wchar` | 142.257.783.952 [igual] | 148.595.885.658 [igual] |
| `read_bytes` | 15.971.323.904 [12.127.416.320–25.597.689.856] | 45.330.464.768 [44.555.083.776–56.913.240.064] |
| `write_bytes` | 142.332.952.576 [142.329.679.872–142.344.290.304] | 148.693.774.336 [148.690.595.840–148.694.028.288] |
| `cancelled_write_bytes` | 0 [igual] | 0 [igual] |
| Crecimiento temporal positivo observado | 138.151.447.690 [136.381.487.927–138.202.995.929] | 143.238.118.012 [141.603.641.653–144.140.568.844] |

Las seis ejecuciones tienen exit cero, validación profunda, informes completos y ninguna
incidencia del observador. Los SHA-256 de ambos archivos coinciden en las tres repeticiones y
con las referencias v5 publicadas arriba. Majadas conserva **415.465.882** puntos fuente,
**415.602.190** filas con LOD, **6.259** bloques e índice **3.455.366 B**; pareja
**4.214.687.109 + 3.319.094.608 = 7.533.781.717 B** (**5,608553×** fuente).
Albuera conserva **508.809.556** puntos, **508.989.347** filas, **8.437** bloques e índice
**5.071.052 B**; pareja **6.105.617.040 + 7.561.226.723 = 13.666.843.763 B**
(**3,174287×** fuente). Índices contados una vez; no cambian precisión, codec ni atributos.

En ambas fuentes y en los seis procesos, el máximo temporal muestreado pertenece a jerarquía
y el máximo RSS muestreado a validación. La jerarquía sigue siendo el intervalo más largo;
Albuera muestra también dispersión importante al escribir bloques. Priorizar el perfil interno
de jerarquía/spools para tiempo/disco, y validar el lote/reader para entender RSS. Los contadores
no aíslan CPU frente a I/O ni memoria exclusiva de una función. No se propone rebajar validación.

Los picos temporales son menores que en las rondas históricas anteriores a liberar spools,
pero RSS es mayor y Albuera también tarda más. **No es una comparación emparejada antes/después**:
no atribuir esas diferencias a una optimización ni prometer aceleración/ahorro RAM. Siguen sin
cumplirse <5 min y pareja ≤3× en ambas fuentes. Falta medir varios workers, condiciones frías,
memoria de navegador y escala; no se cierra la base completa del README.

Logs, parejas, muestras, entorno/binario y datos crudos/agregados:
`target/phase-peaks-large-2026-10-12/{majadas,albuera}/run-{1,2,3}/`, `environment.json`,
`results.json` y `summary.json`. Se comprobaron estados, contabilidad de muestras, invariantes,
hashes de las salidas y que el ejecutable permaneció fijo. Esta entrega de resultados no cambia
código nativo; los ocho tests Python del perfilador pasaron antes de iniciar las medidas.

Reproducir con el comando de perfilado anterior, cambiando fuente y directorio por los de cada
dataset, y leyendo/verificando su MD5 antes de cada una de las tres repeticiones. Preservar
logs y fallos; no ejecutar otros benchmarks o compilaciones simultáneamente.
