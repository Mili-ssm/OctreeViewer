# Albuera UAV y Majadas TLS: pre3 frente a pre4 — 14-10-2026

Medición de las fuentes completas locales SensOFOREST, no prefijos ni cortes de visualización.
[Reporte con estado y evidencia](benchmarks/storage-v6-pre4-sensoforest-2026-10-14.json).
Serie completa: cuatro conversiones y 48 procesos de lectura, todos con salida correcta.

## Protocolo

- Una conversión completa por fuente/revisión: **cuatro observaciones**, sin mediana ni rango
  de conversión. Majadas empieza con pre4→pre3 y Albuera con pre3→pre4.
- Mismas opciones: perfil Cargo `perf`, un trabajador de conversión, división 2, bloques de
  262.144 puntos, error máximo 0,01 y presupuesto estimado del conversor predeterminado.
  Un trabajador no implica que el descompresor LAZ interno use un único hilo.
- Después de cada pareja de conversiones, tres rondas en procesos independientes, alternando
  revisiones: apertura de jerarquía, BFS de todos los bloques/atributos en streaming, solo XYZ
  por orden de ID y clasificación (columna 1). Medianas y mínimo–máximo de las tres lecturas.
- Todas las lecturas de payload descartan cada bloque antes del siguiente. No se ejecuta la
  reconstrucción residente que retiene toda la nube: se evita presión innecesaria de RAM.
  BFS usa preflight de 512 MiB sobre el mayor bloque, no sobre toda la nube.
- Comprobar CRC, número de filas con LOD y checksum BFS idéntico entre revisiones y repeticiones.
  Las lecturas selectivas cuentan bytes de geometría/atributos; su cronómetro incluye CRC,
  decodificación, asignación y descarte. El checksum BFS se calcula fuera de `load_ms`.
- RSS máximo por proceso mediante `/usr/bin/time`; no mide memoria JS/GPU ni un pico combinado
  del navegador. El tiempo externo del proceso incluye su resumen; el tiempo interno de
  conversión y `load_ms` tienen sus propias fronteras, registradas por el binario.

Se reutilizan los binarios de conversión/lectura validados de
[la entrega pre4](STORAGE_V6_PRE4_BENCH.md), con las dependencias de HEAD en
`/tmp/octree-v6-pre4-20261014`. El inspector de payloads es el binario de metadata del workspace
principal usado en aquella entrega, con hash propio registrado; no participa en los cronómetros
ni decodifica puntos. Se verificaron los hashes de binarios y fuentes antes de iniciar,
conservando la actualización ajena de dependencias del workspace.
No hay cambio de implementación ni compilación durante la serie.

Hardware: Ryzen AI 9 365, aproximadamente 30 GiB de RAM, NVMe local Btrfs con compresión
transparente. La caché no se purga ni controla; hashing y operaciones anteriores leen archivos
completos, pero su tamaño no permite garantizar residencia. No llamar a esto lectura fría,
HDD/remota ni necesariamente lectura íntegramente desde RAM.

## Espacio y reproducibilidad

Se conservan `.oct` y `.octattrs` en directorios nuevos bajo
`target/sensoforest-pre4-20261014/`. Los temporales también se sitúan en ese filesystem, con
`TMPDIR` explícito. No se reemplazan las parejas existentes de `data/datasets/sensoforest/`.

El reporte distingue tamaño lógico (`st_size`) de bloques asignados al archivo
(`st_blocks * 512`), antes y después de las lecturas. El segundo refleja asignación observada
en este Btrfs, no tamaño transferible, espacio global de dispositivo ni metadata del filesystem.
La compresión de Btrfs puede hacer que el ahorro físico difiera del ahorro del formato.

El reporte identifica fuentes, binarios, runner, orquestación local, opciones, hashes de
salidas, tiempos, RSS y logs de cada operación. La orquestación y los JSON completos quedan
en el directorio local; los archivos de datos están ignorados por Git. El runner conserva el
último estado completado y un error explícito si falla una operación.

## Estado

Ambos conjuntos han completado ambas conversiones y las tres rondas de lectura. Una observación de conversión no acredita una mejora estadística de velocidad;
los tamaños son exactos para estos archivos/opciones.

## Majadas TLS

415.465.882 puntos fuente; 415.602.190 filas con LOD y 6.259 bloques en ambas revisiones.
Checksum BFS `b830818c4aeddac0`, igual en las seis lecturas completas.

