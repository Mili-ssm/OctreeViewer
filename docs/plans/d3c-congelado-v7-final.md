# D3c: congelado de V7 final — plan de implementación

Estado: listo para ejecutar · 15-11-2026 · tarea D3c del [plan de mejoras](mejoras-pendientes-13-11-2026.md#fase-d-fuente-remota-y-v7-final); enmendado por su pre-mortem el mismo día

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** crear la identidad final `FormatVersion::V7` (bytes de `v7.5-pre`, escritor híbrido fijo) y hacerla la
salida predeterminada del CLI, el wasm y la app web, con datasets y referencias regenerados.

**Architecture:** V7 es un `StorageLayout` nuevo con las capacidades de `V7Pre5` y magics propios; la E/S paginada
deja de fijar `V7Pre5` y toma la disposición del pie. `ConvertOptions` guarda lo que el usuario pidió y resuelve
contra el formato los valores que este fija (hoja, rejilla, divisiones, orden de filas), sin depender del orden de
los setters. El valor por defecto cambia en un solo punto (`DEFAULT_OUTPUT`, [ADR 0006](../adr/0006-politica-de-version-solo-en-viewer-format.md))
y solo después de que los tests fijen `v5` donde prueban V5 y de medir RQ-07.

**Tech Stack:** Rust nightly (workspace), wasm32, JS del host web, Python de los scripts de checks.

**Spec:** [ADR 0044](../adr/0044-v7-final-congelada-y-predeterminada.md) (decisión), [ADR 0042](../adr/0042-escritor-hibrido-en-la-identidad-final-de-v7.md)
(escritor), [ADR 0043](../adr/0043-coste-de-conversion-de-v7-frente-a-v5.md) (RQ-07), [especificación v7.5-pre](../reference/format/FORMAT_V7_PRE5.md).

## Global Constraints

- Rama `feat/r3-v75`, worktree `~/.cache/codex-worktrees/octree-r3-v75`; un commit por tarea; sin push hasta el final.
- `scripts/check.sh --offline` verde al cerrar cada tarea de código (en segundo plano, ~15 min; no editar el worktree mientras corre).
- `FormatVersion::V<n>` solo dentro de `crates/viewer_format/` salvo en tests (`scripts/check_version_literals.sh`).
- Magics de V7: índice `OCTVIDX7`, pie `OCTVEND7`; cabecera mayor 7, revisión 0; texto `v7`.
- V7 se añade **al final** de `FormatVersion::WRITABLE`: `fuzz/src/bin/gen_seeds.rs` nombra las semillas por posición.
- V5, V6, `v6.N-pre` y `v7.N-pre` siguen legibles y escribibles **con sus bytes actuales**; `v7.5-pre` conserva el orden de la fuente por defecto.
- Valores que fija el marco global (`uses_global_frame`): hoja `GLOBAL_LEAF_POINT_LIMIT` (64000), rejilla `GLOBAL_SAMPLE_GRID` (256), 2 divisiones; V7 fija además el orden híbrido.
- Código, comentarios y commits en inglés; documentación en español (salvo `examples/web-host/README.md`); fechas `dd-mm-aaaa`.
- `docs/records/` es inmutable: la evidencia nueva va en registros nuevos fechados.

## Criterios de parada

- RQ-07 por encima de ×2,1 o +40 % (Tarea 3) → parar antes de la Tarea 5 y consultar al propietario.
- Cambia un SHA de V5 en un test que pasa `v5` explícito → parar: la resolución de opciones está mal.
- Una salida V7 cuya política de muestreo no coincide con su hoja real → parar.
- La conversión web de Belvedere en V7 se queda sin memoria o tarda más de 2× V5 → no cambiar el valor por defecto de la app web y consultar.
- La Tarea 4 pasa de ~100 ediciones de tests → replanificar con un helper.

## Review Focus

1. Cabecera y pie con identidades distintas (`OCTVEND7` con cabecera `v7.5-pre`, o al revés): el lector síncrono lo rechaza; los perezosos, que no leen la cabecera, quedan documentados como límite en `FORMAT_V7.md`. Tarea 1.
2. `--format-version v5 --spatial-divisions 4` funciona sea cual sea el orden de las opciones; `--points-per-block 4096` sin formato (V7 por defecto) da error que nombra la opción y `--format-version v5`. Tarea 2.
3. `--format-version v7 --row-order source`: error que nombra el orden de filas; `v7.5-pre` sin `--row-order` escribe los mismos bytes que hoy. Tarea 2.
4. Conversión por defecto con checkpoint/reanudación o entrada EPT, o con Extra Bytes o coordenadas que el marco global no admite: error que nombra la versión **pedida** (o la predeterminada) y la pista `--format-version v5`, sin salida parcial. Reanudar un checkpoint V5 sin `--format-version` se rechaza por identidad. Tarea 5.
5. App web: sin opción escribe V7; `v5` y `v7.5-pre` se respetan; con V7 elegido, hoja y divisiones no son editables. Tarea 6.

---

### Task 1: Identidad V7 en `viewer_format` y E/S paginada por disposición

**Files:**
- Modify: `crates/viewer_format/src/layout.rs`, `version.rs`, `paged_index.rs`, `reader.rs:239,296`, `page_codec.rs:655`, `writer.rs:212-257`
- Create: `crates/viewer_format/tests/v7_final.rs`, `docs/reference/format/FORMAT_V7.md`
- Modify docs: `docs/reference/format/FORMAT_V7_PRE5.md` (enlace a la final), `docs/reference/inventory.md` (tabla de identidades), `docs/README.md` (índice)

**Interfaces:**
- Produces: `StorageLayout::V7` con `version() == FormatVersion::V7`, `index_magic() == b"OCTVIDX7"`, `footer_magic() == b"OCTVEND7"`, `true` en todas las capacidades donde hoy `V7Pre5` da `true` y `false` en `supports_block_checkpoint`.
- Produces: `FormatVersion::V7` escribible y último en `WRITABLE`.
- Produces: `PagedCore::is_paged_footer(footer: &[u8]) -> bool` acepta los pies de `V7Pre5` y `V7`; `PagedCore` guarda la disposición leída del pie y escribe con la pedida (la constante `LAYOUT` desaparece).

- [x] **Step 1: tests que fallan** en `tests/v7_final.rs`: `v7_round_trips_paged_pair` (escribir con `V7`, leer con el lector síncrono y el perezoso: mismos nodos y puntos; `version() == V7`; pie `OCTVEND7`); `v7_and_v7_5_pre_differ_only_in_identity` (misma entrada y mismo orden de filas: solo difieren la revisión de cabecera, los CRC afectados y los magics); `eager_reader_rejects_mismatched_header_and_footer` (Review Focus 1); `writer_rejects_more_than_256_attributes` (RF-27, revisión 30-10-2026 del ADR 0007: error claro en todas las versiones escribibles; el test `wide_schema_requires_explicit_reader_limits` se adapta). Ajustar en `version.rs` los tests que afirman que `V7` no es escribible y el texto de `WRITABLE`.
- [x] **Step 2:** `cargo test -p viewer_format --offline --test v7_final` falla.
- [x] **Step 3:** implementar la disposición, las capacidades, la E/S paginada parametrizada, la comparación cabecera/pie del lector síncrono, el rechazo de más de 256 atributos en el escritor y textos de error que nombren la versión en uso en lugar de «v7.5-pre».
- [x] **Step 4:** `cargo test -p viewer_format --offline` pasa, `tests/v7_5.rs` sin cambios en sus aserciones de bytes.
- [x] **Step 5:** `FORMAT_V7.md`: identidad, magics, diferencia con v7.5-pre (enlace a `FORMAT_V7_PRE5.md`); el lector admite cualquier orden de filas por bloque (ADR 0041) y el escritor de V7 usa siempre el híbrido (ADR 0044); el híbrido sobre `v7.5-pre` es solo para medir (ADR 0042); el límite de los lectores perezosos. Índices. `python3 scripts/check_docs_links.py && python3 scripts/check_docs_index.py`.
- [x] **Step 6:** commit `feat(format): add the final V7 identity on the v7.5-pre layout`.

### Task 2: Opciones resueltas contra el formato y orden de filas fijado

**Files:**
- Modify: `crates/viewer_format/src/version.rs`, `prepare.rs:446-475,865-898`, `error.rs`, `row_order_tests.rs`
- Modify: `crates/viewer_convert_core/src/options.rs` (campos pedidos como `Option`, accesores resueltos, `validate`), `error.rs`, los puntos de entrada de conversión (llaman a `validate`), checkpoint y diario (`crates/viewer_convert/src/checkpoint.rs`, `ept_checkpoint.rs`: guardan valores **resueltos**)
- Modify: `crates/viewer_convert/src/main.rs:46-100,209-217`, `crates/viewer_wasm/src/core_convert.rs:58-100`, `tools/convert_parity/src/lib.rs:45-60,116-126` (formato primero; orden de filas `None` salvo que se pida)

**Interfaces:**
- Consumes: `FormatVersion::V7` (Tarea 1).
- Produces: `FormatVersion::fixed_row_order(self) -> Option<RowOrder>` (`Some(RowOrder::Hybrid)` solo para `V7`).
- Produces: `FormatWriterOptions` guarda `Option<RowOrder>`; `None` = `fixed_row_order().unwrap_or(RowOrder::Source)`; un orden explícito distinto del fijado → `FormatError::RowOrderFixedByFormat(FormatVersion)`.
- Produces: `ConvertOptions` guarda `Option` para formato, hoja, rejilla, divisiones y orden de filas. Los accesores `format_version()`, `points_per_block()`, `sample_grid()`, `spatial_divisions()` y `row_order()` devuelven valores resueltos: formato (pedido o `DEFAULT_OUTPUT`) → valores fijados por el formato → valor pedido → valor por defecto actual. `ConvertOptions::validate(self) -> Result<Self, ConvertError>` devuelve la variante existente `ConvertError::FixedByFormat(&'static str)` si un valor pedido contradice uno fijado, y la existente `HybridRowOrderUnsupported` si se pide híbrido con un formato que no es v7; los setters solo comprueban rangos, así que el orden de llamada no importa.

- [x] **Step 1: tests que fallan:** `v5_with_four_divisions_in_any_order` (setters en los dos órdenes); `explicit_v7_with_4096_points_is_fixed_by_format`; `v7_resolves_global_values_and_hybrid` (hoja 64000, rejilla 256, híbrido sin pedirlos); `v7_with_source_is_fixed_by_format`; `v7_5_pre_keeps_source_default`; en `row_order_tests.rs`, `v7_resolves_hybrid_row_order_by_default` y `v7_rejects_explicit_source_order`; en `tests/cli.rs`, `--format-version v7 --row-order source` y `--spatial-divisions 4 --format-version v5` (los dos órdenes).
- [x] **Step 2:** fallan.
- [x] **Step 3:** implementar la resolución, `validate` en cada entrada de conversión (CLI, wasm, `convert_file` y equivalentes), la ayuda del CLI y del wasm (V7 fija el orden híbrido; `--row-order` solo vale para `v7.N-pre`) y las herramientas de paridad.
- [x] **Step 4:** `scripts/check.sh --offline` verde; ningún SHA de V5 ni de las `-pre` cambia.
- [x] **Step 5:** commit `feat(convert): resolve format-fixed options independently of setter order`.

### Task 3: RQ-07 con V7 antes del cambio de valor por defecto

**Files:**
- Create: `docs/records/benchmarks/D3C_RQ07_V7_<fecha>.md` (+ JSON en `docs/records/benchmarks/data/`)

- [x] **Step 1:** compilar `viewer_convert` con perfil `perf`; Belvedere con `--format-version v7` frente a `v5`, ≥ 5 rondas intercaladas con la máquina libre, salidas en `target/` (borrarlas antes de cada conversión); tiempo total y RSS pico en KiB con `/usr/bin/time`; medianas sin redondear; tamaño del par frente a V5.
- [x] **Step 2:** pasa si tiempo ≤ 2,1× y RSS ≤ +40 % ([ADR 0043](../adr/0043-coste-de-conversion-de-v7-frente-a-v5.md)); si no, criterio de parada.
- [x] **Step 3:** registro con entorno, comandos, medianas y rangos; commit `docs(records): measure RQ-07 with the final V7`.

### Task 4: Tests que prueban V5, EPT o checkpoints fijan `v5` (sin cambio de comportamiento)

**Files:**
- Modify: `crates/viewer_convert/src/lib_tests.rs` (caracterizaciones con SHA, `1710-1811`), `crates/viewer_convert/src/ept_tests.rs`, los tests de checkpoint, `crates/viewer_format/tests/v7_5.rs:385`, `tests/v7_4.rs:756`, `tests/index_amplification.rs:112` (dejan de depender de `DEFAULT_OUTPUT`), `fuzz/src/bin/gen_seeds.rs:131` (`uses_pages()` en lugar de `== V7_PRE5`), `tools/convert_parity/src/lib.rs`, `tools/convert_parity/run.mjs:91-125`, `tools/convert_parity/run-bindings.mjs:180-204`, `tools/convert_parity/src/bin/native_convert.rs` (códigos `v5`, `v7.4-pre`, `v7.5-pre`, `v7` explícitos; los casos `*-d8`, COPC y EPT en `v5`)

- [x] **Step 1:** fijar `v5` explícito en cada test que prueba V5, EPT o checkpoints; añadir a `gen_seeds` y a la paridad el código de V7.
- [x] **Step 2:** `scripts/check.sh --offline` verde con `DEFAULT_OUTPUT` todavía en V5 y **sin ningún SHA cambiado**.
- [x] **Step 3:** commit `test: pin V5 where tests exercise V5, EPT or checkpoints`.

### Task 5: V7 como salida predeterminada

**Files:**
- Modify: `crates/viewer_format/src/version.rs:42` (`DEFAULT_OUTPUT = V7`), `crates/viewer_convert_core/src/error.rs:89-96` (`UnsupportedForPagedFormat` y `UnsupportedForGlobalFormat` pasan a `{ what: &'static str, version: FormatVersion }`; el mensaje nombra esa versión y añade la pista `--format-version v5`), `crates/viewer_convert/src/main.rs` (ayuda), `crates/viewer_format/examples/create_web_sample.rs:21` si la ruta por defecto lo rompe

**Interfaces:**
- Consumes: Tareas 1, 2 y 4.
- Produces: `ConvertOptions::default().format_version() == FormatVersion::DEFAULT_OUTPUT == V7`, `row_order() == Hybrid`, `points_per_block() == 64000`.

- [x] **Step 1: tests que fallan:** `default_output_is_v7_hybrid`; en `tests/cli.rs`, `default_conversion_writes_v7_pair` (pie `OCTVEND7`, política de muestreo `leaf:64000`); `default_checkpoint_points_to_v5`, `default_ept_points_to_v5`, `default_extra_bytes_error_names_version` y `resume_v5_checkpoint_without_format_is_rejected` (Review Focus 4).
- [x] **Step 2:** fallan.
- [x] **Step 3:** cambiar el valor por defecto, los errores y la ayuda.
- [x] **Step 4:** `scripts/check.sh --offline` verde; solo cambian los SHA de casos sin formato explícito, y cada uno se anota en el informe.
- [x] **Step 5:** commit `feat(convert): make the final V7 the default output`.

### Task 6: App web con V7 por defecto

**Files:**
- Modify: `examples/web-host/converter-ui.mjs:7-59`, `main.js:821-822`, `tests/converter-ui.test.mjs`, `examples/web-host/README.md` (inglés)

**Interfaces:**
- Consumes: el wasm acepta `formatVersion: "v7"` (Tareas 2 y 5).
- Produces: `FORMAT_VERSIONS` empieza por `"v7"` e incluye `"v7.5-pre"`; `OPTION_DEFAULTS.formatVersion === "v7"`; con un formato de marco global, `pointsPerBlock` y `spatialDivisions` no se envían y sus controles quedan deshabilitados.

- [x] **Step 1: test que falla** en `converter-ui.test.mjs`: valor por defecto `v7`; `v5` y `v7.5-pre` aceptados; con `v7` no se envían hoja ni divisiones (Review Focus 5).
- [x] **Step 2:** `node --test examples/web-host/tests/converter-ui.test.mjs` falla.
- [x] **Step 3:** cambiar la lista, el valor preseleccionado, los controles y el README.
- [x] **Step 4:** `node --test examples/web-host/tests/*.test.mjs` verde. Medir la conversión web de Belvedere en V7 frente a V5 (tiempo y memoria pico de la pestaña) con el arnés del conversor web; criterio de parada. Verificación en navegador (`agent-skills:ui-verifier`): convertir un LAS pequeño con la opción por defecto, abrir el par, consola sin errores, escritorio y móvil, teclado.
- [x] **Step 5:** commit `feat(web-host): default the converter to the final V7`.

### Task 7: Datasets, referencias y semillas

**Files:**
- Create (fuera de git): `target/render-data/belvedere-v7.{oct,octattrs}`, `majadas-v7.{oct,octattrs}` con el CLI por defecto (perfil `release`)
- Modify: rutas a `*-v7` en `crates/viewer_wasm/src/lod_cells_stream_tests.rs:1189-1445`, `lod_cells_tests.rs:1263`, `lod_cells_glue_sim.rs:761`, `lod_cells_sim.rs:1247`, `lod_cells_bench.rs:116,291`, `examples/web-host/run-r3-b6-matrix.sh`
- Regenerate: `fuzz/corpus/fuzz_format_reader/seed-sample*` con `gen_seeds`
- Create: `docs/records/benchmarks/D3C_V7_REFERENCES_<fecha>.md` (+ JSON)

- [x] **Step 1:** convertir Belvedere y Majadas por defecto; pie `OCTVEND7`; la app llega al objetivo (`probe-orbit-load.mjs --rest-first`, servidor `python3 -m http.server 8767 --bind 127.0.0.1` en la raíz del worktree).
- [x] **Step 2:** `R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --offline cells` pasa con las rutas nuevas.
- [x] **Step 3:** SHA con el CLI por defecto sobre las entradas de [R1](../records/benchmarks/R1_REFERENCE_SHA_2026-10-29.md#comandos) y SHA del corpus de fuzz; registro con entorno, comandos y resultados.
- [x] **Step 4:** `scripts/check.sh --offline` verde; commit `test(data): regenerate V7 datasets, references and fuzz seeds`.

### Task 8: Documentación del congelado

**Files:**
- Modify: `README.md` (formato predeterminado, «Prioridades pendientes»), `CONTRIBUTING.md:68-70`, `docs/reference/inventory.md`, `docs/plans/format-v7-plan.md:440-447` (inventario de promoción: qué queda para la retirada), `docs/plans/mejoras-pendientes-13-11-2026.md` (D3c hecho), este plan (estado)

- [ ] **Step 1:** buscar afirmaciones que el cambio vuelve falsas: `grep -rn 'V5 sigue\|predetermin\|DEFAULT_OUTPUT\|v7.5-pre' README.md CONTRIBUTING.md docs --include='*.md' | grep -v docs/records/`; corregir cada una o enlazarla al ADR 0044.
- [ ] **Step 2:** `python3 scripts/check_docs_links.py && python3 scripts/check_docs_index.py` verdes.
- [ ] **Step 3:** commit `docs: record the V7 freeze and the new default output`.
