use viewer_core::{
    AttributeColumn, AttributeDescriptor, AttributeValues, CategoricalAttribute, ColorRgb,
    NodeAttributePolicy as Policy, NodeSummaryError, OccupiedCellIndex, OccupiedOctreeRoot,
    VoxelCoord, summarize_attribute, summarize_colors,
};

#[test]
fn categorization_densifies_sparse_values_and_preserves_complete_tuple_bits() {
    let source = column(AttributeValues::U64(vec![u64::MAX, 42, u64::MAX, 42]), 1);
    let categorized = CategoricalAttribute::new(&source).unwrap();
    assert_eq!(categorized.category_count(), 2);
    assert_eq!(
        categorized.codes().values(),
        &AttributeValues::U32(vec![0, 1, 0, 1])
    );
    assert_eq!(
        categorized.dictionary().values(),
        &AttributeValues::U64(vec![u64::MAX, 42])
    );
    let summary = summarize_attribute(
        &index(),
        categorized.codes(),
        Policy::Categories {
            category_count: categorized.category_count(),
        },
    )
    .unwrap();
    assert_eq!(summary.record(0, 0), Some([3].as_slice()));
    let bits = [
        0x7ff8_0000_0000_0001_u64,
        0,
        0x7ff8_0000_0000_0002,
        1 << 63,
        0x7ff8_0000_0000_0001,
        0,
    ];
    let source = column(AttributeValues::F64(bits.map(f64::from_bits).to_vec()), 2);
    let categorized = CategoricalAttribute::new(&source).unwrap();
    assert_eq!(categorized.category_count(), 2);
    assert_eq!(
        categorized.codes().values(),
        &AttributeValues::U32(vec![0, 1, 0])
    );
    let AttributeValues::F64(values) = categorized.dictionary().values() else {
        panic!("original primitive type lost")
    };
    assert_eq!(
        values.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        bits[..4]
    );
    let empty = CategoricalAttribute::new(&column(AttributeValues::U8(vec![]), 1)).unwrap();
    assert_eq!(empty.category_count(), 0);
    assert!(empty.codes().values().is_empty());
}

fn index() -> OccupiedCellIndex {
    OccupiedCellIndex::new(
        OccupiedOctreeRoot::new([0; 3], 2).unwrap(),
        [
            VoxelCoord::new(0, 0, 0),
            VoxelCoord::new(0, 0, 0),
            VoxelCoord::new(0, 0, 1),
            VoxelCoord::new(3, 3, 3),
        ],
    )
    .unwrap()
}
fn column(values: AttributeValues, components: u32) -> AttributeColumn {
    let rows = values.len() / components as usize;
    AttributeColumn::new(
        AttributeDescriptor::new("explicit", values.data_type(), components).unwrap(),
        values,
        rows,
    )
    .unwrap()
}
fn count(record: &[u8]) -> u64 {
    u64::from_le_bytes(record[..8].try_into().unwrap())
}
fn mean(record: &[u8]) -> f64 {
    f64::from_le_bytes(record[8..16].try_into().unwrap())
}

#[test]
fn means_weight_original_rows_and_keep_internal_topology_alignment() {
    let index = index();
    let source = column(AttributeValues::U16(vec![0, 0, 0, 100]), 1);
    let result = summarize_attribute(&index, &source, Policy::Mean).unwrap();
    assert_eq!(result.levels().len(), 2);
    for (bytes, nodes) in result.levels().iter().zip(index.topology().levels()) {
        assert_eq!(bytes.len(), nodes.len() * result.record_width());
    }
    assert_eq!(count(result.record(0, 0).unwrap()), 4);
    assert!((mean(result.record(0, 0).unwrap()) - 25.0).abs() < 1e-12);
    assert_eq!(mean(result.record(1, 0).unwrap()), 0.0);
    assert_eq!(mean(result.record(1, 1).unwrap()), 100.0);
    assert!(result.record(2, 0).is_none());
    assert!(result.record(0, usize::MAX).is_none());
    assert_eq!(index.rows_for_leaf(0).unwrap(), &[0, 1]);
    assert_eq!(source.values(), &AttributeValues::U16(vec![0, 0, 0, 100]));
}

#[test]
fn nonfinite_components_have_independent_counts_and_explicit_absence() {
    let source = column(
        AttributeValues::F64(vec![
            f64::NAN,
            2.0,
            f64::INFINITY,
            4.0,
            f64::NEG_INFINITY,
            6.0,
            f64::NAN,
            8.0,
        ]),
        2,
    );
    for policy in [Policy::Mean, Policy::Range] {
        let result = summarize_attribute(&index(), &source, policy).unwrap();
        let record = result.record(0, 0).unwrap();
        assert_eq!(count(record), 0);
        let stride = if policy == Policy::Mean { 16 } else { 24 };
        assert_eq!(count(&record[stride..]), 4);
        if policy == Policy::Mean {
            assert_eq!(mean(&record[stride..]), 5.0);
        } else {
            assert_eq!(mean(&record[stride..]), 2.0);
            assert_eq!(
                f64::from_le_bytes(record[stride + 16..stride + 24].try_into().unwrap()),
                8.0
            );
        }
    }
    assert!(matches!(source.values(), AttributeValues::F64(v) if v[0].is_nan() && v[1] == 2.0));
}

