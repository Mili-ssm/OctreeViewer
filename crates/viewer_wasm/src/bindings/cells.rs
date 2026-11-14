//! R3 Task 10b: JS-facing side of the cells path. Everything here converts between `js_sys`
//! values and the plain Rust of `cells_glue`; the logic lives there and is tested natively.
//!
//! Wire shapes (ids are `BigUint64Array`, epochs and generations are numbers below 2^53):
//!
//! * builder `update` / `updateCells` in cells mode:
//!   `{ epoch, generation, cutEpoch, viewEpoch, cut, support, idle, targetPixels,
//!   nodeCells: ArrayBuffer[],
//!   needsRefinement, sceneBuffer: null, instanceCount: 0, blockIds: [], digest: null, lod }`.
//!   `nodeCells` keeps build order (parents first) and its buffers are transferable.
//!   `viewEpoch` is the one the host gave the builder with the camera (`setViewEpoch`, before
//!   `update`); the viewer's `viewEpoch()` is the epoch of its current `cameraTransport()`.
//! * viewer `addCellNode(generation, bytes)` per entry of `nodeCells` (it throws on bytes that
//!   do not decode), then `setCellCut({ cutEpoch, viewEpoch, cut, support, idle, targetPixels })`.
//! * viewer `takeCellMessages()` returns `{ poolAck: { cutEpoch, drawn, missing } | null, evict,
//!   poolReset, driverReplaced }`; the build worker applies them in this order: `driverReplaced`,
//!   `poolReset`, `applyEvict`, `applyPoolAck` (the same order `ViewerCells::take_messages`
//!   documents).

use js_sys::{Array, BigUint64Array, Object, Reflect, Uint8Array};
use viewer_core::NodeId;
use viewer_render_wgpu::{CellGrid, CutApply};
use wasm_bindgen::prelude::*;

use super::{WasmVoxelViewer, builder::WasmSceneBuilder};
use crate::{
    CellPlanStats, CellUpdateConfig, PoolAck,
    cells_glue::{
        BuilderCells, BuilderOutput, CellsStats, TARGET_PIXELS_RANGE, ViewerCells,
        WASM_CPU_BUDGET_BYTES, driver_config,
    },
    scene_builder::SceneBuilderCore,
};

type FormatCore =
    SceneBuilderCore<viewer_format::FormatReader<crate::browser_io::BrowserBlobReader>>;

pub(super) const PATH_OFF: &str = "off";
pub(super) const PATH_ENABLED: &str = "enabled";
pub(super) const PATH_UNSUPPORTED: &str = "CellPathUnsupported";

fn set(object: &Object, name: &str, value: &JsValue) -> Result<(), JsValue> {
    Reflect::set(object, &name.into(), value).map(drop)
}

fn ids_to_js(ids: &[NodeId]) -> BigUint64Array {
    let raw: Vec<u64> = ids.iter().map(|id| id.get()).collect();
    BigUint64Array::from(raw.as_slice())
}

fn ids_from_js(value: &JsValue, name: &str) -> Result<Vec<NodeId>, JsValue> {
    let array: BigUint64Array = value
        .clone()
        .dyn_into()
        .map_err(|_| JsValue::from_str(&format!("{name} must be a BigUint64Array")))?;
    Ok(array.to_vec().into_iter().map(NodeId::new).collect())
}

fn field(object: &JsValue, name: &str) -> Result<JsValue, JsValue> {
    Reflect::get(object, &name.into())
}

/// A non-negative integer below 2^53 (epochs and generations).
fn counter(value: f64, name: &str) -> Result<u64, JsValue> {
    if value >= 0.0 && value.fract() == 0.0 && value <= 9_007_199_254_740_991.0 {
        Ok(value as u64)
    } else {
        Err(JsValue::from_str(&format!(
            "{name} must be a non-negative integer"
        )))
    }
}

fn number_field(object: &JsValue, name: &str) -> Result<f64, JsValue> {
    field(object, name)?
        .as_f64()
        .ok_or_else(|| JsValue::from_str(&format!("{name} must be a number")))
}

/// A new object holding [`add_cells_stats`], for the `cellStats` of either side.
pub(super) fn cells_stats_object(path: &str, stats: &CellsStats) -> Result<JsValue, JsValue> {
    let value: JsValue = Object::new().into();
    add_cells_stats(&value, path, stats)?;
    Ok(value)
}

