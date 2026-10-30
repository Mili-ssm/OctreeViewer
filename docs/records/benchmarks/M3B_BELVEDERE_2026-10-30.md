# M3b: conversión de Belvedere en los navegadores (30-10-2026)

Registro fechado; solo admite añadidos hasta el merge de la rama. Contexto: [plan de la etapa 4](../../plans/conversor-web-paralelo-etapa4.md) (Tarea 6, decisiones D2 y D3, enmiendas C6 y C9). Referencias SHA: [R1](R1_REFERENCE_SHA_2026-10-29.md). Puerta Node previa: [M3b Node](M3B_NODE_GATE_2026-10-30.md). Datos crudos: [JSONL](data/m3b-belvedere-2026-10-30.jsonl), [driver](data/m3b-belvedere-2026-10-30-driver.mjs), [página](data/m3b-belvedere-2026-10-30.html).

## Entorno y método

AMD Ryzen AI 9 365 (20 hilos), 30 GiB de RAM, Linux 7.2.13-200.fc44, gobernador `powersave`; Chrome 156.0.8078.73 (`/usr/bin/google-chrome`, headless), Firefox 157.0 (build de Playwright), Playwright-core, `rustc 1.101.0-nightly`, wasm-bindgen 0.2.129. Commit `c62a7f6`, `viewer_wasm` en `--release`, `pkg/` regenerado en la sesión; paquete `file://` con `package-local.py`. Sin otros agentes. Antes de cada celda el driver esperaba a `loadavg` 1 min < 1,5 (valores al empezar 0,5-1,4; PSI de CPU `avg10` 0,00).

- **Celdas HTTP:** perfil nuevo bajo `target/m3b-profiles` (borrado al acabar), `python3 -m http.server`, página propia con `converter-api.mjs` y `engine: "core"`; Belvedere entra por `<input type=file>` (File respaldado por disco). Opciones: `spatialDivisions`, `spoolBudgetBytes`, `ramBudget`; resto por defecto (262144 puntos/bloque, error 0,01, formato v5).
- **Celdas `file://`:** `index.html` empaquetado, manejado por su propia interfaz (Playwright: fichero, divisiones, clic en «Convert»). La interfaz no tiene campo de presupuesto de spools: usa el defecto de 2 GiB y `ramBudget` 4 GB.
- **Tiempo:** de `convert()` a su promesa (incluye `getFile` de la salida; la lectura posterior para el SHA queda fuera). Coincide con `timings.totalMs` del worker (±2 ms), salvo la interfaz `file://` (sondeo de 250 ms). Los natives corren con `native_hybrid` (1 hilo, `HybridSpoolStore` con páginas en fichero, 262144 puntos/bloque) en la misma sesión, antes de los navegadores; no incluye la validación que sí hace el wasm.
- **SHA:** la salida se guarda con una descarga del navegador (`Blob` de OPFS) y se calcula en Node; se compara `.oct` y `.octattrs` con R1.
- **PSS:** `sample-process-memory.py` cada 0,5 s sobre el árbol del navegador; pico dentro de la ventana de conversión (7-11 procesos). **OPFS:** `navigator.storage.estimate().usage` y listado del directorio cada 1 s desde la página (aproximado: Firefox lo infrarreporta).
- Firefox: se fijaron las prefs de S8 (`dom.storageManager.prompt.testing`, `.allow`), pero `persisted()` seguía en `false` al acabar. Ninguna celda lo necesitó: la estimación (volcado 6,2 GB + salida 2,0 GB = 7,6 GiB) cabe bajo el límite temporal de ~10 GiB.

## Referencia nativa (`native_hybrid`, misma sesión)

| División | Presupuesto | ms | RSS máx | Límite 4 × |
| --- | --- | --- | --- | --- |
| 2 | 2 GiB | 43.633 / 43.703 | 2,26 GiB | 174,7 s |
| 8 | 2 GiB | 34.688 | 2,16 GiB | 138,8 s |
| 2 | 4 GiB | 43.518 | 4,07 GiB | 174,1 s |

Los SHA nativos son R1. Estadísticas de spool idénticas a las del registro de la puerta Node.

## Resultados

| Celda | ms | Razón vs nativo | SHA = R1 | PSS pico (GiB) | Volcado (bytes / spools) | Pages peak | Sobre presupuesto (MiB) | RAM peak (GiB) | OPFS pico (GiB) | Zonas creadas / quedan |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Chrome http d2 2 GiB, r1 | 93.117 | 2,13 | sí | 3,44 | 4.073.405.871 / 265 | 10.152 | 1,1 | 2,00 | 4,13 | 13 / 0 |
| Chrome http d2 2 GiB, r2 | 107.324 | 2,46 | sí | 3,45 | idem | idem | idem | idem | 4,13 | 13 / 0 |
| Chrome http d2 4 GiB (ram 8 GB) | 100.704 | 2,31 | sí | 5,77 | 715.961.709 / 3 | 2.733 | 0,7 | 4,00 | 2,39 | 13 / 0 |
| Chrome http d8 2 GiB | 77.759 | 2,24 | sí | 4,52 | 2.620.483.664 / 1.025 | 5.753 | 1,2 | 2,00 | 3,13 | 10 / 0 |
| Firefox http d2 2 GiB | 93.352 | 2,14 | sí | 3,39 | 4.073.405.871 / 265 | 10.152 | 1,1 | 2,00 | 2,63 (*) | 13 / 0 |
| Firefox http d2 4 GiB (ram 8 GB) | 107.506 | 2,47 | sí | 5,73 | 715.961.709 / 3 | 2.733 | 0,7 | 4,00 | 1,48 (*) | 13 / 0 |
| Firefox http d8 2 GiB | 101.551 | **2,93** | sí | 4,48 | 2.620.483.664 / 1.025 | 5.753 | 1,2 | 2,00 | 2,75 (*) | 10 / 0 |
| Firefox `file://` d2 2 GiB | 97.212 | 2,23 | sí | 3,48 | 4.073.405.871 / 265 | 10.152 | 1,1 | 2,00 | 2,70 (*) | 13 / 0 |

