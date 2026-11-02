use viewer_core::{
    BlockId, ColorRgb, DecodedBlockCache, FrustumPlane, HierarchySemantics, LodSelection,
    LodSelectionConfig, LodTransitionPlan, LodTransitionPlanner, NodeId, Point, PointBatch,
    PointBlock, SpatialBounds, SpatialFrustum, SpatialHierarchy, SpatialNode,
};

use std::sync::Arc;

use super::{
    CacheCycleDetector, LodDataset, LodDatasetError, SuccessfulBuildContext, bounds_side,
    highest_priority_pending_block, index_block_owners, load_priority, next_lod_stage,
    prefix_extension_points, prioritized_pending_blocks, priority_precedes,
    resident_target_plan_is_complete, reusable_build_match,
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

#[test]
fn pending_priority_projects_only_visible_pending_owners() {
    let bounds = |x| SpatialBounds::new([x, 0.0, 0.0], [x + 0.5, 0.5, 0.5]).unwrap();
    let mut nodes = vec![
        SpatialNode::new(
            NodeId::new(1),
            None,
            0,
            SpatialBounds::new([-100.0; 3], [100.0; 3]).unwrap(),
            1,
            0.0,
            vec![BlockId::new(1)],
        )
        .unwrap(),
    ];
    for id in 2..10_006 {
        let blocks = match id {
            2 => vec![10, 11, 14],
            3 => vec![12, 13, 14],
            4 => vec![10], // A higher-scoring owner outside the frustum.
            _ => vec![id + 100],
        };
        nodes.push(
            SpatialNode::new(
                NodeId::new(id),
                Some(NodeId::new(1)),
                1,
                bounds(if id == 4 { -2.0 } else { 1.0 }),
                1,
                0.0,
                blocks.into_iter().map(BlockId::new).collect(),
            )
            .unwrap(),
        );
    }
    let hierarchy = SpatialHierarchy::new(nodes, Vec::new(), None).unwrap();
    let frustum =
        SpatialFrustum::new(vec![FrustumPlane::new([1.0, 0.0, 0.0], 0.0).unwrap()]).unwrap();
    let pending = [13, 10, 99_999, 14, 12, 11, 10].map(BlockId::new);
    let uncovered = [BlockId::new(11)].into_iter().collect();
    let owners = index_block_owners(&hierarchy);
    let calls = std::cell::Cell::new(0);
    let project = |node: &SpatialNode| {
        calls.set(calls.get() + 1);
        node.id().get() as f64
    };
    assert!(
        prioritized_pending_blocks(&hierarchy, &owners, &[], &uncovered, &frustum, project)
            .is_empty()
    );
    assert_eq!(calls.get(), 0);
    assert_eq!(
        highest_priority_pending_block(&hierarchy, &owners, &[], &uncovered, &frustum, project),
        None
    );
    assert_eq!(calls.get(), 0);
    let ordered =
        prioritized_pending_blocks(&hierarchy, &owners, &pending, &uncovered, &frustum, project);
    assert_eq!(
        ordered,
        vec![
            (BlockId::new(11), 2.0),
            (BlockId::new(12), 3.0),
            (BlockId::new(13), 3.0),
            (BlockId::new(14), 3.0),
            (BlockId::new(10), 2.0)
        ]
    );
    assert_eq!(calls.get(), 2);
    let mut old_scores = std::collections::HashMap::<BlockId, f64>::new();
    calls.set(0);
    for node in hierarchy.query_frustum(frustum.clone()) {
        let score = load_priority(project(node), node);
        for block in node.blocks().iter().filter(|block| pending.contains(block)) {
            old_scores
                .entry(*block)
                .and_modify(|current| {
                    if score.total_cmp(current).is_gt() {
                        *current = score;
                    }
                })
                .or_insert(score);
        }
    }
    assert_eq!(calls.get(), 10_004);
    assert_eq!(old_scores.len(), ordered.len());
    for (block, score) in &ordered {
        assert_eq!(old_scores.get(block), Some(score));
    }
    calls.set(0);
    assert_eq!(
        highest_priority_pending_block(
            &hierarchy, &owners, &pending, &uncovered, &frustum, project
        ),
        Some(ordered[0].0)
    );
    assert_eq!(calls.get(), 2);
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
            node.representative_spacing(),
            [0.0; 3],
            viewport,
        )
    };
    let pending = [BlockId::new(104), BlockId::new(103), BlockId::new(102)];
    let owners = index_block_owners(&hierarchy);
    let ordered = prioritized_pending_blocks(
        &hierarchy,
        &owners,
        &pending,
        &std::collections::HashSet::new(),
        &frustum,
        project,
    );
    let order = ordered.iter().map(|(id, _)| id.get()).collect::<Vec<_>>();
    assert_eq!(order, [102, 103, 104]);
    assert!(ordered[0].1 > ordered[1].1 && ordered[1].1 > ordered[2].1);
    assert_eq!(
        highest_priority_pending_block(
            &hierarchy,
            &owners,
            &pending,
            &std::collections::HashSet::new(),
            &frustum,
            project
        ),
        Some(BlockId::new(102))
    );
}