/// Writes `cellPath`, `cellsBusy`, the epochs, the pool counters and `wasmLinearMemoryBytes`.
pub(super) fn add_cells_stats(
    value: &JsValue,
    path: &str,
    stats: &CellsStats,
) -> Result<(), JsValue> {
    let object: &Object = value.unchecked_ref();
    set(object, "cellPath", &JsValue::from_str(path))?;
    set(object, "cellsBusy", &JsValue::from_bool(stats.cells_busy))?;
    set(
        object,
        "cellsNeedFrame",
        &JsValue::from_bool(stats.needs_frame),
    )?;
    set(
        object,
        "cutDigestAtRest",
        &stats.cut_digest_at_rest.map_or(JsValue::NULL, |digest| {
            JsValue::from_str(&format!("{digest:016x}"))
        }),
    )?;
    set(
        object,
        "cutApply",
        &stats.cut_apply.map_or(JsValue::NULL, |apply| {
            JsValue::from_str(match apply {
                CutApply::Applied => "applied",
                CutApply::RetainedPrevious => "retained",
            })
        }),
    )?;
    let counters = &stats.counters;
    for (name, number) in [
        ("viewEpoch", stats.view_epoch as f64),
        ("cutEpochSent", stats.cut_epoch_sent as f64),
        ("cutEpochApplied", stats.cut_epoch_applied as f64),
        ("queuedNodeCells", stats.queued_nodes as f64),
        ("queuedNodeBytes", stats.queued_bytes as f64),
        ("cellsForgottenTotal", stats.forgotten_total as f64),
        ("cellCpuBytes", stats.cpu_cell_bytes as f64),
        ("poolCapacityPages", f64::from(stats.pool_capacity_pages)),
        ("poolStages", f64::from(counters.stages)),
        (
            "poolNodesWithoutLevel",
            f64::from(counters.nodes_without_level),
        ),
        ("poolCoarsened", f64::from(counters.pool_coarsened)),
        ("poolOverflow", f64::from(counters.pool_overflow)),
        ("poolResidentPages", f64::from(counters.resident_pages)),
        ("poolUploadedBytes", counters.uploaded_bytes as f64),
        (
            "wasmLinearMemoryBytes",
            stats.wasm_linear_memory_bytes as f64,
        ),
    ] {
        set(object, name, &JsValue::from_f64(number))?;
    }
    set(
        object,
        "poolDescendantTooFine",
        &JsValue::from_bool(counters.descendant_too_fine),
    )
}

/// The update config of the viewer: the build side fits the cut to the same CPU budget the driver
/// has (ADR 0037) and to the pages of the GPU pool the render side runs (ADR 0040).
fn wasm_update_config(gpu_pool_pages: Option<u32>) -> CellUpdateConfig {
    CellUpdateConfig {
        cpu_budget_bytes: Some(WASM_CPU_BUDGET_BYTES),
        gpu_pool_pages,
        ..CellUpdateConfig::default()
    }
}

