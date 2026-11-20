# Formato V8, revisión v8.2-pre: zstd en columnas de `.octattrs` (plan de implementación)

Estado: medido · 20-11-2026 · experimento de D3; el propietario aprobó el plan («hagamos la prueba, pero creo que al final nos quedaremos con LZ4»); tareas 0–4 hechas ([registro](../records/benchmarks/FORMATO_V8_PRE2_20-11-2026.md)); falta la decisión del propietario sobre D3

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
  pánico ni una reserva sin límite. Tramas zstd sin número mágico ni checksum (el payload ya lleva CRC).
- **Codificador:** C `zstd` nivel 3 en nativo y en wasm32. `ruzstd` 0.8.3 solo codifica en `Fastest` (≈ nivel 1;
  `Default`, `Better` y `Best` están sin implementar), así que no sirve de respaldo: si C zstd no compila para wasm32
  o no da los mismos bytes que en nativo, se para y se lleva al propietario.
- **Ahorro esperado sobre v8.1-pre, no sobre v7:** las cifras de la medida de zstd son de v7; v8.1-pre ya quitó tiempo
  GPS y RGB de 16 bits de las filas LOD, así que se infiere ≈ −3–4 % (`cold`) y ≈ −7 % (`cold-rgb`) en Albuera. La
  tarea 0 lo proyecta antes de construir nada.
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

### Task 0: Feasibility and projection (spike, no commit to production code)

- [x] **Step 1:** comprobar que `zstd`/`zstd-sys` 2.1.3 compila para `wasm32-unknown-unknown` con el toolchain local
  (crate desechable fuera del workspace).
- [x] **Step 2:** en payloads de una muestra de bloques **v8.1-pre** de Albuera (semilla fija; hojas y LOD), codificar
  con C zstd nivel 3 en nativo y en wasm (node) y comparar el hash de los bytes por tramos de tamaño (≤ 16 KB,
  ≤ 128 KB, ≤ 256 KB, > 256 KB); medir la velocidad de decodificación en wasm.
- [x] **Step 3:** proyectar el tamaño total de Albuera v8.1-pre con cada esquema (`cold`, `cold-rgb`, `all`), con zstd
  en todas las etapas de entropía (también planos de bytes y la tabla del diccionario).
- [x] **Step 4: puerta.** Parar y llevar las cifras al propietario antes de la tarea 1 si: C zstd no compila para
  wasm32, algún hash nativo/wasm difiere, la decodificación en wasm baja de 500 MB/s, o `cold-rgb` ahorra < 5 % del
  total de Albuera. Anotar las cifras en «Codificador».

### Task 1: v8.2-pre identity

**Files:** los mismos sitios que la identidad de v8.1-pre (commit `37a75c3`): `crates/viewer_format/src/{version.rs,layout.rs,paged_index.rs,reader.rs,prepare.rs}`,
`crates/viewer_convert/src/main.rs`, `crates/viewer_convert/tests/cli.rs`, `crates/viewer_convert_core/src/options.rs`,
`crates/viewer_wasm/src/core_convert.rs`, `examples/web-host/converter-ui.mjs` (y su test),
`docs/reference/format/FORMAT_V8_PRE2.md` (nuevo).

**Interfaces:**
- Produces: `FormatVersion::V8_PRE2` (`V8Pre(2)`, texto `v8.2-pre`, en `WRITABLE`), `StorageLayout::V8Pre2` con las
  mismas capacidades que `V8Pre1`, más `supports_zstd()` (cierto solo en `V8Pre2`), magics `OCTVI82P` / `OCTVE82P`.

- [x] **Step 1: failing tests:** `v8_pre2_parses_and_prints`; `v8_pre2_is_writable`;
  `v8_pre2_has_the_v8_pre1_capabilities_plus_zstd`; `cli_accepts_v8_pre2`.
- [x] **Step 2:** fallan.
- [x] **Step 3:** implementar; un fichero `v8.2-pre` sin zstd es v8.1-pre con otra identidad.
- [x] **Step 4:** tests del workspace (SHA de v7 y v8.1-pre intactos), clippy nativo y wasm32 → OK.
- [x] **Step 5:** commit `feat(format): v8.2-pre identity`.

### Task 2: Zstd payload codec

**Files:** `crates/viewer_format/src/codec.rs` (enum `BlockCodec`, `:6-29`), `crates/viewer_format/src/attribute_codec.rs`
(candidatos `encode_raw_or_constant` ~92, `encode` ~100-160, `encode_byte_planes` ~200-225, `decode` ~236),
`crates/viewer_format/src/prepare.rs` (`append_payload` ~826), `crates/viewer_format/src/read_plan.rs` (~211),
`crates/viewer_format/src/index_codec.rs` (~562-595, ~969-970), `crates/viewer_format/Cargo.toml`, `tools/format_parity`.

