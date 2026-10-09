use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Seek, SeekFrom, Write},
};

use thiserror::Error;
use viewer_core::{
    AttributeDataType, AttributeValues, BlockId, DatasetMetadata, NodeId, SpatialBounds,
    SpatialHierarchy, SpatialNode,
};

use crate::{
    BlockCodec, CodecError, Crc32, FormatMetadata, MetadataError, NodeAttributeRange,
    QuantizationError, SourcePointBlock, compress_block, crc32, quantize_local_positions,
};

pub(crate) const HEADER_SIZE: u64 = 24;
pub(crate) const FOOTER_SIZE: u64 = 40;
pub(crate) const HEADER_MAGIC: &[u8; 8] = b"OCTVWFRM";
pub(crate) const INDEX_MAGIC: &[u8; 8] = b"OCTVIDX1";
pub(crate) const FOOTER_MAGIC: &[u8; 8] = b"OCTVEND1";

/// Bounds per-block allocations made by the writer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormatWriterOptions {
    max_quantization_error: f64,
    max_uncompressed_block_bytes: u64,
}

impl FormatWriterOptions {
    pub fn new(
        max_quantization_error: f64,
        max_uncompressed_block_bytes: u64,
    ) -> Result<Self, FormatError> {
        if !max_quantization_error.is_finite() || max_quantization_error < 0.0 {
            return Err(FormatError::InvalidQuantizationLimit(
                max_quantization_error,
            ));
        }
        if max_uncompressed_block_bytes == 0 {
            return Err(FormatError::ZeroBlockLimit);
        }
        Ok(Self {
            max_quantization_error,
            max_uncompressed_block_bytes,
        })
    }
}

/// Writes independently encoded blocks and a canonical index to a fresh seekable stream.
/// A footer is emitted only after every payload and the complete index have been written.
pub struct FormatWriter<W> {
    writer: W,
    metadata: FormatMetadata,
    options: FormatWriterOptions,
    block_nodes: BTreeMap<BlockId, Vec<NodeId>>,
    blocks: BTreeMap<BlockId, BlockRecord>,
    failed: bool,
}

impl<W: Write + Seek> FormatWriter<W> {
    pub fn new(
        mut writer: W,
        metadata: FormatMetadata,
        options: FormatWriterOptions,
    ) -> Result<Self, FormatError> {
        if writer.stream_position()? != 0 || writer.seek(SeekFrom::End(0))? != 0 {
            return Err(FormatError::OutputMustBeEmpty);
        }
        writer.seek(SeekFrom::Start(0))?;
        validate_metadata(&metadata)?;
        let block_nodes = build_block_nodes(metadata.dataset().hierarchy())?;
        writer.write_all(&header_bytes())?;
        Ok(Self {
            writer,
            metadata,
            options,
            block_nodes,
            blocks: BTreeMap::new(),
            failed: false,
        })
    }

    /// Encodes and appends one block while retaining only its directory metadata afterward.
    pub fn write_block(&mut self, block: SourcePointBlock) -> Result<(), FormatError> {
        if self.failed {
            return Err(FormatError::WriterPoisoned);
        }
        if self.blocks.contains_key(&block.id()) {
            return Err(FormatError::DuplicateBlock(block.id()));
        }
        if !self.block_nodes.contains_key(&block.id()) {
            return Err(FormatError::UnreferencedBlock(block.id()));
        }
        let point_count = block.positions_local().len();
        if block.colors().len() != point_count {
            return Err(FormatError::ColorCountMismatch {
                points: point_count,
                colors: block.colors().len(),
            });
        }
        let schema = self.metadata.dataset().hierarchy().attributes();
        let attributes = order_attributes(block.attributes(), schema, point_count)?;
        let uncompressed_size = block_payload_size(point_count, schema)?;
        if uncompressed_size > self.options.max_uncompressed_block_bytes {
            return Err(FormatError::BlockExceedsLimit {
                requested: uncompressed_size,
                limit: self.options.max_uncompressed_block_bytes,
            });
        }
        let uncompressed_size_usize = usize::try_from(uncompressed_size)
            .map_err(|_| FormatError::SizeOutOfRange(uncompressed_size))?;

        let quantized =
            quantize_local_positions(block.positions_local(), self.options.max_quantization_error)?;
        let decoded = quantized.decode_local()?;
        let payload = encode_payload(&block, &attributes, &quantized, uncompressed_size_usize)?;
        let encoded = compress_block(&payload);
        let payload_checksum = crc32(encoded.bytes());
        let world_bounds =
            block_world_bounds(block.positions_local(), &decoded, self.metadata.dataset())?;
        let offset = self.writer.stream_position()?;
        let payload_length = u64::try_from(encoded.bytes().len())
            .map_err(|_| FormatError::SizeOutOfRange(encoded.bytes().len() as u64))?;
        if let Err(error) = self.writer.write_all(encoded.bytes()) {
            self.failed = true;
            return Err(FormatError::Io(error));
        }
        self.blocks.insert(
            block.id(),
            BlockRecord {
                offset,
                payload_length,
                point_count: u64::try_from(point_count)
                    .map_err(|_| FormatError::PointCountOutOfRange)?,
                quantization_error: quantized.max_error(),
                origin: quantized.transform().origin(),
                scale: quantized.transform().scale(),
                codec: encoded.codec(),
                checksum: payload_checksum,
                world_bounds,
            },
        );
        Ok(())
    }

