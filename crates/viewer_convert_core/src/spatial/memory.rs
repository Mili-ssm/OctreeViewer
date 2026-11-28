//! Memory estimates and budgets of the spatial phase.

use super::spool::CHILD_SPOOL_BUFFER_BYTES;
use crate::ConvertError;

const PLANNED_NODE_BYTES: u64 = 256;
pub(super) const SPATIAL_WORKER_OVERHEAD_BYTES: u64 = 1024 * 1024;

/// The node and block share of [`estimate_memory`], saturating.
pub(super) fn index_bytes(node_count: usize, block_count: usize) -> u64 {
    (node_count as u64)
        .saturating_mul(PLANNED_NODE_BYTES)
        .saturating_add(
            (block_count as u64)
                .saturating_mul(crate::options::ESTIMATED_DIRECTORY_BYTES_PER_BLOCK),
        )
}

pub(super) fn estimate_memory(
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
pub(super) fn range_memory_bytes(
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

pub(super) fn child_spool_buffer_bytes(spatial_divisions: u8) -> Result<u64, ConvertError> {
    u64::from(spatial_divisions)
        .checked_pow(3)
        .and_then(|children| children.checked_mul(CHILD_SPOOL_BUFFER_BYTES as u64))
        .ok_or(ConvertError::SizeOverflow)
}

/// What `threads` workers reserve beyond the first: each one's overhead, and the child spool
/// buffers of every worker but the first (whose buffers are part of [`estimate_memory`]'s base).
fn thread_reservations(threads: u64, spatial_divisions: u8) -> Result<u64, ConvertError> {
    threads
        .checked_mul(SPATIAL_WORKER_OVERHEAD_BYTES)
        .zip((threads - 1).checked_mul(child_spool_buffer_bytes(spatial_divisions)?))
        .and_then(|(overhead, buffers)| overhead.checked_add(buffers))
        .ok_or(ConvertError::SizeOverflow)
}

/// Budget of the branches of one batch, which coexist until appended while only `threads` jobs run
/// at once: the per-thread overhead and extra spool buffers scale with threads, and the index
/// share (half of what is left, the other half being the copy made when the branches are
/// appended) is one pool for the whole batch. Each commit is checked against this budget less the
/// index the other branches already hold, so a branch holding most nodes does not fail where one
/// thread would not.
pub(super) fn branch_memory_budget(
    total_budget: u64,
    point_limit: u64,
    completed_nodes: usize,
    completed_blocks: usize,
    threads: usize,
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
    let threads = u64::try_from(threads.max(1)).map_err(|_| ConvertError::SizeOverflow)?;
    let reserved = base
        .checked_add(
            current_index
                .checked_mul(2)
                .ok_or(ConvertError::SizeOverflow)?,
        )
        .and_then(|bytes| bytes.checked_add(thread_reservations(threads, spatial_divisions).ok()?))
        .ok_or(ConvertError::SizeOverflow)?;
    let available =
        total_budget
            .checked_sub(reserved)
            .ok_or(ConvertError::MemoryBudgetExceeded {
                requested: reserved,
                limit: total_budget,
            })?;
    let concurrent_index_share = available / 2;
    base.checked_add(concurrent_index_share)
        .ok_or(ConvertError::SizeOverflow)
}

/// Peak memory estimate of a plan that ran on `threads` workers and holds `node_count` nodes and
/// `block_count` blocks: the same reservations as [`branch_memory_budget`] admits (thread
/// overhead, extra spool buffers, and the index twice for the append copy). One thread has no
/// copy and keeps [`estimate_memory`].
pub(super) fn peak_memory_bytes(
    point_limit: u64,
    node_count: usize,
    block_count: usize,
    threads: usize,
    spatial_divisions: u8,
    sample_grid: usize,
) -> Result<u64, ConvertError> {
    let single = estimate_memory(
        point_limit,
        node_count,
        block_count,
        spatial_divisions,
        sample_grid,
    )?;
    if threads <= 1 {
        return Ok(single);
    }
    let index = index_bytes(node_count, block_count);
    let base = single.saturating_sub(index);
    let threads = u64::try_from(threads).map_err(|_| ConvertError::SizeOverflow)?;
    index
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(base))
        .and_then(|bytes| bytes.checked_add(thread_reservations(threads, spatial_divisions).ok()?))
        .ok_or(ConvertError::SizeOverflow)
}

/// Index room a batch must leave to be worth running on several threads: the nodes and blocks of
/// a tree with `ceil(point_count / leaf_point_limit)` leaves, counting twice as many nodes as
/// leaves (an octree has fewer inner nodes than leaves). Real trees stay well under it, so a
/// large cloud is not run serially for an index it will never have; the guarantee that a higher
/// thread count never fails where one thread passes is the serial fallback in `plan.rs`.
pub(super) fn expected_index_bytes(point_count: u64, leaf_point_limit: u64) -> u64 {
    let leaves = point_count.div_ceil(leaf_point_limit.max(1));
    index_bytes(
        usize::try_from(leaves.saturating_mul(2)).unwrap_or(usize::MAX),
        usize::try_from(leaves).unwrap_or(usize::MAX),
    )
}

/// The most threads, from 2 up to `requested`, that the budget holds for a batch, with the branch
/// budget for that count. A count qualifies when, after its fixed reservations, the index share
/// still has `expected_index` of room. `None` when not even 2 qualify, and the plan runs
/// serially as with one worker.
#[expect(
    clippy::too_many_arguments,
    reason = "distinct budget inputs; a struct would be single-use"
)]
pub(super) fn memory_limited_batch(
    total_budget: u64,
    point_limit: u64,
    completed_nodes: usize,
    completed_blocks: usize,
    expected_index: u64,
    requested: usize,
    spatial_divisions: u8,
    sample_grid: usize,
) -> Option<(usize, u64)> {
    let base = estimate_memory(point_limit, 0, 0, spatial_divisions, sample_grid).ok()?;
    (2..=requested).rev().find_map(|threads| {
        let budget = branch_memory_budget(
            total_budget,
            point_limit,
            completed_nodes,
            completed_blocks,
            threads,
            spatial_divisions,
            sample_grid,
        )
        .ok()?;
        (budget - base >= expected_index).then_some((threads, budget))
    })
}

