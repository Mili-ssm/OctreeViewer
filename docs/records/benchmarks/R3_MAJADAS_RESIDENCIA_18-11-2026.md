# R3: bytes residentes del modo celda en Majadas — 18-11-2026

**Estado: medido; el tramo contiguo cabe en el pool.** En Majadas, con el modo celda y el pool de 512 MiB, los
niveles residentes no pasan de 361 MiB en ninguna de las dos poses de los registros anteriores, a 1, 2, 4 y 8 px,
con las dos reglas de niveles activos. Ningún nodo se recortó y no hubo desbordamiento. Responde a la medida que
pedía la [OQ-14](../../requirements.md#preguntas-abiertas)
([#69](https://github.com/Mili-ssm/OctreeViewer/issues/69)): con estas poses no hace falta subir el objetivo de px.

Medida nativa, sin GPU: el test ignorado `cells_bench` (`crates/viewer_wasm/src/lod_cells_bench_tests.rs`) ejecuta
el `CellDriver` y el `CellPool` reales sobre el fichero, con construcción de niveles a demanda.

## Condiciones

| Dato | Valor |
| --- | --- |
| Revisión | `ff61c0f` (opciones del banco) sobre `4a93d68`; en esta rama, `d78a934` |
| Perfil y toolchain | `perf` · rustc 1.101.0-nightly (2026-10-27) |
| Equipo | AMD Ryzen AI 9 365; pico de RSS 2,6 GB |
| Dataset | `target/render-data/majadas-v7.oct` (2 120 782 616 B) y su `.octattrs` |
| Modo | `LodMode::Cell`; pool de 65 536 páginas (512 MiB); lienzo de 1280 px de ancho |
| Poses | Las de `coverage_spike`: «dentro de una hoja» (ojo en la hoja con más puntos, mirando a +x) y «rasante» (5° sobre el plano horizontal por el centro de la raíz, a la distancia de encuadre) |
| Comando | `BENCH_DATASET=majadas BENCH_POSE={px,grazing} BENCH_PX={1,2,4,8} BENCH_MODE=cell BENCH_RULE={window,contiguous} BENCH_TIMEOUT_S=900 <binario de tests de viewer_wasm> --ignored --nocapture cells_bench` |

`BENCH_POSE=px` es la pose «dentro de una hoja».

## Resultado

Los bytes, las páginas y los nodos del corte son idénticos con `Window` y con `Contiguous` en las 16 ejecuciones.
Todas terminaron asentadas, sin huecos (`hole_frames` 0).

| Pose | px | Residente en reposo | Pico | Páginas | Libre del pool en el pico | Nodos del corte | Recortes · desbordamiento |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Dentro de una hoja | 1 | 356,6 MiB | 361,1 MiB | 45 639 | 29 % | 814 | 0 · 0 |
| Dentro de una hoja | 2 | 256,8 MiB | 256,8 MiB | 32 871 | 50 % | 450 | 0 · 0 |
| Dentro de una hoja | 4 | 132,3 MiB | 132,3 MiB | 16 937 | 74 % | 232 | 0 · 0 |
| Dentro de una hoja | 8 | 69,9 MiB | 69,9 MiB | 8 951 | 86 % | 131 | 0 · 0 |
| Rasante | 1 | 0,6 MiB | 0,6 MiB | 72 | > 99 % | 11 | 0 · 0 |
| Rasante | 2 | 0,5 MiB | 0,5 MiB | 60 | > 99 % | 4 | 0 · 0 |
| Rasante | 4 | 0,1 MiB | 0,1 MiB | 13 | > 99 % | 1 | 0 · 0 |
| Rasante | 8 | 0,1 MiB | 0,1 MiB | 7 | > 99 % | 1 | 0 · 0 |

«Recortes» son los picos de `pool_coarsened`, `gpu_coarsened` y `cpu_coarsened`; «desbordamiento», el de
`pool_overflow`.

- **La pose pesada es «dentro de una hoja»**, no la rasante. La rasante de los registros es una vista lejana: el ojo
  queda a unas 286 unidades de una nube de unas 100 de ancho y 57 de alto, y el corte tiene 11 nodos a 1 px. La
  cámara del banco y el corte coinciden con los de `coverage_spike` (11, 4, 1 y 1 nodos a 1, 2, 4 y 8 px).
- A 1 px dentro de una hoja la convergencia tardó 1051 actualizaciones (528 s con `Contiguous`, 614 s con `Window`);
  el pico solo supera al reposo en 4,5 MiB.
- El [registro de la fase 0a](R3_CELL_LOD_0A_15-11-2026.md) estimó 531 MiB para ese caso contando todos los niveles
  intermedios; el pool real retiene solo los que hacen falta.

## Consecuencias

- Para OQ-14: en Majadas el tramo contiguo cabe con un 29 % de margen en el peor caso medido. La respuesta del
  propietario («si no cabe, sube el objetivo de px de toda la nube») no llega a aplicarse con estas poses.
- El margen se reduce si el rango por nodo se ensancha: el [plan del ADR 0055](../archive/adr-0055-rango-por-nodo.md)
  debe repetir esta medida tras cambiar el extremo fino del rango.

## Límites

- Dos poses estáticas. Sin movimiento de cámara, sin una rasante a ras de suelo dentro de la nube y sin otras
  direcciones dentro de la hoja.
- No se midió la necesidad sin tope: el `fit` solo la calcula cuando hay presión, y no la hubo.
- No se midió el tiempo del paso de celdas: el banco mide el frame entero del arnés, con el oráculo y la
  construcción de nodos (mediana de 509 ms a 1 px dentro de una hoja), y no sirve como tiempo de producción.
- No se probó un pool menor de 512 MiB, así que el comportamiento del recorte sigue sin ejercitarse en Majadas.
