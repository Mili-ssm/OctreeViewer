use std::io::{Cursor, Write};

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};

use crate::{
    FormatMetadata, FormatReaderLimits, FormatVersion, OctreeReader, OctreeWriter,
    PreparedOctreeWriter, SourcePointBlock, crc32, prepare_point_block,
};

use super::{FormatError, FormatWriter, FormatWriterOptions};

fn sample_metadata() -> FormatMetadata {
    let origin = [1_000_000_000_000.0, -2_000_000_000_000.0, 10.0];
    let bounds =
        SpatialBounds::new(origin, [origin[0] + 2.0, origin[1] + 1.0, origin[2] + 1.0]).unwrap();
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

type MemoryOutput = (Cursor<Vec<u8>>, Option<Cursor<Vec<u8>>>);

fn options() -> FormatWriterOptions {
    FormatWriterOptions::new(0.001, 1024).unwrap()
}

fn two_block_metadata() -> FormatMetadata {
    let base = sample_metadata();
    let bounds = base.dataset().hierarchy().stats().root_bounds;
    let root = SpatialNode::new(
        NodeId::new(0),
        None,
        0,
        bounds,
        4,
        0.25,
        vec![BlockId::new(7), BlockId::new(8)],
    )
    .unwrap();
    let hierarchy = SpatialHierarchy::new(
        vec![root],
        vec![AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap()],
        Some(4),
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

fn second_sample_block() -> SourcePointBlock {
    let descriptor = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
    let attribute =
        AttributeColumn::new(descriptor, AttributeValues::U16(vec![300, 400]), 2).unwrap();
    SourcePointBlock::new(
        BlockId::new(8),
        vec![[0.5, 0.25, 0.0], [1.25, 0.5, 1.0]],
        vec![ColorRgb::new(11, 21, 31), ColorRgb::new(41, 51, 61)],
        vec![attribute],
    )
}

#[test]
fn split_writer_resumes_from_committed_checkpoint_byte_identically() {
    let metadata = two_block_metadata();
    let mut clean = FormatWriter::new_split_for_version(
        FormatVersion::V7_PRE1,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata.clone(),
        options(),
    )
    .unwrap();
    clean.write_block(sample_block()).unwrap();
    clean.write_block(second_sample_block()).unwrap();
    let (clean_primary, clean_attributes) = clean.finish_pair().unwrap();

    let mut partial = FormatWriter::new_split_for_version(
        FormatVersion::V7_PRE1,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata.clone(),
        options(),
    )
    .unwrap();
    partial.write_block(sample_block()).unwrap();
    let checkpoint = partial.checkpoint_record(BlockId::new(7)).unwrap();
    let (mut primary, attributes) = partial.into_candidate_streams();
    let mut attributes = attributes.unwrap();
    primary.flush().unwrap();
    attributes.flush().unwrap();
    let mut resumed = FormatWriter::resume_for_version(
        FormatVersion::V7_PRE1,
        primary,
        Some(attributes),
        metadata.clone(),
        options(),
    )
    .unwrap();
    resumed.restore_committed_record(&checkpoint).unwrap();
    resumed.write_block(second_sample_block()).unwrap();
    let (resumed_primary, resumed_attributes) = resumed.finish_pair().unwrap();
    assert_eq!(resumed_primary.into_inner(), clean_primary.into_inner());
    assert_eq!(
        resumed_attributes.unwrap().into_inner(),
        clean_attributes.unwrap().into_inner()
    );
}

#[test]
fn v72_pre2_uses_its_exact_identity_and_roundtrips_geometry() {
    let mut writer = FormatWriter::new_for_version(
        FormatVersion::V7_PRE2,
        Cursor::new(Vec::new()),
        sample_metadata(),
        options(),
    )
    .unwrap();
    writer.write_block(sample_block()).unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    assert_eq!(&bytes[..8], b"OCTVFRM1");
    assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 7);
    assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 2);
    assert!(bytes.windows(8).any(|window| window == b"OCTVI72P"));
    assert_eq!(&bytes[bytes.len() - 40..bytes.len() - 32], b"OCTVE72P");

    let mut reader = crate::FormatReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(reader.format_version(), FormatVersion::V7_PRE2);
    assert_eq!(reader.read_positions(BlockId::new(7)).unwrap().len(), 2);
}

#[test]
fn split_resume_rejects_truncated_or_corrupt_record_without_writing_candidates() {
    let metadata = two_block_metadata();
    let mut partial = FormatWriter::new_split_for_version(
        FormatVersion::V7_PRE1,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata.clone(),
        options(),
    )
    .unwrap();
    partial.write_block(sample_block()).unwrap();
    let valid = partial.checkpoint_record(BlockId::new(7)).unwrap();
    let (primary, attributes) = partial.into_candidate_streams();
    let primary_bytes = primary.get_ref().clone();
    let attributes = attributes.unwrap();
    let attributes_bytes = attributes.get_ref().clone();

    let mut corrupt = valid.clone();
    corrupt[10] ^= 0x40;
    // One geometry and one color directory entry precede the first schema attribute.
    let mut bad_attribute_length = valid.clone();
    let attribute_raw_length = 152 + 2 * 29 + 16;
    bad_attribute_length[attribute_raw_length..attribute_raw_length + 8]
        .copy_from_slice(&3u64.to_le_bytes());
    let checksum_at = bad_attribute_length.len() - 4;
    let checksum = crc32(&bad_attribute_length[..checksum_at]);
    bad_attribute_length[checksum_at..].copy_from_slice(&checksum.to_le_bytes());
    let bad_records = [
        valid[..valid.len() - 1].to_vec(),
        valid[..valid.len() - 4].to_vec(),
        corrupt,
        bad_attribute_length,
    ];
    for bad_record in &bad_records {
        let mut resumed = FormatWriter::resume_for_version(
            FormatVersion::V7_PRE1,
            Cursor::new(primary_bytes.clone()),
            Some(Cursor::new(attributes_bytes.clone())),
            metadata.clone(),
            options(),
        )
        .unwrap();
        assert!(resumed.restore_committed_record(bad_record).is_err());
        let (primary, attributes) = resumed.into_candidate_streams();
        assert_eq!(primary.into_inner(), primary_bytes);
        assert_eq!(attributes.unwrap().into_inner(), attributes_bytes);
    }
}

#[test]
fn wide_schema_requires_explicit_reader_limits() {
    let base = sample_metadata();
    let schema = (0..257)
        .map(|index| {
            AttributeDescriptor::new(format!("column_{index}"), AttributeDataType::U8, 1).unwrap()
        })
        .collect::<Vec<_>>();
    let hierarchy = SpatialHierarchy::new(
        base.dataset()
            .hierarchy()
            .query(base.dataset().hierarchy().stats().root_bounds)
            .cloned()
            .collect(),
        schema.clone(),
        Some(2),
    )
    .unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(base.dataset().coordinate_frame(), hierarchy),
        base.crs().map(str::to_owned),
        base.sampling_policy().map(str::to_owned),
        Vec::new(),
    )
    .unwrap();
    let columns = schema
        .into_iter()
        .map(|descriptor| {
            AttributeColumn::new(descriptor, AttributeValues::U8(vec![1, 2]), 2).unwrap()
        })
        .collect();
    let block = SourcePointBlock::new(
        BlockId::new(7),
        vec![[0.25, 0.5, 0.0], [1.5, 0.75, 1.0]],
        vec![ColorRgb::new(10, 20, 30), ColorRgb::new(40, 50, 60)],
        columns,
    );
    let mut writer = FormatWriter::new(Cursor::new(Vec::new()), metadata, options()).unwrap();
    writer.write_block(block).unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    assert!(matches!(
        crate::FormatReader::new(Cursor::new(bytes.clone())),
        Err(FormatError::CountLimit {
            kind: "attributes",
            requested: 257,
            limit: 256
        })
    ));
    let limits = FormatReaderLimits::new(1 << 20, 1 << 20, 10, 10, 257, 65_535, 1 << 20).unwrap();
    let reader = crate::FormatReader::with_limits(Cursor::new(bytes), limits).unwrap();
    assert_eq!(
        reader
            .format_metadata()
            .dataset()
            .hierarchy()
            .attributes()
            .len(),
        257
    );
}

