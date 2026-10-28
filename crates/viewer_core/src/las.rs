//! Shared LAS-to-attribute contract; independent of any LAS parsing crate.

use crate::{AttributeDataType, AttributeDescriptor, SpatialBounds};

/// Longest bounds axis is scaled to this many render units.
pub const TARGET_RENDER_EXTENT: f64 = 100.0;

/// Attribute schema stored for LAS sources, in on-disk column order.
pub fn las_attribute_schema(
    has_gps_time: bool,
    has_color: bool,
    has_nir: bool,
) -> Vec<AttributeDescriptor> {
    let descriptor = |name, data_type, components| {
        AttributeDescriptor::new(name, data_type, components)
            .expect("static attribute schema is valid")
    };
    let mut schema = vec![
        descriptor("intensity", AttributeDataType::U16, 1),
        descriptor("classification", AttributeDataType::U8, 1),
        descriptor("return_number", AttributeDataType::U8, 1),
        descriptor("number_of_returns", AttributeDataType::U8, 1),
        descriptor("scan_angle_degrees", AttributeDataType::F32, 1),
        descriptor("user_data", AttributeDataType::U8, 1),
        descriptor("point_source_id", AttributeDataType::U16, 1),
    ];
    if has_gps_time {
        schema.push(descriptor("gps_time", AttributeDataType::F64, 1));
    }
    if has_color {
        schema.push(descriptor("rgb", AttributeDataType::U16, 3));
    }
    if has_nir {
        schema.push(descriptor("nir", AttributeDataType::U16, 1));
    }
    schema
}

/// Visual scale for `bounds`; `None` when it is not representable as a finite
/// non-zero `f32`.
pub fn visual_scale_for_bounds(bounds: SpatialBounds) -> Option<f64> {
    let min = bounds.min();
    let max = bounds.max();
    let extent = (0..3)
        .map(|axis| max[axis] - min[axis])
        .fold(0.0_f64, f64::max);
    if extent == 0.0 {
        return Some(1.0);
    }
    let scale = TARGET_RENDER_EXTENT / extent;
    if !scale.is_finite() || scale <= 0.0 || !(scale as f32).is_finite() || (scale as f32) == 0.0 {
        return None;
    }
    Some(scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_schema_has_seven_entries() {
        assert_eq!(las_attribute_schema(false, false, false).len(), 7);
    }

    #[test]
    fn full_schema_keeps_contract_order() {
        let schema = las_attribute_schema(true, true, true);
        let expected = [
            ("intensity", AttributeDataType::U16, 1),
            ("classification", AttributeDataType::U8, 1),
            ("return_number", AttributeDataType::U8, 1),
            ("number_of_returns", AttributeDataType::U8, 1),
            ("scan_angle_degrees", AttributeDataType::F32, 1),
            ("user_data", AttributeDataType::U8, 1),
            ("point_source_id", AttributeDataType::U16, 1),
            ("gps_time", AttributeDataType::F64, 1),
            ("rgb", AttributeDataType::U16, 3),
            ("nir", AttributeDataType::U16, 1),
        ];
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(name, data_type, components)| {
                AttributeDescriptor::new(name, data_type, components).unwrap()
            })
            .collect();
        assert_eq!(schema.len(), 10);
        assert_eq!(schema, expected);
    }

    #[test]
    fn visual_scale_matches_contract() {
        let bounds = SpatialBounds::new([10.0, -4.0, 0.0], [20.0, 16.0, 2.0]).unwrap();
        assert_eq!(visual_scale_for_bounds(bounds), Some(5.0));
        let zero = SpatialBounds::new([0.0; 3], [0.0; 3]).unwrap();
        assert_eq!(visual_scale_for_bounds(zero), Some(1.0));
        let tiny = SpatialBounds::new([0.0; 3], [f64::MIN_POSITIVE, 0.0, 0.0]).unwrap();
        assert_eq!(visual_scale_for_bounds(tiny), None);
    }
}
