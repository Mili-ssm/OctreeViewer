# 0028 R4 (pirámide densa) antes que R3 (cubos por nodo en GPU)

Estado: Aceptada · Fecha: 01-11-2026 · Decide: propietario

## Contexto
- El orden acordado el 30-10-2026 era R3 → R4 ([requisitos](../requirements.md#orden-de-trabajo-acordado)), y [ADR 0027](0027-celdas-por-nodo-en-pool-paginado.md) diseñó R3 sobre el formato actual: cada hoja construye una cadena de profundidades que llega hasta la densidad de su fuente.
- La Tarea 1 del [plan de R3](../plans/render-r3-cubos-en-gpu.md) midió esa cadena sobre todos los nodos reales ([registro](../records/benchmarks/RENDER_R3_T1_NODE_CELLS_01-11-2026.md)). Las hojas tienen unos 75–83 k puntos y su cadena llega al tope (profundidad de representantes + 7) en el 78 % de las hojas de Majadas y en el 99 % de las de Belvedere. Ocupa unas 2,7 veces los bytes de sus puntos (18 GB en Majadas). Se superan dos criterios de parada del plan: más del 1 % de hojas en el tope y cadenas de más de 4 × los bytes de sus puntos.

## Decisión
Se hace R4 ([ADR 0020](0020-piramide-lod-densa.md), [ADR 0021](0021-rejilla-global-determinista-y-crs.md)) antes que R3. Las hojas de 16–64 k puntos y el espaciado por nodo de la pirámide densa acortan la cadena de hoja. R3 se reanuda sobre el formato reconvertido, y la Tarea 1 se vuelve a medir con él antes de la Tarea 3.

Lo que se conserva de la Tarea 1: `node_cells.rs`, sus tests y el bench, y la tolerancia de profundidad compartida con la ruta CPU. Lo usa R4 como fuente de celdas.

## Alternativas
| Alternativa | Por qué no |
| --- | --- |
| Generar cada nivel de hoja bajo demanda, con caché LRU | Cambia el contrato de ADR 0027; ~70 ms por hoja en cada cambio de profundidad |
| Acortar `CHAIN_SPAN` | Cubos mayores que el objetivo en hojas densas: incumple RQ-09 |

## Consecuencias
- R3 queda en pausa; R1-T1b (Tarea 14 de R3, modos de px) también.
- La edad de escena tras T1a ([diagnóstico](../records/benchmarks/RENDER_R1_T7_SCENE_AGE_01-11-2026.md)) se mantiene hasta R3.
- La Tarea 1 de R3 y R4 deben medirse juntas: si la pirámide densa no lleva la cadena por debajo de los criterios, ADR 0027 se revisa.

## Revisar si
Con el formato de R4, más del 1 % de las hojas siguen llegando al tope de la cadena con celdas aún creciendo.
