# Temporales del conversor, fase A: escritura por subárbol (plan de implementación)

Estado: pendiente · 20-11-2026 · plan escrito; antes de ejecutarlo pasa el análisis de riesgos

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** que el CLI escriba los bloques de cada nodo en cuanto el planificador lo confirma, en el mismo orden que
hoy, para que los spools de hojas y LOD no se acumulen hasta el final, sin cambiar un byte de la salida.

**Architecture:** el codificador de bloques deja de necesitar la jerarquía completa (la caja de un bloque solo depende
de la profundidad y el prefijo de su nodo y de la retícula de la raíz). El escritor gana un modo en flujo que recibe
los metadatos completos al final. El planificador entrega los bloques confirmados a un sumidero (`BlockSink`), en
preorden y con id global; el CLI los prepara y escribe con el pipeline de bloques existente. El ejecutor procesa un
hijo de la raíz cada vez, con W hilos en su cola de trabajos, así que los ids son globales al confirmar. La web y los
tests siguen con «planificar y luego escribir» (`CollectSink`).

**Tech Stack:** Rust (nightly fijado); sin dependencias nuevas.

**Spec:** [temporales-del-conversor.md](temporales-del-conversor.md), sección «A: escritura por subárbol de la raíz»
(diseño aprobado el 19-11-2026). Fase B: [plan archivado](../records/archive/temporales-del-conversor-fase-b.md) y
[registro](../records/benchmarks/TEMPORALES_CLI_FASE_B_20-11-2026.md).

## Global Constraints

- T1: `.oct`/`.octattrs` byte a byte iguales a `main` con las mismas opciones. SHA de referencia (sin cambios desde
  `dc98e5a`): Belvedere `da206a659ea0bfdfbfb3d8141592387cb2dde7133e6a5e44ed9a7a5db3c9f0c4` /
  `e2f587ab136906d8fc43f2f34c46be0f23a566f3954a8288056b95dae9802323`; Albuera `5c15df5e9043…` / `b76636f41c6f…`
  y Majadas `a213eb2391ca…` / `0cffd5279cf3…` (completos en el registro de la fase B). La paridad web/CLI de
  `scripts/check.sh` (`tools/convert_parity`) y los SHA fijados en `lib_tests.rs` (`assert_structure`) siguen en verde.
