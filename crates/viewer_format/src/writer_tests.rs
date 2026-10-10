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
    assert_eq!(&bytes[bytes.len() - 40..bytes.len() - 32], b"OCTVEND4");
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
    assert_eq!(&index[..8], b"OCTVIDX4");
    assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 4);
    assert_eq!(
        u32::from_le_bytes(bytes[bytes.len() - 8..bytes.len() - 4].try_into().unwrap()),
        crc32(index)
    );
    let directory_start = index.len() - (93 + 2 * 29);
    assert_eq!(&index[directory_start + 88..directory_start + 93], &[0; 5]);
    assert_eq!(
        u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap()),
        crc32(&bytes[bytes.len() - 40..bytes.len() - 4])
    );
}

#[test]
fn prepares_blocks_independently_and_appends_them_to_v4_writer() {
    let metadata = sample_metadata();
    let defaults = FormatWriterOptions::new(0.001, 1024).unwrap();
    let mut writer =
        FormatWriter::new(Cursor::new(Vec::new()), metadata.clone(), defaults).unwrap();
    let prepared = super::prepare_point_block(sample_block(), &metadata, defaults).unwrap();
    writer.write_prepared_block(prepared).unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 4);
    let mut direct = FormatWriter::new(Cursor::new(Vec::new()), metadata, defaults).unwrap();
    direct.write_block(sample_block()).unwrap();
    assert_eq!(bytes, direct.finish().unwrap().into_inner());
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
