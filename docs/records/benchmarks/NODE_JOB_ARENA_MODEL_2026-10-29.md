# Modelo de pico de los trabajos por nodo (S1b, 29-10-2026)

Registro fechado e inmutable (etapa 2 del conversor web paralelo, Tarea 1). Mide si el pico de spool vivo del modelo «un trabajo por nodo» (cada trabajo lee el spool de entrada de un nodo y escribe todos sus hijos en una arena nueva; la arena del padre se borra cuando el último hijo la ha consumido) queda dentro de 1,5 × el `peak_live_spool_bytes` nativo. Datos crudos y filas completas: [JSON](data/node-job-arena-model-2026-10-29.json). Contexto de las arenas: [spike OPFS](OPFS_ARENA_SPIKE_2026-10-29.md); referencias de salida del CLI: [M0](WEB_CONVERTER_BASELINE_2026-10-29.md).

## Resultado

**Criterio de parada activado: ningún reparto (FIFO, LIFO, mayor primero; N = 4 y 8) queda en ≤ 1,5 × en la variante (a) ni en la (b) para todos los casos.** La variante (a) (hojas y LOD fijan su arena) llega a 1,9–7,2 × el nativo de 1 worker y no libera nada. La variante (b) (hojas y LOD en una arena retenida) sí libera espacio, pero queda en 1,6–3,6 × el nativo de 1 worker (1,6–2,1 × con FIFO/LIFO) y solo baja de 1,5 × en Belvedere/2 divisiones frente al nativo de 4 workers (1,32–1,47 ×). Majadas, y Belvedere con 8 divisiones, superan 1,5 × con cualquier reparto y comparador. La Tarea 4 queda pendiente de la decisión del propietario.

## Método

- **Eventos nativos.** `real_dataset_bench convert-profile` imprime una línea `internal_spatial_node_event=` por nodo (solo con diagnósticos activos; los SHA no cambian): `node_id` final en preorden (los ids de los workers se reasignan con el desplazamiento de `append_branch`), `parent_id`, `depth`, `input_spool_bytes`, `child_spool_bytes` por hijo, `lod_bytes`, `is_leaf`, `became_leaf_all_equal`, `elapsed_ns` (trabajo propio del nodo, sin hijos) y `start_seq`/`end_seq` (secuencia, no reloj). SHA comprobados tras el cambio: LAS pequeño `413858a9…` y Belvedere `6ca0f86d…` (iguales a M0).
- **Simulador.** [`model_node_job_arena.py`](../../../crates/viewer_convert/benchmarks/model_node_job_arena.py) (Python, como el resto de `crates/viewer_convert/benchmarks/`; pruebas en `test_model_node_job_arena.py`). Cola de listos con N trabajos concurrentes; duración = `elapsed_ns` del nodo.
- **Pico** = máximo de la suma de bytes vivos. Los bytes de la arena de hijos de un trabajo se cuentan enteros al empezar el trabajo (cota superior: la arena real crece mientras corre); el LOD, al terminar. Las hojas puras no son trabajos.
- **Variante (a), como está:** el LOD del nodo y los spools de hojas viven en la arena del trabajo, que solo se libera cuando todos sus extents han muerto; como el LOD y las hojas se leen en la fase de bloques, la arena no se libera nunca (el pico es todo lo escrito salvo temporales).
- **Variante (b):** LOD y spools de hojas van a una arena retenida (vivos hasta la fase de bloques); los spools internos ya consumidos se liberan cuando el último hermano termina. Un nodo que resulta hoja por puntos idénticos (`became_leaf_all_equal`) fijaría la arena; no aparece en estos casos.
- **Repartos:** FIFO (BFS), LIFO (DFS) y mayor entrada primero. Las duraciones se midieron en una máquina compartida con otras cargas: valen solo como pesos relativos, no se dan tiempos.
- **Calibración:** el mismo simulador con N = 1, LIFO y cada spool muerto en su propio fin reproduce el pico nativo de 1 worker (0,9998–1,0001 ×; ver columna de la segunda tabla). Con 4 workers el nativo es mayor (concurrencia real), por eso se citan ambos comparadores.
- **Comparador.** Cada caso se ejecutó con 1 y 4 workers (`w1`, `w4`; mismo árbol y bytes, distinto orden y pico nativo). «× nativo» usa el `peak_live_spool_bytes` de la misma ejecución; el modelado de N = 4/8 no depende apenas del log usado.
- **Fórmula M6.** `pico_modelado ≈ k × bytes_fuente` con `bytes_fuente` = carga útil de todos los puntos de entrada (3.348.975.489 B Belvedere, 13.710.374.106 B Majadas): variante (b) 2,3–2,6 × (Belvedere/2 divisiones y Majadas), 1,99 × (Belvedere/8); variante (a) 4,8–9,8 ×. Mínimo inevitable (hojas+LOD retenidos) ≈ 1,0 × fuente.

