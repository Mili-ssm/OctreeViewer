# Exploración: bytes de la cadena de hoja de R3

Fecha: 02-11-2026. Rama: `explore/leaf-chain-bytes` (`5e95755`). Estado: **criterio de 4 × cumplido; pendiente del propietario**.

## Diagnóstico

El criterio compara 16 B por celda con `size_of::<Point>()` (16 B): más de 4 × significa más celdas en la cadena que 4 × puntos. Las hojas que lo superan no son densas: en la profundidad de representantes ya tienen ~0,8 × N celdas, y cada nivel de la cadena repite casi N celdas. Peor hoja de Majadas v7.5 (13 314 puntos): 10 802, 12 626, 13 114, 13 261, 13 297, 13 308, 13 313 celdas por nivel (~6,7 × N).

## Cambio

Poda de niveles casi duplicados (`chain_keep_mask`, `CHAIN_MIN_GROWTH` = 1,5 en `node_cells.rs`): de fino a grueso, un nivel se guarda solo si sus celdas × 1,5 ≤ las del siguiente nivel guardado; el más fino se guarda siempre. La cadena queda acotada a 3 × el nivel más fino. Una profundidad podada se resuelve con el siguiente nivel **más fino** guardado (mantiene RQ-09).

## Resultados (`node_cells_bench`)

| Dataset | Hojas > 4 × | Peor | Cadena / puntos | Niveles por hoja | Hojas en el tope |
| --- | --- | --- | --- | --- | --- |
| Majadas v7.5 | 578 → 0 | 6,74 → 2,58 × | 1,81 → 0,98 × | 1,91 → 1,03 | 0,14 % |
| Majadas v7.4 | 595 → 0 | 6,74 → 2,58 × | 1,81 → 0,98 × | 2,03 → 1,13 | 0,52 % |
| Belvedere v7.5 | 0 → 0 | 2,43 → 1,37 × | 1,96 → 0,99 × | 1,98 → 1,00 | 0,06 % |
| Belvedere v7.4 | 0 → 0 | 3,71 → 2,00 × | 1,96 → 0,99 × | 2,07 → 1,09 | 1,13 % |

Con g = 1,25 el peor es 3,40 ×; con g = 2, 1,83 ×. Belvedere v7.4 sigue por encima del 1 % de hojas en el tope (sin cambio); v7.5 cumple en ambos.

## Consecuencias para R3

- A6 del [plan de R3](../../plans/render-r3-cubos-en-gpu.md): `resident_depth` debe elegir la menor profundidad guardada ≥ la deseada (o la más fina), no la más gruesa cercana.
- `leaf_cell_depth` usa las profundidades guardadas, no un rango contiguo.
