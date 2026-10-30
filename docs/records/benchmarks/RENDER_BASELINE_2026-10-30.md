# Línea base de render y primer A/B de R1 (30-10-2026)

Registro fechado de R0-T1 paso 2 del [plan de render y carga a escala](../../plans/render-y-carga-escala.md). Mide el arnés `probe-orbit-load.mjs` (Chrome, GPU hardware) sobre dos brazos: **A = `ede7e0a`** (instrumentación, antes de T0/T3a/T4) y **B = `87ef649`** (con el detector de thrash, selección única y métrica de error única). Datos crudos: [JSON por ejecución](data/render-baseline-2026-10-30/) (un `.json`, su `.png` final y su `.log` y la serie de PSS `pss-chrome-tree.txt`).

## Entorno y método

- AMD Ryzen AI 9 365 (20 hilos), Linux 7.2.13, gobernador `powersave`; Chrome 156.0.8078.73 con `--use-angle=vulkan` vía `playwright-core`; adaptador `{vendor: "amd", architecture: "rdna-3"}` en las 18 ejecuciones (`mode: "timing"`; `libvulkan_radeon` mapeada y fds DRI en el proceso GPU). Node 22.23.1; wasm en `--release`.
- Brazos construidos en worktrees `target/rb-A` y `target/rb-B` (cada uno con su `pkg`), servidos en puertos distintos. SHA-256 del `.wasm`: A `9833061a09a9…`, B `5063fc13321b…` (completos en el JSON). Sonda: `probe-orbit-load.mjs` de `f189849` (HEAD; añade `lodFinal` con `refinementWork`, `pendingTargetEvictions`, `cacheCycleDetected` leídos con `getStats` al final; commit aparte).
- Datasets (`target/render-data/`, conversor `ede7e0a`): `majadas.oct` (4 214 687 109 B, sha `38168777…`, 415 465 882 puntos, 6259 nodos) y `belvedere.oct` (981 063 326 B, sha `ecdc4889…`, 81 682 329 puntos, 1343 nodos); coinciden con `MANIFEST.txt`.
- Matriz alternando A/B por ronda, `loadavg` 1 min < 1,5 antes de cada una (inicio real 0,95–1,72; dos rondas de Majadas arrancaron con 1,45 y 1,72 por el propio navegador anterior). Majadas 4 px (tope 180 s) 3 rondas; Belvedere 8 px (300 s) 3 rondas; Belvedere 4 px (300 s) 2 rondas por brazo (sin recorte de presupuesto: cada ejecución duró 1–5,5 min). Una ejecución extra con `--pixels` por brazo en Majadas.
- Órbita guionizada de 10 s (un movimiento cada 16 ms): "carga" = los primeros 10 s tras empezar a cargar; "órbita" = 10 s tras alcanzar el objetivo (solo si se alcanza). Las medianas llevan [mín–máx]. PSS = pico de la suma `smaps_rollup` de todos los procesos `chrome` (muestreo cada 2 s) en la ventana de cada ejecución.

## Resultados (mediana [rango])

Intervalo entre respuestas de frame del worker, en ms.

| Escenario | Brazo | Carga p50 / p95 / máx | Órbita p50 / p95 / máx | 1.er frame ms | Objetivo | Carga entera p95 / máx |
| --- | --- | --- | --- | --- | --- | --- |
| Majadas 4 px (n=3) | A | 82 [67–99] / 232 [197–290] / 376 [263–487] | 480 [478–491] / 490 / 510 | 207 [137–208] | 55,8 s [54,5–58,3] | 975 / 1879 |
| Majadas 4 px (n=3) | B | 67 [67–69] / 101 [87–141] / 320 [255–346] | 482 [477–484] / 488 / 488 | 207 [144–208] | 61,4 s [60,8–62,2] | 1230 / 1885 |
| Belvedere 8 px (n=3) | A | 67 [67–98] / 124 [102–128] / 341 [262–344] | 2619 [2596–2658] / 2680 / 2680 | 88 [61–95] | 254 s [252–255] | 5742 / 10 438 |
| Belvedere 8 px (n=3) | B | 67 [67–68] / 239 [239–262] / 287 [286–315] | sin objetivo | 77 [72–110] | **pendiente a 300 s** (54 pendientes) | 6678 / 7924 |
| Belvedere 4 px (n=2) | A | 68 / 105 [84–125] / 307 [207–408] | sin objetivo | 88 | pendiente a 300 s (etapa 4 px a 260–270 s, 865 pendientes) | 6946 / 19 266 |
| Belvedere 4 px (n=2) | B | 68 / 234 [203–264] / 291 [279–304] | sin objetivo | 79 | pendiente a 300 s (etapa 8 px, 54 pendientes) | 5555 / 9099 |

