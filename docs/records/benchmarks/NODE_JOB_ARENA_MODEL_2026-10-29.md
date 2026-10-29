# Modelo de pico de los trabajos por nodo (S1b, 29-10-2026)

Registro fechado e inmutable (etapa 2 del conversor web paralelo, Tarea 1). Mide si el pico de spool vivo del modelo «un trabajo por nodo» (cada trabajo lee el spool de entrada de un nodo y escribe todos sus hijos en una arena nueva; la arena del padre se borra cuando el último hijo la ha consumido) queda dentro de 1,5 × el `peak_live_spool_bytes` nativo. Datos crudos y filas completas: [JSON](data/node-job-arena-model-2026-10-29.json). Contexto de las arenas: [spike OPFS](OPFS_ARENA_SPIKE_2026-10-29.md); referencias de salida del CLI: [M0](WEB_CONVERTER_BASELINE_2026-10-29.md).

## Resultado

**Criterio de parada activado para las arenas por trabajo (variantes a y b): ningún reparto (FIFO, LIFO, mayor primero; N = 4 y 8) queda en ≤ 1,5 × en todos los casos. La variante c (un archivo por spool de hijo) sí cumple ≤ 1,46 × en los seis casos y con los tres repartos (ver «Variante c»).** La variante (a) (hojas y LOD fijan su arena) llega a 1,9–7,2 × el nativo de 1 worker y no libera nada. La variante (b) (hojas y LOD en una arena retenida) sí libera espacio, pero queda en 1,6–3,6 × el nativo de 1 worker (1,6–2,1 × con FIFO/LIFO) y solo baja de 1,5 × en Belvedere/2 divisiones frente al nativo de 4 workers (1,32–1,47 ×). Majadas, y Belvedere con 8 divisiones, superan 1,5 × con cualquier reparto y comparador. La Tarea 4 queda pendiente de la decisión del propietario.

## Método

- **Eventos nativos.** `real_dataset_bench convert-profile` imprime una línea `internal_spatial_node_event=` por nodo (solo con diagnósticos activos; los SHA no cambian): `node_id` final en preorden (los ids de los workers se reasignan con el desplazamiento de `append_branch`), `parent_id`, `depth`, `input_spool_bytes`, `child_spool_bytes` por hijo, `lod_bytes`, `is_leaf`, `became_leaf_all_equal`, `elapsed_ns` (trabajo propio del nodo, sin hijos) y `start_seq`/`end_seq` (secuencia, no reloj). SHA comprobados tras el cambio: LAS pequeño `413858a9…` y Belvedere `6ca0f86d…` (iguales a M0).
- **Simulador.** [`model_node_job_arena.py`](../../../crates/viewer_convert/benchmarks/model_node_job_arena.py) (Python, como el resto de `crates/viewer_convert/benchmarks/`; pruebas en `test_model_node_job_arena.py`). Cola de listos con N trabajos concurrentes; duración = `elapsed_ns` del nodo.
- **Pico** = máximo de la suma de bytes vivos. Los bytes de la arena de hijos de un trabajo se cuentan enteros al empezar el trabajo (cota superior: la arena real crece mientras corre); el LOD, al terminar. Las hojas puras no son trabajos.
- **Variante (a), como está:** el LOD del nodo y los spools de hojas viven en la arena del trabajo, que solo se libera cuando todos sus extents han muerto; como el LOD y las hojas se leen en la fase de bloques, la arena no se libera nunca (el pico es todo lo escrito salvo temporales).
- **Variante (b):** LOD y spools de hojas van a una arena retenida (vivos hasta la fase de bloques); los spools internos ya consumidos se liberan cuando el último hermano termina. Un nodo que resulta hoja por puntos idénticos (`became_leaf_all_equal`) fijaría la arena; no aparece en estos casos.
- **Repartos:** FIFO (BFS), LIFO (DFS) y mayor entrada primero. Las duraciones se midieron en una máquina compartida con otras cargas: valen solo como pesos relativos, no se dan tiempos.
- **Calibración (validez acotada):** el mismo simulador con N = 1, LIFO y cada spool muerto en su propio fin reproduce el pico nativo de 1 worker (0,9998–1,0001 ×). Eso valida la contabilidad de bytes, los datos de los eventos y el orden DFS; **no** valida la lógica de vida de las arenas (solo la cubre la prueba manual de 5 nodos) ni la concurrencia. Frente al nativo de 4 workers el mismo modelo da 0,76 / 0,97 / 0,92 (Belvedere/2 div., Belvedere/8 div., Majadas; segunda ejecución): el nativo con 4 workers solapa subárboles de raíz (el modelo N = 1 no), así que cae por debajo y no es un error del modelo.
- **Comparador (matiz):** el nativo con 4 workers paraleliza por hijo de la raíz (hasta 4 ramas a la vez); el modelo reparte por nodo. Los dos «× nativo» no miden la misma concurrencia.
- **Comparador.** Cada caso se ejecutó con 1 y 4 workers (`w1`, `w4`; mismo árbol y bytes, distinto orden y pico nativo). «× nativo» usa el `peak_live_spool_bytes` de la misma ejecución; el modelado de N = 4/8 no depende apenas del log usado.
- **Fórmula M6.** `pico_modelado ≈ k × bytes_fuente` con `bytes_fuente` = carga útil de todos los puntos de entrada (3.348.975.489 B Belvedere, 13.710.374.106 B Majadas): variante (b) 2,3–2,6 × (Belvedere/2 divisiones y Majadas) solo con FIFO/LIFO (con «mayor primero» llega a 4,99 × en Majadas, N = 4), 1,99 × (Belvedere/8); variante (a) 4,8–9,8 ×; variante c 2,00 × con 2 divisiones y 1,10–1,21 × con 8 (segunda ejecución). Mínimo inevitable (hojas+LOD retenidos) ≈ 1,0 × fuente.

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

