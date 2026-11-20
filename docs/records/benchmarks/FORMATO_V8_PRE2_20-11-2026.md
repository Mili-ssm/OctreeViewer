# Formato v8.2-pre: cuatro esquemas de compresión de `.octattrs` — 20-11-2026

**Estado: medido.** Es la tarea 4 del [plan de v8.2-pre](../../plans/formato-v8-pre2.md), el experimento de D3 del
[diseño de la V8](../../plans/formato-v8.md#resoluciones-tras-medir-v81-pre-propietario-20-11-2026). Parte de la
[proyección de la tarea 0](../../plans/formato-v8-pre2.md#codificador) y del
[registro de v8.1-pre](FORMATO_V8_PRE1_20-11-2026.md).

**Conclusión:** frente a `lz4`, los esquemas `cold`, `cold-rgb` y `all` reducen la salida total un **−3,08 / −5,69 /
−9,45 %** en Albuera (igual que la proyección, ≤ 0,07 puntos), 0 / −5,00 / −7,86 % en Belvedere y 0 / −4,43 / −8,73 %
en Majadas. Convertir cuesta como mucho un ~3 % más de tiempo, del orden de la dispersión entre repeticiones de `lz4`
(≤ 2,3 %). En el navegador (Albuera, GPU real, 8 ejecuciones por esquema) **la diferencia entre `lz4` y `all` en
decodificación y en tiempo hasta el objetivo no es medible**: el control A/A (`cold` frente a `lz4` en modo RGB) difiere
tanto o más. Lo mismo al leer una columna fría (`gps_time`) y por red (allí sin control A/A: la diferencia queda dentro
del rango de `lz4`). Los fotogramas p50/p95 son similares en todos los esquemas. No se recomienda esquema: decide el
propietario (D3). Las cifras valen solo para esta revisión, el perfil `release`, estos datasets y este equipo.

## Condiciones

| Dato | Valor |
| --- | --- |
| Binario | `release` de `viewer_convert` compilado desde `4ef57b8` (rama `experiment/formato-v8-pre1`) |
| Equipo | El del [registro de v8.1-pre](FORMATO_V8_PRE1_20-11-2026.md#condiciones) (AMD Ryzen AI 9 365, 30 GiB de RAM, Radeon 880M); salida en `/home` (btrfs) |
| Conversión | `scripts/measure_temp_disk.sh target/release/viewer_convert ENTRADA DIR -- --format-version v8.2-pre --attr-codec ESQUEMA` (copia sin el `rm` final para poder pasar `verify`), una cada vez. Orden: Belvedere `lz4, all, cold, cold-rgb` (1 vez); Albuera y Majadas `lz4, all, cold, cold-rgb, lz4, all` |
| Navegador | `node examples/web-host/probe-orbit-load.mjs albuera-ESQUEMA.oct SALIDA.json --url=http://127.0.0.1:8791/examples/web-host/index.html --attrs --rest-first --px=0.5 --max-load-s=120 --orbit-s=10 --color=rgb\|attribute:gps_time [--remote]`, con el wasm reconstruido de esta rama y `PORT=8791 scripts/serve-demo.sh --no-build`. Chrome headless con adaptador `amd`/`rdna-3`; `mode` = `timing` en las 80 ejecuciones; el modo de color se lee de vuelta de la página y del worker de render en cada una |
| Por qué 0,5 px | En la ejecución piloto, 4 px alcanza el objetivo en 0,8 s con 31 bloques (110 ms de decodificación, nada que medir); 0,5 px carga ~315 bloques y ~4 s de decodificación |
| Orden en el navegador | Por brazo, A = `lz4`, B = `cold`, C = `cold-rgb`, D = `all`: ABCD DCBA BADC CDAB, dos veces; por red ADAD DADA DADA ADAD. RED-REF por defecto: 100 Mbit/s, 50 ms de RTT |
| Datos | [`data/formato-v8-pre2-20-11-2026/`](data/formato-v8-pre2-20-11-2026/): `summary.tsv` y por caso `.phases.txt`, `.tsv`, `.time.txt`, `.sizes.txt`, `.verify.txt`; [`browser/`](data/formato-v8-pre2-20-11-2026/browser/) con un JSON por ejecución y `browser_summary.tsv` |

## Tamaño de la salida

Las 16 conversiones pasan `viewer_convert verify`. Las repeticiones dan los mismos bytes (SHA iguales) y `lz4` en
v8.2-pre ocupa lo mismo que v8.1-pre. `.oct` no cambia de tamaño en ningún esquema (la geometría es siempre LZ4).

| Nube | `lz4` (B) | `cold` | `cold-rgb` | `all` |
| --- | ---: | ---: | ---: | ---: |
| Albuera | 9 362 079 659 | 9 074 062 123 (**−3,08 %**) | 8 828 971 754 (**−5,69 %**) | 8 477 233 294 (**−9,45 %**) |
| Majadas | 4 575 835 569 | igual a `lz4` (0 %) | 4 372 917 137 (−4,43 %) | 4 176 200 649 (−8,73 %) |
| Belvedere | 926 000 077 | igual a `lz4` (0 %) | 879 697 747 (−5,00 %) | 853 198 518 (−7,86 %) |

En Majadas y Belvedere `cold` escribe ficheros idénticos byte a byte a `lz4`: en sus columnas frías zstd nunca resulta
el candidato más pequeño (se deduce de los SHA iguales; no se midió por candidato). No se investigó por qué
(hipótesis: columnas frías constantes por bloque).

## Tiempo de conversión

| Nube | `lz4` (s, pared) | `cold` | `cold-rgb` | `all` |
| --- | --- | --- | --- | --- |
| Albuera | 681,1 · 697,0 (dispersión 2,3 %) | 684,2 (−0,7 %) | 680,0 (−1,3 %) | 703,5 · 694,2 (+1,4 %) |
| Majadas | 326,7 · 328,8 | 333,7 (+1,8 %) | 326,7 (−0,3 %) | 339,9 · 333,8 (+2,8 %) |
| Belvedere | 55,7 | 51,6 | 51,6 | 55,7 |

Porcentajes frente a la media de `lz4`. En Belvedere `cold` produce los mismos bytes que `lz4` y aun así tarda un −7,4 %
(una ejecución de cada uno): el ruido de una sola ejecución allí es de ese orden. El tiempo de CPU de `all` sube +1,8 % (Albuera) y +3,1 % (Majadas). RSS y pico
de temporales quedan en el mismo orden que en v8.1-pre, que se midió una sola vez (217–260 MB de RSS en Albuera y
Majadas; 15,0 GB y 6,5 GB de temporales).

## Carga y render en el navegador (Albuera)

Mediana y mínimo–máximo de 8 ejecuciones (ms). `decodeMs` es la suma de la decodificación de bloques del lector
asíncrono (`asyncBlocks.decodeMs`); «hasta objetivo» es `timeToReachedMs` de la sonda; fotograma es el intervalo de la
órbita tras alcanzar el objetivo.

| Brazo | Esquema | Bloques | `decodeMs` | Hasta objetivo | Fotograma p50 / p95 |
| --- | --- | ---: | ---: | ---: | ---: |
| Local, RGB | `lz4` | 313 | 3758 (3594–4281) | 13 776 (12 521–17 075) | 37,3 / 53,5 |
| | `cold` | 310 | 3668 (3534–4385) | 13 272 (12 768–17 592) | 37,5 / 53,4 |
| | `cold-rgb` | 314 | 3639 (3478–4046) | 12 796 (12 552–16 050) | 36,3 / 53,3 |
| | `all` | 314 | 3740 (3505–4338) | 13 341 (12 542–16 839) | 37,3 / 53,8 |
| Local, `gps_time` | `lz4` | 322 | 3641 (3502–3974) | 13 356 (12 891–15 514) | 36,6 / 53,5 |
| | `cold` | 322 | 3556 (3526–4190) | 13 047 (12 935–17 231) | 36,8 / 52,7 |
| | `cold-rgb` | 324 | 3666 (3507–4009) | 13 454 (12 910–15 712) | 37,1 / 53,7 |
| | `all` | 327 | 3645 (3602–4159) | 13 190 (12 700–16 214) | 36,6 / 53,2 |
| Red, RGB | `lz4` | 271 | 2998 (2876–3397) | 17 483 (17 094–19 162) | 33,6 / 51,8 |
| | `all` | 276 | 3039 (2991–3346) | 17 119 (16 603–18 166) | 33,6 / 51,0 |

- **Regla del plan:** si la dispersión A/A es ≥ la diferencia `lz4`/`all`, la diferencia de carga no es medible. En modo
  RGB local, `lz4` − `all` es 18 ms en `decodeMs` y 435 ms hasta el objetivo; `cold` − `lz4` (A/A) es −90 ms y −504 ms, y el
  rango de `lz4` solo es 687 ms y 4,5 s. Con `gps_time` (`all` y `cold` leen la columna fría con zstd, así que `cold` no
  es un control) `lz4` − `all` es −4 ms y 166 ms, frente a un `cold` − `lz4` de −85 ms y −309 ms. Por red no hay control
  A/A: `lz4` − `all` es −41 ms y 364 ms, dentro de rangos de `lz4` de 521 ms y 2,1 s.
  **La diferencia `lz4`/`all` no es medible en ningún brazo**; `cold-rgb` queda dentro de los mismos rangos.
- La decodificación por bloque más lenta (`maxDecodeMs`) tiene una mediana de 26–31 ms en todos los esquemas (por
  ejecución, 24,9–37,5 ms).
- Por red, el piloto con `all` leyó ~170 MB en 1 335 peticiones (19,7 s); la mediana de `all` hasta el objetivo queda
  364 ms por debajo de la de `lz4`, dentro del rango de 2,1 s de `lz4`. No se investigó cuánto de la carga por red es
  ancho de banda y cuánto latencia.

## Límites

- **Caché del sistema:** los ficheros locales salen de la caché de páginas tras el primer acceso (30 GiB de RAM, ~35 GB
  de ficheros); el orden alterno lo reparte entre esquemas, no lo elimina. Es coherente con la cautela del plan: las
  velocidades de la tarea 0 eran de caché caliente, y aquí la decodificación de zstd y de LZ4 tampoco se distingue.
- **Recuento de bloques:** varía unos pocos por ciento entre ejecuciones (la elección de LOD depende del tiempo), así
  que `decodeMs` no está normalizado por bloque.
- **Brazo `gps_time`:** `openTimings.targetLodMs` sale nulo porque cambiar el color tras abrir reinicia la ventana de
  medida de la demo; por eso la tabla usa `timeToReachedMs` en todos los brazos.
- `loadavg` a 1 min al empezar: 1,24–3,29, probablemente inflado por el navegador de la ejecución anterior; sin `cargo` ni otro
  navegador en marcha.
- Cada ejecución registra los dos errores de consola `404` ya conocidos del registro de v8.1-pre (sin identificar).

## Lo que no se midió

- El tiempo del conversor web: no existe una sonda que mida la conversión en el navegador (la referencia del 29-10 es
  nativa).
- Majadas y Belvedere en el navegador; objetivos de px más finos o la carga completa de Albuera; Firefox; una red más
  lenta que la de RED-REF por defecto.
- La causa de que `cold` no cambie nada en Majadas y Belvedere.
