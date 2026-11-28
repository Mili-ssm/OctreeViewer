//! The builder that walks the hierarchy and commits node jobs.

use std::sync::atomic::{AtomicU64, Ordering};

use viewer_core::las::LasExtraByte;
use viewer_core::{BlockId, NodeId, SpatialBounds};

use super::block::PlannedBlock;
use super::consume_rank;
use super::executor::{BatchPlanner, RangeRunner};
use super::geometry::{center_rounding_error, lod_sample_error};
use super::memory::{
    check_memory_budget, estimate_memory, memory_limited_batch, range_memory_bytes,
    split_range_capacity,
};
use super::node_job::{
    BlockPlan, ChildOutput, JobContext, JobMetrics, JobProgress, NodeJob, NodeJobOutput, NodePath,
    RangeJob, RootPartition, lod_blocks, merge_root_ranges, run_node_job, run_root_partition,
    spool_source, write_lod,
};
use super::plan::NodeRecord;
use super::source_points::emit_source_progress;
use super::spool::SpoolLayout;
use crate::ConvertError;
use crate::diagnostics::{NodeSpoolEvent, SpatialPhaseDiagnostics, SpatialSpoolDiagnostics};
use crate::pipeline::{
    clock::Clock,
    point_source::{PointRange, SourceFactory},
    spool_store::{PlanStore, SpoolId},
};
use crate::report::{ConversionProgress, ConversionStage, emit_progress};
use crate::source::SourceInfo;

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

    /// The planning estimate of the index this branch holds (nodes and blocks).
    pub fn index_bytes(&self) -> u64 {
        super::memory::index_bytes(self.node_records.len(), self.output_blocks.len())
    }

    /// Nodes committed so far; the next node's branch-local id.
    pub fn node_count(&self) -> u64 {
        self.node_records.len() as u64
    }
}

pub struct SpatialBuilder<'a, S: PlanStore, C: Clock, X> {
    pub(super) store: &'a S,
    pub(super) clock: &'a C,
    pub(super) executor: &'a X,
    /// Next unreserved spool id, shared by every branch.
    pub(super) spool_ids: &'a AtomicU64,
    pub(super) point_limit: u64,
    pub(super) memory_budget: u64,
    pub(super) spatial_divisions: u8,
    pub(super) sample_grid: usize,
    pub(super) frame: viewer_core::CoordinateFrame,
    pub(super) layout: SpoolLayout,
    pub(super) extras: std::sync::Arc<[LasExtraByte]>,
    pub(super) node_records: Vec<NodeRecord>,
    pub(super) output_blocks: Vec<PlannedBlock>,
    pub(super) next_block_id: u64,
    pub(super) on_progress: &'a mut dyn FnMut(ConversionProgress) -> bool,
    pub(super) source_point_count: u64,
    /// The output version stores distance quantiles: leaves collect nearest-neighbour samples.
    pub(super) collect_distance_quantiles: bool,
    pub(super) nn_sample_threshold: u64,
    pub(super) parallel_workers: usize,
    /// Ranges the root partition may use; 0 scans the root as one stream.
    pub(super) root_ranges: usize,
    /// Ranges the root partition used; 0 when it did not run by ranges.
    pub(super) ranges_used: usize,
    pub(super) workers_used: usize,
    /// Test-overridable threshold of [`Self::range_split_points`].
    pub(super) range_split_points: u64,
    /// Most range jobs of split nodes held at once (see [`Self::split_range_capacity`]).
    pub(super) split_ranges_peak: usize,
    /// Estimated index of the whole plan, to keep a worker count from starving the branches.
    pub(super) expected_index_bytes: u64,
    pub(super) spool_diagnostics: Option<SpatialSpoolDiagnostics>,
    pub(super) spatial_phases: Option<SpatialPhaseDiagnostics>,
    /// Bytes this builder (not its siblings) has written, for per-root-child accounting.
    pub(super) spool_written_local: u64,
    /// Node events with this builder's local node ids; `append_branch` rebases a worker's events.
    pub(super) node_events: Vec<NodeSpoolEvent>,
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

    /// Threads the executor runs the current batch on: the requested count lowered by the budget.
    pub fn batch_workers(&self) -> usize {
        self.workers_used
    }

    /// Reserves `count` consecutive spool ids (the segments of a range-split node job).
    pub fn reserve_spool_ids(&self, count: u64) -> SpoolId {
        SpoolId(self.spool_ids.fetch_add(count, Ordering::Relaxed))
    }

    /// Points from which a node job is scanned by range jobs.
    pub fn range_split_points(&self) -> u64 {
        self.range_split_points
    }

    /// Range jobs the batch may hold at once (queued, running or waiting for their merge): what
    /// the memory budget leaves after the batch's own reservations, in units of
    /// [`range_memory_bytes`]. A node whose split does not fit runs unsplit.
    pub fn split_range_capacity(&self) -> usize {
        split_range_capacity(
            self.memory_budget,
            self.point_limit,
            self.expected_index_bytes,
            self.workers_used,
            self.spatial_divisions,
            self.sample_grid,
        )
        .map_or(0, |ranges| usize::try_from(ranges).unwrap_or(usize::MAX))
    }

    /// Records the most range jobs the batch held at once, for the peak memory estimate.
    pub fn note_split_ranges(&mut self, outstanding: usize) {
        self.split_ranges_peak = self.split_ranges_peak.max(outstanding);
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
            collect_distance_quantiles: self.collect_distance_quantiles,
            nn_sample_threshold: self.nn_sample_threshold,
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
        let first_output = if self.job_context().splits(point_count, bounds) {
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
            segments: Vec::new(),
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

    pub(super) fn build_root_from_source<F: SourceFactory>(
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
        if !ctx.splits(source.point_count, bounds) {
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
                nn_samples: Vec::new(),
                id: root_id,
                parent: None,
                level: 0,
                bounds,
                point_count: sample_count as u64,
                geometric_error: lod_sample_error(bounds, self.sample_grid)
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
                nn_samples: output.nn_samples,
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
        // The budget lowers the worker count (down to the serial plan) instead of failing.
        let batch = (level == 0 && self.parallel_workers > 1 && !children.is_empty())
            .then(|| {
                memory_limited_batch(
                    self.memory_budget,
                    self.point_limit,
                    self.node_records.len(),
                    self.output_blocks.len(),
                    self.expected_index_bytes,
                    self.parallel_workers,
                    self.spatial_divisions,
                    self.sample_grid,
                )
            })
            .flatten();
        if let Some((workers, branch_budget)) = batch {
            self.build_root_children_parallel(parent_id, children, workers, branch_budget)?;
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

    /// Plans every root child in one batch: the ready queue runs node jobs of all branches on
    /// `workers` threads, with `branch_budget` for each branch.
    fn build_root_children_parallel(
        &mut self,
        parent_id: NodeId,
        tasks: Vec<ChildOutput>,
        workers: usize,
        branch_budget: u64,
    ) -> Result<(), ConvertError> {
        self.workers_used = self.workers_used.max(workers);
        let executor = self.executor;
        let outputs = executor.plan_batch(self, tasks, branch_budget)?;
        for (_, mut branch) in outputs {
            self.append_branch(parent_id, &mut branch)?;
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
            collect_distance_quantiles: self.collect_distance_quantiles,
            nn_sample_threshold: self.nn_sample_threshold,
            parallel_workers: 1,
            root_ranges: 0,
            ranges_used: 0,
            workers_used: 1,
            range_split_points: self.range_split_points,
            split_ranges_peak: 0,
            expected_index_bytes: 0,
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
