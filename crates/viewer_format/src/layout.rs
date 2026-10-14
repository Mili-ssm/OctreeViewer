//! Binary layouts shared by the reader and writer.
use viewer_core::AttributeDataType;

use crate::{BlockCodec, FormatError};

pub const FORMAT_VERSION: u16 = 5;
/// Common header envelope, identified by the final byte of OCTVFRM1.
pub const HEADER_ENVELOPE_VERSION: u16 = 1;

pub(crate) const HEADER_SIZE: u64 = 24;
pub(crate) const FOOTER_SIZE: u64 = 40;
pub const HEADER_MAGIC: &[u8; 8] = b"OCTVFRM1";
pub(crate) const PROVISIONAL_HEADER_MAGIC: &[u8; 8] = b"OCTVWFRM";
pub(crate) const INDEX_MAGIC: &[u8; 8] = b"OCTVIDX5";
pub(crate) const FOOTER_MAGIC: &[u8; 8] = b"OCTVEND5";

/// Only exact implemented identities select a payload layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageLayout {
    V5,
    V6Pre1,
    V6Pre2,
    V6Pre3,
    V6Pre4,
}

impl StorageLayout {
    pub(crate) fn from_version(version: crate::FormatVersion) -> Result<Self, FormatError> {
        match version {
            crate::FormatVersion::V5 => Ok(Self::V5),
            crate::FormatVersion::V6Pre(revision) if revision.get() == 1 => Ok(Self::V6Pre1),
            crate::FormatVersion::V6Pre(revision) if revision.get() == 2 => Ok(Self::V6Pre2),
            crate::FormatVersion::V6Pre(revision) if revision.get() == 3 => Ok(Self::V6Pre3),
            crate::FormatVersion::V6Pre(revision) if revision.get() == 4 => Ok(Self::V6Pre4),
            _ => Err(FormatError::UnsupportedFormatVersion(version)),
        }
    }

    pub(crate) const fn version(self) -> crate::FormatVersion {
        match self {
            Self::V5 => crate::FormatVersion::V5,
            Self::V6Pre1 => crate::FormatVersion::V6_PRE1,
            Self::V6Pre2 => crate::FormatVersion::V6_PRE2,
            Self::V6Pre3 => crate::FormatVersion::V6_PRE3,
            Self::V6Pre4 => crate::FormatVersion::V6_PRE4,
        }
    }

    pub(crate) const fn index_magic(self) -> &'static [u8; 8] {
        match self {
            Self::V5 => INDEX_MAGIC,
            Self::V6Pre1 => b"OCTVI61P",
            Self::V6Pre2 => b"OCTVI62P",
            Self::V6Pre3 => b"OCTVI63P",
            Self::V6Pre4 => b"OCTVI64P",
        }
    }

    pub(crate) const fn footer_magic(self) -> &'static [u8; 8] {
        match self {
            Self::V5 => FOOTER_MAGIC,
            Self::V6Pre1 => b"OCTVE61P",
            Self::V6Pre2 => b"OCTVE62P",
            Self::V6Pre3 => b"OCTVE63P",
            Self::V6Pre4 => b"OCTVE64P",
        }
    }
}

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

pub(crate) fn primitive_width(data_type: AttributeDataType) -> u64 {
    match data_type {
        AttributeDataType::U8 | AttributeDataType::I8 => 1,
        AttributeDataType::U16 | AttributeDataType::I16 => 2,
        AttributeDataType::U32 | AttributeDataType::I32 | AttributeDataType::F32 => 4,
        AttributeDataType::U64 | AttributeDataType::I64 | AttributeDataType::F64 => 8,
    }
}

/// One complete row of a typed column, including all components.
pub(crate) fn attribute_tuple_width(
    descriptor: &viewer_core::AttributeDescriptor,
) -> Result<usize, FormatError> {
    let width = primitive_width(descriptor.data_type()) * u64::from(descriptor.component_count());
    usize::try_from(width).map_err(|_| FormatError::SizeOutOfRange(width))
}
