use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek, SeekFrom},
    sync::Arc,
};

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, DatasetSource, HierarchySemantics, NodeId, Point, PointBatch,
    PointBlock, SpatialBounds, SpatialHierarchy, SpatialNode,
};

use crate::layout::{
    ColumnPayload, FOOTER_MAGIC_V1, FOOTER_MAGIC_V2, FOOTER_MAGIC_V3, FOOTER_MAGIC_V4, FOOTER_SIZE,
    HEADER_MAGIC, HEADER_SIZE, INDEX_MAGIC_V1, INDEX_MAGIC_V2, INDEX_MAGIC_V3, INDEX_MAGIC_V4,
    attribute_payload_size, block_payload_size,
};
use crate::metadata::validate_metadata;
use crate::{BlockCodec, FormatError, FormatMetadata, NodeAttributeRange, crc32, decompress_block};

const MAX_ATTRIBUTES_DEFAULT: u32 = 256;
const MAX_COMPONENTS_DEFAULT: u32 = 65_535;
const MAX_STRING_BYTES_DEFAULT: u32 = 1_048_576;
const MAX_INDEX_BYTES_DEFAULT: u64 = 256 * 1024 * 1024;
const MAX_BLOCK_BYTES_DEFAULT: u64 = 256 * 1024 * 1024;
const MAX_NODES_DEFAULT: u64 = 2_000_000;
const MAX_BLOCKS_DEFAULT: u64 = 2_000_000;

/// Resource limits checked before reader allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatReaderLimits {
    max_index_bytes: u64,
    max_block_bytes: u64,
    max_nodes: u64,
    max_blocks: u64,
    max_attributes: u32,
    max_attribute_components: u32,
    max_string_bytes: u32,
}

impl FormatReaderLimits {
    pub fn new(
        max_index_bytes: u64,
        max_block_bytes: u64,
        max_nodes: u64,
        max_blocks: u64,
        max_attributes: u32,
        max_attribute_components: u32,
        max_string_bytes: u32,
    ) -> Result<Self, FormatError> {
        if [
            max_index_bytes,
            max_block_bytes,
            max_nodes,
            max_blocks,
            u64::from(max_attributes),
            u64::from(max_attribute_components),
            u64::from(max_string_bytes),
        ]
        .contains(&0)
        {
            return Err(FormatError::InvalidReaderLimits);
        }
        Ok(Self {
            max_index_bytes,
            max_block_bytes,
            max_nodes,
            max_blocks,
            max_attributes,
            max_attribute_components,
            max_string_bytes,
        })
    }
}

impl Default for FormatReaderLimits {
    fn default() -> Self {
        Self {
            max_index_bytes: MAX_INDEX_BYTES_DEFAULT,
            max_block_bytes: MAX_BLOCK_BYTES_DEFAULT,
            max_nodes: MAX_NODES_DEFAULT,
            max_blocks: MAX_BLOCKS_DEFAULT,
            max_attributes: MAX_ATTRIBUTES_DEFAULT,
            max_attribute_components: MAX_COMPONENTS_DEFAULT,
            max_string_bytes: MAX_STRING_BYTES_DEFAULT,
        }
    }
}

#[derive(Debug, Clone)]
struct BlockDirectoryEntry {
    offset: u64,
    payload_length: u64,
    point_count: u64,
    quantization_error: f64,
    origin: [f64; 3],
    scale: [f64; 3],
    codec: BlockCodec,
    checksum: u32,
    columns: Option<Vec<ColumnPayload>>,
}

type ParsedIndex = (
    FormatMetadata,
    BTreeMap<BlockId, BlockDirectoryEntry>,
    BTreeMap<BlockId, Vec<SpatialBounds>>,
);

/// Aggregate sizes for an entire block, excluding the file index.
/// V4 totals geometry and all attribute payloads; use `block_payload_storage_stats`
/// to inspect their independent codecs and sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockStorageStats {
    pub id: viewer_core::BlockId,
    /// Legacy block codec; `None` in v4, where each payload has its own codec.
    pub codec: BlockCodec,
    pub point_count: u64,
    pub stored_bytes: u64,
    pub raw_bytes: u64,
}

/// Identity of an independently encoded payload in a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockPayloadKind {
    Geometry,
    Attribute {
        schema_index: u32,
        name: String,
    },
    /// V1-v3 encode geometry and all attributes together, so stored bytes cannot
    /// be attributed independently to geometry or individual columns.
    CombinedLegacy,
}

/// Stored and decoded bytes of one payload from the validated file index.
/// These sizes exclude header, index and footer bytes and require no payload I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockPayloadStorageStats {
    pub id: BlockId,
    pub kind: BlockPayloadKind,
    pub codec: BlockCodec,
    pub stored_bytes: u64,
    pub raw_bytes: u64,
}

/// Seekable reader that loads metadata at open and verifies only requested block payloads.
pub struct FormatReader<R> {
    reader: R,
    metadata: Arc<FormatMetadata>,
    blocks: Arc<BTreeMap<BlockId, BlockDirectoryEntry>>,
    block_bounds: Arc<BTreeMap<BlockId, Vec<SpatialBounds>>>,
    limits: FormatReaderLimits,
}

impl<R: Read + Seek> FormatReader<R> {
    pub fn new(reader: R) -> Result<Self, FormatError> {
        Self::with_limits(reader, FormatReaderLimits::default())
    }

    pub fn with_limits(mut reader: R, limits: FormatReaderLimits) -> Result<Self, FormatError> {
        let file_length = reader.seek(SeekFrom::End(0))?;
        if file_length < HEADER_SIZE + FOOTER_SIZE {
            return Err(FormatError::FileTooShort);
        }

        reader.seek(SeekFrom::Start(0))?;
        let mut header = [0; HEADER_SIZE as usize];
        reader.read_exact(&mut header)?;
        let version = validate_header(&header)?;

        let footer_offset = file_length - FOOTER_SIZE;
        reader.seek(SeekFrom::Start(footer_offset))?;
        let mut footer = [0; FOOTER_SIZE as usize];
        reader.read_exact(&mut footer)?;
        let (index_offset, index_length, declared_file_length, index_checksum) =
            parse_footer(&footer, version)?;
        if index_length > limits.max_index_bytes {
            return Err(FormatError::IndexExceedsLimit {
                requested: index_length,
                limit: limits.max_index_bytes,
            });
        }
        if declared_file_length != file_length
            || index_offset < HEADER_SIZE
            || index_offset.checked_add(index_length) != Some(footer_offset)
        {
            return Err(FormatError::InvalidFooter);
        }
        let index_size =
            usize::try_from(index_length).map_err(|_| FormatError::SizeOutOfRange(index_length))?;
        let mut index = Vec::new();
        index
            .try_reserve_exact(index_size)
            .map_err(|_| FormatError::AllocationFailed(index_size))?;
        index.resize(index_size, 0);
        reader.seek(SeekFrom::Start(index_offset))?;
        reader.read_exact(&mut index)?;
        if crc32(&index) != index_checksum {
            return Err(FormatError::InvalidChecksum("index"));
        }

        let (metadata, blocks, block_bounds) = parse_index(&index, &limits, index_offset, version)?;
        Ok(Self {
            reader,
            metadata: Arc::new(metadata),
            blocks: Arc::new(blocks),
            block_bounds: Arc::new(block_bounds),
            limits,
        })
    }

    pub fn format_metadata(&self) -> &FormatMetadata {
        &self.metadata
    }

    /// Returns aggregate block sizes from the validated directory without payload I/O.
    /// V4's codec is `None` because the aggregate combines independent payloads;
    /// inspect `block_payload_storage_stats` for their actual compression codecs.
    pub fn block_storage_stats(&self) -> Result<Vec<BlockStorageStats>, FormatError> {
        self.blocks
            .iter()
            .map(|(id, entry)| {
                Ok(BlockStorageStats {
                    id: *id,
                    codec: entry.codec,
                    point_count: entry.point_count,
                    stored_bytes: entry.payload_length,
                    raw_bytes: block_payload_size(
                        usize::try_from(entry.point_count)
                            .map_err(|_| FormatError::PointCountOutOfRange)?,
                        self.metadata.dataset().hierarchy().attributes(),
                    )?,
                })
            })
            .collect()
    }

