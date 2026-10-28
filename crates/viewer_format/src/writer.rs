use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Seek, SeekFrom, Write},
};

use viewer_core::{BlockId, DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode};

use crate::geometry::GeometryEncoding;
use crate::layout::{FOOTER_SIZE, HEADER_SIZE, PayloadDirectoryEntry, StorageLayout};
use crate::metadata::validate_metadata;
use crate::version::FileRole;
use crate::{FormatError, FormatMetadata, FormatVersion, SourcePointBlock};

use crate::checkpoint_record::{
    decode_checkpoint_record, encode_checkpoint_record, validate_checkpoint_record,
};
use crate::index_codec::{footer_bytes, header_for_role, write_index};
pub use crate::prepare::prepare_point_block;
pub(crate) use crate::prepare::{
    BlockRecord, BoundsAccumulator, build_block_nodes, prepare_point_block_for_version,
    prepare_point_block_for_version_with_diagnostics,
};

/// Bounds per-block allocations made by the writer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormatWriterOptions {
    pub(crate) max_quantization_error: f64,
    pub(crate) max_uncompressed_block_bytes: u64,
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
    pub(crate) layout: StorageLayout,
    pub(crate) geometry_encoding: GeometryEncoding,
    pub(crate) id: BlockId,
    pub(crate) point_count: u64,
    pub(crate) quantization_error: f64,
    pub(crate) origin: [f64; 3],
    pub(crate) scale: [f64; 3],
    pub(crate) world_bounds: Option<SpatialBounds>,
    pub(crate) bytes: Vec<u8>,
    pub(crate) payloads: Vec<PayloadDirectoryEntry>,
    pub(crate) ranges: Vec<crate::NodeAttributeRange>,
}

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

#[cfg(test)]
use crate::prepare::block_world_bounds;

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;
