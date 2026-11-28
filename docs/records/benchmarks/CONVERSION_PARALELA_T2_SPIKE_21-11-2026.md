# Conversión paralela, tarea 2: spike de rangos en nodos internos grandes — 21-11-2026

**Estado: medido (spike descartable). La variante con lector encadenado cumple la puerta; la de unión con copia, no.** Tarea 2 del
[plan de conversión paralela](../../plans/conversion-paralela.md); base en
[CONVERSION_PARALELA_T1](CONVERSION_PARALELA_T1_21-11-2026.md). Datos en
[`data/conversion-paralela-t2-spike-21-11-2026/`](data/conversion-paralela-t2-spike-21-11-2026/).

## Conclusión

### Variante con lector encadenado (`OCTREE_SPIKE_CHAIN=1`, commit `fc19b56`): puerta cumplida

Los hijos internos de un nodo partido leen como entrada los segmentos de rango de su padre, en orden, en lugar de un
spool unido por copia. Medido el 21-11-2026 con `driver-chain.sh` (`ROUND=1` y `ROUND=2`; 60 s de pausa antes de cada
ejecución, carga previa de 1 min 0,5–2,1):

| Ejecución | Total (s) | Partición raíz | Jerarquía | Bloques | Ruta crítica (s) | RSS pico (MB) |
| --- | --- | --- | --- | --- | --- | --- |
| Majadas W = 8 (2 ejecuciones) | 223,3 / 220,5 | 50,6 / 49,2 | **109,5 / 108,3** | 53,0 / 52,9 | 23,6 / 22,5 | 386 / 409 |
| Majadas W = 4 | 296,4 | 49,7 | 165,0 | 66,2 | 31,8 | 258 |
| Belvedere W = 8 (3 rondas) | 36,9 (36,3–38,2) | 13,4 | 13,5 | 7,9 | 3,3–3,5 | 290–302 |
| Belvedere W = 1 | 131,4 | 13,0 | 73,3 | 33,5 | 12,4 | 51 |

- **Puerta (jerarquía de Majadas con W = 8 ≤ 124 s): cumplida**, 108,3–109,5 s frente a 161,6 s de T1 (×1,48–1,49).
  Bytes idénticos: `.oct` y `.octattrs` iguales a T1 en las 7 ejecuciones. Belvedere W = 1 131,4 s (+1,1 % sobre 130,0 s;
  con W = 1 no se parte ningún nodo).
- **Total frente a T1:** Majadas W = 8 ×1,23 (273,0 → 220,5–223,3 s) y W = 4 ×1,09 (323,7 → 296,4 s); Belvedere W = 8 ×1,13
  (41,6 → 36,9 s). Criterio original del plan, Majadas W8/W4: ×1,33 (≥ 1,3).
- **Siguiente límite:** con W = 8 la jerarquía ya no domina; partición de la raíz (~50 s, 4 rangos) y bloques (~53 s)
  suman el 46 % del total de Majadas.
- La ruta crítica de `analyze.py` usa el `elapsed_ns` del evento de un nodo partido (rango más largo + fusión), así que es
  el tiempo crítico real; el «trabajo» sumado de esos eventos no cuenta los demás rangos y no se compara con T1.
- Sin medir con esta variante: W = 16, réplicas de W = 4, otros `T`/`K`.

### Variante con unión por copia (commits `c977b90`, `43f4158`): puerta no cumplida

- **Puerta no cumplida.** Jerarquía de Majadas con W = 8: **178,6 s** (puerta ≤ 124 s; T1: 161,6 s), es decir, **más lenta**
  que sin partir. Bytes idénticos (SHA `.oct` y `.octattrs` iguales a T1 en las 16 ejecuciones) y Belvedere con W = 1 en
  132,0 s de mediana (+1,5 % sobre 130,0 s; dentro del 3 %).
- **Total de Majadas (informativo):** W = 4 / 8 / 16 = 377,9 / 299,5 / 261,6 s (T1: 323,7 / 273,0 / 257,0). W8/W4 ×1,26 y
  W16/W8 ×1,14, pero por empeorar W = 4, no por mejorar W = 8.
