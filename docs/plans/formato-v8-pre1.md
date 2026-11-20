# Formato V8, revisión v8.1-pre (plan de implementación)

Estado: pendiente · 20-11-2026 · plan escrito; antes de ejecutarlo pasa el análisis de riesgos

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** añadir la revisión experimental `v8.1-pre` junto a v7: LOD sin las columnas que el visor no dibuja,
RGB de 16 bits (o intensidad) de las hojas sin duplicar el color de pantalla, y cuantiles de la distancia al vecino
más cercano en la cabecera; el conversor la escribe con `--format-version v8.1-pre` y el visor la lee.

**Architecture:** nueva identidad (`V8Pre(1)`, `StorageLayout::V8Pre1`, magics propios) sobre la disposición de v7, con
tres capacidades nuevas en `StorageLayout`. Los bloques LOD son los de nodos con hijos (`has_children`, sin campo
nuevo); su directorio omite las columnas fuera del conjunto LOD de la cabecera, y en memoria se marcan `Absent`. En las
hojas, la columna de origen del color (RGB o intensidad) guarda solo el byte bajo y el lector (`IndexedReader`)
recompone `alto << 8 | bajo` con la columna 1 (color de pantalla). Los cuantiles se calculan en los trabajos de hoja
y se agregan en orden de id de nodo.

**Tech Stack:** Rust (nightly fijado), wasm-bindgen, JS del visor (`examples/web-host`); sin dependencias nuevas.

**Spec:** [formato-v8.md](formato-v8.md) (diseño aprobado el 20-11-2026) y el
[estudio del tamaño de la salida](../records/benchmarks/TAMANO_SALIDA_V7_20-11-2026.md).

## Global Constraints

- **v7 no cambia:** los SHA fijados en `crates/viewer_convert/src/lib_tests.rs:1587-1812`, el digest de
  `tools/format_parity` y los SHA de referencia de Belvedere/Albuera/Majadas del
  [registro de la fase B](../records/benchmarks/TEMPORALES_CLI_FASE_B_20-11-2026.md) siguen iguales en todas las tareas.
- `FormatVersion::DEFAULT_OUTPUT` sigue siendo `V7`; `v8.1-pre` solo se escribe pedido.
- `FormatVersion::V<n>` solo dentro de `viewer_format` (`scripts/check_version_literals.sh`): fuera, capacidades de
  `StorageLayout`.
- Conjunto de columnas LOD de `v8.1-pre`: intensidad y clasificación (por nombre de esquema LAS al convertir; en el
  fichero, lista explícita de índices en la cabecera). El color de pantalla (columna 1) y la geometría siempre están.
- Cuantiles: p1, p5, p50 de la distancia al vecino más cercano en unidades del dataset, sin contar distancias 0
  (duplicados exactos; si todo son duplicados, los tres valen 0); finitos, ≥ 0 y p1 ≤ p5 ≤ p50; muestra elegida por
  hash con semilla fija de la posición cuantizada, independiente del orden y de W; el recomendado es p5.
- Paridad web/CLI (RQ-08) también para `v8.1-pre` (caso nuevo en `tools/convert_parity`).
- Sin `cargo clean`; worktree con su propio target; `FORMAT_PARITY_REAL_INPUT` con la ruta absoluta del LAZ de
  Belvedere del checkout principal. Código y commits en inglés, documentos en español, sin menciones a asistentes.

## Review Focus

1. **Un bloque LOD sin la columna pedida:** el lector devuelve «ausente» (nunca pánico ni índice fuera de rango) y el
   visor colorea con la intensidad y su rango global.
2. **Lectura de RGB de 16 bits o de intensidad de una hoja v8.1-pre:** idéntica, valor a valor, a la de la misma nube en
   v7 (también con orden de filas híbrido, que permuta todas las columnas por igual).
