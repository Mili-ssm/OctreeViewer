use viewer_core::{
    AttributeDataType, AttributeDescriptor, BlockId, FrustumPlane, NodeId, SpatialBounds,
    SpatialError, SpatialFrustum, SpatialHierarchy, SpatialNode, SpatialSubdivision,
};

fn bounds(min: [f64; 3], max: [f64; 3]) -> SpatialBounds {
    SpatialBounds::new(min, max).unwrap()
}

fn node(
    id: u64,
    parent: Option<u64>,
    level: u32,
    min: [f64; 3],
    max: [f64; 3],
    point_count: u64,
    blocks: &[u64],
) -> SpatialNode {
    SpatialNode::new(
        NodeId::new(id),
        parent.map(NodeId::new),
        level,
        bounds(min, max),
        point_count,
        0.0,
        blocks.iter().copied().map(BlockId::new).collect(),
    )
    .unwrap()
}

fn unit_cube_frustum() -> SpatialFrustum {
    SpatialFrustum::new(vec![
        FrustumPlane::new([1.0, 0.0, 0.0], 0.0).unwrap(),
        FrustumPlane::new([-1.0, 0.0, 0.0], 1.0).unwrap(),
        FrustumPlane::new([0.0, 1.0, 0.0], 0.0).unwrap(),
        FrustumPlane::new([0.0, -1.0, 0.0], 1.0).unwrap(),
        FrustumPlane::new([0.0, 0.0, 1.0], 0.0).unwrap(),
        FrustumPlane::new([0.0, 0.0, -1.0], 1.0).unwrap(),
    ])
    .unwrap()
}

#[test]
fn uniform_subdivision_maps_points_and_bounds_consistently() {
    let bounds = bounds([-2.0, 4.0, 8.0], [6.0, 12.0, 16.0]);
    for divisions in [2, 4, 8] {
        let subdivision = SpatialSubdivision::new(bounds, divisions).unwrap();
        assert_eq!(subdivision.cell_count(), usize::from(divisions).pow(3));

        for index in 0..subdivision.cell_count() {
            let cell = subdivision.cell_bounds(index).unwrap();
            assert!(bounds.contains(&cell));
            let center = std::array::from_fn(|axis| (cell.min()[axis] + cell.max()[axis]) / 2.0);
            assert_eq!(subdivision.cell_index(center), index);
        }
        assert_eq!(
            subdivision.cell_index(bounds.max()),
            subdivision.cell_count() - 1
        );
        assert!(subdivision.cell_bounds(subdivision.cell_count()).is_err());
    }
}

#[test]
fn uniform_subdivision_rejects_unsupported_divisions() {
    let bounds = bounds([0.0; 3], [1.0; 3]);
    assert_eq!(
        SpatialSubdivision::new(bounds, 3).unwrap_err(),
        SpatialError::InvalidSubdivisionDivisions(3)
    );
}

#[test]
fn queries_hierarchy_in_stable_depth_first_order_and_prune_disjoint_subtrees() {
    let hierarchy = SpatialHierarchy::new(
        vec![
            node(1, None, 0, [0.0; 3], [10.0; 3], 0, &[]),
            node(2, Some(1), 1, [0.0; 3], [4.0; 3], 10, &[20]),
            node(3, Some(2), 2, [1.0; 3], [2.0; 3], 8, &[30]),
            node(4, Some(1), 1, [6.0; 3], [9.0; 3], 12, &[40]),
        ],
        Vec::new(),
        Some(30),
    )
    .unwrap();

    let all_ids = hierarchy
        .query(bounds([0.0; 3], [10.0; 3]))
        .map(|item| item.id().get())
        .collect::<Vec<_>>();
    assert_eq!(all_ids, [1, 2, 3, 4]);

    let left_ids = hierarchy
        .query(bounds([0.5; 3], [2.5; 3]))
        .map(|item| item.id().get())
        .collect::<Vec<_>>();
    assert_eq!(left_ids, [1, 2, 3]);

    assert!(
        hierarchy
            .query(bounds([20.0; 3], [21.0; 3]))
            .next()
            .is_none()
    );
    assert_eq!(hierarchy.stats().node_count, 4);
    assert_eq!(hierarchy.stats().max_level, 2);
    assert_eq!(hierarchy.stats().source_point_count, Some(30));
    assert_eq!(hierarchy.node(NodeId::new(3)).unwrap().point_count(), 8);
}

