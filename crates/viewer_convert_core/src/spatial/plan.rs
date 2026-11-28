//! Building the spatial plan of a conversion.

use std::sync::atomic::{AtomicU64, Ordering};

use viewer_core::{BlockId, DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode};
use viewer_format::FormatMetadata;

use super::block::PlannedBlock;
use super::builder::SpatialBuilder;
use super::executor::{BatchPlanner, RangeRunner};
use super::geometry::quanta;
use super::memory::{
    SPATIAL_WORKER_OVERHEAD_BYTES, check_memory_budget, child_spool_buffer_bytes, estimate_memory,
    range_memory_bytes,
};
use super::spool::SpoolLayout;
use crate::diagnostics::{SpatialPhaseDiagnostics, SpatialSpoolDiagnostics};
use crate::pipeline::{
    clock::Clock,
    point_source::SourceFactory,
    spool_store::{PlanStore, SpoolId, SpoolStore},
};
use crate::report::ConversionProgress;
use crate::source::SourceInfo;
use crate::{ConvertError, ConvertOptions};

pub struct SpatialPlan {
    pub metadata: FormatMetadata,
    pub blocks: Vec<PlannedBlock>,
    pub point_count: u64,
    pub estimated_peak_memory_bytes: u64,
    pub omitted_dimensions: Vec<String>,
    pub spool_diagnostics: Option<SpatialSpoolDiagnostics>,
    pub spatial_phases: Option<SpatialPhaseDiagnostics>,
}

/// Plans from `first`, a source positioned at the start of the points; `factory` reopens the
/// input when the root has to be read again.
#[expect(
    clippy::too_many_arguments,
    reason = "distinct inputs of one planning step; bundling them would add a single-use struct"
)]
pub fn build_spatial_plan_from_source<S, C, F, X>(
    first: &mut F::Source,
    factory: &F,
    source: &SourceInfo,
    options: ConvertOptions,
    store: &S,
    clock: &C,
    executor: &X,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    collect_spool_diagnostics: bool,
) -> Result<SpatialPlan, ConvertError>
where
    S: PlanStore,
    C: Clock,
    F: SourceFactory,
    X: BatchPlanner<S, C> + RangeRunner<S, C, F>,
{
    build_spatial_plan_inner(
        source,
        options,
        store,
        clock,
        executor,
        |builder, bounds| builder.build_root_from_source(first, factory, source, bounds),
        on_progress,
        collect_spool_diagnostics,
    )
}

