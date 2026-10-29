//! Per-node spatial planning jobs. A job reads one input spool (or the source points for the
//! root), writes its child and LOD spools through a [`SpoolStore`] and returns what the host
//! commits: blocks without ids, non-empty children and the spools to release. Jobs see only the
//! store, spool ids and plain values; ids, budgets and progress forwarding belong to the host.

use las::PointDataBuilder;
use viewer_core::{CoordinateFrame, SpatialBounds, SpatialSubdivision};

use super::{
    clock::Clock,
    point_source::PointSource,
    spool_store::{SPOOL_IO_BUFFER_BYTES, SpoolExtent, SpoolId, SpoolStore},
};
use crate::ConvertError;
use crate::las_source::SourceInfo;
use crate::spatial::{
    ChildSpool, MAX_SPATIAL_DEPTH, SAMPLE_GRID_SIZE, SampleAccumulator, SpoolLayout, SpoolPoint,
    StoreWriter, center_rounding_error, cube_side, decode_point, read_records, sample_index,
    visit_source_points, voxel_center, write_point, write_source_points,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct NodePath(pub Vec<u16>); // child keys from the root; Ord = preorder

impl NodePath {
    pub fn child(&self, key: u16) -> Self {
        let mut keys = self.0.clone();
        keys.push(key);
        Self(keys)
    }
}

pub(crate) struct JobContext {
    pub point_limit: u64,
    pub spatial_divisions: u8,
    pub frame: CoordinateFrame,
    pub layout: SpoolLayout,
    pub collect_diagnostics: bool,
}

impl JobContext {
    /// Whether a node with these values is partitioned; the root (level 0) uses the same rule.
    pub fn splits(&self, point_count: u64, level: u32, bounds: SpatialBounds) -> bool {
        point_count > self.point_limit && level < MAX_SPATIAL_DEPTH && cube_side(bounds) > 0.0
    }

    fn child_count(&self) -> usize {
        usize::from(self.spatial_divisions).pow(3)
    }
}

/// `first_output..first_output + divisions³` are reserved by the host only when the job splits
/// (child `k` → `first_output + k`, LOD → `first_output + divisions³`); a leaf job ignores it.
pub(crate) struct NodeJob {
    pub path: NodePath,
    pub level: u32,
    pub bounds: SpatialBounds,
    pub input: SpoolId,
    pub point_count: u64,
    pub input_is_child_spool: bool,
    pub first_output: SpoolId,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChildOutput {
    pub key: u16,
    pub spool: SpoolId,
    pub point_count: u64,
    pub bounds: SpatialBounds,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BlockPlan {
    pub spool: SpoolId,
    pub offset_points: u64,
    pub point_count: usize,
    pub is_source_points: bool,
    pub final_block_in_spool: bool,
}

pub(crate) struct NodeJobOutput {
    pub path: NodePath,
    pub point_count: u64, // LOD samples, or input points for a leaf
    pub geometric_error: f64,
    pub blocks: Vec<BlockPlan>,      // no ids yet, in allocation order
    pub children: Vec<ChildOutput>,  // ascending key, empty ones skipped
    pub release: Vec<SpoolId>,       // deleted by the host after the job
    pub metrics: Option<JobMetrics>, // bytes per child, scanned, LOD, phases in ns
}

impl NodeJobOutput {
    /// A divisible node always has a non-empty child, so no children means a leaf.
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }
}

/// Diagnostics of one job (only with `JobContext::collect_diagnostics`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct JobMetrics {
    /// Payload bytes flushed to each created child spool, ascending key; includes the children
    /// of an all-equal node, which are released again.
    pub child_spool_bytes: Vec<u64>,
    pub scanned_bytes: u64,
    pub lod_bytes: u64,
    pub became_leaf_all_equal: bool,
    pub las_fill_ns: u64,
    pub scan_read_ns: u64,
    pub partition_and_sample_ns: u64,
    pub lod_write_ns: u64,
}

pub(crate) enum JobProgress {
    /// Source points read so far by this job (relative to its range start).
    SourcePoints(u64),
    /// Emitted every `point_limit` scanned spool records.
    NodeHeartbeat,
}

/// The 8×8×8 LOD accumulators of one node, indexed by voxel.
pub(crate) struct SampleGrid(Vec<Option<SampleAccumulator>>);

impl SampleGrid {
    fn new() -> Self {
        Self(
            std::iter::repeat_with(|| None)
                .take(SAMPLE_GRID_SIZE.pow(3))
                .collect(),
        )
    }

    #[inline(always)]
    fn add(&mut self, point: &SpoolPoint, bounds: SpatialBounds) {
        let voxel_key = sample_index(point.position, bounds);
        let center = voxel_center(voxel_key, bounds);
        self.0[voxel_key]
            .get_or_insert_with(SampleAccumulator::default)
            .add(point, center);
    }
}

pub(crate) fn run_node_job<S: SpoolStore, C: Clock>(
    ctx: &JobContext,
    job: &NodeJob,
    store: &S,
    clock: &C,
    progress: &mut impl FnMut(JobProgress) -> Result<(), ConvertError>,
) -> Result<NodeJobOutput, ConvertError> {
    let mut metrics = ctx.collect_diagnostics.then(JobMetrics::default);
    if !ctx.splits(job.point_count, job.level, job.bounds) {
        return leaf_output(ctx, job, Vec::new(), metrics);
    }
    // A leaf job gets `SOURCE` as a placeholder; a splitting one must have reserved ids.
    debug_assert_ne!(job.first_output, SpoolId::SOURCE, "{:?}", job.path);

    let child_count = ctx.child_count();
    let mut children = (0..child_count)
        .map(|_| None)
        .collect::<Vec<Option<ChildSpool>>>();
    let mut samples = SampleGrid::new();
    let sample_cell_size = cube_side(job.bounds) / SAMPLE_GRID_SIZE as f64;
    let sample_error = (3.0_f64).sqrt() * sample_cell_size / 2.0;
    let mut source_bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 3];
    let subdivision = SpatialSubdivision::new(job.bounds, ctx.spatial_divisions)
        .expect("converter subdivisions are validated");
    let mut scanned = 0_u64;
    // The whole input spool is read, so one longer than `point_count` is still a mismatch.
    let mut reader = store.reader(
        job.input,
        SpoolExtent {
            offset: 0,
            len: store.len(job.input)?,
        },
    )?;
    let record_bytes = ctx.layout.record_bytes();
    let records_per_batch = (SPOOL_IO_BUFFER_BYTES / record_bytes).max(1);
    let mut record_buffer = vec![0; records_per_batch * record_bytes];
    loop {
        let read_started = start_phase(clock, &metrics);
        let records = read_records(&mut reader, &mut record_buffer, ctx.layout)?;
        end_phase(clock, &mut metrics, read_started, |m| &mut m.scan_read_ns);
        if records == 0 {
            break;
        }
        if let Some(metrics) = metrics.as_mut() {
            let bytes = records
                .checked_mul(record_bytes)
                .and_then(|bytes| u64::try_from(bytes).ok())
                .ok_or(ConvertError::SizeOverflow)?;
            metrics.scanned_bytes = metrics
                .scanned_bytes
                .checked_add(bytes)
                .ok_or(ConvertError::SizeOverflow)?;
        }
        let classify_started = start_phase(clock, &metrics);
        for bytes in record_buffer[..records * record_bytes].chunks_exact(record_bytes) {
            let point = decode_point(bytes, ctx.layout);
            scanned += 1;
            for (axis, axis_bounds) in source_bounds.iter_mut().enumerate() {
                axis_bounds[0] = axis_bounds[0].min(point.position[axis]);
                axis_bounds[1] = axis_bounds[1].max(point.position[axis]);
            }
            if scanned.is_multiple_of(ctx.point_limit) {
                progress(JobProgress::NodeHeartbeat)?;
            }
            let child_key = subdivision.cell_index(point.position);
            let child = children[child_key].get_or_insert_with(|| {
                ChildSpool::new(
                    SpoolId(job.first_output.0 + child_key as u64),
                    ctx.layout,
                    ctx.collect_diagnostics,
                )
            });
            child.write_point(store, &point)?;
            samples.add(&point, job.bounds);
        }
        end_phase(clock, &mut metrics, classify_started, |m| {
            &mut m.partition_and_sample_ns
        });
    }
    if scanned != job.point_count {
        return Err(ConvertError::PointCountMismatch {
            expected: job.point_count,
            actual: scanned,
        });
    }
    for child in children.iter_mut().flatten() {
        child.finish(store)?;
    }
    if let Some(metrics) = metrics.as_mut() {
        metrics.child_spool_bytes = child_payload_bytes(&children);
    }

    if all_equal(&source_bounds) {
        if let Some(metrics) = metrics.as_mut() {
            metrics.became_leaf_all_equal = true;
        }
        let release = created_spools(&children);
        return leaf_output(ctx, job, release, metrics);
    }

    let lod_started = start_phase(clock, &metrics);
    let lod_spool = SpoolId(job.first_output.0 + child_count as u64);
    let sample_count = write_lod(store, lod_spool, &samples, job.bounds, ctx.layout)?;
    end_phase(clock, &mut metrics, lod_started, |m| &mut m.lod_write_ns);
    if let Some(metrics) = metrics.as_mut() {
        metrics.lod_bytes = (sample_count * ctx.layout.lod().record_bytes()) as u64;
    }
    Ok(NodeJobOutput {
        path: job.path.clone(),
        point_count: sample_count as u64,
        geometric_error: sample_error + center_rounding_error(job.bounds, ctx.frame),
        blocks: lod_blocks(lod_spool, sample_count),
        children: child_outputs(children, subdivision),
        release: vec![job.input],
        metrics,
    })
}

