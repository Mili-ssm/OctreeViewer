use viewer_core::{
    BlockId, ColorRgb, DecodedBlockCache, HierarchySemantics, NodeId, Point, PointBatch,
    PointBlock, SpatialBounds, SpatialHierarchy, SpatialNode,
};
use viewer_render_wgpu::{RenderScene, VoxelBuildLimits, build_voxel_scene_with_limits};

fn fixture(
    semantics: HierarchySemantics,
    origin: [f64; 3],
    coarse_max_point: bool,
) -> (SpatialHierarchy, DecodedBlockCache) {
    let bounds = |min: [f64; 3], max: [f64; 3]| {
        SpatialBounds::new(
            std::array::from_fn(|axis| min[axis] + origin[axis]),
            std::array::from_fn(|axis| max[axis] + origin[axis]),
        )
        .unwrap()
    };
    let node = |id, parent: Option<u64>, level, min, max, blocks| {
        SpatialNode::new(
            NodeId::new(id),
            parent.map(NodeId::new),
            level,
            bounds(min, max),
            3,
            1.0,
            blocks,
        )
        .unwrap()
    };
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            node(0, None, 0, [0.0; 3], [16.0; 3], vec![]),
            node(1, Some(0), 1, [0.0; 3], [8.0; 3], vec![BlockId::new(1)]),
            node(
                2,
                Some(0),
                1,
                [8.0, 0.0, 0.0],
                [16.0, 8.0, 8.0],
                vec![BlockId::new(2)],
            ),
            node(3, Some(1), 2, [0.0; 3], [4.0; 3], vec![]),
        ],
        vec![],
        None,
        semantics,
    )
    .unwrap();
    let mut cache = DecodedBlockCache::new(1024 * 1024);
    for (id, positions) in [
        (
            1,
            vec![if coarse_max_point {
                [8.0, 0.5, 0.5]
            } else {
                [0.5; 3]
            }],
        ),
        (2, vec![[8.125, 0.125, 0.125], [8.375, 0.125, 0.125]]),
    ] {
        cache.insert(PointBlock::new(
            BlockId::new(id),
            PointBatch::new(
                positions
                    .into_iter()
                    .map(|p| Point::new(p, ColorRgb::new(20, 30, 40)))
                    .collect(),
            ),
            vec![],
        ));
    }
    (hierarchy, cache)
}

fn project(bounds: SpatialBounds) -> f64 {
    (bounds.max()[0] - bounds.min()[0]) * 4.0
}

fn assert_disjoint(scene: &RenderScene) {
    for (index, a) in scene.instances().iter().enumerate() {
        for b in &scene.instances()[index + 1..] {
            assert!(
                (0..3).any(|axis| {
                    let distance = (f64::from(a.center[axis]) - f64::from(b.center[axis])).abs();
                    distance >= (f64::from(a.size) + f64::from(b.size)) * 0.5
                }),
                "overlapping cells {a:?} and {b:?}"
            );
        }
    }
}

#[test]
fn aligned_coarse_region_preserves_its_cells_and_allows_unrelated_leaf_detail() {
    for origin in [[0.0; 3], [1_000_000.0, -2_000_000.0, 3_000_000.0]] {
        let (hierarchy, cache) = fixture(HierarchySemantics::Replacement, origin, false);
        let build = |nodes: &[NodeId]| {
            build_voxel_scene_with_limits(
                &hierarchy,
                nodes,
                &cache,
                origin,
                1.0,
                &project,
                VoxelBuildLimits::default(),
            )
            .unwrap()
        };
        let scene = build(&[NodeId::new(1), NodeId::new(2)]);
        assert_eq!(
            scene.scene.instance_count(),
            3,
            "a coarse sibling must not merge fine points"
        );
        assert_eq!(scene.scene, build(&[NodeId::new(2), NodeId::new(1)]).scene);
        let coarse = scene
            .scene
            .instances()
            .iter()
            .find(|v| v.center[0] < 8.0)
            .unwrap();
        assert!(
            (coarse.size - 1.0).abs() < 2e-6,
            "parent representative shrank"
        );
        assert_eq!(
            scene
                .scene
                .instances()
                .iter()
                .filter(|v| v.size < 0.5)
                .count(),
            2
        );
        assert_disjoint(&scene.scene);
    }
}

