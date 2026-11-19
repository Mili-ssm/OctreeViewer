use std::cell::Cell;

use super::{
    AdaptiveVoxelGrid, MAX_PROJECTED_EDGE_CACHE_ENTRIES, VoxelBuildLimits, VoxelKey,
    VoxelSceneBuildError, VoxelSceneBuilder, accumulator_charge_bytes, bounds_side,
    build_voxel_scene_with_limit, projection_entry_charge_bytes, reserve_instances,
};
use viewer_core::{
    BlockId, ColorRgb, DecodedBlockCache, HierarchySemantics, Point, PointBatch, PointBlock,
    SpatialBounds, SpatialHierarchy, SpatialNode,
};

#[test]
fn point_outside_frustum_is_culled_before_voxel_accumulation() {
    let projection_calls = Cell::new(0);
    let project = |_: SpatialBounds| {
        projection_calls.set(projection_calls.get() + 1);
        0.0
    };
    let mut grid = AdaptiveVoxelGrid {
        origin: [0.0; 3],
        side: 1.0,
        target_pixels: 4.0,
        max_depth: 24,
        representative_regions: Vec::new(),
        projected_edge_cache_limit: 0,
        projected_edges: std::collections::BTreeMap::default(),
        projected_edge_order: std::collections::VecDeque::default(),
        projected_edge_peak_entries: 0,
        last_resolved_cell: None,
    };

    assert_eq!(grid.key_for_point([0.5; 3], &project).unwrap(), None);
    assert_eq!(projection_calls.get(), 1);
    assert!(grid.projected_edges.is_empty());
}

#[test]
fn visible_point_keeps_its_projected_cell() {
    let mut grid = AdaptiveVoxelGrid {
        origin: [0.0; 3],
        side: 1.0,
        target_pixels: 4.0,
        max_depth: 24,
        representative_regions: Vec::new(),
        projected_edge_cache_limit: MAX_PROJECTED_EDGE_CACHE_ENTRIES,
        projected_edges: std::collections::BTreeMap::default(),
        projected_edge_order: std::collections::VecDeque::default(),
        projected_edge_peak_entries: 0,
        last_resolved_cell: None,
    };

    assert_eq!(
        grid.key_for_point([0.5; 3], &|_| 2.0).unwrap(),
        Some(VoxelKey {
            depth: 0,
            coord: [0; 3],
        })
    );
}

#[test]
fn terminal_hint_reuses_result_with_projection_cache_disabled() {
    let projection_calls = Cell::new(0);
    let project = |_: SpatialBounds| {
        projection_calls.set(projection_calls.get() + 1);
        2.0
    };
    let mut grid = AdaptiveVoxelGrid {
        origin: [0.0; 3],
        side: 1.0,
        target_pixels: 4.0,
        max_depth: 24,
        representative_regions: Vec::new(),
        projected_edge_cache_limit: 0,
        projected_edges: std::collections::BTreeMap::default(),
        projected_edge_order: std::collections::VecDeque::default(),
        projected_edge_peak_entries: 0,
        last_resolved_cell: None,
    };

    assert!(grid.key_for_point([0.5; 3], &project).unwrap().is_some());
    assert!(grid.key_for_point([0.5; 3], &project).unwrap().is_some());
    assert_eq!(projection_calls.get(), 1);
    assert!(grid.projected_edges.is_empty());
}

