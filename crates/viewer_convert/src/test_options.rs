//! Test-only constructors: small leaves, so small fixtures still build multi-level trees. V8
//! fixes the leaf limit at 64 000 points; production paths cannot lower it.

use crate::{ConvertError, ConvertOptions};

pub(crate) fn leaf_new(
    points_per_block: u64,
    max_quantization_error: f64,
) -> Result<ConvertOptions, ConvertError> {
    Ok(ConvertOptions::new(max_quantization_error)?
        .with_leaf_point_limit_for_tests(points_per_block))
}

pub(crate) fn leaf_budget(
    points_per_block: u64,
    max_quantization_error: f64,
    max_memory_bytes: u64,
) -> Result<ConvertOptions, ConvertError> {
    Ok(
        ConvertOptions::with_memory_budget(max_quantization_error, max_memory_bytes)?
            .with_leaf_point_limit_for_tests(points_per_block),
    )
}
