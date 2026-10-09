use serde_wasm_bindgen::to_value;
use std::io::{self, Read, Seek, SeekFrom};
use viewer_core::{BlockId, ColorRgb, DatasetSource, Point, PointBatch, PointBlock};
use wasm_bindgen::prelude::*;
use web_sys::{Blob, FileReaderSync, HtmlCanvasElement};

use crate::{
    LasPointStream, LodDataset, LodViewStats, RenderSceneSnapshot, ViewerConfig, ViewerController,
    ViewerError, lod_metadata::LodDatasetDescriptor,
};
use viewer_format::FormatReader;
use viewer_render_wgpu::{ColorMode, RendererConfig, SurfaceRenderer};

const FILE_CACHE_BYTES: u64 = 256 * 1024;

#[wasm_bindgen]
pub struct WasmLasImportSession {
    inner: LasPointStream,
}

#[wasm_bindgen]
impl WasmLasImportSession {
    /// Opens a LAS or LAZ Blob. Construct this class in a dedicated Web Worker.
    #[wasm_bindgen(constructor)]
    pub fn new(file: Blob) -> Result<WasmLasImportSession, JsValue> {
        let source = BrowserBlobReader::new(file)?;
        let inner =
            LasPointStream::new(source).map_err(|error| JsValue::from_str(&error.to_string()))?;
        Ok(Self { inner })
    }

    #[wasm_bindgen(js_name = readNext)]
    pub fn read_next(&mut self, requested_points: u32) -> Result<JsValue, JsValue> {
        let chunk = self
            .inner
            .read_next(u64::from(requested_points))
            .map_err(|error| JsValue::from_str(&error.to_string()))?;

        let result = js_sys::Object::new();
        let positions = js_sys::Float32Array::from(chunk.positions.as_slice());
        let colors = js_sys::Uint8Array::from(chunk.colors.as_slice());
        let origin = js_sys::Float64Array::from(chunk.origin.as_slice());

        js_sys::Reflect::set(&result, &"positions".into(), &positions)?;
        js_sys::Reflect::set(&result, &"colors".into(), &colors)?;
        js_sys::Reflect::set(&result, &"origin".into(), &origin)?;
        js_sys::Reflect::set(
            &result,
            &"processedPoints".into(),
            &JsValue::from_str(&chunk.processed_points.to_string()),
        )?;
        js_sys::Reflect::set(
            &result,
            &"totalPoints".into(),
            &JsValue::from_str(&chunk.total_points.to_string()),
        )?;
        js_sys::Reflect::set(&result, &"done".into(), &JsValue::from_bool(chunk.done))?;
        Ok(result.into())
    }
}

struct BrowserBlobReader {
    blob: Blob,
    file_reader: FileReaderSync,
    file_size: u64,
    position: u64,
    cached_start: u64,
    cache: Vec<u8>,
}

impl BrowserBlobReader {
    fn new(blob: Blob) -> Result<Self, JsValue> {
        let size = blob.size();
        if !size.is_finite() || size < 0.0 || size > u64::MAX as f64 {
            return Err(JsValue::from_str(
                "file size is outside the supported range",
            ));
        }

        Ok(Self {
            blob,
            file_reader: FileReaderSync::new()?,
            file_size: size as u64,
            position: 0,
            cached_start: u64::MAX,
            cache: Vec::new(),
        })
    }

    fn load_cache(&mut self) -> io::Result<()> {
        let start = self.position / FILE_CACHE_BYTES * FILE_CACHE_BYTES;
        let end = start.saturating_add(FILE_CACHE_BYTES).min(self.file_size);
        let slice = self
            .blob
            .slice_with_f64_and_f64(start as f64, end as f64)
            .map_err(js_error_to_io)?;
        let array_buffer = self
            .file_reader
            .read_as_array_buffer(&slice)
            .map_err(js_error_to_io)?;
        let array = js_sys::Uint8Array::new(&array_buffer);
        self.cache.resize((end - start) as usize, 0);
        array.copy_to(&mut self.cache);
        self.cached_start = start;
        Ok(())
    }
}