/// Range jobs of split nodes a `threads`-worker batch may hold at once. One rule for admission
/// and the peak estimate: each such job counts [`range_memory_bytes`] (its sample accumulators
/// wait for the merge, and it is a root range's working set while it runs), on top of what the
/// batch's own admission reserves, i.e. [`memory_limited_batch`]'s base, twice the expected index
/// and the thread reservations. `Err` only on overflow.
pub(super) fn split_range_capacity(
    total_budget: u64,
    point_limit: u64,
    expected_index: u64,
    threads: usize,
    spatial_divisions: u8,
    sample_grid: usize,
) -> Result<u64, ConvertError> {
    let reserved = peak_memory_bytes_for_index(
        point_limit,
        expected_index,
        threads,
        spatial_divisions,
        sample_grid,
    )?;
    Ok(total_budget.saturating_sub(reserved)
        / range_memory_bytes(point_limit, spatial_divisions, sample_grid)?)
}

/// What [`memory_limited_batch`] reserves for `threads` workers and an index of `index` bytes.
fn peak_memory_bytes_for_index(
    point_limit: u64,
    index: u64,
    threads: usize,
    spatial_divisions: u8,
    sample_grid: usize,
) -> Result<u64, ConvertError> {
    let base = estimate_memory(point_limit, 0, 0, spatial_divisions, sample_grid)?;
    let threads = u64::try_from(threads.max(1)).map_err(|_| ConvertError::SizeOverflow)?;
    index
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(base))
        .and_then(|bytes| bytes.checked_add(thread_reservations(threads, spatial_divisions).ok()?))
        .ok_or(ConvertError::SizeOverflow)
}

