# Referencia SHA de las semillas `v7.5-pre` (R5, 02-11-2026, segunda)

Registro fechado e inmutable. **Sustituye en la práctica** al [primero](R5_V75_REFERENCE_SHA_02-11-2026.md): el 02-11-2026 el propietario decidió que `v7.5-pre` vuelve a la pareja `.oct` + `.octattrs` ([ADR 0001](../../adr/0001-pareja-separada-oct-octattrs.md)) y que el id de nodo es `(página, índice)` sin límite de profundidad, así que los bytes cambian. Generadas por `fuzz/src/bin/gen_seeds.rs` (`cargo run --bin gen_seeds` desde `fuzz/`). Las semillas de `rev0`…`rev8` no cambian.

Fixtures:

- `seed-sample` / `seed-sample-rev9`: `write_revision(FormatVersion::V7_PRE5)`, un nodo y una página; es el `.oct` solo. `seed-sample-rev9-split`: longitud `u32` LE del `.oct`, `.oct` y `.octattrs`.
- `seed-sample-chain-k5` / `-k1` (`seed-sample-paged-chain-*` en `fuzz_format_reader`): cuatro nodos anidados con *k* por defecto y con *k* = 1. Las variantes `-split` en `fuzz_format_reader` añaden el `.octattrs`.

| Fichero | Bytes | SHA-256 |
| --- | ---: | --- |
| `seed-sample` (= `seed-sample-rev9`) | 536 | `cfa1be10ed960c33a67f6697f7ae95ac258c3d769befa26d7f17ee8b6cbf05a2` |
| `seed-sample-rev9-split` | 614 | `51f75ba008805252dd00c145246827b5b12c113f4f38f1f549389c6eba278205` |
| `seed-sample-chain-k5` (= `seed-sample-paged-chain-k5`) | 843 | `a7fe6363401cb76343c4fe4ff775a3693fd3da3198489fda41da580903d0c679` |
| `seed-sample-paged-chain-k5-split` | 951 | `2458d50b7e0cd99563102f1176d31bbc79199985921f99d393130f744b98d4db` |
| `seed-sample-chain-k1` (= `seed-sample-paged-chain-k1`) | 959 | `6a113d6832175a94a65287d99aef339ae1c0a008e88d26e0e43882a19bafeb5e` |
| `seed-sample-paged-chain-k1-split` | 1067 | `88d1a94fa73e02c69e88af2311cd9ea2da30323116ab5125b28b4ac1d378d78e` |

Válido para esta revisión del escritor (nightly fijado, LZ4 determinista). No se midió ningún tiempo.
