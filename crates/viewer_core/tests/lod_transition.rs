use viewer_core::{
    AttributeColumn, BlockId, ColorRgb, DecodedBlockCache, FrustumPlane, HierarchySemantics,
    LodSelectionConfig, LodTransitionPlanner, NodeId, Point, PointBatch, PointBlock, SpatialBounds,
    SpatialFrustum, SpatialHierarchy, SpatialNode,
};

fn node(
    id: u64,
    parent: Option<u64>,
    min: [f64; 3],
    max: [f64; 3],
    block_id: Option<u64>,
) -> SpatialNode {
    SpatialNode::new(
        NodeId::new(id),
        parent.map(NodeId::new),
        match parent {
            None => 0,
            Some(1) => 1,
            Some(_) => 2,
        },
        SpatialBounds::new(min, max).unwrap(),
        u64::from(block_id.is_some()),
        if id == 1 { 10.0 } else { 0.0 },
        block_id.into_iter().map(BlockId::new).collect(),
    )
    .unwrap()
}

fn hierarchy(semantics: HierarchySemantics) -> SpatialHierarchy {
    SpatialHierarchy::new_with_semantics(
        vec![
            node(1, None, [0.0; 3], [10.0; 3], Some(10)),
            node(2, Some(1), [0.0; 3], [5.0, 10.0, 10.0], Some(20)),
            node(3, Some(1), [5.0, 0.0, 0.0], [10.0; 3], Some(30)),
        ],
        Vec::new(),
        Some(3),
        semantics,
    )
    .unwrap()
}

fn block(id: u64) -> PointBlock {
    block_with_points(id, 1)
}

fn block_with_points(id: u64, point_count: usize) -> PointBlock {
    PointBlock::new(
        BlockId::new(id),
        PointBatch::new(
            (0..point_count)
                .map(|index| {
                    Point::new([id as f32 + index as f32, 0.0, 0.0], ColorRgb::new(1, 2, 3))
                })
                .collect(),
        ),
        Vec::<AttributeColumn>::new(),
    )
}

fn target(hierarchy: &SpatialHierarchy, refined: bool) -> viewer_core::LodSelection {
    let frustum = SpatialFrustum::new(vec![
        FrustumPlane::new([1.0, 0.0, 0.0], 0.0).unwrap(),
        FrustumPlane::new([-1.0, 0.0, 0.0], 10.0).unwrap(),
        FrustumPlane::new([0.0, 1.0, 0.0], 0.0).unwrap(),
        FrustumPlane::new([0.0, -1.0, 0.0], 10.0).unwrap(),
        FrustumPlane::new([0.0, 0.0, 1.0], 0.0).unwrap(),
        FrustumPlane::new([0.0, 0.0, -1.0], 10.0).unwrap(),
    ])
    .unwrap();
    hierarchy
        .select_lod(
            &frustum,
            LodSelectionConfig::new(1.0, 10).unwrap(),
            |node| {
                if refined && node.id() == NodeId::new(1) {
                    10.0
                } else {
                    0.0
                }
            },
        )
        .unwrap()
}

#[test]
fn replacement_keeps_parent_until_the_complete_child_cut_is_resident() {
    let hierarchy = hierarchy(HierarchySemantics::Replacement);
    let root = block(10);
    let child_a = block(20);
    let child_b = block(30);
    let budget = root.estimated_memory_bytes()
        + child_a.estimated_memory_bytes()
        + child_b.estimated_memory_bytes();
    let mut cache = DecodedBlockCache::new(budget);
    cache.insert(root);
    let mut planner = LodTransitionPlanner::new();

    let initial = planner.update(&hierarchy, &target(&hierarchy, false), &mut cache);
    assert_eq!(initial.node_ids(), [NodeId::new(1)]);

    let desired = target(&hierarchy, true);
    let waiting = planner.update(&hierarchy, &desired, &mut cache);
    assert_eq!(waiting.node_ids(), [NodeId::new(1)]);
    assert_eq!(waiting.fallback_nodes(), [NodeId::new(1)]);
    assert_eq!(
        waiting.pending_blocks(),
        [BlockId::new(20), BlockId::new(30)]
    );
    assert!(waiting.coverage_complete());

    assert!(cache.insert(child_a).cached);
    let partial = planner.update(&hierarchy, &desired, &mut cache);
    assert_eq!(partial.node_ids(), [NodeId::new(1)]);
    assert!(partial.coverage_complete());

    assert!(cache.insert(child_b).cached);
    let complete = planner.update(&hierarchy, &desired, &mut cache);
    assert_eq!(complete.node_ids(), [NodeId::new(2), NodeId::new(3)]);
    assert_eq!(complete.block_ids(), [BlockId::new(20), BlockId::new(30)]);
    assert!(complete.fallback_nodes().is_empty());
    assert!(complete.pending_blocks().is_empty());
    assert!(complete.coverage_complete());

    let parent_ids = complete.node_ids();
    assert!(!parent_ids.iter().any(|id| {
        hierarchy.node(*id).and_then(SpatialNode::parent).is_some()
            && parent_ids.contains(&NodeId::new(1))
    }));

    cache.remove(BlockId::new(10));
    let coarsening = planner.update(&hierarchy, &target(&hierarchy, false), &mut cache);
    assert_eq!(coarsening.node_ids(), [NodeId::new(2), NodeId::new(3)]);
    assert_eq!(coarsening.pending_blocks(), [BlockId::new(10)]);
    assert_eq!(
        coarsening.fallback_nodes(),
        [NodeId::new(2), NodeId::new(3)]
    );
    assert_eq!(coarsening.uncovered_nodes(), [NodeId::new(1)]);
    assert!(!coarsening.coverage_complete());
}