#[test]
fn representative_on_inclusive_max_caps_the_shared_region() {
    let (hierarchy, cache) = fixture(HierarchySemantics::Replacement, [0.0; 3], true);
    let scene = build_voxel_scene_with_limits(
        &hierarchy,
        &[NodeId::new(1), NodeId::new(2)],
        &cache,
        [0.0; 3],
        1.0,
        &project,
        VoxelBuildLimits::default(),
    )
    .unwrap();
    assert!(
        scene
            .scene
            .instances()
            .iter()
            .all(|v| (v.size - 1.0).abs() < 2e-6)
    );
    assert_disjoint(&scene.scene);
}

#[test]
fn additive_does_not_apply_replacement_representative_limits() {
    let (hierarchy, cache) = fixture(HierarchySemantics::Additive, [0.0; 3], false);
    let scene = build_voxel_scene_with_limits(
        &hierarchy,
        &[NodeId::new(1), NodeId::new(2)],
        &cache,
        [0.0; 3],
        1.0,
        &project,
        VoxelBuildLimits::default(),
    )
    .unwrap();
    assert_eq!(scene.scene.instance_count(), 3);
    assert!(scene.scene.instances().iter().all(|v| v.size < 0.5));
    assert_disjoint(&scene.scene);
}

#[test]
fn regional_frontier_is_independent_of_projection_cache_and_respects_budget() {
    let (hierarchy, cache) = fixture(HierarchySemantics::Replacement, [0.0; 3], false);
    let build = |limits| {
        build_voxel_scene_with_limits(
            &hierarchy,
            &[NodeId::new(1), NodeId::new(2)],
            &cache,
            [0.0; 3],
            1.0,
            &project,
            limits,
        )
    };
    let full = build(VoxelBuildLimits::default()).unwrap();
    let small = build(VoxelBuildLimits {
        max_estimated_bytes: 1024,
        ..VoxelBuildLimits::default()
    })
    .unwrap();
    assert_eq!(small.scene, full.scene);
    assert!(small.memory_stats.estimated_peak_bytes <= 1024);
    assert_eq!(
        build(VoxelBuildLimits {
            max_voxels: 2,
            ..VoxelBuildLimits::default()
        })
        .unwrap_err(),
        viewer_render_wgpu::VoxelSceneBuildError::InstanceLimitExceeded { limit: 2 }
    );
}

#[test]
fn points_from_other_blocks_inside_a_coarse_cell_follow_the_same_frontier() {
    let (hierarchy, mut cache) = fixture(HierarchySemantics::Replacement, [0.0; 3], false);
    // A generic/external provider may supply a point outside its node bounds.
    cache.insert(PointBlock::new(
        BlockId::new(2),
        PointBatch::new(vec![
            Point::new([0.6; 3], ColorRgb::new(20, 30, 40)),
            Point::new([8.125, 0.125, 0.125], ColorRgb::new(20, 30, 40)),
        ]),
        vec![],
    ));
    let scene = build_voxel_scene_with_limits(
        &hierarchy,
        &[NodeId::new(1), NodeId::new(2)],
        &cache,
        [0.0; 3],
        1.0,
        &project,
        VoxelBuildLimits::default(),
    )
    .unwrap();
    assert_eq!(scene.scene.instance_count(), 2);
    assert_disjoint(&scene.scene);
    assert_eq!(
        scene
            .scene
            .instances()
            .iter()
            .filter(|v| v.center[0] < 8.0)
            .count(),
        1
    );
}

