# D3b: escritor híbrido de orden de filas detrás de una opción — 15-11-2026

**Estado: implementado y medido; cumple los criterios del plan tras dos optimizaciones.** Con
`--row-order hybrid`, los bloques que en orden de la fuente ganan con la ocupación compacta se reescriben
ordenados por Morton y codificados por diferencias cuando eso los reduce
([ADR 0041](../../adr/0041-orden-de-filas-libre-por-bloque.md),
[ADR 0042](../../adr/0042-escritor-hibrido-en-la-identidad-final-de-v7.md)). En Belvedere, el `.oct` baja
de 815,3 a 530,96 MB (−34,9 %) con un +1,3 % de tiempo total de conversión y sin más memoria. En Majadas
(escáner terrestre) apenas cambia: el orden de la fuente ya es local y casi ningún bloque usa la ocupación.

## Qué hace

- Opción `RowOrder` del escritor (`Source` por defecto, `Hybrid`), en el CLI (`--row-order`), en la
  conversión wasm (`rowOrder`) y en `ConvertOptions`; solo se acepta con salidas V7, y la ruta híbrida solo
  actúa sobre capas V7.
- Regla: se codifica como siempre; si gana la ocupación compacta, se ordenan las filas por (clave Morton de
  96 bits, fila) y se prueba la codificación por diferencias; se queda la más pequeña. Colores y atributos se
  permutan igual. No cambian el decodificador ni la identidad del formato.
- La clave se calcula con desplazamientos y máscaras de bits, y el orden con un radix estable que solo
  recorre los bits en que difieren las claves del bloque; la codificación lee las filas a través de la
  permutación, sin copia ordenada.
- El manifiesto de checkpoint LAS (versión 6) y el diario EPT guardan el orden: reanudar con el otro da
  `Mismatch`. La paridad nativo/wasm incluye casos híbridos: mismo SHA en los dos lados y distinto del orden
  de la fuente.

## Medidas

Conversión en perfil `perf` con `/usr/bin/time` (tiempo total y RSS pico en KiB), rondas intercaladas con
el orden de las dos opciones alternado por ronda. Mediana y rango; los porcentajes se calculan sobre las
medianas en KiB y segundos, sin redondear.

**Belvedere, versión final (`aba5286`, 5 rondas):**

| Opción | `.oct` | Etapa de bloques | Tiempo total | RSS pico |
| --- | --- | --- | --- | --- |
| Fuente | 815.312.074 B | 9,41 s (8,97–10,27) | 40,84 s (38,14–42,05) | 92.988 KiB (91.496–95.060) |
| Híbrido | **530.962.031 B** | 10,49 s (10,09–10,90) | 41,39 s (41,19–41,72) | 91.040 KiB (83.912–95.560) |
| Diferencia | **−34,9 %** | **+11,5 %** | **+1,3 %** | **−2,1 %** |

**Cómo se llegó** (Belvedere, misma tanda de 5 rondas por versión; diferencias del híbrido frente a la
fuente):

| Versión | Etapa de bloques | Tiempo total | RSS pico |
| --- | --- | --- | --- |
| Primera (`cff20e0`, 3 rondas, una con la jerarquía anómala) | +16,2 % | −1,6 % | +5,45 % |
| Sin copia ordenada (`59415f9`) | +29,2 % | +9,0 % | +0,3 % |
| Clave con máscaras de bits (`bbd8ead`) | +11,9 % | +6,4 % | +0,1 % |
| Radix sobre los bits que varían (`aba5286`) | +11,5 % | +1,3 % | −2,1 % |

- La clave con un bucle de 96 pasos por punto era el coste principal; con máscaras de bits la etapa de bloques
  pasa de +29 % a +12 %.
- El radix apenas mueve la etapa de bloques (dentro del ruido). Se mantiene porque es el código medido, está
  probado contra la ordenación por (clave, fila) y escala mejor con bloques grandes.
- Entre `bbd8ead` y `aba5286`, la mayor parte del cambio del tiempo total está en la planificación del índice,
  que se ejecuta antes que cualquier código del orden híbrido: es ruido de la máquina.

**Majadas** (`cff20e0`, 3 rondas; las optimizaciones posteriores solo aceleran la ruta híbrida y no cambian
los bytes):

| Opción | `.oct` | Etapa de bloques | Tiempo total | RSS pico |
| --- | --- | --- | --- | --- |
| Fuente | 2.120.805.496 B | 87,5 s (73,5–90,6) | 322,8 s (298,0–326,8) | 221.652 KiB |
| Híbrido | 2.120.782.616 B | 65,9 s (63,3–66,7) | 303,6 s (293,3–332,8) | 221.568 KiB |

**Criterios del plan:**

| Criterio | Resultado |
| --- | --- |
| Belvedere cambia 2.770 bloques | 2.770, los que ganaban con la ocupación compacta |
| `.oct` de Belvedere ≤ 531,5 MB | 530.962.031 B, la estimación del pre-mortem (529.778.249 B de geometría más 1.183.782 B fijos) |
| Contenido igual al de la fuente | Resumen por bloque de posición, color y atributos idéntico en Belvedere y en Majadas |
| Etapa de bloques ≤ +25 % | Belvedere +11,5 % |
| Tiempo total ≤ +5 % | Belvedere +1,3 %; Majadas −5,9 % |
| RSS pico ≤ +5 % | Belvedere −2,1 %; Majadas igual |
| Majadas ≤ 2 % sobre su óptimo | +0,26 %: probando los dos órdenes en cada bloque (escritor experimental `OQ09_MORTON=best`) el `.oct` queda en 2.115.385.082 B |

En Majadas solo 1 de 24.091 bloques gana con la ocupación compacta, y solo ese cambia.

## Límites

- 5 rondas en Belvedere y 3 en Majadas, con la máquina a `loadavg` ~5 (las conversiones son multihilo): los
  rangos son anchos. Que la etapa de bloques de Majadas salga más rápida con la opción híbrida, que hace más
  trabajo, apunta a ruido o a la caché del disco.
- Majadas se midió antes de las dos optimizaciones; sus bytes no cambian con ellas.
- El caso con bloques de 1 M de puntos (v7.3-pre, una ejecución sobre `cff20e0`) solo mira la memoria: el
  RSS pico pasa de 91 a 88 MiB. Su tiempo total (37,6 → 23,3 s) es ruido de una sola ejecución.
- Dos datasets. Los ficheros híbridos solo sirven para medir hasta el congelado (ADR 0042).

## Identidad y reproducción

- Código: rama `explore/d3b-hybrid-writer` (`68fdc4e`, `9999b4d`, `b7b3b03`, `af64e3f`, `cff20e0`,
  `59415f9`, `bbd8ead`, `aba5286`), integrada en `feat/r3-v75`. Comprobaciones de contenido, de bloques y del
  óptimo con los ejemplos de `explore/oq09-remote` (`3775a6f`): `oq09_content_hash`, `oq09_block_sizes` y
  `OQ09_MORTON=best`. Entorno: el de [T11](data/r3-t11-smoke-13-11-2026/identity.txt).
- `target/perf/viewer_convert ENTRADA.laz SALIDA.oct --format-version v7.5-pre --row-order hybrid`
- Datos en [data/d3b-hybrid-writer-15-11-2026](data/d3b-hybrid-writer-15-11-2026/): una línea por conversión
  en `runs-cff20e0.jsonl` (Belvedere, Majadas y el caso de 1 M) y `runs-belvedere-<commit>.jsonl` (las tres
  tandas posteriores), y tamaños y códecs por bloque de las dos opciones (`belvedere-*.tsv`,
  `majadas-*.tsv.gz`).
