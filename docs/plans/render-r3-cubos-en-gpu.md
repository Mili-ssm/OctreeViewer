# R3 — Cubos por nodo residentes en GPU: plan de implementación

Estado: en pausa · 01-11-2026

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Objetivo:** sustituir la voxelización en CPU del corte completo por vista (B1) por celdas por nodo,
independientes de la cámara, residentes en GPU y dibujadas como cubos, sin cambiar el formato.

**Arquitectura:** cada nodo produce una vez sus celdas en la rejilla diádica anclada en la raíz: el nodo
interno, a su profundidad de representantes; la hoja, una cadena de profundidades hasta la densidad de la
fuente. Las celdas viven en un pool de páginas de 8 KiB (512 instancias de 16 B) con una máscara de
cobertura de 512 bits por página. La selección LOD actual no cambia; un corte de celdas puro decide la
profundidad de cada nodo y las máscaras. El worker de construcción construye celdas y propone la
residencia; el de render sube páginas dentro de un presupuesto por frame, confirma lo residente, aplica
cortes y dibuja. R4 solo cambia la fuente de celdas y el ancla de la rejilla.

**Tecnología:** Rust nightly fijado, wgpu 30/WGSL, wasm-bindgen, workers JS, `playwright-core` con
Chrome (arnés R0).

**Spec:** [ADR 0019](../adr/0019-cubos-por-nodo-residentes-en-gpu.md),
[ADR 0027](../adr/0027-celdas-por-nodo-en-pool-paginado.md) (diseño de este plan),
[requisitos](../requirements.md) RF-03 a RF-08, RQ-01, RQ-02, RQ-05, RQ-09;
[plan de render](render-y-carga-escala.md) (arnés R0, criterios de parada);
[contrato de celdas](../explanation/REGIONAL_VOXEL_LOD.md).

**Progreso (30-10-2026):** redactado tras el análisis del arquitecto y enmendado tras su pre-mortem (abajo).
Requisito previo: R1-T1a cerrada (toca el código ★ de la Tarea 9).

**Pausa (01-11-2026):** la Tarea 1 superó dos criterios de parada con datos reales (cadenas de hoja en el tope en el 78–99 % de las hojas, 2,7 × los bytes de sus puntos; [registro](../records/benchmarks/RENDER_R3_T1_NODE_CELLS_01-11-2026.md)). El propietario decide hacer R4 primero ([ADR 0028](../adr/0028-r4-antes-que-r3.md)); al reanudar, la Tarea 1 se vuelve a medir con el formato de R4.

## Decisiones técnicas del coordinador (30-10-2026)

| Id | Decisión | Motivo |
| --- | --- | --- |
| D1 | Las hojas construyen una **cadena de profundidades** (Tarea 1) | Sin ella, o se incumple RQ-09 (celdas mayores que el objetivo) o una hoja da hasta 262 144 instancias |
| D2 | **Additive** (importación EPT) sigue en la ruta CPU hasta R4 | R4 reconvierte todo a la pirámide densa; una tercera ruta no compensa |
| D3 | El color por **atributo seleccionado** recalcula las celdas; RGB, intensidad, elevación y clase se resuelven en el shader con rangos por página | Paridad bit a bit (fidelidad primero) |
| D4 | El dispositivo pide `min(adapter.limits, 256 MiB)` para `max_buffer_size` y `max_storage_buffer_binding_size`; si no, `downlevel_defaults` con el pool limitado a 128 MiB | El techo actual es 128 MiB (`gpu.rs:326`); la iGPU comparte ~2 GB |
| D5 | Las celdas lejanas más finas que hoy (F1) se aceptan **solo si** la Tarea 4 no muestra más píxeles vacíos ni más del doble de instancias en pantalla; si no, la Tarea 15 (F2) entra en R3 | RQ-02 prohíbe más píxeles vacíos que la línea base |

## Enmiendas del pre-mortem (30-10-2026)

