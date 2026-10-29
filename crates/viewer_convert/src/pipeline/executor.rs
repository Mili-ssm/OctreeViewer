//! How a batch of root children is planned: the native ready queue (`ThreadedExecutor`, in the
//! shell) or in order on the calling thread ([`SerialExecutor`]), which is also the oracle of the
//! queue.

use crate::ConvertError;
use crate::pipeline::{
    clock::Clock,
    node_job::{ChildOutput, NodePath},
    spool_store::PlanStore,
};
use crate::spatial::{BranchOutput, SpatialBuilder};

pub(crate) trait BatchPlanner<S: PlanStore, C: Clock> {
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
#[cfg_attr(not(test), allow(dead_code, reason = "used by the serial host loop"))]
pub(crate) struct SerialExecutor;

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
