use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{Read, Seek, SeekFrom},
    sync::{Arc, Mutex},
};

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, DatasetSource, HierarchySemantics, NodeId, Point, PointBatch,
    PointBlock, SpatialBounds, SpatialHierarchy, SpatialNode,
};

use crate::geometry::{GeometryDecoder, GeometryEncoding};
use crate::layout::{
    FOOTER_SIZE, HEADER_SIZE, PayloadDirectoryEntry, StorageLayout, attribute_payload_size,
    block_payload_size,
};
use crate::metadata::validate_metadata;
use crate::version::FileRole;
use crate::{BlockCodec, FormatError, FormatMetadata, NodeAttributeRange, crc32, decompress_block};

const MAX_ATTRIBUTES_DEFAULT: u32 = 256;
const MAX_COMPONENTS_DEFAULT: u32 = 65_535;
const MAX_STRING_BYTES_DEFAULT: u32 = 1_048_576;
const MAX_INDEX_BYTES_DEFAULT: u64 = 256 * 1024 * 1024;
const MAX_BLOCK_BYTES_DEFAULT: u64 = 256 * 1024 * 1024;
const MAX_NODES_DEFAULT: u64 = 2_000_000;
const MAX_BLOCKS_DEFAULT: u64 = 2_000_000;
const ATTRIBUTE_RANGE_CACHE_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
struct CachedAttributeRange {
    schema_index: u32,
    component: u32,
    range: Option<[f64; 2]>,
}

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
    geometry_encoding: GeometryEncoding,
    payloads: Vec<PayloadDirectoryEntry>,
}

type ParsedIndex = (
    FormatMetadata,
    BTreeMap<BlockId, BlockDirectoryEntry>,
    BTreeMap<BlockId, Vec<SpatialBounds>>,
);

/// Aggregate sizes for an entire block, excluding the file index.
/// `logical_bytes` is the canonical geometry and attribute size; `raw_bytes` is the
/// sum of the independently decoded payloads and can differ for encoded geometry.
/// Use `block_payload_storage_stats`
/// to inspect their independent codecs and sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockStorageStats {
    pub id: viewer_core::BlockId,
    pub point_count: u64,
    pub stored_bytes: u64,
    pub raw_bytes: u64,
    pub logical_bytes: u64,
    pub geometry_encoding: GeometryEncoding,
}

/// Identity of an independently encoded payload in a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockPayloadKind {
    Geometry,
    DisplayColor,
    Attribute { schema_index: u32, name: String },
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
pub(crate) struct IndexedReader<R> {
    reader: R,
    attributes: Option<R>,
    split: bool,
    layout: StorageLayout,
    index_identity: (u64, u64, u32),
    attribute_file_length: u64,
    metadata: Arc<FormatMetadata>,
    blocks: Arc<BTreeMap<BlockId, BlockDirectoryEntry>>,
    block_bounds: Arc<BTreeMap<BlockId, Vec<SpatialBounds>>>,
    attribute_range_cache: Arc<Mutex<VecDeque<CachedAttributeRange>>>,
    limits: FormatReaderLimits,
}

impl<R: Read + Seek> IndexedReader<R> {
    #[cfg(test)]
    pub fn new(reader: R) -> Result<Self, FormatError> {
        Self::with_limits(reader, FormatReaderLimits::default())
    }

    #[cfg(test)]
    pub fn with_limits(mut reader: R, limits: FormatReaderLimits) -> Result<Self, FormatError> {
        reader.seek(SeekFrom::Start(0))?;
        let mut header = [0; HEADER_SIZE as usize];
        reader.read_exact(&mut header)?;
        let identity = crate::version::parse_identity(&header)?;
        Self::with_identity(reader, limits, identity)
    }

