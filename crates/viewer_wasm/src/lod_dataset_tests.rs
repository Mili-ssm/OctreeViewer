use viewer_core::{
    BlockId, ColorRgb, DecodedBlockCache, FrustumPlane, HierarchySemantics, LodSelection,
    LodSelectionConfig, LodTransitionPlan, LodTransitionPlanner, NodeId, Point, PointBatch,
    PointBlock, SpatialBounds, SpatialFrustum, SpatialHierarchy, SpatialNode,
};

use std::sync::Arc;

use super::{
    CacheCycleDetector, LodDataset, LodDatasetError, SuccessfulBuildContext, bounds_side,
    highest_priority_pending_block, next_lod_stage, node_spacing, prefix_extension_points,
    prioritized_pending_blocks, priority_precedes, resident_target_plan_is_complete,
    reusable_build_match,
};
use viewer_render_wgpu::{OrbitCamera, RenderScene, SceneBounds, build_voxel_scene};

fn scene_for_test(
    hierarchy: &SpatialHierarchy,
    ids: &[NodeId],
    cache: &DecodedBlockCache,
    origin: [f64; 3],
) -> RenderScene {
    build_voxel_scene(hierarchy, ids, cache, origin, 1.0, &|bounds| {
        bounds_side(bounds) * 8.0
    })
    .scene
}

#[test]
fn progressive_lod_advances_from_coarse_to_requested_pixel_stages() {
    let (stage, continue_loading) = next_lod_stage(None, 4.0, false, true);
    assert_eq!(stage, Some(8.0));
    assert!(continue_loading);

    let (stage, continue_loading) = next_lod_stage(stage, 4.0, false, true);
    assert_eq!(stage, Some(4.0));
    assert!(continue_loading);

    let (stage, continue_loading) = next_lod_stage(stage, 4.0, false, true);
    assert_eq!(stage, Some(4.0));
    assert!(!continue_loading);
}

#[test]
fn progressive_lod_can_refine_to_half_pixel_target() {
    let mut stage = None;
    let mut stages = vec![16.0];
    loop {
        let (next, continue_loading) = next_lod_stage(stage, 0.5, false, true);
        if next == stage {
            assert!(!continue_loading);
            break;
        }
        stage = next;
        stages.push(stage.unwrap());
    }

    assert_eq!(stages, [16.0, 8.0, 4.0, 2.0, 1.0, 0.5]);
}

#[test]
fn progressive_lod_waits_for_current_stage_blocks_and_stops_on_read_failure() {
    assert_eq!(next_lod_stage(None, 4.0, true, true), (None, true));
    assert_eq!(
        next_lod_stage(Some(8.0), 4.0, true, true),
        (Some(8.0), true)
    );
    assert_eq!(
        next_lod_stage(Some(8.0), 4.0, true, false),
        (Some(8.0), false)
    );
    assert_eq!(next_lod_stage(None, 16.0, false, true), (Some(16.0), false));
}

#[test]
fn resident_target_requires_complete_multiblock_coverage_and_all_budgets() {
    fn plan(
        semantics: HierarchySemantics,
        budget: u64,
        resident_blocks: &[u64],
    ) -> (viewer_core::LodSelection, viewer_core::LodTransitionPlan) {
        let hierarchy = SpatialHierarchy::new_with_semantics(
            vec![
                SpatialNode::new(
                    NodeId::new(1),
                    None,
                    0,
                    SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                    1,
                    100.0,
                    vec![BlockId::new(10)],
                )
                .unwrap(),
                SpatialNode::new(
                    NodeId::new(2),
                    Some(NodeId::new(1)),
                    1,
                    SpatialBounds::new([0.0; 3], [0.5; 3]).unwrap(),
                    1,
                    0.0,
                    vec![BlockId::new(11)],
                )
                .unwrap(),
                SpatialNode::new(
                    NodeId::new(3),
                    Some(NodeId::new(1)),
                    1,
                    SpatialBounds::new([0.5, 0.0, 0.0], [1.0, 0.5, 0.5]).unwrap(),
                    1,
                    0.0,
                    vec![BlockId::new(12)],
                )
                .unwrap(),
            ],
            Vec::new(),
            None,
            semantics,
        )
        .unwrap();
        let frustum =
            SpatialFrustum::new(vec![FrustumPlane::new([1.0, 0.0, 0.0], 10.0).unwrap()]).unwrap();
        let target = hierarchy
            .select_lod(
                &frustum,
                LodSelectionConfig::new(4.0, budget).unwrap(),
                |node| if node.level() == 0 { 100.0 } else { 0.0 },
            )
            .unwrap();
        let mut cache = DecodedBlockCache::new(usize::MAX);
        for id in resident_blocks {
            cache.insert(if *id == 11 {
                block_with_points(*id, 2)
            } else {
                block(*id)
            });
        }
        let prepared = LodTransitionPlanner::new().prepare(&hierarchy, &target, &cache);
        (target, prepared)
    }

    let (target, prepared) = plan(HierarchySemantics::Replacement, u64::MAX, &[11, 12]);
    assert!(resident_target_plan_is_complete(&target, &prepared));
    assert_eq!(prepared.block_ids(), [BlockId::new(11), BlockId::new(12)]);

    let (target, prepared) = plan(HierarchySemantics::Replacement, u64::MAX, &[10, 11]);
    assert!(prepared.uncovered_nodes().is_empty()); // The visible parent still covers the gap.
    assert_eq!(prepared.pending_blocks(), [BlockId::new(12)]);
    assert!(!resident_target_plan_is_complete(&target, &prepared));

    let (target, prepared) = plan(HierarchySemantics::Additive, u64::MAX, &[10, 11]);
    assert_eq!(prepared.pending_blocks(), [BlockId::new(12)]);
    assert_eq!(prepared.uncovered_nodes(), [NodeId::new(3)]);
    assert!(!resident_target_plan_is_complete(&target, &prepared));

    let (target, prepared) = plan(HierarchySemantics::Additive, 3, &[10, 11, 12]);
    assert!(!prepared.budget_omitted_nodes().is_empty());
    assert!(!resident_target_plan_is_complete(&target, &prepared));

    let (target, prepared) = plan(HierarchySemantics::Replacement, 1, &[10]);
    assert!(!target.budget_limited().is_empty());
    assert!(prepared.pending_blocks().is_empty());
    assert!(!resident_target_plan_is_complete(&target, &prepared));
}

