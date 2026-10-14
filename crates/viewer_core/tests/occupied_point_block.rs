use std::collections::HashMap;

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    DatasetError, OccupiedOctreeError, OccupiedOctreeRoot, OccupiedPointBlock,
    OccupiedPointBlockError, Point, PointBlock, VoxelCoord,
};

fn block(count: usize, attributes: Vec<AttributeColumn>) -> PointBlock {
    PointBlock::new(
        BlockId::new(19),
        (0..count)
            .map(|i| {
                Point::new(
                    [if i == 0 { -0.0 } else { i as f32 + 0.125 }, 0.25, -1.5],
                    ColorRgb::new(i as u8, (i + 1) as u8, (i + 2) as u8),
                )
            })
            .collect::<Vec<_>>()
            .into(),
        attributes,
    )
}

fn column(name: &str, values: AttributeValues, components: u32, count: usize) -> AttributeColumn {
    AttributeColumn::new(
        AttributeDescriptor::new(name, values.data_type(), components).unwrap(),
        values,
        count,
    )
    .unwrap()
}

#[test]
fn cell_selection_preserves_every_row_rgb_components_and_attribute_bits() {
    let a = VoxelCoord::new(-1, 0, 0);
    let b = VoxelCoord::new(0, -1, 0);
    let c = VoxelCoord::new(0, 0, -1);
    let cells = [a, b, a, c, b, a];
    let nan32 = f32::from_bits(0x7fc0_1234);
    let nan64 = f64::from_bits(0x7ff8_0000_0000_1234);
    let source = block(
        6,
        vec![
            column(
                "rgb16",
                AttributeValues::U16(vec![
                    10, 11, 12, 20, 21, 22, 30, 31, 32, 40, 41, 42, 50, 51, 52, 60, 61, 62,
                ]),
                3,
                6,
            ),
            column(
                "id64",
                AttributeValues::U64(vec![u64::MAX, 1, 9_007_199_254_740_993, 3, 4, 5]),
                1,
                6,
            ),
            column(
                "signed",
                AttributeValues::I64(vec![i64::MIN, 1, i64::MAX, 3, 4, 5]),
                1,
                6,
            ),
            column(
                "f32",
                AttributeValues::F32(vec![nan32, 1.0, -0.0, 3.0, 4.0, f32::INFINITY]),
                1,
                6,
            ),
            column(
                "f64",
                AttributeValues::F64(vec![nan64, 1.0, -0.0, 3.0, 4.0, f64::NEG_INFINITY]),
                1,
                6,
            ),
        ],
    );
    let source_points = source.points().points().as_ptr();
    let root = OccupiedOctreeRoot::new([-2; 3], 2).unwrap();
    let occupied = OccupiedPointBlock::new(root, source, &cells).unwrap();
    assert_eq!(source_points, occupied.source().points().points().as_ptr());
    assert_eq!(occupied.rows_for_cell(a), Some([0, 2, 5].as_slice()));
    assert_eq!(occupied.rows_for_cell(b), Some([1, 4].as_slice()));
    let selected = occupied.select_cell(a).unwrap().unwrap();
    assert_eq!(selected.id(), BlockId::new(19));
    assert_eq!(selected.points().len(), 3);
    for (point, &row) in selected.points().points().iter().zip(&[0, 2, 5]) {
        let original = occupied.source().points().points()[row];
        assert_eq!(
            point.position.map(f32::to_bits),
            original.position.map(f32::to_bits)
        );
        assert_eq!(point.color, original.color);
    }
    for (actual, original) in selected
        .attributes()
        .iter()
        .zip(occupied.source().attributes())
    {
        assert_eq!(actual.descriptor(), original.descriptor());
    }
    assert_eq!(
        selected.attributes()[0].values(),
        &AttributeValues::U16(vec![10, 11, 12, 30, 31, 32, 60, 61, 62])
    );
    assert_eq!(
        selected.attributes()[1].values(),
        &AttributeValues::U64(vec![u64::MAX, 9_007_199_254_740_993, 5])
    );
    assert_eq!(
        selected.attributes()[2].values(),
        &AttributeValues::I64(vec![i64::MIN, i64::MAX, 5])
    );
    let AttributeValues::F32(actual) = selected.attributes()[3].values() else {
        panic!("changed type")
    };
    assert_eq!(
        actual.iter().copied().map(f32::to_bits).collect::<Vec<_>>(),
        [
            nan32.to_bits(),
            (-0.0_f32).to_bits(),
            f32::INFINITY.to_bits()
        ]
    );
    let AttributeValues::F64(actual) = selected.attributes()[4].values() else {
        panic!("changed type")
    };
    assert_eq!(
        actual.iter().copied().map(f64::to_bits).collect::<Vec<_>>(),
        [
            nan64.to_bits(),
            (-0.0_f64).to_bits(),
            f64::NEG_INFINITY.to_bits()
        ]
    );
    assert!(
        occupied
            .select_cell(VoxelCoord::new(1, 1, 1))
            .unwrap()
            .is_none()
    );
    assert!(
        occupied
            .select_cell(VoxelCoord::new(2, 0, 0))
            .unwrap()
            .is_none()
    );
    let source = occupied.into_source();
    assert_eq!(source.points().len(), 6);
    assert_eq!(source_points, source.points().points().as_ptr());
}

