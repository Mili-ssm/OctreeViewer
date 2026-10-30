//! LOD stage machine and colour selection shared by every binding that drives a `LodDataset`.
//!
//! Both `WasmVoxelViewer` (uploads to a GPU) and `WasmSceneBuilder` (hands the scene to another
//! worker) call these functions, so the two paths cannot drift. Nothing here touches a GPU, which
//! is what lets native tests pin both bindings to the same scene digests.

use viewer_core::{AttributeColumn, AttributeDescriptor, BlockId, PointBatch, PointBlock};
use viewer_render_wgpu::{ColorMode, ViewProjection};

use crate::{
    LodBuiltScene, LodDataset, LodViewStats,
    lod_dataset::{WorkClock, next_lod_stage},
};

/// Largest stage (coarsest projected voxel edge, in pixels) a refinement starts from.
const COARSEST_STAGE_PIXELS: f64 = 16.0;

/// Receives each rebuilt scene: the viewer uploads it, the builder keeps its bytes.
pub(crate) trait SceneSink {
    /// An `Err` makes the update fail and the dataset forget the scene, so the next update
    /// rebuilds it instead of reusing something the consumer never received.
    fn publish(&mut self, built: LodBuiltScene) -> Result<(), String>;

    /// The base cut no longer fits the voxel budget: nothing from the dataset stays visible.
    fn base_cut_exceeded(&mut self, dataset: &LodDataset) -> Result<(), String>;
}

/// Inputs of one stage-machine step.
pub(crate) struct LodStep<'a> {
    pub view: &'a ViewProjection,
    pub target_pixels: f64,
    /// Whether the caller wants the missing blocks ordered (blocks arrive asynchronously).
    pub track_async_order: bool,
    /// Whether missing blocks can be supplied, either by a reader or by the host.
    pub can_load_blocks: bool,
}

pub(crate) struct LodStepResult {
    pub stats: LodViewStats,
    pub async_priority_order: Vec<BlockId>,
}

/// Runs one update: starts at the target stage when its cut is already resident, otherwise at
/// the current coarse stage, then advances `stage_pixels`/`dirty` for the next refinement.
pub(crate) fn update_lod_view(
    dataset: &mut LodDataset,
    step: &LodStep<'_>,
    stage_pixels: &mut Option<f64>,
    dirty: &mut bool,
    sink: &mut impl SceneSink,
    load_block: &mut impl FnMut(BlockId) -> Result<Option<PointBlock>, String>,
) -> Result<LodStepResult, String> {
    let target = step.target_pixels;
    if stage_pixels.is_none()
        && target < COARSEST_STAGE_PIXELS
        && dataset
            .target_cut_is_resident(step.view, target)
            .map_err(|error| error.to_string())?
    {
        match update_at_stage(dataset, step, target, stage_pixels, dirty, sink, load_block) {
            Ok(result) => return Ok(result),
            Err(_) => {
                // A fine resident cut can still exceed the voxel/GPU build budget. Retry
                // through the normal coarse stage before reporting the failure.
                *stage_pixels = None;
            }
        }
    }
    let stage = stage_pixels.unwrap_or(COARSEST_STAGE_PIXELS);
    update_at_stage(dataset, step, stage, stage_pixels, dirty, sink, load_block)
}

fn update_at_stage(
    dataset: &mut LodDataset,
    step: &LodStep<'_>,
    stage: f64,
    stage_pixels: &mut Option<f64>,
    dirty: &mut bool,
    sink: &mut impl SceneSink,
    load_block: &mut impl FnMut(BlockId) -> Result<Option<PointBlock>, String>,
) -> Result<LodStepResult, String> {
    let mut async_priority_order = Vec::new();
    let update = dataset.update_view_progressive(
        step.view,
        step.target_pixels,
        stage,
        stage,
        step.track_async_order.then_some(&mut async_priority_order),
        &mut *load_block,
    );
    let mut update = match update {
        Ok(update) => update,
        Err(error) => {
            if error.is_base_cut_exceeded() {
                sink.base_cut_exceeded(dataset)?;
            }
            return Err(error.to_string());
        }
    };
    if let Some(built) = update.scene.take() {
        let started = WorkClock::start();
        let published = sink.publish(built);
        dataset.record_upload_ms(&mut update.stats, started.elapsed_ms());
        if let Err(message) = published {
            dataset.discard_visible_scene();
            return Err(message);
        }
    }
    (*stage_pixels, *dirty) = next_lod_stage(
        Some(stage),
        step.target_pixels,
        !update.stats.pending_block_ids.is_empty(),
        step.can_load_blocks,
    );
    Ok(LodStepResult {
        stats: update.stats,
        async_priority_order,
    })
}