#[test]
fn validates_node_identity_parent_levels_and_containment() {
    let root = node(1, None, 0, [0.0; 3], [10.0; 3], 0, &[]);

    assert_eq!(
        SpatialHierarchy::new(Vec::new(), Vec::new(), None).unwrap_err(),
        SpatialError::EmptyHierarchy
    );
    assert_eq!(
        SpatialHierarchy::new(vec![root.clone(), root.clone()], Vec::new(), None).unwrap_err(),
        SpatialError::DuplicateNodeId(NodeId::new(1))
    );
    assert_eq!(
        SpatialHierarchy::new(
            vec![node(7, None, 2, [0.0; 3], [1.0; 3], 0, &[])],
            Vec::new(),
            None,
        )
        .unwrap_err(),
        SpatialError::InvalidRootLevel {
            node: NodeId::new(7),
            level: 2,
        }
    );
    assert_eq!(
        SpatialHierarchy::new(
            vec![root.clone(), node(2, None, 0, [0.0; 3], [1.0; 3], 0, &[])],
            Vec::new(),
            None,
        )
        .unwrap_err(),
        SpatialError::RootCount(2)
    );
    assert_eq!(
        SpatialHierarchy::new(
            vec![node(2, Some(9), 1, [0.0; 3], [1.0; 3], 0, &[])],
            Vec::new(),
            None,
        )
        .unwrap_err(),
        SpatialError::RootCount(0)
    );
    assert_eq!(
        SpatialHierarchy::new(
            vec![
                root.clone(),
                node(2, Some(9), 1, [0.0; 3], [1.0; 3], 0, &[])
            ],
            Vec::new(),
            None,
        )
        .unwrap_err(),
        SpatialError::UnknownParent {
            node: NodeId::new(2),
            parent: NodeId::new(9),
        }
    );
    assert_eq!(
        SpatialHierarchy::new(
            vec![
                root.clone(),
                node(2, Some(1), 2, [0.0; 3], [1.0; 3], 0, &[])
            ],
            Vec::new(),
            None,
        )
        .unwrap_err(),
        SpatialError::InvalidChildLevel {
            node: NodeId::new(2),
            parent: NodeId::new(1),
        }
    );
    assert_eq!(
        SpatialHierarchy::new(
            vec![
                root,
                node(2, Some(1), 1, [9.0, 0.0, 0.0], [11.0, 1.0, 1.0], 0, &[])
            ],
            Vec::new(),
            None,
        )
        .unwrap_err(),
        SpatialError::ChildOutsideParent {
            node: NodeId::new(2),
            parent: NodeId::new(1),
        }
    );
}

#[test]
fn validates_spatial_numeric_values_and_preserves_large_world_coordinates() {
    assert!(matches!(
        SpatialBounds::new([0.0, f64::NAN, 0.0], [1.0; 3]),
        Err(SpatialError::InvalidBounds { .. })
    ));
    assert!(matches!(
        SpatialBounds::new([0.0; 3], [1.0, -1.0, 1.0]),
        Err(SpatialError::InvalidBounds { .. })
    ));
    assert!(matches!(
        SpatialNode::new(
            NodeId::new(1),
            None,
            0,
            bounds([0.0; 3], [1.0; 3]),
            0,
            f64::INFINITY,
            Vec::new(),
        ),
        Err(SpatialError::InvalidGeometricError { .. })
    ));

    let origin = 1_000_000_000_000.0_f64;
    let child_min = origin + 1.0;
    assert_eq!(origin as f32, child_min as f32);
    let child_bounds = bounds([child_min, 0.0, 0.0], [origin + 2.0, 1.0, 1.0]);
    let hierarchy = SpatialHierarchy::new(
        vec![
            node(
                1,
                None,
                0,
                [origin, 0.0, 0.0],
                [origin + 10.0, 10.0, 10.0],
                0,
                &[],
            ),
            SpatialNode::new(
                NodeId::new(2),
                Some(NodeId::new(1)),
                1,
                child_bounds,
                u64::from(u32::MAX) + 5,
                0.0,
                Vec::new(),
            )
            .unwrap(),
        ],
        Vec::new(),
        None,
    )
    .unwrap();

    assert_eq!(
        hierarchy.node(NodeId::new(2)).unwrap().bounds().min()[0],
        child_min
    );
    assert_eq!(
        hierarchy.node(NodeId::new(2)).unwrap().point_count(),
        u64::from(u32::MAX) + 5
    );
}

