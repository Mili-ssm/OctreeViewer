use viewer_core::{AttributeValues, PointBlock};

#[derive(Clone, Copy)]
pub(crate) struct ScalarAttribute<'a> {
    values: &'a AttributeValues,
}

impl<'a> ScalarAttribute<'a> {
    pub(crate) fn find(block: &'a PointBlock, name: &str) -> Option<Self> {
        let point_count = block.points().len();
        let column = block.attributes().iter().find(|column| {
            column.descriptor().name().eq_ignore_ascii_case(name)
                && column.descriptor().component_count() == 1
        })?;
        let values = column.values();
        (values.len() == point_count && values_are_valid(values)).then_some(Self { values })
    }

    pub(crate) fn value_at(self, index: usize) -> Option<f32> {
        let value = match self.values {
            AttributeValues::U8(values) => f64::from(*values.get(index)?),
            AttributeValues::I8(values) => f64::from(*values.get(index)?),
            AttributeValues::U16(values) => f64::from(*values.get(index)?),
            AttributeValues::I16(values) => f64::from(*values.get(index)?),
            AttributeValues::U32(values) => f64::from(*values.get(index)?),
            AttributeValues::I32(values) => f64::from(*values.get(index)?),
            AttributeValues::U64(values) => *values.get(index)? as f64,
            AttributeValues::I64(values) => *values.get(index)? as f64,
            AttributeValues::F32(values) => f64::from(*values.get(index)?),
            AttributeValues::F64(values) => *values.get(index)?,
        };
        scalar_value_to_f32(value)
    }
}

fn values_are_valid(values: &AttributeValues) -> bool {
    match values {
        AttributeValues::U8(_)
        | AttributeValues::I8(_)
        | AttributeValues::U16(_)
        | AttributeValues::I16(_)
        | AttributeValues::U32(_)
        | AttributeValues::I32(_)
        | AttributeValues::U64(_)
        | AttributeValues::I64(_) => true,
        AttributeValues::F32(values) => values.iter().all(|value| value.is_finite()),
        AttributeValues::F64(values) => values
            .iter()
            .all(|value| scalar_value_to_f32(*value).is_some()),
    }
}

fn scalar_value_to_f32(value: f64) -> Option<f32> {
    (value.is_finite() && value.abs() <= f64::from(f32::MAX)).then_some(value as f32)
}

#[cfg(test)]
mod tests {
    use viewer_core::{
        AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId,
        ColorRgb, Point, PointBatch, PointBlock,
    };

    use super::ScalarAttribute;

    fn block(name: &str, data_type: AttributeDataType, values: AttributeValues) -> PointBlock {
        let count = values.len();
        let column = AttributeColumn::new(
            AttributeDescriptor::new(name, data_type, 1).unwrap(),
            values,
            count,
        )
        .unwrap();
        PointBlock::new(
            BlockId::new(0),
            PointBatch::new(
                (0..count)
                    .map(|_| Point::new([0.0; 3], ColorRgb::new(0, 0, 0)))
                    .collect(),
            ),
            vec![column],
        )
    }

    #[test]
    fn accepts_all_scalar_attribute_types_and_finds_names_case_insensitively() {
        let cases = [
            (AttributeDataType::U8, AttributeValues::U8(vec![8]), 8.0),
            (AttributeDataType::I8, AttributeValues::I8(vec![-8]), -8.0),
            (AttributeDataType::U16, AttributeValues::U16(vec![16]), 16.0),
            (
                AttributeDataType::I16,
                AttributeValues::I16(vec![-16]),
                -16.0,
            ),
            (AttributeDataType::U32, AttributeValues::U32(vec![32]), 32.0),
            (
                AttributeDataType::I32,
                AttributeValues::I32(vec![-32]),
                -32.0,
            ),
            (AttributeDataType::U64, AttributeValues::U64(vec![64]), 64.0),
            (
                AttributeDataType::I64,
                AttributeValues::I64(vec![-64]),
                -64.0,
            ),
            (
                AttributeDataType::F32,
                AttributeValues::F32(vec![-3.5]),
                -3.5,
            ),
            (AttributeDataType::F64, AttributeValues::F64(vec![4.5]), 4.5),
        ];

        for (data_type, values, expected) in cases {
            let block = block("InTeNsItY", data_type, values);
            assert_eq!(
                ScalarAttribute::find(&block, "intensity")
                    .unwrap()
                    .value_at(0),
                Some(expected)
            );
        }
    }

    #[test]
    fn rejects_multicomponent_and_point_count_mismatched_columns() {
        let multicomponent = AttributeColumn::new(
            AttributeDescriptor::new("intensity", AttributeDataType::U8, 2).unwrap(),
            AttributeValues::U8(vec![1, 2]),
            1,
        )
        .unwrap();
        let multicomponent_block = PointBlock::new(
            BlockId::new(0),
            PointBatch::new(vec![Point::new([0.0; 3], ColorRgb::new(0, 0, 0))]),
            vec![multicomponent],
        );
        assert!(ScalarAttribute::find(&multicomponent_block, "intensity").is_none());

        let one_point = block(
            "intensity",
            AttributeDataType::U8,
            AttributeValues::U8(vec![1]),
        );
        let mismatched = PointBlock::new(
            one_point.id(),
            PointBatch::new(vec![
                Point::new([0.0; 3], ColorRgb::new(0, 0, 0)),
                Point::new([1.0; 3], ColorRgb::new(0, 0, 0)),
            ]),
            one_point.attributes().to_vec(),
        );
        assert!(ScalarAttribute::find(&mismatched, "intensity").is_none());
    }

    #[test]
    fn rejects_non_finite_and_out_of_f32_range_float_values() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let block = block(
                "intensity",
                AttributeDataType::F32,
                AttributeValues::F32(vec![value]),
            );
            assert!(ScalarAttribute::find(&block, "intensity").is_none());
        }
        for value in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::from(f32::MAX) * 2.0,
        ] {
            let block = block(
                "intensity",
                AttributeDataType::F64,
                AttributeValues::F64(vec![value]),
            );
            assert!(ScalarAttribute::find(&block, "intensity").is_none());
        }
    }
}
