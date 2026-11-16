use viewer_core::{
    BlockId, FrustumPlane, HierarchySemantics, LodSelectionConfig, LodSelectionError, NodeId,
    SpatialBounds, SpatialFrustum, SpatialHierarchy, SpatialNode,
};

fn cube_frustum() -> SpatialFrustum {
    SpatialFrustum::new(vec![
        FrustumPlane::new([1.0, 0.0, 0.0], 0.0).unwrap(),
        FrustumPlane::new([-1.0, 0.0, 0.0], 10.0).unwrap(),
        FrustumPlane::new([0.0, 1.0, 0.0], 0.0).unwrap(),
        FrustumPlane::new([0.0, -1.0, 0.0], 10.0).unwrap(),
        FrustumPlane::new([0.0, 0.0, 1.0], 0.0).unwrap(),
        FrustumPlane::new([0.0, 0.0, -1.0], 10.0).unwrap(),
    ])
    .unwrap()
}

fn node(
    id: u64,
    parent: Option<u64>,
    level: u32,
    min: [f64; 3],
    max: [f64; 3],
    point_count: u64,
) -> SpatialNode {
    SpatialNode::new(
        NodeId::new(id),
        parent.map(NodeId::new),
        level,
        SpatialBounds::new(min, max).unwrap(),
        point_count,
        0.0,
        (point_count > 0)
            .then_some(BlockId::new(id))
            .into_iter()
            .collect(),
    )
    .unwrap()
}

fn hierarchy(root_points: u64) -> SpatialHierarchy {
    SpatialHierarchy::new(
        vec![
            node(1, None, 0, [0.0; 3], [10.0; 3], root_points),
            node(2, Some(1), 1, [0.0; 3], [5.0; 3], 6),
            node(3, Some(1), 1, [5.0, 0.0, 0.0], [10.0; 3], 6),
        ],
        Vec::new(),
        None,
    )
    .unwrap()
}

#[test]
fn refines_a_parent_atomically_when_the_full_child_cut_fits() {
    let hierarchy = hierarchy(10);
    let selection = hierarchy
        .select_lod(
            &cube_frustum(),
            LodSelectionConfig::new(1.0, 22).unwrap(),
            |node| match node.id().get() {
                1 => 10.0,
                _ => 0.5,
            },
        )
        .unwrap();

    assert_eq!(selection.node_ids(), [NodeId::new(2), NodeId::new(3)]);
    assert_eq!(selection.point_count(), 12);
    assert_eq!(selection.point_budget(), 22);
    assert_eq!(selection.max_error_pixels(), 0.5);
    assert!(
        selection.budget_limited().is_empty(),
        "{:?}",
        selection.budget_limited()
    );
    assert!(
        selection.resolution_limited().is_empty(),
        "{:?}",
        selection.resolution_limited()
    );
}

#[test]
fn keeps_the_parent_when_only_part_of_its_refinement_would_fit() {
    let hierarchy = hierarchy(10);
    let selection = hierarchy
        .select_lod(
            &cube_frustum(),
            LodSelectionConfig::new(1.0, 11).unwrap(),
            |_| 10.0,
        )
        .unwrap();

    assert_eq!(selection.node_ids(), [NodeId::new(1)]);
    assert_eq!(selection.point_count(), 10);
    assert_eq!(selection.budget_limited(), [NodeId::new(1)]);
}

#[test]
fn traverses_structural_root_and_rejects_a_base_cut_over_budget() {
    let hierarchy = hierarchy(0);
    let result = hierarchy.select_lod(
        &cube_frustum(),
        LodSelectionConfig::new(1.0, 11).unwrap(),
        |_| 0.0,
    );

    assert_eq!(
        result.unwrap_err(),
        LodSelectionError::BaseCutExceedsBudget {
            required: 12,
            budget: 11,
        }
    );
}

