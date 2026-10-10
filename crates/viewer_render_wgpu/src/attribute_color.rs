use viewer_core::{AttributeColumn, ColorRgb, PointBatch};

/// Applies one numeric component using a dataset-wide finite range. Nonfinite or
/// constant values are neutral gray; original attributes are never modified.
pub fn colorize_attribute(
    geometry: PointBatch,
    column: &AttributeColumn,
    component: u32,
    range: Option<[f64; 2]>,
) -> Result<PointBatch, &'static str> {
    let width = column.descriptor().component_count() as usize;
    if component as usize >= width
        || geometry.len().checked_mul(width) != Some(column.values().len())
    {
        return Err("attribute component or row count does not match geometry");
    }
    let mut points = geometry.into_points();
    for (row, point) in points.iter_mut().enumerate() {
        let value = column
            .values()
            .scalar_as_f64(row * width + component as usize)
            .ok_or("missing attribute value")?;
        point.color = ColorRgb::new(160, 160, 160);
        if let Some([min, max]) = range
            && value.is_finite()
            && min.is_finite()
            && max.is_finite()
            && max > min
        {
            let span = max - min;
            let t = if span.is_finite() {
                (value - min) / span
            } else {
                // Halving keeps opposite finite extremes from overflowing.
                (value * 0.5 - min * 0.5) / (max * 0.5 - min * 0.5)
            }
            .clamp(0.0, 1.0);
            point.color = ColorRgb::new(
                (255.0 * t) as u8,
                (255.0 * (1.0 - (2.0 * t - 1.0).abs())) as u8,
                (255.0 * (1.0 - t)) as u8,
            );
        }
    }
    Ok(PointBatch::new(points))
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{AttributeDataType, AttributeDescriptor, AttributeValues, Point};
    #[test]
    fn colors_selected_component_with_stable_range_and_gray_nonfinite_values() {
        let column = AttributeColumn::new(
            AttributeDescriptor::new("vector", AttributeDataType::F64, 2).unwrap(),
            AttributeValues::F64(vec![999.0, -10.0, 999.0, 10.0, 999.0, f64::NAN]),
            3,
        )
        .unwrap();
        let points = || PointBatch::new(vec![Point::new([0.0; 3], ColorRgb::new(0, 0, 0)); 3]);
        let colored = colorize_attribute(points(), &column, 1, Some([-10.0, 10.0])).unwrap();
        assert_eq!(colored.points()[0].color, ColorRgb::new(0, 0, 255));
        assert_eq!(colored.points()[1].color, ColorRgb::new(255, 0, 0));
        assert_eq!(colored.points()[2].color, ColorRgb::new(160, 160, 160));
        assert!(colorize_attribute(points(), &column, 2, None).is_err());
        assert!(
            colorize_attribute(points(), &column, 0, Some([999.0, 999.0]))
                .unwrap()
                .points()
                .iter()
                .all(|p| p.color == ColorRgb::new(160, 160, 160))
        );
    }
}