## Por qué suben (a) y (b)

- **(a)** pico = todo lo escrito: el LOD de cada nodo comparte arena con sus hijos y se lee en la fase de bloques, así que ninguna arena se libera (Belvedere/2 div. 16,2 GB frente a 4,7 GB nativo).
- **(b)** una arena de hijos no se libera hasta que termina el último hermano: con 8 divisiones la arena de 81 hijos de la raíz (≈ 3,3 GB) vive mientras todos los hijos escriben las suyas; con 2 divisiones y Majadas, los subárboles grandes abiertos a la vez retienen sus arenas completas.

## Ampliación: variantes c, r y descomposición (segunda ejecución)

Variantes añadidas ([JSON](data/node-job-arena-model-variants-2026-10-29.json)): **por spool** = ninguna arena, cada spool muere al terminar su trabajo (línea base de concurrencia pura); **c** = cada spool de hijo es su propio archivo OPFS, liberado al terminar el trabajo que lo consume (LOD y hojas en la arena retenida, como en b), así que sus bytes son los de «por spool» y aporta el recuento de archivos; **r** = una arena por hijo de la raíz, liberada cuando termina su subárbol (LOD y hojas retenidos).

**Se repitieron las seis ejecuciones nativas** (los registros de la primera no se conservaron): el árbol y los bytes son los mismos, pero `elapsed_ns` y el pico nativo con 4 workers cambian (Belvedere/2 div.: 5,81 → 6,15 GB; el de 1 worker es idéntico), así que los números de a y b de esta sección difieren algo de las tablas anteriores; para a y b valen las anteriores, para la comparación entre variantes, esta sección.

### Descomposición del exceso (× pico nativo de la misma ejecución, mín–máx sobre FIFO/LIFO/mayor primero)

| Caso | nativo | por spool N=4 | por spool N=8 | rama N=4 | rama N=8 | b N=4 | b N=8 | a |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| belvedere-d2-w1 | 1,00 | 1,43 | 1,43 | 2,66–3,23 | 2,68–2,89 | 1,58–1,73 | 1,72 | 3,46 |
| belvedere-d2-w4 | 1,00 | 1,09 | 1,09 | 2,04–2,62 | 2,09–2,20 | 1,19–1,34 | 1,31 | 2,63 |
| belvedere-d8-w1 | 1,00 | 1,07–1,08 | 1,15–1,18 | 1,07–1,08 | 1,15–1,18 | 1,93 | 1,93 | 1,93 |
| belvedere-d8-w4 | 1,00 | 1,05 | 1,12–1,15 | 1,05 | 1,12–1,15 | 1,88 | 1,88 | 1,88 |
| majadas-d2-w1 | 1,00 | 1,46 | 1,46 | 5,09–7,15 | 3,76–3,83 | 1,91–3,63 | 1,94 | 7,15 |
| majadas-d2-w4 | 1,00 | 1,33 | 1,33 | 5,63–6,55 | 4,52–5,83 | 1,55–3,34 | 1,80–1,94 | 6,55 |

Lectura: la concurrencia pura (por spool) explica hasta ×1,46; la granularidad de arena añade el resto, y es lo que más pesa en Belvedere/8 div. (b ×1,9 frente a ×1,05–1,18 por spool) y en «rama» (la arena por rama retiene subárboles enteros de las ramas concurrentes). La retención de LOD y hojas dentro de la arena (a frente a b) cuesta ×3,46 → ×1,6–1,7 en Belvedere/2 div. y ×7,15 → ×1,9–3,6 en Majadas (w1). Con 8 divisiones «rama» coincide con «por spool» (5.070 de 5.140 nodos son hojas, hay poco que retener dentro de cada rama).

