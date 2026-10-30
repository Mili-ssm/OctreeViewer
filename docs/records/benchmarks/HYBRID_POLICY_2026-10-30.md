# Política de volcado y granularidad de segmentos del almacén híbrido (30-10-2026)

> Registro fechado, resultado de un modelo (no de una medición de navegador). Cifras válidas para las trazas nativas de
> [S10](HYBRID_SPOOL_RAM_2026-10-29.md) (Belvedere con 2 y 8 divisiones, Majadas con 2; 1 worker, perfil `perf`; no se
> regeneraron). Contexto: decisión 5 del [plan de la etapa 4](../../plans/conversor-web-paralelo-etapa4.md) (spools en
> segmentos de JS, volcado síncrono a páginas de zonas OPFS, lista de páginas libres, zonas vacías truncadas). Datos:
> [JSON](data/hybrid-policy-2026-10-30.json); simulador
> [`model_hybrid_zones.py`](../../../crates/viewer_convert/benchmarks/model_hybrid_zones.py) (reutiliza `parse_log` y `source_bytes`
> del modelo anterior) y su [test](../../../crates/viewer_convert/benchmarks/test_model_hybrid_zones.py) con el árbol manual de 5 nodos.

## Conclusión

**Segmentos progresivos (32 KiB → 1 MiB) recortados al cerrar el spool, volcado de spools enteros con pistas del núcleo (retenido y orden de consumo) y páginas de 256 KiB.** Con páginas de 1 MiB el OPFS ocupa hasta 2,0 × los bytes volcados vivos (24 de 126 celdas superan el criterio de 1,5 ×); con 64 o 256 KiB, ninguna celda (máximo 1,03 y 1,18). El anclaje del lector no hace imposible el presupuesto en ninguna de las 270 celdas.

## Método

- **Eventos.** Semántica de la variante c de S10: el trabajo crea los spools de sus hijos al empezar, su LOD al terminar y libera su entrada al terminar; hojas y LOD se retienen hasta la fase de bloques. A diferencia de S10 (hijos contados enteros al empezar), los spools crecen en volcados de 32 KiB (el tamaño del núcleo) intercalados en proporción a su tamaño final, así que el pico es una cota menos holgada. N = 1, orden **primero en profundidad** (el de `process_child_tasks`, recursión serial nativa); S10 usó LIFO, que da el mismo pico de payload (4,35 GiB en Belvedere/2). FIFO, LIFO y «mayor primero» se guardan como sensibilidad.
- **RAM.** Segmentos por spool: fijos de 1 MiB, progresivos (32 KiB duplicando hasta 1 MiB), fijos de 32 KiB, y la variante extra *progresivo con recorte* (al cerrar, el último segmento se ajusta al payload: `ArrayBuffer.prototype.transfer(n)` o copia). RAM retenida = suma de capacidades; desperdicio = retenida − payload.
- **Volcado.** Si retenida > presupuesto B se vuelcan spools enteros; uno abierto sigue añadiendo a páginas. Antes de añadir un segmento no hay reserva: el pico se mide tras cumplir el presupuesto. Políticas: **`oldest_closed`** (cerrados, el más antiguo primero; abiertos al final, el mayor primero); **`hinted`** (hojas/LOD retenidos primero, luego el resto de cerrados por antigüedad); **`p3`** (orden de S10: retenidos del mayor al menor, luego entradas en cola consumidas más tarde primero). **Anclaje (A):** el spool de entrada del trabajo en curso nunca se vuelca (su lector está particionándolo); si se volcó antes de empezar, el trabajo lo lee desde las páginas (relectura). La fase de bloques solo lee y no añade RAM.
- **Páginas.** Lista de libres (se reutiliza la de índice más bajo, así las zonas se llenan en orden), zonas de 1 GiB, zona vacía truncada. Un spool volcado ocupa ⌈bytes/página⌉ páginas (la última, a medias). Un spool leído de las páginas no vuelve a RAM. Variantes de tamaño (B): páginas de 64 KiB, 256 KiB y 1 MiB; spools menores que una página volcados (`spill`), mantenidos en RAM (`keep`) o empaquetados en páginas compartidas (`pack`, empaquetado ideal con compactación perfecta, es decir optimista).
- **Rejilla.** 3 trazas × B = 2 y 4 GiB × 3 políticas × 4 segmentos (páginas de 1 MiB), más páginas × `keep/pack/spill` × políticas con segmento recortado y los cuatro órdenes: 270 celdas con presupuesto.

## Resultados

Todo en GiB (coma decimal en el texto, punto en el JSON). Orden primero en profundidad.

