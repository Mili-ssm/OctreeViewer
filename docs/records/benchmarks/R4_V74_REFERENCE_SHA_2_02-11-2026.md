# Referencia SHA del fixture `v7.4-pre`, origen de marco en cuantos (R4 tarea 4, 02-11-2026)

Registro fechado e inmutable. Sustituye las SHA de `seed-sample-rev8*` del [registro anterior](R4_V74_REFERENCE_SHA_02-11-2026.md): el índice `v7.4-pre` guarda ahora el origen de marco como 3 × `i64` cuantos (24 bytes más), dentro del nodo raíz, en vez de derivarlo de la esquina de la raíz (necesario para raíces poco profundas, R4-Q5/Q6). Las demás semillas no cambian.

Fixture: igual que el registro anterior (`fuzz/src/bin/gen_seeds.rs`, `write_revision(FormatVersion::V7_PRE4)`); el origen de marco es la esquina de la raíz.

| Fichero | Bytes | SHA-256 |
| --- | ---: | --- |
| `seed-sample-rev8` (monolítico) | 645 | `ef004c23df0bacc6a62808318b1cd780154876cbc1182f786bccfc8fc330708f` |
| `seed-sample-rev8-split` (u32 + primario + atributos) | 713 | `86af25828325b570745ce0f91445830360c97fbdf82ddf580d7a8e0bf5356424` |

Válido para esta revisión del escritor (nightly fijado, LZ4 determinista). No se midió ningún tiempo.