#[test]
fn noncubic_representative_bounds_allow_remote_detail() {
    let (hierarchy, cache) = fixture(HierarchySemantics::Replacement, [0.0; 3], false);
    let nodes = (0..4)
        .map(|id| {
            let n = hierarchy.node(NodeId::new(id)).unwrap();
            let bounds = if id == 1 {
                SpatialBounds::new([0.0; 3], [8.0, 7.0, 8.0]).unwrap()
            } else {
                n.bounds()
            };
            SpatialNode::new(
                n.id(),
                n.parent(),
                n.level(),
                bounds,
                n.point_count(),
                n.geometric_error(),
                n.blocks().to_vec(),
            )
            .unwrap()
        })
        .collect();
    let hierarchy =
        SpatialHierarchy::new_with_semantics(nodes, vec![], None, HierarchySemantics::Replacement)
            .unwrap();
    let scene = build_voxel_scene_with_limits(
        &hierarchy,
        &[NodeId::new(1), NodeId::new(2)],
        &cache,
        [0.0; 3],
        1.0,
        &project,
        VoxelBuildLimits::default(),
    )
    .unwrap();
    assert_eq!(scene.scene.instance_count(), 3);
    assert!(
        scene
            .scene
            .instances()
            .iter()
            .filter(|v| v.center[0] < 8.0)
            .all(|v| (v.size - 1.0).abs() < 2e-6)
    );
    assert_disjoint(&scene.scene);
}

#[test]
fn negative_local_coordinates_preserve_the_regional_frontier() {
    let origin = [-16.0, -8.0, -4.0];
    let (hierarchy, cache) = fixture(HierarchySemantics::Replacement, origin, false);
    let mut local_cache = DecodedBlockCache::new(1024 * 1024);
    for id in [BlockId::new(1), BlockId::new(2)] {
        let points = cache
            .peek(id)
            .unwrap()
            .points()
            .points()
            .iter()
            .map(|p| {
                Point::new(
                    std::array::from_fn(|axis| p.position[axis] + origin[axis] as f32),
                    p.color,
                )
            })
            .collect();
        local_cache.insert(PointBlock::new(id, PointBatch::new(points), vec![]));
    }
    let scene = build_voxel_scene_with_limits(
        &hierarchy,
        &[NodeId::new(1), NodeId::new(2)],
        &local_cache,
        [0.0; 3],
        1.0,
        &project,
        VoxelBuildLimits::default(),
    )
    .unwrap();
    assert_eq!(scene.scene.instance_count(), 3);
    assert_disjoint(&scene.scene);
}

