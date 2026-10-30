//! Golden characterization of LOD selection and transition order.
//!
//! Pins the exact `node_ids` / pending-block order (not set equality) of
//! `select_lod`, `select_lod_stable_projector` (priority path and both fallback
//! causes) and `LodTransitionPlanner` on deterministic synthetic hierarchies.
//! Outputs are recorded as FNV-1a hashes captured from the pre-refactor code.
//! Run with `GOLDEN_DUMP=1 -- --nocapture` to print the current hashes.
//! The 10^6-node cases and the timing benches are `#[ignore]`d:
//! `cargo test -p viewer_core --profile fast --test lod_golden -- --ignored --nocapture`.
use std::time::Instant;

use viewer_core::{
    BlockId, ColorRgb, DEFAULT_LOD_PRIORITY_BYTES, DecodedBlockCache, FrustumPlane,
    HierarchySemantics, LodPriorityFallback, LodSelection, LodSelectionConfig, LodTransitionPlan,
    LodTransitionPlanner, NodeId, Point, PointBatch, PointBlock, SpatialBounds, SpatialFrustum,
    SpatialHierarchy, SpatialNode,
};

/// Captured from the code before the frontier refactor (6d67a1d).
const EXPECTED: &[(&str, u64)] = &[
    ("select/2k/Replacement/overview", 0x7c05ec4a61ea8f52),
    ("select/2k/Replacement/closeup", 0xb6236e16a797a0af),
    ("select/2k/Replacement/oblique", 0x79895c069b6b25be),
    ("select/2k/Replacement/ties", 0x02fe4ae133121b09),
    ("select/2k/Additive/overview", 0x4d5cf13d7e116447),
    ("select/2k/Additive/closeup", 0xb47ec349837f3cee),
    ("select/2k/Additive/oblique", 0xcb5d2fadeae1b2df),
    ("select/2k/Additive/ties", 0x16f9b85bbb97eba9),
    (
        "transition/2k/Replacement/overview-closeup/initial",
        0xbcdb218b51925bf5,
    ),
    (
        "transition/2k/Replacement/overview-closeup/waiting",
        0xd44033329b70966f,
    ),
    (
        "transition/2k/Replacement/overview-closeup/waiting-commit",
        0xd44033329b70966f,
    ),
    (
        "transition/2k/Replacement/overview-closeup/done",
        0x8a07a85f1e4040b3,
    ),
    (
        "transition/2k/Replacement/overview-closeup/tight",
        0xd528faaa788ac9ee,
    ),
    (
        "transition/2k/Replacement/closeup-oblique/initial",
        0x8a07a85f1e4040b3,
    ),
    (
        "transition/2k/Replacement/closeup-oblique/waiting",
        0x0317704156ec8285,
    ),
    (
        "transition/2k/Replacement/closeup-oblique/waiting-commit",
        0x0317704156ec8285,
    ),
    (
        "transition/2k/Replacement/closeup-oblique/done",
        0x3e4ca7a8c99f0f2f,
    ),
    (
        "transition/2k/Replacement/closeup-oblique/tight",
        0xa007dd3fdd52e8d5,
    ),
    (
        "transition/2k/Replacement/oblique-overview/initial",
        0x3e4ca7a8c99f0f2f,
    ),
    (
        "transition/2k/Replacement/oblique-overview/waiting",
        0x8e1c5fab3ed8c335,
    ),
    (
        "transition/2k/Replacement/oblique-overview/waiting-commit",
        0x8e1c5fab3ed8c335,
    ),
    (
        "transition/2k/Replacement/oblique-overview/done",
        0xbcdb218b51925bf5,
    ),
    (
        "transition/2k/Replacement/oblique-overview/tight",
        0x6d0a8cd1e6d4d78b,
    ),
    (
        "transition/2k/Replacement/ties-closeup/initial",
        0xa04cfc686e146d22,
    ),
    (
        "transition/2k/Replacement/ties-closeup/waiting",
        0x152b82effcce9b30,
    ),
    (
        "transition/2k/Replacement/ties-closeup/waiting-commit",
        0x152b82effcce9b30,
    ),
    (
        "transition/2k/Replacement/ties-closeup/done",
        0x8a07a85f1e4040b3,
    ),
    (
        "transition/2k/Replacement/ties-closeup/tight",
        0x7779e89f13657512,
    ),
    (
        "transition/2k/Additive/overview-closeup/initial",
        0x467095b91580013b,
    ),
    (
        "transition/2k/Additive/overview-closeup/waiting",
        0x538868a999e2569f,
    ),
    (
        "transition/2k/Additive/overview-closeup/waiting-commit",
        0x538868a999e2569f,
    ),
    (
        "transition/2k/Additive/overview-closeup/done",
        0xc8a696270a304237,
    ),
    (
        "transition/2k/Additive/overview-closeup/tight",
        0x51a734080f83911c,
    ),
    (
        "transition/2k/Additive/closeup-oblique/initial",
        0xc8a696270a304237,
    ),
    (
        "transition/2k/Additive/closeup-oblique/waiting",
        0x8316f3c165f26dab,
    ),
    (
        "transition/2k/Additive/closeup-oblique/waiting-commit",
        0x8316f3c165f26dab,
    ),
    (
        "transition/2k/Additive/closeup-oblique/done",
        0xe9d57b9456edb5fd,
    ),
    (
        "transition/2k/Additive/closeup-oblique/tight",
        0xa21960a8d4c14fb2,
    ),
    (
        "transition/2k/Additive/oblique-overview/initial",
        0xe9d57b9456edb5fd,
    ),
    (
        "transition/2k/Additive/oblique-overview/waiting",
        0x5311cf594ebf6008,
    ),
    (
        "transition/2k/Additive/oblique-overview/waiting-commit",
        0x5311cf594ebf6008,
    ),
    (
        "transition/2k/Additive/oblique-overview/done",
        0x467095b91580013b,
    ),
    (
        "transition/2k/Additive/oblique-overview/tight",
        0x0967b7f9c17cbb53,
    ),
    (
        "transition/2k/Additive/ties-closeup/initial",
        0x1e842aaccb5c8f2e,
    ),
    (
        "transition/2k/Additive/ties-closeup/waiting",
        0x527f0279aeb0c3b3,
    ),
    (
        "transition/2k/Additive/ties-closeup/waiting-commit",
        0x527f0279aeb0c3b3,
    ),
    (
        "transition/2k/Additive/ties-closeup/done",
        0xc8a696270a304237,
    ),
    (
        "transition/2k/Additive/ties-closeup/tight",
        0x51a734080f83911c,
    ),
    ("select/100k/Replacement/overview", 0x3fbe3ab1edd8bea1),
    ("select/100k/Replacement/closeup", 0xde88a6e00dad46db),
    ("select/100k/Replacement/oblique", 0xd1885ac2448d0b8d),
    ("select/100k/Replacement/ties", 0xbbed16de1225327b),
    ("select/1m/Replacement/overview", 0x3fbe3ab1edd8bea1),
    ("select/100k/Additive/overview", 0xfdbd331cb37a8c07),
    ("select/100k/Additive/closeup", 0x9468c5519a444dc2),
    ("select/1m/Replacement/closeup", 0x6fbdd97de89c251b),
    ("select/100k/Additive/oblique", 0xd3338791a35c013a),
    ("select/100k/Additive/ties", 0xae85aa599e3cca2b),
    ("select/1m/Replacement/oblique", 0xd1885ac2448d0b8d),
    (
        "transition/100k/Replacement/overview-closeup/initial",
        0x823394fb6711cc5f,
    ),
    (
        "transition/100k/Replacement/overview-closeup/waiting",
        0x57801523b6b1c350,
    ),
    (
        "transition/100k/Replacement/overview-closeup/waiting-commit",
        0x57801523b6b1c350,
    ),
    (
        "transition/100k/Replacement/overview-closeup/done",
        0x35600c8912bf01fb,
    ),
    ("select/1m/Replacement/ties", 0xbbed16de1225327b),
    (
        "transition/100k/Replacement/overview-closeup/tight",
        0x11de1cf4c1c4791d,
    ),
    (
        "transition/100k/Replacement/closeup-oblique/initial",
        0x35600c8912bf01fb,
    ),
    (
        "transition/100k/Replacement/closeup-oblique/waiting",
        0x479e2efaba7d785d,
    ),
    (
        "transition/100k/Replacement/closeup-oblique/waiting-commit",
        0x479e2efaba7d785d,
    ),
    (
        "transition/100k/Replacement/closeup-oblique/done",
        0xafcbc155b4cfe79f,
    ),
    (
        "transition/100k/Replacement/closeup-oblique/tight",
        0x54f53119ba52b448,
    ),
    (
        "transition/100k/Replacement/oblique-overview/initial",
        0xafcbc155b4cfe79f,
    ),
    (
        "transition/100k/Replacement/oblique-overview/waiting",
        0x5605ae0c17523a41,
    ),
    (
        "transition/100k/Replacement/oblique-overview/waiting-commit",
        0x5605ae0c17523a41,
    ),
    (
        "transition/100k/Replacement/oblique-overview/done",
        0x823394fb6711cc5f,
    ),
    (
        "transition/100k/Replacement/oblique-overview/tight",
        0x03040426e0ed6ce1,
    ),
    (
        "transition/100k/Replacement/ties-closeup/initial",
        0x9636a34164778977,
    ),
    (
        "transition/100k/Replacement/ties-closeup/waiting",
        0x542756993df8cbda,
    ),
    (
        "transition/100k/Replacement/ties-closeup/waiting-commit",
        0x542756993df8cbda,
    ),
    (
        "transition/100k/Replacement/ties-closeup/done",
        0x35600c8912bf01fb,
    ),
    ("select/1m/Additive/overview", 0xfdbd331cb37a8c07),
    (
        "transition/100k/Replacement/ties-closeup/tight",
        0x20992c4dd3c8a0f3,
    ),
    ("select/1m/Additive/closeup", 0x6a6e1de1373571bf),
    ("select/1m/Additive/oblique", 0xd3338791a35c013a),
    ("select/1m/Additive/ties", 0xae85aa599e3cca2b),
    (
        "transition/100k/Additive/overview-closeup/initial",
        0x8f7ccc90bc2ad8db,
    ),
    (
        "transition/100k/Additive/overview-closeup/waiting",
        0x399f574a57a789aa,
    ),
    (
        "transition/100k/Additive/overview-closeup/waiting-commit",
        0x399f574a57a789aa,
    ),
    (
        "transition/100k/Additive/overview-closeup/done",
        0xb9ad4121e3ac7133,
    ),
    (
        "transition/100k/Additive/overview-closeup/tight",
        0x7d0fd509cef06dc2,
    ),
    (
        "transition/100k/Additive/closeup-oblique/initial",
        0xb9ad4121e3ac7133,
    ),
    (
        "transition/100k/Additive/closeup-oblique/waiting",
        0x35cfecccb1ace7ee,
    ),
    (
        "transition/100k/Additive/closeup-oblique/waiting-commit",
        0x35cfecccb1ace7ee,
    ),
    (
        "transition/100k/Additive/closeup-oblique/done",
        0x267d250c1899962f,
    ),
    (
        "transition/100k/Additive/closeup-oblique/tight",
        0x22abc8b75cf59e4a,
    ),
    (
        "transition/100k/Additive/oblique-overview/initial",
        0x267d250c1899962f,
    ),
    (
        "transition/100k/Additive/oblique-overview/waiting",
        0xa964a807a23aa414,
    ),
    (
        "transition/100k/Additive/oblique-overview/waiting-commit",
        0xa964a807a23aa414,
    ),
    (
        "transition/100k/Additive/oblique-overview/done",
        0x8f7ccc90bc2ad8db,
    ),
    (
        "transition/100k/Additive/oblique-overview/tight",
        0x0487d4f40db68634,
    ),
    (
        "transition/100k/Additive/ties-closeup/initial",
        0xaf396de96db90f6f,
    ),
    (
        "transition/100k/Additive/ties-closeup/waiting",
        0xa7efe1ea61f952d5,
    ),
    (
        "transition/100k/Additive/ties-closeup/waiting-commit",
        0xa7efe1ea61f952d5,
    ),
    (
        "transition/100k/Additive/ties-closeup/done",
        0xb9ad4121e3ac7133,
    ),
    (
        "transition/100k/Additive/ties-closeup/tight",
        0x7d0fd509cef06dc2,
    ),
    (
        "transition/1m/Replacement/overview-closeup/initial",
        0x823394fb6711cc5f,
    ),
    (
        "transition/1m/Replacement/overview-closeup/waiting",
        0x0452ea6dcfc6e2f8,
    ),
    (
        "transition/1m/Replacement/overview-closeup/waiting-commit",
        0x0452ea6dcfc6e2f8,
    ),
    (
        "transition/1m/Replacement/overview-closeup/done",
        0xb8951dad5a294f88,
    ),
    (
        "transition/1m/Replacement/overview-closeup/tight",
        0x8427a43bca07325d,
    ),
    (
        "transition/1m/Replacement/closeup-oblique/initial",
        0xb8951dad5a294f88,
    ),
    (
        "transition/1m/Replacement/closeup-oblique/waiting",
        0xc2d19f1efaaf7a22,
    ),
    (
        "transition/1m/Replacement/closeup-oblique/waiting-commit",
        0xc2d19f1efaaf7a22,
    ),
    (
        "transition/1m/Replacement/closeup-oblique/done",
        0xafcbc155b4cfe79f,
    ),
    (
        "transition/1m/Replacement/closeup-oblique/tight",
        0x54f53119ba52b448,
    ),
    (
        "transition/1m/Replacement/oblique-overview/initial",
        0xafcbc155b4cfe79f,
    ),
    (
        "transition/1m/Replacement/oblique-overview/waiting",
        0x5605ae0c17523a41,
    ),
    (
        "transition/1m/Replacement/oblique-overview/waiting-commit",
        0x5605ae0c17523a41,
    ),
    (
        "transition/1m/Replacement/oblique-overview/done",
        0x823394fb6711cc5f,
    ),
    (
        "transition/1m/Replacement/oblique-overview/tight",
        0x03040426e0ed6ce1,
    ),
    (
        "transition/1m/Replacement/ties-closeup/initial",
        0x9636a34164778977,
    ),
    (
        "transition/1m/Replacement/ties-closeup/waiting",
        0xe1634a147c56e722,
    ),
    (
        "transition/1m/Replacement/ties-closeup/waiting-commit",
        0xe1634a147c56e722,
    ),
    (
        "transition/1m/Replacement/ties-closeup/done",
        0xb8951dad5a294f88,
    ),
    (
        "transition/1m/Replacement/ties-closeup/tight",
        0x3be6a9abd9dcf9af,
    ),
    (
        "transition/1m/Additive/overview-closeup/initial",
        0x8f7ccc90bc2ad8db,
    ),
    (
        "transition/1m/Additive/overview-closeup/waiting",
        0xa36f494e9bae06a2,
    ),
    (
        "transition/1m/Additive/overview-closeup/waiting-commit",
        0xa36f494e9bae06a2,
    ),
    (
        "transition/1m/Additive/overview-closeup/done",
        0x3401fd7877d65cd7,
    ),
    (
        "transition/1m/Additive/overview-closeup/tight",
        0x4c7b51836a1858f0,
    ),
    (
        "transition/1m/Additive/closeup-oblique/initial",
        0x3401fd7877d65cd7,
    ),
    (
        "transition/1m/Additive/closeup-oblique/waiting",
        0xb0425d9114a8c6fb,
    ),
    (
        "transition/1m/Additive/closeup-oblique/waiting-commit",
        0xb0425d9114a8c6fb,
    ),
    (
        "transition/1m/Additive/closeup-oblique/done",
        0x267d250c1899962f,
    ),
    (
        "transition/1m/Additive/closeup-oblique/tight",
        0x22abc8b75cf59e4a,
    ),
    (
        "transition/1m/Additive/oblique-overview/initial",
        0x267d250c1899962f,
    ),
    (
        "transition/1m/Additive/oblique-overview/waiting",
        0xa964a807a23aa414,
    ),
    (
        "transition/1m/Additive/oblique-overview/waiting-commit",
        0xa964a807a23aa414,
    ),
    (
        "transition/1m/Additive/oblique-overview/done",
        0x8f7ccc90bc2ad8db,
    ),
    (
        "transition/1m/Additive/oblique-overview/tight",
        0x0487d4f40db68634,
    ),
    (
        "transition/1m/Additive/ties-closeup/initial",
        0xaf396de96db90f6f,
    ),
    (
        "transition/1m/Additive/ties-closeup/waiting",
        0x72020fd04fa31d56,
    ),
    (
        "transition/1m/Additive/ties-closeup/waiting-commit",
        0x72020fd04fa31d56,
    ),
    (
        "transition/1m/Additive/ties-closeup/done",
        0x3401fd7877d65cd7,
    ),
    (
        "transition/1m/Additive/ties-closeup/tight",
        0x4c7b51836a1858f0,
    ),
];

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn u64(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    /// Order-sensitive digest of an id sequence, with its length.
    fn ids(&mut self, tag: u64, ids: impl IntoIterator<Item = u64>) {
        let mut count = 0;
        let mut body = Fnv::new();
        for id in ids {
            body.u64(id);
            count += 1;
        }
        self.u64(tag);
        self.u64(count);
        self.u64(body.0);
    }
}