    pub(crate) fn with_identity(
        mut reader: R,
        limits: FormatReaderLimits,
        identity: crate::FormatIdentity,
    ) -> Result<Self, FormatError> {
        let layout = validate_supported_primary(identity)?;
        let file_length = reader.seek(SeekFrom::End(0))?;
        if file_length < HEADER_SIZE + FOOTER_SIZE {
            return Err(FormatError::FileTooShort);
        }
        if identity.role == FileRole::Attributes {
            return Err(FormatError::InvalidHeader);
        }
        let split = identity.role == FileRole::SplitPrimary;

        let footer_offset = file_length - FOOTER_SIZE;
        reader.seek(SeekFrom::Start(footer_offset))?;
        let mut footer = [0; FOOTER_SIZE as usize];
        reader.read_exact(&mut footer)?;
        let (index_offset, index_length, declared_file_length, index_checksum) =
            parse_footer(&footer, layout.footer_magic())?;
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

        let (metadata, blocks, block_bounds) =
            parse_index(&index, &limits, index_offset, split, layout)?;
        let attribute_file_length = if split {
            blocks
                .values()
                .flat_map(|block| block.payloads.iter().skip(1))
                .try_fold(HEADER_SIZE, |end, p| {
                    p.offset
                        .checked_add(p.stored_length)
                        .map(|next| end.max(next))
                        .ok_or(FormatError::FileSizeOverflow)
                })?
                .checked_add(FOOTER_SIZE)
                .ok_or(FormatError::FileSizeOverflow)?
        } else {
            0
        };
        Ok(Self {
            reader,
            attributes: None,
            split,
            layout,
            index_identity: (index_offset, index_length, index_checksum),
            attribute_file_length,
            metadata: Arc::new(metadata),
            blocks: Arc::new(blocks),
            block_bounds: Arc::new(block_bounds),
            attribute_range_cache: Arc::new(Mutex::new(VecDeque::new())),
            limits,
        })
    }

    /// Attach a committed companion belonging to this primary index. Payload CRCs are
    /// checked lazily on access; binding protects accidental mismatch, not forgery.
    pub fn attach_attributes(&mut self, mut attributes: R) -> Result<(), FormatError> {
        if !self.split {
            return Err(FormatError::InvalidMetadata(
                "monolithic file cannot attach attributes",
            ));
        }
        let length = attributes.seek(SeekFrom::End(0))?;
        if length != self.attribute_file_length {
            return Err(FormatError::InvalidFooter);
        }
        attributes.seek(SeekFrom::Start(0))?;
        let mut header = [0; HEADER_SIZE as usize];
        attributes.read_exact(&mut header)?;
        let attribute_identity = crate::version::parse_identity(&header)?;
        if attribute_identity.version != self.layout.version()
            || attribute_identity.role != FileRole::Attributes
        {
            return Err(FormatError::InvalidHeader);
        }
        attributes.seek(SeekFrom::Start(length - FOOTER_SIZE))?;
        let mut footer = [0; FOOTER_SIZE as usize];
        attributes.read_exact(&mut footer)?;
        let (offset, index_length, declared_length, checksum) =
            parse_footer(&footer, self.layout.footer_magic())?;
        if declared_length != length || (offset, index_length, checksum) != self.index_identity {
            return Err(FormatError::InvalidMetadata(
                "attribute companion does not match primary index",
            ));
        }
        self.attributes = Some(attributes);
        Ok(())
    }

    pub fn has_attributes(&self) -> bool {
        !self.split || self.attributes.is_some()
    }

