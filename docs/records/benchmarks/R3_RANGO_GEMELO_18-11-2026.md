# R3: niveles que pide la regla de la GPU frente al rango por nodo — 18-11-2026

**Estado: medido.** En la muestra integrada, poses P1 a P5, el rango `[lo, hi]` que la CPU calcula hoy por nodo no
contiene lo que pide el gemelo de la regla de la GPU en los nodos junto al ojo: `lo` queda de 1 a 3 niveles
demasiado fino en 5 de 46 casos y `hi` de 3 a 6 niveles corto en 6. Una cota de fórmula cerrada contiene lo pedido
en los 46 casos, con 0 niveles de holgura en `lo` y de 0 a 1 en `hi`. El extremo fino que pide el gemelo junto al
ojo lo fija el recorte cercano del LOD en puntos sin datos: es una cuestión de presupuesto, no de fórmula.

Es la tarea 2 del [plan del ADR 0055 en modo celda](../../plans/adr-0055-rango-por-nodo.md). Medida nativa, sin
GPU, con tests ignorados que solo imprimen.

## Condiciones

| Dato | Valor |
| --- | --- |
| Revisión | `52e56e5` (en esta rama, el commit «measure the twin-wanted depth range per node») |
| Perfil y toolchain | `fast` · rustc 1.101.0-nightly (2026-10-27) |
| Dataset y modo | Muestra integrada de la demo; `LodMode::Cell`; objetivo 4 px; lienzo 870 × 750 px |
| Poses | P1 a P5 derivadas del encuadre inicial como en `p4_border_levels` (se supone un paso de rueda = `zoom(-0.3)` y arrastre hacia arriba) |
| Muestreo | Rayos de pantalla cada 6 px y puntos cada × 1,1 en profundidad de vista, dentro de nodo ∩ límites de datos ∩ vista; P4 repetida a 3 px con filas idénticas |
| Exclusiones | Puntos más cercanos que `lod_near_depth` (1,27 M en el nodo 14 en P2, P3 y P5; 0 en el resto) y celdas que cruzan el plano del ojo (0) |
| Comandos | `cargo test --offline --profile fast -p viewer_wasm --lib twin_wanted_range_vs_node_range -- --ignored --nocapture` (174 s) · `… p4_border_levels -- --ignored --nocapture` · `… the_node_range_contains -- --ignored` |

«Nivel pedido por el gemelo» en un punto: se desciende desde el nivel construible más grueso del nodo mientras
`refines` se cumple, como si todos los niveles fueran residentes.

## Resultado

| Pose | Nodos con muestras | `lo` más fino que lo pedido | `hi` más corto que lo pedido | Nodo con el ojo: exceso de `lo` · falta de `hi` | La cota candidata contiene lo pedido | Holgura de `hi` de la candidata |
| --- | --- | --- | --- | --- | --- | --- |
| P1 | 14 | 0 | 0 | sin nodo con el ojo | 14 de 14 | 0 en 6, 1 en 8 |
| P2 | 8 | 1 | 2 | 1 · 6 | 8 de 8 | 0 en 5, 1 en 3 |
| P3 | 8 | 2 | 2 | 3 · 3 | 8 de 8 | 0 en 3, 1 en 5 |
| P4 | 8 | 1 | 0 | 1 · −1 | 8 de 8 | 0 en 4, 1 en 4 |
| P5 | 8 | 1 | 2 | 3 · 3 | 8 de 8 | 0 en 1, 1 en 7 |

- La holgura de `lo` de la candidata es 0 en los 46 casos.
- Los casos con `lo` demasiado fino son el nodo 14, que contiene el ojo (P2 a P5), y el nodo 17, contiguo (P3).
- El test de propiedad `the_node_range_contains_what_the_twin_wants_within_one_level` falla hoy con 11 infracciones.

**Cota candidata** (evaluada en el test, sin conectar):

- `lo_c`: primer nivel que no refina para una celda **en el eje de la vista** a la profundidad de vista de la
  esquina más lejana de la región.
- `hi_c`: lo mismo a la profundidad más cercana (no menor que `lod_near_depth`), con el objetivo de px dividido por
  el estiramiento fuera del eje en la esquina del campo de visión, `sqrt(1 + tan²)` ≈ 1,18 aquí.
- Las dos, limitadas al tramo construible y a la regla del lado mínimo.

**Causa del `lo` de hoy en P4** (comprobada): `node_far_depth` sondea la esquina de la caja del nodo más lejana
según la vista, que cae 1174 px fuera de pantalla; allí la perspectiva plana estira la celda y la hija del nivel 10
mide 5,28 px, frente a 3,18 px en la parte visible del mismo borde
([registro del borde de P4](R3_FASE2B_BORDE_P4_18-11-2026.md)). En P2, P3 y P5 no se comprobó que la causa sea la misma.

**El extremo fino junto al ojo.** En P2, P3 y P5 el ojo está dentro de los límites de los datos. El máximo que
pide el gemelo en el nodo 14 es el nivel 18, y lo alcanza a profundidad de vista 0,072, es decir, en el recorte
cercano (`lod_near_depth` = 0,0708), en un punto del volumen sin datos. Hoy `range.hi` vale 12 en P2 y 15 en P3 y
P5, porque solo cuenta los octantes enteros delante del ojo, aunque el octante más exigente ya pide 18.

| Pose | Páginas estimadas del tramo pedido por el gemelo | Páginas de más por la holgura de la candidata |
| --- | --- | --- |
| P1 | 1408 | 576 |
| P2 | 1944 | 216 |
| P3 | 2088 | 360 |
| P4 | 1296 | 288 |
| P5 | 1944 | 504 |

Son cotas superiores con la fórmula del estimador del pool reescrita en el test, no su llamada. En P2, el tramo
hasta el nivel 18 del nodo 14 son 576 páginas estimadas frente a 72 del rango de hoy.

## Consecuencias

- **`lo` tiene arreglo con fórmula cerrada y sin coste**: la cota en el eje contiene lo pedido con holgura 0. Es el
  tramo A del plan.
- **`hi` no es un problema de fórmula sino de qué se quiere residir junto al ojo**: una cota fiel a la regla pide
  hasta el último nivel donde no hay puntos. Antes de cambiar `hi` hay que decidir con qué se limita (por ejemplo,
  la distancia a la celda ocupada más cercana en vez del recorte cercano) y medir su coste en páginas.

## Límites

- Solo la muestra integrada. Las simulaciones con datos reales (Belvedere) no se midieron: viven en otro arnés.
- El muestreo recorre volumen, no superficie: incluye aire.
- No se midió la candidata frente al gemelo desplazado por la tolerancia, ni la capacidad real del pool.
