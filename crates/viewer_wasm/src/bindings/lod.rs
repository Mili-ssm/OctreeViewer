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
    DEFAULT_PAGE_BUDGET_BYTES, LodBuiltScene, LodDataset, LodDatasetError, LodViewStats,
    PageInsert, WasmAsyncFormatReader, WasmPageGraft,
    browser_io::{BrowserBlobReader, BrowserReadStats},
    lod_driver::{LodStep, LodStepResult, SceneSink, assemble_async_block, update_lod_view_paged},
    lod_metadata::LodDatasetDescriptor,
    lod_session::LodSession,
};
use viewer_core::NodeId;
use viewer_format::{
    FormatReader, FormatReaderLimits, OctreeReader, PageGraft, PageSource, probe_format,
};
use viewer_render_wgpu::{ColorMode, SurfaceRenderer, ViewProjection};

/// Uploads each rebuilt scene to the viewer's own GPU renderer.
struct RendererSink<'a, 'r> {
    renderer: &'a mut SurfaceRenderer<'r>,
    /// Digest of the scene the renderer currently displays; `None` while it shows nothing.
    digest: &'a mut Option<u64>,
}

impl SceneSink for RendererSink<'_, '_> {
    fn publish(&mut self, built: LodBuiltScene) -> Result<(), String> {
        self.renderer
            .upload_voxel_cut(&built.voxels.scene, &built.block_ids)
            .map_err(|error| LodDatasetError::Gpu(error).to_string())?;
        *self.digest = Some(built.scene_digest());
        Ok(())
    }

