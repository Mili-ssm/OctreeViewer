//! Golden digests of the voxel instance buffer across fixtures, cameras and target pixels.
//!
//! These pin the exact bytes the CPU build produces, so moving view math (the `ViewProjection`
//! seam) or the build itself to another thread cannot change a scene unnoticed.

use viewer_core::{
    BlockId, ColorRgb, DecodedBlockCache, HierarchySemantics, NodeId, Point, PointBatch,
    PointBlock, SpatialBounds, SpatialHierarchy, SpatialNode,
};

use crate::{OrbitCamera, ViewProjection, VoxelBuildLimits, build_voxel_scene_with_limits};

// The importers of these fixtures read them here; they live in `test_support`, which the wasm
// tests reach too.
pub(super) use crate::test_support::{ORIGIN, SIDE, VIEWPORT, cameras};

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

pub(super) struct Fixture {
    pub(super) hierarchy: SpatialHierarchy,
    pub(super) cache: DecodedBlockCache,
}

/// Root (id 1) plus eight octant children (ids 2..=9), each node owning the block with its id.
pub(super) fn fixture(semantics: HierarchySemantics) -> Fixture {
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
        HierarchySemantics::Additive => points_in(&mut rng, [0.0; 3], SIDE, 300),
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

pub(super) fn ids(values: &[u64]) -> Vec<NodeId> {
    values.iter().copied().map(NodeId::new).collect()
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
    ("replacement_root/fit/2px", 333, 0x952a_70b7_b86d_b135),
    ("replacement_root/fit/4px", 333, 0x952a_70b7_b86d_b135),
    ("replacement_root/fit/8px", 333, 0x952a_70b7_b86d_b135),
    ("replacement_root/orbit/2px", 333, 0x952a_70b7_b86d_b135),
    ("replacement_root/orbit/4px", 333, 0x952a_70b7_b86d_b135),
    ("replacement_root/orbit/8px", 333, 0x952a_70b7_b86d_b135),
    ("replacement_root/zoom/2px", 333, 0x952a_70b7_b86d_b135),
    ("replacement_root/zoom/4px", 333, 0x952a_70b7_b86d_b135),
    ("replacement_root/zoom/8px", 333, 0x952a_70b7_b86d_b135),
    ("replacement_root/pan/2px", 333, 0x952a_70b7_b86d_b135),
    ("replacement_root/pan/4px", 333, 0x952a_70b7_b86d_b135),
    ("replacement_root/pan/8px", 333, 0x952a_70b7_b86d_b135),
    ("replacement_children/fit/2px", 2400, 0xa9f0_9c49_5b42_94fd),
    ("replacement_children/fit/4px", 2400, 0x35e5_da58_11e8_a315),
    ("replacement_children/fit/8px", 2387, 0x5fed_a9ac_d146_35b5),
    (
        "replacement_children/orbit/2px",
        2400,
        0x826b_8904_6d8e_6865,
    ),
    (
        "replacement_children/orbit/4px",
        2400,
        0x76e8_0791_fed2_e67e,
    ),
    (
        "replacement_children/orbit/8px",
        2388,
        0xe9f0_bdbd_05e1_99eb,
    ),
    ("replacement_children/zoom/2px", 2398, 0xeda6_60ca_f516_55cd),
    ("replacement_children/zoom/4px", 2398, 0x711c_6fbc_c92f_2fca),
    ("replacement_children/zoom/8px", 2398, 0x7b44_455b_c54f_81d0),
    ("replacement_children/pan/2px", 2400, 0xaa8c_fea6_9791_32f4),
    ("replacement_children/pan/4px", 2400, 0x984a_35ea_4101_1b10),
    ("replacement_children/pan/8px", 2387, 0x17f6_c776_bce8_0043),
    ("replacement_mixed/fit/2px", 846, 0x550f_f6b0_65f1_2718),
    ("replacement_mixed/fit/4px", 846, 0x332a_3e3c_bc98_d5ce),
    ("replacement_mixed/fit/8px", 842, 0x5069_1b4c_2602_23ce),
    ("replacement_mixed/orbit/2px", 846, 0x550f_f6b0_65f1_2718),
    ("replacement_mixed/orbit/4px", 846, 0x332a_3e3c_bc98_d5ce),
    ("replacement_mixed/orbit/8px", 842, 0x5069_1b4c_2602_23ce),
    ("replacement_mixed/zoom/2px", 846, 0x6bc5_12c4_6de1_9a2b),
    ("replacement_mixed/zoom/4px", 846, 0x54b2_7d0f_22dd_62c0),
    ("replacement_mixed/zoom/8px", 846, 0xe823_b13e_8106_8b41),
    ("replacement_mixed/pan/2px", 846, 0xdb7e_4915_acb9_e174),
    ("replacement_mixed/pan/4px", 846, 0x5f59_91f5_8b7f_9aa9),
    ("replacement_mixed/pan/8px", 842, 0xdd60_1138_64fc_607a),
    ("additive_all/fit/2px", 2700, 0xa2fc_ede8_a7ec_5a53),
    ("additive_all/fit/4px", 2700, 0x0af0_2dd1_9318_bf7a),
    ("additive_all/fit/8px", 2684, 0x3857_2e82_1d00_089d),
    ("additive_all/orbit/2px", 2700, 0x65dc_1bbb_04bf_0eef),
    ("additive_all/orbit/4px", 2700, 0xb7c1_13f3_8677_01f9),
    ("additive_all/orbit/8px", 2685, 0x8792_b7f2_468b_5a53),
    ("additive_all/zoom/2px", 2698, 0xcc9e_bddf_4033_b5c2),
    ("additive_all/zoom/4px", 2698, 0xd4b6_febf_d416_6fa3),
    ("additive_all/zoom/8px", 2698, 0xba2d_5842_415c_a184),
    ("additive_all/pan/2px", 2700, 0x76a8_b0f8_f68e_e5d0),
    ("additive_all/pan/4px", 2700, 0xc4bc_af59_91c5_492d),
    ("additive_all/pan/8px", 2684, 0xaad1_8cf7_42ff_05ba),
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
        use std::fmt::Write as _;
        let table: String = actual
            .iter()
            .fold(String::new(), |mut table, (name, count, hash)| {
                let _ = writeln!(table, "    (\"{name}\", {count}, {hash:#018x}),");
                table
            });
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