impl Read for BrowserBlobReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.position >= self.file_size {
            return Ok(0);
        }
        if self.cached_start == u64::MAX
            || self.position < self.cached_start
            || self.position >= self.cached_start + self.cache.len() as u64
        {
            self.load_cache()?;
        }

        let cache_offset = (self.position - self.cached_start) as usize;
        let count = output
            .len()
            .min(self.cache.len().saturating_sub(cache_offset));
        output[..count].copy_from_slice(&self.cache[cache_offset..cache_offset + count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for BrowserBlobReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.file_size) + i128::from(offset),
        };
        if !(0..=i128::from(u64::MAX)).contains(&next) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek position is outside the supported range",
            ));
        }
        self.position = next as u64;
        Ok(self.position)
    }
}

fn js_error_to_io(error: JsValue) -> io::Error {
    let message = error
        .as_string()
        .unwrap_or_else(|| "browser file read failed".to_owned());
    io::Error::other(message)
}

#[wasm_bindgen]
pub struct WasmVoxelViewer {
    inner: ViewerController,
    gpu_renderer: Option<SurfaceRenderer<'static>>,
    gpu_scene_dirty: bool,
    lod_dataset: Option<LodDataset>,
    format_reader: Option<FormatReader<BrowserBlobReader>>,
    lod_config: Option<(f64, u64)>,
    lod_dirty: bool,
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
            lod_dirty: false,
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
    pub fn replace_points(&mut self, positions: Vec<f32>, colors: Vec<u8>) -> Result<(), JsValue> {
        self.inner
            .replace_points(&positions, &colors)
            .map_err(viewer_error_to_js)?;
        self.gpu_scene_dirty = true;
        self.clear_lod_dataset();
        Ok(())
    }

    #[wasm_bindgen(js_name = appendPoints)]
    pub fn append_points(&mut self, positions: Vec<f32>, colors: Vec<u8>) -> Result<(), JsValue> {
        self.inner
            .append_points(&positions, &colors)
            .map_err(viewer_error_to_js)?;
        self.gpu_scene_dirty = true;
        self.clear_lod_dataset();
        Ok(())
    }

    #[wasm_bindgen(js_name = replaceVoxelizedPoints)]
    pub fn replace_voxelized_points(
        &mut self,
        positions: Vec<f32>,
        colors: Vec<u8>,
    ) -> Result<(), JsValue> {
        self.inner
            .replace_voxelized_points(&positions, &colors)
            .map_err(viewer_error_to_js)?;
        self.gpu_scene_dirty = true;
        self.clear_lod_dataset();
        Ok(())
    }

    #[wasm_bindgen(js_name = appendVoxelizedPoints)]
    pub fn append_voxelized_points(
        &mut self,
        positions: Vec<f32>,
        colors: Vec<u8>,
    ) -> Result<(), JsValue> {
        self.inner
            .append_voxelized_points(&positions, &colors)
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
        self.lod_dirty = false;
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
        let source = BrowserBlobReader::new(file)?;
        let reader =
            FormatReader::new(source).map_err(|error| JsValue::from_str(&error.to_string()))?;
        let hierarchy_stats = reader.metadata().hierarchy().stats();
        let metadata = reader.format_metadata().shared_dataset();
        let dataset = LodDataset::from_shared_metadata(metadata, cache_bytes as usize);
        if let Some(renderer) = &mut self.gpu_renderer {
            renderer.clear_dataset();
        }
        self.lod_dataset = Some(dataset);
        self.format_reader = Some(reader);
        self.lod_config = None;
        self.lod_dirty = false;
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
        let positions = positions.to_vec();
        let colors = colors.to_vec();
        if positions.len() % 3 != 0 || colors.len() != positions.len() {
            return Err(JsValue::from_str(
                "positions and colors must contain three values per point",
            ));
        }
        if positions.iter().any(|value| !value.is_finite()) {
            return Err(JsValue::from_str("block positions must be finite"));
        }
        let points = positions
            .chunks_exact(3)
            .zip(colors.chunks_exact(3))
            .map(|(position, color)| {
                Point::new(
                    [position[0], position[1], position[2]],
                    ColorRgb::new(color[0], color[1], color[2]),
                )
            })
            .collect();
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
        self.lod_dirty = self.lod_config.is_some();
        Ok(accounted_bytes)
    }