- **Causa medida: la unión de segmentos.** `join_segments` suma 236,7 s (W = 8) sobre 376 nodos partidos; el trabajo total
  de la jerarquía sube de ~763 s (T1) a ~1070 s, y la concurrencia media a 6,0. Reparte en 8 hilos son ≈ 29,6 s,
  el **16,6 %** de la pared (> 15 %: umbral del encargo para probar el lector encadenado). El camino crítico baja (99,6 s
  frente a 115,1 s en T1) pero ya no limita: ahora limita el trabajo añadido, no las dependencias.
- **Paso 0 (rangos de raíz).** Belvedere W = 1, 3 rondas intercaladas: 8 rangos 127,0 s (125,8–127,7) frente a 4 rangos
  132,5 s (132,4–132,7): −4,1 %, la partición de raíz baja de 13,4 a 8,6 s. Majadas W = 8 con 8 rangos: total 277,5 s,
  raíz 39,8 s (T1 con 4: 273,0 s y 49,2 s), pero bloques 65,8 s (T1 51,9 s) y jerarquía 161,5 s igual: no mejora el total
  en la única ejecución. Bytes iguales.
- **Dónde se aplana la curva ahora:** la jerarquía de Majadas pasa de 243,8 (W = 4) a 178,6 (W = 8) y 145,5 s (W = 16); con
  W = 16 la concurrencia media es 10,3. El tope pasa del árbol al coste de unión y fusión (suma de fusión 277–357 s).
- **RSS pico** (`/usr/bin/time`, máximo del árbol): Majadas W = 4/8/16 = 334/397/627 MB; sin partir, W = 8 con 8 rangos
  352 MB (otra configuración de rangos). Belvedere W = 1 48 MB, W = 8 237–262 MB.
  Pico de bytes vivos de spool idéntico a W = 4/8/16 (33,17 GB).

## Diseño del spike (rama `spike/rangos-nodos-internos`)

- `T` = 2^20 puntos (variable `OCTREE_SPIKE_SPLIT_POINTS`), `K = min(W, ceil(puntos / T))`; con W = 1 nunca se parte.
- Un trabajo de nodo con ≥ T puntos pasa a K trabajos de rango en la cola de listos (clave `(ruta, índice)`); cada uno lee un
  tramo alineado a registros del spool de entrada (`run_node_range`), con sus segmentos de hijo, su `SampleGrid` y sus
  límites. Cuando termina el último, el coordinador encola un trabajo de **fusión** que corre en un hilo de trabajo
  (`merge_node_ranges`: `samples.merge`, límites, `join_segments` por hijo a `first_output + key`, comprobación de
  puntos, `write_lod`). El coordinador sigue siendo el único dueño de la reserva de ids de salida, del cursor de
  consolidación y del escritor; solo se reservan ids de segmentos con el mismo contador atómico de spools.
- `samples.merge` reproduce el orden serie para nodos no raíz (sumas enteras; el más cercano gana solo si es
  estrictamente menor, así que ganan los empates de los puntos anteriores): no hizo falta cambio.
- `elapsed_ns` del evento de un nodo partido = rango más largo + fusión (tiempo crítico); el trabajo sumado va en la línea
  `spike_split` de la salida de errores.
- Prueba: `range_split_nodes_keep_the_pinned_planner_structure` (ignorada; fija el umbral global a 300 puntos y comprueba
  que hubo partición y las SHA fijadas para W = 1, 2, 4, 8; también con `OCTREE_SPIKE_CHAIN=1`). `cargo test -p viewer_convert
  -p viewer_convert_core` verde. Esa prueba va ignorada porque un umbral global desajusta los contadores de
  `spool_diagnostics_account_written_and_live_bytes`: la unión escribe bytes que el diagnóstico local no cuenta
  (a corregir en la implementación real).
- Variante encadenada (commit `fc19b56`, `OCTREE_SPIKE_CHAIN=1`): un hijo interno lee los segmentos de su padre en
  su sitio con un `ChainReader` en lugar de unirlos; una hoja o un nodo de puntos iguales los une al final. Pasa la prueba
  de SHA, **no se midió**.

## Condiciones

