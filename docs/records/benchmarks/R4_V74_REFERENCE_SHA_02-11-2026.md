# Referencia SHA del fixture `v7.4-pre` (R4 tarea 3, 02-11-2026)

Registro fechado e inmutable. Complementa las [referencias SHA R1](R1_REFERENCE_SHA_2026-10-29.md) con la primera salida `v7.4-pre` del escritor de `viewer_format` (no del CLI: el conversor `v7.4-pre` llega en la tarea 4).

Fixture: `fuzz/src/bin/gen_seeds.rs`, `write_revision(FormatVersion::V7_PRE4)`; dos puntos, raíz en nivel 48, CRS `EPSG:4978` con WKT, escala/offset 0,001/0. Se genera con `cd fuzz && cargo run --bin gen_seeds` y se versiona en `fuzz/corpus/fuzz_format_reader/`.

| Fichero | Bytes | SHA-256 |
| --- | ---: | --- |
| `seed-sample-rev8` (monolítico) | 621 | `eeaed66c692c1eb2811b405102860a9d3042af6a2ccf3830b2df681f36de6754` |
| `seed-sample-rev8-split` (u32 + primario + atributos) | - | `1a18c5fe4c605d53b61a59892fc9a66565fe9dec6b5b5fafb94c8ef364e75635` |

Válido para esta revisión del escritor (nightly fijado, LZ4 determinista). No se midió ningún tiempo.