(*) Firefox declara un `usage` bastante menor que Chrome para los mismos bytes; solo orientativo. «Zonas quedan» = ficheros `.zone-*` al acabar (0 en todas; `zonesReleased` del conversor 5, 1 y 2 a mitad de conversión). Quedan solo los dos `.partial` de salida. Sin errores en ninguna celda; la consola de Chrome mostró un 404 (favicon) y avisos del inicializador wasm-bindgen. Los `HybridStats` coinciden con el nativo y con la puerta Node (`livePayloadBytes` y `pagesInUse` acaban en 0). Salida: 981.063.326 B (d2) y 982.400.758 B (d8).

### Chrome `file://` (D3, Review Focus 5)

| Caso | Resultado |
| --- | --- |
| LAS pequeño `fs_e430_n4008_18.las` | Convertido solo en RAM en 0,5 s (worker 162 ms); aviso «Running without browser storage: the conversion is limited to RAM. Serve the page over http://localhost for large files.»; `.oct` `176ee42d…` y `.octattrs` `ee1e5daa…` = CLI/R1. `spool`: 0 volcado, RAM peak 4,2 MB. Sin PSS útil (una muestra) |
| Belvedere | Rechazado a los 3,5 s (cabecera e inspección), sin estado «Converting» ni progreso: «Not enough memory without browser storage: this file needs about 8.2 GB of RAM but the limit is 4.0 GB. Serve the page over http://localhost (for example python3 -m http.server) so the converter can use disk-backed storage.» PSS del navegador 1,24 GiB (línea base) |

## Veredictos

| Criterio | Resultado | Veredicto |
| --- | --- | --- |
| SHA = R1 en todas las celdas | 8 de 8 Belvedere + LAS pequeño | pasa |
| Parada > 4 × el nativo (D2) | máximo 2,93 × (Firefox d8); resto 2,13-2,47 × | **pasa** (margen >= 27 %) |
| PSS > `ram_budget` + 1 GiB | 2 GiB: máximo 4,52 GiB frente a 4,73 GiB (4 GB + 1 GiB); 4 GiB: 5,77 frente a 8,5 | pasa (d8 con 0,21 GiB de margen) |
| OPFS pico > 1,5 × bytes volcados vivos | Chrome d2 2 GiB: 4,13 GiB; menos la salida final (1,87 GiB) quedan 2,26 GiB frente a 2,48 GiB de páginas pico (0,91 ×) | pasa (aprox.) |
| Chrome `file://`: LAS pequeño en RAM con aviso; Belvedere rechazado antes | cumple | pasa |

## Decisión 3: 2 GiB frente a 4 GiB de spools (d2)

| | 2 GiB (ram 4 GB) | 4 GiB (ram 8 GB) |
| --- | --- | --- |
| Tiempo Chrome / Firefox | 93,1-107,3 s / 93,4 s | 100,7 s / 107,5 s |
| PSS pico Chrome / Firefox | 3,44 / 3,39 GiB | 5,77 / 5,73 GiB (+2,3) |
| Volcado | 4,07 GB, 265 spools, 10.152 páginas | 0,72 GB, 3 spools, 2.733 páginas |
| OPFS pico (Chrome) | 4,13 GiB | 2,39 GiB |

**Recomendación: 2 GiB (y `ram_budget` 4 GB, el defecto).** Volcar 3,4 GB más no cuesta tiempo medible (la dispersión entre rondas de Chrome, 93-107 s, es mayor que la diferencia entre brazos) y 4 GiB añade 2,3 GiB de PSS y exige `ram_budget` 8 GB, que deja poco margen en equipos de 16 GiB. El brazo de 4 GiB solo ahorra ~1,7 GiB de OPFS.

## Límites

- 1 ronda por celda (2 en Chrome d2 2 GiB: 93 y 107 s, ±7 %); no son distribuciones. Una máquina, headless, disco NVMe con caché de página del SO.
- **No se midió la memoria wasm (C9) en el navegador**: el worker no expone `memory.buffer` y no se modificó código de producto; la puerta Node midió 28-40 MiB. El PSS observado (<= 4,5 GiB con 2 GiB de spools) es coherente con RAM de JS + zonas, no con wasm.
- La razón de Firefox d8 (2,93 ×) es la más alta y la de mayor dispersión probable (101,6 s frente a 77,8 s de Chrome); con una sola ronda no se distingue ruido de diferencia real.
- Firefox no recibió `persist()` (a pesar de las prefs); no se probó una conversión que exija más de 10 GiB.
- La interfaz `file://` no permite fijar `spoolBudgetBytes`; el tiempo de las celdas de interfaz incluye el sondeo de 250 ms.
- El nativo no valida la salida y el wasm sí (~1,3 s), por lo que la razón de trabajo equivalente es algo menor.
- Tarea 6 no cubre cancelación ni reconversión (Tarea 7) ni el CLI por defecto como contexto (no ejecutado).
