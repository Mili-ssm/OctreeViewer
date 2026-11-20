//! Binary layouts shared by the reader and writer.
use viewer_core::AttributeDataType;

use crate::{BlockCodec, FormatError};

pub const FORMAT_VERSION: u16 = 5;
/// Common header envelope, identified by the final byte of OCTVFRM1.
pub const HEADER_ENVELOPE_VERSION: u16 = 1;

/// Byte length of the common split-file header envelope.
pub const SPLIT_HEADER_BYTES: u64 = 24;
/// Byte length of the common split-file footer envelope.
pub const SPLIT_FOOTER_BYTES: u64 = 40;
pub(crate) const HEADER_SIZE: u64 = SPLIT_HEADER_BYTES;
pub(crate) const FOOTER_SIZE: u64 = SPLIT_FOOTER_BYTES;
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
    V7Pre1,
    V7Pre2,
    V7Pre3,
    V7Pre4,
    V7Pre5,
    /// Final v7: the v7.5-pre byte layout under its own identity.
    V7,
    /// Experimental v8.1-pre: the v7 byte layout under its own identity; later tasks change the payload.
    V8Pre1,
}

impl StorageLayout {
    pub(crate) fn from_version(version: crate::FormatVersion) -> Result<Self, FormatError> {
        match version {
            crate::FormatVersion::V5 => Ok(Self::V5),
            crate::FormatVersion::V6Pre(revision) if revision.get() == 1 => Ok(Self::V6Pre1),
            crate::FormatVersion::V6Pre(revision) if revision.get() == 2 => Ok(Self::V6Pre2),
            crate::FormatVersion::V6Pre(revision) if revision.get() == 3 => Ok(Self::V6Pre3),
            crate::FormatVersion::V6Pre(revision) if revision.get() == 4 => Ok(Self::V6Pre4),
            crate::FormatVersion::V7Pre(revision) if revision.get() == 1 => Ok(Self::V7Pre1),
            crate::FormatVersion::V7Pre(revision) if revision.get() == 2 => Ok(Self::V7Pre2),
            crate::FormatVersion::V7Pre(revision) if revision.get() == 3 => Ok(Self::V7Pre3),
            crate::FormatVersion::V7Pre(revision) if revision.get() == 4 => Ok(Self::V7Pre4),
            crate::FormatVersion::V7Pre(revision) if revision.get() == 5 => Ok(Self::V7Pre5),
            crate::FormatVersion::V7 => Ok(Self::V7),
            crate::FormatVersion::V8Pre(revision) if revision.get() == 1 => Ok(Self::V8Pre1),
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
            Self::V7Pre1 => crate::FormatVersion::V7_PRE1,
            Self::V7Pre2 => crate::FormatVersion::V7_PRE2,
            Self::V7Pre3 => crate::FormatVersion::V7_PRE3,
            Self::V7Pre4 => crate::FormatVersion::V7_PRE4,
            Self::V7Pre5 => crate::FormatVersion::V7_PRE5,
            Self::V7 => crate::FormatVersion::V7,
            Self::V8Pre1 => crate::FormatVersion::V8_PRE1,
        }
    }