    /// Returns one geometry entry and one entry per schema attribute for each v4 block.
    /// V1-v3 return one `CombinedLegacy` entry per block: their jointly compressed
    /// payload has no independent stored geometry/attribute sizes. Entries are
    /// ordered by block ID, then geometry and schema order. This reads no payloads.
    pub fn block_payload_storage_stats(
        &self,
    ) -> Result<Vec<BlockPayloadStorageStats>, FormatError> {
        let schema = self.metadata.dataset().hierarchy().attributes();
        let mut stats = Vec::new();
        for (id, entry) in self.blocks.iter() {
            if let Some(columns) = &entry.columns {
                stats.try_reserve(columns.len()).map_err(|_| {
                    FormatError::AllocationFailed(
                        columns.len() * std::mem::size_of::<BlockPayloadStorageStats>(),
                    )
                })?;
                for (index, column) in columns.iter().enumerate() {
                    let kind = if index == 0 {
                        BlockPayloadKind::Geometry
                    } else {
                        BlockPayloadKind::Attribute {
                            schema_index: (index - 1) as u32,
                            name: schema[index - 1].name().to_owned(),
                        }
                    };
                    stats.push(BlockPayloadStorageStats {
                        id: *id,
                        kind,
                        codec: column.codec,
                        stored_bytes: column.stored_length,
                        raw_bytes: column.raw_length,
                    });
                }
            } else {
                stats.try_reserve(1).map_err(|_| {
                    FormatError::AllocationFailed(std::mem::size_of::<BlockPayloadStorageStats>())
                })?;
                stats.push(BlockPayloadStorageStats {
                    id: *id,
                    kind: BlockPayloadKind::CombinedLegacy,
                    codec: entry.codec,
                    stored_bytes: entry.payload_length,
                    raw_bytes: block_payload_size(
                        usize::try_from(entry.point_count)
                            .map_err(|_| FormatError::PointCountOutOfRange)?,
                        schema,
                    )?,
                });
            }
        }
        Ok(stats)
    }

    pub fn into_inner(self) -> R {
        self.reader
    }

    /// Creates a reader over an independent cursor while sharing the validated immutable index.
    /// The supplied stream must refer to the same unchanged file snapshot as this reader.
    pub fn fork_reader<S: Read + Seek>(&self, reader: S) -> FormatReader<S> {
        FormatReader {
            reader,
            metadata: Arc::clone(&self.metadata),
            blocks: Arc::clone(&self.blocks),
            block_bounds: Arc::clone(&self.block_bounds),
            limits: self.limits,
        }
    }

    fn decode_geometry(
        &self,
        cursor: &mut SliceCursor<'_>,
        id: BlockId,
        entry: &BlockDirectoryEntry,
        point_count: usize,
    ) -> Result<PointBatch, FormatError> {
        let point_capacity = allocation_capacity(point_count, std::mem::size_of::<Point>())?;
        let mut points = Vec::new();
        points.try_reserve_exact(point_capacity).map_err(|_| {
            FormatError::AllocationFailed(point_capacity * std::mem::size_of::<Point>())
        })?;
        for _ in 0..point_count {
            let mut quantized = [0_u32; 3];
            for value in &mut quantized {
                *value = cursor.u32()?;
            }
            let position = std::array::from_fn(|axis| {
                (entry.origin[axis] + f64::from(quantized[axis]) * entry.scale[axis]) as f32
            });
            if position.iter().any(|value| !value.is_finite()) {
                return Err(FormatError::MalformedBlock(
                    "decoded position is out of range",
                ));
            }
            let origin = self.metadata.dataset().coordinate_frame().origin();
            let world = std::array::from_fn(|axis| origin[axis] + f64::from(position[axis]));
            let bounds = self
                .block_bounds
                .get(&id)
                .ok_or(FormatError::MalformedIndex("block has no node bounds"))?;
            if world.iter().any(|coordinate| !coordinate.is_finite())
                || bounds.iter().any(|bounds| !contains_point(*bounds, world))
            {
                return Err(FormatError::MalformedBlock(
                    "decoded position is outside its node bounds",
                ));
            }
            points.push(Point::new(position, ColorRgb::new(0, 0, 0)));
        }
        for point in &mut points {
            point.color = ColorRgb::new(cursor.u8()?, cursor.u8()?, cursor.u8()?);
        }

        Ok(PointBatch::new(points))
    }

    /// Loads XYZ and display RGB only. For v1-v3 the legacy combined payload must be read.
    /// Row indices in the returned batch are stable within this immutable block.
    pub fn read_geometry(&mut self, id: BlockId) -> Result<PointBatch, FormatError> {
        let entry = self
            .blocks
            .get(&id)
            .ok_or(FormatError::UnknownBlock(id))?
            .clone();
        let Some(columns) = &entry.columns else {
            return Ok(self.read_point_block(id)?.into_parts().1);
        };
        let raw = self.read_column_payload(id, &entry, &columns[0])?;
        let mut cursor = SliceCursor::new(&raw);
        let count =
            usize::try_from(entry.point_count).map_err(|_| FormatError::PointCountOutOfRange)?;
        let points = self.decode_geometry(&mut cursor, id, &entry, count)?;
        if cursor.remaining() != 0 {
            return Err(FormatError::MalformedBlock("trailing geometry bytes"));
        }
        Ok(points)
    }

    /// Loads one complete attribute column by its index in the shared dataset schema.
    /// In v4 this reads no geometry or other attribute payloads.
    pub fn read_attribute(
        &mut self,
        id: BlockId,
        schema_index: u32,
    ) -> Result<AttributeColumn, FormatError> {
        let entry = self
            .blocks
            .get(&id)
            .ok_or(FormatError::UnknownBlock(id))?
            .clone();
        let descriptor = self
            .metadata
            .dataset()
            .hierarchy()
            .attributes()
            .get(schema_index as usize)
            .ok_or(FormatError::UnknownAttribute(schema_index))?
            .clone();
        let Some(columns) = &entry.columns else {
            let block = self.read_point_block(id)?;
            return Ok(block.attributes()[schema_index as usize].clone());
        };
        let raw = self.read_column_payload(id, &entry, &columns[schema_index as usize + 1])?;
        let count =
            usize::try_from(entry.point_count).map_err(|_| FormatError::PointCountOutOfRange)?;
        let value_count = count
            .checked_mul(descriptor.component_count() as usize)
            .ok_or_else(|| FormatError::AttributeLengthOverflow(descriptor.name().to_owned()))?;
        let mut cursor = SliceCursor::new(&raw);
        let values = read_attribute_values(&mut cursor, descriptor.data_type(), value_count)?;
        if cursor.remaining() != 0 {
            return Err(FormatError::MalformedBlock("trailing attribute bytes"));
        }
        Ok(AttributeColumn::new(descriptor, values, count)?)
    }

    /// Resolves (block_id, row_id) to the requested attribute components.
    /// Compression granularity is one column per block, so this decodes that whole column.
    pub fn read_attribute_row(
        &mut self,
        id: BlockId,
        row_id: u64,
        schema_index: u32,
    ) -> Result<AttributeValues, FormatError> {
        let entry = self.blocks.get(&id).ok_or(FormatError::UnknownBlock(id))?;
        if row_id >= entry.point_count {
            return Err(FormatError::UnknownRow { block: id, row_id });
        }
        let column = self.read_attribute(id, schema_index)?;
        let width = column.descriptor().component_count() as usize;
        let start = usize::try_from(row_id)
            .map_err(|_| FormatError::PointCountOutOfRange)?
            .checked_mul(width)
            .ok_or(FormatError::BlockSizeOverflow)?;
        macro_rules! select_row {
            ($($variant:ident),*) => { match column.values() {
                $(AttributeValues::$variant(values) => AttributeValues::$variant(values[start..start + width].to_vec()),)*
            } };
        }
        Ok(select_row!(U8, I8, U16, I16, U32, I32, U64, I64, F32, F64))
    }

    fn read_column_payload(
        &mut self,
        id: BlockId,
        block: &BlockDirectoryEntry,
        column: &ColumnPayload,
    ) -> Result<Vec<u8>, FormatError> {
        for requested in [column.stored_length, column.raw_length] {
            if requested > self.limits.max_block_bytes {
                return Err(FormatError::BlockExceedsLimit {
                    requested,
                    limit: self.limits.max_block_bytes,
                });
            }
        }
        let size = usize::try_from(column.stored_length)
            .map_err(|_| FormatError::SizeOutOfRange(column.stored_length))?;
        let mut stored = Vec::new();
        stored
            .try_reserve_exact(size)
            .map_err(|_| FormatError::AllocationFailed(size))?;
        stored.resize(size, 0);
        self.reader.seek(SeekFrom::Start(
            block
                .offset
                .checked_add(column.offset)
                .ok_or(FormatError::FileSizeOverflow)?,
        ))?;
        self.reader.read_exact(&mut stored)?;
        if crc32(&stored) != column.checksum {
            return Err(FormatError::InvalidBlockChecksum(id));
        }
        let raw_size = usize::try_from(column.raw_length)
            .map_err(|_| FormatError::SizeOutOfRange(column.raw_length))?;
        if column.codec == BlockCodec::None {
            Ok(stored)
        } else {
            Ok(decompress_block(
                column.codec,
                &stored,
                raw_size,
                usize::try_from(self.limits.max_block_bytes).unwrap_or(usize::MAX),
            )?)
        }
    }

