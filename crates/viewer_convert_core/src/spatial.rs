use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    sync::atomic::{AtomicU64, Ordering},
};

use las::PointData;
use viewer_core::las::{LasExtraByte, extra::MAX_EXTRA_BYTES_PER_POINT, las_flags_of_record};
use viewer_core::{
    AttributeColumn, AttributeValues, BlockId, ColorRgb, DatasetMetadata, NodeId, SpatialBounds,
    SpatialHierarchy, SpatialNode,
};
use viewer_format::{FormatMetadata, SourcePointBlock};

use super::{ConvertError, ConvertOptions};
use crate::diagnostics::{NodeSpoolEvent, SpatialPhaseDiagnostics, SpatialSpoolDiagnostics};
use crate::pipeline::{
    clock::Clock,
    executor::{BatchPlanner, RangeRunner},
    node_job::{
        BlockPlan, ChildOutput, JobContext, JobMetrics, JobProgress, NodeJob, NodeJobOutput,
        NodePath, RangeJob, RootPartition, lod_blocks, merge_root_ranges, run_node_job,
        run_root_partition, spool_source, write_lod,
    },
    point_source::{PointRange, SourceFactory},
    spool_store::{PlanStore, SPOOL_IO_BUFFER_BYTES, SpoolExtent, SpoolId, SpoolStore},
};
use crate::report::{ConversionProgress, ConversionStage, emit_progress};
use crate::source::{SourceInfo, next_descriptor};

pub(crate) const MAX_SPOOL_RECORD_BYTES: usize = 55 + 1 + MAX_EXTRA_BYTES_PER_POINT;
const CHILD_SPOOL_BUFFER_BYTES: usize = 32 * 1024;
pub const MAX_SPATIAL_DEPTH: u32 = 32;

/// Deepest job depth `consume_rank` accepts. Global-frame layouts split up to absolute level
/// `GLOBAL_MAX_INTERNAL_LEVEL`, deeper than the legacy `MAX_SPATIAL_DEPTH`; the headroom keeps
/// `(MAX_CONSUME_DEPTH + 1) << 16` inside `u32`.
pub const MAX_CONSUME_DEPTH: u32 = 64;
const _: () = assert!(MAX_CONSUME_DEPTH > crate::pipeline::node_job::GLOBAL_MAX_INTERNAL_LEVEL);
const _: () = assert!(MAX_CONSUME_DEPTH > MAX_SPATIAL_DEPTH);

/// Consume-order rank of the child spool `key` at `depth` (>= 1). The depth-first planner
/// consumes deeper spools first and, at equal depth, the lower key first; the queued spools of
/// one depth always share a parent, so `key` alone orders them.
pub fn consume_rank(depth: u32, key: u16) -> u32 {
    ((MAX_CONSUME_DEPTH + 1 - depth) << 16) | u32::from(key)
}
const PLANNED_NODE_BYTES: u64 = 256;
const SPATIAL_WORKER_OVERHEAD_BYTES: u64 = 1024 * 1024;
const HEADER_BOUNDS_RELATIVE_TOLERANCE: f64 = 1.0e-7;
const HEADER_BOUNDS_MAX_TOLERANCE: f64 = 0.5;

/// All temporary files for a source share these attributes and transforms.
/// Source coordinates stay on the exact LAS integer grid; generated LOD centers use f64.
#[derive(Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent layout flags of the spool record"
)]
pub struct SpoolLayout {
    transforms: las::Vector<las::Transform>,
    source_coordinates: bool,
    has_gps_time: bool,
    has_color: bool,
    has_nir: bool,
    /// `v7.5-pre`: the record carries the `las_flags` byte and `extra_len` raw Extra Bytes.
    has_flags: bool,
    extra_len: u8,
    is_extended: bool,
    /// `v7.4-pre`: source positions are snapped to the 2^-15 grid so that node and cell
    /// membership is decided on exact integers.
    snap_to_quanta: bool,
}

impl SpoolLayout {
    pub const fn is_global(self) -> bool {
        self.snap_to_quanta
    }

    pub fn source(source: &SourceInfo) -> Self {
        Self {
            transforms: source.transforms,
            source_coordinates: true,
            has_gps_time: source.point_format.has_gps_time,
            has_color: source.point_format.has_color,
            has_nir: source.point_format.has_nir,
            has_flags: source.las_extra_bytes.is_some(),
            // Checked against `MAX_EXTRA_BYTES_PER_POINT` by `SourceInfo::schema`.
            extra_len: source.las_extra_bytes.as_deref().map_or(0, |extras| {
                extras.iter().map(LasExtraByte::byte_len).sum::<usize>() as u8
            }),
            is_extended: source.point_format.is_extended,
            snap_to_quanta: source.global.is_some(),
        }
    }

    #[must_use]
    pub fn lod(self) -> Self {
        Self {
            source_coordinates: false,
            ..self
        }
    }

    #[inline]
    pub fn record_bytes(self) -> usize {
        let coordinates = if self.source_coordinates { 12 } else { 24 };
        coordinates
            + 15
            + usize::from(self.has_gps_time) * 8
            + usize::from(self.has_color) * 6
            + usize::from(self.has_nir) * 2
            + usize::from(self.has_flags)
            + usize::from(self.extra_len)
    }

    /// Offset of the RGB16 triplet in a record (valid when `has_color`).
    #[inline]
    fn rgb_offset(self) -> usize {
        (if self.source_coordinates { 12 } else { 24 }) + 12 + usize::from(self.has_gps_time) * 8
    }

    /// The LAS coordinate before any snapping (header bounds are checked against it).
    #[inline]
    fn exact_position(self, raw: [i32; 3]) -> [f64; 3] {
        [
            self.transforms.x.direct(raw[0]),
            self.transforms.y.direct(raw[1]),
            self.transforms.z.direct(raw[2]),
        ]
    }

    #[inline]
    fn position(self, raw: [i32; 3]) -> [f64; 3] {
        let exact = self.exact_position(raw);
        if self.snap_to_quanta {
            exact.map(|value| {
                viewer_format::quantize_coordinate(value)
                    .map_or(value, viewer_format::quanta_to_units)
            })
        } else {
            exact
        }
    }
}

#[derive(Clone, Copy)]
pub struct SpoolPoint {
    raw_position: [i32; 3],
    pub position: [f64; 3],
    intensity: u16,
    classification: u8,
    return_number: u8,
    returns_per_pulse: u8,
    scan_angle: f32,
    user_data: u8,
    point_source_id: u16,
    gps_time: f64,
    rgb: [u16; 3],
    nir: u16,
    las_flags: u8,
    /// Raw Extra Bytes of the record; only the first `SpoolLayout::extra_len` are meaningful.
    extra: [u8; MAX_EXTRA_BYTES_PER_POINT],
    display_color: ColorRgb,
}

pub struct PlannedBlock {
    pub id: BlockId,
    spool: SpoolId,
    offset_points: u64,
    point_count: usize,
    layout: SpoolLayout,
    /// Descriptors of the Extra Byte columns (`v7.5-pre`); empty otherwise.
    extras: std::sync::Arc<[LasExtraByte]>,
    pub is_source_points: bool,
    // Blocks sharing a spool stay in construction order when streamed. Only the
    // last range may release it; blocks are written in plan order, so by then every other
    // read of the spool has finished.
    final_block_in_spool: bool,
}

impl PlannedBlock {
    pub fn point_count(&self) -> usize {
        self.point_count
    }

    pub fn payload_bytes(&self) -> Result<u64, ConvertError> {
        u64::try_from(self.point_count)
            .ok()
            .and_then(|points| points.checked_mul(self.layout.record_bytes() as u64))
            .ok_or(ConvertError::SizeOverflow)
    }

    pub fn release_spool_after_write(&self, store: &impl SpoolStore) -> Result<(), ConvertError> {
        if self.final_block_in_spool {
            store.delete(self.spool)?;
        }
        Ok(())
    }

    pub fn read<S: SpoolStore>(
        &self,
        store: &S,
        frame: viewer_core::CoordinateFrame,
    ) -> Result<SourcePointBlock, ConvertError> {
        let record_bytes_wide = self.layout.record_bytes() as u64;
        let extent = SpoolExtent {
            offset: self
                .offset_points
                .checked_mul(record_bytes_wide)
                .ok_or(ConvertError::SizeOverflow)?,
            len: (self.point_count as u64)
                .checked_mul(record_bytes_wide)
                .ok_or(ConvertError::SizeOverflow)?,
        };
        let mut reader = store.reader(self.spool, extent)?;
        let record_bytes = self.layout.record_bytes();
        let batch_capacity = (SPOOL_IO_BUFFER_BYTES / record_bytes).max(1);
        let mut record_buffer = vec![0; batch_capacity * record_bytes];
        let mut positions = Vec::with_capacity(self.point_count);
        let mut colors = Vec::with_capacity(self.point_count);
        let mut intensities = Vec::with_capacity(self.point_count);
        let mut classifications = Vec::with_capacity(self.point_count);
        let mut return_numbers = Vec::with_capacity(self.point_count);
        let mut returns_per_pulse = Vec::with_capacity(self.point_count);
        let mut scan_angles = Vec::with_capacity(self.point_count);
        let mut user_data = Vec::with_capacity(self.point_count);
        let mut point_source_ids = Vec::with_capacity(self.point_count);
        let mut gps_times = Vec::with_capacity(if self.layout.has_gps_time {
            self.point_count
        } else {
            0
        });
        let mut rgb_values = Vec::with_capacity(if self.layout.has_color {
            self.point_count.saturating_mul(3)
        } else {
            0
        });
        let mut nir_values = Vec::with_capacity(if self.layout.has_nir {
            self.point_count
        } else {
            0
        });
        let mut las_flags = Vec::with_capacity(if self.layout.has_flags {
            self.point_count
        } else {
            0
        });
        let mut extra_values: Vec<_> = self
            .extras
            .iter()
            .map(|extra| extra.new_values(self.point_count))
            .collect();
        let mut remaining = self.point_count;
        while remaining > 0 {
            let requested = remaining.min(batch_capacity);
            let buffer_bytes = requested * record_bytes;
            let records =
                read_records(&mut reader, &mut record_buffer[..buffer_bytes], self.layout)?;
            if records != requested {
                return Err(ConvertError::PointCountMismatch {
                    expected: self.point_count as u64,
                    actual: (self.point_count - remaining + records) as u64,
                });
            }
            for bytes in record_buffer[..buffer_bytes].chunks_exact(record_bytes) {
                let point = decode_point(bytes, self.layout);
                positions.push(std::array::from_fn(|axis| {
                    point.position[axis] - frame.origin()[axis]
                }));
                colors.push(point.display_color);
                intensities.push(point.intensity);
                classifications.push(point.classification);
                return_numbers.push(point.return_number);
                returns_per_pulse.push(point.returns_per_pulse);
                scan_angles.push(point.scan_angle);
                user_data.push(point.user_data);
                point_source_ids.push(point.point_source_id);
                if self.layout.has_gps_time {
                    gps_times.push(point.gps_time);
                }
                if self.layout.has_color {
                    rgb_values.extend_from_slice(&point.rgb);
                }
                if self.layout.has_nir {
                    nir_values.push(point.nir);
                }
                if self.layout.has_flags {
                    las_flags.push(point.las_flags);
                }
                let mut offset = 0;
                for (extra, values) in self.extras.iter().zip(&mut extra_values) {
                    let end = offset + extra.byte_len();
                    extra.push_values(&point.extra[offset..end], values);
                    offset = end;
                }
            }
            remaining -= records;
        }

        let mut descriptors =
            viewer_core::las::las_attribute_schema_for(&viewer_core::las::LasSchemaSpec {
                has_gps_time: self.layout.has_gps_time,
                has_color: self.layout.has_color,
                has_nir: self.layout.has_nir,
                las_flags: self.layout.has_flags,
                extra_bytes: &self.extras,
            })?
            .into_iter();
        let mut attributes = Vec::with_capacity(10);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "intensity"),
            AttributeValues::U16(intensities),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "classification"),
            AttributeValues::U8(classifications),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "return_number"),
            AttributeValues::U8(return_numbers),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "number_of_returns"),
            AttributeValues::U8(returns_per_pulse),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "scan_angle_degrees"),
            AttributeValues::F32(scan_angles),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "user_data"),
            AttributeValues::U8(user_data),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "point_source_id"),
            AttributeValues::U16(point_source_ids),
            self.point_count,
        )?);
        if self.layout.has_gps_time {
            attributes.push(AttributeColumn::new(
                next_descriptor(&mut descriptors, "gps_time"),
                AttributeValues::F64(gps_times),
                self.point_count,
            )?);
        }
        if self.layout.has_color {
            attributes.push(AttributeColumn::new(
                next_descriptor(&mut descriptors, "rgb"),
                AttributeValues::U16(rgb_values),
                self.point_count,
            )?);
        }
        if self.layout.has_nir {
            attributes.push(AttributeColumn::new(
                next_descriptor(&mut descriptors, "nir"),
                AttributeValues::U16(nir_values),
                self.point_count,
            )?);
        }
        if self.layout.has_flags {
            attributes.push(AttributeColumn::new(
                next_descriptor(&mut descriptors, viewer_core::las::extra::LAS_FLAGS_NAME),
                AttributeValues::U8(las_flags),
                self.point_count,
            )?);
        }
        for values in extra_values {
            let descriptor = descriptors
                .next()
                .expect("LAS attribute schema covers every column");
            attributes.push(AttributeColumn::new(descriptor, values, self.point_count)?);
        }
        Ok(SourcePointBlock::new(
            self.id, positions, colors, attributes,
        ))
    }
}