    /// Finite range over all node payloads, used consistently across LOD blocks.
    /// The last 64 component queries (including absent ranges) are cached across forked readers.
    /// The first query scans resident metadata; neither hits nor misses read payloads.
    pub fn attribute_range(
        &self,
        schema_index: u32,
        component: u32,
    ) -> Result<Option<[f64; 2]>, FormatError> {
        let descriptor = self
            .metadata
            .dataset()
            .hierarchy()
            .attributes()
            .get(schema_index as usize)
            .ok_or(FormatError::UnknownAttribute(schema_index))?;
        if component >= descriptor.component_count() {
            return Err(FormatError::InvalidSchema);
        }
        let mut cache = self
            .attribute_range_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(position) = cache
            .iter()
            .position(|entry| entry.schema_index == schema_index && entry.component == component)
        {
            let entry = cache.remove(position).expect("located range cache entry");
            let range = entry.range;
            cache.push_back(entry);
            return Ok(range);
        }

        let mut result: Option<[f64; 2]> = None;
        for (_, ranges) in self.metadata.node_range_entries() {
            if let Some(range) = ranges
                .iter()
                .find(|range| range.schema_index() == schema_index)
            {
                let low = range
                    .min()
                    .scalar_as_f64(component as usize)
                    .ok_or(FormatError::InvalidSchema)?;
                let high = range
                    .max()
                    .scalar_as_f64(component as usize)
                    .ok_or(FormatError::InvalidSchema)?;
                if low.is_finite() && high.is_finite() {
                    result = Some(result.map_or([low, high], |r| [r[0].min(low), r[1].max(high)]));
                }
            }
        }
        if cache.len() == ATTRIBUTE_RANGE_CACHE_CAPACITY {
            cache.pop_front();
        }
        cache.push_back(CachedAttributeRange {
            schema_index,
            component,
            range: result,
        });
        Ok(result)
    }

    pub fn format_metadata(&self) -> &FormatMetadata {
        &self.metadata
    }

    /// Bytes of the serialized common index, stored only in the primary file.
    /// This is not an estimate of resident or peak memory used by the decoded index.
    pub fn index_size_bytes(&self) -> u64 {
        self.index_identity.1
    }

    /// Returns aggregate block sizes from the validated directory without payload I/O.
    /// Inspect `block_payload_storage_stats` for per-payload codecs and sizes.
    pub fn block_storage_stats(&self) -> Result<Vec<BlockStorageStats>, FormatError> {
        self.blocks
            .iter()
            .map(|(id, entry)| {
                Ok(BlockStorageStats {
                    id: *id,
                    point_count: entry.point_count,
                    stored_bytes: entry
                        .payloads
                        .iter()
                        .map(|payload| payload.stored_length)
                        .sum(),
                    raw_bytes: entry.payloads.iter().try_fold(0_u64, |sum, payload| {
                        sum.checked_add(payload.raw_length)
                            .ok_or(FormatError::BlockSizeOverflow)
                    })?,
                    logical_bytes: block_payload_size(
                        usize::try_from(entry.point_count)
                            .map_err(|_| FormatError::PointCountOutOfRange)?,
                        self.metadata.dataset().hierarchy().attributes(),
                    )?,
                    geometry_encoding: entry.geometry_encoding,
                })
            })
            .collect()
    }

    /// Returns geometry and attribute payload sizes in block/schema order without I/O.
    pub fn block_payload_storage_stats(
        &self,
    ) -> Result<Vec<BlockPayloadStorageStats>, FormatError> {
        let schema = self.metadata.dataset().hierarchy().attributes();
        let mut stats = Vec::new();
        for (id, entry) in self.blocks.iter() {
            stats.try_reserve(entry.payloads.len()).map_err(|_| {
                FormatError::AllocationFailed(
                    entry.payloads.len() * std::mem::size_of::<BlockPayloadStorageStats>(),
                )
            })?;
            for (index, payload) in entry.payloads.iter().enumerate() {
                let kind = if index == 0 {
                    BlockPayloadKind::Geometry
                } else if index == 1 {
                    BlockPayloadKind::DisplayColor
                } else {
                    BlockPayloadKind::Attribute {
                        schema_index: (index - 2) as u32,
                        name: schema[index - 2].name().to_owned(),
                    }
                };
                stats.push(BlockPayloadStorageStats {
                    id: *id,
                    kind,
                    codec: payload.codec,
                    stored_bytes: payload.stored_length,
                    raw_bytes: payload.raw_length,
                });
            }
        }
        Ok(stats)
    }

    pub fn into_inner(self) -> R {
        self.reader
    }