Contadores y trabajo del worker (mediana [rango]).

| Escenario | Brazo | `sceneBuilds` | `voxelSourcePoints` (M) | bloques async | `uncoveredNodes` máx/final | `oversizedVoxels` máx/final | `pendingTargetEvictions` | sel./prep. por actualización | PSS pico MiB |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Majadas 4 px | A | 378 [360–394] | 905 [884–932] | 239 | 59 / 0 | 0 / 0 | n/d | n/d | 592 [586–594] |
| Majadas 4 px | B | 404 [394–406] | 1041 [1039–1049] | 239 | 59 / 0 | 0 / 0 | 0 (`cacheCycleDetected` false) | 0,6 / 1,3 | 589 [584–589] |
| Belvedere 8 px | A | 527 [505–535] | 1293 [1277–1302] | 389 | 81 / 0 | 243 / 0 | n/d | n/d | 667 [664–668] |
| Belvedere 8 px | B | 444 [439–449] | 1746 [1744–1749] | 326 | 81 / 0 | 243 / 10 | 0 (false) | 0,4 / 1,2 | 641 [639–641] |
| Belvedere 4 px | A | 540 [526–553] | 1439 [1425–1452] | 399 | 81 / 0 | 435 / 435 | n/d | n/d | 677 [673–681] |
| Belvedere 4 px | B | 452 [450–453] | 1759 [1747–1771] | 326 | 81 / 0 | 243 / 10 | 0 (false) | 0,4 / 1,2 | 644 [639–650] |

Ocupación del worker (suma de `busyMs`) y lectura/decodificación async (ms acumulados; la lectura es espera de E/S, no ocupa el worker).

| Escenario | Brazo | Top 3 de ocupación por mensaje | Lectura / decodificación / inserción | Voxelización (`voxelBuildMs`) | Decodificación ÷ ocupado |
| --- | --- | --- | --- | --- | --- |
| Majadas 4 px | A | `asyncInsert` 71 % (46,3 s), `frame` 29 % (18,8 s), `asyncOpen` 0,1 s | 139 s / 0,54 s / 45,9 s | 63,2 s (98 % del ocupado) | 0,8 % |
| Majadas 4 px | B | `asyncInsert` 72 % (50,9 s), `frame` 28 % (19,5 s), `init` 0,1 s | 153 s / 0,52 s / 50,8 s | 69,3 s (98 %) | 0,7 % |
| Belvedere 8 px | A | `asyncInsert` 93 % (243 s), `frame` 7 % (19 s), `init` 0,1 s | 720 s / 0,93 s / 243 s | 261 s (99,5 %) | 0,4 % |
| Belvedere 8 px | B | `asyncInsert` 98 % (295 s), `frame` 2 % (6,9 s), `init` 0,1 s | 879 s / 0,84 s / 295 s | 311 s (99 %) | 0,3 % |
| Belvedere 4 px | A | `asyncInsert` 96 %, `frame` 4 % | 838 s / 0,97 s / 293 s | 320 s | 0,3 % |
| Belvedere 4 px | B | `asyncInsert` 98 %, `frame` 2 % | 884 s / 0,81 s / 297 s | 312 s | 0,3 % |

`gpu_busy_percent` (informativo): máximo 3–4 % en todas las ejecuciones (muestreo cada 250 ms; el visor es limitado por CPU, no por GPU).

Píxeles vacíos (una ejecución con `--pixels` por brazo, Majadas 4 px; fracción de píxeles de fondo dentro del casco del AABB proyectado, 3 capturas por órbita): carga A 0,772 / 0,780 / 0,778, B 0,758 / 0,768 / 0,781; tras el objetivo A 0,885 / 0,889 / 0,896, B 0,885 / 0,892 / 0,892; final A 0,8996, B 0,8983. Valores absolutos altos porque el casco del AABB es mucho mayor que la nube (la captura muestra un cúmulo central); solo vale la comparación entre brazos. Esas dos ejecuciones no entran en las medianas de tiempo (el volcado de capturas las perturba).

## Criterios de éxito (p95 < 33 ms y máx < 100 ms en carga y en órbita de 10 s)