    /// Completes the index and footer. On any earlier error the stream has no valid footer.
    pub fn finish(mut self) -> Result<W, FormatError> {
        if self.failed {
            return Err(FormatError::WriterPoisoned);
        }
        let expected: BTreeSet<_> = self.block_nodes.keys().copied().collect();
        let written: BTreeSet<_> = self.blocks.keys().copied().collect();
        if expected != written {
            let missing = expected.difference(&written).next().copied();
            return Err(FormatError::MissingBlock(missing.expect("sets differ")));
        }

        let metadata = self.adjusted_metadata()?;
        validate_metadata(&metadata)?;
        let index_offset = self.writer.stream_position()?;
        let (index_length, index_checksum) =
            write_index(&mut self.writer, &metadata, &self.blocks)?;
        let file_length = index_offset
            .checked_add(index_length)
            .and_then(|value| value.checked_add(FOOTER_SIZE))
            .ok_or(FormatError::FileSizeOverflow)?;
        let footer = footer_bytes(index_offset, index_length, file_length, index_checksum);
        self.writer.write_all(&footer)?;
        self.writer.flush()?;
        Ok(self.writer)
    }

    fn adjusted_metadata(&self) -> Result<FormatMetadata, FormatError> {
        let source_metadata = &self.metadata;
        let source_hierarchy = source_metadata.dataset().hierarchy();
        let root = source_hierarchy
            .node(source_hierarchy.root_id())
            .ok_or(FormatError::InvalidMetadata("root node is missing"))?;
        let mut nodes = source_hierarchy
            .query(root.bounds())
            .cloned()
            .collect::<Vec<_>>();
        let mut bounds = nodes
            .iter()
            .map(|node| (node.id(), BoundsAccumulator::from_bounds(node.bounds())))
            .collect::<BTreeMap<_, _>>();

        for (block_id, record) in &self.blocks {
            if let Some(block_bounds) = record.world_bounds {
                for node_id in &self.block_nodes[block_id] {
                    bounds
                        .get_mut(node_id)
                        .expect("known node")
                        .include_bounds(block_bounds);
                }
            }
        }

        let mut by_level = nodes
            .iter()
            .map(|node| (node.level(), node.id()))
            .collect::<Vec<_>>();
        by_level.sort_unstable_by(|left, right| right.cmp(left));
        for (_, node_id) in by_level {
            let node = source_hierarchy.node(node_id).expect("indexed node");
            if let Some(parent_id) = node.parent() {
                let child_bounds = bounds[&node_id].to_bounds()?;
                bounds
                    .get_mut(&parent_id)
                    .expect("validated parent")
                    .include_bounds(child_bounds);
            }
        }

        let mut adjusted_nodes = Vec::with_capacity(nodes.len());
        for node in nodes.drain(..) {
            let block_ids = node.blocks();
            let mut own_point_count = 0_u64;
            let mut block_error: f64 = 0.0;
            for block_id in block_ids {
                let record = self
                    .blocks
                    .get(block_id)
                    .ok_or(FormatError::UnreferencedBlock(*block_id))?;
                own_point_count = own_point_count
                    .checked_add(record.point_count)
                    .ok_or(FormatError::PointCountOverflow(node.id()))?;
                block_error = block_error.max(record.quantization_error);
            }
            if own_point_count != node.point_count() {
                return Err(FormatError::NodePointCountMismatch {
                    node: node.id(),
                    declared: node.point_count(),
                    blocks: own_point_count,
                });
            }
            let geometric_error = node.geometric_error() + block_error;
            if !geometric_error.is_finite() {
                return Err(FormatError::GeometricErrorOverflow(node.id()));
            }
            adjusted_nodes.push(SpatialNode::new(
                node.id(),
                node.parent(),
                node.level(),
                bounds[&node.id()].to_bounds()?,
                node.point_count(),
                geometric_error,
                node.blocks().to_vec(),
            )?);
        }

        let hierarchy = SpatialHierarchy::new(
            adjusted_nodes,
            source_hierarchy.attributes().to_vec(),
            source_hierarchy.stats().source_point_count,
        )?;
        let dataset = DatasetMetadata::new(source_metadata.dataset().coordinate_frame(), hierarchy);
        Ok(source_metadata.with_dataset(dataset))
    }
}