pub struct SpatialPlan {
    pub metadata: FormatMetadata,
    pub blocks: Vec<PlannedBlock>,
    pub point_count: u64,
    pub estimated_peak_memory_bytes: u64,
    pub omitted_dimensions: Vec<String>,
    pub spool_diagnostics: Option<SpatialSpoolDiagnostics>,
    pub spatial_phases: Option<SpatialPhaseDiagnostics>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "distinct inputs of one planning step; bundling them would add a single-use struct"
)]
pub fn build_spatial_plan<S: PlanStore, C: Clock, X: BatchPlanner<S, C>>(
    source: &SourceInfo,
    options: ConvertOptions,
    store: &S,
    clock: &C,
    executor: &X,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    collect_spool_diagnostics: bool,
    source_spool_bytes_written: u64,
) -> Result<SpatialPlan, ConvertError> {
    build_spatial_plan_inner(
        source,
        options,
        store,
        clock,
        executor,
        // `SpoolId::SOURCE` already holds the closed source spool.
        true,
        |builder, bounds| {
            let job = builder.node_job(
                NodePath(Vec::new()),
                0,
                bounds,
                SpoolId::SOURCE,
                source.point_count,
                false,
            );
            builder.build_node(job, None).map(|_| ())
        },
        on_progress,
        collect_spool_diagnostics,
        source_spool_bytes_written,
    )
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
        false,
        |builder, bounds| builder.build_root_from_source(first, factory, source, bounds),
        on_progress,
        collect_spool_diagnostics,
        0,
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
    keep_source: bool,
    build_root: impl FnOnce(&mut SpatialBuilder<'_, S, C, X>, SpatialBounds) -> Result<(), ConvertError>,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    collect_spool_diagnostics: bool,
    source_spool_bytes_written: u64,
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
        source_spool_bytes_written,
        point_limit,
        &spool_ids,
    );
    if result.is_err() {
        discard_spools(store, spool_ids.load(Ordering::Relaxed), keep_source);
    }
    result
}

