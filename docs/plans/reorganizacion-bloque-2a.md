# Reorganización por componentes, bloque 2a (plan de implementación)

Estado: en curso · 28-11-2026 · pre-mortem hecho (enmiendas incorporadas) · pedido por el propietario («avanza con el siguiente bloque», 28-11-2026)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** que cada responsabilidad viva en el crate y el módulo que cambian con ella, sin cambiar
comportamiento ni bytes: la compresión de spools en el núcleo de conversión, el preflight LAS fuera del
crate de dominio, `viewer_core::spatial` en carpeta y los últimos ficheros mal situados de `viewer_wasm`.

**Architecture:** movimientos de código entre módulos y crates con `git mv` o con `split.py`
(copia byte a byte); a mano solo `use`, `mod`, `#[path]`, reexportaciones, visibilidad
(`pub(crate)`/`pub(super)`, o `pub` donde un ítem cambia de crate y el crate de origen lo usaba),
dependencias en `Cargo.toml` y comentarios que nombran rutas.

**Tech Stack:** Rust (workspace nightly), Python 3 stdlib.

**Origen:** revisión de arquitectura del 28-11-2026 (hallazgos F4, F5 y F7 de la revisión de crates y F7
de la de render/wasm) y la revisión final del bloque 1
([plan cerrado](../records/archive/reorganizacion-por-componentes.md)). Rama `refactor/layout-2a`, apilada
sobre `refactor/layout` (PR #123).

**Fuera de este bloque (bloque 2b, después de la PR #122, que cambia esos ficheros):** el ciclo
`pipeline`↔`spatial` de `viewer_convert_core` (`node_job.rs`, `spatial/*`), la división de
`viewer_convert/src/lib_tests.rs` y los shims de reexportación de `viewer_convert/src/pipeline/` y
`diagnostics.rs` (20 de sus 27 usos están en `lib_tests.rs`). No se tocan aquí: `viewer_format` plano
(V8 congelada; se divide cuando haya trabajo de V9), los dobles de prueba de GPU (`FakeGpu`, decisión del
propietario) y `quantized_occupancy_bench.rs` (experimento v6.2 cerrado; retirarlo es decisión aparte).

## Global Constraints

- Ningún cambio de comportamiento ni de bytes de salida (conversión, formato, render).
- Ficheros que toca la PR #122 y que este bloque **no** puede editar: `viewer_convert/src/{executor,las_source,lib_tests,main}.rs`, `viewer_convert/src/pipeline/node_job.rs`, `viewer_convert/examples/real_dataset_bench.rs`, `viewer_convert_core/src/{blocks,options,report}.rs`, `viewer_convert_core/src/pipeline/{executor,node_job}.rs`, `viewer_convert_core/src/spatial/*`, `viewer_wasm/src/core_convert.rs`. Si una tarea los necesita, parar e informar.
- Disposición: padre `x.rs` + carpeta `x/`; tests en hermanos `x_tests.rs` con `#[path]`, salvo que el fichero tenga hijos (un fichero cargado con `#[path]` resuelve sus `mod hijo;` como un `mod.rs`).
- ADR 0014: el núcleo de conversión no hace I/O de sistema (`clippy.toml` de `viewer_convert_core`); los rasgos `std::io::{Read, Write, Seek}` sobre buffers sí están permitidos.
- ADR 0026: los crates son internos; los módulos internos del núcleo se exponen `pub` + `#[doc(hidden)]` como los existentes.
- Commits en inglés, Conventional Commits, sin menciones a IA; no `cargo clean` ni borrar nada en `target/`.
- Puerta por tarea: `.superpowers/sdd/limpieza/gate/gate.sh run <crates>` → `GATE PASS` (compara el multiconjunto de nombres hoja de los 1.315 tests; incluye `check_layout.py`). Un test que cambie de crate cambia de binario pero no de nombre hoja.

## Review Focus

- Paridad de conversión: mover `spool_frame` o el preflight no debe cambiar ningún byte; la puerta no ejecuta las paridades, así que la tarea 1 y la 2 ejecutan además los pasos de paridad de `scripts/check.sh` (`format parity wasm32`, `convert parity wasm32`).
- wasm32: el núcleo debe seguir compilando para `wasm32-unknown-unknown` con `lz4_flex` (el `[patch]` del workspace apunta a `vendor/lz4_flex`).
- Un test que se mueve de crate puede quedar sin compilar en silencio si su módulo no se enlaza: la comparación de nombres hoja lo detecta.
- `check_layout.py` tiene listas de permitidos con detección de entradas obsoletas: actualizar `ALLOWED_PATHS` y `ALLOWED_EDGES` cuando una tarea quite un `#[path]` o cambie una arista.
- Fuzz: `fuzz/` tiene su propio lockfile y usa el preflight; `cargo check` dentro de `fuzz/` (paso `fuzz check` de `check.sh`).

---

### Task 1: `spool_frame` al núcleo de conversión

**Files:** `crates/viewer_convert/src/spool_frame.rs` → `crates/viewer_convert_core/src/pipeline/spool_frame.rs`; `viewer_convert_core/Cargo.toml` (añadir `lz4_flex = "0.14"`), `viewer_convert_core/src/pipeline.rs` o `pipeline/mod.rs` (declaración), `viewer_convert/src/lib.rs:18`, `viewer_convert/src/file_spool_store.rs:4,22`, `scripts/check_layout.py` (`ALLOWED_EDGES` no cambia: `lz4_flex` no es del workspace).

**Interfaces:** Produces `viewer_convert_core::pipeline::spool_frame::*` (mismos nombres y firmas que hoy; visibilidad `pub` + `#[doc(hidden)]` en el módulo) para `FileSpoolStore` hoy y para el `HybridSpoolStore` de la tarea 3 de #122.

- [ ] `git mv` y ajustar `use`; los tests del fichero se mueven con él. Sus ítems `pub(crate)` pasan a `pub` (los usa `viewer_convert`).
- [ ] Tras el commit, comentar en la PR #122 que `spool_frame` vive ahora en `viewer_convert_core::pipeline::spool_frame` y que su tarea 3 (compresión en `HybridSpoolStore`) conviene hacerla después de rebasar sobre esta rama.
- [ ] `cargo check -p viewer_convert_core --target wasm32-unknown-unknown --offline` y `cargo clippy` del núcleo en wasm32 sin avisos (la `clippy.toml` del núcleo no debe saltar).
- [ ] Gate `viewer_convert_core viewer_convert` y, además, los pasos `format parity wasm32` y `convert parity wasm32` de `scripts/check.sh` (copiar sus órdenes); commit `refactor(convert): move the spool frame codec into the core`.

### Task 2: preflight LAS fuera de `viewer_core` — **en espera de decisión del propietario**

El [ADR 0012](../adr/0012-validacion-previa-de-cabeceras-las-laz.md) (aceptado) fija `viewer_core::las::preflight_las` en su decisión: moverlo necesita un ADR nuevo y el visto bueno del propietario (AGENTS.md, «No reabrir decisiones»). Si se aprueba, se aplican además las enmiendas del pre-mortem: el test `sample_las_files_pass` (`las.rs:685-697`, usa `std::fs`) va a `viewer_convert/tests/las_preflight.rs`, no al núcleo; se quedan en `viewer_core` `TARGET_RENDER_EXTENT`, `las_attribute_schema`, `visual_scale_for_bounds` y `checked_visual_scale` (sus usuarios son ficheros de #122); Files añade `viewer_convert/Cargo.toml:21`, `viewer_wasm/Cargo.toml:21`, `fuzz/Cargo.toml` (+`viewer_convert_core`) y `fuzz/Cargo.lock`; se borra el `#[cfg(feature = "laz")]` de `las.rs:308`; `fuzz check` es obligatorio; se divide en mover y quitar la feature.

**Files:** `crates/viewer_core/src/las.rs` (1.049 líneas), `viewer_core/Cargo.toml` (feature `laz`), `viewer_convert_core/src/{lib.rs,error.rs,source.rs}`, `viewer_convert_core/Cargo.toml`, `viewer_convert/tests/las_preflight.rs`, `viewer_convert/src/las_extra_tests.rs:165` (solo si su función se mueve), `fuzz/fuzz_targets/fuzz_las_preflight.rs`, `docs/reference/inventory.md`.

**Interfaces:** Produces `viewer_convert_core::las_preflight` (módulo `pub` + `#[doc(hidden)]`) con `LasPreflightError`, `preflight_las`, `preflight_las_layout`, `check_laz_chunk_table`, `check_layered_chunk`, `check_chunk_bytes`, `LazChunkTableLayout`, `LayeredChunks`, `MAX_LAZ_CHUNK_BYTES` y sus tests. `viewer_core::las` conserva lo que usan otros crates que no son de conversión (`las/extra.rs`, que `viewer_format` usa por `LasExtraByte`, y cualquier ítem como `visual_scale_for_bounds`, `checked_visual_scale`, `las_attribute_schema` o `TARGET_RENDER_EXTENT` que use `viewer_format`, `viewer_render_wgpu` o `viewer_wasm`; comprobar con `git grep` cada uno).

- [ ] Tabla de ítems de `las.rs` con su lista de usuarios por crate (en el informe); se mueven solo los que solo usan `viewer_convert_core`, `viewer_convert`, `fuzz/` y tests.
- [ ] Mover con `split.py` (o, si sale el fichero entero, `git mv` + quitar lo que se queda) a `viewer_convert_core/src/las_preflight.rs` (+ `las_preflight_tests.rs` si los tests eran `mod tests`). Si `viewer_core` ya no usa `laz`, quitar la feature y la dependencia, y quitar `features = ["laz"]` del núcleo.
- [ ] Prueba de aptitud: `! cargo tree -p viewer_core -e normal --offline | grep -q '^.*laz'` si la feature se fue; añadirla a `check_layout.py` como una arista prohibida o un paso de `check.sh` (lo más simple).
- [ ] Fuzz: `cargo check` en `fuzz/` (orden del paso `fuzz check` de `check.sh`); corregir el comentario obsoleto de `fuzz_las_preflight.rs:25`.
- [ ] Gate `viewer_core viewer_convert_core viewer_convert` + las dos paridades; commit `refactor(core): move the LAS preflight into the conversion core`.

### Task 3: `viewer_core::spatial` en carpeta

**Files:** `crates/viewer_core/src/spatial.rs` (1.211 líneas), `spatial_lod_priority.rs`, `spatial_lod_tests.rs`, `dataset.rs`, `lib.rs`, `scripts/check_layout.py` (`ALLOWED_PATHS`).

- [ ] `AttributeDataType` y `AttributeDescriptor` (con su `impl`, ~L155–220) pasan a `dataset.rs` con `split.py`; `lib.rs` reexporta igual.
- [ ] `spatial_lod_priority.rs` → `spatial/lod_priority.rs` y `spatial_lod_tests.rs` → `spatial/lod_tests.rs`, sin `#[path]` (los caminos de módulo no cambian); quitar la entrada `spatial_lod_priority.rs` de `ALLOWED_PATHS`.
- [ ] `docs/reference/inventory.md:22` enlaza `spatial_lod_priority.rs`: actualizarlo en esta tarea.
- [ ] Gate `viewer_core viewer_format viewer_convert_core`; commit `refactor(core): give spatial its folder and move the attribute schema to dataset`.

### Task 4: últimos ficheros mal situados de `viewer_wasm` (4a tests, 4b bindings: un trabajador cada una)

**Files:** `crates/viewer_wasm/src/lod_dataset/cells_stream_tests.rs` (3.266 líneas) y `cells_stream_tests/{colour,real}.rs`; `crates/viewer_wasm/src/bindings/mod.rs` → `bindings.rs`, `lod_request_scheduler.rs` → `bindings/scheduler.rs`, `lib.rs:18,57`; `scripts/check_layout.py` (`ALLOWED_PATHS` nombra `bindings/mod.rs`).

- [ ] Con `split.py`: **4a**: los 4 tests de datos reales (L1844–2053 de hoy) a `cells_stream_tests/real.rs`; se quedan en el padre `struct Real`, `impl Real`, `real_path`, `mod bench; mod sim;` y `open_real*` (L1690–1843), que usan `bench`, `sim` y `glue`. `far_diagnosis` (solo lo usa `colour.rs:372`) pasa a `colour.rs`. Corregir el banner «belvedere v7.5» si el dataset es V8 (el nombre del fichero lo dice).
- [ ] **4b**: `git mv bindings/mod.rs bindings.rs` (el `#[path = "../../tests/support/reference_spacing_probe.rs"]` cambia a `../tests/support/…` porque el directorio base pasa a `src/`; comprobar) y `lod_request_scheduler.rs` → `bindings/scheduler.rs` declarado en `bindings.rs`; `lib.rs` mantiene `pub use … WasmBlockRequestScheduler` desde su nueva ruta. Solo compila en wasm32: `cargo clippy -p viewer_wasm --target wasm32-unknown-unknown` sin avisos y, como el probe está tras una feature que ninguna puerta compila, `cargo check -p viewer_wasm --target wasm32-unknown-unknown --features test-reference-spacing --offline`. Actualizar la entrada de `ALLOWED_PATHS` (fichero y destino) y `docs/reference/inventory.md:27` (enlaza `bindings/mod.rs`).
- [ ] Gate `viewer_wasm`; además el paso `convert parity bindings` de `check.sh` si `wasm-bindgen` coincide con el lock; commit `refactor(wasm): move the real-data stream tests and the scheduler binding`.

### Task 5: comentarios obsoletos, documentación y cierre

- [ ] Comentarios que nombran versiones retiradas como vigentes o rutas antiguas: `viewer_format/src/verify.rs:1`, `global_frame.rs`, `occupancy_codec.rs`, `crs.rs` (corregir solo el texto; si el código mismo es de una versión retirada, anotarlo en el informe y no tocarlo).
- [ ] `docs/reference/inventory.md`, `docs/estado.md` y CONTRIBUTING si nombran rutas movidas; `check_docs_links.py`, `check_docs_index.py`, `check_layout.py` en verde.
- [ ] `scripts/check.sh` completo → todo `ok`.
- [ ] Cierre: `Estado: completado`, mover el plan a `docs/records/archive/` con su fila en `docs/records/README.md`, nota en `docs/estado.md`, PR con base `refactor/layout`.
