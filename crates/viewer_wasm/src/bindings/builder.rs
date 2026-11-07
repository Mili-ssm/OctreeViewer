use std::sync::{Arc, Mutex};

use viewer_core::BlockId;
use wasm_bindgen::prelude::*;
use web_sys::Blob;

use super::{
    AsyncFormatContext, attribute_availability,
    cells::{PATH_OFF, builder_output_to_js},
    js_stats::{
        accumulate_browser_read_delta, reset_browser_read_stats, snapshot_browser_read_stats,
    },
    lod::{
        graft_page, insert_async_block, lod_update_stats_to_js, open_descriptor, open_format,
        open_paged, read_page_insert,
    },
    selected_range_to_js,
};
use crate::{
    WasmAsyncFormatReader, WasmPageGraft,
    browser_io::{BrowserBlobReader, BrowserReadStats},
    cells_glue::BuilderCells,
    lod_driver::resolve_color_selection,
    lod_session::LodSession,
    scene_builder::{BuilderStep, BuilderUpdate, SceneBuilderCore, camera_from_transport},
};
use viewer_format::FormatReader;
use viewer_render_wgpu::{ColorMode, VoxelBuildLimits};

type FormatSession = LodSession<FormatReader<BrowserBlobReader>>;

/// LOD scene builder without a GPU, canvas or renderer, for a dedicated build worker.
///
/// It owns the dataset, block cache, planner and voxel builder. The host sends the camera with
/// every `update` and receives the instance bytes as a transferable `ArrayBuffer`; a separate
/// render worker uploads and draws them.
#[wasm_bindgen]
pub struct WasmSceneBuilder {
    pub(super) core: Option<SceneBuilderCore<FormatReader<BrowserBlobReader>>>,
    limits: VoxelBuildLimits,
    async_format_context: Option<AsyncFormatContext>,
    pending_async_blocks: Vec<BlockId>,
    browser_read_stats: Arc<Mutex<BrowserReadStats>>,
    range_read_stats: BrowserReadStats,
    /// The host opted in to the cells path (`enableCells`).
    pub(super) cells_requested: bool,
    pub(super) cells: Option<BuilderCells>,
    /// `"off"`, `"enabled"` or `"CellPathUnsupported"` for the open dataset.
    pub(super) cell_path: &'static str,
}

#[wasm_bindgen]
impl WasmSceneBuilder {
    /// The limits bound estimated construction memory and voxel count, not process RSS. Pass
    /// the render worker's effective limits, already capped by its GPU instance budget.
    #[wasm_bindgen(constructor)]
    pub fn new(max_voxels: u32, max_estimated_bytes: u32) -> Result<WasmSceneBuilder, JsValue> {
        if max_voxels == 0 || max_estimated_bytes == 0 {
            return Err(JsValue::from_str(
                "voxel construction limits must be positive",
            ));
        }
        Ok(Self {
            core: None,
            limits: VoxelBuildLimits {
                max_voxels: max_voxels as usize,
                max_estimated_bytes: u64::from(max_estimated_bytes),
            },
            async_format_context: None,
            pending_async_blocks: Vec::new(),
            browser_read_stats: Arc::new(Mutex::new(BrowserReadStats::default())),
            range_read_stats: BrowserReadStats::default(),
            cells_requested: false,
            cells: None,
            cell_path: PATH_OFF,
        })
    }

    /// Opens hierarchy metadata whose blocks the host delivers with `insertAsyncLodBlock`.
    #[wasm_bindgen(js_name = openLodDataset)]
    pub fn open_lod_dataset(&mut self, descriptor: JsValue) -> Result<JsValue, JsValue> {
        let (dataset, async_format_context) = open_descriptor(descriptor)?;
        let visual_scale = dataset.visual_scale();
        self.install(LodSession::External(dataset), async_format_context, None);
        let result = js_sys::Object::new();
        js_sys::Reflect::set(
            &result,
            &"status".into(),
            &JsValue::from_str("metadata_ready"),
        )?;
        js_sys::Reflect::set(
            &result,
            &"visualScale".into(),
            &JsValue::from_f64(visual_scale),
        )?;
        self.add_open_fields(&result)?;
        Ok(result.into())
    }

    /// Opens a paged format: the reader's root page is resident, other pages arrive with
    /// `graftPage` (see `lod.pendingPageAnchors` in `update`).
    #[wasm_bindgen(js_name = openPagedLodDataset)]
    pub fn open_paged_lod_dataset(
        &mut self,
        reader: &WasmAsyncFormatReader,
        cache_bytes: u32,
    ) -> Result<JsValue, JsValue> {
        let (dataset, async_format_context) = open_paged(reader, cache_bytes)?;
        let visual_scale = dataset.visual_scale();
        self.install(LodSession::External(dataset), async_format_context, None);
        let result = js_sys::Object::new();
        js_sys::Reflect::set(
            &result,
            &"status".into(),
            &JsValue::from_str("metadata_ready"),
        )?;
        js_sys::Reflect::set(
            &result,
            &"visualScale".into(),
            &JsValue::from_f64(visual_scale),
        )?;
        self.add_open_fields(&result)?;
        Ok(result.into())
    }