#[test]
fn progressive_block_priority_prefers_larger_error_then_lower_block_id() {
    assert!(priority_precedes(
        (BlockId::new(2), 8.0),
        (BlockId::new(1), 4.0)
    ));
    assert!(priority_precedes(
        (BlockId::new(1), 4.0),
        (BlockId::new(2), 4.0)
    ));
    assert!(!priority_precedes(
        (BlockId::new(2), 4.0),
        (BlockId::new(1), 4.0)
    ));
}

fn dataset(cache_bytes: usize) -> LodDataset {
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            SpatialNode::new(
                NodeId::new(1),
                None,
                0,
                SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                1,
                0.0,
                vec![BlockId::new(10)],
            )
            .unwrap(),
        ],
        Vec::new(),
        Some(1),
        HierarchySemantics::Replacement,
    )
    .unwrap();
    LodDataset::new([0.0; 3], hierarchy, cache_bytes).unwrap()
}

fn block(id: u64) -> PointBlock {
    block_with_points(id, 1)
}

fn block_with_points(id: u64, count: usize) -> PointBlock {
    PointBlock::new(
        BlockId::new(id),
        PointBatch::new(
            (0..count)
                .map(|_| Point::new([0.5; 3], ColorRgb::new(1, 2, 3)))
                .collect(),
        ),
        Vec::new(),
    )
}

fn build_context(
    plane_offset: f64,
    viewport: [u32; 2],
    stage_voxel_pixels: f64,
) -> SuccessfulBuildContext {
    let mut camera = OrbitCamera::default();
    camera.set_pose([plane_offset, -5.0, 2.0], [0.0; 3]);
    SuccessfulBuildContext {
        camera,
        viewport,
        stage_voxel_pixels,
    }
}

#[test]
fn reusable_build_match_requires_same_cut_and_every_build_input() {
    let nodes = [NodeId::new(1), NodeId::new(2)];
    let blocks = [BlockId::new(10), BlockId::new(11)];
    let baseline = build_context(1.0, [800, 600], 4.0);

    assert_eq!(
        reusable_build_match(
            Some(&nodes),
            Some(&blocks),
            &nodes,
            &blocks,
            Some(&baseline),
            &baseline
        ),
        (true, true)
    );
    assert_eq!(
        reusable_build_match(
            Some(&nodes),
            Some(&blocks),
            &[NodeId::new(1)],
            &blocks,
            Some(&baseline),
            &baseline
        ),
        (false, false)
    );
    for changed in [
        build_context(2.0, [800, 600], 4.0),
        build_context(1.0, [1024, 600], 4.0),
        build_context(1.0, [800, 600], 8.0),
    ] {
        assert_eq!(
            reusable_build_match(
                Some(&nodes),
                Some(&blocks),
                &nodes,
                &blocks,
                Some(&baseline),
                &changed
            ),
            (true, false)
        );
    }
}

#[test]
fn prefix_measurement_preserves_traversal_order_and_context_invalidation() {
    let make = |id, parent: Option<u64>, level, ids: Vec<u64>| {
        SpatialNode::new(
            NodeId::new(id),
            parent.map(NodeId::new),
            level,
            SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
            ids.len() as u64,
            0.0,
            ids.into_iter().map(BlockId::new).collect(),
        )
        .unwrap()
    };
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            make(1, None, 0, vec![13]),
            make(2, Some(1), 1, vec![11, 10]),
            make(3, Some(1), 1, vec![12]),
            make(4, Some(1), 1, vec![10]),
        ],
        vec![],
        None,
        HierarchySemantics::Additive,
    )
    .unwrap();
    let mut cache = DecodedBlockCache::new(4096);
    for id in 10..=13 {
        assert!(cache.insert(block(id)).cached());
    }
    let baseline = build_context(1.0, [800, 600], 4.0);
    let measure = |old: &[u64], new: &[u64], context: Option<&SuccessfulBuildContext>| {
        prefix_extension_points(
            &hierarchy,
            &cache,
            Some(&old.iter().copied().map(NodeId::new).collect::<Vec<_>>()),
            &new.iter().copied().map(NodeId::new).collect::<Vec<_>>(),
            context,
            &baseline,
        )
    };
    assert_eq!(measure(&[2], &[2, 3], Some(&baseline)), Some((2, 1)));
    // Shared references count exactly as the builder visits them.
    assert_eq!(measure(&[2], &[2, 4], Some(&baseline)), Some((2, 1)));
    for (old, new) in [
        (&[2][..], &[2][..]),
        (&[2, 3], &[2]),
        (&[1], &[2, 3]),
        (&[2, 3], &[3, 2]),
        (&[4], &[2]),
        (&[], &[2]),
    ] {
        assert_eq!(measure(old, new, Some(&baseline)), None);
    }
    // Absent context also models payload/attribute invalidation.
    assert_eq!(measure(&[2], &[2, 3], None), None);
    for changed in [
        build_context(2.0, [800, 600], 4.0),
        build_context(1.0, [1024, 600], 4.0),
        build_context(1.0, [800, 600], 8.0),
    ] {
        assert_eq!(measure(&[2], &[2, 3], Some(&changed)), None);
    }
}

#[test]
fn accepts_only_referenced_blocks_and_accounts_decoded_memory() {
    let payload_bytes = block(10).estimated_memory_bytes();
    let mut dataset = dataset(payload_bytes);
    assert!(matches!(
        dataset.insert_block(block(11)),
        Err(LodDatasetError::UnknownBlock(id)) if id == BlockId::new(11)
    ));
    let accounted_bytes = dataset.insert_block(block(10)).unwrap();
    assert_eq!(accounted_bytes, payload_bytes);
    assert_eq!(dataset.cache_stats().insertions, 1);
    assert_eq!(dataset.cache_bytes(), payload_bytes);
    assert!(matches!(
        dataset.insert_block(block_with_points(10, 2)),
        Err(LodDatasetError::CacheRejected(_))
    ));
    assert_eq!(dataset.cache_stats().rejected_oversize, 1);
    assert_eq!(dataset.cache_stats().insertions, 1);
}

