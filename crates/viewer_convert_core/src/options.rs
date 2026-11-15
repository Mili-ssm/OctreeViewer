use viewer_format::{FormatVersion, RowOrder};

use crate::error::ConvertError;

pub const DEFAULT_POINTS_PER_BLOCK: u64 = 262_144;
pub const DEFAULT_MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_POINTS_PER_BLOCK: u64 = 1_000_000;
pub const MAX_SPATIAL_WORKERS: usize = 8;
pub const DEFAULT_SPATIAL_WORKERS: usize = 4;
pub const MAX_ROOT_PARTITION_RANGES: usize = 8;
/// Default root scan split; `admitted_root_ranges` still bounds it by the memory budget.
const DEFAULT_ROOT_PARTITION_RANGES: usize = 4;
pub const DEFAULT_SAMPLE_GRID: usize = 8;
pub const MAX_SAMPLE_GRID: usize = 256;
/// Leaf point limit and sample grid fixed by `v7.4-pre` (R4 task 1, owner resolution 02-11-2026).
pub const GLOBAL_LEAF_POINT_LIMIT: u64 = 64_000;
pub const GLOBAL_SAMPLE_GRID: usize = 256;
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

/// Requested values stay `Option` until a getter resolves them against the format, so the order
/// of the setters does not matter; [`Self::validate`] rejects requests the format contradicts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConvertOptions {
    pub(crate) points_per_block: Option<u64>,
    pub(crate) max_quantization_error: f64,
    pub(crate) max_memory_bytes: u64,
    pub(crate) spatial_workers: usize,
    pub(crate) spatial_workers_explicit: bool,
    pub(crate) pipelined_writes: bool,
    pub(crate) spatial_divisions: Option<u8>,
    pub(crate) sample_grid: Option<usize>,
    pub(crate) root_partition_ranges: usize,
    pub(crate) format_version: Option<FormatVersion>,
    /// `v7.5-pre` page size target; `None` keeps the format default.
    pub(crate) page_target_bytes: Option<usize>,
    /// Requested row order; `None` takes the format's fixed order or `Source` (ADR 0041/0042/0044).
    pub(crate) row_order: Option<RowOrder>,
}

impl ConvertOptions {
    pub fn with_format_version(mut self, version: FormatVersion) -> Result<Self, ConvertError> {
        version.ensure_writable()?;
        self.format_version = Some(version);
        Ok(self)
    }

    /// Checks the requested values against the ones the resolved format fixes. Setters only
    /// check ranges, so each conversion entry point runs this idempotent check.
    pub fn validate(self) -> Result<Self, ConvertError> {
        let version = self.format_version();
        if self.row_order == Some(RowOrder::Hybrid) && version.major() != 7 {
            return Err(ConvertError::HybridRowOrderUnsupported);
        }
        if let (Some(fixed), Some(requested)) = (version.fixed_row_order(), self.row_order)
            && fixed != requested
        {
            return Err(ConvertError::FixedByFormat("row order"));
        }
        if version.uses_global_frame() {
            // `new` always records a leaf limit, so the unfixed default counts as not requested.
            if !self
                .points_per_block
                .is_none_or(|v| matches!(v, DEFAULT_POINTS_PER_BLOCK | GLOBAL_LEAF_POINT_LIMIT))
            {
                return Err(ConvertError::FixedByFormat("points per block (leaf limit)"));
            }
            if self.sample_grid.is_some_and(|v| v != GLOBAL_SAMPLE_GRID) {
                return Err(ConvertError::FixedByFormat("sample grid"));
            }
            if self.spatial_divisions.is_some_and(|v| v != 2) {
                return Err(ConvertError::FixedByFormat("spatial divisions"));
            }
        }
        Ok(self)
    }

    /// For tests and benchmarks: caps `v7.5-pre` pages at about `bytes`, so small files still
    /// need several pages. Production output keeps the default.
    #[doc(hidden)]
    pub const fn with_page_target_bytes(mut self, bytes: usize) -> Self {
        self.page_target_bytes = Some(bytes);
        self
    }

    /// Requests a row order. `v7` fixes `Hybrid` and `v7.N-pre` defaults to `Source`; the
    /// combination with the format is checked by [`Self::validate`].
    pub fn with_row_order(mut self, row_order: RowOrder) -> Result<Self, ConvertError> {
        self.row_order = Some(row_order);
        Ok(self)
    }

    /// Resolved row order: the requested one, else the format's fixed one, else `Source`.
    pub fn row_order(self) -> RowOrder {
        self.row_order
            .or_else(|| self.format_version().fixed_row_order())
            .unwrap_or(RowOrder::Source)
    }

