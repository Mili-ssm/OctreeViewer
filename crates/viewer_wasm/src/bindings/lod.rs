use std::sync::{Arc, Mutex};

use viewer_core::{BlockId, DatasetSource, Point, PointBatch, PointBlock};
use wasm_bindgen::prelude::*;
use web_sys::Blob;

use super::{
    AsyncFormatContext, WasmVoxelViewer,
    js_marshal::{
        attribute_column_from_js, attribute_column_payload_bytes, parse_block_id,
        points_from_typed_arrays,
    },
    js_stats::{
        accumulate_browser_read_delta, add_cache_activity, add_runtime_gpu_stats,
        lod_view_stats_to_js, reset_browser_read_stats, snapshot_browser_read_stats,
    },
};
use crate::{
    LodBuiltScene, LodDataset, LodDatasetError, LodViewStats,
    browser_io::{BrowserBlobReader, BrowserReadStats},
    lod_driver::{LodStep, LodStepResult, SceneSink, assemble_async_block, update_lod_view},
    lod_metadata::LodDatasetDescriptor,
    lod_session::LodSession,
};
use viewer_format::FormatReader;
use viewer_render_wgpu::{ColorMode, SurfaceRenderer, ViewProjection};

/// Uploads each rebuilt scene to the viewer's own GPU renderer.
struct RendererSink<'a, 'r> {
    renderer: &'a mut SurfaceRenderer<'r>,
}

impl SceneSink for RendererSink<'_, '_> {
    fn publish(&mut self, built: LodBuiltScene) -> Result<(), String> {
        self.renderer
            .upload_voxel_cut(&built.voxels.scene, &built.block_ids)
            .map(drop)
            .map_err(|error| LodDatasetError::Gpu(error).to_string())
    }

    fn base_cut_exceeded(&mut self, dataset: &LodDataset) -> Result<(), String> {
        self.renderer
            .sync_visible_blocks(&[], dataset.resident_blocks())
            .map(drop)
            .map_err(|error| error.to_string())
    }
}

/// Aligns the renderer's visual scale with the dataset's and snapshots its view math.
fn lod_view_projection(
    renderer: &mut SurfaceRenderer<'_>,
    dataset: &LodDataset,
) -> Result<ViewProjection, JsValue> {
    renderer
        .set_visual_scale(dataset.visual_scale())
        .map_err(|error| JsValue::from_str(&LodDatasetError::Gpu(error).to_string()))?;
    Ok(renderer.view_projection())
}

/// The object `updateLod` reports; `renderer` is absent when no GPU is involved.
pub(super) fn lod_update_stats_to_js(
    stats: &LodViewStats,
    range_read_stats: BrowserReadStats,
    dataset: Option<&LodDataset>,
    renderer: Option<&SurfaceRenderer<'_>>,
    lod_error: Option<&str>,
    target_voxel_pixels: f64,
) -> Result<JsValue, JsValue> {
    let value = lod_view_stats_to_js(stats, range_read_stats)?;
    add_runtime_gpu_stats(&value, renderer)?;
    add_cache_activity(&value, dataset)?;
    js_sys::Reflect::set(
        &value,
        &"lodError".into(),
        &lod_error.map_or(JsValue::NULL, JsValue::from_str),
    )?;
    js_sys::Reflect::set(
        &value,
        &"targetVoxelPixels".into(),
        &JsValue::from_f64(target_voxel_pixels),
    )?;
    Ok(value)
}

/// Parses a host-provided hierarchy descriptor into a dataset whose blocks arrive from the host.
pub(super) fn open_descriptor(
    descriptor: JsValue,
) -> Result<(LodDataset, Option<AsyncFormatContext>), JsValue> {
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
    Ok((dataset, async_format_context))
}

/// A format session with its read counters and the `format_ready` metadata object.
pub(super) type OpenedFormat = (
    LodSession<FormatReader<BrowserBlobReader>>,
    Arc<Mutex<BrowserReadStats>>,
    JsValue,
);

