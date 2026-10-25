use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Seek, SeekFrom, Write},
};

use viewer_core::{
    AttributeDataType, AttributeValues, BlockId, DatasetMetadata, HierarchySemantics, NodeId,
    SpatialBounds, SpatialHierarchy, SpatialNode,
};

use crate::geometry::{GeometryEncoding, encode_delta};
use crate::layout::{
    FOOTER_SIZE, HEADER_SIZE, PayloadDirectoryEntry, StorageLayout, attribute_payload_size,
    block_payload_size,
};
use crate::metadata::validate_metadata;
use crate::version::FileRole;
use crate::{
    Crc32, FormatError, FormatIdentity, FormatMetadata, FormatVersion, SourcePointBlock,
    compress_block, crc32, quantize_local_positions,
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
    layout: StorageLayout,
    geometry_encoding: GeometryEncoding,
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

const WRITER_RECORD_MAGIC: &[u8; 8] = b"OCTVWRC1";
const WRITER_RECORD_VERSION: u16 = 1;

impl PreparedPointBlock {
    /// Capacity of the retained compressed payload buffer, excluding metadata and allocator overhead.
    pub fn encoded_payload_capacity_bytes(&self) -> usize {
        self.bytes.capacity()
    }
}

/// Writes independently encoded blocks and a canonical index to a fresh seekable stream.
/// A footer is emitted only after every payload and the complete index have been written.
pub struct FormatWriter<W> {
    writer: W,
    attributes: Option<W>,
    metadata: FormatMetadata,
    options: FormatWriterOptions,
    layout: StorageLayout,
    block_nodes: BTreeMap<BlockId, Vec<NodeId>>,
    blocks: BTreeMap<BlockId, BlockRecord>,
    failed: bool,
    resume_boundary_pending: bool,
}

impl<W: Write + Seek> FormatWriter<W> {
    /// Encodes one committed block directory record for an external append-only journal.
    /// The caller owns journal durability and candidate stream syncing.
    pub fn checkpoint_record(&self, id: BlockId) -> Result<Vec<u8>, FormatError> {
        if self.failed {
            return Err(FormatError::WriterPoisoned);
        }
        let record = self.blocks.get(&id).ok_or(FormatError::UnknownBlock(id))?;
        encode_checkpoint_record(id, record)
    }

    /// Returns the candidate streams so the caller can flush/sync and reopen them.
    pub fn into_candidate_streams(self) -> (W, Option<W>) {
        (self.writer, self.attributes)
    }

    /// Opens existing candidate streams for journal replay. This checks both headers and
    /// performs no writes. Replay records in append order before writing another block.
    pub fn resume_for_version(
        version: FormatVersion,
        mut writer: W,
        mut attributes: Option<W>,
        metadata: FormatMetadata,
        options: FormatWriterOptions,
    ) -> Result<Self, FormatError>
    where
        W: std::io::Read,
    {
        let layout = StorageLayout::from_version(version)?;
        validate_metadata(&metadata)?;
        let block_nodes = build_block_nodes(metadata.dataset().hierarchy())?;
        let primary_end = writer.seek(SeekFrom::End(0))?;
        let attr_end = attributes
            .as_mut()
            .map(|s| s.seek(SeekFrom::End(0)))
            .transpose()?;
        let expected_header = header_for_role(
            version,
            if attributes.is_some() {
                FileRole::SplitPrimary
            } else {
                FileRole::Monolithic
            },
        )?;
        writer.seek(SeekFrom::Start(0))?;
        let mut header = vec![0; HEADER_SIZE as usize];
        std::io::Read::read_exact(&mut writer, &mut header)?;
        if header.as_slice() != expected_header {
            return Err(FormatError::InvalidHeader);
        }
        if let Some(stream) = attributes.as_mut() {
            stream.seek(SeekFrom::Start(0))?;
            let mut attr_header = vec![0; HEADER_SIZE as usize];
            std::io::Read::read_exact(stream, &mut attr_header)?;
            if attr_header.as_slice() != header_for_role(version, FileRole::Attributes)? {
                return Err(FormatError::InvalidHeader);
            }
        }
        if primary_end < HEADER_SIZE || attr_end.is_some_and(|end| end < HEADER_SIZE) {
            return Err(FormatError::MalformedIndex("writer resume stream length"));
        }
        writer.seek(SeekFrom::Start(HEADER_SIZE))?;
        if let Some(stream) = attributes.as_mut() {
            stream.seek(SeekFrom::Start(HEADER_SIZE))?;
        }
        Ok(Self {
            writer,
            attributes,
            metadata,
            options,
            layout,
            block_nodes,
            blocks: BTreeMap::new(),
            failed: false,
            resume_boundary_pending: true,
        })
    }

    /// Restores one validated journal record. Payload bytes must already be present.
    pub fn restore_committed_record(&mut self, bytes: &[u8]) -> Result<BlockId, FormatError>
    where
        W: std::io::Read,
    {
        if self.failed {
            return Err(FormatError::WriterPoisoned);
        }
        let (id, record) = decode_checkpoint_record(
            bytes,
            self.metadata.dataset().hierarchy().attributes(),
            self.attributes.is_some(),
        )?;
        self.ensure_block_available(id)?;
        if record.offset != self.writer.stream_position()?
            || record.payload_length != record.payloads[0].stored_length
            || record.payloads.len() != self.metadata.dataset().hierarchy().attributes().len() + 2
        {
            return Err(FormatError::MalformedIndex(
                "writer checkpoint block order or identity",
            ));
        }
        validate_checkpoint_record(
            &record,
            self.attributes.is_some(),
            self.metadata.dataset().hierarchy().attributes(),
        )?;
        let primary_end = self.writer.seek(SeekFrom::End(0))?;
        let primary_next = record
            .offset
            .checked_add(record.payload_length)
            .ok_or(FormatError::FileSizeOverflow)?;
        if primary_next > primary_end {
            return Err(FormatError::MalformedIndex(
                "writer checkpoint primary bounds",
            ));
        }
        self.writer.seek(SeekFrom::Start(primary_next))?;
        if let Some(attributes) = self.attributes.as_mut() {
            let mut next = attributes.stream_position()?;
            for payload in record.payloads.iter().skip(1) {
                if payload.offset != next {
                    return Err(FormatError::MalformedIndex(
                        "writer checkpoint companion order",
                    ));
                }
                next = next
                    .checked_add(payload.stored_length)
                    .ok_or(FormatError::FileSizeOverflow)?;
            }
            let end = attributes.seek(SeekFrom::End(0))?;
            if next > end {
                return Err(FormatError::MalformedIndex(
                    "writer checkpoint companion bounds",
                ));
            }
            attributes.seek(SeekFrom::Start(next))?;
        } else {
            let end = self.writer.seek(SeekFrom::End(0))?;
            let total = record.payloads.iter().try_fold(0u64, |n, payload| {
                n.checked_add(payload.stored_length)
                    .ok_or(FormatError::FileSizeOverflow)
            })?;
            let next = record
                .offset
                .checked_add(total)
                .ok_or(FormatError::FileSizeOverflow)?;
            if next > end {
                return Err(FormatError::MalformedIndex(
                    "writer checkpoint block bounds",
                ));
            }
            self.writer.seek(SeekFrom::Start(next))?;
        }
        self.blocks.insert(id, record);
        Ok(id)
    }

    pub fn format_version(&self) -> FormatVersion {
        self.layout.version()
    }

    /// Select the output backend explicitly. Unsupported identities fail before I/O.
    pub fn new_for_version(
        version: crate::FormatVersion,
        writer: W,
        metadata: FormatMetadata,
        options: FormatWriterOptions,
    ) -> Result<Self, FormatError> {
        let layout = StorageLayout::from_version(version)?;
        Self::new_with_layout(writer, metadata, options, layout)
    }

    /// Select a backend for a primary/companion pair before touching either sink.
    pub fn new_split_for_version(
        version: crate::FormatVersion,
        writer: W,
        attributes: W,
        metadata: FormatMetadata,
        options: FormatWriterOptions,
    ) -> Result<Self, FormatError> {
        let layout = StorageLayout::from_version(version)?;
        Self::new_split_with_layout(writer, attributes, metadata, options, layout)
    }

    pub fn new(
        writer: W,
        metadata: FormatMetadata,
        options: FormatWriterOptions,
    ) -> Result<Self, FormatError> {
        Self::new_with_layout(writer, metadata, options, StorageLayout::V5)
    }

    fn new_with_layout(
        mut writer: W,
        metadata: FormatMetadata,
        options: FormatWriterOptions,
        layout: StorageLayout,
    ) -> Result<Self, FormatError> {
        if writer.stream_position()? != 0 || writer.seek(SeekFrom::End(0))? != 0 {
            return Err(FormatError::OutputMustBeEmpty);
        }
        writer.seek(SeekFrom::Start(0))?;
        validate_metadata(&metadata)?;
        let block_nodes = build_block_nodes(metadata.dataset().hierarchy())?;
        writer.write_all(&header_for_role(layout.version(), FileRole::Monolithic)?)?;
        Ok(Self {
            writer,
            attributes: None,
            metadata,
            options,
            layout,
            block_nodes,
            blocks: BTreeMap::new(),
            failed: false,
            resume_boundary_pending: false,
        })
    }

    /// Streams XYZ to the primary file and display color/columns to an optional companion.
    pub fn new_split(
        writer: W,
        attributes: W,
        metadata: FormatMetadata,
        options: FormatWriterOptions,
    ) -> Result<Self, FormatError> {
        Self::new_split_with_layout(writer, attributes, metadata, options, StorageLayout::V5)
    }

    fn new_split_with_layout(
        writer: W,
        mut attributes: W,
        metadata: FormatMetadata,
        options: FormatWriterOptions,
        layout: StorageLayout,
    ) -> Result<Self, FormatError> {
        if attributes.stream_position()? != 0 || attributes.seek(SeekFrom::End(0))? != 0 {
            return Err(FormatError::OutputMustBeEmpty);
        }
        let mut result = Self::new_with_layout(writer, metadata, options, layout)?;
        attributes.write_all(&header_for_role(layout.version(), FileRole::Attributes)?)?;
        result.writer.seek(SeekFrom::Start(0))?;
        result
            .writer
            .write_all(&header_for_role(layout.version(), FileRole::SplitPrimary)?)?;
        result.attributes = Some(attributes);
        Ok(result)
    }

    /// Encodes and appends one block while retaining only its directory metadata afterward.
    pub fn write_block(&mut self, block: SourcePointBlock) -> Result<(), FormatError> {
        self.ensure_block_available(block.id())?;
        self.write_prepared_block(prepare_point_block_for_version(
            self.layout.version(),
            block,
            &self.metadata,
            self.options,
        )?)
    }

    /// Appends a block encoded independently from this writer.
    pub fn write_prepared_block(
        &mut self,
        mut prepared: PreparedPointBlock,
    ) -> Result<(), FormatError> {
        if prepared.layout != self.layout {
            return Err(FormatError::PreparedVersionMismatch {
                writer: self.layout.version(),
                block: prepared.layout.version(),
            });
        }
        self.ensure_append_boundary()?;
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
                geometry_encoding: prepared.geometry_encoding,
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

    fn ensure_append_boundary(&mut self) -> Result<(), FormatError> {
        if !self.resume_boundary_pending {
            return Ok(());
        }
        let primary = self.writer.stream_position()?;
        if self.writer.seek(SeekFrom::End(0))? != primary {
            return Err(FormatError::MalformedIndex(
                "writer candidate primary cursor",
            ));
        }
        self.writer.seek(SeekFrom::Start(primary))?;
        if let Some(attributes) = self.attributes.as_mut() {
            let cursor = attributes.stream_position()?;
            if attributes.seek(SeekFrom::End(0))? != cursor {
                return Err(FormatError::MalformedIndex(
                    "writer candidate companion cursor",
                ));
            }
            attributes.seek(SeekFrom::Start(cursor))?;
        }
        self.resume_boundary_pending = false;
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
            write_index(&mut self.writer, &metadata, &self.blocks, self.layout)?;
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
                self.layout.footer_magic(),
                index_offset,
                index_length,
                length,
                index_checksum,
            ))?;
            attributes.flush()?;
        }
        let footer = footer_bytes(
            self.layout.footer_magic(),
            index_offset,
            index_length,
            file_length,
            index_checksum,
        );
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

fn validate_checkpoint_record(
    record: &BlockRecord,
    split: bool,
    schema: &[viewer_core::AttributeDescriptor],
) -> Result<(), FormatError> {
    if !record.quantization_error.is_finite()
        || record.quantization_error < 0.0
        || record
            .origin
            .iter()
            .chain(record.scale.iter())
            .any(|v| !v.is_finite())
        || record.scale.iter().any(|v| *v < 0.0)
    {
        return Err(FormatError::MalformedIndex(
            "writer checkpoint numeric metadata",
        ));
    }
    for (index, payload) in record.payloads.iter().enumerate() {
        payload
            .offset
            .checked_add(payload.stored_length)
            .ok_or(FormatError::FileSizeOverflow)?;
        if payload.stored_length == 0 && payload.raw_length != 0 {
            return Err(FormatError::MalformedIndex(
                "writer checkpoint empty payload",
            ));
        }
        if index == 0 {
            record
                .geometry_encoding
                .validate_raw_length(record.point_count, payload.raw_length)?;
        } else if index == 1 {
            let expected = record
                .point_count
                .checked_mul(3)
                .ok_or(FormatError::BlockSizeOverflow)?;
            if payload.raw_length != expected {
                return Err(FormatError::MalformedIndex(
                    "writer checkpoint color raw length",
                ));
            }
        } else {
            let descriptor = schema.get(index - 2).ok_or(FormatError::MalformedIndex(
                "writer checkpoint attribute schema",
            ))?;
            if payload.raw_length != attribute_payload_size(record.point_count, descriptor)? {
                return Err(FormatError::MalformedIndex(
                    "writer checkpoint attribute raw length",
                ));
            }
        }
        if !split || index == 0 {
            let expected_offset = record.payloads[..index].iter().try_fold(0u64, |at, item| {
                at.checked_add(item.stored_length)
                    .ok_or(FormatError::FileSizeOverflow)
            })?;
            if payload.offset != expected_offset {
                return Err(FormatError::MalformedIndex(
                    "writer checkpoint payload offset",
                ));
            }
        }
    }
    Ok(())
}

fn encode_checkpoint_record(id: BlockId, record: &BlockRecord) -> Result<Vec<u8>, FormatError> {
    let mut out = Vec::new();
    out.extend_from_slice(WRITER_RECORD_MAGIC);
    out.extend_from_slice(&WRITER_RECORD_VERSION.to_le_bytes());
    out.extend_from_slice(&id.get().to_le_bytes());
    out.extend_from_slice(&record.offset.to_le_bytes());
    out.extend_from_slice(&record.payload_length.to_le_bytes());
    out.extend_from_slice(&record.point_count.to_le_bytes());
    out.push(record.geometry_encoding as u8);
    out.extend_from_slice(&record.quantization_error.to_bits().to_le_bytes());
    for value in record.origin.into_iter().chain(record.scale) {
        out.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    if let Some(bounds) = record.world_bounds {
        out.push(1);
        for value in bounds.min().into_iter().chain(bounds.max()) {
            out.extend_from_slice(&value.to_bits().to_le_bytes());
        }
    } else {
        out.push(0);
    }
    put_count(&mut out, record.payloads.len())?;
    for payload in &record.payloads {
        for value in [payload.offset, payload.stored_length, payload.raw_length] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.push(payload.codec as u8);
        out.extend_from_slice(&payload.checksum.to_le_bytes());
    }
    put_count(&mut out, record.ranges.len())?;
    for range in &record.ranges {
        out.extend_from_slice(&range.schema_index().to_le_bytes());
        encode_values(&range.min, &mut out);
        encode_values(&range.max, &mut out);
    }
    let checksum = crc32(&out);
    out.extend_from_slice(&checksum.to_le_bytes());
    Ok(out)
}

fn decode_checkpoint_record(
    bytes: &[u8],
    schema: &[viewer_core::AttributeDescriptor],
    split: bool,
) -> Result<(BlockId, BlockRecord), FormatError> {
    let mut maximum_length = 160usize
        .checked_add(
            schema
                .len()
                .checked_add(2)
                .ok_or(FormatError::SchemaTooLarge)?
                .checked_mul(29)
                .ok_or(FormatError::BlockSizeOverflow)?,
        )
        .ok_or(FormatError::BlockSizeOverflow)?;
    for descriptor in schema {
        let width = usize::try_from(crate::layout::primitive_width(descriptor.data_type()))
            .map_err(|_| FormatError::BlockSizeOverflow)?;
        maximum_length = maximum_length
            .checked_add(4)
            .and_then(|n| {
                n.checked_add(
                    (descriptor.component_count() as usize)
                        .checked_mul(width)?
                        .checked_mul(2)?,
                )
            })
            .ok_or(FormatError::BlockSizeOverflow)?;
    }
    if bytes.len() > maximum_length {
        return Err(FormatError::MalformedIndex(
            "writer checkpoint exceeds schema bounds",
        ));
    }
    if bytes.len() < 14 {
        return Err(FormatError::MalformedIndex("truncated writer checkpoint"));
    }
    let data_len = bytes
        .len()
        .checked_sub(4)
        .ok_or(FormatError::MalformedIndex("truncated writer checkpoint"))?;
    let expected_crc = u32::from_le_bytes(
        bytes[data_len..]
            .try_into()
            .map_err(|_| FormatError::MalformedIndex("truncated writer checkpoint"))?,
    );
    if crc32(&bytes[..data_len]) != expected_crc {
        return Err(FormatError::InvalidChecksum("writer checkpoint"));
    }
    let mut input = CheckpointInput {
        bytes: &bytes[..data_len],
        at: 0,
    };
    if input.take(8)? != WRITER_RECORD_MAGIC || input.u16()? != WRITER_RECORD_VERSION {
        return Err(FormatError::MalformedIndex("writer checkpoint version"));
    }
    let id = BlockId::new(input.u64()?);
    let offset = input.u64()?;
    let payload_length = input.u64()?;
    let point_count = input.u64()?;
    let geometry_encoding = crate::GeometryEncoding::try_from(input.u8()?)?;
    let quantization_error = f64::from_bits(input.u64()?);
    let origin = input.triple()?;
    let scale = input.triple()?;
    let world_bounds = match input.u8()? {
        0 => None,
        1 => Some(SpatialBounds::new(input.triple()?, input.triple()?)?),
        tag => {
            return Err(FormatError::InvalidTag {
                field: "checkpoint bounds",
                tag,
            });
        }
    };
    let payload_count = input.count(
        schema
            .len()
            .checked_add(2)
            .ok_or(FormatError::SchemaTooLarge)?,
    )?;
    let mut payloads = Vec::new();
    payloads
        .try_reserve_exact(payload_count)
        .map_err(|_| FormatError::AllocationFailed(payload_count))?;
    for _ in 0..payload_count {
        payloads.push(PayloadDirectoryEntry {
            offset: input.u64()?,
            stored_length: input.u64()?,
            raw_length: input.u64()?,
            codec: crate::BlockCodec::try_from(input.u8()?)?,
            checksum: input.u32()?,
        });
    }
    if payload_count != schema.len() + 2 {
        return Err(FormatError::MalformedIndex(
            "writer checkpoint payload count",
        ));
    }
    let range_count = input.count(schema.len())?;
    let mut ranges = Vec::new();
    ranges
        .try_reserve_exact(range_count)
        .map_err(|_| FormatError::AllocationFailed(range_count))?;
    let mut seen = BTreeSet::new();
    for _ in 0..range_count {
        let schema_index = input.u32()?;
        let descriptor = schema
            .get(schema_index as usize)
            .ok_or(FormatError::MalformedIndex(
                "writer checkpoint range schema",
            ))?;
        if !seen.insert(schema_index) {
            return Err(FormatError::MalformedIndex(
                "writer checkpoint duplicate range",
            ));
        }
        let count = descriptor.component_count() as usize;
        let min = input.values(descriptor.data_type(), count)?;
        let max = input.values(descriptor.data_type(), count)?;
        for component in 0..count {
            let low = min
                .scalar_as_f64(component)
                .ok_or(FormatError::MalformedIndex("writer checkpoint range"))?;
            let high = max
                .scalar_as_f64(component)
                .ok_or(FormatError::MalformedIndex("writer checkpoint range"))?;
            if !low.is_finite() || !high.is_finite() || low > high {
                return Err(FormatError::MalformedIndex(
                    "writer checkpoint range values",
                ));
            }
        }
        ranges.push(crate::NodeAttributeRange::new(schema_index, min, max));
    }
    if input.at != input.bytes.len() {
        return Err(FormatError::MalformedIndex(
            "writer checkpoint trailing bytes",
        ));
    }
    let record = BlockRecord {
        offset,
        payload_length,
        point_count,
        geometry_encoding,
        quantization_error,
        origin,
        scale,
        world_bounds,
        payloads,
        ranges,
    };
    validate_checkpoint_record(&record, split, schema)?;
    Ok((id, record))
}

fn put_count(out: &mut Vec<u8>, count: usize) -> Result<(), FormatError> {
    out.extend_from_slice(
        &u32::try_from(count)
            .map_err(|_| FormatError::TooManyBlocks)?
            .to_le_bytes(),
    );
    Ok(())
}

struct CheckpointInput<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl CheckpointInput<'_> {
    fn take(&mut self, len: usize) -> Result<&[u8], FormatError> {
        let end = self.at.checked_add(len).ok_or(FormatError::MalformedIndex(
            "writer checkpoint length overflow",
        ))?;
        let out = self
            .bytes
            .get(self.at..end)
            .ok_or(FormatError::MalformedIndex("truncated writer checkpoint"))?;
        self.at = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, FormatError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().map_err(
            |_| FormatError::MalformedIndex("truncated writer checkpoint"),
        )?))
    }
    fn u32(&mut self) -> Result<u32, FormatError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(
            |_| FormatError::MalformedIndex("truncated writer checkpoint"),
        )?))
    }
    fn u64(&mut self) -> Result<u64, FormatError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(
            |_| FormatError::MalformedIndex("truncated writer checkpoint"),
        )?))
    }
    fn triple(&mut self) -> Result<[f64; 3], FormatError> {
        Ok([
            f64::from_bits(self.u64()?),
            f64::from_bits(self.u64()?),
            f64::from_bits(self.u64()?),
        ])
    }
    fn count(&mut self, max: usize) -> Result<usize, FormatError> {
        let count = usize::try_from(self.u32()?)
            .map_err(|_| FormatError::MalformedIndex("writer checkpoint count"))?;
        if count > max {
            return Err(FormatError::MalformedIndex(
                "writer checkpoint count exceeds schema",
            ));
        }
        Ok(count)
    }
    fn values(
        &mut self,
        data_type: AttributeDataType,
        count: usize,
    ) -> Result<AttributeValues, FormatError> {
        let bytes = count
            .checked_mul(crate::layout::primitive_width(data_type) as usize)
            .ok_or(FormatError::MalformedIndex(
                "writer checkpoint range length overflow",
            ))?;
        let raw = self.take(bytes)?;
        macro_rules! parse {
            ($t:ty, $n:expr, $variant:ident) => {{
                let chunks = raw.chunks_exact($n);
                if !chunks.remainder().is_empty() {
                    return Err(FormatError::MalformedIndex("writer checkpoint range width"));
                }
                AttributeValues::$variant(
                    chunks
                        .map(|c| <$t>::from_le_bytes(c.try_into().expect("exact chunk")))
                        .collect(),
                )
            }};
        }
        Ok(match data_type {
            AttributeDataType::U8 => AttributeValues::U8(raw.to_vec()),
            AttributeDataType::I8 => AttributeValues::I8(raw.iter().map(|v| *v as i8).collect()),
            AttributeDataType::U16 => parse!(u16, 2, U16),
            AttributeDataType::I16 => parse!(i16, 2, I16),
            AttributeDataType::U32 => parse!(u32, 4, U32),
            AttributeDataType::I32 => parse!(i32, 4, I32),
            AttributeDataType::U64 => parse!(u64, 8, U64),
            AttributeDataType::I64 => parse!(i64, 8, I64),
            AttributeDataType::F32 => AttributeValues::F32(
                raw.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|chunk| f32::from_bits(u32::from_le_bytes(*chunk)))
                    .collect(),
            ),
            AttributeDataType::F64 => AttributeValues::F64(
                raw.as_chunks::<8>()
                    .0
                    .iter()
                    .map(|chunk| f64::from_bits(u64::from_le_bytes(*chunk)))
                    .collect(),
            ),
        })
    }
}