fn write_sample<W: OctreeWriter<Error = FormatError> + ?Sized>(writer: &mut W) {
    writer.write_block(sample_block()).unwrap();
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
    let mut direct =
        FormatWriter::new(Cursor::new(Vec::new()), sample_metadata(), options()).unwrap();
    direct.write_block(sample_block()).unwrap();
    let direct_primary = direct.finish_pair().unwrap().0.into_inner();

    let mut single =
        FormatWriter::new(Cursor::new(Vec::new()), sample_metadata(), options()).unwrap();
    write_sample(&mut single);
    let (single_primary, single_attributes): MemoryOutput =
        OctreeWriter::finish_pair(single).unwrap();
    assert!(single_attributes.is_none());
    let single_bytes = single_primary.into_inner();
    assert_eq!(single_bytes, direct_primary);

    let mut split = FormatWriter::new_split(
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        sample_metadata(),
        options(),
    )
    .unwrap();
    let mut direct_split = FormatWriter::new_split(
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        sample_metadata(),
        options(),
    )
    .unwrap();
    direct_split.write_block(sample_block()).unwrap();
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
    let decoded = viewer_core::DatasetSource::read_block(&mut reader, BlockId::new(7)).unwrap();
    assert_eq!(decoded.id(), BlockId::new(7));
    assert_eq!(decoded.points().len(), 2);
    assert!(
        viewer_core::DatasetSource::metadata(&reader)
            .hierarchy()
            .node(NodeId::new(0))
            .is_some()
    );
    assert!(!direct_primary.is_empty());
}

