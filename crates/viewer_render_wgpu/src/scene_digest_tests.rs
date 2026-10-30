//! Golden digests of the voxel instance buffer across fixtures, cameras and target pixels.
//!
//! These pin the exact bytes the CPU build produces, so moving view math (the `ViewProjection`
//! seam) or the build itself to another thread cannot change a scene unnoticed.

use viewer_core::{
    BlockId, ColorRgb, DecodedBlockCache, HierarchySemantics, NodeId, Point, PointBatch,
    PointBlock, SpatialBounds, SpatialHierarchy, SpatialNode,
};

use crate::{
    OrbitCamera, SceneBounds, ViewProjection, VoxelBuildLimits, build_voxel_scene_with_limits,
};

const ORIGIN: [f64; 3] = [1000.0, -2000.0, 50.0];
const SIDE: f64 = 64.0;
const VIEWPORT: [u32; 2] = [800, 600];
const TARGET_PIXELS: [f64; 3] = [2.0, 4.0, 8.0];

struct Lcg(u64);

impl Lcg {
    fn next_unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / ((1_u64 << 24) as f32)
    }
}

fn octant_bounds(index: usize) -> ([f64; 3], [f64; 3]) {
    let half = SIDE / 2.0;
    let min = std::array::from_fn(|axis| ORIGIN[axis] + half * ((index >> axis) & 1) as f64);
    (min, min.map(|value| value + half))
}

/// Points are local to `ORIGIN`, like decoded blocks.
fn points_in(rng: &mut Lcg, lo: [f64; 3], size: f64, count: usize) -> Vec<Point> {
    (0..count)
        .map(|_| {
            let position =
                std::array::from_fn(|axis| (lo[axis] + f64::from(rng.next_unit()) * size) as f32);
            let color = ColorRgb::new(
                (rng.next_unit() * 255.0) as u8,
                (rng.next_unit() * 255.0) as u8,
                (rng.next_unit() * 255.0) as u8,
            );
            Point::new(position, color)
        })
        .collect()
}

struct Fixture {
    hierarchy: SpatialHierarchy,
    cache: DecodedBlockCache,
}

/// Root (id 1) plus eight octant children (ids 2..=9), each node owning the block with its id.
fn fixture(semantics: HierarchySemantics) -> Fixture {
    let mut rng = Lcg(0x5eed_0001);
    let mut blocks = Vec::new();
    let mut child_points = Vec::new();
    for index in 0..8 {
        let (min, _) = octant_bounds(index);
        let local = std::array::from_fn(|axis| min[axis] - ORIGIN[axis]);
        let points = points_in(&mut rng, local, SIDE / 2.0, 300);
        child_points.push(points);
    }
    let root_points = match semantics {
        // A coarse stand-in: every fourth child point.
        HierarchySemantics::Replacement => child_points
            .iter()
            .flatten()
            .step_by(4)
            .copied()
            .collect::<Vec<_>>(),
        _ => points_in(&mut rng, [0.0; 3], SIDE, 300),
    };
    let root_count = root_points.len() as u64;
    let mut nodes = Vec::new();
    let root_bounds = SpatialBounds::new(ORIGIN, ORIGIN.map(|value| value + SIDE)).unwrap();
    nodes.push(
        SpatialNode::new(
            NodeId::new(1),
            None,
            0,
            root_bounds,
            root_count,
            0.0,
            vec![BlockId::new(1)],
        )
        .unwrap(),
    );
    blocks.push(PointBlock::new(
        BlockId::new(1),
        PointBatch::new(root_points),
        vec![],
    ));
    let mut total = root_count;
    for (index, points) in child_points.into_iter().enumerate() {
        let id = index as u64 + 2;
        let (min, max) = octant_bounds(index);
        total += points.len() as u64;
        nodes.push(
            SpatialNode::new(
                NodeId::new(id),
                Some(NodeId::new(1)),
                1,
                SpatialBounds::new(min, max).unwrap(),
                points.len() as u64,
                0.0,
                vec![BlockId::new(id)],
            )
            .unwrap(),
        );
        blocks.push(PointBlock::new(
            BlockId::new(id),
            PointBatch::new(points),
            vec![],
        ));
    }
    let total = if semantics == HierarchySemantics::Replacement {
        total - root_count
    } else {
        total
    };
    let hierarchy =
        SpatialHierarchy::new_with_semantics(nodes, vec![], Some(total), semantics).unwrap();
    let mut cache = DecodedBlockCache::new(64 * 1024 * 1024);
    for block in blocks {
        cache.insert(block);
    }
    Fixture { hierarchy, cache }
}

fn ids(values: &[u64]) -> Vec<NodeId> {
    values.iter().copied().map(NodeId::new).collect()
}

