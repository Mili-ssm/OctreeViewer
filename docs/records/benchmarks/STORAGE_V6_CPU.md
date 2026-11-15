# CPU de diccionarios y checksums — 14-10-2026

Estas mejoras conservan los bytes físicos de v5/pre1–pre4. No se añade una revisión ni se
omiten candidatos de compresión para ganar tiempo. Los resultados nativos no certifican
el rendimiento del navegador, el renderer, HDD ni almacenamiento remoto.

## IDs de diccionario por bytes

Los codecs de atributos de pre3/pre4 empaquetaban y extraían cada ID bit a bit. Ahora el
encoder acumula el ID en un `u64` y emite bytes completos; el decoder rellena un acumulador
con bytes y extrae el ID mediante máscara/desplazamiento. El máximo es 32 bits por ID y
39 bits retenidos en el acumulador. Se conserva el stream LSB-first, incluido padding cero.
No se añade memoria proporcional al dataset ni una copia de las filas originales.

Permanecen los checks de tamaño, CRC antes de expansión, padding e ID dentro del diccionario.
La lectura de un stream truncado devuelve error. Un oracle independiente bit a bit compara
packing y extracción para cada anchura 0–32 y colas/cruces de byte, incluidos 257 IDs.
Las pruebas existentes cubren tipos exactos, NaN/bits, columnas independientes y corrupción.

Comparación completa en Belvedere: 81.682.329 puntos fuente, 1.343 bloques, pareja de
1.404.306.523 B. Tres rondas alternadas, pipeline habilitado, 4 workers, 512 MiB y perfil
`perf`. BFS con 1/4 lectores y presupuesto decodificado estimado de 256 MiB.

| Operación | Antes, mediana [mín., máx.] (s) | IDs por bytes, mediana [mín., máx.] (s) |
| --- | --- | --- |
| Conversión completa | 42,822 [41,820; 43,356] | 40,598 [40,524; 42,258] |
| BFS, 1 lector | 11,242 [11,192; 11,931] | 10,545 [10,500; 10,700] |
| BFS, 4 lectores | 8,742 [7,026; 9,127] | 8,876 [6,895; 9,433] |

Conversión mediana −5,2 %, con rangos solapados; BFS serial −6,2 %. El recorrido con cuatro
lectores varía mucho y no demuestra una mejora. CPU de usuario mediana: conversión 91,77 →
89,29 s; BFS serial 10,88 → 10,25 s. RSS mediano: conversión 244,5 → 246,5 MiB y BFS serial
20,49 → 20,62 MiB; no se demuestra una reducción de RAM. Las seis parejas conservan SHA-256
idéntico y las doce lecturas conservan 81.706.211 filas y checksum `0737ca650c6081b4`.

[Comandos, binarios y evidencia](data/storage-v6-packed-ids-2026-10-14.json).
El runner conserva la primera pareja antes como fuente común y elimina parejas redundantes
solo tras verificar hashes. No purga caché ni aísla carga del host; pruebas breves de CRC
coincidieron con algunas conversiones de esta serie. N=3 no acredita mejora universal.
Un [smoke con Outer Banks](data/storage-v6-packed-ids-smoke-2026-10-14.json) comprueba el
runner actualizado (sintaxis, CLI, conversiones y lecturas); no se usa como benchmark de velocidad.

## CRC IEEE por grupos de ocho bytes

El CRC de todos los payloads usa el polinomio IEEE reflejado `0xEDB88320`, inicialización y
XOR final `0xffffffff`. La variante slicing-by-8 deriva siete tablas de la original y reduce
la dependencia byte a byte a una combinación de ocho consultas. Carga explícitamente LE,
acepta slices sin alineación, procesa la cola con la recurrencia anterior y no usa SIMD,
`unsafe`, dependencias nuevas ni buffers heap por payload. Las tablas lógicas ocupan 8 KiB
(frente a 1 KiB); el estado incremental sigue siendo un `u32`.

Validación previa a medir: vector IEEE `123456789`, oracle independiente que recorre el
polinomio bit a bit, offsets 0–7, longitudes 0–257 y todos sus puntos de corte incremental;
payload de 65.537 bytes en chunks de 1/7/8/9/63/1.024/4.093 bytes. El CRC sigue comprobándose
antes de expandir o devolver payloads; no se sustituye por CRC-32C.

La comparación CRC se ejecuta después de cerrar IDs: ambos binarios ya contienen IDs por
bytes y mantienen iguales opciones. No se atribuyen al CRC los ahorros de empaquetado de IDs.


Control aislado en un mismo buffer determinista de 64 MiB, nueve rondas alternadas y perfil
`perf`: mediana bytewise 152,774 ms [152,687; 154,431], slicing-by-8 34,440 ms
[34,332; 34,492], 4,44×. Todos los checksums coinciden. Incluye únicamente cálculo CRC sobre
RAM, no I/O, parsing, compresión ni verificación de filas: no equivale al speedup de V6.
[Comandos, muestras y binario](data/storage-v6-crc-micro-2026-10-14.json).

