# Referencia SHA de las semillas `v7.5-pre` (R5, 02-11-2026)

Registro fechado e inmutable. Semillas de fuzz del formato paginado ([spec](../../reference/format/FORMAT_V7_PRE5.md), tarea P2 del [plan R5](../archive/render-r5-jerarquia-paginada.md)), generadas por `fuzz/src/bin/gen_seeds.rs` (`cargo run --bin gen_seeds` desde `fuzz/`). Las semillas de las demás revisiones no cambian: `seed-sample-rev0`…`rev8` conservan las SHA de los registros anteriores.

Fixtures:

- `seed-sample` / `seed-sample-rev9`: `write_revision(FormatVersion::V7_PRE5)`, un nodo y una página (la raíz). Sin versión partida: `v7.5-pre` no tiene capa partida.
- `seed-sample-chain-k5`: `write_paged_chain(None)`, cuatro nodos anidados con el *k* por defecto (todo cabe en la página raíz).
- `seed-sample-chain-k1`: `write_paged_chain(Some(1))`, los mismos cuatro nodos con *k* = 1: página raíz y tres páginas hijas encadenadas.

`seed-sample` y `seed-sample-chain-*` están en `corpus/fuzz_paged_reader/`; `seed-sample-rev9` y `seed-sample-paged-chain-*` (mismos bytes) en `corpus/fuzz_format_reader/`.

| Fichero | Bytes | SHA-256 |
| --- | ---: | --- |
| `seed-sample` (= `seed-sample-rev9`) | 546 | `ac841505716279360897d5c51adcce97e7368336a38f27bfe456b93848738b54` |
| `seed-sample-chain-k5` (= `seed-sample-paged-chain-k5`) | 883 | `992d552e3e2fc70afe0893fc7ed037b3e5fda2424130b2d217484ffec0239c38` |
| `seed-sample-chain-k1` (= `seed-sample-paged-chain-k1`) | 991 | `b15c2387b4022325f1f3f0b90a867553872ab8849e6b41919dc937b8459a0920` |

Válido para esta revisión del escritor (nightly fijado, LZ4 determinista). No se midió ningún tiempo.
