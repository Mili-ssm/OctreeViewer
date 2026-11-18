# R3 fase 2b: el borde recto de la pose P4 es de un nivel — 18-11-2026

**Estado: medido; no hace falta la cadena entre nodos.** En la pose P4 de la muestra integrada, las celdas
dibujadas a los dos lados de cada borde de nodo visible difieren como mucho en un nivel, con las dos reglas de
niveles activos (`ActiveRule::Window` y `Contiguous`). El borde se ve por otra causa: el extremo grueso del rango
del nodo que contiene el ojo lo fija una esquina de su caja que queda fuera de pantalla.

Medida nativa, sin GPU: el gemelo en CPU de la regla de la GPU sobre la muestra integrada, en modo celda. Cierra
la casilla 2c de la [fase 2b](../../plans/fase-2b-rango-de-niveles-y-transiciones.md) («P4 straight border
unmeasured»).

## Condiciones

| Dato | Valor |
| --- | --- |
| Revisión | `7f3dca9` (rama `docs/fase-2b-evidencia`; el test parte de la opción `cellActive`, PR #74) |
| Perfil y toolchain | `fast` · rustc 1.101.0-nightly (2026-10-27) |
| Equipo | AMD Ryzen AI 9 365 (la medida no depende del tiempo) |
| Dataset | Muestra integrada de la demo; `data_bounds` (0, 0, −0,06)..(63,9, 63,9, 4,06) |
| Pose P4 | Encuadre inicial, 8 pasos de rueda en (400, 500) y arrastre derecho de 120 px; derivada como la demo (encuadre, `zoom`, `orbit`), equivale a la de la prueba de pose cercana (ojo ≈ (31,96, 38,76, 6,12)) |
| Lienzo | 870 × 750 px (el que informa la demo con la ventana de 1280 × 800); objetivo 4 px |
| Comando | `cargo test --offline --profile fast -p viewer_wasm p4_border_levels -- --ignored --nocapture` |

El test `p4_border_levels` (`crates/viewer_wasm/src/lod_cells_stream_tests.rs`) solo imprime; `P4_DUMP=<dir>`
vuelca cada par. Muestrea las caras entre nodos dentro de `data_bounds` y cuenta cada par en los dos sentidos.

## Resultado

| Diferencia de nivel entre las dos celdas del par | Pares visibles |
| --- | --- |
| 0 | 3028 |
| 1 | 330 |
| 2 o más | 0 |

El histograma es idéntico con `Window` y con `Contiguous`. Los pares con diferencia 1 están en dos bordes:

| Borde | Niveles dibujados | Pares | Filas de pantalla | Arista proyectada de las celdas |
| --- | --- | --- | --- | --- |
| Nodo 8 (lejano) con nodo 14 (contiene el ojo) | 10 frente a 11 | 128 | 222–542 | 6,4–7,9 px (media 7,1) frente a 3,2–4,0 px (media 3,5) |
| Nodo 14 con nodo 17 | 11 frente a 10 | 37 | 269–695 (costura vertical en x ≈ 440) | media 3,7 px frente a 7,4 px |

Las aristas salen del volcado `P4_DUMP` (columnas `pxA` y `pxB` de los pares con diferencia 1).

| Nodo | Activos | Residentes | Rango `[lo, hi]` |
| --- | --- | --- | --- |
| 8 | 9, 10, 11 | 5, 9, 10, 11 | 9–11 |
| 14 | 11, 12, 13 | 5, 7–13 | 11–13 |
| 17 | 10–13 | 5, 9–13 | 10–13 |

## Causa

- La regla de refinado deja cada celda entre el objetivo y su doble: las celdas de nivel 10 del borde miden entre
  6,4 y 7,9 px y sus hijas medirían entre 3,2 y 4,0 px, por debajo de los 4 px, así que no deben refinarse.
- El nodo 14 no puede dibujar el nivel 10: no está en su rango. Su nivel activo más grueso es `lo` = 11, y dibuja
  celdas de 3,2 a 4,0 px, por debajo del objetivo.
- `lo` sale de `node_far_depth` (`crates/viewer_render_wgpu/src/cell_cut.rs`): toma la esquina de la caja del nodo
  más lejana según la vista, aquí (16, 32, −0,1), que queda fuera de pantalla a la izquierda, donde las celdas
  proyectan más grande y piden nivel 11. El suelo visible junto al borde pide nivel 10.
- El salto de unos 7 px a unos 3,5 px coincide con la cara del nodo. Además, las celdas de 3,5 px son más finas que la
  separación de los puntos del suelo (unas 0,13 unidades) y se ven como puntos sueltos: por eso parece una línea.

## Consecuencias

- **La cadena entre nodos (antiguo F4) no hace falta para P4**: no hay pares visibles con más de un nivel.
- El defecto es que el rango de un nodo se calcula con partes de su caja que no se ven. Corresponde al paso 1 del
  [ADR 0055](../../adr/0055-la-gpu-decide-el-nivel-la-cpu-la-residencia.md) (rango por nodo): restringir la esquina
  lejana a la parte del nodo que cae en pantalla y dentro de los datos.
- El nodo 14 dibuja por debajo del objetivo de px: es un caso de la casilla abierta de la tarea 1 de la fase 2b
  («ninguna celda más fina de lo que pide su celda madre»).

## Límites

- La cámara nativa salta del encuadre inicial a P4 sin los fotogramas intermedios de la rueda; otra historia de
  construcción podría retener otros nodos.
- Las aristas en px salen del gemelo en CPU; en la captura del navegador los cubos miden lo mismo a ojo (unos 7 px
  encima del borde, 3–4 px debajo), pero no se leyeron los niveles de la GPU.
- Solo la muestra integrada y solo P4.
