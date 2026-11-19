# R3: fuentes baratas para limitar el extremo fino por datos — 18-11-2026

**Estado: medido; se elige una fuente, con un límite conocido.** Para calcular el extremo fino `hi` del rango de
un nodo a la distancia de su celda ocupada más cercana sin recorrer celdas, la única fuente útil de las cuatro
probadas son **las cajas de página del nivel residente más fino, recortadas a la vista**: es válida en los 44
casos, coincide con la cota celda a celda en el nodo que contiene el ojo y no cambia al quitar niveles finos. En el
nodo contiguo al ojo queda 3 o 4 niveles por encima. Sin el recorte a la vista, todas las fuentes vuelven al
último nivel.

Es la tarea 5a del [plan del ADR 0055 en modo celda](../archive/adr-0055-rango-por-nodo.md). Medida nativa, sin
GPU, con tests ignorados que solo imprimen; no cambia la lógica del visor. Continúa el registro de la
[cota por datos](R3_EXTREMO_FINO_POR_DATOS_18-11-2026.md), cuya cota celda a celda es aquí la referencia.

## Condiciones

| Dato | Valor |
| --- | --- |
| Revisión | `6416ad5` (rama `feat/adr-0055-tramo-b`) |
| Perfil y toolchain | Muestra: `fast`. Majadas: `perf`. rustc 1.101.0-nightly (2026-10-27) |
| Equipo | AMD Ryzen AI 9 365 |
| Datasets | Muestra integrada de la demo, poses P1 a P5, objetivo 4 px (44 casos nodo-pose; 10 junto al ojo, nodos 14 y 17); `target/render-data/majadas-v7.oct`, dentro de una hoja, 1 px (814 nodos del corte, 404 con rango, candidata y datos) |
| Comandos | `TWIN_POSES=… cargo test --offline --profile fast -p viewer_wasm --lib data_bounded_hi -- --ignored --nocapture` · `BENCH_DATASET=majadas BENCH_POSE=px BENCH_PX=1 BENCH_HI_D=1 <binario perf de tests de viewer_wasm> --ignored --nocapture cells_bench` (630 s) |

Fuentes de la celda ocupada más cercana, cada una sin recortar y recortada a la vista:

| Fuente | Qué usa |
| --- | --- |
| Referencia | Las celdas del nivel residente más fino cuyo centro está en pantalla (cota celda a celda) |
| a | Cajas de página del nivel residente más fino |
| b | Cajas de página del nivel de la cota de referencia |
| c | Cajas de página del nivel residente más grueso del nodo |
| d | Cajas de datos por octante del nodo (`octant_data_bounds`) |

El recorte es exacto: cada caja se corta con los cuatro planos laterales de la vista y el plano cercano, y se toma
la profundidad mínima de los vértices del resultado. Su error solo puede dar una profundidad menor, es decir, una
cota más alta.

## Resultado en la muestra

- **Válidas todas**: ninguna fuente queda por debajo de lo que pide la regla de la GPU donde hay puntos (0
  infracciones en 44 casos, en las nueve columnas).
- **Sin recorte no sirven**: llegan a la cota fiel a la regla (`hi_c`) en 39 de 44 casos las fuentes a y b, y en 44
  de 44 las c y d. La referencia llega en 19.
- La fuente b da lo mismo que la a en todos los casos.

`hi` junto al ojo (`hi_c` vale 18 en P2, P3 y P5):

| Pose y nodo | Referencia | a recortada | a sin recortar | c recortada | d recortada |
| --- | --- | --- | --- | --- | --- |
| P2, nodo 14 (contiene el ojo) | 13 | 13 | 14 | 18 | 18 |
| P3, nodo 14 | 14 | 14 | 16 | 18 | 18 |
| P4, nodo 14 | 11 | 11 | 12 | 12 | 12 |
| P5, nodo 14 | 14 | 14 | 15 | 18 | 18 |
| P2, nodo 17 (contiguo) | 13 | 17 | 18 | 17 | 17 |
| P3, nodo 17 | 14 | 17 | 18 | 17 | 17 |
| P5, nodo 17 | 14 | 17 | 18 | 17 | 17 |
| P4, nodo 17 | 11 | 12 | 12 | — | — |

Diferencia con la referencia en los 44 casos:

| Fuente | Igual | +1 | +2 | +3 | +4 | +5 |
| --- | --- | --- | --- | --- | --- | --- |
| a recortada | 38 | 3 | 0 | 2 | 1 | 0 |
| a sin recortar | 19 | 21 | 1 | 0 | 2 | 1 |
| c recortada | 33 | 4 | 0 | 2 | 3 | 2 |
| d recortada | 33 | 5 | 0 | 2 | 3 | 1 |

**El nodo 17.** En P2 la fuente a recortada da una profundidad de vista de 0,135, frente a 2,153 de la referencia.
La página que la produce tiene 512 celdas y una caja de 6,13 × 6,13 × 2,12 unidades. Interpretación, sin medir:
la caja envuelve celdas consecutivas en orden Morton y es mucho mayor que lo que ocupan.

**Estabilidad** (fuentes a y b, nodos 14 y 17, P1 a P5): quitando los niveles más finos que la cota, o dejando
solo los que no pasan del `hi` de hoy, la cota no cambia; iterando «cota → niveles hasta la cota → cota» queda
constante desde el primer paso. La sonda solo puede quitar niveles: no prueba un nodo que solo tenga niveles gruesos.

## Resultado en Majadas, dentro de una hoja, 1 px

| Columna | Páginas estimadas |
| --- | --- |
| `hi` de hoy | 30 373 |
| Referencia | 41 556 |
| a, b y c recortadas | 41 592 |
| d recortada | 41 597 |
| a y b sin recortar | 47 404 |
| c sin recortar | 47 501 |
| `hi_c` y d sin recortar | 47 638 |

Las fuentes recortadas añaden 36 o 41 páginas a la referencia; las no recortadas, unas 5 900. El pool retiene hoy
45 806 páginas, 19 595 en nodos del corte, de 57 344 usables. Como en el registro anterior, estas estimaciones no
describen la residencia real y no sirven para decidir si cabe.

## Consecuencias

- **Fuente elegida para la tarea 5**: cajas de página del nivel residente más fino, recortadas a la vista.
- El recorte a la vista es obligatorio: `nearest_visible_depth` no lo hace hoy.
- La holgura del nodo contiguo al ojo cuesta, en la muestra, de 72 a 288 páginas estimadas sobre la referencia por
  pose (P2: 1656 frente a 1368; P3: 1872 frente a 1656; P4: 1080 frente a 1008; P5: 1728 frente a 1512). Un envolvente más ajustado por página (por ejemplo, varias cajas por página) la reduciría; no se midió.
- Si el margen en Majadas resiste solo lo dirá la residencia medida con la cota conectada (tarea 7a del plan).

## Límites

- Una muestra con cinco poses y una pose de Majadas.
- La referencia conserva el filtro «centro en pantalla»; las fuentes no lo usan.
- El binario de Majadas se compiló antes de añadir una línea de diagnóstico; el cálculo de `hi` es el mismo.