- T2: pico aparente de `.work` en Albuera ≤ 2 × LAZ (≤ 8 610 969 578 B) como objetivo; si no llega, se mide y se
  lleva al propietario (zstd, C #106 o aceptar), sin más trabajo antes.
- T3: el tiempo se mide frente a `main` (fase B: Albuera 695,6 s); una pérdida > +25 % frente a `main` se presenta al
  propietario antes de seguir.
- T4: RSS dentro del presupuesto estimado (512 MiB por defecto); la preparación de bloques que corre a la vez que la
  planificación tiene un tope fijado de antemano.
- T6: ante error o cancelación no se publica salida, y `.work` y los temporales de salida se borran.
- T7: el conversor web conserva su flujo y sus bytes; el núcleo compila para wasm32 sin dependencias nativas.
- `--checkpoint`/`--resume` (solo formatos heredados) mantienen el flujo actual de «planificar y luego escribir».
- Sin `cargo clean` ni borrar nada de `target/` salvo los directorios de sonda de la tarea. Worktree con su propio
  target; `FORMAT_PARITY_REAL_INPUT` con la ruta absoluta del LAZ de Belvedere del checkout principal.
- Código, comentarios y commits en inglés; documentos en español; sin menciones a asistentes en commits.

## Decisiones tomadas en el plan

- **Presupuesto de memoria con W ≥ 2** (técnico y blando): hoy cada rama del lote se comprueba contra su parte del
  presupuesto (`branch_memory_budget`, `memory.rs:73-118`) y las ramas se fusionan al final del lote. Con un hijo de la
  raíz por lote no hay ramas: cada confirmación se comprueba contra el presupuesto completo menos la sobrecarga de los
  W workers (la misma que suma la estimación final, `plan.rs:237-257`), de modo que una conversión falla por memoria
  si y solo si su estimación final supera el presupuesto. Los umbrales de los tests con W ≥ 2 que fijan literales se
  recalculan con esa regla y se anota el motivo en el commit. El web usa el mismo planificador y cambia igual (sin
  cambiar bytes).
- **Paralelismo:** se pierde el paralelismo de la primera división de cada hijo de la raíz; se mide (T3). Recuperarlo
  (adelantar el siguiente hijo cuando haya hilos ociosos) queda fuera de este plan.
- **Tope de la preparación concurrente, por bytes:** los bloques de hoja tienen como mucho `point_limit` puntos
  (`node_job.rs:1129-1134`), pero un bloque LOD lleva todas las muestras de su nodo (`lod_blocks`, `node_job.rs:948-978`;
  hasta `GLOBAL_SAMPLE_GRID`³ en el peor caso, ~257 000 visto en Albuera). Por eso `WritingSink` admite bloques en
  preparación mientras la suma de sus bytes sin comprimir (`point_count` × tamaño de registro) no pase de la parte del
  presupuesto que la planificación no usa, y siempre admite al menos uno.

## Review Focus

1. **Bytes idénticos en cualquier W y ejecutor** (W ∈ {1, 2, 4, 8}, ejecutor serie y con hilos): la salida en flujo es
   igual a la de «planificar y luego escribir».
2. **Errores tras escribir bloques** (`TooManyBlocks`, presupuesto, rangos de atributos, `SpatialHierarchy::new`,
   cancelación a mitad): mismo tipo de error que hoy, sin salida publicada y sin temporales.
3. **Un spool compartido por varios bloques** (LOD o hoja con varios bloques): se borra tras su último bloque y nunca
   antes, aunque sus bloques se preparen en paralelo.
4. **Nube con un solo nodo (raíz hoja) o raíz con todos los puntos iguales:** el camino en flujo escribe el bloque de la
   raíz y termina igual que hoy.
5. **Validación de la salida** (`OutputValidation`, `las_source.rs:481-498`): sigue comprobando los ids de bloque y de
   fuente aunque los bloques se escriban durante la planificación.

---

### Task 1: Block encoding without the full hierarchy

**Files:**
- Modify: `crates/viewer_format/src/prepare.rs:49-447`, `crates/viewer_format/src/block_encoder.rs`,
  `crates/viewer_format/src/global_frame.rs:372-518`, `crates/viewer_format/src/lib.rs`
- Test: `crates/viewer_format/src/writer_tests.rs` (o el fichero de tests del codificador que ya exista)

**Interfaces:**
- Produces (todo en `viewer_format`, público):
  - `pub struct EncodingHeader` — marco de coordenadas, esquema de atributos, Extra Bytes de LAS y
    `Option<GlobalFrame>` solo de la raíz (sin nodos). `EncodingHeader::new(frame: CoordinateFrame, schema: Vec<AttributeDescriptor>, global: Option<GlobalFrame>, extras: Vec<LasExtraByte>) -> Result<Self, FormatError>`
    y `EncodingHeader::from_metadata(metadata: &FormatMetadata) -> Self`.
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct BlockCell { pub depth: u32, pub prefix: [u64; 3] }`.
  - `FormatBlockEncoder::for_header(version: FormatVersion, header: &EncodingHeader, options: FormatWriterOptions) -> Result<Self, FormatError>`
    y `prepare_in_cell(&self, block: SourcePointBlock, cell: Option<BlockCell>) -> Result<PreparedPointBlock, FormatError>`
    (`cell` es obligatoria en formatos de marco global e ignorada en el resto).
  - El camino actual (`for_metadata` / `prepare`) pasa a ser un envoltorio que calcula la celda con `block_box(id)`.

- [ ] **Step 1: failing test** `header_path_prepares_the_same_bytes` — para metadatos v5, v7.4-pre y v7 de los tests
  existentes, cada bloque preparado con `for_header` + `prepare_in_cell(celda del nodo dueño)` es byte a byte igual al
  preparado con el camino de metadatos.
- [ ] **Step 2:** `cargo test -p viewer_format --offline header_path` → FAIL (no existe la API).
- [ ] **Step 3:** implementar; `prepare_point_block_for_version_impl` recibe una vista (esquema, origen,
  `Option<&GlobalFrame>`) y una `Option<CellBox>` en vez de `&FormatMetadata`.
- [ ] **Step 4:** `cargo test --workspace --offline` y `cargo clippy --workspace --all-targets --offline -- -D warnings` → PASS.
- [ ] **Step 5:** commit `feat(format): prepare blocks from an encoding header and the node cell`.

### Task 2: Streaming writer

**Files:**
- Modify: `crates/viewer_format/src/writer.rs:241-604`
- Test: `crates/viewer_format/src/writer_tests.rs`

**Interfaces:**
- Consumes: `EncodingHeader`, `BlockCell` (tarea 1).
- Produces: `FormatWriter::new_split_streaming(version, writer, attributes, header: &EncodingHeader, options) -> Result<Self, FormatError>`
  y `finish_pair_with_metadata(self, metadata: FormatMetadata) -> Result<(W, Option<W>), FormatError>`. En modo flujo,
  `block_nodes` es `None`; por bloque solo se comprueban envenenamiento, duplicados e ids estrictamente crecientes, y
  se guarda la celda usada. `finish_pair_with_metadata` valida los metadatos (como el constructor de hoy), comprueba
  que cada bloque escrito está en la jerarquía y que su celda coincide con `metadata.block_box`, y escribe índice y pie
  como `finish_pair`.

- [ ] **Step 1: failing tests** — `streaming_writer_matches_the_classic_bytes` (misma secuencia de bloques, mismos
  bytes de `.oct` y `.octattrs` que `new_split_with_layout` + `finish_pair`); `streaming_finish_rejects` con casos
  bloque que falta (`MissingBlock`), bloque no referenciado, duplicado, ids no crecientes y celda distinta.
- [ ] **Step 2:** `cargo test -p viewer_format --offline streaming` → FAIL.
- [ ] **Step 3:** implementar.
- [ ] **Step 4:** tests del workspace y clippy → PASS.
- [ ] **Step 5:** commit `feat(format): streaming split writer that takes the metadata at finish`.

### Task 3: Block sink in the planner

**Files:**
- Modify: `crates/viewer_convert_core/src/spatial/builder.rs` (`commit_node` 636-665, `allocate_block_id` 900-906,
  `append_branch` 791-898), `crates/viewer_convert_core/src/spatial/plan.rs:237-345`, `crates/viewer_convert_core/src/spatial.rs`
- Test: tests del núcleo junto a `spatial.rs` y `crates/viewer_convert/src/lib_tests.rs`

**Interfaces:**
- Produces: `pub struct CommittedNode { pub id: NodeId, pub level: u32, pub cell: Option<viewer_format::BlockCell> }`;
  `pub trait BlockSink { fn accept(&mut self, node: &CommittedNode, blocks: Vec<PlannedBlock>) -> Result<(), ConvertError>; }`;
  `pub struct CollectSink(pub Vec<PlannedBlock>)`; `build_spatial_plan_from_source_with_sink(…, sink: &mut dyn BlockSink)`
  (las funciones actuales la envuelven con `CollectSink`, así que `SpatialPlan.blocks` no cambia). `SpatialBuilder`
  lleva `block_count: usize`, que sustituye a `output_blocks.len()` en `builder.rs:662`, `:729`, `:906` y `plan.rs:240`.
  Mientras existan ramas, `append_branch` entrega al sumidero los bloques ya renumerados, así que el sumidero siempre
  ve ids globales en preorden.

- [ ] **Step 1: failing test** `sink_sees_the_planned_blocks_in_preorder` — un sumidero que registra recibe exactamente
  `SpatialPlan.blocks` (mismos ids, orden y spools) con W ∈ {1, 2, 8} y con los ejecutores serie y con hilos.
- [ ] **Step 2:** el test falla (no existe el sumidero).
- [ ] **Step 3:** implementar.
- [ ] **Step 4:** tests del workspace (incluida la paridad nativa/serie de `lib_tests.rs:2829-2950`) y clippy → PASS.
- [ ] **Step 5:** commit `feat(convert): the planner hands committed blocks to a sink`.

### Task 4: One root child at a time with W threads

**Files:**
- Modify: `crates/viewer_convert_core/src/spatial/builder.rs:684-898`, `crates/viewer_convert_core/src/spatial/memory.rs`,
  `crates/viewer_convert_core/src/pipeline/executor.rs:36-68`, `crates/viewer_convert/src/executor.rs`,
  `crates/viewer_convert/src/lib_tests.rs` (literales de presupuesto con W ≥ 2: `:1904-1929`, `:1949-1998`,
  `:2665-2700`; precedencia de errores `:2596-2630`)

**Interfaces:**
- Consumes: `BlockSink`, `block_count` (tarea 3).
- Produces: el trait del ejecutor pasa a `fn plan_root_child(&self, builder: …, parent: NodeId, task: ChildOutput, threads: usize) -> Result<(), ConvertError>`
  (serie: `build_node(job, Some(parent))`); el ejecutor con hilos lanza `threads = parallel_workers` hilos sobre la
  cola de trabajos de ese hijo y su cursor en preorden llama a `builder.commit_job` directamente; los ids de
  `OpenNodeEvent` salen de `SpatialBuilder::node_count()`. Se borran `BranchOutput`, `with_branch`, `append_branch` y
  `branch_memory_budget`. `workers_used` sigue siendo `min(W, k)` para que la estimación final no cambie
  (`lib_tests.rs:559-562`). Regla de presupuesto: la de «Decisiones tomadas en el plan».

- [ ] **Step 1:** recalcular los literales de presupuesto afectados con la regla nueva y reescribir el test de
  precedencia para que tenga dos nodos con error bajo un mismo hijo de la raíz; el test del pánico durante el progreso
  (`:2631`) se mantiene. Ejecutar → los tests nuevos fallan con el código actual.
- [ ] **Step 2:** implementar.
- [ ] **Step 3:** tests del workspace (paridad de bytes nativa/serie, forma y pico de `lib_tests.rs:2702-2765`) y clippy
  → PASS; `cargo check -p viewer_convert_core --target wasm32-unknown-unknown --offline` → OK.
- [ ] **Step 4:** commit `refactor(convert): plan one root child at a time with W threads; ids are global at commit`.

### Task 5: CLI writes blocks while planning

**Files:**
- Modify: `crates/viewer_convert/src/las_source.rs:300-710` (camino sin checkpoint), `crates/viewer_convert/src/blocks.rs:86-125`,
  `crates/viewer_convert/src/publish.rs`
- Test: `crates/viewer_convert/src/lib_tests.rs`, `crates/viewer_convert/src/cli.rs` (tests de cancelación)

**Interfaces:**
- Consumes: `new_split_streaming`, `finish_pair_with_metadata` (tarea 2); `BlockSink`, `CommittedNode`,
  `build_spatial_plan_from_source_with_sink` (tarea 3).
- Produces: `WritingSink` (en `viewer_convert`): acumula bloques confirmados y, al llegar al tope de bytes de
  «Decisiones tomadas en el plan» o al cerrar, los pasa por `process_batches` (prepara con `prepare_planned_block`, escribe en orden con `write_prepared`, que
  borra cada spool tras su último bloque); registra ids de bloque y de fuente para `OutputValidation`. Sin checkpoint,
  el CLI crea los temporales de salida y el escritor en flujo antes de planificar, planifica con `WritingSink` y
  termina con `finish_pair_with_metadata(plan.metadata)`; con checkpoint, el flujo actual.

- [ ] **Step 1: failing tests**: `streaming_cli_output_matches_the_planned_path` (Belvedere recortado o fixture de
  `lib_tests.rs`: mismos bytes que el camino de «planificar y luego escribir» con W ∈ {1, 4});
  `spools_are_deleted_during_planning` (con el ledger de diagnóstico: el pico de spools vivos es menor que el del
  camino actual en un fixture con varios hijos de la raíz); `a_late_plan_error_leaves_no_output` (forzar
  `TooManyBlocks` o presupuesto tras escribir bloques: mismo error, ningún fichero de salida ni temporal).
- [ ] **Step 2:** los tests fallan.
- [ ] **Step 3:** implementar; las etapas de progreso no se intercalan en la consola (`main.rs:147-149` imprime una
  línea por cambio de etapa): mientras se planifica, el progreso de escritura se agrega a la etapa de planificación.
- [ ] **Step 4:** tests del workspace (incluidos `:1000-1015`, `:857-893`, `:1052-1080`, `cli.rs:368`, `:423`) y clippy
  → PASS; ajustar solo expectativas que fijaban el orden interno de etapas, nunca la salida.
- [ ] **Step 5:** Belvedere con `scripts/measure_temp_disk.sh`: SHA iguales a T1; anotar pico y tiempo.
- [ ] **Step 6:** commit `feat(convert): the CLI writes blocks while it plans (RQ-06 phase A)`.

### Task 6: Measure phase A and record it

**Files:**
- Create: `docs/records/benchmarks/TEMPORALES_CLI_FASE_A_<dd-mm-aaaa>.md` y
  `docs/records/benchmarks/data/temporales-cli-fase-a-<dd-mm-aaaa>/`
- Modify: `docs/plans/temporales-del-conversor.md` (`Estado:`), este plan (`Estado:` y casillas), `docs/estado.md`,
  `docs/README.md`, `docs/requirements.md` (RQ-06 y OQ-04)

- [ ] **Step 1:** `FUZZ_CHECK=0 scripts/check.sh --offline` → 23/23, con la paridad de bloques reales.
- [ ] **Step 2:** `scripts/measure_temp_disk.sh` con `main` y con la rama, una conversión cada vez, en Albuera y
  Majadas (y la rama en Belvedere), con tiempos por fase. Esperado: SHA iguales; pico aparente de Albuera ≈ 1,6–2,0 ×
  el LAZ (estimado en el diseño).
- [ ] **Step 3:** parar y llevar las cifras al propietario si: el tiempo frente a `main` sube > +25 %; el RSS sube
  > +100 MB; Albuera no llega a ≤ 2 × LAZ (T2). Si un SHA difiere, parar y buscar el fallo antes de medir más.
- [ ] **Step 4:** registro fechado con las series de muestras, la orden exacta y el equipo; actualizar los documentos
  listados; `python3 scripts/check_docs_links.py` y `python3 scripts/check_docs_index.py` → OK.
- [ ] **Step 5:** commit `docs: measure per-subtree writes against main (RQ-06)`.