#[test]
fn replacing_visible_payload_invalidates_build_context_only_after_success() {
    let mut dataset = dataset(block(10).estimated_memory_bytes());
    dataset.insert_block(block(10)).unwrap();
    let frustum = viewer_core::SpatialFrustum::new(vec![
        viewer_core::FrustumPlane::new([1.0, 0.0, 0.0], 1.0).unwrap(),
    ])
    .unwrap();
    let target = dataset
        .metadata
        .hierarchy()
        .select_lod(
            &frustum,
            viewer_core::LodSelectionConfig::new(4.0, u64::MAX).unwrap(),
            |_| 0.0,
        )
        .unwrap();
    dataset.visible_plan = Some(dataset.planner.prepare(
        dataset.metadata.hierarchy(),
        &target,
        &dataset.cache,
    ));
    dataset.last_successful_build = Some(build_context(1.0, [800, 600], 4.0));
    assert!(dataset.insert_block(block_with_points(10, 2)).is_err());
    assert!(dataset.last_successful_build.is_some());
    dataset.cache_cycle = Some(CacheCycleDetector::new(&dataset.cache, 4.0, 4.0));
    dataset.insert_block(block(10)).unwrap();
    // Arrivals feed the detector; only a new target or camera resets it.
    assert!(dataset.cache_cycle.is_some());
    assert!(dataset.last_successful_build.is_none());
}

#[test]
fn cache_cycles_compare_order_sizes_and_protection_without_activity_counters() {
    let mut cache = DecodedBlockCache::new(4096);
    cache.insert(block(10));
    cache.insert(block(11));
    let mut detector = CacheCycleDetector::new(&cache, 4.0, 4.0);
    // A different LRU order must not match, despite identical membership.
    cache.get(BlockId::new(10));
    assert!(!detector.observe(&cache));
    cache.set_protected_blocks([BlockId::new(10)]);
    assert!(!detector.observe(&cache));
    cache.set_protected_blocks([]);
    cache.insert(block_with_points(10, 2));
    assert!(!detector.observe(&cache));
    // Resident state can recur even though hit/insertion counters have advanced.
    let mut detector = CacheCycleDetector::new(&cache, 4.0, 4.0);
    cache.get(BlockId::new(11));
    cache.get(BlockId::new(10));
    assert!(detector.observe(&cache));
    assert!(detector.paused);
}

#[test]
fn cache_cycle_detection_handles_longer_cycles_and_finite_progress() {
    let mut cache = DecodedBlockCache::new(block(10).estimated_memory_bytes());
    cache.insert(block(10));
    let mut detector = CacheCycleDetector::new(&cache, 4.0, 4.0);
    // A long sequence of new states is progress, regardless of checkpoint intervals.
    for id in 11..30 {
        cache.insert(block(id));
        assert!(!detector.observe(&cache));
    }
    // A cycle longer than two is detected without retaining its full history.
    let mut detected = false;
    for id in [40, 41, 42, 43].into_iter().cycle().take(32) {
        cache.insert(block(id));
        if detector.observe(&cache) {
            detected = true;
            break;
        }
    }
    assert!(detected);
    assert_eq!(detector.checkpoint.len(), 1);
}

#[test]
#[ignore = "release microbenchmark; run with --release --ignored --nocapture"]
fn cache_cycle_detector_microbenchmark() {
    use std::{hint::black_box, time::Instant};

    const ENTRIES: u64 = 500;
    const ITERATIONS: u64 = 100_000;
    const ROUNDS: usize = 5;

    fn populated_cache() -> DecodedBlockCache {
        let mut cache = DecodedBlockCache::new(usize::MAX);
        for id in 1..=ENTRIES {
            cache.insert(block(id));
        }
        cache
    }

    let init_cache = populated_cache();
    let init_started = Instant::now();
    for _ in 0..ITERATIONS / 10 {
        black_box(CacheCycleDetector::new(&init_cache, 4.0, 4.0));
    }
    let init_ns = init_started.elapsed().as_nanos() / u128::from(ITERATIONS / 10);

    let mut match_ns = Vec::with_capacity(ROUNDS);
    let mut mismatch_ns = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let matching_cache = populated_cache();
        let mut matching_detector = CacheCycleDetector::new(&matching_cache, 4.0, 4.0);
        matching_detector.power = u64::MAX;
        let match_started = Instant::now();
        for _ in 0..ITERATIONS {
            black_box(matching_detector.observe(&matching_cache));
        }
        match_ns.push(match_started.elapsed().as_nanos() / u128::from(ITERATIONS));

        let mut changed_cache = populated_cache();
        let mut changed_detector = CacheCycleDetector::new(&changed_cache, 4.0, 4.0);
        changed_detector.power = u64::MAX;
        // Protecting the MRU entry makes the only state difference occur at the final item.
        changed_cache.set_protected_blocks([BlockId::new(ENTRIES)]);
        let mismatch_started = Instant::now();
        for _ in 0..ITERATIONS {
            black_box(changed_detector.observe(&changed_cache));
        }
        mismatch_ns.push(mismatch_started.elapsed().as_nanos() / u128::from(ITERATIONS));
    }
    match_ns.sort_unstable();
    mismatch_ns.sort_unstable();
    eprintln!(
        "entries={ENTRIES}, detector_init_ns={init_ns}, matching_check_ns=[{}, {}, {}], full_scan_mismatch_ns=[{}, {}, {}]",
        match_ns[0],
        match_ns[ROUNDS / 2],
        match_ns[ROUNDS - 1],
        mismatch_ns[0],
        mismatch_ns[ROUNDS / 2],
        mismatch_ns[ROUNDS - 1],
    );
}

#[test]
fn reports_cache_budget_for_bounding_external_block_imports() {
    let dataset = dataset(4096);
    assert_eq!(dataset.block_cache_budget_bytes(), 4096);
}

#[test]
fn reports_the_deepest_level_in_the_visible_cut() {
    let dataset = dataset(4096);
    assert_eq!(
        super::max_visible_level(dataset.metadata.hierarchy(), &[NodeId::new(1)]),
        Some(0)
    );
    assert_eq!(
        super::max_visible_level(dataset.metadata.hierarchy(), &[]),
        None
    );
}

#[test]
fn replacement_voxels_use_integer_cells_and_deduplicate_source_points() {
    let hierarchy = dataset(4096).metadata.hierarchy().clone();
    let mut cache = DecodedBlockCache::new(4096);
    cache.insert(PointBlock::new(
        BlockId::new(10),
        PointBatch::new(vec![
            Point::new([0.13, 0.2, 0.2], ColorRgb::new(100, 0, 0)),
            Point::new([0.20, 0.2, 0.2], ColorRgb::new(0, 100, 0)),
            Point::new([0.25, 0.2, 0.2], ColorRgb::new(0, 0, 100)),
        ]),
        Vec::new(),
    ));

    let scene = scene_for_test(&hierarchy, &[NodeId::new(1)], &cache, [0.0; 3]);
    let instances = scene.instances();
    assert_eq!(instances.len(), 2);
    assert!(
        instances
            .iter()
            .all(|instance| instance.size < 0.125 && instance.size > 0.124)
    );
    let mut centers: Vec<_> = instances
        .iter()
        .map(|instance| instance.center[0])
        .collect();
    centers.sort_by(f32::total_cmp);
    assert_eq!(centers, [0.1875, 0.3125]);
    let half_size = f32::from_bits(instances[0].size.to_bits()) / 2.0;
    assert!(centers[0] + half_size <= centers[1] - half_size);
}