/// The planner's side of a cells update, under the names `lod` stats use.
pub(super) fn add_plan_stats(value: &JsValue, plan: &CellPlanStats) -> Result<(), JsValue> {
    let object: &Object = value.unchecked_ref();
    let blocks: Array = plan
        .pending_block_ids
        .iter()
        .map(|id| JsValue::from_str(&id.get().to_string()))
        .collect();
    let pages: Array = plan
        .pending_page_anchors
        .iter()
        .map(|id| JsValue::from_str(&id.get().to_string()))
        .collect();
    set(object, "pendingBlockIds", &blocks)?;
    set(object, "pendingPageAnchors", &pages)?;
    for (name, number) in [
        ("builtNodes", plan.built_nodes.len() as f64),
        ("buildDeferred", plan.build_deferred as f64),
        ("buildMs", plan.build_ms),
        ("cutNodes", plan.cut_nodes as f64),
        ("maxGap", f64::from(plan.max_gap)),
        ("gapsOverOne", plan.gaps_over_one as f64),
        ("uncoveredNodes", plan.uncovered_nodes as f64),
        ("cpuCoarsened", plan.cpu_coarsened as f64),
        ("gpuCoarsened", plan.gpu_coarsened as f64),
        ("gpuPoolPages", f64::from(plan.gpu_pool_pages)),
        ("forgottenSelected", plan.diag.forgotten_selected as f64),
        ("forgottenWaiter", plan.diag.forgotten_waiter as f64),
        ("forgottenAncestor", plan.diag.forgotten_ancestor as f64),
        ("rebuiltTotal", plan.diag.rebuilt_total as f64),
        ("selectionNodes", plan.diag.selection_nodes as f64),
        (
            "selectedBuiltNotInCut",
            plan.diag.selected_built_not_in_cut as f64,
        ),
        (
            "selectedBuiltBlocksMissing",
            plan.diag.selected_built_blocks_missing as f64,
        ),
        ("cpuNeededBytes", plan.diag.cpu_needed_bytes as f64),
        ("blockCacheBytes", plan.diag.block_cache_bytes as f64),
        (
            "blockCacheLimitBytes",
            plan.diag.block_cache_limit_bytes as f64,
        ),
        ("blockEvictions", plan.diag.block_evictions as f64),
    ] {
        set(object, name, &JsValue::from_f64(number))?;
    }
    set(
        object,
        "buildCapped",
        &JsValue::from_bool(plan.build_capped),
    )
}

fn ack_to_js(ack: &PoolAck) -> Result<JsValue, JsValue> {
    let object = Object::new();
    set(
        &object,
        "cutEpoch",
        &JsValue::from_f64(ack.cut_epoch as f64),
    )?;
    set(&object, "drawn", &ids_to_js(&ack.drawn))?;
    set(&object, "missing", &ids_to_js(&ack.missing))?;
    Ok(object.into())
}

fn ack_from_js(value: &JsValue) -> Result<PoolAck, JsValue> {
    Ok(PoolAck {
        cut_epoch: counter(number_field(value, "cutEpoch")?, "cutEpoch")?,
        drawn: ids_from_js(&field(value, "drawn")?, "drawn")?,
        missing: ids_from_js(&field(value, "missing")?, "missing")?,
    })
}

/// `{ epoch, generation, cutEpoch, cut, ... }` for a cells update; the scene fields of the CPU
/// path are present and empty so a host can read one shape.
pub(super) fn builder_output_to_js(
    output: &BuilderOutput,
    needs_refinement: bool,
    lod: &JsValue,
) -> Result<JsValue, JsValue> {
    let result = Object::new();
    let node_cells: Array = output
        .nodes
        .iter()
        // `Uint8Array::from` copies into JS memory, so each buffer can be transferred.
        .map(|node| Uint8Array::from(node.bytes.as_slice()).buffer())
        .collect();
    set(
        &result,
        "epoch",
        &JsValue::from_f64(output.cut_epoch as f64),
    )?;
    set(
        &result,
        "generation",
        &JsValue::from_f64(output.generation as f64),
    )?;
    set(
        &result,
        "cutEpoch",
        &JsValue::from_f64(output.cut_epoch as f64),
    )?;
    set(
        &result,
        "viewEpoch",
        &JsValue::from_f64(output.view_epoch as f64),
    )?;
    set(&result, "cut", &ids_to_js(&output.cut))?;
    set(&result, "support", &ids_to_js(&output.support))?;
    set(&result, "idle", &JsValue::from_bool(output.idle))?;
    set(
        &result,
        "stagePixels",
        &JsValue::from_f64(output.stage_pixels),
    )?;
    set(
        &result,
        "targetPixels",
        &JsValue::from_f64(output.target_pixels),
    )?;
    set(&result, "nodeCells", &node_cells)?;
    set(
        &result,
        "needsRefinement",
        &JsValue::from_bool(needs_refinement),
    )?;
    set(&result, "sceneBuffer", &JsValue::NULL)?;
    set(&result, "instanceCount", &JsValue::from_f64(0.0))?;
    set(&result, "blockIds", &Array::new())?;
    set(&result, "digest", &JsValue::NULL)?;
    set(&result, "lod", lod)?;
    Ok(result.into())
}

