use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
};

use viewer_core::{
    AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, CoordinateFrame,
    DatasetMetadata, HierarchySemantics, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};

use crate::geometry::GeometryEncoding;
use crate::layout::{
    FOOTER_SIZE, HEADER_SIZE, PayloadDirectoryEntry, StorageLayout, StoredColumn,
    attribute_payload_size, block_payload_size,
};
use crate::metadata::validate_metadata;
use crate::prepare::BlockRecord;
use crate::reader::read_attribute_values;
use crate::reader_limits::FormatReaderLimits;
use crate::slice_cursor::SliceCursor;
use crate::version::FileRole;
use crate::{
    BlockCodec, Crc32, FormatError, FormatIdentity, FormatMetadata, FormatVersion,
    NodeAttributeRange, crc32, crc32::checksum_matches,
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

pub(crate) fn write_index<W: Write>(
    writer: &mut W,
    metadata: &FormatMetadata,
    blocks: &BTreeMap<BlockId, BlockRecord>,
    layout: StorageLayout,
) -> Result<(u64, u32), FormatError> {
    let mut index = IndexWriter {
        writer,
        checksum: Crc32::new(),
        length: 0,
    };
    let global = if layout.uses_global_frame() {
        Some(metadata.global_frame().ok_or(FormatError::InvalidMetadata(
            "v7.4-pre needs a global frame in the metadata",
        ))?)
    } else {
        None
    };
    index.write(layout.index_magic())?;
    write_index_prefix(&mut index, metadata, global)?;
    let hierarchy = metadata.dataset().hierarchy();

    let root = hierarchy
        .node(hierarchy.root_id())
        .ok_or(FormatError::InvalidMetadata("root node is missing"))?;
    let mut nodes = hierarchy.query(root.bounds()).collect::<Vec<_>>();
    nodes.sort_unstable_by_key(|node| node.id());
    index.u64(u64::try_from(nodes.len()).map_err(|_| FormatError::TooManyNodes)?)?;
    for node in nodes {
        index.u64(node.id().get())?;
        if let Some(parent) = node.parent() {
            index.u8(1)?;
            index.u64(parent.get())?;
        } else {
            index.u8(0)?;
        }
        index.u32(node.level())?;
        let cell = global
            .map(|frame| {
                frame
                    .node_cell(node.id())
                    .ok_or(FormatError::UnknownNode(node.id()))
            })
            .transpose()?;
        if let Some(cell) = cell {
            for prefix in cell.prefix() {
                index.u64(prefix)?;
            }
        }
        for coordinate in node.bounds().min().into_iter().chain(node.bounds().max()) {
            index.f64(coordinate)?;
        }
        index.u64(node.point_count())?;
        index.f64(node.geometric_error())?;
        if let Some(cell) = cell {
            index.f64(cell.spacing())?;
        }
        let mut block_ids = node.blocks().to_vec();
        block_ids.sort_unstable();
        index.u32(
            u32::try_from(block_ids.len())
                .map_err(|_| FormatError::TooManyNodeBlocks(node.id()))?,
        )?;
        for block_id in block_ids {
            index.u64(block_id.get())?;
        }
        let mut ranges = metadata
            .node_attribute_ranges(node.id())
            .iter()
            .collect::<Vec<_>>();
        ranges.sort_unstable_by_key(|range| range.schema_index());
        index.u32(
            u32::try_from(ranges.len())
                .map_err(|_| FormatError::TooManyAttributeRanges(node.id()))?,
        )?;
        for range in ranges {
            index.u32(range.schema_index())?;
            encode_index_values(&mut index, range.min())?;
            encode_index_values(&mut index, range.max())?;
        }
    }

    index.u64(u64::try_from(blocks.len()).map_err(|_| FormatError::TooManyBlocks)?)?;
    for (id, block) in blocks {
        index.u64(id.get())?;
        index.u64(block.offset)?;
        index.u64(block.payload_length)?;
        index.u64(block.point_count)?;
        if layout != StorageLayout::V5 {
            index.u8(block.geometry_encoding as u8)?;
        }
        index.f64(block.quantization_error)?;
        if global.is_some() {
            for value in block.origin_quanta {
                index.i64(value)?;
            }
        } else {
            for value in block.origin {
                index.f64(value)?;
            }
            for value in block.scale {
                index.f64(value)?;
            }
        }
        // Reserved aggregate fields in the v5 layout; codecs and CRCs belong to payloads.
        index.u8(0)?;
        index.u32(0)?;
        for payload in &block.payloads {
            // The whole-index layouts have no way to say that a column is missing.
            if !payload.stored.is_present() {
                return Err(FormatError::InvalidMetadata(
                    "this index layout stores every column",
                ));
            }
            index.u64(payload.offset)?;
            index.u64(payload.stored_length)?;
            index.u64(payload.raw_length)?;
            index.u8(payload.codec as u8)?;
            index.u32(payload.checksum)?;
        }
    }
    let length = index.length;
    let checksum = index.checksum.finish();
    Ok((length, checksum))
}

/// Everything before the node table, shared by the whole-index and the paged layouts: frame,
/// source count, CRS, sampling policy, semantics, visual scale and schema.
pub(crate) fn write_index_prefix<W: Write>(
    index: &mut IndexWriter<'_, W>,
    metadata: &FormatMetadata,
    global: Option<&crate::GlobalFrame>,
) -> Result<(), FormatError> {
    for coordinate in metadata.dataset().coordinate_frame().origin() {
        index.f64(coordinate)?;
    }
    if let Some(frame) = global {
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
    }
    if let Some(point_count) = metadata.dataset().hierarchy().stats().source_point_count {
        index.u8(1)?;
        index.u64(point_count)?;
    } else {
        index.u8(0)?;
    }
    if let Some(frame) = global {
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
    } else {
        write_string(index, metadata.crs())?;
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
const MIN_NODE_RECORD_LEN: usize = 85;
/// `v7.4-pre` adds the prefix (3 x u64) and the spacing (f64).
const GLOBAL_NODE_EXTRA_LEN: usize = 32;

/// Fields before the node table, shared by the whole-index and the paged layouts.
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
    global: bool,
) -> Result<IndexPrefix, FormatError> {
    let dataset_origin = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
    let (root_level, root_prefix, frame_origin_quanta, data_max_quanta) = if global {
        (
            cursor.u32()?,
            [cursor.u64()?, cursor.u64()?, cursor.u64()?],
            [cursor.i64()?, cursor.i64()?, cursor.i64()?],
            [cursor.i64()?, cursor.i64()?, cursor.i64()?],
        )
    } else {
        (0, [0; 3], [0; 3], [0; 3])
    };
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
    let crs;
    let mut crs_record = None;
    let mut source_scale_offset = None;
    if global {
        let record = read_crs_record(cursor, limits)?;
        crs = record.identity_label();
        crs_record = Some(record);
        source_scale_offset =
            match cursor.u8()? {
                0 => None,
                1 => {
                    let scale = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
                    let offset = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
                    Some(crate::SourceScaleOffset::new(scale, offset).map_err(|_| {
                        FormatError::MalformedIndex("invalid source scale or offset")
                    })?)
                }
                tag => {
                    return Err(FormatError::InvalidTag {
                        field: "source scale and offset",
                        tag,
                    });
                }
            };
    } else {
        crs = cursor.string(limits.max_string_bytes)?;
    }
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
    layout: StorageLayout,
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
        if !layout.supports_dictionary()
            || payload_index == 0
            || point_count < 2
            || column.stored_length >= column.raw_length
            || (column.codec == BlockCodec::Constant && column.stored_length != width)
            || (column.codec == BlockCodec::Dictionary && column.stored_length < 10)
            || (column.codec == BlockCodec::BytePlanes
                && (!layout.supports_byte_planes() || width <= 1 || column.stored_length == 0))
        {
            return Err(FormatError::MalformedIndex(
                "attribute codec not valid in this revision or column",
            ));
        }
    }
    Ok(())
}

/// Range a block occupies in the split companion: from the first stored column (a derived display
/// colour has no payload of its own, its slot only carries an offset) to the furthest end of a
/// stored column. A column that is not stored has no bytes, whatever its offset.
fn companion_range(block: &BlockDirectoryEntry) -> Result<(u64, u64), FormatError> {
    let mut stored = block
        .payloads
        .iter()
        .skip(1)
        .filter(|payload| payload.stored.is_present())
        .peekable();
    let Some(start) = stored.peek().map(|payload| payload.offset) else {
        let offset = block.payloads[1].offset;
        return Ok((offset, offset));
    };
    let mut start = start;
    let mut end = start;
    for payload in stored {
        start = start.min(payload.offset);
        let next = payload
            .offset
            .checked_add(payload.stored_length)
            .ok_or(FormatError::FileSizeOverflow)?;
        end = end.max(next);
    }
    Ok((start, end))
}

pub(crate) fn parse_index(
    bytes: &[u8],
    limits: &FormatReaderLimits,
    index_offset: u64,
    split: bool,
    layout: StorageLayout,
) -> Result<ParsedIndex, FormatError> {
    if layout.uses_pages() {
        // Paged indexes are read page by page (`paged_index`), never as one buffer.
        return Err(FormatError::UnsupportedFormatVersion(layout.version()));
    }
    let mut cursor = SliceCursor::new(bytes);
    if cursor.take(8)? != layout.index_magic() {
        return Err(FormatError::MalformedIndex("invalid index magic"));
    }
    let IndexPrefix {
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
    } = parse_index_prefix(&mut cursor, limits, layout.uses_global_frame())?;
    let global = layout.uses_global_frame();
    let attribute_count = schema.len() as u32;

    let node_count = cursor.u64()?;
    if node_count == 0 || node_count > limits.max_nodes {
        return Err(FormatError::CountLimit {
            kind: "nodes",
            requested: node_count,
            limit: limits.max_nodes,
        });
    }
    let node_capacity = usize::try_from(node_count).map_err(|_| FormatError::TooManyNodes)?;
    // Reserving before this check would let a tiny index force ~100 B per declared node.
    let min_node_record_len = MIN_NODE_RECORD_LEN + if global { GLOBAL_NODE_EXTRA_LEN } else { 0 };
    if node_capacity.saturating_mul(min_node_record_len) > cursor.remaining() {
        return Err(FormatError::TruncatedIndex);
    }
    let mut nodes = Vec::new();
    nodes
        .try_reserve_exact(node_capacity)
        .map_err(|_| FormatError::AllocationFailed(node_capacity))?;
    let mut node_ranges = Vec::new();
    node_ranges
        .try_reserve_exact(node_capacity)
        .map_err(|_| FormatError::AllocationFailed(node_capacity))?;
    let mut node_cells = Vec::new();
    let mut previous_node_id = None;
    let mut total_ranges = 0_u64;
    for _ in 0..node_count {
        let id = NodeId::new(cursor.u64()?);
        if previous_node_id.is_some_and(|previous| previous >= id) {
            return Err(FormatError::MalformedIndex("nodes are not uniquely sorted"));
        }
        previous_node_id = Some(id);
        let parent = match cursor.u8()? {
            0 => None,
            1 => Some(NodeId::new(cursor.u64()?)),
            tag => {
                return Err(FormatError::InvalidTag {
                    field: "node parent",
                    tag,
                });
            }
        };
        let level = cursor.u32()?;
        let prefix = if global {
            [cursor.u64()?, cursor.u64()?, cursor.u64()?]
        } else {
            [0; 3]
        };
        let min = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
        let max = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
        let bounds = SpatialBounds::new(min, max)?;
        let point_count = cursor.u64()?;
        let geometric_error = cursor.f64()?;
        let spacing = if global { cursor.f64()? } else { 0.0 };
        let block_count = cursor.u32()?;
        let block_count_usize = block_count as usize;
        if u64::from(block_count) > limits.max_blocks {
            return Err(FormatError::CountLimit {
                kind: "node block references",
                requested: u64::from(block_count),
                limit: limits.max_blocks,
            });
        }
        let mut block_ids = Vec::new();
        block_ids
            .try_reserve_exact(block_count_usize)
            .map_err(|_| FormatError::AllocationFailed(block_count_usize))?;
        let mut previous_block = None;
        for _ in 0..block_count {
            let block_id = BlockId::new(cursor.u64()?);
            if previous_block.is_some_and(|previous| previous >= block_id) {
                return Err(FormatError::MalformedIndex(
                    "node block IDs are not uniquely sorted",
                ));
            }
            previous_block = Some(block_id);
            block_ids.push(block_id);
        }

        let range_count = cursor.u32()?;
        if range_count > attribute_count {
            return Err(FormatError::MalformedIndex(
                "too many node attribute ranges",
            ));
        }
        total_ranges = total_ranges.saturating_add(u64::from(range_count));
        if total_ranges > limits.max_attribute_ranges {
            return Err(FormatError::CountLimit {
                kind: "node attribute ranges",
                requested: total_ranges,
                limit: limits.max_attribute_ranges,
            });
        }
        let mut ranges = Vec::new();
        ranges
            .try_reserve_exact(range_count as usize)
            .map_err(|_| FormatError::AllocationFailed(range_count as usize))?;
        let mut previous_schema_index = None;
        for _ in 0..range_count {
            let schema_index = cursor.u32()?;
            if schema_index >= attribute_count
                || previous_schema_index.is_some_and(|previous| previous >= schema_index)
            {
                return Err(FormatError::MalformedIndex(
                    "node attribute ranges are not uniquely sorted",
                ));
            }
            previous_schema_index = Some(schema_index);
            let descriptor = &schema[schema_index as usize];
            let value_count = descriptor.component_count() as usize;
            let min_values =
                read_attribute_values(&mut cursor, descriptor.data_type(), value_count)?;
            let max_values =
                read_attribute_values(&mut cursor, descriptor.data_type(), value_count)?;
            ranges.push(NodeAttributeRange::new(
                schema_index,
                min_values,
                max_values,
            ));
        }
        if point_count == 0 && !ranges.is_empty() {
            return Err(FormatError::MalformedIndex(
                "empty node has attribute ranges",
            ));
        }
        nodes.push(SpatialNode::new(
            id,
            parent,
            level,
            bounds,
            point_count,
            geometric_error,
            block_ids,
        )?);
        node_ranges.push((id, ranges));
        if global {
            node_cells.push((id, crate::NodeCell::new(prefix, spacing)));
        }
    }
    let global_frame = match crs_record {
        Some(crs) if global => {
            let mut frame = crate::GlobalFrame::new(
                root_level,
                root_prefix,
                frame_origin_quanta,
                data_max_quanta,
                crs,
                source_scale_offset,
                node_cells,
            )
            .map_err(|_| {
                FormatError::MalformedIndex(
                    "invalid root level, prefix, frame origin or data maximum",
                )
            })?;
            let node_refs: Vec<&SpatialNode> = nodes.iter().collect();
            crate::global_frame::validate_nodes(&frame, dataset_origin, &node_refs)
                .map_err(FormatError::MalformedIndex)?;
            frame.bind_blocks(&node_refs).map_err(|_| {
                FormatError::MalformedIndex("a block belongs to more than one node")
            })?;
            Some(frame)
        }
        _ => None,
    };

    let block_count = cursor.u64()?;
    if block_count > limits.max_blocks {
        return Err(FormatError::CountLimit {
            kind: "blocks",
            requested: block_count,
            limit: limits.max_blocks,
        });
    }
    let block_capacity = usize::try_from(block_count).map_err(|_| FormatError::TooManyBlocks)?;
    let mut blocks = BTreeMap::new();
    let mut previous_block_id = None;
    for _ in 0..block_count {
        let id = BlockId::new(cursor.u64()?);
        if previous_block_id.is_some_and(|previous| previous >= id) {
            return Err(FormatError::MalformedIndex(
                "block directory is not uniquely sorted",
            ));
        }
        previous_block_id = Some(id);
        let offset = cursor.u64()?;
        let payload_length = cursor.u64()?;
        let point_count = cursor.u64()?;
        let geometry_encoding = if layout == StorageLayout::V5 {
            GeometryEncoding::RawU32
        } else {
            GeometryEncoding::try_from(cursor.u8()?)?
        };
        if geometry_encoding == GeometryEncoding::Occupied && !layout.supports_occupancy() {
            return Err(FormatError::MalformedIndex(
                "geometry encoding not supported in this revision",
            ));
        }
        if geometry_encoding == GeometryEncoding::CompactOccupied
            && !layout.supports_compact_occupancy()
        {
            return Err(FormatError::MalformedIndex(
                "compact geometry not supported in this revision",
            ));
        }
        let quantization_error = cursor.f64()?;
        let mut origin_quanta = [0_i64; 3];
        let mut node_cell = None;
        let (block_origin, scale) = if let Some(frame) = &global_frame {
            origin_quanta = [cursor.i64()?, cursor.i64()?, cursor.i64()?];
            let cell = frame
                .block_box(id)
                .ok_or(FormatError::MalformedIndex("block has no owning node"))?;
            if !crate::global_frame::block_origin_is_valid(cell, origin_quanta) {
                return Err(FormatError::MalformedIndex(
                    "block origin does not follow the node origin or split rule",
                ));
            }
            node_cell = Some(cell);
            ([0.0; 3], [0.0; 3])
        } else {
            (
                [cursor.f64()?, cursor.f64()?, cursor.f64()?],
                [cursor.f64()?, cursor.f64()?, cursor.f64()?],
            )
        };
        if cursor.u8()? != 0 || cursor.u32()? != 0 {
            return Err(FormatError::MalformedIndex("nonzero reserved block fields"));
        }
        if !quantization_error.is_finite()
            || quantization_error < 0.0
            || block_origin.iter().any(|value| !value.is_finite())
            || scale.iter().any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(FormatError::MalformedIndex(
                "invalid block directory values",
            ));
        }
        if payload_length > limits.max_block_bytes {
            return Err(FormatError::BlockExceedsLimit {
                requested: payload_length,
                limit: limits.max_block_bytes,
            });
        }
        let point_count_usize =
            usize::try_from(point_count).map_err(|_| FormatError::PointCountOutOfRange)?;
        // Whole-index layouts store every column.
        let logical_size = block_payload_size(point_count_usize, &schema, |_| true)?;
        if logical_size > limits.max_block_bytes {
            return Err(FormatError::BlockExceedsLimit {
                requested: logical_size,
                limit: limits.max_block_bytes,
            });
        }
        if offset < HEADER_SIZE
            || offset
                .checked_add(payload_length)
                .is_none_or(|end| end > index_offset)
        {
            return Err(FormatError::MalformedIndex(
                "block payload range is outside the data section",
            ));
        }
        let mut payloads = Vec::new();
        payloads
            .try_reserve_exact(schema.len() + 2)
            .map_err(|_| FormatError::AllocationFailed(schema.len() + 2))?;
        let geometry_length = point_count
            .checked_mul(12)
            .ok_or(FormatError::BlockSizeOverflow)?;
        let mut expected_offset = 0;
        for (payload_index, raw_length) in [
            Ok(geometry_length),
            point_count
                .checked_mul(3)
                .ok_or(FormatError::BlockSizeOverflow),
        ]
        .into_iter()
        .chain(
            schema
                .iter()
                .map(|descriptor| attribute_payload_size(point_count, descriptor)),
        )
        .enumerate()
        {
            let expected_raw_length = raw_length?;
            let column = PayloadDirectoryEntry {
                offset: cursor.u64()?,
                stored_length: cursor.u64()?,
                raw_length: cursor.u64()?,
                codec: BlockCodec::try_from(cursor.u8()?)?,
                checksum: cursor.u32()?,
                stored: StoredColumn::Present,
            };
            validate_column_codec(layout, &schema, payload_index, point_count, &column)?;
            if split && payload_index > 0 {
                if payload_index == 1 {
                    expected_offset = column.offset;
                }
                if column.offset < HEADER_SIZE {
                    return Err(FormatError::MalformedIndex(
                        "attribute offset before header",
                    ));
                }
            }
            if column.offset != expected_offset
                || (payload_index != 0 && column.raw_length != expected_raw_length)
                || (payload_index == 0
                    && geometry_encoding
                        .validate_raw_length(point_count, column.raw_length)
                        .is_err())
                || (column.codec == BlockCodec::None && column.stored_length != column.raw_length)
                || (column.codec == BlockCodec::Lz4 && column.stored_length >= column.raw_length)
            {
                return Err(FormatError::MalformedIndex(
                    "invalid payload range or length",
                ));
            }
            expected_offset = expected_offset
                .checked_add(column.stored_length)
                .ok_or(FormatError::FileSizeOverflow)?;
            if (!split || payload_index == 0) && expected_offset > payload_length {
                return Err(FormatError::MalformedIndex(
                    "column extends beyond its block",
                ));
            }
            payloads.push(column);
        }
        let total_raw_length = payloads.iter().try_fold(0_u64, |sum, payload| {
            sum.checked_add(payload.raw_length)
                .ok_or(FormatError::BlockSizeOverflow)
        })?;
        if total_raw_length > limits.max_block_bytes {
            return Err(FormatError::BlockExceedsLimit {
                requested: total_raw_length,
                limit: limits.max_block_bytes,
            });
        }
        if (!split && expected_offset != payload_length)
            || (split && payloads[0].stored_length != payload_length)
        {
            return Err(FormatError::MalformedIndex(
                "column payload section does not fill its block",
            ));
        }
        blocks.insert(
            id,
            BlockDirectoryEntry {
                offset,
                payload_length,
                point_count,
                quantization_error,
                origin: block_origin,
                scale,
                origin_quanta,
                node_cell,
                geometry_encoding,
                payloads,
            },
        );
    }
    if cursor.remaining() != 0 {
        return Err(FormatError::MalformedIndex("trailing index bytes"));
    }
    if blocks.len() != block_capacity {
        return Err(FormatError::MalformedIndex("duplicate block IDs"));
    }

    if split {
        let mut ranges = blocks
            .values()
            .map(companion_range)
            .collect::<Result<Vec<_>, FormatError>>()?;
        ranges.sort_unstable();
        let mut end = HEADER_SIZE;
        for (start, next) in ranges {
            if start != end {
                return Err(FormatError::MalformedIndex(
                    "attribute blocks contain gaps or overlaps",
                ));
            }
            end = next;
        }
    }
    validate_block_layout(&nodes, &blocks, index_offset)?;
    validate_node_point_counts(&nodes, &blocks)?;
    validate_node_error_bounds(&nodes, &blocks)?;
    let mut block_bounds = BTreeMap::<BlockId, Vec<SpatialBounds>>::new();
    for node in &nodes {
        for block_id in node.blocks() {
            block_bounds
                .entry(*block_id)
                .or_default()
                .push(node.bounds());
        }
    }
    let hierarchy =
        SpatialHierarchy::new_with_semantics(nodes, schema, source_point_count, semantics)?;
    let mut metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new(dataset_origin)?, hierarchy),
        crs,
        sampling_policy,
        node_ranges,
    )?
    .with_visual_scale(visual_scale)?;
    if let Some(frame) = global_frame {
        metadata = metadata.with_global_frame(frame)?;
    }
    validate_metadata(&metadata)?;
    Ok((metadata, blocks, block_bounds))
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
    pub(crate) origin: [f64; 3],
    pub(crate) scale: [f64; 3],
    /// `v7.4-pre` block origin in quanta and the integer box of its node; zero/`None` otherwise.
    pub(crate) origin_quanta: [i64; 3],
    pub(crate) node_cell: Option<crate::global_frame::CellBox>,
    pub(crate) geometry_encoding: GeometryEncoding,
    pub(crate) payloads: Vec<PayloadDirectoryEntry>,
}