Comparación completa CRC, misma fuente/opciones que IDs, tres rondas alternadas por variante.
Ambos binarios ya incorporan IDs por bytes; la única diferencia de runtime es el CRC.
Esta serie se ejecuta secuencialmente sin compilación/tests concurrentes, con caché y carga
externa del host sin controlar.

| Operación | CRC bytewise, mediana [mín., máx.] (s) | CRC slicing-by-8, mediana [mín., máx.] (s) |
| --- | --- | --- |
| Conversión completa | 41,661 [39,942; 49,593] | 40,399 [39,693; 40,816] |
| BFS, 1 lector | 10,529 [10,484; 11,056] | 8,951 [8,926; 9,058] |
| BFS, 4 lectores | 8,418 [6,239; 9,009] | 7,774 [7,454; 7,977] |

Conversión mediana −3,0 %, con rangos solapados y una ejecución control más lenta. CPU de
usuario mediana 87,26 → 83,37 s (−4,5 %). BFS serial −15,0 %, con CPU de usuario
10,22 → 8,67 s. Cuatro lectores bajan la mediana un 7,7 %, pero sus rangos se solapan y hay
una regresión individual: no acredita ganancia general en paralelo. El microbenchmark 4,44×
no se extrapola al tiempo total. No sumar porcentajes entre las dos series como speedup medido.

RSS mediano serial 20,63 → 20,59 MiB, paralelo 46,29 → 48,09 MiB y conversión
251,5 → 243,5 MiB; los picos de conversión varían ampliamente (211,7–290,1 MiB entre casos).
No se atribuyen esas diferencias al ahorro de memoria: el cambio sustituye tablas constantes,
sin alterar presupuesto, workers ni buffers heap por payload.

Las seis parejas conservan el mismo SHA-256 y las doce lecturas conservan las 81.706.211
filas, 1.343 bloques y checksum BFS `0737ca650c6081b4`. El CRC optimizado se mantiene como
implementación común compatible; v5 sigue siendo el formato por defecto y V6 final sigue
rechazado. [Comandos, binarios, CPU/RSS y rondas CRC](data/storage-v6-crc-2026-10-14.json).

## Validación y próximos experimentos

Pasan tests de core/format/convert con todos sus targets, Clippy con `-D warnings`, comprobación
WASM de format, fmt y diff. El runner se verifica con éxito real pequeño y pruebas de mismatch
que conservan ambas parejas. Los checks de WASM acreditan compilación; rendimiento y ejecución
en navegador no se midieron. No se modifica interfaz web ni se valida la migración del renderer.
Ruff no está instalado; Python se comprueba mediante sintaxis/CLI y la suite unittest disponible.