pub(crate) struct RootPartition {
    pub children: Vec<ChildOutput>,
    pub samples: SampleGrid,
    pub all_points_equal: bool,
    pub release: Vec<SpoolId>,
    pub metrics: Option<JobMetrics>,
} // release: created children when all points are equal

/// Partitions the source points of a divisible root into child spools and LOD accumulators. When
/// every point is equal the created children are closed without flushing and returned in
/// `release` (and `children` is empty); the host then spools the source and runs the root as a
/// `NodeJob`, which rescans it and becomes a leaf.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_root_partition<P: PointSource, S: SpoolStore, C: Clock>(
    ctx: &JobContext,
    info: &SourceInfo,
    bounds: SpatialBounds,
    first_output: SpoolId,
    source: &mut P,
    store: &S,
    clock: &C,
    progress: &mut impl FnMut(JobProgress) -> Result<(), ConvertError>,
) -> Result<RootPartition, ConvertError> {
    debug_assert_ne!(first_output, SpoolId::SOURCE);
    // M1 reads the whole source as one range (A9); `processed` below is a global index.
    let range = source.range();
    assert_eq!(
        (range.start, range.end),
        (0, info.point_count),
        "the root partition reads the full source range"
    );
    let mut metrics = ctx.collect_diagnostics.then(JobMetrics::default);
    let mut children = (0..ctx.child_count())
        .map(|_| None)
        .collect::<Vec<Option<ChildSpool>>>();
    let mut samples = SampleGrid::new();
    let mut source_bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 3];
    let subdivision = SpatialSubdivision::new(bounds, ctx.spatial_divisions)
        .expect("converter subdivisions are validated");
    let point_limit = crate::options::input_batch_point_limit(ctx.point_limit, info.record_length);
    let mut point_data = PointDataBuilder::new().for_header(source.header()).build();
    let mut processed = 0_u64;
    while processed < info.point_count {
        let requested = point_limit.min(info.point_count - processed);
        let fill_started = start_phase(clock, &metrics);
        let read = source.fill(requested, &mut point_data)?;
        end_phase(clock, &mut metrics, fill_started, |m| &mut m.las_fill_ns);
        if read != requested {
            return Err(ConvertError::PointCountMismatch {
                expected: info.point_count,
                actual: source.next_index(),
            });
        }
        let classify_started = start_phase(clock, &metrics);
        visit_source_points(&point_data, info, processed, |point| {
            for (axis, axis_bounds) in source_bounds.iter_mut().enumerate() {
                axis_bounds[0] = axis_bounds[0].min(point.position[axis]);
                axis_bounds[1] = axis_bounds[1].max(point.position[axis]);
            }
            let child_key = subdivision.cell_index(point.position);
            let child = children[child_key].get_or_insert_with(|| {
                ChildSpool::new(
                    SpoolId(first_output.0 + child_key as u64),
                    ctx.layout,
                    ctx.collect_diagnostics,
                )
            });
            child.write_point(store, &point)?;
            samples.add(&point, bounds);
            Ok(())
        })?;
        end_phase(clock, &mut metrics, classify_started, |m| {
            &mut m.partition_and_sample_ns
        });
        processed += read;
        progress(JobProgress::SourcePoints(processed))?;
    }

    if all_equal(&source_bounds) {
        if let Some(metrics) = metrics.as_mut() {
            metrics.child_spool_bytes = child_payload_bytes(&children);
        }
        for child in children.iter().flatten() {
            child.close_created(store)?;
        }
        return Ok(RootPartition {
            children: Vec::new(),
            samples,
            all_points_equal: true,
            release: created_spools(&children),
            metrics,
        });
    }

    for child in children.iter_mut().flatten() {
        child.finish(store)?;
    }
    if let Some(metrics) = metrics.as_mut() {
        metrics.child_spool_bytes = child_payload_bytes(&children);
    }
    Ok(RootPartition {
        children: child_outputs(children, subdivision),
        samples,
        all_points_equal: false,
        release: Vec::new(),
        metrics,
    })
}

