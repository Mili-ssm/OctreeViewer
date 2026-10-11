use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Seek, SeekFrom, Write},
};

use viewer_core::{
    AttributeDataType, AttributeValues, BlockId, DatasetMetadata, HierarchySemantics, NodeId,
    SpatialBounds, SpatialHierarchy, SpatialNode,
};

use crate::layout::{
    FOOTER_MAGIC, FOOTER_SIZE, FORMAT_VERSION, HEADER_MAGIC, HEADER_SIZE, INDEX_MAGIC,
    PayloadDirectoryEntry, attribute_payload_size, block_payload_size,
};
use crate::metadata::validate_metadata;
use crate::{
    Crc32, FormatError, FormatMetadata, SourcePointBlock, compress_block, crc32,
    quantize_local_positions,
};

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

/// A fully encoded block ready for ordered append to a format writer.
pub struct PreparedPointBlock {
    id: BlockId,
    point_count: u64,
    quantization_error: f64,
    origin: [f64; 3],
    scale: [f64; 3],
    world_bounds: Option<SpatialBounds>,
    bytes: Vec<u8>,
    payloads: Vec<PayloadDirectoryEntry>,
    ranges: Vec<crate::NodeAttributeRange>,
}

/// Writes independently encoded blocks and a canonical index to a fresh seekable stream.
/// A footer is emitted only after every payload and the complete index have been written.
pub struct FormatWriter<W> {
    writer: W,
    attributes: Option<W>,
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
            attributes: None,
            metadata,
            options,
            block_nodes,
            blocks: BTreeMap::new(),
            failed: false,
        })
    }

    /// Streams XYZ to the primary file and display color/columns to an optional companion.
    pub fn new_split(
        writer: W,
        mut attributes: W,
        metadata: FormatMetadata,
        options: FormatWriterOptions,
    ) -> Result<Self, FormatError> {
        if attributes.stream_position()? != 0 || attributes.seek(SeekFrom::End(0))? != 0 {
            return Err(FormatError::OutputMustBeEmpty);
        }
        let mut result = Self::new(writer, metadata, options)?;
        attributes.write_all(&flagged_header(crate::layout::ATTRIBUTE_FILE))?;
        result.writer.seek(SeekFrom::Start(0))?;
        result
            .writer
            .write_all(&flagged_header(crate::layout::SPLIT_ATTRIBUTES))?;
        result.attributes = Some(attributes);
        Ok(result)
    }

    /// Encodes and appends one block while retaining only its directory metadata afterward.
    pub fn write_block(&mut self, block: SourcePointBlock) -> Result<(), FormatError> {
        self.ensure_block_available(block.id())?;
        self.write_prepared_block(prepare_point_block(block, &self.metadata, self.options)?)
    }

    /// Appends a block encoded independently from this writer.
    pub fn write_prepared_block(
        &mut self,
        mut prepared: PreparedPointBlock,
    ) -> Result<(), FormatError> {
        self.ensure_block_available(prepared.id)?;
        let offset = self.writer.stream_position()?;
        let split_at = if self.attributes.is_some() {
            usize::try_from(prepared.payloads[0].stored_length)
                .map_err(|_| FormatError::BlockSizeOverflow)?
        } else {
            prepared.bytes.len()
        };
        let payload_length = split_at as u64;
        let write_result = (|| -> Result<(), FormatError> {
            if let Some(attributes) = self.attributes.as_mut() {
                let attribute_offset = attributes.stream_position()?;
                for payload in prepared.payloads.iter_mut().skip(1) {
                    payload.offset = attribute_offset
                        .checked_add(payload.offset - split_at as u64)
                        .ok_or(FormatError::FileSizeOverflow)?;
                }
                attributes.write_all(&prepared.bytes[split_at..])?;
            }
            self.writer.write_all(&prepared.bytes[..split_at])?;
            Ok(())
        })();
        if let Err(error) = write_result {
            self.failed = true;
            return Err(error);
        }
        self.blocks.insert(
            prepared.id,
            BlockRecord {
                offset,
                payload_length,
                point_count: prepared.point_count,
                quantization_error: prepared.quantization_error,
                origin: prepared.origin,
                scale: prepared.scale,
                world_bounds: prepared.world_bounds,
                payloads: prepared.payloads,
                ranges: prepared.ranges,
            },
        );
        Ok(())
    }

    fn ensure_block_available(&self, id: BlockId) -> Result<(), FormatError> {
        if self.failed {
            return Err(FormatError::WriterPoisoned);
        }
        if self.blocks.contains_key(&id) {
            return Err(FormatError::DuplicateBlock(id));
        }
        if !self.block_nodes.contains_key(&id) {
            return Err(FormatError::UnreferencedBlock(id));
        }
        Ok(())
    }

    /// Completes the index and footer. On any earlier error the stream has no valid footer.
    pub fn finish(self) -> Result<W, FormatError> {
        self.finish_pair().map(|(primary, _)| primary)
    }

    /// Completes the companion before committing the primary footer.
    pub fn finish_pair(mut self) -> Result<(W, Option<W>), FormatError> {
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
        if let Some(attributes) = self.attributes.as_mut() {
            let length = attributes
                .stream_position()?
                .checked_add(FOOTER_SIZE)
                .ok_or(FormatError::FileSizeOverflow)?;
            attributes.write_all(&footer_bytes(
                index_offset,
                index_length,
                length,
                index_checksum,
            ))?;
            attributes.flush()?;
        }
        let footer = footer_bytes(index_offset, index_length, file_length, index_checksum);
        self.writer.write_all(&footer)?;
        self.writer.flush()?;
        Ok((self.writer, self.attributes))
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

        let hierarchy = SpatialHierarchy::new_with_semantics(
            adjusted_nodes,
            source_hierarchy.attributes().to_vec(),
            source_hierarchy.stats().source_point_count,
            source_hierarchy.semantics(),
        )?;
        let dataset = DatasetMetadata::new(source_metadata.dataset().coordinate_frame(), hierarchy);
        let mut ranges = BTreeMap::<NodeId, Vec<crate::NodeAttributeRange>>::new();
        for (block_id, record) in &self.blocks {
            for node in &self.block_nodes[block_id] {
                let entry = ranges.entry(*node).or_default();
                for range in &record.ranges {
                    if let Some(existing) = entry
                        .iter_mut()
                        .find(|r| r.schema_index() == range.schema_index())
                    {
                        crate::ranges::merge_range(existing, range);
                    } else {
                        entry.push(range.clone());
                    }
                }
            }
        }
        Ok(source_metadata.with_dataset(dataset, ranges))
    }
}