/// Deletes every spool a failed plan may have left (open ones are closed first), so a store
/// without a workspace directory (`MemSpoolStore`, the web) holds none after an error. Ids that
/// were never created or are already deleted are rejected by the store and skipped. A checkpoint
/// `SOURCE` belongs to the caller and is kept.
fn discard_spools(store: &impl SpoolStore, next_id: u64, keep_source: bool) {
    for id in (u64::from(keep_source)..next_id).map(SpoolId) {
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
    source_spool_bytes_written: u64,
    point_limit: u64,
    spool_ids: &AtomicU64,
) -> Result<SpatialPlan, ConvertError> {
    let root_bounds = match &source.global {
        Some(root) => root.bounds()?,
        None => cube_bounds(source.bounds),
    };
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
        parallel_workers: options.spatial_workers(),
        root_ranges: options.root_partition_ranges(),
        ranges_used: 0,
        workers_used: 1,
        spool_diagnostics: collect_spool_diagnostics.then(|| SpatialSpoolDiagnostics {
            source_payload_written_bytes: source_spool_bytes_written,
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
    let global_frame = source
        .global
        .as_ref()
        .map(|root| global_frame(source, root, &builder.node_records))
        .transpose()?;
    let nodes = builder
        .node_records
        .iter()
        .map(|node| {
            let bounds = match &global_frame {
                Some(frame) => frame.node_bounds(
                    node.level,
                    global_prefix(frame.root_level(), frame.lattice_base(), node),
                )?,
                None => node.bounds,
            };
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
    let sampling_policy = if global_frame.is_some() {
        format!(
            "sample-grid:{};leaf:{}",
            options.sample_grid(),
            crate::options::GLOBAL_LEAF_POINT_LIMIT
        )
    } else {
        format!(
            "adaptive {}x{}x{} spatial partition; full-resolution leaves; {}x{}x{} voxel-center LOD samples; color averaged per voxel; other attributes from nearest source point",
            options.spatial_divisions(),
            options.spatial_divisions(),
            options.spatial_divisions(),
            options.sample_grid(),
            options.sample_grid(),
            options.sample_grid()
        )
    };
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
    if let Some(frame) = global_frame {
        metadata = metadata.with_global_frame(frame)?;
    }
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

struct NodeRecord {
    /// Measured leaf spacing of `v7.4-pre` in CRS units; 0 when not measured.
    spacing: f64,
    id: NodeId,
    parent: Option<NodeId>,
    level: u32,
    bounds: SpatialBounds,
    point_count: u64,
    geometric_error: f64,
    blocks: Vec<BlockId>,
}

pub struct ChildSpool {
    layout: SpoolLayout,
    pub id: SpoolId,
    pub point_count: u64,
    buffer: Vec<u8>,
    /// The store spool exists: an empty child never creates one.
    pub created: bool,
    pub payload_written_bytes: Option<u64>,
}

impl ChildSpool {
    pub fn new(id: SpoolId, layout: SpoolLayout, collect_diagnostics: bool) -> Self {
        Self {
            layout,
            id,
            point_count: 0,
            buffer: Vec::with_capacity(CHILD_SPOOL_BUFFER_BYTES),
            created: false,
            payload_written_bytes: collect_diagnostics.then_some(0),
        }
    }

    #[inline]
    pub fn write_point(
        &mut self,
        store: &impl SpoolStore,
        point: &SpoolPoint,
    ) -> Result<(), ConvertError> {
        if self.buffer.len() + self.layout.record_bytes() > CHILD_SPOOL_BUFFER_BYTES {
            self.flush(store)?;
        }
        write_point(&mut self.buffer, point, self.layout)?;
        self.point_count += 1;
        Ok(())
    }

    /// Appends an encoded record of the spool's layout unchanged.
    #[inline]
    pub fn write_record(
        &mut self,
        store: &impl SpoolStore,
        record: &[u8],
    ) -> Result<(), ConvertError> {
        if self.buffer.len() + record.len() > CHILD_SPOOL_BUFFER_BYTES {
            self.flush(store)?;
        }
        self.buffer.extend_from_slice(record);
        self.point_count += 1;
        Ok(())
    }

    #[cold]
    fn flush(&mut self, store: &impl SpoolStore) -> Result<(), ConvertError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        if !self.created {
            store.create(self.id)?;
            self.created = true;
        }
        store.append(self.id, &self.buffer)?;
        if let Some(bytes_written) = self.payload_written_bytes.as_mut() {
            *bytes_written = bytes_written
                .checked_add(self.buffer.len() as u64)
                .ok_or(ConvertError::SizeOverflow)?;
        }
        self.buffer.clear();
        Ok(())
    }

    pub fn finish(&mut self, store: &impl SpoolStore) -> Result<(), ConvertError> {
        self.flush(store)?;
        self.close_created(store)
    }

    /// Closes the spool without flushing the buffer (all-equal root: the children are discarded).
    pub fn close_created(&self, store: &impl SpoolStore) -> Result<(), ConvertError> {
        if self.created {
            store.close(self.id)?;
        }
        Ok(())
    }
}

/// The nodes, blocks and diagnostics one root child contributes, in branch-local ids.
pub struct BranchOutput {
    node_records: Vec<NodeRecord>,
    output_blocks: Vec<PlannedBlock>,
    next_block_id: u64,
    spool_diagnostics: Option<SpatialSpoolDiagnostics>,
    spatial_phases: Option<SpatialPhaseDiagnostics>,
    spool_written_local: u64,
    node_events: Vec<NodeSpoolEvent>,
}

impl BranchOutput {
    pub fn new(collect_diagnostics: bool) -> Self {
        Self {
            node_records: Vec::new(),
            output_blocks: Vec::new(),
            next_block_id: 0,
            spool_diagnostics: collect_diagnostics.then(SpatialSpoolDiagnostics::default),
            spatial_phases: collect_diagnostics.then(SpatialPhaseDiagnostics::default),
            spool_written_local: 0,
            node_events: Vec::new(),
        }
    }

    /// Nodes committed so far; the next node's branch-local id.
    pub fn node_count(&self) -> u64 {
        self.node_records.len() as u64
    }
}

pub struct SpatialBuilder<'a, S: PlanStore, C: Clock, X> {
    store: &'a S,
    clock: &'a C,
    executor: &'a X,
    /// Next unreserved spool id, shared by every branch.
    spool_ids: &'a AtomicU64,
    point_limit: u64,
    memory_budget: u64,
    spatial_divisions: u8,
    sample_grid: usize,
    frame: viewer_core::CoordinateFrame,
    layout: SpoolLayout,
    extras: std::sync::Arc<[LasExtraByte]>,
    node_records: Vec<NodeRecord>,
    output_blocks: Vec<PlannedBlock>,
    next_block_id: u64,
    on_progress: &'a mut dyn FnMut(ConversionProgress) -> bool,
    source_point_count: u64,
    parallel_workers: usize,
    /// Ranges the root partition may use; 0 scans the root as one stream.
    root_ranges: usize,
    /// Ranges the root partition used; 0 when it did not run by ranges.
    ranges_used: usize,
    workers_used: usize,
    spool_diagnostics: Option<SpatialSpoolDiagnostics>,
    spatial_phases: Option<SpatialPhaseDiagnostics>,
    /// Bytes this builder (not its siblings) has written, for per-root-child accounting.
    spool_written_local: u64,
    /// Node events with this builder's local node ids; `append_branch` rebases a worker's events.
    node_events: Vec<NodeSpoolEvent>,
}

/// A node event between `begin_node_event` and `finish_node_event`.
pub struct OpenNodeEvent<I> {
    pub event: NodeSpoolEvent,
    started: I,
}

impl<I> OpenNodeEvent<I> {
    pub fn new(
        node_id: NodeId,
        parent: Option<NodeId>,
        depth: u32,
        input_spool_bytes: u64,
        start_seq: u64,
        started: I,
    ) -> Self {
        Self {
            event: NodeSpoolEvent {
                node_id: node_id.get(),
                parent_id: parent.map(NodeId::get),
                depth,
                input_spool_bytes,
                start_seq,
                ..NodeSpoolEvent::default()
            },
            started,
        }
    }
}

impl<'a, S: PlanStore, C: Clock, X> SpatialBuilder<'a, S, C, X> {
    pub fn store(&self) -> &'a S {
        self.store
    }

    pub fn layout(&self) -> SpoolLayout {
        self.layout
    }

    pub fn collects_diagnostics(&self) -> bool {
        self.spool_diagnostics.is_some()
    }
}

impl<S: PlanStore, C: Clock, X: BatchPlanner<S, C>> SpatialBuilder<'_, S, C, X> {
    fn record_lod_write_elapsed(&mut self, started: Option<C::Instant>) {
        if let (Some(metrics), Some(started)) = (self.spatial_phases.as_mut(), started) {
            metrics.lod_sample_write_elapsed_ns = metrics
                .lod_sample_write_elapsed_ns
                .saturating_add(u128::from(self.clock.elapsed_ns(started)));
        }
    }

    /// The store credits its ledger on append; this keeps the per-builder count.
    fn note_spool_written(&mut self, bytes: u64) {
        if self.spool_diagnostics.is_some() {
            self.spool_written_local = self.spool_written_local.saturating_add(bytes);
        }
    }

    /// Opens a node event (diagnostics only); `finish_node_event` closes it.
    fn begin_node_event(
        &self,
        node_id: NodeId,
        parent: Option<NodeId>,
        depth: u32,
        input_spool_bytes: u64,
    ) -> Option<OpenNodeEvent<C::Instant>> {
        let start_seq = self.store.next_event_seq()?;
        Some(OpenNodeEvent::new(
            node_id,
            parent,
            depth,
            input_spool_bytes,
            start_seq,
            self.clock.now(),
        ))
    }

    pub fn finish_node_event(&mut self, open: Option<OpenNodeEvent<C::Instant>>) {
        let Some(mut open) = open else {
            return;
        };
        let Some(end_seq) = self.store.next_event_seq() else {
            return;
        };
        open.event.end_seq = end_seq;
        if open.event.elapsed_ns == 0 {
            open.event.elapsed_ns = self.clock.elapsed_ns(open.started);
        }
        self.node_events.push(open.event);
    }

    /// Reserves ids for one divisible node: child `k` → `base + k`, LOD → `base + divisions³`.
    fn reserve_node_spools(&self) -> u64 {
        let count = usize::from(self.spatial_divisions).pow(3) as u64 + 1;
        self.spool_ids.fetch_add(count, Ordering::Relaxed)
    }

    fn add_source_spool_bytes(&mut self, bytes: u64) -> Result<(), ConvertError> {
        self.note_spool_written(bytes);
        if let Some(diagnostics) = self.spool_diagnostics.as_mut() {
            diagnostics.source_payload_written_bytes = diagnostics
                .source_payload_written_bytes
                .checked_add(bytes)
                .ok_or(ConvertError::SizeOverflow)?;
        }
        Ok(())
    }

    fn add_scanned_spool_bytes(
        &mut self,
        bytes: u64,
        child_spool: bool,
    ) -> Result<(), ConvertError> {
        let Some(diagnostics) = self.spool_diagnostics.as_mut() else {
            return Ok(());
        };
        let counter = if child_spool {
            &mut diagnostics.child_payload_scanned_bytes
        } else {
            &mut diagnostics.source_payload_scanned_bytes
        };
        *counter = counter
            .checked_add(bytes)
            .ok_or(ConvertError::SizeOverflow)?;
        Ok(())
    }

    /// `child_spool_bytes`: payload flushed to each created child spool of one node.
    fn record_child_spool_writes(&mut self, child_spool_bytes: &[u64]) -> Result<(), ConvertError> {
        let Some(diagnostics) = self.spool_diagnostics.as_mut() else {
            return Ok(());
        };
        let bytes = child_spool_bytes
            .iter()
            .try_fold(0_u64, |total, bytes| total.checked_add(*bytes))
            .ok_or(ConvertError::SizeOverflow)?;
        diagnostics.child_payload_written_bytes = diagnostics
            .child_payload_written_bytes
            .checked_add(bytes)
            .ok_or(ConvertError::SizeOverflow)?;
        // The shared ledger was already credited at each flush; only the local count is added.
        self.spool_written_local = self.spool_written_local.saturating_add(bytes);
        Ok(())
    }

    fn add_lod_spool_bytes(&mut self, bytes: u64) -> Result<(), ConvertError> {
        let Some(diagnostics) = self.spool_diagnostics.as_mut() else {
            return Ok(());
        };
        diagnostics.lod_payload_written_bytes = diagnostics
            .lod_payload_written_bytes
            .checked_add(bytes)
            .ok_or(ConvertError::SizeOverflow)?;
        self.note_spool_written(bytes);
        Ok(())
    }

    pub fn job_context(&self) -> JobContext {
        JobContext {
            point_limit: self.point_limit,
            spatial_divisions: self.spatial_divisions,
            sample_grid: self.sample_grid,
            frame: self.frame,
            layout: self.layout,
            collect_diagnostics: self.spool_diagnostics.is_some(),
        }
    }

    /// Builds the job of one node, reserving its output ids only when it will split.
    pub fn node_job(
        &self,
        path: NodePath,
        level: u32,
        bounds: SpatialBounds,
        input: SpoolId,
        point_count: u64,
        input_is_child_spool: bool,
    ) -> NodeJob {
        let first_output = if self.job_context().splits(point_count, level, bounds) {
            SpoolId(self.reserve_node_spools())
        } else {
            SpoolId::SOURCE
        };
        NodeJob {
            path,
            level,
            bounds,
            input,
            point_count,
            input_is_child_spool,
            first_output,
        }
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "the progress value is handed over by the job callback"
    )]
    pub fn emit_job_progress(&mut self, progress: JobProgress) -> Result<(), ConvertError> {
        match progress {
            JobProgress::NodeHeartbeat => self.emit_plan_progress(),
            JobProgress::SourcePoints(processed) => {
                emit_source_progress(self.on_progress, self.source_point_count, processed)
            }
        }
    }

    /// Admitted root ranges: the requested count, reduced so that the extra ranges fit in the
    /// budget left after the root's own working set (as `memory_limited_workers` does).
    fn admitted_root_ranges(&self) -> Result<usize, ConvertError> {
        let base = estimate_memory(
            self.point_limit,
            0,
            0,
            self.spatial_divisions,
            self.sample_grid,
        )?;
        let per_range =
            range_memory_bytes(self.point_limit, self.spatial_divisions, self.sample_grid)?;
        let extra = self.memory_budget.saturating_sub(base) / per_range;
        Ok(self
            .root_ranges
            .min(usize::try_from(extra).map_or(usize::MAX, |extra| extra.saturating_add(1))))
    }

    /// The root partition by ranges: split, run every range (the runner opens its source), then
    /// merge and join. A range whose source cannot be opened fails like any other error in that
    /// range, so the lowest-starting failure wins, as in a serial scan.
    fn partition_root_ranges<F: SourceFactory>(
        &mut self,
        factory: &F,
        source: &SourceInfo,
        bounds: SpatialBounds,
        first_output: SpoolId,
    ) -> Result<RootPartition, ConvertError>
    where
        X: RangeRunner<S, C, F>,
    {
        let ctx = self.job_context();
        let children = usize::from(self.spatial_divisions).pow(3) as u64;
        let ranges = factory.split(self.admitted_root_ranges()?, self.point_limit)?;
        // Segment (r, k) → `base + r · divisions³ + k`.
        let base = self.spool_ids.fetch_add(
            (ranges.len() as u64)
                .checked_mul(children)
                .ok_or(ConvertError::SizeOverflow)?,
            Ordering::Relaxed,
        );
        self.ranges_used = ranges.len();
        let job = RangeJob {
            ctx: &ctx,
            info: source,
            bounds,
            batch: self.point_limit,
        };
        let (store, clock, executor) = (self.store, self.clock, self.executor);
        let ranges = ranges
            .into_iter()
            .enumerate()
            .map(|(index, range)| (range, SpoolId(base + index as u64 * children)))
            .collect::<Vec<_>>();
        let mut done = vec![0_u64; ranges.len()];
        let results =
            executor.run_ranges(&job, factory, ranges, store, clock, &mut |range, update| {
                match update {
                    JobProgress::SourcePoints(points) => {
                        done[range] = points;
                        self.emit_job_progress(JobProgress::SourcePoints(done.iter().sum()))
                    }
                    JobProgress::NodeHeartbeat => {
                        self.emit_job_progress(JobProgress::NodeHeartbeat)
                    }
                }
            });
        merge_root_ranges(&ctx, bounds, first_output, results, store)
    }

    fn build_root_from_source<F: SourceFactory>(
        &mut self,
        first: &mut F::Source,
        factory: &F,
        source: &SourceInfo,
        bounds: SpatialBounds,
    ) -> Result<(), ConvertError>
    where
        X: RangeRunner<S, C, F>,
    {
        let ctx = self.job_context();
        let root_path = NodePath(Vec::new());
        let store = self.store;
        if !ctx.splits(source.point_count, 0, bounds) {
            let bytes = spool_source(
                &mut *first,
                source,
                self.point_limit,
                store,
                SpoolId::SOURCE,
                &mut |progress| self.emit_job_progress(progress),
            )?;
            self.add_source_spool_bytes(bytes)?;
            let job = self.node_job(
                root_path,
                0,
                bounds,
                SpoolId::SOURCE,
                source.point_count,
                false,
            );
            self.build_node(job, None)?;
            return Ok(());
        }

        let root_id = NodeId::new(0);
        let mut open = self.begin_node_event(root_id, None, 0, 0);
        let first_output = SpoolId(self.reserve_node_spools());
        let partition = if self.root_ranges == 0 {
            run_root_partition(
                &ctx,
                source,
                bounds,
                first_output,
                &mut *first,
                store,
                self.clock,
                &mut |progress| self.emit_job_progress(progress),
            )?
        } else {
            self.partition_root_ranges(factory, source, bounds, first_output)?
        };
        self.record_job_metrics(partition.metrics.as_ref(), false, &mut open)?;

        if partition.all_points_equal {
            if let Some(open) = open.as_mut() {
                open.event.input_spool_bytes = source
                    .point_count
                    .saturating_mul(self.layout.record_bytes() as u64);
            }
            for spool in partition.release {
                store.delete(spool)?;
            }
            let mut reopened = factory.open(PointRange {
                start: 0,
                end: source.point_count,
            })?;
            let bytes = spool_source(
                &mut reopened,
                source,
                self.point_limit,
                store,
                SpoolId::SOURCE,
                &mut |progress| self.emit_job_progress(progress),
            )?;
            self.add_source_spool_bytes(bytes)?;
            let job = self.node_job(
                root_path,
                0,
                bounds,
                SpoolId::SOURCE,
                source.point_count,
                false,
            );
            self.build_node_with_event(job, None, open)?;
            return Ok(());
        }

        let lod_write_started = self.spatial_phases.as_ref().map(|_| self.clock.now());
        let sample_spool =
            SpoolId(first_output.0 + usize::from(self.spatial_divisions).pow(3) as u64);
        let lod = write_lod(store, sample_spool, &partition.samples, bounds, self.layout)?;
        let sample_count = lod.count;
        self.record_lod_write_elapsed(lod_write_started);
        let lod_bytes = (sample_count * self.layout.lod().record_bytes()) as u64;
        self.add_lod_spool_bytes(lod_bytes)?;
        if let Some(open) = open.as_mut() {
            open.event.lod_bytes = lod_bytes;
        }
        self.commit_node(
            NodeRecord {
                spacing: 0.0,
                id: root_id,
                parent: None,
                level: 0,
                bounds,
                point_count: sample_count as u64,
                geometric_error: lod_sample_error(bounds, self.sample_grid, self.layout)
                    + center_rounding_error(bounds, self.frame),
                blocks: Vec::new(),
            },
            lod_blocks(sample_spool, &lod),
        )?;

        if let Some(open) = open.as_mut() {
            open.event.elapsed_ns = self.clock.elapsed_ns(open.started);
        }
        self.hint_children(0, &partition.children);
        self.process_child_tasks(&root_path, root_id, 0, partition.children)?;
        self.finish_node_event(open);
        Ok(())
    }

    pub fn build_node(
        &mut self,
        job: NodeJob,
        parent: Option<NodeId>,
    ) -> Result<NodeId, ConvertError> {
        let id = NodeId::new(self.node_records.len() as u64);
        let input_bytes = job
            .point_count
            .saturating_mul(self.layout.record_bytes() as u64);
        let open = self.begin_node_event(id, parent, job.level, input_bytes);
        self.build_node_with_event(job, parent, open)
    }

    /// `open` may already hold the child spools of a failed root scan; they are kept in the event.
    fn build_node_with_event(
        &mut self,
        job: NodeJob,
        parent: Option<NodeId>,
        mut open: Option<OpenNodeEvent<C::Instant>>,
    ) -> Result<NodeId, ConvertError> {
        let id = NodeId::new(self.node_records.len() as u64);
        if let Some(open) = open.as_mut() {
            debug_assert_eq!(open.event.node_id, id.get());
        }
        let result = self.run_and_commit(job, parent, &mut open);
        self.finish_node_event(open);
        result
    }

    /// Runs one node job, releases its spools, commits the node and then its children in key
    /// order, so node and block ids follow the preorder.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the job is consumed by its commit"
    )]
    fn run_and_commit(
        &mut self,
        job: NodeJob,
        parent: Option<NodeId>,
        open: &mut Option<OpenNodeEvent<C::Instant>>,
    ) -> Result<NodeId, ConvertError> {
        let id = NodeId::new(self.node_records.len() as u64);
        let ctx = self.job_context();
        let store = self.store;
        let output = run_node_job(&ctx, &job, store, self.clock, &mut |progress| {
            self.emit_job_progress(progress)
        })?;
        for spool in &output.release {
            store.delete(*spool)?;
        }
        let children = self.commit_job(&job, output, parent, open)?;
        debug_assert_eq!(id, NodeId::new(self.node_records.len() as u64 - 1));
        if !children.is_empty() {
            self.process_child_tasks(&job.path, id, job.level, children)?;
        }
        Ok(id)
    }

    /// Commits a finished job whose `release` was already deleted: metrics, event flags, block
    /// ids and the node record. Returns its children (empty for a leaf), not yet planned.
    pub fn commit_job(
        &mut self,
        job: &NodeJob,
        mut output: NodeJobOutput,
        parent: Option<NodeId>,
        open: &mut Option<OpenNodeEvent<C::Instant>>,
    ) -> Result<Vec<ChildOutput>, ConvertError> {
        debug_assert_eq!(output.path, job.path);
        let id = NodeId::new(self.node_records.len() as u64);
        self.record_job_metrics(output.metrics.as_ref(), job.input_is_child_spool, open)?;
        let is_leaf = output.is_leaf();
        if let Some(open) = open.as_mut() {
            open.event.is_leaf = is_leaf;
            open.event.became_leaf_all_equal = output
                .metrics
                .as_ref()
                .is_some_and(|metrics| metrics.became_leaf_all_equal);
        }
        let children = std::mem::take(&mut output.children);
        self.hint_children(job.level, &children);
        self.commit_node(
            NodeRecord {
                spacing: output.spacing,
                id,
                parent,
                level: job.level,
                bounds: job.bounds,
                point_count: output.point_count,
                geometric_error: output.geometric_error,
                blocks: Vec::new(),
            },
            output.blocks,
        )?;
        if !is_leaf && let Some(open) = open.as_mut() {
            open.event.elapsed_ns = self.clock.elapsed_ns(open.started);
        }
        Ok(children)
    }

    /// Adds a job's diagnostics to the builder and to the node event.
    fn record_job_metrics(
        &mut self,
        metrics: Option<&JobMetrics>,
        input_is_child_spool: bool,
        open: &mut Option<OpenNodeEvent<C::Instant>>,
    ) -> Result<(), ConvertError> {
        let Some(metrics) = metrics else {
            return Ok(());
        };
        if let Some(phases) = self.spatial_phases.as_mut() {
            phases.root_las_fill_elapsed_ns = phases
                .root_las_fill_elapsed_ns
                .saturating_add(u128::from(metrics.las_fill_ns));
            phases.spool_scan_read_elapsed_ns = phases
                .spool_scan_read_elapsed_ns
                .saturating_add(u128::from(metrics.scan_read_ns));
            phases.spool_partition_and_sample_elapsed_ns = phases
                .spool_partition_and_sample_elapsed_ns
                .saturating_add(u128::from(metrics.partition_and_sample_ns));
            phases.lod_sample_write_elapsed_ns = phases
                .lod_sample_write_elapsed_ns
                .saturating_add(u128::from(metrics.lod_write_ns));
        }
        self.add_scanned_spool_bytes(metrics.scanned_bytes, input_is_child_spool)?;
        self.record_child_spool_writes(&metrics.child_spool_bytes)?;
        self.add_lod_spool_bytes(metrics.lod_bytes)?;
        if let Some(open) = open.as_mut() {
            open.event
                .child_spool_bytes
                .extend_from_slice(&metrics.child_spool_bytes);
            open.event.lod_bytes = metrics.lod_bytes;
        }
        Ok(())
    }

    /// Allocates the block ids of a node in plan order, then records the node (same checks and
    /// order as the recursive planner had).
    fn commit_node(
        &mut self,
        mut record: NodeRecord,
        plans: Vec<BlockPlan>,
    ) -> Result<(), ConvertError> {
        for plan in plans {
            if plan.final_block_in_spool {
                self.store.hint_retained(plan.spool);
            }
            let block_id = self.allocate_block_id()?;
            record.blocks.push(block_id);
            self.output_blocks.push(PlannedBlock {
                id: block_id,
                spool: plan.spool,
                offset_points: plan.offset_points,
                point_count: plan.point_count,
                layout: if plan.is_source_points {
                    self.layout
                } else {
                    self.layout.lod()
                },
                extras: self.extras.clone(),
                is_source_points: plan.is_source_points,
                final_block_in_spool: plan.final_block_in_spool,
            });
        }
        self.ensure_memory(self.node_records.len() + 1, self.output_blocks.len())?;
        self.node_records.push(record);
        self.emit_plan_progress()
    }

    /// Tells the store the consume order of the child spools of a node at `level`. Emitted where
    /// children are born (`commit_job` for both executors, the root scan) rather than in
    /// `process_child_tasks`, which the threaded ready queue bypasses below the root.
    fn hint_children(&self, level: u32, children: &[ChildOutput]) {
        for child in children {
            self.store
                .hint_consume_rank(child.spool, consume_rank(level + 1, child.key));
        }
    }

    fn process_child_tasks(
        &mut self,
        parent_path: &NodePath,
        parent_id: NodeId,
        level: u32,
        children: Vec<ChildOutput>,
    ) -> Result<(), ConvertError> {
        if level == 0 && self.parallel_workers > 1 && children.len() > 1 {
            self.build_root_children_parallel(parent_id, children)?;
        } else {
            for child in children {
                let written_before = self.spool_written_local;
                let job = self.node_job(
                    parent_path.child(child.key),
                    level + 1,
                    child.bounds,
                    child.spool,
                    child.point_count,
                    true,
                );
                self.build_node(job, Some(parent_id))?;
                if level == 0
                    && let Some(phases) = self.spatial_phases.as_mut()
                {
                    phases
                        .root_child_spool_bytes_written
                        .push(self.spool_written_local - written_before);
                }
            }
        }
        Ok(())
    }

    fn build_root_children_parallel(
        &mut self,
        parent_id: NodeId,
        tasks: Vec<ChildOutput>,
    ) -> Result<(), ConvertError> {
        let mut tasks = VecDeque::from(tasks);
        while !tasks.is_empty() {
            let batch_count = self.parallel_workers.min(tasks.len());
            let batch = (0..batch_count)
                .map(|_| {
                    tasks
                        .pop_front()
                        .expect("batch size is bounded by queued tasks")
                })
                .collect::<Vec<_>>();
            let worker_memory_budget = branch_memory_budget(
                self.memory_budget,
                self.point_limit,
                self.node_records.len(),
                self.output_blocks.len(),
                batch_count,
                self.spatial_divisions,
                self.sample_grid,
            )?;
            self.workers_used = self.workers_used.max(batch_count);
            let executor = self.executor;
            let outputs = executor.plan_batch(self, batch, worker_memory_budget)?;
            for (_, mut branch) in outputs {
                self.append_branch(parent_id, &mut branch)?;
            }
        }
        Ok(())
    }

    /// Runs `f` on a branch-local builder that holds `branch` and
    /// reports to this builder's progress callback, then stores the builder back into `branch`.
    pub fn with_branch<R>(
        &mut self,
        branch: &mut BranchOutput,
        memory_budget: u64,
        f: impl FnOnce(&mut SpatialBuilder<'_, S, C, X>) -> R,
    ) -> R {
        let mut builder = SpatialBuilder {
            store: self.store,
            clock: self.clock,
            executor: self.executor,
            spool_ids: self.spool_ids,
            point_limit: self.point_limit,
            memory_budget,
            spatial_divisions: self.spatial_divisions,
            sample_grid: self.sample_grid,
            frame: self.frame,
            layout: self.layout,
            extras: self.extras.clone(),
            node_records: std::mem::take(&mut branch.node_records),
            output_blocks: std::mem::take(&mut branch.output_blocks),
            next_block_id: branch.next_block_id,
            on_progress: &mut *self.on_progress,
            source_point_count: self.source_point_count,
            parallel_workers: 1,
            root_ranges: 0,
            ranges_used: 0,
            workers_used: 1,
            spool_diagnostics: branch.spool_diagnostics.take(),
            spatial_phases: branch.spatial_phases.take(),
            spool_written_local: branch.spool_written_local,
            node_events: std::mem::take(&mut branch.node_events),
        };
        let result = f(&mut builder);
        *branch = BranchOutput {
            node_records: builder.node_records,
            output_blocks: builder.output_blocks,
            next_block_id: builder.next_block_id,
            spool_diagnostics: builder.spool_diagnostics,
            spatial_phases: builder.spatial_phases,
            spool_written_local: builder.spool_written_local,
            node_events: builder.node_events,
        };
        result
    }

    fn append_branch(
        &mut self,
        parent_id: NodeId,
        branch: &mut BranchOutput,
    ) -> Result<(), ConvertError> {
        let node_offset =
            u64::try_from(self.node_records.len()).map_err(|_| ConvertError::SizeOverflow)?;
        let block_offset = self.next_block_id;
        let next_block_id = block_offset
            .checked_add(branch.next_block_id)
            .ok_or(ConvertError::SizeOverflow)?;
        if next_block_id > crate::options::MAX_BLOCKS || next_block_id >= u64::from(u32::MAX) {
            return Err(ConvertError::TooManyBlocks(next_block_id));
        }
        for node in &mut branch.node_records {
            node.id = NodeId::new(
                node.id
                    .get()
                    .checked_add(node_offset)
                    .ok_or(ConvertError::SizeOverflow)?,
            );
            node.parent = Some(match node.parent {
                Some(parent) => NodeId::new(
                    parent
                        .get()
                        .checked_add(node_offset)
                        .ok_or(ConvertError::SizeOverflow)?,
                ),
                None => parent_id,
            });
            for block in &mut node.blocks {
                *block = BlockId::new(
                    block
                        .get()
                        .checked_add(block_offset)
                        .ok_or(ConvertError::SizeOverflow)?,
                );
            }
        }
        for block in &mut branch.output_blocks {
            block.id = BlockId::new(
                block
                    .id
                    .get()
                    .checked_add(block_offset)
                    .ok_or(ConvertError::SizeOverflow)?,
            );
        }
        if let Some(phases) = self.spatial_phases.as_mut() {
            phases
                .root_child_spool_bytes_written
                .push(branch.spool_written_local);
        }
        for event in &mut branch.node_events {
            event.node_id += node_offset;
            event.parent_id = Some(
                event
                    .parent_id
                    .map_or(parent_id.get(), |id| id + node_offset),
            );
        }
        self.node_events.append(&mut branch.node_events);
        self.node_records.append(&mut branch.node_records);
        self.output_blocks.append(&mut branch.output_blocks);
        if let (Some(target), Some(source)) = (
            self.spool_diagnostics.as_mut(),
            branch.spool_diagnostics.as_ref(),
        ) {
            target.source_payload_written_bytes = target
                .source_payload_written_bytes
                .checked_add(source.source_payload_written_bytes)
                .ok_or(ConvertError::SizeOverflow)?;
            target.source_payload_scanned_bytes = target
                .source_payload_scanned_bytes
                .checked_add(source.source_payload_scanned_bytes)
                .ok_or(ConvertError::SizeOverflow)?;
            target.child_payload_written_bytes = target
                .child_payload_written_bytes
                .checked_add(source.child_payload_written_bytes)
                .ok_or(ConvertError::SizeOverflow)?;
            target.child_payload_scanned_bytes = target
                .child_payload_scanned_bytes
                .checked_add(source.child_payload_scanned_bytes)
                .ok_or(ConvertError::SizeOverflow)?;
            target.lod_payload_written_bytes = target
                .lod_payload_written_bytes
                .checked_add(source.lod_payload_written_bytes)
                .ok_or(ConvertError::SizeOverflow)?;
        }
        if let (Some(target), Some(source)) =
            (self.spatial_phases.as_mut(), branch.spatial_phases.as_ref())
        {
            target.root_las_fill_elapsed_ns = target
                .root_las_fill_elapsed_ns
                .saturating_add(source.root_las_fill_elapsed_ns);
            target.spool_scan_read_elapsed_ns = target
                .spool_scan_read_elapsed_ns
                .saturating_add(source.spool_scan_read_elapsed_ns);
            target.spool_partition_and_sample_elapsed_ns = target
                .spool_partition_and_sample_elapsed_ns
                .saturating_add(source.spool_partition_and_sample_elapsed_ns);
            target.lod_sample_write_elapsed_ns = target
                .lod_sample_write_elapsed_ns
                .saturating_add(source.lod_sample_write_elapsed_ns);
        }
        self.next_block_id = next_block_id;
        Ok(())
    }

    fn allocate_block_id(&mut self) -> Result<BlockId, ConvertError> {
        if self.next_block_id >= crate::options::MAX_BLOCKS
            || self.next_block_id >= u64::from(u32::MAX)
        {
            return Err(ConvertError::TooManyBlocks(self.next_block_id + 1));
        }
        self.ensure_memory(self.node_records.len(), self.output_blocks.len() + 1)?;
        let id = BlockId::new(self.next_block_id);
        self.next_block_id += 1;
        Ok(id)
    }

    fn ensure_memory(&self, node_count: usize, block_count: usize) -> Result<(), ConvertError> {
        check_memory_budget(
            estimate_memory(
                self.point_limit,
                node_count,
                block_count,
                self.spatial_divisions,
                self.sample_grid,
            )?,
            self.memory_budget,
        )
    }

    fn emit_plan_progress(&mut self) -> Result<(), ConvertError> {
        emit_progress(
            self.on_progress,
            ConversionProgress {
                stage: ConversionStage::BuildingSpatialHierarchy,
                points_processed: 0,
                total_points: self.source_point_count,
                bytes_written: 0,
            },
            0,
        )
    }
}

