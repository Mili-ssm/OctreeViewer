# R3, fase 2c: transiciones de nivel en modo celda — 19-11-2026

**Estado: medido; la decisión de cierre la toma el propietario** ([OQ-17](../../requirements.md#preguntas-abiertas),
[#99](https://github.com/Mili-ssm/OctreeViewer/issues/99)). Es la comprobación de la fase 2c del
[plan del nivel pedido y el LOD por celda](../../plans/nivel-pedido-y-lod-por-celda.md).

**Conclusión:** la puerta espacial del ADR 0053 se cumple en pantalla en Belvedere (datos reales) y no en el octree
sintético del arnés; la temporal (que no filtra por pantalla) no se cumple en ninguno de los dos: 280 saltos en una
órbita de Belvedere y 62–1338 por simulación en el sintético.

| Datos | Pares vecinos en pantalla, dibujados por nodos distintos, con más de un nivel de diferencia | Saltos de más de un nivel dentro de un nodo entre dos frames |
| --- | --- | --- |
| Belvedere, 4 simulaciones reales | 0 (de 0 a 44 pares visibles) | 0, salvo 280 en la órbita de semilla 7 |
| Belvedere, 3 casos del arnés de streaming | sin pares visibles | 0 a 343 |
| Octree sintético, 18 simulaciones `sim_synthetic_*` | 48 a 855 en las 14 con pares visibles (0,9 % a 22,4 %; las peores, el dolly) | 62 a 1338 |

Los 280 saltos de la órbita de semilla 7 son los mismos que el experimento E-B1 midió en esa órbita al quitar la
ventana de un nivel por frame ([replanteamiento](../archive/replanteamiento-lod-por-celda-18-11-2026.md): «280 saltos…
antes 0»). Las demás cifras no tienen línea base con la que compararse.

Según OQ-11 y RF-31 ([requisitos](../../requirements.md)), el fin es que no se vean saltos, y lo juzga el propietario
en la demo. El plan pide abrir una pregunta si la puerta no se cumple: es OQ-17.

## Condiciones

| Dato | Valor |
| --- | --- |
| Revisión | `cd90b7f` (`main`, tras fusionar #95 y #97) |
| Perfil y toolchain | `fast` en los dos comandos (el mensaje de `#[ignore]` de `real_sim` sugiere `perf`; los contadores no dependen del perfil); nightly fijado en `rust-toolchain.toml` |
| Equipo | AMD Ryzen AI 9 365, Radeon 880M |
| Datasets | `target/render-data/belvedere-v7.oct` (simulaciones reales y casos `belvedere *` del arnés) y el octree sintético del arnés |
| Comandos | `R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile fast --offline --lib -- cells_stream_tests --nocapture --test-threads=1` (70 pasan, 17 ignorados) · `… -- --ignored real_sim --nocapture --test-threads=1` (3 pasan) |
| Datos | [`data/fase-2c-19-11-2026/`](data/fase-2c-19-11-2026/): `real_sim.txt`, `stream_tests.txt` (líneas `ADR 0053 gate`, resúmenes y resultados) |

Las cifras solo valen para esta revisión, perfil, datasets y equipo.

## Qué cuenta la puerta

`StepGate` del oráculo de muestras (`crates/viewer_wasm/src/lod_cells_stream_tests.rs`), sobre la rejilla de
muestras de cada nodo:

| Contador | Cuenta |
| --- | --- |
| `cell_within_node` | Muestras cuyo nivel dibujado cambia más de uno entre dos frames, con el mismo nodo dibujando |
| `cell_node_changed` | Lo mismo cuando el nodo que dibuja la muestra cambia entre los dos frames (relevo padre ↔ hijo) |
| `cell_border_pairs` / `_over_one` | Pares de muestras vecinas dibujadas por nodos distintos, y los que difieren en más de un nivel |
| `cell_border_visible_*` | Los mismos pares cuando las dos muestras están en pantalla (frustum y planos de recorte) |
| `hops` | Saltos por tiempo de espera que cuenta el pool en cada frame |

## Resultado

### Simulaciones reales de Belvedere

Los resúmenes (`digest`) coinciden con el «in-process» en las cuatro.

| Simulación | `cell_within_node` | `cell_node_changed` | Pares de borde (>1 / total) | Pares visibles (>1 / total) | Línea base 18-11 (`within`, `node_changed`) |
| --- | --- | --- | --- | --- | --- |
| dolly, semilla 3 | 0 | 382 | 1180 / 4540 | 0 / 0 | 0, 827 |
| órbita, semilla 1 | 0 | 1854 | 2656 / 17602 | 0 / 44 | 0, 2166 |
| órbita, semilla 7 | **280** | 343 | 1811 / 12149 | 0 / 22 | 0, 343 |
| órbita con reinicio, semilla 5 | 0 | 1282 | 1894 / 12060 | 0 / 27 | 0, 1282 |

La línea base es la del [registro del modo celda](R3_LINEA_BASE_CELDA_18-11-2026.md), anterior a retirar la regla
`Window` (19-11-2026). La órbita de semilla 1 tiene además 7 frames con hueco transitorio (`active_gaps`, el primero en el frame 36,
nodo 77), que el [cierre del rango por nodo](R3_CIERRE_RANGO_POR_NODO_19-11-2026.md) acepta.

### Simulaciones sintéticas (`sim_synthetic_*`)

| Simulación | Semillas | `cell_within_node` | Pares visibles con más de un nivel (por semilla) | `hops` |
| --- | --- | --- | --- | --- |
| dolly | 1, 7, 42, 1234 | 1198–1338 | 560/4554, 855/5375, 756/3369, 737/6048 | 8 |
| órbita | 1, 7, 42, 1234 | 363–738 | 77/3602, 89/3595, 48/2418, 188/3436 | 0 |
| órbita con reinicio | 1, 7, 42, 1234 | 191–972 | 170/5454, 253/4616, 418/6613, 48/5505 | 0 |
| dolly con reinicio | 1, 7 | 968–1335 | 432/4774, 566/4697 | 0, 16 |
| páginas ajustadas | 1, 7, 42, 1234 | 62–418 | sin pares visibles | 0 |

### Arnés de streaming (resúmenes por caso)

Pares visibles: ninguno en estos casos.

| Caso | `cell_within_node` | `cell_node_changed` | Pares de borde (>1 / total) |
| --- | --- | --- | --- |
| belvedere streaming | 0 | 668 | 1477 / 5050 |
| belvedere regraft | 343 | 2357 | 1502 / 5452 |
| belvedere pinned | 343 | 1971 | 1699 / 5759 |
| synthetic streaming | 39 | 0 | 54 / 517 |
| synthetic capped builds | 78 | 0 | 81 / 1026 |
| synthetic regraft | 378 | 148 | 54 / 1107 |

### Navegador (demo pública)

Demo publicada desde `cd90b7f` y abierta en Chromium 1248 sin interfaz, con WebGPU: muestra y recorte de Belvedere.

- **Consola:** sin errores ni avisos.
- **Acercamiento a la muestra:** 40 pasos de rueda en 8 s. Las capturas muestran una frontera de banda curva con
  cubos de un nivel de diferencia a cada lado y, ya dentro de la nube, los puntos sueltos de la cola bajo la
  separación.
- **Límites:** la lectura de las capturas es a ojo, no hay una diferencia de frames automática, y las capturas no se
  guardan en el repositorio.

## Límites

- Los contadores salen del gemelo de Rust, no de la GPU: la GPU usa el mismo predicado (`refines`, D2 del plan),
  pero no se leyó su salida por celda en el navegador.
- `cell_within_node` y `cell_node_changed` no filtran por pantalla: cuentan muestras dentro y fuera del frustum.
- `cell_node_changed` compara la profundidad que dibujan dos nodos distintos, y la línea base tampoco la juzga.
- No se ha separado qué pares sintéticos son relevos padre ↔ hijo y cuáles nodos vecinos con rangos distintos: es la
  opción 2 de OQ-17.
- La cola bajo la separación sigue pendiente de revisión (ADR 0053).
