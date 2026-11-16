# R3 fase 2a: métrica de refinado por celda medida en CPU — 16-11-2026

**Estado: puerta superada con `edge`; `bound` (la E1 provisional) no pasa.** Los criterios de parada de la [fase 2a](../../plans/fase-2a-lod-por-celda-en-gpu.md) se evaluaron con tres reglas de refinado. `edge` (refinar mientras la arista proyectada real de la celda supere el objetivo) pasa todos los criterios y da las menos instancias; `bound` falla el de celdas por encima del objetivo dentro de Majadas a 8 px (0,596 % > 0,5 %); `distance` (la regla de la fase 0a) falla por mucho. Este registro no cambia el plan: enmendar la E1 y el gemelo `cell_lod` es decisión del controlador.

Spike nativo, sin GPU, como el de la [fase 0a](R3_CELL_LOD_0A_15-11-2026.md): mide recuentos y bytes empaquetados, no tiempos de GPU. Opción nueva `--metric bound|edge|distance` de `coverage_spike.rs`.

## Veredicto

**Regla** (fijada antes de medir): se elige la métrica que pasa los criterios de parada con menos instancias. Criterios, en Majadas y Belvedere salvo donde se indica:

1. Majadas, pose rasante: instancias ≤ 2,5× la ruta CPU a 1 px y ≤ 2× a 2–8 px.
2. Celdas dibujadas por encima del objetivo según la arista proyectada real (`projected_voxel_edge_pixels`): ≤ 0,5 % dentro del dataset (pose «dentro de una hoja») y 0 fuera (órbita 0° y rasante).
3. `edge` solo califica con 0 pares no monótonos.

Si ninguna pasa, la fase 2a se detiene y se replantea la métrica con el propietario.

| Métrica | Rasante Majadas vs CPU (1/2/4/8 px) | Sobre objetivo dentro (peor caso) | Sobre objetivo fuera | Pares no monótonos | Veredicto |
| --- | --- | --- | --- | ---: | --- |
| `bound` | 2,27 / 1,00 / 1,00 / 1,00 | **0,596 %** (Majadas, 8 px) | 0 | 0 | **Falla** (criterio 2) |
| `edge` | 2,27 / 1,00 / 1,00 / 1,00 | 0,000 % (4 celdas de 16,9 M, Majadas 1 px; Belvedere 0,006 %) | 0 | 0 | **Pasa** |
| `distance` | 2,27 / 1,00 / 1,00 / 1,00 | 14,8 % (Majadas, 8 px) | 0 en Majadas; 4,6–5,1 % en la rasante de Belvedere | 0 | **Falla** (criterio 2) |

**Elegida: `edge`.** Es la única que pasa y da menos instancias que `bound` o las mismas (empates en la órbita 0° y la rasante a 1 px de Majadas) (0,85–0,99× `octant_tight` dentro de Majadas frente a 0,995–1,08×).

### Alcance del veredicto (revisión del controlador, 16-11-2026)

- El criterio 2 lo cumple `edge` **por construcción**: refina mientras la misma arista proyectada supera el objetivo, así que solo quedan por encima las celdas que no pueden refinar (varios puntos en S o la profundidad máxima) y las que tienen una esquina detrás del ojo (`+inf`). Lo que decide entre `edge` y `bound` es el número de instancias y de bytes, no ese criterio.
- Los pares no monótonos se cuentan solo por encima de la profundidad de separación S; por debajo, la cola de un punto por celda (hasta el 2,4 % de las instancias de `edge` dentro de Majadas a 8 px) no se comprobó.
- `bound` y `edge` dependen de `cell_lod.rs` y `camera.rs` del commit medido (`c3091fb`), que `identity.txt` no resume; el spike de commits posteriores calcula `bound` con el plano cercano del frustum y no es idéntico bit a bit.

## Hechos

