//! Binary layout shared by the reader and writer. Only the current pre-release version is supported.
use viewer_core::AttributeDataType;

use crate::{BlockCodec, FormatError};

pub const FORMAT_VERSION: u16 = 5;

pub(crate) const HEADER_SIZE: u64 = 24;
pub(crate) const FOOTER_SIZE: u64 = 40;
pub(crate) const HEADER_MAGIC: &[u8; 8] = b"OCTVWFRM";
pub(crate) const INDEX_MAGIC: &[u8; 8] = b"OCTVIDX5";
pub(crate) const FOOTER_MAGIC: &[u8; 8] = b"OCTVEND5";

/// One independently compressed payload. Offsets are relative to the primary block, or absolute in a split companion.
#[derive(Debug, Clone)]
pub(crate) struct PayloadDirectoryEntry {
    pub offset: u64,
    pub stored_length: u64,
    pub raw_length: u64,
    pub codec: BlockCodec,
    pub checksum: u32,
}

pub(crate) fn block_payload_size(
    point_count: usize,
    schema: &[viewer_core::AttributeDescriptor],
) -> Result<u64, FormatError> {
    let point_count = u64::try_from(point_count).map_err(|_| FormatError::PointCountOutOfRange)?;
    let base = point_count
        .checked_mul(15)
        .ok_or(FormatError::BlockSizeOverflow)?;
    schema.iter().try_fold(base, |size, descriptor| {
        let attribute_size = attribute_payload_size(point_count, descriptor)?;
        size.checked_add(attribute_size)
            .ok_or(FormatError::BlockSizeOverflow)
    })
}

pub(crate) fn attribute_payload_size(
    point_count: u64,
    descriptor: &viewer_core::AttributeDescriptor,
) -> Result<u64, FormatError> {
    point_count
        .checked_mul(u64::from(descriptor.component_count()))
        .and_then(|count| count.checked_mul(primitive_width(descriptor.data_type())))
        .ok_or(FormatError::BlockSizeOverflow)
}

fn primitive_width(data_type: AttributeDataType) -> u64 {
    match data_type {
        AttributeDataType::U8 | AttributeDataType::I8 => 1,
        AttributeDataType::U16 | AttributeDataType::I16 => 2,
        AttributeDataType::U32 | AttributeDataType::I32 | AttributeDataType::F32 => 4,
        AttributeDataType::U64 | AttributeDataType::I64 | AttributeDataType::F64 => 8,
    }
}

pub(crate) const SPLIT_ATTRIBUTES: u32 = 1;
pub(crate) const ATTRIBUTE_FILE: u32 = 2;