| Archivo / medida | pre3 | pre4 |
|---|---:|---:|
| `.oct`, bytes lógicos | 2.197.688.136 | 2.197.676.578 |
| `.octattrs`, bytes lógicos | 3.142.295.061 | 2.414.967.531 |
| Pareja lógica (B) | 5.339.983.197 | 4.612.644.109 |
| Pareja, bloques asignados Btrfs (B, tras lecturas) | 5.339.983.872 | 4.612.648.960 |
| Conversión observada (s, N=1) | 487,922 | 499,240 |
| Pico RSS de conversión (MiB, N=1) | 105,70 | 102,23 |

Ahorro lógico **727.339.088 B (13,62 %)**; `.octattrs` aporta 727.327.530 B y `.oct` 11.558 B.
La conversión pre4 tardó 11,319 s más (2,32 %) en este par observado; no hay repeticiones de
conversión para estimar su dispersión.

| Lectura / decodificación / CRC, segundos, mediana [mín.; máx.] | pre3 | pre4 |
|---|---:|---:|
| BFS, todos los payloads | 40,948 [40,836; 41,337] | 38,684 [38,576; 40,220] |
| Solo XYZ, por ID | 17,380 [17,369; 17,527] | 17,422 [17,357; 17,451] |
| Clasificación, por ID | 1,635 [1,612; 1,724] | 1,619 [1,616; 1,760] |

Las medianas BFS bajan 5,53 %. XYZ/clasificación tienen rangos solapados y no demuestran una
mejora. Clasificación lee **6.259 B de payload** por pasada en ambas revisiones, con cero bytes
de payload XYZ; XYZ no lee payloads de atributos. Los bytes de metadata se cuentan aparte.
Los tiempos BFS excluyen la verificación del checksum, que añade aproximadamente 11 s.

## Albuera UAV

508.809.556 puntos fuente; 508.989.347 filas con LOD y 8.437 bloques en ambas revisiones.
Checksum BFS `8caae2a09753e50e`, igual en las seis lecturas completas.

| Archivo / medida | pre3 | pre4 |
|---|---:|---:|
| `.oct`, bytes lógicos | 6.047.947.978 | 6.037.118.837 |
| `.octattrs`, bytes lógicos | 6.717.835.216 | 4.283.615.930 |
| Pareja lógica (B) | 12.765.783.194 | 10.320.734.767 |
| Pareja, bloques asignados Btrfs (B, tras lecturas) | 12.765.786.112 | 10.320.736.256 |
| Conversión observada (s, N=1) | 582,455 | 610,453 |
| Pico RSS de conversión (MiB, N=1) | 116,96 | 123,66 |

Ahorro lógico **2.445.048.427 B (19,15 %)**; `.octattrs` aporta 2.434.219.286 B y `.oct`
10.829.141 B. La conversión pre4 tardó 27,998 s más (4,81 %) en el par observado.

| Lectura / decodificación / CRC, segundos, mediana [mín.; máx.] | pre3 | pre4 |
|---|---:|---:|
| BFS, todos los payloads | 68,628 [68,359; 69,235] | 59,790 [59,562; 59,931] |
| Solo XYZ, por ID | 23,516 [23,481; 23,562] | 23,325 [23,316; 23,376] |
| Clasificación, por ID | 2,458 [2,409; 2,887] | 2,425 [2,368; 2,845] |

La mediana BFS baja **12,88 %**. El cambio XYZ es pequeño (0,81 %); los rangos de
clasificación se solapan. Clasificación lee **23.207.301 B de payload** en ambas revisiones,
con cero payload XYZ; XYZ no lee payloads de atributos. Los tiempos BFS excluyen la
verificación del checksum, que añade aproximadamente 17 s.

## Interpretación y validación

El ahorro combinado es **3.172.387.515 B**, principalmente en `.octattrs`. La geometría `.oct`
apenas cambia en Majadas y baja 10,83 MB en Albuera. La lectura completa mejora en estas series,
a cambio de una conversión observada algo más lenta. No se atribuye toda la diferencia de
lectura al disco: el cronómetro también incluye decodificación y CRC.

La mediana del pico RSS de las lecturas BFS es 34,73 → 33,90 MiB en Majadas y
45,88 → 45,95 MiB en Albuera. Son procesos de streaming, no cargas residentes de toda la nube.

Revisión final: hashes de los 52 JSON de operación y sus códigos de salida, tres muestras por
modo/revisión, recuentos de bloques/filas, checksums BFS, aislamiento de payloads selectivos
y tamaños/asignación final de los ocho archivos. Los hashes SHA-256 de los datos se registraron
tras su conversión; todas las lecturas posteriores comprueban CRC. No se modificó código de
producción. HDD, blobs remotos y caché controlada quedan fuera de esta medición.