### Variante c (un archivo por spool de hijo)

| Caso | Pico nativo (B) | N | Pico c (B) mín–máx | × nativo | × fuente | Archivos vivos máx (mín–máx) |
| --- | --- | --- | --- | --- | --- | --- |
| belvedere-d2-w1 | 4.671.716.287 | 4 | 6.697.957.338 | 1,43 | 2,00 | 49–141 |
| belvedere-d2-w1 | 4.671.716.287 | 8 | 6.697.959.935 | 1,43 | 2,00 | 69–141 |
| belvedere-d2-w4 | 6.145.601.064 | 4 | 6.697.957.338 | 1,09 | 2,00 | 40–145 |
| belvedere-d2-w4 | 6.145.601.064 | 8 | 6.697.959.935 | 1,09 | 2,00 | 64–142 |
| belvedere-d8-w1 | 3.442.632.020 | 4 | 3.696.786.973–3.716.712.713 | 1,07–1,08 | 1,10–1,11 | 70 |
| belvedere-d8-w1 | 3.442.632.020 | 8 | 3.949.921.410–4.061.538.761 | 1,15–1,18 | 1,18–1,21 | 70 |
| belvedere-d8-w4 | 3.535.787.435 | 4 | 3.696.786.867–3.716.712.713 | 1,05 | 1,10–1,11 | 70 |
| belvedere-d8-w4 | 3.535.787.435 | 8 | 3.949.921.410–4.061.538.655 | 1,12–1,15 | 1,18–1,21 | 70 |
| majadas-d2-w1 | 18.829.664.481 | 4 | 27.420.759.618 | 1,46 | 2,00 | 47–642 |
| majadas-d2-w1 | 18.829.664.481 | 8 | 27.420.759.618 | 1,46 | 2,00 | 55–220 |
| majadas-d2-w4 | 20.572.047.024 | 4 | 27.420.759.618 | 1,33 | 2,00 | 78–642 |
| majadas-d2-w4 | 20.572.047.024 | 8 | 27.420.759.618 | 1,33 | 2,00 | 67–398 |

**Con c el criterio de 1,5 × se cumple en los seis casos y con los tres repartos** (máximo 1,46 ×, Majadas con el nativo de 1 worker). El coste es el número de archivos vivos: hasta 145 en Belvedere/2 div. y 642 en Majadas (N = 4, el reparto peor), más 70 con 8 divisiones (un archivo por hijo de la raíz, casi todos hojas retenidas en una arena). Para 2 divisiones el pico de c es ≈ 2,0 × los bytes de la fuente (fórmula M6); para 8 divisiones, 1,10–1,21 ×. Los datos de las variantes `own` y de calibración del JSON no llevan `max_live_files`: el simulador lo acumulaba en vez de contar archivos vivos y los registros nativos de esas ejecuciones ya no existen para regenerarlo (el simulador está corregido). Para comparar variantes valen los números de la segunda ejecución (esta sección y la anterior); la primera solo sirve para a y b. Archivos vivos de las otras variantes (mín–máx entre N = 4 y 8, repartos): a 260 (Belvedere/2 div.), 1.269 (Majadas); b 13–59 y 25–442; r 5 (Belvedere/2 div. y Majadas) y 70 (8 div.).

## Límites

- Una ejecución por celda; perfil `perf`; este equipo; no trasladar las cifras a otras revisiones ni perfiles.
- Cota superior: la arena de hijos se cuenta entera al empezar el trabajo. Sin esa cota el pico real estaría entre este y el de contar al terminar; no se midió.
- Veredictos frente a la cota: en (a) y en (b) con 8 divisiones el pico modelado es igual a todo lo escrito, así que no depende de cuándo se cuenta la arena; en (b) con Majadas y 2 divisiones no se midió la otra cota, y su veredicto (> 1,5 ×) queda sin confirmar frente a ella. En c y «por spool» el pico ya incluye el crecimiento completo de la arena al inicio del trabajo (cota superior), luego el pico real es igual o menor y el margen frente a 1,5 × es igual o mayor.
- El criterio de parada se lee por caso (cada fila de Belvedere/Majadas, w1 y w4, es un caso): c lo cumple en los seis; a y b no lo cumplen en Majadas ni en Belvedere/8 div.
- Las duraciones salen de una máquina con carga ajena y de ejecuciones con solape entre workers; el reparto solo usa su orden relativo.
- Modelo de disco: un spool es una extensión de una arena sin hueco reutilizable; no se modelan fragmentación ni compactación de OPFS.
