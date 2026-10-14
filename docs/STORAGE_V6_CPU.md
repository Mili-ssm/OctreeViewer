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

[Comandos, binarios y evidencia](benchmarks/storage-v6-packed-ids-2026-10-14.json).
El runner conserva la primera pareja antes como fuente común y elimina parejas redundantes
solo tras verificar hashes. No purga caché ni aísla carga del host; pruebas breves de CRC
coincidieron con algunas conversiones de esta serie. N=3 no acredita mejora universal.
Un [smoke con Outer Banks](benchmarks/storage-v6-packed-ids-smoke-2026-10-14.json) comprueba el
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
[Comandos, muestras y binario](benchmarks/storage-v6-crc-micro-2026-10-14.json).

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
rechazado. [Comandos, binarios, CPU/RSS y rondas CRC](benchmarks/storage-v6-crc-2026-10-14.json).

## Validación y próximos experimentos

Pasan tests de core/format/convert con todos sus targets, Clippy con `-D warnings`, comprobación
WASM de format, fmt y diff. El runner se verifica con éxito real pequeño y pruebas de mismatch
que conservan ambas parejas. Los checks de WASM acreditan compilación; rendimiento y ejecución
en navegador no se midieron. No se modifica interfaz web ni se valida la migración del renderer.
Ruff no está instalado; Python se comprueba mediante sintaxis/CLI y la suite unittest disponible.

Siguientes candidatos acotados, pendientes de medición antes de integrar:

- Tabla directa de 256 entradas para diccionarios de columnas `u8`, conservando orden de
  primera aparición. Comparar CPU y bytes exactos frente a HashMap; no omitir candidatos.
- Transposición por tiles en planos de bytes, especialmente RGB y columnas de 8 bytes:
  medir encode/decode y locality con patrones reales y alta entropía, manteniendo bits exactos.
- Extender lectura paralela al benchmark selectivo XYZ/RGB/columna y medir solicitudes/rangos
  antes de introducir una API concurrente o prefetch en Blob/remoto/HDD.