3. **Nube sin color:** el color de pantalla sale de la intensidad (`>> 8`, r = g = b) y la intensidad se recompone igual.
4. **Cuantiles deterministas:** mismos valores con W ∈ {1, 2, 8}, ejecutor serie y con hilos, nativo y wasm.
5. **Fichero corrupto o truncado** en las partes nuevas (lista LOD, cuantiles, entradas ausentes): error, nunca pánico.

---

### Task 1: v8.1-pre identity

**Files:**
- Modify: `crates/viewer_format/src/version.rs` (enum :16-23, consts :27-57, `fixed_row_order` :80-85, `major`/`revision`
  :92-107, `Display`/`FromStr` :110-145, `parse_identity` :225-232, tests :266, :277, :311, :317),
  `crates/viewer_format/src/layout.rs:39-193`, `crates/viewer_format/src/paged_index.rs:264`,
  `crates/viewer_convert_core/src/options.rs:65`, `crates/viewer_convert/src/main.rs:216-226`,
  `crates/viewer_wasm/src/core_convert.rs:57`, `examples/web-host/converter-ui.mjs:6-10` (y su test)
- Create: `docs/reference/format/FORMAT_V8_PRE1.md` (esqueleto con lo que cambia frente a FORMAT_V7.md),
  `docs/adr/0058-formato-v8-experimental.md` (decisiones D1–D6 del diseño, `Estado: Aceptada`, decide el propietario);
  enlazarlos en `docs/README.md`, `docs/adr/README.md` y `docs/reference/inventory.md`

**Interfaces:**
- Produces: `FormatVersion::V8Pre(NonZeroU32)`, `FormatVersion::V8_PRE1` (texto `v8.1-pre`), en `WRITABLE`;
  `StorageLayout::V8Pre1` (magics `OCTVI81P`/`OCTVE81P`) con las capacidades de v7 (global, paginado, ocupación, diccionarios,
  planos de bytes, filas híbridas fijas) más `derives_display_attribute()`, `omits_lod_attributes()` y
  `stores_distance_quantiles()`, que en esta tarea devuelven `true` para V8Pre1 pero aún no cambian bytes (el
  comportamiento llega en las tareas 2, 4 y 5). `is_v7` pasa a `supports_hybrid_rows()`. `options.rs:65` usa esa
  capacidad en vez de `major() != 7`.

- [ ] **Step 1: failing tests**: `v8_pre1_parses_and_prints` (`"v8.1-pre".parse()` ↔ `Display`), `v8_pre1_is_writable`,
  `unknown_major_nine_is_rejected` (el caso que hoy usa `(8, 9)` pasa a major 9) y `cli_accepts_v8_pre1` (CLI tests).
- [ ] **Step 2:** los tests fallan.
- [ ] **Step 3:** implementar; los tests que recorren `WRITABLE` (corruption.rs, v7_final.rs, v7_4.rs, writer_tests.rs)
  deben pasar con V8Pre1 sin cambiar sus aserciones.
- [ ] **Step 4:** `cargo test --workspace --offline`, `cargo clippy --workspace --all-targets --offline -- -D warnings`,
  `bash scripts/check_version_literals.sh`, `python3 scripts/check_docs_links.py`, `python3 scripts/check_docs_index.py` → OK.
- [ ] **Step 5:** commit `feat(format): v8.1-pre identity (experimental V8)`.

### Task 2: Distance quantiles in the paged header

**Files:**
- Modify: `crates/viewer_format/src/metadata.rs`, `crates/viewer_format/src/paged_index.rs` (`PagedHeader` :40-56,
  parser :275-324, escritor :1322-1344, `load_all` :805, `assemble_with` :959), `crates/viewer_format/src/reader.rs:334-451`,
  `crates/viewer_format/src/lazy_pages.rs:98`, `crates/viewer_format/src/storage_reader.rs:173`, `crates/viewer_format/src/verify.rs`
- Test: `crates/viewer_format/tests/` (round trip y corrupción)

