# Exploración: `opt-level` 3 frente a `"s"` en el conversor nativo

Fecha: 02-11-2026. Rama: `explore/opt-level` (sin cambios de código). Estado: **medido; decisión del propietario**.

## Método

Mismo código (`ed702e9`), dos builds `release` (`lto = true`, `codegen-units = 1`): `opt-level = "s"` (actual) y `opt-level = 3` (`CARGO_PROFILE_RELEASE_OPT_LEVEL=3`). `viewer_convert` por defecto, `/usr/bin/time -v`. Belvedere: 3 rondas intercaladas; Majadas: 1. Carga a 1 min: 1,8–6,6 (equipo ocupado; tómense como orientativas). Datos: [explore-opt-level-02-11-2026/](data/explore-opt-level-02-11-2026/).

## Resultados

| | `"s"` | `3` | Diferencia |
| --- | --- | --- | --- |
| Belvedere V5 (mediana) | 27,4 s | 21,8 s | −20 % |
| Belvedere v7.4 (mediana) | 53,7 s | 42,2 s | −21 % |
| Majadas V5 (1) | 288 s | 220 s | −24 % |
| Majadas v7.4 (1) | 362 s | 346 s | −4 % |
| Binario `viewer_convert` | 2,34 MB | 2,73 MB | +17 % |

RSS sin cambios relevantes. Salida idéntica en todas las rondas y en los dos builds (SHA-256).

## Lectura

- `opt-level` 3 acelera V5 y v7.4 por igual: v7.4 frente a V5 con el mismo build sigue en ~+94 % (Belvedere), así que no resuelve RQ-07, que compara versiones.
- Sin medir: tamaño y rendimiento del WASM con `opt-level` 3.