| # | Enmienda | Tareas |
| --- | --- | --- |
| A1 | La profundidad de representantes sale del **lado del nodo** con una `representative_depth` compartida (`voxel_grid.rs:794`), no de `level + 3`: el conversor admite `--spatial-divisions` 4 y 8. Fixtures con divisiones 4 y 8; el bench recorre todos los nodos reales | 1, 3 |
| A2 | Asignación punto → celda **acotada al rango propio del nodo** con la regla de cara máxima inclusiva (`spatial.rs:440,473`); invariante: cada punto cuenta exactamente una vez | 1, 2 |
| A3 | La cadena de hoja para cuando el número de celdas deja de crecer (puntos duplicados) o en profundidad de representantes + 7 (10 bits por eje); puerta de bytes por hoja | 1 |
| A4 | El spike de cobertura añade una pose rasante cerca del suelo y una cámara dentro de una hoja; cuenta instancias **en pantalla**; 3 rondas con tolerancia de ruido explícita | 4 |
| A5 | El spike de GPU va **después** del de cobertura y mide con sus recuentos reales (incluidas las instancias enmascaradas) | 4, 5 |
| A6 | Cada hoja dibuja su **mejor nivel residente** (sin todo-o-nada); presupuesto de subida por frame y contador `uploadBytesPerFrame` | 3, 7, 9 |
| A7 | El worker de render **confirma** el estado del pool; `nodeCells` y `evict` nunca se coalescen; resincronización ante desbordamiento o pérdida de dispositivo; tests con mensajes perdidos | 10 |
| A8 | El oráculo declara sus precondiciones (celdas dentro del frustum, cámara fuera de los nodos) y lista las divergencias conocidas | 2 |
| A9 | Mínimo y máximo de z e intensidad por página; los rangos de color son los de las páginas del corte; imágenes doradas por modo | 6, 12 |
| A10 | El espaciado de continuidad sale del **área ocupada**, no del volumen de los límites; test con nube plana de espaciado conocido | 13 |
| A11 | R1-T1a se cierra antes de la Tarea 9; rebanada de humo en navegador tras la Tarea 10; R1-T1b es la Tarea 14; `update_view_cells` conserva las entradas de etapa | 9, 11, 14 |
| A12 | Pool con techo explícito (≤ 256 MiB); el arnés registra la memoria GPU/GTT junto al PSS (el PSS no incluye VRAM) | 7, 17 |

**Criterios de parada:**
- La Tarea 1 encuentra un nodo real cuya profundidad derivada del lado no es la esperada y A1 no lo
  resuelve → parar antes de la Tarea 3.
- La cadena de una hoja ocupa más de 4 × los bytes de sus puntos, o más del 1 % de las hojas llega al
  tope de profundidad con celdas aún creciendo → replantear el almacenamiento de la cadena.
- La Tarea 4 muestra, en la pose rasante, más de 2 × instancias en pantalla que la CPU o más píxeles
  vacíos que la línea base más la tolerancia → F2 obligatoria y reestimar antes de la Tarea 6.
- La Tarea 5, con los recuentos de Belvedere a 4 px, da más de 16,7 ms de GPU por frame → parar y volver
  al propietario (disparador de ADR 0019).
- El oráculo necesita más excepciones que las listadas en la Tarea 2 → parar y rediseñar el contrato.
- La Tarea 11 muestra `RetainedPrevious` más de 1 s seguido en órbita, `poolOverflow` > 0 en Majadas a
  4 px o un intervalo máximo de frame ≥ 100 ms al hacer zoom → parar.
- Cualquier pérdida de dispositivo, o memoria GPU/GTT > 1 GiB en EQ-REF → parar.

## Restricciones globales

- `scripts/check.sh` en verde al cerrar cada tarea; sin cambiar el nightly ni añadir `#![feature]`.
- Sin cambios de formato ni de salida del conversor (eso es R4).
- `build_voxel_scene_with_limits` y la ruta actual se conservan: oráculo de tests, ruta Additive (D2) y
  respaldo hasta la Tarea 17.
- **Serie ★** sobre `crates/viewer_wasm/src/{lod_dataset.rs, bindings/lod.rs, bindings/builder.rs}`:
  una tarea cada vez.
- Puertas por contadores deterministas; tiempos solo en EQ-REF con adaptador hardware y sin otro Chrome,
  `cargo` ni `rustc` activos (criterios de parada de R0).
- Registros nuevos fechados y en español en `docs/records/benchmarks/`; commits convencionales en inglés
  con `Co-Authored-By`; sin push.
- Contrato visible: ningún cubo por encima del objetivo salvo por densidad de la fuente (RQ-09); sin solape
  entre generaciones; se retiene el último corte con cobertura completa.

## Review Focus

1. **Pool lleno:** si lo fijado del corte no cabe, se sigue dibujando el último corte con cobertura
   completa y se cuenta `poolOverflow`; nunca huecos (Tareas 7 y 9).
