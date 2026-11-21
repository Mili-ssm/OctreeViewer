use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
};

use viewer_core::{
    AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, HierarchySemantics,
    SpatialNode,
};

use crate::geometry::GeometryEncoding;
use crate::layout::{FOOTER_SIZE, HEADER_SIZE, PayloadDirectoryEntry};
use crate::reader_limits::FormatReaderLimits;
use crate::slice_cursor::SliceCursor;
use crate::version::FileRole;
use crate::{
    BlockCodec, Crc32, FormatError, FormatIdentity, FormatMetadata, FormatVersion, crc32,
    crc32::checksum_matches,
};

pub(crate) fn header_for_role(
    version: FormatVersion,
    role: FileRole,
) -> Result<[u8; HEADER_SIZE as usize], FormatError> {
    FormatIdentity { version, role }.to_header_bytes()
}

// The lint fires only on 64-bit targets (wasm32 pointers are 4 bytes).
#[cfg_attr(
    target_pointer_width = "64",
    expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "the layout hands out the magic as a shared `&'static` constant"
    )
)]
pub(crate) fn footer_bytes(
    magic: &'static [u8; 8],
    index_offset: u64,
    index_length: u64,
    file_length: u64,
    index_crc: u32,
) -> [u8; 40] {
    let mut bytes = [0; 40];
    bytes[..8].copy_from_slice(magic);
    bytes[8..16].copy_from_slice(&index_offset.to_le_bytes());
    bytes[16..24].copy_from_slice(&index_length.to_le_bytes());
    bytes[24..32].copy_from_slice(&file_length.to_le_bytes());
    bytes[32..36].copy_from_slice(&index_crc.to_le_bytes());
    let footer_crc = crc32(&bytes[..36]);
    bytes[36..40].copy_from_slice(&footer_crc.to_le_bytes());
    bytes
}

pub(crate) struct IndexWriter<'a, W> {
    pub(crate) writer: &'a mut W,
    pub(crate) checksum: Crc32,
    pub(crate) length: u64,
}

impl<W: Write> IndexWriter<'_, W> {
    pub(crate) fn write(&mut self, bytes: &[u8]) -> Result<(), FormatError> {
        self.writer.write_all(bytes)?;
        self.checksum.update(bytes);
        self.length = self
            .length
            .checked_add(bytes.len() as u64)
            .ok_or(FormatError::FileSizeOverflow)?;
        Ok(())
    }

    pub(crate) fn u8(&mut self, value: u8) -> Result<(), FormatError> {
        self.write(&[value])
    }

    pub(crate) fn u32(&mut self, value: u32) -> Result<(), FormatError> {
        self.write(&value.to_le_bytes())
    }

    pub(crate) fn u64(&mut self, value: u64) -> Result<(), FormatError> {
        self.write(&value.to_le_bytes())
    }

    pub(crate) fn i64(&mut self, value: i64) -> Result<(), FormatError> {
        self.write(&value.to_le_bytes())
    }

    pub(crate) fn f64(&mut self, value: f64) -> Result<(), FormatError> {
        self.write(&value.to_bits().to_le_bytes())
    }
}