#[test]
fn terminal_hint_matches_full_descent_at_boundaries_and_with_eviction() {
    let bounds = SpatialBounds::new([-1.0; 3], [1.0; 3]).unwrap();
    let mut points = vec![[-2.0; 3], [-1.0; 3], [0.0; 3], [1.0; 3], [2.0; 3]];
    for face in [-0.5_f64, 0.0, 0.5] {
        points.extend([
            [face - f64::EPSILON; 3],
            [face; 3],
            [face + f64::EPSILON; 3],
        ]);
    }
    let mut seed = 17_u64;
    for _ in 0..2_000 {
        points.push(std::array::from_fn(|_| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (seed >> 32) as f64 / f64::from(u32::MAX) * 4.0 - 2.0
        }));
    }
    for cache_limit in [0, 1, 32] {
        for target in [0.5, 4.0, 16.0] {
            let mut hinted = VoxelSceneBuilder::new(bounds, [0.0; 3], target, 64).grid;
            let mut reference = VoxelSceneBuilder::new(bounds, [0.0; 3], target, 64).grid;
            hinted.projected_edge_cache_limit = cache_limit;
            reference.projected_edge_cache_limit = cache_limit;
            let project = |bounds: SpatialBounds| {
                if bounds.min()[0] >= 0.5 {
                    0.0
                } else {
                    bounds_side(bounds) * 64.0 / (1.0 + bounds.min()[2].abs())
                }
            };
            for point in &points {
                // Reset only the hint to exercise the original root-to-terminal descent.
                reference.last_resolved_cell = None;
                let expected = reference.key_for_point(*point, &project).unwrap();
                assert_eq!(hinted.key_for_point(*point, &project).unwrap(), expected);
                assert_eq!(hinted.key_for_point(*point, &project).unwrap(), expected);
            }
        }
    }
    // Nonfinite projected edges retain the existing depth-24 fallback.
    let mut grid = VoxelSceneBuilder::new(bounds, [0.0; 3], 4.0, 64).grid;
    let key = grid.key_for_point([0.25; 3], &|_| f64::NAN).unwrap();
    assert_eq!(key.unwrap().depth, 24);
    assert_eq!(grid.key_for_point([0.25; 3], &|_| f64::NAN).unwrap(), key);
}

#[test]
fn terminal_hint_preserves_scene_colors_attributes_and_budget_errors() {
    use viewer_core::{AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues};
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let project = |bounds: SpatialBounds| bounds_side(bounds) * 32.0;
    let make_block = |index: usize| {
        PointBlock::new(
            BlockId::new(index as u64),
            PointBatch::new(vec![Point::new(
                [index as f32 / 200.0; 3],
                ColorRgb::new(index as u8, 20, 90),
            )]),
            vec![
                AttributeColumn::new(
                    AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap(),
                    AttributeValues::U16(vec![index as u16 * 3]),
                    1,
                )
                .unwrap(),
                AttributeColumn::new(
                    AttributeDescriptor::new("classification", AttributeDataType::U8, 1).unwrap(),
                    AttributeValues::U8(vec![(index % 8) as u8]),
                    1,
                )
                .unwrap(),
            ],
        )
    };
    for limits in [
        VoxelBuildLimits::default(),
        VoxelBuildLimits {
            max_voxels: 1,
            ..VoxelBuildLimits::default()
        },
        VoxelBuildLimits {
            max_estimated_bytes: 256,
            ..VoxelBuildLimits::default()
        },
    ] {
        let mut hinted = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 4.0, limits);
        let mut reference = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 4.0, limits);
        for index in 0..100 {
            let block = make_block(index);
            reference.grid.last_resolved_cell = None;
            let expected = reference.add_block(&block, &project);
            assert_eq!(hinted.add_block(&block, &project), expected);
            if expected.is_err() {
                break;
            }
        }
        match (hinted.try_finish(&project), reference.try_finish(&project)) {
            (Ok(actual), Ok(expected)) => {
                assert_eq!(actual.scene, expected.scene);
                assert_eq!(actual.min_edge_pixels, expected.min_edge_pixels);
                assert_eq!(actual.max_edge_pixels, expected.max_edge_pixels);
                assert_eq!(actual.oversized_voxels, 0);
            }
            (Err(actual), Err(expected)) => assert_eq!(actual, expected),
            _ => panic!("hint changed scene completion"),
        }
    }
}