/// Copies the points of `source` into spool `id` (created and closed here) and returns the
/// payload bytes written.
pub(crate) fn spool_source<P: PointSource, S: SpoolStore>(
    source: &mut P,
    info: &SourceInfo,
    point_limit: u64,
    store: &S,
    id: SpoolId,
    progress: &mut impl FnMut(JobProgress) -> Result<(), ConvertError>,
) -> Result<u64, ConvertError> {
    let range = source.range();
    store.create(id)?;
    let mut writer = StoreWriter { store, id };
    let mut point_data = PointDataBuilder::new().for_header(source.header()).build();
    while source.next_index() < range.end {
        let first_index = source.next_index();
        let requested = point_limit.min(range.end - first_index);
        let read = source.fill(requested, &mut point_data)?;
        if read != requested {
            return Err(ConvertError::PointCountMismatch {
                expected: info.point_count,
                actual: source.next_index(),
            });
        }
        write_source_points(&point_data, info, first_index, &mut writer)?;
        progress(JobProgress::SourcePoints(source.next_index() - range.start))?;
    }
    store.close(id)?;
    (range.end - range.start)
        .checked_mul(SpoolLayout::source(info).record_bytes() as u64)
        .ok_or(ConvertError::SizeOverflow)
}

/// Writes the LOD samples of a node into `id` (no spool when there are none) and returns how many
/// there are. `layout` is the source layout; samples use its LOD form.
pub(crate) fn write_lod(
    store: &impl SpoolStore,
    id: SpoolId,
    samples: &SampleGrid,
    bounds: SpatialBounds,
    layout: SpoolLayout,
) -> Result<usize, ConvertError> {
    let mut encoded = Vec::new();
    let mut count = 0;
    for (key, accumulator) in samples
        .0
        .iter()
        .enumerate()
        .filter_map(|(key, sample)| sample.as_ref().map(|accumulator| (key, accumulator)))
    {
        write_point(
            &mut encoded,
            &accumulator.finish(voxel_center(key, bounds)),
            layout.lod(),
        )?;
        count += 1;
    }
    if count > 0 {
        store.create(id)?;
        store.append(id, &encoded)?;
        store.close(id)?;
    }
    Ok(count)
}

