# D3c-T7: referencias, datasets y semillas con el V7 final — 15-11-2026

**Estado: regenerado y verificado.** Con el CLI por defecto (formato V7) se regeneran los datasets de render
(`belvedere-v7`, `majadas-v7`), las referencias SHA de los casos de [R1](R1_REFERENCE_SHA_2026-10-29.md#comandos) y las
semillas de fuzz. El `.oct` de Belvedere tiene SHA `da206a659ea0bfdf…`, **idéntico** al de la conversión web
([D3c-T6b](D3C_WEB_BELVEDERE_15-11-2026.md)). Las semillas ya versionadas (`seed-sample`, `-rev0`…`-rev9`, `-paged-chain-*`, `-v7` y sus `-split`)
salen byte a byte iguales; solo aparece una posición nueva, `seed-sample-rev10` (+ `-split`). Los tests con datos reales
pasan con las rutas `*-v7`, y la app alcanza el objetivo con ambos datasets. Registro fechado e inmutable. Datos
completos: [JSON](data/d3c-v7-references-15-11-2026.json). Contexto: [D3c-T3](D3C_RQ07_V7_15-11-2026.md).

## Entorno

AMD Ryzen AI 9 365, Linux 7.3.3-300.fc45, `rustc 1.101.0-nightly (41089d07a 2026-10-27)`, Chrome headless (adaptador `amd`/`rdna-3`, hardware).
Rama `feat/r3-v75`, commit base `21e04fe`; `viewer_convert` en perfil `release`, `--offline`. Una sola ejecución por caso.

## Comandos

```sh
cargo build --release -p viewer_convert --offline
target/release/viewer_convert INPUT OUT.oct      # sin --format-version: V7
tail -c 40 OUT.oct | head -c 8                   # OCTVEND7 en todos
(cd fuzz && cargo run --bin gen_seeds --offline)
R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --offline cells
R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --offline multi_owner_blocks -- --ignored
R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --offline --profile perf -- --ignored real_sim real_glue
node examples/web-host/probe-orbit-load.mjs target/render-data/DATASET.oct OUT.json --rest-first   # servidor: python3 -m http.server 8767 en la raíz
```

## Referencias V7 de los casos de R1

V7 fija los puntos por bloque y las divisiones espaciales: el CLI rechaza `--points-per-block` y `--spatial-divisions`. Por eso
**no se pueden reproducir** los casos de R1 con 4096 puntos por bloque ni con divisiones 2/8 (LAS pequeño ×2, Belvedere divisiones 8, sintético ×2);
quedan los cinco casos con opciones por defecto. SHA completos en el JSON.

| Caso | `.oct` V7 | `.octattrs` V7 |
| --- | --- | --- |
| LAS pequeño, defecto | `1b5f4992ede680d1…` | `d8586a3829764114…` |
| COPC pequeño (autzen) | `40861e07abdd6ef4…` | `88c453e6c19930f0…` |
| Belvedere, defecto | `da206a659ea0bfdf…` | `e2f587ab136906d8…` |
| Majadas | `a213eb2391cac51c…` | `0cffd5279cf3bd57…` |
| Sintético, defecto | `334cb274e0fb3e00…` | `5d795d63c0c2a790…` |

Los SHA de V5 de R1 no son comparables (otro formato).

## Datasets de render (`target/render-data/`, fuera de git)

| Fichero | Bytes | SHA-256 del `.oct` |
| --- | ---: | --- |
| `belvedere-v7.oct` / `.octattrs` | 530.962.031 / 889.640.140 | `da206a659ea0bfdf…` |
| `majadas-v7.oct` / `.octattrs` | 2.120.782.616 / 5.567.762.310 | `a213eb2391cac51c…` |

Los `*-v75` y los enlaces `oq09*` no se tocaron. Belvedere: 3.941 bloques, 81.682.329 puntos, 40 s de reloj (350 % de CPU).

## Semillas de fuzz

`gen_seeds` no modificó ninguna semilla versionada (`git status` limpio en `fuzz/` salvo las nuevas). Nuevas: `seed-sample-rev10`
(`c9595b3c0e5f9763…`) y `seed-sample-rev10-split` (`4e91fe44de1511a7…`). `seed-sample-v7`: `6792aa4eabb5c852…`; todos en el JSON.

## Tests con datos reales

- El filtro `lod_cells` del plan **no selecciona ningún test** (los módulos se llaman `cells_tests`/`cells_stream_tests`); se usó `cells`.
- `cells` (con `R3_REQUIRE_REAL_DATA=1`): 69 pasan, 9 ignorados; incluye los 3 `real_*_belvedere`.
- Ignorados: `multi_owner_blocks_in_the_v7_datasets` pasa (Majadas y Belvedere); `real_sim`/`real_glue` (6 tests, perfil `perf`) pasan.
- No se ejecutaron los benchmarks `#[ignore]` de `lod_cells_bench.rs` (medición, no verificación).

## La app llega al objetivo (`probe-orbit-load.mjs --rest-first`, 4 px, Chrome con adaptador hardware, `mode: timing`)

| Dataset | Primer LOD | Tiempo hasta el objetivo |
| --- | ---: | ---: |
| `belvedere-v75` | 268 ms | 1.091 ms |
| `belvedere-v7` | 249 ms | 808 ms |
| `majadas-v7` | — | 558 ms |

Indicativo (una ejecución cada uno). Único error de consola: un 404 (recurso sin identificar, presente también con v75).

## Límites

- Una ejecución por caso; los tiempos de la sonda no son un benchmark.
- Referencias solo para este formato, perfil `release` y opciones por defecto (R1 usó `perf`; el perfil no cambia los bytes).
