//! Hybrid row order (ADR 0041/0042): the Morton path must keep every row tuple intact. `v8` fixes
//! the order, so the source-order side of the retired comparisons is checked through the codec
//! candidates the writer reports.

use std::io::Cursor;

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialHierarchy, SpatialNode,
};

use super::{FormatWriterOptions, RowOrder, prepare_point_block_for_version_impl};
use crate::geometry::GeometryEncoding;
use crate::test_support::{prefix_of, v8_crs};
use crate::{
    DisplayColourSource, FormatMetadata, FormatReader, FormatVersion, FormatWriter, GlobalFrame,
    NodeCell, PointDistanceQuantiles, SourcePointBlock,
};

const VERSION: FormatVersion = FormatVersion::V8;
const BLOCK: BlockId = BlockId::new(0);
/// Cells of this level are 256 CRS units wide, so the 100-unit cloud below fits the root.
const LEVEL: u32 = 41;

fn schema() -> Vec<AttributeDescriptor> {
    vec![
        AttributeDescriptor::new("id", AttributeDataType::U32, 1).unwrap(),
        AttributeDescriptor::new("pair", AttributeDataType::U8, 2).unwrap(),
        AttributeDescriptor::new("f", AttributeDataType::F32, 1).unwrap(),
    ]
}

fn metadata(count: u64) -> FormatMetadata {
    let prefix = [prefix_of(0.5, LEVEL); 3];
    let origin = prefix.map(|p| crate::cell_origin_quanta(LEVEL, p));
    let frame = GlobalFrame::new(
        LEVEL,
        prefix,
        origin,
        origin,
        v8_crs(),
        None,
        vec![(NodeId::new(0), NodeCell::new(prefix, 2.0_f64.powi(-5)))],
    )
    .unwrap();
    let bounds = frame.node_bounds(0, prefix).unwrap();
    let root = SpatialNode::new(NodeId::new(0), None, 0, bounds, count, 0.0, vec![BLOCK]).unwrap();
    let hierarchy = SpatialHierarchy::new(vec![root], schema(), Some(count)).unwrap();
    FormatMetadata::new(
        DatasetMetadata::new(
            CoordinateFrame::new(frame.frame_origin()).unwrap(),
            hierarchy,
        ),
        None,
        Some("sample-grid:256;leaf:64000".to_owned()),
        Vec::new(),
    )
    .unwrap()
    .with_global_frame(frame)
    .unwrap()
    .with_point_distance_quantiles(PointDistanceQuantiles::new(0.25, 0.5, 1.5).unwrap())
    .with_lod_attribute_columns(Vec::new())
    .with_display_colour_source(DisplayColourSource::None)
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

fn options() -> FormatWriterOptions {
    FormatWriterOptions::new(0.001, 16 << 20).unwrap()
}

fn write() -> (Vec<u8>, Vec<u8>) {
    let (block, count) = dense_block();
    let mut writer = FormatWriter::new_split_for_version(
        VERSION,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata(count as u64),
        options(),
    )
    .unwrap();
    writer.write_block(block).unwrap();
    let (primary, attributes) = writer.finish_pair().unwrap();
    (primary.into_inner(), attributes.unwrap().into_inner())
}

type Tuple = ([u32; 3], [u8; 3], u32, [u8; 2], u32);

/// The row tuples of the file as `(position bits, colour, id, pair, float bits)`, sorted.
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

fn prepare_with_diagnostics() -> (crate::PreparedPointBlock, crate::GeometryCodecDiagnostics) {
    let (block, count) = dense_block();
    let (prepared, diagnostics) = prepare_point_block_for_version_impl(
        VERSION,
        block,
        &metadata(count as u64),
        options(),
        false,
        true,
    )
    .unwrap();
    (prepared, diagnostics.expect("diagnostics were requested"))
}

fn candidate(
    diagnostics: &crate::GeometryCodecDiagnostics,
    kind: crate::GeometryCandidateKind,
) -> &crate::GeometryCandidateDiagnostics {
    diagnostics
        .candidates
        .iter()
        .find(|c| c.kind == kind)
        .unwrap_or_else(|| panic!("{kind:?} candidate reported"))
}

#[test]
fn hybrid_takes_the_morton_path_and_keeps_every_row_tuple() {
    // The fixture must make CompactOccupied win in source order (it beats plain delta), and the
    // sorted Morton delta must replace it by being strictly smaller.
    let (prepared, diagnostics) = prepare_with_diagnostics();
    let compact = candidate(
        &diagnostics,
        crate::GeometryCandidateKind::CompactOccupancy { residual_bits: 24 },
    );
    let delta = candidate(&diagnostics, crate::GeometryCandidateKind::Delta);
    let morton = candidate(&diagnostics, crate::GeometryCandidateKind::DeltaMorton);
    assert!(compact.compressed_bytes < delta.compressed_bytes);
    assert!(morton.compressed_bytes < compact.compressed_bytes);
    assert_eq!(prepared.geometry_encoding, GeometryEncoding::DeltaVarint);

    // Every source row survives the permutation with its colour and attributes; the id is unique
    // per row, so it names the source row whose position the decoded one must match.
    let (block, count) = dense_block();
    let (primary, attributes) = write();
    let actual = tuples(primary, attributes);
    assert_eq!(actual.len(), count);
    let (AttributeValues::U32(ids), AttributeValues::U8(pairs), AttributeValues::F32(floats)) = (
        block.attributes()[0].values().clone(),
        block.attributes()[1].values().clone(),
        block.attributes()[2].values().clone(),
    ) else {
        panic!("unexpected attribute types");
    };
    let mut expected_ids: Vec<u32> = ids.clone();
    expected_ids.sort_unstable();
    let mut seen_ids: Vec<u32> = actual.iter().map(|row| row.2).collect();
    seen_ids.sort_unstable();
    assert_eq!(seen_ids, expected_ids);
    let row_of: std::collections::HashMap<u32, usize> =
        ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    assert_eq!(row_of.len(), count, "ids identify rows");
    for (position, color, id, pair, float) in &actual {
        let i = row_of[id];
        let source = block.positions_local()[i];
        for axis in 0..3 {
            let decoded = f64::from(f32::from_bits(position[axis]));
            assert!(
                (decoded - source[axis]).abs() < 0.001,
                "row {i} axis {axis}"
            );
        }
        let expected_color = block.colors()[i];
        assert_eq!(
            *color,
            [expected_color.r, expected_color.g, expected_color.b]
        );
        assert_eq!(*pair, [pairs[2 * i], pairs[2 * i + 1]]);
        assert_eq!(*float, floats[i].to_bits());
    }
}

#[test]
fn hybrid_output_is_deterministic_with_duplicate_morton_keys() {
    // The fixture repeats positions, so the sort sees equal Morton keys.
    assert_eq!(write(), write());
}

#[test]
fn diagnostics_report_delta_morton_when_it_wins() {
    let (prepared, diagnostics) = prepare_with_diagnostics();
    assert_eq!(prepared.geometry_encoding, GeometryEncoding::DeltaVarint);
    let morton = candidate(&diagnostics, crate::GeometryCandidateKind::DeltaMorton);
    assert_eq!((morton.attempts, morton.selected_winners), (1, 1));
}

#[test]
fn morton_key_covers_all_96_bits_and_ties_break_by_source_row() {
    assert_eq!(super::morton_key([u32::MAX; 3]), (1u128 << 96) - 1);
    assert_eq!(super::morton_key([1, 0, 0]), 0b100);
    assert_eq!(super::morton_key([0, 0, 1]), 1);
    assert_eq!(super::morton_key([1 << 31, 0, 0]), 1u128 << 95);
    assert_eq!(super::morton_key([0, 0, 1 << 31]), 1u128 << 93);
    let coordinates = [[9, 9, 9], [0, 0, 0], [9, 9, 9], [0, 0, 1 << 31]];
    assert_eq!(super::morton_permutation(&coordinates), vec![1, 0, 2, 3]);
}

#[test]
fn morton_key_matches_the_bit_loop_reference() {
    fn reference(coordinate: [u32; 3]) -> u128 {
        let mut key = 0u128;
        for bit in 0..32 {
            for (axis, value) in coordinate.iter().enumerate() {
                key |= u128::from((value >> bit) & 1) << (3 * bit + (2 - axis));
            }
        }
        key
    }
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 32) as u32
    };
    for _ in 0..10_000 {
        let coordinate = [next(), next(), next()];
        assert_eq!(
            super::morton_key(coordinate),
            reference(coordinate),
            "{coordinate:?}"
        );
    }
    for coordinate in [
        [0; 3],
        [u32::MAX; 3],
        [1 << 20, 1 << 21, 1 << 31],
        [0x1f_ffff, 0, 0x7ff << 21],
    ] {
        assert_eq!(
            super::morton_key(coordinate),
            reference(coordinate),
            "{coordinate:?}"
        );
    }
}

