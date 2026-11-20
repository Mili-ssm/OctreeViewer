//! Binary layouts shared by the reader and writer.
use viewer_core::AttributeDataType;

use crate::{BlockCodec, DisplayColourSource, FormatError};

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
    /// Experimental v8.1-pre: the v7 byte layout plus the v8 header fields and per-block column rules.
    V8Pre1,
    /// Experimental v8.2-pre: v8.1-pre bytes under its own identity; later tasks add the zstd codec.
    V8Pre2,
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
            crate::FormatVersion::V8Pre(revision) if revision.get() == 2 => Ok(Self::V8Pre2),
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
            Self::V8Pre2 => crate::FormatVersion::V8_PRE2,
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
            Self::V8Pre2 => b"OCTVI82P",
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
            Self::V8Pre2 => b"OCTVE82P",
        }
    }

    /// Only the v7 pre-releases up to v7.3 can be resumed block by block.
    pub(crate) const fn supports_block_checkpoint(self) -> bool {
        matches!(self, Self::V7Pre1 | Self::V7Pre2 | Self::V7Pre3)
    }

    /// Global `i64` integer frame, node prefixes, CRS record and `u32` offsets (v7.4-pre onward).
    pub(crate) const fn uses_global_frame(self) -> bool {
        matches!(
            self,
            Self::V7Pre4 | Self::V7Pre5 | Self::V7 | Self::V8Pre1 | Self::V8Pre2
        )
    }

    /// Paged hierarchy: the index is a metadata block plus a root page, and the rest of the
    /// node table lives in child pages (v7.5-pre and v7).
    pub(crate) const fn uses_pages(self) -> bool {
        matches!(self, Self::V7Pre5 | Self::V7 | Self::V8Pre1 | Self::V8Pre2)
    }

    /// Writer policy: occupancy blocks are always compact 24-bit candidates.
    pub(crate) const fn restricts_occupancy_to_compact24(self) -> bool {
        matches!(
            self,
            Self::V7Pre2
                | Self::V7Pre3
                | Self::V7Pre4
                | Self::V7Pre5
                | Self::V7
                | Self::V8Pre1
                | Self::V8Pre2
        )
    }

    /// Writer policy: attribute payloads use the v7.3 dictionary-tuple encoder.
    pub(crate) const fn restricts_dictionary_tuples(self) -> bool {
        matches!(
            self,
            Self::V7Pre3 | Self::V7Pre4 | Self::V7Pre5 | Self::V7 | Self::V8Pre1 | Self::V8Pre2
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
                | Self::V8Pre2
        )
    }

    /// The display colour is derived from stored data instead of being stored twice.
    pub(crate) const fn derives_display_attribute(self) -> bool {
        matches!(self, Self::V8Pre1 | Self::V8Pre2)
    }

    /// Interior LOD nodes omit the attribute columns the viewer does not draw.
    pub(crate) const fn omits_lod_attributes(self) -> bool {
        matches!(self, Self::V8Pre1 | Self::V8Pre2)
    }

    /// Nodes store nearest-neighbour distance quantiles.
    pub(crate) const fn stores_distance_quantiles(self) -> bool {
        matches!(self, Self::V8Pre1 | Self::V8Pre2)
    }

    /// The one rule for which payload columns a block stores. `column` is the payload index
    /// (0 geometry, 1 display colour, then the schema); `block_is_lod` is true for the blocks of
    /// nodes with children. Writers, codecs, readers and verification all ask this and never
    /// assume that every column is stored.
    pub(crate) fn stored_column(
        self,
        block_is_lod: bool,
        column: usize,
        rules: ColumnRules<'_>,
    ) -> StoredColumn {
        // Geometry (0) is always stored; schema column `s` is payload `s + 2`.
        if self.derives_display_attribute() && !block_is_lod && column == 1 {
            return match rules.display {
                DisplayColourSource::None => StoredColumn::Present,
                DisplayColourSource::Rgb(schema) | DisplayColourSource::Intensity(schema) => {
                    StoredColumn::Derived {
                        from: schema as usize + 2,
                    }
                }
            };
        }
        if self.omits_lod_attributes()
            && block_is_lod
            && column >= 2
            && !u32::try_from(column - 2).is_ok_and(|schema| rules.lod_columns.contains(&schema))
        {
            return StoredColumn::Absent;
        }
        StoredColumn::Present
    }

    /// Whether the zstd payload codec may be written (v8.2-pre onward; unused until its codec lands).
    pub(crate) const fn supports_zstd(self) -> bool {
        matches!(self, Self::V8Pre2)
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
                | Self::V8Pre2
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
                | Self::V8Pre2
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
                | Self::V8Pre2
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
                | Self::V8Pre2
        )
    }
}