**Segmentos sin volcado (RAM retenida frente a payload en el pico):**

| Traza | Payload vivo | 1 MiB fijo | Progresivo | 32 KiB fijo | Progresivo + recorte |
| --- | ---: | ---: | ---: | ---: | ---: |
| Belvedere/2 | 4,35 | 4,66 (+0,31) | 4,53 (+0,18) | 4,36 (+0,01) | 4,35 (+0,00) |
| Belvedere/8 | 3,21 | **6,24 (+3,10)** | 4,71 (+1,57) | 3,27 (+0,07) | 3,25 (+0,05) |
| Majadas/2 | 17,54 | 20,05 (+2,51) | 18,82 (+1,28) | 17,61 (+0,07) | 17,54 (+0,00) |

Con 8 divisiones hay ≈ 5.000 spools menores de 1 MiB: un segmento fijo de 1 MiB casi duplica la RAM (revisión 3 del plan). Con presupuesto, el desperdicio máximo en RAM (progresivo recortado) es ≤ 0,05 GiB; el progresivo sin recortar llega a 0,86 GiB con B = 4 en Belvedere/8, y el fijo de 1 MiB a 2,0 GiB.

**Políticas (segmento progresivo recortado, páginas de 1 MiB, `spill`).** Volcado total, relectura de entradas (hiladas), relectura en la fase de bloques (inevitable para lo retenido), OPFS (páginas × tamaño) frente al pico de bytes volcados vivos, zonas Z:

| Traza | Pol. | B | Volcado | Relect. entrada | Relect. bloques | OPFS pico / volcado vivo | Z | Volcados (spools) |
| --- | --- | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| Belvedere/2 | oldest_closed | 2 | 3,78 | 2,48 | 1,30 | 2,48 / 2,48 | 3 | 537 |
| Belvedere/2 | hinted | 2 | 3,78 | 2,48 | 1,30 | 2,48 / 2,48 | 3 | 537 |
| Belvedere/2 | p3 | 2 | 3,78 | 2,48 | 1,30 | 2,48 / 2,48 | 3 | 261 |
| Belvedere/2 | oldest_closed | 4 | 1,23 | 1,23 | 0,00 | 1,23 / 1,23 | 2 | 1 |
| Belvedere/2 | hinted | 4 | 1,23 | 1,23 | 0,00 | 1,23 / 1,23 | 2 | 2 |
| Belvedere/2 | **p3** | 4 | **0,67** | **0,00** | 0,67 | 0,67 / 0,67 | 1 | 3 |
| Belvedere/8 | oldest_closed | 2 | 4,22 | 3,08 | 1,13 | 2,25 / 1,28 | 3 | 1.810 |
| Belvedere/8 | hinted | 2 | 2,41 | 1,22 | 1,20 | 2,35 / 1,25 | 3 | 1.851 |
| Belvedere/8 | **p3** | 2 | **2,36** | **1,22** | 1,14 | 1,88 / 1,26 | 2 | 974 |
| Belvedere/8 | todas | 4 | 0 | 0 | 0 | sin volcado (RAM 3,25) | 0 | 0 |
| Majadas/2 | oldest_closed | 2 / 4 | 60,35 / 44,84 | 49,30 / 35,79 | 11,05 / 9,05 | 20,04 / 17,54 | 21 | 5.478 / 4.464 |
| Majadas/2 | p3 | 2 / 4 | 58,79 / 41,24 | 47,74 / 32,19 | 11,05 / 9,05 | 20,04 / 17,54 | 21 | 5.385 / 4.103 |

- **¿Hace falta una pista?** Sin pistas (`oldest_closed`) es viable (todas las celdas cumplen el presupuesto), pero en Belvedere/2 con 4 GiB vuelca 1,8 × más y relee 1,23 GiB de entradas, y en Belvedere/8 con 2 GiB vuelca 1,8 × más (4,22 frente a 2,36) y relee 2,5 × más. La causa: con recursión en profundidad el spool cerrado más antiguo es un hermano cuyo consumo está *cerca*, mientras que `p3` elige el consumido más tarde. `hinted` (solo «retenido») no basta en recursión en profundidad (iguala a `oldest_closed` en Belvedere/2). La ganancia de `p3` en Majadas es ≈ 3–8 %.
- **Majadas** (fuera del objetivo de la etapa, solo Belvedere): el presupuesto apenas ayuda, porque la raíz escribe 12,77 GiB y casi todo va a las páginas; se vuelcan 41–60 GiB y se releen 32–49 GiB (3–4 pasadas por dato). El OPFS pico ≈ el pico vivo completo (17,5–20 GiB), **no** pico − B.
- **Sensibilidad al orden** (progresivo recortado, `oldest_closed`, Belvedere/2, B = 2): FIFO y «mayor primero» releen 11,8 y 11,0 GiB frente a 3,8 con profundidad primero. El modelo depende de que el planificador del núcleo siga siendo recursivo en profundidad en N = 1.

