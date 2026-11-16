//! LOD stage machine and colour selection shared by every binding that drives a `LodDataset`.
//!
//! Both `WasmVoxelViewer` (uploads to a GPU) and `WasmSceneBuilder` (hands the scene to another
//! worker) call these functions, so the two paths cannot drift. Nothing here touches a GPU, which
//! is what lets native tests pin both bindings to the same scene digests.

use viewer_core::{AttributeColumn, AttributeDescriptor, BlockId, NodeId, PointBatch, PointBlock};
use viewer_render_wgpu::{CellDriver, CellTarget, ColorMode, ViewProjection};

use crate::{
    CellUpdate, CellUpdateConfig, LodBuiltScene, LodCellUpdate, LodDataset, LodDatasetError,
    LodViewStats, PageInsert,
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

/// The px ladder shared by the CPU and the cells paths: the first update starts at the target
/// stage when `target_ready` says its cut is already built, otherwise at the current coarse
/// stage. `at_stage` runs one update at a stage and advances `stage_pixels` for the next one. With
/// `retry_coarse` a failure at the target stage (a fine cut can exceed the build budget) is
/// retried through the coarse stage instead of reported.
fn run_stage_ladder<C, T, E>(
    context: &mut C,
    stage_pixels: &mut Option<f64>,
    target: f64,
    retry_coarse: bool,
    target_ready: impl FnOnce(&mut C) -> Result<bool, E>,
    mut at_stage: impl FnMut(&mut C, f64, &mut Option<f64>) -> Result<T, E>,
) -> Result<T, E> {
    if stage_pixels.is_none() && target < COARSEST_STAGE_PIXELS && target_ready(context)? {
        match at_stage(context, target, stage_pixels) {
            Ok(result) => return Ok(result),
            Err(error) if !retry_coarse => return Err(error),
            Err(_) => *stage_pixels = None,
        }
    }
    let stage = stage_pixels.unwrap_or(COARSEST_STAGE_PIXELS);
    at_stage(context, stage, stage_pixels)
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
    run_stage_ladder(
        dataset,
        stage_pixels,
        step.target_pixels,
        true,
        |dataset| {
            dataset
                .target_cut_is_resident(step.view, step.target_pixels)
                .map_err(|error| error.to_string())
        },
        |dataset, stage, stage_pixels| {
            update_at_stage(dataset, step, stage, stage_pixels, dirty, sink, load_block)
        },
    )
}

/// Inputs of one cells ladder step.
pub(crate) struct CellStep<'a> {
    pub view: &'a ViewProjection,
    pub target_pixels: f64,
    pub config: CellUpdateConfig,
    /// Whether missing blocks can be supplied, either by a reader or by the host.
    pub can_load_blocks: bool,
}

/// One cells update and the rung of the ladder it ran at.
pub(crate) struct CellStageUpdate {
    pub update: LodCellUpdate,
    pub stage_pixels: f64,
}

/// Step 1 of the cells ladder and the rung it ran at.
pub(crate) struct CellStagePlan {
    pub update: CellUpdate,
    pub stage_pixels: f64,
}

/// The px ladder of the cells path (R3-Q19), same stage machine as [`update_lod_view`]: coarse
/// to fine, never loading a node's cells before its parents'. Each rung's cut stays as the
/// planner's retained fallback for the next, and the rung halves toward `target_pixels` once the
/// rung's cut has all its cells built. The caller keeps `stage_pixels` and `dirty` between
/// frames (`dirty`: another update is worth running even if the view does not change).
///
/// The ladder starts when the dataset opens (`stage_pixels = None`) and after a camera jump (no
/// node of the cut has cells or an ancestor with cells, R3-Q22); this function detects the jump
/// itself and restarts. Camera moves that keep part of the cut do not reset it: the new nodes
/// are built after their parents (`LodDataset::plan_cell_update`).
///
/// This is step 1 only: it needs no driver, so the build worker runs it alone. `dirty` stays
/// true while builds are deferred by the cap; the driver's own work is the render side's.
pub(crate) fn plan_cells_staged(
    dataset: &mut LodDataset,
    step: &CellStep<'_>,
    stage_pixels: &mut Option<f64>,
    dirty: &mut bool,
    mut async_priority_order: Option<&mut Vec<BlockId>>,
    mut load_block: impl FnMut(BlockId) -> Result<Option<PointBlock>, String>,
) -> Result<CellStagePlan, LodDatasetError> {
    if let Some(stage) = *stage_pixels
        && dataset.camera_jumped(step.view, stage)?
    {
        *stage_pixels = None;
        *dirty = true;
    }
    run_stage_ladder(
        dataset,
        stage_pixels,
        step.target_pixels,
        false,
        |dataset| dataset.target_cells_are_ready(step.view, step.target_pixels),
        |dataset, stage, stage_pixels| {
            let update = dataset.plan_cell_update(
                step.view,
                stage,
                step.config,
                async_priority_order.as_deref_mut(),
                &mut load_block,
            )?;
            // Nodes deferred by the build cap are pending too: the rung is not left, and the
            // update repeats, until every selected node has its cells.
            let stats = &update.stats;
            let pending = !stats.pending_block_ids.is_empty()
                || !stats.pending_page_anchors.is_empty()
                || stats.build_deferred > 0;
            let levels_pending = dataset.has_pending_level_requests();
            (*stage_pixels, *dirty) = next_lod_stage(
                Some(stage),
                step.target_pixels,
                pending,
                step.can_load_blocks,
            );
            *dirty |= stats.build_deferred > 0 || levels_pending;
            Ok(CellStagePlan {
                update,
                stage_pixels: stage,
            })
        },
    )
}