#[test]
fn uncovered_multiblock_node_precedes_covered_refinement_in_both_load_modes() {
    let bounds = |x: f64| SpatialBounds::new([x, 0.0, 0.0], [x + 20.0, 20.0, 20.0]).unwrap();
    let node = |id, parent: Option<u64>, level, x, blocks: Vec<u64>| {
        SpatialNode::new(
            NodeId::new(id),
            parent.map(NodeId::new),
            level,
            bounds(x),
            1,
            0.0,
            blocks.into_iter().map(BlockId::new).collect(),
        )
        .unwrap()
    };
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            SpatialNode::new(
                NodeId::new(1),
                None,
                0,
                SpatialBounds::new([0.0; 3], [100.0, 20.0, 20.0]).unwrap(),
                1,
                0.0,
                vec![BlockId::new(10)],
            )
            .unwrap(),
            node(2, Some(1), 1, 80.0, vec![11]),
            node(4, Some(2), 2, 80.0, vec![12]),
            node(3, Some(1), 1, 0.0, vec![13, 14]),
        ],
        Vec::new(),
        None,
        HierarchySemantics::Replacement,
    )
    .unwrap();
    let mut camera = OrbitCamera::default();
    let viewport = [800, 600];
    assert!(camera.fit(
        SceneBounds {
            min: [0.0; 3],
            max: [100.0, 20.0, 20.0]
        },
        viewport
    ));
    assert!(camera.set_pose([350.0, 10.0, 10.0], [50.0, 10.0, 10.0]));
    let view = viewer_render_wgpu::ViewProjection::new(camera, viewport, 1.0, Default::default());
    let make_dataset = || {
        let mut dataset = LodDataset::new([0.0; 3], hierarchy.clone(), usize::MAX).unwrap();
        dataset.insert_block(block(11)).unwrap();
        dataset
    };
    let mut async_dataset = make_dataset();
    let mut order = Vec::new();
    let update = async_dataset
        .update_view_progressive(&view, 4.0, 4.0, 4.0, Some(&mut order), |_| Ok(None))
        .unwrap();
    assert_eq!(update.stats.uncovered_nodes, 1);
    assert_eq!(
        order,
        [BlockId::new(13), BlockId::new(14), BlockId::new(12)]
    );
    let mut sync_dataset = make_dataset();
    let mut requested = None;
    sync_dataset
        .update_view_progressive(&view, 4.0, 4.0, 4.0, None, |id| {
            requested = Some(id);
            Ok(None)
        })
        .unwrap();
    assert_eq!(requested, Some(BlockId::new(13)));
}

/// Same bounds and camera for every node, so only the point count and the error argument vary.
#[test]
fn pending_priority_is_screen_space_error_per_source_point() {
    let node = |id: u64, points: u64| {
        SpatialNode::new(
            NodeId::new(id),
            None,
            0,
            SpatialBounds::new([0.0; 3], [10.0; 3]).unwrap(),
            points,
            0.0,
            vec![BlockId::new(id)],
        )
        .unwrap()
    };
    let (sparse, dense, empty) = (node(1, 1_000), node(2, 100_000), node(3, 0));
    // Equal error: fewer points first.
    assert!(load_priority(8.0, &sparse) > load_priority(8.0, &dense));
    // 8 px / 1_000 = 0.008 versus 64 px / 100_000 = 0.00064 versus 800 px / 100_000 = 0.008.
    assert!(load_priority(64.0, &dense) < load_priority(8.0, &sparse));
    assert_eq!(load_priority(800.0, &dense), load_priority(8.0, &sparse));
    // A much larger error outweighs 100x more points.
    assert!(load_priority(1_600.0, &dense) > load_priority(8.0, &sparse));
    // A zero count is treated as one.
    assert_eq!(load_priority(8.0, &empty), 8.0);
}

// ---------------------------------------------------------------------------------------------
// Native end-to-end update: no GPU, so the whole select -> load -> build path runs in a test.
// ---------------------------------------------------------------------------------------------

