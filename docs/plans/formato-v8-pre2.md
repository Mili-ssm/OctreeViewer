# Formato V8, revisión v8.2-pre: zstd en columnas de `.octattrs` (plan de implementación)

Estado: propuesto · 20-11-2026 · experimento de D3 decidido por el propietario tras medir v8.1-pre

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** medir si comprimir con zstd parte de `.octattrs` compensa frente a LZ4 en todo (v8.1-pre). Se comparan
cuatro esquemas fijos en tamaño, tiempo de conversión, tiempo de carga y rendimiento de render, y el propietario
elige uno o se queda con LZ4.

**Architecture:** revisión nueva `v8.2-pre` = v8.1-pre más un códec de payload `Zstd`, que leen el lector nativo y el
de wasm (el mismo `attribute_codec::decode`). Qué columnas usan zstd lo fija una opción experimental del conversor,
`--attr-codec lz4|cold|cold-rgb|all`, por clase de columna (no por id dinámico, como pidió el propietario). El
fichero no guarda la opción: cada payload lleva su códec, así que el lector no la necesita. Al congelar la V8 se
fija un esquema y la opción desaparece (D4).

**Tech Stack:** Rust, wasm-bindgen, el crate de zstd que elija la tarea 0 (en la caché local: `zstd` 0.13.5 con
`zstd-sys` 2.1.3 en C, y `ruzstd` 0.8.3 en Rust puro).

**Spec:** [formato-v8.md](formato-v8.md), «Resoluciones tras medir v8.1-pre» (D3, D4) y «Medida de zstd
(20-11-2026)»; [FORMAT_V8_PRE1](../reference/format/FORMAT_V8_PRE1.md).

## Global Constraints

- **v7 y v8.1-pre no cambian:** sus SHA fijados, el digest de `tools/format_parity` y los SHA del
  [registro de v8.1-pre](../records/benchmarks/FORMATO_V8_PRE1_20-11-2026.md) siguen iguales en todas las tareas.
- **Paridad web/CLI (RQ-08):** el conversor web produce los mismos bytes que el CLI en `v8.2-pre` con cada esquema,
  así que el codificador zstd es el mismo código en nativo y en wasm32.
- **zstd nivel 3** (el codo medido; el nivel 6 gana 1–2 puntos y codifica 2,5–3 × más lento).
- **Esquemas** (columnas del esquema LAS de `las_attribute_schema`, `viewer_core/src/las.rs:22`, más los extra
  bytes, que van al final del esquema):
  - `lz4`: ninguna en zstd (bytes como v8.1-pre salvo la identidad).
  - `cold`: zstd en las columnas que el render no lee: `return_number`, `number_of_returns`,
    `scan_angle_degrees`, `user_data`, `point_source_id`, `gps_time`, `nir` y los extra bytes.
  - `cold-rgb`: `cold` más `rgb`.
  - `all`: todos los payloads de `.octattrs` (también intensidad, clase y el color de pantalla de los LOD).
  - La geometría (`.oct`) sigue en LZ4 siempre.
- En una columna en zstd se mantienen los candidatos actuales (constante, diccionario, planos de bytes) y su elección
  por tamaño, pero zstd sustituye a LZ4 en cada etapa de entropía. La elección es determinista.
- Decodificar un payload zstd está acotado por su `raw_length` del directorio: un payload corrupto da error, nunca
  pánico ni una reserva sin límite.
- `FormatVersion::V<n>` solo dentro de `viewer_format`. Sin `cargo clean`; worktree con su propio target; salidas
  grandes en el `target/` del worktree, no en `/tmp` (tmpfs de 16 GB). Código y commits en inglés, documentos en
  español, sin menciones a asistentes.

## Review Focus

1. **Payload zstd corrupto o truncado:** error de formato con el bloque, sin pánico ni reserva por encima de `raw_length`.
2. **Mismo fichero, otro esquema:** un lector no necesita saber el esquema; un fichero `cold` y uno `all` se leen
   igual de bien con el mismo binario, y los valores coinciden con los de `lz4`.