    fn read_point_block(&mut self, id: BlockId) -> Result<PointBlock, FormatError> {
        if self
            .blocks
            .get(&id)
            .is_some_and(|entry| entry.columns.is_some())
        {
            let points = self.read_geometry(id)?;
            let schema_count = self.metadata.dataset().hierarchy().attributes().len();
            let mut attributes = Vec::new();
            attributes
                .try_reserve_exact(schema_count)
                .map_err(|_| FormatError::AllocationFailed(schema_count))?;
            for schema_index in 0..schema_count {
                attributes.push(self.read_attribute(id, schema_index as u32)?);
            }
            return Ok(PointBlock::new(id, points, attributes));
        }
        let entry = self
            .blocks
            .get(&id)
            .ok_or(FormatError::UnknownBlock(id))?
            .clone();
        let stored_size = usize::try_from(entry.payload_length)
            .map_err(|_| FormatError::SizeOutOfRange(entry.payload_length))?;
        if entry.payload_length > self.limits.max_block_bytes {
            return Err(FormatError::BlockExceedsLimit {
                requested: entry.payload_length,
                limit: self.limits.max_block_bytes,
            });
        }
        let mut stored = Vec::new();
        stored
            .try_reserve_exact(stored_size)
            .map_err(|_| FormatError::AllocationFailed(stored_size))?;
        stored.resize(stored_size, 0);
        self.reader.seek(SeekFrom::Start(entry.offset))?;
        self.reader.read_exact(&mut stored)?;
        if crc32(&stored) != entry.checksum {
            return Err(FormatError::InvalidBlockChecksum(id));
        }

        let schema = self.metadata.dataset().hierarchy().attributes();
        let point_count_u64 = entry.point_count;
        let point_count =
            usize::try_from(point_count_u64).map_err(|_| FormatError::PointCountOutOfRange)?;
        let uncompressed_size = block_payload_size(point_count, schema)?;
        if uncompressed_size > self.limits.max_block_bytes {
            return Err(FormatError::BlockExceedsLimit {
                requested: uncompressed_size,
                limit: self.limits.max_block_bytes,
            });
        }
        let uncompressed_size = usize::try_from(uncompressed_size)
            .map_err(|_| FormatError::SizeOutOfRange(uncompressed_size))?;
        let raw = match entry.codec {
            BlockCodec::None => {
                if stored.len() != uncompressed_size {
                    return Err(FormatError::MalformedBlock("raw payload length mismatch"));
                }
                stored
            }
            codec => decompress_block(
                codec,
                &stored,
                uncompressed_size,
                usize::try_from(self.limits.max_block_bytes).unwrap_or(usize::MAX),
            )?,
        };

        let mut cursor = SliceCursor::new(&raw);
        let points = self.decode_geometry(&mut cursor, id, &entry, point_count)?;

        let mut attributes = Vec::new();
        attributes
            .try_reserve_exact(schema.len())
            .map_err(|_| FormatError::AllocationFailed(schema.len()))?;
        for descriptor in schema {
            let value_count = point_count
                .checked_mul(descriptor.component_count() as usize)
                .ok_or_else(|| {
                    FormatError::AttributeLengthOverflow(descriptor.name().to_owned())
                })?;
            let values = read_attribute_values(&mut cursor, descriptor.data_type(), value_count)?;
            attributes.push(AttributeColumn::new(
                descriptor.clone(),
                values,
                point_count,
            )?);
        }
        if cursor.remaining() != 0 {
            return Err(FormatError::MalformedBlock("trailing payload bytes"));
        }

        Ok(PointBlock::new(id, points, attributes))
    }
}

impl<R: Read + Seek> DatasetSource for FormatReader<R> {
    type Error = FormatError;

    fn metadata(&self) -> &DatasetMetadata {
        self.metadata.dataset()
    }

    fn read_block(&mut self, id: BlockId) -> Result<PointBlock, Self::Error> {
        self.read_point_block(id)
    }
}

fn validate_header(header: &[u8; HEADER_SIZE as usize]) -> Result<u16, FormatError> {
    if &header[..8] != HEADER_MAGIC {
        return Err(FormatError::InvalidHeader);
    }
    let version = u16::from_le_bytes(header[8..10].try_into().expect("fixed width"));
    if ![1, 2, 3, 4].contains(&version) {
        return Err(FormatError::UnsupportedVersion(version));
    }
    let header_length = u16::from_le_bytes(header[10..12].try_into().expect("fixed width"));
    if u64::from(header_length) != HEADER_SIZE
        || header[12..16] != [0; 4]
        || u32::from_le_bytes(header[16..20].try_into().expect("fixed width")) != 0
        || u32::from_le_bytes(header[20..24].try_into().expect("fixed width"))
            != crc32(&header[..20])
    {
        return Err(FormatError::InvalidHeader);
    }
    Ok(version)
}

