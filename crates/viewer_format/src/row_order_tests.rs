//! Hybrid row order (ADR 0041/0042): the Morton path must keep every row tuple intact.

use std::io::Cursor;

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};

use super::{FormatWriterOptions, RowOrder, prepare_point_block_for_version};
use crate::geometry::GeometryEncoding;
use crate::{FormatMetadata, FormatReader, FormatVersion, FormatWriter, SourcePointBlock};

const VERSION: FormatVersion = FormatVersion::V7_PRE2;
const BLOCK: BlockId = BlockId::new(5);

fn schema() -> Vec<AttributeDescriptor> {
    vec![
        AttributeDescriptor::new("id", AttributeDataType::U32, 1).unwrap(),
        AttributeDescriptor::new("pair", AttributeDataType::U8, 2).unwrap(),
        AttributeDescriptor::new("f", AttributeDataType::F32, 1).unwrap(),
    ]
}

fn metadata(count: u64) -> FormatMetadata {
    let bounds = SpatialBounds::new([0.0; 3], [101.0; 3]).unwrap();
    let root = SpatialNode::new(NodeId::new(0), None, 0, bounds, count, 0.0, vec![BLOCK]).unwrap();
    let hierarchy = SpatialHierarchy::new(vec![root], schema(), Some(count)).unwrap();
    FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
        None,
        None,
        Vec::new(),
    )
    .unwrap()
}

/// Clustered noisy points in source (blob-interleaved) order, with every 7th point repeated at the
/// same position but with a different colour and attributes, so all row tuples are distinguishable.
/// Source order makes `CompactOccupied` the best geometry encoding; sorting makes Morton delta
/// smaller.
fn dense_block() -> (SourcePointBlock, usize) {
    let mut state = 7u64;
    let mut rnd = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let centers: Vec<[f64; 3]> = (0..40)
        .map(|_| [rnd() * 100.0, rnd() * 100.0, rnd() * 100.0])
        .collect();
    let mut rows: Vec<[f64; 3]> = (0..4000)
        .map(|i| {
            let c = centers[i % 40];
            [c[0] + rnd() * 0.3, c[1] + rnd() * 0.3, c[2] + rnd() * 0.01]
        })
        .collect();
    for i in (0..4000).step_by(7) {
        rows.push(rows[i]);
    }
    let count = rows.len();
    let colors = (0..count)
        .map(|i| ColorRgb::new(i as u8, (i >> 3) as u8, ((i * 7) >> 2) as u8))
        .collect();
    let ids = (0..count as u32)
        .map(|i| i.wrapping_mul(2_654_435_761))
        .collect();
    let pairs = (0..count)
        .flat_map(|i| [(i * 3) as u8, (i >> 2) as u8])
        .collect();
    let floats = (0..count)
        .map(|i| f32::from_bits(0x3f80_0000 + (i as u32) * 977))
        .collect();
    let columns = vec![
        AttributeColumn::new(schema()[0].clone(), AttributeValues::U32(ids), count).unwrap(),
        AttributeColumn::new(schema()[1].clone(), AttributeValues::U8(pairs), count).unwrap(),
        AttributeColumn::new(schema()[2].clone(), AttributeValues::F32(floats), count).unwrap(),
    ];
    (SourcePointBlock::new(BLOCK, rows, colors, columns), count)
}

fn options(order: RowOrder) -> FormatWriterOptions {
    FormatWriterOptions::new(0.001, 16 << 20)
        .unwrap()
        .with_row_order(order)
}

fn write(order: RowOrder) -> (Vec<u8>, Vec<u8>) {
    let (block, count) = dense_block();
    let mut writer = FormatWriter::new_split_for_version(
        VERSION,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata(count as u64),
        options(order),
    )
    .unwrap();
    writer.write_block(block).unwrap();
    let (primary, attributes) = writer.finish_pair().unwrap();
    (primary.into_inner(), attributes.unwrap().into_inner())
}

type Tuple = ([u32; 3], [u8; 3], u32, [u8; 2], u32);

fn tuples(primary: Vec<u8>, attributes: Vec<u8>) -> Vec<Tuple> {
    let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
    reader.attach_attributes(Cursor::new(attributes)).unwrap();
    let points = reader.read_geometry(BLOCK).unwrap().into_points();
    let mut column = |index| reader.read_attribute(BLOCK, index).unwrap();
    let (ids, pairs, floats) = (column(0), column(1), column(2));
    let (AttributeValues::U32(ids), AttributeValues::U8(pairs), AttributeValues::F32(floats)) = (
        ids.values().clone(),
        pairs.values().clone(),
        floats.values().clone(),
    ) else {
        panic!("unexpected attribute types");
    };
    let mut out: Vec<Tuple> = points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            (
                p.position.map(f32::to_bits),
                [p.color.r, p.color.g, p.color.b],
                ids[i],
                [pairs[2 * i], pairs[2 * i + 1]],
                floats[i].to_bits(),
            )
        })
        .collect();
    out.sort_unstable();
    out
}

fn encoding(order: RowOrder) -> GeometryEncoding {
    let (block, _) = dense_block();
    prepare_point_block_for_version(VERSION, block, &metadata(5000), options(order))
        .unwrap()
        .geometry_encoding
}

#[test]
fn hybrid_takes_the_morton_path_and_keeps_every_row_tuple() {
    // The fixture must make CompactOccupied win in source order, and hybrid must replace it.
    assert_eq!(
        encoding(RowOrder::Source),
        GeometryEncoding::CompactOccupied
    );
    assert_eq!(encoding(RowOrder::Hybrid), GeometryEncoding::DeltaVarint);

    let count = dense_block().1;
    let (source_primary, source_attributes) = write(RowOrder::Source);
    let (hybrid_primary, hybrid_attributes) = write(RowOrder::Hybrid);
    assert!(hybrid_primary.len() < source_primary.len());
    let expected = tuples(source_primary, source_attributes);
    assert_eq!(expected.len(), count);
    assert_eq!(tuples(hybrid_primary, hybrid_attributes), expected);
}

#[test]
fn hybrid_output_is_deterministic_with_duplicate_morton_keys() {
    // The fixture repeats positions, so the sort sees equal Morton keys.
    assert_eq!(write(RowOrder::Hybrid), write(RowOrder::Hybrid));
}

#[test]
fn morton_key_covers_all_96_bits_and_ties_break_by_source_row() {
    assert_eq!(super::morton_key([u32::MAX; 3]), (1u128 << 96) - 1);
    assert_eq!(super::morton_key([1, 0, 0]), 1);
    assert_eq!(super::morton_key([0, 0, 1 << 31]), 1u128 << 95);
    let coordinates = [[9, 9, 9], [0, 0, 0], [9, 9, 9], [0, 0, 1 << 31]];
    assert_eq!(super::morton_permutation(&coordinates), vec![1, 0, 2, 3]);
}