/// Root (block 11) plus two children split on x (blocks 12, 13); `count` points per block.
fn native_dataset(count: usize) -> (LodDataset, Vec<PointBlock>) {
    let node = |id: u64, parent: Option<u64>, level: u32, x: [f64; 2]| {
        SpatialNode::new(
            NodeId::new(id),
            parent.map(NodeId::new),
            level,
            SpatialBounds::new([x[0], 0.0, 0.0], [x[1], 64.0, 64.0]).unwrap(),
            count as u64,
            0.0,
            vec![BlockId::new(10 + id)],
        )
        .unwrap()
    };
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            node(1, None, 0, [0.0, 64.0]),
            node(2, Some(1), 1, [0.0, 32.0]),
            node(3, Some(1), 1, [32.0, 64.0]),
        ],
        Vec::new(),
        Some(2 * count as u64),
        HierarchySemantics::Replacement,
    )
    .unwrap();
    let mut state = 0x5eed_u64;
    let mut unit = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 40) as f32) / ((1_u64 << 24) as f32)
    };
    let mut points_in = |x0: f32, width: f32| -> Vec<Point> {
        (0..count)
            .map(|_| {
                Point::new(
                    [x0 + unit() * width, unit() * 64.0, unit() * 64.0],
                    ColorRgb::new(10, 120, 200),
                )
            })
            .collect()
    };
    let blocks = vec![
        PointBlock::new(
            BlockId::new(11),
            PointBatch::new(points_in(0.0, 64.0)),
            vec![],
        ),
        PointBlock::new(
            BlockId::new(12),
            PointBatch::new(points_in(0.0, 32.0)),
            vec![],
        ),
        PointBlock::new(
            BlockId::new(13),
            PointBatch::new(points_in(32.0, 32.0)),
            vec![],
        ),
    ];
    let cache_bytes = 8 * blocks[0].estimated_memory_bytes();
    (
        LodDataset::new([0.0; 3], hierarchy, cache_bytes).unwrap(),
        blocks,
    )
}

fn native_view() -> viewer_render_wgpu::ViewProjection {
    let viewport = [800, 600];
    let mut camera = OrbitCamera::default();
    assert!(camera.fit(
        SceneBounds {
            min: [0.0; 3],
            max: [64.0; 3],
        },
        viewport,
    ));
    viewer_render_wgpu::ViewProjection::new(
        camera,
        viewport,
        1.0,
        viewer_render_wgpu::VoxelBuildLimits::default(),
    )
}

#[test]
fn a_native_update_builds_the_scene_without_a_renderer() {
    let view = native_view();
    let (mut dataset, blocks) = native_dataset(400);
    for block in &blocks {
        dataset.insert_block(block.clone()).unwrap();
    }

    let first = dataset
        .update_view_progressive(&view, 4.0, 4.0, 4.0, None, |_| Ok(None))
        .unwrap();
    let built = first.scene.expect("first update builds a scene");
    assert!(first.stats.pending_block_ids.is_empty());
    assert_eq!(
        first.stats.rendered_voxels,
        built.voxels.scene.instance_count()
    );
    assert_eq!(first.stats.refinement_work.scene_builds, 1);
    assert_eq!(built.block_ids, [BlockId::new(12), BlockId::new(13)]);
    assert_eq!(
        (built.voxels.scene.instance_count(), built.scene_digest()),
        NATIVE_UPDATE_GOLDEN
    );

    // Same view and cut: the scene is reused, so nothing new is handed to the caller.
    let again = dataset
        .update_view_progressive(&view, 4.0, 4.0, 4.0, None, |_| Ok(None))
        .unwrap();
    assert!(again.scene.is_none());
    assert_eq!(again.stats.rendered_voxels, first.stats.rendered_voxels);
    assert_eq!(again.stats.refinement_work.reused_scene_updates, 1);
}

#[test]
fn a_native_update_loading_blocks_on_demand_reaches_the_same_digest() {
    let view = native_view();
    let (mut dataset, blocks) = native_dataset(400);
    let mut digest = None;
    for _ in 0..8 {
        let update = dataset
            .update_view_progressive(&view, 4.0, 4.0, 4.0, None, |id| {
                Ok(blocks.iter().find(|block| block.id() == id).cloned())
            })
            .unwrap();
        if update.stats.pending_block_ids.is_empty() {
            digest = update.scene.map(|scene| scene.scene_digest());
            break;
        }
    }
    assert_eq!(digest, Some(NATIVE_UPDATE_GOLDEN.1));
}

#[test]
fn a_discarded_scene_is_rebuilt_by_the_next_update() {
    let view = native_view();
    let (mut dataset, blocks) = native_dataset(400);
    for block in &blocks {
        dataset.insert_block(block.clone()).unwrap();
    }
    let first = dataset
        .update_view_progressive(&view, 4.0, 4.0, 4.0, None, |_| Ok(None))
        .unwrap();
    dataset.discard_visible_scene();
    let rebuilt = dataset
        .update_view_progressive(&view, 4.0, 4.0, 4.0, None, |_| Ok(None))
        .unwrap();
    assert_eq!(
        rebuilt.scene.map(|scene| scene.scene_digest()),
        first.scene.map(|scene| scene.scene_digest())
    );
}

const NATIVE_UPDATE_GOLDEN: (usize, u64) = (800, 7_206_320_447_428_042_895);

