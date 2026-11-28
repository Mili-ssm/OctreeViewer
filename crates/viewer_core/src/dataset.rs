use thiserror::Error;

use crate::{BlockId, CoordinateFrame, PointBatch, SpatialError, SpatialHierarchy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeDataType {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeDescriptor {
    name: String,
    data_type: AttributeDataType,
    component_count: u32,
}

impl AttributeDescriptor {
    pub fn new(
        name: impl Into<String>,
        data_type: AttributeDataType,
        component_count: u32,
    ) -> Result<Self, SpatialError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(SpatialError::EmptyAttributeName);
        }
        if component_count == 0 {
            return Err(SpatialError::ZeroAttributeComponents { name });
        }

        Ok(Self {
            name,
            data_type,
            component_count,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn data_type(&self) -> AttributeDataType {
        self.data_type
    }

    pub const fn component_count(&self) -> u32 {
        self.component_count
    }

    pub(crate) fn allocated_name_bytes(&self) -> usize {
        self.name.capacity()
    }
}

/// Dataset information shared by readers and consumers of spatial blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct DatasetMetadata {
    coordinate_frame: CoordinateFrame,
    hierarchy: SpatialHierarchy,
}

impl DatasetMetadata {
    pub const fn new(coordinate_frame: CoordinateFrame, hierarchy: SpatialHierarchy) -> Self {
        Self {
            coordinate_frame,
            hierarchy,
        }
    }

    pub const fn coordinate_frame(&self) -> CoordinateFrame {
        self.coordinate_frame
    }

    pub const fn hierarchy(&self) -> &SpatialHierarchy {
        &self.hierarchy
    }

    /// Grows or shrinks the hierarchy as pages arrive or are evicted.
    pub const fn hierarchy_mut(&mut self) -> &mut SpatialHierarchy {
        &mut self.hierarchy
    }
}

/// Typed values for one attribute column, stored compactly in its declared type.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeValues {
    U8(Vec<u8>),
    I8(Vec<i8>),
    U16(Vec<u16>),
    I16(Vec<i16>),
    U32(Vec<u32>),
    I32(Vec<i32>),
    U64(Vec<u64>),
    I64(Vec<i64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

impl AttributeValues {
    /// Numeric projection for display; 64-bit integers may lose precision in f64.
    /// Stored typed values remain unchanged.
    pub fn scalar_as_f64(&self, index: usize) -> Option<f64> {
        Some(match self {
            Self::U8(v) => f64::from(*v.get(index)?),
            Self::I8(v) => f64::from(*v.get(index)?),
            Self::U16(v) => f64::from(*v.get(index)?),
            Self::I16(v) => f64::from(*v.get(index)?),
            Self::U32(v) => f64::from(*v.get(index)?),
            Self::I32(v) => f64::from(*v.get(index)?),
            Self::U64(v) => *v.get(index)? as f64,
            Self::I64(v) => *v.get(index)? as f64,
            Self::F32(v) => f64::from(*v.get(index)?),
            Self::F64(v) => *v.get(index)?,
        })
    }

    pub fn empty(data_type: AttributeDataType) -> Self {
        match data_type {
            AttributeDataType::U8 => Self::U8(Vec::new()),
            AttributeDataType::I8 => Self::I8(Vec::new()),
            AttributeDataType::U16 => Self::U16(Vec::new()),
            AttributeDataType::I16 => Self::I16(Vec::new()),
            AttributeDataType::U32 => Self::U32(Vec::new()),
            AttributeDataType::I32 => Self::I32(Vec::new()),
            AttributeDataType::U64 => Self::U64(Vec::new()),
            AttributeDataType::I64 => Self::I64(Vec::new()),
            AttributeDataType::F32 => Self::F32(Vec::new()),
            AttributeDataType::F64 => Self::F64(Vec::new()),
        }
    }

    /// Selects interleaved rows in the requested order, preserving repeated indices and bits.
    pub fn select_rows(&self, indices: &[usize], components: usize) -> Result<Self, DatasetError> {
        if components == 0 {
            return Err(DatasetError::ZeroAttributeComponents);
        }
        let value_count = indices
            .len()
            .checked_mul(components)
            .ok_or(DatasetError::AttributeSelectionOverflow)?;

        macro_rules! select {
            ($values:expr, $variant:ident) => {{
                let values = $values;
                let mut selected = Vec::new();
                selected
                    .try_reserve_exact(value_count)
                    .map_err(|_| DatasetError::AttributeAllocationFailed)?;
                for index in indices {
                    let start = index
                        .checked_mul(components)
                        .ok_or(DatasetError::AttributeIndexOverflow { index: *index })?;
                    let end = start
                        .checked_add(components)
                        .ok_or(DatasetError::AttributeIndexOverflow { index: *index })?;
                    selected.extend_from_slice(
                        values
                            .get(start..end)
                            .ok_or(DatasetError::AttributeRowOutOfRange { index: *index })?,
                    );
                }
                Ok(Self::$variant(selected))
            }};
        }

        match self {
            Self::U8(values) => select!(values, U8),
            Self::I8(values) => select!(values, I8),
            Self::U16(values) => select!(values, U16),
            Self::I16(values) => select!(values, I16),
            Self::U32(values) => select!(values, U32),
            Self::I32(values) => select!(values, I32),
            Self::U64(values) => select!(values, U64),
            Self::I64(values) => select!(values, I64),
            Self::F32(values) => select!(values, F32),
            Self::F64(values) => select!(values, F64),
        }
    }

    /// Appends values of the same type, reserving before mutation.
    pub fn extend_from(&mut self, other: &Self) -> Result<(), DatasetError> {
        if self.data_type() != other.data_type() {
            return Err(DatasetError::AttributeValuesTypeMismatch {
                expected: self.data_type(),
                actual: other.data_type(),
            });
        }
        macro_rules! extend {
            ($target:expr, $values:expr) => {{
                $target
                    .try_reserve($values.len())
                    .map_err(|_| DatasetError::AttributeAllocationFailed)?;
                $target.extend_from_slice($values);
                Ok(())
            }};
        }
        match (self, other) {
            (Self::U8(target), Self::U8(values)) => extend!(target, values),
            (Self::I8(target), Self::I8(values)) => extend!(target, values),
            (Self::U16(target), Self::U16(values)) => extend!(target, values),
            (Self::I16(target), Self::I16(values)) => extend!(target, values),
            (Self::U32(target), Self::U32(values)) => extend!(target, values),
            (Self::I32(target), Self::I32(values)) => extend!(target, values),
            (Self::U64(target), Self::U64(values)) => extend!(target, values),
            (Self::I64(target), Self::I64(values)) => extend!(target, values),
            (Self::F32(target), Self::F32(values)) => extend!(target, values),
            (Self::F64(target), Self::F64(values)) => extend!(target, values),
            _ => unreachable!("matching attribute value types must have matching variants"),
        }
    }

    pub const fn data_type(&self) -> AttributeDataType {
        match self {
            Self::U8(_) => AttributeDataType::U8,
            Self::I8(_) => AttributeDataType::I8,
            Self::U16(_) => AttributeDataType::U16,
            Self::I16(_) => AttributeDataType::I16,
            Self::U32(_) => AttributeDataType::U32,
            Self::I32(_) => AttributeDataType::I32,
            Self::U64(_) => AttributeDataType::U64,
            Self::I64(_) => AttributeDataType::I64,
            Self::F32(_) => AttributeDataType::F32,
            Self::F64(_) => AttributeDataType::F64,
        }
    }

    /// Returns the number of scalar values, not the number of points.
    pub fn len(&self) -> usize {
        match self {
            Self::U8(values) => values.len(),
            Self::I8(values) => values.len(),
            Self::U16(values) => values.len(),
            Self::I16(values) => values.len(),
            Self::U32(values) => values.len(),
            Self::I32(values) => values.len(),
            Self::U64(values) => values.len(),
            Self::I64(values) => values.len(),
            Self::F32(values) => values.len(),
            Self::F64(values) => values.len(),
        }
    }

    /// Returns whether this column contains no scalar values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn allocated_bytes(&self) -> usize {
        match self {
            Self::U8(values) => values.capacity() * std::mem::size_of::<u8>(),
            Self::I8(values) => values.capacity() * std::mem::size_of::<i8>(),
            Self::U16(values) => values.capacity() * std::mem::size_of::<u16>(),
            Self::I16(values) => values.capacity() * std::mem::size_of::<i16>(),
            Self::U32(values) => values.capacity() * std::mem::size_of::<u32>(),
            Self::I32(values) => values.capacity() * std::mem::size_of::<i32>(),
            Self::U64(values) => values.capacity() * std::mem::size_of::<u64>(),
            Self::I64(values) => values.capacity() * std::mem::size_of::<i64>(),
            Self::F32(values) => values.capacity() * std::mem::size_of::<f32>(),
            Self::F64(values) => values.capacity() * std::mem::size_of::<f64>(),
        }
    }
}

/// One named, typed attribute column. Multi-component values are interleaved.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeColumn {
    descriptor: AttributeDescriptor,
    values: AttributeValues,
}

impl AttributeColumn {
    pub fn new(
        descriptor: AttributeDescriptor,
        values: AttributeValues,
        point_count: usize,
    ) -> Result<Self, DatasetError> {
        if descriptor.data_type() != values.data_type() {
            return Err(DatasetError::AttributeTypeMismatch {
                name: descriptor.name().to_owned(),
                expected: descriptor.data_type(),
                actual: values.data_type(),
            });
        }
        let column = Self { descriptor, values };
        column.validate_row_count(point_count)?;
        Ok(column)
    }

    pub(crate) fn validate_row_count(&self, point_count: usize) -> Result<(), DatasetError> {
        let expected_len = point_count
            .checked_mul(self.descriptor.component_count() as usize)
            .ok_or_else(|| DatasetError::AttributeLengthOverflow {
                name: self.descriptor.name().to_owned(),
            })?;
        let actual_len = self.values.len();
        if actual_len != expected_len {
            return Err(DatasetError::InvalidAttributeLength {
                name: self.descriptor.name().to_owned(),
                expected: expected_len,
                actual: actual_len,
            });
        }
        Ok(())
    }

    pub fn descriptor(&self) -> &AttributeDescriptor {
        &self.descriptor
    }

    pub fn values(&self) -> &AttributeValues {
        &self.values
    }
}

/// Payload for a single independently readable spatial block.
#[derive(Debug, Clone, PartialEq)]
pub struct PointBlock {
    id: BlockId,
    points: PointBatch,
    attributes: Vec<AttributeColumn>,
}

impl PointBlock {
    pub fn new(id: BlockId, points: PointBatch, attributes: Vec<AttributeColumn>) -> Self {
        Self {
            id,
            points,
            attributes,
        }
    }

    pub const fn id(&self) -> BlockId {
        self.id
    }

    pub const fn points(&self) -> &PointBatch {
        &self.points
    }

    /// The columns the block carries, in schema order. A format revision may omit some, so
    /// identify a column by its descriptor, not by its position.
    pub fn attributes(&self) -> &[AttributeColumn] {
        &self.attributes
    }

    /// Moves the block's ID, points, and attributes out without cloning payloads.
    pub fn into_parts(self) -> (BlockId, PointBatch, Vec<AttributeColumn>) {
        (self.id, self.points, self.attributes)
    }

    /// Estimates bytes retained by this block's payload allocations and inline data.
    ///
    /// The estimate uses vector capacities, so reserved but currently unused payload
    /// space counts against a cache budget. Allocator and cache-index overhead is not
    /// included.
    pub fn estimated_memory_bytes(&self) -> usize {
        let point_bytes = self.points.allocated_bytes();
        let columns_bytes = self.attributes.capacity() * std::mem::size_of::<AttributeColumn>();
        let column_payload_bytes = self
            .attributes
            .iter()
            .map(|column| {
                column
                    .descriptor
                    .allocated_name_bytes()
                    .saturating_add(column.values.allocated_bytes())
            })
            .fold(0_usize, usize::saturating_add);
        std::mem::size_of::<Self>()
            .saturating_add(point_bytes)
            .saturating_add(columns_bytes)
            .saturating_add(column_payload_bytes)
    }
}

/// Borrowed access to already decoded resident blocks, without I/O or ownership transfer.
///
/// Queries must not change residency, recency or activity counters. A returned block
/// must have the requested ID. Presence and payload must remain consistent throughout
/// a consumer operation. `contains` is the residency predicate; by default it means `peek` returns
/// a block. A source for the planner in cells mode (ADR 0038) overrides it to mean "resident for
/// planning", which may hold while `peek` is `None`: only the planner (`contains` and
/// `budget_points`) may be given such a source, and any consumer that needs the payload must
/// handle a `None` from `peek`.
/// No thread-safety bounds are imposed on implementations.
pub trait ResidentBlockSource {
    fn peek(&self, id: BlockId) -> Option<&PointBlock>;

    fn contains(&self, id: BlockId) -> bool {
        self.peek(id).is_some()
    }

    /// Points the planner charges to its budget for a resident block. A source whose residency
    /// does not mean decoded points (cells mode, ADR 0038) may charge less and has its own budget.
    fn budget_points(&self, id: BlockId) -> Option<u64> {
        self.peek(id)
            .map(|block| u64::try_from(block.points().len()).unwrap_or(u64::MAX))
    }
}

/// Platform-independent contract for reading selected blocks from a dataset.
/// Implementations may perform blocking I/O; callers choose the appropriate worker.
pub trait DatasetSource {
    type Error;

    fn metadata(&self) -> &DatasetMetadata;

    /// Returns the requested block with its declared ID and any dataset attributes.
    fn read_block(&mut self, id: BlockId) -> Result<PointBlock, Self::Error>;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DatasetError {
    #[error("attribute component count must be nonzero")]
    ZeroAttributeComponents,
    #[error("attribute row selection index is out of range: {index}")]
    AttributeRowOutOfRange { index: usize },
    #[error("attribute row selection index overflow: {index}")]
    AttributeIndexOverflow { index: usize },
    #[error("attribute selection length overflow")]
    AttributeSelectionOverflow,
    #[error("attribute value allocation failed")]
    AttributeAllocationFailed,
    #[error("attribute values have type {actual:?}, expected {expected:?}")]
    AttributeValuesTypeMismatch {
        expected: AttributeDataType,
        actual: AttributeDataType,
    },
    #[error("attribute {name:?} has type {actual:?}, expected {expected:?}")]
    AttributeTypeMismatch {
        name: String,
        expected: AttributeDataType,
        actual: AttributeDataType,
    },
    #[error("attribute {name:?} component count overflows the platform length")]
    AttributeLengthOverflow { name: String },
    #[error("attribute {name:?} has {actual} values, expected {expected}")]
    InvalidAttributeLength {
        name: String,
        expected: usize,
        actual: usize,
    },
}

#[cfg(test)]
mod tests {
    use crate::{AttributeDataType, AttributeDescriptor};

    use super::{AttributeColumn, AttributeValues, DatasetError};

    #[test]
    fn attribute_column_checks_type_and_interleaved_component_count() {
        let descriptor = AttributeDescriptor::new("rgb", AttributeDataType::U8, 3).unwrap();
        let column = AttributeColumn::new(
            descriptor.clone(),
            AttributeValues::U8(vec![1, 2, 3, 4, 5, 6]),
            2,
        )
        .unwrap();
        assert_eq!(column.descriptor(), &descriptor);

        assert_eq!(
            AttributeColumn::new(
                descriptor.clone(),
                AttributeValues::U16(vec![1, 2, 3, 4, 5, 6]),
                2,
            )
            .unwrap_err(),
            DatasetError::AttributeTypeMismatch {
                name: "rgb".to_owned(),
                expected: AttributeDataType::U8,
                actual: AttributeDataType::U16,
            }
        );
        assert_eq!(
            AttributeColumn::new(descriptor, AttributeValues::U8(vec![1, 2, 3]), 2).unwrap_err(),
            DatasetError::InvalidAttributeLength {
                name: "rgb".to_owned(),
                expected: 6,
                actual: 3,
            }
        );
    }

    #[test]
    fn attribute_values_select_interleaved_rows_in_order_with_repeats() {
        let source = AttributeValues::U16(vec![10, 11, 20, 21, 30, 31]);
        assert_eq!(
            source.select_rows(&[2, 0, 2], 2).unwrap(),
            AttributeValues::U16(vec![30, 31, 10, 11, 30, 31])
        );
    }

    #[test]
    fn attribute_values_selection_preserves_nan_payload_bits() {
        let nan_bits = 0x7fc0_1234;
        let values = AttributeValues::F32(vec![f32::from_bits(nan_bits), -0.0]);
        let selected = values.select_rows(&[0, 1], 1).unwrap();
        let AttributeValues::F32(selected) = selected else {
            panic!("selection changed value type");
        };
        assert_eq!(selected[0].to_bits(), nan_bits);
        assert_eq!(selected[1].to_bits(), (-0.0_f32).to_bits());
    }

    #[test]
    fn attribute_values_selection_rejects_invalid_components_and_indices() {
        assert_eq!(
            AttributeValues::U8(vec![1]).select_rows(&[0], 0),
            Err(DatasetError::ZeroAttributeComponents)
        );
        assert_eq!(
            AttributeValues::U8(vec![1]).select_rows(&[1], 1),
            Err(DatasetError::AttributeRowOutOfRange { index: 1 })
        );
        assert_eq!(
            AttributeValues::U8(vec![1]).select_rows(&[usize::MAX], 2),
            Err(DatasetError::AttributeIndexOverflow { index: usize::MAX })
        );
        assert_eq!(
            AttributeValues::U8(vec![1]).select_rows(&[0, 1], usize::MAX),
            Err(DatasetError::AttributeSelectionOverflow)
        );
    }

    #[test]
    fn attribute_values_extend_checks_type_before_mutating() {
        let mut target = AttributeValues::U8(vec![1, 2]);
        let before = target.clone();
        assert_eq!(
            target.extend_from(&AttributeValues::U16(vec![3])),
            Err(DatasetError::AttributeValuesTypeMismatch {
                expected: AttributeDataType::U8,
                actual: AttributeDataType::U16,
            })
        );
        assert_eq!(target, before);
        target.extend_from(&AttributeValues::U8(vec![3])).unwrap();
        assert_eq!(target, AttributeValues::U8(vec![1, 2, 3]));
    }
}
