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

Pendiente: calibrar el modelo/RSS conjunto del builder, conversiones v5 grandes repetidas,
temporales máximos/acumulados e I/O por fase, lectura aleatoria y Chrome/Firefox con WebGPU.
