# Exploración: `opt-level` 3 frente a `"s"` en el WASM del visor

Fecha: 02-11-2026. Rama: `explore/opt-level` (sin cambios de código). Estado: **medido; decisión del propietario**. Sigue a la [medida nativa](EXPLORE_OPT_LEVEL_02-11-2026.md).

## Método

Mismo código, dos builds wasm32 `release` (`CARGO_PROFILE_RELEASE_OPT_LEVEL=3` en un `CARGO_TARGET_DIR` aparte) + `wasm-bindgen`. Belvedere v7.4 a 8 px con el arnés de T7, 3 rondas por build intercaladas; carga a 1 min 1,1–2,2. El tiempo al objetivo se toma de `lodLog` (primera entrada en la etapa objetivo sin pendientes), no del criterio de T7. No hay arnés de conversión en el navegador. Datos: [explore-opt-level-wasm-02-11-2026/](data/explore-opt-level-wasm-02-11-2026/).

## Resultados

| | `"s"` | `3` |
| --- | --- | --- |
| `.wasm` sin comprimir | 2 656 957 B | 2 228 520 B (−16 %) |
| gzip -9 / brotli -q11 | 644 419 / 465 194 B | 690 132 (+7 %) / 502 071 B (+8 %) |
| Tiempo al objetivo (`lodLog`) | 892 ms [805–899] | 735 ms [507–753] |
| Edad de escena p95 | 274 ms [258–277] | 237 ms [227–245] |
| Respuesta p95 / máx | 1,1 / 3,4 ms | 1,2 / 5,7 ms [3,0–16,7] |
| `sceneBuilds` en 10 s | 105 | 127 |
| PSS pico | 574 MiB | 579 MiB |

El digest de escena en reposo es idéntico en las 6 rondas.

## Límites

- 3 rondas por build; el máximo de respuesta de `3` tiene una ronda con 16,7 ms.
