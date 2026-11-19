# OQ-09: orden Morton de los puntos dentro de cada bloque — 14-11-2026

**Estado: medido; la decisión está en el [ADR 0041](../../adr/0041-orden-de-filas-libre-por-bloque.md).** En Belvedere v7.5, escribir los puntos de cada
bloque en orden Morton cuando eso reduce su geometría (orden **híbrido**) deja el `.oct` un **35 %**
más pequeño (815 → 528 MB) y descarga los mismos bytes por vista o menos. Leer en remoto tarda lo mismo
a 4 px y un ~6 % más a 1 px (dentro de la variación entre rondas); la etapa de bloques de la conversión
tarda el doble (una ejecución). Ordenar **todos** los bloques en Morton acelera la carga remota un
16–24 % porque las celdas se construyen antes, pero descarga un 34–49 % más por vista. La causa de esa construcción más rápida no
se ha encontrado. La estimación anterior de un 0,63 % ([requisitos](../../requirements.md#catálogo-funcional-o-técnico-duro-o-blando-aceptado-19-11-2026)) solo contaba quitar el índice de filas; el
ahorro real viene de que la codificación por diferencias comprime mejor los puntos ordenados.

## Variantes

Las tres contienen los mismos puntos: un resumen de posición, color y atributos que no depende del
orden coincide en los 3.941 bloques. `viewer_convert verify` da OK en la híbrida.

| Variante | Cómo se escribe | Códecs de geometría |
| --- | --- | --- |
| Fuente (actual) | Orden de la fuente | 2.770 ocupación compacta, 1.169 diferencias, 2 sin comprimir |
| Todo Morton (`OQ09_MORTON=1`) | Cada bloque ordenado por código Morton | 3.939 diferencias, 2 sin comprimir |
| Híbrida (`OQ09_MORTON=best`) | Cada bloque en el orden cuya geometría ocupa menos | 3.164 diferencias en Morton, 775 diferencias en orden de la fuente, 2 sin comprimir |

El códec de ocupación sin índice de filas (bandera 0x80) no gana en ningún bloque.

## Tamaño

| Variante | `.oct` | `.octattrs` | Geometría de los niveles 0–5 |
| --- | --- | --- | --- |
| Fuente | 815.312.074 B | 896,3 MB | 26,5 MB |
| Todo Morton | 574.558.611 B (−29,5 %) | 889,1 MB | 36,7 MB (+39 %) |
| Híbrida | **528.373.180 B (−35,2 %)** | 889,6 MB | **23,2 MB (−12,5 %)** |

Con todo Morton, los bloques que eran de diferencias en el orden de la fuente (más dispersos; todos los de
los niveles 0–5 salvo ~38) crecen un 52 %, y los que eran de ocupación (densos, casi todos del nivel 6 o
más) bajan un 39 %. Los niveles 0–5 son los primeros que se leen en una vista.

## Velocidad

**Lectura remota.** `probe-orbit-load.mjs --remote --rest-first --cells=1` con RED-REF (100 Mbit/s,
50 ms de RTT), pose encuadrada, 3 rondas intercaladas y el mismo wasm para las tres variantes. Se
muestra la mediana.

| Variante | 4 px: objetivo / bytes / construcción | 1 px: objetivo / bytes / construcción |
| --- | --- | --- |
| Fuente | 1.600 ms / 3,09 MB / 1.068 ms | 4.127 ms / 21,2 MB / 5.720 ms |
| Todo Morton | **1.340 ms** / 4,59 MB / **753 ms** | **3.125 ms** / 28,4 MB / **3.879 ms** |
| Híbrida | 1.593 ms / 3,08 MB / 1.244 ms | 4.392 ms / **18,5 MB** / 5.997 ms |

- **Construcción:** suma de los pasos de construcción de celdas del worker. Con todo Morton, el peor
  paso también baja (mediana 60 frente a 126 ms a 4 px). Los bytes de celdas y los nodos construidos son
  parecidos en las tres variantes.
- **Decodificación en wasm:** igual a 4 px (~150–170 ms) y un ~25 % mayor con todo Morton a 1 px
  (994 frente a 790 ms).
- **Benchmark nativo** (`cells_bench`, pose `py`, 1 px, 562 nodos, 2 ejecuciones): p95 de construcción
  ~28 ms con todo Morton, ~47–57 ms con la fuente y ~41–51 ms con la híbrida. En `bench/` solo queda la
  última ejecución de cada variante; la de la fuente es una con el orden en el visor activado (40,9 ms).

**Ordenar en el visor no lo reproduce.** Ordenar por Morton cada bloque al guardarlo en la caché del
visor, con el fichero en orden de la fuente (`explore/oq09-viewer-sort`, 3 ejecuciones; cifras del
informe del experimento, sin JSON guardado):

| Caso | Mediana | p95 | Coste del orden |
| --- | --- | --- | --- |
| Fuente sin ordenar | 8,9–10,0 ms | 43–50 ms | — |
| Fuente ordenada en el visor | 9,0 ms | 41–43 ms | ~630 ms en 562 bloques |
| Fichero todo Morton | 7,8–8,4 ms | 27–37 ms | — |

**Escritura.** Conversión de Belvedere en perfil `perf` con la máquina en reposo y ejecuciones
intercaladas:

| Variante | Etapa de bloques | Conversión total | Ejecuciones |
| --- | --- | --- | --- |
| Fuente | 10,7 s (10,6–11,8) | 47,5 s | 5 |
| Todo Morton | 14,2 s (13,4–16,0) | 49,8 s | 5 |
| Híbrida | 20,1 s | — | 1, del informe del experimento (el log no guarda tiempos); prepara cada bloque dos veces y escribe una línea por bloque |

**Decodificación nativa** de todos los bloques, mediana de 3 por invocación y 2 invocaciones: geometría
y color 3,73–3,76 s con la fuente y 4,46–4,52 s con todo Morton (+19–21 %); atributos iguales (~5,3 s).

## Límites

- Un único dataset (Belvedere), un equipo y una red simulada de 100 Mbit/s. Con una red más lenta,
  los bytes de más de todo Morton pesan más: ~1,5 MB a 4 px son ~1,2 s a 10 Mbit/s.
- No se sabe por qué el fichero todo Morton construye las celdas más rápido. El orden Morton calculado
  en el visor (normalizado a los límites del bloque) no es el del fichero (coordenadas cuantizadas): no
  se ha descartado que esa diferencia importe.
- La escritura híbrida se midió una sola vez y con instrumentación por bloque.
- Las cifras de las primeras medidas de esta pregunta, tomadas con la máquina ocupada, se descartaron.
- `remote-2way/` (primera comparación) usó otro wasm que `remote/`; las cifras del texto son de `remote/`.

## Identidad y reproducción

- Código (sin integrar): `explore/oq09-morton` (`832b578`, `1005c53`, en el worktree del agente) y
  `explore/oq09-remote` (sobre `feat/r3-v75` `20a7078`: `effa381`, `4178f30`, `e2c362d`, `3775a6f`);
  orden en el visor en `explore/oq09-viewer-sort` (`9cb48d9`). El SHA-256 del wasm está en cada JSON.
  Entorno: el de [T11](data/r3-t11-smoke-13-11-2026/identity.txt).
- Conversión: `OQ09_MORTON=best target/perf/viewer_convert data/datasets/belv_2009_digital-aerial_pcd_copc.laz SALIDA.oct --format-version v7.5-pre`.
- Tamaños por bloque: `cargo run --offline --profile perf -p viewer_format --example oq09_block_sizes -- FICHERO.oct`.
- Datos en [data/oq09-morton-order-14-11-2026](data/oq09-morton-order-14-11-2026/): `remote/` (tres
  variantes, `VARIANTE-PX-nRONDA.json`), `remote-2way/` (primera comparación fuente/Morton),
  `timing/` (conversiones y decodificación), `blocks/` (TSV por bloque) y `bench/` (última ejecución
  de `cells_bench` por variante).