#[test]
fn voxel_scene_converts_world_bounds_back_to_dataset_local_positions() {
    let origin = [100.0, 200.0, 300.0];
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            SpatialNode::new(
                NodeId::new(1),
                None,
                0,
                SpatialBounds::new(origin, [101.0, 201.0, 301.0]).unwrap(),
                1,
                0.0,
                vec![BlockId::new(10)],
            )
            .unwrap(),
        ],
        Vec::new(),
        Some(1),
        HierarchySemantics::Replacement,
    )
    .unwrap();
    let mut cache = DecodedBlockCache::new(4096);
    cache.insert(PointBlock::new(
        BlockId::new(10),
        PointBatch::new(vec![Point::new([0.5; 3], ColorRgb::new(20, 30, 40))]),
        Vec::new(),
    ));

    let scene = scene_for_test(&hierarchy, &[NodeId::new(1)], &cache, origin);

    assert_eq!(scene.instance_count(), 1);
    assert_eq!(scene.instances()[0].center, [0.5625; 3]);
    assert!(scene.instances()[0].center.iter().all(|value| *value < 1.0));
}

#[test]
fn additive_ancestors_and_descendants_share_one_integer_grid() {
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            SpatialNode::new(
                NodeId::new(1),
                None,
                0,
                SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                1,
                0.0,
                vec![BlockId::new(10)],
            )
            .unwrap(),
            SpatialNode::new(
                NodeId::new(2),
                Some(NodeId::new(1)),
                1,
                SpatialBounds::new([0.0; 3], [0.5; 3]).unwrap(),
                1,
                0.0,
                vec![BlockId::new(11)],
            )
            .unwrap(),
        ],
        Vec::new(),
        Some(2),
        HierarchySemantics::Additive,
    )
    .unwrap();
    let mut cache = DecodedBlockCache::new(4096);
    for id in [10, 11] {
        cache.insert(PointBlock::new(
            BlockId::new(id),
            PointBatch::new(vec![Point::new([0.2; 3], ColorRgb::new(20, 30, 40))]),
            Vec::new(),
        ));
    }

    let scene = scene_for_test(
        &hierarchy,
        &[NodeId::new(1), NodeId::new(2)],
        &cache,
        [0.0; 3],
    );
    assert_eq!(scene.instance_count(), 1);
    assert!(scene.instances()[0].size < 0.125);
}
#[test]
fn replacement_parent_keeps_cell_extent_until_children_are_available() {
    let make = |id, parent: Option<u64>, level, side, block| {
        SpatialNode::new(
            NodeId::new(id),
            parent.map(NodeId::new),
            level,
            SpatialBounds::new([0.0; 3], [side; 3]).unwrap(),
            1,
            0.0,
            vec![BlockId::new(block)],
        )
        .unwrap()
    };
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![make(1, None, 0, 8.0, 10), make(2, Some(1), 1, 4.0, 11)],
        vec![],
        None,
        HierarchySemantics::Replacement,
    )
    .unwrap();
    let mut cache = DecodedBlockCache::new(4096);
    cache.insert(PointBlock::new(
        BlockId::new(10),
        PointBatch::new(vec![Point::new([0.5; 3], ColorRgb::new(1, 2, 3))]),
        vec![],
    ));
    let project = |bounds| bounds_side(bounds) * 32.0;
    for target in [16.0, 4.0, 1.0] {
        let parent = build_voxel_scene(
            &hierarchy,
            &[NodeId::new(1)],
            &cache,
            [0.0; 3],
            target,
            &project,
        );
        assert!((parent.scene.instances()[0].size - 1.0).abs() < 1e-5);
    }
    cache.insert(PointBlock::new(
        BlockId::new(11),
        PointBatch::new(vec![
            Point::new([0.25; 3], ColorRgb::new(1, 2, 3)),
            Point::new([0.75; 3], ColorRgb::new(1, 2, 3)),
        ]),
        vec![],
    ));
    let children = build_voxel_scene(
        &hierarchy,
        &[NodeId::new(2)],
        &cache,
        [0.0; 3],
        16.0,
        &project,
    );
    assert_eq!(children.scene.instance_count(), 2);
    assert!(
        children
            .scene
            .instances()
            .iter()
            .all(|cell| (cell.size - 0.5).abs() < 1e-5)
    );
}

#[test]
fn source_leaf_depth_does_not_determine_voxel_size_or_limit_granularity() {
    let make = |id, parent: Option<u64>, level, min, max, blocks: Vec<u64>| {
        SpatialNode::new(
            NodeId::new(id),
            parent.map(NodeId::new),
            level,
            SpatialBounds::new(min, max).unwrap(),
            if blocks.is_empty() { 0 } else { 1 },
            0.0,
            blocks.into_iter().map(BlockId::new).collect(),
        )
        .unwrap()
    };
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            make(1, None, 0, [0.0; 3], [2.0; 3], vec![]),
            make(2, Some(1), 1, [0.0; 3], [1.0; 3], vec![10]),
            make(3, Some(1), 1, [1.0, 0.0, 0.0], [2.0, 1.0, 1.0], vec![]),
            make(4, Some(3), 2, [1.0, 0.0, 0.0], [1.5, 0.5, 0.5], vec![11]),
        ],
        vec![],
        Some(2),
        HierarchySemantics::Replacement,
    )
    .unwrap();
    let mut cache = DecodedBlockCache::new(4096);
    for (id, point) in [(10, [0.2, 0.2, 0.5]), (11, [1.2, 0.2, 0.5])] {
        cache.insert(PointBlock::new(
            BlockId::new(id),
            PointBatch::new(vec![Point::new(point, ColorRgb::new(1, 2, 3))]),
            vec![],
        ));
    }
    let result = build_voxel_scene(
        &hierarchy,
        &[NodeId::new(2), NodeId::new(4)],
        &cache,
        [0.0; 3],
        1.0,
        &|bounds| bounds_side(bounds) * 32.0,
    );
    assert_eq!(result.scene.instance_count(), 2);
    assert_eq!(
        result.scene.instances()[0].size,
        result.scene.instances()[1].size
    );
    assert!(result.scene.instances()[0].size < 1.0 / 32.0);
    assert_eq!(result.oversized_voxels, 0);
    assert!((result.max_edge_pixels - 1.0).abs() < 1e-10);
}