/// [`plan_cells_staged`] followed by the driver's frame and the acknowledgement, in one thread.
/// `dirty` also stays true while the driver has work (`CellDriver::needs_frame`), which includes
/// a permanent pool overflow and a failed frame: the caller stops on `pool_overflow > 0` or on
/// errors that repeat.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_cells_staged<T: CellTarget>(
    dataset: &mut LodDataset,
    step: &CellStep<'_>,
    stage_pixels: &mut Option<f64>,
    dirty: &mut bool,
    driver: &mut CellDriver,
    target: &mut T,
    async_priority_order: Option<&mut Vec<BlockId>>,
    load_block: impl FnMut(BlockId) -> Result<Option<PointBlock>, String>,
) -> Result<CellStageUpdate, LodDatasetError> {
    dataset.reconcile_driver(driver)?;
    let planned = plan_cells_staged(
        dataset,
        step,
        stage_pixels,
        dirty,
        async_priority_order,
        load_block,
    )?;
    let update = dataset.draw_planned_cells(
        planned.update,
        step.view,
        step.target_pixels,
        driver,
        target,
    )?;
    // Builds and the driver's own work (uploads, retained cut, a target that lost its pool) need
    // no new input.
    *dirty |= driver.needs_frame_on(&*target) || dataset.has_pending_level_requests();
    Ok(CellStageUpdate {
        update,
        stage_pixels: planned.stage_pixels,
    })
}

/// Most page-fetch rounds one synchronous update runs before it returns what it has.
const MAX_PAGE_ROUNDS: usize = 32;

/// [`update_lod_view`] for a reader that serves blocks and hierarchy pages from the same stream:
/// after each step, the pages the cut lacks are read and grafted and the step is repeated until
/// none is missing. Hosts that deliver pages themselves pass `reader = None`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_lod_view_paged<R>(
    dataset: &mut LodDataset,
    step: &LodStep<'_>,
    stage_pixels: &mut Option<f64>,
    dirty: &mut bool,
    sink: &mut impl SceneSink,
    mut reader: Option<&mut R>,
    load_block: &mut impl FnMut(&mut R, BlockId) -> Result<Option<PointBlock>, String>,
    load_page: &mut impl FnMut(&mut R, NodeId) -> Result<PageInsert, String>,
) -> Result<LodStepResult, String> {
    let mut rounds = 0;
    loop {
        let mut load = |id| match reader.as_deref_mut() {
            Some(reader) => load_block(reader, id),
            None => Ok(None),
        };
        let result = update_lod_view(dataset, step, stage_pixels, dirty, sink, &mut load)?;
        rounds += 1;
        let Some(reader) = reader.as_deref_mut() else {
            return Ok(result);
        };
        if result.stats.pending_page_anchors.is_empty() || rounds == MAX_PAGE_ROUNDS {
            return Ok(result);
        }
        for anchor in &result.stats.pending_page_anchors {
            let page = load_page(reader, *anchor)?;
            dataset
                .graft_page(page)
                .map_err(|error| error.to_string())?;
        }
    }
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
        update.stats.has_pending_loads(),
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
/// `carried` is `Some` on the cells path only: its columns, and a painted one, travel with the block (R3-Q32).
#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble_async_block(
    block_id: BlockId,
    geometry: PointBatch,
    column: Option<AttributeColumn>,
    carried: Option<Vec<AttributeColumn>>,
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
        }
        // On the cells path (`carried` is `Some`) the painted column stays with the block too:
        // the cells keep intensity and class words even when that column is the painted one
        // (R3-Q32). The CPU path keeps only what it always did.
        if selected_attribute.is_none() || carried.is_some() {
            columns.push(column);
        }
    } else if selected_attribute.is_some() {
        return Err("selected attribute payload is required".to_owned());
    }
    columns.extend(carried.into_iter().flatten());
    let block = PointBlock::new(block_id, geometry, columns);
    if block.estimated_memory_bytes() > budget_bytes {
        return Err("decoded block allocation exceeds the cache budget".to_owned());
    }
    Ok(block)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs frames of the ladder the way the drivers do (`next_lod_stage` after each rung, no
    /// pending blocks) and returns the rungs in the order they were drawn.
    fn drawn_rungs(target: f64, target_ready: bool, retry_coarse: bool) -> Vec<f64> {
        let mut stage = None;
        let mut drawn = Vec::new();
        for _ in 0..8 {
            let _ = run_stage_ladder::<(), (), ()>(
                &mut (),
                &mut stage,
                target,
                retry_coarse,
                |_| Ok(target_ready),
                |_, rung, stage_pixels| {
                    drawn.push(rung);
                    *stage_pixels = next_lod_stage(Some(rung), target, false, true).0;
                    Ok(())
                },
            );
            if drawn.last() == Some(&target) {
                break;
            }
        }
        drawn
    }

    /// RF-06: fixed mode draws what is resident at a coarser rung first and refines frame by
    /// frame, instead of waiting for the target's cut.
    #[test]
    fn fixed_mode_draws_a_coarse_rung_before_the_target() {
        // `true` is the CPU path (`update_lod_view`), `false` the cells path (`plan_cells_staged`).
        for retry_coarse in [true, false] {
            assert_eq!(drawn_rungs(2.0, false, retry_coarse), [16.0, 8.0, 4.0, 2.0]);
            assert_eq!(
                drawn_rungs(0.5, false, retry_coarse),
                [16.0, 8.0, 4.0, 2.0, 1.0, 0.5]
            );
        }
    }

    /// A target whose cut is already built is drawn straight away (nothing coarser to show).
    #[test]
    fn a_resident_target_cut_is_drawn_first() {
        for retry_coarse in [true, false] {
            assert_eq!(drawn_rungs(2.0, true, retry_coarse), [2.0]);
        }
    }
}