/// Fixed-size source records of the nearest points of a sample grid, indexed by slot: a cell
/// keeps a `u32` slot instead of a decoded `SpoolPoint` (about 180 bytes with its Extra Bytes
/// array), which keeps accumulators small and avoids decoding and copying a full point per
/// candidate.
pub struct NearestRecords {
    record_bytes: usize,
    bytes: Vec<u8>,
}

impl NearestRecords {
    pub fn new(layout: SpoolLayout, reserve_records: usize) -> Self {
        let record_bytes = layout.record_bytes();
        Self {
            record_bytes,
            bytes: Vec::with_capacity(reserve_records * record_bytes),
        }
    }

    fn push(&mut self, record: &[u8]) -> u32 {
        let slot = (self.bytes.len() / self.record_bytes) as u32;
        self.bytes.extend_from_slice(record);
        slot
    }

    fn set(&mut self, slot: u32, record: &[u8]) {
        let start = slot as usize * self.record_bytes;
        self.bytes[start..start + self.record_bytes].copy_from_slice(record);
    }

    fn get(&self, slot: u32) -> &[u8] {
        let start = slot as usize * self.record_bytes;
        &self.bytes[start..start + self.record_bytes]
    }
}

pub struct SampleAccumulator {
    count: u64,
    // `u64` sums hold `count * 65535` while `count < 2^48`, far above any point cloud; `u128`
    // would double the size of every occupied cell of a large sample grid.
    display_color: [u64; 3],
    rgb: [u64; 3],
    nearest_distance_squared: f64,
    /// Slot of the nearest record in the grid's [`NearestRecords`]; `NO_SLOT` before the first.
    slot: u32,
}