3. **Web frente a CLI:** bytes iguales con los cuatro esquemas, también con varios hilos.
4. **Nube sin columnas frías** (solo intensidad y clase): `cold` produce los mismos bytes de payload que `lz4`.
5. **Hojas con color derivado de `rgb` en zstd (`cold-rgb`, `all`):** el color de pantalla leído es idéntico al de
   `lz4`.

---

### Task 0: Choose the zstd encoder (spike, no commit to production code)

- [ ] **Step 1:** comprobar si `zstd`/`zstd-sys` compila para `wasm32-unknown-unknown` con el toolchain local
  (`cargo check --target wasm32-unknown-unknown` en un crate desechable fuera del workspace).
- [ ] **Step 2:** medir en una muestra de payloads de Albuera (la del experimento del 20-11-2026, semilla fija) la
  relación y la velocidad de codificación de `ruzstd` 0.8.3 (sus niveles disponibles) frente a `zstd` nivel 3, y la
  decodificación en wasm (node) de ambos.
- [ ] **Step 3: decisión.** C `zstd` en las dos plataformas si compila para wasm32. Si no, `ruzstd` para codificar
  y decodificar si su relación queda a ≤ 3 puntos de la del nivel 3. Si ninguna se cumple, parar y llevarlo al
  propietario. Anotar la decisión y las cifras en la sección «Codificador» de este plan.

### Task 1: v8.2-pre identity

**Files:** los mismos sitios que la identidad de v8.1-pre (commit `37a75c3`): `crates/viewer_format/src/{version.rs,layout.rs,paged_index.rs,reader.rs,prepare.rs}`,
`crates/viewer_convert/src/main.rs`, `crates/viewer_convert/tests/cli.rs`, `crates/viewer_convert_core/src/options.rs`,
`crates/viewer_wasm/src/core_convert.rs`, `examples/web-host/converter-ui.mjs` (y su test),
`docs/reference/format/FORMAT_V8_PRE2.md` (nuevo).

**Interfaces:**
- Produces: `FormatVersion::V8_PRE2` (`V8Pre(2)`, texto `v8.2-pre`, en `WRITABLE`), `StorageLayout::V8Pre2` con las
  mismas capacidades que `V8Pre1`, más `supports_zstd()` (cierto solo en `V8Pre2`), magics `OCTVI82P` / `OCTVE82P`.

- [ ] **Step 1: failing tests:** `v8_pre2_parses_and_prints`; `v8_pre2_is_writable`;
  `v8_pre2_has_the_v8_pre1_capabilities_plus_zstd`; `cli_accepts_v8_pre2`.
- [ ] **Step 2:** fallan.
- [ ] **Step 3:** implementar; un fichero `v8.2-pre` sin zstd es v8.1-pre con otra identidad.
- [ ] **Step 4:** tests del workspace (SHA de v7 y v8.1-pre intactos), clippy nativo y wasm32 → OK.
- [ ] **Step 5:** commit `feat(format): v8.2-pre identity`.

### Task 2: Zstd payload codec

**Files:** `crates/viewer_format/src/codec.rs` (enum `BlockCodec`, `:6-29`), `crates/viewer_format/src/attribute_codec.rs`
(candidatos `encode_raw_or_constant` ~92, `encode` ~100-160, `encode_byte_planes` ~200-225, `decode` ~236),
`crates/viewer_format/src/prepare.rs` (`append_payload` ~826), `crates/viewer_format/Cargo.toml`.

**Interfaces:**
- Consumes: `supports_zstd()` (tarea 1); el crate de la tarea 0.
- Produces: `BlockCodec::Zstd = 5` (válido solo con `supports_zstd()`); `EntropyCoder { Lz4, Zstd }` como parámetro de
  los candidatos de `attribute_codec`, que hoy fijan LZ4; `append_payload(..., coder: EntropyCoder)`.

- [ ] **Step 1: failing tests:** `zstd_payload_round_trips` (raw, planos de bytes y diccionario con
  `EntropyCoder::Zstd`); `zstd_is_rejected_before_v8_pre2` (un directorio v8.1-pre con códec 5 es error);
  `corrupt_zstd_payload_is_an_error` (bytes cambiados, truncado y `raw_length` menor que lo que descomprime: error, sin
  pánico); `lz4_coder_keeps_v8_pre1_bytes` (con `EntropyCoder::Lz4` los payloads son los de hoy).