## Pico modelado frente a nativo (mín–máx sobre los tres repartos)

Casos: Belvedere y Majadas; `d2`/`d8` = divisiones; `w1`/`w4` = workers de la ejecución nativa.

| Caso | Pico nativo (B) | Var. | N | Pico modelado mín–máx (B) | × nativo mín–máx | × fuente mín–máx |
| --- | --- | --- | --- | --- | --- | --- |
| belvedere-d2-w1 | 4.671.716.287 | a | 4 | 16.165.156.750–16.165.156.750 | 3,46–3,46 | 4,83–4,83 |
| belvedere-d2-w1 | 4.671.716.287 | a | 8 | 16.165.156.750–16.165.156.750 | 3,46–3,46 | 4,83–4,83 |
| belvedere-d2-w1 | 4.671.716.287 | b | 4 | 7.640.648.386–8.162.331.216 | 1,64–1,75 | 2,28–2,44 |
| belvedere-d2-w1 | 4.671.716.287 | b | 8 | 8.394.186.759–8.432.742.509 | 1,80–1,81 | 2,51–2,52 |
| belvedere-d2-w4 | 5.806.739.358 | a | 4 | 16.165.156.750–16.165.156.750 | 2,78–2,78 | 4,83–4,83 |
| belvedere-d2-w4 | 5.806.739.358 | a | 8 | 16.165.156.750–16.165.156.750 | 2,78–2,78 | 4,83–4,83 |
| belvedere-d2-w4 | 5.806.739.358 | b | 4 | 7.687.511.091–8.119.141.434 | 1,32–1,40 | 2,30–2,42 |
| belvedere-d2-w4 | 5.806.739.358 | b | 8 | 8.292.245.704–8.528.304.094 | 1,43–1,47 | 2,48–2,55 |
| belvedere-d8-w1 | 3.442.632.020 | a | 4 | 6.660.894.157–6.660.894.157 | 1,93–1,93 | 1,99–1,99 |
| belvedere-d8-w1 | 3.442.632.020 | a | 8 | 6.660.894.157–6.660.894.157 | 1,93–1,93 | 1,99–1,99 |
| belvedere-d8-w1 | 3.442.632.020 | b | 4 | 6.660.894.157–6.660.894.157 | 1,93–1,93 | 1,99–1,99 |
| belvedere-d8-w1 | 3.442.632.020 | b | 8 | 6.660.894.157–6.660.894.157 | 1,93–1,93 | 1,99–1,99 |
| belvedere-d8-w4 | 3.535.842.000 | a | 4 | 6.660.894.157–6.660.894.157 | 1,88–1,88 | 1,99–1,99 |
| belvedere-d8-w4 | 3.535.842.000 | a | 8 | 6.660.894.157–6.660.894.157 | 1,88–1,88 | 1,99–1,99 |
| belvedere-d8-w4 | 3.535.842.000 | b | 4 | 6.660.894.157–6.660.894.157 | 1,88–1,88 | 1,99–1,99 |
| belvedere-d8-w4 | 3.535.842.000 | b | 8 | 6.660.894.157–6.660.894.157 | 1,88–1,88 | 1,99–1,99 |
| majadas-d2-w1 | 18.829.664.481 | a | 4 | 134.724.002.211–134.724.002.211 | 7,15–7,15 | 9,83–9,83 |
| majadas-d2-w1 | 18.829.664.481 | a | 8 | 134.724.002.211–134.724.002.211 | 7,15–7,15 | 9,83–9,83 |
| majadas-d2-w1 | 18.829.664.481 | b | 4 | 36.008.913.438–68.384.675.373 | 1,91–3,63 | 2,63–4,99 |
| majadas-d2-w1 | 18.829.664.481 | b | 8 | 36.008.940.708–38.860.379.985 | 1,91–2,06 | 2,63–2,83 |
| majadas-d2-w4 | 20.639.155.824 | a | 4 | 134.724.002.211–134.724.002.211 | 6,53–6,53 | 9,83–9,83 |
| majadas-d2-w4 | 20.639.155.824 | a | 8 | 134.724.002.211–134.724.002.211 | 6,53–6,53 | 9,83–9,83 |
| majadas-d2-w4 | 20.639.155.824 | b | 4 | 36.008.908.803–68.320.631.151 | 1,74–3,31 | 2,63–4,98 |
| majadas-d2-w4 | 20.639.155.824 | b | 8 | 36.024.795.324–36.024.795.324 | 1,75–1,75 | 2,63–2,63 |

