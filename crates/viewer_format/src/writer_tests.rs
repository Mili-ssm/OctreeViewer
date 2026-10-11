use std::io::Cursor;

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};

use crate::{FormatMetadata, SourcePointBlock, crc32};

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

fn options() -> FormatWriterOptions {
    FormatWriterOptions::new(0.001, 1024).unwrap()
}

#[test]
fn writes_a_closed_file_with_checksums_and_quantized_block() {
    let mut writer =
        FormatWriter::new(Cursor::new(Vec::new()), sample_metadata(), options()).unwrap();
    writer.write_block(sample_block()).unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    assert_eq!(&bytes[..8], b"OCTVWFRM");
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
        assert!(writer.write_block(sample_block()).is_err());
        assert!(matches!(
            writer.finish_pair(),
            Err(FormatError::WriterPoisoned)
        ));
    }
}
