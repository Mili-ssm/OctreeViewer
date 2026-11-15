# Referencia SHA de las semillas `v7.5-pre` (R3-T0b, 03-11-2026)

Registro fechado e inmutable. **Sustituye en la práctica** a la [segunda referencia](R5_V75_REFERENCE_SHA_02-11-2026_2.md): el bloque de metadatos de `v7.5-pre` incorpora el espaciado de referencia por nube (`f64`, 8 bytes; RF-05, R3-Q4), así que los bytes cambian. Generadas por `fuzz/src/bin/gen_seeds.rs` (`gen_seeds` ejecutado desde `fuzz/`). Las semillas de `rev0`…`rev8` no cambian. Los fixtures son los de la referencia anterior.

| Fichero | Bytes | SHA-256 |
| --- | ---: | --- |
| `seed-sample` (= `seed-sample-rev9`) | 561 | `75250c0a4f712159595ce56506527e48b8e4960d791c4f70bacb29bae8ded0e2` |
| `seed-sample-rev9-split` | 639 | `984f6c86cd1cdcdfa563f59e3766eb21a506273c6258e380cb42a40fbc733ea9` |
| `seed-sample-chain-k5` (= `seed-sample-paged-chain-k5`) | 868 | `e8ca5c301f1623f427eefa69b16461c318ee5fc77854876a35e71a703bc6a046` |
| `seed-sample-paged-chain-k5-split` | 976 | `1a1171d27b41e1daffa701ea38e99943976a12c2d21ddbb3cffc2fdb3eb4448b` |
| `seed-sample-chain-k1` (= `seed-sample-paged-chain-k1`) | 984 | `761b2a42decce0d1396dc562efe4ae94882d89b58765588fa2727fc3101364b2` |
| `seed-sample-paged-chain-k1-split` | 1092 | `8dd064be467d03d1104b8c983154b879ef30d92861682b3a23e61ae6678ba550` |

Válido para esta revisión del escritor (nightly fijado, LZ4 determinista). No se midió ningún tiempo.
