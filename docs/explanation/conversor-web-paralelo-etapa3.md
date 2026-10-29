# Conversor web paralelo — plan de la etapa 3

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Estado (29-10-2026):** Tareas 1–3 hechas. S2 ([registro](../records/benchmarks/LAZ_CHUNK_RANGES_2026-10-29.md)): los rangos de chunks LAZ se decodifican sin `seek` ni parche de `laz`; wasm32 decodifica a 1,74 × el nativo. S10 ([registro](../records/benchmarks/HYBRID_SPOOL_RAM_2026-10-29.md)): P1/P2 activan el criterio de parada en divisiones 8 (513 handles por trabajo); se propone P3 (solo el coordinador vuelca), segmentos de 1 MiB, presupuesto de 2 GiB y contrapresión K = 8, pendiente de decisión del propietario antes de M3b. Firmas de M2 fijadas (Tarea 3). Tarea 4 en curso.

**Objetivo:** llegar a M3a: un núcleo `viewer_convert_core` sin E/S que compila para wasm32 (M2),
la partición raíz por rangos de chunks con los mismos bytes en nativo, y una conversión web con un
worker sobre ese núcleo con paridad byte a byte en Chrome y Firefox. Los spikes S2 y S10 cierran
las dos incógnitas que M2 y M3b necesitan.

**Arquitectura:** la [spec](conversor-web-paralelo.md) es la autoridad; la etapa anterior es la
[etapa 2](conversor-web-paralelo-etapa2.md), cuyas firmas (`SpoolStore`, `PointSource`, `Clock`,
`run_node_job`, `run_root_partition`) se conservan. El núcleo recibe la lógica pura de
`viewer_convert` (planificador por nodos, codificación de spools, acumuladores LOD, presupuestos);
el shell nativo conserva ficheros, hilos, checkpoints, EPT y publicación. La partición por rangos
escribe un segmento por (rango, hijo) y el host los une en orden de rango.

**Tecnología:** Rust nightly fijado, `las` 0.11 sin `laz-parallel` en el núcleo, `laz` 0.13,
wasm-bindgen 0.2.129, Node 22 (`node:test` y módulos wasm sin imports, como `tools/format_parity`),
Playwright 1.64 vía scripts Node (como los drivers de S8/S9).

**Spec:** [conversor-web-paralelo.md](conversor-web-paralelo.md).

## Restricciones globales

- `scripts/check.sh` en verde (todos los pasos) al cerrar cada tarea.
- No cambiar la fecha del nightly ni añadir `#![feature]`.
- `FormatVersion::V<n>` solo en `crates/viewer_format`.
- R1: los SHA de salida del CLI no cambian; referencia [M0](../records/benchmarks/WEB_CONVERTER_BASELINE_2026-10-29.md) (LAS pequeño `413858a9…`, Belvedere `6ca0f86d…`, Belvedere divisiones 8 `fb90a490…`, Majadas `74226286…`).
- R7: mediana pareada de `real_dataset_bench convert` en Belvedere (4 workers, perfil `perf`), 7–10 pares A/B alternados en máquina en reposo, registrando `/proc/pressure`, governor y perfil de energía; dentro de ±3% de la rama base medida en la misma sesión.
- El núcleo no usa `std::fs`, `std::thread`, `rayon`, `Instant::now`, `SystemTime::now`, `std::process` ni `laz-parallel`; un lint `disallowed-methods` (denegado) y `cargo check --target wasm32-unknown-unknown` en `check.sh` lo garantizan. Sin `dyn` ni asignación por punto en el bucle caliente (criterio de parada de M2).
- Registros nuevos fechados con el día de ejecución (`dd-mm-aaaa` en el texto, `<aaaa-mm-dd>` ISO en el nombre), en español; no editar registros ya mergeados; no trasladar cifras entre revisiones, perfiles ni equipos.
- Commits convencionales en inglés con la línea `Co-Authored-By` del modelo que los escribe; sin push.
- No enviar avisos upstream (backlog, prioridad 9 del README).

## Review Focus

1. **Fronteras de rango LAZ en chunks atípicos**: chunks diminutos, último chunk parcial y tamaños variables `[7, 13, 5, 11, 50000, 3]` → mismos puntos que la lectura secuencial (Tarea 5, fixture multi-chunk variable).
2. **Empates del punto más cercano entre rangos**: un punto duplicado a ambos lados de una frontera de rango debe elegir el del rango anterior, como la serie (Tarea 5).
3. **Hijos vacíos en algunos rangos**: un hijo con puntos solo en el último rango, o en ninguno, no crea spool ni segmento vacío y conserva el orden de ids (Tarea 5).
4. **Errores en un rango posterior**: LAS truncado, punto fuera de la cabecera o pánico del decodificador en el rango k > 0 → mismo `Display` e índice global que la serie; si fallan dos rangos, gana el menor `PointRange::start` (Tarea 5).
5. **Determinismo de todo el pipeline en wasm32**, no solo del formato: la pareja completa convertida en wasm32 bajo Node tiene el SHA del CLI (Tarea 7).

## Enmiendas del pre-mortem (29-10-2026)

Ya aplicadas en el texto de las tareas; se listan para trazabilidad.