#[test]
fn adaptive_cells_obey_pixel_target_grow_with_distance_and_never_overlap() {
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            SpatialNode::new(
                NodeId::new(1),
                None,
                0,
                SpatialBounds::new([0.0; 3], [2.0; 3]).unwrap(),
                256,
                0.0,
                vec![BlockId::new(10)],
            )
            .unwrap(),
        ],
        vec![],
        Some(256),
        HierarchySemantics::Replacement,
    )
    .unwrap();
    let points = (0..16)
        .flat_map(|x| {
            (0..16).map(move |z| {
                Point::new(
                    [x as f32 / 8.0, 0.25, z as f32 / 8.0],
                    ColorRgb::new(10, 20, 30),
                )
            })
        })
        .collect();
    let mut cache = DecodedBlockCache::new(65536);
    cache.insert(PointBlock::new(
        BlockId::new(10),
        PointBatch::new(points),
        vec![],
    ));
    let project = |bounds: SpatialBounds| bounds_side(bounds) * 16.0 / (1.0 + bounds.min()[2]);
    for target in [1.0, 2.0, 4.0, 16.0] {
        let result = build_voxel_scene(
            &hierarchy,
            &[NodeId::new(1)],
            &cache,
            [0.0; 3],
            target,
            &project,
        );
        assert_eq!(result.oversized_voxels, 0);
        assert!(result.max_edge_pixels <= target + 1e-10);
        assert!(result.min_edge_pixels >= target / 2.0 - 1e-10);
        let instances = result.scene.instances();
        let near = instances
            .iter()
            .filter(|v| v.center[2] < 0.25)
            .map(|v| v.size)
            .fold(f32::INFINITY, f32::min);
        let far = instances
            .iter()
            .filter(|v| v.center[2] > 1.5)
            .map(|v| v.size)
            .fold(0.0_f32, f32::max);
        if target <= 4.0 {
            assert!(far > near);
        }
        for (i, a) in instances.iter().enumerate() {
            for b in &instances[i + 1..] {
                let overlap = (0..3).all(|axis| {
                    (f64::from(a.center[axis]) - f64::from(b.center[axis])).abs()
                        < (f64::from(a.size) + f64::from(b.size)) * 0.5 - 1e-8
                });
                assert!(!overlap, "adaptive cells must not overlap");
            }
        }
    }
}

#[test]
fn fine_local_cells_survive_a_large_geographic_origin() {
    let origin = [1.0e12; 3];
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            SpatialNode::new(
                NodeId::new(1),
                None,
                0,
                SpatialBounds::new(origin, origin.map(|value| value + 1.0)).unwrap(),
                2,
                0.0,
                vec![BlockId::new(10)],
            )
            .unwrap(),
        ],
        vec![],
        Some(2),
        HierarchySemantics::Replacement,
    )
    .unwrap();
    let mut cache = DecodedBlockCache::new(4096);
    cache.insert(PointBlock::new(
        BlockId::new(10),
        PointBatch::new(vec![
            Point::new([0.5; 3], ColorRgb::new(1, 2, 3)),
            Point::new([0.500002, 0.5, 0.5], ColorRgb::new(1, 2, 3)),
        ]),
        vec![],
    ));
    let result = build_voxel_scene(
        &hierarchy,
        &[NodeId::new(1)],
        &cache,
        origin,
        1.0,
        &|bounds| bounds_side(bounds) * 1_048_576.0,
    );
    assert_eq!(result.scene.instance_count(), 2);
    assert_eq!(result.oversized_voxels, 0);
    assert_ne!(
        result.scene.instances()[0].center,
        result.scene.instances()[1].center
    );
    assert!(
        result
            .scene
            .instances()
            .iter()
            .all(|cell| cell.size < 1.0e-6)
    );
}

fn multi_block_dataset(block_ids: &[u64], cache_blocks: usize) -> LodDataset {
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            SpatialNode::new(
                NodeId::new(1),
                None,
                0,
                SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                1,
                0.0,
                block_ids.iter().copied().map(BlockId::new).collect(),
            )
            .unwrap(),
        ],
        Vec::new(),
        Some(1),
        HierarchySemantics::Replacement,
    )
    .unwrap();
    LodDataset::new(
        [0.0; 3],
        hierarchy,
        cache_blocks * block(10).estimated_memory_bytes(),
    )
    .unwrap()
}

/// Mirrors the load accounting of `update_view_progressive` for one arrival: the async path
/// inserts through `insert_block` and the next update drains the stash; the sync path loads
/// inside the update. Both end in the same `observe_cache_cycle` call.
fn load_step(
    dataset: &mut LodDataset,
    id: u64,
    target: &[u64],
    async_arrival: bool,
) -> Result<(), LodDatasetError> {
    let (loaded, evicted) = if async_arrival {
        dataset.insert_block(block(id)).unwrap();
        (
            std::mem::take(&mut dataset.arrived_blocks) > 0,
            std::mem::take(&mut dataset.arrival_evictions),
        )
    } else {
        let result = dataset.cache_block(block(id)).unwrap();
        (true, result.evicted_blocks().to_vec())
    };
    let evicted_pending = evicted
        .iter()
        .filter(|evicted| target.contains(&evicted.get()))
        .count() as u64;
    dataset.refinement_work.evicted_pending_blocks += evicted_pending;
    dataset.observe_cache_cycle(loaded, evicted_pending, true, false, 4.0, 4.0)
}

fn run_loads(dataset: &mut LodDataset, ids: &[u64], target: &[u64], async_arrival: bool) -> usize {
    for (step, id) in ids.iter().enumerate() {
        if load_step(dataset, *id, target, async_arrival).is_err() {
            return step + 1;
        }
    }
    0
}

