# R4 T6 (repetición): matriz de render tras los arreglos del visor

Fecha: 02-11-2026. Código medido: `2bddbff`. Estado: **pasa los criterios de render de la Tarea 6; la conversión sigue en +87–99 % (aceptado por el propietario); R3 sigue en pausa**.

Repite el [registro T6](R4_T6_MEASUREMENT_02-11-2026.md) tras dos arreglos: la parada de celdas representativas por nodo (`0a30552`) y la extensión real de los datos en el índice, que usan la cámara y la sonda (`2bddbff`).

## Método

Mismo arnés y escenarios que T6 y [T7](RENDER_R1_T7_01-11-2026.md); 3 rondas de tiempo por escenario y una de `--pixels` por escenario. Una ronda `--pixels` de los ficheros V5 de T7 con el mismo build, para comparar la métrica de píxeles vacíos con el mismo código. Carga a 1 min antes de cada ronda: 0,80–1,83 (más que en T6). Único error de consola: 404 de `favicon.ico`. Datos y SHA-256: [r4-t6b-02-11-2026/](data/r4-t6b-02-11-2026/).

## Resultados (mediana [rango], n = 3)

| | Majadas 4 px | Belvedere 8 px | Belvedere 4 px |
| --- | --- | --- | --- |
| Tiempo al objetivo (T7 V5) | 11,2 s (10,8) | 10,8 s (48,8) | 10,7 s (sin objetivo en 300 s) |
| Respuestas p95 / máx | 17,7 / 18,8 ms | 17,6 / 19,6 ms | 17,5 / 21,8 ms |
| Edad de escena p95 (T7 V5) | 72 ms (539) | 270 ms (2932) | 994 ms (907) |
| Puntos fuente (T7 V5) | 41,5 M (291) | 21,2 M (310) | 16,4 M (957) |
| Vóxeles sobredimensionados | 0 | 0 | 0 |
| Nodos sin cubrir al final | 0 | 0 | 0 |
| Píxeles vacíos en el casco | 0,85–0,87 | **0,29–0,36** | 0,30–0,38 |
| Píxeles vacíos, V5 con el mismo build | 0,88–0,90 | 0,71–0,76 | — |

El tiempo al objetivo tiene un suelo de 10 s: la sonda orbita 10 s antes de comprobarlo (`probe-orbit-load.mjs`).

## Lectura frente a los criterios del plan

| Criterio | Resultado |
| --- | --- |
| Disco ≤ V5 + 35 % | Cumple: +2,1 % y +7,6 % ([T6](R4_T6_MEASUREMENT_02-11-2026.md)) |
| Conversión ≤ +30 % | No cumple: +87 % y +99 %; el propietario acepta seguir y optimizar antes de promover V7 |
| Nodos sin cubrir sin regresión | Cumple: 0 |
| Píxeles vacíos sin regresión | Cumple y mejora: Belvedere 0,71–0,76 → 0,29–0,36; Majadas 0,88–0,90 → 0,85–0,87 |
| R3-T1: < 1 % de hojas en el tope | No cumple: 1,96 % y 1,42 % ([T6](R4_T6_MEASUREMENT_02-11-2026.md)); R3 sigue en pausa |

## Límites

- Equipo más cargado que en T6; una ronda `--pixels` por escenario.
- La extensión de datos sale de la cabecera LAS.
