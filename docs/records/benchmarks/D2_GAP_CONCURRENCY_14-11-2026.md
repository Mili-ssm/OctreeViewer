# D2: unión con huecos y concurrencia por fuente en la carga remota — 14-11-2026

**Estado: implementado; 8 lecturas concurrentes por defecto en fuentes remotas, `maxGap` sigue en 0.**
Con RED-REF, pasar de 4 a 8 lecturas concurrentes reduce la mediana del tiempo hasta alcanzar el
objetivo entre un 15 y un 30 % en la ruta CPU, sin empeorar los frames. 16 lecturas no mejoran más y
descargan más bytes. Unir lecturas separadas por huecos (16–256 KiB) ahorra como mucho 3 de 31
peticiones, a cambio de hasta un 6 % más de bytes, y no cambia el tiempo. En la ruta de celdas, la
concurrencia no influye: Belvedere tarda lo mismo con 4 que con 8 lecturas.

## Qué cambió

| Propuesta ([revisión](REVIEW_TECHNIQUES_13-11-2026.md#carga-web-y-api)) | Comportamiento |
| --- | --- |
| TW4 unión con huecos | `HttpRangeSource` une las lecturas del mismo tick separadas por hasta `maxGap` bytes en una petición; los bytes sobrantes se cuentan en `gapBytes`. Por defecto `0` (solo rangos contiguos) |
| TW10 concurrencia por fuente | `readConcurrency(spec)`: una fuente remota usa `remoteReads` (por defecto 8, máximo 64); un `Blob` local, 4. Lecturas de bloques y de páginas se limitan por separado |
| Configuración | Descriptor `{ url, maxGap?, remoteReads? }` y parámetros `&maxGap=` y `&remoteReads=` en la demo; el probe acepta `--max-gap=` y `--remote-reads=` |

El contrato está en `examples/web-host/README.md`.

## Medidas

Probe `probe-orbit-load.mjs --remote --rest-first` con el servidor RED-REF (100 Mbit/s, 50 ms de RTT),
datasets v7.5 sin fichero de atributos. Tiempo desde el primer GET hasta alcanzar el objetivo en la
pose encuadrada; mediana de 2 rondas intercaladas × 4 valores de `maxGap` (8 ejecuciones por celda).

| Caso | 4 lecturas | 8 lecturas | 16 lecturas |
| --- | --- | --- | --- |
| Majadas 4 px · CPU | 1.091 ms · 12 GET | **828 ms (−24 %)** · 8 GET | 1.089 ms · 8 GET |
| Majadas 1 px · CPU | 1.601 ms · 36 GET | **1.354 ms (−15 %)** · 29 GET | 1.359 ms · 25 GET |
| Belvedere 4 px · CPU | 4.636 ms · 2,43 MB | **3.243 ms (−30 %)** · 2,74 MB | 3.356 ms · 2,92 MB |
| Belvedere 4 px · celdas (2 ejecuciones, `maxGap` 0) | 1.717 ms · 42 GET | 1.720 ms · 42 GET | — |

- **Variación:** los tiempos de Majadas se agrupan en dos valores separados por unos 250 ms (~5 RTT),
  así que una mediana de 8 ejecuciones es la cifra útil, no la diferencia entre dos.
- **`maxGap` con 8 lecturas:** en Majadas a 1 px pasa de 31 a 29 GET con 16 y 64 KiB (+0,7 % de
  bytes) y a 28 GET con 256 KiB (+5,8 %); el tiempo queda dentro del ruido. En Majadas a 4 px no cambia
  nada; en Belvedere el número de GET varía entre ejecuciones más que entre valores de `maxGap`.
- **RQ-01 en la órbita de 10 s:** intervalo entre frames p95 ≤ 18,1 ms y máximo ≤ 31,3 ms en todos
  los casos (Belvedere CPU a 4 px y Majadas a 1 px, con 4, 8 y 16 lecturas).
- **Celdas frente a CPU en Belvedere remoto:** 1,7 s frente a 3,2 s con 8 lecturas. La ruta de celdas
  lee primero la cadena de ancestros y no llena las 8 plazas.

## Límites

- El barrido de la ruta CPU se tomó antes de corregir el orden de lectura de las celdas (`4a47812`,
  `671ad44`); entonces Belvedere con celdas en remoto no llegaba nunca al objetivo. Las cuatro
  ejecuciones de celdas son posteriores a la corrección.
- La ruta CPU de Belvedere a 1 px no construye la escena: estima 268.435.648 bytes frente al límite de
  268.435.456 (límite de la ruta CPU, ya visible en los datos de R1). Esas ejecuciones se guardan sin
  tiempo.
- Solo Majadas y Belvedere, un equipo y una red simulada. Con un dataset mayor (ESC-1) o una red más
  lenta, los huecos y 16 lecturas podrían compensar; no se midió.

## Identidad y reproducción

- Código: rama `explore/d2-gap` (`d7f674d`, `606fa77`, `671ad44`, `1837ab9`, `046e72a`), integrada en
  `feat/r3-v75`. El barrido de la ruta CPU usó el cambio de `d7f674d` antes de rebasarlo (el SHA-256
  del wasm está en cada JSON); las ejecuciones de celdas, `046e72a`. Entorno: el de
  [T11](data/r3-t11-smoke-13-11-2026/identity.txt).
- `node examples/web-host/probe-orbit-load.mjs target/render-data/majadas-v75.oct SALIDA.json --remote --rest-first --px=4 --cells=0 --max-gap=0 --remote-reads=8`
  (las órbitas, sin `--rest-first`).
- Datos en [data/d2-gap-concurrency-14-11-2026](data/d2-gap-concurrency-14-11-2026/): un JSON por
  ejecución (`rest|orbit-DATASET-PX-gMAXGAP-rLECTURAS-nRONDA.json`) y los registros del barrido
  (`sweep-*.out`).