#[test]
fn two_coarse_regions_keep_their_own_representative_resolution() {
    let (hierarchy, mut cache) = fixture(HierarchySemantics::Replacement, [0.0; 3], false);
    let mut nodes = (0..4)
        .map(|id| {
            let n = hierarchy.node(NodeId::new(id)).unwrap();
            let bounds = if id == 2 {
                SpatialBounds::new([8.0, 0.0, 0.0], [12.0, 4.0, 4.0]).unwrap()
            } else {
                n.bounds()
            };
            SpatialNode::new(
                n.id(),
                n.parent(),
                n.level(),
                bounds,
                n.point_count(),
                n.geometric_error(),
                n.blocks().to_vec(),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    nodes.push(
        SpatialNode::new(
            NodeId::new(4),
            Some(NodeId::new(2)),
            2,
            SpatialBounds::new([8.0, 0.0, 0.0], [10.0, 2.0, 2.0]).unwrap(),
            0,
            0.0,
            vec![],
        )
        .unwrap(),
    );
    let hierarchy =
        SpatialHierarchy::new_with_semantics(nodes, vec![], None, HierarchySemantics::Replacement)
            .unwrap();
    cache.insert(PointBlock::new(
        BlockId::new(2),
        PointBatch::new(vec![
            Point::new([8.25, 0.25, 0.25], ColorRgb::new(20, 30, 40)),
            Point::new([8.75, 0.25, 0.25], ColorRgb::new(20, 30, 40)),
        ]),
        vec![],
    ));
    let scene = build_voxel_scene_with_limits(
        &hierarchy,
        &[NodeId::new(1), NodeId::new(2)],
        &cache,
        [0.0; 3],
        1.0,
        &project,
        VoxelBuildLimits::default(),
    )
    .unwrap();
    assert_eq!(scene.scene.instance_count(), 3);
    assert!(
        scene
            .scene
            .instances()
            .iter()
            .filter(|v| v.center[0] >= 8.0)
            .all(|v| (v.size - 0.5).abs() < 2e-6)
    );
    assert_disjoint(&scene.scene);
}

#[test]
fn culling_a_coarse_region_does_not_remove_the_fine_sibling() {
    let (hierarchy, cache) = fixture(HierarchySemantics::Replacement, [0.0; 3], false);
    let scene = build_voxel_scene_with_limits(
        &hierarchy,
        &[NodeId::new(1), NodeId::new(2)],
        &cache,
        [0.0; 3],
        1.0,
        &|b| if b.max()[0] <= 8.0 { 0.0 } else { project(b) },
        VoxelBuildLimits::default(),
    )
    .unwrap();
    assert_eq!(scene.scene.instance_count(), 2);
    assert!(
        scene
            .scene
            .instances()
            .iter()
            .all(|v| v.size < 0.5 && v.center[0] > 8.0)
    );
    assert_disjoint(&scene.scene);
}

#[test]
fn more_than_eight_regions_in_an_external_coarse_payload_uses_global_fallback() {
    let (hierarchy, mut cache) = fixture(HierarchySemantics::Replacement, [0.0; 3], false);
    let nodes = (0..4)
        .map(|id| {
            let n = hierarchy.node(NodeId::new(id)).unwrap();
            let bounds = if id == 1 {
                SpatialBounds::new([0.0; 3], [4.0; 3]).unwrap()
            } else {
                n.bounds()
            };
            SpatialNode::new(
                n.id(),
                n.parent(),
                n.level(),
                bounds,
                n.point_count(),
                n.geometric_error(),
                n.blocks().to_vec(),
            )
            .unwrap()
        })
        .collect();
    let hierarchy =
        SpatialHierarchy::new_with_semantics(nodes, vec![], None, HierarchySemantics::Replacement)
            .unwrap();
    // Deliberately outside the metadata bounds: nine different depth-2 regions.
    let points = (0..9)
        .map(|i| {
            Point::new(
                [
                    0.625 + 4.0 * (i % 4) as f32,
                    0.625 + 4.0 * (i / 4) as f32,
                    0.625,
                ],
                ColorRgb::new(20, 30, 40),
            )
        })
        .collect();
    cache.insert(PointBlock::new(
        BlockId::new(1),
        PointBatch::new(points),
        vec![],
    ));
    let scene = build_voxel_scene_with_limits(
        &hierarchy,
        &[NodeId::new(1), NodeId::new(2)],
        &cache,
        [0.0; 3],
        1.0,
        &project,
        VoxelBuildLimits::default(),
    )
    .unwrap();
    assert_eq!(scene.scene.instance_count(), 10);
    assert!(
        scene
            .scene
            .instances()
            .iter()
            .all(|v| (v.size - 0.5).abs() < 2e-6)
    );
    assert_disjoint(&scene.scene);
}

#[test]
fn independent_fine_cells_retain_their_rgb_and_scalar_attributes() {
    use viewer_core::{AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues};
    let (hierarchy, mut cache) = fixture(HierarchySemantics::Replacement, [0.0; 3], false);
    let blue = ColorRgb::new(0, 0, 255);
    let green = ColorRgb::new(0, 255, 0);
    cache.insert(PointBlock::new(
        BlockId::new(2),
        PointBatch::new(vec![
            Point::new([8.125, 0.125, 0.125], blue),
            Point::new([8.375, 0.125, 0.125], green),
        ]),
        vec![
            AttributeColumn::new(
                AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap(),
                AttributeValues::U16(vec![10, 30]),
                2,
            )
            .unwrap(),
        ],
    ));
    let scene = build_voxel_scene_with_limits(
        &hierarchy,
        &[NodeId::new(1), NodeId::new(2)],
        &cache,
        [0.0; 3],
        1.0,
        &project,
        VoxelBuildLimits::default(),
    )
    .unwrap();
    let fine = scene
        .scene
        .instances()
        .iter()
        .filter(|v| v.center[0] > 8.0)
        .collect::<Vec<_>>();
    assert_eq!(fine.len(), 2);
    assert!(
        fine.iter()
            .any(|v| v.color == blue.to_linear_rgba() && v.intensity == 10.0)
    );
    assert!(
        fine.iter()
            .any(|v| v.color == green.to_linear_rgba() && v.intensity == 30.0)
    );
}