/// Everything before the node table: frame,
/// source count, CRS, sampling policy, semantics, visual scale and schema.
pub(crate) fn write_index_prefix<W: Write>(
    index: &mut IndexWriter<'_, W>,
    metadata: &FormatMetadata,
    frame: &crate::GlobalFrame,
) -> Result<(), FormatError> {
    for coordinate in metadata.dataset().coordinate_frame().origin() {
        index.f64(coordinate)?;
    }
    index.u32(frame.root_level())?;
    for prefix in frame.root_prefix() {
        index.u64(prefix)?;
    }
    for quanta in frame.frame_origin_quanta() {
        index.i64(quanta)?;
    }
    for quanta in frame.data_max_quanta() {
        index.i64(quanta)?;
    }
    if let Some(point_count) = metadata.dataset().hierarchy().stats().source_point_count {
        index.u8(1)?;
        index.u64(point_count)?;
    } else {
        index.u8(0)?;
    }
    write_crs_record(index, frame.crs())?;
    match frame.source_scale_offset() {
        Some(source) => {
            index.u8(1)?;
            for value in source.scale().into_iter().chain(source.offset()) {
                index.f64(value)?;
            }
        }
        None => index.u8(0)?,
    }
    write_string(index, metadata.sampling_policy())?;

    let hierarchy = metadata.dataset().hierarchy();
    index.u8(match hierarchy.semantics() {
        HierarchySemantics::Replacement => 0,
        HierarchySemantics::Additive => 1,
    })?;
    index.f64(metadata.visual_scale())?;
    index.u32(
        u32::try_from(hierarchy.attributes().len()).map_err(|_| FormatError::SchemaTooLarge)?,
    )?;
    for descriptor in hierarchy.attributes() {
        index.u32(
            u32::try_from(descriptor.name().len())
                .map_err(|_| FormatError::StringTooLong(descriptor.name().len()))?,
        )?;
        index.write(descriptor.name().as_bytes())?;
        index.u8(attribute_type_tag(descriptor.data_type()))?;
        index.u32(descriptor.component_count())?;
    }
    Ok(())
}

pub(crate) fn write_crs_record<W: Write>(
    index: &mut IndexWriter<'_, W>,
    crs: &crate::CrsRecord,
) -> Result<(), FormatError> {
    let authority = crs.authority().as_bytes();
    index.u32(authority.len() as u32)?;
    index.write(authority)?;
    index.u32(crs.code())?;
    index.u8(crs.raw_encoding())?;
    let raw = crs.raw_bytes();
    index.u32(u32::try_from(raw.len()).map_err(|_| FormatError::StringTooLong(raw.len()))?)?;
    index.write(&raw)
}

pub(crate) fn write_string<W: Write>(
    index: &mut IndexWriter<'_, W>,
    value: Option<&str>,
) -> Result<(), FormatError> {
    let bytes = value.unwrap_or_default().as_bytes();
    index.u32(u32::try_from(bytes.len()).map_err(|_| FormatError::StringTooLong(bytes.len()))?)?;
    index.write(bytes)
}

pub(crate) fn encode_index_values<W: Write>(
    index: &mut IndexWriter<'_, W>,
    values: &AttributeValues,
) -> Result<(), FormatError> {
    match values {
        AttributeValues::U8(values) => index.write(values)?,
        AttributeValues::I8(values) => {
            index.write(&values.iter().map(|v| *v as u8).collect::<Vec<_>>())?;
        }
        AttributeValues::U16(values) => {
            for value in values {
                index.write(&value.to_le_bytes())?;
            }
        }
        AttributeValues::I16(values) => {
            for value in values {
                index.write(&value.to_le_bytes())?;
            }
        }
        AttributeValues::U32(values) => {
            for value in values {
                index.write(&value.to_le_bytes())?;
            }
        }
        AttributeValues::I32(values) => {
            for value in values {
                index.write(&value.to_le_bytes())?;
            }
        }
        AttributeValues::U64(values) => {
            for value in values {
                index.write(&value.to_le_bytes())?;
            }
        }
        AttributeValues::I64(values) => {
            for value in values {
                index.write(&value.to_le_bytes())?;
            }
        }
        AttributeValues::F32(values) => {
            for value in values {
                index.write(&value.to_bits().to_le_bytes())?;
            }
        }
        AttributeValues::F64(values) => {
            for value in values {
                index.write(&value.to_bits().to_le_bytes())?;
            }
        }
    }
    Ok(())
}

pub(crate) fn attribute_type_tag(data_type: AttributeDataType) -> u8 {
    match data_type {
        AttributeDataType::U8 => 0,
        AttributeDataType::I8 => 1,
        AttributeDataType::U16 => 2,
        AttributeDataType::I16 => 3,
        AttributeDataType::U32 => 4,
        AttributeDataType::I32 => 5,
        AttributeDataType::U64 => 6,
        AttributeDataType::I64 => 7,
        AttributeDataType::F32 => 8,
        AttributeDataType::F64 => 9,
    }
}

