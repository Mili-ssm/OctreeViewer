use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek, SeekFrom},
};

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, DatasetSource, NodeId, Point, PointBatch, PointBlock,
    SpatialBounds, SpatialHierarchy, SpatialNode,
};

use crate::writer::{
    FOOTER_MAGIC, FOOTER_SIZE, HEADER_MAGIC, HEADER_SIZE, INDEX_MAGIC, block_payload_size,
    validate_metadata,
};
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
}

/// Physical and decoded size information from a validated block-directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockStorageStats {
    pub id: viewer_core::BlockId,
    pub codec: BlockCodec,
    pub point_count: u64,
    pub stored_bytes: u64,
    pub raw_bytes: u64,
}

/// Seekable reader that loads metadata at open and verifies only requested block payloads.
pub struct FormatReader<R> {
    reader: R,
    metadata: FormatMetadata,
    blocks: BTreeMap<BlockId, BlockDirectoryEntry>,
    block_bounds: BTreeMap<BlockId, Vec<SpatialBounds>>,
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
        validate_header(&header)?;

        let footer_offset = file_length - FOOTER_SIZE;
        reader.seek(SeekFrom::Start(footer_offset))?;
        let mut footer = [0; FOOTER_SIZE as usize];
        reader.read_exact(&mut footer)?;
        let (index_offset, index_length, declared_file_length, index_checksum) =
            parse_footer(&footer)?;
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

        let (metadata, blocks, block_bounds) = parse_index(&index, &limits, index_offset)?;
        Ok(Self {
            reader,
            metadata,
            blocks,
            block_bounds,
            limits,
        })
    }

    pub fn format_metadata(&self) -> &FormatMetadata {
        &self.metadata
    }

    /// Returns payload sizes from the validated block directory, excluding the file index.
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

    pub fn into_inner(self) -> R {
        self.reader
    }

    fn read_point_block(&mut self, id: BlockId) -> Result<PointBlock, FormatError> {
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
        let position_capacity = allocation_capacity(point_count, std::mem::size_of::<[f32; 3]>())?;
        let mut positions = Vec::new();
        positions
            .try_reserve_exact(position_capacity)
            .map_err(|_| {
                FormatError::AllocationFailed(position_capacity * std::mem::size_of::<[f32; 3]>())
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
            positions.push(position);
        }
        let point_capacity = allocation_capacity(point_count, std::mem::size_of::<Point>())?;
        let mut points = Vec::new();
        points.try_reserve_exact(point_capacity).map_err(|_| {
            FormatError::AllocationFailed(point_capacity * std::mem::size_of::<Point>())
        })?;
        for position in positions {
            let color = ColorRgb::new(cursor.u8()?, cursor.u8()?, cursor.u8()?);
            points.push(Point::new(position, color));
        }

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

        Ok(PointBlock::new(id, PointBatch::new(points), attributes))
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

fn validate_header(header: &[u8; HEADER_SIZE as usize]) -> Result<(), FormatError> {
    if &header[..8] != HEADER_MAGIC {
        return Err(FormatError::InvalidHeader);
    }
    let version = u16::from_le_bytes(header[8..10].try_into().expect("fixed width"));
    if version != 1 {
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
    Ok(())
}

fn parse_footer(footer: &[u8; FOOTER_SIZE as usize]) -> Result<(u64, u64, u64, u32), FormatError> {
    if &footer[..8] != FOOTER_MAGIC
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
) -> Result<
    (
        FormatMetadata,
        BTreeMap<BlockId, BlockDirectoryEntry>,
        BTreeMap<BlockId, Vec<SpatialBounds>>,
    ),
    FormatError,
> {
    let mut cursor = SliceCursor::new(bytes);
    if cursor.take(8)? != INDEX_MAGIC {
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
        if codec == BlockCodec::None && payload_length != expected_length {
            return Err(FormatError::MalformedIndex(
                "raw block size disagrees with its point count",
            ));
        }
        if codec == BlockCodec::Lz4 && payload_length >= expected_length {
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
    let hierarchy = SpatialHierarchy::new(nodes, schema, source_point_count)?;
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new(dataset_origin)?, hierarchy),
        crs,
        sampling_policy,
        node_ranges,
    )?;
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
        ColorRgb, CoordinateFrame, DatasetMetadata, DatasetSource, NodeId, SpatialBounds,
        SpatialHierarchy, SpatialNode,
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

    fn format_bytes() -> Vec<u8> {
        let options = FormatWriterOptions::new(0.001, 1024).unwrap();
        let mut writer = FormatWriter::new(Cursor::new(Vec::new()), metadata(), options).unwrap();
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
        assert_eq!(format.metadata().coordinate_frame().origin()[0], 1e12);

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
        unsupported[8..10].copy_from_slice(&2_u16.to_le_bytes());
        let crc = crate::crc32(&unsupported[..20]);
        unsupported[20..24].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(
            FormatReader::new(Cursor::new(unsupported)),
            Err(FormatError::UnsupportedVersion(2))
        ));
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
            FormatWriterOptions::new(0.0, 1024 * 1024).unwrap(),
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
        let mut writer = FormatWriter::new(
            Cursor::new(Vec::new()),
            metadata,
            FormatWriterOptions::new(0.0, 1024).unwrap(),
        )
        .unwrap();
        writer.write_block(source).unwrap();
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
