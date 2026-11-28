# Conversión paralela, tarea 0: camino crítico y atribución de E/S — 21-11-2026

**Estado: medido.** Tarea 0 del [plan de conversión paralela](../../plans/conversion-paralela.md). Datos en bruto en
[`data/conversion-paralela-t0-21-11-2026/`](data/conversion-paralela-t0-21-11-2026/).

**Conclusión:**

- **(a) Lo que limita de W = 4 a W = 8 es la jerarquía bajo la raíz, que no cambia (24,1 → 24,7 s en Belvedere; 196,6 →
  198,4 s en Majadas).** Con W = 8 solo bajan los bloques y la validación (Belvedere −3,6 y −1,2 s; Majadas −14,4 y
  −5,0 s). Total: ×1,08 en Belvedere (49,4 → 45,9 s) y ×1,05 en Majadas (334,0 → 317,3 s). La jerarquía es el 54 % del
  tiempo de pared con W = 8 en Belvedere y el 63 % en Majadas. La causa es doble: `threads = tasks.len()` ≤ hijos
  ocupados de la raíz (4), y el reparto es muy desigual (un hijo concentra el 87 % de los bytes de spool en Belvedere y
  el ~100 % en Majadas). Un modelo de cola de listos sobre los eventos por nodo (`analyze.py`) con los tiempos de W = 1 reproduce la
  jerarquía observada con 4 hilos con un 9–13 % de defecto (20,9 s frente a 24,1 s en Belvedere; 178,0 s frente a
  196,6 s en Majadas) y con 8 hilos libres da 15,4 s y 129,8 s; la ruta crítica del árbol es de 12,2 s y 99,1 s. **Total
  estimado con W = 8 y cola libre** = resto de etapas medidas con W = 8 (21,2 s y 118,9 s) + jerarquía modelada con 8
  hilos libres: con duraciones de W = 1, 36,6 s (Belvedere, ×1,35 frente a W = 4) y 248,7 s (Majadas, ×1,34); con
  duraciones de W = 8 (más trabajo por nodo), 38,8 s (×1,27) y 260,5 s (×1,28). El trabajo por nodo crece con la
  concurrencia (suma de la jerarquía: Belvedere 73,1 s con W = 1 frente a 85,1 s con W = 4; Majadas 551,7 frente a
  623,1 s), y el modelo no tiene término de contención, por eso subestima lo observado: **el rango ×1,27–1,35 es una
  cota superior del modelo, no una medición; queda justo en el umbral de 1,3× de la tarea 1, y la tarea 1b mide el valor
  real.** El criterio del plan (Majadas con W = 8 frente a W = 4 tras la tarea 1; hoy, ×1,05 medido) se decide con la
  medición de la tarea 1b, no con este modelo. La partición de la raíz (11–13 s y 51–57 s) tampoco escala con W (usa 4 rangos fijos).
- **(b) La puerta de E/S no salta: con W = 4 y en estas ejecuciones, el directorio de trabajo no cambia el tiempo más
  de un 10 %.** Con W = 4, tmpfs frente a btrfs difiere +1,6 %
  en Belvedere (3 ejecuciones) y −3,5 % en Majadas (1); btrfs sin compresión, +4,7 % y +0,6 %. Todo < 10 %. PSI de E/S
  ≤ 1,6 % y `iowait` ≤ 0,11 núcleos. **Salvedades:** las ejecuciones de Belvedere en tmpfs y sin compresión tuvieron
  carga previa de 3,5–4,0 frente a 1,1 en btrfs (otra sesión), y las de Majadas al revés (btrfs 3,8, tmpfs 1,3, sin
  compresión 5,0); solo se probó W = 4; W = 8 en tmpfs no se probó.
