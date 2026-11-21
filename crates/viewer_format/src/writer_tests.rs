use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};

use crate::test_support::{
    COLORS, POINTS, V8_BLOCK, intensity, v8_block, v8_metadata, v8_metadata_at, v8_options, v8_pair,
};
use crate::{
    FormatMetadata, FormatVersion, OctreeReader, OctreeWriter, PreparedOctreeWriter,
    SourcePointBlock, crc32, prepare_point_block,
};

use super::{FormatError, FormatWriter, FormatWriterOptions};

type MemoryOutput = (Cursor<Vec<u8>>, Option<Cursor<Vec<u8>>>);

fn sample_metadata() -> FormatMetadata {
    v8_metadata(4, vec![intensity()])
}

fn new_split(
    metadata: FormatMetadata,
    options: FormatWriterOptions,
) -> Result<FormatWriter<Cursor<Vec<u8>>>, FormatError> {
    FormatWriter::new_split_for_version(
        FormatVersion::V8,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata,
        options,
    )
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

#[test]
fn v8_uses_exact_identities_and_roundtrips_geometry() {
    let (primary, attributes) = v8_pair();
    // The envelope: magic, major, header size, role (1 primary, 2 attributes), revision.
    for (bytes, role) in [(&primary, 1), (&attributes, 2)] {
        assert_eq!(&bytes[..8], b"OCTVFRM1");
        assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 8);
        assert_eq!(le_u32(bytes, 12), role);
        assert_eq!(&bytes[bytes.len() - 40..bytes.len() - 32], b"OCTVEND8");
    }
    assert_eq!(le_u32(&primary, 16), le_u32(&attributes, 16));
    assert!(primary.windows(8).any(|window| window == b"OCTVIDX8"));

    let mut reader = crate::FormatReader::new(Cursor::new(primary)).unwrap();
    reader.attach_attributes(Cursor::new(attributes)).unwrap();
    assert_eq!(reader.format_version(), FormatVersion::V8);
    assert_eq!(reader.read_positions(V8_BLOCK).unwrap().len(), 4);
}

#[test]
fn wide_schema_is_limited_to_the_default_reader_limit() {
    // RF-27: the writer refuses schemas the default reader limit (256) would refuse, so every
    // file it produces opens without explicit limits.
    let schema = |count: usize| {
        (0..count)
            .map(|index| {
                AttributeDescriptor::new(format!("column_{index}"), AttributeDataType::U8, 1)
                    .unwrap()
            })
            .collect::<Vec<_>>()
    };
    let result = new_split(v8_metadata(4, schema(257)), v8_options());
    assert!(matches!(
        result,
        Err(FormatError::TooManyAttributes {
            count: 257,
            limit: 256
        })
    ));

    let schema = schema(256);
    let columns = schema
        .iter()
        .cloned()
        .map(|descriptor| {
            AttributeColumn::new(descriptor, AttributeValues::U8(vec![1, 2, 3, 4]), 4).unwrap()
        })
        .collect();
    let block = SourcePointBlock::new(V8_BLOCK, POINTS.to_vec(), COLORS.to_vec(), columns);
    let mut writer = new_split(v8_metadata(4, schema), v8_options()).unwrap();
    writer.write_block(block).unwrap();
    let (primary, attributes) = writer.finish_pair().unwrap();
    let mut reader = crate::FormatReader::new(Cursor::new(primary.into_inner())).unwrap();
    reader
        .attach_attributes(Cursor::new(attributes.unwrap().into_inner()))
        .unwrap();
    let attributes = reader.format_metadata().dataset().hierarchy().attributes();
    assert_eq!(attributes.len(), 256);
}

fn write_sample<W: OctreeWriter<Error = FormatError> + ?Sized>(writer: &mut W) {
    writer.write_block(v8_block()).unwrap();
}

fn write_prepared_sample<
    W: PreparedOctreeWriter<Error = FormatError, PreparedBlock = crate::PreparedPointBlock> + ?Sized,
>(
    writer: &mut W,
    block: crate::PreparedPointBlock,
) {
    writer.write_prepared_block(block).unwrap();
}