// ---------------------------------------------------------------------------------------------
// Binding parity: `WasmVoxelViewer` and `WasmSceneBuilder` must publish identical scenes.
// ---------------------------------------------------------------------------------------------

mod binding_parity {
    use super::*;
    use crate::{
        LodBuiltScene,
        lod_driver::{
            AttributeAvailability, ColorSelection, LodStep, SceneSink, assemble_async_block,
            resolve_color_selection, update_lod_view,
        },
        lod_session::LodSession,
        scene_builder::{SceneBuilderCore, camera_from_transport},
    };
    use viewer_core::{AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues};
    use viewer_render_wgpu::{ColorMode, ViewProjection, VoxelBuildLimits};

    /// What a consumer of one update observes.
    #[derive(Debug, PartialEq)]
    struct Observed {
        digest: Option<u64>,
        instance_count: Option<usize>,
        block_ids: Option<Vec<BlockId>>,
        rendered_voxels: usize,
        stage: f64,
        pending: Vec<BlockId>,
        async_order: Vec<BlockId>,
        needs_refinement: bool,
    }

    enum Op {
        Insert(u64),
        EvictAll,
        /// Inserts a block with a `temperature` column, painted when an attribute is selected.
        InsertAttributed(u64),
        Update {
            camera: usize,
            target: f64,
        },
        Color(&'static str),
        /// Selects `attribute:0:0` of `attribute_schema()` with the range the dataset reports.
        Attribute,
    }

    const ATTRIBUTE_RANGE: [f64; 2] = [0.0, 1.0];

    fn attribute_schema() -> Vec<AttributeDescriptor> {
        vec![AttributeDescriptor::new("temperature", AttributeDataType::F32, 1).unwrap()]
    }

    /// One value per point, spread across (and slightly beyond) the dataset range.
    fn temperature_column(point_count: usize) -> AttributeColumn {
        let values = (0..point_count)
            .map(|row| row as f32 / point_count as f32 * 1.2 - 0.1)
            .collect();
        AttributeColumn::new(
            attribute_schema().remove(0),
            AttributeValues::F32(values),
            point_count,
        )
        .unwrap()
    }

    /// The block a host would cache for the current colour selection.
    fn attributed_block(
        source: &PointBlock,
        selected: Option<(u32, u32)>,
        range: Option<[f64; 2]>,
    ) -> PointBlock {
        let points = source.points().clone();
        let column = temperature_column(points.len());
        assemble_async_block(
            source.id(),
            points,
            Some(column),
            &attribute_schema(),
            selected,
            range,
            usize::MAX,
        )
        .unwrap()
    }

    fn update(camera: usize, target: f64) -> Op {
        Op::Update { camera, target }
    }

    fn cameras() -> Vec<OrbitCamera> {
        let base = native_view().camera();
        let mut orbited = base;
        assert!(orbited.orbit(35.0, 12.0));
        let mut zoomed = orbited;
        assert!(zoomed.zoom(0.4));
        vec![base, orbited, zoomed]
    }

    #[derive(Default)]
    struct KeepSink(Option<LodBuiltScene>);

    impl SceneSink for KeepSink {
        fn publish(&mut self, built: LodBuiltScene) -> Result<(), String> {
            self.0 = Some(built);
            Ok(())
        }
        fn base_cut_exceeded(&mut self, _: &LodDataset) -> Result<(), String> {
            Ok(())
        }
    }

    fn observe(
        scene: Option<LodBuiltScene>,
        stats: &crate::LodViewStats,
        async_order: Vec<BlockId>,
        needs_refinement: bool,
    ) -> Observed {
        Observed {
            digest: scene.as_ref().map(LodBuiltScene::scene_digest),
            instance_count: scene.as_ref().map(|s| s.voxels.scene.instance_count()),
            block_ids: scene.map(|s| s.block_ids),
            rendered_voxels: stats.rendered_voxels,
            stage: stats.lod_stage_pixels,
            pending: stats.pending_block_ids.clone(),
            async_order,
            needs_refinement,
        }
    }

    /// The state `WasmVoxelViewer` keeps around the shared functions.
    struct ViewerPath {
        dataset: LodDataset,
        reader: Option<Vec<PointBlock>>,
        mode: ColorMode,
        attribute: Option<(u32, u32)>,
        attribute_range: Option<[f64; 2]>,
        target: Option<f64>,
        stage: Option<f64>,
        dirty: bool,
        camera: Option<OrbitCamera>,
    }