impl Default for SampleAccumulator {
    fn default() -> Self {
        Self {
            count: 0,
            display_color: [0; 3],
            rgb: [0; 3],
            nearest_distance_squared: 0.0,
            slot: Self::NO_SLOT,
        }
    }
}

impl SampleAccumulator {
    const NO_SLOT: u32 = u32::MAX;

    /// Adds the source record `record` (of `layout`) at `position`, which must be the position
    /// `decode_position` returns for it.
    #[inline]
    pub fn add(
        &mut self,
        records: &mut NearestRecords,
        layout: SpoolLayout,
        record: &[u8],
        position: [f64; 3],
        center: [f64; 3],
    ) {
        self.count += 1;
        let tail = record.len() - 3;
        for axis in 0..3 {
            self.display_color[axis] += u64::from(record[tail + axis]);
        }
        if layout.has_color {
            let offset = layout.rgb_offset();
            for axis in 0..3 {
                self.rgb[axis] += u64::from(u16::from_le_bytes([
                    record[offset + 2 * axis],
                    record[offset + 2 * axis + 1],
                ]));
            }
        }
        let delta_x = position[0] - center[0];
        let delta_y = position[1] - center[1];
        let delta_z = position[2] - center[2];
        let distance_squared = delta_x * delta_x + delta_y * delta_y + delta_z * delta_z;
        if self.slot == Self::NO_SLOT {
            self.slot = records.push(record);
            self.nearest_distance_squared = distance_squared;
        } else if distance_squared < self.nearest_distance_squared {
            self.nearest_distance_squared = distance_squared;
            records.set(self.slot, record);
        }
    }

    /// Folds in the accumulator of the points that follow this one's in the source order. The
    /// later nearest point wins only when strictly closer, as in `add`, so ties keep the earlier.
    pub fn merge(
        &mut self,
        records: &mut NearestRecords,
        later: &Self,
        later_records: &NearestRecords,
    ) {
        self.count += later.count;
        for axis in 0..3 {
            self.display_color[axis] += later.display_color[axis];
            self.rgb[axis] += later.rgb[axis];
        }
        if later.slot == Self::NO_SLOT {
            return;
        }
        let record = later_records.get(later.slot);
        if self.slot == Self::NO_SLOT {
            self.slot = records.push(record);
            self.nearest_distance_squared = later.nearest_distance_squared;
        } else if later.nearest_distance_squared < self.nearest_distance_squared {
            self.nearest_distance_squared = later.nearest_distance_squared;
            records.set(self.slot, record);
        }
    }

    fn nearest(&self, records: &NearestRecords, layout: SpoolLayout) -> SpoolPoint {
        assert_ne!(self.slot, Self::NO_SLOT, "sample has a source point");
        decode_point(records.get(self.slot), layout)
    }

    /// Sample placed at its nearest source point instead of the cell center: used where the cell
    /// is so large (`a < 32`) that a center far from the data could not be held in `f32`.
    pub fn finish_at_nearest(&self, records: &NearestRecords, layout: SpoolLayout) -> SpoolPoint {
        let nearest = self.nearest(records, layout);
        self.finish(records, layout, nearest.position)
    }

    pub fn finish(
        &self,
        records: &NearestRecords,
        layout: SpoolLayout,
        center: [f64; 3],
    ) -> SpoolPoint {
        let mut point = self.nearest(records, layout);
        point.position = center;
        point.display_color = ColorRgb::new(
            average(self.display_color[0], self.count) as u8,
            average(self.display_color[1], self.count) as u8,
            average(self.display_color[2], self.count) as u8,
        );
        point.rgb = std::array::from_fn(|axis| average(self.rgb[axis], self.count) as u16);
        point
    }
}

#[inline]
fn average(sum: u64, count: u64) -> u64 {
    (sum + count / 2) / count
}

/// Forwards `JobProgress::SourcePoints` of a job reading the source to the host callback.
pub fn emit_source_progress(
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    total_points: u64,
    processed: u64,
) -> Result<(), ConvertError> {
    emit_progress(
        on_progress,
        ConversionProgress {
            stage: ConversionStage::PlanningSpatialIndex,
            points_processed: processed,
            total_points,
            bytes_written: 0,
        },
        processed,
    )
}

/// Appends whatever `write_source_points` hands over; each call is already a ≤ 32 KiB chunk.
pub struct StoreWriter<'a, S> {
    pub store: &'a S,
    pub id: SpoolId,
}

impl<S: SpoolStore> Write for StoreWriter<'_, S> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.store
            .append(self.id, bytes)
            .map_err(|error| match error {
                ConvertError::Io(error) => error,
                other => io::Error::other(other),
            })?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn write_source_points(
    data: &PointData,
    source: &SourceInfo,
    processed: u64,
    writer: &mut impl Write,
) -> Result<(), ConvertError> {
    let layout = SpoolLayout::source(source);
    let mut encoded = Vec::with_capacity(CHILD_SPOOL_BUFFER_BYTES);
    visit_source_points(data, source, processed, |point| {
        if encoded.len() + layout.record_bytes() > CHILD_SPOOL_BUFFER_BYTES {
            writer.write_all(&encoded)?;
            encoded.clear();
        }
        write_point(&mut encoded, &point, layout).map_err(ConvertError::Io)
    })?;
    if !encoded.is_empty() {
        writer.write_all(&encoded)?;
    }
    Ok(())
}

pub fn visit_source_points(
    data: &PointData,
    source: &SourceInfo,
    processed: u64,
    mut visitor: impl FnMut(SpoolPoint) -> Result<(), ConvertError>,
) -> Result<(), ConvertError> {
    let mut intensities = data.intensity();
    let mut classifications = data.classification();
    let mut return_numbers = data.return_number();
    let mut returns_per_pulse = data.number_of_returns();
    let mut scan_angles = data.scan_angle_degrees();
    let mut user_data = data.user_data();
    let mut point_source_ids = data.point_source_id();
    let mut gps_times = data.gps_time();
    let mut nir_values = data.nir();
    let mut rgb_values = data.rgb();
    if source.point_format.has_gps_time != gps_times.is_some()
        || source.point_format.has_nir != nir_values.is_some()
        || source.point_format.has_color != rgb_values.is_some()
    {
        return Err(ConvertError::InvalidAttributeSchema);
    }
    let layout = SpoolLayout::source(source);
    let records = data.raw_bytes().chunks_exact(data.record_len());
    for (index, (((x, y), z), record)) in data
        .x_raw()
        .zip(data.y_raw())
        .zip(data.z_raw())
        .zip(records)
        .enumerate()
    {
        let raw_position = [x, y, z];
        let exact = layout.exact_position(raw_position);
        let position = layout.position(raw_position);
        let local = std::array::from_fn(|axis| position[axis] - source.frame.origin()[axis]);
        if position.iter().any(|coordinate| !coordinate.is_finite())
            || local.iter().any(|coordinate| !coordinate.is_finite())
        {
            return Err(ConvertError::CoordinateFrame(
                viewer_core::CoordinateFrameError::PositionOutOfRange(local),
            ));
        }
        if outside_header_bounds(exact, source.bounds) {
            return Err(ConvertError::PointOutsideHeaderBounds {
                point_index: processed + index as u64,
                position: exact,
            });
        }
        let intensity = intensities
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let classification = classifications
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let return_number = return_numbers
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let returns_per_pulse = returns_per_pulse
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let scan_angle = scan_angles
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let user_data = user_data
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let point_source_id = point_source_ids
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let gps_time = gps_times
            .as_mut()
            .map(|values| values.next().ok_or(ConvertError::InvalidAttributeSchema))
            .transpose()?
            .unwrap_or(0.0);
        let nir = nir_values
            .as_mut()
            .map(|values| values.next().ok_or(ConvertError::InvalidAttributeSchema))
            .transpose()?
            .unwrap_or(0);
        let rgb = rgb_values
            .as_mut()
            .map(|values| values.next().ok_or(ConvertError::InvalidAttributeSchema))
            .transpose()?
            .map_or_default(|(red, green, blue)| [red, green, blue]);
        let display_color = if source.point_format.has_color {
            ColorRgb::new(
                (rgb[0] >> 8) as u8,
                (rgb[1] >> 8) as u8,
                (rgb[2] >> 8) as u8,
            )
        } else {
            let gray = (intensity >> 8) as u8;
            ColorRgb::new(gray, gray, gray)
        };
        let mut extra = [0; MAX_EXTRA_BYTES_PER_POINT];
        let extra_len = usize::from(layout.extra_len);
        let las_flags = if layout.has_flags {
            extra[..extra_len].copy_from_slice(&record[record.len() - extra_len..]);
            las_flags_of_record(record, layout.is_extended)
        } else {
            0
        };
        visitor(SpoolPoint {
            raw_position,
            position,
            intensity,
            classification,
            return_number,
            returns_per_pulse,
            scan_angle,
            user_data,
            point_source_id,
            gps_time,
            rgb,
            nir,
            las_flags,
            extra,
            display_color,
        })?;
    }
    if intensities.next().is_some()
        || classifications.next().is_some()
        || return_numbers.next().is_some()
        || returns_per_pulse.next().is_some()
        || scan_angles.next().is_some()
        || user_data.next().is_some()
        || point_source_ids.next().is_some()
        || gps_times
            .as_mut()
            .is_some_and(|values| values.next().is_some())
        || nir_values
            .as_mut()
            .is_some_and(|values| values.next().is_some())
        || rgb_values
            .as_mut()
            .is_some_and(|values| values.next().is_some())
    {
        return Err(ConvertError::InvalidAttributeSchema);
    }
    Ok(())
}

