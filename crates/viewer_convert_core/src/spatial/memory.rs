//! Memory estimates and budgets of the spatial phase.

use super::spool::CHILD_SPOOL_BUFFER_BYTES;
use crate::ConvertError;

const PLANNED_NODE_BYTES: u64 = 256;
pub(super) const SPATIAL_WORKER_OVERHEAD_BYTES: u64 = 1024 * 1024;

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

pub(super) fn branch_memory_budget(
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

pub(super) fn check_memory_budget(requested: u64, limit: u64) -> Result<(), ConvertError> {
    if requested > limit {
        Err(ConvertError::MemoryBudgetExceeded { requested, limit })
    } else {
        Ok(())
    }
}