/// Peak estimate of a plan whose batch held up to `split_ranges` range jobs of split nodes.
pub(super) fn split_peak_memory_bytes(
    point_limit: u64,
    node_count: usize,
    block_count: usize,
    threads: usize,
    split_ranges: usize,
    spatial_divisions: u8,
    sample_grid: usize,
) -> Result<u64, ConvertError> {
    peak_memory_bytes_for_index(
        point_limit,
        index_bytes(node_count, block_count),
        threads,
        spatial_divisions,
        sample_grid,
    )?
    .checked_add(
        (split_ranges as u64)
            .checked_mul(range_memory_bytes(
                point_limit,
                spatial_divisions,
                sample_grid,
            )?)
            .ok_or(ConvertError::SizeOverflow)?,
    )
    .ok_or(ConvertError::SizeOverflow)
}

pub(super) fn check_memory_budget(requested: u64, limit: u64) -> Result<(), ConvertError> {
    if requested > limit {
        Err(ConvertError::MemoryBudgetExceeded { requested, limit })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{DEFAULT_MEMORY_BUDGET_BYTES, LEAF_POINT_LIMIT, SAMPLE_GRID};

    const DIVISIONS: u8 = 2;

    #[test]
    fn a_huge_cloud_still_gets_the_requested_workers_at_the_default_budget() {
        // 1.5 G points: about 23 k leaves, an index of tens of MB against a ~244 MB share.
        let expected = expected_index_bytes(1_500_000_000, LEAF_POINT_LIMIT);
        assert!(expected < 64 * 1024 * 1024, "{expected}");
        let (workers, _) = memory_limited_batch(
            DEFAULT_MEMORY_BUDGET_BYTES,
            LEAF_POINT_LIMIT,
            1,
            0,
            expected,
            4,
            DIVISIONS,
            SAMPLE_GRID,
        )
        .expect("a batch");
        assert_eq!(workers, 4);
    }

    #[test]
    fn split_range_capacity_is_what_the_batch_reservations_leave() {
        let per_range = range_memory_bytes(LEAF_POINT_LIMIT, DIVISIONS, SAMPLE_GRID).unwrap();
        let reserved =
            peak_memory_bytes_for_index(LEAF_POINT_LIMIT, 1_000, 4, DIVISIONS, SAMPLE_GRID)
                .unwrap();
        let capacity = |budget| {
            split_range_capacity(budget, LEAF_POINT_LIMIT, 1_000, 4, DIVISIONS, SAMPLE_GRID)
                .unwrap()
        };
        assert_eq!(capacity(reserved), 0);
        assert_eq!(capacity(reserved + per_range - 1), 0);
        assert_eq!(capacity(reserved + 3 * per_range), 3);
        assert_eq!(capacity(0), 0);
        // The peak of a plan that held `n` ranges is the reservations plus `n` ranges.
        let peak =
            split_peak_memory_bytes(LEAF_POINT_LIMIT, 4, 0, 4, 3, DIVISIONS, SAMPLE_GRID).unwrap();
        let index = index_bytes(4, 0);
        let base = peak_memory_bytes_for_index(LEAF_POINT_LIMIT, index, 4, DIVISIONS, SAMPLE_GRID)
            .unwrap();
        assert_eq!(peak, base + 3 * per_range);
    }

    #[test]
    fn the_peak_estimate_uses_the_reservations_the_admission_rule_holds() {
        let (nodes, blocks) = (1000, 700);
        for threads in [2, 4, 8] {
            let peak = peak_memory_bytes(
                LEAF_POINT_LIMIT,
                nodes,
                blocks,
                threads,
                DIVISIONS,
                SAMPLE_GRID,
            )
            .unwrap();
            // A budget equal to the peak leaves exactly this index as the batch's share.
            let budget = branch_memory_budget(
                peak,
                LEAF_POINT_LIMIT,
                0,
                0,
                threads,
                DIVISIONS,
                SAMPLE_GRID,
            )
            .unwrap();
            let base = estimate_memory(LEAF_POINT_LIMIT, 0, 0, DIVISIONS, SAMPLE_GRID).unwrap();
            assert_eq!(
                budget - base,
                index_bytes(nodes, blocks),
                "{threads} threads"
            );
        }
        assert_eq!(
            peak_memory_bytes(LEAF_POINT_LIMIT, nodes, blocks, 1, DIVISIONS, SAMPLE_GRID).unwrap(),
            estimate_memory(LEAF_POINT_LIMIT, nodes, blocks, DIVISIONS, SAMPLE_GRID).unwrap()
        );
    }
}
