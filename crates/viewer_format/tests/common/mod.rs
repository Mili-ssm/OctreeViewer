//! Fixtures and helpers shared by the integration tests.
#![allow(dead_code)]

use std::io::Cursor;

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    FormatMetadata, FormatVersion, FormatWriter, FormatWriterOptions, SourcePointBlock,
};

pub const BLOCK_ID: BlockId = BlockId::new(17);
pub const POINTS: [[f64; 3]; 4] = [
    [0.125, 0.25, 0.5],
    [0.25, 0.375, 0.625],
    [0.5, 0.5, 0.75],
    [0.875, 0.75, 0.125],
];
pub const COLORS: [ColorRgb; 4] = [
    ColorRgb::new(1, 2, 3),
    ColorRgb::new(40, 50, 60),
    ColorRgb::new(127, 128, 129),
    ColorRgb::new(253, 254, 255),
];

pub fn options() -> FormatWriterOptions {
    FormatWriterOptions::new(0.001, 1024 * 1024).unwrap()
}

pub fn descriptors() -> Vec<AttributeDescriptor> {
    [
        ("u8", AttributeDataType::U8),
        ("i8", AttributeDataType::I8),
        ("u16", AttributeDataType::U16),
        ("i16", AttributeDataType::I16),
        ("u32", AttributeDataType::U32),
        ("i32", AttributeDataType::I32),
        ("u64", AttributeDataType::U64),
        ("i64", AttributeDataType::I64),
        ("f32", AttributeDataType::F32),
        ("f64", AttributeDataType::F64),
    ]
    .into_iter()
    .map(|(name, kind)| AttributeDescriptor::new(name, kind, 1).unwrap())
    .collect()
}

pub fn fixture_attributes() -> Vec<AttributeColumn> {
    let schema = descriptors();
    let values = [
        AttributeValues::U8(vec![0, 1, 255, 42]),
        AttributeValues::I8(vec![i8::MIN, -1, i8::MAX, 0]),
        AttributeValues::U16(vec![0, 1, u16::MAX, 42]),
        AttributeValues::I16(vec![i16::MIN, -1, i16::MAX, 0]),
        AttributeValues::U32(vec![0, 1, u32::MAX, 42]),
        AttributeValues::I32(vec![i32::MIN, -1, i32::MAX, 0]),
        AttributeValues::U64(vec![0, 1, u64::MAX, 9_007_199_254_740_993]),
        AttributeValues::I64(vec![i64::MIN, -1, i64::MAX, 0]),
        AttributeValues::F32(vec![
            f32::from_bits(0x7fc0_0042),
            -0.0,
            f32::INFINITY,
            f32::MIN,
        ]),
        AttributeValues::F64(vec![
            f64::from_bits(0x7ff8_0000_0000_0042),
            -0.0,
            f64::INFINITY,
            f64::MIN,
        ]),
    ];
    schema
        .into_iter()
        .zip(values)
        .map(|(descriptor, values)| AttributeColumn::new(descriptor, values, POINTS.len()).unwrap())
        .collect()
}

pub fn metadata(point_count: u64, schema: Vec<AttributeDescriptor>) -> FormatMetadata {
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let node = SpatialNode::new(
        NodeId::new(0),
        None,
        0,
        bounds,
        point_count,
        0.0,
        vec![BLOCK_ID],
    )
    .unwrap();
    let hierarchy = SpatialHierarchy::new(vec![node], schema, Some(point_count)).unwrap();
    FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
        None,
        None,
        Vec::new(),
    )
    .unwrap()
}

pub fn fixture_block() -> SourcePointBlock {
    SourcePointBlock::new(
        BLOCK_ID,
        POINTS.to_vec(),
        COLORS.to_vec(),
        fixture_attributes(),
    )
}

pub fn write_split(
    version: FormatVersion,
    block: SourcePointBlock,
    meta: FormatMetadata,
) -> (Vec<u8>, Vec<u8>) {
    let mut writer = FormatWriter::new_split_for_version(
        version,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        meta,
        options(),
    )
    .unwrap();
    writer.write_block(block).unwrap();
    let (primary, attributes) = writer.finish_pair().unwrap();
    (primary.into_inner(), attributes.unwrap().into_inner())
}

pub fn write_monolithic(
    version: FormatVersion,
    block: SourcePointBlock,
    meta: FormatMetadata,
) -> Vec<u8> {
    let mut writer =
        FormatWriter::new_for_version(version, Cursor::new(Vec::new()), meta, options()).unwrap();
    writer.write_block(block).unwrap();
    writer.finish().unwrap().into_inner()
}

pub fn repair_index_and_footer(bytes: &mut [u8]) {
    let footer = bytes.len() - 40;
    let index = u64::from_le_bytes(bytes[footer + 8..footer + 16].try_into().unwrap()) as usize;
    let length = u64::from_le_bytes(bytes[footer + 16..footer + 24].try_into().unwrap()) as usize;
    let crc = viewer_format::crc32(&bytes[index..index + length]);
    bytes[footer + 32..footer + 36].copy_from_slice(&crc.to_le_bytes());
    let crc = viewer_format::crc32(&bytes[footer..footer + 36]);
    bytes[footer + 36..].copy_from_slice(&crc.to_le_bytes());
}