#[inline]
fn outside_header_bounds(position: [f64; 3], bounds: SpatialBounds) -> bool {
    (0..3).any(|axis| {
        let min = bounds.min()[axis];
        let max = bounds.max()[axis];
        let coordinate_magnitude = position[axis].abs().max(min.abs()).max(max.abs());
        let tolerance = (coordinate_magnitude * HEADER_BOUNDS_RELATIVE_TOLERANCE)
            .min(HEADER_BOUNDS_MAX_TOLERANCE);
        position[axis] < min - tolerance || position[axis] > max + tolerance
    })
}

#[inline]
pub fn write_point(
    writer: &mut impl Write,
    point: &SpoolPoint,
    layout: SpoolLayout,
) -> io::Result<()> {
    let mut record = [0_u8; MAX_SPOOL_RECORD_BYTES];
    let mut offset = 0;
    macro_rules! append_bytes {
        ($bytes:expr) => {{
            let bytes = $bytes;
            let end = offset + bytes.len();
            record[offset..end].copy_from_slice(&bytes);
            offset = end;
        }};
    }
    if layout.source_coordinates {
        for coordinate in point.raw_position {
            append_bytes!(coordinate.to_le_bytes());
        }
    } else {
        for coordinate in point.position {
            append_bytes!(coordinate.to_le_bytes());
        }
    }
    append_bytes!(point.intensity.to_le_bytes());
    append_bytes!([
        point.classification,
        point.return_number,
        point.returns_per_pulse,
    ]);
    append_bytes!(point.scan_angle.to_le_bytes());
    append_bytes!([point.user_data]);
    append_bytes!(point.point_source_id.to_le_bytes());
    if layout.has_gps_time {
        append_bytes!(point.gps_time.to_le_bytes());
    }
    if layout.has_color {
        for channel in point.rgb {
            append_bytes!(channel.to_le_bytes());
        }
    }
    if layout.has_nir {
        append_bytes!(point.nir.to_le_bytes());
    }
    if layout.has_flags {
        append_bytes!([point.las_flags]);
    }
    append_bytes!(&point.extra[..usize::from(layout.extra_len)]);
    append_bytes!([
        point.display_color.r,
        point.display_color.g,
        point.display_color.b,
    ]);
    writer.write_all(&record[..offset])
}

#[inline]
pub fn read_records(
    reader: &mut impl Read,
    buffer: &mut [u8],
    layout: SpoolLayout,
) -> io::Result<usize> {
    let mut bytes_read = 0;
    while bytes_read < buffer.len() {
        match reader.read(&mut buffer[bytes_read..]) {
            Ok(0) => break,
            Ok(count) => bytes_read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let record_bytes = layout.record_bytes();
    if !bytes_read.is_multiple_of(record_bytes) {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    Ok(bytes_read / record_bytes)
}

#[cfg(test)]
fn read_point_if_present(
    reader: &mut impl Read,
    layout: SpoolLayout,
) -> io::Result<Option<SpoolPoint>> {
    let mut buffer = [0_u8; MAX_SPOOL_RECORD_BYTES];
    let bytes = &mut buffer[..layout.record_bytes()];
    let read = reader.read(&mut bytes[..1])?;
    if read == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut bytes[1..])?;
    Ok(Some(decode_point(bytes, layout)))
}

#[cfg(test)]
fn read_point(reader: &mut impl Read, layout: SpoolLayout) -> Result<SpoolPoint, ConvertError> {
    read_point_if_present(reader, layout)?
        .ok_or_else(|| ConvertError::Io(io::Error::from(io::ErrorKind::UnexpectedEof)))
}

// Hot loop: `inline(always)` saves ~0.55% Ir over `inline` (M1_R7 record, diagnosis).
#[expect(
    clippy::inline_always,
    reason = "hot loop: measured gain over `inline` (see the comment above)"
)]
#[inline(always)]
pub fn decode_point(bytes: &[u8], layout: SpoolLayout) -> SpoolPoint {
    let mut offset = 0;
    let mut raw_position = [0; 3];
    let position = if layout.source_coordinates {
        for coordinate in &mut raw_position {
            *coordinate =
                i32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("fixed width"));
            offset += 4;
        }
        layout.position(raw_position)
    } else {
        std::array::from_fn(|_| read_f64(bytes, &mut offset))
    };
    let intensity = read_u16(bytes, &mut offset);
    let classification = bytes[offset];
    let return_number = bytes[offset + 1];
    let returns_per_pulse = bytes[offset + 2];
    offset += 3;
    let scan_angle = f32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("fixed width"));
    offset += 4;
    let user_data = bytes[offset];
    offset += 1;
    let point_source_id = read_u16(bytes, &mut offset);
    let gps_time = if layout.has_gps_time {
        read_f64(bytes, &mut offset)
    } else {
        0.0
    };
    let rgb = if layout.has_color {
        std::array::from_fn(|_| read_u16(bytes, &mut offset))
    } else {
        [0; 3]
    };
    let nir = if layout.has_nir {
        read_u16(bytes, &mut offset)
    } else {
        0
    };
    let las_flags = if layout.has_flags {
        offset += 1;
        bytes[offset - 1]
    } else {
        0
    };
    let mut extra = [0; MAX_EXTRA_BYTES_PER_POINT];
    let extra_len = usize::from(layout.extra_len);
    extra[..extra_len].copy_from_slice(&bytes[offset..offset + extra_len]);
    offset += extra_len;
    let display_color = ColorRgb::new(bytes[offset], bytes[offset + 1], bytes[offset + 2]);
    SpoolPoint {
        raw_position,
        position,
        intensity,
        classification,
        return_number,
        returns_per_pulse,
        scan_angle,
        user_data,
        point_source_id,
        gps_time,
        rgb,
        nir,
        las_flags,
        extra,
        display_color,
    }
}

/// Position of a source-layout record: the first half of [`decode_point`] without the rest.
#[expect(
    clippy::inline_always,
    reason = "hot loop: the same measured gain as `decode_point`"
)]
#[inline(always)]
pub fn decode_position(bytes: &[u8], layout: SpoolLayout) -> [f64; 3] {
    debug_assert!(layout.source_coordinates);
    let raw = std::array::from_fn(|axis| {
        i32::from_le_bytes(
            bytes[4 * axis..4 * axis + 4]
                .try_into()
                .expect("fixed width"),
        )
    });
    layout.position(raw)
}

#[inline]
fn read_u16(bytes: &[u8], offset: &mut usize) -> u16 {
    let value = u16::from_le_bytes([bytes[*offset], bytes[*offset + 1]]);
    *offset += 2;
    value
}

#[inline]
fn read_f64(bytes: &[u8], offset: &mut usize) -> f64 {
    let value = f64::from_le_bytes(bytes[*offset..*offset + 8].try_into().expect("fixed width"));
    *offset += 8;
    value
}

fn cube_bounds(bounds: SpatialBounds) -> SpatialBounds {
    let side = (0..3)
        .map(|axis| bounds.max()[axis] - bounds.min()[axis])
        .fold(0.0, f64::max);
    if side == 0.0 {
        return bounds;
    }
    let min = bounds.min();
    let max = std::array::from_fn(|axis| (min[axis] + side).max(bounds.max()[axis]));
    SpatialBounds::new(min, max).expect("finite source bounds produce finite cube bounds")
}

#[inline]
pub fn cube_side(bounds: SpatialBounds) -> f64 {
    (0..3)
        .map(|axis| bounds.max()[axis] - bounds.min()[axis])
        .fold(0.0, f64::max)
}

#[cfg(test)]
fn child_index(position: [f64; 3], bounds: SpatialBounds, divisions: u8) -> usize {
    viewer_core::SpatialSubdivision::new(bounds, divisions)
        .expect("converter subdivisions are validated")
        .cell_index(position)
}

#[cfg(test)]
fn child_bounds(bounds: SpatialBounds, child: usize, divisions: u8) -> SpatialBounds {
    viewer_core::SpatialSubdivision::new(bounds, divisions)
        .expect("converter subdivisions are validated")
        .cell_bounds(child)
        .expect("child index is in range")
}

#[inline]
pub fn sample_index(position: [f64; 3], bounds: SpatialBounds, grid: usize) -> usize {
    let side = cube_side(bounds);
    let mut cell = [0_usize; 3];
    for axis in 0..3 {
        let scaled = ((position[axis] - bounds.min()[axis]) / side * grid as f64).floor();
        cell[axis] = (scaled as usize).min(grid - 1);
    }
    (cell[0] * grid + cell[1]) * grid + cell[2]
}

/// `sample_index` and `voxel_center` of the same position in one pass: the cell coordinates are
/// reused instead of divided back out of the index (same arithmetic, so identical results).
#[inline]
pub fn sample_cell(position: [f64; 3], bounds: SpatialBounds, grid: usize) -> (usize, [f64; 3]) {
    let side = cube_side(bounds);
    let cell_size = side / grid as f64;
    let mut cell = [0_usize; 3];
    for axis in 0..3 {
        let scaled = ((position[axis] - bounds.min()[axis]) / side * grid as f64).floor();
        cell[axis] = (scaled as usize).min(grid - 1);
    }
    let center =
        std::array::from_fn(|axis| bounds.min()[axis] + (cell[axis] as f64 + 0.5) * cell_size);
    ((cell[0] * grid + cell[1]) * grid + cell[2], center)
}

/// Exponent of a positive power-of-two side read from the `f64` bits (no `log2`).
fn power_of_two_exponent(value: f64) -> i32 {
    ((value.to_bits() >> 52) & 0x7ff) as i32 - 1023
}

/// Absolute level `a` of a `v7.4-pre` node from its cube (side `2^(49 - a)` CRS units).
pub fn global_level(bounds: SpatialBounds) -> u32 {
    (49 - power_of_two_exponent(cube_side(bounds))).clamp(0, 64) as u32
}

/// Side of a `v7.4-pre` node in quanta as a power-of-two exponent (`64 - a`).
fn global_side_exponent(bounds: SpatialBounds) -> u32 {
    64 - global_level(bounds)
}

