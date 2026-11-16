//! Experimental internal-node summaries. Exact source rows remain separate and unchanged.
use thiserror::Error;

use crate::{AttributeColumn, AttributeValues, ColorRgb, OccupiedCellIndex};

/// Explicit roles: names alone never determine aggregation semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeAttributePolicy {
    /// Per component: finite-value count u64 followed by approximate mean f64.
    Mean,
    /// Per component: finite-value count u64, then min/max in the original primitive type.
    Range,
    /// Scalar unsigned dense codes in [0, `category_count`), not arbitrary numeric labels.
    Categories { category_count: u32 },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NodeSummaryError {
    #[error("attribute/source row counts differ")]
    RowCountMismatch,
    #[error("categories require scalar unsigned codes and a nonzero domain")]
    InvalidCategories,
    #[error("category code {code} is outside domain of size {category_count}")]
    CategoryOutOfRange { code: u64, category_count: u32 },
}

/// Fixed-width little-endian records, aligned with INTERNAL topology levels only.
/// No leaf record replaces the original rows; duplicate-cell payloads stay exact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSummaryColumn {
    record_width: usize,
    levels: Vec<Vec<u8>>,
}

impl NodeSummaryColumn {
    pub fn record_width(&self) -> usize {
        self.record_width
    }

    pub fn levels(&self) -> &[Vec<u8>] {
        &self.levels
    }

    pub fn record(&self, depth: usize, index: usize) -> Option<&[u8]> {
        let start = index.checked_mul(self.record_width)?;
        self.levels
            .get(depth)?
            .get(start..start.checked_add(self.record_width)?)
    }
}

// Only the current child states and next parent states coexist. Leaf states are temporary.
fn aggregate<T>(
    index: &OccupiedCellIndex,
    width: usize,
    mut leaf: impl FnMut(&[usize]) -> Result<T, NodeSummaryError>,
    mut parent: impl FnMut(&[T]) -> T,
    mut encode: impl FnMut(&T, &mut Vec<u8>),
) -> Result<NodeSummaryColumn, NodeSummaryError> {
    let topology = index.topology();
    let mut children = (0..index.leaf_count())
        .map(|i| leaf(index.rows_for_leaf(i).expect("valid leaf")))
        .collect::<Result<Vec<_>, _>>()?;
    let mut levels = Vec::with_capacity(usize::from(topology.root().depth()));
    for nodes in topology.levels().iter().rev().skip(1) {
        let mut bytes = Vec::with_capacity(nodes.len() * width);
        let mut parents = Vec::with_capacity(nodes.len());
        for node in nodes {
            let start = node.first_child();
            let state = parent(&children[start..start + node.child_count()]);
            encode(&state, &mut bytes);
            parents.push(state);
        }
        levels.push(bytes);
        children = parents;
    }
    levels.reverse();
    Ok(NodeSummaryColumn {
        record_width: width,
        levels,
    })
}

#[derive(Clone, Copy, Default)]
struct Mean {
    count: u64,
    value: f64,
}

impl Mean {
    fn merge(&mut self, other: Self) {
        if other.count == 0 {
            return;
        }
        if self.count == 0 {
            *self = other;
            return;
        }
        let count = self.count + other.count;
        let weight = other.count as f64 / count as f64;
        // Convex combination avoids overflowing a sum or subtraction of opposite extremes.
        self.value = self.value * (1.0 - weight) + other.value * weight;
        self.count = count;
    }
}

fn means(
    index: &OccupiedCellIndex,
    components: usize,
    value: impl Fn(usize, usize) -> f64,
) -> Result<NodeSummaryColumn, NodeSummaryError> {
    aggregate(
        index,
        components * 16,
        |rows| {
            let mut state = vec![Mean::default(); components];
            for &row in rows {
                for (component, mean) in state.iter_mut().enumerate() {
                    let v = value(row, component);
                    if v.is_finite() {
                        mean.merge(Mean { count: 1, value: v });
                    }
                }
            }
            Ok(state)
        },
        |children| {
            let mut state = vec![Mean::default(); components];
            for child in children {
                for (mean, &part) in state.iter_mut().zip(child) {
                    mean.merge(part);
                }
            }
            state
        },
        |state, bytes| {
            for mean in state {
                bytes.extend_from_slice(&mean.count.to_le_bytes());
                bytes.extend_from_slice(&mean.value.to_le_bytes());
            }
        },
    )
}

/// Linear-light RGB means (existing RGB8 -> linear f32 projection, accumulated in f64).
/// The output has three (count:u64, mean:f64) components; it is not quantized back to RGB8.
pub fn summarize_colors(
    index: &OccupiedCellIndex,
    colors: &[ColorRgb],
) -> Result<NodeSummaryColumn, NodeSummaryError> {
    if colors.len() != index.row_count() {
        return Err(NodeSummaryError::RowCountMismatch);
    }
    means(index, 3, |row, component| {
        f64::from(colors[row].to_linear_rgba()[component])
    })
}