- **Instancias en la rasante de Majadas:** las tres métricas dan lo mismo que `octant_tight` y la CPU (0,99–1,00×), salvo el 2,27× a 1 px que ya tenía `octant_tight`. `bound` da 3.873 y 1.957 instancias a 4 y 8 px (1 y 6 más que `edge` y `distance`).
- **`bound` dentro de una hoja (Majadas):** más instancias que `octant_tight` a 1, 2 y 4 px (1,08×, 1,07×, 1,03×; 18,5 M frente a 17,1 M a 1 px) y 0,995× a 8 px. Celdas sobre el objetivo: 27.982 (0,151 %) a 1 px, 6.339 (0,063 %) a 2, 1.063 (0,023 %) a 4 y **10.993 (0,596 %) a 8 px**.
- **`edge` dentro de una hoja (Majadas):** 0,985×, 0,957×, 0,890× y 0,846× las de `octant_tight`; sobre el objetivo, 4 celdas a 1 px y 0 a 2–8 px.
- **`distance`:** reproduce exactamente las instancias y las celdas sobre el objetivo de la [ejecución de residencia del 16-11-2026](R3_CELL_LOD_0A_15-11-2026.md#variante-de-residencia-16-11-2026) (distancia al punto más cercano, ADR 0034). Sobre objetivo dentro de Majadas: 6,8 / 8,2 / 9,7 / 14,8 % a 1/2/4/8 px; en la rasante de Belvedere, 4,6–5,1 %.
- **Belvedere:** `bound` da 1,07–1,17× la CPU en la rasante y la órbita 0° (frente a 1,00–1,03× de `edge`) y 0,016–0,084 % sobre el objetivo dentro de una hoja; `edge`, 0,003–0,006 %. Con `bound` la órbita 0° a 8 px da 891 instancias frente a 761.
- **Residente (r2, solo px), peor caso dentro de una hoja:** Majadas 408,6 / 268,6 / 173,7 / 114,0 MiB con `bound`, 347,5 / 227,6 / 148,4 / 99,0 con `edge` y 294,4 / 171,3 / 97,2 / 48,7 con `distance` (1/2/4/8 px); todos bajo 512 MiB a 1 px y 460 MiB a ≥ 2 px.
- **Pares no monótonos (hijo refina y padre no):** 0 en las tres métricas, en los 12 casos de cada dataset y métrica.
- Todos los nodos de los cortes se construyeron (`unbuilt_nodes` = 0). Tiempos de cada ejecución: 31–34 s en Belvedere y 72–81 s en Majadas, ≤ 2,2 GB residentes (ficheros `.time`).

## Inferencias

- *Inferido:* `edge` pasa porque la regla de parada y la medida de aceptación son la misma función: una celda dibujada con el nivel siguiente activo no supera el objetivo salvo que el descenso se detenga por profundidad máxima (las 4 celdas de Majadas a 1 px). No se ha aislado esa causa.
- *Inferido:* `bound` deja celdas sobre el objetivo a pesar de ser una cota porque la cota solo vale para `zmin ≥ near` (con `near` = `lod_min_depth`, que ignora el plano cercano de render) y el recorte `max(zmin, near)` no acota las celdas que cruzan o quedan por detrás del plano de la cámara; con la cámara dentro de una hoja a 8 px hay más de estas relativas al total. No se ha comprobado celda a celda.
- *Inferido:* el coste de `bound` en instancias (hasta 1,17× en Belvedere) viene del factor de la cota fuera de eje y de `dmax` (hasta ≈ 1,3×, ver E1).
- *Inferido:* que `edge` sea monótona en estos casos no prueba que lo sea siempre: la medida solo cuenta, para cada celda dibujada por encima de `S`, los hijos con puntos dentro del frustum. Una GPU que evalúe la arista proyectada sin la cota necesita proyectar 8 esquinas por celda y por ancestro (E1 y E4 lo piden por celda y por ancestro); su coste no se ha medido aquí.

## Resultados por caso

Corte con `select_lod_stable_projector` y `octant_tight` de la biblioteca como en la [fase 0a](R3_CELL_LOD_0A_15-11-2026.md#resultados-por-caso), presupuesto sin límite, residencia r2, solo «solo px». Las instancias cuentan celdas dentro del frustum. «NA» en CPU: la ruta CPU supera su límite de memoria estimada (256 MiB). «Sobre objetivo» usa la arista proyectada real. «Pares no monótonos»: celdas dibujadas con algún hijo (en el frustum, con puntos) que sí refinaría.

### Belvedere

| Pose | Objetivo (px) | Métrica | Instancias | Razón vs `octant_tight` | Razón vs CPU | Sobre objetivo (real) | % | Residente r2 (MiB) | Pares no monótonos |
| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| orbit-0 | 1 | `bound` | 50.410 | 0,68 | 1,08 | 0 | 0,000 | 1,4 | 0 |
| orbit-0 | 1 | `edge` | 47.825 | 0,65 | 1,03 | 0 | 0,000 | 0,8 | 0 |
| orbit-0 | 1 | `distance` | 47.825 | 0,65 | 1,03 | 0 | 0,000 | 0,8 | 0 |
| orbit-0 | 2 | `bound` | 12.371 | 0,65 | 1,07 | 0 | 0,000 | 0,8 | 0 |
| orbit-0 | 2 | `edge` | 11.549 | 0,60 | 1,00 | 0 | 0,000 | 0,2 | 0 |
| orbit-0 | 2 | `distance` | 11.549 | 0,60 | 1,00 | 0 | 0,000 | 0,2 | 0 |
| orbit-0 | 4 | `bound` | 3.232 | 0,67 | 1,11 | 0 | 0,000 | 0,2 | 0 |
| orbit-0 | 4 | `edge` | 2.921 | 0,61 | 1,00 | 0 | 0,000 | 0,1 | 0 |
| orbit-0 | 4 | `distance` | 2.921 | 0,61 | 1,00 | 0 | 0,000 | 0,1 | 0 |
| orbit-0 | 8 | `bound` | 891 | 0,33 | 1,17 | 0 | 0,000 | 0,1 | 0 |
| orbit-0 | 8 | `edge` | 761 | 0,28 | 1,00 | 0 | 0,000 | 0,0 | 0 |
| orbit-0 | 8 | `distance` | 761 | 0,28 | 1,00 | 0 | 0,000 | 0,0 | 0 |
| grazing | 1 | `bound` | 148.260 | 0,86 | 1,13 | 0 | 0,000 | 3,2 | 0 |
| grazing | 1 | `edge` | 132.240 | 0,77 | 1,01 | 0 | 0,000 | 3,2 | 0 |
| grazing | 1 | `distance` | 114.961 | 0,67 | 0,88 | 5.729 | 4,983 | 3,2 | 0 |
| grazing | 2 | `bound` | 37.016 | 0,86 | 1,13 | 0 | 0,000 | 0,8 | 0 |
| grazing | 2 | `edge` | 32.877 | 0,77 | 1,00 | 0 | 0,000 | 0,8 | 0 |
| grazing | 2 | `distance` | 28.591 | 0,67 | 0,87 | 1.400 | 4,897 | 0,8 | 0 |
| grazing | 4 | `bound` | 9.282 | 0,87 | 1,13 | 0 | 0,000 | 0,2 | 0 |
| grazing | 4 | `edge` | 8.208 | 0,77 | 1,00 | 0 | 0,000 | 0,2 | 0 |
| grazing | 4 | `distance` | 7.140 | 0,67 | 0,87 | 363 | 5,084 | 0,2 | 0 |
| grazing | 8 | `bound` | 2.425 | 0,90 | 1,15 | 0 | 0,000 | 0,1 | 0 |
| grazing | 8 | `edge` | 2.115 | 0,78 | 1,00 | 0 | 0,000 | 0,1 | 0 |
| grazing | 8 | `distance` | 1.850 | 0,69 | 0,88 | 86 | 4,649 | 0,1 | 0 |
| inside-leaf | 1 | `bound` | 7.166.585 | 1,00 | NA | 1.133 | 0,016 | 150,6 | 0 |
| inside-leaf | 1 | `edge` | 7.166.681 | 1,00 | NA | 417 | 0,006 | 148,2 | 0 |
| inside-leaf | 1 | `distance` | 7.101.473 | 0,99 | NA | 113.092 | 1,593 | 137,0 | 0 |
| inside-leaf | 2 | `bound` | 4.120.369 | 1,04 | NA | 911 | 0,022 | 118,9 | 0 |
| inside-leaf | 2 | `edge` | 3.825.479 | 0,96 | NA | 195 | 0,005 | 111,8 | 0 |
| inside-leaf | 2 | `distance` | 3.322.575 | 0,84 | NA | 258.750 | 7,788 | 73,2 | 0 |
| inside-leaf | 4 | `bound` | 1.600.575 | 1,03 | NA | 795 | 0,050 | 71,5 | 0 |
| inside-leaf | 4 | `edge` | 1.395.263 | 0,90 | NA | 79 | 0,006 | 61,4 | 0 |
| inside-leaf | 4 | `distance` | 1.049.411 | 0,68 | NA | 137.555 | 13,108 | 34,5 | 0 |
| inside-leaf | 8 | `bound` | 885.740 | 1,02 | 2,07 | 745 | 0,084 | 34,9 | 0 |
| inside-leaf | 8 | `edge` | 781.863 | 0,90 | 1,83 | 26 | 0,003 | 32,7 | 0 |
| inside-leaf | 8 | `distance` | 675.154 | 0,78 | 1,58 | 37.391 | 5,538 | 20,7 | 0 |

### Majadas

| Pose | Objetivo (px) | Métrica | Instancias | Razón vs `octant_tight` | Razón vs CPU | Sobre objetivo (real) | % | Residente r2 (MiB) | Pares no monótonos |
| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| orbit-0 | 1 | `bound` | 54.457 | 1,00 | 2,27 | 0 | 0,000 | 0,9 | 0 |
| orbit-0 | 1 | `edge` | 54.457 | 1,00 | 2,27 | 0 | 0,000 | 0,9 | 0 |
| orbit-0 | 1 | `distance` | 54.457 | 1,00 | 2,27 | 0 | 0,000 | 0,9 | 0 |
| orbit-0 | 2 | `bound` | 8.538 | 0,99 | 1,00 | 0 | 0,000 | 0,2 | 0 |
| orbit-0 | 2 | `edge` | 8.538 | 0,99 | 1,00 | 0 | 0,000 | 0,2 | 0 |
| orbit-0 | 2 | `distance` | 8.538 | 0,99 | 1,00 | 0 | 0,000 | 0,2 | 0 |
| orbit-0 | 4 | `bound` | 3.921 | 1,00 | 1,01 | 0 | 0,000 | 0,1 | 0 |
| orbit-0 | 4 | `edge` | 3.921 | 1,00 | 1,01 | 0 | 0,000 | 0,1 | 0 |
| orbit-0 | 4 | `distance` | 3.921 | 1,00 | 1,01 | 0 | 0,000 | 0,1 | 0 |
| orbit-0 | 8 | `bound` | 1.951 | 0,99 | 1,00 | 0 | 0,000 | 0,0 | 0 |
| orbit-0 | 8 | `edge` | 1.951 | 0,99 | 1,00 | 0 | 0,000 | 0,0 | 0 |
| orbit-0 | 8 | `distance` | 1.951 | 0,99 | 1,00 | 0 | 0,000 | 0,0 | 0 |
| grazing | 1 | `bound` | 54.457 | 1,00 | 2,27 | 0 | 0,000 | 0,9 | 0 |
| grazing | 1 | `edge` | 54.457 | 1,00 | 2,27 | 0 | 0,000 | 0,9 | 0 |
| grazing | 1 | `distance` | 54.457 | 1,00 | 2,27 | 0 | 0,000 | 0,9 | 0 |
| grazing | 2 | `bound` | 8.542 | 1,00 | 1,00 | 0 | 0,000 | 0,2 | 0 |
| grazing | 2 | `edge` | 8.538 | 0,99 | 1,00 | 0 | 0,000 | 0,2 | 0 |
| grazing | 2 | `distance` | 8.538 | 0,99 | 1,00 | 0 | 0,000 | 0,2 | 0 |
| grazing | 4 | `bound` | 3.873 | 1,00 | 1,00 | 0 | 0,000 | 0,2 | 0 |
| grazing | 4 | `edge` | 3.872 | 1,00 | 1,00 | 0 | 0,000 | 0,1 | 0 |
| grazing | 4 | `distance` | 3.872 | 1,00 | 1,00 | 0 | 0,000 | 0,1 | 0 |
| grazing | 8 | `bound` | 1.957 | 0,99 | 1,00 | 0 | 0,000 | 0,1 | 0 |
| grazing | 8 | `edge` | 1.951 | 0,99 | 1,00 | 0 | 0,000 | 0,0 | 0 |
| grazing | 8 | `distance` | 1.951 | 0,99 | 1,00 | 0 | 0,000 | 0,0 | 0 |
| inside-leaf | 1 | `bound` | 18.530.206 | 1,08 | NA | 27.982 | 0,151 | 408,6 | 0 |
| inside-leaf | 1 | `edge` | 16.857.549 | 0,98 | NA | 4 | 0,000 | 347,5 | 0 |
| inside-leaf | 1 | `distance` | 15.199.730 | 0,89 | NA | 1.039.851 | 6,841 | 294,4 | 0 |
| inside-leaf | 2 | `bound` | 10.141.684 | 1,07 | NA | 6.339 | 0,063 | 268,6 | 0 |
| inside-leaf | 2 | `edge` | 9.081.801 | 0,96 | NA | 0 | 0,000 | 227,6 | 0 |
| inside-leaf | 2 | `distance` | 7.813.037 | 0,82 | NA | 639.578 | 8,186 | 171,3 | 0 |
| inside-leaf | 4 | `bound` | 4.694.554 | 1,03 | NA | 1.063 | 0,023 | 173,7 | 0 |
| inside-leaf | 4 | `edge` | 4.049.924 | 0,89 | NA | 0 | 0,000 | 148,4 | 0 |
| inside-leaf | 4 | `distance` | 3.241.930 | 0,71 | NA | 314.318 | 9,695 | 97,2 | 0 |
| inside-leaf | 8 | `bound` | 1.843.890 | 0,99 | NA | 10.993 | 0,596 | 114,0 | 0 |
| inside-leaf | 8 | `edge` | 1.567.282 | 0,85 | NA | 0 | 0,000 | 99,0 | 0 |
| inside-leaf | 8 | `distance` | 1.091.977 | 0,59 | NA | 161.573 | 14,796 | 48,7 | 0 |

## Qué mide y qué no (método)

- **Reglas.** `distance`: `lado > max(lado_mín, px · max(dist_más_cercana, cerca) / focal)` (la regla de la fase 0a). `edge`: `lado > lado_mín` y `projected_voxel_edge_pixels(celda) > px`. `bound`: `cell_lod::refines` (E1, `edge_bound` con `LodView::from_camera`, `cerca` = `lod_min_depth`), con la celda convertida a coordenadas escaladas relativas al ojo en `f32` como `(esquina · escala − ojo)`.
- **Descenso** por nodo del corte con la regla elegida; el resto del modelo (nodos enmascarados, celdas bajo `S`, residencia r2, bytes por `pack_level`) es el de la fase 0a. El código de la rama `explore/cell-lod-spike` (commits `9e0ec17`, `5f8ba56`) se trajo sobre `main` sin conflictos.
- **Poses:** órbita 0°, rasante y dentro de una hoja (`--poses`); no se midió la órbita de 90° ni el zoom.
- **Pares no monótonos:** solo para celdas dibujadas por encima de la profundidad `S`; las celdas de un punto bajo `S` (cola) no se comprueban.
- **`octant_mismatches`:** el spike compara su `level_ok` con el corte de la biblioteca y termina con error si difieren: 0 pares en Belvedere y 215 de 13.312 en Majadas con este `main` (la fase 0a daba 0). Es una divergencia propia de `octant_tight`, ajena a la regla de refinado; la salida estándar está completa y por eso `command.sh` ignora el código de salida. Las columnas `octant_tight` son las de la biblioteca de `main`. *Inferido (controlador):* el oráculo del spike espera la siguiente profundidad **guardada** para las octantes que piden un nivel que abarca octantes, mientras que desde la fase 1 ([ADR 0048](../../adr/0048-el-render-dibuja-el-nivel-pedido.md)) la biblioteca pide cualquier profundidad **construible**; la divergencia es ese cambio de contrato, no una regresión (los 20 ejemplos no se revisaron uno a uno).
- Sin GPU: no hay tiempos, píxeles vacíos ni coste de evaluar cada regla en WGSL.

## Identidad y reproducción

[identity.txt](data/r3-fase2a-metrica-16-11-2026/identity.txt), [comando](data/r3-fase2a-metrica-16-11-2026/command.sh) y salidas por dataset y métrica en [data/r3-fase2a-metrica-16-11-2026/](data/r3-fase2a-metrica-16-11-2026/) (`<dataset>-<métrica>.tsv` y `.time`).