    impl ViewerPath {
        fn update(&mut self, camera: OrbitCamera, target: f64) -> Observed {
            if self.target != Some(target) {
                self.stage = None;
            }
            if self.camera != Some(camera) {
                self.dirty = true;
            }
            self.camera = Some(camera);
            self.target = Some(target);
            let view = ViewProjection::new(
                camera,
                [800, 600],
                self.dataset.visual_scale(),
                VoxelBuildLimits::default(),
            );
            let mut sink = KeepSink::default();
            let reader = self.reader.as_ref();
            let mut load = |id: BlockId| {
                Ok(reader.and_then(|blocks| blocks.iter().find(|b| b.id() == id).cloned()))
            };
            let result = update_lod_view(
                &mut self.dataset,
                &LodStep {
                    view: &view,
                    target_pixels: target,
                    track_async_order: self.reader.is_none(),
                    can_load_blocks: true,
                },
                &mut self.stage,
                &mut self.dirty,
                &mut sink,
                &mut load,
            )
            .unwrap();
            observe(
                sink.0,
                &result.stats,
                result.async_priority_order,
                self.dirty,
            )
        }

        fn color(&mut self, selection: ColorSelection) {
            if self.mode != selection.mode || self.attribute != selection.attribute {
                self.mode = selection.mode;
                self.attribute = selection.attribute;
                self.attribute_range = selection.attribute_range;
                self.dataset.clear_loaded_blocks();
                self.stage = Some(16.0);
                self.dirty = true;
            }
        }
    }

    fn builder_update(
        core: &mut SceneBuilderCore<Vec<PointBlock>>,
        camera: OrbitCamera,
        target: f64,
    ) -> Observed {
        // The camera crosses the worker boundary as an f64 array.
        let camera = camera_from_transport(&camera.to_f64_array()).unwrap();
        let visual_scale = core.session().dataset().visual_scale();
        let update = core
            .update(camera, [800, 600], visual_scale, target, |blocks, id| {
                Ok(blocks.iter().find(|b| b.id() == id).cloned())
            })
            .unwrap();
        assert!(update.epoch >= 1);
        observe(
            update.scene,
            &update.stats,
            update.async_priority_order,
            update.needs_refinement,
        )
    }

    /// Runs `ops` through both paths and asserts identical observations at every update.
    fn run_parity(ops: &[Op], session_reads_blocks: bool) -> Vec<Observed> {
        let (viewer_dataset, blocks) = native_dataset(400);
        let (builder_dataset, _) = native_dataset(400);
        let mut viewer = ViewerPath {
            dataset: viewer_dataset,
            reader: session_reads_blocks.then(|| blocks.clone()),
            mode: ColorMode::Rgb,
            attribute: None,
            attribute_range: None,
            target: None,
            stage: None,
            dirty: false,
            camera: None,
        };
        let session = if session_reads_blocks {
            LodSession::Format {
                dataset: builder_dataset,
                reader: blocks.clone(),
            }
        } else {
            LodSession::External(builder_dataset)
        };
        let mut builder =
            SceneBuilderCore::new(session, !session_reads_blocks, VoxelBuildLimits::default());
        let cameras = cameras();
        let mut seen = Vec::new();
        for (step, op) in ops.iter().enumerate() {
            match op {
                Op::EvictAll => {
                    viewer.dataset.clear_loaded_blocks();
                    builder.session_mut().dataset_mut().clear_loaded_blocks();
                }
                Op::Insert(id) => {
                    let block = blocks.iter().find(|b| b.id().get() == *id).unwrap().clone();
                    viewer.dataset.insert_block(block.clone()).unwrap();
                    viewer.dirty = viewer.target.is_some();
                    let dataset = builder.session_mut().dataset_mut();
                    dataset.insert_block(block).unwrap();
                    builder.block_inserted();
                }
                Op::InsertAttributed(id) => {
                    let source = blocks.iter().find(|b| b.id().get() == *id).unwrap();
                    let block = attributed_block(source, viewer.attribute, viewer.attribute_range);
                    viewer.dataset.insert_block(block).unwrap();
                    viewer.dirty = viewer.target.is_some();
                    let (selected, range) = builder.selected_attribute();
                    let block = attributed_block(source, selected, range);
                    builder
                        .session_mut()
                        .dataset_mut()
                        .insert_block(block)
                        .unwrap();
                    builder.block_inserted();
                }
                Op::Attribute => {
                    let schema = attribute_schema();
                    let selection =
                        resolve_color_selection("attribute:0:0", Some(&schema), |_, _| {
                            Ok(Some(AttributeAvailability {
                                has_attributes: true,
                                range: Some(ATTRIBUTE_RANGE),
                            }))
                        })
                        .unwrap();
                    viewer.color(selection);
                    builder.set_color_selection(selection);
                    assert_eq!(builder.selected_attribute().0, viewer.attribute);
                    assert_eq!(builder.selected_attribute().1, viewer.attribute_range);
                }
                Op::Color(mode) => {
                    let selection =
                        resolve_color_selection(mode, Some(&[]), |_, _| Ok(None)).unwrap();
                    viewer.color(selection);
                    builder.set_color_selection(selection);
                    assert_eq!(builder.color_mode(), viewer.mode, "step {step}");
                    assert_eq!(builder.selected_attribute().0, viewer.attribute);
                }
                Op::Update { camera, target } => {
                    let expected = viewer.update(cameras[*camera], *target);
                    let actual = builder_update(&mut builder, cameras[*camera], *target);
                    assert_eq!(actual, expected, "step {step}");
                    seen.push(actual);
                }
            }
        }
        seen
    }

