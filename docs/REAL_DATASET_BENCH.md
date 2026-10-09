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
total y estadísticas de memoria por tipo. Al repetirlo, elimina primero el `.oct` anterior.

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

### Memoria esperable para render

En la ruta actual, `Point` ocupa 16 B y `InstanceRaw` 32 B. Mantener los 81.682.329 puntos como
`Point` junto con instancias de escena supondría **3.920.751.792 B (3,65 GiB) de geometría residente
en CPU**. Crear una copia completa de instancias para el snapshot añade al menos **2.613.834.528 B
(2,43 GiB)**. Un buffer hipotético con todas las instancias ocuparía otros **2,43 GiB** en GPU. Estos
valores implican al menos **6,09 GiB de geometría simultánea en CPU** si se crea ese snapshot completo,
más la reserva hipotética de GPU de 2,43 GiB. Son multiplicaciones de tamaños de tipo y conteo de
puntos, no mediciones de RAM/GPU del renderer: no existe aún un camino de presentación GPU conectado.
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
