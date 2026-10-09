use thiserror::Error;

use crate::{
    AttributeDataType, AttributeDescriptor, BlockId, CoordinateFrame, PointBatch, SpatialHierarchy,
};

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

    fn len(&self) -> usize {
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
        let expected_len = point_count
            .checked_mul(descriptor.component_count() as usize)
            .ok_or_else(|| DatasetError::AttributeLengthOverflow {
                name: descriptor.name().to_owned(),
            })?;
        let actual_len = values.len();
        if actual_len != expected_len {
            return Err(DatasetError::InvalidAttributeLength {
                name: descriptor.name().to_owned(),
                expected: expected_len,
                actual: actual_len,
            });
        }
        Ok(Self { descriptor, values })
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

    pub fn attributes(&self) -> &[AttributeColumn] {
        &self.attributes
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
}