    #[test]
    fn alternating_cameras_keep_published_stage_with_missing_blocks() {
        let seen = run_parity(
            &[
                update(0, 4.0),
                Op::Insert(11),
                Op::Insert(12),
                Op::Insert(13),
                update(0, 4.0),
                update(0, 4.0),
                update(0, 4.0),
                Op::EvictAll,
                update(1, 4.0),
                update(0, 4.0),
                update(1, 4.0),
            ],
            false,
        );
        assert!(seen.iter().skip(4).all(|o| !o.pending.is_empty()));
        for pair in seen.windows(2) {
            assert!(
                pair[1].stage <= pair[0].stage,
                "stages: {:?}",
                seen.iter().map(|o| o.stage).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn target_change_and_new_dataset_restart_nonresident_stage() {
        let seen = run_parity(
            &[
                update(0, 4.0),
                Op::Insert(11),
                Op::Insert(12),
                Op::Insert(13),
                update(0, 4.0),
                update(0, 4.0),
                update(0, 4.0),
                Op::EvictAll,
                update(1, 4.0),
                update(1, 2.0),
            ],
            false,
        );
        assert_eq!(seen[4].stage, 4.0);
        assert!(!seen[5].pending.is_empty());
        assert_eq!(seen[5].stage, 16.0);
        let fresh_dataset = run_parity(&[update(1, 2.0)], false);
        assert!(!fresh_dataset[0].pending.is_empty());
        assert_eq!(fresh_dataset[0].stage, 16.0);
    }

    #[test]
    fn builder_matches_the_viewer_while_the_host_inserts_blocks() {
        let ops = [
            update(0, 4.0),
            Op::Insert(11),
            update(0, 4.0),
            update(0, 4.0),
            Op::Insert(12),
            update(0, 4.0),
            Op::Insert(13),
            update(0, 4.0),
            update(0, 4.0),
            update(0, 4.0),
            // Camera movement preserves the current stage.
            update(1, 4.0),
            update(1, 4.0),
            update(2, 4.0),
            // Everything is resident, so a finer target starts at the fine stage directly.
            update(2, 1.0),
            update(2, 1.0),
            update(2, 16.0),
        ];
        let seen = run_parity(&ops, false);
        assert!(seen.iter().filter(|o| o.digest.is_some()).count() >= 3);
        assert!(seen.iter().any(|o| !o.async_order.is_empty()));
        assert!(seen.iter().any(|o| o.digest.is_none()), "a scene is reused");
    }

    #[test]
    fn builder_matches_the_viewer_when_the_session_reads_blocks() {
        let ops = [
            update(0, 4.0),
            update(0, 4.0),
            update(0, 4.0),
            update(0, 4.0),
            update(1, 4.0),
            update(1, 4.0),
            update(1, 4.0),
            // A colour change drops the decoded blocks, which are then read again.
            Op::Color("intensity"),
            update(1, 4.0),
            update(1, 4.0),
            update(1, 4.0),
            Op::Color("rgb"),
            update(2, 2.0),
            update(2, 2.0),
            update(2, 2.0),
        ];
        let seen = run_parity(&ops, true);
        assert!(seen.iter().filter(|o| o.digest.is_some()).count() >= 3);
    }

    #[test]
    fn builder_matches_the_viewer_when_colouring_by_attribute() {
        let insert_all = |ops: &mut Vec<Op>| {
            ops.extend([Op::InsertAttributed(11), Op::InsertAttributed(12)]);
            ops.extend([Op::InsertAttributed(13)]);
        };
        let settle = |ops: &mut Vec<Op>| ops.extend((0..4).map(|_| update(0, 4.0)));
        let mut ops = Vec::new();
        insert_all(&mut ops);
        settle(&mut ops);
        let plain_end = ops.len();
        // Selecting the attribute drops the cached blocks, which the host then delivers again.
        ops.push(Op::Attribute);
        insert_all(&mut ops);
        settle(&mut ops);
        let painted_end = ops.len();
        ops.push(Op::Color("rgb"));
        insert_all(&mut ops);
        settle(&mut ops);
        let seen = run_parity(&ops, false);
        let last_digest = |range: std::ops::Range<usize>| {
            // `seen` only holds updates, so map the op range onto it.
            let first = ops[..range.start]
                .iter()
                .filter(|op| matches!(op, Op::Update { .. }))
                .count();
            let count = ops[range]
                .iter()
                .filter(|op| matches!(op, Op::Update { .. }))
                .count();
            seen[first..first + count]
                .iter()
                .rev()
                .find_map(|o| o.digest)
                .unwrap()
        };
        let rgb = last_digest(0..plain_end);
        let attribute = last_digest(plain_end..painted_end);
        assert_ne!(attribute, rgb, "the attribute colours reach the scene");
        assert_eq!(last_digest(painted_end..ops.len()), rgb);
    }

    #[test]
    fn the_scene_reached_through_the_builder_keeps_the_pinned_digest() {
        let ops = [
            Op::Insert(11),
            Op::Insert(12),
            Op::Insert(13),
            update(0, 4.0),
            update(0, 4.0),
            update(0, 4.0),
            update(0, 4.0),
        ];
        let seen = run_parity(&ops, false);
        let last = seen.iter().rev().find_map(|o| o.digest).unwrap();
        assert_eq!(last, NATIVE_UPDATE_GOLDEN.1);
    }

    #[test]
    fn the_builder_validates_its_inputs() {
        let (dataset, _) = native_dataset(10);
        let mut core: SceneBuilderCore<()> = SceneBuilderCore::new(
            LodSession::External(dataset),
            true,
            VoxelBuildLimits::default(),
        );
        let camera = cameras()[0];
        let scale = core.session().dataset().visual_scale();
        let mut run = |viewport: [u32; 2], scale: f64, target: f64| {
            core.update(camera, viewport, scale, target, |_, _| Ok(None))
                .err()
        };
        assert!(run([800, 600], scale, 0.1).is_some());
        assert!(run([800, 600], scale, f64::NAN).is_some());
        assert!(run([0, 600], scale, 4.0).is_some());
        assert!(run([800, 600], scale * 2.0, 4.0).is_some());
        assert!(run([800, 600], scale, 4.0).is_none());
        assert!(camera_from_transport(&[0.0; 3]).is_err());
    }

    #[test]
    fn color_selection_parses_modes_and_rejects_unusable_attributes() {
        let none = |_: u32, _: u32| Ok(None);
        assert_eq!(
            resolve_color_selection("elevation", None, none).unwrap(),
            ColorSelection {
                mode: ColorMode::Elevation,
                attribute: None,
                attribute_range: None,
            }
        );
        assert!(resolve_color_selection("bogus", None, none).is_err());
        assert!(resolve_color_selection("attribute:0:0", None, none).is_err());
        assert!(resolve_color_selection("attribute:x:0", Some(&[]), none).is_err());
        assert!(resolve_color_selection("attribute:0", Some(&[]), none).is_err());
        // Index 0 does not exist in an empty schema, however available the attributes are.
        let available = |_: u32, _: u32| {
            Ok(Some(AttributeAvailability {
                has_attributes: true,
                range: Some([0.0, 1.0]),
            }))
        };
        assert!(resolve_color_selection("attribute:0:0", Some(&[]), available).is_err());
    }

    #[test]
    fn async_blocks_are_cached_as_delivered_unless_an_attribute_is_missing() {
        let (_, blocks) = native_dataset(8);
        let source = &blocks[0];
        let assemble = |selected, budget| {
            assemble_async_block(
                source.id(),
                source.points().clone(),
                None,
                &[],
                selected,
                None,
                budget,
            )
        };
        assert_eq!(
            assemble(None, usize::MAX).unwrap().points().points().len(),
            8
        );
        assert!(assemble(None, 1).is_err());
        assert!(assemble(Some((0, 0)), usize::MAX).is_err());
    }
}

#[test]
fn an_unloaded_anchor_holds_the_stage_and_is_requested() {
    let (_, blocks) = native_dataset(400);
    let root = SpatialNode::new(
        NodeId::new(1),
        None,
        0,
        SpatialBounds::new([0.0; 3], [64.0; 3]).unwrap(),
        400,
        0.0,
        vec![BlockId::new(11)],
    )
    .unwrap();
    let mut hierarchy = SpatialHierarchy::new(vec![root], Vec::new(), Some(400)).unwrap();
    hierarchy.mark_unloaded(NodeId::new(1)).unwrap();
    let mut dataset =
        LodDataset::new([0.0; 3], hierarchy, 8 * blocks[0].estimated_memory_bytes()).unwrap();
    dataset.insert_block(blocks[0].clone()).unwrap();

    let update = dataset
        .update_view_progressive(&native_view(), 4.0, 4.0, 4.0, None, |_| Ok(None))
        .unwrap();
    // The root block is resident and covers the view, yet its page of children is missing.
    assert!(update.stats.pending_block_ids.is_empty());
    assert_eq!(update.stats.pending_page_anchors, [NodeId::new(1)]);
    assert!(update.stats.has_pending_loads());
    assert_eq!(
        next_lod_stage(Some(8.0), 4.0, update.stats.has_pending_loads(), true),
        (Some(8.0), true)
    );

    let target = dataset
        .metadata
        .hierarchy()
        .select_lod(
            &SpatialFrustum::new(vec![FrustumPlane::new([1.0, 0.0, 0.0], 1000.0).unwrap()])
                .unwrap(),
            LodSelectionConfig::new(1.0, u64::MAX).unwrap(),
            |_| 100.0,
        )
        .unwrap();
    let plan = LodTransitionPlanner::new().prepare(
        dataset.metadata.hierarchy(),
        &target,
        &DecodedBlockCache::new(usize::MAX),
    );
    assert!(!resident_target_plan_is_complete(&target, &plan));
}

/// Root (id 1) with two unloaded anchors 2 and 3; pages below them hold one child each.
fn paged_dataset(budget: usize) -> (LodDataset, std::rc::Rc<std::cell::RefCell<Vec<NodeId>>>) {
    let node = |id: u64, parent: Option<u64>, level: u32, min: f64, max: f64| {
        SpatialNode::new(
            NodeId::new(id),
            parent.map(NodeId::new),
            level,
            SpatialBounds::new([min, 0.0, 0.0], [max, 8.0, 8.0]).unwrap(),
            10,
            0.0,
            vec![BlockId::new(id)],
        )
        .unwrap()
    };
    let hierarchy = SpatialHierarchy::new(
        vec![
            node(1, None, 0, 0.0, 8.0),
            node(2, Some(1), 1, 0.0, 4.0),
            node(3, Some(1), 1, 4.0, 8.0),
        ],
        Vec::new(),
        None,
    )
    .unwrap();
    let released = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = std::rc::Rc::clone(&released);
    let dataset = LodDataset::new([0.0; 3], hierarchy, 1 << 20)
        .unwrap()
        .with_pages(
            &[NodeId::new(2), NodeId::new(3)],
            Box::new(move |anchor| sink.borrow_mut().push(anchor)),
            budget,
        )
        .unwrap();
    (dataset, released)
}

fn page_below(anchor: u64, number: u32, child: u64, min: f64, max: f64) -> crate::PageInsert {
    let node = SpatialNode::new(
        NodeId::new(child),
        Some(NodeId::new(anchor)),
        2,
        SpatialBounds::new([min, 0.0, 0.0], [max, 4.0, 4.0]).unwrap(),
        5,
        0.0,
        vec![BlockId::new(child)],
    )
    .unwrap();
    crate::PageInsert {
        anchor: NodeId::new(anchor),
        number,
        nodes: vec![node],
        unloaded: Vec::new(),
        bytes: 100,
    }
}

#[test]
fn grafting_beyond_the_page_budget_evicts_the_oldest_unpinned_page() {
    let (mut dataset, released) = paged_dataset(150);
    dataset.graft_page(page_below(2, 1, 20, 0.0, 2.0)).unwrap();
    assert!(dataset.references_block(BlockId::new(20)));
    assert_eq!(dataset.resident_node_count(), 4);
    // The second page needs room: the first one carries no retained node, so it goes.
    dataset.graft_page(page_below(3, 2, 30, 4.0, 6.0)).unwrap();
    assert_eq!(*released.borrow(), [NodeId::new(2)]);
    assert!(!dataset.references_block(BlockId::new(20)));
    assert!(dataset.references_block(BlockId::new(30)));
    assert_eq!(dataset.page_stats(), (1, 100, 1));
}

#[test]
fn pinned_pages_are_never_evicted_and_a_full_budget_is_an_error() {
    let (mut dataset, released) = paged_dataset(150);
    dataset.graft_page(page_below(2, 1, 20, 0.0, 2.0)).unwrap();
    // The retained cut holds the page's node: nothing may be evicted.
    dataset.insert_block(block(20)).unwrap();
    dataset.insert_block(block(3)).unwrap();
    let target = dataset
        .metadata
        .hierarchy()
        .select_lod(
            &SpatialFrustum::new(vec![FrustumPlane::new([1.0, 0.0, 0.0], 1000.0).unwrap()])
                .unwrap(),
            LodSelectionConfig::new(1.0, u64::MAX).unwrap(),
            |_| 100.0,
        )
        .unwrap();
    dataset
        .planner
        .update(dataset.metadata.hierarchy(), &target, &mut dataset.cache);
    assert!(dataset.planner.retained_nodes().contains(&NodeId::new(20)));
    let error = dataset
        .graft_page(page_below(3, 2, 30, 4.0, 6.0))
        .unwrap_err();
    assert!(
        matches!(error, LodDatasetError::PageBudget { pinned: 100, .. }),
        "{error}"
    );
    assert!(released.borrow().is_empty());
    assert!(dataset.references_block(BlockId::new(20)));
    assert!(!dataset.references_block(BlockId::new(30)));
}