Con 8 divisiones el pico modelado coincide con todo lo escrito: la raíz produce 81 hijos y su arena (≈ 3,3 GB de fuente) solo se libera al terminar todos, mientras los hijos ya escriben sus propias arenas. En Majadas con N = 4 el reparto «mayor primero» casi duplica el pico (hasta 3,6 × el nativo de 1 worker) frente a FIFO/LIFO, porque abre a la vez varios subárboles grandes.

## Forma de los árboles y camino crítico

| Caso | Fuente (B) | Escrito (B) | Nodos | Hojas | Retenido hojas+LOD (B) | Camino crítico (ns, relativo) | Calibración N=1 (× nativo) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| belvedere-d2-w1 | 3.348.975.489 | 16.165.156.750 | 1343 | 1083 | 3.350.241.235 | 17.754.855.918 | 1,0 |
| belvedere-d2-w4 | 3.348.975.489 | 16.165.156.750 | 1343 | 1083 | 3.350.241.235 | 18.077.183.238 | 0,8045 |
| belvedere-d8-w1 | 3.348.975.489 | 6.660.894.157 | 5140 | 5070 | 3.349.247.856 | 12.212.701.582 | 1,0001 |
| belvedere-d8-w4 | 3.348.975.489 | 6.660.894.157 | 5140 | 5070 | 3.349.247.856 | 14.668.564.711 | 0,9737 |
| majadas-d2-w1 | 13.710.374.106 | 134.724.002.211 | 6259 | 4990 | 13.716.507.966 | 92.319.194.591 | 0,9998 |
| majadas-d2-w4 | 13.710.374.106 | 134.724.002.211 | 6259 | 4990 | 13.716.507.966 | 111.687.727.135 | 0,9121 |

El camino crítico (cadena más larga de duraciones con trabajadores ilimitados) y las duraciones de las filas `w4` incluyen solapes entre workers y carga ajena: solo son relativos. Los makespans por reparto están en el JSON.

## Enmiendas del pre-mortem del 29-10-2026

- **A1:** el evento lleva `lod_bytes`, `is_leaf`, `became_leaf_all_equal` y `elapsed_ns`; el simulador sigue la vida de cada extent y libera una arena solo cuando todos están muertos; variantes (a) y (b) arriba.
- **A2:** repartos FIFO, LIFO y mayor primero con N = 4 y 8, guiados por `elapsed_ns`; se informa pico por reparto × variante frente al nativo, pico / bytes de fuente (fórmula M6) y camino crítico.
- **A3:** ids finales en preorden (reasignados con el desplazamiento de `append_branch`); la prueba corre con 1, 2 y 4 workers y comprueba unicidad, contigüidad y padre antes que hijo.
- **Interpretación del criterio:** parada solo si ningún reparto de (a) ni de (b) queda en ≤ 1,5 ×. Aquí sí ocurre en Majadas y Belvedere/8 divisiones (y en Belvedere/2 divisiones frente al nativo de 1 worker); solo Belvedere/2 divisiones frente al nativo de 4 workers queda en 1,32–1,47 × (variante b).

## Límites

- Una ejecución por celda; perfil `perf`; este equipo; no trasladar las cifras a otras revisiones ni perfiles.
- Cota superior: la arena de hijos se cuenta entera al empezar el trabajo. Sin esa cota el pico real estaría entre este y el de contar al terminar; no se midió.
- Las duraciones salen de una máquina con carga ajena y de ejecuciones con solape entre workers; el reparto solo usa su orden relativo.
- Modelo de disco: un spool es una extensión de una arena sin hueco reutilizable; no se modelan fragmentación ni compactación de OPFS.