#[test]
fn writer_contract_supports_dyn_ingestion_pair_finalization_and_reader_roundtrip() {
    let mut split = new_split(sample_metadata(), v8_options()).unwrap();
    let mut direct_split = new_split(sample_metadata(), v8_options()).unwrap();
    direct_split.write_block(v8_block()).unwrap();
    let (expected_primary, expected_attributes) = direct_split.finish_pair().unwrap();
    {
        let writer: &mut dyn OctreeWriter<Error = FormatError, Output = MemoryOutput> = &mut split;
        write_sample(writer);
    }
    let (primary, attributes) = OctreeWriter::finish_pair(split).unwrap();
    let primary = primary.into_inner();
    let attributes = attributes
        .expect("split output includes the attribute companion")
        .into_inner();

    assert_eq!(primary, expected_primary.into_inner());
    assert_eq!(attributes, expected_attributes.unwrap().into_inner());
    let mut reader = crate::FormatReader::new(Cursor::new(primary)).unwrap();
    reader.attach_attributes(Cursor::new(attributes)).unwrap();
    let decoded = viewer_core::DatasetSource::read_block(&mut reader, V8_BLOCK).unwrap();
    assert_eq!(decoded.id(), V8_BLOCK);
    assert_eq!(decoded.points().len(), 4);
    assert!(
        viewer_core::DatasetSource::metadata(&reader)
            .hierarchy()
            .node(NodeId::new(0))
            .is_some()
    );
}

#[test]
fn prepared_writer_contract_matches_direct_block_writing() {
    let metadata = sample_metadata();
    let prepared = prepare_point_block(v8_block(), &metadata, v8_options()).unwrap();

    let mut direct = new_split(metadata.clone(), v8_options()).unwrap();
    direct.write_block(v8_block()).unwrap();
    let (direct_primary, direct_attributes) = direct.finish_pair().unwrap();

    let mut prepared_writer = new_split(metadata, v8_options()).unwrap();
    {
        let writer: &mut dyn PreparedOctreeWriter<
            Error = FormatError,
            Output = MemoryOutput,
            PreparedBlock = crate::PreparedPointBlock,
        > = &mut prepared_writer;
        write_prepared_sample(writer, prepared);
    }
    let (primary, attributes) = OctreeWriter::finish_pair(prepared_writer).unwrap();
    assert_eq!(primary.into_inner(), direct_primary.into_inner());
    assert_eq!(
        attributes.unwrap().into_inner(),
        direct_attributes.unwrap().into_inner()
    );
}

/// Replaces `PreparedVersionMismatch`: a block prepared under another storage rule (here a leaf
/// that stores its colour, offered to a writer whose rule derives it) must be rejected.
#[test]
fn prepared_block_following_another_storage_rule_is_rejected_without_poisoning_writer() {
    let stores_colour = sample_metadata();
    let derives_colour =
        sample_metadata().with_display_colour_source(crate::DisplayColourSource::Intensity(0));
    let prepared = prepare_point_block(v8_block(), &stores_colour, v8_options()).unwrap();
    let mut writer = new_split(derives_colour, v8_options()).unwrap();

    assert!(matches!(
        writer.write_prepared_block(prepared),
        Err(FormatError::InvalidMetadata(
            "prepared block columns do not follow the writer's storage rule"
        ))
    ));
    assert_eq!(writer.format_version(), FormatVersion::V8);
    // Not poisoned: a block whose colours are the derivable ones still writes and finishes.
    let derived = SourcePointBlock::new(
        V8_BLOCK,
        POINTS.to_vec(),
        [0_u8, 0, 0, 255]
            .map(|channel| ColorRgb::new(channel, channel, channel))
            .to_vec(),
        v8_block().attributes().to_vec(),
    );
    writer.write_block(derived).unwrap();
    assert!(!writer.finish().unwrap().into_inner().is_empty());
}

#[test]
fn logical_block_over_limit_is_rejected_even_when_delta_would_be_small() {
    let point_count = 1_000;
    let metadata = v8_metadata_at(0.5, vec![V8_BLOCK], point_count as u64, vec![intensity()]);
    let values = AttributeColumn::new(
        intensity(),
        AttributeValues::U16(vec![128; point_count]),
        point_count,
    )
    .unwrap();
    let block = SourcePointBlock::new(
        V8_BLOCK,
        vec![[0.5; 3]; point_count],
        vec![ColorRgb::new(20, 40, 60); point_count],
        vec![values],
    );
    let options = FormatWriterOptions::new(0.001, 1024).unwrap();

    // 12 position + 3 colour + 2 intensity bytes per point; identical points compress to almost
    // nothing, but the limit is on the logical size.
    assert!(matches!(
        super::prepare_point_block(block, &metadata, options),
        Err(FormatError::BlockExceedsLimit {
            requested: 17_000,
            limit: 1024,
        })
    ));
}