| # | Enmienda | Tareas |
| --- | --- | --- |
| B1 | El LAS pequeño (111 190 puntos < 262 144 por bloque) deja la raíz como hoja: la paridad de M3a también usa 4096 puntos por bloque × divisiones {2, 8}, autzen y la fixture multi-chunk variable, con más de un nodo comprobado; SHA de referencia del CLI serie registrados antes de la Tarea 5 | 1, 7 |
| B2 | El bucle host serie (`inspect_source`, metadatos, plan y fase de bloques serie sobre `Write + Seek`) va al núcleo en la Tarea 4, antes de que las Tareas 5 y 7 se separen; la fase de bloques nativa y la serie comparten la función por bloque | 3, 4, 7 |
| B3 | El `PointSource` por rangos de chunks vive en el núcleo (genérico en `Read + Seek`); las llamadas a `fill` se cortan en múltiplos globales de `input_batch_point_limit`; con error en el rango k se cancelan solo los rangos > k y gana el de menor inicio; los errores de `laz` dan el mismo `Display` que la serie | 3, 5 |
| B4 | Las firmas de `PointSource` y de rangos son provisionales hasta el paso 1 de S2 | 3 |
| B5 | Tareas en paralelo con código: worktree y `CARGO_TARGET_DIR` propios; `git add` solo de rutas propias; el miembro nuevo del workspace (Tarea 1) se integra antes de la Tarea 4 | todas |
| B6 | `#[inline]` en las funciones calientes que pasan a ser entre crates y puerta de instrucciones (callgrind) + R7 corto antes de la Tarea 5 | 4 |
| B7 | Mediciones de tiempo (S2 paso 2, R7) sin otros agentes activos, anotado en el registro | 1, 6 |
| B8 | S10 reserva el peor caso en la admisión, modela 513 handles por trabajo en divisiones 8 y la raíz por rangos (W × d³), y añade P3: solo el coordinador abre handles y vuelca | 2 |
| B9 | Cancelación en M3a: API por pasos o `terminate()` más limpieza de huérfanos, fijada en la Tarea 3 y probada en la Tarea 7 | 3, 7 |
| B10 | El lint añade `disallowed-types` (`File`, `OpenOptions`, `Instant`, `SystemTime`, tipos de hilo); el núcleo no depende de rayon, ctrlc ni `laz-parallel` | 4 |
| B11 | La estimación de memoria suma W × d³ búferes de hijo solo con rangos activos; los tests pasan también W = 1 por la unión de segmentos | 5 |

**Criterios de parada añadidos:** instrucciones del LAS pequeño en divisiones 8 > +2% frente a
`fb9a3b7` tras B6 → parar antes de la Tarea 5 y rediseñar la frontera del bucle caliente (o revertir);
S2 necesita parchear `laz` → decide el propietario antes de fijar firmas; SHA por rangos (W > 1)
distinto tras dos días → rangos desactivados y M2 se cierra sin ellos; S10 con > 1 s de handles por
trabajo volcado en Firefox en todas las políticas, o > 664 handles simultáneos → replantear los
spools con el propietario antes de M3b.

---

### Tarea 1: spike S2 — rangos de chunks LAZ y decodificador en wasm32

**Pregunta:** ¿se puede decodificar un rango de chunks LAZ arrancando un decodificador nuevo en el
offset de bytes del chunk (tabla ya validada, ADR 0012) con los mismos puntos que la lectura
secuencial, sin `Reader::seek` (falla con chunks variables)? ¿Cuánto más lento es el decodificador
secuencial en wasm32 que en nativo?

**Archivos:**
- Crear: `tools/laz_chunk_probe/` (miembro del workspace: bin nativo y `cdylib` wasm32 sin imports con runner Node, como `tools/format_parity`)
- Crear: `docs/records/benchmarks/LAZ_CHUNK_RANGES_2026-10-29.md` y sus datos en `docs/records/benchmarks/data/`

**Interfaces:**
- Produce: un prototipo `decode_chunk_range(bytes_or_reader, header, chunk_table, chunks: Range<usize>, out: &mut las::PointData) -> Result<u64, _>` y la receta exacta de la API de `laz` 0.13 que usa, para que la Tarea 5 lo convierta en `PointSource`.

- [x] **Paso 1:** prototipo nativo; test que compara, para cada partición de los chunks en W = 1..8 rangos, la concatenación de puntos decodificados por rango con la lectura secuencial completa: autzen COPC, Belvedere COPC (ignorado si falta el fichero) y una fixture sintética con chunks `[7, 13, 5, 11, 50000, 3]`.
- [x] **Paso 1b (B1):** SHA de referencia del CLI serie (base de la etapa) para la fixture multi-chunk variable con 4096 puntos por bloque, divisiones 2 y 8; fixture versionada si ocupa < 2 MB o generador con semilla fija.
- [x] **Paso 2 (B7, sin otros agentes activos):** coste de arranque por chunk y puntos/s nativo frente a wasm32 bajo Node, sobre los primeros ≥ 64 chunks de Belvedere, 5 rondas alternadas; mediana y rango.
- [x] **Paso 3:** registro con método, cifras, límites y decisión: receta para la Tarea 5. Si `laz` 0.13 no lo permite sin parche, el registro compara parche vendorizado (como `vendor/lz4_flex`) frente a otra ruta y **para** antes de la Tarea 5. Si wasm32 > 4 × nativo, se anota como riesgo del criterio de parada de M3b.
- [x] **Paso 4:** commit.

### Tarea 2: spike S10 — pico de RAM del híbrido y apertura de handles

**Pregunta:** con spools como segmentos `ArrayBuffer` en RAM volcados a un fichero OPFS por spool,
¿qué pico de RAM resulta con el presupuesto de la spec, y cuándo se abren los handles de volcado?
El trabajo de partición raíz escribe todo el dataset, así que «volcar solo entre trabajos» no basta.
Políticas a comparar:

