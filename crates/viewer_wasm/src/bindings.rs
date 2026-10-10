use serde_wasm_bindgen::to_value;
use viewer_core::{BlockId, ColorRgb, DatasetSource, Point, PointBatch, PointBlock};
use wasm_bindgen::prelude::*;
use web_sys::{Blob, HtmlCanvasElement};

use crate::{
    LodDataset, LodViewStats, RenderInstanceSnapshot, ViewerConfig, ViewerController, ViewerError,
    browser_io::BrowserBlobReader, lod_dataset::next_lod_stage, lod_metadata::LodDatasetDescriptor,
};
use viewer_format::FormatReader;
use viewer_render_wgpu::{ColorMode, RendererConfig, SurfaceRenderer};

const WASM_POINT_BATCH_POINTS: usize = 16_384;
const MAX_WASM_TRANSFER_INSTANCES: usize = 16_384;
#[wasm_bindgen]
pub struct WasmVoxelViewer {
    inner: ViewerController,
    gpu_renderer: Option<SurfaceRenderer<'static>>,
    gpu_scene_dirty: bool,
    lod_dataset: Option<LodDataset>,
    format_reader: Option<FormatReader<BrowserBlobReader>>,
    lod_config: Option<f64>,
    lod_stage_pixels: Option<f64>,
    lod_dirty: bool,
    last_lod_stats: Option<LodViewStats>,
    last_lod_error: Option<String>,
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
            lod_dataset: None,
            format_reader: None,
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
        let mut viewer = Self::new(voxel_size)?;
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::OffscreenCanvas(canvas.clone()))
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

    #[wasm_bindgen(js_name = resize)]
    pub fn resize(&mut self, width: u32, height: u32) {
        self.inner.resize(width, height);
        if let Some(renderer) = &mut self.gpu_renderer {
            renderer.resize(width, height);
        }
        self.lod_dirty = self.lod_dataset.is_some();
    }

    #[wasm_bindgen(js_name = replacePoints)]
    pub fn replace_points(
        &mut self,
        positions: js_sys::Float32Array,
        colors: js_sys::Uint8Array,
    ) -> Result<(), JsValue> {
        let points = points_from_typed_arrays(&positions, &colors, raw_scene_point_limit())?;
        self.inner.replace_points_owned(points);
        self.gpu_scene_dirty = true;
        self.clear_lod_dataset();
        Ok(())
    }

    #[wasm_bindgen(js_name = appendPoints)]
    pub fn append_points(
        &mut self,
        positions: js_sys::Float32Array,
        colors: js_sys::Uint8Array,
    ) -> Result<(), JsValue> {
        let remaining_points = raw_scene_point_limit()
            .saturating_sub(usize::try_from(self.inner.stats().point_count).unwrap_or(usize::MAX));
        let points = points_from_typed_arrays(&positions, &colors, remaining_points)?;
        self.inner
            .append_points_owned(points)
            .map_err(viewer_error_to_js)?;
        self.gpu_scene_dirty = true;
        self.clear_lod_dataset();
        Ok(())
    }

    #[wasm_bindgen(js_name = replaceVoxelizedPoints)]
    pub fn replace_voxelized_points(
        &mut self,
        positions: js_sys::Float32Array,
        colors: js_sys::Uint8Array,
    ) -> Result<(), JsValue> {
        let points = points_from_typed_arrays(&positions, &colors, raw_scene_point_limit())?;
        self.inner
            .replace_voxelized_points_owned(points)
            .map_err(viewer_error_to_js)?;
        self.gpu_scene_dirty = true;
        self.clear_lod_dataset();
        Ok(())
    }

    #[wasm_bindgen(js_name = appendVoxelizedPoints)]
    pub fn append_voxelized_points(
        &mut self,
        positions: js_sys::Float32Array,
        colors: js_sys::Uint8Array,
    ) -> Result<(), JsValue> {
        let remaining_points = raw_scene_point_limit()
            .saturating_sub(usize::try_from(self.inner.stats().point_count).unwrap_or(usize::MAX));
        let points = points_from_typed_arrays(&positions, &colors, remaining_points)?;
        self.inner
            .append_voxelized_points_owned(points)
            .map_err(viewer_error_to_js)?;
        self.gpu_scene_dirty = true;
        self.clear_lod_dataset();
        Ok(())
    }

    #[wasm_bindgen(js_name = clear)]
    pub fn clear(&mut self) {
        self.inner.clear();
        self.gpu_scene_dirty = true;
        self.clear_lod_dataset();
    }

    /// Opens resident hierarchy metadata for decoded block streaming.
    #[wasm_bindgen(js_name = openLodDataset)]
    pub fn open_lod_dataset(&mut self, descriptor: JsValue) -> Result<JsValue, JsValue> {
        let descriptor: LodDatasetDescriptor = serde_wasm_bindgen::from_value(descriptor)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let dataset = descriptor
            .build()
            .map_err(|error| JsValue::from_str(&error))?;
        if let Some(renderer) = &mut self.gpu_renderer {
            renderer.clear_dataset();
        }
        self.lod_dataset = Some(dataset);
        self.format_reader = None;
        self.lod_config = None;
        self.lod_stage_pixels = None;
        self.lod_dirty = false;
        self.last_lod_stats = None;
        self.last_lod_error = None;
        self.gpu_scene_dirty = false;
        let result = js_sys::Object::new();
        js_sys::Reflect::set(
            &result,
            &"status".into(),
            &JsValue::from_str("metadata_ready"),
        )?;
        Ok(result.into())
    }

    /// Opens the project's random-access format from a Blob; call from a dedicated worker.
    #[wasm_bindgen(js_name = openFormatDataset)]
    pub fn open_format_dataset(
        &mut self,
        file: Blob,
        cache_bytes: u32,
    ) -> Result<JsValue, JsValue> {
        let source = BrowserBlobReader::new(file, true)?;
        let reader =
            FormatReader::new(source).map_err(|error| JsValue::from_str(&error.to_string()))?;
        let hierarchy_stats = reader.metadata().hierarchy().stats();
        let visual_scale = reader.format_metadata().visual_scale();
        let metadata = reader.format_metadata().shared_dataset();
        let coordinate_origin = metadata.coordinate_frame().origin();
        let semantics = format!("{:?}", metadata.hierarchy().semantics()).to_lowercase();
        let root_bounds = hierarchy_stats.root_bounds;
        let dataset = LodDataset::from_shared_metadata_and_visual_scale(
            metadata,
            visual_scale,
            cache_bytes as usize,
        );
        if let Some(renderer) = &mut self.gpu_renderer {
            renderer.clear_dataset();
        }
        self.lod_dataset = Some(dataset);
        self.format_reader = Some(reader);
        self.lod_config = None;
        self.lod_stage_pixels = None;
        self.lod_dirty = false;
        self.last_lod_stats = None;
        self.last_lod_error = None;
        self.gpu_scene_dirty = false;

        let result = js_sys::Object::new();
        js_sys::Reflect::set(
            &result,
            &"status".into(),
            &JsValue::from_str("format_ready"),
        )?;
        js_sys::Reflect::set(
            &result,
            &"nodeCount".into(),
            &JsValue::from_f64(hierarchy_stats.node_count as f64),
        )?;
        js_sys::Reflect::set(
            &result,
            &"maxLevel".into(),
            &JsValue::from_f64(f64::from(hierarchy_stats.max_level)),
        )?;
        js_sys::Reflect::set(
            &result,
            &"coordinateOrigin".into(),
            &js_sys::Float64Array::from(coordinate_origin.as_slice()),
        )?;
        js_sys::Reflect::set(
            &result,
            &"boundsMin".into(),
            &js_sys::Float64Array::from(root_bounds.min().as_slice()),
        )?;
        js_sys::Reflect::set(
            &result,
            &"boundsMax".into(),
            &js_sys::Float64Array::from(root_bounds.max().as_slice()),
        )?;
        js_sys::Reflect::set(&result, &"semantics".into(), &JsValue::from_str(&semantics))?;
        if let Some(source_points) = hierarchy_stats.source_point_count {
            js_sys::Reflect::set(
                &result,
                &"sourcePointCount".into(),
                &JsValue::from_str(&source_points.to_string()),
            )?;
        }
        Ok(result.into())
    }

    /// Inserts one decoded RGB block. Position coordinates are local to coordinateOrigin.
    #[wasm_bindgen(js_name = insertLodBlock)]
    pub fn insert_lod_block(
        &mut self,
        block_id: String,
        positions: js_sys::Float32Array,
        colors: js_sys::Uint8Array,
    ) -> Result<usize, JsValue> {
        let block_id = block_id
            .parse::<u64>()
            .map(BlockId::new)
            .map_err(|_| JsValue::from_str("blockId must be an unsigned 64-bit integer string"))?;
        let position_components = positions.length() as usize;
        let color_components = colors.length() as usize;
        if !position_components.is_multiple_of(3) || color_components != position_components {
            return Err(JsValue::from_str(
                "positions and colors must contain three values per point",
            ));
        }
        let point_count = position_components / 3;
        let dataset = self
            .lod_dataset
            .as_ref()
            .ok_or_else(|| JsValue::from_str("no LOD dataset is open"))?;
        if !dataset.references_block(block_id) {
            return Err(JsValue::from_str(
                "blockId is not referenced by the dataset",
            ));
        }
        let budget_bytes = dataset.block_cache_budget_bytes();
        let requested_bytes = point_count
            .checked_mul(std::mem::size_of::<Point>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PointBlock>()))
            .ok_or_else(|| JsValue::from_str("block size exceeds the cache budget"))?;
        if requested_bytes > budget_bytes {
            return Err(JsValue::from_str(&format!(
                "block needs at least {requested_bytes} bytes, cache budget is {budget_bytes}"
            )));
        }

        let points = points_from_typed_arrays(&positions, &colors, point_count)?;
        let allocated_bytes = points
            .capacity()
            .saturating_mul(std::mem::size_of::<Point>())
            .saturating_add(std::mem::size_of::<PointBlock>());
        if allocated_bytes > budget_bytes {
            return Err(JsValue::from_str(
                "decoded block allocation exceeds the cache budget",
            ));
        }
        let dataset = self
            .lod_dataset
            .as_mut()
            .ok_or_else(|| JsValue::from_str("no LOD dataset is open"))?;
        let accounted_bytes = dataset
            .insert_block(PointBlock::new(
                block_id,
                PointBatch::new(points),
                Vec::new(),
            ))
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        self.last_lod_error = None;
        self.lod_dirty = self.lod_config.is_some();
        Ok(accounted_bytes)
    }

    /// Selects a camera-aware voxel cut using the target projected voxel edge in pixels.
    #[wasm_bindgen(js_name = updateLod)]
    pub fn update_lod(&mut self, target_voxel_pixels: f64) -> Result<JsValue, JsValue> {
        if !target_voxel_pixels.is_finite() || !(0.5..=16.0).contains(&target_voxel_pixels) {
            return Err(JsValue::from_str(
                "targetVoxelPixels must be finite and between 0.5 and 16",
            ));
        }
        if self.lod_config != Some(target_voxel_pixels) {
            self.lod_stage_pixels = None;
        }
        self.lod_config = Some(target_voxel_pixels);
        let stats = self.update_lod_view(target_voxel_pixels)?;
        self.last_lod_error = None;
        let value = lod_view_stats_to_js(&stats)?;
        add_runtime_gpu_stats(&value, self.gpu_renderer.as_ref())?;
        js_sys::Reflect::set(
            &value,
            &"lodError".into(),
            &self
                .last_lod_error
                .as_ref()
                .map_or(JsValue::NULL, |message| JsValue::from_str(message)),
        )?;
        js_sys::Reflect::set(
            &value,
            &"targetVoxelPixels".into(),
            &JsValue::from_f64(target_voxel_pixels),
        )?;
        Ok(value)
    }

    #[wasm_bindgen(js_name = lodStats)]
    pub fn lod_stats(&self) -> Result<JsValue, JsValue> {
        match &self.last_lod_stats {
            Some(stats) => {
                let value = lod_view_stats_to_js(stats)?;
                add_runtime_gpu_stats(&value, self.gpu_renderer.as_ref())?;
                js_sys::Reflect::set(
                    &value,
                    &"lodError".into(),
                    &self
                        .last_lod_error
                        .as_ref()
                        .map_or(JsValue::NULL, |message| JsValue::from_str(message)),
                )?;
                Ok(value)
            }
            None => Ok(JsValue::NULL),
        }
    }

    #[wasm_bindgen(js_name = setColorMode)]
    pub fn set_color_mode(&mut self, mode: String) -> Result<(), JsValue> {
        let mode = ColorMode::parse(&mode).ok_or_else(|| {
            JsValue::from_str("color mode must be rgb, intensity, elevation, or classification")
        })?;
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        if renderer.color_mode() != mode {
            renderer.set_color_mode(mode);
            if let Some(dataset) = self.lod_dataset.as_mut() {
                dataset
                    .clear_loaded_blocks(renderer)
                    .map_err(|error| JsValue::from_str(&error.to_string()))?;
                self.lod_stage_pixels = Some(16.0);
                self.lod_dirty = true;
            }
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = setDepthShading)]
    pub fn set_depth_shading(&mut self, enabled: bool) -> Result<(), JsValue> {
        self.gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?
            .set_depth_shading(enabled);
        Ok(())
    }

    #[wasm_bindgen(js_name = stats)]
    pub fn stats(&self) -> Result<JsValue, JsValue> {
        to_value(&self.inner.stats()).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    #[wasm_bindgen(js_name = renderSceneBatch)]
    /// Returns a bounded diagnostic page of the current CPU render scene.
    pub fn render_scene_batch(&self, start: u32, count: u32) -> Result<JsValue, JsValue> {
        if count == 0 || count as usize > MAX_WASM_TRANSFER_INSTANCES {
            return Err(JsValue::from_str(
                "renderSceneBatch count must be between 1 and 16384",
            ));
        }
        let instances = self.inner.render_scene().instances();
        let start = (start as usize).min(instances.len());
        let end = start.saturating_add(count as usize).min(instances.len());
        let scene = instances[start..end]
            .iter()
            .copied()
            .map(RenderInstanceSnapshot::from)
            .collect::<Vec<_>>();
        let result = js_sys::Object::new();
        js_sys::Reflect::set(&result, &"start".into(), &JsValue::from_f64(start as f64))?;
        js_sys::Reflect::set(
            &result,
            &"total".into(),
            &JsValue::from_f64(instances.len() as f64),
        )?;
        js_sys::Reflect::set(
            &result,
            &"instances".into(),
            &to_value(&scene).map_err(|error| JsValue::from_str(&error.to_string()))?,
        )?;
        Ok(result.into())
    }

    #[wasm_bindgen(js_name = renderFrame)]
    pub fn render_frame(&mut self) -> Result<String, JsValue> {
        self.refresh_lod_if_dirty()?;
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        if self.gpu_scene_dirty && self.lod_dataset.is_none() {
            renderer
                .upload_scene(self.inner.render_scene())
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            self.gpu_scene_dirty = false;
        }
        renderer
            .render()
            .map(|status| match status {
                viewer_render_wgpu::FrameStatus::Presented => "presented",
                viewer_render_wgpu::FrameStatus::SkippedTimeout => "skipped_timeout",
                viewer_render_wgpu::FrameStatus::Reconfigured => "reconfigured",
                viewer_render_wgpu::FrameStatus::Suspended => "suspended",
            })
            .map(str::to_owned)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    #[wasm_bindgen(js_name = fitScene)]
    pub fn fit_scene(&mut self) -> Result<bool, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let fitted = if let Some(dataset) = &self.lod_dataset {
            dataset.fit_camera(renderer)
        } else {
            renderer.fit_scene(self.inner.render_scene())
        };
        if fitted && self.lod_dataset.is_some() {
            self.lod_stage_pixels = None;
            self.lod_dirty = self.lod_config.is_some();
        }
        Ok(fitted)
    }

    #[wasm_bindgen(js_name = orbit)]
    pub fn orbit(&mut self, delta_x: f64, delta_y: f64) -> Result<bool, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let changed = renderer.orbit_camera(delta_x, delta_y);
        if changed && self.lod_dataset.is_some() {
            self.lod_stage_pixels = None;
            self.lod_dirty = self.lod_config.is_some();
        }
        Ok(changed)
    }

    #[wasm_bindgen(js_name = pan)]
    pub fn pan(&mut self, delta_x: f64, delta_y: f64) -> Result<bool, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let changed = renderer.pan_camera(delta_x, delta_y);
        if changed && self.lod_dataset.is_some() {
            self.lod_stage_pixels = None;
            self.lod_dirty = self.lod_config.is_some();
        }
        Ok(changed)
    }

    #[wasm_bindgen(js_name = zoom)]
    pub fn zoom(&mut self, delta: f64) -> Result<bool, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let changed = renderer.zoom_camera(delta);
        if changed && self.lod_dataset.is_some() {
            self.lod_stage_pixels = None;
            self.lod_dirty = self.lod_config.is_some();
        }
        Ok(changed)
    }

    /// Returns the camera pose in source dataset coordinates.
    #[wasm_bindgen(js_name = cameraPose)]
    pub fn camera_pose(&self) -> Result<JsValue, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_ref()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let (position, target) = renderer.camera_pose();
        let pose = js_sys::Object::new();
        js_sys::Reflect::set(
            &pose,
            &"position".into(),
            &js_sys::Float64Array::from(position.as_slice()),
        )?;
        js_sys::Reflect::set(
            &pose,
            &"target".into(),
            &js_sys::Float64Array::from(target.as_slice()),
        )?;
        Ok(pose.into())
    }

    /// Sets camera position and orbit target in source dataset coordinates.
    #[wasm_bindgen(js_name = setCameraPose)]
    pub fn set_camera_pose(
        &mut self,
        position: js_sys::Float64Array,
        target: js_sys::Float64Array,
    ) -> Result<bool, JsValue> {
        if position.length() != 3 || target.length() != 3 {
            return Err(JsValue::from_str(
                "camera position and target must have 3 coordinates",
            ));
        }
        let mut position_values = [0.0; 3];
        let mut target_values = [0.0; 3];
        position.copy_to(&mut position_values);
        target.copy_to(&mut target_values);
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let changed = renderer.set_camera_pose(position_values, target_values);
        self.camera_changed(changed);
        Ok(changed)
    }

    /// Moves the orbit center while preserving the camera position.
    #[wasm_bindgen(js_name = setOrbitTarget)]
    pub fn set_orbit_target(&mut self, target: js_sys::Float64Array) -> Result<bool, JsValue> {
        if target.length() != 3 {
            return Err(JsValue::from_str("camera target must have 3 coordinates"));
        }
        let mut target_values = [0.0; 3];
        target.copy_to(&mut target_values);
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let changed = renderer.set_orbit_target(target_values);
        self.camera_changed(changed);
        Ok(changed)
    }

    /// Translates the camera in source dataset coordinates.
    #[wasm_bindgen(js_name = moveCamera)]
    pub fn move_camera(&mut self, x: f64, y: f64, z: f64) -> Result<bool, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let changed = renderer.translate_camera([x, y, z]);
        self.camera_changed(changed);
        Ok(changed)
    }

    /// Points the camera down along the dataset's positive Z up axis.
    #[wasm_bindgen(js_name = setBirdView)]
    pub fn set_bird_view(&mut self) -> Result<(), JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        renderer.set_bird_view();
        self.camera_changed(true);
        Ok(())
    }

    /// Picks the frontmost rendered voxel at a framebuffer pixel. Coordinates and voxel data are
    /// returned in the source dataset's coordinate system; `None` means that the ray hit no voxel.
    #[wasm_bindgen]
    pub async fn pick(&self, x: f64, y: f64) -> Result<JsValue, JsValue> {
        let readback = {
            let renderer = self
                .gpu_renderer
                .as_ref()
                .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
            renderer
                .prepare_pick([x, y])
                .map_err(|error| JsValue::from_str(&error))?
        };
        let Some(readback) = readback else {
            return Ok(JsValue::NULL);
        };
        let buffer = readback.buffer().clone();
        let map_buffer = buffer.clone();
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            map_buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| match result {
                    Ok(()) => {
                        let _ = resolve.call0(&JsValue::UNDEFINED);
                    }
                    Err(error) => {
                        let _ = reject
                            .call1(&JsValue::UNDEFINED, &JsValue::from_str(&error.to_string()));
                    }
                });
        });
        wasm_bindgen_futures::JsFuture::from(promise).await?;
        let mapped = buffer.slice(..).get_mapped_range();
        let hit = self
            .gpu_renderer
            .as_ref()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?
            .resolve_pick(&readback, &mapped);
        drop(mapped);
        buffer.unmap();
        let hit = hit.map_err(|error| JsValue::from_str(&error))?;
        let Some(hit) = hit else {
            return Ok(JsValue::NULL);
        };
        let result = js_sys::Object::new();
        js_sys::Reflect::set(
            &result,
            &"instanceIndex".into(),
            &JsValue::from_f64(hit.instance_index as f64),
        )?;
        js_sys::Reflect::set(
            &result,
            &"position".into(),
            &js_sys::Float64Array::from(hit.position.as_slice()),
        )?;
        js_sys::Reflect::set(
            &result,
            &"voxelCenter".into(),
            &js_sys::Float64Array::from(hit.voxel_center.as_slice()),
        )?;
        js_sys::Reflect::set(
            &result,
            &"voxelSize".into(),
            &JsValue::from_f64(hit.voxel_size),
        )?;
        js_sys::Reflect::set(
            &result,
            &"distance".into(),
            &JsValue::from_f64(hit.distance),
        )?;
        Ok(result.into())
    }

    #[wasm_bindgen(js_name = gpuStats)]
    pub fn gpu_stats(&self) -> Result<JsValue, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_ref()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let stats = js_sys::Object::new();
        let adapter_info = renderer.adapter_info();
        let adapter = js_sys::Object::new();
        js_sys::Reflect::set(
            &adapter,
            &"name".into(),
            &JsValue::from_str(&adapter_info.name),
        )?;
        js_sys::Reflect::set(
            &adapter,
            &"backend".into(),
            &JsValue::from_str(&format!("{:?}", adapter_info.backend)),
        )?;
        js_sys::Reflect::set(
            &adapter,
            &"deviceType".into(),
            &JsValue::from_str(&format!("{:?}", adapter_info.device_type)),
        )?;
        js_sys::Reflect::set(
            &adapter,
            &"driver".into(),
            &JsValue::from_str(&adapter_info.driver),
        )?;
        js_sys::Reflect::set(
            &adapter,
            &"driverInfo".into(),
            &JsValue::from_str(&adapter_info.driver_info),
        )?;
        js_sys::Reflect::set(&stats, &"adapter".into(), &adapter)?;
        js_sys::Reflect::set(
            &stats,
            &"uploadedBytes".into(),
            &JsValue::from_str(&renderer.uploaded_bytes().to_string()),
        )?;
        js_sys::Reflect::set(
            &stats,
            &"visibleInstances".into(),
            &JsValue::from_f64(renderer.visible_instance_count() as f64),
        )?;
        js_sys::Reflect::set(
            &stats,
            &"visibleBlocks".into(),
            &JsValue::from_f64(renderer.visible_block_count() as f64),
        )?;
        js_sys::Reflect::set(
            &stats,
            &"hzbActive".into(),
            &JsValue::from_bool(renderer.hzb_active()),
        )?;
        Ok(stats.into())
    }

    #[wasm_bindgen(js_name = viewportSize)]
    pub fn viewport_size(&self) -> Result<js_sys::Uint32Array, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_ref()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let [width, height] = renderer.viewport_size();
        Ok(js_sys::Uint32Array::from([width, height].as_slice()))
    }
}