    pub(crate) const fn index_magic(self) -> &'static [u8; 8] {
        match self {
            Self::V5 => INDEX_MAGIC,
            Self::V6Pre1 => b"OCTVI61P",
            Self::V6Pre2 => b"OCTVI62P",
            Self::V6Pre3 => b"OCTVI63P",
            Self::V6Pre4 => b"OCTVI64P",
            Self::V7Pre1 => b"OCTVI71P",
            Self::V7Pre2 => b"OCTVI72P",
            Self::V7Pre3 => b"OCTVI73P",
            Self::V7Pre4 => b"OCTVI74P",
            Self::V7Pre5 => b"OCTVI75P",
            Self::V7 => b"OCTVIDX7",
            Self::V8Pre1 => b"OCTVI81P",
        }
    }

    pub(crate) const fn footer_magic(self) -> &'static [u8; 8] {
        match self {
            Self::V5 => FOOTER_MAGIC,
            Self::V6Pre1 => b"OCTVE61P",
            Self::V6Pre2 => b"OCTVE62P",
            Self::V6Pre3 => b"OCTVE63P",
            Self::V6Pre4 => b"OCTVE64P",
            Self::V7Pre1 => b"OCTVE71P",
            Self::V7Pre2 => b"OCTVE72P",
            Self::V7Pre3 => b"OCTVE73P",
            Self::V7Pre4 => b"OCTVE74P",
            Self::V7Pre5 => b"OCTVE75P",
            Self::V7 => b"OCTVEND7",
            Self::V8Pre1 => b"OCTVE81P",
        }
    }

    /// Only the v7 pre-releases up to v7.3 can be resumed block by block.
    pub(crate) const fn supports_block_checkpoint(self) -> bool {
        matches!(self, Self::V7Pre1 | Self::V7Pre2 | Self::V7Pre3)
    }

    /// Global `i64` integer frame, node prefixes, CRS record and `u32` offsets (v7.4-pre onward).
    pub(crate) const fn uses_global_frame(self) -> bool {
        matches!(self, Self::V7Pre4 | Self::V7Pre5 | Self::V7 | Self::V8Pre1)
    }

    /// Paged hierarchy: the index is a metadata block plus a root page, and the rest of the
    /// node table lives in child pages (v7.5-pre and v7).
    pub(crate) const fn uses_pages(self) -> bool {
        matches!(self, Self::V7Pre5 | Self::V7 | Self::V8Pre1)
    }

    /// Writer policy: occupancy blocks are always compact 24-bit candidates.
    pub(crate) const fn restricts_occupancy_to_compact24(self) -> bool {
        matches!(
            self,
            Self::V7Pre2 | Self::V7Pre3 | Self::V7Pre4 | Self::V7Pre5 | Self::V7 | Self::V8Pre1
        )
    }

    /// Writer policy: attribute payloads use the v7.3 dictionary-tuple encoder.
    pub(crate) const fn restricts_dictionary_tuples(self) -> bool {
        matches!(
            self,
            Self::V7Pre3 | Self::V7Pre4 | Self::V7Pre5 | Self::V7 | Self::V8Pre1
        )
    }

    /// Whether the writer may re-sort a block's rows by Morton key (the hybrid row order).
    pub(crate) const fn supports_hybrid_rows(self) -> bool {
        matches!(
            self,
            Self::V7Pre1
                | Self::V7Pre2
                | Self::V7Pre3
                | Self::V7Pre4
                | Self::V7Pre5
                | Self::V7
                | Self::V8Pre1
        )
    }

    /// The display colour is derived from stored data instead of being stored twice.
    pub(crate) const fn derives_display_attribute(self) -> bool {
        matches!(self, Self::V8Pre1)
    }

    /// Interior LOD nodes omit the attribute columns the viewer does not draw.
    pub(crate) const fn omits_lod_attributes(self) -> bool {
        matches!(self, Self::V8Pre1)
    }

    /// Nodes store nearest-neighbour distance quantiles.
    pub(crate) const fn stores_distance_quantiles(self) -> bool {
        matches!(self, Self::V8Pre1)
    }

    pub(crate) const fn supports_occupancy(self) -> bool {
        matches!(
            self,
            Self::V6Pre2
                | Self::V6Pre3
                | Self::V6Pre4
                | Self::V7Pre1
                | Self::V7Pre2
                | Self::V7Pre3
                | Self::V7Pre4
                | Self::V7Pre5
                | Self::V7
                | Self::V8Pre1
        )
    }

    pub(crate) const fn supports_compact_occupancy(self) -> bool {
        matches!(
            self,
            Self::V6Pre4
                | Self::V7Pre1
                | Self::V7Pre2
                | Self::V7Pre3
                | Self::V7Pre4
                | Self::V7Pre5
                | Self::V7
                | Self::V8Pre1
        )
    }

    pub(crate) const fn supports_dictionary(self) -> bool {
        matches!(
            self,
            Self::V6Pre3
                | Self::V6Pre4
                | Self::V7Pre1
                | Self::V7Pre2
                | Self::V7Pre3
                | Self::V7Pre4
                | Self::V7Pre5
                | Self::V7
                | Self::V8Pre1
        )
    }

    pub(crate) const fn supports_byte_planes(self) -> bool {
        matches!(
            self,
            Self::V6Pre4
                | Self::V7Pre1
                | Self::V7Pre2
                | Self::V7Pre3
                | Self::V7Pre4
                | Self::V7Pre5
                | Self::V7
                | Self::V8Pre1
        )
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