#[test]
fn morton_permutation_equals_sorting_by_key_then_row() {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = |modulus: u32| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 33) as u32) % modulus
    };
    // Narrow ranges give many duplicate keys; the wide case spans the high key bits.
    for (count, modulus, offset) in [
        (0, 1, 0),
        (1, 1, 0),
        (500, 1, 7),
        (5000, 16, 1 << 20),
        (5000, u32::MAX, 0),
    ] {
        let coordinates: Vec<[u32; 3]> = (0..count)
            .map(|_| {
                [
                    offset + next(modulus),
                    offset + next(modulus),
                    offset + next(modulus),
                ]
            })
            .collect();
        let mut expected: Vec<u32> = (0..count as u32).collect();
        expected.sort_by_key(|&row| (super::morton_key(coordinates[row as usize]), row));
        assert_eq!(
            super::morton_permutation(&coordinates),
            expected,
            "count {count} modulus {modulus}"
        );
    }
}

#[test]
fn v8_resolves_hybrid_row_order_and_rejects_source_order() {
    let unset = FormatWriterOptions::new(0.001, 16 << 20).unwrap();
    assert_eq!(unset.resolved_row_order(VERSION).unwrap(), RowOrder::Hybrid);
    assert_eq!(VERSION.fixed_row_order(), Some(RowOrder::Hybrid));
    // Explicit hybrid is what the format fixes anyway.
    assert!(
        unset
            .with_row_order(RowOrder::Hybrid)
            .resolved_row_order(VERSION)
            .is_ok()
    );
    let (block, count) = dense_block();
    let result = prepare_point_block_for_version_impl(
        VERSION,
        block,
        &metadata(count as u64),
        unset.with_row_order(RowOrder::Source),
        false,
        false,
    );
    assert!(matches!(
        result,
        Err(crate::FormatError::RowOrderFixedByFormat(FormatVersion::V8))
    ));
}