- **(c) Navegador con un worker, en V8 (Node, no Chrome), con el presupuesto de spools que usa la app web (2 GiB,
  `DEFAULT_SPOOL_BUDGET_BYTES`):** Belvedere 144,7 y 141,8 s (×1,08–1,11 el CLI con W = 1: 130,7 s), con 35–36 s de
  partición, 50–51 s de jerarquía, 35–38 s de escritura y 19 s de validación (×1,7 la del CLI con W = 1). Pico RSS
  4,31 GB (RAM de spools 2,15 GB + 14,6 GB derramados a zonas). Majadas (1 ejecución): 809,7 s (CLI con W = 1:
  838,4 s), pico RSS 4,66 GB y 111,1 GB derramados a 55 zonas (pico de 129.662 páginas de zona en uso: ojo con la cuota
  OPFS). Fila secundaria con 4 GiB (no es el valor de la app): Belvedere 139,4 y 134,6 s (×1,03–1,07), RSS 7,7 GB, 4,36 GB
  derramados; Majadas 808,4 s, RSS 9,17 GB, 99,1 GB derramados. Con 2 GiB el tiempo sube ~4 % en Belvedere y no cambia en
  Majadas. La SHA del `.oct` coincide con la del CLI en todas las ejecuciones; la del `.octattrs` solo se comprobó en
  un par Belvedere (CLI con W = 4 y sonda con 2 GiB: idénticas).

## Condiciones

- Equipo del propietario (20 hilos, btrfs con `compress=zstd:1`, /dev/shm de 16 GB), rama `feat/conversion-paralela`,
  binario construido en `c9f4722` (sin cambios de código). Perfil `perf` para el CLI:
  `cargo build --offline --profile perf -p viewer_convert --example real_dataset_bench`; wasm: `--release`,
  `wasm-bindgen --target web`. Cifras solo válidas para esa revisión, perfil, equipo y datasets.
- Comando por ejecución (`run.sh`, con muestreo de `/proc/stat` y PSI cada segundo, `sample.py`):
  `real_dataset_bench convert-profile ENTRADA SALIDA W RANGOS`. **El ejemplo usa 0 rangos si se omite; el CLI usa 4**, así
  que todas las ejecuciones pasan los rangos de forma explícita (4, salvo la fila «W = 1, 0 rangos»). Orden y matriz:
  `driver.sh` (rondas intercaladas en Belvedere, una por configuración en Majadas, una conversión a la vez).
- Directorios: btrfs = `target/t0scratch/btrfs`; sin compresión = directorio nuevo con `chattr +m` (`lsattr` confirmado);
  tmpfs = `/dev/shm/t0work`. La entrada siempre se lee de btrfs. Datos: Belvedere (`belv_2009_…laz`, 81,7 M puntos) y
  Majadas (`SensOFOREST_…Majadas…laz`).
- Carga previa (`loadavg`) en cada `.meta`: de 1,1 a 5,0; otras sesiones del equipo estaban activas en parte de las
  rondas (se anota donde pesa). SHA del `.oct` idéntica en las 24 conversiones del CLI y en las ejecuciones del navegador por nube (solo
  se calcula el `.oct`; el `.octattrs` se comparó en un par, ver (c)). Binario del CLI en `c9f4722`; los bindings wasm de
  la sonda con 2 GiB, construidos en `711b8b5`, cuyo cambio de código (tarea 1a) solo toca la ruta por lotes, que con
  W = 1 (el valor por defecto en wasm32; ni la web ni la sonda pasan `spatialWorkers`) no se usa; la SHA igual lo
  confirma para los bytes, no para el tiempo. Las ejecuciones con 4 GiB usan los bindings de `c9f4722` y la sonda
  anterior a `f1a268b` (campo `wasmMemoryBytes`, sin hash del `.octattrs`), así que la diferencia de ~4 % entre 2 y
  4 GiB mezcla dos versiones. Carga previa de la sonda: 0,9–1,4 con 2 GiB y 1,3–2,8
  con 4 GiB.
- Las ventanas de pared salen de las etapas de progreso (`stage_*_ms`), no de los temporizadores por trabajo, que suman
  CPU entre hilos. La jerarquía por nodo sale de `internal_spatial_node_event` (`elapsed_ns` es trabajo propio; `start_seq`
  y `end_seq` no son reloj), con `analyze.py`: planificación por lista (cola FIFO en preorden, hijos listos al acabar el
  padre) sobre los nodos de profundidad ≥ 1.