#[test]
fn writes_a_closed_file_with_checksums_and_quantized_block() {
    let (primary, attributes) = v8_pair();

    for (bytes, role) in [(&primary, "primary"), (&attributes, "attributes")] {
        assert_eq!(&bytes[..8], b"OCTVFRM1", "{role}");
        assert_eq!(le_u32(bytes, 20), crc32(&bytes[..20]), "{role}");
        let footer = bytes.len() - 40;
        assert_eq!(&bytes[footer..footer + 8], b"OCTVEND8", "{role}");
        assert_eq!(le_u64(bytes, footer + 24), bytes.len() as u64, "{role}");
        assert_eq!(
            le_u32(bytes, footer + 36),
            crc32(&bytes[footer..footer + 36])
        );
    }
    // The primary's footer locates the index; the companion's footer repeats its location.
    let footer = primary.len() - 40;
    let index_offset = le_u64(&primary, footer + 8) as usize;
    let index_length = le_u64(&primary, footer + 16) as usize;
    assert_eq!(index_offset + index_length + 40, primary.len());
    assert_eq!(
        le_u32(&primary, footer + 32),
        crc32(&primary[index_offset..index_offset + index_length])
    );
    let companion = attributes.len() - 40;
    assert_eq!(
        attributes[companion + 8..companion + 24],
        primary[footer + 8..footer + 24]
    );
    assert_eq!(
        le_u32(&attributes, companion + 32),
        le_u32(&primary, footer + 32)
    );
}

#[test]
fn prepares_blocks_independently_and_appends_them_to_the_writer() {
    let metadata = sample_metadata();
    let defaults = FormatWriterOptions::new(0.001, 1024).unwrap();
    let mut writer = new_split(metadata.clone(), defaults).unwrap();
    let prepared = prepare_point_block(v8_block(), &metadata, defaults).unwrap();
    writer.write_prepared_block(prepared).unwrap();
    let (primary, attributes) = writer.finish_pair().unwrap();
    let primary = primary.into_inner();
    assert_eq!(u16::from_le_bytes(primary[8..10].try_into().unwrap()), 8);
    let mut direct = new_split(metadata, defaults).unwrap();
    direct.write_block(v8_block()).unwrap();
    let (direct_primary, direct_attributes) = direct.finish_pair().unwrap();
    assert_eq!(primary, direct_primary.into_inner());
    assert_eq!(
        attributes.unwrap().into_inner(),
        direct_attributes.unwrap().into_inner()
    );
}

#[test]
fn refuses_to_finalize_when_a_referenced_block_is_missing() {
    let writer = new_split(sample_metadata(), v8_options()).unwrap();

    assert!(matches!(
        OctreeWriter::finish_pair(writer),
        Err(FormatError::MissingBlock(block)) if block == V8_BLOCK
    ));
}

#[test]
fn validates_attribute_schema_and_point_counts_before_writing_a_block() {
    let mut writer = new_split(sample_metadata(), v8_options()).unwrap();
    let wrong_descriptor =
        AttributeDescriptor::new("classification", AttributeDataType::U8, 1).unwrap();
    let wrong_column =
        AttributeColumn::new(wrong_descriptor, AttributeValues::U8(vec![1, 2, 3, 4]), 4).unwrap();
    let block = SourcePointBlock::new(
        V8_BLOCK,
        POINTS.to_vec(),
        COLORS.to_vec(),
        vec![wrong_column],
    );

    let writer: &mut dyn OctreeWriter<Error = FormatError, Output = MemoryOutput> = &mut writer;
    assert!(matches!(
        OctreeWriter::write_block(writer, block),
        Err(FormatError::MissingAttribute(name)) if name == "intensity"
    ));
}

#[test]
fn rejects_duplicate_blocks_through_writer_contract() {
    let mut writer = new_split(sample_metadata(), v8_options()).unwrap();
    OctreeWriter::write_block(&mut writer, v8_block()).unwrap();

    let writer: &mut dyn OctreeWriter<Error = FormatError, Output = MemoryOutput> = &mut writer;
    assert!(matches!(
        OctreeWriter::write_block(writer, v8_block()),
        Err(FormatError::DuplicateBlock(block)) if block == V8_BLOCK
    ));
}

