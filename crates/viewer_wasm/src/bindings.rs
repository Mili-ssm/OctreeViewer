use std::sync::{Arc, Mutex};

use serde_wasm_bindgen::to_value;
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    DatasetSource, Point, PointBatch, PointBlock,
};
use wasm_bindgen::prelude::*;
use web_sys::{Blob, HtmlCanvasElement};

use crate::{
    LodDataset, LodViewStats, RenderInstanceSnapshot, ViewerConfig, ViewerController, ViewerError,
    browser_io::{BrowserBlobReader, BrowserReadStats},
    lod_dataset::next_lod_stage,
    lod_metadata::LodDatasetDescriptor,
    lod_session::LodSession,
};
use viewer_format::FormatReader;
use viewer_render_wgpu::{ColorMode, RendererConfig, SurfaceRenderer, VoxelBuildLimits};

const WASM_POINT_BATCH_POINTS: usize = 16_384;
const MAX_WASM_TRANSFER_INSTANCES: usize = 16_384;
#[wasm_bindgen]
pub struct WasmVoxelViewer {
    inner: ViewerController,
    gpu_renderer: Option<SurfaceRenderer<'static>>,
    gpu_scene_dirty: bool,
    lod_session: Option<LodSession<FormatReader<BrowserBlobReader>>>,
    async_format_context: Option<AsyncFormatContext>,
    pending_async_block: Option<BlockId>,
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
        let async_format_context =
            descriptor
                .attribute_ranges
                .as_ref()
                .map(|ranges| AsyncFormatContext {
                    has_attributes: descriptor.has_attributes,
                    attribute_ranges: ranges.clone(),
                });
        let dataset = descriptor
            .build()
            .map_err(|error| JsValue::from_str(&error))?;
        let result = js_sys::Object::new();
        js_sys::Reflect::set(
            &result,
            &"status".into(),
            &JsValue::from_str("metadata_ready"),
        )?;
        if let Some(renderer) = &mut self.gpu_renderer {
            renderer.clear_dataset();
        }
        self.lod_session = Some(LodSession::External(dataset));
        self.async_format_context = async_format_context;
        self.pending_async_block = None;
        self.reset_range_read_stats();
        self.selected_attribute = None;
        self.selected_attribute_range = None;
        self.lod_config = None;
        self.lod_stage_pixels = None;
        self.lod_dirty = false;
        self.last_lod_stats = None;
        self.last_lod_error = None;
        self.gpu_scene_dirty = false;
        Ok(result.into())
    }

    /// Opens the project's random-access format from a Blob; call from a dedicated worker.
    #[wasm_bindgen(js_name = openFormatDataset)]
    pub fn open_format_dataset(
        &mut self,
        file: Blob,
        cache_bytes: u32,
        attributes: Option<Blob>,
    ) -> Result<JsValue, JsValue> {
        let browser_read_stats = Arc::new(Mutex::new(BrowserReadStats::default()));
        let source =
            BrowserBlobReader::new_with_stats(file, true, Arc::clone(&browser_read_stats))?;
        let mut reader =
            FormatReader::new(source).map_err(|error| JsValue::from_str(&error.to_string()))?;
        if let Some(attributes) = attributes {
            let source = BrowserBlobReader::new_with_stats(
                attributes,
                false,
                Arc::clone(&browser_read_stats),
            )?;
            reader
                .attach_attributes(source)
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
        }
        // Exclude metadata open/attribute attachment reads; report LOD block fills only.
        reset_browser_read_stats(&browser_read_stats);
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
        let schema = js_sys::Array::new();
        for descriptor in reader.format_metadata().dataset().hierarchy().attributes() {
            let entry = js_sys::Object::new();
            js_sys::Reflect::set(
                &entry,
                &"name".into(),
                &JsValue::from_str(descriptor.name()),
            )?;
            js_sys::Reflect::set(
                &entry,
                &"dataType".into(),
                &JsValue::from_str(&format!("{:?}", descriptor.data_type()).to_lowercase()),
            )?;
            js_sys::Reflect::set(
                &entry,
                &"componentCount".into(),
                &JsValue::from_f64(f64::from(descriptor.component_count())),
            )?;
            schema.push(&entry);
        }
        js_sys::Reflect::set(&result, &"attributeSchema".into(), &schema)?;
        js_sys::Reflect::set(
            &result,
            &"hasAttributes".into(),
            &JsValue::from_bool(reader.has_attributes()),
        )?;
        if let Some(source_points) = hierarchy_stats.source_point_count {
            js_sys::Reflect::set(
                &result,
                &"sourcePointCount".into(),
                &JsValue::from_str(&source_points.to_string()),
            )?;
        }
        if let Some(renderer) = &mut self.gpu_renderer {
            renderer.clear_dataset();
            renderer.set_color_mode(ColorMode::Rgb);
        }
        self.lod_session = Some(LodSession::Format { dataset, reader });
        self.async_format_context = None;
        self.pending_async_block = None;
        self.browser_read_stats = browser_read_stats;
        self.lod_range_read_stats = BrowserReadStats::default();
        self.selected_attribute = None;
        self.selected_attribute_range = None;
        self.lod_config = None;
        self.lod_stage_pixels = None;
        self.lod_dirty = false;
        self.last_lod_stats = None;
        self.last_lod_error = None;
        self.gpu_scene_dirty = false;

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
            .lod_session
            .as_ref()
            .map(LodSession::dataset)
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
            .lod_session
            .as_mut()
            .map(LodSession::dataset_mut)
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

    /// Inserts one block received from an async format reader, preserving an optional typed column.
    #[wasm_bindgen(js_name = insertAsyncLodBlock)]
    pub fn insert_async_lod_block(
        &mut self,
        block_id: String,
        positions: js_sys::Float32Array,
        colors: Option<js_sys::Uint8Array>,
        attribute: Option<JsValue>,
    ) -> Result<usize, JsValue> {
        let block_id = parse_block_id(&block_id)?;
        let position_components = positions.length() as usize;
        if !position_components.is_multiple_of(3) {
            return Err(JsValue::from_str(
                "positions must contain three values per point",
            ));
        }
        let point_count = position_components / 3;
        let dataset = self
            .lod_session
            .as_ref()
            .map(LodSession::dataset)
            .ok_or_else(|| JsValue::from_str("no LOD dataset is open"))?;
        if !dataset.references_block(block_id) {
            return Err(JsValue::from_str(
                "blockId is not referenced by the dataset",
            ));
        }
        let schema = dataset.attribute_schema();
        let column = attribute
            .as_ref()
            .map(|value| attribute_column_from_js(schema, value, point_count))
            .transpose()?;
        let attribute_bytes = column
            .as_ref()
            .map(attribute_column_payload_bytes)
            .transpose()?
            .unwrap_or_default();
        let budget_bytes = dataset.block_cache_budget_bytes();
        let requested_bytes = point_count
            .checked_mul(std::mem::size_of::<Point>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PointBlock>()))
            .and_then(|bytes| bytes.checked_add(attribute_bytes))
            .ok_or_else(|| JsValue::from_str("block size exceeds the cache budget"))?;
        if requested_bytes > budget_bytes {
            return Err(JsValue::from_str(&format!(
                "block needs at least {requested_bytes} bytes, cache budget is {budget_bytes}"
            )));
        }

        let colors = colors.unwrap_or_else(|| {
            let values = js_sys::Uint8Array::new_with_length(position_components as u32);
            for index in 0..position_components as u32 {
                values.set_index(index, 160);
            }
            values
        });
        let points = points_from_typed_arrays(&positions, &colors, point_count)?;
        let mut geometry = PointBatch::new(points);
        let mut columns = Vec::new();
        if let Some(column) = column {
            if let Some((schema_index, component)) = self.selected_attribute {
                if schema
                    .get(schema_index as usize)
                    .map(AttributeDescriptor::name)
                    != Some(column.descriptor().name())
                {
                    return Err(JsValue::from_str(
                        "async attribute does not match the selected color column",
                    ));
                }
                geometry = viewer_render_wgpu::colorize_attribute(
                    geometry,
                    &column,
                    component,
                    self.selected_attribute_range,
                )
                .map_err(JsValue::from_str)?;
            } else {
                columns.push(column);
            }
        } else if self.selected_attribute.is_some() {
            return Err(JsValue::from_str("selected attribute payload is required"));
        }
        let block = PointBlock::new(block_id, geometry, columns);
        if block.estimated_memory_bytes() > budget_bytes {
            return Err(JsValue::from_str(
                "decoded block allocation exceeds the cache budget",
            ));
        }
        let dataset = self
            .lod_session
            .as_mut()
            .map(LodSession::dataset_mut)
            .ok_or_else(|| JsValue::from_str("no LOD dataset is open"))?;
        let accounted_bytes = dataset
            .insert_block(block)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        self.pending_async_block = None;
        self.last_lod_error = None;
        self.lod_dirty = self.lod_config.is_some();
        Ok(accounted_bytes)
    }

    /// Highest-priority missing block selected by the most recent async LOD update.
    #[wasm_bindgen(js_name = nextAsyncLodBlock)]
    pub fn next_async_lod_block(&self) -> JsValue {
        self.pending_async_block
            .map(|id| JsValue::from_str(&id.get().to_string()))
            .unwrap_or(JsValue::NULL)
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
        let value = lod_view_stats_to_js(&stats, self.lod_range_read_stats)?;
        add_runtime_gpu_stats(&value, self.gpu_renderer.as_ref())?;
        add_cache_activity(&value, self.lod_session.as_ref().map(LodSession::dataset))?;
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
            Some((stats, range_read_stats)) => {
                let mut current_stats = stats.clone();
                if let Some(dataset) = self.lod_session.as_ref().map(LodSession::dataset) {
                    current_stats.refinement_work = dataset.refinement_work().clone();
                }
                let value = lod_view_stats_to_js(&current_stats, *range_read_stats)?;
                add_runtime_gpu_stats(&value, self.gpu_renderer.as_ref())?;
                add_cache_activity(&value, self.lod_session.as_ref().map(LodSession::dataset))?;
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
        let mut selected_range = None;
        let selected_attribute = if let Some(selection) = mode.strip_prefix("attribute:") {
            let (index, component) = selection
                .split_once(':')
                .ok_or_else(|| JsValue::from_str("expected attribute:index:component"))?;
            let index = index
                .parse::<u32>()
                .map_err(|_| JsValue::from_str("invalid attribute index"))?;
            let component = component
                .parse::<u32>()
                .map_err(|_| JsValue::from_str("invalid attribute component"))?;
            let session = self
                .lod_session
                .as_ref()
                .ok_or_else(|| JsValue::from_str("no format dataset open"))?;
            let descriptor = session
                .dataset()
                .attribute_schema()
                .get(index as usize)
                .ok_or_else(|| JsValue::from_str("invalid attribute index"))?;
            if component >= descriptor.component_count() {
                return Err(JsValue::from_str("invalid attribute component"));
            }
            let (has_attributes, range) = if let Some(reader) = session.reader() {
                let range = reader
                    .attribute_range(index, component)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
                (reader.has_attributes(), range)
            } else if let Some(context) = &self.async_format_context {
                let range = context
                    .attribute_ranges
                    .get(index as usize)
                    .and_then(|components| components.get(component as usize))
                    .copied()
                    .flatten();
                (context.has_attributes, range)
            } else {
                return Err(JsValue::from_str("no format dataset open"));
            };
            if !has_attributes {
                return Err(JsValue::from_str("attach the attribute file first"));
            }
            selected_range = range;
            Some((index, component))
        } else {
            None
        };
        let mode = if selected_attribute.is_some() {
            Some(ColorMode::Rgb)
        } else {
            ColorMode::parse(&mode)
        }
        .ok_or_else(|| {
            JsValue::from_str("color mode must be rgb, intensity, elevation, or classification")
        })?;
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        if renderer.color_mode() != mode || self.selected_attribute != selected_attribute {
            self.selected_attribute = selected_attribute;
            self.selected_attribute_range = selected_range;
            renderer.set_color_mode(mode);
            if let Some(dataset) = self.lod_session.as_mut().map(LodSession::dataset_mut) {
                dataset
                    .clear_loaded_blocks(renderer)
                    .map_err(|error| JsValue::from_str(&error.to_string()))?;
                self.reset_range_read_stats();
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
        if self.gpu_scene_dirty && self.lod_session.is_none() {
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
        let fitted = if let Some(dataset) = self.lod_session.as_ref().map(LodSession::dataset) {
            dataset.fit_camera(renderer)
        } else {
            renderer.fit_scene(self.inner.render_scene())
        };
        if fitted && self.lod_session.is_some() {
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
        if changed && self.lod_session.is_some() {
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
        if changed && self.lod_session.is_some() {
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
        if changed && self.lod_session.is_some() {
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
        let mapped = buffer.slice(..).get_mapped_range().map_err(|error| {
            buffer.unmap();
            JsValue::from_str(&error.to_string())
        })?;
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
        let resources = renderer.resource_stats();
        let memory = js_sys::Object::new();
        for (name, bytes) in [
            ("instanceBufferBytes", resources.instance_buffer_bytes),
            ("fixedBufferBytes", resources.fixed_buffer_bytes),
            ("hzbBufferBytes", resources.hzb_buffer_bytes),
            ("depthTextureBytes", resources.depth_texture_bytes),
            ("hzbTextureBytes", resources.hzb_texture_bytes),
            ("totalBytes", resources.total_bytes()),
        ] {
            js_sys::Reflect::set(
                &memory,
                &name.into(),
                &JsValue::from_str(&bytes.to_string()),
            )?;
        }
        js_sys::Reflect::set(&stats, &"requestedResources".into(), &memory)?;
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

    fn camera_changed(&mut self, changed: bool) {
        if changed && self.lod_session.is_some() {
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
        if self.lod_stage_pixels.is_none()
            && target_voxel_pixels < 16.0
            && self.target_cut_is_resident(target_voxel_pixels)?
        {
            match self.update_lod_view_at_stage(target_voxel_pixels, target_voxel_pixels) {
                Ok(stats) => return Ok(stats),
                Err(_) => {
                    // A fine resident cut can still exceed the voxel/GPU build budget. Retry
                    // through the normal coarse stage before reporting the failure.
                    self.lod_stage_pixels = None;
                }
            }
        }
        self.update_lod_view_at_stage(target_voxel_pixels, self.lod_stage_pixels.unwrap_or(16.0))
    }

    fn target_cut_is_resident(&mut self, target_voxel_pixels: f64) -> Result<bool, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let dataset = self
            .lod_session
            .as_mut()
            .map(LodSession::dataset_mut)
            .ok_or_else(|| JsValue::from_str("no LOD dataset is open"))?;
        dataset
            .target_cut_is_resident(renderer, target_voxel_pixels)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    fn update_lod_view_at_stage(
        &mut self,
        target_voxel_pixels: f64,
        stage_pixels: f64,
    ) -> Result<LodViewStats, JsValue> {
        let selection_error_pixels = stage_pixels;
        let requested_mode = self.gpu_renderer.as_ref().map(SurfaceRenderer::color_mode);
        let selected_attribute = self.selected_attribute;
        let selected_range = self.selected_attribute_range;
        let browser_read_stats = Arc::clone(&self.browser_read_stats);
        let mut lod_range_read_stats = self.lod_range_read_stats;
        let has_async_reader = self.async_format_context.is_some();
        let mut pending_async_block = None;
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let session = self
            .lod_session
            .as_mut()
            .ok_or_else(|| JsValue::from_str("no LOD dataset is open"))?;
        let (dataset, mut format_reader) = session.parts_mut();
        let can_load_blocks = format_reader.is_some() || has_async_reader;
        let stats = dataset.update_view_progressive(
            renderer,
            target_voxel_pixels,
            selection_error_pixels,
            stage_pixels,
            |block_id| {
                format_reader
                    .as_mut()
                    .map(|reader| {
                        let before = snapshot_browser_read_stats(&browser_read_stats);
                        let loaded = crate::block_loading::load_render_block(
                            *reader,
                            block_id,
                            requested_mode,
                            selected_attribute,
                            selected_range,
                        )
                        .map(Some);
                        let after = snapshot_browser_read_stats(&browser_read_stats);
                        accumulate_browser_read_delta(&mut lod_range_read_stats, before, after);
                        loaded
                    })
                    .unwrap_or_else(|| {
                        if has_async_reader {
                            pending_async_block = Some(block_id);
                        }
                        Ok(None)
                    })
            },
        );
        self.lod_range_read_stats = lod_range_read_stats;
        let stats = stats.map_err(|error| JsValue::from_str(&error.to_string()))?;
        self.pending_async_block = pending_async_block;
        (self.lod_stage_pixels, self.lod_dirty) = next_lod_stage(
            Some(stage_pixels),
            target_voxel_pixels,
            !stats.pending_block_ids.is_empty(),
            can_load_blocks,
        );
        self.last_lod_stats = Some((stats.clone(), lod_range_read_stats));
        Ok(stats)
    }

    fn clear_lod_dataset(&mut self) {
        if self.lod_session.take().is_some()
            && let Some(renderer) = &mut self.gpu_renderer
        {
            renderer.clear_dataset();
        }
        self.lod_config = None;
        self.async_format_context = None;
        self.pending_async_block = None;
        self.lod_stage_pixels = None;
        self.lod_dirty = false;
        self.last_lod_stats = None;
        self.last_lod_error = None;
        self.reset_range_read_stats();
    }

    fn reset_range_read_stats(&mut self) {
        reset_browser_read_stats(&self.browser_read_stats);
        self.lod_range_read_stats = BrowserReadStats::default();
    }
}

fn accumulate_browser_read_delta(
    total: &mut BrowserReadStats,
    before: BrowserReadStats,
    after: BrowserReadStats,
) {
    total.calls = total
        .calls
        .saturating_add(after.calls.saturating_sub(before.calls));
    total.bytes = total
        .bytes
        .saturating_add(after.bytes.saturating_sub(before.bytes));
    total.elapsed_ms += (after.elapsed_ms - before.elapsed_ms).max(0.0);
}

fn snapshot_browser_read_stats(stats: &Mutex<BrowserReadStats>) -> BrowserReadStats {
    *stats
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn reset_browser_read_stats(stats: &Mutex<BrowserReadStats>) {
    *stats
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = BrowserReadStats::default();
}

// Cache activity is live even when the last accepted scene predates a failed insertion.
fn add_cache_activity(value: &JsValue, dataset: Option<&LodDataset>) -> Result<(), JsValue> {
    let Some(dataset) = dataset else {
        return Ok(());
    };
    let stats = dataset.cache_stats();
    let activity = js_sys::Object::new();
    for (name, count) in [
        ("insertions", stats.insertions),
        ("evictions", stats.evictions),
        ("rejectedOversize", stats.rejected_oversize),
        ("rejectedProtected", stats.rejected_protected),
    ] {
        js_sys::Reflect::set(
            &activity,
            &name.into(),
            &JsValue::from_str(&count.to_string()),
        )?;
    }
    js_sys::Reflect::set(value, &"cacheActivity".into(), &activity)?;
    Ok(())
}

fn lod_view_stats_to_js(
    stats: &LodViewStats,
    range_read_stats: BrowserReadStats,
) -> Result<JsValue, JsValue> {
    let value = js_sys::Object::new();
    for (name, number) in [
        ("minVoxelEdgePixels", stats.min_voxel_edge_pixels),
        ("maxVoxelEdgePixels", stats.max_voxel_edge_pixels),
        ("oversizedVoxels", stats.oversized_voxels as f64),
        (
            "projectionCachePeakEntries",
            stats.projection_cache_peak_entries as f64,
        ),
    ] {
        js_sys::Reflect::set(&value, &name.into(), &JsValue::from_f64(number))?;
    }
    for (name, bytes) in [
        ("voxelBuildBudgetBytes", stats.voxel_build_budget_bytes),
        (
            "voxelBuildEstimatedPeakBytes",
            stats.voxel_build_estimated_peak_bytes,
        ),
        (
            "voxelInstanceCapacityBytes",
            stats.voxel_instance_capacity_bytes,
        ),
    ] {
        js_sys::Reflect::set(&value, &name.into(), &JsValue::from_str(&bytes.to_string()))?;
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
    let work = js_sys::Object::new();
    for (name, number) in [
        ("updateCalls", stats.refinement_work.update_calls as f64),
        (
            "blockLoadCalls",
            stats.refinement_work.block_load_calls as f64,
        ),
        ("loadedBlocks", stats.refinement_work.loaded_blocks as f64),
        (
            "evictedPendingBlocks",
            stats.refinement_work.evicted_pending_blocks as f64,
        ),
        ("sceneBuilds", stats.refinement_work.scene_builds as f64),
        (
            "prefixRebuilds",
            stats.refinement_work.prefix_rebuilds as f64,
        ),
        (
            "prefixVoxelBuildMs",
            stats.refinement_work.prefix_voxel_build_ms,
        ),
        (
            "reusedSceneUpdates",
            stats.refinement_work.reused_scene_updates as f64,
        ),
        ("selectionMs", stats.refinement_work.selection_ms),
        ("planningMs", stats.refinement_work.planning_ms),
        ("blockLoadMs", stats.refinement_work.block_load_ms),
        ("rangeReadMs", range_read_stats.elapsed_ms),
        // Residual synchronous block-callback time; it is not pure decompression CPU time.
        (
            "decodeAndOtherCpuMs",
            (stats.refinement_work.block_load_ms - range_read_stats.elapsed_ms).max(0.0),
        ),
        ("voxelBuildMs", stats.refinement_work.voxel_build_ms),
        ("uploadMs", stats.refinement_work.upload_ms),
        (
            "unchangedCutBuilds",
            stats.refinement_work.unchanged_cut_builds as f64,
        ),
        (
            "reusableSceneBuilds",
            stats.refinement_work.reusable_scene_builds as f64,
        ),
        (
            "reusableVoxelBuildMs",
            stats.refinement_work.reusable_voxel_build_ms,
        ),
    ] {
        js_sys::Reflect::set(&work, &name.into(), &JsValue::from_f64(number))?;
    }
    js_sys::Reflect::set(
        &work,
        &"rangeReadCalls".into(),
        &JsValue::from_f64(range_read_stats.calls as f64),
    )?;
    js_sys::Reflect::set(
        &work,
        &"rangeReadBytes".into(),
        &JsValue::from_str(&range_read_stats.bytes.to_string()),
    )?;
    js_sys::Reflect::set(
        &work,
        &"voxelSourcePoints".into(),
        &JsValue::from_str(&stats.refinement_work.voxel_source_points.to_string()),
    )?;
    js_sys::Reflect::set(
        &work,
        &"reusableVoxelSourcePoints".into(),
        &JsValue::from_str(
            &stats
                .refinement_work
                .reusable_voxel_source_points
                .to_string(),
        ),
    )?;
    js_sys::Reflect::set(
        &work,
        &"reusedSourcePoints".into(),
        &JsValue::from_str(&stats.refinement_work.reused_source_points.to_string()),
    )?;
    for (name, points) in [
        (
            "prefixRetainedSourcePoints",
            stats.refinement_work.prefix_retained_source_points,
        ),
        (
            "prefixAddedSourcePoints",
            stats.refinement_work.prefix_added_source_points,
        ),
    ] {
        js_sys::Reflect::set(&work, &name.into(), &JsValue::from_str(&points.to_string()))?;
    }
    js_sys::Reflect::set(&value, &"refinementWork".into(), &work)?;
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

fn parse_block_id(value: &str) -> Result<BlockId, JsValue> {
    value
        .parse::<u64>()
        .map(BlockId::new)
        .map_err(|_| JsValue::from_str("blockId must be an unsigned 64-bit integer string"))
}

fn attribute_column_from_js(
    schema: &[AttributeDescriptor],
    value: &JsValue,
    point_count: usize,
) -> Result<AttributeColumn, JsValue> {
    let name = js_sys::Reflect::get(value, &"name".into())?
        .as_string()
        .ok_or_else(|| JsValue::from_str("attribute result is missing its name"))?;
    let descriptor = schema
        .iter()
        .find(|descriptor| descriptor.name() == name)
        .cloned()
        .ok_or_else(|| JsValue::from_str("attribute result is absent from the dataset schema"))?;
    let data = js_sys::Reflect::get(value, &"values".into())?;
    let values = match descriptor.data_type() {
        AttributeDataType::U8 if data.is_instance_of::<js_sys::Uint8Array>() => {
            AttributeValues::U8(js_sys::Uint8Array::new(&data).to_vec())
        }
        AttributeDataType::I8 if data.is_instance_of::<js_sys::Int8Array>() => {
            AttributeValues::I8(js_sys::Int8Array::new(&data).to_vec())
        }
        AttributeDataType::U16 if data.is_instance_of::<js_sys::Uint16Array>() => {
            AttributeValues::U16(js_sys::Uint16Array::new(&data).to_vec())
        }
        AttributeDataType::I16 if data.is_instance_of::<js_sys::Int16Array>() => {
            AttributeValues::I16(js_sys::Int16Array::new(&data).to_vec())
        }
        AttributeDataType::U32 if data.is_instance_of::<js_sys::Uint32Array>() => {
            AttributeValues::U32(js_sys::Uint32Array::new(&data).to_vec())
        }
        AttributeDataType::I32 if data.is_instance_of::<js_sys::Int32Array>() => {
            AttributeValues::I32(js_sys::Int32Array::new(&data).to_vec())
        }
        AttributeDataType::U64 if data.is_instance_of::<js_sys::BigUint64Array>() => {
            AttributeValues::U64(js_sys::BigUint64Array::new(&data).to_vec())
        }
        AttributeDataType::I64 if data.is_instance_of::<js_sys::BigInt64Array>() => {
            AttributeValues::I64(js_sys::BigInt64Array::new(&data).to_vec())
        }
        AttributeDataType::F32 if data.is_instance_of::<js_sys::Float32Array>() => {
            AttributeValues::F32(js_sys::Float32Array::new(&data).to_vec())
        }
        AttributeDataType::F64 if data.is_instance_of::<js_sys::Float64Array>() => {
            AttributeValues::F64(js_sys::Float64Array::new(&data).to_vec())
        }
        _ => {
            return Err(JsValue::from_str(
                "attribute typed array does not match the dataset schema",
            ));
        }
    };
    AttributeColumn::new(descriptor, values, point_count)
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

fn attribute_column_payload_bytes(column: &AttributeColumn) -> Result<usize, JsValue> {
    let value_bytes = match column.values() {
        AttributeValues::U8(values) => values.len().checked_mul(std::mem::size_of::<u8>()),
        AttributeValues::I8(values) => values.len().checked_mul(std::mem::size_of::<i8>()),
        AttributeValues::U16(values) => values.len().checked_mul(std::mem::size_of::<u16>()),
        AttributeValues::I16(values) => values.len().checked_mul(std::mem::size_of::<i16>()),
        AttributeValues::U32(values) => values.len().checked_mul(std::mem::size_of::<u32>()),
        AttributeValues::I32(values) => values.len().checked_mul(std::mem::size_of::<i32>()),
        AttributeValues::U64(values) => values.len().checked_mul(std::mem::size_of::<u64>()),
        AttributeValues::I64(values) => values.len().checked_mul(std::mem::size_of::<i64>()),
        AttributeValues::F32(values) => values.len().checked_mul(std::mem::size_of::<f32>()),
        AttributeValues::F64(values) => values.len().checked_mul(std::mem::size_of::<f64>()),
    }
    .ok_or_else(|| JsValue::from_str("attribute size exceeds the cache budget"))?;
    value_bytes
        .checked_add(column.descriptor().name().len())
        .ok_or_else(|| JsValue::from_str("attribute size exceeds the cache budget"))
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
