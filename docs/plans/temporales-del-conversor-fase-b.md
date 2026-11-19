# Temporales del conversor, fase B: spools comprimidos (plan de implementación)

Estado: pendiente · 19-11-2026 · a la espera de la revisión del propietario

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** que los spools nativos del CLI se guarden comprimidos (planos de bytes por registro + LZ4) sin cambiar ni
un byte de la salida, como primer paso hacia RQ-06.

**Architecture:** solo cambia el almacén nativo `FileSpoolStore`. Cada `append` se parte en tramas de registros
completos; cada trama se transpone a planos de bytes, se comprime con `lz4_flex` y se escribe con una cabecera de
8 bytes. El lector descomprime trama a trama y salta hasta el offset pedido con un índice disperso en RAM. El trait
`SpoolStore` solo gana el tamaño de registro en `create`; el almacén web y el de tests lo ignoran.

**Tech Stack:** Rust (nightly fijado), `lz4_flex` 0.14 (ya en el workspace, parcheado en `vendor/lz4_flex`), bash
para la medida.

**Spec:** [temporales-del-conversor.md](temporales-del-conversor.md) (diseño aprobado el 19-11-2026). La fase A
(escritura por subárbol) tendrá su propio plan cuando esta fase esté medida.

## Global Constraints

- T1: `.oct`/`.octattrs` byte a byte iguales a los de `dc98e5a` con las mismas opciones; la paridad de
  `scripts/check.sh` sigue en verde.
- T4: RSS pico dentro del presupuesto estimado (512 MiB por defecto); ningún búfer nuevo crece con el número de
  spools vivos ni con el tamaño total de la nube salvo el índice disperso (≤ 16 B por cada 64 tramas).
- T5: el ledger (`SpoolLedger`) cuenta bytes **físicos**; `SpoolStore::len()` sigue devolviendo bytes lógicos.
- T6: ante error o cancelación no se publica salida y `.work` se borra, como hoy.
- T7: el conversor web (`HybridSpoolStore`) no cambia de comportamiento; `cargo tree` del núcleo para wasm32 sigue
  sin dependencias nativas (paso «core has no native-only deps» de `check.sh`).
- `SOURCE` (`source.spool`) sigue en crudo: `with_closed_source` y el checkpoint no cambian.
- Sin `cargo clean` ni borrar nada de `target/` salvo los directorios de sonda que cree la tarea (guarda 38 GB de
  datos de prueba). En un worktree: `CARGO_TARGET_DIR` propio y `FORMAT_PARITY_REAL_INPUT` con la ruta absoluta del
  LAZ de Belvedere del checkout principal.
- Código, comentarios y commits en inglés; documentos en español; sin menciones a asistentes en commits.

## Review Focus

1. **Appends que no acaban en un registro completo** (`join_segments` copia en trozos de 64 KiB, `node_job.rs:830`):
   el resto se guarda y se antepone al siguiente append; los bytes leídos son idénticos a los escritos.
2. **Lecturas con offset dentro de una trama y extensiones que acaban a mitad de trama** (`PlannedBlock::read`,
   `block.rs:51-61`): devuelven exactamente `extent.len` bytes desde `extent.offset`.
3. **Lectores concurrentes del mismo spool** (preparación de bloques con `par_iter`): cada lector abre su fichero
   y no comparte estado mutable.
4. **Datos incompresibles**: la trama se guarda en crudo con la marca de la cabecera; nunca ocupa más que
   registros + 8 bytes.
5. **Spool truncado o corrupto en disco**: error (`ConvertError::Io` o `PointCountMismatch`), nunca pánico.

---

### Task 1: Temp-disk measurement script and baseline

**Files:**
- Create: `scripts/measure_temp_disk.sh`
- Modify: `docs/plans/temporales-del-conversor-fase-b.md` (sección «Línea base», al final)

**Interfaces:**
- Produces: `scripts/measure_temp_disk.sh CONVERTER INPUT OUTPUT_DIR [-- EXTRA_ARGS…]`. Imprime una línea
  `peak_work_bytes=<n> wall_s=<s> max_rss_kb=<n> sha_oct=<hex> sha_octattrs=<hex>` y deja la serie de muestras en
  `OUTPUT_DIR/samples.tsv` (`elapsed_s<TAB>work_bytes`). Borra `.oct`/`.octattrs` tras calcular los SHA.