#[test]
fn async_thrash_evicting_the_target_cut_pauses_refinement_like_the_sync_path() {
    let target = [10, 11, 12];
    let arrivals: Vec<u64> = target.iter().copied().cycle().take(60).collect();
    let mut sync = multi_block_dataset(&target, 2);
    let mut asynchronous = multi_block_dataset(&target, 2);
    let sync_stop = run_loads(&mut sync, &arrivals, &target, false);
    let async_stop = run_loads(&mut asynchronous, &arrivals, &target, true);
    assert!(sync_stop > 0, "sync path must detect the cycle");
    assert_eq!(async_stop, sync_stop);
    for dataset in [&sync, &asynchronous] {
        assert!(dataset.cache_cycle.as_ref().unwrap().paused);
        assert!(dataset.cache_cycle_detected());
        assert_eq!(dataset.refinement_work.cache_cycle_detections, 1);
    }
    assert_eq!(
        asynchronous.pending_target_evictions(),
        sync.pending_target_evictions()
    );
    assert!(asynchronous.pending_target_evictions() > 1);
    assert!(
        asynchronous
            .cache
            .lru_entries()
            .eq(sync.cache.lru_entries())
    );
}

#[test]
fn async_load_without_thrash_or_with_churn_outside_the_target_never_fires() {
    let target = [10, 11, 12];
    // Cut fits the budget: no eviction of target blocks at all.
    let mut fits = multi_block_dataset(&[10, 11, 12], 3);
    assert_eq!(run_loads(&mut fits, &[10, 11, 12], &target, true), 0);
    assert_eq!(fits.pending_target_evictions(), 0);
    assert!(!fits.cache_cycle_detected());
    // LRU churn on blocks outside the target cut is not a target eviction.
    let mut churn = multi_block_dataset(&[10, 11, 12, 20, 21, 22], 3);
    let arrivals = [20, 21, 22, 20, 21, 22, 20, 21, 22, 10, 11, 12];
    assert_eq!(run_loads(&mut churn, &arrivals, &target, true), 0);
    assert_eq!(churn.pending_target_evictions(), 0);
    assert!(churn.cache_cycle.is_none());
    assert!(!churn.cache_cycle_detected());
}

#[test]
fn a_new_target_resets_the_async_detector() {
    let target = [10, 11, 12];
    let mut dataset = multi_block_dataset(&target, 2);
    assert_eq!(run_loads(&mut dataset, &[10, 11, 12], &target, true), 0);
    assert!(dataset.cache_cycle.is_some());
    // An update whose cut is not reusable (new target/camera) drops the detector.
    dataset
        .observe_cache_cycle(false, 0, false, false, 4.0, 4.0)
        .unwrap();
    assert!(dataset.cache_cycle.is_none());
}

#[test]
fn cut_retention_still_protects_target_blocks_from_async_arrivals() {
    let mut dataset = multi_block_dataset(&[10, 11, 12], 2);
    dataset.insert_block(block(10)).unwrap();
    dataset.cache.set_protected_blocks([BlockId::new(10)]);
    dataset.insert_block(block(11)).unwrap();
    dataset.insert_block(block(12)).unwrap();
    assert!(dataset.cache.contains(BlockId::new(10)));
    assert!(!dataset.arrival_evictions.contains(&BlockId::new(10)));
}

// ---------------------------------------------------------------------------------------------
// Characterization of the per-update decision path (selection -> plan -> load -> plan -> rebuild
// decision) across several views and block arrivals. `SurfaceRenderer` needs a GPU surface, so the
// renderer-free pieces are driven here with a synthetic frustum and projector.
// ---------------------------------------------------------------------------------------------

/// Three levels, binary: 1 -> {2, 3} -> {4..7}; node `k` owns block `10 + k`.
fn tree_dataset(cache_blocks: usize) -> LodDataset {
    let node = |id: u64, parent: Option<u64>, level: u32, x: [f64; 2], error: f64| {
        SpatialNode::new(
            NodeId::new(id),
            parent.map(NodeId::new),
            level,
            SpatialBounds::new([x[0], 0.0, 0.0], [x[1], 1.0, 1.0]).unwrap(),
            1,
            error,
            vec![BlockId::new(10 + id)],
        )
        .unwrap()
    };
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            node(1, None, 0, [0.0, 1.0], 8.0),
            node(2, Some(1), 1, [0.0, 0.5], 4.0),
            node(3, Some(1), 1, [0.5, 1.0], 4.0),
            node(4, Some(2), 2, [0.0, 0.25], 2.0),
            node(5, Some(2), 2, [0.25, 0.5], 2.0),
            node(6, Some(3), 2, [0.5, 0.75], 2.0),
            node(7, Some(3), 2, [0.75, 1.0], 2.0),
        ],
        Vec::new(),
        Some(1),
        HierarchySemantics::Replacement,
    )
    .unwrap();
    LodDataset::new(
        [0.0; 3],
        hierarchy,
        cache_blocks * block(10).estimated_memory_bytes(),
    )
    .unwrap()
}

struct SimView {
    id: f64,
    frustum: SpatialFrustum,
    scale: f64,
}

fn sim_views() -> Vec<SimView> {
    let half_space = |normal: [f64; 3], offset: f64| {
        SpatialFrustum::new(vec![FrustumPlane::new(normal, offset).unwrap()]).unwrap()
    };
    let view = |id: f64, normal: [f64; 3], offset: f64, scale: f64| SimView {
        id,
        frustum: half_space(normal, offset),
        scale,
    };
    vec![
        view(0.0, [1.0, 0.0, 0.0], 1.0, 2.0),
        view(1.0, [1.0, 0.0, 0.0], 1.0, 8.0),
        view(2.0, [1.0, 0.0, 0.0], -0.5, 8.0),
        view(3.0, [-1.0, 0.0, 0.0], 0.5, 3.0),
        // Back to the first view: the visible cut and build context are reusable again.
        view(0.0, [1.0, 0.0, 0.0], 1.0, 2.0),
    ]
}

fn pick_target(dataset: &mut LodDataset, view: &SimView, stage: f64) -> Arc<LodSelection> {
    let context = build_context(view.id, [100, 100], stage);
    let scale = view.scale;
    dataset
        .resolve_target(
            &context.camera,
            context.viewport,
            stage,
            &view.frustum,
            move |node| node.geometric_error() * scale,
        )
        .unwrap()
}

fn ids<T: Copy>(values: &[T], get: impl Fn(T) -> u64) -> String {
    format!("{:?}", values.iter().map(|v| get(*v)).collect::<Vec<_>>())
}

