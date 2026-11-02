# Referencia SHA del fixture `v7.4-pre`, extensión de datos (R4, 02-11-2026)

Registro fechado e inmutable. Sustituye las SHA de `seed-sample-rev8*` del [registro anterior](R4_V74_REFERENCE_SHA_2_02-11-2026.md): el índice `v7.4-pre` guarda ahora, justo después del origen de marco, la esquina máxima cuantizada de los datos (3 × `i64` cuantos, 24 bytes más). Con el origen como mínimo es la extensión ajustada del dataset, que el visor usa para encuadrar la cámara (los límites de nodo siguen siendo cubos del retículo). Las demás semillas no cambian.

Fixture: igual que el registro anterior (`fuzz/src/bin/gen_seeds.rs`, `write_revision(FormatVersion::V7_PRE4)`); el origen de marco es la esquina de la raíz y el máximo de datos es esa esquina más 1 000 cuantos por eje.

| Fichero | Bytes | SHA-256 |
| --- | ---: | --- |
| `seed-sample-rev8` (monolítico) | 669 | `6a8119403a9a9f2b8d467b810611fdb66fc6603c6247067cc582d1cf90e3e05c` |
| `seed-sample-rev8-split` (u32 + primario + atributos) | 737 | `975aa8bc3a06ee01dd9882beefc8722f2e5636e248381706884911022feeaef6` |

Válido para esta revisión del escritor (nightly fijado, LZ4 determinista). No se midió ningún tiempo.