impl WasmSceneBuilder {
    /// Creates the cells side after a dataset was installed, when the host asked for it.
    pub(super) fn install_cells(&mut self) {
        self.cells = None;
        self.cell_path = PATH_OFF;
        if !self.cells_requested {
            return;
        }
        let supported = self
            .core
            .as_ref()
            .is_some_and(|core| core.session().dataset().supports_cells());
        if supported {
            self.cells = Some(BuilderCells::new(wasm_update_config(self.cell_pool_pages)));
            self.cell_path = PATH_ENABLED;
        } else {
            self.cell_path = PATH_UNSUPPORTED;
        }
    }

    /// `cellPath` (and `cellsInfo` when enabled) on an open result.
    pub(super) fn add_open_fields(&self, result: &JsValue) -> Result<(), JsValue> {
        let object: &Object = result.unchecked_ref();
        set(object, "cellPath", &JsValue::from_str(self.cell_path))?;
        set(object, "cellsInfo", &self.cells_info()?)
    }

    fn cells_info(&self) -> Result<JsValue, JsValue> {
        let (Some(core), Some(_)) = (self.core.as_ref(), self.cells.as_ref()) else {
            return Ok(JsValue::NULL);
        };
        let dataset = core.session().dataset();
        let grid = dataset.cell_grid();
        let info = Object::new();
        set(
            &info,
            "generation",
            &JsValue::from_f64(dataset.cell_generation() as f64),
        )?;
        set(
            &info,
            "origin",
            &js_sys::Float64Array::from(dataset.coordinate_origin().as_slice()),
        )?;
        set(
            &info,
            "rootMin",
            &js_sys::Float64Array::from(grid.root_min.as_slice()),
        )?;
        set(&info, "rootSide", &JsValue::from_f64(grid.root_side))?;
        Ok(info.into())
    }

    pub(super) fn cells_lod_stats(&self, plan: &CellPlanStats) -> Result<JsValue, JsValue> {
        let value: JsValue = Object::new().into();
        add_plan_stats(&value, plan)?;
        if let (Some(core), Some(cells)) = (self.core.as_ref(), self.cells.as_ref()) {
            add_cells_stats(
                &value,
                self.cell_path,
                &cells.stats(core.session().dataset()),
            )?;
        }
        Ok(value)
    }
}

#[wasm_bindgen]
impl WasmSceneBuilder {
    /// Opt in to the cells path; call before opening a dataset. A format the cells cannot draw
    /// (no global frame, not replacement) keeps the CPU path and reports `cellPath:
    /// "CellPathUnsupported"` on the open result.
    #[wasm_bindgen(js_name = enableCells)]
    pub fn enable_cells(&mut self, enabled: bool) {
        self.cells_requested = enabled;
    }

    /// Keeps intensity, elevation and classification on the cells path (shader modes, R3-Q32)
    /// instead of the CPU path of R3-Q23. Off by default until the CPU fallback goes.
    #[wasm_bindgen(js_name = enableCellShaderModes)]
    pub fn enable_cell_shader_modes(&mut self, enabled: bool) {
        self.cells_shader_modes = enabled;
    }

    /// The pages of the render worker's GPU pool (`cellPoolPages` of the viewer); call before
    /// opening a dataset. Without it the cut is not fitted to the pool.
    #[wasm_bindgen(js_name = setCellPoolPages)]
    pub fn set_cell_pool_pages(&mut self, pages: u32) {
        self.cell_pool_pages = (pages > 0).then_some(pages);
    }

    /// `"off"`, `"enabled"` or `"CellPathUnsupported"` for the open dataset.
    #[wasm_bindgen(js_name = cellPath)]
    pub fn cell_path(&self) -> String {
        self.cell_path.to_owned()
    }

    /// Like `update`, but errors unless the cells path is active.
    #[wasm_bindgen(js_name = updateCells)]
    pub fn update_cells(
        &mut self,
        camera: js_sys::Float64Array,
        viewport_width: u32,
        viewport_height: u32,
        visual_scale: f64,
        target_voxel_pixels: f64,
    ) -> Result<JsValue, JsValue> {
        if !self.cells.as_ref().is_some_and(BuilderCells::is_active) {
            return Err(JsValue::from_str("the cells path is not active"));
        }
        self.update(
            camera,
            viewport_width,
            viewport_height,
            visual_scale,
            target_voxel_pixels,
        )
    }