fn parse_footer(
    footer: &[u8; FOOTER_SIZE as usize],
    version: u16,
) -> Result<(u64, u64, u64, u32), FormatError> {
    let expected_magic = if version >= 4 {
        FOOTER_MAGIC_V4
    } else if version >= 3 {
        FOOTER_MAGIC_V3
    } else if version == 2 {
        FOOTER_MAGIC_V2
    } else {
        FOOTER_MAGIC_V1
    };
    if &footer[..8] != expected_magic
        || u32::from_le_bytes(footer[36..40].try_into().expect("fixed width"))
            != crc32(&footer[..36])
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

fn parse_index(
    bytes: &[u8],
    limits: &FormatReaderLimits,
    index_offset: u64,
    version: u16,
) -> Result<ParsedIndex, FormatError> {
    let mut cursor = SliceCursor::new(bytes);
    let expected_magic = if version >= 4 {
        INDEX_MAGIC_V4
    } else if version >= 3 {
        INDEX_MAGIC_V3
    } else if version == 2 {
        INDEX_MAGIC_V2
    } else {
        INDEX_MAGIC_V1
    };
    if cursor.take(8)? != expected_magic {
        return Err(FormatError::MalformedIndex("invalid index magic"));
    }
    let dataset_origin = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
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
    let crs = cursor.string(limits.max_string_bytes)?;
    let sampling_policy = cursor.string(limits.max_string_bytes)?;
    let semantics = if version >= 2 {
        match cursor.u8()? {
            0 => HierarchySemantics::Replacement,
            1 => HierarchySemantics::Additive,
            tag => {
                return Err(FormatError::InvalidTag {
                    field: "hierarchy semantics",
                    tag,
                });
            }
        }
    } else {
        HierarchySemantics::Replacement
    };
    let visual_scale = if version >= 3 { cursor.f64()? } else { 1.0 };

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

    let node_count = cursor.u64()?;
    if node_count == 0 || node_count > limits.max_nodes {
        return Err(FormatError::CountLimit {
            kind: "nodes",
            requested: node_count,
            limit: limits.max_nodes,
        });
    }
    let node_capacity = usize::try_from(node_count).map_err(|_| FormatError::TooManyNodes)?;
    let mut nodes = Vec::new();
    nodes
        .try_reserve_exact(node_capacity)
        .map_err(|_| FormatError::AllocationFailed(node_capacity))?;
    let mut node_ranges = Vec::new();
    node_ranges
        .try_reserve_exact(node_capacity)
        .map_err(|_| FormatError::AllocationFailed(node_capacity))?;
    let mut previous_node_id = None;
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
        let min = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
        let max = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
        let bounds = SpatialBounds::new(min, max)?;
        let point_count = cursor.u64()?;
        let geometric_error = cursor.f64()?;
        let block_count = cursor.u32()?;
        let block_count_usize = block_count as usize;
        if block_count as u64 > limits.max_blocks {
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
    }

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
        let quantization_error = cursor.f64()?;
        let block_origin = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
        let scale = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
        let codec = BlockCodec::try_from(cursor.u8()?)?;
        let checksum = cursor.u32()?;
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
        let expected_length = block_payload_size(point_count_usize, &schema)?;
        if expected_length > limits.max_block_bytes {
            return Err(FormatError::BlockExceedsLimit {
                requested: expected_length,
                limit: limits.max_block_bytes,
            });
        }
        if version < 4 && codec == BlockCodec::None && payload_length != expected_length {
            return Err(FormatError::MalformedIndex(
                "raw block size disagrees with its point count",
            ));
        }
        if version < 4 && codec == BlockCodec::Lz4 && payload_length >= expected_length {
            return Err(FormatError::MalformedIndex(
                "compressed payload is not smaller than raw payload",
            ));
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
        let columns = if version >= 4 {
            if codec != BlockCodec::None || checksum != 0 {
                return Err(FormatError::MalformedIndex(
                    "invalid v4 aggregate codec or checksum",
                ));
            }
            let mut columns = Vec::new();
            columns
                .try_reserve_exact(schema.len() + 1)
                .map_err(|_| FormatError::AllocationFailed(schema.len() + 1))?;
            let geometry_length = point_count
                .checked_mul(15)
                .ok_or(FormatError::BlockSizeOverflow)?;
            let mut expected_offset = 0;
            for raw_length in std::iter::once(Ok(geometry_length)).chain(
                schema
                    .iter()
                    .map(|descriptor| attribute_payload_size(point_count, descriptor)),
            ) {
                let expected_raw_length = raw_length?;
                let column = ColumnPayload {
                    offset: cursor.u64()?,
                    stored_length: cursor.u64()?,
                    raw_length: cursor.u64()?,
                    codec: BlockCodec::try_from(cursor.u8()?)?,
                    checksum: cursor.u32()?,
                };
                if column.offset != expected_offset
                    || column.raw_length != expected_raw_length
                    || (column.codec == BlockCodec::None
                        && column.stored_length != column.raw_length)
                    || (column.codec == BlockCodec::Lz4
                        && column.stored_length >= column.raw_length)
                {
                    return Err(FormatError::MalformedIndex(
                        "invalid v4 column range or length",
                    ));
                }
                expected_offset = expected_offset
                    .checked_add(column.stored_length)
                    .ok_or(FormatError::FileSizeOverflow)?;
                if expected_offset > payload_length {
                    return Err(FormatError::MalformedIndex(
                        "column extends beyond its block",
                    ));
                }
                columns.push(column);
            }
            if expected_offset != payload_length {
                return Err(FormatError::MalformedIndex(
                    "column payload section does not fill its block",
                ));
            }
            Some(columns)
        } else {
            None
        };
        blocks.insert(
            id,
            BlockDirectoryEntry {
                offset,
                payload_length,
                point_count,
                quantization_error,
                origin: block_origin,
                scale,
                codec,
                checksum,
                columns,
            },
        );
    }
    if cursor.remaining() != 0 {
        return Err(FormatError::MalformedIndex("trailing index bytes"));
    }
    if blocks.len() != block_capacity {
        return Err(FormatError::MalformedIndex("duplicate block IDs"));
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
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new(dataset_origin)?, hierarchy),
        crs,
        sampling_policy,
        node_ranges,
    )?
    .with_visual_scale(visual_scale)?;
    validate_metadata(&metadata)?;
    Ok((metadata, blocks, block_bounds))
}

fn contains_point(bounds: SpatialBounds, point: [f64; 3]) -> bool {
    let min = bounds.min();
    let max = bounds.max();
    (0..3).all(|axis| point[axis] >= min[axis] && point[axis] <= max[axis])
}

fn validate_block_layout(
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

fn validate_node_point_counts(
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

fn validate_node_error_bounds(
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

fn parse_attribute_type(tag: u8) -> Result<AttributeDataType, FormatError> {
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

fn read_attribute_values(
    cursor: &mut SliceCursor<'_>,
    data_type: AttributeDataType,
    count: usize,
) -> Result<AttributeValues, FormatError> {
    macro_rules! read_values {
        ($ty:ty, $read:ident) => {{
            let mut values = Vec::new();
            values
                .try_reserve_exact(count)
                .map_err(|_| FormatError::AllocationFailed(count * std::mem::size_of::<$ty>()))?;
            for _ in 0..count {
                values.push(cursor.$read()?);
            }
            values
        }};
    }
    Ok(match data_type {
        AttributeDataType::U8 => AttributeValues::U8(read_values!(u8, u8)),
        AttributeDataType::I8 => AttributeValues::I8(read_values!(i8, i8)),
        AttributeDataType::U16 => AttributeValues::U16(read_values!(u16, u16)),
        AttributeDataType::I16 => AttributeValues::I16(read_values!(i16, i16)),
        AttributeDataType::U32 => AttributeValues::U32(read_values!(u32, u32)),
        AttributeDataType::I32 => AttributeValues::I32(read_values!(i32, i32)),
        AttributeDataType::U64 => AttributeValues::U64(read_values!(u64, u64)),
        AttributeDataType::I64 => AttributeValues::I64(read_values!(i64, i64)),
        AttributeDataType::F32 => AttributeValues::F32(read_values!(f32, f32)),
        AttributeDataType::F64 => AttributeValues::F64(read_values!(f64, f64)),
    })
}

struct SliceCursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> SliceCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], FormatError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(FormatError::TruncatedIndex)?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or(FormatError::TruncatedIndex)?;
        self.position = end;
        Ok(bytes)
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, FormatError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("fixed width"),
        ))
    }

    fn i8(&mut self) -> Result<i8, FormatError> {
        Ok(self.u8()? as i8)
    }

    fn i16(&mut self) -> Result<i16, FormatError> {
        Ok(i16::from_le_bytes(
            self.take(2)?.try_into().expect("fixed width"),
        ))
    }

    fn u32(&mut self) -> Result<u32, FormatError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("fixed width"),
        ))
    }

    fn i32(&mut self) -> Result<i32, FormatError> {
        Ok(i32::from_le_bytes(
            self.take(4)?.try_into().expect("fixed width"),
        ))
    }

    fn u64(&mut self) -> Result<u64, FormatError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("fixed width"),
        ))
    }

    fn i64(&mut self) -> Result<i64, FormatError> {
        Ok(i64::from_le_bytes(
            self.take(8)?.try_into().expect("fixed width"),
        ))
    }

    fn f32(&mut self) -> Result<f32, FormatError> {
        Ok(f32::from_bits(self.u32()?))
    }

    fn f64(&mut self) -> Result<f64, FormatError> {
        Ok(f64::from_bits(self.u64()?))
    }

    fn string(&mut self, max_bytes: u32) -> Result<Option<String>, FormatError> {
        let length = self.u32()?;
        if length == 0 {
            return Ok(None);
        }
        if length > max_bytes {
            return Err(FormatError::StringExceedsLimit {
                requested: length,
                limit: max_bytes,
            });
        }
        let length = length as usize;
        let bytes = self.take(length)?;
        Ok(Some(
            std::str::from_utf8(bytes)
                .map_err(|_| FormatError::InvalidUtf8)?
                .to_owned(),
        ))
    }
}

