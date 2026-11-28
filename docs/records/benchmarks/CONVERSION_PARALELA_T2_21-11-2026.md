# Conversión paralela, tarea 2: rangos en los nodos internos grandes — 21-11-2026

**Estado: medido.** Tarea 2 del [plan de conversión paralela](../../plans/conversion-paralela.md), segunda casilla: mide
la implementación de producción de la variante encadenada del
[spike](CONVERSION_PARALELA_T2_SPIKE_21-11-2026.md). Base anterior en
[CONVERSION_PARALELA_T1](CONVERSION_PARALELA_T1_21-11-2026.md). Datos en bruto en
[`data/conversion-paralela-t2-21-11-2026/`](data/conversion-paralela-t2-21-11-2026/).

**Conclusión:**

- **(a) W = 1 no empeora más del 3 %: +1,0 % (mediana de las diferencias emparejadas B frente a A; rango −1,0 % a
  +1,4 %, 5 rondas intercaladas en Belvedere).** Medianas: A (`99e39e1`, código de la tarea 1) 131,4 s, B (`d7d653e`) 132,6 s.
  Con W = 1 no se parte ningún nodo (por diseño: la partición exige W > 1); la diferencia está dentro del ruido visto en
  T1 (−5,5 % a +0,3 %). Entre sesiones, el mismo código de la tarea 1 midió 129,9 s en T1 y 131,4 s aquí (+1,2 %, sin
  explicar).
- **(b) Puerta del spike cumplida: jerarquía de Majadas con W = 8 110,0 s (mediana de 3; 108,5–110,2), frente a 161,6 s
  en T1 (T1 ÷ T2 ×1,47; objetivo ≤ 124 s).** Igual que el spike (108,3–109,5 s). Totales frente a T1: Majadas W = 4 / 8 / 16
  300,9 / 224,6 / 215,3 s (T1 323,7 / 273,0 / 257,0; T1 ÷ T2 ×1,08 / ×1,22 / ×1,19); Belvedere 47,9 / 36,7 / 31,4 s
  (T1 52,4 / 41,6 / 38,8; ×1,09 / ×1,13 / ×1,24).
- **(c) Criterio original del plan (Majadas W = 8 ≥ 1,3× W = 4): ×1,34, ahora sí se cumple** (T1: ×1,19), con una sola
  ejecución de W = 4 y un margen del 3 % (en Belvedere W = 4 varía un 12 % entre rondas: 47,3–53,1 s). Majadas W = 16
  frente a W = 8: ×1,04. Belvedere W = 8 frente a W = 4: ×1,31; W = 16 frente a W = 8: ×1,17.
- **(d) La jerarquía deja de ser el único límite.** Con W = 8, Majadas reparte el total así: partición de la raíz 22,7 %,
  jerarquía 49,0 %, bloques 23,9 %, validación 4,3 %; con W = 16: 26,0 / 45,2 / 25,1 / 3,5 %. La raíz (50,9 → 56,0 s) y
  los bloques (53,7 → 54,1 s) no bajan de forma apreciable de W = 8 a W = 16 (una sola ejecución con W = 16) y suman el
  47–51 % del total. El desequilibrio entre hijos de la raíz sigue: en Majadas el primer hijo recibe el 100 % de los bytes
  de spool y en Belvedere uno recibe el ~87 % (`internal_spatial_root_child_spool_bytes_written`). En Belvedere la raíz es ya la
  etapa más larga con W = 16 (13,4 s, 42,7 %; jerarquía 10,9 s, bloques 5,8 s, validación 1,5 s).