impl WasmVoxelViewer {
    fn camera_changed(&mut self, changed: bool) {
        if changed && self.lod_dataset.is_some() {
            self.lod_stage_pixels = None;
            self.lod_dirty = self.lod_config.is_some();
        }
    }

    fn refresh_lod_if_dirty(&mut self) -> Result<(), JsValue> {
        if !self.lod_dirty {
            return Ok(());
        }
        let Some(target_voxel_pixels) = self.lod_config else {
            return Ok(());
        };
        self.lod_dirty = false;
        match self.update_lod_view(target_voxel_pixels) {
            Ok(_) => {
                self.last_lod_error = None;
                Ok(())
            }
            Err(error) => {
                self.last_lod_error = Some(
                    error
                        .as_string()
                        .unwrap_or_else(|| "LOD refinement failed".to_owned()),
                );
                self.lod_dirty = false;
                Ok(())
            }
        }
    }

    fn update_lod_view(&mut self, target_voxel_pixels: f64) -> Result<LodViewStats, JsValue> {
        let stage_pixels = self.lod_stage_pixels.unwrap_or(16.0);
        let selection_error_pixels = stage_pixels;
        let requested_attribute = self
            .gpu_renderer
            .as_ref()
            .map(SurfaceRenderer::color_mode)
            .and_then(color_attribute_name);
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let dataset = self
            .lod_dataset
            .as_mut()
            .ok_or_else(|| JsValue::from_str("no LOD dataset is open"))?;
        let can_load_blocks = self.format_reader.is_some();
        let mut format_reader = self.format_reader.as_mut();
        let stats = dataset
            .update_view_progressive(
                renderer,
                target_voxel_pixels,
                selection_error_pixels,
                stage_pixels,
                |block_id| {
                    format_reader
                        .as_mut()
                        .map(|reader| {
                            let geometry = reader
                                .read_geometry(block_id)
                                .map_err(|error| error.to_string())?;
                            let mut attributes = Vec::new();
                            if let Some(name) = requested_attribute
                                && let Some(schema_index) = reader
                                    .format_metadata()
                                    .dataset()
                                    .hierarchy()
                                    .attributes()
                                    .iter()
                                    .position(|descriptor| descriptor.name() == name)
                            {
                                let schema_index = u32::try_from(schema_index)
                                    .map_err(|_| "attribute schema is too large".to_owned())?;
                                attributes.push(
                                    reader
                                        .read_attribute(block_id, schema_index)
                                        .map_err(|error| error.to_string())?,
                                );
                            }
                            Ok(Some(PointBlock::new(block_id, geometry, attributes)))
                        })
                        .unwrap_or(Ok(None))
                },
            )
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        (self.lod_stage_pixels, self.lod_dirty) = next_lod_stage(
            self.lod_stage_pixels,
            target_voxel_pixels,
            !stats.pending_block_ids.is_empty(),
            can_load_blocks,
        );
        self.last_lod_stats = Some(stats.clone());
        Ok(stats)
    }