**Interfaces:**
- Produces: `#[derive(Clone, Copy, Debug, PartialEq)] pub struct PointDistanceQuantiles { pub p1: f64, pub p5: f64, pub p50: f64 }`
  con `PointDistanceQuantiles::new(p1, p5, p50) -> Result<Self, FormatError>` (valida finitos, ≥ 0, ordenados);
  `FormatMetadata::with_point_distance_quantiles(self, PointDistanceQuantiles) -> Self` y
  `point_distance_quantiles() -> Option<PointDistanceQuantiles>`; accesor `point_distance_quantiles()` en
  `IndexedReader`, `PageSource` y el lector de almacenamiento. En la cabecera, 3 × f64 justo después de
  `reference_spacing`, solo si `layout.stores_distance_quantiles()`. El escritor da error si V8Pre1 no los trae.

- [ ] **Step 1: failing tests**: `distance_quantiles_round_trip` (escribir y leer con los lectores eager y lazy);
  `v8_pre1_without_quantiles_is_rejected`; `corrupt_quantiles_are_rejected` (NaN, negativo, desordenados, truncado).
- [ ] **Step 2:** fallan.
- [ ] **Step 3:** implementar.
- [ ] **Step 4:** tests del workspace, clippy, SHA de v7 intactos → OK.
- [ ] **Step 5:** commit `feat(format): v8.1-pre stores nearest-neighbour distance quantiles`.

### Task 3: Converter computes the quantiles

**Files:**
- Modify: `crates/viewer_convert_core/src/pipeline/node_job.rs` (`JobContext` :50-57, `NodeJobOutput` :108-118,
  `measure_leaf_spacing` :984-1020, `leaf_output` :1120-1162), `crates/viewer_convert_core/src/spatial/builder.rs:559-595`,
  `crates/viewer_convert_core/src/spatial/plan.rs` (`NodeRecord` :401-411; agregación antes de `with_global_frame` :338-340)
- Test: tests del núcleo junto a `node_job.rs` y `crates/viewer_convert/src/lib_tests.rs`

**Interfaces:**
- Consumes: `PointDistanceQuantiles`, `with_point_distance_quantiles` (tarea 2).
- Produces: `NodeJobOutput.nn_samples: Vec<u64>` (distancias² en cuantos, 1 cuanto = 2⁻¹⁵ unidades, sin ceros);
  `NodeRecord.nn_samples`; en `plan.rs`, agregación en orden de id de nodo, orden total, cuantiles por rango más
  cercano, raíz cuadrada y paso a unidades una sola vez al final. Muestreo: un punto entra si
  `hash(semilla_fija, posición_cuantizada) < tasa`, con `tasa = objetivo / source.point_count` y
  `objetivo = 1 << 20` puntos (decisión técnica; ajustable tras medir). Vecino: celdas del espaciado medido de la hoja,
  27 vecinas, ampliando si están vacías. Solo con `JobContext.collect_distance_quantiles` (verdadero solo si
  `stores_distance_quantiles()`), así que v7 no hace trabajo extra.

- [ ] **Step 1: failing tests**: `quantiles_of_a_regular_grid` (rejilla de paso conocido d: p1 = p5 = p50 = d con
  tolerancia de un cuanto); `duplicates_do_not_count` (puntos repetidos más una rejilla: mismos cuantiles);
  `quantiles_do_not_depend_on_workers` (W ∈ {1, 2, 8}, ejecutor serie y con hilos: mismos valores).
- [ ] **Step 2:** fallan.
- [ ] **Step 3:** implementar.
- [ ] **Step 4:** tests del workspace (SHA de v7 intactos) y clippy → OK.
- [ ] **Step 5:** commit `feat(convert): nearest-neighbour distance quantiles for v8.1-pre`.

### Task 4: LOD blocks omit non-drawing columns

**Files:**
- Modify: `crates/viewer_format/src/page_codec.rs` (directorio :700-751, `raw_length` :711-719), `crates/viewer_format/src/index_codec.rs:556-591`,
  `crates/viewer_format/src/prepare.rs:395-428`, `crates/viewer_format/src/reader.rs` (`plan_read` :640-668,
  `read_attribute` :1094-1126, `block_payload_storage_stats` :558), `crates/viewer_format/src/verify.rs`,
  `crates/viewer_format/examples/storage_payload_stats.rs`, cabecera paginada (lista LOD)
