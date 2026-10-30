# Medición del worker de construcción (R2-T5, 30-10-2026)

Mide `?buildWorker=1` (R2-T3, HEAD `199a82f`) frente a la ruta actual. Datos crudos, `.log`, `.png` final y muestras de PSS: [JSON por ejecución](data/render-build-worker-2026-10-30/).

## Método

- Un solo `pkg/` (wasm `--release`, sha `abe458c6e5ea…`) servido desde la raíz del repo en el puerto 8811; sonda `probe-orbit-load.mjs` de HEAD, Chrome con ANGLE-Vulkan, adaptador `amd` `rdna-3`, `mode: "timing"` en las 14 ejecuciones. `loadavg` de arranque < 1,5.
- **OFF** = HEAD sin `--build-worker`; **ON** = HEAD con `--build-worker`. Alternados por ronda. Majadas 4 px (tope 180 s): 3 rondas por brazo y 1 ronda `--pixels` por brazo (fuera de las medianas). Belvedere 8 px (tope 300 s): 3 rondas por brazo.
- Medianas con [mín–máx]. PSS = suma del árbol de Chrome (`sample-process-memory.py`, 1 s), máximo de la ejecución. «Tiempo a objetivo» = `loadElapsedMs`.
- Intervalo entre respuestas de frame en ms; «carga» = primeros 10 s de órbita durante la carga; «órbita» = 10 s tras el objetivo.

## Resultados

| Escenario | Brazo | Tiempo a objetivo (s) | Carga p50 / p95 / máx | Órbita p50 / p95 / máx |
| --- | --- | --- | --- | --- |
| Majadas 4 px | OFF | 48,8 [48,8–48,8] | 66,7 / 80 [70–85] / 285 [196–308] | 483 / 501 / 507 [494–539] |
| Majadas 4 px | ON | **18,8** [18,8–18,8] | 16,7 / 17,5 [17,5–17,6] / 18,8 [18,5–18,8] | 16,7 / 17,5 / 20,3 [18,5–33,9] |
| Belvedere 8 px | OFF | 157,0 [156,9–157,0] | 67 / 99,7 [83–100] / 202 [167–204] | 67 / 2473 [2456–2487] / 2638 [2608–2678] |
| Belvedere 8 px | ON | **66,8** [60,8–70,8] | 16,7 / 17,6 / 18,9 [18,2–19,4] | 16,7 / 17,5 / 19,0 [18,7–20,4] |

Resolución de los tiempos a objetivo: paso de sondeo de 2 s (el estado de la UI da 17,0–18,3 s en Majadas ON y 59,3–69,8 s en Belvedere ON).

| Escenario | Brazo | `sceneBuilds` | `voxelSourcePoints` (M) | Bloques async | `uncoveredNodes` máx/final | `pendingTargetEvictions` / ciclo | PSS máx del árbol (MiB) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Majadas | OFF | 398 [381–415] | 693 [666–789] | 239 | 59 / 0 | 0 / false | 601 [601–609] |
| Majadas | ON | 197 [175–199] | 381 [379–410] | 239 | 59 / 0 | 0 / false | 664 [654–670] |
| Belvedere | OFF | 547 [546–567] | 904 [902–929] | 380 | 81 / 0 | 0 / false | 677 [673–682] |
| Belvedere | ON | 232 [227–238] | 424 [420–449] | 380 | 81 / 0 | 0 / false | 717 [717–719] |

`loadedBlocks` del contador síncrono vale 0 en ambos brazos (solo cuenta la ruta síncrona); la fila «Bloques async» es la cifra válida. `oversizedVoxels` 0 en todo. Píxeles vacíos (Majadas, 1 ronda; región del dataset en carga / tras objetivo): OFF 0,764–0,783 / 0,885–0,892; ON 0,774–0,783 / 0,884–0,895.

Métricas solo de ON (`buildWorkerMetrics`):

| Escenario | `sceneAgeMs` p50 / p95 / máx | `epochLag` p50 / p95 / máx | Vistas enviadas / fusionadas | `scenesDropped` | `sceneApplyMs` máx | Wasm render / build (MiB) | Build máx (ms) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Majadas | 77 [76–95] / 527 [485–586] / 560 [498–729] | 4 / 31 [29–34] / 33 [29–43] | 1245 / 1057 [1049–1099] | 0 | 1,3 [1,3–1,6] | 1,9 / 122,8 | 557 [481–715] |
| Belvedere | 86 [80–88] / 120 [114–124] / 2638 [2629–2690] | 4 / 6 / 157 [156–161] | 1245 / 1069 [1068–1076] | 0 | 1,5 [1,3–1,5] | 1,8 / 204,3 | 2696 [2660–3157] |

Memoria wasm de OFF (un solo worker): la sonda no la expone, no medido.

## Puertas

| Puerta | Veredicto |
| --- | --- |
| ON: respuestas de frame p95 < 33 ms y máx < 100 ms, en carga y órbita, por dataset | **Cumple**: p95 ≤ 17,6 ms y máx ≤ 33,9 ms en las 12 fases (peor caso: Majadas, una ronda con máx 33,9 ms en órbita). OFF no cumple (p95 80–2473 ms) |
| ON no sube `uncoveredNodes` (59 / 81 máx, 0 final), píxeles vacíos ni `pendingTargetEvictions` | **Cumple**: iguales a OFF; píxeles vacíos dentro del ruido (±0,02); evicciones 0, `cacheCycleDetected` false |
| Tiempo a objetivo de ON no peor que OFF + 10 % | **Cumple, con holgura**: −62 % Majadas, −57 % Belvedere (rangos sin solapar) |
| Sin `SourceDetailBudgetExceeded` | **Cumple** (no aparece en ningún JSON ni registro) |
| Digest final en reposo igual en ON y OFF | **No comprobado**: la sonda no expone el digest (solo `build-worker.js` lo adjunta al mensaje `scene`) |

## Lectura y límites

- ON es mucho más rápido en llegar al objetivo porque las vistas se fusionan («gana la última», ~85 % de 1245 vistas) y se hace la mitad de construcciones y de puntos de fuente; en OFF cada movimiento de cámara encola una reconstrucción serie. `observed`; la causa exacta del ahorro de puntos no se aisló.
- La métrica de intervalo mide respuestas de frame, no frescura: durante la órbita ON dibuja una escena envejecida. `sceneAgeMs` p95 es 120 ms (Belvedere) y 527 ms (Majadas), pero el máximo es una construcción completa (0,56 s y 2,6 s), igual que el diseño predecía (B–2B). `sceneAgeMs` no está separado por fase.
- R2-T4 (subida en trozos) no hace falta: `sceneApplyMs` máx 1,6 ms < 8 ms.
- Coste: +63 MiB (Majadas) y +40 MiB (Belvedere) de PSS máximo del árbol frente a OFF; el wasm de construcción tiene 123 y 204 MiB. Una petición 404 en consola en todas las ejecuciones (sin relación con el brazo).
- Sin repetir: margen de frustum 0/5/10° (parámetro aún inexistente), así que la decisión de R2b queda con estos datos de `sceneAgeMs` y sin esa matriz. Un solo ordenador, una sola GPU, Chrome únicamente; Majadas n = 3 con paso de sondeo de 2 s.