2. **Descendiente más fino que las celdas del padre:** la máscara no lo expresa; `cell_cut` devuelve
   `DescendantTooFine` y se retiene el corte anterior, como hoy `MixedCutNotAligned` (Tarea 3).
3. **Cambio de color en plena carga:** las páginas de una generación de color anterior se descartan al
   llegar (Tarea 12).
4. **Cámara dentro de un nodo o pegada al suelo:** `leaf_cell_depth` devuelve la profundidad más fina sin
   NaN ni pánico y respeta `lod_min_depth` (Tarea 3).
5. **Dataset Additive:** abre por la ruta CPU con su digest dorado intacto (Tarea 9).

---

## Tarea 1: celdas por nodo (`NodeCellSource`)

**Archivos:** crear `crates/viewer_render_wgpu/src/node_cells.rs` y `node_cells_tests.rs`; modificar
`lib.rs` y `voxel_grid.rs` (exportar `representative_depth`, A1); crear
`crates/viewer_render_wgpu/examples/node_cells_bench.rs`.

**Interfaces (produce):**

```rust
pub const MAX_CELL_DEPTH: u8 = 24;
pub const CHAIN_SPAN: u8 = 7;                       // profundidad de representantes + 7 = 10 bits por eje (A3)
pub struct CellGrid { pub root_min: [f64; 3], pub root_side: f64 }          // R4 cambia el ancla
pub fn representative_depth(grid: &CellGrid, bounds: SpatialBounds) -> u8;  // compartida con voxel_grid (A1)
pub struct NodeCell { pub coord: [u32; 3], pub rgb_sum: [u64; 3], pub intensity_sum: f64,
                      pub count: u32, pub classification: u8, pub z_min: f32, pub z_max: f32 }
pub struct NodeCellLevel { pub depth: u8, pub cells: Vec<NodeCell> }         // orden Morton
pub struct NodeCells { pub node: NodeId, pub levels: Vec<NodeCellLevel> }    // interno: 1 nivel; hoja: cadena
pub trait NodeCellSource {
    fn node_cells(&self, grid: &CellGrid, node: NodeId, bounds: SpatialBounds,
                  is_leaf: bool, block: &PointBlock) -> Result<NodeCells, NodeCellError>;
}
pub struct PointNodeCellSource;                                              // implementación de R3
pub fn node_cells_digest(cells: &NodeCells) -> u64;                          // FNV-64, como scene_digest
```

Acumulación en el orden de los puntos con las mismas sumas exactas que `accumulate_block`
(`voxel_grid.rs:836`); la clase la fija el primer punto. La coordenada de celda se acota al rango del
nodo con la regla de cara máxima inclusiva (A2). Cadena de hoja: desde la profundidad de representantes
hasta que el número de celdas no crece entre dos niveles o se alcanza `representative_depth + CHAIN_SPAN`
(A3).

- [x] **Paso 1: tests que fallan** en `node_cells_tests.rs`:
  `internal_node_cells_are_its_representatives` (una celda por subcelda ocupada, con divisiones 2, 4 y 8),
  `node_cells_do_not_depend_on_camera` (mismo digest con cuatro cámaras en el contexto),
  `cell_sums_match_accumulate_block`, `points_on_shared_and_max_faces_count_once` (A2: Σ recuentos = puntos
  finitos; ninguna celda fuera del rango del nodo), `leaf_chain_stops_when_cells_stop_growing` (hoja con
  xyz duplicados), `leaf_chain_respects_ten_bit_span`, `page_origin_relative_precision` (origen a 10⁶ m de
  la cámara: error f32 del centro ≤ 10⁻⁶ × distancia).
- [x] **Paso 2:** `cargo test -p viewer_render_wgpu node_cells` → FALLA (módulo inexistente).
- [x] **Paso 3:** implementar `node_cells.rs`.
- [x] **Paso 4:** los tests pasan. `node_cells_bench.rs` (`#[ignore]`, fuera de `check.sh`) recorre
  **todos** los nodos de Majadas y Belvedere: nodos no cúbicos o con profundidad inesperada, histograma de
  longitud de cadena, % de hojas en el tope, ms y bytes por hoja frente a los bytes de sus puntos (criterios
  de parada). Resultado al registro de la Tarea 4.
- [x] **Paso 5:** commit `feat(render): build camera-independent cells per LOD node`.

## Tarea 2: oráculo de refinamiento frente a la ruta CPU