- Test: `crates/viewer_format/tests/` (round trip, corrupción)

**Interfaces:**
- Produces: en la cabecera de `v8.1-pre`, la lista de índices de esquema del conjunto LOD (`lod_attribute_columns`);
  en memoria, el vector de payloads mantiene `schema.len() + 2` entradas con un marcador `Absent` para las omitidas en
  bloques de nodos con hijos; `IndexedReader::attribute_present(block, index) -> bool`; `plan_read` de una columna
  ausente devuelve `FormatError::AttributeAbsent { block, attribute }` (nunca pánico). El conversor pasa el conjunto
  LOD (intensidad y clasificación del esquema LAS) en los metadatos.

- [ ] **Step 1: failing tests**: `lod_blocks_omit_columns_outside_the_lod_set` (un fichero v8.1-pre: bloques de nodos
  internos sin esas entradas; las hojas con todas); `reading_an_absent_column_is_an_error_not_a_panic`;
  `corrupt_lod_set_is_rejected` (índice fuera del esquema, repetido, truncado).
- [ ] **Step 2:** fallan.
- [ ] **Step 3:** implementar.
- [ ] **Step 4:** tests del workspace (SHA de v7 intactos) y clippy → OK.
- [ ] **Step 5:** commit `feat(format): v8.1-pre LOD blocks store only the drawing columns`.

### Task 5: Leaf colour source stores only its low byte

**Files:**
- Modify: `crates/viewer_format/src/prepare.rs:395-428`, `crates/viewer_format/src/page_codec.rs:711-719`,
  `crates/viewer_format/src/index_codec.rs:556-591`, `crates/viewer_format/src/reader.rs` (`plan_read`,
  `materialize_selected_from_buffers`, `read_attribute`, estimación de memoria del `ReadPlan`), cabecera (origen del color)
- Test: `crates/viewer_format/tests/`

**Interfaces:**
- Produces: campo de cabecera «origen del color de pantalla» (`none` | índice de la columna RGB | índice de la
  intensidad); en hojas de `v8.1-pre` esa columna guarda solo el byte bajo (ancho 3 o 1) y el lector, al pedirla,
  planifica dos rangos (columna 1 y bytes bajos) y devuelve `alto << 8 | bajo`. El escritor comprueba en cada hoja que
  el color de pantalla es el byte alto (y r = g = b con intensidad) y da error si no.

- [ ] **Step 1: failing tests**: `leaf_rgb16_reads_back_identical_to_v7` y `leaf_intensity_reads_back_identical_without_colour`
  (misma nube convertida en v7 y v8.1-pre: valores iguales bloque a bloque, también con filas híbridas);
  `display_mismatch_is_rejected_by_the_writer`.
- [ ] **Step 2:** fallan.
- [ ] **Step 3:** implementar.
- [ ] **Step 4:** tests del workspace (SHA de v7 intactos) y clippy → OK.
- [ ] **Step 5:** commit `feat(format): v8.1-pre leaves store only the low byte of the colour source`.

### Task 6: Viewer reads v8.1-pre

**Files:**
- Modify: `crates/viewer_wasm/src/async_format_reader.rs:493-566`, `crates/viewer_wasm/src/lod_driver.rs:363-425`,
  `crates/viewer_wasm/src/lod_dataset.rs:567-580`, `crates/viewer_wasm/src/bindings/lod.rs:175`,
  `crates/viewer_wasm/src/bindings/cells.rs:403-409`, `examples/web-host/viewer-worker.js` (:705-716, :1012, :1381),
  `examples/web-host/build-worker.js:327-348`
- Test: tests de `viewer_wasm` y `node --test examples/web-host/tests/`