| Política | Regla |
| --- | --- |
| P0 | Solo RAM (cota de RAM, Chrome `file://`) |
| P1 | Admisión por entrada: un trabajo cuyo peor caso (`input_spool_bytes` + LOD) no cabe en el presupuesto libre recibe del host, antes de empezar, handles abiertos para sus `divisions³ + 1` salidas y escribe en ellos; si cabe, solo RAM. Entre trabajos, el host vuelca primero los spools retenidos que se consumen más tarde (LOD, hojas). Se modela el coste de 513 handles por trabajo en divisiones 8, la raíz por rangos (W × d³) y los handles que acaban vacíos |
| P2 | Handles abiertos para todo trabajo (coste de referencia de S9) |
| P3 | Solo el coordinador toca OPFS: el worker llena segmentos de tamaño fijo y los transfiere; el coordinador lleva el libro de RAM y vuelca spools enteros de forma asíncrona entre mensajes; contrapresión tras K segmentos sin confirmar |

**Archivos:**
- Crear: `crates/viewer_convert/benchmarks/model_hybrid_spools.py` y `test_model_hybrid_spools.py` (reutiliza los eventos y el simulador de `model_node_job_arena.py`)
- Crear: `docs/records/benchmarks/data/hybrid-spool-ram-2026-10-29.{html,js,-driver.mjs,jsonl,json}`
- Crear: `docs/records/benchmarks/HYBRID_SPOOL_RAM_2026-10-29.md`

- [x] **Paso 1:** test del modelo con un árbol manual de 5 nodos: pico, bytes volcados, ficheros y handles por política coinciden con valores calculados a mano.
- [x] **Paso 2:** modelo sobre los eventos de Belvedere (2 y 8 divisiones) y Majadas: pico de RAM de spools, bytes y ficheros volcados, handles abiertos y su coste con las latencias medidas de S9, para presupuestos de spools de 0,5/1/2/3 GiB y N = 1, 4, 8.
- [x] **Paso 3:** navegador (Chrome y Firefox, HTTP, Playwright): PSS frente a bytes retenidos para segmentos de 32 KiB y 1 MiB hasta 2 GiB en un worker, y coste de transferirlos al host; muestreo con `examples/web-host/sample-process-memory.py`.
- [x] **Paso 4:** registro con decisión: política, tamaño de segmento, presupuesto inicial y fórmula del pico para M3b/M6. Commit.

### Tarea 3: M2 paso 1 — firmas del núcleo y de la partición por rangos (arquitecto)

Un arquitecto (Opus) fija, a partir del código de M1, lo que las Tareas 4, 5 y 7 implementan, y lo
anota en una sección «Firmas fijadas» de este documento antes de seguir:

- Lista de módulos y tipos que pasan al núcleo y los que quedan en el shell; visibilidad (`pub` del núcleo, reexportes en `viewer_convert` para no tocar llamadores).
- `ConvertError` del núcleo frente al del shell: variantes con `PathBuf`/`Checkpoint`/`io::Error`, sin cambiar ningún `Display` ni los `match` de los tests.
- Si `PointSource::header() -> &las::Header` se mantiene (dependencia `las` sin `laz-parallel` en el núcleo).
- `SpatialBuilder` genérico en `Clock`; `ConvertOptions::default` sin `available_parallelism` en el núcleo.
- Partición por rangos: `RangePartition` (segmentos por hijo, `SampleGrid` y límites fusionables), `SampleGrid::merge` (sumas; más cercano con `<` estricto y el rango anterior primero), unión de segmentos (`join_segments` genérica sobre `SpoolStore`, borrando cada segmento tras copiarlo), orden de ids, errores (A8) y casos raíz con puntos iguales y no divisible.
- Cómo se activa en nativo (`ConvertOptions` y `real_dataset_bench`, sin cambiar el defecto) para que R7 no se vea afectado; memoria de W × d³ búferes solo con rangos (B11).
- `PointSource` por rangos de chunks en el núcleo, genérico en `Read + Seek`, con cortes de `fill` en múltiplos globales y la regla de errores de B3; provisional hasta S2 (B4).
- Bucle host serie del núcleo (B2): `inspect_source`, metadatos, plan y fase de bloques serie sobre `Write + Seek`, con la función por bloque compartida con la fase nativa paralela.
- Cancelación en M3a (B9): API por pasos o `terminate()` más limpieza de huérfanos.
- Lista exacta de `disallowed-methods` y `disallowed-types` (B10) y funciones que llevan `#[inline]` (B6).

- [x] **Paso 1:** firmas anotadas aquí; revisión del coordinador antes de la Tarea 4 (aprobadas el 29-10-2026 con los tres desvíos señalados).

#### Firmas fijadas en el paso 1 de M2 (29-10-2026)

Líneas citadas de `1cde1a9`; S2 en `37cc81c`. Integra B1–B11 y la receta de S2, con tres desvíos
motivados: memoria de rangos (B11), redondeo del índice de pánico (B3) y defecto de rangos = 0.

**1. Reparto.** `crates/viewer_convert_core` depende de `las` y `laz` 0.13 sin `parallel`,
`viewer_core` (`laz`), `viewer_format` y `thiserror`; sin `rayon`, `ctrlc` ni `laz-parallel`.

```text
src/lib.rs          pub use de la API estable; módulos internos `#[doc(hidden)] pub mod`
src/error.rs        ConvertError + CheckpointError (copiados tal cual de error.rs y checkpoint.rs:60)
src/options.rs      ConvertOptions, constantes, input_batch_point_limit
src/report.rs       ConversionReport/Progress/Stage/WorkerStats, emit_progress, aggregate_geometry_codec_diagnostics
src/diagnostics.rs  solo structs de datos (DiagnosticTimer y unix_ns se quedan en el shell)
src/source.rs       SourceInfo, inspect_source/inspect_header, source_schema, next_descriptor, omitted_dimensions,
                    contain_decoder_panic(_at), fill_points, seek_points(_at), open_las_reader_from,
                    check_laz_chunk_table (de las_source.rs)
