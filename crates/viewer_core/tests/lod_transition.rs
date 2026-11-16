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

fn target(
    hierarchy: &SpatialHierarchy,
    refined: bool,
    point_budget: u64,
) -> viewer_core::LodSelection {
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
            LodSelectionConfig::new(1.0, point_budget).unwrap(),
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

fn target_side(hierarchy: &SpatialHierarchy, left: bool) -> viewer_core::LodSelection {
    let (min_x, max_x) = if left { (0.0, 4.0) } else { (6.0, 10.0) };
    let frustum = SpatialFrustum::new(vec![
        FrustumPlane::new([1.0, 0.0, 0.0], -min_x).unwrap(),
        FrustumPlane::new([-1.0, 0.0, 0.0], max_x).unwrap(),
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
                if node.id() == NodeId::new(1) {
                    10.0
                } else {
                    0.0
                }
            },
        )
        .unwrap()
}

#[test]
fn replacement_keeps_parent_and_exposes_a_resident_child_until_the_cut_is_complete() {
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

    let initial = planner.update(&hierarchy, &target(&hierarchy, false, 10), &mut cache);
    assert_eq!(initial.node_ids(), [NodeId::new(1)]);

    let desired = target(&hierarchy, true, 10);
    let waiting = planner.update(&hierarchy, &desired, &mut cache);
    assert_eq!(waiting.node_ids(), [NodeId::new(1)]);
    assert_eq!(waiting.fallback_nodes(), [NodeId::new(1)]);
    assert_eq!(
        waiting.pending_blocks(),
        [BlockId::new(20), BlockId::new(30)]
    );
    assert!(waiting.coverage_complete());

    assert!(cache.insert(child_a).cached());
    let partial = planner.update(&hierarchy, &desired, &mut cache);
    assert_eq!(partial.node_ids(), [NodeId::new(1), NodeId::new(2)]);
    assert_eq!(partial.fallback_nodes(), [NodeId::new(1)]);
    assert!(partial.coverage_complete());

    assert!(cache.insert(child_b).cached());
    let complete = planner.update(&hierarchy, &desired, &mut cache);
    assert_eq!(complete.node_ids(), [NodeId::new(2), NodeId::new(3)]);
    assert_eq!(complete.block_ids(), [BlockId::new(20), BlockId::new(30)]);
    assert!(
        complete.fallback_nodes().is_empty(),
        "{:?}",
        complete.fallback_nodes()
    );
    assert!(
        complete.pending_blocks().is_empty(),
        "{:?}",
        complete.pending_blocks()
    );
    assert!(complete.coverage_complete());

    let parent_ids = complete.node_ids();
    assert!(!parent_ids.iter().any(|id| {
        hierarchy.node(*id).and_then(SpatialNode::parent).is_some()
            && parent_ids.contains(&NodeId::new(1))
    }));

    cache.remove(BlockId::new(10));
    let coarsening = planner.update(&hierarchy, &target(&hierarchy, false, 1), &mut cache);
    assert_eq!(coarsening.node_ids(), [NodeId::new(2)]);
    assert_eq!(coarsening.point_count(), 1);
    assert_eq!(coarsening.point_budget(), 1);
    assert_eq!(coarsening.pending_blocks(), [BlockId::new(10)]);
    assert_eq!(coarsening.fallback_nodes(), [NodeId::new(2)]);
    assert_eq!(coarsening.budget_omitted_nodes(), [NodeId::new(3)]);
    assert_eq!(coarsening.uncovered_nodes(), [NodeId::new(1)]);
    assert!(!coarsening.coverage_complete());

    assert!(cache.insert(block(10)).cached());
    let within_budget = planner.update(&hierarchy, &target(&hierarchy, false, 1), &mut cache);
    assert_eq!(within_budget.node_ids(), [NodeId::new(1)]);
    assert_eq!(within_budget.point_count(), 1);
    assert!(within_budget.coverage_complete());
}

#[test]
fn replacement_does_not_mix_a_child_finer_than_the_parent_representatives() {
    let bounds = SpatialBounds::new([0.0; 3], [16.0; 3]).unwrap();
    let nodes = (0..=6)
        .map(|level| {
            SpatialNode::new(
                NodeId::new(level),
                (level > 0).then(|| NodeId::new(if level == 6 { 4 } else { level - 1 })),
                level.min(5) as u32,
                match level {
                    5 => SpatialBounds::new([0.0; 3], [8.0, 16.0, 16.0]).unwrap(),
                    6 => SpatialBounds::new([8.0, 0.0, 0.0], [16.0; 3]).unwrap(),
                    _ => bounds,
                },
                u64::from(level >= 4),
                if level < 5 { 16.0 } else { 0.0 },
                match level {
                    4 => vec![BlockId::new(40)],
                    5 => vec![BlockId::new(50)],
                    6 => vec![BlockId::new(60)],
                    _ => vec![],
                },
            )
            .unwrap()
        })
        .collect();
    let hierarchy =
        SpatialHierarchy::new_with_semantics(nodes, vec![], None, HierarchySemantics::Replacement)
            .unwrap();
    let mut cache = DecodedBlockCache::new(4096);
    cache.insert(block(40));
    cache.insert(block(50));
    let frustum =
        SpatialFrustum::new(vec![FrustumPlane::new([1.0, 0.0, 0.0], 0.0).unwrap()]).unwrap();
    let target = hierarchy
        .select_lod(
            &frustum,
            LodSelectionConfig::new(1.0, 10).unwrap(),
            |node| {
                if node.level() < 5 { 16.0 } else { 0.0 }
            },
        )
        .unwrap();
    let plan = LodTransitionPlanner::new().prepare(&hierarchy, &target, &cache);
    assert_eq!(plan.node_ids(), [NodeId::new(4)]);
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
    planner.update(&hierarchy, &target(&hierarchy, false, 10), &mut cache);

    let rejected = cache.insert(replacement);
    assert!(!rejected.cached());
    assert_eq!(
        rejected.rejection(),
        Some(viewer_core::CacheInsertRejection::ProtectedEntries)
    );
    assert!(cache.contains(BlockId::new(10)));
    assert!(!cache.contains(BlockId::new(20)));
    assert!(cache.contains(BlockId::new(11)));
    assert!(
        rejected.evicted_blocks().is_empty(),
        "{:?}",
        rejected.evicted_blocks()
    );
    assert_eq!(cache.used_bytes(), budget);

    planner.reset(&mut cache);
    let replacement_result = cache.insert(block_with_points(20, 3));
    assert!(replacement_result.cached());
    assert_eq!(replacement_result.evicted_blocks(), [BlockId::new(10)]);
    assert!(cache.contains(BlockId::new(11)));
}

#[test]
fn prepared_cut_does_not_change_active_cache_protection_until_commit() {
    let hierarchy = hierarchy(HierarchySemantics::Replacement);
    let root = block(10);
    let child_a = block(20);
    let child_b = block(30);
    let budget = root.estimated_memory_bytes()
        + child_a.estimated_memory_bytes()
        + child_b.estimated_memory_bytes();
    let mut cache = DecodedBlockCache::new(budget);
    cache.insert(root);
    cache.insert(child_a);
    cache.insert(child_b);
    let mut planner = LodTransitionPlanner::new();

    let root_target = target(&hierarchy, false, 10);
    planner.update(&hierarchy, &root_target, &mut cache);

    let child_target = target(&hierarchy, true, 10);
    let _child_plan = planner.prepare(&hierarchy, &child_target, &cache);
    assert!(cache.insert(block(40)).cached());
    assert!(cache.contains(BlockId::new(10)));
    assert!(!cache.contains(BlockId::new(20)));
    assert_eq!(
        planner.prepare(&hierarchy, &root_target, &cache).node_ids(),
        [NodeId::new(1)]
    );

    planner.reset(&mut cache);
    cache.clear();
    cache.insert(block(10));
    cache.insert(block(20));
    cache.insert(block(30));
    planner.update(&hierarchy, &root_target, &mut cache);
    let child_plan = planner.prepare(&hierarchy, &child_target, &cache);
    planner.commit(&child_plan, &mut cache);
    assert!(cache.insert(block(40)).cached());
    assert!(!cache.contains(BlockId::new(10)));
    assert!(cache.contains(BlockId::new(20)));
    assert!(cache.contains(BlockId::new(30)));
}

#[test]
fn additive_transition_keeps_available_ancestors_and_reports_missing_children() {
    let hierarchy = hierarchy(HierarchySemantics::Additive);
    let mut cache = DecodedBlockCache::new(10_000);
    cache.insert(block(10));
    let mut planner = LodTransitionPlanner::new();

    let plan = planner.update(&hierarchy, &target(&hierarchy, true, 10), &mut cache);
    assert_eq!(plan.node_ids(), [NodeId::new(1)]);
    assert_eq!(plan.pending_blocks(), [BlockId::new(20), BlockId::new(30)]);
    assert_eq!(plan.uncovered_nodes(), [NodeId::new(2), NodeId::new(3)]);
    assert!(!plan.coverage_complete());
}

#[test]
fn replacement_retains_the_previous_region_until_the_new_target_is_complete() {
    let hierarchy = hierarchy(HierarchySemantics::Replacement);
    let left = block(20);
    let right = block(30);
    let mut cache =
        DecodedBlockCache::new(left.estimated_memory_bytes() + right.estimated_memory_bytes());
    cache.insert(left);
    let mut planner = LodTransitionPlanner::new();

    let first = planner.update(&hierarchy, &target_side(&hierarchy, true), &mut cache);
    assert_eq!(first.node_ids(), [NodeId::new(2)]);

    let waiting = planner.prepare(&hierarchy, &target_side(&hierarchy, false), &cache);
    assert_eq!(waiting.node_ids(), [NodeId::new(2)]);
    assert_eq!(waiting.pending_blocks(), [BlockId::new(30)]);
    assert_eq!(waiting.uncovered_nodes(), [NodeId::new(3)]);
    planner.commit(&waiting, &mut cache);
    assert!(
        cache
            .lru_entries()
            .any(|(id, _, protected)| { id == BlockId::new(20) && protected })
    );

    assert!(cache.insert(right).cached());
    let complete = planner.prepare(&hierarchy, &target_side(&hierarchy, false), &cache);
    assert_eq!(complete.node_ids(), [NodeId::new(3)]);
    assert!(
        complete.pending_blocks().is_empty(),
        "{:?}",
        complete.pending_blocks()
    );
    planner.commit(&complete, &mut cache);
    cache.remove(BlockId::new(20));

    let waiting_again = planner.prepare(&hierarchy, &target_side(&hierarchy, true), &cache);
    assert_eq!(waiting_again.node_ids(), [NodeId::new(3)]);
    assert_eq!(waiting_again.pending_blocks(), [BlockId::new(20)]);
}

#[test]
fn retained_complete_cut_stays_resident_when_the_next_region_cannot_fit() {
    let hierarchy = hierarchy(HierarchySemantics::Replacement);
    let left = block(20);
    let right = block(30);
    let mut cache = DecodedBlockCache::new(left.estimated_memory_bytes());
    cache.insert(left);
    let mut planner = LodTransitionPlanner::new();

    let first = planner.update(&hierarchy, &target_side(&hierarchy, true), &mut cache);
    assert_eq!(first.node_ids(), [NodeId::new(2)]);
    let waiting = planner.update(&hierarchy, &target_side(&hierarchy, false), &mut cache);
    assert_eq!(waiting.node_ids(), [NodeId::new(2)]);
    assert_eq!(waiting.pending_blocks(), [BlockId::new(30)]);

    let rejected = cache.insert(right);
    assert_eq!(
        rejected.rejection(),
        Some(viewer_core::CacheInsertRejection::ProtectedEntries)
    );
    assert!(cache.contains(BlockId::new(20)));
    assert!(!cache.contains(BlockId::new(30)));
    assert_eq!(cache.len(), 1);
}

#[test]
fn additive_retains_the_previous_region_only_while_the_new_target_loads() {
    let hierarchy = hierarchy(HierarchySemantics::Additive);
    let root = block(10);
    let left = block(20);
    let right = block(30);
    let mut cache = DecodedBlockCache::new(
        root.estimated_memory_bytes()
            + left.estimated_memory_bytes()
            + right.estimated_memory_bytes(),
    );
    cache.insert(root);
    cache.insert(left);
    let mut planner = LodTransitionPlanner::new();

    let first = planner.update(&hierarchy, &target_side(&hierarchy, true), &mut cache);
    assert_eq!(first.node_ids(), [NodeId::new(1), NodeId::new(2)]);

    let waiting = planner.prepare(&hierarchy, &target_side(&hierarchy, false), &cache);
    assert_eq!(waiting.node_ids(), [NodeId::new(1), NodeId::new(2)]);
    assert_eq!(waiting.pending_blocks(), [BlockId::new(30)]);
    planner.commit(&waiting, &mut cache);
    assert!(
        cache
            .lru_entries()
            .any(|(id, _, protected)| { id == BlockId::new(20) && protected })
    );

    assert!(cache.insert(right).cached());
    let complete = planner.prepare(&hierarchy, &target_side(&hierarchy, false), &cache);
    assert_eq!(complete.node_ids(), [NodeId::new(1), NodeId::new(3)]);
    assert!(
        complete.pending_blocks().is_empty(),
        "{:?}",
        complete.pending_blocks()
    );
    planner.commit(&complete, &mut cache);
    cache.remove(BlockId::new(20));
    let waiting_again = planner.prepare(&hierarchy, &target_side(&hierarchy, true), &cache);
    assert_eq!(waiting_again.node_ids(), [NodeId::new(1), NodeId::new(3)]);
    assert_eq!(waiting_again.pending_blocks(), [BlockId::new(20)]);
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
    assert_eq!(plan.point_count(), 2);
    assert_eq!(plan.point_budget(), 10);
    assert!(plan.coverage_complete());
}

#[test]
fn resident_payload_count_not_node_metadata_controls_the_strict_budget() {
    let hierarchy = SpatialHierarchy::new(
        vec![node(1, None, [0.0; 3], [10.0; 3], Some(10))],
        Vec::new(),
        None,
    )
    .unwrap();
    let mut cache = DecodedBlockCache::new(10_000);
    cache.insert(block_with_points(10, 3));
    let desired = target(&hierarchy, false, 2);
    assert_eq!(desired.point_count(), 1);

    let plan = LodTransitionPlanner::new().update(&hierarchy, &desired, &mut cache);
    assert!(plan.node_ids().is_empty(), "{:?}", plan.node_ids());
    assert!(plan.block_ids().is_empty(), "{:?}", plan.block_ids());
    assert_eq!(plan.point_count(), 0);
    assert_eq!(plan.point_budget(), 2);
    assert_eq!(plan.budget_omitted_nodes(), [NodeId::new(1)]);
    assert_eq!(plan.uncovered_nodes(), [NodeId::new(1)]);
}

#[test]
fn multi_block_nodes_are_admitted_as_a_whole_and_empty_cuts_fit_zero_budget() {
    let hierarchy = SpatialHierarchy::new(
        vec![
            SpatialNode::new(
                NodeId::new(1),
                None,
                0,
                SpatialBounds::new([0.0; 3], [10.0; 3]).unwrap(),
                2,
                0.0,
                vec![BlockId::new(10), BlockId::new(10), BlockId::new(11)],
            )
            .unwrap(),
        ],
        Vec::new(),
        None,
    )
    .unwrap();
    let mut cache = DecodedBlockCache::new(10_000);
    cache.insert(block(10));
    let desired = target(&hierarchy, false, 2);
    let mut planner = LodTransitionPlanner::new();
    let partial = planner.update(&hierarchy, &desired, &mut cache);
    assert!(partial.node_ids().is_empty(), "{:?}", partial.node_ids());
    assert_eq!(partial.pending_blocks(), [BlockId::new(11)]);
    assert_eq!(partial.uncovered_nodes(), [NodeId::new(1)]);

    cache.insert(block(11));
    let complete = planner.update(&hierarchy, &desired, &mut cache);
    assert_eq!(complete.node_ids(), [NodeId::new(1)]);
    assert_eq!(complete.block_ids(), [BlockId::new(10), BlockId::new(11)]);
    assert_eq!(complete.point_count(), 2);

    let outside =
        SpatialFrustum::new(vec![FrustumPlane::new([1.0, 0.0, 0.0], -20.0).unwrap()]).unwrap();
    let empty_target = hierarchy
        .select_lod(&outside, LodSelectionConfig::new(1.0, 0).unwrap(), |_| 0.0)
        .unwrap();
    let empty = planner.update(&hierarchy, &empty_target, &mut cache);
    assert!(empty.node_ids().is_empty(), "{:?}", empty.node_ids());
    assert_eq!(empty.point_count(), 0);
    assert_eq!(empty.point_budget(), 0);
    assert!(empty.coverage_complete());
}

#[test]
fn protected_fallback_allows_partial_children_to_rotate_under_a_small_cache_budget() {
    let hierarchy = hierarchy(HierarchySemantics::Replacement);
    let root = block(10);
    let size = root.estimated_memory_bytes();
    let mut cache = DecodedBlockCache::new(size * 2);
    assert!(cache.insert(root).cached());
    let mut planner = LodTransitionPlanner::new();
    planner.update(&hierarchy, &target(&hierarchy, false, 10), &mut cache);
    let desired = target(&hierarchy, true, 10);

    for (arriving, pending, evicted) in [(20, 30, None), (30, 20, Some(20)), (20, 30, Some(30))] {
        let inserted = cache.insert(block(arriving));
        assert!(inserted.cached());
        assert_eq!(inserted.rejection(), None);
        assert_eq!(
            inserted.evicted_blocks(),
            evicted.into_iter().map(BlockId::new).collect::<Vec<_>>()
        );
        let plan = planner.update(&hierarchy, &desired, &mut cache);
        assert_eq!(
            plan.node_ids(),
            [
                NodeId::new(1),
                NodeId::new(if arriving == 20 { 2 } else { 3 })
            ]
        );
        assert_eq!(plan.block_ids(), [BlockId::new(10), BlockId::new(arriving)]);
        assert_eq!(plan.fallback_nodes(), [NodeId::new(1)]);
        assert_eq!(plan.pending_blocks(), [BlockId::new(pending)]);
        assert!(plan.coverage_complete());
        assert!(cache.contains(BlockId::new(10)));
        assert!(!cache.contains(BlockId::new(pending)));
    }
    assert_eq!(cache.stats().rejected_oversize, 0);
    assert_eq!(cache.stats().rejected_protected, 0);
    assert_eq!(cache.stats().evictions, 2);
}

/// An experiment can own decoded blocks without adopting the LRU policy.
struct ResidentBlocks(Vec<PointBlock>);

impl viewer_core::ResidentBlockSource for ResidentBlocks {
    fn peek(&self, id: BlockId) -> Option<&PointBlock> {
        self.0.iter().find(|block| block.id() == id)
    }
}

#[test]
fn alternate_resident_source_preserves_lod_fallback_and_budgets_without_touching_lru() {
    for semantics in [
        HierarchySemantics::Replacement,
        HierarchySemantics::Additive,
    ] {
        let hierarchy = hierarchy(semantics);
        let mut cache = DecodedBlockCache::new(4096);
        cache.insert(block(10));
        let mut planner = LodTransitionPlanner::new();
        planner.update(&hierarchy, &target(&hierarchy, false, 10), &mut cache);
        // Exercise absent target blocks, partial refinement, then a complete cut.
        for ids in [vec![10], vec![10, 20], vec![10, 20, 30]] {
            let alternative = ResidentBlocks(ids.iter().map(|id| block(*id)).collect());
            for id in ids {
                if !cache.contains(BlockId::new(id)) {
                    assert!(cache.insert(block(id)).cached());
                }
            }
            let stats = cache.stats();
            let recency = cache.lru_entries().collect::<Vec<_>>();
            for budget in [1, 2, 10] {
                let target = target(&hierarchy, true, budget);
                let expected = planner.prepare(&hierarchy, &target, &cache);
                assert_eq!(planner.prepare(&hierarchy, &target, &alternative), expected);
                let erased: &dyn viewer_core::ResidentBlockSource = &alternative;
                assert_eq!(planner.prepare(&hierarchy, &target, erased), expected);
            }
            assert_eq!(cache.stats(), stats);
            assert_eq!(cache.lru_entries().collect::<Vec<_>>(), recency);
        }
    }
}
