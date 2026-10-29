//! Pure limit checks shared by the native shell and the conversion core.

use viewer_core::SpatialBounds;
use viewer_format::FormatReaderLimits;

use crate::error::ConvertError;

/// Rejects a plan whose index could carry more attribute ranges (nodes x attributes) than the
/// default reader accepts, so the converter never writes a file its own reader refuses.
pub(crate) fn check_reader_attribute_ranges(
    planned_nodes: usize,
    attribute_count: usize,
) -> Result<(), ConvertError> {
    check_attribute_ranges_against(
        planned_nodes,
        attribute_count,
        FormatReaderLimits::DEFAULT_MAX_ATTRIBUTE_RANGES,
    )
}

fn check_attribute_ranges_against(
    planned_nodes: usize,
    attribute_count: usize,
    limit: u64,
) -> Result<(), ConvertError> {
    let nodes = u64::try_from(planned_nodes).map_err(|_| ConvertError::SizeOverflow)?;
    let attributes = u64::try_from(attribute_count).map_err(|_| ConvertError::SizeOverflow)?;
    let ranges = nodes
        .checked_mul(attributes)
        .ok_or(ConvertError::SizeOverflow)?;
    if ranges > limit {
        return Err(ConvertError::TooManyAttributeRanges {
            nodes,
            attributes,
            ranges,
            limit,
        });
    }
    Ok(())
}

pub(crate) fn visual_scale_for_bounds(bounds: SpatialBounds) -> Result<f64, ConvertError> {
    viewer_core::las::checked_visual_scale(bounds).map_err(ConvertError::InvalidVisualScale)
}

#[cfg(test)]
mod range_bound_tests {
    use super::*;

    #[test]
    fn attribute_range_bound_names_both_numbers() {
        check_attribute_ranges_against(1_000, 10, 10_000).unwrap();
        let error = check_attribute_ranges_against(1_001, 10, 10_000).unwrap_err();
        assert!(matches!(
            error,
            ConvertError::TooManyAttributeRanges {
                ranges: 10_010,
                limit: 10_000,
                ..
            }
        ));
        let message = error.to_string();
        assert!(
            message.contains("10010") && message.contains("10000"),
            "{message}"
        );
    }

    #[test]
    fn default_bound_rejects_a_million_nodes_with_ten_attributes() {
        assert!(check_reader_attribute_ranges(1_000_000, 10).is_err());
        check_reader_attribute_ranges(100_000, 10).unwrap();
    }
}