    /// Grafts a page returned by `WasmAsyncFormatReader.loadPage`.
    #[wasm_bindgen(js_name = graftPage)]
    pub fn graft_page(&mut self, page: WasmPageGraft) -> Result<(), JsValue> {
        let core = self.core_mut()?;
        graft_page(core.session_mut().dataset_mut(), page)?;
        core.block_inserted();
        if let Some(cells) = self.cells.as_mut() {
            cells.block_inserted();
        }
        Ok(())
    }

    /// Opens the random-access format from a Blob; the builder reads blocks itself.
    #[wasm_bindgen(js_name = openFormatDataset)]
    pub fn open_format_dataset(
        &mut self,
        file: Blob,
        cache_bytes: u32,
        attributes: Option<Blob>,
    ) -> Result<JsValue, JsValue> {
        let (session, browser_read_stats, result) = open_format(file, cache_bytes, attributes)?;
        let visual_scale = session.dataset().visual_scale();
        js_sys::Reflect::set(
            &result,
            &"visualScale".into(),
            &JsValue::from_f64(visual_scale),
        )?;
        self.install(session, None, Some(browser_read_stats));
        self.add_open_fields(&result)?;
        Ok(result)
    }

    /// Inserts one block delivered by the host; same contract as `WasmVoxelViewer`.
    #[wasm_bindgen(js_name = insertAsyncLodBlock)]
    pub fn insert_async_lod_block(
        &mut self,
        block_id: String,
        positions: js_sys::Float32Array,
        colors: Option<js_sys::Uint8Array>,
        attribute: Option<JsValue>,
    ) -> Result<usize, JsValue> {
        let core = self.core_mut()?;
        let selection = core.selected_attribute();
        let accounted_bytes = insert_async_block(
            Some(core.session_mut()),
            selection,
            block_id,
            positions,
            colors,
            attribute,
        )?;
        core.block_inserted();
        if let Some(cells) = self.cells.as_mut() {
            cells.block_inserted();
        }
        self.pending_async_blocks.clear();
        Ok(accounted_bytes)
    }

    /// Selects `rgb`, `intensity`, `elevation`, `classification` or `attribute:index:component`.
    #[wasm_bindgen(js_name = setColorMode)]
    pub fn set_color_mode(&mut self, mode: String) -> Result<(), JsValue> {
        let core = self
            .core
            .as_mut()
            .ok_or_else(|| JsValue::from_str("no LOD dataset is open"))?;
        let selection = {
            let session: &FormatSession = core.session();
            let async_context = self.async_format_context.as_ref();
            resolve_color_selection(
                &mode,
                Some(session.dataset().attribute_schema()),
                |index, component| {
                    attribute_availability(Some(session), async_context, index, component)
                },
            )
            .map_err(|message| JsValue::from_str(&message))?
        };
        if core.set_color_selection(selection) {
            reset_browser_read_stats(&self.browser_read_stats);
            self.range_read_stats = BrowserReadStats::default();
        }
        // R3-Q23: cells are RGB only; any other mode sends scenes again, RGB brings cells back.
        if let Some(cells) = self.cells.as_mut() {
            let rgb = selection.mode == ColorMode::Rgb && selection.attribute.is_none();
            cells.set_active(core.session_mut().dataset_mut(), rgb);
        }
        Ok(())
    }

    /// `[min, max]` the selected attribute is normalised with, or `null` when no attribute (or
    /// no range for it) is selected. Lets the render worker show the same legend as the viewer.
    #[wasm_bindgen(js_name = attributeRange)]
    pub fn attribute_range(&self) -> JsValue {
        selected_range_to_js(
            self.core
                .as_ref()
                .and_then(|core| core.selected_attribute().1),
        )
    }

    /// Missing blocks in the order the LOD plan wants them, as decimal id strings.
    #[wasm_bindgen(js_name = pendingAsyncLodBlocks)]
    pub fn pending_async_lod_blocks(&self) -> js_sys::Array {
        self.pending_async_blocks
            .iter()
            .map(|id| JsValue::from_str(&id.get().to_string()))
            .collect()
    }

