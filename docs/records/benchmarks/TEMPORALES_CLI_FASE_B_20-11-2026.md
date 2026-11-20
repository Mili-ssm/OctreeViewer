# Temporales del CLI, fase B: spools comprimidos — 20-11-2026

**Estado: medido.** Es la comprobación de la tarea 5 del
[plan de la fase B](../../plans/temporales-del-conversor-fase-b.md), dentro del
[diseño de temporales del conversor](../../plans/temporales-del-conversor.md) (RQ-06, OQ-04, #24).

**Conclusión:** guardar los spools nativos como tramas comprimidas (planos de bytes por registro + LZ4) divide el pico
aparente de temporales entre **3,2 (Albuera), 5,1 (Majadas) y 4,3 (Belvedere)** con salida **byte a byte idéntica**,
a cambio de **+24 % / +20 % / +7 %** de tiempo total y sin cambio apreciable de RSS. Albuera queda en **3,5 × el LAZ**
frente a 11,1 ×: RQ-06 (≤ 2 ×) todavía no se cumple y la fase A (escritura por subárbol) sigue siendo necesaria.

## Condiciones

| Dato | Valor |
| --- | --- |
| Binarios | Base: `release` de `dc98e5a`. Fase B: `release` de `2833708` (rama `feat/temporales-fase-b`) |
| Opciones | Por defecto (`v7`, 4 workers espaciales, 4 rangos de raíz); orden: `viewer_convert ENTRADA SALIDA.oct` |
| Medida | `scripts/measure_temp_disk.sh`: muestreo de 1 s de `du -sb` del directorio `.work` (tamaño **aparente**), `/usr/bin/time -v`, SHA-256 de `.oct` y `.octattrs` |
| Equipo | Equipo del propietario; salida en `/home`: `btrfs rw,relatime,seclabel,compress=zstd:1,ssd,discard=async,space_cache=v2` |
| Orden | Una conversión cada vez, sin otra carga: base Albuera, B Albuera, base Majadas, B Majadas, B Belvedere (la base de Belvedere es la del 19-11-2026, en la «Línea base» del plan) |
| Datos | [`data/temporales-cli-20-11-2026/`](data/temporales-cli-20-11-2026/): `<ejecución>.tsv` (`segundos`, bytes de `.work`, delta de `df`) y `<ejecución>.phases.txt` (segundo en que empieza cada fase y línea de resultado) |

## Resultados

| Nube (LAZ) | Ejecución | Pico aparente | × LAZ | Tiempo | RSS máx. | SHA igual a la base |
| --- | --- | --- | --- | --- | --- | --- |
| Albuera (4 305 484 789 B) | base | 47 945 156 154 B | 11,14 | 562,0 s | 242 816 kB | — |
| | **B** | **15 011 404 557 B** | **3,49** | 695,6 s (+23,8 %) | 248 572 kB | sí |
| Majadas (1 343 266 470 B) | base | 33 082 974 280 B | 24,63 | 282,5 s | 229 872 kB | — |
| | **B** | **6 495 237 932 B** | **4,84** | 340,1 s (+20,4 %) | 249 312 kB | sí |
| Belvedere (618 390 994 B) | base (19-11-2026) | 6 466 222 890 B | 10,46 | 47,1 s | 98 132 kB | — |
| | **B** | **1 511 437 315 B** | **2,44** | 50,6 s (+7,4 %) | 104 464 kB | sí |

**Por fases** (segundo de inicio de cada fase en el log; Albuera y Majadas):

| Fase | Albuera base | Albuera B | Majadas base | Majadas B |
| --- | --- | --- | --- | --- |
| Partición de la raíz (`PlanningSpatialIndex`) | 0–87 s | 0–95 s | 0–38 s | 0–48 s |
| División de nodos (`BuildingSpatialHierarchy`) | 87–424 s (337 s) | 95–564 s (**469 s, +39 %**) | 38–193 s (155 s) | 48–248 s (**200 s, +29 %**) |
| Bloques (`ReadingAndWritingBlocks`) | 424–532 s | 564–666 s | 193–265 s | 248–321 s |
| Validación | 532–561 s | 666–695 s | 265–282 s | 321–339 s |

## Lectura

- **El coste de tiempo está en la división de nodos**, donde cada spool se descomprime al leerse y sus hijos y su LOD
  se comprimen al escribirse. La partición de la raíz, que incluye unir los segmentos de los 4 rangos, solo añade
  8–10 s: por debajo del umbral del plan (> 30 s o > 5 % del total), así que no hace falta unir copiando tramas.
- **Umbrales del plan (tarea 5, paso 3):** tiempo de Albuera +23,8 % (límite +25 %, al borde); RSS +6 MB (límite
  +100 MB); SHA iguales en las tres nubes.
- **Uso real en btrfs:** el delta de `df` no sirve como medida aquí. Con la compresión de btrfs y el descarte
  asíncrono marca 22 GB en la base de Albuera pero 0–6 MB en la base de Majadas y en las B de Albuera y Belvedere (3,9 GB en la B
  de Majadas), así que
  no se puede afirmar cuánto baja el uso real en este equipo. Los spools LZ4 apenas se recomprimen con zstd, de modo
  que en btrfs la ganancia real esperable es menor que la aparente (inferido, no medido). La cifra aparente es la
  que ocuparía en ext4 o NTFS.
- **Frente a la estimación del diseño:** Albuera queda en 15,0 GB, dentro de lo previsto tras medir los LOD de Albuera
  (14–15 GB); Majadas comprime mejor (÷ 5,1).

## Lo que queda

La fase A (escribir los bloques de cada nodo al confirmarse, en el mismo orden) elimina la acumulación de LOD y de
hojas pendientes; con la compresión de B, el diseño la estima en ~1,6–2,0 × el LAZ de Albuera. La pérdida de tiempo en
la división de nodos queda como trabajo posterior (más paralelismo, según el propietario el 19-11-2026).