fn allocation_capacity(count: usize, item_size: usize) -> Result<usize, FormatError> {
    count
        .checked_mul(item_size)
        .ok_or(FormatError::BlockSizeOverflow)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        io::{Cursor, Read, Seek, SeekFrom},
        rc::Rc,
    };

    use viewer_core::{
        AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId,
        BlockRequestScheduler, ColorRgb, CoordinateFrame, DatasetMetadata, DatasetSource,
        FrustumPlane, HierarchySemantics, LodSelectionConfig, NodeId, RequestCompletion,
        SpatialBounds, SpatialFrustum, SpatialHierarchy, SpatialNode,
    };

    use crate::{BlockCodec, FormatMetadata, FormatReaderLimits, SourcePointBlock};

    use super::{FormatError, FormatReader};
    use crate::writer::{FormatWriter, FormatWriterOptions};

    fn metadata() -> FormatMetadata {
        let origin = [1_000_000_000_000.0, -2_000_000_000_000.0, 10.0];
        let bounds =
            SpatialBounds::new(origin, [origin[0] + 2.0, origin[1] + 1.0, origin[2] + 1.0])
                .unwrap();
        let block_id = BlockId::new(42);
        let node =
            SpatialNode::new(NodeId::new(0), None, 0, bounds, 2, 0.25, vec![block_id]).unwrap();
        let schema = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
        let hierarchy = SpatialHierarchy::new(vec![node], vec![schema], Some(2)).unwrap();
        FormatMetadata::new(
            DatasetMetadata::new(CoordinateFrame::new(origin).unwrap(), hierarchy),
            Some("EPSG:4978".to_owned()),
            Some("uniform voxel sampling".to_owned()),
            Vec::new(),
        )
        .unwrap()
    }

    #[test]
    fn shared_dataset_metadata_reuses_the_same_hierarchy_allocation() {
        let metadata = metadata();
        let first = metadata.shared_dataset();
        let second = metadata.shared_dataset();
        assert!(std::sync::Arc::ptr_eq(&first, &second));
    }

    fn source_block() -> SourcePointBlock {
        let descriptor = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
        let column =
            AttributeColumn::new(descriptor, AttributeValues::U16(vec![100, 200]), 2).unwrap();
        SourcePointBlock::new(
            BlockId::new(42),
            vec![[0.25, 0.5, 0.0], [1.5, 0.75, 1.0]],
            vec![ColorRgb::new(10, 20, 30), ColorRgb::new(40, 50, 60)],
            vec![column],
        )
    }

    fn additive_metadata() -> FormatMetadata {
        let base = metadata();
        let hierarchy = SpatialHierarchy::new_with_semantics(
            vec![
                SpatialNode::new(
                    NodeId::new(0),
                    None,
                    0,
                    base.dataset().hierarchy().stats().root_bounds,
                    2,
                    0.25,
                    vec![BlockId::new(42)],
                )
                .unwrap(),
            ],
            base.dataset().hierarchy().attributes().to_vec(),
            Some(2),
            HierarchySemantics::Additive,
        )
        .unwrap();
        FormatMetadata::new(
            DatasetMetadata::new(base.dataset().coordinate_frame(), hierarchy),
            base.crs().map(str::to_owned),
            base.sampling_policy().map(str::to_owned),
            Vec::new(),
        )
        .unwrap()
    }

    fn format_bytes() -> Vec<u8> {
        let options = FormatWriterOptions::new(0.001, 1024)
            .unwrap()
            .with_columnar_payloads(false);
        let mut writer = FormatWriter::new(Cursor::new(Vec::new()), metadata(), options).unwrap();
        writer.write_block(source_block()).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn additive_format_bytes() -> Vec<u8> {
        let mut writer = FormatWriter::new(
            Cursor::new(Vec::new()),
            additive_metadata(),
            FormatWriterOptions::new(0.01, 1024 * 1024)
                .unwrap()
                .with_columnar_payloads(false),
        )
        .unwrap();
        writer.write_block(source_block()).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_metadata_and_only_the_requested_point_block() {
        let bytes = format_bytes();
        let reads = Rc::new(RefCell::new(Vec::new()));
        let reader = ReadSpy {
            inner: Cursor::new(bytes.clone()),
            reads: Rc::clone(&reads),
        };
        let mut format = FormatReader::new(reader).unwrap();
        let storage = format.block_storage_stats().unwrap();
        assert_eq!(storage.len(), 1);
        assert_eq!(storage[0].id, BlockId::new(42));
        assert_eq!(storage[0].point_count, 2);
        assert_eq!(storage[0].raw_bytes, 34);
        assert!(storage[0].stored_bytes < storage[0].raw_bytes);
        assert_eq!(storage[0].codec, BlockCodec::Lz4);
        let index_offset = u64::from_le_bytes(
            bytes[bytes.len() - 32..bytes.len() - 24]
                .try_into()
                .unwrap(),
        );
        assert!(
            reads
                .borrow()
                .iter()
                .all(|(start, length)| *start + *length as u64 <= 24 || *start >= index_offset)
        );
        assert_eq!(format.format_metadata().crs(), Some("EPSG:4978"));
        assert_eq!(format.format_metadata().visual_scale(), 1.0);
        assert_eq!(format.metadata().coordinate_frame().origin()[0], 1e12);
        assert_eq!(
            format.metadata().hierarchy().semantics(),
            HierarchySemantics::Replacement
        );

        let entry = format.blocks[&BlockId::new(42)].clone();
        reads.borrow_mut().clear();
        let block = format.read_block(BlockId::new(42)).unwrap();
        assert_eq!(block.points().points()[0].position, [0.25, 0.5, 0.0]);
        assert_eq!(block.points().points()[1].color, ColorRgb::new(40, 50, 60));
        assert_eq!(
            block.attributes()[0].values(),
            &AttributeValues::U16(vec![100, 200])
        );
        assert!(reads.borrow().iter().all(|(start, length)| {
            *start >= entry.offset && *start + *length as u64 <= entry.offset + entry.payload_length
        }));
    }

    #[test]
    fn forked_readers_share_the_index_and_read_with_independent_cursors() {
        let bytes = format_bytes();
        let reader = FormatReader::new(Cursor::new(bytes.clone())).unwrap();
        let first = reader.fork_reader(Cursor::new(bytes.clone()));
        let second = reader.fork_reader(Cursor::new(bytes));
        assert!(std::sync::Arc::ptr_eq(&first.blocks, &second.blocks));
        assert!(std::sync::Arc::ptr_eq(&first.metadata, &second.metadata));

        let (first_points, second_points) = std::thread::scope(|scope| {
            let first = scope.spawn(move || {
                let mut reader = first;
                reader.read_block(BlockId::new(42)).unwrap().points().len()
            });
            let second = scope.spawn(move || {
                let mut reader = second;
                reader.read_block(BlockId::new(42)).unwrap().points().len()
            });
            (first.join().unwrap(), second.join().unwrap())
        });
        assert_eq!(first_points, 2);
        assert_eq!(second_points, 2);
    }

    #[test]
    fn scheduler_tickets_read_selected_format_blocks_through_dataset_source() {
        let mut reader = FormatReader::new(Cursor::new(format_bytes())).unwrap();
        let bounds = reader.metadata().hierarchy().stats().root_bounds;
        let min = bounds.min();
        let max = bounds.max();
        let frustum = SpatialFrustum::new(vec![
            FrustumPlane::new([1.0, 0.0, 0.0], -min[0]).unwrap(),
            FrustumPlane::new([-1.0, 0.0, 0.0], max[0]).unwrap(),
            FrustumPlane::new([0.0, 1.0, 0.0], -min[1]).unwrap(),
            FrustumPlane::new([0.0, -1.0, 0.0], max[1]).unwrap(),
            FrustumPlane::new([0.0, 0.0, 1.0], -min[2]).unwrap(),
            FrustumPlane::new([0.0, 0.0, -1.0], max[2]).unwrap(),
        ])
        .unwrap();
        let mut scheduler = BlockRequestScheduler::new(1).unwrap();
        scheduler
            .update_view(
                reader.metadata().hierarchy(),
                &frustum,
                LodSelectionConfig::new(1.0, 2).unwrap(),
                min,
                |_| 0.0,
            )
            .unwrap();

        let request = scheduler.take_next().unwrap().unwrap();
        let block = reader.read_block(request.ticket().block_id()).unwrap();
        assert_eq!(block.points().len(), 2);
        assert_eq!(
            scheduler
                .complete(request.ticket(), Ok(block.id()))
                .unwrap(),
            RequestCompletion::Delivered
        );
        assert_eq!(scheduler.pending_count(), 0);
        assert_eq!(scheduler.in_flight_count(), 0);
    }

    #[test]
    fn verifies_block_crc_when_read_and_keeps_open_lazy() {
        let mut bytes = format_bytes();
        let format = FormatReader::new(Cursor::new(bytes.clone())).unwrap();
        let entry = format.blocks[&BlockId::new(42)].clone();
        bytes[entry.offset as usize] ^= 1;
        let mut corrupted = FormatReader::new(Cursor::new(bytes)).unwrap();

        assert!(matches!(
            corrupted.read_block(BlockId::new(42)),
            Err(FormatError::InvalidBlockChecksum(id)) if id.get() == 42
        ));
    }

    #[test]
    fn applies_index_limit_before_reading_the_index() {
        let bytes = format_bytes();
        let limits = FormatReaderLimits::new(1, 1024, 10, 10, 10, 10, 100).unwrap();

        assert!(matches!(
            FormatReader::with_limits(Cursor::new(bytes), limits),
            Err(FormatError::IndexExceedsLimit { limit: 1, .. })
        ));
    }

    #[test]
    fn rejects_corrupt_index_and_blocks_over_the_configured_limit() {
        let mut bytes = format_bytes();
        let index_offset = u64::from_le_bytes(
            bytes[bytes.len() - 32..bytes.len() - 24]
                .try_into()
                .unwrap(),
        ) as usize;
        bytes[index_offset] ^= 1;
        assert!(matches!(
            FormatReader::new(Cursor::new(bytes)),
            Err(FormatError::InvalidChecksum("index"))
        ));

        let limits = FormatReaderLimits::new(1024, 1, 10, 10, 10, 10, 100).unwrap();
        assert!(matches!(
            FormatReader::with_limits(Cursor::new(format_bytes()), limits),
            Err(FormatError::BlockExceedsLimit { limit: 1, .. })
        ));
    }

    #[test]
    fn rejects_truncated_and_invalid_version_files() {
        let mut truncated = format_bytes();
        truncated.pop();
        assert!(matches!(
            FormatReader::new(Cursor::new(truncated)),
            Err(FormatError::InvalidFooter)
        ));

        let mut unsupported = format_bytes();
        unsupported[8..10].copy_from_slice(&5_u16.to_le_bytes());
        let crc = crate::crc32(&unsupported[..20]);
        unsupported[20..24].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(
            FormatReader::new(Cursor::new(unsupported)),
            Err(FormatError::UnsupportedVersion(5))
        ));
    }

    #[test]
    fn reads_v3_visual_scale_and_additive_hierarchy() {
        let metadata = additive_metadata().with_visual_scale(50.0).unwrap();
        let mut writer = FormatWriter::new(
            Cursor::new(Vec::new()),
            metadata,
            FormatWriterOptions::new(0.001, 1024)
                .unwrap()
                .with_columnar_payloads(false),
        )
        .unwrap();
        writer.write_block(source_block()).unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 3);
        assert_eq!(&bytes[bytes.len() - 40..bytes.len() - 32], b"OCTVEND3");
        let mut reader = FormatReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.format_metadata().visual_scale(), 50.0);
        assert_eq!(
            reader.metadata().hierarchy().semantics(),
            HierarchySemantics::Additive
        );
        assert_eq!(reader.format_metadata().crs(), Some("EPSG:4978"));
        assert_eq!(
            reader
                .read_block(BlockId::new(42))
                .unwrap()
                .points()
                .points()[0]
                .position,
            [0.25, 0.5, 0.0]
        );
    }

    #[test]
    fn reads_additive_version_two_and_preserves_ancestor_payload_counts() {
        let bytes = additive_format_bytes();
        assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 2);
        let mut reader = FormatReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(
            reader.metadata().hierarchy().semantics(),
            HierarchySemantics::Additive
        );
        assert_eq!(reader.format_metadata().visual_scale(), 1.0);
        assert_eq!(
            reader
                .metadata()
                .hierarchy()
                .node(NodeId::new(0))
                .unwrap()
                .point_count(),
            2
        );
        assert_eq!(
            reader
                .read_point_block(BlockId::new(42))
                .unwrap()
                .points()
                .len(),
            2
        );
    }

    #[test]
    fn reads_an_independently_compressed_block() {
        let origin = [0.0; 3];
        let block_id = BlockId::new(9);
        let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
        let node =
            SpatialNode::new(NodeId::new(0), None, 0, bounds, 4096, 0.0, vec![block_id]).unwrap();
        let descriptor = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
        let hierarchy =
            SpatialHierarchy::new(vec![node], vec![descriptor.clone()], Some(4096)).unwrap();
        let metadata = FormatMetadata::new(
            DatasetMetadata::new(CoordinateFrame::new(origin).unwrap(), hierarchy),
            None,
            None,
            Vec::new(),
        )
        .unwrap();
        let column =
            AttributeColumn::new(descriptor, AttributeValues::U16(vec![128; 4096]), 4096).unwrap();
        let source = SourcePointBlock::new(
            block_id,
            vec![[0.25, 0.5, 0.75]; 4096],
            vec![ColorRgb::new(20, 40, 60); 4096],
            vec![column],
        );
        let mut writer = FormatWriter::new(
            Cursor::new(Vec::new()),
            metadata,
            FormatWriterOptions::new(0.0, 1024 * 1024)
                .unwrap()
                .with_columnar_payloads(false),
        )
        .unwrap();
        writer.write_block(source).unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let mut reader = FormatReader::new(Cursor::new(bytes)).unwrap();

        assert_eq!(reader.blocks[&block_id].codec, BlockCodec::Lz4);
        let storage = reader.block_storage_stats().unwrap();
        assert_eq!(storage.len(), 1);
        assert_eq!(storage[0].point_count, 4096);
        assert_eq!(storage[0].raw_bytes, 4096 * 17);
        assert!(storage[0].stored_bytes < storage[0].raw_bytes);
        let block = reader.read_block(block_id).unwrap();
        assert_eq!(block.points().len(), 4096);
        assert_eq!(block.points().points()[2048].position, [0.25, 0.5, 0.75]);
        assert_eq!(
            block.attributes()[0].values(),
            &AttributeValues::U16(vec![128; 4096])
        );
    }

    #[test]
    fn round_trips_a_larger_synthetic_block() {
        let block_id = BlockId::new(77);
        let point_count = 65_536;
        let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
        let node = SpatialNode::new(
            NodeId::new(0),
            None,
            0,
            bounds,
            point_count,
            0.0,
            vec![block_id],
        )
        .unwrap();
        let hierarchy = SpatialHierarchy::new(vec![node], Vec::new(), Some(point_count)).unwrap();
        let metadata = FormatMetadata::new(
            DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
            None,
            None,
            Vec::new(),
        )
        .unwrap();
        let source = SourcePointBlock::new(
            block_id,
            vec![[0.25, 0.5, 0.75]; point_count as usize],
            vec![ColorRgb::new(20, 40, 60); point_count as usize],
            Vec::new(),
        );
        let mut writer = FormatWriter::new(
            Cursor::new(Vec::new()),
            metadata,
            FormatWriterOptions::new(0.0, 4 * 1024 * 1024).unwrap(),
        )
        .unwrap();
        writer.write_block(source).unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let mut reader = FormatReader::new(Cursor::new(bytes)).unwrap();

        let block = reader.read_block(block_id).unwrap();
        assert_eq!(block.points().len(), point_count as usize);
        assert_eq!(block.points().points()[0].position, [0.25, 0.5, 0.75]);
        assert_eq!(
            block.points().points()[point_count as usize - 1].color,
            ColorRgb::new(20, 40, 60)
        );
    }

    #[test]
    fn preserves_each_supported_attribute_type_and_float_bits() {
        let block_id = BlockId::new(100);
        let descriptors = vec![
            AttributeDescriptor::new("u8", AttributeDataType::U8, 2).unwrap(),
            AttributeDescriptor::new("i8", AttributeDataType::I8, 1).unwrap(),
            AttributeDescriptor::new("u16", AttributeDataType::U16, 1).unwrap(),
            AttributeDescriptor::new("i16", AttributeDataType::I16, 1).unwrap(),
            AttributeDescriptor::new("u32", AttributeDataType::U32, 1).unwrap(),
            AttributeDescriptor::new("i32", AttributeDataType::I32, 1).unwrap(),
            AttributeDescriptor::new("u64", AttributeDataType::U64, 1).unwrap(),
            AttributeDescriptor::new("i64", AttributeDataType::I64, 1).unwrap(),
            AttributeDescriptor::new("f32", AttributeDataType::F32, 1).unwrap(),
            AttributeDescriptor::new("f64", AttributeDataType::F64, 1).unwrap(),
        ];
        let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
        let node =
            SpatialNode::new(NodeId::new(0), None, 0, bounds, 2, 0.0, vec![block_id]).unwrap();
        let hierarchy = SpatialHierarchy::new(vec![node], descriptors.clone(), Some(2)).unwrap();
        let metadata = FormatMetadata::new(
            DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
            None,
            None,
            Vec::new(),
        )
        .unwrap();
        let source_values = vec![
            AttributeValues::U8(vec![1, 2, 3, 4]),
            AttributeValues::I8(vec![-1, 2]),
            AttributeValues::U16(vec![500, u16::MAX]),
            AttributeValues::I16(vec![-300, 22]),
            AttributeValues::U32(vec![u32::MAX, 1]),
            AttributeValues::I32(vec![-22, 99]),
            AttributeValues::U64(vec![u64::MAX, 42]),
            AttributeValues::I64(vec![-100, 999]),
            AttributeValues::F32(vec![f32::from_bits(0x7FC0_1234), 1.5]),
            AttributeValues::F64(vec![f64::from_bits(0x7FF8_0000_0000_1234), -0.0]),
        ];
        let attributes = descriptors
            .into_iter()
            .zip(source_values.clone())
            .map(|(descriptor, values)| AttributeColumn::new(descriptor, values, 2).unwrap())
            .collect();
        let source = SourcePointBlock::new(
            block_id,
            vec![[0.0; 3], [1.0; 3]],
            vec![ColorRgb::new(0, 1, 2), ColorRgb::new(255, 254, 253)],
            attributes,
        );
        for columnar in [false, true] {
            let mut writer = FormatWriter::new(
                Cursor::new(Vec::new()),
                metadata.clone(),
                FormatWriterOptions::new(0.0, 1024)
                    .unwrap()
                    .with_columnar_payloads(columnar),
            )
            .unwrap();
            writer
                .write_block(SourcePointBlock::new(
                    source.id(),
                    source.positions_local().to_vec(),
                    source.colors().to_vec(),
                    source.attributes().to_vec(),
                ))
                .unwrap();
            let bytes = writer.finish().unwrap().into_inner();
            let mut reader = FormatReader::new(Cursor::new(bytes)).unwrap();
            let block = reader.read_block(block_id).unwrap();

            for (column, expected) in block.attributes()[..8].iter().zip(&source_values[..8]) {
                assert_eq!(column.values(), expected);
            }
            let AttributeValues::F32(f32_values) = block.attributes()[8].values() else {
                panic!("expected f32 values");
            };
            assert_eq!(f32_values[0].to_bits(), 0x7FC0_1234);
            assert_eq!(f32_values[1].to_bits(), 1.5_f32.to_bits());
            let AttributeValues::F64(f64_values) = block.attributes()[9].values() else {
                panic!("expected f64 values");
            };
            assert_eq!(f64_values[0].to_bits(), 0x7FF8_0000_0000_1234);
            assert_eq!(f64_values[1].to_bits(), (-0.0_f64).to_bits());
        }
    }

    fn columnar_format_bytes() -> Vec<u8> {
        let base = metadata();
        let ids = vec![BlockId::new(42), BlockId::new(43)];
        let vector = AttributeDescriptor::new("vector", AttributeDataType::F32, 2).unwrap();
        let mut schema = base.dataset().hierarchy().attributes().to_vec();
        schema.push(vector.clone());
        let root = SpatialNode::new(
            NodeId::new(0),
            None,
            0,
            base.dataset().hierarchy().stats().root_bounds,
            4,
            0.25,
            ids.clone(),
        )
        .unwrap();
        let hierarchy = SpatialHierarchy::new_with_semantics(
            vec![root],
            schema,
            Some(4),
            HierarchySemantics::Additive,
        )
        .unwrap();
        let metadata = FormatMetadata::new(
            DatasetMetadata::new(base.dataset().coordinate_frame(), hierarchy),
            base.crs().map(str::to_owned),
            None,
            Vec::new(),
        )
        .unwrap()
        .with_visual_scale(50.0)
        .unwrap();
        let mut writer = FormatWriter::new(
            Cursor::new(Vec::new()),
            metadata,
            FormatWriterOptions::new(0.001, 1024)
                .unwrap()
                .with_columnar_payloads(true),
        )
        .unwrap();
        for id in ids {
            let block = source_block();
            let mut attributes = block.attributes().to_vec();
            attributes.push(
                AttributeColumn::new(
                    vector.clone(),
                    AttributeValues::F32(vec![0.5, -1.0, 1.5, -2.0]),
                    2,
                )
                .unwrap(),
            );
            writer
                .write_block(SourcePointBlock::new(
                    id,
                    block.positions_local().to_vec(),
                    block.colors().to_vec(),
                    attributes,
                ))
                .unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn v4_reads_only_geometry_or_one_requested_column_and_keeps_row_identity() {
        let bytes = columnar_format_bytes();
        assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 4);
        assert_eq!(&bytes[bytes.len() - 40..bytes.len() - 32], b"OCTVEND4");
        let reads = Rc::new(RefCell::new(Vec::new()));
        let mut reader = FormatReader::new(ReadSpy {
            inner: Cursor::new(bytes),
            reads: Rc::clone(&reads),
        })
        .unwrap();
        assert_eq!(reader.format_metadata().visual_scale(), 50.0);
        assert_eq!(
            reader.metadata().hierarchy().semantics(),
            HierarchySemantics::Additive
        );
        let entry = reader.blocks[&BlockId::new(42)].clone();
        let columns = entry.columns.as_ref().unwrap();
        let index_offset = reader
            .blocks
            .values()
            .map(|entry| entry.offset + entry.payload_length)
            .max()
            .unwrap();
        assert!(
            reads
                .borrow()
                .iter()
                .all(|(start, length)| *start + *length as u64 <= 24 || *start >= index_offset)
        );

        reads.borrow_mut().clear();
        let geometry = reader.read_geometry(BlockId::new(42)).unwrap();
        assert_eq!(geometry.points()[0].position, [0.25, 0.5, 0.0]);
        assert_eq!(geometry.points()[1].color, ColorRgb::new(40, 50, 60));
        let assert_only_column = |column: &crate::layout::ColumnPayload| {
            let reads = reads.borrow();
            assert_eq!(
                reads.iter().map(|(_, length)| *length as u64).sum::<u64>(),
                column.stored_length
            );
            assert!(
                reads
                    .iter()
                    .all(|(start, length)| *start >= entry.offset + column.offset
                        && *start + *length as u64
                            <= entry.offset + column.offset + column.stored_length)
            );
        };
        assert_only_column(&columns[0]);

        reads.borrow_mut().clear();
        assert_eq!(
            reader.read_attribute_row(BlockId::new(42), 1, 1).unwrap(),
            AttributeValues::F32(vec![1.5, -2.0])
        );
        assert_only_column(&columns[2]);
        reads.borrow_mut().clear();
        assert_eq!(
            reader.read_attribute(BlockId::new(42), 0).unwrap().values(),
            &AttributeValues::U16(vec![100, 200])
        );
        assert_only_column(&columns[1]);

        for id in [BlockId::new(42), BlockId::new(43)] {
            let block = reader.read_block(id).unwrap();
            assert_eq!(block.points(), &geometry);
            assert_eq!(block.attributes().len(), 2);
            assert_eq!(
                block.attributes()[0].values(),
                &AttributeValues::U16(vec![100, 200])
            );
        }
        reads.borrow_mut().clear();
        assert!(matches!(
            reader.read_attribute_row(BlockId::new(42), 2, 0),
            Err(FormatError::UnknownRow { .. })
        ));
        assert!(matches!(
            reader.read_attribute(BlockId::new(42), 2),
            Err(FormatError::UnknownAttribute(2))
        ));
        assert!(matches!(
            reader.read_geometry(BlockId::new(99)),
            Err(FormatError::UnknownBlock(_))
        ));
        assert!(reads.borrow().is_empty());
    }

    #[test]
    fn payload_stats_report_exact_v4_columns_and_explicit_legacy_aggregates_without_io() {
        let mut writer = FormatWriter::new(
            Cursor::new(Vec::new()),
            metadata().with_visual_scale(2.0).unwrap(),
            FormatWriterOptions::new(0.001, 1024)
                .unwrap()
                .with_columnar_payloads(false),
        )
        .unwrap();
        writer.write_block(source_block()).unwrap();
        let v3 = writer.finish().unwrap().into_inner();
        for (version, bytes) in [
            format_bytes(),
            additive_format_bytes(),
            v3,
            columnar_format_bytes(),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                u16::from_le_bytes(bytes[8..10].try_into().unwrap()),
                version as u16 + 1
            );
            let reads = Rc::new(RefCell::new(Vec::new()));
            let reader = FormatReader::new(ReadSpy {
                inner: Cursor::new(bytes),
                reads: Rc::clone(&reads),
            })
            .unwrap();
            reads.borrow_mut().clear();
            let aggregates = reader.block_storage_stats().unwrap();
            let payloads = reader.block_payload_storage_stats().unwrap();
            assert!(reads.borrow().is_empty());
            for aggregate in &aggregates {
                let matching = payloads
                    .iter()
                    .filter(|payload| payload.id == aggregate.id)
                    .collect::<Vec<_>>();
                assert_eq!(
                    matching
                        .iter()
                        .map(|payload| payload.stored_bytes)
                        .sum::<u64>(),
                    aggregate.stored_bytes
                );
                assert_eq!(
                    matching
                        .iter()
                        .map(|payload| payload.raw_bytes)
                        .sum::<u64>(),
                    aggregate.raw_bytes
                );
                if version < 3 {
                    assert_eq!(matching.len(), 1);
                    assert_eq!(matching[0].kind, super::BlockPayloadKind::CombinedLegacy);
                    assert_eq!(matching[0].codec, aggregate.codec);
                } else {
                    assert_eq!(matching.len(), 3);
                    assert_eq!(aggregate.codec, BlockCodec::None);
                    assert_eq!(matching[0].kind, super::BlockPayloadKind::Geometry);
                    assert_eq!(
                        matching[1].kind,
                        super::BlockPayloadKind::Attribute {
                            schema_index: 0,
                            name: "intensity".to_owned()
                        }
                    );
                    assert_eq!(
                        matching[2].kind,
                        super::BlockPayloadKind::Attribute {
                            schema_index: 1,
                            name: "vector".to_owned()
                        }
                    );
                    for (payload, entry) in matching
                        .iter()
                        .zip(reader.blocks[&aggregate.id].columns.as_ref().unwrap())
                    {
                        assert_eq!(payload.codec, entry.codec);
                        assert_eq!(payload.stored_bytes, entry.stored_length);
                        assert_eq!(payload.raw_bytes, entry.raw_length);
                    }
                }
            }
            if version == 3 {
                assert_eq!(payloads.len(), 6);
                let geometry = payloads
                    .iter()
                    .filter(|payload| payload.kind == super::BlockPayloadKind::Geometry)
                    .map(|payload| payload.raw_bytes)
                    .sum::<u64>();
                let attributes = payloads
                    .iter()
                    .filter(|payload| {
                        matches!(payload.kind, super::BlockPayloadKind::Attribute { .. })
                    })
                    .map(|payload| payload.raw_bytes)
                    .sum::<u64>();
                assert_eq!((geometry, attributes), (60, 40));
            }
        }
    }

    #[test]
    fn v4_checksums_are_lazy_and_independent_for_geometry_and_each_attribute() {
        let bytes = columnar_format_bytes();
        let reader = FormatReader::new(Cursor::new(bytes.clone())).unwrap();
        let entry = &reader.blocks[&BlockId::new(42)];
        for column_index in 0..3 {
            let mut corrupted = bytes.clone();
            corrupted
                [(entry.offset + entry.columns.as_ref().unwrap()[column_index].offset) as usize] ^=
                1;
            let mut reader = FormatReader::new(Cursor::new(corrupted)).unwrap();
            if column_index == 0 {
                reader.read_attribute(BlockId::new(42), 0).unwrap();
                assert!(matches!(
                    reader.read_geometry(BlockId::new(42)),
                    Err(FormatError::InvalidBlockChecksum(_))
                ));
            } else {
                reader.read_geometry(BlockId::new(42)).unwrap();
                reader
                    .read_attribute(BlockId::new(42), (2 - column_index) as u32)
                    .unwrap();
                assert!(matches!(
                    reader.read_attribute(BlockId::new(42), (column_index - 1) as u32),
                    Err(FormatError::InvalidBlockChecksum(_))
                ));
            }
            assert!(matches!(
                reader.read_block(BlockId::new(42)),
                Err(FormatError::InvalidBlockChecksum(_))
            ));
        }
    }

    fn repair_index_and_footer_checksums(bytes: &mut [u8]) {
        let footer = bytes.len() - 40;
        let index = u64::from_le_bytes(bytes[footer + 8..footer + 16].try_into().unwrap()) as usize;
        let checksum = crate::crc32(&bytes[index..footer]);
        bytes[footer + 32..footer + 36].copy_from_slice(&checksum.to_le_bytes());
        let checksum = crate::crc32(&bytes[footer..footer + 36]);
        bytes[footer + 36..].copy_from_slice(&checksum.to_le_bytes());
    }

    #[test]
    fn v4_rejects_column_range_overlap_wrong_lengths_and_codecs_before_payload_reads() {
        let bytes = columnar_format_bytes();
        let footer = bytes.len() - 40;
        // Two block entries, each containing a 93-byte common entry and three 29-byte payload entries.
        let first_column = footer - 2 * (93 + 3 * 29) + 93;
        for (offset, replacement) in [
            (first_column, 1_u64.to_le_bytes().to_vec()), // gap at geometry start
            (first_column + 8, u64::MAX.to_le_bytes().to_vec()), // stored size overflow
            (first_column + 16, u64::MAX.to_le_bytes().to_vec()), // decoded length mismatch
            (first_column + 24, vec![255]),               // unsupported codec
            (first_column + 29, 0_u64.to_le_bytes().to_vec()), // attribute overlaps geometry
        ] {
            let mut corrupted = bytes.clone();
            corrupted[offset..offset + replacement.len()].copy_from_slice(&replacement);
            repair_index_and_footer_checksums(&mut corrupted);
            let reads = Rc::new(RefCell::new(Vec::new()));
            let index = u64::from_le_bytes(corrupted[footer + 8..footer + 16].try_into().unwrap());
            assert!(
                FormatReader::new(ReadSpy {
                    inner: Cursor::new(corrupted),
                    reads: Rc::clone(&reads)
                })
                .is_err()
            );
            assert!(
                reads
                    .borrow()
                    .iter()
                    .all(|(start, length)| *start + *length as u64 <= 24 || *start >= index)
            );
        }
        let limits = FormatReaderLimits::new(4096, 49, 10, 10, 10, 10, 100).unwrap();
        assert!(matches!(
            FormatReader::with_limits(Cursor::new(bytes), limits),
            Err(FormatError::BlockExceedsLimit { .. })
        ));
    }

    #[test]
    fn v4_round_trips_empty_geometry_and_attribute_columns() {
        let id = BlockId::new(0);
        let descriptor = AttributeDescriptor::new("empty", AttributeDataType::U64, 2).unwrap();
        let node = SpatialNode::new(
            NodeId::new(0),
            None,
            0,
            SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
            0,
            0.0,
            vec![id],
        )
        .unwrap();
        let hierarchy =
            SpatialHierarchy::new(vec![node], vec![descriptor.clone()], Some(0)).unwrap();
        let metadata = FormatMetadata::new(
            DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
            None,
            None,
            Vec::new(),
        )
        .unwrap();
        let mut writer = FormatWriter::new(
            Cursor::new(Vec::new()),
            metadata,
            FormatWriterOptions::new(0.0, 1024).unwrap(),
        )
        .unwrap();
        writer
            .write_block(SourcePointBlock::new(
                id,
                Vec::new(),
                Vec::new(),
                vec![
                    AttributeColumn::new(descriptor, AttributeValues::U64(Vec::new()), 0).unwrap(),
                ],
            ))
            .unwrap();
        let mut reader =
            FormatReader::new(Cursor::new(writer.finish().unwrap().into_inner())).unwrap();
        assert_eq!(reader.read_geometry(id).unwrap().len(), 0);
        assert_eq!(
            reader.read_attribute(id, 0).unwrap().values(),
            &AttributeValues::U64(Vec::new())
        );
        assert_eq!(reader.read_block(id).unwrap().points().len(), 0);
        assert!(matches!(
            reader.read_attribute_row(id, 0, 0),
            Err(FormatError::UnknownRow { .. })
        ));
    }

    #[test]
    fn selective_api_preserves_legacy_v1_v2_v3_behavior() {
        let v1 = format_bytes();
        let v2 = additive_format_bytes();
        let mut writer = FormatWriter::new(
            Cursor::new(Vec::new()),
            metadata().with_visual_scale(2.0).unwrap(),
            FormatWriterOptions::new(0.001, 1024)
                .unwrap()
                .with_columnar_payloads(false),
        )
        .unwrap();
        writer.write_block(source_block()).unwrap();
        let v3 = writer.finish().unwrap().into_inner();
        for (expected_version, bytes) in [v1, v2, v3].into_iter().enumerate() {
            assert_eq!(
                u16::from_le_bytes(bytes[8..10].try_into().unwrap()),
                expected_version as u16 + 1
            );
            let mut reader = FormatReader::new(Cursor::new(bytes)).unwrap();
            assert_eq!(reader.read_geometry(BlockId::new(42)).unwrap().len(), 2);
            assert_eq!(
                reader.read_attribute_row(BlockId::new(42), 1, 0).unwrap(),
                AttributeValues::U16(vec![200])
            );
        }
    }

    #[derive(Clone)]
    struct ReadSpy {
        inner: Cursor<Vec<u8>>,
        reads: Rc<RefCell<Vec<(u64, usize)>>>,
    }

    impl Read for ReadSpy {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let start = self.inner.position();
            let read = self.inner.read(buffer)?;
            if read > 0 {
                self.reads.borrow_mut().push((start, read));
            }
            Ok(read)
        }
    }

    impl Seek for ReadSpy {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(position)
        }
    }
}
