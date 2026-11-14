# D3a: el orden de los puntos ralentizaba la construcción de celdas — 14-11-2026

**Estado: causa encontrada y corregida (integrada en `feat/r3-v75`).** Los ficheros con los bloques en
orden Morton construían las celdas un 35–50 % más rápido
([OQ-09](OQ09_MORTON_ORDER_14-11-2026.md)) porque el constructor ordena los puntos por celda con
pdqsort (`sort_unstable_by_key`), que es ~8 veces más rápido con claves casi ordenadas que con claves
desordenadas. Un ordenamiento radix estable lo vuelve independiente del orden de los puntos. En el
navegador, construir las celdas de Belvedere en remoto cuesta un 25–45 % menos con cualquier fichero.

## Causa

`build_level_keyed` (`crates/viewer_render_wgpu/src/node_cells.rs`) calcula para cada punto la clave
Morton de su celda y ordena las entradas por (clave, bloque, índice). Tiempo por fase con
instrumentación temporal, sumado en los 562 nodos del benchmark nativo (pose `py`, 1 px):

| Fase | Orden de la fuente | Todo Morton |
| --- | --- | --- |
| Recoger puntos y claves | ~4,1 s | ~4,1 s |
| **Ordenar** | **~3,0–3,3 s** | **~0,35–0,40 s** |
| Agregar celdas | ~0,93–1,03 s | ~0,94–1,03 s |

Ordenar por Morton en el visor al guardar cada bloque (experimento de [OQ-09](OQ09_MORTON_ORDER_14-11-2026.md))
no ayudaba porque usaba otra clave: la entrada no llegaba ordenada para el constructor.

## Cambio

- Para claves de 64 bits, el constructor ordena pares (clave, posición) con un radix LSD de 11 bits y
  recorre las posiciones en ese orden, sin copiar las entradas. Es estable: los empates conservan el
  orden (bloque, índice), así que las celdas son las mismas que antes.
- Si las claves ya llegan ordenadas, no ordena.
- Las claves de 128 bits (nodos que exceden 2²¹ celdas por eje) y las listas de más de 2³² entradas
  siguen con el ordenamiento anterior.
- Tests: el radix frente a un ordenamiento estable en casos aleatorios y extremos, y las mismas celdas
  con puntos barajados, invertidos u ordenados en Morton (una mutación del atajo «ya ordenado» lo hace
  fallar). `scripts/check.sh` pasa los 24 pasos.

## Medidas

**Navegador** (`probe-orbit-load.mjs --remote --rest-first --cells=1`, RED-REF 100 Mbit/s y 50 ms de
RTT, Belvedere v7.5; antes = wasm de `3775a6f`, después = `2379c6d`; 2 rondas alternas):

| Fichero y px | Objetivo antes → después (ms) | Construcción antes → después (ms) | Peor paso antes → después (ms) |
| --- | --- | --- | --- |
| Fuente, 4 px | 1.784–1.825 → 1.328–1.588 | 1.357–1.425 → 959–1.055 | 180–181 → 107–113 |
| Híbrido, 4 px | 1.599–1.848 → 1.343–1.593 | 1.281–1.409 → 768–1.034 | 150–195 → 93–115 |
| Fuente, 1 px | 3.895–4.880 → 3.613–3.857 | 5.686–7.424 → 4.215–4.928 | 180–201 → 128–131 |
| Híbrido, 1 px | 4.129–5.432 → 3.871–3.882 | 5.943–7.779 → 4.463–5.243 | 177–288 → 131–137 |

Los bytes descargados no cambian. No hay nodos sin cubrir y el único error de consola es el 404 de
`favicon.ico`.

**Benchmark nativo** (`cells_bench`, pose `py`, 1 px, p95 de construcción, 2 rondas intercaladas con la
versión `9234996`): fuente 31,5–39,7 ms, híbrido 37,2–37,6 ms y todo Morton 32,3–33,0 ms, frente a
41–57 ms de la fuente antes del cambio. El p95 varía hasta ~12 ms entre ejecuciones de una misma
variante.

## Alternativa descartada

Usar pdqsort cuando las claves tienen pocos descensos (menos de uno cada 32 puntos) y el radix en otro
caso. Mejora el fichero todo Morton (p95 28,7–31,7 ms), pero con el de la fuente y el híbrido queda
dentro del ruido (36,4–43,9 y 36,4–37,1 ms). El escritor elegido es el híbrido
([ADR 0041](../../adr/0041-orden-de-filas-libre-por-bloque.md)), así que no compensa la regla extra.

## Límites

- Un dataset (Belvedere), un equipo y una red simulada; 2 rondas por caso en el navegador.
- Las cifras por fase vienen de una instrumentación temporal (rama `explore/oq09-build-cause`,
  `b649350`) y no tienen fichero de datos.
- El benchmark nativo de la versión final (`2379c6d`, que solo quita la copia) no se repitió; el
  navegador sí la mide.

## Identidad y reproducción

- Código: `explore/cells-radix-sort` (`9234996`, `2379c6d`), integrada en `feat/r3-v75`. Entorno: el de
  [T11](data/r3-t11-smoke-13-11-2026/identity.txt).
- `BENCH_DATASET=oq09src BENCH_POSE=py BENCH_PX=1 cargo test --profile perf -p viewer_wasm --offline --lib cells_bench -- --ignored --nocapture`
- Datos en [data/d3a-cell-sort-14-11-2026](data/d3a-cell-sort-14-11-2026/): `browser/`
  (`antes|después-FICHERO-PX-nRONDA.json`, con los nombres `before|after`) y `bench/`
  (`radix|adaptive-FICHERO-nRONDA.json`).