#[derive(Debug, Clone)]
struct BlockRecord {
    offset: u64,
    payload_length: u64,
    point_count: u64,
    quantization_error: f64,
    origin: [f64; 3],
    scale: [f64; 3],
    codec: BlockCodec,
    checksum: u32,
    world_bounds: Option<SpatialBounds>,
}

fn build_block_nodes(
    hierarchy: &SpatialHierarchy,
) -> Result<BTreeMap<BlockId, Vec<NodeId>>, FormatError> {
    let root = hierarchy
        .node(hierarchy.root_id())
        .ok_or(FormatError::InvalidMetadata("root node is missing"))?;
    let mut block_nodes = BTreeMap::<BlockId, Vec<NodeId>>::new();
    for node in hierarchy.query(root.bounds()) {
        let mut local_ids = BTreeSet::new();
        for block_id in node.blocks() {
            if !local_ids.insert(*block_id) {
                return Err(FormatError::DuplicateNodeBlock {
                    node: node.id(),
                    block: *block_id,
                });
            }
            block_nodes.entry(*block_id).or_default().push(node.id());
        }
    }
    Ok(block_nodes)
}

pub(crate) fn validate_metadata(metadata: &FormatMetadata) -> Result<(), FormatError> {
    for value in [metadata.crs(), metadata.sampling_policy()]
        .into_iter()
        .flatten()
    {
        if value.len() > u32::MAX as usize {
            return Err(FormatError::StringTooLong(value.len()));
        }
    }
    let hierarchy = metadata.dataset().hierarchy();
    if hierarchy.attributes().len() > u32::MAX as usize {
        return Err(FormatError::SchemaTooLarge);
    }
    for descriptor in hierarchy.attributes() {
        if descriptor.name().len() > u32::MAX as usize {
            return Err(FormatError::StringTooLong(descriptor.name().len()));
        }
    }
    let root = hierarchy
        .node(hierarchy.root_id())
        .ok_or(FormatError::InvalidMetadata("root node is missing"))?;
    let node_ids: BTreeSet<_> = hierarchy
        .query(root.bounds())
        .map(|node| node.id())
        .collect();
    let schema = hierarchy.attributes();
    for (node_id, ranges) in metadata.node_range_entries() {
        if !node_ids.contains(&node_id) {
            return Err(FormatError::UnknownNode(node_id));
        }
        if ranges.len() > u32::MAX as usize {
            return Err(FormatError::TooManyAttributeRanges(node_id));
        }
        let mut indices = BTreeSet::new();
        for range in ranges {
            let index = usize::try_from(range.schema_index())
                .map_err(|_| FormatError::InvalidAttributeRange(node_id))?;
            let Some(descriptor) = schema.get(index) else {
                return Err(FormatError::InvalidAttributeRange(node_id));
            };
            if !indices.insert(index)
                || !valid_range_values(range, descriptor.data_type(), descriptor.component_count())
            {
                return Err(FormatError::InvalidAttributeRange(node_id));
            }
        }
    }
    Ok(())
}

