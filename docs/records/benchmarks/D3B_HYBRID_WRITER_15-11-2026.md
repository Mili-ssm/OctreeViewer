# D3b: escritor híbrido de orden de filas detrás de una opción — 15-11-2026

**Estado: implementado y medido; cumple todos los criterios del plan.** Con `--row-order hybrid`, los
bloques que en orden de la fuente ganan con la ocupación compacta se reescriben ordenados por Morton y
codificados por diferencias cuando eso los reduce ([ADR 0041](../../adr/0041-orden-de-filas-libre-por-bloque.md),
[ADR 0042](../../adr/0042-escritor-hibrido-en-la-identidad-final-de-v7.md)). En Belvedere, el `.oct` baja
de 815,3 a 530,96 MB (−34,9 %) sin coste medible en el tiempo total de conversión. En Majadas (escáner
terrestre) apenas cambia: el orden de la fuente ya es local y casi ningún bloque usa la ocupación.

## Qué hace

- Opción `RowOrder` del escritor (`Source` por defecto, `Hybrid`), en el CLI (`--row-order`), en la
  conversión wasm (`rowOrder`) y en `ConvertOptions`; solo se acepta con salidas V7.
- Regla: se codifica como siempre; si gana la ocupación compacta, se ordenan las filas por (clave Morton de
  96 bits, fila) y se prueba la codificación por diferencias; se queda la más pequeña. Colores y atributos se
  permutan igual. No cambian el decodificador ni la identidad del formato.
- El manifiesto de checkpoint LAS (versión 6) y el diario EPT guardan el orden: reanudar con el otro da
  `Mismatch`.
- La paridad nativo/wasm incluye casos híbridos: mismo SHA en los dos lados y distinto del orden de la
  fuente. `scripts/check.sh` pasa los 24 pasos.

## Medidas

Conversión en perfil `perf`, 3 rondas intercaladas (el orden de las dos opciones alterna por ronda),
mediana y rango. `/usr/bin/time` da el tiempo total y el RSS pico.

| Caso | `.oct` | Etapa de bloques | Tiempo total | RSS pico |
| --- | --- | --- | --- | --- |
| Belvedere, fuente | 815.312.074 B | 9,9 s (8,8–16,1) | 46,2 s (37,2–57,2) | 86 MiB (85–87) |
| Belvedere, híbrido | **530.962.031 B** | 11,5 s (11,2–14,4) | 45,5 s (41,4–59,5) | 90 MiB (86–91) |
| Majadas, fuente | 2.120.805.496 B | 87,5 s (73,5–90,6) | 322,8 s (298,0–326,8) | 216 MiB (214–219) |
| Majadas, híbrido | 2.120.782.616 B | 65,9 s (63,3–66,7) | 303,6 s (293,3–332,8) | 216 MiB (213–219) |
| Belvedere, bloques de 1 M de puntos (v7.3-pre, 1 ejecución) | 980,9 → 979,3 MB | 6,5 → 6,4 s | 37,6 → 23,3 s | 91 → 88 MiB |

| Criterio del plan | Resultado |
| --- | --- |
| Belvedere cambia 2.770 bloques | 2.770, los que ganaban con la ocupación compacta |
| `.oct` de Belvedere ≤ 531,5 MB | 530,96 MB, igual al byte a la estimación del pre-mortem |
| Contenido igual al de la fuente | Resumen por bloque de posición, color y atributos idéntico en Belvedere y en Majadas |
| Etapa de bloques ≤ +25 % | Belvedere +16 %; Majadas más rápida (ver límites) |
| Tiempo total ≤ +5 % | Belvedere −1,5 %; Majadas −5,9 % |
| RSS pico ≤ +5 % | Belvedere +4,7 %; Majadas igual |
| Majadas ≤ 2 % sobre su óptimo | +0,26 %: probando los dos órdenes en cada bloque (escritor experimental `OQ09_MORTON=best`) el `.oct` queda en 2.115.385.082 B |

En Majadas solo 1 de 24.091 bloques gana con la ocupación compacta, y solo ese cambia. Ni el óptimo por
bloque baja más de un 0,26 % respecto al orden de la fuente.

## Límites

- 3 rondas por caso y con la máquina compartiendo carga (`loadavg` ~5 durante las conversiones, que son
  multihilo): los rangos de tiempo son anchos. Que la etapa de bloques de Majadas salga más rápida con la
  opción híbrida, que hace más trabajo, apunta a ruido o a efectos de caché del disco, no a una mejora.
- El RSS pico de Belvedere queda a 0,3 puntos del límite del +5 %.
- El caso de memoria con bloques de 1 M de puntos es una sola ejecución.
- Dos datasets. Los ficheros híbridos solo sirven para medir hasta el congelado (ADR 0042).

## Identidad y reproducción

- Código: rama `explore/d3b-hybrid-writer` (`68fdc4e`, `9999b4d`, `b7b3b03`, `af64e3f`, `cff20e0`),
  integrada en `feat/r3-v75`. Comprobaciones de contenido, de bloques y del óptimo con los ejemplos de
  `explore/oq09-remote` (`3775a6f`): `oq09_content_hash`, `oq09_block_sizes` y `OQ09_MORTON=best`. Entorno:
  el de [T11](data/r3-t11-smoke-13-11-2026/identity.txt).
- `target/perf/viewer_convert ENTRADA.laz SALIDA.oct --format-version v7.5-pre --row-order hybrid`
- Datos en [data/d3b-hybrid-writer-15-11-2026](data/d3b-hybrid-writer-15-11-2026/): `runs.jsonl` (una línea
  por conversión) y tamaños y códecs por bloque de las dos opciones (`belvedere-*.tsv`, `majadas-*.tsv.gz`).