/// Encodes one block without appending it, allowing independent blocks to be prepared concurrently.
pub fn prepare_point_block(
    block: SourcePointBlock,
    metadata: &FormatMetadata,
    options: FormatWriterOptions,
) -> Result<PreparedPointBlock, FormatError> {
    let point_count = block.positions_local().len();
    if block.colors().len() != point_count {
        return Err(FormatError::ColorCountMismatch {
            points: point_count,
            colors: block.colors().len(),
        });
    }
    let schema = metadata.dataset().hierarchy().attributes();
    let attributes = order_attributes(block.attributes(), schema, point_count)?;
    let ranges = attributes
        .iter()
        .enumerate()
        .filter_map(|(index, column)| crate::ranges::column_range(index as u32, column))
        .collect();
    let uncompressed_size = block_payload_size(point_count, schema)?;
    if uncompressed_size > options.max_uncompressed_block_bytes {
        return Err(FormatError::BlockExceedsLimit {
            requested: uncompressed_size,
            limit: options.max_uncompressed_block_bytes,
        });
    }
    let quantized =
        quantize_local_positions(block.positions_local(), options.max_quantization_error)?;
    let world_bounds = block_world_bounds(block.positions_local(), &quantized, metadata.dataset())?;
    let transform = quantized.transform();
    let quantization_error = quantized.max_error();
    let geometry_length = usize::try_from(
        (point_count as u64)
            .checked_mul(12)
            .ok_or(FormatError::BlockSizeOverflow)?,
    )
    .map_err(|_| FormatError::BlockSizeOverflow)?;
    let mut raw = encode_geometry(&quantized, geometry_length)?;
    // The u32 coordinates have been encoded; retain only their scalar transform/error
    // while compressing payloads, rather than another 12 bytes per point.
    drop(quantized);
    let mut bytes = Vec::new();
    let mut payloads = Vec::new();
    payloads
        .try_reserve_exact(schema.len() + 2)
        .map_err(|_| FormatError::AllocationFailed(schema.len() + 2))?;
    append_payload(&raw, &mut bytes, &mut payloads)?;
    raw.clear();
    for color in block.colors() {
        raw.extend_from_slice(&[color.r, color.g, color.b]);
    }
    append_payload(&raw, &mut bytes, &mut payloads)?;
    for column in attributes {
        let length = attribute_payload_size(point_count as u64, column.descriptor())?;
        let length = usize::try_from(length).map_err(|_| FormatError::SizeOutOfRange(length))?;
        raw.clear();
        raw.try_reserve_exact(length)
            .map_err(|_| FormatError::AllocationFailed(length))?;
        encode_values(column.values(), &mut raw);
        append_payload(&raw, &mut bytes, &mut payloads)?;
    }
    Ok(PreparedPointBlock {
        id: block.id(),
        point_count: u64::try_from(point_count).map_err(|_| FormatError::PointCountOutOfRange)?,
        quantization_error,
        origin: transform.origin(),
        scale: transform.scale(),
        world_bounds,
        bytes,
        payloads,
        ranges,
    })
}