fn valid_range_values(
    range: &NodeAttributeRange,
    data_type: AttributeDataType,
    count: u32,
) -> bool {
    if range.min().data_type() != data_type || range.max().data_type() != data_type {
        return false;
    }
    let expected = count as usize;
    match (range.min(), range.max()) {
        (AttributeValues::U8(min), AttributeValues::U8(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::I8(min), AttributeValues::I8(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::U16(min), AttributeValues::U16(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::I16(min), AttributeValues::I16(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::U32(min), AttributeValues::U32(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::I32(min), AttributeValues::I32(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::U64(min), AttributeValues::U64(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::I64(min), AttributeValues::I64(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::F32(min), AttributeValues::F32(max)) => {
            min.len() == expected
                && max.len() == expected
                && min
                    .iter()
                    .zip(max)
                    .all(|(a, b)| a.is_finite() && b.is_finite() && a <= b)
        }
        (AttributeValues::F64(min), AttributeValues::F64(max)) => {
            min.len() == expected
                && max.len() == expected
                && min
                    .iter()
                    .zip(max)
                    .all(|(a, b)| a.is_finite() && b.is_finite() && a <= b)
        }
        _ => false,
    }
}

fn order_attributes<'a>(
    attributes: &'a [viewer_core::AttributeColumn],
    schema: &[viewer_core::AttributeDescriptor],
    point_count: usize,
) -> Result<Vec<&'a viewer_core::AttributeColumn>, FormatError> {
    if attributes.len() != schema.len() {
        return Err(FormatError::AttributeCountMismatch {
            expected: schema.len(),
            actual: attributes.len(),
        });
    }
    let mut ordered = Vec::with_capacity(schema.len());
    let mut seen = BTreeSet::new();
    for descriptor in schema {
        let Some(column) = attributes
            .iter()
            .find(|column| column.descriptor().name() == descriptor.name())
        else {
            return Err(FormatError::MissingAttribute(descriptor.name().to_owned()));
        };
        if !seen.insert(column.descriptor().name()) {
            return Err(FormatError::DuplicateAttribute(
                column.descriptor().name().to_owned(),
            ));
        }
        if column.descriptor().data_type() != descriptor.data_type()
            || column.descriptor().component_count() != descriptor.component_count()
            || column.values().data_type() != descriptor.data_type()
        {
            return Err(FormatError::AttributeSchemaMismatch(
                descriptor.name().to_owned(),
            ));
        }
        let expected = point_count
            .checked_mul(descriptor.component_count() as usize)
            .ok_or_else(|| FormatError::AttributeLengthOverflow(descriptor.name().to_owned()))?;
        if attribute_values_len(column.values()) != expected {
            return Err(FormatError::AttributeLengthMismatch {
                name: descriptor.name().to_owned(),
                expected,
                actual: attribute_values_len(column.values()),
            });
        }
        ordered.push(column);
    }
    Ok(ordered)
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
        let width = primitive_width(descriptor.data_type());
        let attribute_size = point_count
            .checked_mul(u64::from(descriptor.component_count()))
            .and_then(|count| count.checked_mul(width))
            .ok_or(FormatError::BlockSizeOverflow)?;
        size.checked_add(attribute_size)
            .ok_or(FormatError::BlockSizeOverflow)
    })
}

fn encode_payload(
    block: &SourcePointBlock,
    attributes: &[&viewer_core::AttributeColumn],
    quantized: &crate::QuantizedCoordinates,
    expected_size: usize,
) -> Result<Vec<u8>, FormatError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(expected_size)
        .map_err(|_| FormatError::AllocationFailed(expected_size))?;
    for coordinate in quantized.coordinates() {
        for value in coordinate {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    for color in block.colors() {
        bytes.extend_from_slice(&[color.r, color.g, color.b]);
    }
    for column in attributes {
        encode_values(column.values(), &mut bytes);
    }
    if bytes.len() != expected_size {
        return Err(FormatError::InternalPayloadSize {
            expected: expected_size,
            actual: bytes.len(),
        });
    }
    Ok(bytes)
}

fn encode_values(values: &AttributeValues, output: &mut Vec<u8>) {
    match values {
        AttributeValues::U8(values) => output.extend_from_slice(values),
        AttributeValues::I8(values) => output.extend(values.iter().map(|value| *value as u8)),
        AttributeValues::U16(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_le_bytes())),
        AttributeValues::I16(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_le_bytes())),
        AttributeValues::U32(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_le_bytes())),
        AttributeValues::I32(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_le_bytes())),
        AttributeValues::U64(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_le_bytes())),
        AttributeValues::I64(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_le_bytes())),
        AttributeValues::F32(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_bits().to_le_bytes())),
        AttributeValues::F64(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_bits().to_le_bytes())),
    }
}

fn attribute_values_len(values: &AttributeValues) -> usize {
    match values {
        AttributeValues::U8(values) => values.len(),
        AttributeValues::I8(values) => values.len(),
        AttributeValues::U16(values) => values.len(),
        AttributeValues::I16(values) => values.len(),
        AttributeValues::U32(values) => values.len(),
        AttributeValues::I32(values) => values.len(),
        AttributeValues::U64(values) => values.len(),
        AttributeValues::I64(values) => values.len(),
        AttributeValues::F32(values) => values.len(),
        AttributeValues::F64(values) => values.len(),
    }
}