// The lint fires only on 64-bit targets (wasm32 pointers are 4 bytes).
#[cfg_attr(
    target_pointer_width = "64",
    expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "the layout hands out the magic as a shared reference"
    )
)]
pub(crate) fn parse_split_footer(
    footer: &[u8; FOOTER_SIZE as usize],
    magic: &[u8; 8],
) -> Result<(u64, u64, u64, u32), FormatError> {
    if &footer[..8] != magic
        || !checksum_matches(
            &footer[..36],
            u32::from_le_bytes(footer[36..40].try_into().expect("fixed width")),
        )
    {
        return Err(FormatError::InvalidFooter);
    }
    Ok((
        u64::from_le_bytes(footer[8..16].try_into().expect("fixed width")),
        u64::from_le_bytes(footer[16..24].try_into().expect("fixed width")),
        u64::from_le_bytes(footer[24..32].try_into().expect("fixed width")),
        u32::from_le_bytes(footer[32..36].try_into().expect("fixed width")),
    ))
}

pub(crate) fn validate_index_range(
    file_length: u64,
    offset: u64,
    length: u64,
    declared: u64,
    limits: FormatReaderLimits,
) -> Result<(), FormatError> {
    if file_length < HEADER_SIZE + FOOTER_SIZE {
        return Err(FormatError::FileTooShort);
    }
    if length > limits.max_index_bytes {
        return Err(FormatError::IndexExceedsLimit {
            requested: length,
            limit: limits.max_index_bytes,
        });
    }
    let footer_offset = file_length - FOOTER_SIZE;
    if declared != file_length
        || offset < HEADER_SIZE
        || offset.checked_add(length) != Some(footer_offset)
    {
        return Err(FormatError::InvalidFooter);
    }
    Ok(())
}

/// Smallest encoded node: id 8, parent tag 1, level 4, bounds 48, point count 8, geometric
/// error 8, block count 4, range count 4.
/// Fields before the node table.
pub(crate) struct IndexPrefix {
    pub(crate) dataset_origin: [f64; 3],
    pub(crate) root_level: u32,
    pub(crate) root_prefix: [u64; 3],
    pub(crate) frame_origin_quanta: [i64; 3],
    pub(crate) data_max_quanta: [i64; 3],
    pub(crate) source_point_count: Option<u64>,
    pub(crate) crs: Option<String>,
    pub(crate) crs_record: Option<crate::CrsRecord>,
    pub(crate) source_scale_offset: Option<crate::SourceScaleOffset>,
    pub(crate) sampling_policy: Option<String>,
    pub(crate) semantics: HierarchySemantics,
    pub(crate) visual_scale: f64,
    pub(crate) schema: Vec<AttributeDescriptor>,
}