/// Per-file inputs of [`StorageLayout::stored_column`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ColumnRules<'a> {
    /// Schema indices a LOD block keeps (the header's `lod_attribute_columns`); empty on revisions that
    /// store every column in every block.
    pub(crate) lod_columns: &'a [u32],
    /// Where leaf blocks derive their display colour from (the header's `display_colour_source`).
    pub(crate) display: DisplayColourSource,
}

/// How a block holds one payload column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoredColumn {
    /// The column has a payload in the file.
    Present,
    /// No payload and nothing to derive it from.
    Absent,
    /// No payload; readers rebuild the column from the column at index `from`.
    Derived { from: usize },
}

impl StoredColumn {
    pub(crate) const fn is_present(self) -> bool {
        matches!(self, Self::Present)
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
    /// Absent and derived columns keep their slot (so indices stay `schema.len() + 2`) with zero
    /// lengths and the offset the next stored column would have.
    pub stored: StoredColumn,
}

impl PayloadDirectoryEntry {
    pub(crate) const fn not_stored(stored: StoredColumn, offset: u64) -> Self {
        Self {
            offset,
            stored_length: 0,
            raw_length: 0,
            codec: BlockCodec::None,
            checksum: 0,
            stored,
        }
    }
}

/// Decoded size of the columns a block stores; `stored(column)` says whether payload `column` is.
pub(crate) fn block_payload_size(
    point_count: usize,
    schema: &[viewer_core::AttributeDescriptor],
    stored: impl Fn(usize) -> bool,
) -> Result<u64, FormatError> {
    let point_count = u64::try_from(point_count).map_err(|_| FormatError::PointCountOutOfRange)?;
    let base = [(0, 12_u64), (1, 3)]
        .into_iter()
        .filter(|(column, _)| stored(*column))
        .try_fold(0_u64, |size, (_, width)| {
            point_count
                .checked_mul(width)
                .and_then(|bytes| size.checked_add(bytes))
                .ok_or(FormatError::BlockSizeOverflow)
        })?;
    schema
        .iter()
        .enumerate()
        .filter(|(index, _)| stored(index + 2))
        .try_fold(base, |size, (_, descriptor)| {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_column_is_present_for_v7() {
        for version in crate::FormatVersion::WRITABLE {
            let layout = StorageLayout::from_version(version).unwrap();
            if layout.omits_lod_attributes() {
                continue;
            }
            for block_is_lod in [false, true] {
                for column in 0..12 {
                    assert_eq!(
                        layout.stored_column(block_is_lod, column, ColumnRules::default()),
                        StoredColumn::Present,
                        "{version} lod={block_is_lod} column={column}"
                    );
                }
            }
        }
    }

    #[test]
    fn v8_lod_blocks_store_geometry_colour_and_the_lod_set() {
        use StoredColumn::{Absent, Present};
        let layout = StorageLayout::V8Pre1;
        let rules = ColumnRules {
            lod_columns: &[0, 2],
            display: DisplayColourSource::None,
        };
        for column in 0..6 {
            assert_eq!(
                layout.stored_column(false, column, rules),
                StoredColumn::Present,
                "leaf column {column}"
            );
        }
        let lod: Vec<_> = (0..6)
            .map(|column| layout.stored_column(true, column, rules))
            .collect();
        // Geometry, display colour, schema 0, (schema 1 absent), schema 2, (schema 3 absent).
        assert_eq!(lod, [Present, Present, Present, Absent, Present, Absent]);
    }

    #[test]
    fn v8_leaves_derive_the_display_colour_from_its_source() {
        let layout = StorageLayout::V8Pre1;
        for (display, expected) in [
            (DisplayColourSource::None, StoredColumn::Present),
            (
                DisplayColourSource::Rgb(1),
                StoredColumn::Derived { from: 3 },
            ),
            (
                DisplayColourSource::Intensity(0),
                StoredColumn::Derived { from: 2 },
            ),
        ] {
            let rules = ColumnRules {
                lod_columns: &[0],
                display,
            };
            assert_eq!(layout.stored_column(false, 1, rules), expected);
            // LOD blocks keep their own colour; other revisions never derive.
            assert_eq!(layout.stored_column(true, 1, rules), StoredColumn::Present);
            assert_eq!(
                StorageLayout::V7.stored_column(false, 1, rules),
                StoredColumn::Present
            );
        }
    }

    #[test]
    fn block_payload_size_counts_only_the_stored_columns() {
        let schema = [
            viewer_core::AttributeDescriptor::new("a", AttributeDataType::U16, 1).unwrap(),
            viewer_core::AttributeDescriptor::new("b", AttributeDataType::F32, 2).unwrap(),
        ];
        assert_eq!(
            block_payload_size(10, &schema, |_| true).unwrap(),
            150 + 20 + 80
        );
        assert_eq!(
            block_payload_size(10, &schema, |c| c != 2).unwrap(),
            150 + 80
        );
        assert_eq!(block_payload_size(10, &schema, |c| c < 2).unwrap(), 150);
    }
}
