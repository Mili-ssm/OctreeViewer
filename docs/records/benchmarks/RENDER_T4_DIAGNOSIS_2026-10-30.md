# Diagnóstico de la regresión de T4 en Belvedere (30-10-2026)

Sigue al hallazgo de [RENDER_BASELINE_2026-10-30.md](RENDER_BASELINE_2026-10-30.md): el brazo B (`87ef649`) tarda más que A (`ede7e0a`) en llegar al objetivo LOD y construye escenas con más puntos de fuente. Este registro aísla la causa con el índice real de `belvedere.oct`. No cambia comportamiento del producto.

## Conclusión

| Hipótesis | Veredicto | Evidencia |
| --- | --- | --- |
| H1: quitar `- geometric_error` agranda la métrica y el corte es más fino | **Refutada** | A nunca usó el `geometric_error` de la jerarquía en la selección: pasaba `side / 8` como "error" y como desplazamiento de profundidad. El corte es idéntico en A y B a 16, 8 y 4 px (tabla 1). |
| Causa real: el orden de carga de los bloques pendientes | **Confirmada en simulación** | A ordenaba por `node.geometric_error()`: ≈ 0 en hojas, grande en nodos internos. Cargaba primero los internos (≈ 100 puntos cada uno) y las hojas densas al final. B ordena por `side / 8`: intercala hojas densas desde el principio. Con el mismo corte, la escena intermedia de B pesa 2,3 veces más a 8 px (tabla 2). |
| H2: memo de selección obsoleta | No probada | No hace falta: el corte final no cambia y la simulación explica el sentido y el orden de magnitud. La memo se invalida por cámara, viewport y etapa, y la jerarquía es inmutable. |
| H3: detector de thrash | Descartada | `pendingTargetEvictions` 0 y `cacheCycleDetected` false (baseline). |

## Fórmulas

- **Antigua (A), selección:** `error_px = e * H * k / (2 tan(fov/2) * d)` con `e = side/8` (unidades del dataset), `k` el factor fuera de eje, `d = max(prof_centro - semidiagonal - e, near)`. El mismo `e` entra como numerador y como resta de profundidad: la resta aleja la cota por una celda, de forma conservadora.
- **Antigua (A), prioridad de bloques pendientes:** la misma fórmula con `e = node.geometric_error()`. En Belvedere vale 656, 328, 164, 82, 41 por nivel en nodos internos (≈ 0,87 de `side/8`) y 0,0001 en hojas (0 acotado).
- **Nueva (B), selección y prioridad:** `e = side/8`, `d = max(prof_mínima_de_las_8_esquinas, near)`, sin resta. La profundidad de la esquina más cercana es mayor o igual que la de la esfera menos `e`, por lo que B nunca estima más error que A: el corte no puede ser más fino (confirmado abajo).
- Unidades: dataset, escaladas por `visual_scale` (0,0165 en Belvedere) y con `coordinate_origin` restado antes de proyectar.

## Método

Diagnóstico `#[ignore]` en `crates/viewer_render_wgpu/src/camera/t4_diagnosis.rs`. Lee solo el índice con `viewer_format::FormatReader`, ajusta `OrbitCamera::fit` a los límites raíz con viewport 1282x722 (el de la sonda), y selecciona con `select_lod_stable_projector`. Reimplementa la métrica antigua copiada de `ede7e0a`. Ejecución:

`BELVEDERE_OCT=.../belvedere.oct cargo test -p viewer_render_wgpu --lib t4_diagnosis -- --ignored --nocapture`

Belvedere: 1343 nodos, niveles 0 a 5, 81 682 329 puntos, `visual_scale` 0,0165. Los nodos internos llevan ≈ 100 puntos (submuestra); las hojas, los datos densos.

## Tabla 1: corte seleccionado en la cámara inicial

| Objetivo | Métrica | Nodos | Puntos | Bloques | Niveles | Máx. métrica | Celdas > objetivo |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 16 px | antigua | 81 | 915 526 | 81 | 2:1, 3:80 | 10,79 | 1 |
| 16 px | nueva | 81 | 915 526 | 81 | 2:1, 3:80 | 10,56 | 1 |
| 8 px | antigua | 320 | 13 238 581 | 320 | 2:1, 3:11, 4:308 | 5,07 | 1 |
| 8 px | nueva | 320 | 13 238 581 | 320 | 2:1, 3:11, 4:308 | 5,02 | 1 |
| 4 px | antigua | 1004 | 74 188 113 | 1004 | 2:1, 3:11, 4:156, 5:836 | 3,97 | 12 |
| 4 px | nueva | 1004 | 74 188 113 | 1004 | 2:1, 3:11, 4:156, 5:836 | 3,94 | 12 |