src/limits.rs       visual_scale_for_bounds, check_reader_attribute_ranges (+ test) (de publish.rs)
src/pipeline/{clock, point_source, laz_chunks, spool_store, node_job, executor}.rs
src/spatial.rs      spatial.rs entero salvo Workspace (:297-352) y ready_queue
src/blocks.rs       ConversionPlan, ruta por bloque compartida, convert_serial, validate_output_serial
```

- **Shell nativo:** `main.rs`, `convert_file*` de `lib.rs`, `checkpoint.rs`, `ept*.rs`, `fs_util.rs`, `publish.rs` (sin `limits`), `las_source.rs` (`open_las_reader(path)`, `convert_file_inner`, `plan_sample_blocks`, `stream_blocks`, `process_batches`), y nuevos `workspace.rs`, `file_spool_store.rs`, `executor.rs` (`ThreadedExecutor` con el antiguo `ready_queue.rs`); `SystemClock` y `DiagnosticTimer`.
- **Visibilidad:** lo `pub(crate)` pasa a `pub` dentro de módulos `#[doc(hidden)]`. API estable: lo que `viewer_convert` ya reexporta (`ConvertError`, `CheckpointError`, `ConvertOptions`, informes, diagnósticos) más los traits, `MemSpoolStore`, `NoClock`, `LasFactory`, `MemBytes`, `convert_serial` y `validate_output_serial`.
- **Reexportes:** `viewer_convert/src/lib.rs` hace `pub use viewer_convert_core::{…}`, así `main.rs`, `real_dataset_bench`, `dump_blocks` y `tests/` no cambian; cada módulo nativo que pierde ítems los reexporta con `pub(crate) use` bajo su ruta antigua (`crate::las_source::SourceInfo`, `crate::spatial::*`, `crate::pipeline::*`, `crate::options::*`).
- **Tests:** los que usan ficheros o `FileSpoolStore` se quedan en el shell; los demás se mueven con su módulo y cambian el fichero temporal por `las::Writer` sobre `Cursor`, sin cambiar su lógica.

**2. Errores.** `ConvertError` y `CheckpointError` viven en el núcleo, copiados tal cual (`PathBuf`,
`io::Error` y `las::Error` compilan en wasm32; las variantes solo nativas llevan el comentario
«solo shell nativo»). `Display` y `match` quedan idénticos por construcción. Descartado: un error
del núcleo envuelto en uno nativo (rompe decenas de `matches!` y añade `From` en cada `?`). Variante
nueva: `InvalidRootRanges(usize)` («root partition ranges must be between 0 and 8, got {0}»).

**3. `las` en el núcleo: se mantiene** `PointSource::header() -> &las::Header`; abstraerlo exigiría
reescribir el bucle caliente (riesgo R7) sin requisito. `las` arrastra `chrono` con sus features por
defecto y no permite quitarlo: en wasm32 trae imports `__wbindgen_*`; el runner Node usa stubs que
lanzan (como `tools/laz_chunk_probe/run-wasm.mjs`), de modo que una llamada al reloj falla visible.

**4. Builder, reloj, ejecutores y reapertura sin `Path`.**

```rust
// pipeline/clock.rs
pub trait Clock { type Instant: Copy; fn now(&self) -> Self::Instant; fn elapsed_ns(&self, since: Self::Instant) -> u64; }
pub struct NoClock; // Instant = (), 0 ns: tests y wasm sin imports (Tarea 7)
// pipeline/point_source.rs (PointRange, PointSource, LasPointSource: firmas de M1, ahora pub)
pub trait ByteSource { type Reader: Read + Seek + Send + Sync + 'static; fn open(&self) -> Result<Self::Reader, ConvertError>; fn len(&self) -> u64; }
pub struct MemBytes(pub Arc<[u8]>); // Reader = Cursor<Arc<[u8]>>
pub trait SourceFactory {
    type Source: PointSource;
    /// Contiguous ranges in order covering 0..point_count, at most `parts`.
    fn split(&self, parts: usize, batch: u64) -> Result<Vec<PointRange>, ConvertError>;
    /// A fresh source at `range.start`, preflight included (replaces `open_las_reader(input_path)`, spatial.rs:932).
    fn open(&self, range: PointRange) -> Result<Self::Source, ConvertError>;
}
pub struct LasFactory<B: ByteSource> { bytes: B, chunks: Option<Arc<ChunkIndex>> }
pub enum RangeSource<R: Read + Seek> { Las(LasPointSource<las::Reader>), Laz(LazChunkPointSource<R>) }
impl<B: ByteSource> SourceFactory for LasFactory<B> { type Source = RangeSource<B::Reader>; /* … */ }
pub fn open_las_reader_from<R: Read + Seek + Send + Sync + 'static>(input: R, len: u64) -> Result<las::Reader, ConvertError>;
// pipeline/executor.rs
pub trait BatchPlanner<S: PlanStore, C: Clock> {
    fn plan_batch(&self, builder: &mut SpatialBuilder<'_, S, C, Self>, tasks: Vec<ChildOutput>, memory_budget: u64)
        -> Result<Vec<(u16, BranchOutput<C::Instant>)>, ConvertError> where Self: Sized;
}
pub trait RangeRunner<S: PlanStore, C: Clock, P: PointSource> {
    /// Error in range k: cancel ranges > k, let ranges < k finish. Results in range order.
    fn run_ranges(&self, job: &RangeJob<'_>, sources: Vec<(P, SpoolId)>, store: &S, clock: &C,
        progress: &mut dyn FnMut(usize, JobProgress) -> Result<(), ConvertError>) -> Vec<Result<RangePartition, ConvertError>>;
}
pub struct SerialExecutor; // both traits, in order on the calling thread
// spatial.rs
pub struct SpatialBuilder<'a, S, C, X> { store: &'a S, clock: &'a C, executor: &'a X, /* resto igual */ }
pub fn build_spatial_plan<S: PlanStore, C: Clock, X: BatchPlanner<S, C>>(source: &SourceInfo, options: ConvertOptions,
    store: &S, clock: &C, executor: &X, on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    collect_spool_diagnostics: bool, source_spool_bytes_written: u64) -> Result<SpatialPlan, ConvertError>;
pub fn build_spatial_plan_from_source<S, C, F, X>(first: &mut F::Source, factory: &F, source: &SourceInfo,
    options: ConvertOptions, store: &S, clock: &C, executor: &X, on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    collect_spool_diagnostics: bool) -> Result<SpatialPlan, ConvertError>
where S: PlanStore, C: Clock, F: SourceFactory, X: BatchPlanner<S, C> + RangeRunner<S, C, F::Source>;
```