**Archivos:** crear `crates/viewer_render_wgpu/tests/node_cells_refinement.rs`.

**Interfaces:** consume la Tarea 1 y `build_voxel_scene_with_limits`.

- [ ] **Paso 1: test** `refinement_relation_holds` sobre los fixtures de `scene_digest_tests.rs` y
  `tests/voxel_regions.rs` × 4 cámaras × {1, 2, 4, 8} px. **Precondiciones (A8):** solo celdas dentro del
  frustum y cámaras fuera de los nodos. Cada nodo va a la profundidad que daría `leaf_cell_depth` (aquí, un
  stub local). Relaciones: (1) toda celda nueva está dentro de una celda CPU; (2) toda celda CPU contiene
  ≥ 1 nueva; (3) por celda CPU, Σ rgb y Σ recuentos de las nuevas son exactamente iguales; (4) sin solape
  volumétrico. **Divergencias conocidas (lista cerrada):** celdas que la CPU descarta por estar fuera del
  frustum (`lod_dataset.rs:668-674`) y esquinas detrás de la cámara (error ∞, `camera.rs:470`).
- [ ] **Paso 2:** pasa; los 48 digests dorados siguen intactos (`cargo test -p viewer_render_wgpu`).
- [ ] **Paso 3:** commit `test(render): pin per-node cells as a refinement of the CPU voxels`.

## Tarea 3: corte de celdas puro (`cell_cut`)

**Archivos:** crear `crates/viewer_render_wgpu/src/cell_cut.rs` y tests; modificar `lib.rs`; sustituir el
stub de la Tarea 2 por `leaf_cell_depth`.

**Interfaces (produce):**

```rust
pub enum GapMode { Literal, Continuity { min_cell_side: f64 } }
pub struct CellCutEntry { pub node: NodeId, pub depth: u8, pub covered: [u64; 8] }   // 512 bits
pub enum CellCutError { DescendantTooFine { parent: NodeId, child: NodeId } }
pub fn leaf_cell_depth(view: &ViewProjection, bounds: SpatialBounds, grid: &CellGrid,
                       chain: std::ops::RangeInclusive<u8>, target_px: f64, mode: GapMode) -> u8;
pub fn cell_cut(plan: &LodTransitionPlan, node: &dyn Fn(NodeId) -> (SpatialBounds, bool),
                view: &ViewProjection, grid: &CellGrid, target_px: f64, mode: GapMode)
                -> Result<Vec<CellCutEntry>, CellCutError>;
pub fn resident_depth(wanted: u8, resident: &[u8]) -> Option<u8>;   // A6: el más cercano a `wanted`, preferiblemente más grueso
pub fn cell_cut_digest(cut: &[CellCutEntry]) -> u64;
```

`leaf_cell_depth`: la menor profundidad de la cadena con `projected_error_pixels(bounds, root_side /
2^d) ≤ target_px` (`camera.rs:660`); si ninguna cumple, la última (limitada por la fuente, como permite
RQ-09). `Continuity` recorta la profundidad a `floor(log2(root_side / min_cell_side))`. Una celda del
padre se enmascara si su región cae entera bajo un descendiente seleccionado; un descendiente con lado
menor que la celda del padre da `DescendantTooFine`.

- [ ] **Paso 1: tests que fallan:** `cut_respects_target_unless_source_limited` (RQ-09),
  `parent_cells_under_selected_children_are_masked` (volumen visible = volumen cubierto, sin solape),
  `descendant_finer_than_parent_cells_is_rejected` (Review Focus 2),
  `camera_inside_node_returns_finest_depth` (sin NaN; Review Focus 4), `continuity_clamps_depth`,
  `resident_depth_prefers_nearest_coarser` (A6), `cut_digest_golden` (4 cámaras × 3 px en los fixtures).
- [ ] **Paso 2:** fallan. **Paso 3:** implementar. **Paso 4:** pasan con `check.sh`.
- [ ] **Paso 5:** commit `feat(render): derive per-node cell depths and coverage masks from the LOD plan`.

## Tarea 4: spike de cobertura (S2) y decisión F1/F2

**Archivos:** crear `crates/viewer_render_wgpu/examples/r3_coverage_spike.rs`; registro
`docs/records/benchmarks/R3_COVERAGE_SPIKE_<fecha>.md`.