Bloques pendientes = bloques del corte (caché vacía). "Celdas > objetivo": nodos cuya celda representativa más cercana proyecta una arista mayor que el objetivo (aproximación de `oversizedVoxels`, que el binario no calcula sin renderer). Es idéntico en ambas métricas, igual que los nodos limitados por resolución (`resolution_limited`: 1, 12 y 138 a 16, 8 y 4 px). Por tanto A no violaba el contrato visible más que B, ni B lo corrige en esta cámara.

Cortes del 8 px y 4 px: a 8 px hay 168 nodos internos (17 138 puntos) y 152 hojas (13 221 443 puntos); a 4 px, 16 internos (1812 puntos) y 988 hojas (74 186 301 puntos).

## Tabla 2: simulación de carga (un bloque por actualización, corte del 8 px y luego del 4 px)

El planificador (`LodTransitionPlanner::prepare`) resuelve el corte con residencia ficticia (bloques de 1 punto) y se suman los puntos de fuente de los nodos del plan tras cada carga. Misma función de selección (nueva) para todas; solo cambia el orden.

| Orden de carga | Etapa 8 px: acumulado | Media por build | Puntos de las primeras 50 cargas | Etapa 4 px: acumulado |
| --- | --- | --- | --- | --- |
| A (antigua con `geometric_error`) | 1,009 G | 3,15 M | 5 115 | 36,51 G |
| B (nueva con `side/8`) | **2,343 G** (x2,3) | 7,32 M | 2 590 200 | 35,80 G |
| C (métrica nueva con `geometric_error`) | 1,010 G | 3,15 M | 5 115 | 36,51 G |
| D (B dividido por puntos del nodo) | 0,561 G (x0,55 de A) | 1,75 M | 3 724 | 27,89 G |

- C reproduce A: el orden lo fija el uso de `geometric_error` y no la profundidad.
- A y B coinciden a 4 px (diferencia 2 %): la regresión vive en las etapas gruesas. En vivo, B de Belvedere a 4 px se quedaba en la etapa de 8 px.
- La proporción simulada (x2,3) es mayor que la observada en `voxelSourcePoints` por build (3,9 M frente a 2,5 M, x1,56): la simulación no modela la retención del corte previo ni el reuso de escena. Valida el sentido y la causa, no la cifra.

## Opciones de arreglo (no aplicadas)

| Opción | Efecto simulado a 8 px | Coste y riesgo |
| --- | --- | --- |
| 1. Volver a `geometric_error` como prioridad (C) | = A | Reabre los empates por `BlockId` en hojas, motivo de T4. Las hojas no tienen prioridad espacial. |
| 2. Mantener la métrica única y ordenar por error por punto de fuente (D) | 0,55 de A | Cambia el orden de carga: cercanía y coste. Riesgo: retrasa hojas lejanas y densas; hay que medir la sensación visual (nodos sin cubrir, píxeles vacíos). |
| 3. Cargar los nodos internos antes que las hojas (por nivel) y, dentro, por la métrica B | ≈ A en coste, con orden espacial | Regla simple; no probada en este diagnóstico. |
| 4. Aceptar el coste de B | n/a | Rompe el criterio de tiempo hasta objetivo (Belvedere 8 px no llega en 300 s). |

Recomendación: probar 2 y 3 en la simulación y después en un A/B en vivo. Restaurar la resta de `geometric_error` (H1) no cambia nada: no era la causa.

## Límites

- Solo cámara inicial de Belvedere; Majadas (+10 %) no se simuló (415 M puntos, 6259 nodos): el mecanismo es el mismo, falta la cifra.
- Simulación sin retención del planificador, sin lotes asíncronos y con coste = puntos de fuente. `oversizedVoxels` se aproxima con la celda del vértice más cercano (no con el constructor de vóxeles).
- Orden de la carga asíncrona: el viewer pide en orden de prioridad por lotes (`prioritized_pending_blocks`); aquí se modela la carga secuencial.

## Verificación de la corrección del orden de carga

Medición en vivo de `d4604b7` (prioridad de bloque pendiente = error en pantalla ÷ puntos de fuente; opción 2/D de arriba). Datos crudos: [JSON por ejecución](data/render-t4-fix-2026-10-30/) (`.json`, `.png` final y `.log`).

### Método

- Mismo entorno y sonda que [RENDER_BASELINE_2026-10-30.md](RENDER_BASELINE_2026-10-30.md): Chrome con ANGLE-Vulkan, adaptador `amd` `rdna-3` y `mode: "timing"` en las 18 ejecuciones; sonda `probe-orbit-load.mjs` de HEAD; wasm `--release`.
- **A = `ede7e0a`** (se vuelve a medir para emparejar; `.wasm` sha `9833061a09a9…`, idéntico al de la línea base) y **C = `d4604b7`** (`0e2a6a8d0b6b…`). Worktrees `target/fix-A` y `target/fix-C` (retirados), puertos 8801 y 8802.
- Matriz alternando A/C: Majadas 4 px (tope 180 s) 2 rondas y 1 ronda `--pixels` por brazo; Belvedere 8 px (tope 300 s) 3 rondas. `loadavg` de arranque 0,99–1,46 (< 1,5). Las rondas `--pixels` no entran en las medianas.
- Medianas con [mín–máx]. En Majadas el tiempo a objetivo tiene resolución de 2 s (sondeo), por eso el rango es nulo.

