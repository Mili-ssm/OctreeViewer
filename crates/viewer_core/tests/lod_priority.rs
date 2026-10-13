use std::cell::Cell;
use viewer_core::{
    BlockId, DEFAULT_LOD_PRIORITY_BYTES, FrustumPlane, HierarchySemantics, LodPriorityFallback,
    LodSelectionConfig, LodSelectionError, NodeId, SpatialBounds, SpatialFrustum, SpatialHierarchy,
    SpatialNode,
};

fn node(id: u64, parent: Option<u64>, level: u32, count: u64) -> SpatialNode {
    SpatialNode::new(
        NodeId::new(id),
        parent.map(NodeId::new),
        level,
        SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
        count,
        0.0,
        if count > 0 {
            vec![BlockId::new(id)]
        } else {
            vec![]
        },
    )
    .unwrap()
}
fn frustum() -> SpatialFrustum {
    SpatialFrustum::new(vec![FrustumPlane::new([1.0, 0.0, 0.0], 0.0).unwrap()]).unwrap()
}
fn hierarchy(nodes: Vec<SpatialNode>, semantics: HierarchySemantics) -> SpatialHierarchy {
    SpatialHierarchy::new_with_semantics(nodes, vec![], None, semantics).unwrap()
}

#[test]
fn replacement_reconsiders_a_higher_priority_blocked_branch_after_freeing_points() {
    let h = hierarchy(
        vec![
            node(0, None, 0, 0),
            node(2, Some(0), 1, 5),
            node(1, Some(0), 1, 5),
            node(4, Some(2), 2, 1),
            node(3, Some(1), 2, 6),
        ],
        HierarchySemantics::Replacement,
    );
    let config = LodSelectionConfig::new(0.0, 10).unwrap();
    let project = |n: &SpatialNode| match n.id().get() {
        1 => 10.0,
        2 => 5.0,
        _ => 0.0,
    };
    let expected = h.select_lod(&frustum(), config, project).unwrap();
    let (actual, stats) = h
        .select_lod_stable_projector(&frustum(), config, DEFAULT_LOD_PRIORITY_BYTES, project)
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.node_ids(), [NodeId::new(4), NodeId::new(3)]);
    assert_eq!(actual.point_count(), 7);
    assert!(stats.fallback.is_none());
}

#[test]
fn overflow_is_not_hidden_by_a_budget_reducing_higher_priority_refinement() {
    let h = hierarchy(
        vec![
            node(0, None, 0, 0),
            node(1, Some(0), 1, 2),
            node(2, Some(0), 1, 1),
            node(3, Some(1), 2, 1),
            node(4, Some(2), 2, u64::MAX - 1),
        ],
        HierarchySemantics::Replacement,
    );
    let config = LodSelectionConfig::new(0.0, u64::MAX).unwrap();
    let project = |n: &SpatialNode| match n.id().get() {
        1 => 10.0,
        2 => 1.0,
        _ => 0.0,
    };
    let expected = h.select_lod(&frustum(), config, project).unwrap_err();
    assert_eq!(
        expected,
        LodSelectionError::PointCountOverflow(NodeId::new(2))
    );
    assert_eq!(
        h.select_lod_stable_projector(&frustum(), config, DEFAULT_LOD_PRIORITY_BYTES, project)
            .unwrap_err(),
        expected
    );
}

#[test]
fn additive_overflow_after_growth_preserves_legacy_error() {
    let h = hierarchy(
        vec![
            node(0, None, 0, 0),
            node(1, Some(0), 1, 1),
            node(2, Some(0), 1, 1),
            node(3, Some(1), 2, 1),
            node(4, Some(2), 2, u64::MAX - 2),
        ],
        HierarchySemantics::Additive,
    );
    let config = LodSelectionConfig::new(0.0, u64::MAX).unwrap();
    let project = |n: &SpatialNode| match n.id().get() {
        1 => 10.0,
        2 => 1.0,
        _ => 0.0,
    };
    assert_eq!(
        h.select_lod_stable_projector(&frustum(), config, DEFAULT_LOD_PRIORITY_BYTES, project)
            .unwrap_err(),
        h.select_lod(&frustum(), config, project).unwrap_err()
    );
}

