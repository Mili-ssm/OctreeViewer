use std::sync::{Arc, Mutex};

use viewer_core::BlockId;
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

use crate::{
    LodViewStats, ViewerConfig, ViewerController,
    browser_io::{BrowserBlobReader, BrowserReadStats},
    cells_glue::{Continuity, DEFAULT_NODE_BYTES_PER_FRAME, InProcessCells, ViewerCells},
    lod_driver::AttributeAvailability,
    lod_session::LodSession,
};
pub use builder::WasmSceneBuilder;
use js_marshal::viewer_error_to_js;
use viewer_core::SpatialBounds;
use viewer_format::FormatReader;
use viewer_render_wgpu::{RendererConfig, SurfaceRenderer, VoxelBuildLimits};

mod builder;
mod camera;
mod cells;
mod js_marshal;
mod js_stats;
mod lod;
mod points;
#[cfg(feature = "test-reference-spacing")]
#[path = "../../tests/support/reference_spacing_probe.rs"]
mod reference_spacing_probe;
mod render;

#[wasm_bindgen]
pub struct WasmVoxelViewer {
    inner: ViewerController,
    gpu_renderer: Option<SurfaceRenderer<'static>>,
    gpu_scene_dirty: bool,
    lod_session: Option<LodSession<FormatReader<BrowserBlobReader>>>,
    async_format_context: Option<AsyncFormatContext>,
    pending_async_block: Option<BlockId>,
    pending_async_blocks: Vec<BlockId>,
    browser_read_stats: Arc<Mutex<BrowserReadStats>>,
    lod_range_read_stats: BrowserReadStats,
    selected_attribute: Option<(u32, u32)>,
    selected_attribute_range: Option<[f64; 2]>,
    lod_config: Option<f64>,
    lod_stage_pixels: Option<f64>,
    lod_dirty: bool,
    last_lod_stats: Option<(LodViewStats, BrowserReadStats)>,
    /// Plan stats and target of the last in-process cells update, for `lodStats` (that path has no
    /// `LodViewStats`).
    last_cells_plan: Option<(crate::CellPlanStats, f64)>,
    /// Digest of the last scene uploaded to the renderer, exposed as `lodStats().sceneDigest`.
    last_scene_digest: Option<u64>,
    last_lod_error: Option<String>,
    /// Root bounds and origin of a dataset whose hierarchy lives in a build worker, so `fitScene`
    /// can frame it without a local `LodDataset`.
    remote_dataset_frame: Option<RemoteDatasetFrame>,
    /// The host opted in to the cells path (`enableCells`); the dataset may still not support it.
    cells_requested: bool,
    /// `"off"`, `"enabled"` or `"CellPathUnsupported"` for the open dataset.
    cell_path: &'static str,
    /// Cells with the dataset in this worker: the three steps run back to back.
    cells_local: Option<InProcessCells>,
    /// Cells whose dataset lives in a build worker: messages arrive through `addCellNode` and
    /// `setCellCut`.
    cells_remote: Option<ViewerCells>,
    cell_node_cap: usize,
    /// RF-05 continuity of the open cloud (off by default); `setContinuity`.
    continuity: Continuity,
    /// Counts camera moves and resizes (`viewEpoch`): the builder echoes the epoch of the view it
    /// planned for, so the cut at rest is the one of the latest view.
    view_epoch: u64,
}

struct RemoteDatasetFrame {
    bounds: SpatialBounds,
    origin: [f64; 3],
}

pub(super) struct AsyncFormatContext {
    has_attributes: bool,
    attribute_ranges: Vec<Vec<Option<[f64; 2]>>>,
}

/// `[min, max]` as a JS array, or `null` when the selected attribute has no range.
pub(super) fn selected_range_to_js(range: Option<[f64; 2]>) -> JsValue {
    range.map_or(JsValue::NULL, |[min, max]| {
        js_sys::Array::of2(&JsValue::from_f64(min), &JsValue::from_f64(max)).into()
    })
}

/// Whether the open dataset can paint `(index, component)`, and the range to normalise it with.
/// The reader answers for a dataset opened from a Blob, the host-provided context for one
/// whose blocks arrive asynchronously.
pub(super) fn attribute_availability(
    session: Option<&LodSession<FormatReader<BrowserBlobReader>>>,
    async_context: Option<&AsyncFormatContext>,
    index: u32,
    component: u32,
) -> Result<Option<AttributeAvailability>, String> {
    if let Some(reader) = session.and_then(LodSession::reader) {
        let range = reader
            .attribute_range(index, component)
            .map_err(|error| error.to_string())?;
        return Ok(Some(AttributeAvailability {
            has_attributes: reader.has_attributes(),
            range,
        }));
    }
    Ok(async_context.map(|context| AttributeAvailability {
        has_attributes: context.has_attributes,
        range: context
            .attribute_ranges
            .get(index as usize)
            .and_then(|components| components.get(component as usize))
            .copied()
            .flatten(),
    }))
}