pub(crate) type ParsedIndex = (
    FormatMetadata,
    BTreeMap<BlockId, BlockDirectoryEntry>,
    BTreeMap<BlockId, Vec<SpatialBounds>>,
);

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

#[cfg(test)]
mod companion_range_tests {
    use super::*;

    fn payload(offset: u64, length: u64, stored: StoredColumn) -> PayloadDirectoryEntry {
        PayloadDirectoryEntry {
            stored_length: length,
            raw_length: length,
            ..PayloadDirectoryEntry::not_stored(stored, offset)
        }
    }

    #[test]
    fn an_absent_last_column_does_not_move_the_range_end() {
        let block = BlockDirectoryEntry {
            offset: 0,
            payload_length: 12,
            point_count: 1,
            quantization_error: 0.0,
            origin: [0.0; 3],
            scale: [1.0; 3],
            origin_quanta: [0; 3],
            node_cell: None,
            geometry_encoding: GeometryEncoding::RawU32,
            payloads: vec![
                payload(0, 12, StoredColumn::Present),
                payload(100, 3, StoredColumn::Present),
                payload(103, 5, StoredColumn::Present),
                // Absent last column with a stale offset: ignored.
                PayloadDirectoryEntry::not_stored(StoredColumn::Absent, 5000),
            ],
        };
        assert_eq!(companion_range(&block).unwrap(), (100, 108));
    }

    #[test]
    fn a_derived_display_colour_does_not_start_the_range() {
        let block = BlockDirectoryEntry {
            offset: 0,
            payload_length: 12,
            point_count: 1,
            quantization_error: 0.0,
            origin: [0.0; 3],
            scale: [1.0; 3],
            origin_quanta: [0; 3],
            node_cell: None,
            geometry_encoding: GeometryEncoding::RawU32,
            payloads: vec![
                payload(0, 12, StoredColumn::Present),
                // The derived slot carries the source column's offset, which is not the minimum.
                PayloadDirectoryEntry::not_stored(StoredColumn::Derived { from: 3 }, 103),
                payload(100, 3, StoredColumn::Present),
                payload(103, 5, StoredColumn::Present),
            ],
        };
        assert_eq!(companion_range(&block).unwrap(), (100, 108));
    }
}