fn quanta(position: [f64; 3]) -> [i64; 3] {
    // Positions of a global conversion are on the grid and inside the coordinate limit.
    // `as` saturates: a level-0 corner -2^48 units is exactly -2^63 quanta.
    position.map(|value| viewer_format::round_to_quanta(value * 32_768.0))
}

/// Per-node constants of [`sample_cell_global`]: computed once instead of per point.
#[derive(Clone, Copy)]
pub struct GlobalCellFrame {
    origin: [i64; 3],
    shift: u32,
    grid: usize,
    /// Every cell center is an `i64`: the cube lies inside the coordinate domain, so the
    /// centers need no `i128` arithmetic or conversion.
    centers_fit: bool,
}

impl GlobalCellFrame {
    pub fn new(bounds: SpatialBounds, grid: usize) -> Self {
        let origin = quanta(bounds.min());
        let side_exponent = global_side_exponent(bounds);
        let centers_fit = origin
            .iter()
            .all(|&origin| (i128::from(origin) + (1_i128 << side_exponent)) <= 1_i128 << 63);
        Self {
            origin,
            shift: side_exponent - grid.trailing_zeros(),
            grid,
            centers_fit,
        }
    }

    #[inline]
    fn center(&self, axis: usize, cell: usize) -> f64 {
        if self.centers_fit {
            let half = if self.shift == 0 {
                0
            } else {
                1_u64 << (self.shift - 1)
            };
            // The true sum is an `i64`, so wrapping arithmetic yields it exactly.
            let quanta = self.origin[axis]
                .wrapping_add(((cell as u64) << self.shift) as i64)
                .wrapping_add(half as i64);
            quanta as f64 * viewer_format::quanta_to_units(1)
        } else {
            global_cell_center(self.origin[axis], cell, self.shift)
        }
    }

    /// Cell index and center of `position`; see [`sample_cell_global`].
    #[inline]
    pub fn cell(&self, position: [f64; 3]) -> (usize, [f64; 3]) {
        let q = quanta(position);
        let grid = self.grid;
        let mut cell = [0_usize; 3];
        let mut center = [0.0_f64; 3];
        for axis in 0..3 {
            // The difference fits `i64` for any point inside a node below the root; wider
            // ones take the `i128` path (same value, only slower).
            cell[axis] = match q[axis].checked_sub(self.origin[axis]) {
                Some(relative) => (relative >> self.shift).clamp(0, grid as i64 - 1) as usize,
                None => ((i128::from(q[axis]) - i128::from(self.origin[axis])) >> self.shift)
                    .clamp(0, grid as i128 - 1) as usize,
            };
            center[axis] = self.center(axis, cell[axis]);
        }
        ((cell[0] * grid + cell[1]) * grid + cell[2], center)
    }
}

/// [`sample_cell`] on exact integers for a node of a global octree: the cell of `position` is
/// `(q - origin) >> (side_exponent - log2(grid))` and the center is an integer number of quanta.
#[inline]
pub fn sample_cell_global(
    position: [f64; 3],
    bounds: SpatialBounds,
    grid: usize,
) -> (usize, [f64; 3]) {
    GlobalCellFrame::new(bounds, grid).cell(position)
}

#[inline]
fn global_cell_center(origin: i64, cell: usize, shift: u32) -> f64 {
    let half = if shift == 0 { 0 } else { 1_i128 << (shift - 1) };
    let center = i128::from(origin) + ((cell as i128) << shift) + half;
    center as f64 * viewer_format::quanta_to_units(1)
}

#[inline]
pub fn voxel_center_global(index: usize, bounds: SpatialBounds, grid: usize) -> [f64; 3] {
    let shift = global_side_exponent(bounds) - grid.trailing_zeros();
    let origin = quanta(bounds.min());
    let cell = [index / (grid * grid), (index / grid) % grid, index % grid];
    std::array::from_fn(|axis| global_cell_center(origin[axis], cell[axis], shift))
}

#[inline]
pub fn voxel_center(index: usize, bounds: SpatialBounds, grid: usize) -> [f64; 3] {
    let cell = [index / (grid * grid), (index / grid) % grid, index % grid];
    let cell_size = cube_side(bounds) / grid as f64;
    std::array::from_fn(|axis| bounds.min()[axis] + (cell[axis] as f64 + 0.5) * cell_size)
}

/// Half the cell diagonal, or the whole diagonal where samples sit at their nearest point
/// (`v7.4-pre` nodes with `a < 32`).
pub fn lod_sample_error(bounds: SpatialBounds, grid: usize, layout: SpoolLayout) -> f64 {
    let half_diagonal = (3.0_f64).sqrt() * (cube_side(bounds) / grid as f64) / 2.0;
    if layout.is_global() && global_level(bounds) < 32 {
        2.0 * half_diagonal
    } else {
        half_diagonal
    }
}

pub fn center_rounding_error(bounds: SpatialBounds, frame: viewer_core::CoordinateFrame) -> f64 {
    (0..3)
        .map(|axis| {
            let min = bounds.min()[axis] - frame.origin()[axis];
            let max = bounds.max()[axis] - frame.origin()[axis];
            let coordinate = if min.abs() > max.abs() { min } else { max };
            let rounded = coordinate as f32;
            let magnitude_bits = rounded.abs().to_bits();
            let magnitude = f64::from(rounded.abs());
            let lower = if magnitude_bits == 0 {
                0.0
            } else {
                magnitude - f64::from(f32::from_bits(magnitude_bits - 1))
            };
            let upper = magnitude_bits
                .checked_add(1)
                .map(f32::from_bits)
                .filter(|next| next.is_finite())
                .map_or(lower, |next| f64::from(next) - magnitude);
            lower.max(upper) / 2.0
        })
        .map(|error| error * error)
        .sum::<f64>()
        .sqrt()
}

fn estimate_memory(
    point_limit: u64,
    node_count: usize,
    block_count: usize,
    spatial_divisions: u8,
    sample_grid: usize,
) -> Result<u64, ConvertError> {
    let node_bytes = u64::try_from(node_count)
        .map_err(|_| ConvertError::SizeOverflow)?
        .checked_mul(PLANNED_NODE_BYTES)
        .ok_or(ConvertError::SizeOverflow)?;
    let block_bytes = u64::try_from(block_count)
        .map_err(|_| ConvertError::SizeOverflow)?
        .checked_mul(crate::options::ESTIMATED_DIRECTORY_BYTES_PER_BLOCK)
        .ok_or(ConvertError::SizeOverflow)?;
    let working_bytes = point_limit
        .checked_mul(crate::options::ESTIMATED_WORKING_BYTES_PER_POINT)
        .ok_or(ConvertError::SizeOverflow)?;
    let sample_accumulators = sample_accumulator_bytes(sample_grid, point_limit)?;
    let child_spool_buffers = child_spool_buffer_bytes(spatial_divisions)?;
    working_bytes
        .checked_add(node_bytes)
        .and_then(|bytes| bytes.checked_add(block_bytes))
        .and_then(|bytes| bytes.checked_add(sample_accumulators))
        .and_then(|bytes| bytes.checked_add(child_spool_buffers))
        .and_then(|bytes| bytes.checked_add(crate::options::ESTIMATED_FIXED_OVERHEAD_BYTES))
        .ok_or(ConvertError::SizeOverflow)
}

/// What each root range beyond the first adds to the root's working set: its decode batch, child
/// buffers, sample accumulators and fixed overhead.
fn range_memory_bytes(
    point_limit: u64,
    spatial_divisions: u8,
    sample_grid: usize,
) -> Result<u64, ConvertError> {
    point_limit
        .checked_mul(crate::options::ESTIMATED_WORKING_BYTES_PER_POINT)
        .and_then(|bytes| bytes.checked_add(child_spool_buffer_bytes(spatial_divisions).ok()?))
        .and_then(|bytes| {
            bytes.checked_add(sample_accumulator_bytes(sample_grid, point_limit).ok()?)
        })
        .and_then(|bytes| bytes.checked_add(SPATIAL_WORKER_OVERHEAD_BYTES))
        .ok_or(ConvertError::SizeOverflow)
}

/// The sample grid is sparse: at most one accumulator per occupied cell, and a node holds about
/// `point_limit` points, so occupied cells are bounded by `min(g³, max(point_limit, 8³))`; the
/// floor keeps the estimate of the default 8³ grid unchanged for small limits.
fn sample_accumulator_bytes(sample_grid: usize, point_limit: u64) -> Result<u64, ConvertError> {
    (sample_grid as u64)
        .checked_pow(3)
        .map(|cells| cells.min(point_limit.max(512)))
        .and_then(|cells| cells.checked_mul(256))
        .ok_or(ConvertError::SizeOverflow)
}

fn child_spool_buffer_bytes(spatial_divisions: u8) -> Result<u64, ConvertError> {
    u64::from(spatial_divisions)
        .checked_pow(3)
        .and_then(|children| children.checked_mul(CHILD_SPOOL_BUFFER_BYTES as u64))
        .ok_or(ConvertError::SizeOverflow)
}

fn branch_memory_budget(
    total_budget: u64,
    point_limit: u64,
    completed_nodes: usize,
    completed_blocks: usize,
    worker_count: usize,
    spatial_divisions: u8,
    sample_grid: usize,
) -> Result<u64, ConvertError> {
    let base = estimate_memory(point_limit, 0, 0, spatial_divisions, sample_grid)?;
    let current = estimate_memory(
        point_limit,
        completed_nodes,
        completed_blocks,
        spatial_divisions,
        sample_grid,
    )?;
    let current_index = current.saturating_sub(base);
    let workers = u64::try_from(worker_count).map_err(|_| ConvertError::SizeOverflow)?;
    let worker_overhead = workers
        .checked_mul(SPATIAL_WORKER_OVERHEAD_BYTES)
        .ok_or(ConvertError::SizeOverflow)?;
    let additional_spool_buffers = workers
        .saturating_sub(1)
        .checked_mul(child_spool_buffer_bytes(spatial_divisions)?)
        .ok_or(ConvertError::SizeOverflow)?;
    let reserved = base
        .checked_add(
            current_index
                .checked_mul(2)
                .ok_or(ConvertError::SizeOverflow)?,
        )
        .and_then(|bytes| bytes.checked_add(worker_overhead))
        .and_then(|bytes| bytes.checked_add(additional_spool_buffers))
        .ok_or(ConvertError::SizeOverflow)?;
    let available =
        total_budget
            .checked_sub(reserved)
            .ok_or(ConvertError::MemoryBudgetExceeded {
                requested: reserved,
                limit: total_budget,
            })?;
    let concurrent_index_share = available / workers / 2;
    base.checked_add(concurrent_index_share)
        .ok_or(ConvertError::SizeOverflow)
}