#[test]
fn incremental_builder_matches_cached_scene_output() {
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            SpatialNode::new(
                viewer_core::NodeId::new(1),
                None,
                0,
                bounds,
                3,
                0.0,
                vec![BlockId::new(1)],
            )
            .unwrap(),
        ],
        vec![],
        Some(3),
        HierarchySemantics::Replacement,
    )
    .unwrap();
    let block = PointBlock::new(
        BlockId::new(1),
        PointBatch::new(vec![
            Point::new([0.1, 0.1, 0.1], ColorRgb::new(1, 2, 3)),
            Point::new([0.8, 0.1, 0.1], ColorRgb::new(7, 8, 9)),
            Point::new([0.1, 0.8, 0.1], ColorRgb::new(10, 11, 12)),
        ]),
        vec![],
    );
    let mut cache = DecodedBlockCache::new(4096);
    cache.insert(block.clone());
    let project = |bounds| bounds_side(bounds) * 16.0;
    let expected = build_voxel_scene_with_limit(
        &hierarchy,
        &[viewer_core::NodeId::new(1)],
        &cache,
        [0.0; 3],
        4.0,
        &project,
        8,
    )
    .unwrap();

    let mut builder = super::VoxelSceneBuilder::new(bounds, [0.0; 3], 4.0, 8);
    builder.add_block(&block, &project).unwrap();
    let actual = builder.finish(&project);

    assert_eq!(actual.scene, expected.scene);
    assert_eq!(actual.min_edge_pixels, expected.min_edge_pixels);
    assert_eq!(actual.max_edge_pixels, expected.max_edge_pixels);
    assert_eq!(actual.oversized_voxels, expected.oversized_voxels);
}

#[test]
fn alternate_resident_source_preserves_voxels_missing_blocks_and_limits() {
    struct ResidentBlocks(Vec<PointBlock>);
    impl viewer_core::ResidentBlockSource for ResidentBlocks {
        fn peek(&self, id: BlockId) -> Option<&PointBlock> {
            self.0.iter().find(|block| block.id() == id)
        }
    }
    let hierarchy = SpatialHierarchy::new(
        vec![
            SpatialNode::new(
                viewer_core::NodeId::new(1),
                None,
                0,
                SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                3,
                0.0,
                vec![BlockId::new(1)],
            )
            .unwrap(),
        ],
        vec![],
        Some(3),
    )
    .unwrap();
    let blocks = ResidentBlocks(vec![block(vec![
        point([0.1; 3]),
        point([0.8, 0.1, 0.1]),
        point([0.1, 0.8, 0.1]),
    ])]);
    let mut cache = DecodedBlockCache::new(4096);
    cache.insert(blocks.0[0].clone());
    let stats = cache.stats();
    let recency = cache.lru_entries().collect::<Vec<_>>();
    let erased: &dyn viewer_core::ResidentBlockSource = &blocks;
    let nodes = [viewer_core::NodeId::new(1), viewer_core::NodeId::new(999)];
    let project = |bounds| bounds_side(bounds) * 16.0;
    for limits in [
        VoxelBuildLimits {
            max_voxels: 10,
            max_estimated_bytes: u64::MAX,
        },
        VoxelBuildLimits {
            max_voxels: 2,
            max_estimated_bytes: u64::MAX,
        },
        VoxelBuildLimits {
            max_voxels: 10,
            max_estimated_bytes: 0,
        },
    ] {
        let build = |source: &dyn viewer_core::ResidentBlockSource| {
            super::build_voxel_scene_with_limits(
                &hierarchy, &nodes, source, [0.0; 3], 1.0, &project, limits,
            )
            .map(|result| result.scene)
        };
        let concrete = super::build_voxel_scene_with_limits(
            &hierarchy, &nodes, &blocks, [0.0; 3], 1.0, &project, limits,
        )
        .map(|result| result.scene);
        assert_eq!(concrete, build(&cache));
        assert_eq!(build(erased), build(&cache));
    }
    let absent = ResidentBlocks(vec![]);
    let empty = super::build_voxel_scene(&hierarchy, &nodes, &absent, [0.0; 3], 1.0, &project);
    assert_eq!(empty.scene.instance_count(), 0);
    assert_eq!(cache.stats(), stats);
    assert_eq!(cache.lru_entries().collect::<Vec<_>>(), recency);
}

