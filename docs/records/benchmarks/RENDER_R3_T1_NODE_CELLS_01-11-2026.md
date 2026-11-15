# RENDER-R3 T1: celdas por nodo sobre datos reales

Fecha: 01-11-2026. Rama: `feat/render-r3-node-cells`. Estado: **criterios de parada superados; R3 en pausa ([ADR 0028](../../adr/0028-r4-antes-que-r3.md))**.

## Método

`cargo run --release -p viewer_render_wgpu --example node_cells_bench -- target/render-data/<dataset>.oct` recorre todos los nodos con `PointNodeCellSource` (Tarea 1 del [plan de R3](../../plans/render-r3-cubos-en-gpu.md)). Los bytes de la cadena se calculan con instancias de 16 B y los de los puntos con `size_of::<Point>()` × puntos. Se usan los mismos datasets que en el [registro T7](RENDER_R1_T7_01-11-2026.md) (SHA-256 en su `environment.txt`). Salida completa: [Majadas](data/render-r3-t1-node-cells-majadas-01-11-2026.txt), [Belvedere](data/render-r3-t1-node-cells-belvedere-01-11-2026.txt).

## Resultados

| | Majadas | Belvedere |
| --- | --- | --- |
| Nodos (hojas) | 6259 (4990) | 1343 (1083) |
| Hojas con la cadena en el tope (profundidad de representantes + 7) | **77,7 %** | **99,0 %** |
| Histograma de longitud de la cadena (niveles: hojas) | 1: 183 · 2: 40 · 3: 25 · 4: 27 · 5: 9 · 6: 23 · 7: 807 · 8: 3876 | 1: 11 · 8: 1072 |
| Hojas con la cadena > 4 × los bytes de sus puntos | **131** (peor 5,65 ×) | 0 (peor 3,14 ×) |
| Bytes de cadenas / bytes de puntos | 18,1 GB / 6,6 GB (2,72 ×) | 3,5 GB / 1,3 GB (2,71 ×) |
| Construcción por hoja | 68,8 ms (827 ns/punto) | 63,4 ms (841 ns/punto) |
| Construcción por nodo interno | 0,011 ms | 0,010 ms |

Se superan dos criterios de parada del plan: más del 1 % de las hojas llegan al tope con celdas aún creciendo, y hay cadenas de más de 4 × los bytes de sus puntos. Las hojas son densas (unos 75–83 k puntos), así que el número de celdas sigue creciendo en los 7 niveles. El tiempo por punto no está optimizado (código Morton bit a bit y una ordenación por nivel).

## Profundidad de representantes (A1)

En Majadas hay 102 nodos (niveles 7–12) cuyos lados se desvían unos 10⁻⁵ en relación de `root_side / 2^k`, porque los límites se guardan en `f32`. Con `floor(log2)` daban un nivel de menos (por ejemplo, log2 = 13,9999904 se quedaba en 13), tanto en la ruta nueva como en la CPU (`voxel_grid.rs`, que ahora llama a la misma función). Ahora la función compartida `representative_depth` redondea al entero cuando la diferencia es menor de 10⁻³ (decisión del propietario, 01-11-2026). Con esa corrección, cada nivel tiene una única profundidad (nivel *n* → profundidad *n* + 3). Belvedere no tiene nodos afectados. Los 48 digests dorados de `viewer_render_wgpu` no cambian, porque sus fixtures tienen lados exactos.

## Límites

- Mide almacenamiento y CPU, no GPU ni presentación.
- Abiertos para cuando se reanude: `NodeCell` no guarda cuántos puntos tienen intensidad (la media solo coincide con la ruta CPU si todos la tienen), y la clase es `u8`, con 0 cuando falta, mientras que la ruta CPU usa −1.
