# C4: copia diferida del más cercano y razón de conversión v7.5/V5 — 15-11-2026

**Estado: hipótesis descartada por medida; nada integrado.** La copia diferida del registro más cercano
no puede acelerar la etapa de jerarquía: el acumulador ya guarda un índice de 4 bytes y solo copia en el
7,8 % de los puntos. Quitar la copia por completo (salida incorrecta, solo como cota) baja la etapa un
1,8 %, frente al −30 % exigido. El tiempo se va en accesos aleatorios a memoria. La razón de conversión
v7.5/V5 en Belvedere sigue en ~2,0, lejos del ≤ 1,30 de RQ-07.

## Experimento

- `SampleAccumulator` (`crates/viewer_convert_core/src/spatial.rs`) guarda una ranura `u32` en
  `NearestRecords`; la única copia (`records.set`, en `add` y en `merge`) ocurre cuando aparece un punto
  estrictamente más cercano. En la conversión completa de Belvedere v7.5: 569,6 M llamadas a `add` y
  44,2 M copias.
- Cota superior: sin ninguna de las dos copias, ejecuciones intercaladas en perfil `perf`: 5 de la base y 4
  válidas sin copia (la quinta falló por falta de espacio en `/tmp`).

| Variante | Jerarquía, mediana (rango) | Total, mediana |
| --- | --- | --- |
| `feat/r3-v75` (`ec73e01`) | 13,35 s (13,29–13,41) | 32,5 s |
| Sin copia | 13,11 s (13,02–13,17), **−1,8 %** | 33,0 s |

- Perfil con `eu-stack` (60 instantáneas durante la etapa, ~200 pilas activas): `SampleAccumulator::add`
  ~24 %, búsqueda en la tabla hash de celdas (`SparseCells::entry`) ~18 %, `SampleAccumulator::nearest`
  (lecturas aleatorias de la tabla de ranuras al cerrar) ~15 %, recorrido ordenado de celdas ~9 %,
  `run_node_job` ~9 %, `GlobalCellFrame::cell` ~5 %, `memmove` ~6,5 %.
- No hay un núcleo vectorizable: lo que domina es la latencia de memoria aleatoria. Aunque el cálculo de la
  celda (~5 %) desapareciera, no se llegaría al −30 %. Encaja con TC2(b) de la
  [fase C](EXPLORE_CONVERT_SPEED_14-11-2026.md) (−8 %): ordenar por celda traslada el coste sin quitarlo.

## Razón v7.5/V5 actual

`feat/r3-v75` (`ec73e01`), Belvedere, 4 rondas intercaladas (el orden de las variantes se invierte en cada
ronda), `loadavg` 4–7 durante las ejecuciones:

| Formato | Tiempo total | Jerarquía | Etapa de bloques | RSS pico | Salida (`.oct` + `.octattrs`) |
| --- | --- | --- | --- | --- | --- |
| V5 | 20,3 s | 6,1 s | 3,7 s | 67.804 KiB | 981,1 + 610,6 = 1.591,7 MB |
| v7.5, orden de la fuente | 41,1 s (**×2,03**) | 18,2 s | 9,3 s | 94.260 KiB (+39 %) | 815,3 + 896,3 = 1.711,6 MB (+7,5 %) |
| v7.5, orden híbrido | 40,3 s (**×1,99**) | 17,6 s | 10,4 s | 92.024 KiB (+36 %) | 531,0 + 889,6 = 1.420,6 MB (**−10,7 %**) |

Las tallas salen de una conversión aparte de cada formato con el mismo binario.

## Límites

- La jerarquía de v7.5 mide 13,3 s en la serie del experimento y 17,2–20,2 s en la de la razón, con el mismo
  binario: la máquina estaba más cargada en la segunda (`loadavg` 4–7 frente a 3–4). La comparación dentro de
  cada serie es válida; entre series, no.
- Un dataset (Belvedere).
- No se probaron las alternativas que siguen siendo posibles: tablas de celdas densas con lectura anticipada
  (necesita `unsafe` o intrínsecos, prohibidos en varios crates), evitar la tabla hash en los nodos con pocas
  muestras por celda, o el rediseño TC5.

## Identidad y reproducción

- Código medido: `feat/r3-v75` en `ec73e01`; la variante sin copia no se conserva (rama
  `explore/c4-deferred-copy` sin commits, borrada). Entorno: el de
  [T11](data/r3-t11-smoke-13-11-2026/identity.txt).
- Datos en [data/c4-deferred-copy-15-11-2026](data/c4-deferred-copy-15-11-2026/): `ub2.out` (tiempos por
  etapa del experimento), `stacks.txt` (pilas de `eu-stack`), `run.py` y `cmp.sh` (scripts del experimento)
  y `ratio-runs.jsonl` (una línea por conversión de la serie de la razón).