#[test]
fn validates_attribute_schema_and_allows_payload_free_internal_nodes() {
    let rgb = AttributeDescriptor::new("rgb", AttributeDataType::U8, 3).unwrap();
    assert!(matches!(
        AttributeDescriptor::new("  ", AttributeDataType::F32, 1),
        Err(SpatialError::EmptyAttributeName)
    ));
    assert!(matches!(
        AttributeDescriptor::new("empty", AttributeDataType::F32, 0),
        Err(SpatialError::ZeroAttributeComponents { .. })
    ));
    assert_eq!(
        SpatialHierarchy::new(
            vec![node(1, None, 0, [0.0; 3], [1.0; 3], 0, &[])],
            vec![rgb.clone(), rgb],
            None,
        )
        .unwrap_err(),
        SpatialError::DuplicateAttributeName("rgb".to_owned())
    );

    let hierarchy = SpatialHierarchy::new(
        vec![
            node(1, None, 0, [0.0; 3], [1.0; 3], 0, &[]),
            node(2, Some(1), 1, [0.0; 3], [1.0; 3], 0, &[]),
        ],
        vec![AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap()],
        None,
    )
    .unwrap();

    assert!(hierarchy.node(NodeId::new(1)).unwrap().blocks().is_empty());
    assert_eq!(hierarchy.attributes()[0].component_count(), 1);
}

#[test]
fn includes_touching_bounds_in_spatial_queries() {
    let hierarchy = SpatialHierarchy::new(
        vec![
            node(1, None, 0, [0.0; 3], [1.0; 3], 0, &[]),
            node(2, Some(1), 1, [0.0; 3], [1.0; 3], 1, &[2]),
        ],
        Vec::new(),
        None,
    )
    .unwrap();

    let ids = hierarchy
        .query(bounds([1.0; 3], [2.0; 3]))
        .map(|item| item.id().get())
        .collect::<Vec<_>>();
    assert_eq!(ids, [1, 2]);
}

#[test]
fn frustum_query_returns_intersections_in_stable_order_and_prunes_outside_nodes() {
    let hierarchy = SpatialHierarchy::new(
        vec![
            node(1, None, 0, [-10.0; 3], [10.0; 3], 0, &[]),
            node(2, Some(1), 1, [-1.0; 3], [0.25; 3], 1, &[2]),
            node(3, Some(1), 1, [2.0, 0.0, 0.0], [3.0, 1.0, 1.0], 1, &[3]),
            node(4, Some(1), 1, [1.0, 0.2, 0.2], [2.0, 0.8, 0.8], 1, &[4]),
        ],
        Vec::new(),
        None,
    )
    .unwrap();

    let ids = hierarchy
        .query_frustum(unit_cube_frustum())
        .map(|item| item.id().get())
        .collect::<Vec<_>>();
    assert_eq!(ids, [1, 2, 4]);
}

#[test]
fn rejects_empty_frusta_and_invalid_planes() {
    assert_eq!(
        SpatialFrustum::new(Vec::new()).unwrap_err(),
        SpatialError::EmptyFrustum
    );
    assert!(matches!(
        FrustumPlane::new([0.0; 3], 1.0),
        Err(SpatialError::InvalidFrustumPlane { .. })
    ));
    assert!(matches!(
        FrustumPlane::new([1.0, f64::NAN, 0.0], 1.0),
        Err(SpatialError::InvalidFrustumPlane { .. })
    ));
    assert!(matches!(
        FrustumPlane::new([1.0, 0.0, 0.0], f64::INFINITY),
        Err(SpatialError::InvalidFrustumPlane { .. })
    ));
}

#[test]
fn frustum_classification_handles_large_finite_coordinates_without_overflow() {
    let far = 1.0e308;
    let hierarchy = SpatialHierarchy::new(
        vec![node(1, None, 0, [far; 3], [far; 3], 0, &[])],
        Vec::new(),
        None,
    )
    .unwrap();
    let frustum =
        SpatialFrustum::new(vec![FrustumPlane::new([-1.0, -1.0, 1.0], 1.7e308).unwrap()]).unwrap();

    assert_eq!(
        hierarchy
            .query_frustum(frustum)
            .map(|item| item.id().get())
            .collect::<Vec<_>>(),
        [1]
    );
}

#[test]
fn subdivision_bit_fields_preserve_every_cell_and_internal_boundary() {
    let bounds = bounds([0.0; 3], [8.0; 3]);
    for divisions in [2_u8, 4, 8] {
        let grid = SpatialSubdivision::new(bounds, divisions).unwrap();
        let d = divisions as usize;
        for x in 0..d {
            for y in 0..d {
                for z in 0..d {
                    let old_index = (x * d + y) * d + z;
                    let position = [x, y, z].map(|v| v as f64 * 8.0 / d as f64);
                    assert_eq!(grid.cell_index(position), old_index);
                    assert_eq!(grid.cell_bounds(old_index).unwrap().min(), position);
                }
            }
        }
    }
}