- **(e) RSS pico (`/usr/bin/time -f %M`) crece con W:** Belvedere 46–50 MiB (W = 1), 126–139 MiB (W = 4), 205–224 MiB
  (W = 8), 297–350 MiB (W = 16); Majadas 278 MiB (W = 4), 384–392 MiB (W = 8), 562 MiB (W = 16), por encima del
  presupuesto por defecto de 512 MiB (el RSS incluye memoria que el presupuesto no controla). El spike vio 386–409 MB en
  Majadas W = 8. **El pico estimado que informa la conversión (`controlled_memory_estimate_bytes`) también lo supera en
  Majadas:** 512,5 / 517,5 / 527,5 MiB con W = 4 / 8 / 16 (Belvedere 511,6 / 488,0 / 498,0 MiB; T1, sin partición,
  129,4 MiB en ambas nubes). Los rangos se admiten contra el presupuesto con el índice esperado, y el informe los suma al
  índice real final; ese pico se informa sin comprobarlo, porque comprobarlo enviaría Majadas al repliegue de un hilo
  (`c84c884`; seguimiento en [#120](https://github.com/Mili-ssm/OctreeViewer/issues/120)).
- **SHA idéntica en las 24 ejecuciones:** Belvedere `.oct` `8518efc0…` y `.octattrs` `20d08de0…` en las 19; Majadas
  `.oct` `d2d95238…` y `.octattrs` `285d7575…` en las 5 (las mismas de T0 y T1). `spatial_workers_used` = W pedido en
  todas (sin repliegue a un hilo); con W = 16 en Majadas la validación usó 14 workers por el presupuesto.

## Condiciones

- Equipo del propietario: AMD Ryzen AI 9 365, 4 núcleos Zen 5 (hasta 5,09 GHz) y 6 Zen 5c (hasta 3,32 GHz), SMT 2
  (20 hilos), 30 GiB de RAM; btrfs con `compress=zstd:1`. Rama `feat/conversion-paralela`. Binario B: `d7d653e`
  (implementación con su ronda de arreglos: `0983b4e`, `ce76849`, `d7d653e`; umbral de partición `2^20` puntos); binario A:
  `99e39e1` en un worktree temporal con su propio `CARGO_TARGET_DIR` (ya eliminado). Ambos
  `cargo build --offline --profile perf -p viewer_convert --example real_dataset_bench`, copiados a un directorio de
  trabajo para que no cambien durante la matriz. Cifras solo válidas para esa revisión, perfil, equipo y datasets.
- Comando por ejecución: `BIN=<binario> run.sh ETIQUETA ENTRADA.laz DIR_TRABAJO W 4`, que llama a
  `real_dataset_bench convert-profile ENTRADA SALIDA W 4` (4 rangos de raíz explícitos). `run.sh` es el de T1 más el
  RSS pico en el `.meta` (`peak_rss_kib`); orden y matriz en `driver.sh` (rutas absolutas del equipo del propietario;
  mismas entradas que T1): W = 1 con A/B en 5 rondas que alternan quién va primero; Belvedere W = 4, 8, 16 × 3 rondas
  intercaladas; Majadas W = 4 × 1, W = 8 × 3, W = 16 × 1. Una conversión a la vez; antes de cada una, 60 s de espera y
  comprobación de que no corría `cargo`, `rustc` ni otra conversión.
- Carga previa (`loadavg` 1 min, en `driver.log` y cada `.meta`): 0,5–1,2 en W = 1; 0,4–2,2 en Belvedere W > 1;
  1,7–2,6 en Majadas. De ~20:49 a ~20:52 corrió un script de resumen en un núcleo (`analyze.py` sobre los registros de
  Majadas): se solapó con el final de Majadas W = 8 r1 (20:48–20:51, la más lenta: 231,1 s, raíz 58,0 s frente a
  50,2–50,9 s en r2–r3) y quizá con el arranque de r2 (20:52).
- Ventanas de pared de `stage_*_ms`; `analyze.py` es la copia de T1 (ruta crítica corregida). **Con partición, el
  `elapsed_ns` de un nodo partido es el del rango más largo más la fusión**, así que la suma del trabajo por nodo y la
  ruta crítica del árbol no son comparables con T1 y no se informan aquí.

## Curva (mediana; mín.–máx.), pared en segundos

Belvedere, 3 rondas (W = 1: 5 rondas); Majadas W = 8, 3 rondas; resto, 1 ejecución:

| Config. | Belvedere total | ×vs W = 4 | T1 total | Partición raíz | Jerarquía | Bloques | Validación |
| --- | --- | --- | --- | --- | --- | --- | --- |
| W = 1 (A, `99e39e1`) | 131,4 (131,1–131,8) | — | 129,9 (binario B de T1, mismo código) | 13,2 | 73,0 | 33,8 | 11,4 |
| W = 1 (B, `d7d653e`) | 132,6 (130,3–133,6) | — | — | 11,4 | 74,5 | 33,6 | 11,5 |
| W = 4 | 47,9 (47,3–53,1) | 1 | 52,4 | 11,0 | 22,7 | 11,1 | 3,0 |
| W = 8 | 36,7 (34,6–37,1) | 1,31 | 41,6 | 13,5 | 13,5 | 7,7 | 1,9 |
| W = 16 | 31,4 (30,7–31,7) | 1,53 | 38,8 | 13,4 | 10,9 | 5,8 | 1,5 |

| Config. | Majadas total | ×vs W = 4 | T1 total | Partición raíz | Jerarquía | Bloques | Validación |
| --- | --- | --- | --- | --- | --- | --- | --- |
| W = 4 | 300,9 | 1 | 323,7 | 49,7 | 166,4 | 70,1 | 14,7 |
| W = 8 | 224,6 (222,1–231,1) | 1,34 | 273,0 | 50,9 | 110,0 | 53,7 | 9,7 |
| W = 16 | 215,3 | 1,40 | 257,0 | 56,0 | 97,4 | 54,1 | 7,6 |

Las etapas son medianas por columna (no tienen por qué sumar el total de la mediana). Totales: `conversion_elapsed_ms`
del informe.

## W = 1 emparejado (Belvedere, `real_dataset_bench convert-profile … 1 4`)

| Ronda | 1 | 2 | 3 | 4 | 5 |
| --- | --- | --- | --- | --- | --- |
| Primero | A | B | A | B | A |
| A (s) | 131,2 | 131,1 | 131,8 | 131,6 | 131,4 |
| B (s) | 132,6 | 130,6 | 133,6 | 130,3 | 132,9 |
| B frente a A | +1,0 % | −0,4 % | +1,4 % | −1,0 % | +1,1 % |

## Lo que no se midió

- Concurrencia de la jerarquía y ruta crítica con nodos partidos (el evento de nodo no separa los rangos; ver
  Condiciones).
- Dispersión en Majadas W = 4 y W = 16 (1 ejecución); W = 2, 3, 6; tmpfs.
- Rangos de raíz 8 con la partición activa (el spike lo midió sin ella; candidato de la tarea 5).
- Presupuestos de memoria ajustados en datos reales (cubiertos solo por tests unitarios).
- Navegador (tareas 3–5): la partición solo existe en el ejecutor nativo.