#[test]
fn voxel_scene_stops_at_the_rendered_instance_limit() {
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            SpatialNode::new(
                viewer_core::NodeId::new(1),
                None,
                0,
                SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                3,
                0.0,
                vec![BlockId::new(1)],
            )
            .unwrap(),
        ],
        vec![],
        Some(3),
        HierarchySemantics::Replacement,
    )
    .unwrap();
    let mut cache = DecodedBlockCache::new(4096);
    cache.insert(PointBlock::new(
        BlockId::new(1),
        PointBatch::new(vec![
            Point::new([0.1, 0.1, 0.1], ColorRgb::new(1, 2, 3)),
            Point::new([0.8, 0.1, 0.1], ColorRgb::new(1, 2, 3)),
            Point::new([0.1, 0.8, 0.1], ColorRgb::new(1, 2, 3)),
        ]),
        vec![],
    ));

    let error = build_voxel_scene_with_limit(
        &hierarchy,
        &[viewer_core::NodeId::new(1)],
        &cache,
        [0.0; 3],
        1.0,
        &|bounds| bounds_side(bounds) * 16.0,
        2,
    )
    .err()
    .unwrap();

    assert_eq!(
        error,
        VoxelSceneBuildError::InstanceLimitExceeded { limit: 2 }
    );
}

/// A `v7.4-pre` root with 256 representatives per axis must stop at its own representative
/// cells (depth 8), not at the 8-per-axis depth 3 of the older formats.
#[test]
fn stored_spacing_sets_the_terminal_depth_of_a_coarse_node() {
    let node = |id, parent, level, side: f64, blocks| {
        SpatialNode::new(
            viewer_core::NodeId::new(id),
            parent,
            level,
            SpatialBounds::new([0.0; 3], [side; 3]).unwrap(),
            2,
            0.0,
            blocks,
        )
        .unwrap()
    };
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            node(1, None, 0, 1.0, vec![BlockId::new(1)])
                .with_spacing(1.0 / 256.0)
                .unwrap(),
            node(2, Some(viewer_core::NodeId::new(1)), 1, 0.5, vec![]),
        ],
        vec![],
        Some(2),
        HierarchySemantics::Replacement,
    )
    .unwrap();
    let mut cache = DecodedBlockCache::new(4096);
    cache.insert(block(vec![point([0.1; 3]), point([0.8; 3])]));

    // Cells stay above the 1 px target at every depth, so only the representative rule stops.
    let scene = build_voxel_scene_with_limit(
        &hierarchy,
        &[viewer_core::NodeId::new(1)],
        &cache,
        [0.0; 3],
        1.0,
        &|bounds| bounds_side(bounds) * 100_000.0,
        usize::MAX,
    )
    .unwrap();

    let sizes: Vec<f32> = scene.scene.instances().iter().map(|i| i.size).collect();
    assert_eq!(sizes.len(), 2);
    assert!(sizes.iter().all(|size| (size - 1.0 / 256.0).abs() < 1.0e-7));
}

fn block(points: Vec<Point>) -> PointBlock {
    PointBlock::new(BlockId::new(1), PointBatch::new(points), vec![])
}

fn point(position: [f32; 3]) -> Point {
    Point::new(position, ColorRgb::new(10, 20, 30))
}

#[test]
fn memory_budget_accepts_exactly_one_voxel_and_rejects_the_next() {
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let exact_bytes = accumulator_charge_bytes() + std::mem::size_of::<super::InstanceRaw>() as u64;
    let limits = VoxelBuildLimits {
        max_voxels: 10,
        max_estimated_bytes: exact_bytes,
    };
    let project = |bounds| bounds_side(bounds) * 2.0;
    let mut exact = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 1.0, limits);
    exact
        .add_block(&block(vec![point([0.1; 3])]), &project)
        .unwrap();
    let scene = exact.try_finish(&project).unwrap();
    assert_eq!(scene.memory_stats.estimated_peak_bytes, exact_bytes);

    let mut over = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 1.0, limits);
    let error = over
        .add_block(&block(vec![point([0.1; 3]), point([0.8; 3])]), &project)
        .unwrap_err();
    assert_eq!(
        error,
        VoxelSceneBuildError::EstimatedMemoryLimitExceeded {
            required_bytes: 2 * accumulator_charge_bytes()
                + 2 * std::mem::size_of::<super::InstanceRaw>() as u64,
            max_bytes: exact_bytes,
        }
    );
    assert_eq!(over.add_block(&block(vec![]), &project), Err(error));
    assert_eq!(over.try_finish(&project).unwrap_err(), error);
}