    /// Selects the cut for a camera and rebuilds the scene when it changed.
    ///
    /// `camera` is `OrbitCamera.to_f64_array()` (11 numbers). Returns
    /// `{ epoch, sceneBuffer, instanceCount, blockIds, digest, needsRefinement, lod }` where
    /// `sceneBuffer` holds the `InstanceRaw` bytes in a fresh, transferable `ArrayBuffer`, or is
    /// `null` when the previous scene still applies.
    #[wasm_bindgen(js_name = update)]
    pub fn update(
        &mut self,
        camera: js_sys::Float64Array,
        viewport_width: u32,
        viewport_height: u32,
        visual_scale: f64,
        target_voxel_pixels: f64,
    ) -> Result<JsValue, JsValue> {
        let mut camera_values = vec![0.0; camera.length() as usize];
        camera.copy_to(&mut camera_values);
        let camera = camera_from_transport(&camera_values).map_err(|m| JsValue::from_str(&m))?;
        let core = self
            .core
            .as_mut()
            .ok_or_else(|| JsValue::from_str("no LOD dataset is open"))?;
        let mode = core.color_mode();
        let (selected_attribute, selected_range) = core.selected_attribute();
        let browser_read_stats = Arc::clone(&self.browser_read_stats);
        let mut range_read_stats = self.range_read_stats;
        let update = core.update_with_cells(
            self.cells.as_mut(),
            camera,
            [viewport_width, viewport_height],
            visual_scale,
            target_voxel_pixels,
            |reader: &mut FormatReader<BrowserBlobReader>, block_id| {
                let before = snapshot_browser_read_stats(&browser_read_stats);
                let loaded = crate::block_loading::load_render_block(
                    reader,
                    block_id,
                    Some(mode),
                    selected_attribute,
                    selected_range,
                )
                .map(Some);
                let after = snapshot_browser_read_stats(&browser_read_stats);
                accumulate_browser_read_delta(&mut range_read_stats, before, after);
                loaded
            },
            read_page_insert,
        );
        self.range_read_stats = range_read_stats;
        let update = match update.map_err(|message| JsValue::from_str(&message))? {
            BuilderStep::Cells(output) => {
                // Cells mode never emits a scene: the render worker would drop its pool.
                self.pending_async_blocks = output.async_priority_order.clone();
                let lod = self.cells_lod_stats(&output.stats)?;
                let needs_refinement = self.cells.as_ref().is_some_and(BuilderCells::is_dirty);
                return builder_output_to_js(&output, needs_refinement, &lod);
            }
            BuilderStep::Scene(update) => update,
        };
        let BuilderUpdate {
            epoch,
            scene,
            stats,
            async_priority_order,
            needs_refinement,
        } = update;
        self.pending_async_blocks = async_priority_order;

        let dataset = core.session().dataset();
        let lod = lod_update_stats_to_js(
            &stats,
            range_read_stats,
            Some(dataset),
            None,
            None,
            target_voxel_pixels,
        )?;
        let result = js_sys::Object::new();
        let set = |name: &str, value: &JsValue| js_sys::Reflect::set(&result, &name.into(), value);
        set("epoch", &JsValue::from_f64(epoch as f64))?;
        set("needsRefinement", &JsValue::from_bool(needs_refinement))?;
        set("lod", &lod)?;
        match scene {
            Some(built) => {
                let instances = built.voxels.scene.instances();
                // `Uint8Array::from` copies into JS memory, so the buffer is not a view of the
                // wasm heap and can be transferred to another worker.
                let bytes = js_sys::Uint8Array::from(bytemuck::cast_slice::<_, u8>(instances));
                let block_ids = built
                    .block_ids
                    .iter()
                    .map(|id| JsValue::from_str(&id.get().to_string()))
                    .collect::<js_sys::Array>();
                set("sceneBuffer", &bytes.buffer())?;
                set("instanceCount", &JsValue::from_f64(instances.len() as f64))?;
                set("blockIds", &block_ids)?;
                set(
                    "digest",
                    &JsValue::from_str(&format!("{:016x}", built.scene_digest())),
                )?;
            }
            None => {
                set("sceneBuffer", &JsValue::NULL)?;
                set("instanceCount", &JsValue::from_f64(0.0))?;
                set("blockIds", &js_sys::Array::new())?;
                set("digest", &JsValue::NULL)?;
            }
        }
        Ok(result.into())
    }
}

impl WasmSceneBuilder {
    fn core_mut(
        &mut self,
    ) -> Result<&mut SceneBuilderCore<FormatReader<BrowserBlobReader>>, JsValue> {
        self.core
            .as_mut()
            .ok_or_else(|| JsValue::from_str("no LOD dataset is open"))
    }

    fn install(
        &mut self,
        session: FormatSession,
        async_format_context: Option<AsyncFormatContext>,
        browser_read_stats: Option<Arc<Mutex<BrowserReadStats>>>,
    ) {
        let host_delivers_blocks = async_format_context.is_some();
        self.core = Some(SceneBuilderCore::new(
            session,
            host_delivers_blocks,
            self.limits,
        ));
        self.install_cells();
        self.async_format_context = async_format_context;
        self.pending_async_blocks.clear();
        self.browser_read_stats =
            browser_read_stats.unwrap_or_else(|| Arc::new(Mutex::new(BrowserReadStats::default())));
        self.range_read_stats = BrowserReadStats::default();
    }
}