#[test]
fn prepared_writer_contract_matches_direct_block_writing() {
    let metadata = sample_metadata();
    let prepared = prepare_point_block(sample_block(), &metadata, options()).unwrap();

    let mut direct =
        FormatWriter::new(Cursor::new(Vec::new()), metadata.clone(), options()).unwrap();
    direct.write_block(sample_block()).unwrap();
    let direct = direct.finish().unwrap().into_inner();

    let mut prepared_writer =
        FormatWriter::new(Cursor::new(Vec::new()), metadata.clone(), options()).unwrap();
    {
        let writer: &mut dyn PreparedOctreeWriter<
            Error = FormatError,
            Output = MemoryOutput,
            PreparedBlock = crate::PreparedPointBlock,
        > = &mut prepared_writer;
        write_prepared_sample(writer, prepared);
    }
    let prepared = OctreeWriter::finish_pair(prepared_writer)
        .unwrap()
        .0
        .into_inner();
    assert_eq!(prepared, direct);
}

#[test]
fn prepared_block_from_another_version_is_rejected_without_poisoning_writer() {
    let metadata = sample_metadata();
    let prepared_v5 = prepare_point_block(sample_block(), &metadata, options()).unwrap();
    let mut writer = FormatWriter::new_for_version(
        FormatVersion::V6_PRE1,
        Cursor::new(Vec::new()),
        metadata,
        options(),
    )
    .unwrap();

    assert!(matches!(
        writer.write_prepared_block(prepared_v5),
        Err(FormatError::PreparedVersionMismatch {
            writer: FormatVersion::V6Pre(_),
            block: FormatVersion::V5,
        })
    ));
    assert_eq!(writer.format_version(), FormatVersion::V6_PRE1);
    writer.write_block(sample_block()).unwrap();
    assert!(!writer.finish().unwrap().into_inner().is_empty());
}

#[test]
fn v6_rejects_logical_block_over_limit_even_when_delta_would_be_small() {
    let point_count = 1_000;
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let block_id = BlockId::new(7);
    let root = SpatialNode::new(
        NodeId::new(0),
        None,
        0,
        bounds,
        point_count as u64,
        0.25,
        vec![block_id],
    )
    .unwrap();
    let descriptor = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
    let hierarchy = SpatialHierarchy::new(
        vec![root],
        vec![descriptor.clone()],
        Some(point_count as u64),
    )
    .unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    let values = AttributeColumn::new(
        descriptor,
        AttributeValues::U16(vec![128; point_count]),
        point_count,
    )
    .unwrap();
    let block = SourcePointBlock::new(
        block_id,
        vec![[0.5; 3]; point_count],
        vec![ColorRgb::new(20, 40, 60); point_count],
        vec![values],
    );
    let options = FormatWriterOptions::new(0.001, 1024).unwrap();

    assert!(matches!(
        super::prepare_point_block_for_version(FormatVersion::V6_PRE1, block, &metadata, options,),
        Err(FormatError::BlockExceedsLimit {
            requested: 17_000,
            limit: 1024,
        })
    ));
}

#[test]
fn writes_a_closed_file_with_checksums_and_quantized_block() {
    let mut writer =
        FormatWriter::new(Cursor::new(Vec::new()), sample_metadata(), options()).unwrap();
    writer.write_block(sample_block()).unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    assert_eq!(&bytes[..8], b"OCTVFRM1");
    assert_eq!(
        u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
        crc32(&bytes[..20])
    );
    assert_eq!(&bytes[bytes.len() - 40..bytes.len() - 32], b"OCTVEND5");
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
    assert_eq!(&index[..8], b"OCTVIDX5");
    assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 5);
    assert_eq!(
        u32::from_le_bytes(bytes[bytes.len() - 8..bytes.len() - 4].try_into().unwrap()),
        crc32(index)
    );
    let directory_start = index.len() - (93 + 3 * 29);
    assert_eq!(&index[directory_start + 88..directory_start + 93], &[0; 5]);
    assert_eq!(
        u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap()),
        crc32(&bytes[bytes.len() - 40..bytes.len() - 4])
    );
}

