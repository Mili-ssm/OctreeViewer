# D4: fuente `{url}` en la API pública

Estado: completado · 19-11-2026 · [#11](https://github.com/Mili-ssm/OctreeViewer/issues/11), dentro del [proyecto API pública](https://github.com/Mili-ssm/OctreeViewer/issues/27)

**Objetivo:** que `viewer.load()` de `viewer-api.mjs` abra un dataset remoto por HTTP Range, igual que ya hacen la
demo (`?url=`) y el protocolo del worker. Cierra la carencia TW8 de la
[revisión de técnicas](../benchmarks/REVIEW_TECHNIQUES_13-11-2026.md): «`load` solo acepta `Blob`… exponer
`{url}` en la API pública» ([ADR 0023](../../adr/0023-fuentes-locales-y-remotas.md), RF-09 y RF-11 de los
[requisitos](../../requirements.md)).

**Qué es la «plantilla `{url}`»:** el título de #11 la llama así, pero la fuente (TW8) habla del descriptor
`{ url, ... }` que ya entienden `resolveSource` y `readConcurrency` (`http-range-source.mjs`). No hay sustitución de
huecos en cadenas: es ese descriptor, aceptado por la API.

## Estado de partida (19-11-2026)

| Pieza | Hoy |
| --- | --- |
| `viewer-api.mjs`, `load(file, { voxelPixels, cacheBytes, attributes })` | Rechaza todo lo que no es `Blob` («load expects a File or Blob») |
| Workers (`viewer-worker.js`, `build-worker.js`) | Ya convierten `{ url, maxGap?, remoteReads? }` en `HttpRangeSource` con `resolveSource`; el nombre sale de `data.name ?? data.file.name` |
| Demo, `main.js` (`?url=`) | No usa la API; deriva el sidecar con `` `${url}attrs` ``, que falla con una consulta (`a.oct?v=1` → `a.oct?v=1attrs`) |
| Tests | `tests/viewer-api.test.mjs` solo cubre `Blob`; las pruebas remotas en navegador pasan por `?url=` de la demo |

## Contrato (decisiones técnicas del agente)

Blandas: pueden cambiar antes de la 1.0 (CON-05) sin ADR; se revisan con el paquete tipado del [ADR 0026](../../adr/0026-api-publica-licencia-y-version-1.md).

| # | Decisión | Por qué |
| --- | --- | --- |
| C1 | `load(source, options)` acepta un `Blob` o un descriptor `{ url, maxGap?, remoteReads? }`; `url` es una cadena o un `URL`, se resuelve contra `document.baseURI` (o `location.href`) en el hilo principal y viaja al worker como cadena `href`; solo admite `http:` y `https:` | El worker resuelve contra su propia URL, no la de la página; un `URL` no se puede clonar con `postMessage`; otros esquemas no admiten Range |
| C2 | La API copia solo `url`, `maxGap` (entero ≥ 0) y `remoteReads` (entero ≥ 1; por encima de 64 lo recorta `readConcurrency`) al mensaje; cualquier otro campo da `TypeError`; una URL con credenciales (`user:pass@`) también | `resolveSource` reparte el descriptor sobre las opciones de `HttpRangeSource`: un campo libre (`timeoutMs`, `retryDelays`) pasaría a ser API sin decidirlo; `readConcurrency` convierte < 1 en 8 |
| C3 | `attributes` es un `Blob`, un descriptor, o nada o `null` (sin sidecar); **la API no deriva el sidecar**: sin `attributes` se abre sin él, con fuente local o remota. La regla de la demo (`a.oct` → `a.octattrs`) se exporta como `sidecarUrl(url)` en `http-range-source.mjs` para quien la quiera: trabaja sobre `URL.pathname`, conserva la consulta, quita el fragmento y rechaza una ruta que acaba en `/` o ya en `.octattrs` | Pre-mortem: derivarlo por defecto rompe los datasets remotos sin sidecar (404 definitivo, 403 en S3, `TypeError` sin CORS) y obliga a un sidecar «opcional» con reglas propias; explícito es simétrico con `Blob`. Una URL firmada necesita el sidecar explícito de todas formas |
| C4 | El nombre del dataset es el último segmento de la ruta, decodificado si se puede (si no, tal cual); el tamaño lo da la fuente al abrir, en los dos modos: el worker de construcción devuelve `fileBytes` en `opened` | En modo build `bytes` salía `null`, porque venía de `data.file.size` (`viewer-worker.js:905`) |
| C5 | Los errores de red o de Range (`HttpRangeError`, servidor sin Range) rechazan la promesa de `load` con su mensaje; un error de `postMessage` rechaza la petición sin dejarla pendiente. El peor caso de un servidor que no responde (4 intentos de 15 s más esperas, unos 63 s) se documenta; `load` no admite `AbortSignal` todavía | RF-13: los errores llegan por la API; cancelar la apertura es otra función |
| C6 | `main.js` (`?url=`) usa `sidecarUrl` (si la URL no tiene nombre de fichero, abre sin sidecar); `package-local.py` inserta `http-range-source.mjs` en la página del paquete local | Un solo sitio para la regla; arregla `a.oct?v=1` → `a.oct?v=1attrs` de la demo |

**Fuera de alcance:** cabeceras, credenciales o `fetch` propio; exponer `timeoutMs`, `retryDelays` o `headBytes`;
migrar la demo a la API (RF-11, sigue en el [proyecto #27](https://github.com/Mili-ssm/OctreeViewer/issues/27)); el
paquete npm tipado (ADR 0026).

## Tareas

- [x] **1. API.** C1–C6 en `viewer-api.mjs`, `http-range-source.mjs`, `build-worker.js` (`fileBytes`),
  `viewer-worker.js` y `main.js`. Comprobación: tests Node nuevos (descriptor válido y `URL`, URL relativa resuelta,
  esquema rechazado, campo desconocido rechazado, `remoteReads` 0 rechazado, `sidecarUrl` con consulta, fragmento,
  `/` final y `.octattrs`, nombre con `%` inválido, `fileBytes` del worker de construcción); los existentes en verde.
- [x] **2. Navegador.** Sonda `probe-api-remote.mjs` (como `probe-real-additive.mjs`): `createViewer` +
  `load({ url }, { attributes: { url: sidecarUrl(url) } })` de la muestra servida por `red-ref-server.mjs`, sin
  sidecar, y el error con `--ignore-range`. Comprobación: Playwright en Chromium, en modo build (predeterminado) y
  sin worker de construcción: `name`, `bytes` y metadatos iguales en los dos, primer frame; rechazo con mensaje claro
  sin Range; consola limpia. Procedimiento en `PROBES.md`.
- [x] **3. Documentación y cierre.** `examples/web-host/README.md` (API y flujo remoto), ADR 0023 («no hay lector
  remoto» ya no es cierto), [inventario](../../reference/inventory.md), `docs/estado.md`, #11 cerrada. Comprobación:
  `scripts/check.sh` completo.

## Resultado (19-11-2026)

- Tests Node de `examples/web-host`: 261 pasan. Sonda en Chromium sin interfaz: con y sin worker de construcción,
  `name` `sample.oct`, `bytes` 2015820 y metadatos iguales, objetivo alcanzado (0,6–1,9 s); sin Range, `load` rechaza
  en 0,27 s; consola sin errores salvo el `favicon.ico` del servidor estático.
- La sonda destapó un fallo previo de la API: con el worker de construcción (ruta de celdas, la predeterminada) el
  bucle de frames se paraba al cargar el último bloque y el driver de celdas no se asentaba; ahora sigue mientras
  `cellsBusy`, como la demo (`lodStillLoading`).
- Sin hacer (fuera de alcance): `AbortSignal` en `load`, cabeceras o credenciales, migrar la demo a la API (RF-11).
