# R4 T1: spike de densidad de la pirámide

Fecha: 02-11-2026. Rama: `feat/render-r3-node-cells`. Estado: **ninguna combinación cumple la regla de elección; vuelve al propietario**.

## Método

`cargo run --release -p viewer_convert --example density_spike -- <LAZ> <dir> --leaf-points N --sample-grid G` convierte a `v7.3-pre` y emite un JSON con disco (`.oct` + atributos), tiempo de pared, RSS pico, nodos, puntos por hoja y salto de espaciado hoja/padre en niveles (Tarea 1 del [plan de R4](../../plans/render-r4-piramide-densa.md)). Matriz: hoja {16, 32, 64} k × `sample_grid` {32, 64, 128, 256}, más el control actual (hoja 262 144, g = 8). Una ejecución por combinación, en serie, en el mismo equipo; carga previa en cada `.pre.txt`. Datos: [r4-density-spike-01-11-2026/](data/r4-density-spike-01-11-2026/).

Regla de elección, fijada antes de medir: **mínimo disco con salto hoja/padre ≤ 1 nivel en p95 y RSS ≤ `DEFAULT_MEMORY_BUDGET_BYTES` (512 MiB = 537 MB)**.

## Resultados

V5 (disco `.oct` + atributos): Majadas 7,53 GB, Belvedere 1,59 GB; RQ-07 permite +35 %: 10,17 GB y 2,15 GB.

### Majadas (TLS)

| Hoja | g | Salto p95 | RSS MB | Disco GB | Tiempo s | Nodos |
| --- | --- | --- | --- | --- | --- | --- |
| 262 k (control) | 8 | 7 | 272 | 4,63 | 287 | 6 259 |
| 16 k | 32 / 64 / 128 / 256 | 3 / 2 / **1** / **0** | 551 / 588 / 702 / 988 | 5,45 / 6,65 / 9,16 / 12,73 | 411 / 424 / 518 / 660 | 101 006 |
| 32 k | 32 / 64 / 128 / 256 | 4 / 3 / 2 / **1** | 320 / 328 / 458 / 776 | 4,96 / 5,64 / 7,44 / 10,59 | 357 / 383 / 431 / 538 | 48 798 |
| 64 k | 32 / 64 / 128 / 256 | 4 / 3 / 2 / **1** | 277 / 261 / 302 / 653 | 4,79 / 5,16 / 6,35 / 8,89 | 341 / 344 / 371 / 499 | 25 056 |

### Belvedere (aéreo)

| Hoja | g | Salto p95 | RSS MB | Disco GB | Tiempo s | Nodos |
| --- | --- | --- | --- | --- | --- | --- |
| 262 k (control) | 8 | 7 | 274 | 1,47 | 38 | 1 343 |
| 16 k | 32 / 64 / 128 / 256 | 3 / 2 / **1** / **0** | 174 / 177 / 226 / 418 | 1,54 / 1,70 / 2,14 / 2,78 | 62 / 76 / 87 / 113 | 21 268 |
| 32 k | 32 / 64 / 128 / 256 | 4 / 3 / 2 / **1** | 147 / 158 / 162 / 401 | 1,50 / 1,57 / 1,79 / 2,29 | 54 / 58 / 67 / 91 | 8 289 |
| 64 k | 32 / 64 / 128 / 256 | 4 / 3 / 2 / **1** | 150 / 149 / 167 / 379 | 1,49 / 1,54 / 1,70 / 2,13 | 50 / 52 / 58 / 76 | 5 239 |

## Lectura

- **Salto ≤ 1 en p95** solo con g = 256 (cualquier hoja) o con hoja 16 k y g = 128. Cada duplicación de g baja el salto un nivel, como se esperaba.
- **Belvedere** cumple la regla: el mínimo disco es hoja 64 k, g = 256 (2,13 GB, +34 % sobre V5, en el límite; 379 MB).
- **Majadas no la cumple:** todas las combinaciones con salto ≤ 1 superan el presupuesto de RSS (653–988 MB). La más cercana es 64 k / g = 256 (653 MB, 8,89 GB = +18 % sobre V5).
- **Tiempo de conversión:** respecto al control, 64 k / g = 256 cuesta +74 % en Majadas y +102 % en Belvedere. No hay tiempo de V5 medido en este equipo; si el control se toma como referencia, ninguna combinación con salto ≤ 1 cumple ≤ +30 % (RQ-07). *Inferido*, no medido frente a V5.
- La combinación más barata que cumple RSS y tiempo es 64 k / g = 128 (Majadas: 302 MB, 6,35 GB, +29 % de tiempo), pero deja el salto en 2 niveles.
- El RSS crece con g porque la rejilla dispersa (`BTreeMap`) guarda una entrada por celda ocupada; no está optimizada.

## Límites

- Una ejecución por combinación: el tiempo no tiene varianza medida.
- Solo mide el conversor; la cadena de hoja de R3 se vuelve a medir en la Tarea 6.