- [ ] **Step 1:** escribir el script: lanza el conversor con `/usr/bin/time -v`, muestrea `du -sb` de
  `OUTPUT_DIR/*.work` cada 1 s mientras el proceso vive (el bucle acaba con el proceso; sin esperas sin límite),
  y falla con código ≠ 0 si el conversor falla.
- [ ] **Step 2:** ejecutar `bash -n scripts/measure_temp_disk.sh` y `shellcheck` si está instalado. Esperado: sin
  errores.
- [ ] **Step 3:** línea base con el binario de `dc98e5a` (`target/release/viewer_convert` del checkout principal,
  recompilado desde `dc98e5a` si falta) sobre Belvedere
  (`data/datasets/belv_2009_digital-aerial_pcd_copc.laz`) y Albuera
  (`data/datasets/sensoforest/SensOFOREST_2026_02_22_Albuera_Pointcloud_UAV_LiDAR_20260503.laz`), salida en
  `target/temp-disk-baseline/`. Anotar las dos líneas en «Línea base» de este plan. Esperado en Albuera: pico
  ≈ 47,9 GB y ≈ 9 min 30 s (medido el 19-11-2026).
- [ ] **Step 4:** commit: `git add scripts/measure_temp_disk.sh docs/plans/temporales-del-conversor-fase-b.md`,
  `git commit -m "tools: temp-disk measurement script and CLI baseline (RQ-06)"`.

### Task 2: The spool store learns the record size

**Files:**
- Modify: `crates/viewer_convert_core/src/pipeline/spool_store.rs:38-45` (trait y `MemSpoolStore`)
- Modify: `crates/viewer_convert_core/src/pipeline/hybrid/store.rs:535+` (impl; tests del fichero)
- Modify: `crates/viewer_convert_core/src/spatial/spool.rs:179-186` (`ChildSpool::flush`)
- Modify: `crates/viewer_convert_core/src/pipeline/node_job.rs:829, 861, 939`
- Modify: `crates/viewer_convert/src/file_spool_store.rs`, `crates/viewer_convert/src/lib_tests.rs:2403`,
  `crates/viewer_convert_core/src/range_tests.rs:447` y el resto de llamadas que dé `cargo check --all-targets`

**Interfaces:**
- Produces: `fn create(&self, id: SpoolId, record_bytes: usize) -> Result<(), ConvertError>` en `SpoolStore`.
  Llamadas: hijos y destino de `join_segments` con `ctx.layout.record_bytes()` / el `layout` del `ChildSpool`;
  LOD con `layout.lod().record_bytes()`; `SOURCE` con el tamaño de registro de la fuente.
  `FileEntry` gana `record_bytes: usize`.

- [ ] **Step 1: failing test** en `file_spool_store.rs` `mod tests`:
  `create_rejects_a_zero_record_size` — `store.create(SpoolId(3), 0)` devuelve error cuyo texto contiene
  `"record size"`.
- [ ] **Step 2:** `cargo test -p viewer_convert --offline file_spool_store` → FAIL (no compila: `create` recibe un
  argumento).
- [ ] **Step 3:** cambiar el trait y todas las implementaciones y llamadas; `FileSpoolStore::create` rechaza `0`
  con `lifecycle_error(id, "create", "record size must be positive")`; los demás almacenes ignoran el valor.
- [ ] **Step 4:** `cargo test --workspace --offline` y
  `cargo clippy --workspace --all-targets --offline -- -D warnings` → PASS.
- [ ] **Step 5:** commit `git commit -am "refactor(convert): spool stores receive the record size at create"`.

### Task 3: Byte-plane LZ4 frame codec

**Files:**
- Create: `crates/viewer_convert/src/spool_frame.rs` (con `mod tests`)
- Modify: `crates/viewer_convert/src/lib.rs` (declarar el módulo `pub(crate)`), `crates/viewer_convert/Cargo.toml`
  (`lz4_flex = "0.14"`)