#[test]
fn inactive_invalid_descendants_are_not_projected_and_error_order_remains_input_order() {
    let h = hierarchy(
        vec![
            node(0, None, 0, 1),
            node(1, Some(0), 1, 10),
            node(2, Some(0), 1, 0),
        ],
        HierarchySemantics::Replacement,
    );
    let child_calls = Cell::new(0);
    let (cut, stats) = h
        .select_lod_stable_projector(
            &frustum(),
            LodSelectionConfig::new(0.0, 1).unwrap(),
            DEFAULT_LOD_PRIORITY_BYTES,
            |n| {
                if n.id() == NodeId::new(1) {
                    child_calls.set(child_calls.get() + 1);
                    f64::NAN
                } else {
                    1.0
                }
            },
        )
        .unwrap();
    assert_eq!(child_calls.get(), 0);
    assert_eq!(cut.budget_limited(), [NodeId::new(0)]);
    assert!(stats.fallback.is_none());
    let h = hierarchy(
        vec![
            node(0, None, 0, 0),
            node(50, Some(0), 1, 1),
            node(2, Some(0), 1, 1),
        ],
        HierarchySemantics::Replacement,
    );
    let project = |n: &SpatialNode| {
        if n.id() == NodeId::new(50) {
            -1.0
        } else {
            f64::NAN
        }
    };
    assert_eq!(
        h.select_lod_stable_projector(
            &frustum(),
            LodSelectionConfig::new(0.0, 2).unwrap(),
            DEFAULT_LOD_PRIORITY_BYTES,
            project
        )
        .unwrap_err(),
        LodSelectionError::InvalidProjectedError {
            node: NodeId::new(50),
            error: -1.0
        }
    );
}

#[test]
fn exhausted_priority_capacity_restarts_after_refining_without_changing_result() {
    let h = hierarchy(
        vec![
            node(0, None, 0, 1),
            node(1, Some(0), 1, 1),
            node(2, Some(0), 1, 1),
            node(3, Some(1), 2, 1),
            node(4, Some(2), 2, 1),
        ],
        HierarchySemantics::Replacement,
    );
    let config = LodSelectionConfig::new(0.0, 4).unwrap();
    let (cut, stats) = h
        .select_lod_stable_projector(&frustum(), config, 24, |_| 1.0)
        .unwrap();
    assert_eq!(cut, h.select_lod(&frustum(), config, |_| 1.0).unwrap());
    assert_eq!(stats.fallback, Some(LodPriorityFallback::ScratchLimit));
    assert_eq!(stats.peak_priority_bytes, 24);
}

#[test]
fn equal_finite_and_infinite_priorities_keep_smaller_id_and_input_diagnostic_order() {
    for semantics in [
        HierarchySemantics::Replacement,
        HierarchySemantics::Additive,
    ] {
        let h = hierarchy(
            vec![
                node(0, None, 0, 0),
                node(20, Some(0), 1, 1),
                node(10, Some(0), 1, 1),
                node(21, Some(20), 2, 1),
                node(22, Some(20), 2, 1),
                node(11, Some(10), 2, 1),
                node(12, Some(10), 2, 1),
            ],
            semantics,
        );
        let budget = if semantics == HierarchySemantics::Replacement {
            3
        } else {
            4
        };
        for error in [1.0, f64::INFINITY] {
            let config = LodSelectionConfig::new(0.0, budget).unwrap();
            let expected = h.select_lod(&frustum(), config, |_| error).unwrap();
            let (cut, stats) = h
                .select_lod_stable_projector(&frustum(), config, DEFAULT_LOD_PRIORITY_BYTES, |_| {
                    error
                })
                .unwrap();
            assert_eq!(cut, expected);
            assert_eq!(cut.budget_limited(), [NodeId::new(20)]);
            assert_eq!(cut.resolution_limited(), [NodeId::new(11), NodeId::new(12)]);
            assert!(stats.fallback.is_none());
        }
    }
}

#[test]
fn linear_hierarchies_keep_the_original_path_without_priority_allocations() {
    for semantics in [
        HierarchySemantics::Replacement,
        HierarchySemantics::Additive,
    ] {
        let h = hierarchy(
            (0..100)
                .map(|id| node(id, id.checked_sub(1), id as u32, 1))
                .collect(),
            semantics,
        );
        let config = LodSelectionConfig::new(0.0, 100).unwrap();
        let (cut, stats) = h
            .select_lod_stable_projector(&frustum(), config, DEFAULT_LOD_PRIORITY_BYTES, |_| 1.0)
            .unwrap();
        assert_eq!(cut, h.select_lod(&frustum(), config, |_| 1.0).unwrap());
        assert_eq!(stats.fallback, Some(LodPriorityFallback::LinearHierarchy));
        assert_eq!(stats.peak_priority_bytes, 0);
    }
}