#[test]
fn timestamp_endpoints_keep_all_integer_bits() {
    let source = column(
        AttributeValues::U64(vec![(1 << 53) + 1, u64::MAX, (1 << 53) + 3, (1 << 53) + 2]),
        1,
    );
    let result = summarize_attribute(&index(), &source, Policy::Range).unwrap();
    let record = result.record(0, 0).unwrap();
    assert_eq!(count(record), 4);
    assert_eq!(
        u64::from_le_bytes(record[8..16].try_into().unwrap()),
        (1 << 53) + 1
    );
    assert_eq!(
        u64::from_le_bytes(record[16..24].try_into().unwrap()),
        u64::MAX
    );
    let signed = column(AttributeValues::I64(vec![i64::MAX, -1, i64::MIN, 0]), 1);
    let result = summarize_attribute(&index(), &signed, Policy::Range).unwrap();
    let record = result.record(0, 0).unwrap();
    assert_eq!(
        i64::from_le_bytes(record[8..16].try_into().unwrap()),
        i64::MIN
    );
    assert_eq!(
        i64::from_le_bytes(record[16..24].try_into().unwrap()),
        i64::MAX
    );
}

#[test]
fn category_masks_use_smallest_fixed_word_width_and_union() {
    for (domain, width) in [
        (1, 1),
        (8, 1),
        (9, 2),
        (16, 2),
        (17, 4),
        (32, 4),
        (33, 8),
        (64, 8),
        (65, 16),
        (128, 16),
        (129, 24),
        (193, 32),
    ] {
        let source = column(AttributeValues::U32(vec![0, domain - 1, 0, domain - 1]), 1);
        let result = summarize_attribute(
            &index(),
            &source,
            Policy::Categories {
                category_count: domain,
            },
        )
        .unwrap();
        assert_eq!(result.record_width(), width);
        let mut expected = vec![0_u8; width];
        expected[0] |= 1;
        expected[(domain as usize - 1) / 8] |= 1 << ((domain - 1) % 8);
        assert_eq!(result.record(0, 0).unwrap(), expected);
    }
    let invalid = column(AttributeValues::U64(vec![0, 0, 0, u64::MAX]), 1);
    assert_eq!(
        summarize_attribute(&index(), &invalid, Policy::Categories { category_count: 4 })
            .unwrap_err(),
        NodeSummaryError::CategoryOutOfRange {
            code: u64::MAX,
            category_count: 4
        }
    );
    for source in [
        column(AttributeValues::I8(vec![0; 4]), 1),
        column(AttributeValues::F32(vec![0.; 4]), 1),
        column(AttributeValues::U8(vec![0; 8]), 2),
    ] {
        assert_eq!(
            summarize_attribute(&index(), &source, Policy::Categories { category_count: 4 })
                .unwrap_err(),
            NodeSummaryError::InvalidCategories
        );
    }
}

#[test]
fn rgb_uses_linear_light_and_extreme_means_stay_finite() {
    let result = summarize_colors(
        &index(),
        &[
            ColorRgb::new(0, 0, 0),
            ColorRgb::new(0, 0, 0),
            ColorRgb::new(0, 0, 0),
            ColorRgb::new(255, 255, 255),
        ],
    )
    .unwrap();
    for record in result.record(0, 0).unwrap().as_chunks::<16>().0 {
        assert!((mean(record) - 0.25).abs() < 1e-12);
    }
    let source = column(
        AttributeValues::F64(vec![f64::MAX, -f64::MAX, f64::MAX, -f64::MAX]),
        1,
    );
    let result = summarize_attribute(&index(), &source, Policy::Mean).unwrap();
    assert!(mean(result.record(0, 0).unwrap()).is_finite());
}

#[test]
fn empty_depth_zero_and_bad_alignment() {
    for depth in [0, 2, 32] {
        let index =
            OccupiedCellIndex::new(OccupiedOctreeRoot::new([i32::MIN; 3], depth).unwrap(), [])
                .unwrap();
        let result = summarize_attribute(
            &index,
            &column(AttributeValues::U8(vec![]), 1),
            Policy::Mean,
        )
        .unwrap();
        assert_eq!(result.levels().len(), depth as usize);
        assert!(result.levels().iter().all(Vec::is_empty));
    }
    let index = OccupiedCellIndex::new(
        OccupiedOctreeRoot::new([0; 3], 0).unwrap(),
        [VoxelCoord::new(0, 0, 0)],
    )
    .unwrap();
    let summary = summarize_attribute(
        &index,
        &column(AttributeValues::U8(vec![1]), 1),
        Policy::Mean,
    )
    .unwrap();
    assert!(summary.levels().is_empty(), "{:?}", summary.levels());
    assert_eq!(
        summarize_colors(&index, &[]).unwrap_err(),
        NodeSummaryError::RowCountMismatch
    );
}