### Resultados

| Escenario | Brazo | Objetivo | `sceneBuilds` | `voxelSourcePoints` (M) | Bloques async | `uncoveredNodes` máx/final |
| --- | --- | --- | --- | --- | --- | --- |
| Majadas 4 px (n=2) | A | 56,8 s [56,8–56,8] | 373 [353–393] | 836 [747–924] | 239 | 59 / 0 |
| Majadas 4 px (n=2) | C | **50,8 s** [50,8–50,8] | 380 [375–384] | **696** [623–769] | 239 | 59 / 0 |
| Belvedere 8 px (n=3) | A | 253 s [251–257] | 541 [541–542] | 1313 [1308–1315] | 389 | 81 / 0 |
| Belvedere 8 px (n=3) | C | **158,9 s** [158,9–160,9] | 556 [552–570] | **922** [918–933] | 380 | 81 / 0 |

Para comparar con B (línea base): Majadas B 61,4 s, 1041 M puntos; Belvedere 8 px B sin objetivo a 300 s, 1746 M puntos. C reduce el tiempo a objetivo un 17 % (Majadas, frente a B) y de >300 s a 159 s (Belvedere).

Intervalo entre respuestas de frame (ms).

| Escenario | Brazo | Carga p50 / p95 / máx | Órbita p50 / p95 / máx |
| --- | --- | --- | --- |
| Majadas 4 px | A | 67 / 236 [194–278] / 410 [331–489] | 485 / 492 / 499 |
| Majadas 4 px | C | 82 / 104 [85–124] / 226 [154–298] | 485 / 498 / 514 |
| Belvedere 8 px | A | 67 / 88 [86–90] / 239 [185–261] | 2713 / 2720 / 2720 |
| Belvedere 8 px | C | 67 / 84 [83–88] / 144 [140–168] | 67 / 2447 / 2618 |

Contadores de C (A no los expone): `pendingTargetEvictions` 0 y `cacheCycleDetected` false en las 5 ejecuciones; selecciones por actualización 0,61 (Majadas) y 0,57 (Belvedere); preparaciones por actualización 1,28 en ambas. `oversizedVoxels` máximo 0 (Majadas) y 243 (Belvedere), final 0 en A y C. Píxeles vacíos (Majadas, 1 ronda; carga, tras objetivo, final): A 0,766 / 0,780 / 0,778, 0,886 / 0,894 / 0,897, 0,8992; C 0,776 / 0,780 / 0,781, 0,885 / 0,893 / 0,892, 0,8992. `SourceDetailBudgetExceeded` no aparece en ningún estado final ni registro.

### Puertas

| Criterio | Resultado |
| --- | --- |
| C no sube `uncoveredNodes` frente a A | **Cumple** (59/0 y 81/0, igual que A y B) |
| C no sube píxeles vacíos frente a A | **Cumple** (diferencia ≤ 0,01 en la carga, igual en el final; dentro del ruido) |
| C no sube `pendingTargetEvictions` | **Cumple** (0; `cacheCycleDetected` false) |
| Sin `SourceDetailBudgetExceeded` | **Cumple** |
| Tiempo a objetivo de C ≤ A | **Cumple**: Majadas 50,8 s frente a 56,8 s (-11 %); Belvedere 8 px 159 s frente a 253 s (-37 %), rangos sin solaparse |

### Lectura y límites

- C necesita menos puntos de fuente que A (-17 % Majadas, -30 % Belvedere), en el sentido que predecía la simulación (D = 0,55 de A en la etapa 8 px; el vivo da 0,70). `observed`.
- Observado sin investigar la causa: en Belvedere la órbita tras el objetivo tiene p50 de 67 ms con C (43 respuestas en 10 s, frente a 4 con A) y p95 de 2,4 s; el criterio de p95 < 33 ms sigue sin cumplirse. En Majadas la órbita sigue en 485 ms en ambos brazos. Hace falta repetir y mirar el registro antes de atribuirlo a C.
- Belvedere C carga 380 bloques async frente a 389 de A (el corte final difiere en 9 bloques); `uncoveredNodes` y `oversizedVoxels` finales son iguales.
- Majadas n = 2 y tiempos con paso de 2 s: orientativo, aunque el desplazamiento entre brazos supera el rango. Solo se midió 8 px en Belvedere (no 4 px) y una ronda de píxeles por brazo.