pub(crate) fn parse_index_prefix(
    cursor: &mut SliceCursor<'_>,
    limits: &FormatReaderLimits,
) -> Result<IndexPrefix, FormatError> {
    let dataset_origin = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
    let root_level = cursor.u32()?;
    let root_prefix = [cursor.u64()?, cursor.u64()?, cursor.u64()?];
    let frame_origin_quanta = [cursor.i64()?, cursor.i64()?, cursor.i64()?];
    let data_max_quanta = [cursor.i64()?, cursor.i64()?, cursor.i64()?];
    let source_point_count = match cursor.u8()? {
        0 => None,
        1 => Some(cursor.u64()?),
        tag => {
            return Err(FormatError::InvalidTag {
                field: "source count",
                tag,
            });
        }
    };
    let record = read_crs_record(cursor, limits)?;
    let crs = record.identity_label();
    let crs_record = Some(record);
    let source_scale_offset = match cursor.u8()? {
        0 => None,
        1 => {
            let scale = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
            let offset = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
            Some(
                crate::SourceScaleOffset::new(scale, offset)
                    .map_err(|_| FormatError::MalformedIndex("invalid source scale or offset"))?,
            )
        }
        tag => {
            return Err(FormatError::InvalidTag {
                field: "source scale and offset",
                tag,
            });
        }
    };
    let sampling_policy = cursor.string(limits.max_string_bytes)?;
    let semantics = match cursor.u8()? {
        0 => HierarchySemantics::Replacement,
        1 => HierarchySemantics::Additive,
        tag => {
            return Err(FormatError::InvalidTag {
                field: "hierarchy semantics",
                tag,
            });
        }
    };
    let visual_scale = cursor.f64()?;

    let attribute_count = cursor.u32()?;
    if attribute_count > limits.max_attributes {
        return Err(FormatError::CountLimit {
            kind: "attributes",
            requested: u64::from(attribute_count),
            limit: u64::from(limits.max_attributes),
        });
    }
    let mut schema = Vec::new();
    schema
        .try_reserve_exact(attribute_count as usize)
        .map_err(|_| FormatError::AllocationFailed(attribute_count as usize))?;
    for _ in 0..attribute_count {
        let name = cursor.string(limits.max_string_bytes)?.unwrap_or_default();
        let data_type = parse_attribute_type(cursor.u8()?)?;
        let component_count = cursor.u32()?;
        if component_count == 0 || component_count > limits.max_attribute_components {
            return Err(FormatError::InvalidSchema);
        }
        schema.push(AttributeDescriptor::new(name, data_type, component_count)?);
    }
    Ok(IndexPrefix {
        dataset_origin,
        root_level,
        root_prefix,
        frame_origin_quanta,
        data_max_quanta,
        source_point_count,
        crs,
        crs_record,
        source_scale_offset,
        sampling_policy,
        semantics,
        visual_scale,
        schema,
    })
}

/// Codec rules of one payload column (index 0 is geometry, 1 display colors, then the schema).
pub(crate) fn validate_column_codec(
    schema: &[AttributeDescriptor],
    payload_index: usize,
    point_count: u64,
    column: &PayloadDirectoryEntry,
) -> Result<(), FormatError> {
    if matches!(
        column.codec,
        BlockCodec::Constant | BlockCodec::Dictionary | BlockCodec::BytePlanes
    ) {
        let width = match payload_index {
            0 => 0,
            1 => 3,
            _ => {
                let descriptor = &schema[payload_index - 2];
                crate::layout::primitive_width(descriptor.data_type())
                    * u64::from(descriptor.component_count())
            }
        };
        if payload_index == 0
            || point_count < 2
            || column.stored_length >= column.raw_length
            || (column.codec == BlockCodec::Constant && column.stored_length != width)
            || (column.codec == BlockCodec::Dictionary && column.stored_length < 10)
            || (column.codec == BlockCodec::BytePlanes && (width <= 1 || column.stored_length == 0))
        {
            return Err(FormatError::MalformedIndex(
                "attribute codec not valid in this revision or column",
            ));
        }
    }
    Ok(())
}

pub(crate) fn read_crs_record(
    cursor: &mut SliceCursor<'_>,
    limits: &FormatReaderLimits,
) -> Result<crate::CrsRecord, FormatError> {
    let authority_length = cursor.u32()? as usize;
    if authority_length > crate::global_frame::MAX_AUTHORITY_BYTES {
        return Err(FormatError::MalformedIndex("CRS authority is too long"));
    }
    let authority = std::str::from_utf8(cursor.take(authority_length)?)
        .map_err(|_| FormatError::InvalidUtf8)?
        .to_owned();
    let code = cursor.u32()?;
    let encoding = cursor.u8()?;
    let raw_length = cursor.u32()?;
    if raw_length > limits.max_string_bytes {
        return Err(FormatError::StringExceedsLimit {
            requested: raw_length,
            limit: limits.max_string_bytes,
        });
    }
    let raw = cursor.take(raw_length as usize)?;
    crate::CrsRecord::from_parts(authority, code, encoding, raw)
        .map_err(FormatError::MalformedIndex)
}