/// The range intensity is painted with when an attribute mode meets a block that omits the
/// selected column (v8 pre-revision LOD blocks): intensity's global range. `None` when no attribute is
/// selected or the file has no intensity range.
pub(super) fn intensity_fallback_range(
    selected_attribute: Option<(u32, u32)>,
    session: Option<&LodSession<FormatReader<BrowserBlobReader>>>,
    async_context: Option<&AsyncFormatContext>,
) -> Result<Option<[f64; 2]>, String> {
    let Some(schema) = selected_attribute
        .and(session)
        .map(|session| session.dataset().attribute_schema())
    else {
        return Ok(None);
    };
    let Some(intensity) = crate::block_loading::intensity_index(schema) else {
        return Ok(None);
    };
    Ok(
        attribute_availability(session, async_context, intensity, 0)?
            .and_then(|availability| availability.range),
    )
}

#[wasm_bindgen]
impl WasmVoxelViewer {
    #[wasm_bindgen(constructor)]
    pub fn new(voxel_size: f32) -> Result<WasmVoxelViewer, JsValue> {
        let inner = ViewerController::new(ViewerConfig {
            voxel_size,
            ..ViewerConfig::default()
        })
        .map_err(viewer_error_to_js)?;

        Ok(Self {
            inner,
            gpu_renderer: None,
            gpu_scene_dirty: true,
            lod_session: None,
            async_format_context: None,
            pending_async_block: None,
            pending_async_blocks: Vec::new(),
            browser_read_stats: Arc::new(Mutex::new(BrowserReadStats::default())),
            lod_range_read_stats: BrowserReadStats::default(),
            selected_attribute: None,
            selected_attribute_range: None,
            lod_config: None,
            lod_stage_pixels: None,
            lod_dirty: false,
            last_lod_stats: None,
            last_cells_plan: None,
            last_scene_digest: None,
            last_lod_error: None,
            remote_dataset_frame: None,
            cells_requested: false,
            cell_path: cells::PATH_OFF,
            cells_local: None,
            cells_remote: None,
            cell_node_cap: DEFAULT_NODE_BYTES_PER_FRAME,
            continuity: Continuity::default(),
            view_epoch: 0,
        })
    }

    /// Creates a viewer that renders directly to the host-owned canvas.
    #[wasm_bindgen]
    pub async fn create(
        canvas: HtmlCanvasElement,
        voxel_size: f32,
    ) -> Result<WasmVoxelViewer, JsValue> {
        let mut viewer = Self::new(voxel_size)?;
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let renderer = SurfaceRenderer::new(
            &instance,
            surface,
            RendererConfig::default(),
            canvas.width(),
            canvas.height(),
        )
        .await
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
        viewer.gpu_renderer = Some(renderer);
        Ok(viewer)
    }

    /// Creates a renderer for a host-transferred canvas in a dedicated Web Worker.
    #[wasm_bindgen(js_name = createOffscreen)]
    pub async fn create_offscreen(
        canvas: web_sys::OffscreenCanvas,
        voxel_size: f32,
    ) -> Result<WasmVoxelViewer, JsValue> {
        Self::create_offscreen_with_config(canvas, voxel_size, RendererConfig::default()).await
    }

    /// Creates a worker renderer with explicit CPU voxel-construction limits.
    /// The limits bound estimated construction memory and voxel count, not process RSS or VRAM.
    #[wasm_bindgen(js_name = createOffscreenWithVoxelLimits)]
    pub async fn create_offscreen_with_voxel_limits(
        canvas: web_sys::OffscreenCanvas,
        voxel_size: f32,
        max_voxels: u32,
        max_estimated_bytes: u32,
    ) -> Result<WasmVoxelViewer, JsValue> {
        if max_voxels == 0 || max_estimated_bytes == 0 {
            return Err(JsValue::from_str(
                "voxel construction limits must be positive",
            ));
        }
        let config = RendererConfig {
            voxel_build_limits: VoxelBuildLimits {
                max_voxels: max_voxels as usize,
                max_estimated_bytes: u64::from(max_estimated_bytes),
            },
            ..RendererConfig::default()
        };
        Self::create_offscreen_with_config(canvas, voxel_size, config).await
    }

    #[wasm_bindgen(js_name = resize)]
    pub fn resize(&mut self, width: u32, height: u32) {
        self.inner.resize(width, height);
        if let Some(renderer) = &mut self.gpu_renderer {
            renderer.resize(width, height);
        }
        self.camera_changed(true);
        self.lod_dirty = self.lod_session.is_some();
    }

    #[wasm_bindgen(js_name = clear)]
    pub fn clear(&mut self) {
        self.inner.clear();
        self.gpu_scene_dirty = true;
        self.remote_dataset_frame = None;
        self.clear_lod_dataset();
    }
}

impl WasmVoxelViewer {
    async fn create_offscreen_with_config(
        canvas: web_sys::OffscreenCanvas,
        voxel_size: f32,
        config: RendererConfig,
    ) -> Result<WasmVoxelViewer, JsValue> {
        let mut viewer = Self::new(voxel_size)?;
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::OffscreenCanvas(canvas.clone()))
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let renderer =
            SurfaceRenderer::new(&instance, surface, config, canvas.width(), canvas.height())
                .await
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
        viewer.gpu_renderer = Some(renderer);
        Ok(viewer)
    }
}
