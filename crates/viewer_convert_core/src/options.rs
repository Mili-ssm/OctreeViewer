use viewer_format::FormatVersion;

use crate::error::ConvertError;

pub const DEFAULT_MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
/// Sanity bound of the core; hosts cap lower (the CLI at the available parallelism, the browser at 8).
pub const MAX_SPATIAL_WORKERS: usize = 256;
pub const DEFAULT_SPATIAL_WORKERS: usize = 4;
pub const MAX_ROOT_PARTITION_RANGES: usize = 8;
/// A node job of at least this many points is scanned by range jobs when it has several workers.
pub const RANGE_SPLIT_POINTS: u64 = 1 << 20;
/// Default root scan split; `admitted_root_ranges` still bounds it by the memory budget.
const DEFAULT_ROOT_PARTITION_RANGES: usize = 4;
/// Leaf point limit fixed by V8 (owner resolution 02-11-2026, ADR 0051).
pub const LEAF_POINT_LIMIT: u64 = 64_000;
/// LOD representatives per axis of an inner node, fixed by V8.
pub const SAMPLE_GRID: usize = 256;
/// Children per axis of a node split, fixed by V8.
pub const SPATIAL_DIVISIONS: u8 = 2;
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

/// Threads this host offers (1 where it cannot tell, as on wasm32); the CLI's cap for
/// `--spatial-workers`.
#[expect(clippy::disallowed_methods, reason = "native cap; Err on wasm32 → 1")]
pub fn available_spatial_workers() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

/// Conversion options. V8 is the only output: its format, leaf limit, sample grid, spatial
/// divisions and hybrid row order are constants, so only the settings below vary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConvertOptions {
    pub(crate) max_quantization_error: f64,
    pub(crate) max_memory_bytes: u64,
    pub(crate) spatial_workers: usize,
    pub(crate) spatial_workers_explicit: bool,
    pub(crate) pipelined_writes: bool,
    pub(crate) root_partition_ranges: usize,
    /// Page size target; `None` keeps the format default.
    pub(crate) page_target_bytes: Option<usize>,
    /// Test-only override of the leaf limit; `None` keeps the V8 value.
    pub(crate) leaf_point_limit: Option<u64>,
    /// Test-only override of [`RANGE_SPLIT_POINTS`]; `None` keeps the production value.
    pub(crate) range_split_points: Option<u64>,
}

impl ConvertOptions {
    /// For tests and benchmarks: caps pages at about `bytes`, so small files still need several
    /// pages. Production output keeps the default.
    #[doc(hidden)]
    #[must_use]
    pub const fn with_page_target_bytes(mut self, bytes: usize) -> Self {
        self.page_target_bytes = Some(bytes);
        self
    }

    /// For tests and benchmarks: lowers the leaf limit (at least 1), so small inputs still build
    /// multi-level trees. Production output (CLI, web) always uses [`LEAF_POINT_LIMIT`].
    #[doc(hidden)]
    #[must_use]
    pub const fn with_leaf_point_limit_for_tests(mut self, limit: u64) -> Self {
        self.leaf_point_limit = Some(if limit == 0 { 1 } else { limit });
        self
    }

    /// For tests: lowers the point count from which a node job is range-split (at least 1), so
    /// small inputs still exercise the split. Production output always uses [`RANGE_SPLIT_POINTS`].
    #[doc(hidden)]
    #[must_use]
    pub const fn with_range_split_points_for_tests(mut self, points: u64) -> Self {
        self.range_split_points = Some(if points == 0 { 1 } else { points });
        self
    }

    pub const fn range_split_points(self) -> u64 {
        match self.range_split_points {
            Some(points) => points,
            None => RANGE_SPLIT_POINTS,
        }
    }

    pub const fn format_version(self) -> FormatVersion {
        FormatVersion::DEFAULT_OUTPUT
    }

    pub fn new(max_quantization_error: f64) -> Result<Self, ConvertError> {
        Self::with_memory_budget(max_quantization_error, DEFAULT_MEMORY_BUDGET_BYTES)
    }

    pub fn with_memory_budget(
        max_quantization_error: f64,
        max_memory_bytes: u64,
    ) -> Result<Self, ConvertError> {
        if !max_quantization_error.is_finite() || max_quantization_error < 0.0 {
            return Err(ConvertError::InvalidQuantizationError(
                max_quantization_error,
            ));
        }
        if max_memory_bytes == 0 {
            return Err(ConvertError::InvalidMemoryBudget);
        }
        Ok(Self {
            max_quantization_error,
            max_memory_bytes,
            spatial_workers: default_spatial_workers(),
            spatial_workers_explicit: false,
            pipelined_writes: false,
            root_partition_ranges: DEFAULT_ROOT_PARTITION_RANGES,
            page_target_bytes: None,
            leaf_point_limit: None,
            range_split_points: None,
        })
    }

    pub const fn points_per_block(self) -> u64 {
        match self.leaf_point_limit {
            Some(limit) => limit,
            None => LEAF_POINT_LIMIT,
        }
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
    #[must_use]
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

    pub const fn spatial_divisions(self) -> u8 {
        SPATIAL_DIVISIONS
    }

    #[doc(hidden)]
    pub const fn sample_grid(self) -> usize {
        SAMPLE_GRID
    }

    /// Reads the root in up to `ranges` parts (LAS record ranges, LAZ chunk groups) and joins
    /// the per-range child spools; 0 scans it as one stream (the default is 4). Not a stable option.
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
            max_quantization_error: 0.01,
            max_memory_bytes: DEFAULT_MEMORY_BUDGET_BYTES,
            spatial_workers: default_spatial_workers(),
            spatial_workers_explicit: false,
            pipelined_writes: false,
            root_partition_ranges: DEFAULT_ROOT_PARTITION_RANGES,
            page_target_bytes: None,
            leaf_point_limit: None,
            range_split_points: None,
        }
    }
}

/// Applies the same input staging limit to planning, and spooling reads.
pub fn input_batch_point_limit(points_per_block: u64, record_length: usize) -> u64 {
    points_per_block.min((MAX_INPUT_BYTES_PER_BATCH / record_length.max(1)).max(1) as u64)
}

#[cfg(test)]
mod tests {

    use super::{ConvertOptions, LEAF_POINT_LIMIT, SAMPLE_GRID, SPATIAL_DIVISIONS};
    use crate::ConvertError;

    #[test]
    fn options_resolve_to_the_values_v8_fixes() {
        let options = ConvertOptions::default();
        assert_eq!(
            options.format_version(),
            viewer_format::FormatVersion::DEFAULT_OUTPUT
        );
        assert_eq!(options.points_per_block(), LEAF_POINT_LIMIT);
        assert_eq!(options.points_per_block(), 64_000);
        assert_eq!(options.sample_grid(), SAMPLE_GRID);
        assert_eq!(options.spatial_divisions(), SPATIAL_DIVISIONS);
    }

    #[test]
    fn creation_checks_ranges() {
        assert!(matches!(
            ConvertOptions::new(f64::NAN),
            Err(ConvertError::InvalidQuantizationError(_))
        ));
        assert!(matches!(
            ConvertOptions::with_memory_budget(0.01, 0),
            Err(ConvertError::InvalidMemoryBudget)
        ));
        assert!(matches!(
            ConvertOptions::default().with_spatial_workers(0),
            Err(ConvertError::InvalidSpatialWorkerCount(0))
        ));
        assert!(matches!(
            ConvertOptions::default().with_root_partition_ranges(9),
            Err(ConvertError::InvalidRootRanges(9))
        ));
    }
}