/// Encodes one block without appending it, allowing independent blocks to be prepared concurrently.
pub fn prepare_point_block(
    block: SourcePointBlock,
    metadata: &FormatMetadata,
    options: FormatWriterOptions,
) -> Result<PreparedPointBlock, FormatError> {
    prepare_point_block_for_version(FormatVersion::V5, block, metadata, options)
}

pub(crate) fn prepare_point_block_for_version(
    version: FormatVersion,
    block: SourcePointBlock,
    metadata: &FormatMetadata,
    options: FormatWriterOptions,
) -> Result<PreparedPointBlock, FormatError> {
    prepare_point_block_for_version_impl(version, block, metadata, options, false)
        .map(|(block, _)| block)
}

pub(crate) fn prepare_point_block_for_version_with_diagnostics(
    version: FormatVersion,
    block: SourcePointBlock,
    metadata: &FormatMetadata,
    options: FormatWriterOptions,
) -> Result<(PreparedPointBlock, crate::GeometryCodecDiagnostics), FormatError> {
    prepare_point_block_for_version_impl(version, block, metadata, options, true)
        .map(|(block, diagnostics)| (block, diagnostics.expect("diagnostics enabled")))
}

fn prepare_point_block_for_version_impl(
    version: FormatVersion,
    block: SourcePointBlock,
    metadata: &FormatMetadata,
    options: FormatWriterOptions,
    collect_codec_diagnostics: bool,
) -> Result<(PreparedPointBlock, Option<crate::GeometryCodecDiagnostics>), FormatError> {
    let layout = StorageLayout::from_version(version)?;
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
    let raw_encoding_started = collect_codec_diagnostics.then(std::time::Instant::now);
    let raw_geometry = encode_geometry(&quantized, geometry_length)?;
    let raw_encoding_ns = raw_encoding_started.map_or(0, |started| started.elapsed().as_nanos());
    let raw_geometry_length = raw_geometry.len();
    let mut codec_diagnostics =
        collect_codec_diagnostics.then(crate::GeometryCodecDiagnostics::default);
    let mut candidates = Vec::new();
    let (mut geometry_encoding, mut encoded_geometry, mut geometry_raw_length) =
        if layout == StorageLayout::V5 {
            let started = collect_codec_diagnostics.then(std::time::Instant::now);
            let encoded = compress_block(&raw_geometry);
            let elapsed = started.map_or(0, |started| started.elapsed().as_nanos());
            if codec_diagnostics.is_some() {
                candidates.push(candidate_metric(
                    crate::GeometryCandidateKind::Raw,
                    raw_encoding_ns,
                    elapsed,
                    raw_geometry_length,
                    encoded.bytes().len(),
                ));
            }
            (GeometryEncoding::RawU32, encoded, raw_geometry_length)
        } else {
            let delta_started = collect_codec_diagnostics.then(std::time::Instant::now);
            let delta = encode_delta(quantized.coordinates())?;
            let delta_encoding_ns = delta_started.map_or(0, |started| started.elapsed().as_nanos());
            let delta_length = delta.len();
            let other_raw_size = uncompressed_size
                .checked_sub(geometry_length as u64)
                .ok_or(FormatError::BlockSizeOverflow)?;
            let delta_total = other_raw_size
                .checked_add(delta.len() as u64)
                .ok_or(FormatError::BlockSizeOverflow)?;
            let raw_started = collect_codec_diagnostics.then(std::time::Instant::now);
            let raw_encoded = compress_block(&raw_geometry);
            let raw_compression_ns = raw_started.map_or(0, |started| started.elapsed().as_nanos());
            if codec_diagnostics.is_some() {
                candidates.push(candidate_metric(
                    crate::GeometryCandidateKind::Raw,
                    raw_encoding_ns,
                    raw_compression_ns,
                    raw_geometry_length,
                    raw_encoded.bytes().len(),
                ));
            }
            if delta_total <= options.max_uncompressed_block_bytes {
                let delta_started = collect_codec_diagnostics.then(std::time::Instant::now);
                let delta_encoded = compress_block(&delta);
                let delta_compression_ns =
                    delta_started.map_or(0, |started| started.elapsed().as_nanos());
                if codec_diagnostics.is_some() {
                    candidates.push(candidate_metric(
                        crate::GeometryCandidateKind::Delta,
                        delta_encoding_ns,
                        delta_compression_ns,
                        delta_length,
                        delta_encoded.bytes().len(),
                    ));
                }
                if delta_encoded.bytes().len() < raw_encoded.bytes().len() {
                    (GeometryEncoding::DeltaVarint, delta_encoded, delta_length)
                } else {
                    (GeometryEncoding::RawU32, raw_encoded, raw_geometry_length)
                }
            } else {
                if codec_diagnostics.is_some() {
                    candidates.push(candidate_metric(
                        crate::GeometryCandidateKind::Delta,
                        delta_encoding_ns,
                        0,
                        delta_length,
                        0,
                    ));
                }
                (GeometryEncoding::RawU32, raw_encoded, raw_geometry_length)
            }
        };
    let mut selected_candidate = match geometry_encoding {
        GeometryEncoding::RawU32 => crate::GeometryCandidateKind::Raw,
        GeometryEncoding::DeltaVarint => crate::GeometryCandidateKind::Delta,
        GeometryEncoding::Occupied => crate::GeometryCandidateKind::Occupancy {
            residual_bits: 0,
            own_octant: false,
        },
        GeometryEncoding::CompactOccupied => {
            crate::GeometryCandidateKind::CompactOccupancy { residual_bits: 0 }
        }
    };
    if layout.supports_occupancy() && point_count != 0 && u32::try_from(point_count).is_ok() {
        let max_geometry_bytes =
            options.max_uncompressed_block_bytes - (uncompressed_size - geometry_length as u64);
        if matches!(layout, StorageLayout::V7Pre2 | StorageLayout::V7Pre3) {
            let build_started = collect_codec_diagnostics.then(std::time::Instant::now);
            let candidate = crate::QuantizedOccupancy::new(&quantized, 24)
                .map_err(|_| FormatError::BlockSizeOverflow)?;
            record_occupancy_build(&mut codec_diagnostics, 24, build_started);
            let encoding_started = collect_codec_diagnostics.then(std::time::Instant::now);
            if let Some(raw) =
                crate::occupancy_codec::encode_compact(&candidate, max_geometry_bytes)?
            {
                let encoding_ns =
                    encoding_started.map_or(0, |started| started.elapsed().as_nanos());
                let compression_started = collect_codec_diagnostics.then(std::time::Instant::now);
                let encoded = compress_block(&raw);
                let compression_ns =
                    compression_started.map_or(0, |started| started.elapsed().as_nanos());
                if codec_diagnostics.is_some() {
                    candidates.push(candidate_metric(
                        crate::GeometryCandidateKind::CompactOccupancy { residual_bits: 24 },
                        encoding_ns,
                        compression_ns,
                        raw.len(),
                        encoded.bytes().len(),
                    ));
                }
                if encoded.bytes().len() < encoded_geometry.bytes().len() {
                    geometry_encoding = GeometryEncoding::CompactOccupied;
                    selected_candidate =
                        crate::GeometryCandidateKind::CompactOccupancy { residual_bits: 24 };
                    geometry_raw_length = raw.len();
                    encoded_geometry = encoded;
                }
            } else if codec_diagnostics.is_some() {
                candidates.push(candidate_metric(
                    crate::GeometryCandidateKind::CompactOccupancy { residual_bits: 24 },
                    encoding_started.map_or(0, |started| started.elapsed().as_nanos()),
                    0,
                    0,
                    0,
                ));
            }
        } else {
            for bits in [24, 28] {
                let build_started = collect_codec_diagnostics.then(std::time::Instant::now);
                let candidate = crate::QuantizedOccupancy::new(&quantized, bits)
                    .map_err(|_| FormatError::BlockSizeOverflow)?;
                record_occupancy_build(&mut codec_diagnostics, bits, build_started);
                for own_octant in [false, true] {
                    let encoding_started = collect_codec_diagnostics.then(std::time::Instant::now);
                    if let Some(raw) =
                        crate::occupancy_codec::encode(&candidate, own_octant, max_geometry_bytes)?
                    {
                        let encoding_ns =
                            encoding_started.map_or(0, |started| started.elapsed().as_nanos());
                        let compression_started =
                            collect_codec_diagnostics.then(std::time::Instant::now);
                        let encoded = compress_block(&raw);
                        let compression_ns =
                            compression_started.map_or(0, |started| started.elapsed().as_nanos());
                        if codec_diagnostics.is_some() {
                            candidates.push(candidate_metric(
                                crate::GeometryCandidateKind::Occupancy {
                                    residual_bits: bits,
                                    own_octant,
                                },
                                encoding_ns,
                                compression_ns,
                                raw.len(),
                                encoded.bytes().len(),
                            ));
                        }
                        if encoded.bytes().len() < encoded_geometry.bytes().len() {
                            geometry_encoding = GeometryEncoding::Occupied;
                            selected_candidate = crate::GeometryCandidateKind::Occupancy {
                                residual_bits: bits,
                                own_octant,
                            };
                            geometry_raw_length = raw.len();
                            encoded_geometry = encoded;
                        }
                    } else if codec_diagnostics.is_some() {
                        candidates.push(candidate_metric(
                            crate::GeometryCandidateKind::Occupancy {
                                residual_bits: bits,
                                own_octant,
                            },
                            encoding_started.map_or(0, |started| started.elapsed().as_nanos()),
                            0,
                            0,
                            0,
                        ));
                    }
                }
                if layout.supports_compact_occupancy() {
                    let encoding_started = collect_codec_diagnostics.then(std::time::Instant::now);
                    if let Some(raw) =
                        crate::occupancy_codec::encode_compact(&candidate, max_geometry_bytes)?
                    {
                        let encoding_ns =
                            encoding_started.map_or(0, |started| started.elapsed().as_nanos());
                        let compression_started =
                            collect_codec_diagnostics.then(std::time::Instant::now);
                        let encoded = compress_block(&raw);
                        let compression_ns =
                            compression_started.map_or(0, |started| started.elapsed().as_nanos());
                        if codec_diagnostics.is_some() {
                            candidates.push(candidate_metric(
                                crate::GeometryCandidateKind::CompactOccupancy {
                                    residual_bits: bits,
                                },
                                encoding_ns,
                                compression_ns,
                                raw.len(),
                                encoded.bytes().len(),
                            ));
                        }
                        if encoded.bytes().len() < encoded_geometry.bytes().len() {
                            geometry_encoding = GeometryEncoding::CompactOccupied;
                            selected_candidate = crate::GeometryCandidateKind::CompactOccupancy {
                                residual_bits: bits,
                            };
                            geometry_raw_length = raw.len();
                            encoded_geometry = encoded;
                        }
                    } else if codec_diagnostics.is_some() {
                        candidates.push(candidate_metric(
                            crate::GeometryCandidateKind::CompactOccupancy {
                                residual_bits: bits,
                            },
                            encoding_started.map_or(0, |started| started.elapsed().as_nanos()),
                            0,
                            0,
                            0,
                        ));
                    }
                }
            }
        }
    }
    if let Some(diagnostics) = codec_diagnostics.as_mut() {
        for candidate in &mut candidates {
            if candidate.kind == selected_candidate {
                candidate.selected_winners = 1;
                candidate.selected_raw_bytes = geometry_raw_length as u64;
                candidate.selected_compressed_bytes = encoded_geometry.bytes().len() as u64;
            }
        }
        diagnostics.candidates = candidates;
    }
    // The u32 coordinates have been encoded; retain only their scalar transform/error
    // while compressing payloads, rather than another 12 bytes per point.
    drop(quantized);
    let mut bytes = Vec::new();
    let mut payloads = Vec::new();
    payloads
        .try_reserve_exact(schema.len() + 2)
        .map_err(|_| FormatError::AllocationFailed(schema.len() + 2))?;
    let mut raw = raw_geometry;
    append_encoded_payload(
        encoded_geometry,
        geometry_raw_length,
        &mut bytes,
        &mut payloads,
    )?;
    raw.clear();
    for color in block.colors() {
        raw.extend_from_slice(&[color.r, color.g, color.b]);
    }
    append_payload(&raw, &mut bytes, &mut payloads, layout, 3)?;
    for column in attributes {
        let length = attribute_payload_size(point_count as u64, column.descriptor())?;
        let length = usize::try_from(length).map_err(|_| FormatError::SizeOutOfRange(length))?;
        raw.clear();
        raw.try_reserve_exact(length)
            .map_err(|_| FormatError::AllocationFailed(length))?;
        encode_values(column.values(), &mut raw);
        let width = crate::layout::attribute_tuple_width(column.descriptor())?;
        append_payload(&raw, &mut bytes, &mut payloads, layout, width)?;
    }
    Ok((
        PreparedPointBlock {
            layout,
            geometry_encoding,
            id: block.id(),
            point_count: u64::try_from(point_count)
                .map_err(|_| FormatError::PointCountOutOfRange)?,
            quantization_error,
            origin: transform.origin(),
            scale: transform.scale(),
            world_bounds,
            bytes,
            payloads,
            ranges,
        },
        codec_diagnostics,
    ))
}