## A. Anclaje del lector

En 270 celdas el excedente sobre B es 0 B (`over_budget_bytes`): anclar la entrada del trabajo en curso nunca obliga a superar el presupuesto, porque siempre queda algún candidato (spools abiertos de hijos o retenidos). Con `keep` y 1 MiB en una exploración previa con segmento progresivo sin recortar apareció un caso de 0,36 GiB de exceso en Belvedere/8, B = 2 (no archivado: lo sustituyó la rejilla final); el test manual `keep` muestra el mecanismo (150 B retenidos con B = 100). Con N > 1 hay un lector por worker y no se modeló.

## B. Granularidad de página y tormentas de volcados

Hinted, segmento recortado, B = 2 (OPFS pico / bytes volcados vivos; volcados menores que una página y RAM que liberan):

| Traza | Página | `spill` | `keep` | `pack` | Volcados < página (RAM liberada / total) con `spill` |
| --- | ---: | --- | --- | --- | --- |
| Belvedere/8 | 1 MiB | 2,35 / 1,25 = **1,87** | 2,01 / 1,28 = 1,56 | 1,64 / 1,25 = 1,31 | 1.291 de 1.851 (0,54 de 2,16 GiB) |
| Belvedere/8 | 256 KiB | 1,48 / 1,25 = 1,18 | 1,41 = 1,12 | 1,40 = 1,12 | 495 (0,04 de 2,16) |
| Belvedere/8 | 64 KiB | 1,30 / 1,25 = 1,03 | 1,29 = 1,03 | 1,29 = 1,03 | 251 (0,00) |
| Belvedere/2 | 1 MiB | 2,48 / 2,48 = 1,00 | 1,00 | 1,00 | 190 de 537 (0,03 de 3,19) |
| Majadas/2 | 1 MiB | 20,04 / 17,54 = 1,14 | 18,50 / 17,33 = 1,07 | 18,70 = 1,07 | 2.189 de 5.477 (0,28 de 38,3) |
| Majadas/2 | 256 KiB | 1,04 | 1,02 | 1,02 | 1.783 (0,03) |

- **Tormenta de volcados.** Con páginas de 1 MiB, Belvedere/8 y B = 2 vuelca 1.291 spools menores que una página (70 % de los volcados) que solo liberan 0,54 de 2,16 GiB (25 %) y cuestan hasta una página de OPFS cada uno. Con 256 KiB liberan 2 %. No hay una avalancha de volcados diminutos en RAM libre: ninguno libera más de 0,05 GiB por tamaño.
- **Mantener o empaquetar.** `keep` no arregla 1 MiB (1,56) y rompe la regla de simplicidad; `pack` (ideal) llega a 1,31 y exige compactar o gestionar extents. Reducir la página resuelve el problema sin ninguna de las dos. Con 64 KiB ninguna celda pasa de 1,03.
- **Coste no modelado:** más páginas son más llamadas `write`; si los índices libres son contiguos se pueden agrupar. Lo medirá la tarea 1 con páginas de 1 MiB; falta medir 256 KiB.

## C. Zonas

Con Z = ⌈2 × pico OPFS modelado / 1 GiB⌉ + 1, **ninguna de las 270 celdas se queda sin zonas** (0 casos; Z necesarias 0–21, la fórmula da el doble o más).

## D. Criterio de parada (OPFS > 1,5 × bytes volcados vivos)

| Páginas | `spill` | `keep` | `pack` | Máx. ratio |
| ---: | ---: | ---: | ---: | --- |
| 1 MiB | **24 / 126** | 1 / 18 | 0 / 18 | 1,98 (spill), 1,56, 1,48 |
| 256 KiB | 0 / 18 | 0 / 18 | 0 / 18 | 1,18 |
| 64 KiB | 0 / 18 | 0 / 18 | 0 / 18 | 1,03 |

Las 24 celdas están en Belvedere/8 (B = 2 y, con segmentos sin recortar, B = 4), todas con páginas de 1 MiB. La decisión 5 fija páginas de 1 MiB; este registro propone cambiarlas (ver abajo) y debe decidirlo el propietario.