| Criterio | A | B |
| --- | --- | --- |
| Intervalo de respuestas, carga | **no cumple**: p95 124–232 ms, máx 341–376 ms (p50 67–82 ms) | **no cumple**: p95 101–239 ms, máx 287–320 ms |
| Intervalo de respuestas, órbita de 10 s tras el objetivo | **no cumple**: p50 ya 480 ms (Majadas) y 2,6 s (Belvedere 8 px): cada frame reconstruye la escena en CPU | **no cumple** en Majadas (p50 482 ms); Belvedere no llegó al objetivo |
| Etapa LOD publicada nunca sube en órbita | cumple (0 subidas en los registros LOD) | cumple (0 subidas) |
| Sin aumento de nodos sin cubrir / píxeles vacíos vs. base | (es la base) | cumple (igual máx. y final; vacíos dentro del ruido) |

Sobre toda la carga el p95 llega a 1,0 s (Majadas) y 5–7 s (Belvedere): los rebuilds de 0,3–2,7 s por `asyncInsert` bloquean la cola serie del worker.

## Criterios de parada de T4 (B frente a A)

- `uncoveredNodes`: máx 59 (Majadas) y 81 (Belvedere) y final 0 en ambos brazos; no sube. **Cumple.**
- Píxeles vacíos: sin diferencia apreciable (arriba). **Cumple.**
- `pendingTargetEvictions` 0 y `cacheCycleDetected` false en todas las ejecuciones de B (A no los expone; `evictedPendingBlocks` 0 en ambos). **Cumple.**
- `SourceDetailBudgetExceeded`: no aparece en estados finales ni en la consola. **Cumple.**
- `oversizedVoxels` final: A 0/435 (Belvedere 8/4 px) frente a B 10 con el objetivo aún pendiente; sin comparación válida mientras B no termine.

> Hallazgo fuera de la lista literal de criterios: **B es más lento hasta el objetivo**. Majadas 4 px +10 % (61,4 s frente a 55,8 s, rangos sin solaparse); Belvedere 8 px no llega al objetivo en 300 s en 3/3 rondas de B (A: 252–255 s); Belvedere 4 px: A alcanza la etapa 4 px a 260–270 s, B sigue en 8 px a 300 s. Causa observada: la métrica única selecciona más puntos de fuente por reconstrucción (`voxelSourcePoints` ÷ `sceneBuilds`: Majadas 2,6 M frente a 2,4 M, Belvedere 3,9 M frente a 2,5 M), la voxelización por reconstrucción pesa más y se cargan menos bloques en el mismo tiempo (326 frente a 389). Esto contradice el resultado de T4 de "nunca más puntos con la misma cámara": en el visor en vivo el corte intermedio de B es mayor. Es una hipótesis `inferred` y requiere confirmar con un A/B de selección por etapa (puntos seleccionados por etapa para la misma cámara) antes de decidir si se revierte T4 o se acepta como coste de cumplir el contrato de arista.

## Puerta de R2 (E13)

La decodificación async suma 0,3–0,9 % del tiempo ocupado del worker (<< 20 %); la voxelización por vista (`voxelBuildMs`, dentro de `asyncInsert` y `frame`) ocupa 98–99,5 %, y la lectura (140–880 s) es espera de E/S fuera del worker. **Recomendación: spike de worker de construcción** que posea la caché y transfiera buffers de instancias, no workers de decodificación.

## Anomalías y límites

- `sceneBuilds` y `voxelSourcePoints` varían ±5 % entre rondas del mismo brazo (menos de 30 %: aceptable); los tiempos de pared son orientativos (n = 3 / 2).
- La ventana "carga" cubre solo los primeros 10 s de la carga (etapa gruesa), por eso su p95 es mucho menor que el de toda la carga.
- `loadedBlocks` es 0 (solo cuenta la ruta síncrona); se usan `asyncBlocks.blocks`. Un 404 de recurso estático aparece en la consola en todas las ejecuciones (sin errores JS).
- El PSS lo mide un muestreador externo (suma de todos los procesos `chrome`); la sonda no lo registra; no incluye VRAM.
- Cinco ejecuciones iniciales de Majadas con la sonda previa (sin `lodFinal`) se descartaron y toda la matriz se repitió con la sonda de `f189849`; las 18 ejecuciones publicadas usan esa sonda. Un `pkill -f` propio terminó un shell y dejó Chrome huérfano; se mató antes de repetir y no afecta a los datos.