/// The single LOD block of a node, if it has samples.
pub(crate) fn lod_blocks(spool: SpoolId, sample_count: usize) -> Vec<BlockPlan> {
    if sample_count == 0 {
        return Vec::new();
    }
    vec![BlockPlan {
        spool,
        offset_points: 0,
        point_count: sample_count,
        is_source_points: false,
        final_block_in_spool: true,
    }]
}

/// A leaf keeps its input spool for the block phase, read in `point_limit` chunks; an empty one is
/// released.
fn leaf_output(
    ctx: &JobContext,
    job: &NodeJob,
    mut release: Vec<SpoolId>,
    metrics: Option<JobMetrics>,
) -> Result<NodeJobOutput, ConvertError> {
    let mut blocks = Vec::new();
    let mut offset_points = 0_u64;
    while offset_points < job.point_count {
        let count: usize = ctx
            .point_limit
            .min(job.point_count - offset_points)
            .try_into()
            .map_err(|_| ConvertError::SizeOverflow)?;
        blocks.push(BlockPlan {
            spool: job.input,
            offset_points,
            point_count: count,
            is_source_points: true,
            final_block_in_spool: offset_points + count as u64 == job.point_count,
        });
        offset_points += count as u64;
    }
    if job.point_count == 0 {
        release.push(job.input);
    }
    Ok(NodeJobOutput {
        path: job.path.clone(),
        point_count: job.point_count,
        geometric_error: 0.0,
        blocks,
        children: Vec::new(),
        release,
        metrics,
    })
}