- [ ] **Step 2:** fallan.
- [ ] **Step 3:** implementar; la decodificación escribe en un búfer de `raw_length` y falla si sobra o falta.
- [ ] **Step 4:** tests del workspace, clippy nativo y wasm32, paridad sintética de `format_parity` → OK.
- [ ] **Step 5:** commit `feat(format): zstd entropy stage for v8.2-pre payloads`.

### Task 3: `--attr-codec` scheme in the converter (CLI and web)

**Files:** `crates/viewer_format/src/metadata.rs` o `options` del escritor (dónde vive la política por columna),
`crates/viewer_convert_core/src/options.rs`, `crates/viewer_convert/src/main.rs`, `crates/viewer_wasm/src/core_convert.rs`,
`examples/web-host/converter-ui.mjs`, `tools/convert_parity/{src/lib.rs,run.mjs}`.

**Interfaces:**
- Consumes: `EntropyCoder`, `append_payload(..., coder)` (tarea 2).
- Produces: `AttributeCodecScheme { Lz4, Cold, ColdRgb, All }` (texto `lz4|cold|cold-rgb|all`), con
  `coder_for(column_name, is_display_colour) -> EntropyCoder` según las listas de Global Constraints; opción del
  escritor y del conversor (`--attr-codec`, por defecto `lz4`); solo válida con `supports_zstd()` (en otra versión,
  `lz4` o error); en `tools/convert_parity`, casos `v8.2-pre` con los cuatro esquemas, uno con varios hilos.

- [ ] **Step 1: failing tests:** `scheme_assigns_coders_by_column_class` (las cuatro listas, extra bytes incluidos);
  `cold_without_cold_columns_matches_lz4` (Review Focus 4); `every_scheme_reads_back_the_same_values` (los cuatro
  ficheros dan los mismos valores por bloque, también el color de pantalla de las hojas con `rgb` en zstd: Review
  Focus 2 y 5); `attr_codec_is_rejected_on_other_versions`; `cli_converts_and_verifies_each_scheme`.
- [ ] **Step 2:** fallan.
- [ ] **Step 3:** implementar.
- [ ] **Step 4:** `FUZZ_CHECK=0 scripts/check.sh --offline` con `FORMAT_PARITY_REAL_INPUT` → todo OK, con los casos
  nuevos de paridad web/CLI.
- [ ] **Step 5:** commit `feat(convert): --attr-codec scheme for v8.2-pre`.

### Task 4: Measure the four schemes and record them

**Files:** crear `docs/records/benchmarks/FORMATO_V8_PRE2_<dd-mm-aaaa>.md` y sus datos en
`docs/records/benchmarks/data/formato-v8-pre2-<dd-mm-aaaa>/`; completar `docs/reference/format/FORMAT_V8_PRE2.md`;
actualizar el `Estado:` de este plan y de `formato-v8.md`, `docs/estado.md`.

- [ ] **Step 1: CLI.** Belvedere, Albuera y Majadas con `v8.2-pre` y los cuatro esquemas, una conversión cada vez,
  con `scripts/measure_temp_disk.sh`. Se mide el tamaño de `.oct` y de `.octattrs`, el tiempo, el RSS y `verify`.
  Albuera y Majadas se repiten 2 veces con `lz4` y con `all` para conocer el ruido.
- [ ] **Step 2: navegador.** Con `examples/web-host/probe-orbit-load.mjs` en GPU real (`mode` = `timing`) y Albuera,
  4 ejecuciones por esquema en orden alterno. Se mide el tiempo hasta el objetivo de px en modo RGB (lee `rgb` en las
  hojas) y en modo `gps_time` (lee una columna fría), y el rendimiento de render de la órbita (intervalo de frame
  p50/p95).
- [ ] **Step 3:** registro fechado con las series, la orden exacta y el equipo. Sin recomendación de esquema: la
  decisión es del propietario (D3). Se completa `FORMAT_V8_PRE2.md` y se actualizan los documentos;
  `check_docs_links.py` y `check_docs_index.py` → OK.
- [ ] **Step 4:** commit `docs: measure v8.2-pre compression schemes`.

## Codificador

Lo rellena la tarea 0.