#[test]
fn voxel_limit_is_independent_from_memory_budget() {
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let limits = VoxelBuildLimits {
        max_voxels: 1,
        max_estimated_bytes: 1024 * 1024,
    };
    let mut builder = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 1.0, limits);
    let error = builder
        .add_block(&block(vec![point([0.1; 3]), point([0.8; 3])]), &|bounds| {
            bounds_side(bounds) * 2.0
        })
        .unwrap_err();
    assert_eq!(
        error,
        VoxelSceneBuildError::InstanceLimitExceeded { limit: 1 }
    );
}

#[test]
fn repeated_points_in_one_voxel_consume_one_accumulator_budget() {
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let limits = VoxelBuildLimits {
        max_voxels: 1,
        max_estimated_bytes: accumulator_charge_bytes()
            + std::mem::size_of::<super::InstanceRaw>() as u64,
    };
    let mut builder = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 4.0, limits);
    builder
        .add_block(&block(vec![point([0.1; 3]); 100]), &|_| 2.0)
        .unwrap();
    assert_eq!(
        builder.try_finish(&|_| 2.0).unwrap().scene.instance_count(),
        1
    );
}

#[test]
fn low_budget_disables_projection_cache_and_records_estimated_stats() {
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let limits = VoxelBuildLimits {
        max_voxels: 1,
        max_estimated_bytes: projection_entry_charge_bytes() - 1,
    };
    let builder = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 4.0, limits);
    assert_eq!(builder.grid.projected_edge_cache_limit, 0);
    let scene = builder.try_finish(&|_| 1.0).unwrap();
    assert_eq!(scene.memory_stats.projection_cache_peak_entries, 0);
    assert_eq!(scene.memory_stats.estimated_peak_bytes, 0);
}

#[test]
fn unrenderable_cell_does_not_consume_voxel_or_memory_budget() {
    let bounds = SpatialBounds::new([100_000_000.0; 3], [100_000_001.0; 3]).unwrap();
    let limits = VoxelBuildLimits {
        max_voxels: 0,
        max_estimated_bytes: 1024 * 1024,
    };
    let mut builder = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 4.0, limits);
    builder
        .add_block(&block(vec![point([100_000_000.0; 3])]), &|_| 1.0)
        .unwrap();
    let scene = builder.try_finish(&|_| 1.0).unwrap();
    assert_eq!(scene.scene.instance_count(), 0);
    assert_eq!(scene.memory_stats.instance_capacity_bytes, 0);
}

#[test]
fn scene_is_identical_with_projection_cache_disabled_or_evicted() {
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let points = vec![
        point([0.1, 0.1, 0.1]),
        point([0.8, 0.1, 0.1]),
        point([0.1, 0.8, 0.1]),
        point([0.8, 0.8, 0.8]),
    ];
    let limits = VoxelBuildLimits {
        max_voxels: 32,
        max_estimated_bytes: 1024 * 1024,
    };
    let project = |bounds| bounds_side(bounds) * 8.0;
    let mut evicting = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 1.0, limits);
    evicting.grid.projected_edge_cache_limit = 1;
    evicting
        .add_block(&block(points.clone()), &project)
        .unwrap();
    let evicted = evicting.try_finish(&project).unwrap();
    assert!(evicted.memory_stats.projection_cache_peak_entries > 0);

    let mut uncached = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 1.0, limits);
    uncached.grid.projected_edge_cache_limit = 0;
    let uncached = uncached
        .add_block(&block(points), &project)
        .and_then(|()| uncached.try_finish(&project));
    assert_eq!(uncached.unwrap().scene, evicted.scene);
}

#[test]
fn charge_overflow_is_reported_without_large_allocation() {
    assert_eq!(
        super::checked_charge(usize::MAX, u64::MAX),
        Err(VoxelSceneBuildError::SizeOverflow)
    );
}

#[test]
fn impossible_instance_reservation_returns_an_error_without_allocating() {
    assert_eq!(
        reserve_instances(usize::MAX).unwrap_err(),
        VoxelSceneBuildError::SizeOverflow
    );
    // Vec rejects capacities above isize::MAX bytes before asking the allocator.
    let count = isize::MAX as usize / std::mem::size_of::<super::InstanceRaw>() + 1;
    assert_eq!(
        reserve_instances(count).unwrap_err(),
        VoxelSceneBuildError::AllocationFailed {
            requested_bytes: count as u64 * super::VOXEL_INSTANCE_SIZE,
        }
    );
}