fn check(label: &str, actual: u64) {
    if std::env::var_os("GOLDEN_DUMP").is_some() {
        println!("    (\"{label}\", {actual:#018x}),");
        return;
    }
    let expected = EXPECTED
        .iter()
        .find(|(name, _)| *name == label)
        .unwrap_or_else(|| panic!("no golden hash for {label}: {actual:#018x}"))
        .1;
    assert_eq!(actual, expected, "golden mismatch for {label}");
}

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

const EXTENT: f64 = 1000.0;
const CHAIN: usize = 2000;

struct Spec {
    id: u64,
    parent: Option<u64>,
    level: u32,
    min: [f64; 3],
    max: [f64; 3],
}

/// Deterministic tree: BFS with varied branching (0..=4 children, including empty
/// and single-child nodes), then a deep chain hanging from the last node.
fn synthetic(n: usize, semantics: HierarchySemantics) -> SpatialHierarchy {
    let id_of = |index: usize| index as u64 * 3 + 5;
    let mut specs = vec![Spec {
        id: id_of(0),
        parent: None,
        level: 0,
        min: [0.0; 3],
        max: [EXTENT; 3],
    }];
    let bfs_target = n - CHAIN.min(n / 10);
    let mut cursor = 0;
    while specs.len() < bfs_target {
        let parent = &specs[cursor.min(specs.len() - 1)];
        let mut children = match mix(parent.id) % 10 {
            0 | 1 => 0,
            2 | 3 => 1,
            4..=6 => 2,
            7 | 8 => 3,
            _ => 4,
        };
        if cursor == 0 {
            children = 4;
        }
        // Never run out of open nodes before the target size is reached.
        if cursor + 1 >= specs.len() && children == 0 {
            children = 1;
        }
        let (parent_id, level, min, max) = (parent.id, parent.level, parent.min, parent.max);
        let axis = (level % 3) as usize;
        let width = (max[axis] - min[axis]) / children.max(1) as f64;
        for slot in 0..children {
            if specs.len() >= bfs_target {
                break;
            }
            let mut child_min = min;
            let mut child_max = max;
            child_min[axis] = min[axis] + slot as f64 * width;
            child_max[axis] = if slot + 1 == children {
                max[axis]
            } else {
                min[axis] + (slot + 1) as f64 * width
            };
            specs.push(Spec {
                id: id_of(specs.len()),
                parent: Some(parent_id),
                level: level + 1,
                min: child_min,
                max: child_max,
            });
        }
        cursor += 1;
    }
    while specs.len() < n {
        let last = specs.last().expect("non-empty");
        let next = Spec {
            id: id_of(specs.len()),
            parent: Some(last.id),
            level: last.level + 1,
            min: last.min,
            max: last.max,
        };
        specs.push(next);
    }
    let nodes = specs
        .iter()
        .map(|spec| {
            let roll = mix(spec.id ^ 0xabcd);
            let points = if spec.parent.is_none() || roll % 7 == 0 {
                0
            } else {
                1 + (roll >> 8) % 16
            };
            let error =
                100.0 * 0.75_f64.powi(spec.level.min(60) as i32) * (1.0 + (roll % 5) as f64);
            SpatialNode::new(
                NodeId::new(spec.id),
                spec.parent.map(NodeId::new),
                spec.level,
                SpatialBounds::new(spec.min, spec.max).unwrap(),
                points,
                error,
                if points > 0 {
                    vec![BlockId::new(spec.id)]
                } else {
                    vec![]
                },
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    SpatialHierarchy::new_with_semantics(nodes, Vec::new(), None, semantics).unwrap()
}

#[derive(Clone, Copy, PartialEq)]
enum Camera {
    Overview,
    CloseUp,
    Oblique,
    /// Projection depends only on the level: many exact priority ties.
    LevelTies,
}

const CAMERAS: [(Camera, &str); 4] = [
    (Camera::Overview, "overview"),
    (Camera::CloseUp, "closeup"),
    (Camera::Oblique, "oblique"),
    (Camera::LevelTies, "ties"),
];

fn name(camera: Camera) -> &'static str {
    CAMERAS.iter().find(|(c, _)| *c == camera).unwrap().1
}

fn boxed(lo: [f64; 3], hi: [f64; 3]) -> SpatialFrustum {
    let mut planes = Vec::new();
    for axis in 0..3 {
        let mut up = [0.0; 3];
        up[axis] = 1.0;
        let mut down = [0.0; 3];
        down[axis] = -1.0;
        planes.push(FrustumPlane::new(up, -lo[axis]).unwrap());
        planes.push(FrustumPlane::new(down, hi[axis]).unwrap());
    }
    SpatialFrustum::new(planes).unwrap()
}

fn frustum(camera: Camera) -> SpatialFrustum {
    match camera {
        Camera::Overview | Camera::LevelTies => boxed([0.0; 3], [EXTENT; 3]),
        Camera::CloseUp => boxed([150.0; 3], [450.0; 3]),
        Camera::Oblique => SpatialFrustum::new(vec![
            FrustumPlane::new([1.0, 1.0, 0.5], -700.0).unwrap(),
            FrustumPlane::new([-1.0, 0.2, 0.0], 900.0).unwrap(),
        ])
        .unwrap(),
    }
}

fn project(camera: Camera, node: &SpatialNode) -> f64 {
    let eye = match camera {
        Camera::Overview => [-2000.0, 500.0, 500.0],
        Camera::CloseUp => [300.0; 3],
        Camera::Oblique => [900.0, 100.0, -300.0],
        Camera::LevelTies => return 8.0 / (1.0 + f64::from(node.level().min(12))),
    };
    let bounds = node.bounds();
    let distance = (0..3)
        .map(|axis| {
            let center = (bounds.min()[axis] + bounds.max()[axis]) / 2.0;
            (center - eye[axis]).powi(2)
        })
        .sum::<f64>()
        .sqrt();
    2000.0 * node.geometric_error() / (1.0 + distance)
}

/// Golden budget: the O(F^2) reference selector bounds the cut size we can afford.
fn config(n: usize) -> LodSelectionConfig {
    let budget = if n <= 2_000 { n as u64 / 8 } else { 3_000 };
    LodSelectionConfig::new(0.05, budget).unwrap()
}

/// Bench budget: a larger cut to stress the priority frontier.
fn bench_config(n: usize) -> LodSelectionConfig {
    LodSelectionConfig::new(0.05, n as u64 / 8).unwrap()
}

fn hash_selection(selection: &LodSelection) -> u64 {
    let mut hash = Fnv::new();
    hash.ids(1, selection.node_ids().iter().map(|id| id.get()));
    hash.u64(selection.point_count());
    hash.u64(selection.max_error_pixels().to_bits());
    hash.ids(2, selection.budget_limited().iter().map(|id| id.get()));
    hash.ids(3, selection.resolution_limited().iter().map(|id| id.get()));
    hash.0
}

fn hash_plan(plan: &LodTransitionPlan) -> u64 {
    if std::env::var_os("GOLDEN_DUMP").is_some() {
        println!(
            "    // nodes={} pending={} fallback={} uncovered={} omitted={}",
            plan.node_ids().len(),
            plan.pending_blocks().len(),
            plan.fallback_nodes().len(),
            plan.uncovered_nodes().len(),
            plan.budget_omitted_nodes().len()
        );
    }
    let mut hash = Fnv::new();
    hash.ids(1, plan.node_ids().iter().map(|id| id.get()));
    hash.ids(2, plan.block_ids().iter().map(|id| id.get()));
    hash.ids(3, plan.pending_blocks().iter().map(|id| id.get()));
    hash.ids(4, plan.fallback_nodes().iter().map(|id| id.get()));
    hash.ids(5, plan.uncovered_nodes().iter().map(|id| id.get()));
    hash.ids(6, plan.budget_omitted_nodes().iter().map(|id| id.get()));
    hash.u64(plan.point_count());
    hash.0
}

/// Resident payload with the node's own point count (block id == node id).
fn block(hierarchy: &SpatialHierarchy, id: u64) -> PointBlock {
    let points = hierarchy.node(NodeId::new(id)).unwrap().point_count();
    PointBlock::new(
        BlockId::new(id),
        PointBatch::new(
            (0..points)
                .map(|_| Point::new([0.0; 3], ColorRgb::new(1, 2, 3)))
                .collect(),
        ),
        Vec::new(),
    )
}

fn selection_cases(n: usize, tag: &str) {
    for semantics in [
        HierarchySemantics::Replacement,
        HierarchySemantics::Additive,
    ] {
        let hierarchy = synthetic(n, semantics);
        for (camera, camera_name) in CAMERAS {
            let label = format!("select/{tag}/{semantics:?}/{camera_name}");
            let frustum = frustum(camera);
            let project = |node: &SpatialNode| project(camera, node);
            let reference = hierarchy.select_lod(&frustum, config(n), project).unwrap();
            let hash = hash_selection(&reference);
            check(&label, hash);
            assert!(reference.node_ids().len() > 1, "{label}: degenerate cut");

            // Priority path (default 4 MiB scratch).
            let (priority, stats) = hierarchy
                .select_lod_stable_projector(
                    &frustum,
                    config(n),
                    DEFAULT_LOD_PRIORITY_BYTES,
                    project,
                )
                .unwrap();
            assert_eq!(stats.fallback, None, "{label}: priority path expected");
            assert_eq!(hash_selection(&priority), hash, "{label}: priority path");

            // Fallback path forced by a scratch budget that fits no candidate.
            let (limited, stats) = hierarchy
                .select_lod_stable_projector(&frustum, config(n), 8, project)
                .unwrap();
            assert_eq!(stats.fallback, Some(LodPriorityFallback::ScratchLimit));
            assert_eq!(hash_selection(&limited), hash, "{label}: scratch fallback");
            let (disabled, stats) = hierarchy
                .select_lod_stable_projector(&frustum, config(n), 0, project)
                .unwrap();
            assert_eq!(stats.fallback, Some(LodPriorityFallback::ScratchLimit));
            assert_eq!(hash_selection(&disabled), hash, "{label}: disabled");
        }
    }
}

fn transition_cases(n: usize, tag: &str) {
    for semantics in [
        HierarchySemantics::Replacement,
        HierarchySemantics::Additive,
    ] {
        let hierarchy = synthetic(n, semantics);
        let select = |camera: Camera, config: LodSelectionConfig| {
            hierarchy
                .select_lod(&frustum(camera), config, |node| project(camera, node))
                .unwrap()
        };
        let blocks_of = |selection: &LodSelection| {
            selection
                .node_ids()
                .iter()
                .flat_map(|id| hierarchy.node(*id).unwrap().blocks().iter().copied())
                .collect::<Vec<_>>()
        };
        let pairs = [
            (Camera::Overview, Camera::CloseUp),
            (Camera::CloseUp, Camera::Oblique),
            (Camera::Oblique, Camera::Overview),
            (Camera::LevelTies, Camera::CloseUp),
        ];
        for (from, to) in pairs {
            let start = select(from, config(n));
            let goal = select(to, config(n));
            let mut cache = DecodedBlockCache::new(usize::MAX / 2);
            for block_id in blocks_of(&start) {
                cache.insert(block(&hierarchy, block_id.get()));
            }
            let mut planner = LodTransitionPlanner::new();
            let label = format!("transition/{tag}/{semantics:?}/{}-{}", name(from), name(to));
            let initial = planner.update(&hierarchy, &start, &mut cache);
            check(&format!("{label}/initial"), hash_plan(&initial));

            // Partial residency of the new target: pending blocks and fallbacks.
            for block_id in blocks_of(&goal) {
                if mix(block_id.get()) % 10 < 4 {
                    cache.insert(block(&hierarchy, block_id.get()));
                }
            }
            let waiting = planner.prepare(&hierarchy, &goal, &cache);
            check(&format!("{label}/waiting"), hash_plan(&waiting));
            let waiting = planner.update(&hierarchy, &goal, &mut cache);
            check(&format!("{label}/waiting-commit"), hash_plan(&waiting));

            for block_id in blocks_of(&goal) {
                cache.insert(block(&hierarchy, block_id.get()));
            }
            let done = planner.update(&hierarchy, &goal, &mut cache);
            check(&format!("{label}/done"), hash_plan(&done));

            // A missing block keeps the previous cut resident, which no longer fits
            // the smaller budget: exercises budget-omitted and uncovered bookkeeping.
            let tight = LodSelectionConfig::new(0.05, goal.point_count() / 2 + 1).unwrap();
            let tight_goal = select(to, tight);
            if let Some(missing) = blocks_of(&tight_goal).first() {
                cache.remove(*missing);
            }
            let omitted = planner.prepare(&hierarchy, &tight_goal, &cache);
            check(&format!("{label}/tight"), hash_plan(&omitted));
        }
    }
}

#[test]
fn golden_small_hierarchy() {
    selection_cases(2_000, "2k");
    transition_cases(2_000, "2k");
}

#[test]
fn golden_100k_hierarchy() {
    selection_cases(100_000, "100k");
    transition_cases(100_000, "100k");
}

#[test]
#[ignore = "10^6 nodes; run with --ignored"]
fn golden_1m_hierarchy() {
    selection_cases(1_000_000, "1m");
    transition_cases(1_000_000, "1m");
}

fn bench(n: usize) {
    for semantics in [
        HierarchySemantics::Replacement,
        HierarchySemantics::Additive,
    ] {
        let hierarchy = synthetic(n, semantics);
        for (camera, camera_name) in CAMERAS {
            let frustum = frustum(camera);
            let project = |node: &SpatialNode| project(camera, node);
            let started = Instant::now();
            let (selection, _) = hierarchy
                .select_lod_stable_projector(
                    &frustum,
                    bench_config(n),
                    DEFAULT_LOD_PRIORITY_BYTES,
                    project,
                )
                .unwrap();
            let priority = started.elapsed();
            // The O(F^2) reference is only timed where it finishes in reasonable time.
            let reference = (n <= 100_000).then(|| {
                let started = Instant::now();
                let _ = hierarchy
                    .select_lod(&frustum, bench_config(n), project)
                    .unwrap();
                started.elapsed()
            });

            let mut cache = DecodedBlockCache::new(usize::MAX / 2);
            for id in selection.node_ids() {
                for block_id in hierarchy.node(*id).unwrap().blocks() {
                    if mix(block_id.get()) % 10 < 7 {
                        cache.insert(block(&hierarchy, block_id.get()));
                    }
                }
            }
            let mut planner = LodTransitionPlanner::new();
            planner.update(&hierarchy, &selection, &mut cache);
            let started = Instant::now();
            let plan = planner.prepare(&hierarchy, &selection, &cache);
            let transition = started.elapsed();
            println!(
                "bench n={n} {semantics:?} {camera_name}: cut={} priority={priority:?} select_lod={reference:?} prepare={transition:?} pending={}",
                selection.node_ids().len(),
                plan.pending_blocks().len()
            );
        }
    }
}

#[test]
#[ignore = "timing bench; run with --ignored --nocapture"]
fn bench_100k() {
    bench(100_000);
}

#[test]
#[ignore = "timing bench; run with --ignored --nocapture"]
fn bench_1m() {
    bench(1_000_000);
}