- Equipo del propietario: AMD Ryzen AI 9 365 = 4 núcleos Zen 5 (hasta 5,09 GHz) + 6 Zen 5c (hasta 3,32 GHz), SMT 2;
  W = 8 añade núcleos lentos (el crecimiento de trabajo por nodo de T1 coincide con la media de reloj). btrfs
  `compress=zstd:1`. Perfil `perf`, `cargo build --offline --profile perf -p viewer_convert --example real_dataset_bench`.
- Binario del spike: commit `43f4158` (copiado a un directorio de trabajo propio; `CARGO_TARGET_DIR` del worktree
  `octree-spike-rangos`). Base `12eeac6`. Comando: `real_dataset_bench convert-profile ENTRADA SALIDA W 4` (`… 1 8` para
  el paso 0 con partición desactivada por `OCTREE_SPIKE_SPLIT_POINTS=1000000000000`). Orden y matriz en `driver.sh`;
  `run.sh` añade `/usr/bin/time` para el RSS desde la tercera ejecución de Belvedere (las primeras no lo tienen).
- Carga previa (`loadavg` 1 min, en cada `.meta`): 0,8–1,5 en el paso 0; 2,6–4,7 en las ejecuciones de Majadas
  (residuo de la anterior, solo 20 s de pausa) y hasta 6,6 en la primera de Belvedere W = 1 (tras W = 16).
  Esa pausa corta no cumple los 60 s previstos; un W = 1 es poco sensible, W = 4/8/16 de Majadas sí podrían serlo.
- Cifras válidas solo para esta revisión, perfil, equipo y datasets.

## Resultados (pared en s; mediana y rango en Belvedere, una ejecución en Majadas)

| Ejecución | Total | Raíz | Jerarquía | Bloques | Camino crítico | Conc. media | RSS (MB) | Unión (suma) / fusión (suma) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Belvedere W = 1, 4 rangos (paso 0, sin partir) | 132,5 (132,4–132,7) | 13,4 | 73,5 | 33,5 | 12,2 | 1,0 | 48 | — |
| Belvedere W = 1, 8 rangos (paso 0) | 127,0 (125,8–127,7) | 8,6 | 73,9 | 33,5 | 12,4 | 1,0 | 57 | — |
| Belvedere W = 1 spike (K = 1) | 132,0 (131,9–133,3) | 13,1 | 73,4 | 33,5 | 12,3 | 1,0 | 49 | — |
| Belvedere W = 8 spike | 41,5 (40,9–43,9) | 11,6 | 19,9 (19,6–20,2) | 8,2 | 12,9 | 6,6 | 243 | 22,2 s / 24,6 s (46 nodos, 193 rangos) |
| Majadas W = 8, 8 rangos (paso 0, sin partir) | 277,5 | 39,8 | 161,5 | 65,8 | 113,7 | 4,8 | 352 | — |
| Majadas W = 4 spike | 377,9 | 52,9 | 243,8 | 65,9 | 102,3 | 3,4 | 334 | 195,2 s / 226,9 s |
| Majadas W = 8 spike | 299,5 | 57,0 | **178,6** | 53,9 | 99,6 | 6,0 | 397 | 236,7 s / 276,4 s (376 nodos, 1523 rangos) |
| Majadas W = 16 spike | 261,6 | 55,1 | 145,5 | 53,3 | 107,6 | 10,3 | 627 | 308,0 s / 356,8 s |

Camino crítico: `analyze.py` corregido (`023ce48`), con los tiempos de evento (nodo partido = rango más largo + fusión).
Concurrencia media = trabajo (suma de eventos con los nodos partidos sustituidos por la suma de sus rangos) ÷ pared de la jerarquía.
Contexto T1 (camino crítico por ejecución): Majadas W4/8/16 105,2/115,1/122,7 s; Belvedere W8 14,4 s.

## Lo que no se midió

- La variante encadenada (sin copia de unión), la que más puede cambiar la puerta: la copia explica ~237 s de trabajo.
- Otros `T`/`K` (p. ej. `K` mayor que W, o `T` menor); más de una ejecución en Majadas; W = 2, 3, 6.
- Reposo de 60 s entre ejecuciones; RSS de la unión por separado; el efecto en el presupuesto de memoria del plan
  (los `SampleGrid` extra no se contabilizan en el spike).
- Navegador y wasm (sin cambios: el serie sigue siendo el oráculo).