    /// `poolAck` from the render worker: `{ cutEpoch, drawn, missing }`. The last one wins.
    #[wasm_bindgen(js_name = applyPoolAck)]
    pub fn apply_pool_ack(&mut self, ack: JsValue) -> Result<(), JsValue> {
        let ack = ack_from_js(&ack)?;
        let (core, cells) = self.cells_parts()?;
        cells.pool_ack(core.session_mut().dataset_mut(), ack);
        Ok(())
    }

    /// `evict` from the render worker: the nodes its driver forgot (`BigUint64Array`).
    #[wasm_bindgen(js_name = applyEvict)]
    pub fn apply_evict(&mut self, nodes: JsValue) -> Result<(), JsValue> {
        let nodes = ids_from_js(&nodes, "evict")?;
        let (core, cells) = self.cells_parts()?;
        cells.evict(core.session_mut().dataset_mut(), &nodes);
        Ok(())
    }

    /// `poolReset`: the render worker's pool was emptied; its driver keeps the nodes.
    #[wasm_bindgen(js_name = poolReset)]
    pub fn pool_reset(&mut self) -> Result<(), JsValue> {
        let (core, cells) = self.cells_parts()?;
        cells.pool_reset(core.session_mut().dataset_mut());
        Ok(())
    }

    /// `driverReplaced`: the render worker has a new driver; every node is sent again.
    #[wasm_bindgen(js_name = driverReplaced)]
    pub fn driver_replaced(&mut self) -> Result<(), JsValue> {
        let (core, cells) = self.cells_parts()?;
        cells.driver_replaced(core.session_mut().dataset_mut());
        Ok(())
    }

    /// Dataset grid and origin for the render worker's `beginRemoteCells`, or `null`.
    #[wasm_bindgen(js_name = cellsInfo)]
    pub fn cells_info_js(&self) -> Result<JsValue, JsValue> {
        self.cells_info()
    }

    /// Stats of the build side of the cells path (`cellsBusy` is "another update is pending"),
    /// or `null` while the cells path is off.
    #[wasm_bindgen(js_name = cellStats)]
    pub fn cell_stats(&self) -> Result<JsValue, JsValue> {
        let (Some(core), Some(cells)) = (self.core.as_ref(), self.cells.as_ref()) else {
            return Ok(JsValue::NULL);
        };
        cells_stats_object(self.cell_path, &cells.stats(core.session().dataset()))
    }

    /// Epoch of the view of the next `update` (the viewer's `viewEpoch()` when it read the
    /// camera sent with it); echoed as `viewEpoch` in the update result.
    #[wasm_bindgen(js_name = setViewEpoch)]
    pub fn set_view_epoch(&mut self, epoch: f64) -> Result<(), JsValue> {
        let epoch = counter(epoch, "viewEpoch")?;
        let (_, cells) = self.cells_parts()?;
        cells.set_view_epoch(epoch);
        Ok(())
    }
}

