# Benchmark con un escaneo real de gran tamaño

## Dataset y comandos

Se usa una nube fotogramétrica UAV del glaciar Belvedere de 2009: 81.682.329 puntos, RGB, EPSG:7791
y un COPC LAZ de 618.390.994 bytes. La ficha del [dataset](https://doi.org/10.5281/zenodo.19265915) publica
la licencia CC BY 4.0, el tamaño, los puntos y los metadatos. Procedimiento de descarga y comandos en
[`data/datasets/README.md`](../data/datasets/README.md); archivos grandes excluidos de Git.

El benchmark reproducible está en
[`real_dataset_bench.rs`](../crates/viewer_convert/examples/real_dataset_bench.rs). Se compila en
release y `/usr/bin/time -v` mide el máximo RSS por separado para conversión y lectura completa de
bloques. El benchmark informa intervalos por fase, caudal, payload bruto/guardado por codec, tamaño
total y estadísticas de memoria por tipo. Para comparar subárboles espaciales, acepta un cuarto
argumento opcional `SPATIAL_WORKERS` (1 por defecto). Al repetirlo, elimina primero el `.oct` anterior.

## Cómo interpretar las cifras

- `payload_compression_ratio = stored_payload / raw_payload`: ratio LZ4 del payload; menor que 1
  indica ahorro. Incluye atributos conservados y muestras LOD; excluye índice, cabeceras y footer.
- `output_to_input_ratio = archivo .oct / archivo LAZ`: comparación de ficheros completos. La fuente
  ya está comprimida en LAZ y la salida añade índice y LOD.
- `format_overhead_bytes = tamaño .oct - stored_payload`: índice, cabeceras y footer.
- `controlled_memory_estimate_bytes` es el presupuesto del conversor; `Maximum resident set size`
  de `/usr/bin/time` es RSS medido. No son la misma métrica.
- La lectura recorre y decodifica un bloque a la vez; su tiempo incluye I/O, CRC, descompresión y
  reconstrucción a puntos. No aísla el coste puro de LZ4.
- El visor conserva `Point` y `InstanceRaw` cuando mantiene la nube completa: su tamaño concreto se
  mide en el benchmark. La suma es la memoria residente geométrica CPU de esa ruta actual, sin
  arrays de entrada, índices, asignador, buffers temporales ni objetos JS.
- La GPU es una estimación: el renderer actual no crea un dispositivo, buffers, comandos ni frames.
  El tamaño hipotético del buffer usa los bytes de `InstanceRaw` multiplicados por los puntos
  dibujados; LOD y caché acotada deberían reducirlo, pero esa integración aún no está hecha.

## Resultados

Medición del 9 de octubre de 2026, Linux x86_64, AMD Ryzen AI 9 365 (20 hilos lógicos), Rust
1.99.0; conversor y benchmark compilados en `--release`, opciones por defecto (bloques de 262.144
puntos, error de cuantización máximo 0,01, presupuesto de memoria controlada de 512 MiB). Se midió
con `/usr/bin/time -v`; las cifras corresponden a una ejecución en este equipo y no son objetivos de
rendimiento.

### Conversión

| Medida | Resultado |
| --- | ---: |
| Fuente COPC LAZ | 618.390.994 B (589,7 MiB), 81.682.329 puntos |
| Salida `.oct` | 1.596.356.674 B (1,49 GiB), 1.343 bloques |
| Tiempo total | 100,12 s (815.872 puntos/s) |
| RSS pico del proceso | 37.612 KiB (36,7 MiB) |
| Presupuesto controlado informado por el conversor | 62.678.784 B (59,8 MiB) |
| Tiempo de planificación espacial | 22,40 s |
| Tiempo de construcción de jerarquía | 53,84 s |
| Lectura y escritura de bloques | 13,77 s |
| Validación de salida | 9,76 s |
| Payload bruto de bloques | 3.349.954.651 B |
| Payload comprimido LZ4 | 1.596.095.673 B |
| Ratio comprimido/bruto (LZ4) | 0,4765 (2,10×; 52,36 % menos) |
| Índice, cabeceras y footer | 261.001 B |
| Archivo `.oct` / LAZ de origen | 2,581× |
| LOD adicional | 23.882 puntos |

Todos los 1.343 bloques se codificaron con LZ4. El `.oct` ocupa más que el LAZ porque este ya está
comprimido y el formato de trabajo guarda sus coordenadas y atributos en bloques más muestras LOD.
El presupuesto controlado es una estimación interna del conversor; el RSS observado es la memoria
residente reportada por el sistema y puede variar entre ejecuciones.

### Lectura de bloques

| Medida | Resultado |
| --- | ---: |
| Apertura y lectura del índice | 2,87 ms |
| Leer, verificar CRC, descomprimir y reconstruir todos los bloques | 9,81 s |
| Puntos decodificados (fuente + LOD) | 81.706.211 |
| RSS pico del proceso | 27.024 KiB (26,4 MiB) |

La lectura se ejecutó en una pasada, reteniendo un bloque decodificado cada vez. El valor de tiempo
agrupa I/O, CRC, LZ4 y reconstrucción; no mide por separado el coste de CPU del codec ni la primera
vista interactiva.

### Decodificación de posiciones sin vector intermedio

El 10 de octubre de 2026 se midió la misma lectura completa antes y después de cambiar
`FormatReader::read_point_block`: las posiciones se escriben ahora directamente en el vector de
`Point` que se devuelve, y el segmento posterior de colores completa esos mismos puntos. La salida,
conteos, bloques y bytes no cambian. Se ejecutó una sola vez por versión, sobre la misma máquina y
dataset; no se controló una secuencia de caché fría/caliente, así que la diferencia no permite
atribuir mejora de tiempo o RSS.

| Revisión | Lectura de bloques | Tiempo pared | RSS máximo |
| --- | --- | ---: | ---: |
| `3a1812b`, antes | 1.343 bloques; 81.706.211 puntos | 9,945 s | 27.412 KiB |
| Árbol de trabajo, después | Mismo `.oct`, conteos y bytes | 9,870 s | 26.748 KiB |

La asignación eliminada reservaba un vector `[f32; 3]` por punto mientras también existía el vector
final de `Point`. En el mayor bloque de esta nube (229.177 puntos), esa reserva equivale a unos
2,6 MiB. Es una reducción estructural de memoria temporal por bloque; la diferencia de RSS observada
(664 KiB) y de tiempo (75 ms) no es una mejora repetible demostrada. Para cerrarla, hacer tres
lecturas emparejadas por versión con política de caché y orden declarados y comparar medianas.

### Espacio temporal de conversión

En una conversión adicional de la misma revisión, se muestreó cada 100 ms el tamaño de los archivos
regulares bajo el workspace `*.work` y del archivo de salida temporal `*.tmp` junto al destino. La
suma máxima observada fue **8.980.927.112 B (8,37 GiB)**, a los 33,35 s; en esa muestra, todo el
espacio correspondía al workspace de spools y el archivo de salida temporal todavía no existía. La
conversión terminó en 99,68 s con 81.682.329 puntos y las opciones por defecto. El `.oct` producido
para esta medición se eliminó después.

El muestreo puede no capturar picos que duren menos de 100 ms; el valor es un máximo observado en una
ejecución, no un límite garantizado. Mantener al menos 16 GiB libres sigue siendo la recomendación
operativa para esta fuente y configuración, con margen sobre el pico medido. La medición no justifica
por sí sola añadir paralelismo: este queda pendiente hasta comparar tiempo y disco con colas acotadas
y contrapresión.

### Línea de base para evaluar paralelismo

El 9 de octubre de 2026 se repitió una conversión serial con `real_dataset_bench` en la revisión
`35c4ead` (release, mismas opciones y fuente); el resultado de esta ejecución fue:

| Medida | Resultado |
| --- | ---: |
| Tiempo total | 84,78 s (963.492 puntos/s) |
| CPU del proceso (`/usr/bin/time`) | 99 % de un CPU durante la ejecución |
| RSS pico | 36.736 KiB |
| Planificación espacial | 20,56 s |
| Construcción de jerarquía | 39,23 s |
| Lectura y escritura de bloques | 14,24 s |
| Validación | 10,50 s |
| Salida | 1.596.356.674 B; idéntica en tamaño, bloques y payload a la referencia |

La planificación y la jerarquía ocuparon 59,79 s (70,5 % del tiempo medido), y el proceso serial
usó aproximadamente un CPU pese a que el equipo expone 20 hilos lógicos. Esto justificó comparar
subárboles espaciales raíz en paralelo con colas acotadas.

### Comparación de trabajadores espaciales

El 9 de octubre de 2026 se compararon ejecuciones release del mismo binario sobre la misma fuente,
filesystem local y opciones por defecto. El benchmark recibió 1, 2 o 4 trabajadores mediante su
argumento opcional. Cada ejecución se aisló y el pico temporal se muestreó cada 100 ms sumando los
archivos regulares del workspace `.work` correspondiente y el temporal `.tmp`; se excluyó la salida
final. La ejecución serial tuvo dos tiempos válidos, pero solo una muestra de espacio con este
muestreador. Las salidas de todas las ejecuciones produjeron el mismo SHA-256:
`02143f6f66982981a271897e9fe305ddd033ccf4a281120960230e8f35ed9486`.

| Trabajadores | Repetición | Tiempo total | Fase jerarquía | CPU / pared | RSS pico | Pico temporal |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | A | 101,39 s | 52,95 s | 96,6 % | 36,8 MiB | — |
| 1 | B | 102,16 s | 53,36 s | 96,6 % | 36,5 MiB | 8,352 GiB |
| 2 | A | 85,77 s | 36,67 s | 121,9 % | 35,9 MiB | 8,349 GiB |
| 2 | B | 86,30 s | 37,53 s | 120,6 % | 34,0 MiB | 8,348 GiB |
| 4 | A | 81,10 s | 32,07 s | 133,7 % | 37,9 MiB | 8,355 GiB |
| 4 | B | 82,09 s | 33,00 s | 135,0 % | 37,7 MiB | 8,340 GiB |

La media de tiempo total fue 101,77 s, 86,03 s y 81,59 s con 1, 2 y 4 trabajadores: aproximadamente
15 % menos con 2 y 20 % menos con 4. La fase de jerarquía bajó en torno a 30 % y 39 %. El pico
temporal observado se mantuvo entre 8,340 y 8,355 GiB, y el RSS entre 34,0 y 37,9 MiB. El resultado
apoya conservar el paralelismo acotado. Las cifras proceden de dos repeticiones por configuración
salvo el pico serial, que se midió en una sola.

La opción `--spatial-workers` procesa en paralelo solo los hijos directos de la raíz; cada subárbol
mantiene su recorrido serial. Un canal sincronizado de capacidad fija aplica contrapresión a eventos
de progreso, se reúnen resultados en orden de hijo y se reasignan IDs determinísticamente. Cada lote
de trabajadores recibe una parte explícita del presupuesto estimado de memoria, reservando margen
para la integración de los planes. Reanudar desde checkpoints puede cambiar el número de trabajadores.

### Acumuladores indexados para muestras LOD

El 10 de octubre de 2026 se sustituyó el `BTreeMap` consultado por cada punto por una tabla de 512
acumuladores opcionales, indexada por la misma celda 8³. La emisión recorre los índices crecientes,
por lo que se conserva el orden de muestras. Tres conversiones seriales release sobre Belvedere
produjeron el mismo SHA-256 que una repetición de la implementación anterior:
`af78ad1d8d10188773851635ee042d81d25f3176866a3218a6f8103c0af2f977`. Coincidieron también los
conteos, bloques, tamaños y payloads.

| Implementación | Repetición | Tiempo total | Construcción de jerarquía | Salida |
| --- | --- | ---: | ---: | ---: |
| `BTreeMap` | base local | 99,360 s | 53,631 s | 1.596.356.694 B |
| Tabla 8³ | A | 95,613 s | 50,826 s | 1.596.356.694 B |
| Tabla 8³ | B | 96,636 s | 51,494 s | 1.596.356.694 B |
| Tabla 8³ | C | 96,301 s | 51,229 s | 1.596.356.694 B |

La mediana nueva fue 96,301 s total y 51,229 s de jerarquía. Frente a la única base ejecutada con
el mismo árbol de trabajo, las diferencias son −3,1 % y −4,5 %; la base no tiene repeticiones
emparejadas, así que no se considera una mejora concluyente. El hash idéntico valida que no cambiaron
la salida ni los atributos. Repetir la base anterior en el mismo perfil y añadir otros datasets antes
de atribuir la diferencia al cambio.

### Valor predeterminado de trabajadores

Con la tabla 8³, se comparó una conversión serial y otra con cuatro trabajadores en la misma ruta del
SSD local, sin conservar outputs anteriores durante la ejecución. Las dos salidas tuvieron el mismo
SHA-256 indicado arriba y el mismo tamaño. La muestra tmpfs de 91,15 s no se compara con estas cifras.

| Trabajadores | Repetición | Tiempo total | Planificación espacial | Construcción de jerarquía | Presupuesto controlado |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | SSD A | 112,663 s | 24,006 s | 63,903 s | 62.744.320 B |
| 4 | SSD A | 91,153 s | 24,154 s | 41,991 s | 66.086.656 B |

En este par, cuatro trabajadores redujeron el total un 19,1 % y la construcción de jerarquía un
34,3 %, con unos 3,2 MiB más de presupuesto controlado. Es una sola repetición por valor y un dataset;
por eso el valor automático queda acotado a `min(4, available_parallelism)` y el CLI conserva el
override `--spatial-workers`. Repetir en el mismo SSD y en Albuera/Majadas antes de tratar el ahorro
como representativo.

### Una pasada de fuente, LAZ y bloques en paralelo

El 10 de octubre de 2026 se repitió Belvedere tres veces con la implementación optimizada, en el
mismo SSD NVMe, compilación release, tabla 8³, bloques de 262.144 puntos, presupuesto de 512 MiB y
cuatro trabajadores. Cada salida se borró después de registrar su hash para que las ejecuciones
siguientes no heredaran archivos de salida. Las cifras de referencia de cuatro trabajadores arriba
son el control histórico (91,153 s), no una medición intercalada con cada repetición nueva.

| Repetición | Total | Planificación espacial | Jerarquía | Lectura y escritura | Validación |
| --- | ---: | ---: | ---: | ---: | ---: |
| A | 57,824 s | 19,342 s | 26,318 s | 8,911 s | 2,910 s |
| B | 57,869 s | 19,660 s | 25,904 s | 9,117 s | 2,849 s |
| C | 58,098 s | 19,805 s | 25,881 s | 9,210 s | 2,869 s |
| Mediana | **57,869 s** | **19,660 s** | **25,904 s** | **9,117 s** | **2,869 s** |

La mediana total es un 36,5 % menor que la base anterior de cuatro trabajadores (91,153 s), con
33,284 s menos. Las tres salidas conservaron 81.682.329 puntos, 1.343 bloques, 23.882 muestras LOD,
1.596.356.694 B y el SHA-256
`af78ad1d8d10188773851635ee042d81d25f3176866a3218a6f8103c0af2f977`. Esto verifica equivalencia de
salida en este corpus y ejecución, no garantiza el mismo resultado en otros equipos o datasets.

Los cambios acumulados son: la ruta normal LAS/LAZ distribuye los hijos de raíz y acumula LOD en el
mismo recorrido de entrada (el modo checkpoint conserva su spool de fuente); `laz-parallel` habilita
la descompresión paralela de chunks LAZ; preparación/codificación de bloques independientes se hace
en lotes acotados y el append al archivo se mantiene en orden; la validación lee y comprueba bloques
con lectores independientes que comparten el índice inmutable. La mediana de validación bajó a
2,869 s desde aproximadamente 10 s del control previo. No se cambió el formato `.oct` v3. El límite
explícito de ocho trabajadores queda disponible, pero una muestra a ocho fue ligeramente más lenta
que a cuatro con la descompresión LAZ paralela; por eso el valor por defecto sigue limitado a cuatro.

Las mediciones intermedias atribuyeron mejoras sucesivas a las partes del pipeline, pero no forman un
experimento factorial con orden aleatorio. Esta ronda no midió RSS, pico de disco temporal ni bytes de
I/O; el presupuesto controlado reportado por el conversor no equivale a RSS. Repetir al menos en
Albuera y Majadas y medir memoria/almacenamiento antes de generalizar o cambiar el formato.

### SensOFOREST: Albuera UAV y Majadas TLS

El 10 de octubre de 2026 se hizo una ejecución exploratoria de cada dataset con el mismo perfil release
de Belvedere: NVMe local, cuatro trabajadores, divisiones 8³, 262.144 puntos por bloque, error máximo
0,01 y presupuesto controlado predeterminado de 512 MiB. Se ejecutó en secuencia, sin conservar los
`.oct` entre conversiones. Comando base:

```sh
cargo build --release --offline -p viewer_convert --example real_dataset_bench
/usr/bin/time -v target/release/examples/real_dataset_bench convert INPUT.laz OUTPUT.oct 4 8
```

| Dataset | Puntos | LAZ | Tiempo total | Planificación | Jerarquía | Lectura/escritura | Validación | RSS pico |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Albuera UAV | 508.809.556 | 4.305.484.789 B | 378,785 s (wall: 6:18,98) | 143,007 s | 103,334 s | 101,906 s | 28,594 s | 606.840 KiB |
| Majadas TLS | 415.465.882 | 1.343.266.470 B | 395,462 s (wall: 6:35,55) | 94,622 s | 207,917 s | 76,435 s | 15,031 s | 301.844 KiB |

Ambos exceden el umbral esperado de 120 s por más de 3×. La fase dominante depende del corpus: la
planificación espacial en Albuera y la construcción de jerarquía en Majadas. El mayor spool temporal
observado durante muestreos de directorio fue aproximadamente 29 GB en Albuera y 26 GB en Majadas;
no se registró un pico exacto. El RSS medido supera la estimación controlada informada por el
conversor (163.095.616 B y 116.182.528 B, respectivamente), lo que confirma que esta estimación no
es un límite de memoria.

| Dataset | Salida `.oct` | Ratio `.oct`/LAZ | Bloques | Payload LZ4 / bruto | SHA-256 de salida |
| --- | ---: | ---: | ---: | ---: | --- |
| Albuera UAV | 12.444.809.935 B | 2,890× | 102.726 | 0,5955 | `098b3600ebf9a2b0c4a93dfa2686ad03713f83844a50087c4678ddf23d24dc7f` |
| Majadas TLS | 7.404.866.843 B | 5,513× | 37.786 | 0,5395 | `2e7ca9a09b731323ff43fc683cdbfba2af6645a08cb93a52f34a19623bd1c319` |

Albuera queda bajo el objetivo de tamaño `.oct` ≤3× LAZ; Majadas lo supera ampliamente aunque su ratio
de payload comprimido sea mejor. Esto apunta a diferencias en atributos, distribución y overhead por
bloque/índice, y requiere perfilar antes de atribuirlo a un solo factor. Los dos resultados conservan
el conteo de puntos y pasaron la validación completa de salida del conversor. Los MD5 de entrada
coincidieron con los publicados en el registro y documentados en `data/datasets/README.md`. Los
SHA-256 calculados para estas copias fueron: Albuera
`071835a81420e10311821ddd6c06a0f1abf992929c5e3a0f4b5481cec975a90f` y Majadas
`7f99a83a6848b316bddec2b9762ef0771b570d380f4549ee9c79956fbad0bd33`.

Es una muestra de una ejecución por corpus, no una mediana reproducible ni un control intercalado.
No repetir cada conversión de seis minutos antes de aplicar una optimización. Tras reducir el cuello
dominante, repetir tres veces por corpus y registrar el perfil de disco temporal y RSS con el mismo
comando. Las salidas `.oct` de esta medición se eliminaron después de calcular sus hashes.

### Repetición con writer v4 y spool compacto

Ejecuciones release del 10 de octubre, perfil de 4 trabajadores y 8 divisiones, mismos LAZ y
parámetros que la referencia anterior. Se midieron con `/usr/bin/time -v`; las nuevas salidas y
temporales se guardaron bajo `target/format-v4-bench/` y no reemplazaron las conversiones existentes.

| Dataset | Tiempo conversor | Wall | Planificación espacial | Jerarquía | Lectura/escritura | Validación | RSS pico | Spool observado |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Albuera UAV | 346,146 s | 346,49 s | 135,260 s | 84,283 s | 101,639 s | 22,862 s | 675.488 KiB | ≥20 GB |
| Majadas TLS | 356,723 s | 356,83 s | 88,335 s | 179,246 s | 73,939 s | 14,270 s | 315.404 KiB | ≥15 GB |

Las referencias anteriores eran 378,785 s y 395,462 s, respectivamente. En esta única repetición por
dataset, el wall bajó 8,6 % en Albuera y 9,8 % en Majadas; no son ejecuciones emparejadas y el cambio
incluye el writer v4, por lo que no atribuimos toda la diferencia al spool. La reducción de 55 B a
41 B (Albuera) y 33 B (Majadas) sí es exacta por registro. La jerarquía de Majadas continúa siendo el
mayor cuello: 179 s. Ninguna conversión se acerca aún al objetivo de 120 s.

| Dataset | Salida v4 | Ratio salida/LAZ | Bloques | Payload comprimido / bruto | RSS controlado informado |
| --- | ---: | ---: | ---: | ---: | ---: |
| Albuera UAV | 12.435.028.825 B | 2,888× | 102.726 | 0,5936 | 163.095.616 B |
| Majadas TLS | 7.362.893.464 B | 5,481× | 37.786 | 0,5357 | 116.182.528 B |

Los `.oct` v4 quedaron −0,08 % y −0,57 % frente a sus tamaños v3 medidos anteriormente. La columna
independiente no añadió un aumento material al tamaño en estos dos corpus. El RSS pico externo fue
675.488 KiB y 315.404 KiB; el presupuesto controlado no es un límite de memoria. Los tamaños de spool
son muestras observadas durante la ejecución, no picos exactos. Hace falta repetir tres veces antes
de afirmar una mejora estable. El próximo cuello a estudiar es la construcción de jerarquía, en
especial para Majadas, seguido de mediciones explícitas de I/O temporal y reparto de trabajo.

### Microoptimizaciones de recorrido y compilación

El 10 de octubre se aplicaron lecturas y escrituras por lotes en las rutas de spool: lectura de hasta
64 KiB por llamada para reconstruir jerarquía y materializar bloques, codificación de registros en
buffer y escritura de fuente en lotes de 32 KiB. Los buffers de fanout pasaron de 8 a 32 KiB, y el
presupuesto de memoria controlado incluye ese tamaño. También se simplificó la distancia al centro
del LOD a tres restas y multiplicaciones explícitas. Estas modificaciones reducen llamadas de I/O y
trabajo repetido, pero la conversión sigue teniendo scatter a spools, compresión y verificaciones
por punto.

| Albuera, 4 workers × 8 divisiones | Wall | Planificación espacial | Jerarquía | Lectura/escritura | Validación | RSS pico | Salida |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| v4 antes de microoptimizaciones, `release` | 346,49 s | 135,260 s | 84,283 s | 101,639 s | 22,862 s | 675.488 KiB | 12.435.028.825 B |
| I/O por lotes, `release` | 297,36 s | 115,154 s | 64,400 s | 92,591 s | 22,597 s | 757.608 KiB | 12.435.028.825 B |
| I/O por lotes, `perf` + `target-cpu=native` | 233,81 s | 77,783 s | 56,564 s | 80,844 s | 16,208 s | 706.444 KiB | 12.435.028.825 B |
| Experimento temporal: precálculo de rejilla, mismos flags `perf`/nativos | 234,14 s | 77,007 s | 54,375 s | 83,806 s | 16,366 s | 721.008 KiB | 12.435.028.825 B |

Cada fila es una ejecución, no una mediana emparejada. El cambio de buffers en `release` coincidió
con −14,2 % de wall y −23,6 % de fase de jerarquía frente a la primera fila; aumentó el RSS pico
observado en 82.120 KiB. No debe atribuirse todo el cambio a vectorización. El perfil `perf` más
`target-cpu=native` redujo otros 63,55 s frente al `release` ya optimizado (−21,4 % observado), pero
mezcla nivel de optimización, LTO, unidades de código e ISA del host. La repetición con la rejilla
varió +0,14 % en wall frente a la fila anterior: no demuestra mejora medible de esa microoptimización.
Todas las salidas tuvieron el mismo tamaño, conteo de bloques y estadísticas de payload; no se
compararon hashes del archivo entero en estas pasadas. `perf stat` no está instalado/disponible en el
entorno, así que no hay contadores de ciclos, ramas o instrucciones SIMD.

El experimento de rejilla conservaba límites y tamaños de celda por nodo y usaba comparaciones con
el punto medio para dos divisiones. La prueba de equivalencia pasó, pero la medición no mostró una
mejora; se retiró del código de producción por su complejidad adicional. El camino de coordenadas es
irregular (cada punto escribe en un spool e índice de muestra determinados por sus valores), y mezcla
validación, extracción de atributos e I/O; por eso no se añadió SIMD manual. LLVM/Rust habilitan vectorizadores automáticos para bucles
adecuados, pero eso no garantiza vectorización del scatter de esta pipeline. `std::simd` sigue siendo
una API inestable; cualquier kernel intrínseco o portable SIMD debe aislarse, conservar un camino
portable y aportar diagnóstico/benchmark de código máquina antes de incorporarse.

También se revisaron las ramas por punto. Las comprobaciones de coordenadas finitas, límites de
cabecera y esquema de atributos validan entrada y no se eliminan. La presencia de atributos se
comprueba una vez por bloque; opciones GPS/NIR/RGB son invariantes por archivo y predecibles durante
su recorrido. Especializarlas en kernels separados podría quitar ramas, pero multiplicaría rutas y
código y no hay perfil de ramas que demuestre que sea un cuello. Mantenerlas hasta tener contadores.
Próximo cierre: repetir Albuera y Majadas tres veces en `release` y `perf` con disco/caché controlados,
registrar hash exacto y medir I/O temporal. El perfil nativo solo es comparable en este host; no es
una configuración portátil recomendada.

Comando reproducible del binario `perf` nativo:

```bash
RUSTFLAGS="-C target-cpu=native" cargo build --profile perf --offline -p viewer_convert --example real_dataset_bench
/usr/bin/time -v target/perf/examples/real_dataset_bench convert INPUT.laz OUTPUT.oct 4 8
```

Referencias primarias: [vectorizadores LLVM](https://llvm.org/docs/Vectorizers.html),
[perfiles Cargo](https://doc.rust-lang.org/cargo/reference/profiles.html),
[opciones de generación de código de rustc](https://doc.rust-lang.org/rustc/codegen-options/index.html)
y [estado de portable SIMD](https://doc.rust-lang.org/nightly/unstable-book/library-features/portable-simd.html).

### Memoria esperable para render

En una ruta hipotética que materialice el dataset completo, `Point` ocupa 16 B e `InstanceRaw` 32 B.
Mantener los 81.682.329 puntos como
`Point` junto con instancias de escena supondría **3.920.751.792 B (3,65 GiB) de geometría residente
en CPU**. Crear una copia completa de instancias para el snapshot añade al menos **2.613.834.528 B
(2,43 GiB)**. Un buffer hipotético con todas las instancias ocuparía otros **2,43 GiB** en GPU. Estos
valores implican al menos **6,09 GiB de geometría simultánea en CPU** si se crea ese snapshot completo,
más la reserva hipotética de GPU de 2,43 GiB. Son multiplicaciones de tamaños de tipo y conteo de
puntos, no mediciones de RAM/GPU. La demo actual sí presenta bloques LOD visibles.

## Máquina de referencia local

Instantánea observada el 9 de octubre de 2026 para reproducir trabajo local en este checkout; sirve
como perfil de benchmark, no como mínimo de soporte ni como declaración de compatibilidad:

| Componente | Referencia |
| --- | --- |
| Sistema operativo | Fedora Linux 44 Workstation, x86_64 |
| CPU | AMD Ryzen AI 9 365, 10 núcleos / 20 hilos lógicos |
| RAM visible al sistema | 30,4 GiB |
| GPU | AMD Radeon 880M integrada; memoria compartida con RAM |
| Almacenamiento del checkout | NVMe local, Btrfs |
| Firefox instalado | 157.0; en la sesión de validación no expuso `navigator.gpu` |
| Chromium de prueba | Chrome for Testing 153.0.8010.36; WebGPU activado con flags Vulkan |

Las tablas de conversión y lectura no miden GPU. En Chromium se comprobó la carga del `.oct` de
muestra, la propagación del error de un archivo inválido y la cancelación de la descarga de la muestra;
el renderer informó que presentó frames. La captura headless no permitió confirmar visualmente los
puntos y Firefox no expuso WebGPU en esta sesión. La compatibilidad visual y los mínimos oficiales de
RAM/GPU siguen abiertos.
Actualización del 10 de octubre de 2026: Chrome/WebGPU ya se comprobó visualmente con conversiones v3
del Stanford Bunny y del escáner costero Outer Banks. Esa prueba confirma la presentación de estos
datasets pequeños; no sustituye las mediciones de memoria o escala con Belvedere ni la validación de
Firefox.
Una ruta que lea solo bloques LOD visibles y mantenga caché limitada debe presupuestarse por bytes de
bloques seleccionados; el máximo payload bruto individual observado fue 9.396.257 B (8,96 MiB), pero
falta integrar y medir esa ruta de extremo a extremo.

El conversor omitió flags synthetic/key-point/withheld/overlap, flags de dirección/edge de vuelo,
scanner channel y metadatos CRS/WKT; el log de conversión enumera estas dimensiones.

## Buffers intermedios del conversor

La planificación conserva un spool binario de 55 B por punto. Antes de la escritura directa, la
lectura de cada lote también copiaba atributos LAS a varios `Vec` y materializaba una `Vec<SpoolPoint>`
antes de serializarla. Ahora consume los iteradores de `PointData` y escribe cada `SpoolPoint` al
spool en el mismo recorrido, sin esas copias de lote. El archivo `.oct` resultante coincidió byte por
byte con la referencia y los tests de conversión de LAS/LAZ siguen cubriendo atributos estándar y
opcionales.

En una ejecución por versión, el RSS máximo pasó de 37.612 KiB a 36.068 KiB, mientras el tiempo pasó
de 100,12 s a 101,16 s. Esta diferencia no demuestra una reducción estable ni una regresión de tiempo:
no son repeticiones emparejadas y el RSS puede variar por el runtime y el sistema. El presupuesto
controlado de 192 B por punto se mantiene sin cambios; hace falta repetir la comparación antes de
ajustar ese margen o afirmar una mejora de rendimiento.

### Spool temporal compacto

Implementado después de la referencia anterior. Los puntos fuente guardan los tres enteros LAS
originales (`i32`) y aplican escala/offset al leerlos; esto conserva las coordenadas exactas incluso
cuando invertir desde `f64` pierde precisión por un origen grande. Los centros sintéticos LOD siguen
guardándose como `f64`. El esquema del registro omite atributos que el formato fuente no contiene y
mantiene siempre RGB de visualización. El layout del spool también gobierna offsets y lecturas de
bloques planificados. Los manifiestos de checkpoint v1/v2 se rechazan porque sus registros usan el
layout antiguo.

Tamaños de registro verificados por pruebas de codificación y lectura:

| PDRF | Registro fuente | Muestra LOD | Cambio fuente vs 55 B | Cambio LOD vs 55 B |
| ---: | ---: | ---: | ---: | ---: |
| 0 | 27 B | 39 B | −50,91 % | −29,09 % |
| 2 (Majadas) | 33 B | 45 B | −40,00 % | −18,18 % |
| 3 (Albuera) | 41 B | 53 B | −25,45 % | −3,64 % |
| 8 | 43 B | 55 B | −21,82 % | 0 % |

Estos porcentajes describen el tamaño por registro, no el pico temporal ni el tiempo de conversión.
Aún falta repetir las conversiones completas de Albuera y Majadas con el perfil release y writer v4 actuales y medir
duración por fase, bytes temporales/pico, RSS y salida v4. Por tanto, el objetivo de 120 s sigue sin
demostrar y no se atribuye una mejora de rendimiento a la reducción del spool hasta completar esas
medidas.

## Formato v4: lectura geométrica y columnas independientes

El writer v4 escribe por defecto geometría y cada atributo como payload independiente, referidos por
el mismo `block_id` y orden de filas. `FormatReader::read_geometry` evita leer columnas de atributos;
`read_attribute` carga solo la columna solicitada. La vista progresiva WASM usa geometría para RGB y
elevación, y solicita intensidad/clasificación solo para esos modos. Cambiar de modo vacía los bloques
decodificados para reconstruir el corte con la columna correcta. `read_attribute_row` sigue
descomprimiendo la columna completa de ese bloque.

Primera comprobación release con Stanford Bunny, 40.256 puntos (misma fuente LAS de 805.347 B). Una
conversión v4 tardó 47,662 ms en una única ejecución y produjo 413.602 B; el `.oct` histórico de esta
muestra tiene 413.287 B y versión 1. No se usa como comparación de conversión emparejada.

Tres lecturas secuenciales de cada modo sobre cada archivo; tiempos de payload con caché caliente del
sistema y bytes contados por el lector:

| Archivo | Versión | Bytes de archivo | Solo geometría: bytes leídos | Todo: bytes leídos | Mediana solo geometría | Mediana todo |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Bunny histórico | 1 | 413.287 | 412.663 | 412.663 | 8,446 ms | 8,101 ms |
| Bunny nuevo | 4 | 413.602 | 410.752 | 412.726 | 5,326 ms | 6,922 ms |

En v4, leer solo geometría omitió 1.974 B de atributos en este archivo; las columnas `intensity` y
`classification` ocuparon 328 B y 169 B al leerlas individualmente. El nuevo archivo fue 315 B mayor
que el histórico. Los tiempos de unos pocos milisegundos y un solo bloque no permiten atribuir una
mejora general: la ventaja demostrada aquí es que el lector no toca las otras columnas. Faltan las
medidas v4 de Belvedere, Albuera y Majadas y una comparación de lectura sobre bloques LOD grandes.
El primer intento de convertir Albuera falló por cuota de `/tmp`; un segundo intento reubicado en el
volumen del checkout se interrumpió sin resultado con el spool temporal en 27 GB. Ninguno cuenta como
benchmark. Repetir esas medidas con una cuota suficiente y registrar explícitamente el pico temporal.

Desde la limpieza pre-release del 10 de octubre, el lector soporta solo v4. Las medidas v1–v3
conservadas aquí son evidencia histórica; sus archivos deben reconvertirse desde LAS/LAZ/EPT para
abrirlos con la versión actual.
