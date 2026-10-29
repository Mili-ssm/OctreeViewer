use viewer_format::FormatVersion;

use crate::error::ConvertError;

pub const DEFAULT_POINTS_PER_BLOCK: u64 = 262_144;
pub const DEFAULT_MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_POINTS_PER_BLOCK: u64 = 1_000_000;
pub const MAX_SPATIAL_WORKERS: usize = 8;
pub const DEFAULT_SPATIAL_WORKERS: usize = 4;
pub const MAX_ROOT_PARTITION_RANGES: usize = 8;
pub const ALLOWED_SPATIAL_DIVISIONS: [u8; 3] = [2, 4, 8];
pub const MAX_INPUT_BYTES_PER_BATCH: usize = 8 * 1024 * 1024;
pub const MAX_BLOCKS: u64 = 1_000_000;
pub const ESTIMATED_WORKING_BYTES_PER_POINT: u64 = 192;
pub const ESTIMATED_DIRECTORY_BYTES_PER_BLOCK: u64 = 512;
pub const ESTIMATED_FIXED_OVERHEAD_BYTES: u64 = 16 * 1024 * 1024;

#[expect(
    clippy::disallowed_methods,
    reason = "native default; Err on wasm32 → 1"
)]
pub fn default_spatial_workers() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |workers| workers.get().min(DEFAULT_SPATIAL_WORKERS))
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
    pub(crate) root_partition_ranges: usize,
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
            root_partition_ranges: 0,
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

    pub const fn spatial_workers_explicit(self) -> bool {
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

    /// Reads the root in up to `ranges` parts (LAS record ranges, LAZ chunk groups) and joins
    /// the per-range child spools; 0 (the default) scans it as one stream. Not a stable option.
    #[doc(hidden)]
    pub fn with_root_partition_ranges(mut self, ranges: usize) -> Result<Self, ConvertError> {
        if ranges > MAX_ROOT_PARTITION_RANGES {
            return Err(ConvertError::InvalidRootRanges(ranges));
        }
        self.root_partition_ranges = ranges;
        Ok(self)
    }

    #[doc(hidden)]
    pub const fn root_partition_ranges(self) -> usize {
        self.root_partition_ranges
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
            root_partition_ranges: 0,
            format_version: FormatVersion::DEFAULT_OUTPUT,
        }
    }
}

/// Applies the same input staging limit to planning, spooling and EPT reads.
pub fn input_batch_point_limit(points_per_block: u64, record_length: usize) -> u64 {
    points_per_block.min((MAX_INPUT_BYTES_PER_BATCH / record_length.max(1)).max(1) as u64)
}
