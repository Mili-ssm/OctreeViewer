# R3: el extremo fino del rango limitado por datos — 18-11-2026

**Estado: medido; la cota es válida y ajustada, su coste real en el pool no queda medido.** Calcular el extremo
fino `hi` del rango de un nodo a la distancia de su celda ocupada más cercana (`hi_d`), en vez de en el recorte
cercano del LOD, contiene lo que la regla de la GPU puede pedir donde hay puntos en 44 de 44 casos, con 0 o 1
niveles de holgura. Las páginas *estimadas* suben respecto al rango de hoy, pero el estimador no describe lo que
el pool retiene: hoy el pool ya guarda junto al ojo niveles más finos que `hi_d`. El coste real solo se sabrá
implementándolo y midiendo la residencia.

Es la medida que pedía la respuesta del propietario a la [OQ-16](../../requirements.md#preguntas-abiertas)
([#80](https://github.com/Mili-ssm/OctreeViewer/issues/80)). Medida nativa, sin GPU, con tests ignorados que solo
imprimen; no cambia la lógica del visor.

## Condiciones

| Dato | Valor |
| --- | --- |
| Revisión | `1d0c1dd` (rama `refactor/retire-octant`) |
| Perfil y toolchain | Muestra: `fast`. Majadas: `perf`. rustc 1.101.0-nightly (2026-10-27) |
| Equipo | AMD Ryzen AI 9 365 |
| Datasets y modo | Muestra integrada de la demo, poses P1 a P5, objetivo 4 px; `target/render-data/majadas-v7.oct`, pose «dentro de una hoja», 1 y 4 px. `LodMode::Cell` |
| Comandos | `TWIN_POSES=… cargo test --offline --profile fast -p viewer_wasm --lib data_bounded_hi -- --ignored --nocapture` · `BENCH_DATASET=majadas BENCH_POSE=px BENCH_MODE=cell BENCH_PX={1,4} BENCH_HI_D=1 <binario perf de tests de viewer_wasm> --ignored --nocapture cells_bench` |
| Comprobación | Las poses P2 y P4 se repitieron tras recompilar todo el espacio de trabajo: salida idéntica |

Definiciones:

- **Celda ocupada**: una celda del nivel residente más fino del nodo en reposo, dentro de la vista.
- **`hi_d`**: el `hi` de la cota candidata (`node_view_range`) con la profundidad de vista más cercana sustituida
  por la de la celda ocupada más cercana (nunca menor que `lod_near_depth`).
- **`hi_c`**: el `hi` de la cota candidata tal cual, en el recorte cercano
  ([registro anterior](R3_RANGO_GEMELO_18-11-2026.md)).
- **Máximo pedido con datos**: el nivel más fino que pide el gemelo de la regla de la GPU en las celdas ocupadas
  (en su centro y en su punto más cercano al ojo).

## Resultado en la muestra

- **Cota válida**: `hi_d` nunca queda por debajo del máximo pedido con datos (0 infracciones en 44 casos).
- **Ajustada**: la holgura es 0 en 35 casos y 1 nivel en 9.
- **Más baja que `hi_c`**: igual en 19 casos, 1 nivel menos en 17, 2 en 2, 4 en 4 y 5 en 2; las diferencias de 4 y
  5 niveles son los nodos junto al ojo.

Nodo 14, que contiene el ojo (el nodo 17, contiguo, se comporta igual):

| Pose | `hi` de hoy | Deseo máximo por octante | Máximo pedido con datos | `hi_d` | `hi_c` | Páginas estimadas: hoy · `hi_d` · `hi_c` | Páginas residentes hoy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| P2 | 11 | 18 | 12 | 13 | 18 | 72 · 216 · 576 | 701 |
| P3 | 12 | 18 | 14 | 14 | 18 | 72 · 216 · 504 | 701 |
| P4 | 13 | 13 | 11 | 11 | 13 | 288 · 144 · 288 | 341 |
| P5 | 12 | 18 | 13 | 14 | 18 | 72 · 216 · 504 | 701 |

Totales estimados por pose (nodos con rango y candidata) y páginas residentes de esos nodos:

| Pose | Hoy | `hi_d` | `hi_c` | Residentes hoy |
| --- | --- | --- | --- | --- |
| P1 | 1696 | 1408 | 1984 | 555 |
| P2 | 1152 | 1368 | 2160 | 1966 |
| P3 | 1440 | 1656 | 2448 | 2254 |
| P4 | 1296 | 1008 | 1296 | 1145 |
| P5 | 1296 | 1512 | 2448 | 2110 |

**El estimador no describe la residencia.** En P2, P3 y P5 el nodo 14 tiene residente el nivel 18 (701 páginas)
aunque su `hi` es 11 o 12: las peticiones las mueve el deseo máximo por octante (18), no `range.hi`. Las celdas
ocupadas más cercanas al ojo están a una profundidad de vista de 1,0 a 2,8, frente a un recorte cercano de 0,07.

## Resultado en Majadas, dentro de una hoja

| Dato | 4 px | 1 px |
| --- | --- | --- |
| Páginas residentes medidas en reposo | 16 214 | 45 806 |
| De ellas, en nodos del corte | — | 19 595 |
| Nodos del corte · con rango, candidata y celdas ocupadas a la vista | 232 · 89 | 814 · 404 |
| Estimación de esos nodos con el `hi` de hoy | 14 185 | 30 373 |
| Estimación con `hi_d` | 15 861 | 41 556 |
| Estimación con `hi_c` | 18 739 | 47 638 |

El pool tiene 65 536 páginas, de las que el ajuste usa 57 344 (`usable_pages`: la capacidad menos un octavo).
Sumar a lo medido la diferencia entre estimaciones daría unas 56 989 páginas con `hi_d` a 1 px (99,4 % de lo
usable) y unas 63 071 con `hi_c`. **Esa proyección no es fiable**: a 1 px la estimación con el `hi` de hoy (30 373)
ya supera en más de la mitad lo que los nodos del corte retienen de verdad (19 595).

## Consecuencias

- Limitar `hi` por datos es correcto y barato de justificar: contiene lo que la regla puede pedir y queda 4 o 5
  niveles por debajo de la cota fiel junto al ojo.
- Antes de darlo por bueno hay que medir la residencia real con `hi_d` conectado, en Majadas a 1 px, donde el
  margen de hoy es del 20 % de lo usable (45 806 de 57 344).
- Para que `hi` mande de verdad sobre lo que se carga, las peticiones tienen que salir del rango y no del deseo
  por octante: es el tramo B del [plan del ADR 0055](../../plans/adr-0055-rango-por-nodo.md).

## Límites

- La medida recorre todas las celdas del nivel residente más fino: es inviable en producción (unos 23 millones de
  celdas a 1 px). Habría que usar las cajas de página o las cajas de datos por octante; no se midió cuánto aflojan
  la cota.
- `hi_d` depende de qué nivel es residente: con uno más grueso la cota sube (sigue siendo válida) y baja al llegar
  niveles finos. No se probó si eso produce oscilaciones o bucles de peticiones.
- El máximo pedido se muestreó en el centro y en el punto más cercano de cada celda, no en todos sus puntos.
- En Majadas solo entran los nodos del corte con rango, candidata y celdas ocupadas a la vista.