/// Builds summaries bottom-up without mutating source values or materializing parent row lists.
/// Nonfinite float values are excluded from means/ranges; count=0 uses zero placeholders.
/// Means project integers to f64; ranges retain exact integer endpoints, including u64/i64.
/// Allocation failures follow the normal Rust policy; this is not a bounded disk parser.
pub fn summarize_attribute(
    index: &OccupiedCellIndex,
    column: &AttributeColumn,
    policy: NodeAttributePolicy,
) -> Result<NodeSummaryColumn, NodeSummaryError> {
    if column.validate_row_count(index.row_count()).is_err() {
        return Err(NodeSummaryError::RowCountMismatch);
    }
    let components = column.descriptor().component_count() as usize;
    let values = column.values();
    match policy {
        NodeAttributePolicy::Mean => means(index, components, |row, component| {
            values
                .scalar_as_f64(row * components + component)
                .expect("validated column")
        }),
        NodeAttributePolicy::Categories { category_count } => {
            if components != 1
                || category_count == 0
                || !matches!(
                    values,
                    AttributeValues::U8(_)
                        | AttributeValues::U16(_)
                        | AttributeValues::U32(_)
                        | AttributeValues::U64(_)
                )
            {
                return Err(NodeSummaryError::InvalidCategories);
            }
            let width = match category_count {
                1..=8 => 1,
                9..=16 => 2,
                17..=32 => 4,
                33..=64 => 8,
                65..=128 => 16,
                _ => (category_count as usize).div_ceil(64) * 8,
            };
            aggregate(
                index,
                width,
                |rows| {
                    let mut bits = vec![0_u8; width];
                    for &row in rows {
                        let code = match values {
                            AttributeValues::U8(v) => u64::from(v[row]),
                            AttributeValues::U16(v) => u64::from(v[row]),
                            AttributeValues::U32(v) => u64::from(v[row]),
                            AttributeValues::U64(v) => v[row],
                            _ => unreachable!(),
                        };
                        if code >= u64::from(category_count) {
                            return Err(NodeSummaryError::CategoryOutOfRange {
                                code,
                                category_count,
                            });
                        }
                        bits[code as usize / 8] |= 1 << (code % 8);
                    }
                    Ok(bits)
                },
                |children| {
                    let mut bits = vec![0; width];
                    for child in children {
                        for (out, &part) in bits.iter_mut().zip(child) {
                            *out |= part;
                        }
                    }
                    bits
                },
                |state, bytes| bytes.extend_from_slice(state),
            )
        }
        NodeAttributePolicy::Range => {
            // Typed comparisons, never through f64: timestamps above 2^53 stay exact.
            macro_rules! ranges {
                ($values:expr, $ty:ty, $finite:expr) => {{
                    let values = $values;
                    aggregate(
                        index,
                        components * (8 + 2 * std::mem::size_of::<$ty>()),
                        |rows| {
                            let mut state = vec![(0_u64, 0 as $ty, 0 as $ty); components];
                            for &row in rows {
                                for (c, (count, min, max)) in state.iter_mut().enumerate() {
                                    let v = values[row * components + c];
                                    if ($finite)(v) {
                                        if *count == 0 {
                                            *min = v;
                                            *max = v;
                                        } else {
                                            if v < *min {
                                                *min = v;
                                            }
                                            if v > *max {
                                                *max = v;
                                            }
                                        }
                                        *count += 1;
                                    }
                                }
                            }
                            Ok(state)
                        },
                        |children| {
                            let mut state = vec![(0_u64, 0 as $ty, 0 as $ty); components];
                            for child in children {
                                for ((count, min, max), &(n, lo, hi)) in state.iter_mut().zip(child)
                                {
                                    if n == 0 {
                                        continue;
                                    }
                                    if *count == 0 {
                                        *min = lo;
                                        *max = hi;
                                    } else {
                                        if lo < *min {
                                            *min = lo;
                                        }
                                        if hi > *max {
                                            *max = hi;
                                        }
                                    }
                                    *count += n;
                                }
                            }
                            state
                        },
                        |state, bytes| {
                            for &(count, min, max) in state {
                                bytes.extend_from_slice(&count.to_le_bytes());
                                bytes.extend_from_slice(&min.to_le_bytes());
                                bytes.extend_from_slice(&max.to_le_bytes());
                            }
                        },
                    )
                }};
            }
            match values {
                AttributeValues::U8(v) => ranges!(v, u8, |_| true),
                AttributeValues::I8(v) => ranges!(v, i8, |_| true),
                AttributeValues::U16(v) => ranges!(v, u16, |_| true),
                AttributeValues::I16(v) => ranges!(v, i16, |_| true),
                AttributeValues::U32(v) => ranges!(v, u32, |_| true),
                AttributeValues::I32(v) => ranges!(v, i32, |_| true),
                AttributeValues::U64(v) => ranges!(v, u64, |_| true),
                AttributeValues::I64(v) => ranges!(v, i64, |_| true),
                AttributeValues::F32(v) => ranges!(v, f32, f32::is_finite),
                AttributeValues::F64(v) => ranges!(v, f64, f64::is_finite),
            }
        }
    }
}