**Interfaces:**
- Produces:
  - `pub(crate) const FRAME_HEADER_BYTES: usize = 8;`
  - `pub(crate) const MAX_FRAME_RECORDS: usize = 8192;`
  - `pub(crate) fn encode_frame(records: &[u8], record_bytes: usize, out: &mut Vec<u8>)` — añade a `out`
    cabecera + carga. Precondición: `records.len()` es múltiplo de `record_bytes` y ≤ `MAX_FRAME_RECORDS` registros.
  - `pub(crate) struct FrameHeader { pub logical_len: u32, pub stored_len: u32, pub raw: bool }` con
    `fn parse(bytes: [u8; 8]) -> FrameHeader`.
  - `pub(crate) fn decode_frame(header: FrameHeader, payload: &[u8], record_bytes: usize, out: &mut Vec<u8>) -> io::Result<()>`
    — reemplaza el contenido de `out` por los `logical_len` bytes originales; carga corrupta → `io::ErrorKind::InvalidData`.
- Formato de cabecera: `logical_len: u32 LE`, luego `stored_len: u32 LE` cuyo bit 31 marca carga en crudo (sin
  transponer ni comprimir). Carga comprimida: `lz4_flex::block::compress` de los planos (byte *k* de cada registro
  seguidos, *k* = 0..record_bytes). Si la carga comprimida no es más corta que `records.len()`, se guarda en crudo.

- [ ] **Step 1: failing tests**:
  - `frames_round_trip_for_spool_and_lod_records` — para `record_bytes` 42 y 54, registros pseudoaleatorios
    estructurados (coordenadas crecientes, atributos repetidos) de 1, 780 y 8192 registros: `decode(encode(x)) == x`
    y la trama comprimida es más corta que `x`.
  - `incompressible_frames_are_stored_raw` — bytes de un generador pseudoaleatorio: `raw == true` y
    `out.len() == FRAME_HEADER_BYTES + records.len()`.
  - `corrupt_payload_is_invalid_data` — truncar la carga de una trama comprimida: `decode_frame` devuelve
    `ErrorKind::InvalidData`, sin pánico.
- [ ] **Step 2:** `cargo test -p viewer_convert --offline spool_frame` → FAIL (módulo inexistente).
- [ ] **Step 3:** implementar el módulo.
- [ ] **Step 4:** `cargo test -p viewer_convert --offline spool_frame` → PASS; clippy como en la tarea 2.
- [ ] **Step 5:** commit `git commit -m "feat(convert): byte-plane LZ4 frames for spool records"`.

### Task 4: FileSpoolStore writes and reads framed spools

**Files:**
- Modify: `crates/viewer_convert/src/file_spool_store.rs`

**Interfaces:**
- Consumes: `create(id, record_bytes)` (tarea 2); `encode_frame`, `decode_frame`, `FrameHeader`,
  `FRAME_HEADER_BYTES`, `MAX_FRAME_RECORDS` (tarea 3).
- Produces: `type Reader = FramedReader` (nuevo, `impl Read`), que entrega exactamente `extent.len` bytes lógicos
  desde `extent.offset`. `SOURCE` mantiene `io::Take<BufReader<File>>` en crudo: el tipo puede ser un enum de las
  dos variantes.
- Estado por spool en `FileEntry`: `carry: Vec<u8>` (< `record_bytes` bytes pendientes), `physical_len: u64` y
  `sparse_index: Vec<(u64, u64)>` (offset lógico y físico del inicio de cada trama número 0, 64, 128…). El índice se
  completa en `append`; `reader` busca la última entrada ≤ offset, salta ahí y recorre cabeceras hasta la trama que
  contiene el offset.

- [ ] **Step 1: failing tests** (en `mod tests`):
  - `framed_spool_round_trips_unaligned_appends` — `record_bytes = 42`, 20 000 registros añadidos en trozos de
    65 536 bytes: leer `(0, len)` devuelve los mismos bytes; `len()` es lógico.
  - `framed_reads_honour_offsets_inside_and_across_frames` — offsets 0, 42 × 100, 42 × 8191, 42 × 8192 + 42 × 5 y
    el último registro, con longitudes que acaban a mitad de trama: cada lectura coincide con el trozo original.
  - `ledger_counts_physical_bytes` — con diagnóstico, tras escribir registros compresibles,
    `ledger_totals().written` < bytes lógicos; tras `delete`, el vivo vuelve a 0.
  - `source_spool_stays_raw` — el fichero `source.spool` mide exactamente lo escrito.
  - `truncated_spool_reports_an_error` — truncar el fichero en disco a la mitad: leer la extensión completa da
    error o menos bytes, sin pánico.
  - `concurrent_readers_of_one_spool_agree` — 4 hilos leen offsets distintos del mismo spool cerrado y obtienen los
    bytes originales.
  - Ajustar `file_store_matches_the_lifecycle_and_native_names` si comprobaba tamaños de fichero en crudo, sin
    debilitar el resto de aserciones.