- Sonda del navegador: [`tools/convert_parity/time-bindings.mjs`](../../../tools/convert_parity/time-bindings.mjs)
  (`WasmCoreConverter` real, spools en zonas respaldadas por `fs`, presupuesto de spools 2 GiB como la app web; tiempo
  por etapa de progreso). Una primera tanda usó 4 GiB por error (la sonda lo llamaba «valor web»); se conserva como fila
  secundaria.

## CLI: tiempo de pared (mediana; mín.–máx.)

Belvedere, 3 rondas; Majadas, 1 ejecución (sin dispersión):

| Configuración | Belvedere total (s) | ×vs W = 1 | Majadas total (s) | ×vs W = 1 |
| --- | --- | --- | --- | --- |
| W = 1, 4 rangos | 130,7 (130,5–131,6) | 1 | 838,4 | 1 |
| W = 1, 0 rangos | 135,1 (134,9–136,9) | 0,97 | 865,0 | 0,97 |
| W = 4 | 49,4 (48,6–50,9) | 2,65 | 334,0 | 2,51 |
| W = 8 | 45,9 (45,1–46,7) | 2,85 | 317,3 | 2,64 |

Por etapa (s, medianas; metadatos < 0,1 s y publicación < 0,01 s):

| Etapa | Belv. W1 r4 | Belv. W1 r0 | Belv. W4 | Belv. W8 | Maj. W1 r4 | Maj. W1 r0 | Maj. W4 | Maj. W8 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Partición de la raíz | 13,5 | 18,9 | 11,3 | 11,8 | 51,5 | 89,0 | 56,1 | 57,0 |
| **Jerarquía** | 73,1 | 72,4 | 24,1 | 24,7 | 551,8 | 551,3 | 196,6 | 198,4 |
| Bloques (leer, preparar, escribir) | 32,8 | 33,0 | 10,8 | 7,2 | 178,5 | 175,5 | 66,6 | 52,2 |
| Validación | 11,3 | 10,9 | 3,0 | 1,8 | 56,5 | 49,1 | 14,7 | 9,7 |

Los 4 rangos de la raíz ya paralelizan la partición con W = 1 (13,5 frente a 18,9 s; explica el ~117 % de CPU con W = 1).

## Camino crítico de la jerarquía

| | Belvedere | Majadas |
| --- | --- | --- |
| Hijos ocupados de la raíz y reparto de bytes de spool | 4: 86,8 / 9,7 / 3,0 / 0,4 % | 4 con ~100 % en uno |
| Trabajo de la jerarquía con W = 1 (suma de nodos) | 73,1 s (3.941 nodos) | 551,7 s (24.091 nodos) |
| Ruta crítica del árbol (duraciones de W = 1; corregida el 21-11-2026, ver `analyze.py`) | 12,2 s | 99,1 s |
| Observado, W = 4 / modelo con 4 hilos (duraciones de W = 1) | 24,1 / 20,9 s | 196,6 / 178,0 s |
| Modelo con 8 hilos libres / 16 (duraciones de W = 1) | 15,4 / 13,2 s | 129,8 / 107,0 s |
| Modelo con 8 hilos libres (duraciones de W = 8) | 17,6 s | 141,6 s |
| Trabajo de la jerarquía con W = 4 | 85,1 s | 623,1 s |
| Tiempo de la jerarquía con < 4 hilos activos (modelo, 4 hilos) | 21 % | 33 % |
| Tiempo con < 8 hilos activos (modelo, 8 libres) | 55 % | 58 % |

Belvedere: medianas de 3 rondas; Majadas: 1 ejecución. Con las duraciones de W = 4 el modelo con 4 hilos da 24,0 s y
197,3 s, es decir, lo observado: el defecto del modelo viene de las duraciones de W = 1. Con W = 8 el código no usa más de 4 hilos en la jerarquía (hijos ocupados de la raíz), por eso W = 8 no cambia esa fase.

## Atribución de E/S, W = 4

