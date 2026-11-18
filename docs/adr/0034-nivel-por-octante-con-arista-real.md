# 0034 Nivel por octante con la arista proyectada real

Estado: Aceptada (diseño; se confirma en navegador en la Tarea 11 del [plan de R3](../records/archive/render-r3-cubos-en-gpu.md)); la regla «nivel por octante» la sustituye el [ADR 0050](0050-nivel-por-celda-segun-distancia.md) · Fecha: 05-11-2026 · Decide: propietario · Enmienda a [ADR 0031](0031-celdas-por-nodo-en-v7-5.md)

## Contexto
- 0031 elige, por nodo, el nivel guardado más grueso cuyo error proyectado cumple el objetivo de px. Ese error es `projected_error_pixels` (`camera.rs:414`), una cota superior: usa la esquina más cercana del nodo para todas sus celdas y un factor fuera de eje de ≈ 1,31.
- En el [spike de cobertura](../records/benchmarks/R3_T4_COVERAGE_05-11-2026.md), esa regla dibuja 2–4,6× las instancias de la ruta CPU en Majadas y supera el pool de 256 MiB desde dentro de una hoja a 1 px (269,4 MiB).
- Con las [alternativas](../records/benchmarks/R3_T4_METRIC_05-11-2026.md), la arista real de una celda junto con un nivel por octante da en Majadas, en la pose rasante, 1,00–1,01× la CPU a 2–8 px y 2,27× a 1 px, y 232,3 MiB desde dentro de una hoja. Fuera de los nodos, ninguna celda supera el objetivo.
- Con la cámara dentro del dataset, esa métrica deja entre un 0,01 % y un 0,26 % de las celdas en pantalla por encima del objetivo, sin contar la densidad de la fuente. Están en octantes lejos del ojo; *inferido*: la perspectiva las estira hacia los bordes de la pantalla. Se midió una sola pose interior por dataset. Usar la cota con el ojo dentro del octante no cambia nada, y aplicar el factor fuera de eje a todo el corte devuelve Majadas a 4,59× y a 268,3 MiB ([registro](../records/benchmarks/R3_T4_OFFAXIS_05-11-2026.md)).

## Decisión
- **Nivel por octante:** cada uno de los 8 octantes de un nodo elige su propio nivel guardado: el más grueso cuya celda, colocada en el punto del octante más cercano al ojo, tiene una arista proyectada real (`projected_voxel_edge_pixels`) que cumple el objetivo. Si ninguno lo cumple, el más fino (R3-Q9).
- **RQ-09 con la cámara dentro del dataset:** se admite hasta un 0,5 % de celdas en pantalla por encima del objetivo, sin contar la densidad de la fuente. Fuera del dataset no se admite ninguna.
- **Criterio de T4:** como mucho 2× las instancias de la CPU en la pose rasante a 2, 4 y 8 px, y 2,5× a 1 px.
- Residencia, etapas y máscara siguen como en 0031. Un nodo puede tener residentes a la vez los niveles que piden sus octantes: la clave del pool ya incluye la profundidad.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Cota por nodo (0031) | 2–4,6× instancias y más de 256 MiB en Majadas |
| Arista real por nodo, sin octantes | 2,2× a 4 px y 2,27× a 1 px en Majadas |
| Octantes con la cota | Apenas cambia: la cota sigue dominando dentro de cada octante |
| Un nivel por celda de la máscara (512 por nodo) | Cerraría el 2,27× a 1 px, a costa de más complejidad en el corte; sin medir |
| Cota con el ojo dentro del octante | No elimina ninguna celda por encima del objetivo |
| Factor fuera de eje en todo el corte | Casi cumple RQ-09 (0–29 celdas por encima del objetivo), pero falla los criterios de T4 en Majadas |

## Consecuencias
- La Tarea 15 deja de ser condicional y se hace con `cell_cut`. `CellCutEntry` pasa a llevar un nivel por octante, y cambian los dorados de `cell_cut_digest`.
- La residencia todo o nada se mantiene por (nodo, nivel). Un nodo dibuja varios niveles cuando sus octantes los piden.

## Revisar si
- La Tarea 11 muestra en el navegador costuras visibles entre octantes de niveles distintos, o celdas por encima del objetivo que se notan.
- Algún dataset pasa del 0,5 % de celdas por encima del objetivo con la cámara dentro.
- Un dataset nuevo pasa de 2,5× a 1 px o de 2× a 2–8 px en la pose rasante.