    pub fn format_version(&self) -> crate::FormatVersion {
        self.layout.version()
    }

    /// Creates a reader over an independent cursor while sharing the validated immutable index.
    /// The supplied stream must refer to the same unchanged file snapshot as this reader.
    pub fn fork_reader<S: Read + Seek>(&self, reader: S) -> IndexedReader<S> {
        IndexedReader {
            reader,
            attributes: None,
            split: self.split,
            layout: self.layout,
            index_identity: self.index_identity,
            attribute_file_length: self.attribute_file_length,
            metadata: Arc::clone(&self.metadata),
            blocks: Arc::clone(&self.blocks),
            block_bounds: Arc::clone(&self.block_bounds),
            attribute_range_cache: Arc::clone(&self.attribute_range_cache),
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
        let mut decoder = GeometryDecoder::with_count(
            entry.geometry_encoding,
            cursor.remaining_slice(),
            point_count,
        )?;
        let point_bytes = allocation_capacity(point_count, std::mem::size_of::<Point>())?;
        let mut points = Vec::new();
        points
            .try_reserve_exact(point_count)
            .map_err(|_| FormatError::AllocationFailed(point_bytes))?;
        for _ in 0..point_count {
            let quantized = decoder.next_coordinate()?;
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
            points.push(Point::new(position, ColorRgb::new(160, 160, 160)));
        }
        decoder.finish()?;

        Ok(PointBatch::new(points))
    }

    /// Loads XYZ and display RGB without reading attribute payloads.
    /// Row indices in the returned batch are stable within this immutable block.
    pub fn read_geometry(&mut self, id: BlockId) -> Result<PointBatch, FormatError> {
        let geometry = self.read_positions(id)?;
        if !self.has_attributes() {
            return Ok(geometry);
        }
        let colors = self.read_display_colors(id)?;
        let mut points = geometry.into_points();
        for (point, color) in points.iter_mut().zip(colors) {
            point.color = color;
        }
        Ok(PointBatch::new(points))
    }

    pub fn read_display_colors(&mut self, id: BlockId) -> Result<Vec<ColorRgb>, FormatError> {
        let blocks = Arc::clone(&self.blocks);
        let entry = blocks.get(&id).ok_or(FormatError::UnknownBlock(id))?;
        let raw = self.read_payload(id, entry, &entry.payloads[1], true)?;
        Ok(raw
            .as_chunks::<3>()
            .0
            .iter()
            .map(|rgb| ColorRgb::new(rgb[0], rgb[1], rgb[2]))
            .collect())
    }

    /// Reads only XYZ; supplies neutral gray without accessing color or attribute payloads.
    pub fn read_positions(&mut self, id: BlockId) -> Result<PointBatch, FormatError> {
        let blocks = Arc::clone(&self.blocks);
        let entry = blocks.get(&id).ok_or(FormatError::UnknownBlock(id))?;
        let raw = self.read_payload(id, entry, &entry.payloads[0], false)?;
        let mut cursor = SliceCursor::new(&raw);
        let count =
            usize::try_from(entry.point_count).map_err(|_| FormatError::PointCountOutOfRange)?;
        let points = self.decode_geometry(&mut cursor, id, entry, count)?;
        Ok(points)
    }

    /// Loads one complete attribute column by its index in the shared dataset schema.
    /// This reads no geometry or other attribute payloads.
    pub fn read_attribute(
        &mut self,
        id: BlockId,
        schema_index: u32,
    ) -> Result<AttributeColumn, FormatError> {
        let blocks = Arc::clone(&self.blocks);
        let entry = blocks.get(&id).ok_or(FormatError::UnknownBlock(id))?;
        let descriptor = self
            .metadata
            .dataset()
            .hierarchy()
            .attributes()
            .get(schema_index as usize)
            .ok_or(FormatError::UnknownAttribute(schema_index))?
            .clone();
        let raw = self.read_payload(id, entry, &entry.payloads[schema_index as usize + 2], true)?;
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
        let row = usize::try_from(row_id).map_err(|_| FormatError::PointCountOutOfRange)?;
        Ok(column.values().select_rows(&[row], width)?)
    }

    fn read_payload(
        &mut self,
        id: BlockId,
        block: &BlockDirectoryEntry,
        column: &PayloadDirectoryEntry,
        is_attribute: bool,
    ) -> Result<Vec<u8>, FormatError> {
        if self.split && is_attribute && self.attributes.is_none() {
            return Err(FormatError::AttributesUnavailable);
        }
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
        let (reader, offset) = if self.split && is_attribute {
            (
                self.attributes
                    .as_mut()
                    .ok_or(FormatError::AttributesUnavailable)?,
                column.offset,
            )
        } else {
            (
                &mut self.reader,
                block
                    .offset
                    .checked_add(column.offset)
                    .ok_or(FormatError::FileSizeOverflow)?,
            )
        };
        reader.seek(SeekFrom::Start(offset))?;
        reader.read_exact(&mut stored)?;
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

    pub(crate) fn read_point_block(&mut self, id: BlockId) -> Result<PointBlock, FormatError> {
        let points = self.read_geometry(id)?;
        let schema_count = if self.has_attributes() {
            self.metadata.dataset().hierarchy().attributes().len()
        } else {
            0
        };
        let mut attributes = Vec::new();
        attributes
            .try_reserve_exact(schema_count)
            .map_err(|_| FormatError::AllocationFailed(schema_count))?;
        for schema_index in 0..schema_count {
            attributes.push(self.read_attribute(id, schema_index as u32)?);
        }
        Ok(PointBlock::new(id, points, attributes))
    }
}

impl<R: Read + Seek> DatasetSource for IndexedReader<R> {
    type Error = FormatError;

    fn metadata(&self) -> &DatasetMetadata {
        self.metadata.dataset()
    }

    fn read_block(&mut self, id: BlockId) -> Result<PointBlock, Self::Error> {
        self.read_point_block(id)
    }
}

fn validate_supported_primary(
    identity: crate::FormatIdentity,
) -> Result<StorageLayout, FormatError> {
    if !matches!(identity.version.major(), 5 | 6) {
        return Err(FormatError::UnsupportedVersion(identity.version.major()));
    }
    let layout = StorageLayout::from_version(identity.version)?;
    if identity.role == FileRole::Attributes {
        return Err(FormatError::InvalidHeader);
    }
    Ok(layout)
}

fn parse_footer(
    footer: &[u8; FOOTER_SIZE as usize],
    magic: &[u8; 8],
) -> Result<(u64, u64, u64, u32), FormatError> {
    if &footer[..8] != magic
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
    split: bool,
    layout: StorageLayout,
) -> Result<ParsedIndex, FormatError> {
    let mut cursor = SliceCursor::new(bytes);
    if cursor.take(8)? != layout.index_magic() {
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
        let geometry_encoding = if layout != StorageLayout::V5 {
            GeometryEncoding::try_from(cursor.u8()?)?
        } else {
            GeometryEncoding::RawU32
        };
        if geometry_encoding == GeometryEncoding::Occupied && layout != StorageLayout::V6Pre2 {
            return Err(FormatError::MalformedIndex(
                "geometry encoding not supported in this revision",
            ));
        }
        let quantization_error = cursor.f64()?;
        let block_origin = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
        let scale = [cursor.f64()?, cursor.f64()?, cursor.f64()?];
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
        let logical_size = block_payload_size(point_count_usize, &schema)?;
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
            };
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
            .map(|block| {
                let first = &block.payloads[1];
                let last = block.payloads.last().expect("display payload exists");
                Ok((
                    first.offset,
                    last.offset
                        .checked_add(last.stored_length)
                        .ok_or(FormatError::FileSizeOverflow)?,
                ))
            })
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

    fn remaining_slice(&self) -> &'a [u8] {
        &self.bytes[self.position..]
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
#[path = "reader_tests.rs"]
mod tests;