/// A parsed `setColorMode` request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ColorSelection {
    pub mode: ColorMode,
    /// `(schema index, component)` of an attribute painted through the RGB path.
    pub attribute: Option<(u32, u32)>,
    pub attribute_range: Option<[f64; 2]>,
}

/// What the dataset's attribute source says about one `(index, component)`.
pub(crate) struct AttributeAvailability {
    pub has_attributes: bool,
    pub range: Option<[f64; 2]>,
}

/// Parses `rgb | intensity | elevation | classification | attribute:index:component`.
///
/// `schema` is `None` while no dataset is open; `availability` queries the reader (or the
/// host-provided ranges) and returns `None` when no format dataset backs the request.
pub(crate) fn resolve_color_selection(
    mode: &str,
    schema: Option<&[AttributeDescriptor]>,
    availability: impl FnOnce(u32, u32) -> Result<Option<AttributeAvailability>, String>,
) -> Result<ColorSelection, String> {
    let Some(selection) = mode.strip_prefix("attribute:") else {
        let mode = ColorMode::parse(mode).ok_or_else(|| {
            "color mode must be rgb, intensity, elevation, or classification".to_owned()
        })?;
        return Ok(ColorSelection {
            mode,
            attribute: None,
            attribute_range: None,
        });
    };
    let (index, component) = selection
        .split_once(':')
        .ok_or("expected attribute:index:component")?;
    let index = index
        .parse::<u32>()
        .map_err(|_| "invalid attribute index".to_owned())?;
    let component = component
        .parse::<u32>()
        .map_err(|_| "invalid attribute component".to_owned())?;
    let schema = schema.ok_or("no format dataset open")?;
    let descriptor = schema
        .get(index as usize)
        .ok_or("invalid attribute index")?;
    if component >= descriptor.component_count() {
        return Err("invalid attribute component".to_owned());
    }
    let availability = availability(index, component)?.ok_or("no format dataset open")?;
    if !availability.has_attributes {
        return Err("attach the attribute file first".to_owned());
    }
    Ok(ColorSelection {
        mode: ColorMode::Rgb,
        attribute: Some((index, component)),
        attribute_range: availability.range,
    })
}

/// Turns one asynchronously delivered block into a cacheable `PointBlock`. With an attribute
/// selected the column is painted into the colours; otherwise it travels with the block.
pub(crate) fn assemble_async_block(
    block_id: BlockId,
    geometry: PointBatch,
    column: Option<AttributeColumn>,
    schema: &[AttributeDescriptor],
    selected_attribute: Option<(u32, u32)>,
    selected_range: Option<[f64; 2]>,
    budget_bytes: usize,
) -> Result<PointBlock, String> {
    let mut geometry = geometry;
    let mut columns = Vec::new();
    if let Some(column) = column {
        if let Some((schema_index, component)) = selected_attribute {
            if schema
                .get(schema_index as usize)
                .map(AttributeDescriptor::name)
                != Some(column.descriptor().name())
            {
                return Err("async attribute does not match the selected color column".to_owned());
            }
            geometry = viewer_render_wgpu::colorize_attribute(
                geometry,
                &column,
                component,
                selected_range,
            )
            .map_err(str::to_owned)?;
        } else {
            columns.push(column);
        }
    } else if selected_attribute.is_some() {
        return Err("selected attribute payload is required".to_owned());
    }
    let block = PointBlock::new(block_id, geometry, columns);
    if block.estimated_memory_bytes() > budget_bytes {
        return Err("decoded block allocation exceeds the cache budget".to_owned());
    }
    Ok(block)
}
