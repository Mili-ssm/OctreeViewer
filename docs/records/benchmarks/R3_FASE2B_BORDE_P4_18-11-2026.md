# R3 fase 2b: el borde recto de la pose P4 es de un nivel — 18-11-2026

**Estado: medido; no hace falta la cadena entre nodos.** En la pose P4 de la muestra integrada, las celdas
dibujadas a los dos lados de cada borde de nodo visible difieren como mucho en un nivel, con las dos reglas de
niveles activos (`ActiveRule::Window` y `Contiguous`). El borde se ve porque el rango del nodo que contiene el ojo
empieza un nivel más fino de lo que pide su parte visible: lo fija una esquina de su caja que queda fuera de
pantalla, donde la perspectiva plana estira las celdas.

Medida nativa, sin GPU: el gemelo en CPU de la regla de la GPU sobre la muestra integrada, en modo celda. Cierra
la casilla 2c de la [fase 2b](../archive/fase-2b-rango-de-niveles-y-transiciones.md) («P4 straight border
unmeasured»).

## Condiciones

| Dato | Valor |
| --- | --- |
| Revisión | `7f3dca9` (rama `docs/fase-2b-evidencia`; el test fija la regla con `Harness.rule`) |
| Perfil y toolchain | `fast` · rustc 1.101.0-nightly (2026-10-27) |
| Equipo | AMD Ryzen AI 9 365 (la medida no depende del tiempo) |
| Dataset | Muestra integrada de la demo; `data_bounds` (0, 0, −0,06)..(63,9, 63,9, 4,06) |
| Pose P4 | Encuadre inicial, 8 pasos de rueda y arrastre derecho de 120 px, derivada como la demo: `zoom(-0.3 × 8)` y `orbit(0, -120/750)`. El test la compara con la de la prueba de pose cercana (ojo ≈ (31,96, 38,76, 6,12) en el marco del dataset); la equivalencia no se comprobó aquí |
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
| 17 | 10–13 | 5, 8–13 (`Window`) · 5, 9–13 (`Contiguous`) | 10–13 |

## Causa

Comprobado:

- La regla de refinado deja cada celda entre el objetivo y su doble: las celdas de nivel 10 del borde miden entre
  6,4 y 7,9 px y sus hijas medirían entre 3,2 y 4,0 px, por debajo de los 4 px, así que no deben refinarse.
- El nodo 14 no puede dibujar el nivel 10: no está en su rango (11–13), aunque el nivel 10 cabe en el tramo
  construible (el nodo 8, de la misma profundidad, lo tiene). Su nivel activo más grueso es el 11 y dibuja celdas
  de 3,2 a 4,0 px, por debajo del objetivo.
- El extremo grueso del rango es `min(far, hi_wanted)`, con `far` calculado por `node_far_depth` en la esquina de
  la caja del nodo más lejana según la vista (`crates/viewer_render_wgpu/src/cell_cut.rs`, `node_far_depth` y el
  cálculo de `entry.range`). Para el nodo 14 esa esquina es (16, 32, −0,1).
- El salto de unos 7 px a unos 3,5 px coincide con la cara del nodo. Las celdas de 3,5 px son más finas que la
  separación de los puntos del suelo (unas 0,13 unidades, según el
  [plan de la fase 2b](../archive/fase-2b-rango-de-niveles-y-transiciones.md)) y se ven como puntos sueltos: por
  eso parece una línea.

Comprobado después con el diagnóstico `far_diagnosis` del mismo test (revisión `52e56e5`, rama
`feat/adr-0055-tramo-a`; salida en `p4_far.out`):

| Punto del borde (y = 32, suelo) | x en pantalla | Hija más pequeña de la celda de nivel 10 | ¿Refina el nivel 10? |
| --- | --- | --- | --- |
| Esquina lejana del nodo 14, x = 16 | −1174 px (fuera) | 5,28 px | Sí: `far` = 11 |
| x = 20 | −774 px (fuera) | 4,22 px | Sí |
| x = 24, centro del borde | −370 px (fuera) | 3,24 px | No: pide nivel 10 |
| x = 28 a 32, parte visible | 35 a 439 px | 3,18 px | No: pide nivel 10 |

- La esquina que usa `node_far_depth` está a la misma profundidad de vista que el resto del borde (14,0), pero
  1174 px a la izquierda del lienzo. Con proyección plana una celda mide más cuanto más lejos está del eje de la
  vista: allí la hija de la celda de nivel 10 mide 5,28 px y en la parte visible 3,18 px.
- La sonda y el gemelo de la GPU dan el mismo valor en la esquina: no es un error de la sonda, sino que «la esquina
  más lejana según la vista» no es donde las celdas miden menos.

## Consecuencias

- **La cadena entre nodos (antiguo F4) no hace falta para P4**: no hay pares visibles con más de un nivel.
- El defecto está en cómo se calcula el extremo grueso del rango de un nodo: se evalúa en un punto fuera de
  pantalla. Corresponde al paso 1 de «Fuera de
  esta sesión» del [replanteamiento](../archive/replanteamiento-lod-por-celda-18-11-2026.md#tareas) (rango por
  nodo, [ADR 0055](../../adr/0055-la-gpu-decide-el-nivel-la-cpu-la-residencia.md)), que sustituye ese cálculo; P4
  es su caso de prueba.
- El nodo 14 dibuja por debajo del objetivo de px: es un caso de la casilla abierta de la tarea 1 de la fase 2b
  («ninguna celda más fina de lo que pide su celda madre»).

## Límites

- La cámara nativa salta del encuadre inicial a P4 sin los fotogramas intermedios de la rueda; otra historia de
  construcción podría retener otros nodos.
- Las aristas en px salen del gemelo en CPU; en la captura del navegador los cubos miden lo mismo a ojo (unos 7 px
  encima del borde, 3–4 px debajo), pero no se leyeron los niveles de la GPU.
- Solo la muestra integrada y solo P4.