/// One stage evaluation, mirroring `update_view_progressive` without the renderer.
fn sim_update(
    dataset: &mut LodDataset,
    view: &SimView,
    stage: f64,
    async_loads: bool,
    log: &mut Vec<String>,
) -> bool {
    let context = build_context(view.id, [100, 100], stage);
    let target = pick_target(dataset, view, stage);
    let initial = dataset.resolve_plan(&target);
    let initial_epoch = dataset.state_epoch;
    let next = initial.pending_blocks().first().copied();
    dataset.arrived_blocks = 0;
    if let (Some(id), false) = (next, async_loads) {
        dataset.cache_block(block(id.get())).unwrap();
    }
    let plan = dataset.replan_after_load(initial, initial_epoch, &target);
    let mut decision = "skip";
    if !plan.node_ids().is_empty() || plan.pending_blocks().is_empty() {
        let (same_cut, reusable) = reusable_build_match(
            dataset
                .visible_plan
                .as_ref()
                .map(LodTransitionPlan::node_ids),
            dataset
                .visible_plan
                .as_ref()
                .map(LodTransitionPlan::block_ids),
            plan.node_ids(),
            plan.block_ids(),
            dataset.last_successful_build.as_ref(),
            &context,
        );
        decision = match (reusable, same_cut) {
            (true, _) => "reuse",
            (false, true) => "rebuild-same-cut",
            (false, false) => "build",
        };
        dataset.planner.commit(&plan, &mut dataset.cache);
        dataset.visible_plan = Some(plan.clone());
        dataset.last_successful_build = Some(context);
    }
    log.push(format!(
        "stage={stage} cut={} pending={} uncovered={} {decision}",
        ids(plan.node_ids(), |n| n.get()),
        ids(plan.pending_blocks(), |b| b.get()),
        plan.uncovered_nodes().len(),
    ));
    if async_loads && let Some(id) = plan.pending_blocks().first() {
        dataset.insert_block(block(id.get())).unwrap();
    }
    !plan.pending_blocks().is_empty()
}

/// Mirrors `update_lod_view`: fine-stage residency preflight, then the staged refinement loop.
fn simulate_session(async_loads: bool) -> Vec<String> {
    let target_px = 1.0;
    let mut dataset = tree_dataset(5);
    let mut log = Vec::new();
    for (index, view) in sim_views().iter().enumerate() {
        let mut stage: Option<f64> = None;
        for _ in 0..12 {
            log.push(format!("view {index}"));
            let mut used = stage.unwrap_or(16.0);
            let mut pending = None;
            if stage.is_none() && target_px < 16.0 {
                let target = pick_target(&mut dataset, view, target_px);
                let resident = dataset.preflight_resident(target);
                log.push(format!("resident={resident}"));
                if resident {
                    used = target_px;
                    pending = Some(sim_update(&mut dataset, view, used, async_loads, &mut log));
                }
            }
            let pending = pending
                .unwrap_or_else(|| sim_update(&mut dataset, view, used, async_loads, &mut log));
            let (next_stage, dirty) = next_lod_stage(Some(used), target_px, pending, true);
            stage = next_stage;
            if !dirty {
                break;
            }
        }
    }
    log
}

#[test]
fn update_decisions_are_pinned_for_synchronous_loads() {
    let log = simulate_session(false);
    println!("{log:#?}");
    assert_eq!(log, SYNC_GOLDEN);
}

#[test]
fn update_decisions_are_pinned_for_async_arrivals() {
    let log = simulate_session(true);
    println!("{log:#?}");
    assert_eq!(log, ASYNC_GOLDEN);
}

const SYNC_GOLDEN: &[&str] = &[
    "view 0",
    "resident=false",
    "stage=16 cut=[1] pending=[] uncovered=0 build",
    "view 0",
    "stage=8 cut=[1, 2] pending=[13] uncovered=0 build",
    "view 0",
    "stage=8 cut=[2, 3] pending=[] uncovered=0 build",
    "view 0",
    "stage=4 cut=[2, 3, 4] pending=[15, 16, 17] uncovered=0 build",
    "view 0",
    "stage=4 cut=[3, 4, 5] pending=[16, 17] uncovered=0 build",
    "view 0",
    "stage=4 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 0",
    "stage=4 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 0",
    "stage=4 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 0",
    "stage=4 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 0",
    "stage=4 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 0",
    "stage=4 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 0",
    "stage=4 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 1",
    "resident=false",
    "stage=16 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 2",
    "resident=false",
    "stage=16 cut=[3, 5, 7] pending=[16] uncovered=0 build",
    "view 2",
    "stage=16 cut=[5, 6, 7] pending=[] uncovered=0 build",
    "view 2",
    "stage=8 cut=[5, 6, 7] pending=[] uncovered=0 rebuild-same-cut",
    "view 2",
    "stage=4 cut=[5, 6, 7] pending=[] uncovered=0 rebuild-same-cut",
    "view 2",
    "stage=2 cut=[5, 6, 7] pending=[] uncovered=0 rebuild-same-cut",
    "view 2",
    "stage=1 cut=[5, 6, 7] pending=[] uncovered=0 rebuild-same-cut",
    "view 3",
    "resident=false",
    "stage=16 cut=[2, 3] pending=[] uncovered=0 build",
    "view 3",
    "stage=8 cut=[2, 4, 6] pending=[15] uncovered=0 build",
    "view 3",
    "stage=8 cut=[4, 5, 6] pending=[] uncovered=0 build",
    "view 3",
    "stage=4 cut=[4, 5, 6] pending=[] uncovered=0 rebuild-same-cut",
    "view 3",
    "stage=2 cut=[4, 5, 6] pending=[] uncovered=0 rebuild-same-cut",
    "view 3",
    "stage=1 cut=[4, 5, 6] pending=[] uncovered=0 rebuild-same-cut",
    "view 4",
    "resident=false",
    "stage=16 cut=[1] pending=[] uncovered=0 build",
    "view 4",
    "stage=8 cut=[1, 2] pending=[13] uncovered=0 build",
    "view 4",
    "stage=8 cut=[2, 3] pending=[] uncovered=0 build",
    "view 4",
    "stage=4 cut=[2, 3, 5, 6] pending=[14, 17] uncovered=0 build",
    "view 4",
    "stage=4 cut=[2, 3, 4, 6] pending=[15, 17] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
];
const ASYNC_GOLDEN: &[&str] = &[
    "view 0",
    "resident=false",
    "stage=16 cut=[] pending=[11] uncovered=1 skip",
    "view 0",
    "stage=16 cut=[1] pending=[] uncovered=0 build",
    "view 0",
    "stage=8 cut=[1] pending=[12, 13] uncovered=0 rebuild-same-cut",
    "view 0",
    "stage=8 cut=[1, 2] pending=[13] uncovered=0 build",
    "view 0",
    "stage=8 cut=[2, 3] pending=[] uncovered=0 build",
    "view 0",
    "stage=4 cut=[2, 3] pending=[14, 15, 16, 17] uncovered=0 rebuild-same-cut",
    "view 0",
    "stage=4 cut=[2, 3, 4] pending=[15, 16, 17] uncovered=0 build",
    "view 0",
    "stage=4 cut=[3, 4, 5] pending=[16, 17] uncovered=0 build",
    "view 0",
    "stage=4 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 0",
    "stage=4 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 0",
    "stage=4 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 0",
    "stage=4 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 1",
    "resident=false",
    "stage=16 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 1",
    "stage=16 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 2",
    "resident=false",
    "stage=16 cut=[3, 5, 6] pending=[17] uncovered=0 build",
    "view 2",
    "stage=16 cut=[5, 6, 7] pending=[] uncovered=0 build",
    "view 2",
    "stage=8 cut=[5, 6, 7] pending=[] uncovered=0 rebuild-same-cut",
    "view 2",
    "stage=4 cut=[5, 6, 7] pending=[] uncovered=0 rebuild-same-cut",
    "view 2",
    "stage=2 cut=[5, 6, 7] pending=[] uncovered=0 rebuild-same-cut",
    "view 2",
    "stage=1 cut=[5, 6, 7] pending=[] uncovered=0 rebuild-same-cut",
    "view 3",
    "resident=false",
    "stage=16 cut=[2, 3] pending=[] uncovered=0 build",
    "view 3",
    "stage=8 cut=[2, 5, 6] pending=[14] uncovered=0 build",
    "view 3",
    "stage=8 cut=[2, 4, 6] pending=[15] uncovered=0 build",
    "view 3",
    "stage=8 cut=[4, 5, 6] pending=[] uncovered=0 build",
    "view 3",
    "stage=4 cut=[4, 5, 6] pending=[] uncovered=0 rebuild-same-cut",
    "view 3",
    "stage=2 cut=[4, 5, 6] pending=[] uncovered=0 rebuild-same-cut",
    "view 3",
    "stage=1 cut=[4, 5, 6] pending=[] uncovered=0 rebuild-same-cut",
    "view 4",
    "resident=false",
    "stage=16 cut=[4, 5, 6] pending=[11] uncovered=1 rebuild-same-cut",
    "view 4",
    "stage=16 cut=[1] pending=[] uncovered=0 build",
    "view 4",
    "stage=8 cut=[1, 3] pending=[12] uncovered=0 build",
    "view 4",
    "stage=8 cut=[1, 2] pending=[13] uncovered=0 build",
    "view 4",
    "stage=8 cut=[2, 3] pending=[] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5] pending=[16, 17] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5, 6] pending=[17] uncovered=0 build",
    "view 4",
    "stage=4 cut=[3, 4, 5, 7] pending=[16] uncovered=0 build",
];