**Interfaces:**
- Consumes: `supports_zstd()` (tarea 1); el crate de la tarea 0.
- Produces: `EntropyCoder { Lz4, Zstd }` como parámetro de los candidatos de `attribute_codec`, que hoy fijan LZ4
  (también `BytePlanes`, `:224` y `:272`); `BlockCodec::Zstd = 5` y `BlockCodec::BytePlanesZstd = 6` (válidos solo
  con `supports_zstd()`); etiqueta interna `2` = zstd en la tabla y los ids del diccionario (`:302-306`, hoy solo
  0/1); `append_payload(..., coder: EntropyCoder)`; la estimación de memoria de lectura por códec (`read_plan.rs:211`)
  cuenta el contexto de decodificación de zstd; la validación de códecs del índice (`index_codec.rs:562-595`,
  `:969-970`) acepta 5 y 6 solo en v8.2-pre.

- [x] **Step 1: failing tests:** `zstd_payload_round_trips` (raw, planos de bytes y diccionario con
  `EntropyCoder::Zstd`, cada disposición); `zstd_is_rejected_before_v8_pre2` (un directorio v8.1-pre con códec 5 es error);
  `corrupt_zstd_payload_is_an_error` (bytes cambiados, truncado y `raw_length` menor que lo que descomprime: error, sin
  pánico); `lz4_coder_keeps_v8_pre1_bytes` (con `EntropyCoder::Lz4` los payloads son los de hoy).
- [x] **Step 2:** fallan.
- [x] **Step 3:** implementar; la decodificación escribe en un búfer de `raw_length` y falla si sobra o falta.
- [x] **Step 4:** tests del workspace, clippy nativo y wasm32, paridad sintética de `format_parity` → OK.
- [x] **Step 5:** commit `feat(format): zstd entropy stage for v8.2-pre payloads`.

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

- [x] **Step 1: failing tests:** `scheme_assigns_coders_by_column_class` (las cuatro listas, extra bytes incluidos);
  `cold_without_cold_columns_matches_lz4` (Review Focus 4); `every_scheme_reads_back_the_same_values` (los cuatro
  ficheros dan los mismos valores por bloque, también el color de pantalla de las hojas con `rgb` en zstd: Review
  Focus 2 y 5); `attr_codec_is_rejected_on_other_versions`; `cli_converts_and_verifies_each_scheme`.
- [x] **Step 2:** fallan.
- [x] **Step 3:** implementar.
- [x] **Step 4:** `FUZZ_CHECK=0 scripts/check.sh --offline` con `FORMAT_PARITY_REAL_INPUT` → todo OK, con los casos
  nuevos de paridad web/CLI; además, paridad nativo/wasm de bloques reales de **Albuera** con `cold` y `all`
  (Belvedere tiene constantes todas las columnas salvo el RGB y no ejercita zstd).
- [x] **Step 5:** commit `feat(convert): --attr-codec scheme for v8.2-pre`.

### Task 4: Measure the four schemes and record them

**Files:** crear `docs/records/benchmarks/FORMATO_V8_PRE2_<dd-mm-aaaa>.md` y sus datos en
`docs/records/benchmarks/data/formato-v8-pre2-<dd-mm-aaaa>/`; completar `docs/reference/format/FORMAT_V8_PRE2.md`;
actualizar el `Estado:` de este plan y de `formato-v8.md`, `docs/estado.md`.

- [x] **Step 1: CLI.** Belvedere, Albuera y Majadas con `v8.2-pre` y los cuatro esquemas, una conversión cada vez,
  con `scripts/measure_temp_disk.sh`. Se mide el tamaño de `.oct` y de `.octattrs`, el tiempo, el RSS y `verify`.
  Albuera y Majadas se repiten 2 veces con `lz4` y con `all` para conocer el ruido. Se guardan (en el `target/` del
  worktree, ~40 GB) las cuatro salidas de Albuera que usa el paso 2. Tiempo del conversor web en Belvedere con `lz4`
  y `all` (sonda de conversión existente).
  Hecho el 20-11-2026 (2a81060). El tiempo del conversor web no se midió: no existe sonda de conversión en
  navegador (la referencia del 29-10 es nativa).
- [x] **Step 2: navegador.** Opción de modo de color en `examples/web-host/probe-orbit-load.mjs` (hoy no la tiene).
  Con GPU real (`mode` = `timing`) y Albuera, ≥ 8 ejecuciones por esquema en orden alterno, en modo RGB (el render
  lee `rgb` en las hojas; no lee columnas frías) y en modo `gps_time` (lee una columna fría). Métrica principal:
  `decodeMs` (`async_format_reader.rs:161`); además el tiempo hasta el objetivo de px y el intervalo de frame p50/p95
  de la órbita. Control A/A: `cold` frente a `lz4` en modo RGB (no deberían diferir). Si la dispersión A/A es ≥ que la
  diferencia `lz4`/`all`, la diferencia de carga se declara no medible.
  Cautela (investigación del 20-11-2026): las velocidades de decodificación en wasm de la tarea 0 (LZ4 ~5 GB/s y zstd
  ~2 GB/s por flujo) superan todo lo publicado para wasm en navegador (LZ4 0,35–1,4 GB/s, sin cifras fiables de zstd
  ni meshopt en wasm); seguramente son de caché caliente. Solo cuentan las cifras de este paso, con datos reales servidos por red.
  Además de la carga local, un brazo por red (`--remote`, servidor RED-REF por defecto) con `lz4` y `all` en modo
  RGB, 8 ejecuciones cada uno: mide si leer menos bytes compensa la decodificación más lenta.