fn candidate_metric(
    kind: crate::GeometryCandidateKind,
    encoding_ns: u128,
    compression_ns: u128,
    raw_bytes: usize,
    compressed_bytes: usize,
) -> crate::GeometryCandidateDiagnostics {
    crate::GeometryCandidateDiagnostics {
        kind,
        attempts: 1,
        encoding_elapsed_ns: encoding_ns,
        compression_elapsed_ns: compression_ns,
        raw_bytes: raw_bytes as u64,
        compressed_bytes: compressed_bytes as u64,
        selected_winners: 0,
        selected_raw_bytes: 0,
        selected_compressed_bytes: 0,
    }
}

fn record_occupancy_build(
    diagnostics: &mut Option<crate::GeometryCodecDiagnostics>,
    residual_bits: u8,
    started: Option<std::time::Instant>,
) {
    if let (Some(diagnostics), Some(started)) = (diagnostics, started) {
        diagnostics
            .occupancy_builds
            .push(crate::OccupancyBuildDiagnostics {
                residual_bits,
                attempts: 1,
                elapsed_ns: started.elapsed().as_nanos(),
            });
    }
}

#[derive(Debug, Clone)]
struct BlockRecord {
    offset: u64,
    payload_length: u64,
    point_count: u64,
    geometry_encoding: GeometryEncoding,
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
    layout: StorageLayout,
    width: usize,
) -> Result<(), FormatError> {
    let encoded = if layout == StorageLayout::V7Pre3 {
        crate::attribute_codec::encode_v7_pre3(raw, width)?
    } else if layout.supports_byte_planes() {
        crate::attribute_codec::encode_pre4(raw, width)?
    } else if layout.supports_dictionary() {
        crate::attribute_codec::encode(raw, width)?
    } else {
        compress_block(raw)
    };
    append_encoded_payload(encoded, raw.len(), bytes, payloads)
}

fn append_encoded_payload(
    encoded: crate::EncodedBlock,
    raw_length: usize,
    bytes: &mut Vec<u8>,
    payloads: &mut Vec<PayloadDirectoryEntry>,
) -> Result<(), FormatError> {
    bytes
        .try_reserve(encoded.bytes().len())
        .map_err(|_| FormatError::AllocationFailed(encoded.bytes().len()))?;
    payloads.push(PayloadDirectoryEntry {
        offset: bytes.len() as u64,
        stored_length: encoded.bytes().len() as u64,
        raw_length: raw_length as u64,
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

fn header_for_role(
    version: FormatVersion,
    role: FileRole,
) -> Result<[u8; HEADER_SIZE as usize], FormatError> {
    FormatIdentity { version, role }.to_header_bytes()
}

fn footer_bytes(
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
    layout: StorageLayout,
) -> Result<(u64, u32), FormatError> {
    let mut index = IndexWriter {
        writer,
        checksum: Crc32::new(),
        length: 0,
    };
    index.write(layout.index_magic())?;
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
        if layout != StorageLayout::V5 {
            index.u8(block.geometry_encoding as u8)?;
        }
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