- [ ] **Paso 1:** Majadas y Belvedere (`target/render-data/`) con las poses de `probe-orbit-load.mjs`, más
  una **pose rasante** a 1,5 m del suelo y una **cámara dentro de una hoja** (A4), a {1, 2, 4, 8} px,
  3 rondas: instancias totales y **en pantalla** del corte nuevo frente a los vóxeles CPU, e instancias
  enmascaradas; píxeles vacíos de ambos con `render_offscreen` (`headless.rs:19`), convirtiendo celdas a
  `InstanceRaw`. Tolerancia de ruido: el rango entre rondas de la ruta CPU.
- [ ] **Paso 2:** registro con la tabla, el resultado del bench de la Tarea 1 y la decisión D5: **F1** si
  en toda la matriz los píxeles vacíos no superan la ruta CPU más la tolerancia y las instancias en
  pantalla son ≤ 2 × CPU; si no, **F2** (Tarea 15 obligatoria). Commit `docs(bench): measure R3 cell
  coverage against the CPU voxels`.

## Tarea 5: spike de GPU y límites (S1, S3)

**Archivos:** crear `examples/web-host/spike-cube-pool.html`, `spike-cube-pool.mjs` y
`probe-cube-pool.mjs`; registro `docs/records/benchmarks/R3_GPU_SPIKE_<fecha>.md`.

**Interfaces:** produce el WGSL de decodificación de 16 B que reutiliza la Tarea 6 (copiarlo allí).

- [ ] **Paso 1:** página WebGPU sin wasm que dibuja cubos sintéticos de 16 B (layout de la Tarea 6) a
  1920×1080 con los **recuentos de la Tarea 4** (A5): instancias totales, en pantalla y enmascaradas de
  Majadas 4 px y Belvedere 4 y 1 px, en dos variantes: un `draw` por página de 512 y un único `draw`.
  Mide el tiempo GPU con `timestamp-query` si existe y, si no, con `onSubmittedWorkDone`; vuelca
  `adapter.info` y `adapter.limits`.
- [ ] **Paso 2:** `probe-cube-pool.mjs` la conduce en Chrome (lanzamiento de `probe-orbit-load.mjs`),
  3 rondas; comprueba que `instance_index` incluye `firstInstance`.
- [ ] **Paso 3:** registro con mediana y rango. **Reglas:** Belvedere 4 px con `draw` por página ≤ 8 ms →
  `draw` por nodo; si solo el `draw` único ≤ 8 ms → Tarea 16 obligatoria; si el `draw` único supera
  16,7 ms → parar (criterio de parada). Commit `docs(bench): measure the R3 cube pool on the reference GPU`.

## Tarea 6: instancia de 16 B y tabla de páginas

**Archivos:** crear `crates/viewer_render_wgpu/src/cell_pool.rs` (layout, páginas, LUT sRGB) y
`cell_pool.wgsl`; tests en `cell_pool_tests.rs`.

**Interfaces (produce):**

```rust
pub const PAGE_INSTANCES: u32 = 512;
#[repr(C)] pub struct CellInstance { pub coord: u32, pub rgba8: u32, pub intensity: f32, pub class_depth_flags: u32 }
// coord: 10 bits por eje relativos al origen del nodo; class:u16 | depth_offset:u8 | flags:u8
pub struct PageEntry { pub origin_rel_camera: [f32; 3], pub cell_side: f32, pub covered: [u32; 16],
                       pub z_range: [f32; 2], pub intensity_range: [f32; 2] }      // A9
pub fn encode_page(level: &NodeCellLevel, node_origin: [u32; 3]) -> Vec<CellInstance>;
pub fn decode_cell(page: &PageEntry, instance: &CellInstance) -> ([f32; 3], f32, [f32; 4]);  // centro, lado, color lineal
```

- [ ] **Paso 1: tests:** `instance_is_16_bytes`, `srgb_lut_matches_to_linear_rgba` (los 256 valores,
  bit a bit, frente a `to_linear_rgba`, `geometry.rs:63`), `cpu_decode_equals_gpu_decode` (`headless`: un
  compute que aplica `decode_cell` de WGSL a mil celdas; igualdad bit a bit con Rust),
  `page_ranges_cover_their_cells` (A9), `wgsl_validates_with_naga`.
- [ ] **Paso 2–4:** fallan, implementar, pasan. **Paso 5:** commit
  `feat(render): add the 16-byte cell instance and page table`.

## Tarea 7: pool en `SurfaceRenderer`