fn primitive_width(data_type: AttributeDataType) -> u64 {
    match data_type {
        AttributeDataType::U8 | AttributeDataType::I8 => 1,
        AttributeDataType::U16 | AttributeDataType::I16 => 2,
        AttributeDataType::U32 | AttributeDataType::I32 | AttributeDataType::F32 => 4,
        AttributeDataType::U64 | AttributeDataType::I64 | AttributeDataType::F64 => 8,
    }
}

fn block_world_bounds(
    source_positions: &[[f64; 3]],
    decoded_local: &[[f32; 3]],
    metadata: &DatasetMetadata,
) -> Result<Option<SpatialBounds>, FormatError> {
    let origin = metadata.coordinate_frame().origin();
    let mut bounds = BoundsAccumulator::empty();
    for local in source_positions {
        bounds.include_point(std::array::from_fn(|axis| origin[axis] + local[axis]));
    }
    for local in decoded_local {
        bounds.include_point(std::array::from_fn(|axis| {
            origin[axis] + f64::from(local[axis])
        }));
    }
    if bounds.is_empty() {
        Ok(None)
    } else {
        Ok(Some(bounds.to_bounds()?))
    }
}

#[derive(Debug, Clone, Copy)]
struct BoundsAccumulator {
    min: [f64; 3],
    max: [f64; 3],
    empty: bool,
}

impl BoundsAccumulator {
    fn empty() -> Self {
        Self {
            min: [f64::INFINITY; 3],
            max: [f64::NEG_INFINITY; 3],
            empty: true,
        }
    }

    fn from_bounds(bounds: SpatialBounds) -> Self {
        Self {
            min: bounds.min(),
            max: bounds.max(),
            empty: false,
        }
    }

    fn include_point(&mut self, point: [f64; 3]) {
        for axis in 0..3 {
            self.min[axis] = self.min[axis].min(point[axis]);
            self.max[axis] = self.max[axis].max(point[axis]);
        }
        self.empty = false;
    }

    fn include_bounds(&mut self, bounds: SpatialBounds) {
        self.include_point(bounds.min());
        self.include_point(bounds.max());
    }

    fn is_empty(&self) -> bool {
        self.empty
    }

    fn to_bounds(self) -> Result<SpatialBounds, FormatError> {
        if self.empty {
            return Err(FormatError::InvalidMetadata("empty spatial bounds"));
        }
        Ok(SpatialBounds::new(self.min, self.max)?)
    }
}

fn header_bytes() -> [u8; HEADER_SIZE as usize] {
    let mut bytes = [0; HEADER_SIZE as usize];
    bytes[..8].copy_from_slice(HEADER_MAGIC);
    bytes[8..10].copy_from_slice(&1_u16.to_le_bytes());
    bytes[10..12].copy_from_slice(&(HEADER_SIZE as u16).to_le_bytes());
    let checksum = crc32(&bytes[..20]);
    bytes[20..24].copy_from_slice(&checksum.to_le_bytes());
    bytes
}

fn footer_bytes(
    index_offset: u64,
    index_length: u64,
    file_length: u64,
    index_crc: u32,
) -> [u8; 40] {
    let mut bytes = [0; 40];
    bytes[..8].copy_from_slice(FOOTER_MAGIC);
    bytes[8..16].copy_from_slice(&index_offset.to_le_bytes());
    bytes[16..24].copy_from_slice(&index_length.to_le_bytes());
    bytes[24..32].copy_from_slice(&file_length.to_le_bytes());
    bytes[32..36].copy_from_slice(&index_crc.to_le_bytes());
    let footer_crc = crc32(&bytes[..36]);
    bytes[36..40].copy_from_slice(&footer_crc.to_le_bytes());
    bytes
}

struct IndexWriter<'a, W> {
    writer: &'a mut W,
    checksum: Crc32,
    length: u64,
}

impl<W: Write> IndexWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), FormatError> {
        self.writer.write_all(bytes)?;
        self.checksum.update(bytes);
        self.length = self
            .length
            .checked_add(bytes.len() as u64)
            .ok_or(FormatError::FileSizeOverflow)?;
        Ok(())
    }

    fn u8(&mut self, value: u8) -> Result<(), FormatError> {
        self.write(&[value])
    }

    fn u32(&mut self, value: u32) -> Result<(), FormatError> {
        self.write(&value.to_le_bytes())
    }

    fn u64(&mut self, value: u64) -> Result<(), FormatError> {
        self.write(&value.to_le_bytes())
    }

    fn f64(&mut self, value: f64) -> Result<(), FormatError> {
        self.write(&value.to_bits().to_le_bytes())
    }
}