#[test]
fn frustum_exclusion_consumes_no_budget_and_leaves_report_unsatisfied_error() {
    let hierarchy = hierarchy(10);
    let outside =
        SpatialFrustum::new(vec![FrustumPlane::new([1.0, 0.0, 0.0], -20.0).unwrap()]).unwrap();
    let empty = hierarchy
        .select_lod(&outside, LodSelectionConfig::new(0.0, 0).unwrap(), |_| 0.0)
        .unwrap();
    assert!(empty.node_ids().is_empty(), "{:?}", empty.node_ids());
    assert_eq!(empty.point_count(), 0);

    let leaf = SpatialHierarchy::new(
        vec![node(7, None, 0, [0.0; 3], [1.0; 3], 4)],
        Vec::new(),
        None,
    )
    .unwrap();
    let limited = leaf
        .select_lod(
            &cube_frustum(),
            LodSelectionConfig::new(1.0, 4).unwrap(),
            |_| 2.0,
        )
        .unwrap();
    assert_eq!(limited.resolution_limited(), [NodeId::new(7)]);
    assert_eq!(limited.max_error_pixels(), 2.0);
}

#[test]
fn rejects_invalid_projected_errors_and_pixel_tolerances() {
    assert!(matches!(
        LodSelectionConfig::new(f64::NAN, 10),
        Err(LodSelectionError::InvalidPixelTolerance(_))
    ));

    let hierarchy = hierarchy(10);
    assert!(matches!(
        hierarchy.select_lod(
            &cube_frustum(),
            LodSelectionConfig::new(1.0, 10).unwrap(),
            |_| f64::NAN,
        ),
        Err(LodSelectionError::InvalidProjectedError { .. })
    ));
}

#[test]
fn retries_blocked_refinements_after_a_cheaper_substitution_reduces_the_cut() {
    let hierarchy = SpatialHierarchy::new(
        vec![
            node(1, None, 0, [0.0; 3], [10.0; 3], 0),
            node(2, Some(1), 1, [0.0; 3], [5.0; 3], 10),
            node(3, Some(1), 1, [5.0, 0.0, 0.0], [10.0; 3], 10),
            node(4, Some(2), 2, [0.0; 3], [5.0; 3], 1),
            node(5, Some(3), 2, [5.0, 0.0, 0.0], [7.5, 10.0, 10.0], 8),
            node(6, Some(3), 2, [7.5, 0.0, 0.0], [10.0, 10.0, 10.0], 8),
        ],
        Vec::new(),
        None,
    )
    .unwrap();

    let selection = hierarchy
        .select_lod(
            &cube_frustum(),
            LodSelectionConfig::new(1.0, 25).unwrap(),
            |node| match node.id().get() {
                2 => 100.0,
                3 => 99.0,
                _ => 0.0,
            },
        )
        .unwrap();

    assert_eq!(
        selection.node_ids(),
        [NodeId::new(4), NodeId::new(5), NodeId::new(6)]
    );
    assert_eq!(selection.point_count(), 17);
}

#[test]
fn reports_point_count_overflow_while_building_the_visible_cut() {
    let hierarchy = SpatialHierarchy::new(
        vec![
            node(1, None, 0, [0.0; 3], [10.0; 3], 0),
            node(2, Some(1), 1, [0.0; 3], [5.0; 3], u64::MAX),
            node(3, Some(1), 1, [5.0, 0.0, 0.0], [10.0; 3], 1),
        ],
        Vec::new(),
        None,
    )
    .unwrap();

    assert_eq!(
        hierarchy
            .select_lod(
                &cube_frustum(),
                LodSelectionConfig::new(1.0, u64::MAX).unwrap(),
                |_| 0.0,
            )
            .unwrap_err(),
        LodSelectionError::PointCountOverflow(NodeId::new(3))
    );
}

#[test]
fn additive_refinement_keeps_ancestor_payloads_and_counts_each_once() {
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            node(1, None, 0, [0.0; 3], [10.0; 3], 2),
            node(2, Some(1), 1, [0.0; 3], [5.0; 3], 3),
            node(3, Some(2), 2, [0.0; 3], [2.5; 3], 5),
        ],
        Vec::new(),
        Some(10),
        HierarchySemantics::Additive,
    )
    .unwrap();

    let selection = hierarchy
        .select_lod(
            &cube_frustum(),
            LodSelectionConfig::new(1.0, 10).unwrap(),
            |_| 10.0,
        )
        .unwrap();

    assert_eq!(
        selection.node_ids(),
        [NodeId::new(1), NodeId::new(2), NodeId::new(3)]
    );
    assert_eq!(selection.point_count(), 10);
}

