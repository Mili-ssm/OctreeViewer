# R3, fase 2c: transiciones de nivel en modo celda — 19-11-2026

**Estado: medido.** Cierra la comprobación de la fase 2c del
[plan del nivel pedido y el LOD por celda](../archive/nivel-pedido-y-lod-por-celda.md).

**Conclusión:**

- **Puerta espacial:** ningún par de muestras vecinas en pantalla, dibujadas por nodos distintos, difiere en más de
  un nivel, en ninguna simulación.
- **Puerta temporal, dentro de un nodo:** se cumple (0) en todas las simulaciones reales salvo la órbita de
  semilla 7. Allí hay 280 saltos de más de un nivel. Es exactamente el coste previsto al quitar la ventana de un nivel
  por frame (E-B1 del [replanteamiento](../archive/replanteamiento-lod-por-celda-18-11-2026.md): «280 saltos… antes
  0»).
- **Relevos entre nodos:** cuentan cientos o miles de saltos, como en la línea base, y no se juzgan aquí.
- **Por qué basta:** según OQ-11 y RF-31 ([requisitos](../../requirements.md)), el fin es que no se vean saltos, y lo
  juzga el propietario en la demo. La demo pública se comprobó el mismo día (abajo).

## Condiciones

| Dato | Valor |
| --- | --- |
| Revisión | `cd90b7f` (`main`, tras fusionar #95 y #97) |
| Perfil y toolchain | `fast`; nightly fijado en `rust-toolchain.toml` |
| Equipo | AMD Ryzen AI 9 365, Radeon 880M |
| Datasets | `target/render-data/belvedere-v7.oct` (simulaciones reales) y el octree sintético del arnés |
| Comandos | `R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile fast --offline --lib -- cells_stream_tests --nocapture --test-threads=1` (70 pasan, 17 ignorados) · `… -- --ignored real_sim --nocapture --test-threads=1` (3 pasan) |
| Datos | [`data/fase-2c-19-11-2026/`](data/fase-2c-19-11-2026/): `real_sim.txt`, `stream_tests.txt` (líneas `ADR 0053 gate` y resultados) |

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
`Window` (19-11-2026). La órbita de semilla 1 imprime además su hueco transitorio conocido (nodo 77, frame 36), que
el [cierre del rango por nodo](R3_CIERRE_RANGO_POR_NODO_19-11-2026.md) acepta.

### Arnés de streaming

Pares visibles: 0 en todos los casos.

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
- `cell_node_changed` compara la profundidad que dibujan dos nodos distintos, y la línea base tampoco la juzga. Si se
  ven saltos en el relevo entre nodos es lo que decide el propietario en la demo (RF-31).
- La cola bajo la separación sigue pendiente de revisión (ADR 0053).