- **Reloj:** `OpenNodeEvent.started` pasa a `C::Instant`; `:580`, `:717-723`, `:747` y `:954` usan `self.clock`.
- **Ruta nativa por defecto:** `convert_file_inner` llama a `build_spatial_plan_from_source(&mut RangeSource::Las(LasPointSource::full(reader)), &LasFactory::new(FileBytes{path,len}), …, &SystemClock, &ThreadedExecutor, …)`; `open_las_reader(path)` = `File::open` + `open_las_reader_from(file, len)`: mismas operaciones que hoy.
- **Cola de listos:** `impl<S: PlanStore> BatchPlanner<S, SystemClock> for ThreadedExecutor`, cuerpo igual salvo accesores. El builder expone como `#[doc(hidden)] pub` `node_job`, `job_context`, `emit_job_progress`, `with_branch`, `commit_job`, `finish_node_event`, `store()`, `layout()`, `collects_diagnostics()`, y `BranchOutput`/`OpenNodeEvent`.
- **`SerialExecutor::plan_batch`:** por clave, `with_branch(fresh, budget, |b| b.build_node(job, None))` y ramas para `append_branch`; reproduce la semántica de W de la cola (presupuesto por lote, `TooManyBlocks` local, error de menor ruta), así la web planifica con el mismo `spatial_workers` que el CLI (R2) y sirve de oráculo de la cola.
- **`ConvertOptions::default`** se queda en el núcleo con `default_spatial_workers()` bajo `#[expect(clippy::disallowed_methods, reason = "native default; Err on wasm32 → 1")]`: única excepción a «sin `std::thread`». Descartados: sacar las opciones del núcleo (la web duplicaría la validación de R2) y un 4 fijo (rompe `lib_tests.rs:868`).

**5. Partición raíz por rangos (Tarea 5).**

```rust
pub struct RangeJob<'a> { pub ctx: &'a JobContext, pub info: &'a SourceInfo, pub bounds: SpatialBounds, pub batch: u64 }
pub struct RangeSegment { pub key: u16, pub spool: SpoolId, pub point_count: u64 }
pub struct RangePartition { pub range: PointRange, pub segments: Vec<RangeSegment>, pub samples: SampleGrid,
    pub source_bounds: [[f64; 2]; 3], pub metrics: Option<JobMetrics> }
pub fn run_root_partition_range<P: PointSource, S: SpoolStore, C: Clock>(job: &RangeJob<'_>, first_segment: SpoolId,
    source: &mut P, store: &S, clock: &C, progress: &mut impl FnMut(JobProgress) -> Result<(), ConvertError>)
    -> Result<RangePartition, ConvertError>;
pub fn merge_root_ranges<S: SpoolStore>(ctx: &JobContext, bounds: SpatialBounds, first_output: SpoolId,
    results: Vec<Result<RangePartition, ConvertError>>, store: &S) -> Result<RootPartition, ConvertError>;
/// Copies closed `segments` (range order) into the new spool `target` in 64 KiB chunks and
/// deletes each segment right after copying it. Never called with an empty slice.
pub fn join_segments<S: SpoolStore>(store: &S, target: SpoolId, segments: &[SpoolId]) -> Result<u64, ConvertError>;
impl SampleAccumulator { pub fn merge(&mut self, later: &Self); }
impl SampleGrid { pub fn merge(&mut self, later: SampleGrid); }
```

