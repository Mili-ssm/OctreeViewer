//! Test-only constructors that pin `v5`, so tests of V5, EPT or checkpoint behavior do not
//! depend on `FormatVersion::DEFAULT_OUTPUT`.

use crate::{ConvertError, ConvertOptions};

pub(crate) fn v5_default() -> ConvertOptions {
    ConvertOptions::default()
        .with_format_version("v5".parse().unwrap())
        .unwrap()
}

pub(crate) fn v5_new(
    points_per_block: u64,
    max_quantization_error: f64,
) -> Result<ConvertOptions, ConvertError> {
    ConvertOptions::new(points_per_block, max_quantization_error)?
        .with_format_version("v5".parse().unwrap())
}

pub(crate) fn v5_budget(
    points_per_block: u64,
    max_quantization_error: f64,
    max_memory_bytes: u64,
) -> Result<ConvertOptions, ConvertError> {
    ConvertOptions::with_memory_budget(points_per_block, max_quantization_error, max_memory_bytes)?
        .with_format_version("v5".parse().unwrap())
}