fn check_memory_budget(requested: u64, limit: u64) -> Result<(), ConvertError> {
    if requested > limit {
        Err(ConvertError::MemoryBudgetExceeded { requested, limit })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod partition_tests {
    use super::*;
    use crate::pipeline::spool_store::MemSpoolStore;
    use las::PointDataBuilder;

    fn spool_fixture(format_id: u8) -> (SourceInfo, SpoolPoint) {
        let transforms = las::Vector {
            x: las::Transform {
                scale: 0.000_003_7,
                offset: 1.0e16,
            },
            y: las::Transform {
                scale: 0.031_415_926_535,
                offset: -9.0e14,
            },
            z: las::Transform {
                scale: 0.000_123_456_789,
                offset: 8.0e12,
            },
        };
        let raw_position = [i32::MAX - 19, i32::MIN + 31, -123_456_789];
        let position = [
            transforms.x.direct(raw_position[0]),
            transforms.y.direct(raw_position[1]),
            transforms.z.direct(raw_position[2]),
        ];
        let source = SourceInfo {
            bounds: SpatialBounds::new(position, position).unwrap(),
            frame: viewer_core::CoordinateFrame::new(position).unwrap(),
            point_count: 1,
            record_length: las::point::Format::new(format_id).unwrap().len() as usize,
            point_format: las::point::Format::new(format_id).unwrap(),
            transforms,
            has_crs: false,
            crs: viewer_format::CrsRecord::local(),
            scale_offset: None,
            global: None,
            extra_byte_vlrs: Vec::new(),
            las_extra_bytes: None,
        };
        let point = SpoolPoint {
            raw_position,
            position,
            intensity: 45_678,
            classification: 7,
            return_number: 2,
            returns_per_pulse: 3,
            scan_angle: -12.5,
            user_data: 123,
            point_source_id: 12_345,
            gps_time: 123_456.789,
            rgb: [12_345, 23_456, 34_567],
            nir: 54_321,
            las_flags: 0,
            extra: [0; MAX_EXTRA_BYTES_PER_POINT],
            display_color: ColorRgb::new(71, 82, 93),
        };
        (source, point)
    }

    #[test]
    fn source_spools_preserve_exact_raw_xyz_with_large_origins() {
        let (source, fixture) = spool_fixture(3);
        let layout = SpoolLayout::source(&source);
        let mut las_bytes = vec![0; source.record_length];
        for (axis, coordinate) in fixture.raw_position.iter().enumerate() {
            las_bytes[axis * 4..axis * 4 + 4].copy_from_slice(&coordinate.to_le_bytes());
        }
        let data = PointDataBuilder::new()
            .with_format(source.point_format)
            .with_transforms(source.transforms)
            .build_from_bytes(las_bytes)
            .unwrap();
        let expected_position = [
            data.x().next().unwrap(),
            data.y().next().unwrap(),
            data.z().next().unwrap(),
        ];
        // The large origin loses low bits in f64; an inverse transform cannot recover the raw value.
        assert_ne!(
            source.transforms.x.inverse(expected_position[0]).ok(),
            Some(fixture.raw_position[0])
        );
        let mut bytes = Vec::new();
        write_source_points(&data, &source, 0, &mut bytes).unwrap();
        assert_eq!(bytes.len(), 41);
        assert_eq!(
            &bytes[..12],
            &fixture
                .raw_position
                .iter()
                .flat_map(|coordinate| coordinate.to_le_bytes())
                .collect::<Vec<_>>()[..]
        );
        let decoded = read_point(&mut bytes.as_slice(), layout).unwrap();
        assert_eq!(decoded.raw_position, fixture.raw_position);
        assert_eq!(
            decoded.position.map(f64::to_bits),
            expected_position.map(f64::to_bits)
        );
        let mut rewritten = Vec::new();
        write_point(&mut rewritten, &decoded, layout).unwrap();
        assert_eq!(bytes, rewritten);
    }

    #[test]
    fn child_spool_diagnostics_count_only_flushed_payload() {
        let (source, point) = spool_fixture(3);
        let layout = SpoolLayout::source(&source);
        let store = MemSpoolStore::new(true);
        let mut child = ChildSpool::new(SpoolId(1), layout, true);

        child.write_point(&store, &point).unwrap();
        assert_eq!(child.payload_written_bytes, Some(0));
        assert!(!child.created, "an unflushed child has no spool");
        child.flush(&store).unwrap();
        assert_eq!(
            child.payload_written_bytes,
            Some(layout.record_bytes() as u64)
        );
    }

    #[test]
    fn compact_layout_sizes_and_optional_attributes_match_source_format() {
        for (format_id, source_bytes, lod_bytes) in
            [(0, 27, 39), (2, 33, 45), (3, 41, 53), (8, 43, 55)]
        {
            let (source, fixture) = spool_fixture(format_id);
            let source_layout = SpoolLayout::source(&source);
            for (layout, expected_bytes) in [
                (source_layout, source_bytes),
                (source_layout.lod(), lod_bytes),
            ] {
                let mut bytes = Vec::new();
                write_point(&mut bytes, &fixture, layout).unwrap();
                assert_eq!(layout.record_bytes(), expected_bytes);
                assert_eq!(bytes.len(), expected_bytes);
                let decoded = read_point(&mut bytes.as_slice(), layout).unwrap();
                assert_eq!(decoded.intensity, fixture.intensity);
                assert_eq!(decoded.classification, fixture.classification);
                assert_eq!(decoded.return_number, fixture.return_number);
                assert_eq!(decoded.returns_per_pulse, fixture.returns_per_pulse);
                assert_eq!(decoded.scan_angle, fixture.scan_angle);
                assert_eq!(decoded.user_data, fixture.user_data);
                assert_eq!(decoded.point_source_id, fixture.point_source_id);
                assert_eq!(decoded.display_color, fixture.display_color);
                assert_eq!(
                    decoded.gps_time,
                    if layout.has_gps_time {
                        fixture.gps_time
                    } else {
                        0.0
                    }
                );
                assert_eq!(
                    decoded.rgb,
                    if layout.has_color {
                        fixture.rgb
                    } else {
                        [0; 3]
                    }
                );
                assert_eq!(decoded.nir, if layout.has_nir { fixture.nir } else { 0 });
            }
        }
    }

    #[test]
    fn synthetic_lod_centers_preserve_f64_bits_outside_las_grid() {
        let (source, fixture) = spool_fixture(3);
        let source_layout = SpoolLayout::source(&source);
        let layout = source_layout.lod();
        let center = [
            0.123_456_789_012_345,
            -1.234_567_890_123_456,
            987_654.321_098_765_4,
        ];
        let mut record = Vec::new();
        write_point(&mut record, &fixture, source_layout).unwrap();
        let mut records = NearestRecords::new(source_layout, 1);
        let mut sample = SampleAccumulator::default();
        sample.add(
            &mut records,
            source_layout,
            &record,
            fixture.position,
            center,
        );
        let synthetic = sample.finish(&records, source_layout, center);
        let mut bytes = Vec::new();
        write_point(&mut bytes, &synthetic, layout).unwrap();
        assert_eq!(bytes.len(), 53);
        let decoded = read_point(&mut bytes.as_slice(), layout).unwrap();
        assert_eq!(decoded.position.map(f64::to_bits), center.map(f64::to_bits));
        assert_eq!(decoded.gps_time, fixture.gps_time);
        assert_eq!(decoded.rgb, fixture.rgb);
        assert_eq!(decoded.display_color, fixture.display_color);
    }

    #[test]
    fn planned_blocks_seek_using_their_compact_record_layout() {
        let store = MemSpoolStore::new(false);
        let mut next_id = 1;
        for format_id in [0, 2, 3, 8] {
            let (source, fixture) = spool_fixture(format_id);
            let source_layout = SpoolLayout::source(&source);
            for layout in [source_layout, source_layout.lod()] {
                let mut second = fixture;
                second.raw_position[2] += 123;
                second.position = layout.position(second.raw_position);
                second.intensity += 1;
                let spool = SpoolId(next_id);
                next_id += 1;
                let mut bytes = Vec::new();
                write_point(&mut bytes, &fixture, layout).unwrap();
                write_point(&mut bytes, &second, layout).unwrap();
                store.create(spool).unwrap();
                store.append(spool, &bytes).unwrap();
                store.close(spool).unwrap();
                let block = PlannedBlock {
                    id: BlockId::new(0),
                    spool,
                    offset_points: 1,
                    point_count: 1,
                    layout,
                    extras: Vec::new().into(),
                    is_source_points: layout.source_coordinates,
                    final_block_in_spool: true,
                }
                .read(&store, source.frame)
                .unwrap();
                assert_eq!(
                    block.positions_local(),
                    &[std::array::from_fn(
                        |axis| second.position[axis] - source.frame.origin()[axis]
                    )]
                );
                assert_eq!(block.colors(), &[second.display_color]);
                assert_eq!(
                    block.attributes()[0].values(),
                    &AttributeValues::U16(vec![second.intensity])
                );
                assert_eq!(
                    block.attributes().len(),
                    7 + usize::from(layout.has_gps_time)
                        + usize::from(layout.has_color)
                        + usize::from(layout.has_nir)
                );
            }
        }
    }

    #[test]
    fn bulk_spool_reads_decode_records_and_reject_partial_tails() {
        for format_id in [0, 2, 3, 8] {
            let (source, fixture) = spool_fixture(format_id);
            let layout = SpoolLayout::source(&source);
            let mut expected = fixture;
            expected.raw_position[0] += 19;
            expected.position = layout.position(expected.raw_position);
            let mut bytes = Vec::new();
            write_point(&mut bytes, &fixture, layout).unwrap();
            write_point(&mut bytes, &expected, layout).unwrap();

            let mut reader = bytes.as_slice();
            let mut batch = vec![0; layout.record_bytes() * 3];
            assert_eq!(read_records(&mut reader, &mut batch, layout).unwrap(), 2);
            let decoded = batch
                .chunks_exact(layout.record_bytes())
                .map(|record| decode_point(record, layout))
                .collect::<Vec<_>>();
            assert_eq!(decoded[0].raw_position, fixture.raw_position);
            assert_eq!(decoded[1].raw_position, expected.raw_position);
            assert_eq!(decoded[1].intensity, expected.intensity);

            bytes.push(0);
            let mut truncated = bytes.as_slice();
            let error = read_records(&mut truncated, &mut batch, layout).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        }
    }

    #[test]
    fn header_bounds_allow_small_metadata_rounding_but_reject_real_outliers() {
        let bounds = SpatialBounds::new(
            [692_199.5, 4_285_716.0, 329.910_095_214_843_75],
            [692_838.687_5, 4_286_262.5, 359.015_899_658_203_1],
        )
        .unwrap();

        assert!(!outside_header_bounds(
            [692_399.511_1, 4_285_715.937_7, 335.174],
            bounds
        ));
        assert!(outside_header_bounds(
            [692_399.511_1, 4_285_715.0, 335.174],
            bounds
        ));
    }

    #[test]
    fn adaptive_cells_cover_centers_and_assign_internal_planes_to_upper_cell() {
        let bounds = SpatialBounds::new([-4.0, 2.0, -8.0], [4.0, 10.0, 0.0]).unwrap();
        for divisions in [2_u8, 4, 8] {
            let divisions_usize = usize::from(divisions);
            for x in 0..divisions_usize {
                for y in 0..divisions_usize {
                    for z in 0..divisions_usize {
                        let key = (x * divisions_usize + y) * divisions_usize + z;
                        let child = child_bounds(bounds, key, divisions);
                        let position = std::array::from_fn(|axis| {
                            f64::midpoint(child.min()[axis], child.max()[axis])
                        });
                        assert_eq!(child_index(position, bounds, divisions), key);
                        for axis in 0..3 {
                            assert!(position[axis] >= child.min()[axis]);
                            assert!(position[axis] <= child.max()[axis]);
                            if [x, y, z][axis] + 1 < divisions_usize {
                                let plane = child.max()[axis];
                                let mut on_plane = position;
                                on_plane[axis] = plane;
                                let next_key = child_index(on_plane, bounds, divisions);
                                let expected_cell = [x, y, z][axis] + 1;
                                let actual_cell = match axis {
                                    0 => next_key / (divisions_usize * divisions_usize),
                                    1 => (next_key / divisions_usize) % divisions_usize,
                                    _ => next_key % divisions_usize,
                                };
                                assert_eq!(actual_cell, expected_cell);
                            }
                        }
                    }
                }
            }
            assert_eq!(
                child_index(bounds.max(), bounds, divisions),
                divisions_usize.pow(3) - 1
            );
        }
    }

    #[test]
    fn binary_partition_keeps_midpoint_and_key_order() {
        let bounds = SpatialBounds::new([0.0; 3], [8.0; 3]).unwrap();
        assert_eq!(child_index([3.0; 3], bounds, 2), 0);
        assert_eq!(child_index([4.0; 3], bounds, 2), 7);
        let upper = child_bounds(bounds, 7, 2);
        assert_eq!(upper.min(), [4.0; 3]);
        assert_eq!(upper.max(), [8.0; 3]);
    }

    #[test]
    fn fanout_spool_memory_is_bounded_for_supported_divisions() {
        assert_eq!(child_spool_buffer_bytes(2).unwrap(), 8 * 32 * 1024);
        assert_eq!(child_spool_buffer_bytes(4).unwrap(), 64 * 32 * 1024);
        assert_eq!(child_spool_buffer_bytes(8).unwrap(), 512 * 32 * 1024);
    }
}