- **Bucle compartido:** `run_root_partition` (node_job.rs:274) y la versión por rangos comparten `scan_root_range` (el bucle actual con `processed` desde `range.start`); cada `fill` pide `min(batch - processed % batch, range.end - processed)`, idéntico a hoy en serie. `run_root_partition` conserva su `assert` y el cierre sin vaciar de «todos iguales»; la versión por rangos siempre vacía y cierra sus segmentos y no decide «todos iguales».
- **`merge`:** suma `count` y sumas `u128`; el más cercano del posterior gana solo con `<` estricto; en empate se queda el del rango anterior (misma regla que `add`, :1416-1418).
- **`merge_root_ranges`:** gana el primer error real en orden de rango (si solo hay cancelaciones, la primera); no limpia (`discard_spools` cubre los segmentos, que salen del mismo contador). Límites fusionados iguales → `all_points_equal` con `release` = todos los segmentos y el host reabre con `factory.open(full)`. Si no, `join_segments` hacia `first_output + k` por cada clave con segmentos y `ChildOutput` con `cell_bounds(k)`; un hijo sin segmentos no crea nada. Raíz no divisible: sin rangos, `spool_source` desde `first`.
- **Ids:** tras `first_output = reserve_node_spools()`, el builder reserva `n × d³` ids; el segmento (r, k) es `base + r·d³ + k`.
- **Memoria (desvío de B11):** cada rango extra cuesta `per_range = point_limit·192 + d³·32 KiB + 512·256 + 1 MiB` (el lote de decodificación pesa más que los búferes de hijo). Antes de dividir, `n = min(root_ranges, 1 + (budget − estimate_memory(point_limit, 0, 0, d)) / per_range)`, como `memory_limited_workers`; `estimated_peak_memory_bytes = max(fórmula actual, raíz + (n−1)·per_range)` solo con rangos, y nunca dispara `MemoryBudgetExceeded`. La copia de `join_segments` usa 64 KiB tras terminar los rangos; en spools vivos el pico sube en el segmento mayor (el libro la cuenta como bytes escritos, solo diagnóstico).
- **Activación:** `ConvertOptions::with_root_partition_ranges(n)` (`#[doc(hidden)]`, 0..=8); **0 = ruta de hoy** (defecto); con ≥ 1 pasa por `merge` + `join` aunque haya un rango. `real_dataset_bench convert … [FORMAT_VERSION [ROOT_RANGES]]`; sin flag de CLI; no aplica con checkpoint.
- **Ejecución:** `ThreadedExecutor` implementa `RangeRunner<S, SystemClock, RangeSource<File>>` con `thread::scope`, un hilo por rango y un canal `(r, JobProgress)`; `abort_above: AtomicUsize` guarda el menor rango fallido y los mayores devuelven `Cancelled` en su siguiente `fill`; una cancelación del usuario aborta todo. `SerialExecutor` ejecuta los rangos en orden (Review Focus 1–4 con `MemSpoolStore`, sin hilos).
- **Rangos:** LAS, `PointRange::split` (inicios múltiplos de `batch`); LAZ, grupos contiguos de chunks enteros cortados por los puntos acumulados más cercanos a `k·total/n`.

```rust
pub struct ChunkIndex { pub vlr: laz::LazVlr, pub table: laz::laszip::ChunkTable, pub data_start: u64 } // offset_to_point_data + 8
impl ChunkIndex { pub fn read<R: Read + Seek>(src: &mut R, header: &las::Header, point_data_offset: u64) -> Result<Self, ConvertError>; }
pub struct LazChunkPointSource<R: Read + Seek> { decoder: LasZipDecompressor<'static, ChunkRangeStream<R>>, header: las::Header, range: PointRange, next: u64, batch: u64 }
impl<R: Read + Seek + Send + 'static> LazChunkPointSource<R> {
    pub fn new(input: R, header: las::Header, index: &ChunkIndex, chunks: Range<usize>, batch: u64) -> Result<Self, ConvertError>;
}
```

- **Streaming:** `ChunkRangeStream<R>` es un `Read + Seek` virtual `[i64 LE = 8 + bytes][chunks a..b leídos de input][subtabla de ChunkTable::write_to]`; solo prefijo y subtabla (≈ 16 B por chunk) en RAM (el `Vec` del probe retendría cientos de MiB por rango en wasm). Test frente al `RangeDecoder` del probe en las 32 composiciones de `[7,13,5,11,50000,3]`.
- **Decodificación:** `fill` escribe en `data.resize_for(n)` con `decompress_many`, la ruta de `las`; `new` rechaza `items_size != point_format.len()`.
- **Errores:** `io::Error` → `ConvertError::Las(las::Error::Io(e))` y `LasZipError` → `ConvertError::Las(las::Error::LasZipError(e))`, como el lector `las`; test con un LAZ truncado en el rango k > 0. `LasDecoderPanicked.point_index = Some(next − next % batch)`: un rango que empieza en `s` dentro del lote `[kb, (k+1)b)` informa `kb`, como la serie (desvío de B3).
- **Pánicos:** el `message` puede diferir entre `laz-parallel` (serie nativa) y el decodificador secuencial; el test de Review Focus 4 compara con la serie forzada a secuencial o documenta la diferencia.

**6. API serie del núcleo (B2; paso 1b de la Tarea 4).**

```rust
pub const WRITER_MAX_UNCOMPRESSED_BLOCK_BYTES: usize = 64 * 1024 * 1024; // hoy literal en las_source.rs:375
pub fn output_writer<W: Write + Seek>(options: ConvertOptions, metadata: &FormatMetadata, geometry: W, attributes: W)
    -> Result<(FormatWriter<W>, FormatBlockEncoder), ConvertError>;
#[inline] pub fn prepare_planned_block<E: BlockEncoder, S: SpoolStore>(encoder: &E, block: &PlannedBlock, store: &S,
    frame: CoordinateFrame) -> Result<E::PreparedBlock, ConvertError>; // + _with_diagnostics
#[inline] pub fn write_prepared<W: PreparedOctreeWriter<Error = FormatError> + ?Sized, S: SpoolStore>(writer: &mut W,
    block: &PlannedBlock, prepared: W::PreparedBlock, store: &S) -> Result<(), ConvertError>; // + release_spool_after_write
pub struct SerialOutput<W> { pub geometry: W, pub attributes: W, pub plan: PlanSummary } // point/block counts, ids, estimate, omitted
pub fn convert_serial<B: ByteSource, S: PlanStore, C: Clock, W: Write + Seek>(input: &LasFactory<B>, store: &S, clock: &C,
    options: ConvertOptions, geometry: W, attributes: W, on_progress: &mut dyn FnMut(ConversionProgress) -> bool)
    -> Result<SerialOutput<W>, ConvertError>;
pub fn validate_output_serial<R: Read + Seek>(geometry: R, attributes: R, plan: &PlanSummary,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool) -> Result<ConversionWorkerStats, ConvertError>;
```

