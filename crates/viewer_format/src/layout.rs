//! Binary layouts shared by the reader and writer.
use viewer_core::AttributeDataType;

use crate::{BlockCodec, DisplayColourSource, FormatError};

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

/// Only the exact implemented identity selects a payload layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageLayout {
    V8,
}

#[allow(
    clippy::unused_self,
    reason = "methods keep call sites naming the layout they read, should a second one return"
)]
impl StorageLayout {
    pub(crate) fn from_version(version: crate::FormatVersion) -> Result<Self, FormatError> {
        match version {
            crate::FormatVersion::V8 => Ok(Self::V8),
            crate::FormatVersion::Unknown { .. } => {
                Err(FormatError::UnsupportedFormatVersion(version))
            }
        }
    }

    pub(crate) const fn version(self) -> crate::FormatVersion {
        crate::FormatVersion::V8
    }

    pub(crate) const fn index_magic(self) -> &'static [u8; 8] {
        b"OCTVIDX8"
    }

    pub(crate) const fn footer_magic(self) -> &'static [u8; 8] {
        b"OCTVEND8"
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
        if !block_is_lod && column == 1 {
            return match rules.display {
                DisplayColourSource::None => StoredColumn::Present,
                DisplayColourSource::Rgb(schema) | DisplayColourSource::Intensity(schema) => {
                    StoredColumn::Derived {
                        from: schema as usize + 2,
                    }
                }
            };
        }
        if block_is_lod
            && column >= 2
            && !u32::try_from(column - 2).is_ok_and(|schema| rules.lod_columns.contains(&schema))
        {
            return StoredColumn::Absent;
        }
        StoredColumn::Present
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
    fn v8_lod_blocks_store_geometry_colour_and_the_lod_set() {
        use StoredColumn::{Absent, Present};
        let layout = StorageLayout::V8;
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
        let layout = StorageLayout::V8;
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
            // LOD blocks keep their own colour.
            assert_eq!(layout.stored_column(true, 1, rules), StoredColumn::Present);
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