**Archivos:** modificar `crates/viewer_render_wgpu/src/gpu.rs` (`SurfaceRenderer`, `gpu.rs:256`) y
`headless.rs`; tests en `crates/viewer_render_wgpu/tests/cell_pool_render.rs`.

**Interfaces (produce):**

```rust
pub struct CellPoolConfig { pub max_bytes: u64, pub upload_bytes_per_frame: u64 }  // ≤ 256 MiB (A12); 4 MiB por frame
impl SurfaceRenderer<'_> {
    pub fn upload_node_cells(&mut self, node: NodeId, level: &NodeCellLevel, color_gen: u32) -> Result<(), GpuRendererError>;
    pub fn evict_node(&mut self, node: NodeId);
    pub fn set_cell_cut(&mut self, epoch: u64, cut: &[CellCutEntry]) -> Result<CutApply, GpuRendererError>;
    pub fn cell_pool_state(&self) -> CellPoolState;   // (nodo, profundidad, generación) residentes: base del ack (A7)
    pub fn cell_pool_stats(&self) -> CellPoolStats;   // pages, pinned, evictions, overflow, upload_bytes_per_frame
}
pub enum CutApply { Applied, RetainedPrevious { nodes_without_level: u32 } }
```

Cada entrada dibuja `resident_depth` (A6); el corte se aplica si todo nodo tiene al menos un nivel
residente y, si no, se retiene el anterior. Las subidas pendientes se reparten con
`upload_bytes_per_frame`. `draw(…, firstInstance)` por nodo; límites del dispositivo según D4.

- [ ] **Paso 1: tests:** `single_node_cut_matches_cpu_path_pixels` (misma imagen que `upload_scene` con un
  nodo), `masked_parent_cells_are_not_drawn`, `overflow_keeps_previous_complete_cut` (Review Focus 1),
  `leaf_draws_nearest_resident_level` (A6), `uploads_respect_per_frame_budget`,
  `lru_never_evicts_pinned_pages`, `device_requests_capped_limits_with_fallback` (D4).
- [ ] **Paso 2–4.** **Paso 5:** commit `feat(render): keep per-node cells in a paged GPU pool`.

## Tarea 8: picking sobre el pool

**Archivos:** `gpu.rs` (`gpu.rs:879-970`, `:1412`) y los tests de picking existentes.

- [ ] **Paso 1:** los chunks candidatos pasan a ser páginas acotadas por los límites del nodo; se decodifica
  con `decode_cell`. El HZB queda desactivado en la ruta de pool hasta la Tarea 16 (hoy solo actúa con la
  cámara quieta, `gpu.rs:1174`).
- [ ] **Paso 2:** test nuevo `pick_on_pool_hits_same_cell_as_cpu_path`; los tests de picking actuales pasan
  sin cambios. **Paso 3:** commit `feat(render): pick cells from the GPU pool`.

## Tarea 9 ★: `LodDataset` con celdas por nodo

**Requisito:** R1-T1a cerrada (A11).

**Archivos:** `crates/viewer_wasm/src/lod_dataset.rs` (`update_view_progressive`, `:495`),
`lod_dataset_tests.rs`, `bindings/lod.rs`.

**Interfaces (produce):**

```rust
impl LodDataset {
    // Mismas entradas de etapa y de carga que update_view_progressive (A11) más el modo de huecos.
    pub fn update_view_cells(&mut self, view: &ViewProjection, target_voxel_pixels: f64,
                             selection_error_pixels: f64, stage_voxel_pixels: f64, mode: GapMode,
                             async_priority_order: Option<&mut Vec<BlockId>>,
                             load_block: impl FnMut(BlockId) -> Result<Option<PointBlock>, String>)
                             -> Result<CellUpdate, LodDatasetError>;
}
pub struct CellUpdate { pub build: Vec<(NodeId, u8)>, pub evict: Vec<NodeId>, pub cut: Vec<CellCutEntry>,
                        pub epoch: u64, pub stage_voxel_pixels: f64 }
```

Contadores nuevos: `nodeCellBuilds`, `cellSourcePoints`, `cutUpdates`, `poolPages`, `poolEvictions`,
`poolOverflow`, `uploadBytesPerFrame`; `cutDigestAtRest` sustituye a `sceneDigestAtRest` en esta ruta.
Additive (D2) devuelve el error tipado `CellPathUnsupported` y quien llama usa `update_view_progressive`.