#[test]
fn refuses_a_quantization_error_above_the_declared_limit() {
    let mut writer = new_split(
        v8_metadata(3, vec![intensity()]),
        FormatWriterOptions::new(0.0, 1024).unwrap(),
    )
    .unwrap();
    let attribute =
        AttributeColumn::new(intensity(), AttributeValues::U16(vec![100, 150, 200]), 3).unwrap();
    let block = SourcePointBlock::new(
        V8_BLOCK,
        vec![[0.25, 0.5, 0.0], [0.8, 0.6, 0.3], [1.5, 0.75, 1.0]],
        vec![
            ColorRgb::new(10, 20, 30),
            ColorRgb::new(20, 30, 40),
            ColorRgb::new(40, 50, 60),
        ],
        vec![attribute],
    );

    assert!(matches!(
        OctreeWriter::write_block(&mut writer, block),
        Err(FormatError::Quantization(_))
    ));
}

#[test]
fn split_writer_is_poisoned_after_partial_failure_of_either_sink() {
    use std::io::{self, Seek, SeekFrom, Write};
    struct Sink {
        cursor: Cursor<Vec<u8>>,
        limit: u64,
    }
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let available = self.limit.saturating_sub(self.cursor.position()) as usize;
            if available == 0 {
                return Err(io::Error::other("injected sink failure"));
            }
            self.cursor.write(&bytes[..bytes.len().min(available)])
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Seek for Sink {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.cursor.seek(position)
        }
    }
    for failing_primary in [false, true] {
        let sink = |fails| Sink {
            cursor: Cursor::new(Vec::new()),
            limit: if fails { 25 } else { u64::MAX },
        };
        let mut writer = FormatWriter::new_split_for_version(
            FormatVersion::V8,
            sink(failing_primary),
            sink(!failing_primary),
            sample_metadata(),
            v8_options(),
        )
        .unwrap();
        assert!(OctreeWriter::write_block(&mut writer, v8_block()).is_err());
        assert!(matches!(
            OctreeWriter::finish_pair(writer),
            Err(FormatError::WriterPoisoned)
        ));
    }
}

/// Nodes as `(id, parent, blocks)`; every child is the first octant of its parent.
type TestNodes<'a> = &'a [(u64, Option<u64>, &'a [u64])];

fn hierarchy_of(nodes: TestNodes<'_>) -> SpatialHierarchy {
    let mut levels = BTreeMap::<u64, u32>::new();
    let mut built = Vec::new();
    for &(id, parent, blocks) in nodes {
        let level = parent.map_or(0, |parent| levels[&parent] + 1);
        let side = 8.0 / f64::from(1_u32 << level);
        let node_bounds = SpatialBounds::new([0.0; 3], [side; 3]).unwrap();
        levels.insert(id, level);
        built.push(
            SpatialNode::new(
                NodeId::new(id),
                parent.map(NodeId::new),
                level,
                node_bounds,
                blocks.len() as u64,
                0.0,
                blocks.iter().map(|&block| BlockId::new(block)).collect(),
            )
            .unwrap(),
        );
    }
    SpatialHierarchy::new(built, Vec::new(), None).unwrap()
}

#[test]
fn lod_set_matches_has_children() {
    let shapes: [TestNodes<'_>; 3] = [
        // A root leaf, as for a cloud that fits one node.
        &[(0, None, &[0])],
        // A root leaf whose points are all equal: several blocks, still no children.
        &[(0, None, &[0, 1, 2])],
        // A normal tree: interior nodes own the LOD blocks.
        &[
            (0, None, &[0]),
            (1, Some(0), &[1, 2]),
            (2, Some(0), &[3]),
            (3, Some(1), &[4]),
            (4, Some(1), &[5]),
        ],
    ];
    for nodes in shapes {
        let hierarchy = hierarchy_of(nodes);
        let lod = super::lod_block_ids(&hierarchy).unwrap();
        let expected: BTreeSet<_> = nodes
            .iter()
            .filter(|(id, _, _)| !hierarchy.is_leaf(NodeId::new(*id)))
            .flat_map(|(_, _, blocks)| blocks.iter().map(|&block| BlockId::new(block)))
            .collect();
        assert_eq!(lod, expected, "{nodes:?}");
        // Per node: a block is LOD exactly when its node has children, which is what the page's
        // `has_children` flag records.
        for (id, _, blocks) in nodes {
            let has_children = !hierarchy.is_leaf(NodeId::new(*id));
            for &block in *blocks {
                assert_eq!(
                    lod.contains(&BlockId::new(block)),
                    has_children,
                    "node {id} block {block} in {nodes:?}"
                );
            }
        }
        assert_eq!(lod.is_empty(), nodes.len() == 1 || nodes[0].2.len() > 1);
    }
}