    fn clear_lod_dataset(&mut self) {
        if self.lod_dataset.take().is_some()
            && let Some(renderer) = &mut self.gpu_renderer
        {
            renderer.clear_dataset();
        }
        self.lod_config = None;
        self.lod_stage_pixels = None;
        self.lod_dirty = false;
        self.last_lod_stats = None;
        self.last_lod_error = None;
        self.format_reader = None;
    }
}

const fn color_attribute_name(mode: ColorMode) -> Option<&'static str> {
    match mode {
        ColorMode::Intensity => Some("intensity"),
        ColorMode::Classification => Some("classification"),
        ColorMode::Rgb | ColorMode::Elevation => None,
    }
}

fn lod_view_stats_to_js(stats: &LodViewStats) -> Result<JsValue, JsValue> {
    let value = js_sys::Object::new();
    for (name, number) in [
        ("minVoxelEdgePixels", stats.min_voxel_edge_pixels),
        ("maxVoxelEdgePixels", stats.max_voxel_edge_pixels),
        ("oversizedVoxels", stats.oversized_voxels as f64),
    ] {
        js_sys::Reflect::set(&value, &name.into(), &JsValue::from_f64(number))?;
    }
    js_sys::Reflect::set(
        &value,
        &"targetVoxelPixels".into(),
        &JsValue::from_f64(stats.target_voxel_pixels),
    )?;
    js_sys::Reflect::set(
        &value,
        &"visibleBlocks".into(),
        &JsValue::from_f64(stats.visible_blocks as f64),
    )?;
    js_sys::Reflect::set(
        &value,
        &"visiblePoints".into(),
        &JsValue::from_str(&stats.visible_points.to_string()),
    )?;
    js_sys::Reflect::set(
        &value,
        &"renderedVoxels".into(),
        &JsValue::from_f64(stats.rendered_voxels as f64),
    )?;
    js_sys::Reflect::set(
        &value,
        &"voxelBudget".into(),
        &JsValue::from_f64(stats.voxel_budget as f64),
    )?;
    js_sys::Reflect::set(
        &value,
        &"lodStagePixels".into(),
        &JsValue::from_f64(stats.lod_stage_pixels),
    )?;
    js_sys::Reflect::set(
        &value,
        &"maxVisibleLevel".into(),
        &stats
            .max_visible_level
            .map_or(JsValue::NULL, |level| JsValue::from_f64(f64::from(level))),
    )?;
    js_sys::Reflect::set(
        &value,
        &"cacheBytes".into(),
        &JsValue::from_str(&stats.cache_bytes.to_string()),
    )?;
    js_sys::Reflect::set(
        &value,
        &"cacheBudgetBytes".into(),
        &JsValue::from_str(&stats.cache_budget_bytes.to_string()),
    )?;
    let pending_block_ids = js_sys::Array::new();
    for block_id in &stats.pending_block_ids {
        pending_block_ids.push(&JsValue::from_str(&block_id.get().to_string()));
    }
    js_sys::Reflect::set(&value, &"pendingBlockIds".into(), &pending_block_ids)?;
    js_sys::Reflect::set(
        &value,
        &"uncoveredNodes".into(),
        &JsValue::from_f64(stats.uncovered_nodes as f64),
    )?;
    js_sys::Reflect::set(
        &value,
        &"coverageComplete".into(),
        &JsValue::from_bool(stats.uncovered_nodes == 0),
    )?;
    js_sys::Reflect::set(
        &value,
        &"budgetOmittedNodes".into(),
        &JsValue::from_f64(stats.budget_omitted_nodes as f64),
    )?;
    Ok(value.into())
}