#[test]
fn additive_refinement_respects_budget_without_dropping_ancestors() {
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            node(1, None, 0, [0.0; 3], [10.0; 3], 2),
            node(2, Some(1), 1, [0.0; 3], [5.0; 3], 3),
            node(3, Some(2), 2, [0.0; 3], [2.5; 3], 5),
        ],
        Vec::new(),
        Some(10),
        HierarchySemantics::Additive,
    )
    .unwrap();

    let selection = hierarchy
        .select_lod(
            &cube_frustum(),
            LodSelectionConfig::new(1.0, 5).unwrap(),
            |_| 10.0,
        )
        .unwrap();

    assert_eq!(selection.node_ids(), [NodeId::new(1), NodeId::new(2)]);
    assert_eq!(selection.point_count(), 5);
    assert_eq!(selection.budget_limited(), [NodeId::new(2)]);
}

#[test]
fn equal_error_budget_tie_refines_the_smallest_sibling_id_for_both_semantics() {
    fn sibling_hierarchy(
        semantics: HierarchySemantics,
        reverse_siblings: bool,
    ) -> SpatialHierarchy {
        let root = node(100, None, 0, [0.0; 3], [10.0; 3], 0);
        let lower = node(10, Some(100), 1, [0.0; 3], [5.0, 10.0, 10.0], 2);
        let higher = node(20, Some(100), 1, [5.0, 0.0, 0.0], [10.0; 3], 2);
        let mut nodes = vec![root];
        if reverse_siblings {
            nodes.extend([higher, lower]);
        } else {
            nodes.extend([lower, higher]);
        }
        nodes.extend([
            node(11, Some(10), 2, [0.0; 3], [2.5, 10.0, 10.0], 2),
            node(12, Some(10), 2, [2.5, 0.0, 0.0], [5.0, 10.0, 10.0], 2),
            node(21, Some(20), 2, [5.0, 0.0, 0.0], [7.5, 10.0, 10.0], 2),
            node(22, Some(20), 2, [7.5, 0.0, 0.0], [10.0; 3], 2),
        ]);
        SpatialHierarchy::new_with_semantics(
            nodes,
            Vec::new(),
            (semantics == HierarchySemantics::Additive).then_some(12),
            semantics,
        )
        .unwrap()
    }

    for semantics in [
        HierarchySemantics::Replacement,
        HierarchySemantics::Additive,
    ] {
        let budget = match semantics {
            HierarchySemantics::Replacement => 6,
            HierarchySemantics::Additive => 8,
        };
        for tied_error in [5.0, f64::INFINITY] {
            for reverse_siblings in [false, true] {
                let hierarchy = sibling_hierarchy(semantics, reverse_siblings);
                let selection = hierarchy
                    .select_lod(
                        &cube_frustum(),
                        LodSelectionConfig::new(1.0, budget).unwrap(),
                        |candidate| match candidate.id().get() {
                            10 | 20 => tied_error,
                            _ => 0.0,
                        },
                    )
                    .unwrap();

                let expected_ids = match (semantics, reverse_siblings) {
                    (HierarchySemantics::Replacement, false) => vec![11, 12, 20],
                    (HierarchySemantics::Replacement, true) => vec![20, 11, 12],
                    (HierarchySemantics::Additive, false) => vec![10, 11, 12, 20],
                    (HierarchySemantics::Additive, true) => vec![20, 10, 11, 12],
                };
                assert_eq!(
                    selection.node_ids(),
                    expected_ids
                        .into_iter()
                        .map(NodeId::new)
                        .collect::<Vec<_>>()
                );
                assert_eq!(selection.point_count(), budget);
                assert_eq!(selection.point_budget(), budget);
                assert_eq!(selection.budget_limited(), [NodeId::new(20)]);
                assert!(
                    selection.resolution_limited().is_empty(),
                    "{:?}",
                    selection.resolution_limited()
                );
                assert_eq!(selection.max_error_pixels(), tied_error);
            }
        }
    }
}