/// `build_root` plans the root node into the builder.
#[expect(
    clippy::too_many_arguments,
    reason = "distinct inputs of one planning step; bundling them would add a single-use struct"
)]
fn build_spatial_plan_inner<S: PlanStore, C: Clock, X: BatchPlanner<S, C>>(
    source: &SourceInfo,
    options: ConvertOptions,
    store: &S,
    clock: &C,
    executor: &X,
    build_root: impl FnOnce(&mut SpatialBuilder<'_, S, C, X>, SpatialBounds) -> Result<(), ConvertError>,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    collect_spool_diagnostics: bool,
) -> Result<SpatialPlan, ConvertError> {
    let point_limit =
        crate::options::input_batch_point_limit(options.points_per_block(), source.record_length);
    check_memory_budget(
        estimate_memory(
            point_limit,
            0,
            0,
            options.spatial_divisions(),
            options.sample_grid(),
        )?,
        options.max_memory_budget_bytes(),
    )?;
    // Ids are never reused; 0 is `SpoolId::SOURCE`. The counter is host state, not store I/O.
    let spool_ids = AtomicU64::new(1);
    let result = plan_spatial(
        source,
        options,
        store,
        clock,
        executor,
        build_root,
        on_progress,
        collect_spool_diagnostics,
        point_limit,
        &spool_ids,
    );
    if result.is_err() {
        discard_spools(store, spool_ids.load(Ordering::Relaxed));
    }
    result
}

/// Deletes every spool a failed plan may have left (open ones are closed first), so a store
/// without a workspace directory (`MemSpoolStore`, the web) holds none after an error. Ids that
/// were never created or are already deleted are rejected by the store and skipped.
fn discard_spools(store: &impl SpoolStore, next_id: u64) {
    for id in (0..next_id).map(SpoolId) {
        let _ = store.close(id);
        let _ = store.delete(id);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "distinct inputs of one planning step; bundling them would add a single-use struct"
)]
fn plan_spatial<S: PlanStore, C: Clock, X: BatchPlanner<S, C>>(
    source: &SourceInfo,
    options: ConvertOptions,
    store: &S,
    clock: &C,
    executor: &X,
    build_root: impl FnOnce(&mut SpatialBuilder<'_, S, C, X>, SpatialBounds) -> Result<(), ConvertError>,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    collect_spool_diagnostics: bool,
    point_limit: u64,
    spool_ids: &AtomicU64,
) -> Result<SpatialPlan, ConvertError> {
    let root_bounds = source.global.bounds()?;
    let mut builder = SpatialBuilder {
        store,
        clock,
        executor,
        spool_ids,
        point_limit,
        memory_budget: options.max_memory_budget_bytes(),
        spatial_divisions: options.spatial_divisions(),
        sample_grid: options.sample_grid(),
        frame: source.frame,
        layout: SpoolLayout::source(source),
        extras: source
            .las_extra_bytes
            .clone()
            .unwrap_or_else(|| Vec::new().into()),
        node_records: Vec::new(),
        output_blocks: Vec::new(),
        next_block_id: 0,
        on_progress,
        source_point_count: source.point_count,
        collect_distance_quantiles: true,
        nn_sample_threshold: super::nn_distance::sample_threshold(source.point_count),
        parallel_workers: options.spatial_workers(),
        root_ranges: options.root_partition_ranges(),
        ranges_used: 0,
        workers_used: 1,
        spool_diagnostics: collect_spool_diagnostics.then(|| SpatialSpoolDiagnostics {
            ..SpatialSpoolDiagnostics::default()
        }),
        spatial_phases: collect_spool_diagnostics.then(SpatialPhaseDiagnostics::default),
        spool_written_local: 0,
        node_events: Vec::new(),
    };
    build_root(&mut builder, root_bounds)?;

    if let (Some(phases), Some(totals)) = (builder.spatial_phases.as_mut(), store.ledger_totals()) {
        phases.spool_bytes_written = totals.written;
        phases.peak_live_spool_bytes = totals.peak_live;
        phases.node_spool_events = std::mem::take(&mut builder.node_events);
        phases.node_spool_events.sort_by_key(|event| event.node_id);
    }

    let estimated_peak_memory_bytes = estimate_memory(
        point_limit,
        builder.node_records.len(),
        builder.output_blocks.len(),
        options.spatial_divisions(),
        options.sample_grid(),
    )?
    .checked_add(
        (builder.workers_used.saturating_sub(1) as u64)
            .checked_mul(
                SPATIAL_WORKER_OVERHEAD_BYTES
                    .checked_add(child_spool_buffer_bytes(options.spatial_divisions())?)
                    .ok_or(ConvertError::SizeOverflow)?,
            )
            .ok_or(ConvertError::SizeOverflow)?,
    )
    .ok_or(ConvertError::SizeOverflow)?;
    check_memory_budget(
        estimated_peak_memory_bytes,
        options.max_memory_budget_bytes(),
    )?;
    // Ranges were admitted against the budget; their working sets are only reported.
    let estimated_peak_memory_bytes = if builder.ranges_used > 1 {
        let ranged = estimate_memory(
            point_limit,
            0,
            0,
            options.spatial_divisions(),
            options.sample_grid(),
        )?
        .checked_add(
            (builder.ranges_used as u64 - 1)
                .checked_mul(range_memory_bytes(
                    point_limit,
                    options.spatial_divisions(),
                    options.sample_grid(),
                )?)
                .ok_or(ConvertError::SizeOverflow)?,
        )
        .ok_or(ConvertError::SizeOverflow)?;
        estimated_peak_memory_bytes.max(ranged)
    } else {
        estimated_peak_memory_bytes
    };

    let schema = source.schema()?;
    crate::limits::check_reader_attribute_ranges(builder.node_records.len(), schema.len())?;
    let global_frame = global_frame(source, &source.global, &builder.node_records)?;
    let nodes = builder
        .node_records
        .iter()
        .map(|node| {
            let bounds = global_frame.node_bounds(
                node.level,
                global_prefix(global_frame.root_level(), global_frame.lattice_base(), node),
            )?;
            Ok(SpatialNode::new(
                node.id,
                node.parent,
                node.level,
                bounds,
                node.point_count,
                node.geometric_error,
                node.blocks.clone(),
            )?)
        })
        .collect::<Result<Vec<_>, ConvertError>>()?;
    let hierarchy = SpatialHierarchy::new(nodes, schema, Some(source.point_count))?;
    let sampling_policy = format!(
        "sample-grid:{};leaf:{}",
        options.sample_grid(),
        options.points_per_block()
    );
    let mut metadata = FormatMetadata::new(
        DatasetMetadata::new(source.frame, hierarchy),
        None,
        Some(sampling_policy),
        Vec::new(),
    )?
    .with_visual_scale(crate::limits::visual_scale_for_bounds(source.bounds)?)?;
    if let Some(extras) = &source.las_extra_bytes {
        metadata = metadata.with_las_extra_bytes(extras.to_vec());
    }
    metadata = metadata.with_global_frame(global_frame)?;
    metadata = metadata.with_point_distance_quantiles(distance_quantiles(&builder.node_records)?);
    let columns = lod_attribute_columns(&metadata);
    metadata = metadata.with_lod_attribute_columns(columns);
    let display = display_colour_source(metadata.dataset().hierarchy().attributes());
    metadata = metadata.with_display_colour_source(display);
    Ok(SpatialPlan {
        metadata,
        blocks: builder.output_blocks,
        point_count: source.point_count,
        estimated_peak_memory_bytes,
        omitted_dimensions: source.omitted_dimensions(),
        spool_diagnostics: builder.spool_diagnostics,
        spatial_phases: builder.spatial_phases,
    })
}

/// Schema columns the viewer draws from LOD points besides the display colour: intensity and
/// classification, found by their LAS schema names. A source without them gets a shorter list.
fn lod_attribute_columns(metadata: &FormatMetadata) -> Vec<u32> {
    metadata
        .dataset()
        .hierarchy()
        .attributes()
        .iter()
        .enumerate()
        .filter(|(_, attribute)| matches!(attribute.name(), "intensity" | "classification"))
        .map(|(index, _)| index as u32)
        .collect()
}

/// Where leaf display colours come from: the converter sets them to the high byte of `rgb` (or of
/// `intensity` for a cloud without colour), so that column is the source. A column only
/// qualifies as the writer's `DisplayColourSource::validate` accepts it (16-bit; 3 components for
/// `rgb`, 1 for `intensity`); a same-named column of another shape falls through to the next
/// option, and a schema with none keeps the stored colour.
fn display_colour_source(
    attributes: &[viewer_core::AttributeDescriptor],
) -> viewer_format::DisplayColourSource {
    use viewer_format::DisplayColourSource::{Intensity, None, Rgb};
    let find = |name: &str, components: &[u32]| {
        attributes.iter().position(|a| {
            a.name() == name
                && a.data_type() == viewer_core::AttributeDataType::U16
                && components.contains(&a.component_count())
        })
    };
    match (find("rgb", &[3]), find("intensity", &[1])) {
        (Some(index), _) => Rgb(index as u32),
        (Option::None, Some(index)) => Intensity(index as u32),
        (Option::None, Option::None) => None,
    }
}

/// Dataset distance quantiles from the leaves' samples, pooled in node-id order.
fn distance_quantiles(
    records: &[NodeRecord],
) -> Result<viewer_format::PointDistanceQuantiles, ConvertError> {
    let mut ordered: Vec<&NodeRecord> = records.iter().collect();
    ordered.sort_by_key(|node| node.id);
    let pooled = ordered
        .into_iter()
        .flat_map(|node| node.nn_samples.iter().copied())
        .collect();
    let [p1, p5, p50] = super::nn_distance::quantiles_from_samples(pooled);
    Ok(viewer_format::PointDistanceQuantiles::new(p1, p5, p50)?)
}

/// Prefix of a node from its absolute cube: the top `a` bits of its minimum corner minus the
/// lattice base (the flipped sign bit, or the frame origin of a local cloud).
fn global_prefix(root_level: u32, base: [i64; 3], node: &NodeRecord) -> [u64; 3] {
    let level = root_level + node.level;
    let min = quanta(node.bounds.min());
    std::array::from_fn(|axis| {
        ((i128::from(min[axis]) - i128::from(base[axis])) as u64)
            .checked_shr(64 - level)
            .unwrap_or(0)
    })
}

/// Node cells (prefix and spacing) and CRS of a `v7.4-pre` conversion. Internal nodes have
/// spacing `side / 256`; leaves carry the measured value.
fn global_frame(
    source: &SourceInfo,
    root: &crate::source::GlobalRoot,
    records: &[NodeRecord],
) -> Result<viewer_format::GlobalFrame, ConvertError> {
    let parents: std::collections::BTreeSet<NodeId> =
        records.iter().filter_map(|node| node.parent).collect();
    let cells = records
        .iter()
        .map(|node| {
            let level = root.level + node.level;
            let spacing = if parents.contains(&node.id) {
                2.0_f64.powi(41 - level as i32)
            } else {
                node.spacing
            };
            (
                node.id,
                viewer_format::NodeCell::new(
                    global_prefix(root.level, root.lattice_base, node),
                    spacing,
                ),
            )
        })
        .collect();
    Ok(viewer_format::GlobalFrame::new(
        root.level,
        root.prefix,
        root.frame_origin_quanta,
        root.data_max_quanta,
        source.crs.clone(),
        source.scale_offset,
        cells,
    )?)
}

pub(super) struct NodeRecord {
    /// Measured leaf spacing of `v7.4-pre` in CRS units; 0 when not measured.
    pub(super) spacing: f64,
    /// Leaf nearest-neighbour distance samples (quanta²); empty when not collected.
    pub(super) nn_samples: Vec<u64>,
    pub(super) id: NodeId,
    pub(super) parent: Option<NodeId>,
    pub(super) level: u32,
    pub(super) bounds: SpatialBounds,
    pub(super) point_count: u64,
    pub(super) geometric_error: f64,
    pub(super) blocks: Vec<BlockId>,
}

#[cfg(test)]
mod tests {
    use super::display_colour_source;
    use viewer_core::{AttributeDataType, AttributeDescriptor};
    use viewer_format::DisplayColourSource::{Intensity, None, Rgb};

    fn column(name: &str, data_type: AttributeDataType, components: u32) -> AttributeDescriptor {
        AttributeDescriptor::new(name, data_type, components).unwrap()
    }

    #[test]
    fn the_source_is_the_named_column_when_it_is_16_bit() {
        let u16s = |name, n| column(name, AttributeDataType::U16, n);
        let schema = [u16s("intensity", 1), u16s("rgb", 3)];
        assert_eq!(display_colour_source(&schema), Rgb(1));
        assert_eq!(display_colour_source(&schema[..1]), Intensity(0));
        assert_eq!(display_colour_source(&[]), None);
    }

    #[test]
    fn a_same_named_column_of_another_type_falls_through() {
        let u16_intensity = column("intensity", AttributeDataType::U16, 1);
        let u8_rgb = column("rgb", AttributeDataType::U8, 3);
        let two_component_rgb = column("rgb", AttributeDataType::U16, 2);
        let u8_intensity = column("intensity", AttributeDataType::U8, 1);
        assert_eq!(
            display_colour_source(&[u16_intensity.clone(), u8_rgb.clone()]),
            Intensity(0)
        );
        assert_eq!(
            display_colour_source(&[u16_intensity, two_component_rgb.clone()]),
            Intensity(0)
        );
        assert_eq!(display_colour_source(&[u8_intensity, u8_rgb]), None);
        assert_eq!(display_colour_source(&[two_component_rgb]), None);
        // Shapes the writer rejects (one-component rgb, three-component intensity) fall through.
        let one_component_rgb = column("rgb", AttributeDataType::U16, 1);
        let three_component_intensity = column("intensity", AttributeDataType::U16, 3);
        assert_eq!(
            display_colour_source(&[three_component_intensity, one_component_rgb]),
            None
        );
    }
}