impl WasmSceneBuilder {
    fn cells_parts(&mut self) -> Result<(&mut FormatCore, &mut BuilderCells), JsValue> {
        match (self.core.as_mut(), self.cells.as_mut()) {
            (Some(core), Some(cells)) => Ok((core, cells)),
            _ => Err(JsValue::from_str("the cells path is not enabled")),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Viewer
// ---------------------------------------------------------------------------------------------

impl WasmVoxelViewer {
    /// In-process cells for the dataset just installed, when the host opted in and the dataset
    /// and device support them.
    pub(super) fn install_local_cells(&mut self) {
        self.cells_local = None;
        self.cells_remote = None;
        self.cell_path = PATH_OFF;
        if !self.cells_requested {
            return;
        }
        let dataset = self.lod_session.as_ref().map(|session| session.dataset());
        let device_ok = self
            .gpu_renderer
            .as_ref()
            .is_none_or(viewer_render_wgpu::SurfaceRenderer::cells_supported);
        let pool_pages = self
            .gpu_renderer
            .as_ref()
            .and_then(viewer_render_wgpu::SurfaceRenderer::cell_pool_pages);
        match dataset {
            Some(dataset) if dataset.supports_cells() && device_ok => {
                self.cells_local = Some(crate::cells_glue::InProcessCells::new(
                    dataset,
                    wasm_update_config(pool_pages),
                ));
                self.cell_path = PATH_ENABLED;
            }
            _ => self.cell_path = PATH_UNSUPPORTED,
        }
    }

    /// Stats of whichever cells side this viewer runs.
    pub(super) fn cells_stats(&self) -> Option<CellsStats> {
        let renderer = self.gpu_renderer.as_ref()?;
        if let (Some(cells), Some(session)) = (self.cells_local.as_ref(), self.lod_session.as_ref())
        {
            return Some(cells.stats(session.dataset(), renderer));
        }
        self.cells_remote
            .as_ref()
            .map(|cells| cells.stats(renderer))
    }

    /// The driver frame of the remote cells path, before the renderer draws.
    pub(super) fn run_remote_cells_frame(&mut self) {
        let (Some(cells), Some(renderer)) =
            (self.cells_remote.as_mut(), self.gpu_renderer.as_mut())
        else {
            return;
        };
        let view = renderer.view_projection();
        match cells.frame(renderer, &view) {
            Ok(_) => self.last_lod_error = None,
            Err(message) => self.last_lod_error = Some(message),
        }
    }

    /// R3-Q23: only plain RGB runs on cells; any other mode takes the CPU path and RGB brings
    /// the cells back (nodes kept).
    pub(super) fn set_remote_cells_rgb(&mut self, rgb: bool) {
        if let Some(cells) = self.cells_remote.as_mut() {
            cells.set_active(rgb);
        }
    }
}

#[wasm_bindgen]
impl WasmVoxelViewer {
    /// Opt in to the cells path; call before opening a dataset (or `beginRemoteCells`).
    #[wasm_bindgen(js_name = enableCells)]
    pub fn enable_cells(&mut self, enabled: bool) {
        self.cells_requested = enabled;
    }

    /// Keeps intensity, elevation and classification on the cells path (shader modes, R3-Q32)
    /// instead of the CPU path of R3-Q23. Off by default until the CPU fallback goes.
    #[wasm_bindgen(js_name = enableCellShaderModes)]
    pub fn enable_cell_shader_modes(&mut self, enabled: bool) {
        self.cells_shader_modes = enabled;
    }

    /// Pages of the GPU cell pool this device runs (65,536 for 512 MiB, fewer when its limits
    /// admit less, never below 32,768); 0 when only the CPU path is available.
    #[wasm_bindgen(js_name = cellPoolPages)]
    pub fn cell_pool_pages(&self) -> u32 {
        self.gpu_renderer
            .as_ref()
            .and_then(viewer_render_wgpu::SurfaceRenderer::cell_pool_pages)
            .unwrap_or(0)
    }

    /// `"off"`, `"enabled"` or `"CellPathUnsupported"`.
    #[wasm_bindgen(js_name = cellPath)]
    pub fn cell_path(&self) -> String {
        self.cell_path.to_owned()
    }

    /// Render worker of a build-worker setup: creates the driver for the dataset described by
    /// the builder's `cellsInfo` (`origin`, `rootMin`, `rootSide`). Returns the `cellPath`. The
    /// first `takeCellMessages` carries `driverReplaced`.
    #[wasm_bindgen(js_name = beginRemoteCells)]
    pub fn begin_remote_cells(
        &mut self,
        origin: js_sys::Float64Array,
        root_min: js_sys::Float64Array,
        root_side: f64,
    ) -> Result<String, JsValue> {
        // Arguments and renderer are checked before any state changes: an error leaves the
        // previous cells path as it was.
        if self.cells_requested {
            if origin.length() != 3 || root_min.length() != 3 || !root_side.is_finite() {
                return Err(JsValue::from_str(
                    "origin and rootMin need 3 numbers and rootSide must be finite",
                ));
            }
            if self.gpu_renderer.is_none() {
                return Err(JsValue::from_str("GPU renderer has not been initialized"));
            }
        }
        self.cells_local = None;
        self.cells_remote = None;
        self.cell_path = PATH_OFF;
        if !self.cells_requested {
            return Ok(PATH_OFF.to_owned());
        }
        if !self
            .gpu_renderer
            .as_ref()
            .is_some_and(viewer_render_wgpu::SurfaceRenderer::cells_supported)
        {
            self.cell_path = PATH_UNSUPPORTED;
            return Ok(PATH_UNSUPPORTED.to_owned());
        }
        let (mut origin_values, mut root_min_values) = ([0.0; 3], [0.0; 3]);
        origin.copy_to(&mut origin_values);
        root_min.copy_to(&mut root_min_values);
        let mut config = driver_config(
            CellGrid {
                root_min: root_min_values,
                root_side,
            },
            origin_values,
            WASM_CPU_BUDGET_BYTES,
        );
        if let Some(pages) = self
            .gpu_renderer
            .as_ref()
            .and_then(viewer_render_wgpu::SurfaceRenderer::cell_pool_pages)
        {
            config.pool.capacity_pages = pages;
        }
        let mut cells = ViewerCells::new(config, self.cell_node_cap);
        cells.camera_changed(self.view_epoch);
        self.cells_remote = Some(cells);
        self.cell_path = PATH_ENABLED;
        Ok(PATH_ENABLED.to_owned())
    }

    /// Bytes of `nodeCells` applied per frame (at least one node); takes effect on the next
    /// `beginRemoteCells`.
    #[wasm_bindgen(js_name = setCellNodeBytesPerFrame)]
    pub fn set_cell_node_bytes_per_frame(&mut self, bytes: u32) {
        self.cell_node_cap = (bytes as usize).max(1);
    }

    /// One `nodeCells` entry: the codec bytes of a node, queued and applied in order under the
    /// byte cap.
    #[wasm_bindgen(js_name = addCellNode)]
    pub fn add_cell_node(&mut self, generation: f64, bytes: Uint8Array) -> Result<(), JsValue> {
        let generation = counter(generation, "generation")?;
        let cells = self
            .cells_remote
            .as_mut()
            .ok_or_else(|| JsValue::from_str("the cells path is not enabled"))?;
        cells
            .push_node(generation, bytes.to_vec())
            .map_err(|message| JsValue::from_str(&message))
    }

    /// One `cellCut` message: `{ cutEpoch, viewEpoch, cut, support, idle, targetPixels }`. The
    /// last wins.
    #[wasm_bindgen(js_name = setCellCut)]
    pub fn set_cell_cut(&mut self, message: JsValue) -> Result<(), JsValue> {
        let epoch = counter(number_field(&message, "cutEpoch")?, "cutEpoch")?;
        let view_epoch = counter(number_field(&message, "viewEpoch")?, "viewEpoch")?;
        let cut = ids_from_js(&field(&message, "cut")?, "cut")?;
        let support = ids_from_js(&field(&message, "support")?, "support")?;
        let idle = field(&message, "idle")?.is_truthy();
        let target_pixels = number_field(&message, "targetPixels")?;
        if !TARGET_PIXELS_RANGE.contains(&target_pixels) {
            return Err(JsValue::from_str(
                "targetPixels must be finite and between 0.5 and 16",
            ));
        }
        let cells = self
            .cells_remote
            .as_mut()
            .ok_or_else(|| JsValue::from_str("the cells path is not enabled"))?;
        cells.push_cut(epoch, view_epoch, cut, support, idle, target_pixels);
        Ok(())
    }

    /// What the render worker owes the build worker since the last call.
    #[wasm_bindgen(js_name = takeCellMessages)]
    pub fn take_cell_messages(&mut self) -> Result<JsValue, JsValue> {
        let cells = self
            .cells_remote
            .as_mut()
            .ok_or_else(|| JsValue::from_str("the cells path is not enabled"))?;
        let messages = cells.take_messages();
        let result = Object::new();
        set(
            &result,
            "poolAck",
            &match &messages.ack {
                Some(ack) => ack_to_js(ack)?,
                None => JsValue::NULL,
            },
        )?;
        set(&result, "evict", &ids_to_js(&messages.evict))?;
        set(
            &result,
            "poolReset",
            &JsValue::from_bool(messages.pool_reset),
        )?;
        set(
            &result,
            "driverReplaced",
            &JsValue::from_bool(messages.driver_replaced),
        )?;
        Ok(result.into())
    }

    /// Stats of the cells path of this viewer, or `null` while it is off.
    #[wasm_bindgen(js_name = cellStats)]
    pub fn cell_stats(&self) -> Result<JsValue, JsValue> {
        let Some(stats) = self.cells_stats() else {
            return Ok(JsValue::NULL);
        };
        cells_stats_object(self.cell_path, &stats)
    }
}