    fn base_cut_exceeded(&mut self, dataset: &LodDataset) -> Result<(), String> {
        *self.digest = None;
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

/// Builds the dataset of a paged format: the root page's nodes are resident, every other page is
/// grafted later with [`graft_page`]. Attribute ranges come from the root page's nodes.
pub(super) fn open_paged(
    reader: &WasmAsyncFormatReader,
    cache_bytes: u32,
) -> Result<(LodDataset, Option<AsyncFormatContext>), JsValue> {
    let format = reader.format_reader();
    let source = format
        .page_source()
        .ok_or_else(|| JsValue::from_str("the format is not paged"))?
        .clone();
    let metadata = format
        .format_metadata()
        .viewer_dataset()
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    let crs = format
        .format_metadata()
        .global_frame()
        .and_then(|_| format.format_metadata().crs())
        .map(str::to_owned);
    let mut attribute_ranges = Vec::new();
    for (schema_index, attribute) in metadata.hierarchy().attributes().iter().enumerate() {
        let mut row = Vec::new();
        for component in 0..attribute.component_count() {
            row.push(
                format
                    .attribute_range(schema_index as u32, component)
                    .map_err(|error| JsValue::from_str(&error.to_string()))?,
            );
        }
        attribute_ranges.push(row);
    }
    let dataset = LodDataset::from_shared_metadata_and_visual_scale(
        metadata,
        format.format_metadata().visual_scale(),
        cache_bytes as usize,
    )
    .with_crs(crs)
    .with_global_frame(format.format_version().uses_global_frame());
    let dataset = with_page_source(dataset, &source)?;
    let context = AsyncFormatContext {
        has_attributes: format.has_attributes(),
        attribute_ranges,
    };
    Ok((dataset, Some(context)))
}

/// Makes `dataset` (holding a paged file's root page) evict pages through `source`.
fn with_page_source(dataset: LodDataset, source: &Arc<PageSource>) -> Result<LodDataset, JsValue> {
    let release_source = Arc::clone(source);
    dataset
        .with_reference_spacing(source.reference_spacing())
        .with_pages(
            source.root_unloaded(),
            Box::new(move |anchor| release_source.release_page(anchor)),
            DEFAULT_PAGE_BUDGET_BYTES,
        )
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

/// A page read by a synchronous reader, ready to graft.
fn page_insert(graft: PageGraft) -> Result<PageInsert, String> {
    Ok(PageInsert {
        anchor: graft.anchor.ok_or("the root page is grafted at open")?,
        number: graft.number,
        nodes: graft.nodes,
        unloaded: graft.unloaded,
        bytes: graft.encoded_len,
    })
}

/// Reads the page hanging from `anchor` with a session's own reader.
pub(super) fn read_page_insert(
    reader: &mut FormatReader<BrowserBlobReader>,
    anchor: NodeId,
) -> Result<PageInsert, String> {
    reader
        .read_page(anchor)
        .map_err(|error| error.to_string())
        .and_then(page_insert)
}

/// Grafts a page fetched by the reader into `dataset`, evicting least-recently-used pages that
/// the cut does not need when the page budget is full.
pub(super) fn graft_page(
    dataset: &mut LodDataset,
    mut graft: WasmPageGraft,
) -> Result<(), JsValue> {
    let page = page_insert(graft.take()?).map_err(|message| JsValue::from_str(&message))?;
    dataset
        .graft_page(page)
        .map_err(|error| JsValue::from_str(&error.to_string()))
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
    let mut source = source;
    let identity =
        probe_format(&mut source).map_err(|error| JsValue::from_str(&error.to_string()))?;
    // A paged file opens lazily: only its root page is resident, the rest is read on demand.
    let mut reader = if identity.version.uses_pages() {
        FormatReader::open_lazy(source, FormatReaderLimits::default())
    } else {
        FormatReader::new(source)
    }
    .map_err(|error| JsValue::from_str(&error.to_string()))?;
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
    let metadata = reader
        .format_metadata()
        .viewer_dataset()
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    let coordinate_origin = metadata.coordinate_frame().origin();
    let semantics = format!("{:?}", metadata.hierarchy().semantics()).to_lowercase();
    let root_bounds = metadata.hierarchy().stats().data_bounds;
    let crs = reader
        .format_metadata()
        .global_frame()
        .and_then(|_| reader.format_metadata().crs())
        .map(str::to_owned);
    let dataset = LodDataset::from_shared_metadata_and_visual_scale(
        metadata,
        visual_scale,
        cache_bytes as usize,
    )
    .with_crs(crs)
    .with_global_frame(reader.format_version().uses_global_frame());
    let page_source = reader.page_source().cloned();
    let dataset = match &page_source {
        Some(source) => with_page_source(dataset, source)?,
        None => dataset,
    };
    let result = js_sys::Object::new();
    js_sys::Reflect::set(
        &result,
        &"status".into(),
        &JsValue::from_str("format_ready"),
    )?;
    js_sys::Reflect::set(
        &result,
        &"nodeCount".into(),
        &JsValue::from_f64(
            page_source
                .as_ref()
                .map_or(hierarchy_stats.node_count as f64, |source| {
                    source.declared_nodes() as f64
                }),
        ),
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
    /// Opens a paged format: the reader's root page is resident, other pages arrive with
    /// `graftPage` as the LOD asks for them (`pendingPageAnchors`).
    #[wasm_bindgen(js_name = openPagedLodDataset)]
    pub fn open_paged_lod_dataset(
        &mut self,
        reader: &WasmAsyncFormatReader,
        cache_bytes: u32,
    ) -> Result<JsValue, JsValue> {
        let (dataset, async_format_context) = open_paged(reader, cache_bytes)?;
        let result = self.open_lod_dataset_from(dataset, async_format_context)?;
        Ok(result)
    }

    /// Pages the last `updateLod` needed and lacked, highest projected error first.
    #[wasm_bindgen(js_name = pendingPageAnchors)]
    pub fn pending_page_anchors(&self) -> js_sys::Array {
        self.last_lod_stats
            .as_ref()
            .map(|stats| {
                stats
                    .0
                    .pending_page_anchors
                    .iter()
                    .map(|id| JsValue::from_str(&id.get().to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Grafts a page returned by `WasmAsyncFormatReader.loadPage`.
    #[wasm_bindgen(js_name = graftPage)]
    pub fn graft_page(&mut self, page: WasmPageGraft) -> Result<(), JsValue> {
        let session = self
            .lod_session
            .as_mut()
            .ok_or_else(|| JsValue::from_str("no LOD dataset is open"))?;
        graft_page(session.dataset_mut(), page)?;
        self.lod_dirty = true;
        Ok(())
    }

    /// Opens resident hierarchy metadata for decoded block streaming.
    #[wasm_bindgen(js_name = openLodDataset)]
    pub fn open_lod_dataset(&mut self, descriptor: JsValue) -> Result<JsValue, JsValue> {
        let (dataset, async_format_context) = open_descriptor(descriptor)?;
        self.open_lod_dataset_from(dataset, async_format_context)
    }
}

impl WasmVoxelViewer {
    fn open_lod_dataset_from(
        &mut self,
        dataset: LodDataset,
        async_format_context: Option<AsyncFormatContext>,
    ) -> Result<JsValue, JsValue> {
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
        self.last_scene_digest = None;
        self.last_lod_error = None;
        self.gpu_scene_dirty = false;
        Ok(result.into())
    }
}

#[wasm_bindgen]
impl WasmVoxelViewer {
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
        self.last_scene_digest = None;
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
        let (dataset, format_reader) = session.parts_mut();
        let can_load_blocks = format_reader.is_some() || has_async_reader;
        let view = lod_view_projection(renderer, dataset)?;
        let mut load_block = |reader: &mut FormatReader<BrowserBlobReader>, block_id| {
            let before = snapshot_browser_read_stats(&browser_read_stats);
            let loaded = crate::block_loading::load_render_block(
                reader,
                block_id,
                requested_mode,
                selected_attribute,
                selected_range,
            )
            .map(Some);
            let after = snapshot_browser_read_stats(&browser_read_stats);
            accumulate_browser_read_delta(&mut lod_range_read_stats, before, after);
            loaded
        };
        let mut stage_pixels = self.lod_stage_pixels;
        let mut dirty = self.lod_dirty;
        let mut scene_digest = self.last_scene_digest;
        let result = update_lod_view_paged(
            dataset,
            &LodStep {
                view: &view,
                target_pixels: target_voxel_pixels,
                track_async_order: has_async_reader,
                can_load_blocks,
            },
            &mut stage_pixels,
            &mut dirty,
            &mut RendererSink {
                renderer,
                digest: &mut scene_digest,
            },
            format_reader,
            &mut load_block,
            &mut read_page_insert,
        );
        self.lod_range_read_stats = lod_range_read_stats;
        self.last_scene_digest = scene_digest;
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
        self.last_scene_digest = None;
        self.last_lod_error = None;
        self.reset_range_read_stats();
    }

    pub(super) fn reset_range_read_stats(&mut self) {
        reset_browser_read_stats(&self.browser_read_stats);
        self.lod_range_read_stats = BrowserReadStats::default();
    }
}