- [ ] **Paso 1: tests:** `cell_update_golden` (decisiones fijadas, como `NATIVE_UPDATE_GOLDEN`,
  `lod_dataset_tests.rs:1685`), `stage_never_rises_during_orbit_on_cell_path` (RQ-02),
  `additive_dataset_uses_cpu_path` (Review Focus 5: su digest dorado no cambia),
  `pool_overflow_retains_last_complete_cut`; los dorados de la ruta actual siguen intactos.
- [ ] **Paso 2–4.** **Paso 5:** commit `feat(wasm): drive per-node cells from the LOD dataset`.

## Tarea 10 ★: protocolo de workers y respaldo

**Archivos:** `crates/viewer_wasm/src/bindings/builder.rs` (`WasmSceneBuilder`, `:32`),
`bindings/render.rs`, `examples/web-host/{build-worker.js, viewer-worker.js, viewer-api.mjs}`; tests en
`examples/web-host/tests/`.

**Interfaces:**
- Del worker de construcción al de render: `nodeCells {node, depth, colorGen, buf}` (transferido) y
  `evict {nodes}`, **nunca coalescidos** (A7), y `cut {epoch, entries}`, donde gana el último.
- Del worker de render al de construcción: `poolAck {epoch, resident: [[node, depth, colorGen]]}`, que es
  la fuente de verdad de la residencia, y `poolResync` tras desbordamiento o pérdida de dispositivo.
- Opción de API: `renderPath: "cells" | "cpu"`, con `"cpu"` por defecto hasta la Tarea 17.

- [ ] **Paso 1: tests:** test de navegador con `pkg` recién construido: la misma secuencia de bloques y
  vistas con el worker ON y OFF (respaldo sin workers anidados) da el **mismo `cutDigestAtRest`**. Test
  Node del protocolo: `nodeCells` perdido o descartado → el siguiente `poolAck` lo delata y se reenvía;
  `poolResync` reconstruye el estado; los `cut` se coalescen por `epoch`.
- [ ] **Paso 2–4.** **Paso 5:** commit `feat(web-host): stream per-node cells from the build worker`.

## Tarea 11: rebanada de humo en navegador (A11)

**Archivos:** `examples/web-host/probe-orbit-load.mjs` (`--render-path cells`, memoria GPU/GTT de
`/sys/class/drm/card*/device/mem_info_{vram,gtt}_used` junto al PSS, A12); registro
`docs/records/benchmarks/R3_SMOKE_<fecha>.md`.

- [ ] **Paso 1:** Majadas a 4 px, RGB, modo literal, Chrome en EQ-REF: carga, órbita de 10 s y un zoom
  guionizado. Registra el intervalo entre frames (p95 y máx), `RetainedPrevious` seguidos, `poolOverflow`,
  `uploadBytesPerFrame`, nodos sin cubrir, píxeles vacíos y memoria GPU/GTT.
- [ ] **Paso 2:** aplica los criterios de parada (racha de `RetainedPrevious` > 1 s, `poolOverflow` > 0,
  máximo ≥ 100 ms en zoom, GPU/GTT > 1 GiB). Commit `docs(bench): smoke-test the R3 cell path in Chrome`.

## Tarea 12 ★: modos de color

**Archivos:** `crates/viewer_wasm/src/{scene_builder.rs, block_loading.rs}`, `build-worker.js`,
`viewer-worker.js`, `crates/viewer_render_wgpu/src/cell_pool.wgsl`.

- [ ] **Paso 1: tests:**
  - `shader_color_modes_need_no_rebuild`: RGB, intensidad, elevación y clase no cambian `nodeCellBuilds`.
  - `color_ranges_come_from_cut_pages` (A9): elevación e intensidad usan el mín./máx. de las páginas del
    corte; imágenes doradas por modo en `headless`.
  - `attribute_mode_rebuilds_with_bit_parity` (D3): celdas iguales a la ruta CPU en el atributo
    seleccionado, con el worker ON y OFF.
  - `stale_color_generation_pages_are_dropped` (Review Focus 3).
- [ ] **Paso 2–4.** **Paso 5:** commit `feat(web-host): recolour cell pages per colour generation`.

## Tarea 13 ★: continuidad por nube

**Archivos:** `lod_dataset.rs` (cálculo de `min_cell_side`), `viewer-api.mjs` (opción `gapMode:
"literal" | "continuity"`), `main.js` (control) y tests.