#[test]
fn a_resident_fine_update_selects_and_prepares_once() {
    let mut dataset = tree_dataset(8);
    for id in 11..=17 {
        dataset.cache_block(block(id)).unwrap();
    }
    let view = &sim_views()[1];
    let target = pick_target(&mut dataset, view, 1.0);
    assert!(dataset.preflight_resident(target));
    let mut log = Vec::new();
    sim_update(&mut dataset, view, 1.0, false, &mut log);
    // Before reuse this took 2 selections and 3 prepares (preflight, first plan, second plan).
    assert_eq!(dataset.refinement_work.selections, 1);
    assert_eq!(dataset.refinement_work.prepares, 1);
    // Same view and stage again: the selection is memoized; the commit forces one fresh plan.
    sim_update(&mut dataset, view, 1.0, false, &mut log);
    assert_eq!(dataset.refinement_work.selections, 1);
    assert_eq!(dataset.refinement_work.prepares, 2);
    // A cache change invalidates the plan but never the selection.
    dataset.cache_block(block(11)).unwrap();
    sim_update(&mut dataset, view, 1.0, false, &mut log);
    assert_eq!(dataset.refinement_work.selections, 1);
    assert_eq!(dataset.refinement_work.prepares, 3);
}

/// Three equal leaves (geometric error 0) at increasing distance from the camera. Block ids
/// ascend away from the camera, so the old tie-break by `BlockId` would load the farthest first.
#[test]
fn pending_leaf_priority_follows_the_screen_space_metric_not_block_ids() {
    let viewport = [800, 600];
    let leaf = |id: u64, x: f64| {
        SpatialNode::new(
            NodeId::new(id),
            Some(NodeId::new(1)),
            1,
            SpatialBounds::new([x, 0.0, 0.0], [x + 10.0, 10.0, 10.0]).unwrap(),
            1,
            0.0,
            vec![BlockId::new(100 + id)],
        )
        .unwrap()
    };
    let root = SpatialNode::new(
        NodeId::new(1),
        None,
        0,
        SpatialBounds::new([0.0; 3], [100.0, 10.0, 10.0]).unwrap(),
        1,
        0.0,
        vec![BlockId::new(101)],
    )
    .unwrap();
    // The camera sits on the +x side looking toward -x, so the largest x is nearest.
    let hierarchy = SpatialHierarchy::new(
        vec![root, leaf(2, 80.0), leaf(3, 40.0), leaf(4, 0.0)],
        Vec::new(),
        None,
    )
    .unwrap();
    let mut camera = OrbitCamera::default();
    // Fitting sets the near plane from the scene, so the leaves are not all clamped to it.
    assert!(camera.fit(
        SceneBounds {
            min: [0.0; 3],
            max: [100.0, 10.0, 10.0],
        },
        viewport
    ));
    assert!(camera.set_pose([350.0, 5.0, 5.0], [50.0, 5.0, 5.0]));
    let frustum = camera.spatial_frustum(viewport).unwrap();
    let project = |node: &SpatialNode| {
        camera.projected_error_pixels(
            node.bounds(),
            node_spacing(node.bounds()),
            [0.0; 3],
            viewport,
        )
    };
    let pending = [BlockId::new(104), BlockId::new(103), BlockId::new(102)];
    let ordered = prioritized_pending_blocks(&hierarchy, &pending, &frustum, project);
    let order = ordered.iter().map(|(id, _)| id.get()).collect::<Vec<_>>();
    assert_eq!(order, [102, 103, 104]);
    assert!(ordered[0].1 > ordered[1].1 && ordered[1].1 > ordered[2].1);
    assert_eq!(
        highest_priority_pending_block(&hierarchy, &pending, &frustum, project),
        Some(BlockId::new(102))
    );
}
