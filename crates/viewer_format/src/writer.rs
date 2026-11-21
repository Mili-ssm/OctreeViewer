use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Seek, SeekFrom, Write},
};

use viewer_core::{BlockId, DatasetMetadata, NodeId, SpatialHierarchy, SpatialNode};

use crate::MAX_SCHEMA_ATTRIBUTES;
use crate::layout::{FOOTER_SIZE, StorageLayout};
use crate::metadata::validate_metadata;
use crate::version::FileRole;
use crate::{FormatError, FormatMetadata, FormatVersion, SourcePointBlock};

use crate::index_codec::{footer_bytes, header_for_role};
pub(crate) use crate::prepare::{
    BlockRecord, build_block_nodes, global_frame_of, lod_block_ids,
    prepare_point_block_for_version_impl,
};
pub use crate::prepare::{FormatWriterOptions, PreparedPointBlock, RowOrder, prepare_point_block};

/// Writes independently encoded blocks and a canonical index to a fresh seekable stream.
/// A footer is emitted only after every payload and the complete index have been written.
pub struct FormatWriter<W> {
    writer: W,
    attributes: Option<W>,
    metadata: FormatMetadata,
    options: FormatWriterOptions,
    layout: StorageLayout,
    block_nodes: BTreeMap<BlockId, Vec<NodeId>>,
    /// Blocks of nodes with children, built once; `finish` checks it against the written index.
    lod_blocks: BTreeSet<BlockId>,
    blocks: BTreeMap<BlockId, BlockRecord>,
    failed: bool,
}

impl<W: Write + Seek> FormatWriter<W> {
    pub fn format_version(&self) -> FormatVersion {
        self.layout.version()
    }

    /// Writes a `.oct` primary and its `.octattrs` companion. Unsupported identities fail before I/O.
    pub fn new_split_for_version(
        version: crate::FormatVersion,
        mut writer: W,
        mut attributes: W,
        metadata: FormatMetadata,
        options: FormatWriterOptions,
    ) -> Result<Self, FormatError> {
        let layout = StorageLayout::from_version(version)?;
        for sink in [&mut writer, &mut attributes] {
            if sink.stream_position()? != 0 || sink.seek(SeekFrom::End(0))? != 0 {
                return Err(FormatError::OutputMustBeEmpty);
            }
            sink.seek(SeekFrom::Start(0))?;
        }
        global_frame_of(&metadata)?;
        validate_metadata(&metadata)?;
        ensure_schema_within_limit(&metadata)?;
        ensure_lod_columns_present(&metadata)?;
        ensure_display_source_present(&metadata)?;
        if metadata.dataset().hierarchy().semantics()
            != viewer_core::HierarchySemantics::Replacement
        {
            return Err(FormatError::InvalidMetadata(
                "paged layouts do not support additive hierarchies",
            ));
        }
        let block_nodes = build_block_nodes(metadata.dataset().hierarchy())?;
        let lod_blocks = lod_block_ids(metadata.dataset().hierarchy())?;
        writer.write_all(&header_for_role(layout.version(), FileRole::SplitPrimary)?)?;
        attributes.write_all(&header_for_role(layout.version(), FileRole::Attributes)?)?;
        Ok(Self {
            writer,
            attributes: Some(attributes),
            metadata,
            options,
            layout,
            block_nodes,
            lod_blocks,
            blocks: BTreeMap::new(),
            failed: false,
        })
    }

    /// Encodes and appends one block while retaining only its directory metadata afterward.
    pub fn write_block(&mut self, block: SourcePointBlock) -> Result<(), FormatError> {
        self.ensure_block_available(block.id())?;
        let block_is_lod = self.lod_blocks.contains(&block.id());
        let (prepared, _) = prepare_point_block_for_version_impl(
            self.layout.version(),
            block,
            &self.metadata,
            self.options,
            block_is_lod,
            false,
        )?;
        self.write_prepared_block(prepared)
    }

    /// Appends a block encoded independently from this writer.
    pub fn write_prepared_block(
        &mut self,
        mut prepared: PreparedPointBlock,
    ) -> Result<(), FormatError> {
        self.ensure_block_available(prepared.id)?;
        self.ensure_columns_follow_the_rule(&prepared)?;
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
                origin_quanta: prepared.origin_quanta,
                payloads: prepared.payloads,
                ranges: prepared.ranges,
            },
        );
        Ok(())
    }

    /// A block prepared elsewhere must store exactly the columns the rule gives this writer.
    fn ensure_columns_follow_the_rule(
        &self,
        prepared: &PreparedPointBlock,
    ) -> Result<(), FormatError> {
        let block_is_lod = self.lod_blocks.contains(&prepared.id);
        let schema_len = self.metadata.dataset().hierarchy().attributes().len();
        let follows = prepared.payloads.len() == schema_len + 2
            && prepared
                .payloads
                .iter()
                .enumerate()
                .all(|(column, payload)| {
                    payload.stored
                        == self.layout.stored_column(
                            block_is_lod,
                            column,
                            self.metadata.column_rules(),
                        )
                });
        if follows {
            Ok(())
        } else {
            Err(FormatError::InvalidMetadata(
                "prepared block columns do not follow the writer's storage rule",
            ))
        }
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
        let written = crate::paged_index::write_paged_index(
            &mut self.writer,
            &metadata,
            &self.blocks,
            &self.lod_blocks,
            self.options.page_target_bytes(),
            self.layout,
        )?;
        let file_length = written
            .index_offset
            .checked_add(written.index_length)
            .and_then(|value| value.checked_add(FOOTER_SIZE))
            .ok_or(FormatError::FileSizeOverflow)?;
        if let Some(attributes) = self.attributes.as_mut() {
            let length = attributes
                .stream_position()?
                .checked_add(FOOTER_SIZE)
                .ok_or(FormatError::FileSizeOverflow)?;
            attributes.write_all(&footer_bytes(
                self.layout.footer_magic(),
                written.index_offset,
                written.index_length,
                length,
                written.index_crc,
            ))?;
            attributes.flush()?;
        }
        self.writer.write_all(&crate::paged_index::paged_footer(
            self.layout,
            &written,
            file_length,
        ))?;
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
        // Bounds are derived from level and prefix; blocks never widen them.
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
                node.bounds(),
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

/// The LOD set decides which columns blocks store, so the writer needs it up front. The list
/// itself is checked by `validate_metadata`.
fn ensure_lod_columns_present(metadata: &FormatMetadata) -> Result<(), FormatError> {
    if metadata.lod_attribute_columns().is_none() {
        return Err(FormatError::InvalidMetadata(
            "this format revision needs a LOD column set in the metadata",
        ));
    }
    Ok(())
}

/// Leaf display colour is derived from its source (`None` when nothing can be derived), so the
/// writer needs it up front. The source is checked by `validate_metadata`.
fn ensure_display_source_present(metadata: &FormatMetadata) -> Result<(), FormatError> {
    if metadata.display_colour_source().is_none() {
        return Err(FormatError::InvalidMetadata(
            "this format revision needs a display colour source in the metadata",
        ));
    }
    Ok(())
}

fn ensure_schema_within_limit(metadata: &FormatMetadata) -> Result<(), FormatError> {
    let count = metadata.dataset().hierarchy().attributes().len();
    if count > MAX_SCHEMA_ATTRIBUTES {
        return Err(FormatError::TooManyAttributes {
            count,
            limit: MAX_SCHEMA_ATTRIBUTES,
        });
    }
    Ok(())
}
