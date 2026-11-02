# R4 T6: matriz de render y bench R3-T1 sobre `v7.4-pre`

Fecha: 02-11-2026. Código medido: `392abdb`. Estado: **no pasa: regresión de píxeles vacíos en Belvedere; R3 sigue en pausa**.

## Método

Mismo arnés y comando que el [registro T7](RENDER_R1_T7_01-11-2026.md) (`run-sampled.py`, `probe-orbit-load.mjs`, Chrome 156, órbita de 10 s, 3 rondas de tiempo + 1 con `--pixels`), sobre Majadas y Belvedere regenerados en `v7.4-pre`. Carga a 1 min antes de cada ronda: 0,37–0,90. El único error de consola es el 404 de `favicon.ico`. Datos, entorno y SHA-256: [r4-t6-02-11-2026/](data/r4-t6-02-11-2026/).

## Conversión (1 ejecución)

| | V5 ([T4b](R4_T4B_CONVERSION_02-11-2026.md)) | `v7.4-pre` | Diferencia |
| --- | --- | --- | --- |
| Majadas: tiempo / RSS | 222,7 s / 92 MB | 415,8 s / 242 MB | +87 % |
| Majadas: disco | 7 533 781 717 B | 7 695 583 813 B | +2,1 % |
| Belvedere: tiempo / RSS | 27,8 s / 61 MB | 55,2 s / 103 MB | +99 % |
| Belvedere: disco | 1 591 650 185 B | 1 712 809 917 B | +7,6 % |

## Matriz (mediana, n = 3)

| Escenario | Tiempo al objetivo T7 → ahora | Respuestas p95 / máx | Edad de escena p95 T7 → ahora | `sceneBuilds` T7 → ahora | Puntos fuente (M) T7 → ahora |
| --- | --- | --- | --- | --- | --- |
| Majadas 4 px | 10,8 → 11,1 s | 17,2 / 32,8 ms | 539 → 5,9 ms | 64 → 1249 | 291 → 30,0 |
| Belvedere 8 px | 48,8 → 10,7 s | 17,4 / 18,4 ms | 2932 → 2,7 ms | 67 → 1247 | 310 → 14,4 |
| Belvedere 4 px | sin objetivo en 300 s → 10,8 s | 17,2 / 30,3 ms | 907 → 6,5 ms | 111 → 1249 | 957 → 55,5 |

Nodos sin cubrir al final: 0 (como T7). Expulsiones de pendientes: 0. `oversizedVoxels`: 156 / 16 / 56 (T7: 0 salvo Belvedere 4 px, 191–259).

Píxeles vacíos (fracción en el casco, 3 capturas):

| | T7 | Ahora |
| --- | --- | --- |
| Majadas | 0,884–0,895 | 0,875 / 0,894 / 0,911 |
| Belvedere 8 px | 0,699–0,748 | **0,895 / 0,896 / 0,910** |

## Índice

| | Majadas | Belvedere |
| --- | --- | --- |
| Nodos / hojas / nivel máx. | 24 091 / 19 526 / 15 | 3 941 / 3 174 / 8 |
| Nodos con un solo hijo / cadena más larga | 6 / 1 | 9 / 3 |
| Bytes del índice | 13,5 MB (0,64 % del `.oct`) | 2,4 MB (0,29 %) |

## Bench R3-T1

| | Majadas | Belvedere |
| --- | --- | --- |
| Hojas en el tope ([R3-T1](RENDER_R3_T1_NODE_CELLS_01-11-2026.md) → ahora) | 77,7 % → **1,96 %** | 99,0 % → **1,42 %** |
| Hojas con cadena > 4 × sus puntos | 131 → 595 (peor 6,74 ×) | 0 → 0 (peor 3,71 ×) |
| Bytes de cadena / bytes de puntos | 2,72 × → 1,81 × | 2,71 × → 1,96 × |

## Lectura

- Disco y nodos sin cubrir cumplen. **Píxeles vacíos empeoran en Belvedere 8 px** (0,70–0,75 → 0,90): no pasa RQ-02.
- Los contadores son anómalos: unos 1250 `buildScene` por órbita (uno por frame), contadores idénticos entre rondas, tiempo al objetivo ~10,7–11,2 s en los tres escenarios (≈ duración de la órbita) y 5–20 veces menos puntos fuente. Apunta a un fallo del visor con `v7.4-pre`, no a una mejora. *Inferido; sin diagnosticar.*
- R3-T1: las hojas en el tope bajan del 78–99 % al 1,4–2,0 %, pero siguen por encima del 1 % de [ADR 0028](../../adr/0028-r4-antes-que-r3.md), y Majadas tiene más hojas con cadena > 4 ×. R3 no se reanuda todavía.
- Cadenas de nodos (OQ-05): 6 y 9 nodos con un solo hijo; el índice es < 1 % del fichero.

## Límites

- Una conversión por dataset; 3 rondas por escenario.