fn write_index<W: Write>(
    writer: &mut W,
    metadata: &FormatMetadata,
    blocks: &BTreeMap<BlockId, BlockRecord>,
) -> Result<(u64, u32), FormatError> {
    let mut index = IndexWriter {
        writer,
        checksum: Crc32::new(),
        length: 0,
    };
    index.write(INDEX_MAGIC)?;
    for coordinate in metadata.dataset().coordinate_frame().origin() {
        index.f64(coordinate)?;
    }
    if let Some(point_count) = metadata.dataset().hierarchy().stats().source_point_count {
        index.u8(1)?;
        index.u64(point_count)?;
    } else {
        index.u8(0)?;
    }
    write_string(&mut index, metadata.crs())?;
    write_string(&mut index, metadata.sampling_policy())?;

    let hierarchy = metadata.dataset().hierarchy();
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
        for coordinate in node.bounds().min().into_iter().chain(node.bounds().max()) {
            index.f64(coordinate)?;
        }
        index.u64(node.point_count())?;
        index.f64(node.geometric_error())?;
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
        index.f64(block.quantization_error)?;
        for value in block.origin {
            index.f64(value)?;
        }
        for value in block.scale {
            index.f64(value)?;
        }
        index.u8(block.codec as u8)?;
        index.u32(block.checksum)?;
    }
    let length = index.length;
    let checksum = index.checksum.finish();
    Ok((length, checksum))
}

fn write_string<W: Write>(
    index: &mut IndexWriter<'_, W>,
    value: Option<&str>,
) -> Result<(), FormatError> {
    let bytes = value.unwrap_or_default().as_bytes();
    index.u32(u32::try_from(bytes.len()).map_err(|_| FormatError::StringTooLong(bytes.len()))?)?;
    index.write(bytes)
}