- `convert_serial`: `InspectingInput` (`open(full)`, `inspect_header`) → `PlanningMetadata` → `build_spatial_plan_from_source` con `SerialExecutor` → `ConversionPlan::from_spatial` → `output_writer` → por bloque `prepare_planned_block` + `write_prepared` → `processed == point_count` y `finish_pair`.
- `stream_blocks` llama a las mismas funciones dentro de `par_iter`: bytes iguales por construcción. `bytes_written` del progreso de bloques vale 0 en el núcleo (lo rellena el host); publicar es del host.
- `validate_output_serial` relee cada bloque con `FormatReader<R>`; la validación paralela (R6) queda para M5.
- **Cancelación en M3a (B9): `Worker.terminate()` + limpieza de huérfanos OPFS.** Sin `SharedArrayBuffer` el worker no ve mensajes durante una llamada wasm síncrona; una API por nodo exigiría pila explícita en `build_node` (tamaño M5). El host borra los temporales tras `terminate` y los huérfanos al arrancar. Test Playwright: cancelar y reconvertir al mismo nombre.

**7. Lint y `check.sh` (B10).** `Cargo.toml` del núcleo: `[lints.clippy] disallowed_methods = "deny"`,
`disallowed_types = "deny"`. Un lint no ve features: `laz-parallel`/`rayon`/`ctrlc` los vigila `cargo tree`.

```toml
# crates/viewer_convert_core/clippy.toml
disallowed-methods = [
  "std::fs::read", "std::fs::read_to_string", "std::fs::write", "std::fs::copy", "std::fs::rename",
  "std::fs::hard_link", "std::fs::metadata", "std::fs::symlink_metadata", "std::fs::read_dir",
  "std::fs::canonicalize", "std::fs::create_dir", "std::fs::create_dir_all", "std::fs::remove_file",
  "std::fs::remove_dir", "std::fs::remove_dir_all", "std::fs::File::open", "std::fs::File::create",
  "std::fs::OpenOptions::new", "std::thread::spawn", "std::thread::scope", "std::thread::sleep",
  "std::thread::Builder::new", "std::thread::available_parallelism", "std::time::Instant::now",
  "std::time::Instant::elapsed", "std::time::SystemTime::now", "std::time::SystemTime::elapsed",
  "std::process::id", "std::process::exit", "std::process::abort", "std::process::Command::new",
  "std::env::temp_dir", "std::env::current_dir", "std::sync::mpsc::channel", "std::sync::mpsc::sync_channel",
]
disallowed-types = [
  "std::fs::File", "std::fs::OpenOptions", "std::time::Instant", "std::time::SystemTime",
  "std::thread::JoinHandle", "std::thread::Builder", "std::thread::Scope", "std::sync::Condvar",
]
```

```bash
# scripts/check.sh, tras «clippy wasm32»
step "core wasm32 check" cargo check -p viewer_convert_core --target wasm32-unknown-unknown $CARGO_MODE
step "core wasm32 clippy" cargo clippy -p viewer_convert_core --target wasm32-unknown-unknown $CARGO_MODE -- -D warnings
step "core has no native-only deps" bash -c '! cargo tree -p viewer_convert_core -e features --target wasm32-unknown-unknown $0 | grep -E "laz-parallel|rayon|ctrlc"' "$CARGO_MODE"
```

Alias en `.cargo/config.toml`: `check-core-web = "check -p viewer_convert_core --target wasm32-unknown-unknown"`.

**8. Riesgos anotados.** `PlanStore: Sync` choca con un store OPFS que guarde `JsValue` (decidir en
M3b: handles enteros o envoltorio). En modo por rangos el libro de spools, los eventos y los bytes
de hijos de la raíz difieren de la serie (solo diagnóstico, fuera de R1); la unión reescribe el
dataset en la raíz: medirla aunque no entre en R7. `thiserror` sin `std::backtrace` en wasm32:
comprobarlo en el paso 2 de la Tarea 4.

### Tarea 4: M2 paso 2 — crate `viewer_convert_core`

**Archivos:** los fija la Tarea 3. Crear `crates/viewer_convert_core/` (`Cargo.toml` con
`[lints.clippy] disallowed_methods = "deny"`, `clippy.toml`); modificar `crates/viewer_convert/`,
`Cargo.toml` del workspace, `scripts/check.sh`, `.cargo/config.toml` (`check-web`).

Cada paso cierra con tests en verde y los SHA de M0 (LAS pequeño, autzen, Belvedere por defecto y divisiones 8).