#[test]
fn row_partition_matches_independent_grouping_and_covers_source_once() {
    let mut cells = Vec::new();
    for i in 0..300 {
        cells.push(VoxelCoord::new(
            i % 7 - 4,
            (i * 3) % 9 - 4,
            (i * 11) % 5 - 4,
        ));
    }
    for reverse in [false, true] {
        if reverse {
            cells.reverse();
        }
        let mut expected = HashMap::<_, Vec<_>>::new();
        for (row, &cell) in cells.iter().enumerate() {
            expected.entry(cell).or_default().push(row);
        }
        let occupied = OccupiedPointBlock::new(
            OccupiedOctreeRoot::new([-4; 3], 4).unwrap(),
            block(cells.len(), vec![]),
            &cells,
        )
        .unwrap();
        let mut seen = Vec::new();
        for (leaf, coord) in occupied.topology().cells().enumerate() {
            assert_eq!(occupied.topology().leaf_index(coord), Some(leaf));
            let rows = occupied.rows_for_leaf(leaf).unwrap();
            assert_eq!(rows, expected[&coord]);
            assert_eq!(occupied.rows_for_cell(coord), Some(rows));
            seen.extend_from_slice(rows);
        }
        assert_eq!(occupied.topology().cells().count(), expected.len());
        seen.sort_unstable();
        assert_eq!(seen, (0..cells.len()).collect::<Vec<_>>());
        assert_eq!(occupied.rows_for_leaf(expected.len()), None);
        assert_eq!(occupied.rows_for_leaf(usize::MAX), None);
    }
}

#[test]
fn empty_unit_and_full_domain_roots_keep_multiplicity() {
    let root = OccupiedOctreeRoot::new([i32::MAX; 3], 0).unwrap();
    let empty_column = column("empty", AttributeValues::U8(vec![]), 1, 0);
    let empty = OccupiedPointBlock::new(root, block(0, vec![empty_column]), &[]).unwrap();
    assert_eq!(empty.rows_for_leaf(0), None);
    assert!(
        empty
            .select_cell(VoxelCoord::new(i32::MAX, i32::MAX, i32::MAX))
            .unwrap()
            .is_none()
    );
    let cell = VoxelCoord::new(i32::MAX, i32::MAX, i32::MAX);
    let occupied = OccupiedPointBlock::new(root, block(3, vec![]), &[cell; 3]).unwrap();
    assert_eq!(occupied.rows_for_leaf(0), Some([0, 1, 2].as_slice()));
    assert_eq!(occupied.topology().leaf_index(cell), Some(0));
    let cells = [
        VoxelCoord::new(i32::MIN, 0, i32::MAX),
        cell,
        VoxelCoord::new(i32::MIN, 0, i32::MAX),
    ];
    let occupied = OccupiedPointBlock::new(
        OccupiedOctreeRoot::new([i32::MIN; 3], 32).unwrap(),
        block(3, vec![]),
        &cells,
    )
    .unwrap();
    assert_eq!(occupied.rows_for_cell(cells[0]), Some([0, 2].as_slice()));
    assert_eq!(occupied.rows_for_cell(cell), Some([1].as_slice()));
}

#[test]
fn rejects_row_count_out_of_root_and_misaligned_columns() {
    let root = OccupiedOctreeRoot::new([0; 3], 1).unwrap();
    let cell = VoxelCoord::new(0, 0, 0);
    for cells in [vec![], vec![cell; 2]] {
        assert_eq!(
            OccupiedPointBlock::new(root, block(1, vec![]), &cells).unwrap_err(),
            OccupiedPointBlockError::CellCountMismatch {
                points: 1,
                cells: cells.len()
            }
        );
    }
    for outside in [VoxelCoord::new(-1, 0, 0), VoxelCoord::new(0, 2, 0)] {
        assert_eq!(
            OccupiedPointBlock::new(root, block(1, vec![]), &[outside]).unwrap_err(),
            OccupiedPointBlockError::Topology(OccupiedOctreeError::OutsideRoot(outside))
        );
    }
    for count in [1, 3] {
        let malformed = block(
            2,
            vec![column(
                "pair",
                AttributeValues::U16(vec![0; count * 2]),
                2,
                count,
            )],
        );
        assert_eq!(
            OccupiedPointBlock::new(root, malformed, &[cell; 2]).unwrap_err(),
            OccupiedPointBlockError::Dataset(DatasetError::InvalidAttributeLength {
                name: "pair".into(),
                expected: 4,
                actual: count * 2
            })
        );
    }
    // All columns are checked, even after a valid one.
    let good = column("good", AttributeValues::U8(vec![1, 2]), 1, 2);
    let bad = AttributeColumn::new(
        AttributeDescriptor::new("bad", AttributeDataType::U8, 1).unwrap(),
        AttributeValues::U8(vec![1]),
        1,
    )
    .unwrap();
    assert!(matches!(
        OccupiedPointBlock::new(root, block(2, vec![good, bad]), &[cell; 2]),
        Err(OccupiedPointBlockError::Dataset(_))
    ));
}