/// Opens the random-access format from Blobs; returns the session, its read counters and the
/// `format_ready` metadata object.
pub(super) fn open_format(
    file: Blob,
    cache_bytes: u32,
    attributes: Option<Blob>,
) -> Result<OpenedFormat, JsValue> {
    let browser_read_stats = Arc::new(Mutex::new(BrowserReadStats::default()));
    let source = BrowserBlobReader::new_with_stats(file, true, Arc::clone(&browser_read_stats))?;
    let mut reader =
        FormatReader::new(source).map_err(|error| JsValue::from_str(&error.to_string()))?;
    if let Some(attributes) = attributes {
        let source =
            BrowserBlobReader::new_with_stats(attributes, false, Arc::clone(&browser_read_stats))?;
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
    Ok((
        LodSession::Format { dataset, reader },
        browser_read_stats,
        result.into(),
    ))
}

/// Validates and caches one block delivered by the host, painting the selected attribute into
/// its colours. Shared by every binding that accepts asynchronous blocks.
pub(super) fn insert_async_block(
    session: Option<&mut LodSession<FormatReader<BrowserBlobReader>>>,
    (selected_attribute, selected_range): (Option<(u32, u32)>, Option<[f64; 2]>),
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
    let dataset = session
        .as_deref()
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
    let block = assemble_async_block(
        block_id,
        PointBatch::new(points),
        column,
        schema,
        selected_attribute,
        selected_range,
        budget_bytes,
    )
    .map_err(|message| JsValue::from_str(&message))?;
    let dataset = session
        .map(LodSession::dataset_mut)
        .ok_or_else(|| JsValue::from_str("no LOD dataset is open"))?;
    let accounted_bytes = dataset
        .insert_block(block)
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    Ok(accounted_bytes)
}

#[wasm_bindgen]
impl WasmVoxelViewer {
    /// Opens resident hierarchy metadata for decoded block streaming.
    #[wasm_bindgen(js_name = openLodDataset)]
    pub fn open_lod_dataset(&mut self, descriptor: JsValue) -> Result<JsValue, JsValue> {
        let (dataset, async_format_context) = open_descriptor(descriptor)?;
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
        self.pending_async_blocks.clear();
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
        let (session, browser_read_stats, result) = open_format(file, cache_bytes, attributes)?;
        if let Some(renderer) = &mut self.gpu_renderer {
            renderer.clear_dataset();
            renderer.set_color_mode(ColorMode::Rgb);
        }
        self.lod_session = Some(session);
        self.async_format_context = None;
        self.pending_async_block = None;
        self.pending_async_blocks.clear();
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

        Ok(result)
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
        let accounted_bytes = insert_async_block(
            self.lod_session.as_mut(),
            (self.selected_attribute, self.selected_attribute_range),
            block_id,
            positions,
            colors,
            attribute,
        )?;
        self.pending_async_block = None;
        self.pending_async_blocks.clear();
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

    /// Current missing blocks in the projected-error order chosen by the LOD plan.
    #[wasm_bindgen(js_name = pendingAsyncLodBlocks)]
    pub fn pending_async_lod_blocks(&self) -> js_sys::Array {
        self.pending_async_blocks
            .iter()
            .map(|id| JsValue::from_str(&id.get().to_string()))
            .collect()
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
        lod_update_stats_to_js(
            &stats,
            self.lod_range_read_stats,
            self.lod_session.as_ref().map(LodSession::dataset),
            self.gpu_renderer.as_ref(),
            self.last_lod_error.as_deref(),
            target_voxel_pixels,
        )
    }
}

impl WasmVoxelViewer {
    pub(super) fn refresh_lod_if_dirty(&mut self) -> Result<(), JsValue> {
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

    pub(super) fn update_lod_view(
        &mut self,
        target_voxel_pixels: f64,
    ) -> Result<LodViewStats, JsValue> {
        let requested_mode = self.gpu_renderer.as_ref().map(SurfaceRenderer::color_mode);
        let selected_attribute = self.selected_attribute;
        let selected_range = self.selected_attribute_range;
        let browser_read_stats = Arc::clone(&self.browser_read_stats);
        let mut lod_range_read_stats = self.lod_range_read_stats;
        let has_async_reader = self.async_format_context.is_some();
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
        let view = lod_view_projection(renderer, dataset)?;
        let mut load_block = |block_id| {
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
                .unwrap_or(Ok(None))
        };
        let mut stage_pixels = self.lod_stage_pixels;
        let mut dirty = self.lod_dirty;
        let result = update_lod_view(
            dataset,
            &LodStep {
                view: &view,
                target_pixels: target_voxel_pixels,
                track_async_order: has_async_reader,
                can_load_blocks,
            },
            &mut stage_pixels,
            &mut dirty,
            &mut RendererSink { renderer },
            &mut load_block,
        );
        self.lod_range_read_stats = lod_range_read_stats;
        self.lod_stage_pixels = stage_pixels;
        self.lod_dirty = dirty;
        let LodStepResult {
            stats,
            async_priority_order,
        } = result.map_err(|message| JsValue::from_str(&message))?;
        self.pending_async_blocks = async_priority_order;
        self.pending_async_block = self.pending_async_blocks.first().copied();
        self.last_lod_stats = Some((stats.clone(), lod_range_read_stats));
        Ok(stats)
    }

    pub(super) fn clear_lod_dataset(&mut self) {
        if self.lod_session.take().is_some()
            && let Some(renderer) = &mut self.gpu_renderer
        {
            renderer.clear_dataset();
        }
        self.lod_config = None;
        self.async_format_context = None;
        self.pending_async_block = None;
        self.pending_async_blocks.clear();
        self.lod_stage_pixels = None;
        self.lod_dirty = false;
        self.last_lod_stats = None;
        self.last_lod_error = None;
        self.reset_range_read_stats();
    }

    pub(super) fn reset_range_read_stats(&mut self) {
        reset_browser_read_stats(&self.browser_read_stats);
        self.lod_range_read_stats = BrowserReadStats::default();
    }
}