#[test]
fn prepares_blocks_independently_and_appends_them_to_v5_writer() {
    let metadata = sample_metadata();
    let defaults = FormatWriterOptions::new(0.001, 1024).unwrap();
    let mut writer =
        FormatWriter::new(Cursor::new(Vec::new()), metadata.clone(), defaults).unwrap();
    let prepared = super::prepare_point_block(sample_block(), &metadata, defaults).unwrap();
    writer.write_prepared_block(prepared).unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 5);
    let mut direct = FormatWriter::new(Cursor::new(Vec::new()), metadata, defaults).unwrap();
    direct.write_block(sample_block()).unwrap();
    assert_eq!(bytes, direct.finish().unwrap().into_inner());
}

#[test]
fn streamed_bounds_match_materialized_source_and_render_positions() {
    let origins = [[0.0; 3], [1.0e12, -2.0e12, 10.0]];
    let cases = [
        Vec::new(),
        vec![[0.0, 1.0, -2.0]; 3],
        vec![[0.1, 0.7, -0.1]],
        vec![[0.1, 0.7, -0.1], [1.9, -0.7, -1.9], [0.3, 0.0, 0.3]],
    ];
    for origin in origins {
        for source in &cases {
            let quantized = crate::quantize_local_positions(source, 0.001).unwrap();
            let decoded = quantized.decode_local().unwrap();
            let bounds = SpatialBounds::new([-3.0e12; 3], [3.0e12; 3]).unwrap();
            let node = SpatialNode::new(NodeId::new(0), None, 0, bounds, 0, 0.0, vec![]).unwrap();
            let metadata = DatasetMetadata::new(
                CoordinateFrame::new(origin).unwrap(),
                SpatialHierarchy::new(vec![node], vec![], None).unwrap(),
            );
            // Reference the former materialized path, including source positions that
            // can lie outside rounded f32 endpoints and decoded positions that expand them.
            let mut expected = super::BoundsAccumulator::empty();
            for local in source {
                expected.include_point(std::array::from_fn(|axis| origin[axis] + local[axis]));
            }
            for local in decoded {
                expected.include_point(std::array::from_fn(|axis| {
                    origin[axis] + f64::from(local[axis])
                }));
            }
            let expected = if expected.is_empty() {
                None
            } else {
                Some(expected.to_bounds().unwrap())
            };
            assert_eq!(
                super::block_world_bounds(source, &quantized, &metadata).unwrap(),
                expected,
                "source={source:?} origin={origin:?}"
            );
        }
    }
}

#[test]
fn refuses_to_finalize_when_a_referenced_block_is_missing() {
    let writer = FormatWriter::new(Cursor::new(Vec::new()), sample_metadata(), options()).unwrap();

    assert!(matches!(
        OctreeWriter::finish_pair(writer),
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

    let writer: &mut dyn OctreeWriter<Error = FormatError, Output = MemoryOutput> = &mut writer;
    assert!(matches!(
        OctreeWriter::write_block(writer, block),
        Err(FormatError::MissingAttribute(name)) if name == "intensity"
    ));
}

#[test]
fn rejects_duplicate_blocks_through_writer_contract() {
    let mut writer =
        FormatWriter::new(Cursor::new(Vec::new()), sample_metadata(), options()).unwrap();
    OctreeWriter::write_block(&mut writer, sample_block()).unwrap();

    let writer: &mut dyn OctreeWriter<Error = FormatError, Output = MemoryOutput> = &mut writer;
    assert!(matches!(
        OctreeWriter::write_block(writer, sample_block()),
        Err(FormatError::DuplicateBlock(block)) if block.get() == 7
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
        let mut writer = FormatWriter::new_split(
            sink(failing_primary),
            sink(!failing_primary),
            sample_metadata(),
            options(),
        )
        .unwrap();
        assert!(OctreeWriter::write_block(&mut writer, sample_block()).is_err());
        assert!(matches!(
            OctreeWriter::finish_pair(writer),
            Err(FormatError::WriterPoisoned)
        ));
    }
}