fn encode_index_values<W: Write>(
    index: &mut IndexWriter<'_, W>,
    values: &AttributeValues,
) -> Result<(), FormatError> {
    match values {
        AttributeValues::U8(values) => index.write(values)?,
        AttributeValues::I8(values) => {
            index.write(&values.iter().map(|v| *v as u8).collect::<Vec<_>>())?
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

fn attribute_type_tag(data_type: AttributeDataType) -> u8 {
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

#[derive(Debug, Error)]
pub enum FormatError {
    #[error("I/O error while accessing the format: {0}")]
    Io(#[from] std::io::Error),
    #[error("format file is shorter than its header and footer")]
    FileTooShort,
    #[error("format header is malformed or has an invalid checksum")]
    InvalidHeader,
    #[error("format version {0} is not supported")]
    UnsupportedVersion(u16),
    #[error("format footer is malformed, truncated or inconsistent")]
    InvalidFooter,
    #[error("{0} checksum does not match")]
    InvalidChecksum(&'static str),
    #[error("block {0:?} checksum does not match")]
    InvalidBlockChecksum(BlockId),
    #[error("format reader limits must be greater than zero")]
    InvalidReaderLimits,
    #[error("format index needs {requested} bytes, exceeding limit {limit}")]
    IndexExceedsLimit { requested: u64, limit: u64 },
    #[error("format count for {kind} is {requested}, exceeding limit {limit}")]
    CountLimit {
        kind: &'static str,
        requested: u64,
        limit: u64,
    },
    #[error("format string has {requested} bytes, exceeding limit {limit}")]
    StringExceedsLimit { requested: u32, limit: u32 },
    #[error("format contains invalid UTF-8")]
    InvalidUtf8,
    #[error("format index is malformed: {0}")]
    MalformedIndex(&'static str),
    #[error("format block is malformed: {0}")]
    MalformedBlock(&'static str),
    #[error("format index ended before a field was complete")]
    TruncatedIndex,
    #[error("format schema is invalid")]
    InvalidSchema,
    #[error("format uses unsupported tag {tag} for {field}")]
    InvalidTag { field: &'static str, tag: u8 },
    #[error("requested block {0:?} does not exist")]
    UnknownBlock(BlockId),
    #[error("invalid quantization error limit {0}")]
    InvalidQuantizationLimit(f64),
    #[error("maximum uncompressed block size must be greater than zero")]
    ZeroBlockLimit,
    #[error("output stream must be empty and positioned at offset zero")]
    OutputMustBeEmpty,
    #[error("writer cannot continue after a partial I/O failure")]
    WriterPoisoned,
    #[error("format metadata is invalid: {0}")]
    InvalidMetadata(&'static str),
    #[error("metadata string has {0} bytes, above the format limit")]
    StringTooLong(usize),
    #[error("attribute schema has too many entries")]
    SchemaTooLarge,
    #[error("hierarchy has too many nodes")]
    TooManyNodes,
    #[error("format contains too many blocks")]
    TooManyBlocks,
    #[error("node {0:?} contains too many block references")]
    TooManyNodeBlocks(NodeId),
    #[error("node {0:?} contains too many attribute ranges")]
    TooManyAttributeRanges(NodeId),
    #[error("node {0:?} has attribute range metadata with an invalid schema index or values")]
    InvalidAttributeRange(NodeId),
    #[error("attribute ranges are repeated for node {0:?}")]
    DuplicateNodeRanges(NodeId),
    #[error("metadata contains unknown node {0:?}")]
    UnknownNode(NodeId),
    #[error("block {block:?} is referenced more than once in node {node:?}")]
    DuplicateNodeBlock { node: NodeId, block: BlockId },
    #[error("block {0:?} is referenced by the hierarchy zero times")]
    UnreferencedBlock(BlockId),
    #[error("block {0:?} was written more than once")]
    DuplicateBlock(BlockId),
    #[error("block {0:?} was referenced but not written")]
    MissingBlock(BlockId),
    #[error("block has {points} positions but {colors} colors")]
    ColorCountMismatch { points: usize, colors: usize },
    #[error("block contains {actual} attributes, expected {expected}")]
    AttributeCountMismatch { expected: usize, actual: usize },
    #[error("block is missing attribute {0:?}")]
    MissingAttribute(String),
    #[error("block repeats attribute {0:?}")]
    DuplicateAttribute(String),
    #[error("block attribute {0:?} does not match the dataset schema")]
    AttributeSchemaMismatch(String),
    #[error("attribute {name:?} has {actual} values, expected {expected}")]
    AttributeLengthMismatch {
        name: String,
        expected: usize,
        actual: usize,
    },
    #[error("attribute {0:?} component count overflows the platform length")]
    AttributeLengthOverflow(String),
    #[error("block point count cannot be represented as u64")]
    PointCountOutOfRange,
    #[error("block payload size overflows u64")]
    BlockSizeOverflow,
    #[error("block needs {requested} raw bytes, exceeding limit {limit}")]
    BlockExceedsLimit { requested: u64, limit: u64 },
    #[error("size {0} cannot be represented by this platform")]
    SizeOutOfRange(u64),
    #[error("could not reserve {0} bytes for an encoded block")]
    AllocationFailed(usize),
    #[error("encoded payload has {actual} bytes, expected {expected}")]
    InternalPayloadSize { expected: usize, actual: usize },
    #[error("point count overflows for node {0:?}")]
    PointCountOverflow(NodeId),
    #[error("node {node:?} declares {declared} points but its blocks contain {blocks}")]
    NodePointCountMismatch {
        node: NodeId,
        declared: u64,
        blocks: u64,
    },
    #[error("quantization error overflow for node {0:?}")]
    GeometricErrorOverflow(NodeId),
    #[error("file size overflows u64")]
    FileSizeOverflow,
    #[error(transparent)]
    Quantization(#[from] QuantizationError),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error(transparent)]
    Spatial(#[from] viewer_core::SpatialError),
    #[error(transparent)]
    CoordinateFrame(#[from] viewer_core::CoordinateFrameError),
    #[error(transparent)]
    Metadata(#[from] MetadataError),
    #[error(transparent)]
    Dataset(#[from] viewer_core::DatasetError),
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use viewer_core::{
        AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId,
        ColorRgb, CoordinateFrame, DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy,
        SpatialNode,
    };

    use crate::{FormatMetadata, SourcePointBlock, crc32};

    use super::{FormatError, FormatWriter, FormatWriterOptions};

    fn sample_metadata() -> FormatMetadata {
        let origin = [1_000_000_000_000.0, -2_000_000_000_000.0, 10.0];
        let bounds =
            SpatialBounds::new(origin, [origin[0] + 2.0, origin[1] + 1.0, origin[2] + 1.0])
                .unwrap();
        let block = BlockId::new(7);
        let root = SpatialNode::new(NodeId::new(0), None, 0, bounds, 2, 0.25, vec![block]).unwrap();
        let intensity = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
        let hierarchy = SpatialHierarchy::new(vec![root], vec![intensity], Some(2)).unwrap();
        FormatMetadata::new(
            DatasetMetadata::new(CoordinateFrame::new(origin).unwrap(), hierarchy),
            Some("EPSG:4978".to_owned()),
            Some("uniform voxel sampling".to_owned()),
            Vec::new(),
        )
        .unwrap()
    }

    fn sample_block() -> SourcePointBlock {
        let descriptor = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
        let attribute =
            AttributeColumn::new(descriptor, AttributeValues::U16(vec![100, 200]), 2).unwrap();
        SourcePointBlock::new(
            BlockId::new(7),
            vec![[0.25, 0.5, 0.0], [1.5, 0.75, 1.0]],
            vec![ColorRgb::new(10, 20, 30), ColorRgb::new(40, 50, 60)],
            vec![attribute],
        )
    }

    fn options() -> FormatWriterOptions {
        FormatWriterOptions::new(0.001, 1024).unwrap()
    }

    #[test]
    fn writes_a_closed_file_with_checksums_and_quantized_block() {
        let writer =
            FormatWriter::new(Cursor::new(Vec::new()), sample_metadata(), options()).unwrap();
        let mut writer = writer;
        writer.write_block(sample_block()).unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        assert_eq!(&bytes[..8], b"OCTVWFRM");
        assert_eq!(
            u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
            crc32(&bytes[..20])
        );
        assert_eq!(&bytes[bytes.len() - 40..bytes.len() - 32], b"OCTVEND1");
        assert_eq!(
            u64::from_le_bytes(bytes[bytes.len() - 16..bytes.len() - 8].try_into().unwrap()),
            bytes.len() as u64
        );
        let index_offset = u64::from_le_bytes(
            bytes[bytes.len() - 32..bytes.len() - 24]
                .try_into()
                .unwrap(),
        ) as usize;
        let index_length = u64::from_le_bytes(
            bytes[bytes.len() - 24..bytes.len() - 16]
                .try_into()
                .unwrap(),
        ) as usize;
        let index = &bytes[index_offset..index_offset + index_length];
        assert_eq!(&index[..8], b"OCTVIDX1");
        assert_eq!(
            u32::from_le_bytes(bytes[bytes.len() - 8..bytes.len() - 4].try_into().unwrap()),
            crc32(index)
        );
        assert_eq!(
            u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap()),
            crc32(&bytes[bytes.len() - 40..bytes.len() - 4])
        );
    }

    #[test]
    fn refuses_to_finalize_when_a_referenced_block_is_missing() {
        let writer =
            FormatWriter::new(Cursor::new(Vec::new()), sample_metadata(), options()).unwrap();

        assert!(matches!(
            writer.finish(),
            Err(FormatError::MissingBlock(block)) if block.get() == 7
        ));
    }

    #[test]
    fn validates_attribute_schema_and_point_counts_before_writing_a_block() {
        let mut writer =
            FormatWriter::new(Cursor::new(Vec::new()), sample_metadata(), options()).unwrap();
        let sample = sample_block();
        let wrong_descriptor =
            AttributeDescriptor::new("classification", AttributeDataType::U8, 1).unwrap();
        let wrong_column =
            AttributeColumn::new(wrong_descriptor, AttributeValues::U8(vec![1, 2]), 2).unwrap();
        let block = SourcePointBlock::new(
            sample.id(),
            sample.positions_local().to_vec(),
            sample.colors().to_vec(),
            vec![wrong_column],
        );

        assert!(matches!(
            writer.write_block(block),
            Err(FormatError::MissingAttribute(name)) if name == "intensity"
        ));
    }

    #[test]
    fn refuses_a_quantization_error_above_the_declared_limit() {
        let mut writer = FormatWriter::new(
            Cursor::new(Vec::new()),
            sample_metadata(),
            FormatWriterOptions::new(0.0, 1024).unwrap(),
        )
        .unwrap();
        let descriptor = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
        let attribute =
            AttributeColumn::new(descriptor, AttributeValues::U16(vec![100, 150, 200]), 3).unwrap();
        let block = SourcePointBlock::new(
            BlockId::new(7),
            vec![[0.25, 0.5, 0.0], [0.8, 0.6, 0.3], [1.5, 0.75, 1.0]],
            vec![
                ColorRgb::new(10, 20, 30),
                ColorRgb::new(20, 30, 40),
                ColorRgb::new(40, 50, 60),
            ],
            vec![attribute],
        );

        assert!(matches!(
            writer.write_block(block),
            Err(FormatError::Quantization(_))
        ));
    }
}
