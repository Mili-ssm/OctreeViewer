# D1: cancelación real y reintentos en la fuente HTTP Range — 14-11-2026

**Estado: implementado y cubierto por tests; sin efecto medible con estos datos.** La fuente remota
reintenta los fallos transitorios con un tiempo límite por petición (TW5) y aborta la descarga de un
bloque cuando su ticket se cancela (TW3), en lugar de descargar entero un cuerpo que ya no sirve. En
Majadas por RED-REF no hay lecturas obsoletas que cancelar: el objetivo se alcanza antes que cualquier
cambio de vista. La ganancia en bytes queda sin medir hasta tener un dataset mayor (ESC-1).

## Qué cambió

| Propuesta ([revisión](REVIEW_TECHNIQUES_13-11-2026.md#carga-web-y-api)) | Comportamiento |
| --- | --- |
| TW5 reintentos y tiempo límite | Reintenta errores de red, 429, 5xx y cuerpos truncados: 3 veces, con esperas de 200, 800 y 2.400 ms (±25 %), y respeta `Retry-After` hasta 10 s. Tiempo límite de 15 s por intento (`timeoutMs`). No reintenta otros 4xx, un 200 a una petición Range, un ETag cambiado, un cuerpo corto ni un aborto |
| TW3 cancelación real | La petición de lectura del lector wasm acepta su propia fuente (`setSource`); la fuente HTTP da una vista por ticket (`withSignal`) que comparte caché y agrupación. Una petición agrupada solo se aborta cuando la abandonan todas sus lecturas. La cancelación llega como `AbortError` y no cuenta como fallo ni se reintenta |
| Estadísticas | `requests`, `bytes`, `cachedReads`, `retries`, `timeouts`, `aborted` y `abortedBytes` por worker |

El contrato está documentado en `examples/web-host/README.md`. Las fuentes locales (`File`/`Blob`) no
cambian.

## Medidas

Probe `probe-orbit-load.mjs` con el servidor RED-REF (100 Mbit/s, 50 ms de RTT), Majadas v7.5 sin
fichero de atributos, dos rondas intercaladas. «Antes» es la revisión `0aaf0bc` (solo reintentos);
«después», `7c095b8`.

| Escenario | Antes | Después |
| --- | --- | --- |
| 4 px, encuadre y órbita de 10 s | 12 GET, 1.458.308 B, 0 abortos | igual |
| 1 px, salto de cámara a los 1,5 s y órbita de 10 s | 36 GET, 2.019.638 B, 0 abortos | igual |

A 100 Mbit/s, Majadas descarga 2 MB y llega al objetivo en ~0,6 s, así que el salto de cámara llega
cuando ya no queda nada en vuelo. La ruta real con `setSource` funcionó en el navegador sin errores.

## Límites

- La ganancia de TW3 no se midió: haría falta un dataset con muchas más lecturas en vuelo (ESC-1) o una
  red más lenta.
- La ruta wasm que rechaza con `AbortError` entre lecturas de un mismo bloque no tiene test automático:
  `JsValue` no funciona fuera de wasm. Lo cubren los tests de los workers con un lector simulado.
- `AbortSignal.any` exige Chrome 116, Firefox 124 o Safari 17.4, o posteriores.
- Una revisión encontró y corrigió una apertura antigua que sobrescribía las fuentes del dataset actual,
  contadores inflados por lecturas ya abortadas y un test dependiente del orden de las microtareas.

## Identidad y reproducción

- Código: rama `explore/d1-remote` (`0aaf0bc`, `87401d3`, `7c095b8`), integrada después en `feat/r3-v75`.
  Entorno: el de [T11](data/r3-t11-smoke-13-11-2026/identity.txt).
- `node examples/web-host/probe-orbit-load.mjs target/render-data/majadas-v75.oct SALIDA.json --url=… --remote --orbit-s=10 --px=1 --jump-after-ms=1500`
- Datos en [data/d1-remote-cancel-14-11-2026](data/d1-remote-cancel-14-11-2026/).