- [ ] **Paso 0 (costuras dentro de `viewer_convert`, aún un crate):** `SpatialBuilder<S, C, X>`, `BatchPlanner` (la cola como `ThreadedExecutor`) y `SerialExecutor`; `ByteSource`/`LasFactory` y `build_spatial_plan_from_source` en lugar de `_from_reader`; reparto de `publish`/`las_source`/`diagnostics` en partes puras y nativas; `#[inline]` en `decode_point`, `write_point`, `read_records`, `read_u16`, `read_f64`, `sample_index`, `voxel_center`, `cube_side`, `outside_header_bounds`, `SpoolLayout::{position, record_bytes}`, `SpoolPoint::display_color_component`, `SampleGrid::add` y `average` (B6).
- [ ] **Paso 1:** traslado mecánico al crate nuevo con los reexportes; los tests con ficheros se quedan en el shell.
- [ ] **Paso 1b (B2):** `blocks.rs` y `stream_blocks` sobre la ruta por bloque compartida. Test nativo: `convert_serial(MemSpoolStore, MemBytes)` da el SHA del CLI en el LAS pequeño y en `synthetic_variable_chunks.laz` con 4096 puntos por bloque (SHA de la Tarea 1).
- [ ] **Paso 2:** `check.sh` añade `cargo check -p viewer_convert_core --target wasm32-unknown-unknown` y clippy con el lint (B10); un uso de prueba de `std::fs::read` en el núcleo debe hacerlo fallar (comprobarlo en el worktree propio y revertirlo).
- [ ] **Paso 3:** tests existentes en verde; SHA de M0 en LAS pequeño, autzen y Belvedere (defecto y divisiones 8) iguales.
- [ ] **Paso 3b (B6, sin otros agentes activos):** instrucciones (callgrind) del LAS pequeño, divisiones 8, 2048 puntos por bloque, ≤ +2% frente a `fb9a3b7`, y R7 de 3 rondas en divisiones 8. Criterio de parada arriba.
- [ ] **Paso 4:** commit.

### Tarea 5: M2 paso 3 — partición raíz por rangos de chunks en nativo

**Archivos:** los fija la Tarea 3; en el núcleo, la partición, la unión de segmentos y el
`PointSource` por rangos de chunks LAZ (receta de la Tarea 1, B3); en el shell, la ejecución de W
rangos con `thread::scope`.

- [ ] **Paso 1:** tests que fallan: matriz W ∈ {1, …, 8} × LAS/LAZ multi-chunk variable × divisiones {2, 4, 8} igual byte a byte a la serie, con W = 1 también por la unión de segmentos (B11); Review Focus 1–4 (fronteras, empates, hijos vacíos, errores en el rango k > 0 con el mismo `Display` e índice global, B3); raíz con todos los puntos iguales y no divisible con W > 1.
- [ ] **Paso 2:** implementar hasta verde; `MemSpoolStore` no deja ids vivos tras la fase de bloques.
- [ ] **Paso 3:** SHA de Belvedere (defecto y divisiones 8) con W = 1, 2, 4, 8 rangos iguales a M0. Criterio de parada (spec): el núcleo necesita `dyn` o asignación por punto.
- [ ] **Paso 4:** commit.

### Tarea 6: M2 paso 4 — puerta R7, registro y estado

- [ ] **Paso 1:** R7 frente a `fb9a3b7` (base de la etapa 3) en la misma sesión; registro `docs/records/benchmarks/M2_R7_<aaaa-mm-dd>.md` con SHA por rangos de la Tarea 5. Criterio de parada: regresión > 3% tras dos intentos → revertir la extracción.
- [ ] **Paso 2:** actualizar estado en README (fila 7), este plan, la spec (líneas citadas que M1 dejó obsoletas), el inventario (crate nuevo) y `CONTRIBUTING.md` (paso nuevo de `check.sh`). Commit.

### Tarea 7: M3a — conversión web con un worker sobre el núcleo

**Archivos:**
- Modificar: `crates/viewer_wasm/` (un conversor wasm-bindgen nuevo sobre `convert_serial` y `validate_output_serial`: `ByteSource` sobre `Blob` con `BrowserBlobReader`, `Clock` con `performance.now()`, `MemSpoolStore`, salida a handles OPFS como `OpfsWriter`; cancelación por `terminate()` y limpieza de huérfanos, firmas §6), `examples/web-host/converter-worker.js` y `converter-api.mjs` (opción `engine: "core"`; el conversor actual sigue por defecto hasta M3b, que lo elimina)
- Crear: export en `tools/format_parity` (o un tool hermano) que convierte un LAS en memoria con `convert_serial` (`MemBytes`, `NoClock`) y devuelve los SHA de la pareja; runner Node con stubs de `__wbindgen_*` que lanzan (firmas §3); paso en `check.sh`
- Crear: driver Playwright `docs/records/benchmarks/data/m3a-parity-<aaaa-mm-dd>-driver.mjs` y registro `docs/records/benchmarks/M3A_PARITY_<aaaa-mm-dd>.md`

Se ejecuta en su propio worktree (B5) y solo toca `viewer_wasm`, el host web y `tools/`: el bucle
serie ya está en el núcleo (Tarea 4, B2).

- [ ] **Paso 1 (B1):** test Node que falla: SHA de la pareja convertida en wasm32 con el núcleo = SHA del CLI para el LAS pequeño de M0 (`413858a9…`), el LAS pequeño con 4096 puntos por bloque y divisiones 2 y 8, autzen y la fixture multi-chunk variable (SHA de la Tarea 1); cada caso salvo el primero comprueba más de un nodo.
- [ ] **Paso 2:** implementar hasta verde; tests Node de `converter-api`/`converter-worker` para `engine: "core"` (progreso, error y la cancelación fijada en la Tarea 3, B9).
- [ ] **Paso 3:** navegador: Chrome y Firefox por HTTP convierten el LAS pequeño con `engine: "core"`; SHA = CLI; registro. Criterio de parada (spec): SHA distinto sin explicación tras una semana.
- [ ] **Paso 4:** estado en README y este plan; commit.

---

## Después de la etapa 3

M3b (un worker con spools híbridos a escala Belvedere según S10, eliminar `browser_convert.rs`,
Chrome `file://` solo RAM), M4 (pool de preparación y escritor ordenado, S3/S4/S7), M5 (pool de
partición y trabajos por nodo, validación paralela) y M6 (Majadas), en el orden de la spec.