    pub fn format_version(self) -> FormatVersion {
        self.format_version.unwrap_or(FormatVersion::DEFAULT_OUTPUT)
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
            points_per_block: Some(points_per_block),
            max_quantization_error,
            max_memory_bytes,
            spatial_workers: default_spatial_workers(),
            spatial_workers_explicit: false,
            pipelined_writes: false,
            spatial_divisions: None,
            sample_grid: None,
            root_partition_ranges: DEFAULT_ROOT_PARTITION_RANGES,
            format_version: None,
            page_target_bytes: None,
            row_order: None,
        })
    }

    pub fn points_per_block(self) -> u64 {
        if self.format_version().uses_global_frame() {
            GLOBAL_LEAF_POINT_LIMIT
        } else {
            self.points_per_block.unwrap_or(DEFAULT_POINTS_PER_BLOCK)
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
        self.spatial_divisions = Some(divisions);
        Ok(self)
    }

    pub fn spatial_divisions(self) -> u8 {
        if self.format_version().uses_global_frame() {
            2
        } else {
            self.spatial_divisions.unwrap_or(2)
        }
    }

    /// Internal density parameter: LOD representatives per axis of an inner node (default 8).
    /// Not a stable option; the format records it only through the sampling policy text.
    #[doc(hidden)]
    pub fn with_sample_grid(mut self, grid: usize) -> Result<Self, ConvertError> {
        if !(1..=MAX_SAMPLE_GRID).contains(&grid) {
            return Err(ConvertError::InvalidSampleGrid(grid));
        }
        self.sample_grid = Some(grid);
        Ok(self)
    }

    #[doc(hidden)]
    pub fn sample_grid(self) -> usize {
        if self.format_version().uses_global_frame() {
            GLOBAL_SAMPLE_GRID
        } else {
            self.sample_grid.unwrap_or(DEFAULT_SAMPLE_GRID)
        }
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
            points_per_block: None,
            max_quantization_error: 0.01,
            max_memory_bytes: DEFAULT_MEMORY_BUDGET_BYTES,
            spatial_workers: default_spatial_workers(),
            spatial_workers_explicit: false,
            pipelined_writes: false,
            spatial_divisions: None,
            sample_grid: None,
            root_partition_ranges: DEFAULT_ROOT_PARTITION_RANGES,
            format_version: None,
            page_target_bytes: None,
            row_order: None,
        }
    }
}

/// Applies the same input staging limit to planning, spooling and EPT reads.
pub fn input_batch_point_limit(points_per_block: u64, record_length: usize) -> u64 {
    points_per_block.min((MAX_INPUT_BYTES_PER_BATCH / record_length.max(1)).max(1) as u64)
}

#[cfg(test)]
mod tests {
    use viewer_format::{FormatVersion, RowOrder};

    use super::{ConvertOptions, GLOBAL_LEAF_POINT_LIMIT, GLOBAL_SAMPLE_GRID};
    use crate::ConvertError;

    fn v(options: ConvertOptions, version: FormatVersion) -> ConvertOptions {
        options.with_format_version(version).unwrap()
    }

    #[test]
    fn hybrid_row_order_needs_a_v7_format_in_either_call_order() {
        let hybrid = ConvertOptions::default()
            .with_row_order(RowOrder::Hybrid)
            .unwrap();
        assert!(matches!(
            v(hybrid, FormatVersion::V5).validate(),
            Err(ConvertError::HybridRowOrderUnsupported)
        ));
        let pre = v(hybrid, FormatVersion::V7_PRE5).validate().unwrap();
        assert_eq!(pre.row_order(), RowOrder::Hybrid);
        assert_eq!(
            v(ConvertOptions::default(), FormatVersion::V5).row_order(),
            RowOrder::Source,
            "an unfixed format defaults to source"
        );
    }

    #[test]
    fn v5_with_four_divisions_in_any_order() {
        let base = ConvertOptions::default();
        let a = v(base, FormatVersion::V5)
            .with_spatial_divisions(4)
            .unwrap()
            .validate()
            .unwrap();
        let b = v(base.with_spatial_divisions(4).unwrap(), FormatVersion::V5)
            .validate()
            .unwrap();
        assert_eq!((a.spatial_divisions(), b.spatial_divisions()), (4, 4));
    }

    #[test]
    fn explicit_v7_with_4096_points_is_fixed_by_format() {
        let options = ConvertOptions::new(4096, 0.01).unwrap();
        assert!(matches!(
            v(options, FormatVersion::V7).validate(),
            Err(ConvertError::FixedByFormat(_))
        ));
        let four = ConvertOptions::default().with_spatial_divisions(4).unwrap();
        assert!(matches!(
            v(four, FormatVersion::V7).validate(),
            Err(ConvertError::FixedByFormat(_))
        ));
    }

    #[test]
    fn v7_resolves_global_values_and_hybrid() {
        let options = v(ConvertOptions::default(), FormatVersion::V7)
            .validate()
            .unwrap();
        assert_eq!(options.points_per_block(), GLOBAL_LEAF_POINT_LIMIT);
        assert_eq!(options.sample_grid(), GLOBAL_SAMPLE_GRID);
        assert_eq!(options.spatial_divisions(), 2);
        assert_eq!(options.row_order(), RowOrder::Hybrid);
        // The default leaf limit recorded by `new` is not a request.
        let new = v(
            ConvertOptions::new(262_144, 0.01).unwrap(),
            FormatVersion::V7,
        );
        assert_eq!(new.validate().unwrap().points_per_block(), 64_000);
    }

    #[test]
    fn v7_with_source_is_fixed_by_format() {
        let source = ConvertOptions::default()
            .with_row_order(RowOrder::Source)
            .unwrap();
        assert!(matches!(
            v(source, FormatVersion::V7).validate(),
            Err(ConvertError::FixedByFormat(_))
        ));
    }

    #[test]
    fn v7_5_pre_keeps_source_default() {
        let options = v(ConvertOptions::default(), FormatVersion::V7_PRE5)
            .validate()
            .unwrap();
        assert_eq!(options.row_order(), RowOrder::Source);
        assert_eq!(options.points_per_block(), GLOBAL_LEAF_POINT_LIMIT);
    }
}
