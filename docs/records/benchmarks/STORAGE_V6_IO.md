# V6: solapamiento de escritura y lectura paralela — 14-10-2026

Objetivo: reducir tiempo de conversión y carga manteniendo contratos y bytes de v5/pre1–pre4,
con un balance explícito de concurrencia y memoria. No requiere una revisión física nueva.

## Escritura nativa

`ConvertOptions::with_pipelined_writes(true)` / CLI `--pipeline-writes` habilita para LAS/LAZ
un productor que lee los spools y prepara bloques con Rayon mientras el consumidor escribe
el lote anterior. El canal de capacidad cero admite como máximo dos lotes: el que se escribe
y el que se prepara o espera a ser entregado. No hay cola creciente ni escritura fuera de orden.
El callback de progreso y los sinks permanecen en el hilo llamante. El cierre de la pareja,
`sync_all`, validación y publicación conservan su secuencia.

El planificador usa el presupuesto `--max-memory-mib` y el mismo modelo conservador por
bloque de la ruta existente. Cuenta ambos lotes antes de elegir su anchura; con poco margen
puede reducir workers o deshabilitar el solapamiento. Aumentar el presupuesto puede recuperar
anchura, pero no garantiza menor tiempo. El modelo y la capacidad codificada de un lote no
son RSS ni un límite estricto del asignador. Las duraciones de preparación y escritura se
solapan: su suma ya no representa tiempo de pared. EPT conserva su ruta serial y no usa esta
opción; WASM/Blob/OPFS tampoco cambian.

En error/cancelación se descarta el receptor para desbloquear al productor y se espera su
finalización antes de limpiar temporales. Como máximo puede terminar la preparación del
lote siguiente; no se promete cancelación dentro de un codec. Un spool compartido se borra
solo tras escribir su último rango, manteniendo el orden de planificación.

Pruebas: salida idéntica pre1–pre4 con RGB/atributos, solapamiento observable mediante
sincronización, orden estable, propagación de error del productor/consumidor y cancelación
sin publicar parejas. La configuración anterior sigue siendo el valor predeterminado.

## Lectura paralela experimental

El ejemplo `storage_bench` admite `progressive 1 MAX_DECODED_MIB --workers N` (1–8).
Con N=1 conserva la ruta serial. Con N>1 abre cursores independientes sobre la misma pareja
inmutable y comparte el índice validado. Los workers persistentes reciben lotes de hasta N
bloques consecutivos del BFS; el agregador verifica y calcula el checksum en el orden serial.
Retiene como máximo N bloques decodificados, sin reconstruir la nube completa. El preflight
multiplica el mayor bloque estimado por N y rechaza antes de leer payloads si excede presupuesto.
Esta estimación excluye metadatos del asignador y buffers transitorios: no es un límite de RSS.

`progressive_wall_ms` mide recorrido, coordinación y verificación; `full_ms` también incluye
apertura/preflight. `read_ms` y las latencias por nodo suman duraciones de bloques y pueden
superar el tiempo de pared en paralelo. `worker_threads` cuenta hilos adicionales (0 serial).
La verificación/checksum continúa serial, de modo que limita el speedup observable. El pool
queda en el ejemplo: todavía no añade una API de lotes concurrentes al lector del producto.

## Siguientes propuestas

La validación del conversor ya distribuye bloques entre lectores independientes con índice
compartido (`fork_reader`). Un runtime async no acelera por sí solo la decodificación ni
convierte `Read + Seek` en I/O no bloqueante. Primero se mide paralelismo nativo acotado;
una API async/remota necesitaría contrato de transporte, presupuesto y cancelación propios.

Prioridades tras esta medición:

1. Medir XYZ, RGB y una columna por separado con 1/2/4 lectores y presupuesto simultáneo.
   El beneficio de lectura completa no demuestra beneficio en consultas selectivas.
2. Perfilar el selector de codecs: [`encode_pre4`](../../../crates/viewer_format/src/attribute_codec.rs) evalúa raw/LZ4, diccionario y planos de bytes.
   Antes de saltarse candidatos, comparar CPU/RSS, bytes finales y casos de alta entropía.
   Un filtro por muestreo puede perder el candidato óptimo; no cambiarlo silenciosamente.
3. Retomar grupos RGB exactos solo si sus tablas/IDs mejoran tamaño y consultas RGB sin
   penalizar lectura de otra columna. El [prototipo de diccionario de tuplas](STORAGE_V6_PRE4_BENCH.md) ya mostró ese
   riesgo; no reintroducir dependencia de columnas descartada por pre4 sin nueva evidencia.
4. Coalescer rangos/prefetch para HDD/remoto requiere contar seeks/solicitudes y bytes extra,
   medir latencia real y limitar datos en vuelo. NVMe caliente no permite cerrar ese criterio.

La acumulación persistente de escena y las propuestas de grilla común son trabajos distintos;
esta mejora del pipeline de almacenamiento no mide sus vidas simultáneas ni su viabilidad.