- [x] **Step 3:** registro fechado con las series, la orden exacta y el equipo. Sin recomendación de esquema: la
  decisión es del propietario (D3). Se completa `FORMAT_V8_PRE2.md` y se actualizan los documentos;
  `check_docs_links.py` y `check_docs_index.py` → OK.
- [x] **Step 4:** commit `docs: measure v8.2-pre compression schemes`.

## Codificador

Cifras de la tarea 0 (20-11-2026, equipo del propietario, node 22, clang 23.1.5 para wasm32). Puerta: **superada**.

| Medida | Resultado |
| --- | --- |
| `zstd` 0.13.5 / `zstd-sys` 2.1.3 (C, features `experimental`, sin `default`) para `wasm32-unknown-unknown` | Compila sin errores con el toolchain local y `--offline` (clang apunta a wasm32; `zstd-sys` aporta sus shims de `malloc`/`memcpy`); el módulo no importa nada |
| Bytes nativo frente a wasm (SHA-256 por payload, codificador completo con candidatos y nivel 3) | 5 389 de 5 389 iguales: ≤ 16 KB 853/853, ≤ 128 KB 3 440/3 440, ≤ 256 KB 796/796, > 256 KB 300/300 |
| Decodificación en wasm, mediana de 7 | Etapas de entropía de los flujos reales: zstd 2 040–2 050 MB/s, LZ4 5 040–5 060 MB/s. Bloque completo (con planos y diccionario): zstd 630–680 MB/s, LZ4 550–560 MB/s; una primera pasada dio 454 MB/s para zstd (rango 628–964 ms, con ruido de la máquina) |
| Réplica del codificador LZ4 frente a los bytes guardados | 0 diferencias en los 5 389 payloads |

Muestra: 646 de 25 838 bloques de Albuera v8.1-pre (493 hojas, 153 LOD), 1/40 con semilla 20 261 120 (hash SplitMix64 del id
de bloque), 5 389 payloads. Proyección por columna: bytes guardados de todo el fichero × (zstd / LZ4 de la muestra). Total de
v8.1-pre: 9 362 079 659 B (`.oct` 4 562 037 055 + `.octattrs` 4 800 042 604).

| Esquema | `.octattrs` proyectado | % del `.octattrs` | Total proyectado | % del total | Ahorro |
| --- | --- | --- | --- | --- | --- |
| `lz4` | 4 800 042 604 | 100 % | 9 362 079 659 | 100 % | 0 |
| `cold` | 4 512 944 105 | 94,02 % | 9 074 981 160 | 96,93 % | −3,07 % |
| `cold-rgb` | 4 273 628 217 | 89,03 % | 8 835 665 272 | 94,38 % | −5,62 % |
| `all` | 3 919 094 854 | 81,65 % | 8 481 131 909 | 90,59 % | −9,41 % |

Sensibilidad: con otras 5 semillas (623–709 bloques) el ahorro es 3,05–3,09 % (`cold`), 5,60–5,76 % (`cold-rgb`) y
9,40–9,59 % (`all`). Relación zstd/LZ4 por columna: color de pantalla de LOD 0,886; intensidad 0,768; clase 0,763; rgb 0,841;
tiempo GPS 0,756; ángulo de escaneo 0,576; `point_source_id` 0,484; el resto (< 25 MB cada una) ≈ 0,6–1,0. `las_flags`
(extra bytes, ≈ 20 KB) cuenta como fría. Los zstd se codifican sin número mágico, sin checksum y sin tamaño de contenido en
la cabecera de trama (el directorio ya guarda `raw_length`).

Código desechable y registro completo fuera del repositorio: `target/v82-spike/` (código) e informe de la tarea 0 en
`.superpowers/sdd/formato-v8-pre2/task-0-report.md`. Orden: `viewer_convert ALBUERA.laz target/v82-spike/out/albuera.oct
--format-version v8.1-pre` (release, 11 min); `native out/albuera.oct out/albuera.octattrs data` (recorre las 3 233 páginas con
`PagedReader`, muestrea y recodifica); `cargo build --release --target wasm32-unknown-unknown -p wasmcodec`;
`node run.mjs wasmcodec.wasm samples.bin native_zstd.bin`.
