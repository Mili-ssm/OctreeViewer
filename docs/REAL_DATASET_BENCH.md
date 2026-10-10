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