fn add_runtime_gpu_stats(
    value: &JsValue,
    renderer: Option<&SurfaceRenderer<'_>>,
) -> Result<(), JsValue> {
    let Some(renderer) = renderer else {
        return Ok(());
    };
    js_sys::Reflect::set(
        value,
        &"renderedVoxels".into(),
        &JsValue::from_f64(renderer.visible_instance_count() as f64),
    )?;
    js_sys::Reflect::set(
        value,
        &"hzbActive".into(),
        &JsValue::from_bool(renderer.hzb_active()),
    )?;
    Ok(())
}

fn raw_scene_point_limit() -> usize {
    RendererConfig::default().gpu_budget.max_visible_instances
}

fn points_from_typed_arrays(
    positions: &js_sys::Float32Array,
    colors: &js_sys::Uint8Array,
    max_points: usize,
) -> Result<Vec<Point>, JsValue> {
    let position_components = positions.length() as usize;
    let color_components = colors.length() as usize;
    if !position_components.is_multiple_of(3) || color_components != position_components {
        return Err(JsValue::from_str(
            "positions and colors must contain three values per point",
        ));
    }
    let point_count = position_components / 3;
    if point_count > max_points {
        return Err(JsValue::from_str(&format!(
            "point batch has {point_count} points, limit is {max_points}"
        )));
    }
    let mut points = Vec::new();
    points
        .try_reserve_exact(point_count)
        .map_err(|_| JsValue::from_str("unable to allocate decoded point batch"))?;
    let staged_points = point_count.min(WASM_POINT_BATCH_POINTS);
    let mut position_batch = vec![0.0_f32; staged_points * 3];
    let mut color_batch = vec![0_u8; staged_points * 3];
    let mut point_offset = 0;
    while point_offset < point_count {
        let batch_points = (point_count - point_offset).min(WASM_POINT_BATCH_POINTS);
        let batch_components = batch_points * 3;
        let component_start = (point_offset * 3) as u32;
        let component_end = component_start + batch_components as u32;
        positions
            .subarray(component_start, component_end)
            .copy_to(&mut position_batch[..batch_components]);
        colors
            .subarray(component_start, component_end)
            .copy_to(&mut color_batch[..batch_components]);
        for (position, color) in position_batch[..batch_components]
            .as_chunks::<3>()
            .0
            .iter()
            .zip(color_batch[..batch_components].as_chunks::<3>().0.iter())
        {
            if position.iter().any(|value| !value.is_finite()) {
                return Err(JsValue::from_str("block positions must be finite"));
            }
            points.push(Point::new(
                [position[0], position[1], position[2]],
                ColorRgb::new(color[0], color[1], color[2]),
            ));
        }
        point_offset += batch_points;
    }
    Ok(points)
}

fn viewer_error_to_js(error: ViewerError) -> JsValue {
    JsValue::from_str(&error.to_string())
}