## Evidencia

Belvedere completo: 81.682.329 puntos fuente, 1.343 bloques y pareja pre4 de 1.404.306.523 B.
Tres rondas alternadas por configuración, 4 workers solicitados, bloques de 262.144 puntos,
división 2 y perfil `perf`. SHA-256 de ambos archivos idéntico en las nueve conversiones.

| Configuración | Total mediano [mín., máx.] (s) | Stream + cierre/sync mediano (s) | Pico RSS mediano [mín., máx.] (MiB) |
| --- | --- | --- | --- |
| Anterior, 512 MiB | 42,681 [40,960; 42,823] | 19,319 | 242,8 [232,3; 263,96] |
| Pipeline, 512 MiB | 41,959 [40,096; 42,745] | 18,562 | 271,8 [248,4; 281,8] |
| Pipeline, 1.024 MiB | 42,213 [41,027; 42,263] | 18,479 | 239,3 [237,4; 250,7] |

El intervalo stream+cierre/sync baja aproximadamente 3,9 % a 512 MiB; el total mediano baja
1,7 %, con rangos solapados. Es un piloto de N=3, no evidencia de mejora universal ni de
significancia estadística. Los intervalos de fase se observan en stderr de la CLI e incluyen
finalización/sync; no son perfiles internos del encoder. La suma de CPU de usuario/sistema
no equivale a tiempo de pared. RSS cambia entre procesos incluso con igual anchura de lotes;
no atribuir el menor RSS observado a elevar el presupuesto.

Ambos presupuestos ya permiten 4 workers por lote para esta fuente: 1.024 MiB no incrementa
concurrencia ni aporta una mejora demostrada. El valor predeterminado sigue desactivado.
NVMe local Btrfs, caché sin purgar y carga del host sin controlar; compilaciones/tests breves
coincidieron con algunas conversiones. No extrapolar a HDD, remoto, EPT, WASM o GPU.
Comandos, rondas, hashes, CPU, RSS y tiempos de fase están en
[la evidencia de conversión](data/storage-v6-io-conversion-2026-10-14.json).

### Lectura BFS completa

Misma pareja pre4 de Belvedere, tres rondas alternadas por número de lectores, perfil `perf`,
presupuesto decodificado estimado de 256 MiB. Ejecuciones secuenciales después de las
conversiones; sin purgar caché. Incluye verificación serial de todas las filas y columnas.

| Lectores | Total mediano [mín., máx.] (s) | Pico RSS mediano [mín., máx.] (MiB) |
| --- | --- | --- |
| 1 | 11,131 [11,108; 11,138] | 20,5 [20,4; 20,6] |
| 2 | 9,054 [8,916; 9,959] | 30,5 [30,4; 30,6] |
| 4 | 6,918 [6,727; 7,106] | 46,2 [46,1; 50,6] |

Cuatro lectores reducen la mediana observada un 37,8 % (1,61×) respecto a uno; suben el RSS
mediano unos 25,8 MiB. Los nueve recorridos conservan 81.706.211 filas con LOD, 1.343 bloques
y checksum BFS `0737ca650c6081b4`. `read_ms` suma duraciones concurrentes y no es el speedup:
la comparación usa tiempo total de pared. El hash serial consume unos 2,7–2,9 s y limita el
escalado. El presupuesto de 256 MiB es una estimación de datos vivos, no el RSS observado.
[Evidencia de lectura](data/storage-v6-io-reading-2026-10-14.json).

Se verifica también error CRC en un payload de la muestra V5 con cuatro workers (termina sin
bloquearse), rechazo de `--workers 0` y rechazo de presupuesto de 1 MiB en Belvedere antes de
leer payloads. [Casos de error](data/storage-v6-io-errors-2026-10-14.json).

Reproducción después de compilar con `--profile perf`:

```sh
target/perf/viewer_convert INPUT.laz OUTPUT.oct --format-version v6.4-pre \
  --spatial-workers 4 --max-memory-mib 512 --pipeline-writes
target/perf/examples/storage_bench OUTPUT.oct progressive 1 256 --workers 4
```

La mejora medida de lectura corresponde al ejemplo nativo; no está integrada en la carga del
viewer, Blob/OPFS ni consultas selectivas. Para incorporarla al producto quedan contrato de
lotes/cancelación y pruebas por consumidor. No se ha cambiado la revisión física del formato.

## Validación de esta entrega

Pasan `cargo test -p viewer_core -p viewer_format -p viewer_convert --all-targets`, Clippy de
los tres paquetes y todos sus targets con `-D warnings`, `cargo fmt --all -- --check` y
`git diff --check`. Incluye 47 tests de biblioteca del conversor y los tests del ejemplo de
almacenamiento. No se ha modificado interfaz web ni se atribuye validación del renderer,
WASM o workspace completo a estos checks. Se preservan los cambios previos del checkout.