`min_cell_side` es el espaciado superficial de toda la nube, sacado del índice sin leer bloques (A10):
- `s = sqrt(A / N)`;
- `A` = Σ, sobre los nodos padre de hojas, de sus representantes (celdas ocupadas) × el lado de su celda²;
- `N` = Σ de los puntos de esas hojas.

- [ ] **Paso 1: tests:**
  - `literal_is_default`.
  - `flat_cloud_spacing_is_recovered`: nube plana sintética con espaciado de 0,1 m da `s` ∈ [0,09; 0,11].
  - `continuity_uses_one_spacing_for_the_whole_cloud` (RF-05).
  - `continuity_never_overlaps`.
  - Test de la opción en la API.
- [ ] **Paso 2–4.** **Paso 5:** commit `feat(viewer): add per-cloud continuity mode`.

## Tarea 14: modos de px (antes R1-T1b)

**Archivos:** crear `crates/viewer_core/src/px_controller.rs` y tests; `viewer-api.mjs` (opción
`pxMode: { kind: "fixed", px } | { kind: "dynamic", frameMs: 16.7 }`), `viewer-worker.js`.

**Interfaces:**

```rust
pub struct PxController { /* objetivo, banda del 15 %, permanencia mínima en frames */ }
impl PxController {
    pub fn fixed(px: f64) -> Self;
    pub fn dynamic(min_px: f64, max_px: f64, frame_ms_target: f64) -> Self;   // 1..8 px, 16.7 ms
    pub fn observe(&mut self, gpu_frame_ms: f64) -> f64;                      // devuelve el objetivo de px
}
```

- [ ] **Paso 1: tests** con tiempos sintéticos:
  - `fixed_never_changes`.
  - `dynamic_raises_px_when_frames_exceed_target`.
  - `dynamic_has_hysteresis`: no oscila con tiempos alternos dentro de la banda.
  - `dynamic_stays_within_1_to_8_px`.
- [ ] **Paso 2–4.** Solo cambia el corte; las celdas no se reconstruyen. **Paso 5:** commit
  `feat(viewer): add fixed and frame-time px modes`.

## Tarea 15 (condicional, D5): refinado por octante (F2)

Solo si la Tarea 4 decide F2. Los octantes del padre cuyo error proyectado supera el objetivo se
sustituyen por celdas del hijo, usando las máscaras de la Tarea 3. Cambia la selección, así que se mide
aparte (tiempo hasta el objetivo, píxeles vacíos, expulsiones), como R1-T3b. Antes de empezar: anexo a
este plan con tareas y pre-mortem propio; tope de 3 días antes de volver al propietario.

## Tarea 16 (condicional, Tarea 5): compactación y HZB sobre el pool

Solo si la Tarea 5 lo exige o la Tarea 17 incumple RQ-01. Se generaliza `cull_instances` (`hzb.rs:890`) a
un pase de compactación (corte, máscara, frustum y HZB) que escribe `visible_indices` y los argumentos
indirectos. `firstInstance` en dibujo indirecto necesita la feature opcional `indirect-first-instance`, o
índices absolutos. Antes de empezar: anexo con tareas y pre-mortem propio.

## Tarea 17: medida y cambio de ruta por defecto

**Archivos:** `viewer-api.mjs` (defecto `renderPath: "cells"`), `probe-orbit-load.mjs`, registro
`docs/records/benchmarks/R3_MEASURE_<fecha>.md`, plan de render y README.

- [ ] **Paso 1:** matriz de R0 (Majadas 4 px, Belvedere 8 y 4 px, 3 rondas, EQ-REF, Chrome) con `cells`
  frente a `cpu`. **Puertas:**
  - RQ-01: p95 < 33 ms y máx < 100 ms, en carga, órbita y zoom.
  - RQ-02: la etapa nunca sube; nodos sin cubrir y píxeles vacíos ≤ línea base.
  - RQ-05: PSS ≤ 8 GiB, y memoria GPU/GTT registrada (A12).
  - Se registran también las instancias, `poolOverflow` y `uploadBytesPerFrame`.
- [ ] **Paso 2:** si pasan, `renderPath: "cells"` por defecto (Additive sigue en CPU, D2) y ADR 0027 pasa a
  aceptada. Si no, criterio de parada: registrar y volver a ADR 0019/0027 sin cambiar el defecto.
- [ ] **Paso 3:** commit `feat(viewer)!: draw per-node GPU cells by default`, y actualizar el plan de render
  (R3 hecha) y el README.