fn cameras() -> Vec<(&'static str, OrbitCamera)> {
    let bounds = SceneBounds {
        min: [0.0; 3],
        max: [SIDE; 3],
    };
    let fitted = || {
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(bounds, VIEWPORT));
        camera
    };
    let mut orbited = fitted();
    orbited.orbit(0.2, 0.1);
    let mut zoomed = fitted();
    zoomed.zoom(-0.7);
    let mut panned = fitted();
    panned.orbit(-0.1, -0.05);
    panned.pan(0.1, -0.05, VIEWPORT);
    vec![
        ("fit", fitted()),
        ("orbit", orbited),
        ("zoom", zoomed),
        ("pan", panned),
    ]
}

/// How a test turns a camera into the frustum and projected-edge functions of the build.
type Build<'a> = &'a dyn Fn(&Fixture, &[NodeId], &OrbitCamera, f64) -> (usize, u64);

/// The build exactly as the LOD update drives it: frustum-gated projected edge, scale 1.
fn digest(
    fixture: &Fixture,
    cut: &[NodeId],
    camera: &OrbitCamera,
    target_pixels: f64,
) -> (usize, u64) {
    let frustum = camera.spatial_frustum(VIEWPORT).expect("frustum");
    let scene = build_voxel_scene_with_limits(
        &fixture.hierarchy,
        cut,
        &fixture.cache,
        ORIGIN,
        target_pixels,
        &|bounds| {
            if frustum.intersects(&bounds) {
                camera.projected_voxel_edge_pixels(bounds, [0.0; 3], VIEWPORT)
            } else {
                0.0
            }
        },
        crate::VoxelBuildLimits::default(),
    )
    .expect("scene");
    (scene.scene.instance_count(), scene.scene.scene_digest())
}

/// The same build through the GPU-free `ViewProjection`, as `LodDataset` drives it.
fn digest_via_view(
    fixture: &Fixture,
    cut: &[NodeId],
    camera: &OrbitCamera,
    target_pixels: f64,
) -> (usize, u64) {
    let view = ViewProjection::new(*camera, VIEWPORT, 1.0, VoxelBuildLimits::default());
    let local_frustum = view.spatial_frustum([0.0; 3]).expect("frustum");
    let scene = build_voxel_scene_with_limits(
        &fixture.hierarchy,
        cut,
        &fixture.cache,
        ORIGIN,
        target_pixels,
        &|bounds| {
            if local_frustum.intersects(&bounds) {
                view.projected_voxel_edge_pixels(bounds, [0.0; 3])
            } else {
                0.0
            }
        },
        view.voxel_build_limits(),
    )
    .expect("scene");
    (scene.scene.instance_count(), scene.scene.scene_digest())
}

fn all_cases(build: Build<'_>) -> Vec<(String, usize, u64)> {
    let replacement = fixture(HierarchySemantics::Replacement);
    let additive = fixture(HierarchySemantics::Additive);
    let cuts: Vec<(&str, &Fixture, Vec<NodeId>)> = vec![
        ("replacement_root", &replacement, ids(&[1])),
        (
            "replacement_children",
            &replacement,
            ids(&[2, 3, 4, 5, 6, 7, 8, 9]),
        ),
        ("replacement_mixed", &replacement, ids(&[1, 2, 3])),
        ("additive_all", &additive, ids(&[1, 2, 3, 4, 5, 6, 7, 8, 9])),
    ];
    let mut cases = Vec::new();
    for (cut_name, fixture, cut) in &cuts {
        for (camera_name, camera) in cameras() {
            for target in TARGET_PIXELS {
                let (count, hash) = build(fixture, cut, &camera, target);
                cases.push((format!("{cut_name}/{camera_name}/{target}px"), count, hash));
            }
        }
    }
    cases
}

