use viewer_format::FormatVersion;

use crate::error::ConvertError;

pub(crate) const DEFAULT_POINTS_PER_BLOCK: u64 = 262_144;
pub(crate) const DEFAULT_MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const MAX_POINTS_PER_BLOCK: u64 = 1_000_000;
pub(crate) const MAX_SPATIAL_WORKERS: usize = 8;
pub(crate) const DEFAULT_SPATIAL_WORKERS: usize = 4;
pub(crate) const ALLOWED_SPATIAL_DIVISIONS: [u8; 3] = [2, 4, 8];
pub(crate) const MAX_INPUT_BYTES_PER_BATCH: usize = 8 * 1024 * 1024;
pub(crate) const MAX_BLOCKS: u64 = 1_000_000;
pub(crate) const ESTIMATED_WORKING_BYTES_PER_POINT: u64 = 192;
pub(crate) const ESTIMATED_DIRECTORY_BYTES_PER_BLOCK: u64 = 512;
pub(crate) const ESTIMATED_FIXED_OVERHEAD_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn default_spatial_workers() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |workers| workers.get().min(DEFAULT_SPATIAL_WORKERS))
}

/// How the LAS/LAZ spatial planner runs the node jobs under the root children. Both give the
/// same output bytes; only scheduling and timing differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpatialScheduler {
    /// Each root child of a batch is one thread that plans its subtree serially (default).
    #[default]
    Recursive,
    /// Up to `min(workers, root children)` threads take the ready node job with the lowest
    /// preorder path under the batch being committed (experimental until it is measured).
    ReadyQueue,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConvertOptions {
    pub(crate) points_per_block: u64,
    pub(crate) max_quantization_error: f64,
    pub(crate) max_memory_bytes: u64,
    pub(crate) spatial_workers: usize,
    pub(crate) spatial_workers_explicit: bool,
    pub(crate) pipelined_writes: bool,
    pub(crate) spatial_divisions: u8,
    pub(crate) spatial_scheduler: SpatialScheduler,
    pub(crate) format_version: FormatVersion,
}

impl ConvertOptions {
    pub fn with_format_version(mut self, version: FormatVersion) -> Result<Self, ConvertError> {
        version.ensure_writable()?;
        self.format_version = version;
        Ok(self)
    }

    pub const fn format_version(self) -> FormatVersion {
        self.format_version
    }

    pub fn new(points_per_block: u64, max_quantization_error: f64) -> Result<Self, ConvertError> {
        Self::with_memory_budget(
            points_per_block,
            max_quantization_error,
            DEFAULT_MEMORY_BUDGET_BYTES,
        )
    }

    pub fn with_memory_budget(
        points_per_block: u64,
        max_quantization_error: f64,
        max_memory_bytes: u64,
    ) -> Result<Self, ConvertError> {
        if points_per_block == 0 || points_per_block > MAX_POINTS_PER_BLOCK {
            return Err(ConvertError::InvalidBlockPointLimit(points_per_block));
        }
        if !max_quantization_error.is_finite() || max_quantization_error < 0.0 {
            return Err(ConvertError::InvalidQuantizationError(
                max_quantization_error,
            ));
        }
        if max_memory_bytes == 0 {
            return Err(ConvertError::InvalidMemoryBudget);
        }
        Ok(Self {
            points_per_block,
            max_quantization_error,
            max_memory_bytes,
            spatial_workers: default_spatial_workers(),
            spatial_workers_explicit: false,
            pipelined_writes: false,
            spatial_divisions: 2,
            spatial_scheduler: SpatialScheduler::Recursive,
            format_version: FormatVersion::DEFAULT_OUTPUT,
        })
    }

    pub const fn points_per_block(self) -> u64 {
        self.points_per_block
    }

    pub const fn max_quantization_error(self) -> f64 {
        self.max_quantization_error
    }

    pub const fn max_memory_budget_bytes(self) -> u64 {
        self.max_memory_bytes
    }

    pub fn with_spatial_workers(mut self, workers: usize) -> Result<Self, ConvertError> {
        if !(1..=MAX_SPATIAL_WORKERS).contains(&workers) {
            return Err(ConvertError::InvalidSpatialWorkerCount(workers));
        }
        self.spatial_workers = workers;
        self.spatial_workers_explicit = true;
        Ok(self)
    }

    /// Overlap ordered output with preparation of one following batch.
    /// The planner reserves two batches; this is an estimate, not an RSS limit.
    pub fn with_pipelined_writes(mut self, enabled: bool) -> Self {
        self.pipelined_writes = enabled;
        self
    }

    pub const fn pipelined_writes(self) -> bool {
        self.pipelined_writes
    }

    pub const fn spatial_workers(self) -> usize {
        self.spatial_workers
    }

    pub(crate) const fn spatial_workers_explicit(self) -> bool {
        self.spatial_workers_explicit
    }

    pub fn with_spatial_divisions(mut self, divisions: u8) -> Result<Self, ConvertError> {
        if !ALLOWED_SPATIAL_DIVISIONS.contains(&divisions) {
            return Err(ConvertError::InvalidSpatialDivisions(divisions));
        }
        self.spatial_divisions = divisions;
        Ok(self)
    }

    pub const fn spatial_divisions(self) -> u8 {
        self.spatial_divisions
    }

    /// Selects the spatial planner scheduler; the output bytes do not depend on it.
    pub const fn with_spatial_scheduler(mut self, scheduler: SpatialScheduler) -> Self {
        self.spatial_scheduler = scheduler;
        self
    }

    pub const fn spatial_scheduler(self) -> SpatialScheduler {
        self.spatial_scheduler
    }
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            points_per_block: DEFAULT_POINTS_PER_BLOCK,
            max_quantization_error: 0.01,
            max_memory_bytes: DEFAULT_MEMORY_BUDGET_BYTES,
            spatial_workers: default_spatial_workers(),
            spatial_workers_explicit: false,
            pipelined_writes: false,
            spatial_divisions: 2,
            spatial_scheduler: SpatialScheduler::Recursive,
            format_version: FormatVersion::DEFAULT_OUTPUT,
        }
    }
}

/// Applies the same input staging limit to planning, spooling and EPT reads.
pub(crate) fn input_batch_point_limit(points_per_block: u64, record_length: usize) -> u64 {
    points_per_block.min((MAX_INPUT_BYTES_PER_BATCH / record_length.max(1)).max(1) as u64)
}