fn all_equal(bounds: &[[f64; 2]; 3]) -> bool {
    bounds
        .iter()
        .all(|axis_bounds| axis_bounds[0] == axis_bounds[1])
}

/// Every non-empty child, ascending key. A child exists only once a point was written to it.
fn child_outputs(
    children: Vec<Option<ChildSpool>>,
    subdivision: SpatialSubdivision,
) -> Vec<ChildOutput> {
    children
        .into_iter()
        .enumerate()
        .filter_map(|(key, child)| {
            let child = child?;
            (child.point_count > 0).then(|| ChildOutput {
                key: u16::try_from(key).expect("at most 8³ children"),
                spool: child.id,
                point_count: child.point_count,
                bounds: subdivision
                    .cell_bounds(key)
                    .expect("child index is in range"),
            })
        })
        .collect()
}

fn created_spools(children: &[Option<ChildSpool>]) -> Vec<SpoolId> {
    children
        .iter()
        .flatten()
        .filter(|child| child.created)
        .map(|child| child.id)
        .collect()
}

/// Payload bytes of every spool a node created, in child-key order (diagnostics only).
fn child_payload_bytes(children: &[Option<ChildSpool>]) -> Vec<u64> {
    children
        .iter()
        .flatten()
        .filter_map(|child| child.payload_written_bytes)
        .filter(|bytes| *bytes > 0)
        .collect()
}

fn start_phase<C: Clock>(clock: &C, metrics: &Option<JobMetrics>) -> Option<C::Instant> {
    metrics.as_ref().map(|_| clock.now())
}