## Estimación previa a la conversión

Datos conocidos antes de convertir: puntos P, bytes por registro r (Belvedere 41 B, Majadas 33 B), divisiones d, puntos por bloque p (262.144), presupuesto B. Con S = P · r:

| Magnitud | Fórmula | Simulado (máx.) | Estimado |
| --- | --- | --- | --- |
| Pico de payload vivo | `min(2·S, 1,1 · S · (1 + 2,5 / d²))` | 4,35 / 3,21 / 17,54 | 5,58 / 3,56 / 22,82 (×1,28 / 1,11 / 1,30) |
| Nodos (spools) | `⌈2,4 · d · P / p⌉` | 1.343 / 5.140 / 6.259 | 1.496 / 5.983 / 7.608 |
| Pico OPFS (si `1,03 · vivo > B`) | `vivo + nodos · página / 2` | 2,5 / 1,5 / 18,2 (B = 2, 256 KiB) | 5,76 / 4,30 / 23,75 |
| Zonas a abrir | `⌈2 · OPFS_est / 1 GiB⌉ + 1` | 3 / 2 / 19 | 13 / 10 / 49 |
| RAM sin OPFS (Chrome `file://`) | `1,03 · vivo + 0,6 · S` | 5,8 / 4,7 / – (RAM del pico + salida real 1,48 GiB) | 7,61 / 5,54 / 31,2 |

- Sustento de la cota: un punto vive en un solo spool salvo mientras corre el trabajo de su nodo, así que el pico es ≤ S + entrada máxima + LOD ≤ 2·S. El término `2,5 / d²` sale del ajuste de tres trazas (extras observados 0,40, 0,03 y 0,37 con d = 2, 8, 2); es una cota empírica, no demostrada, y una nube mucho más desigual puede exceder la fracción de la raíz. La relación de salida 0,6 · S proviene de los tamaños medidos (Belvedere 0,47, Majadas 0,55 de S).
- Margen: la estimación supera a la simulación en las 12 combinaciones traza × B × página (×1,3 a ×5,1 en OPFS). Como `OPFS_est` ya lleva ≥ 30 % de margen, `⌈OPFS_est / zona⌉ + 1` también cubre las 12; se propone la de 2 × por coherencia con el modelo y porque abrir un handle vacío es barato (Z = 49 ≪ 664 de S9). El rechazo de `file://` con presupuesto de 4 GB rechaza Belvedere (5,54 y 7,61 GiB), no el LAS pequeño.
- La fórmula supone N = 1, orden en profundidad y segmento recortado. Si no se recorta, sumar el desperdicio de 0,3–1,6 GiB de la tabla de segmentos (Belvedere/8: con B = 4 ya se vuelca).

## Decisión propuesta

(La decide el propietario.)

| Elemento | Propuesta | Motivo |
| --- | --- | --- |
| Segmentos | Progresivos 32 KiB → 1 MiB con recorte al cerrar | Desperdicio ≤ 0,05 GiB; 1 MiB fijo casi duplica la RAM con 8 divisiones |
| Política | `p3`: retenidos primero (mayor primero), luego cerrados por orden de consumo inverso, abiertos al final; entrada del trabajo en curso anclada | −45 % de volcado en Belvedere/2 con 4 GiB y −44 % en Belvedere/8 con 2 GiB frente a sin pistas; sin pistas funciona |
| Pistas del núcleo | Sí, baratas: `retained` y un rango de consumo por spool al crearlo | Sin ellas el planificador en profundidad vuelca el hermano más próximo |
| Página | **256 KiB** (64 KiB si la tarea 1 muestra escrituras baratas) | Cumple el criterio D en todas las celdas; 1 MiB no |
| Z | `⌈2 · OPFS_est / 1 GiB⌉ + 1` sobre la estimación anterior | Cubre las 270 celdas |

## Límites

- Tres trazas, 1 worker, una por caso; Majadas solo con 2 divisiones. N > 1 no se modeló (un lector anclado por worker; el pico crece con N).
- Sin reloj ni coste de E/S: volcado y relectura se miden en bytes, no en tiempo. Majadas relee 32–49 GiB: con Firefox (150–250 MB/s en S9) serían minutos extra.
- El empaquetado supone compactación perfecta; el recorte de segmentos supone `transfer(n)` o una copia que no se cobró. Los volcados abiertos intercalan en proporción al tamaño final, supuesto no medido.
- `hinted` y `p3` son políticas modeladas; el orden exacto del núcleo (primero en profundidad) se tomó de `process_child_tasks`, no de una traza propia.