    /// Selects a camera-aware cut using a built-in quality or performance profile.
    #[wasm_bindgen(js_name = updateLod)]
    pub fn update_lod(
        &mut self,
        profile: String,
        point_budget: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let (max_error_pixels, default_point_budget) = match profile.as_str() {
            "quality" => (1.0 / 16.0, 4_000_000),
            "performance" => (2.0, 1_000_000),
            _ => {
                return Err(JsValue::from_str(
                    "LOD profile must be quality or performance",
                ));
            }
        };
        let point_budget = point_budget
            .as_deref()
            .map(|value| {
                value.parse::<u64>().map_err(|_| {
                    JsValue::from_str("pointBudget must be an unsigned 64-bit integer string")
                })
            })
            .transpose()?
            .unwrap_or(default_point_budget);
        self.lod_config = Some((max_error_pixels, point_budget));
        self.lod_dirty = false;
        let stats = self.update_lod_view(max_error_pixels, point_budget)?;
        let value = js_sys::Object::new();
        js_sys::Reflect::set(&value, &"profile".into(), &JsValue::from_str(&profile))?;
        js_sys::Reflect::set(
            &value,
            &"maxErrorPixels".into(),
            &JsValue::from_f64(max_error_pixels),
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
            &"pointBudget".into(),
            &JsValue::from_str(&stats.point_budget.to_string()),
        )?;
        let pending_block_ids = js_sys::Array::new();
        for block_id in stats.pending_block_ids {
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

    #[wasm_bindgen(js_name = setColorMode)]
    pub fn set_color_mode(&mut self, mode: String) -> Result<(), JsValue> {
        let mode = ColorMode::parse(&mode).ok_or_else(|| {
            JsValue::from_str("color mode must be rgb, intensity, elevation, or classification")
        })?;
        self.gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?
            .set_color_mode(mode);
        Ok(())
    }

    #[wasm_bindgen(js_name = setPointSize)]
    pub fn set_point_size(&mut self, size: f32) -> Result<(), JsValue> {
        if !size.is_finite() || !(0.1..=10.0).contains(&size) {
            return Err(JsValue::from_str(
                "point size multiplier must be between 0.1 and 10",
            ));
        }
        self.gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?
            .set_point_size(size)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        Ok(())
    }

    #[wasm_bindgen(js_name = stats)]
    pub fn stats(&self) -> Result<JsValue, JsValue> {
        to_value(&self.inner.stats()).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    #[wasm_bindgen(js_name = renderScene)]
    pub fn render_scene(&self) -> Result<JsValue, JsValue> {
        let scene = RenderSceneSnapshot::from(self.inner.render_scene());
        to_value(&scene).map_err(|error| JsValue::from_str(&error.to_string()))
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
        let fitted = if self.lod_dataset.is_some() {
            renderer.fit_visible_blocks()
        } else {
            renderer.fit_scene(self.inner.render_scene())
        };
        if fitted && self.lod_dataset.is_some() {
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
            self.lod_dirty = self.lod_config.is_some();
        }
        Ok(changed)
    }

    #[wasm_bindgen(js_name = gpuStats)]
    pub fn gpu_stats(&self) -> Result<JsValue, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_ref()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let stats = js_sys::Object::new();
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
        Ok(stats.into())
    }
}

impl WasmVoxelViewer {
    fn refresh_lod_if_dirty(&mut self) -> Result<(), JsValue> {
        if !self.lod_dirty {
            return Ok(());
        }
        let Some((max_error_pixels, point_budget)) = self.lod_config else {
            return Ok(());
        };
        self.lod_dirty = false;
        self.update_lod_view(max_error_pixels, point_budget)
            .map(|_| ())
    }

    fn update_lod_view(
        &mut self,
        max_error_pixels: f64,
        point_budget: u64,
    ) -> Result<LodViewStats, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let dataset = self
            .lod_dataset
            .as_mut()
            .ok_or_else(|| JsValue::from_str("no LOD dataset is open"))?;
        let mut format_reader = self.format_reader.as_mut();
        dataset
            .update_view_with_loader(renderer, max_error_pixels, point_budget, |block_id| {
                format_reader
                    .as_mut()
                    .map(|reader| {
                        reader
                            .read_block(block_id)
                            .map(Some)
                            .map_err(|error| error.to_string())
                    })
                    .unwrap_or(Ok(None))
            })
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    fn clear_lod_dataset(&mut self) {
        if self.lod_dataset.take().is_some()
            && let Some(renderer) = &mut self.gpu_renderer
        {
            renderer.clear_dataset();
        }
        self.lod_config = None;
        self.lod_dirty = false;
        self.format_reader = None;
    }
}

fn viewer_error_to_js(error: ViewerError) -> JsValue {
    JsValue::from_str(&error.to_string())
}