- [ ] **Step 2:** `cargo test -p viewer_convert --offline file_spool_store` → FAIL.
- [ ] **Step 3:** implementar. `append`: antepone `carry`, parte en tramas de ≤ `MAX_FRAME_RECORDS` registros
  completos, guarda el resto en `carry`, abre el fichero en modo añadir como hoy y escribe todas las tramas en una
  llamada; `close` escribe `carry` como trama en crudo si no está vacío. El ledger suma y resta bytes físicos.
- [ ] **Step 4:** `cargo test --workspace --offline` → PASS (incluye las conversiones de
  `lib_tests.rs`).
- [ ] **Step 5:** commit `git commit -m "feat(convert): native spools are stored as compressed frames (RQ-06)"`.

### Task 5: Measure phase B and record it

**Files:**
- Create: `docs/records/benchmarks/TEMPORALES_CLI_FASE_B_<dd-mm-aaaa>.md`
- Modify: `docs/plans/temporales-del-conversor.md` (línea `Estado:`), `docs/plans/temporales-del-conversor-fase-b.md`
  (`Estado:` y casillas), `docs/estado.md` («Prioridades pendientes»), `docs/README.md` (índices de planes y
  registros), `docs/requirements.md` (fila de OQ-04 si cambia su texto)

- [ ] **Step 1:** `FUZZ_CHECK=0 scripts/check.sh --offline` (en segundo plano) → todos los pasos en verde, incluida la
  paridad de bloques reales de Belvedere.
- [ ] **Step 2:** `scripts/measure_temp_disk.sh` con el binario nuevo (`cargo build -p viewer_convert --release
  --offline`) en Belvedere, Albuera y Majadas
  (`data/datasets/sensoforest/SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz`). Esperado: SHA de
  Belvedere y Albuera iguales a la línea base (T1); pico de Albuera ≈ 12–14 GB (≈ 2,7–3,3 × LAZ, estimado en el
  diseño); anotar tiempo y RSS (T3, T4).
- [ ] **Step 3:** si la pérdida de tiempo en Albuera supera +25 %, parar y llevar las cifras al propietario (T3).
- [ ] **Step 4:** escribir el registro fechado (línea base, fase B, equipo, perfil `release`, opciones por defecto) y
  actualizar los documentos listados; `python3 scripts/check_docs_links.py` y `python3 scripts/check_docs_index.py`
  → OK.
- [ ] **Step 5:** commit `git commit -m "docs: measure compressed spools against the CLI baseline (RQ-06)"`.

## Línea base

Medido el 19-11-2026 con `scripts/measure_temp_disk.sh`, binario `release` del commit `dc98e5a`, opciones por
defecto, en el equipo del propietario. Sistema de ficheros de la salida: `btrfs rw,relatime,seclabel,compress=zstd:1,ssd,discard=async,space_cache=v2,subvolid=256,subvol=/home`.

`peak_work_bytes` es el tamaño **aparente** (`du -sb`): en un sistema comprimido sobrestima el uso real.
`peak_fs_used_delta_bytes` (uso del sistema de ficheros menos el previo a la ejecución) es un proxy **ruidoso**
del uso real: otros procesos que escriben en `/home` lo alteran.

- Belvedere: `peak_work_bytes=6466222890 peak_fs_used_delta_bytes=1198350336 wall_s=47.1 max_rss_kb=98132 sha_oct=da206a659ea0bfdfbfb3d8141592387cb2dde7133e6a5e44ed9a7a5db3c9f0c4 sha_octattrs=e2f587ab136906d8fc43f2f34c46be0f23a566f3954a8288056b95dae9802323`
- Albuera: `peak_work_bytes=47866199052 peak_fs_used_delta_bytes=20444745728 wall_s=578.6 max_rss_kb=241988 sha_oct=5c15df5e9043003d53fc9e59f741e8420913cdd0088dbd39955f995e44fc8bb6 sha_octattrs=b76636f41c6f59abbfda4fc1c324d9ccfaee72f3073815bb733ca0c6d9b61076`