const GOLDEN: &[(&str, usize, u64)] = &[
    ("replacement_root/fit/2px", 333, 0x952a70b7b86db135),
    ("replacement_root/fit/4px", 333, 0x952a70b7b86db135),
    ("replacement_root/fit/8px", 333, 0x952a70b7b86db135),
    ("replacement_root/orbit/2px", 333, 0x952a70b7b86db135),
    ("replacement_root/orbit/4px", 333, 0x952a70b7b86db135),
    ("replacement_root/orbit/8px", 333, 0x952a70b7b86db135),
    ("replacement_root/zoom/2px", 333, 0x952a70b7b86db135),
    ("replacement_root/zoom/4px", 333, 0x952a70b7b86db135),
    ("replacement_root/zoom/8px", 333, 0x952a70b7b86db135),
    ("replacement_root/pan/2px", 333, 0x952a70b7b86db135),
    ("replacement_root/pan/4px", 333, 0x952a70b7b86db135),
    ("replacement_root/pan/8px", 333, 0x952a70b7b86db135),
    ("replacement_children/fit/2px", 2400, 0xa9f09c495b4294fd),
    ("replacement_children/fit/4px", 2400, 0x35e5da5811e8a315),
    ("replacement_children/fit/8px", 2387, 0x5feda9acd14635b5),
    ("replacement_children/orbit/2px", 2400, 0x826b89046d8e6865),
    ("replacement_children/orbit/4px", 2400, 0x76e80791fed2e67e),
    ("replacement_children/orbit/8px", 2388, 0xe9f0bdbd05e199eb),
    ("replacement_children/zoom/2px", 2398, 0xeda660caf51655cd),
    ("replacement_children/zoom/4px", 2398, 0x711c6fbcc92f2fca),
    ("replacement_children/zoom/8px", 2398, 0x7b44455bc54f81d0),
    ("replacement_children/pan/2px", 2400, 0xaa8cfea6979132f4),
    ("replacement_children/pan/4px", 2400, 0x984a35ea41011b10),
    ("replacement_children/pan/8px", 2387, 0x17f6c776bce80043),
    ("replacement_mixed/fit/2px", 846, 0x550ff6b065f12718),
    ("replacement_mixed/fit/4px", 846, 0x332a3e3cbc98d5ce),
    ("replacement_mixed/fit/8px", 842, 0x50691b4c260223ce),
    ("replacement_mixed/orbit/2px", 846, 0x550ff6b065f12718),
    ("replacement_mixed/orbit/4px", 846, 0x332a3e3cbc98d5ce),
    ("replacement_mixed/orbit/8px", 842, 0x50691b4c260223ce),
    ("replacement_mixed/zoom/2px", 846, 0x6bc512c46de19a2b),
    ("replacement_mixed/zoom/4px", 846, 0x54b27d0f22dd62c0),
    ("replacement_mixed/zoom/8px", 846, 0xe823b13e81068b41),
    ("replacement_mixed/pan/2px", 846, 0xdb7e4915acb9e174),
    ("replacement_mixed/pan/4px", 846, 0x5f5991f58b7f9aa9),
    ("replacement_mixed/pan/8px", 842, 0xdd60113864fc607a),
    ("additive_all/fit/2px", 2700, 0xa2fcede8a7ec5a53),
    ("additive_all/fit/4px", 2700, 0x0af02dd19318bf7a),
    ("additive_all/fit/8px", 2684, 0x38572e821d00089d),
    ("additive_all/orbit/2px", 2700, 0x65dc1bbb04bf0eef),
    ("additive_all/orbit/4px", 2700, 0xb7c113f3867701f9),
    ("additive_all/orbit/8px", 2685, 0x8792b7f2468b5a53),
    ("additive_all/zoom/2px", 2698, 0xcc9ebddf4033b5c2),
    ("additive_all/zoom/4px", 2698, 0xd4b6febfd4166fa3),
    ("additive_all/zoom/8px", 2698, 0xba2d5842415ca184),
    ("additive_all/pan/2px", 2700, 0x76a8b0f8f68ee5d0),
    ("additive_all/pan/4px", 2700, 0xc4bcaf5991c5492d),
    ("additive_all/pan/8px", 2684, 0xaad18cf742ff05ba),
];

#[test]
fn voxel_scene_digests_match_the_golden_table() {
    check_golden(&all_cases(&digest));
}

#[test]
fn view_projection_reproduces_the_golden_digests() {
    check_golden(&all_cases(&digest_via_view));
}

fn check_golden(actual: &[(String, usize, u64)]) {
    let matches = actual.len() == GOLDEN.len()
        && actual
            .iter()
            .zip(GOLDEN)
            .all(|(a, g)| a.0 == g.0 && a.1 == g.1 && a.2 == g.2);
    if !matches {
        let table: String = actual
            .iter()
            .map(|(name, count, hash)| format!("    (\"{name}\", {count}, {hash:#018x}),\n"))
            .collect();
        panic!("digest table changed; actual:\n{table}");
    }
    assert!(
        actual.iter().filter(|case| case.1 > 0).count() >= actual.len() * 3 / 4,
        "most cases must render voxels"
    );
}

#[test]
fn digest_depends_on_count_order_and_content() {
    let replacement = fixture(HierarchySemantics::Replacement);
    let camera = cameras().remove(0).1;
    let a = digest(&replacement, &ids(&[1]), &camera, 4.0);
    let b = digest(&replacement, &ids(&[2, 3, 4, 5, 6, 7, 8, 9]), &camera, 4.0);
    assert_ne!(a.1, b.1);
    assert_eq!(a, digest(&replacement, &ids(&[1]), &camera, 4.0));
}