**Interfaces:**
- Consumes: `attribute_present`, `AttributeAbsent` (tarea 4); `point_distance_quantiles` (tarea 2).
- Produces: con un modo de color `attribute:N:M` y un bloque sin esa columna, el visor colorea con la intensidad y su
  rango global; `cellsInfo().recommendedMinSize` (p5, o `null` si el fichero no lo trae) y
  `LodDataset::with_recommended_min_size`. La demo aún **no** lo usa por defecto (D5).

- [ ] **Step 1: failing tests**: `attribute_mode_falls_back_to_intensity_on_lod_blocks` (wasm); `cells_info_reports_recommended_min_size`.
- [ ] **Step 2:** fallan.
- [ ] **Step 3:** implementar (camino de celdas y camino CPU, que debe pedir la intensidad en vez de la columna ausente).
- [ ] **Step 4:** tests de wasm, tests de node de la demo, clippy wasm32 → OK; verificación en navegador con Playwright
  (`ui-verifier`) de un `.oct` v8.1-pre de Belvedere: modos RGB, intensidad, elevación, clase y un atributo genérico,
  sin errores de consola.
- [ ] **Step 5:** commit `feat(viewer): read v8.1-pre (absent LOD columns, recommended minimum size)`.

### Task 7: Web/CLI parity for v8.1-pre

**Files:**
- Modify: `tools/convert_parity/run.mjs` (:98-104, :128), `tools/convert_parity/src/lib.rs:123-128`,
  `crates/viewer_convert/tests/cli.rs`

**Interfaces:**
- Produces: código de formato 5 = `v8.1-pre` en la herramienta de paridad (sin el `_` que cae en v7.5-pre) y su caso en
  `run.mjs`; un test de CLI que convierte con `--format-version v8.1-pre` y verifica (`viewer_convert verify`).

- [ ] **Step 1:** añadir el caso y el test → fallan si no se emiten los mismos bytes o si `verify` rechaza.
- [ ] **Step 2:** arreglar lo que salga.
- [ ] **Step 3:** `FUZZ_CHECK=0 scripts/check.sh --offline` → 23/23 (más el caso nuevo), con la paridad de bloques reales.
- [ ] **Step 4:** commit `test(convert): web/CLI parity and CLI round trip for v8.1-pre`.

### Task 8: Measure v8.1-pre against v7 and record it

**Files:**
- Create: `docs/records/benchmarks/FORMATO_V8_PRE1_<dd-mm-aaaa>.md` y sus datos en
  `docs/records/benchmarks/data/formato-v8-pre1-<dd-mm-aaaa>/`
- Modify: `docs/plans/formato-v8.md` (`Estado:`), este plan (`Estado:` y casillas), `docs/estado.md`, `docs/README.md`,
  `docs/reference/format/FORMAT_V8_PRE1.md` (completar)

- [ ] **Step 1:** con `scripts/measure_temp_disk.sh` y `--format-version v8.1-pre` frente a la salida v7 de la misma
  rama, en Belvedere, Albuera y Majadas: tamaño de `.oct` y `.octattrs`, tiempo, RSS, pico de temporales, y los
  cuantiles de cada nube junto al espaciado de referencia. Esperado (diseño): Albuera ≥ 35 % más pequeña (V2).
- [ ] **Step 2:** lectura en el navegador: tiempo hasta el objetivo de px en Belvedere v7 frente a v8.1-pre (sondas
  existentes de `examples/web-host/PROBES.md`), para comprobar que la recomposición del RGB no empeora la carga.
- [ ] **Step 3:** parar y llevar al propietario si: V2 no se cumple, el tiempo de conversión sube > +25 % o la carga
  en el navegador empeora > 10 %.
- [ ] **Step 4:** registro fechado con las series, la orden exacta y el equipo; completar FORMAT_V8_PRE1.md;
  actualizar documentos; `check_docs_links.py` y `check_docs_index.py` → OK.
- [ ] **Step 5:** commit `docs: measure v8.1-pre against v7`.