| Directorio de trabajo y salida | Belvedere total (s) | Majadas total (s) | PSI E/S «some» | `iowait` (núcleos) | Sistema (núcleos) |
| --- | --- | --- | --- | --- | --- |
| btrfs (compresión por defecto) | 49,4 (48,6–50,9) | 334,0 | 0,14 % / 0,98 % | 0,02 / 0,08 | 0,39 / 0,37 |
| btrfs sin compresión (`+m`) | 51,7 (51,2–51,9) | 336,0 | 0,17 % / 1,48 % | 0,02 / 0,11 | 0,39 / 0,36 |
| tmpfs (`/dev/shm`) | 50,2 (48,4–52,7) | 322,2 | 0,02 % / 0,20 % | 0,00 / 0,02 | 0,33 / 0,29 |

Cada celda doble es Belvedere / Majadas. Diferencias máximas: +4,7 % y −3,5 %. Las ejecuciones de Belvedere en btrfs
corrieron con carga previa de ~1,1 y las de tmpfs y sin compresión con ~3,9 (otra sesión), así que la diferencia de
+4,7 % es probablemente ruido. El contador de CPU de hilos del kernel (`kworker`, `btrfs-*`) no es utilizable: sale
negativo cuando terminan hilos; se usan `iowait`, el tiempo de sistema y PSI.

## Navegador, un worker (V8 en Node)

Presupuesto de spools de 2 GiB, el de la app web (`DEFAULT_SPOOL_BUDGET_BYTES`):

| | Belvedere 1 | Belvedere 2 | Majadas |
| --- | --- | --- | --- |
| Total | 144,7 s | 141,8 s | 809,7 s |
| Inspección + metadatos | 1,1 s | 1,2 s | 0,06 s |
| Partición (`planning`) | 35,2 s | 35,8 s | 157,6 s |
| Jerarquía | 51,0 s | 50,1 s | 381,5 s |
| Escritura | 38,1 s | 35,3 s | 168,9 s |
| Validación | 19,3 s | 19,4 s | 101,5 s |
| Pico RSS (`VmHWM`) | 4,31 GB | 4,31 GB | 4,66 GB |
| RAM de spools (pico) / derramado | 2,15 / 14,64 GB | 2,15 / 14,64 GB | 2,15 / 111,1 GB |

Fila secundaria, presupuesto de 4 GiB (no es el de la app; primera tanda):

| | Belvedere 1 | Belvedere 2 | Majadas |
| --- | --- | --- | --- |
| Total | 139,4 s | 134,6 s | 808,4 s |
| Partición / jerarquía / escritura / validación | 36,2 / 47,9 / 34,4 / 19,7 s | 34,3 / 47,1 / 33,1 / 19,2 s | 157,9 / 377,2 / 171,2 / 102,1 s |
| Pico RSS (`VmHWM`) | 7,70 GB | 7,71 GB | 9,17 GB |
| RAM de spools (pico) / derramado | 4,30 / 4,36 GB | 4,30 / 4,36 GB | 4,30 / 99,1 GB |

Las etapas del navegador no coinciden con las del CLI (la partición incluye parte de la jerarquía), así que se comparan
las sumas en Belvedere: 87 s (2 GiB) frente a 86,6 s del CLI con W = 1 hasta el final de la jerarquía; la escritura,
35–38 s frente a 32,8 s, y la validación, ×1,7. Respecto al CLI con W = 1 el navegador es ×1,08–1,11 en Belvedere
(2 GiB; ×1,03–1,07 con 4 GiB) y ×0,97 en Majadas. La memoria wasm no se midió bien (el muestreo solo ocurre en
callbacks de progreso: 83 MB en Belvedere y 165 MB en Majadas, no es un pico).

## Lo que no se midió

- Chrome con OPFS (la sonda usa Node con zonas en `fs`; no hay PSS, solo RSS), Firefox y la memoria wasm máxima.
- Majadas con más de una ejecución por configuración; Albuera.
- CPU de hilos del kernel por hilo (contador inutilizable); dispersión de Majadas.
- La predicción del modelo (cola libre con 8 hilos) es una estimación sobre los tiempos por nodo de W = 1/8, no una
  medición; el modelo no tiene contención de memoria, que con más hilos puede reducir la ganancia.
