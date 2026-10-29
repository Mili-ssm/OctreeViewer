# Conversor web paralelo — plan de la etapa 2

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Estado (29-10-2026):** Tareas 1–3 hechas ([modelo por nodos](../records/benchmarks/NODE_JOB_ARENA_MODEL_2026-10-29.md), [cuota OPFS](../records/benchmarks/OPFS_QUOTA_2026-10-29.md)); S0 con bloques reales de Belvedere corre en `check.sh`. La Tarea 1 activó el criterio de parada para arenas por trabajo; el propietario decidió seguir con la variante c (un fichero OPFS por spool) tras el spike S9 (Tarea 5). La Tarea 5 está hecha y el paso 3 de la Tarea 4 (nativo) queda desbloqueado; en la web, S9 llevó al híbrido RAM/OPFS ([decisión 2](#tarea-5-spike-s9--ficheros-opfs-por-spool)).

**Objetivo:** cerrar las tres incógnitas que dejó la etapa 1 (pico de los trabajos por nodo, S0
con bloques reales, cuota real) y llegar a M1: el pipeline nativo expresado con traits y trabajos
por nodo dentro de `viewer_convert`, con los mismos bytes y sin regresión.

**Arquitectura:** la [spec](conversor-web-paralelo.md) es la autoridad. Decisiones del propietario
del 29-10-2026 que este plan aplica: **trabajos por nodo** (cada trabajo lee la entrada de un nodo y
escribe un fichero OPFS por spool de hijo; el host borra cada fichero cuando termina el trabajo que lo
consume, y LOD y hojas se retienen hasta la fase de bloques: variante c), **Chrome `file://` solo con spools en RAM** (R8) y **medir la cuota real** antes de M6
(S8). Etapa anterior: [etapa 1](conversor-web-paralelo-etapa1.md); evidencia:
[referencia M0](../records/benchmarks/WEB_CONVERTER_BASELINE_2026-10-29.md) y
[spike S1](../records/benchmarks/OPFS_ARENA_SPIKE_2026-10-29.md).

**Tecnología:** Rust nightly fijado, `viewer_convert` (planificador en `src/spatial.rs`),
`tools/format_parity` (digest nativo/wasm32 bajo node), OPFS `FileSystemSyncAccessHandle`,
Playwright para los navegadores.

**Spec:** [conversor-web-paralelo.md](conversor-web-paralelo.md).

## Restricciones globales

- `scripts/check.sh` en verde (todos los pasos) al cerrar cada tarea.
- No cambiar la fecha del nightly ni añadir `#![feature]`.
- `FormatVersion::V<n>` solo en `crates/viewer_format`.
- R1: los SHA de salida del CLI no cambian; referencia: [M0](../records/benchmarks/WEB_CONVERTER_BASELINE_2026-10-29.md) (LAS pequeño `413858a9…`, Belvedere `6ca0f86d…`, Belvedere divisiones 8 `fb90a490…`, Majadas `74226286…`).
- R7: mediana pareada de `real_dataset_bench convert` en Belvedere (4 workers, perfil `perf`), ≥ 5 rondas A/B alternadas, dentro de ±3% de la rama base medida en la misma sesión.
- Registros nuevos fechados con el día de ejecución (`dd-mm-aaaa` en el texto, `<aaaa-mm-dd>` ISO en el nombre), en español; no editar registros ya mergeados; no trasladar cifras entre revisiones, perfiles ni equipos.
- Commits convencionales en inglés con la línea `Co-Authored-By` del modelo que los escribe; sin push.
- No enviar avisos upstream (backlog, prioridad 9 del README).

## Review Focus

1. **Raíz con todos los puntos iguales o no divisible** con el planificador por nodos → Tarea 4 (tests de la etapa 1 en verde y `coincident_fixture` en la matriz).
2. **Spools vacíos** (hijos sin puntos) con un fichero por spool: no deben crear fichero ni romper el orden de identificadores → Tarea 4.
3. **Bloques reales con atributos dispersos** (clasificación, retornos, GPS) en S0 → Tarea 2 usa bloques de Belvedere, no sintéticos.
4. **Cuota denegada o `persist()` rechazado** en S8 → Tarea 3 registra el comportamiento, no solo el caso feliz.
5. **Errores con índice global de punto** al partir la entrada por rangos de chunks → Tarea 4 compara el mensaje de error de un LAS corrupto entre ejecución en serie y por rangos.

---

### Tarea 1: modelo de pico de los trabajos por nodo (S1b)

**Archivos:**
- Modificar: `crates/viewer_convert/src/spatial.rs` (registro de eventos de spool, solo con diagnósticos activos), `crates/viewer_convert/examples/real_dataset_bench.rs`
- Crear: `tools/arena_model/` (binario de simulación, `publish = false`) o un script Python en `crates/viewer_convert/benchmarks/` si encaja mejor con los existentes
- Crear: `docs/records/benchmarks/NODE_JOB_ARENA_MODEL_<aaaa-mm-dd>.md` y `docs/records/benchmarks/data/node-job-arena-model-<aaaa-mm-dd>.json`

**Interfaces:**
- Produce: un registro de eventos por nodo en el orden real de ejecución nativa, con `node_id`, `parent_id`, `depth`, `input_spool_bytes`, `child_spool_bytes: Vec<u64>` (por hijo) y el instante relativo de inicio y fin del nodo (secuencia, no reloj). Solo se emite con `collect_spool_diagnostics`; no cambia bytes de salida.

- [x] **Paso 1: test del registro** — con la fixture de 4 000 puntos y divisiones 4: la suma de `child_spool_bytes` de todos los eventos es igual a `child_spool_bytes_written` de los diagnósticos actuales, y cada `parent_id` aparece antes como `node_id`.
- [x] **Paso 2: implementar el registro** junto a los contadores de la etapa 1 (`SpoolLedger`, `spatial.rs:616`; `build_root_from_las` `:803`, `build_node` `:977`, `process_child_tasks` `:1158`). Comprobar los SHA de LAS pequeño y Belvedere contra M0.
- [x] **Paso 3: simulación** — reproducir los eventos con N trabajos concurrentes (N = 4 y 8, reparto en el orden de los eventos): cada trabajo de nodo crea una arena con todos sus hijos; la arena del padre se borra cuando el último hijo termina su trabajo; las arenas hoja viven hasta la fase de bloques. Pico = máximo de la suma de arenas vivas. Ejecutar con Belvedere (divisiones 2 y 8) y Majadas (divisiones 2).
- [x] **Paso 4: registro** — tabla pico modelado frente a `peak_live_spool_bytes` nativo de la misma ejecución, con la fórmula y supuestos. Criterio de parada (spec): > 1,5 × → volver al propietario antes de la Tarea 4.
- [x] **Paso 5: commits** — `feat(convert): log per-node spool events for arena modelling` y `docs(bench): record per-node job arena model`.

---

### Tarea 2: S0 con bloques reales de Belvedere

**Archivos:**
- Modificar: `tools/format_parity/src/lib.rs`, `tools/format_parity/src/bin/native_digest.rs`, `tools/format_parity/run-wasm.mjs`, `scripts/check.sh`
- Crear: un volcador de bloques (`tools/format_parity/src/bin/dump_blocks.rs`) que escribe los primeros K = 4 `SourcePointBlock` planificados de una conversión real a un fichero binario en `target/format_parity/`

**Interfaces:**
- Consume: `parity_digest()` y `format_parity_digest()` de la etapa 1 (`lib.rs:178`, `:193`).
- Produce: `pub fn parity_digest_of_blocks(serialized: &[u8]) -> u64`; exportaciones wasm `format_parity_alloc(len: usize) -> *mut u8` y `format_parity_digest_blocks(ptr: *const u8, len: usize) -> u64`; `run-wasm.mjs` acepta una ruta opcional y copia el fichero en la memoria wasm.

- [x] **Paso 1:** volcar 4 bloques de Belvedere (`data/datasets/belv_2009_digital-aerial_pcd_copc.laz`, opciones por defecto del CLI) y comprobar que el digest nativo de esos bloques es estable entre dos ejecuciones.
- [x] **Paso 2:** digest wasm32 del mismo fichero bajo node → igual al nativo.
- [x] **Paso 3:** paso opcional en `scripts/check.sh` («format parity real blocks»), que se omite con un aviso si no existe el fichero de Belvedere, igual que `FUZZ_CHECK` (`check.sh:39-42`); el paso sintético existente no cambia.
- [x] **Paso 4: commit** — `test(format): check native/wasm32 parity on real Belvedere blocks`. Cierra el criterio S0 de la spec.

---

### Tarea 3: cuota real de OPFS (S8)

**Archivos:**
- Crear: `docs/records/benchmarks/data/opfs-quota-<aaaa-mm-dd>.{html,js,jsonl}` y `docs/records/benchmarks/OPFS_QUOTA_<aaaa-mm-dd>.md`

- [x] **Paso 1:** página que llama a `navigator.storage.persisted()`, `persist()` y `estimate()` y escribe en OPFS hasta `QuotaExceededError` (bloques de 1 MiB, borrando al terminar), registrando la cuota declarada, los bytes escritos al fallar y el resultado de `persist()`.
- [x] **Paso 2:** ejecutar en Chrome y en el Firefox del sistema (158), con perfil nuevo persistente y con `persist()` concedido y denegado, por `http.server` y por `file://` (en Chrome `file://` se espera `SecurityError`, ya registrado en S1). Si conceder `persist()` exige un diálogo que Playwright no maneja, documentar el paso manual y repetirlo a mano.
- [x] **Paso 3: registro** — tabla por navegador × modo × `persist()`; conclusión explícita: ¿cabe Majadas (≈ 19 GiB de spools vivos, según la spec, más ~7,5 GB de salida)? Si no cabe en ningún perfil, volver al propietario antes de M6.
- [x] **Paso 4: commit** — `docs(bench): record real OPFS quota per browser`.

---

### Tarea 5: spike S9 — ficheros OPFS por spool

Decisión del propietario (29-10-2026): la Tarea 4 sigue con la variante c; este spike comprueba que el navegador aguanta ~650 ficheros OPFS vivos y handles síncronos concurrentes.

**Archivos:**
- Crear: `docs/records/benchmarks/OPFS_SPOOL_FILES_<aaaa-mm-dd>.md` y `docs/records/benchmarks/data/opfs-spool-files-<aaaa-mm-dd>.{html,js,jsonl}`, más un driver de Playwright (reutilizar el patrón del driver de la Tarea 3; perfiles bajo `target/` en `/home`, nunca en `/tmp`; limpieza en `finally`).

- [x] **Paso 1:** página en la que N = 4 y 8 workers `Blob` crean cada uno los ficheros hijos de su trabajo (8 por trabajo con divisiones 2; hasta 81 en la raíz con divisiones 8) con `createSyncAccessHandle` abiertos a la vez, añaden en bloques de 32 KiB y de 1 MiB, cierran, y el host borra cada fichero en cuanto termina su consumidor. Hasta 700 ficheros vivos a la vez. Total de bytes ≤ 8 GiB por ejecución.
- [x] **Paso 2:** medir en Chrome y en Firefox de Playwright por http (y Firefox `file://`): ficheros vivos y handles abiertos simultáneos alcanzados, errores (`NoModificationAllowedError`, límites), latencia de `createSyncAccessHandle`, `flush` y `removeEntry` (p50/p95/máx) y rendimiento de escritura frente a la arena única de S1 de la misma sesión (volver a ejecutar la página de S1 como comparación; no copiar sus cifras).
- [x] **Paso 3: registro** — tabla por navegador × N; una ejecución por celda es aceptable si se declara.
- [x] **Criterio de parada:** si algún navegador falla por debajo de 700 ficheros vivos o de 8 × 81 handles abiertos, o el rendimiento cae más de 2 × frente a la arena única, volver al propietario antes del paso 3 de la Tarea 4.
- [x] **Paso 4: commit** — `docs(bench): record OPFS per-spool file spike`.

**Estado (29-10-2026):** hecha ([registro](../records/benchmarks/OPFS_SPOOL_FILES_2026-10-29.md)). Un fichero por spool funciona en Chrome y Firefox (~850 ficheros vivos, 664 handles, 0 errores), pero con un coste fijo por fichero de ~26–40 ms en Firefox y ~2,9–4,4 ms en Chrome (el criterio de 2 × no se cumple). **Decisión del propietario 2:** los spools web siguen el híbrido de la spec (segmentos en RAM que se vuelcan a su propio fichero al superar un presupuesto); el `SpoolStore` nativo no cambia, así que el paso 3 de la Tarea 4 sigue. Un spike S10 mide el pico de RAM del híbrido antes de M3b.

---

### Tarea 4: M1 — traits y trabajos por nodo dentro de `viewer_convert`

Los pasos 1 y 2 pueden ir en paralelo con la Tarea 5; el paso 3 (nativo) está desbloqueado desde que la Tarea 5 se cerró. La Tarea 1 activó su criterio de parada para arenas por trabajo; el propietario decidió seguir con la variante c (29-10-2026).

**Archivos:**
- Modificar: `crates/viewer_convert/src/spatial.rs`, `crates/viewer_convert/src/las_source.rs`, `crates/viewer_convert/src/lib.rs`, `crates/viewer_convert/src/error.rs`, `crates/viewer_convert/src/ept.rs`
- Crear: `crates/viewer_convert/src/pipeline/` con `spool_store.rs`, `point_source.rs`, `clock.rs`, `node_job.rs`

**Interfaces** (los nombres son los de la spec; las firmas exactas las fija el paso 1 y se anotan aquí antes de implementar):
- `SpoolStore`: un fichero por spool: crear, añadir, leer y borrar, con semántica cercana a la nativa; implementación nativa sobre ficheros con los mismos bytes que hoy. Los extents de LOD y de hojas se retienen hasta la fase de bloques.
- `PointSource`: leer puntos por rango de chunks LAZ o de registros LAS, con offset global de punto; sin `Reader::seek` en chunks variables (spec, bloqueos comprobados).
- `Clock`: `Instant` en nativo.
- Enmiendas del pre-mortem del 29-10-2026 que aplican a esta tarea:
  - A6: dividir el paso 4 en 4a (NodeJob en preorden serial + SHA de M0, incluidos Belvedere divisiones 8 y autzen, invariantes estructurales, barrido de `planner_budget` igual a la base) y 4b (cola de listos concurrente tras una opción, conservando la ruta antigua hasta que R7 pase).
  - A7: fijar en el paso 1 el contrato de presupuesto/estimación.
  - A8: errores deterministas (gana el nodo o rango de menor preorden; índice global en `PointCountMismatch`/`LasDecoderPanicked`).
  - A9: el `PointSource` de M1 es el Reader actual sobre el rango completo más rangos de registros LAS; los rangos por offset de chunk LAZ pasan a M2 tras un spike tipo S2.
  - A10: sin `PathBuf`/`File`/`Instant`/tipos de hilo en la E/S de los trabajos; un `SpoolStore` en memoria que imponga la semántica, con su pico comprobado frente al modelo de la Tarea 1 (variante c).
  - A11: R7 solo en máquina en reposo, 7–10 pares A/B, registrando `/proc/pressure`, el governor y el perfil de energía.
- `NodeJob`: entrada = nodo e identificador de spool de entrada; salida = hijos (identificadores de spool, recuentos y acumuladores LOD) y bloques planificados; el planificador nativo ejecuta trabajos con `std::thread::scope` y fusiona en preorden como `append_branch`.

#### Firmas fijadas en el paso 1 (29-10-2026)

Módulo `crates/viewer_convert/src/pipeline/`, todo `pub(crate)`. Las líneas citadas son de `spatial.rs` en `44d2a9a`. Enmendado el mismo día tras la revisión de diseño («aprobar con cambios»; R1 se mantiene en 4a y 4b).

```rust
// spool_store.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct SpoolId(pub u64);
impl SpoolId { pub const SOURCE: Self = Self(0); }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpoolExtent { pub offset: u64, pub len: u64 } // bytes
/// One file per spool. Per id: create → append* → close → (len | reader)* → delete.
/// Ids are never reused; any other order is an error.
pub(crate) trait SpoolStore {
    type Reader: std::io::Read;
    fn create(&self, id: SpoolId) -> Result<(), ConvertError>;
    fn append(&self, id: SpoolId, bytes: &[u8]) -> Result<(), ConvertError>;
    fn close(&self, id: SpoolId) -> Result<(), ConvertError>;
    fn len(&self, id: SpoolId) -> Result<u64, ConvertError>;
    fn reader(&self, id: SpoolId, extent: SpoolExtent) -> Result<Self::Reader, ConvertError>;
    fn delete(&self, id: SpoolId) -> Result<(), ConvertError>;
}
pub(crate) struct FileSpoolStore { root: PathBuf, ledger: Option<SpoolLedger>, open: Mutex<OpenSpools> }
impl FileSpoolStore {
    pub fn new(root: &Path, diagnostics: bool) -> Self;
    /// Checkpoint path: `SOURCE` already exists and is closed; the ledger starts at `source_len`.
    pub fn with_closed_source(root: &Path, source_len: u64, diagnostics: bool) -> Self;
}
pub(crate) struct MemSpoolStore { state: Mutex<MemSpools> } // + fn stats() -> MemSpoolStats
pub(crate) struct MemSpoolStats { pub live_bytes: u64, pub peak_bytes: u64, pub live_spools: usize, pub peak_spools: usize }
```

- **Nativo, misma E/S que hoy:** `SOURCE` → `source.spool` (el nombre del checkpoint, que usa un `FileSpoolStore` sobre su raíz); el resto → `spool-{id}.spool`.
  - `SOURCE` mantiene un único `BufWriter<File>` abierto de `create` a `close`, y `close` hace `sync_all` (como `spool_source`, `:1768-1796`).
  - En los demás, `create` es perezoso (solo anota el id en `open`); el primer `append` hace `create_new` y los siguientes reabren en modo añadir, como cada vaciado de 32 KiB de hoy (`:600`). Así no se agotan descriptores con 8 trabajos × 512 hijos.
  - `delete` descuenta del `SpoolLedger`, que pasa al store junto con `event_seq`.
  - Checkpoint: `with_closed_source` sustituye a `SpoolLedger::with_live` (`:442`) tras el `link_or_copy` de `las_source.rs:342-345`.
- **Un store compartido (4a):** un solo `S: SpoolStore + Sync` sirve a todas las ramas. Desaparecen los directorios por rama (`:1396`) y el `Arc<SpoolLedger>` (`:703`, `:1422`). La fase de bloques lee cada `PlannedBlock.spool` a través del mismo `&S`.
- **Spools vacíos (Review Focus 2):** el búfer de hijo crea su spool en el primer vaciado; un hijo sin puntos no llama a `create`, su id queda reservado y se omite.
- **Ids:** el host reserva `divisions³ + 1` ids por trabajo divisible (hijo `k` → base + `k`; LOD → base + `divisions³`), con un contador atómico propio o con rangos disjuntos por rama; el contador no forma parte de la E/S del trabajo. Los ids no llegan a la salida.
- **Invariante:** un trabajo solo toca `input` y `first_output..first_output + divisions³`, así que el host puede preabrirlos.
- **Extents:** `PlannedBlock` cambia `path` por `spool: SpoolId`; `read` y `release_spool_after_write` reciben `&S`. LOD y hojas se retienen hasta la fase de bloques; el nodo lee su entrada entera (`len`) para que un spool largo siga dando `PointCountMismatch`.
- **En memoria (A10):** `MemSpoolStore` rechaza cualquier llamada fuera del ciclo (añadir tras `close`, leer antes de `close`, borrar abierto, extent fuera de rango, id repetido) y cuenta bytes y spools vivos al añadir. Test: con 4 000 puntos y divisiones 4 en serie, `peak_bytes` = `peak_live_spool_bytes` del ledger nativo y ≤ variante c con N = 1 de `model_node_job_arena.py` sobre los eventos de la misma ejecución; tras la fase de bloques no queda ningún id.

```rust
// point_source.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PointRange { pub start: u64, pub end: u64 } // global point indices
impl PointRange { pub fn split(total: u64, batch: u64, parts: usize) -> Vec<Self>; } // starts are multiples of `batch`
pub(crate) trait PointSource {
    fn header(&self) -> &las::Header;
    fn range(&self) -> PointRange;
    fn next_index(&self) -> u64; // global index of the next point
    /// Up to `count` points, clamped to the range end; fewer only if the file ends early.
    fn fill(&mut self, count: u64, data: &mut las::PointData) -> Result<u64, ConvertError>;
}
pub(crate) struct LasPointSource<R: BorrowMut<las::Reader>> { reader: R, range: PointRange, next: u64 }
impl<R: BorrowMut<las::Reader>> LasPointSource<R> {
    pub fn full(reader: R) -> Self;
    /// `seek` to `range.start` on fixed-size records; compressed input is rejected (A9).
    pub fn records(reader: R, range: PointRange) -> Result<Self, ConvertError>;
}
// clock.rs
pub(crate) trait Clock { type Instant: Copy; fn now(&self) -> Self::Instant; fn elapsed_ns(&self, since: Self::Instant) -> u64; }
pub(crate) struct SystemClock; // Instant = std::time::Instant
```

M1 lee en producción con `full` (un solo rango); `records` existe para los tests de rangos y para M2. Los rangos por chunk LAZ pasan a M2 tras un spike tipo S2 (A9). `batch` = `input_batch_point_limit`: así las llamadas a `fill` de una ejecución por rangos coinciden con las de la serie. **Pendiente de portabilidad en M2:** `header() -> &las::Header` ata el trait al crate `las`; M2 decide si el núcleo lo conserva.

```rust
// node_job.rs
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct NodePath(pub Vec<u16>); // child keys from the root; Ord = preorder
pub(crate) struct JobContext { pub point_limit: u64, pub spatial_divisions: u8, pub frame: CoordinateFrame, pub layout: SpoolLayout, pub collect_diagnostics: bool }
pub(crate) struct NodeJob { pub path: NodePath, pub level: u32, pub bounds: SpatialBounds, pub input: SpoolId, pub point_count: u64, pub input_is_child_spool: bool, pub first_output: SpoolId }
pub(crate) struct ChildOutput { pub key: u16, pub spool: SpoolId, pub point_count: u64, pub bounds: SpatialBounds }
pub(crate) struct BlockPlan { pub spool: SpoolId, pub offset_points: u64, pub point_count: usize, pub is_source_points: bool, pub final_block_in_spool: bool }
pub(crate) struct NodeJobOutput {
    pub path: NodePath,
    pub point_count: u64,            // LOD samples, or input points for a leaf
    pub geometric_error: f64,
    pub blocks: Vec<BlockPlan>,      // no ids yet, in allocation order
    pub children: Vec<ChildOutput>,  // ascending key, empty ones skipped
    pub release: Vec<SpoolId>,       // deleted by the host after the job
    pub metrics: Option<JobMetrics>, // bytes per child, scanned, LOD, phases in ns
}
pub(crate) enum JobProgress { SourcePoints(u64), NodeHeartbeat }
pub(crate) fn run_node_job<S: SpoolStore, C: Clock>(ctx: &JobContext, job: &NodeJob, store: &S, clock: &C,
    progress: &mut impl FnMut(JobProgress) -> Result<(), ConvertError>) -> Result<NodeJobOutput, ConvertError>;
pub(crate) struct RootPartition { pub children: Vec<ChildOutput>, pub samples: SampleGrid, pub all_points_equal: bool,
    pub release: Vec<SpoolId>, pub metrics: Option<JobMetrics> } // release: created children when all points are equal
pub(crate) fn run_root_partition<P: PointSource, S: SpoolStore, C: Clock>(ctx: &JobContext, info: &SourceInfo, bounds: SpatialBounds,
    first_output: SpoolId, source: &mut P, store: &S, clock: &C, progress: &mut impl FnMut(JobProgress) -> Result<(), ConvertError>) -> Result<RootPartition, ConvertError>;
pub(crate) fn spool_source<P: PointSource, S: SpoolStore>(source: &mut P, info: &SourceInfo, point_limit: u64, store: &S, id: SpoolId,
    progress: &mut impl FnMut(JobProgress) -> Result<(), ConvertError>) -> Result<u64, ConvertError>;
```

- **Nodo interno:** escanea, escribe hijos y LOD (`write_lod`, compartida con la raíz) y cierra sus spools; `release` = [entrada]. **Hoja:** no lee; `blocks` = trozos de `point_limit` del spool de entrada, que se retiene; si tiene 0 puntos, `release` = [entrada] (`:1660`). **Todos los puntos iguales:** hoja con `release` = hijos creados; la entrada se retiene (`:1213-1226`).
- **Raíz** (`:867`): `SampleGrid` expone los acumuladores LOD (fusionables por rango en M2). No divisible: `spool_source` a `SOURCE` y `NodeJob` hoja. Todos iguales (`:957-973`): el trabajo cierra los hijos ya creados **sin vaciar sus búferes**, como hoy, y los devuelve en `release`. Después el host los borra, reabre la fuente, `spool_source` a `SOURCE` y `NodeJob` de la raíz, que vuelve a escanear y queda hoja. Checkpoint: `NodeJob` sobre `SOURCE`.
- **Commit** (host, en preorden): `id = node_records.len()`; por cada `BlockPlan`, `allocate_block_id` y `PlannedBlock`; luego `ensure_memory`, `NodeRecord` y progreso. Mismo orden y comprobaciones que hoy, así que los ids de nodo y bloque son los de la serie.

**Planificador (A6).**

1. **4a, preorden serial:** `build_node` sigue siendo el driver recursivo: `run_node_job` → el host borra `release` → commit → hijos por clave ascendente. Los hilos por hijo raíz, `append_branch` y los presupuestos de rama no cambian; cada rama usa el mismo driver. Cierre: SHA de M0 (incluidos Belvedere divisiones 8 y autzen), invariantes y barrido de `planner_budget` igual a la base.
2. **4b, cola de listos:** `NodeScheduler::{RootBranches (por defecto), ReadyQueue}` en `ConvertOptions` (`pub(crate)`). `workers_used` hilos en `thread::scope` (`S: SpoolStore + Sync`) toman el trabajo listo de menor `NodePath`.
   - **Sin anticipación (R16):** solo se despachan trabajos bajo el lote virtual cuyo commit está en curso. Dentro del lote los hilos se reparten el trabajo de forma dinámica; el lote siguiente empieza cuando el cursor llega a su inicio. Es la barrera por lote de hoy con el mismo límite `min(W, k)`. La anticipación y los límites por encima de `W` quedan para M5.
   - El tamaño del lote virtual sale de la opción `spatial_workers`, no del número de hilos, para que M5 pueda usar N ≠ W.
   - Al terminar un trabajo, el host borra `release`, encola los hijos y guarda la salida en un `BTreeMap<NodePath, _>`. Un cursor (pila de rutas esperadas) hace commit en preorden en cuanto llega la salida siguiente. `start_seq` se fija al despachar y `end_seq` cuando el cursor sale del subárbol, de modo que el anidamiento del test de eventos se mantiene.
3. **Cambio de valor por defecto** en un commit propio, medido por R7; si falla dos veces, se revierte ese commit. Tras pasar R7 se borran `RootBranches` y `append_branch`.

**Presupuesto y estimación (A7).**

- `workers_used` = `min(W, k)` si `W > 1` y `k > 1`, si no 1 (`W` = `spatial_workers`, `k` = hijos no vacíos de la raíz). Es la del lote mayor de hoy (`:1358`): una función del plan, no una medida. En 4b es además el límite de trabajos concurrentes, así que nunca hay más paralelismo que hoy y el término por worker extra sigue cubriéndolo.
- `estimated_peak_memory_bytes`: misma fórmula (`:469-484`); también decide los workers de la fase de bloques.
- `branch_memory_budget` en 4b: lotes virtuales de `workers_used` hijos raíz en orden de clave.
  - El presupuesto de cada lote se calcula cuando el cursor llega al inicio del lote, antes de despachar sus trabajos, como hoy (`:1366-1373`). Usa los recuentos globales ya confirmados y el tamaño del lote.
  - Cada subárbol comprueba sus recuentos locales frente a ese presupuesto, como el builder de rama. Con `workers_used` = 1 se usan los recuentos globales frente a `planner_budget`.
  - Resultado: `MemoryBudgetExceeded` (incluidos `requested` y `limit`) es **idéntico al orden serie, y a hoy cuando como mucho una rama por lote falla**. Los tests `lib_tests.rs:500`, `:702`, `:829` y `:975` no cambian.
- `TooManyBlocks` (R17): se reproduce tal cual. Con `workers_used` > 1 se comprueba dentro del subárbol con el contador local y, al salir de él, con el global, como `append_branch` (`:1518`); con 1, en cada asignación global.

**Errores deterministas (A8).**

- Orden de errores: `(NodePath, fase)` con `BatchBudget < Job < Commit` (`check_memory_budget`, `:2287`), o menor `PointRange::start`. El presupuesto de lote se ordena en la ruta del primer hijo del lote, antes de cualquier error de trabajo de ese lote (I1).
- 4b: con un error en `Q`, no se despachan rutas > `Q`; las < `Q` (y sus hijos < `Q`) terminan, y se devuelve el mínimo. `Cancelled` pierde frente a cualquier error real, como hoy.
- Índices globales (`processed` = puntos ya leídos del rango):
  - `LasDecoderPanicked { message, point_index: Option<u64> }` (R18): `point_index = Some(range.start + processed)` del `fill` o `seek` que falla. `None` al abrir el lector y en EPT.
  - `PointCountMismatch` no gana campo. En lecturas de la fuente, `expected` = `point_count` de la fuente y `actual = next_index()` tras el `fill` corto, que es el índice global del primer punto que falta. En spools, el recuento sigue siendo local al nodo.
  - `PointOutsideHeaderBounds` (`:1866`): `point_index = range.start + processed + index`.
- Review Focus 5: un LAS truncado y uno con un punto fuera de los límites de la cabecera, en serie frente a rangos, dan el mismo `Display`.

**Lista A10.**

- La E/S de los trabajos (`NodeJob`, `NodeJobOutput`, `ChildOutput`, `BlockPlan`, `JobMetrics`, `PointRange`, `SpoolId`, `SpoolExtent`, `NodePath`) es solo enteros, `f64`, `SpatialBounds`, `CoordinateFrame`, `SpoolLayout` y `Vec`. Sin `PathBuf`, `File`, `Instant`, `Arc`, atómicos ni canales: el `SpoolLedger` vive en el store y el tiempo llega como `u64` ns vía `Clock::Instant`.
- Despacho estático: los `run_*` son genéricos en `S`, `P`, `C` y el cierre de progreso. El bucle por punto (`decode_point`, `cell_index`, escritura en el búfer de hijo, `SampleAccumulator::add`) no tiene `dyn`; el store se llama una vez por vaciado de 32 KiB. `on_progress: &mut dyn FnMut` queda solo en el host.
- `Sync` va en el planificador nativo, no en el trait: en la web, cada worker tendrá su propio store. La forma serializable (cable) se decide en M2/M3.

**Mapeo OPFS pendiente de S9** (solo cambia la implementación web; el trait síncrono por id se mantiene):

| Aspecto | Opciones que decide S9 |
| --- | --- |
| `create` | Handles abiertos por el host antes del trabajo (hasta `divisions³ + 1`) o segmento `ArrayBuffer` en RAM volcado al cerrar (R10) |
| `append` | Escritura directa en el handle síncrono o agrupada en bloques de 1 MiB (32 KiB frente a 1 MiB en S9) |
| `close` | Momento del `flush`/`close` y máximo de handles abiertos a la vez (8 × 81 en S9) |
| `delete` | `removeEntry` inmediato entre trabajos o diferido por lotes según su latencia |
| `reader` | `getFile()` tras cerrar o handle síncrono de lectura en los workers de preparación |
| Ficheros vivos | Si no se llega a ~650, empaquetar LOD y hojas retenidos en ficheros compartidos (id → fichero + offset) |

Dependencia abierta (R19): si S9 obliga a empaquetar LOD y hojas, cómo se hace se decide cuando S9 informe.

- [x] **Paso 1: diseño de firmas** — un arquitecto (Opus) fija firmas y el flujo de trabajos por nodo a partir de `spatial.rs` (`build_node` `:1067`, `process_child_tasks` `:1320`) y lo anota en este documento (sección anterior); revisión antes de seguir.
- [ ] **Paso 2: `Clock` y `PointSource`** sin cambio de bytes; tests existentes y SHA M0.
- [ ] **Paso 3: `SpoolStore` nativo** con un fichero por spool (tras la Tarea 5); SHA M0.
- [ ] **Paso 4: planificador por trabajos de nodo** (sustituye el paralelismo solo en hijos raíz); tests de independencia de la etapa 1 (`lib_tests.rs`) y los casos del Review Focus 1, 2 y 5. Antes de 4b, escribir tests de caracterización sobre la ruta actual: serie; W = 2 con comprobaciones locales de rama; y un presupuesto de lote que falla, con `requested` fijado.
- [ ] **Paso 5: R7** — ≥ 5 rondas A/B alternadas frente a la rama base en la misma sesión; registro fechado. Criterio de parada (spec): regresión > 3% tras dos intentos → revertir la extracción.
- [ ] **Paso 6: commits** por paso, convencionales.

---

## Después de la etapa 2

Se planifican con los resultados de las Tareas 1–5, en este orden (spec, «Hitos»):

| Hito | Contenido | Depende de |
| --- | --- | --- |
| M2 | Crate `viewer_convert_core` sin I/O que compila para wasm32; partición por rangos ejecutada en nativo con W = 1..8 y mismos SHA | Tarea 4 |
| M3a | Conversión web con un worker sobre el núcleo, LAS pequeño, paridad en Chrome y Firefox | M2, Tarea 2 |
| S10 | Spike corto: pico de RAM del híbrido segmentos RAM + volcado por spool, con el presupuesto de la spec | Decisión 2 tras S9, Tarea 5 |
| M3b | Un worker con spools híbridos RAM/OPFS a escala Belvedere; eliminar `browser_convert.rs`; Chrome `file://` solo RAM (R8) | M3a, S10 |
| M4 | Pool de preparación y escritor ordenado; dimensionado S7 | M3b |
| M5 | Pool de partición y trabajos por nodo; validación paralela | M4 |
| M6 | Escala Majadas: preflight de cuota, spools en RAM con volcado, limpieza | M5, Tarea 3 |
