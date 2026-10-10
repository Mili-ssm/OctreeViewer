//! Typed finite ranges for stable coloring across independently loaded blocks.
use crate::NodeAttributeRange;
use viewer_core::{AttributeColumn, AttributeValues};

pub(crate) fn column_range(index: u32, column: &AttributeColumn) -> Option<NodeAttributeRange> {
    let width = column.descriptor().component_count() as usize;
    macro_rules! range {
        ($values:expr, $variant:ident) => {{
            let mut min = Vec::with_capacity(width);
            let mut max = Vec::with_capacity(width);
            for component in 0..width {
                let mut values = $values
                    .iter()
                    .skip(component)
                    .step_by(width)
                    .copied()
                    .filter(|value| (*value as f64).is_finite());
                let first = values.next()?;
                let (low, high) = values.fold((first, first), |(low, high), value| {
                    (
                        if value < low { value } else { low },
                        if value > high { value } else { high },
                    )
                });
                min.push(low);
                max.push(high);
            }
            Some(NodeAttributeRange::new(
                index,
                AttributeValues::$variant(min),
                AttributeValues::$variant(max),
            ))
        }};
    }
    match column.values() {
        AttributeValues::U8(v) => range!(v, U8),
        AttributeValues::I8(v) => range!(v, I8),
        AttributeValues::U16(v) => range!(v, U16),
        AttributeValues::I16(v) => range!(v, I16),
        AttributeValues::U32(v) => range!(v, U32),
        AttributeValues::I32(v) => range!(v, I32),
        AttributeValues::U64(v) => range!(v, U64),
        AttributeValues::I64(v) => range!(v, I64),
        AttributeValues::F32(v) => range!(v, F32),
        AttributeValues::F64(v) => range!(v, F64),
    }
}

pub(crate) fn merge_range(target: &mut NodeAttributeRange, other: &NodeAttributeRange) {
    macro_rules! merge {
        ($a:expr, $b:expr, $variant:ident, $less:expr) => {
            if let (AttributeValues::$variant(a), AttributeValues::$variant(b)) = ($a, $b) {
                for (a, b) in a.iter_mut().zip(b) {
                    if $less(*b, *a) {
                        *a = *b;
                    }
                }
            }
        };
    }
    macro_rules! both {
        ($variant:ident) => {
            merge!(&mut target.min, &other.min, $variant, |a, b| a < b);
            merge!(&mut target.max, &other.max, $variant, |a, b| a > b);
        };
    }
    both!(U8);
    both!(I8);
    both!(U16);
    both!(I16);
    both!(U32);
    both!(I32);
    both!(U64);
    both!(I64);
    both!(F32);
    both!(F64);
}