#[test]
fn protected_active_cut_blocks_an_over_budget_transition_without_eviction() {
    let hierarchy = hierarchy(HierarchySemantics::Replacement);
    let root = block_with_points(10, 3);
    let unrelated = block(11);
    let replacement = block_with_points(20, 3);
    let budget = root.estimated_memory_bytes();
    let budget = budget + unrelated.estimated_memory_bytes();
    let mut cache = DecodedBlockCache::new(budget);
    cache.insert(root);
    cache.insert(unrelated);
    let mut planner = LodTransitionPlanner::new();
    planner.update(&hierarchy, &target(&hierarchy, false), &mut cache);

    let rejected = cache.insert(replacement);
    assert!(!rejected.cached);
    assert_eq!(
        rejected.rejection,
        Some(viewer_core::CacheInsertRejection::ProtectedEntries)
    );
    assert!(cache.contains(BlockId::new(10)));
    assert!(!cache.contains(BlockId::new(20)));
    assert!(cache.contains(BlockId::new(11)));
    assert!(rejected.evicted_blocks.is_empty());
    assert_eq!(cache.used_bytes(), budget);

    planner.reset(&mut cache);
    let replacement_result = cache.insert(block_with_points(20, 3));
    assert!(replacement_result.cached);
    assert_eq!(replacement_result.evicted_blocks, [BlockId::new(10)]);
    assert!(cache.contains(BlockId::new(11)));
}

#[test]
fn additive_transition_keeps_available_ancestors_and_reports_missing_children() {
    let hierarchy = hierarchy(HierarchySemantics::Additive);
    let mut cache = DecodedBlockCache::new(10_000);
    cache.insert(block(10));
    let mut planner = LodTransitionPlanner::new();

    let plan = planner.update(&hierarchy, &target(&hierarchy, true), &mut cache);
    assert_eq!(plan.node_ids(), [NodeId::new(1)]);
    assert_eq!(plan.pending_blocks(), [BlockId::new(20), BlockId::new(30)]);
    assert_eq!(plan.uncovered_nodes(), [NodeId::new(2), NodeId::new(3)]);
    assert!(!plan.coverage_complete());
}

#[test]
fn replacement_subtrees_advance_independently_and_structural_nodes_are_not_fallbacks() {
    let hierarchy = SpatialHierarchy::new(
        vec![
            node(1, None, [0.0; 3], [10.0; 3], None),
            node(2, Some(1), [0.0; 3], [5.0, 10.0, 10.0], Some(20)),
            node(3, Some(1), [5.0, 0.0, 0.0], [10.0; 3], Some(30)),
            node(4, Some(2), [0.0; 3], [2.5, 10.0, 10.0], Some(40)),
            node(5, Some(2), [2.5, 0.0, 0.0], [5.0, 10.0, 10.0], Some(40)),
            node(6, Some(3), [5.0, 0.0, 0.0], [7.5, 10.0, 10.0], Some(60)),
            node(7, Some(3), [7.5, 0.0, 0.0], [10.0; 3], Some(70)),
        ],
        Vec::new(),
        None,
    )
    .unwrap();
    let mut cache = DecodedBlockCache::new(10_000);
    for id in [30, 40] {
        cache.insert(block(id));
    }
    let desired = hierarchy
        .select_lod(
            &SpatialFrustum::new(vec![FrustumPlane::new([1.0, 0.0, 0.0], 0.0).unwrap()]).unwrap(),
            LodSelectionConfig::new(1.0, 10).unwrap(),
            |node| if node.level() < 2 { 10.0 } else { 0.0 },
        )
        .unwrap();
    assert_eq!(
        desired.node_ids(),
        [
            NodeId::new(4),
            NodeId::new(5),
            NodeId::new(6),
            NodeId::new(7)
        ]
    );

    let mut planner = LodTransitionPlanner::new();
    let plan = planner.update(&hierarchy, &desired, &mut cache);
    assert_eq!(
        plan.node_ids(),
        [NodeId::new(3), NodeId::new(4), NodeId::new(5)]
    );
    assert_eq!(plan.fallback_nodes(), [NodeId::new(3)]);
    assert_eq!(plan.block_ids(), [BlockId::new(30), BlockId::new(40)]);
    assert_eq!(plan.pending_blocks(), [BlockId::new(60), BlockId::new(70)]);
    assert!(plan.coverage_complete());
}