pub(crate) fn parse_attribute_type(tag: u8) -> Result<AttributeDataType, FormatError> {
    match tag {
        0 => Ok(AttributeDataType::U8),
        1 => Ok(AttributeDataType::I8),
        2 => Ok(AttributeDataType::U16),
        3 => Ok(AttributeDataType::I16),
        4 => Ok(AttributeDataType::U32),
        5 => Ok(AttributeDataType::I32),
        6 => Ok(AttributeDataType::U64),
        7 => Ok(AttributeDataType::I64),
        8 => Ok(AttributeDataType::F32),
        9 => Ok(AttributeDataType::F64),
        tag => Err(FormatError::InvalidTag {
            field: "attribute type",
            tag,
        }),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BlockDirectoryEntry {
    pub(crate) offset: u64,
    pub(crate) payload_length: u64,
    pub(crate) point_count: u64,
    pub(crate) quantization_error: f64,
    /// Block origin in quanta and the integer box of its node.
    pub(crate) origin_quanta: [i64; 3],
    pub(crate) node_cell: Option<crate::global_frame::CellBox>,
    pub(crate) geometry_encoding: GeometryEncoding,
    pub(crate) payloads: Vec<PayloadDirectoryEntry>,
}

pub(crate) type ParsedIndex = (FormatMetadata, BTreeMap<BlockId, BlockDirectoryEntry>);

pub(crate) fn validate_block_layout(
    nodes: &[SpatialNode],
    blocks: &BTreeMap<BlockId, BlockDirectoryEntry>,
    index_offset: u64,
) -> Result<(), FormatError> {
    let referenced: BTreeSet<_> = nodes
        .iter()
        .flat_map(|node| node.blocks().iter().copied())
        .collect();
    if referenced.len() != blocks.len() || referenced.iter().any(|id| !blocks.contains_key(id)) {
        return Err(FormatError::MalformedIndex(
            "block directory and node references disagree",
        ));
    }
    let mut by_offset = blocks.iter().collect::<Vec<_>>();
    by_offset.sort_unstable_by_key(|(id, entry)| (entry.offset, entry.payload_length, **id));
    let mut expected_offset = HEADER_SIZE;
    for (_, entry) in by_offset {
        if entry.offset != expected_offset {
            return Err(FormatError::MalformedIndex(
                "block payloads overlap or leave gaps",
            ));
        }
        expected_offset = expected_offset
            .checked_add(entry.payload_length)
            .ok_or(FormatError::FileSizeOverflow)?;
    }
    if expected_offset != index_offset {
        return Err(FormatError::MalformedIndex(
            "block payload section does not end at the index",
        ));
    }
    Ok(())
}

pub(crate) fn validate_node_point_counts(
    nodes: &[SpatialNode],
    blocks: &BTreeMap<BlockId, BlockDirectoryEntry>,
) -> Result<(), FormatError> {
    for node in nodes {
        let count = node.blocks().iter().try_fold(0_u64, |sum, id| {
            blocks
                .get(id)
                .and_then(|entry| sum.checked_add(entry.point_count))
                .ok_or(FormatError::PointCountOverflow(node.id()))
        })?;
        if count != node.point_count() {
            return Err(FormatError::NodePointCountMismatch {
                node: node.id(),
                declared: node.point_count(),
                blocks: count,
            });
        }
    }
    Ok(())
}

pub(crate) fn validate_node_error_bounds(
    nodes: &[SpatialNode],
    blocks: &BTreeMap<BlockId, BlockDirectoryEntry>,
) -> Result<(), FormatError> {
    for node in nodes {
        let required = node.blocks().iter().try_fold(0.0_f64, |maximum, id| {
            blocks
                .get(id)
                .map(|entry| maximum.max(entry.quantization_error))
                .ok_or(FormatError::UnreferencedBlock(*id))
        })?;
        if node.geometric_error() < required {
            return Err(FormatError::MalformedIndex(
                "node geometric error excludes block quantization error",
            ));
        }
    }
    Ok(())
}
