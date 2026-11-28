//! How a batch of root children is planned: the native ready queue (`ThreadedExecutor`, in the
//! shell) or in order on the calling thread ([`SerialExecutor`]), which is also the oracle of the
//! queue.

use super::node_job::{
    ChildOutput, JobProgress, NodePath, RangeJob, RangePartition, run_root_partition_range,
};
use super::{BranchOutput, SpatialBuilder};
use crate::ConvertError;
use crate::pipeline::{
    clock::Clock,
    point_source::{PointRange, SourceFactory},
    spool_store::{PlanStore, SpoolId},
};

pub trait BatchPlanner<S: PlanStore, C: Clock> {
    /// Plans the subtrees of one batch of root children and returns their branches in key order,
    /// ready for `append_branch`. `memory_budget` is the per-batch budget of each branch.
    fn plan_batch(
        &self,
        builder: &mut SpatialBuilder<'_, S, C, Self>,
        tasks: Vec<ChildOutput>,
        memory_budget: u64,
    ) -> Result<Vec<(u16, BranchOutput)>, ConvertError>
    where
        Self: Sized;
}

/// Runs everything in order on the calling thread.
pub struct SerialExecutor;

impl<S: PlanStore, C: Clock> BatchPlanner<S, C> for SerialExecutor {
    /// Same result as the ready queue: one branch-local builder per root child with the batch
    /// budget, so `TooManyBlocks` is local and the first error in key order (the lowest path)
    /// wins.
    fn plan_batch(
        &self,
        builder: &mut SpatialBuilder<'_, S, C, Self>,
        tasks: Vec<ChildOutput>,
        memory_budget: u64,
    ) -> Result<Vec<(u16, BranchOutput)>, ConvertError> {
        let collect = builder.collects_diagnostics();
        // Ids are reserved up front for every task, as the queue does.
        let jobs = tasks
            .iter()
            .map(|task| {
                builder.node_job(
                    NodePath(vec![task.key]),
                    1,
                    task.bounds,
                    task.spool,
                    task.point_count,
                    true,
                )
            })
            .collect::<Vec<_>>();
        let mut branches = Vec::with_capacity(tasks.len());
        for (task, job) in tasks.iter().zip(jobs) {
            let mut branch = BranchOutput::new(collect);
            builder
                .with_branch(&mut branch, memory_budget, |branch_builder| {
                    branch_builder.build_node(job, None)
                })
                .map(|_| ())?;
            branches.push((task.key, branch));
        }
        Ok(branches)
    }
}

/// Runs the ranges of one root partition.
pub trait RangeRunner<S: PlanStore, C: Clock, F: SourceFactory> {
    /// One result per range, in range order; each range opens its own source (on its own thread
    /// when the runner has threads). An error in range `k`, an open error included, cancels the
    /// ranges above it (their result is `Cancelled`) and lets the ones below finish; a
    /// cancellation reported by `progress` stops them all. `progress` runs on the calling thread.
    fn run_ranges(
        &self,
        job: &RangeJob<'_>,
        factory: &F,
        ranges: Vec<(PointRange, SpoolId)>,
        store: &S,
        clock: &C,
        progress: &mut dyn FnMut(usize, JobProgress) -> Result<(), ConvertError>,
    ) -> Vec<Result<RangePartition, ConvertError>>;
}

impl<S: PlanStore, C: Clock, F: SourceFactory> RangeRunner<S, C, F> for SerialExecutor {
    /// In range order; once a range fails, the rest are not started.
    fn run_ranges(
        &self,
        job: &RangeJob<'_>,
        factory: &F,
        ranges: Vec<(PointRange, SpoolId)>,
        store: &S,
        clock: &C,
        progress: &mut dyn FnMut(usize, JobProgress) -> Result<(), ConvertError>,
    ) -> Vec<Result<RangePartition, ConvertError>> {
        let mut failed = false;
        ranges
            .into_iter()
            .enumerate()
            .map(|(range, (points, first_segment))| {
                if failed {
                    return Err(ConvertError::Cancelled {
                        points_processed: 0,
                    });
                }
                let result = factory.open(points).and_then(|mut source| {
                    run_root_partition_range(
                        job,
                        first_segment,
                        &mut source,
                        store,
                        clock,
                        &mut |update| progress(range, update),
                    )
                });
                failed = result.is_err();
                result
            })
            .collect()
    }
}