#[derive(Debug, Clone)]
struct BlockRecord {
    offset: u64,
    payload_length: u64,
    point_count: u64,
    quantization_error: f64,
    origin: [f64; 3],
    scale: [f64; 3],
    world_bounds: Option<SpatialBounds>,
    payloads: Vec<PayloadDirectoryEntry>,
    ranges: Vec<crate::NodeAttributeRange>,
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
        if column.values().len() != expected {
            return Err(FormatError::AttributeLengthMismatch {
                name: descriptor.name().to_owned(),
                expected,
                actual: column.values().len(),
            });
        }
        ordered.push(column);
    }
    Ok(ordered)
}

fn encode_geometry(
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
    if bytes.len() != expected_size {
        return Err(FormatError::InternalPayloadSize {
            expected: expected_size,
            actual: bytes.len(),
        });
    }
    Ok(bytes)
}

/// Compresses one independent payload without retaining a combined raw block.
fn append_payload(
    raw: &[u8],
    bytes: &mut Vec<u8>,
    payloads: &mut Vec<PayloadDirectoryEntry>,
) -> Result<(), FormatError> {
    let encoded = compress_block(raw);
    bytes
        .try_reserve(encoded.bytes().len())
        .map_err(|_| FormatError::AllocationFailed(encoded.bytes().len()))?;
    payloads.push(PayloadDirectoryEntry {
        offset: bytes.len() as u64,
        stored_length: encoded.bytes().len() as u64,
        raw_length: raw.len() as u64,
        codec: encoded.codec(),
        checksum: crc32(encoded.bytes()),
    });
    bytes.extend_from_slice(encoded.bytes());
    Ok(())
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

fn block_world_bounds(
    source_positions: &[[f64; 3]],
    quantized: &crate::QuantizedCoordinates,
    metadata: &DatasetMetadata,
) -> Result<Option<SpatialBounds>, FormatError> {
    let origin = metadata.coordinate_frame().origin();
    let mut bounds = BoundsAccumulator::empty();
    for local in source_positions {
        bounds.include_point(std::array::from_fn(|axis| origin[axis] + local[axis]));
    }
    for local in quantized.decoded_local_positions() {
        let local = local?;
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
        for (axis, coordinate) in point.into_iter().enumerate() {
            self.min[axis] = self.min[axis].min(coordinate);
            self.max[axis] = self.max[axis].max(coordinate);
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
    flagged_header(0)
}

fn flagged_header(flags: u32) -> [u8; HEADER_SIZE as usize] {
    let mut bytes = [0; HEADER_SIZE as usize];
    bytes[..8].copy_from_slice(HEADER_MAGIC);
    bytes[8..10].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes[10..12].copy_from_slice(&(HEADER_SIZE as u16).to_le_bytes());
    bytes[12..16].copy_from_slice(&flags.to_le_bytes());
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
        // Reserved aggregate fields in the v5 layout; codecs and CRCs belong to payloads.
        index.u8(0)?;
        index.u32(0)?;
        for payload in &block.payloads {
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

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;