Candidatos al cerrar la primera ronda (estado actualizado al final de este documento):
la tabla u8 ya está integrada y el [experimento de tiles](STORAGE_V6_BYTE_PLANES.md) está cerrado
sin cambio del codec. La lectura selectiva paralela sigue pendiente en el
[estado](../../estado.md#prioridades-pendientes).

- Tabla directa de 256 entradas para diccionarios de columnas `u8`, conservando orden de
  primera aparición. Comparar CPU y bytes exactos frente a HashMap; no omitir candidatos.
- Transposición por tiles en planos de bytes, especialmente RGB y columnas de 8 bytes:
  medir encode/decode y locality con patrones reales y alta entropía, manteniendo bits exactos.
- Extender lectura paralela al benchmark selectivo XYZ/RGB/columna y medir solicitudes/rangos
  antes de introducir una API concurrente o prefetch en Blob/remoto/HDD.

## Tabla directa para columnas de un byte

`append_byte_dictionary` usa 256 slots `u16` para localizar bytes ya vistos (512 B de estado local),
con `u16::MAX` como sentinel y paleta de hasta 256 bytes. Los IDs siguen siendo `u32` y la
paleta mantiene orden de primera aparición. No se cambia packing, selección de codec ni
formatos. Las columnas de más de un byte siguen usando HashMap. El ahorro del estado local
no equivale por sí solo al RSS del proceso: IDs, codecs, spools y preparación siguen vivos.

Un oracle HashMap comprueba paleta e IDs para todas las cardinalidades 1–256, orden inverso,
filas vacías/cortas y 4.096 filas, más roundtrip pre4. Se mantiene el fallback raw/LZ4 y los
checks de corrupción existentes.

### Primer prototipo, no integrado

El primer helper extraía también la construcción de diccionarios anchos y adelantaba la
liberación de su HashMap. Se descarta esa reorganización: la versión final conserva su
orden de asignaciones y vida original; solo la columna de un byte usa la tabla directa.
Los resultados siguientes pertenecen al primer prototipo y no se atribuyen a la versión final.

Belvedere completo es un **control negativo**: sus columnas de un byte son constantes en
los 1.343 bloques y no llegan a construir diccionario. La inspección por payload ya está
registrada en [el análisis pre4](data/storage-v6-pre4-payloads-2026-10-14.json).
Conversión mediana 39,907 → 40,621 s, rangos [38,975; 40,219] / [40,076; 41,056]; no hay
mejora demostrada, y el aumento observado de 1,8 % queda registrado. El RSS mediano sube
de 246,1 a 286,0 MiB; ese contraejemplo motivó limitar la reorganización. Los resultados de lectura
no se atribuyen a esta optimización del encoder. Seis parejas y doce recorridos conservan
hashes/checksums. [Serie completa](data/storage-v6-u8-2026-10-14.json).

Para ejercitar la ruta variable se extraen los **primeros 4.000.000 puntos de Albuera UAV**,
con staging de 65.536 puntos, sin modificar la fuente. El ejemplo `extract_las_prefix` conserva
formato de puntos/transforms y deja que el escritor LAS recalcule bounds y conteos; rechaza
sobrescribir una salida existente. La muestra contiene 2 clasificaciones, 3 números de retorno,
3 conteos de retornos y un valor de user_data. No representa toda la nube ni un muestreo aleatorio.

Tres rondas alternadas por variante, perfil `perf`, pipeline habilitado, 4 workers y presupuesto
512 MiB. Lecturas BFS con 1/4 lectores como control de compatibilidad; decoder sin cambios.

| Albuera, prefijo de 4 M | Antes, mediana [mín., máx.] | Tabla directa, mediana [mín., máx.] |
| --- | --- | --- |
| Conversión, segundos | 2,232 [2,054; 2,391] | 1,906 [1,760; 1,991] |
| Pico RSS, MiB | 219,0 [185,5; 227,3] | 182,4 [170,6; 183,9] |

En el primer prototipo, conversión mediana −14,6 % y pico RSS mediano −16,7 % en esta muestra. No demuestra ese
beneficio en el dataset completo ni en columnas constantes. NVMe local, caché sin purgar,
carga del host sin controlar y N=3. Las seis parejas son idénticas byte a byte; todas las
lecturas conservan filas y checksum. [Comandos, binarios, hashes, CPU/RSS y rondas](data/storage-v6-u8-albuera-2026-10-14.json).

```sh
cargo run --profile perf -p viewer_convert --example extract_las_prefix -- \
  INPUT.laz target/albuera-prefix.las 4000000
```

### Versión final localizada

La versión final solo sustituye la búsqueda de columnas de un byte. Para las demás conserva
la asignación del HashMap antes de paleta/IDs y su vida hasta terminar el encoder, como en el
control. La reorganización del primer prototipo queda descartada. Se mantiene esta mejora
por su coste local medido y su sencillez; no se promete una aceleración global demostrada.

Control aislado de construcción de paleta/IDs con 262.144 filas, nueve rondas alternadas,
perfil `perf`, asignaciones incluidas y drop/packing/compresión/validación fuera del tiempo:

| Cardinalidad | HashMap mediano (ms) | Tabla directa mediana (ms) | Factor local |
| --- | --- | --- | --- |
| 2 | 1,773319 | 0,156746 | 11,31× |
| 3 | 1,820869 | 0,144072 | 12,64× |
| 256 | 2,340076 | 0,101340 | 23,09× |

Este kernel representa una fracción pequeña de la conversión completa; no se extrapola el
factor local. [Muestras, scope y comando](data/storage-v6-u8-lookup-micro-2026-10-14.json).

Tres rondas de conversión por caso en la versión final, opciones anteriores sin cambiar:

| Corpus | Antes, mediana [mín., máx.] (s) | Final, mediana [mín., máx.] (s) | RSS mediano antes/final (MiB) |
| --- | --- | --- | --- |
| Belvedere completo, control negativo | 44,516 [41,836; 45,699] | 41,424 [40,743; 41,757] | 247,2 / 234,7 |
| Prefijo Albuera de 4 M puntos | 2,193 [1,916; 2,227] | 2,176 [2,048; 2,183] | 207,1 / 189,1 |

En Albuera el cambio total observado es −0,7 %, con rangos solapados: **no demuestra una
mejora de conversión**. CPU de usuario mediana 3,88 → 3,74 s; RSS varía ampliamente y sus
rangos se solapan. En Belvedere la ruta directa no se ejercita: su bajada de tiempo de pared
no se atribuye a la tabla; CPU de usuario 84,86 → 85,71 s. La comparación completa sirve
para conservar esos contraejemplos y verificar compatibilidad, no para prometer velocidad.

En las doce conversiones finales coinciden SHA-256 de ambas salidas dentro de cada corpus. Cada conversión valida
todos los bloques antes de publicar. El runner usa `--skip-reads` para omitir repeticiones de
un decoder sin cambios; las lecturas previas y los tests de lectura/corrupción se conservan.
[Control final Belvedere](data/storage-v6-u8-final-2026-10-14.json) y
[final Albuera](data/storage-v6-u8-final-albuera-2026-10-14.json).

La inspección del prefijo confirma 145 payloads Dictionary en clasificación/retornos, por
lo que la prueba no depende solo de cardinalidades globales. Nuevas pruebas del runner
comprueban que el modo encoder omite lectores, conserva validación de hashes y artefactos.