fn end_phase<C: Clock>(
    clock: &C,
    metrics: &mut Option<JobMetrics>,
    started: Option<C::Instant>,
    target: impl FnOnce(&mut JobMetrics) -> &mut u64,
) {
    if let (Some(metrics), Some(started)) = (metrics.as_mut(), started) {
        let target = target(metrics);
        *target = target.saturating_add(clock.elapsed_ns(started));
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use las::{Builder, Writer};
    use viewer_core::SpatialBounds;

    use super::*;
    use crate::las_source::{inspect_source, open_las_reader};
    use crate::pipeline::{
        clock::SystemClock,
        point_source::LasPointSource,
        spool_store::{MemSpoolStore, SpoolAccounting},
    };

    const LIMIT: u64 = 16;

    fn write_las(path: &Path, points: &[[f64; 3]]) {
        let header = Builder::from((1, 2)).into_header().unwrap();
        let mut writer = Writer::from_path(path, header).unwrap();
        for (index, [x, y, z]) in points.iter().copied().enumerate() {
            writer
                .write_point(las::Point {
                    x,
                    y,
                    z,
                    intensity: index as u16,
                    ..las::Point::default()
                })
                .unwrap();
        }
        writer.close().unwrap();
    }

    fn context(info: &SourceInfo, divisions: u8) -> JobContext {
        JobContext {
            point_limit: LIMIT,
            spatial_divisions: divisions,
            frame: info.frame,
            layout: SpoolLayout::source(info),
            collect_diagnostics: true,
        }
    }

    fn cube(min: f64, max: f64) -> SpatialBounds {
        SpatialBounds::new([min; 3], [max; 3]).unwrap()
    }

    /// Spools `points` into `SOURCE` of a fresh in-memory store.
    fn spooled(dir: &Path, points: &[[f64; 3]]) -> (SourceInfo, MemSpoolStore) {
        let path = dir.join("input.las");
        write_las(&path, points);
        let reader = open_las_reader(&path).unwrap();
        let info = inspect_source(&reader).unwrap();
        let store = MemSpoolStore::new(false);
        spool_source(
            &mut LasPointSource::full(reader),
            &info,
            LIMIT,
            &store,
            SpoolId::SOURCE,
            &mut |_| Ok(()),
        )
        .unwrap();
        (info, store)
    }

    fn job(info: &SourceInfo, bounds: SpatialBounds) -> NodeJob {
        NodeJob {
            path: NodePath(vec![3]),
            level: 1,
            bounds,
            input: SpoolId::SOURCE,
            point_count: info.point_count,
            input_is_child_spool: true,
            first_output: SpoolId(100),
        }
    }

    #[test]
    fn node_paths_order_as_preorder() {
        let mut paths = [vec![1], vec![0, 7], vec![], vec![0], vec![1, 0]]
            .map(NodePath)
            .to_vec();
        paths.sort();
        assert_eq!(
            paths,
            [vec![], vec![0], vec![0, 7], vec![1], vec![1, 0]].map(NodePath)
        );
    }

    #[test]
    fn empty_children_create_no_spool_and_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        // Two opposite octants of a divisions-2 node; the other six stay empty.
        let points = (0..40)
            .map(|i| {
                let offset = f64::from(i % 20) * 0.1;
                if i < 20 {
                    [1.0 + offset; 3]
                } else {
                    [7.0 + offset; 3]
                }
            })
            .collect::<Vec<_>>();
        let (info, store) = spooled(dir.path(), &points);
        let output = run_node_job(
            &context(&info, 2),
            &job(&info, cube(0.0, 10.0)),
            &store,
            &SystemClock,
            &mut |_| Ok(()),
        )
        .unwrap();

        let keys = output.children.iter().map(|c| c.key).collect::<Vec<_>>();
        assert_eq!(keys, [0, 7]);
        assert_eq!(output.children[0].spool, SpoolId(100));
        assert_eq!(output.children[1].spool, SpoolId(107));
        assert!(output.children.iter().all(|c| c.point_count == 20));
        for key in 1..7 {
            assert!(store.len(SpoolId(100 + key)).is_err(), "key {key}");
        }
        assert_eq!(output.release, [SpoolId::SOURCE]);
        assert_eq!(output.path, NodePath(vec![3]));
        let lod = &output.blocks[..];
        assert_eq!(lod.len(), 1);
        assert_eq!(
            lod[0].spool,
            SpoolId(108),
            "LOD id is first_output + divisions³"
        );
        assert!(!lod[0].is_source_points && lod[0].final_block_in_spool);
        assert_eq!(output.point_count, lod[0].point_count as u64);
        let metrics = output.metrics.unwrap();
        assert_eq!(metrics.child_spool_bytes.len(), 2);
        assert_eq!(metrics.scanned_bytes, store.len(SpoolId::SOURCE).unwrap());
        assert!(!metrics.became_leaf_all_equal);
    }

    #[test]
    fn a_node_with_all_points_equal_becomes_a_leaf_and_releases_its_children() {
        let dir = tempfile::tempdir().unwrap();
        // Enough records for one 32 KiB flush, so a child spool is really created.
        let (info, store) = spooled(dir.path(), &[[2.0, 3.0, 4.0]; 2_000]);
        let output = run_node_job(
            &context(&info, 2),
            &job(&info, cube(0.0, 10.0)),
            &store,
            &SystemClock,
            &mut |_| Ok(()),
        )
        .unwrap();

        assert!(output.is_leaf());
        assert_eq!(output.point_count, 2_000);
        assert_eq!(output.geometric_error, 0.0);
        assert_eq!(output.release, [SpoolId(100)], "the only created child");
        assert!(output.metrics.as_ref().unwrap().became_leaf_all_equal);
        assert_eq!(output.blocks.len(), 2_000_usize.div_ceil(LIMIT as usize));
        assert!(output.blocks.iter().all(|b| b.spool == SpoolId::SOURCE));
        let finals = output.blocks.iter().filter(|b| b.final_block_in_spool);
        assert_eq!(finals.count(), 1);
        assert!(output.blocks.last().unwrap().final_block_in_spool);
        // Released children are closed (the store rejects deleting an open spool); the input is kept.
        store.delete(SpoolId(100)).unwrap();
        assert_eq!(store.stats().live_spools, 1);
        assert!(store.len(SpoolId::SOURCE).is_ok());
    }

    #[test]
    fn an_all_equal_root_closes_its_children_unflushed_and_releases_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.las");
        write_las(&path, &[[2.0, 3.0, 4.0]; 2_000]);
        let reader = open_las_reader(&path).unwrap();
        let info = inspect_source(&reader).unwrap();
        let store = MemSpoolStore::new(true);
        let mut seen = Vec::new();
        let partition = run_root_partition(
            &context(&info, 2),
            &info,
            cube(0.0, 10.0),
            SpoolId(1),
            &mut LasPointSource::full(reader),
            &store,
            &SystemClock,
            &mut |progress| {
                if let JobProgress::SourcePoints(points) = progress {
                    seen.push(points);
                }
                Ok(())
            },
        )
        .unwrap();

        assert!(partition.all_points_equal);
        assert!(partition.children.is_empty());
        assert_eq!(partition.release, [SpoolId(1)]);
        assert_eq!(seen.last(), Some(&2_000));
        let flushed = partition.metrics.unwrap().child_spool_bytes;
        assert_eq!(flushed.len(), 1);
        // Only the flushed 32 KiB chunk reached the store; the buffered tail was dropped.
        assert_eq!(store.len(SpoolId(1)).unwrap(), flushed[0]);
        assert!(flushed[0] < 2_000 * SpoolLayout::source(&info).record_bytes() as u64);
        store.delete(SpoolId(1)).unwrap();
        assert_eq!(store.stats().live_spools, 0);
        assert!(store.ledger_totals().is_some());
    }
}
