use std::sync::{Arc, Mutex};

use viewer_core::BlockId;
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

use crate::{
    LodViewStats, ViewerConfig, ViewerController,
    browser_io::{BrowserBlobReader, BrowserReadStats},
    lod_session::LodSession,
};
use js_marshal::viewer_error_to_js;
use viewer_format::FormatReader;
use viewer_render_wgpu::{RendererConfig, SurfaceRenderer, VoxelBuildLimits};

mod camera;
mod js_marshal;
mod js_stats;
mod lod;
mod points;
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
    last_lod_error: Option<String>,
}

struct AsyncFormatContext {
    has_attributes: bool,
    attribute_ranges: Vec<Vec<Option<[f64; 2]>>>,
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
            last_lod_error: None,
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
        self.lod_dirty = self.lod_session.is_some();
    }

    #[wasm_bindgen(js_name = clear)]
    pub fn clear(&mut self) {
        self.inner.clear();
        self.gpu_scene_dirty = true;
        self.clear_lod_dataset();
    }
}

impl WasmVoxelViewer {
    pub(super) async fn create_offscreen_with_config(
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
