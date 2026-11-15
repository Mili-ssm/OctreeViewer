# D3c-T6b: conversión web de Belvedere con el V7 final frente a V5 — 15-11-2026

**Estado: pasa el criterio de parada.** En Chrome headless, con `engine: "core"` y los mismos
presupuestos (spools 2 GiB, `ramBudget` 4 GB), `formatVersion: "v7"` tarda **×1,87** lo que `"v5"`
(mediana 141,9 s frente a 75,9 s; cota del [ADR 0043](../../adr/0043-coste-de-conversion-de-v7-frente-a-v5.md): ≤ ×2,1)
con 4 rondas intercaladas, sin errores ni falta de memoria en ninguna de las 8 conversiones. El pico de PSS
del navegador pasa de 3,76 a 4,75 GiB (+26 %). Los `.oct` y `.octattrs` del wasm son byte a byte los del CLI
nativo del mismo formato (SHA-256 idénticos). Registro fechado e inmutable; contexto: Tarea 6 del plan D3c,
método de [M3b](M3B_BELVEDERE_2026-10-30.md) y [D3c-T3](D3C_RQ07_V7_15-11-2026.md) para el lado nativo.
Datos crudos, driver y página: [data/d3c-web-belvedere-15-11-2026/](data/d3c-web-belvedere-15-11-2026/).

## Entorno

AMD Ryzen AI 9 365 (20 hilos), Linux 7.3.3-300.fc45, Google Chrome 157.0.8103.78 (`/usr/bin/google-chrome`,
headless). Rama `feat/r3-v75`, commit `9cdd7c1`; `viewer_wasm` en `--release` con `pkg/` regenerado y
`viewer_convert` nativo en `--release`, ambos con `--offline`. Entrada: `belv_2009_digital-aerial_pcd_copc.laz`.

## Método

- Adaptación del driver y la página de M3b (celdas HTTP): Chrome nuevo con perfil propio bajo `target/` por
  conversión (borrado al acabar), `python3 -m http.server` propio, Belvedere por `<input type=file>`,
  `converter-api.mjs`. Opciones: `formatVersion` (`v5` o `v7`), `spoolBudgetBytes` = 2 GiB, `ramBudget` = 4 GB;
  sin `pointsPerBlock` ni `spatialDivisions` (V5 usa sus defectos; V7 los fija).
- 4 rondas intercaladas (v5,v7 / v7,v5 / v5,v7 / v7,v5). Antes de cada una, espera acotada a `loadavg` 1 min < 1,5
  (al empezar: 1,27-1,49; en ningún caso se agotó la espera).
- Tiempo: de `convert()` a su promesa (igual que M3b); coincide con `timings.totalMs` del worker (±1 ms).
  PSS: `sample-process-memory.py` cada 0,5 s sobre el árbol del navegador, pico dentro de la ventana de conversión.
- SHA: descarga de las salidas desde OPFS, hash en Node; comparado con una ejecución nativa por formato
  (`viewer_convert INPUT OUT.oct --format-version v5|v7`, `--release`).

## Resultados

| Variante | Tiempo mediana (s) | Rango (s) | PSS pico mediana (GiB) | Rango PSS (GiB) | `.oct` + `.octattrs` (B) |
| --- | --- | --- | --- | --- | --- |
| V5 | 75,94 | 72,23-79,32 | 3,76 | 3,75-3,76 | 981.063.326 + 610.586.859 |
| V7 | 141,91 | 141,16-148,62 | 4,75 | 4,74-4,75 | 530.962.031 + 889.640.140 |
| Razón V7/V5 | **1,869** | | 1,26 | | par: 0,893 |

Tiempos por ronda (s): V5 74,04 / 77,84 / 79,32 / 72,23; V7 141,16 / 141,86 / 141,95 / 148,62.
Volcado a OPFS (bytes volcados / spools): V5 7,87 GB / 279; V7 14,64 GB / 1.589 (pico de páginas 12.778 frente a 22.914).
Pico de `navigator.storage.estimate().usage`: V5 4,76 GiB; V7 7,01 GiB. Sin zonas `.zone-*` al acabar; solo
quedan los dos `.partial` de salida. La consola solo mostró el 404 del favicon.

| Referencia nativa (una ejecución, `--release`) | Tiempo (s) | Web / nativo |
| --- | --- | --- |
| V5 | 23,20 | 3,27 |
| V7 | 50,39 | 2,82 |

El nativo V7/V5 es ×2,17 en esta ejecución única (no es la medida del ADR 0043: esa es D3c-T3, perfil `perf`, 5 rondas).

## Paridad de SHA con el nativo

| Formato | `.oct` SHA-256 | `.octattrs` SHA-256 | web = nativo |
| --- | --- | --- | --- |
| V5 | `ecdc4889778c082ee4ba67d8d0a2fc37182a9d3f2f35abd965a8293bec6fccad` | `cd3cad4571b2474377b977fe2237b645eef221f97f68c6e1783c5fd4a2a8fe9a` | sí (4 de 4; igual a R1 d2) |
| V7 | `da206a659ea0bfdfbfb3d8141592387cb2dde7133e6a5e44ed9a7a5db3c9f0c4` | `e2f587ab136906d8fc43f2f34c46be0f23a566f3954a8288056b95dae9802323` | sí (4 de 4) |

## Veredicto

| Criterio | Resultado | Veredicto |
| --- | --- | --- |
| Ninguna conversión V7 falla ni se queda sin memoria | 4 de 4 correctas | pasa |
| Mediana web V7 ≤ 2,1 × mediana web V5 | ×1,869 (margen 11 %) | **pasa** |

## Límites

- Solo Chrome 157 headless, un dataset (Belvedere) y una máquina; Firefox no se midió. Cifras válidas solo para
  este commit, perfil `release` y equipo; no se trasladan.
- El `loadavg` inicial osciló entre 1,27 y 1,49 (sin otros agentes de medida, pero no era 0): el ruido cabe dentro
  de los rangos de la tabla (V5 ±5 %, V7 ±3 %).
- La web no usa el perfil `perf` del nativo y no cuenta la lectura posterior para el SHA. El PSS incluye el
  navegador entero (línea base ~1,2 GiB según M3b), no solo el conversor.
- El nativo se midió una vez por formato (solo para el SHA y un orden de magnitud). Los RSS nativos no se registran.
