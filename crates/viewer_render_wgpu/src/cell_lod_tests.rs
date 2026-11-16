//! Phase 2a: the per-cell decision partitions every point and matches the projected edge.

use viewer_core::SpatialBounds;

use super::*;
use crate::SceneBounds;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }

    /// Log-uniform in `[low, high]`.
    fn log_range(&mut self, low: f64, high: f64) -> f64 {
        (low.ln() + (high.ln() - low.ln()) * self.unit()).exp()
    }
}

const VIEWPORT: [u32; 2] = [1280, 720];

fn random_camera(rng: &mut Rng) -> OrbitCamera {
    let radius = rng.log_range(5.0, 2000.0);
    let mut camera = OrbitCamera::default();
    assert!(camera.fit(
        SceneBounds {
            min: [-radius; 3],
            max: [radius; 3],
        },
        VIEWPORT,
    ));
    let target = [0; 3].map(|_| rng.range(-radius, radius));
    let direction = [0; 3].map(|_| rng.range(-1.0, 1.0));
    let norm = direction
        .iter()
        .map(|v| v * v)
        .sum::<f64>()
        .sqrt()
        .max(1e-3);
    let distance = rng.log_range(radius * 0.02, radius * 4.0);
    let position = std::array::from_fn(|axis| target[axis] + direction[axis] / norm * distance);
    assert!(camera.set_pose(position, target));
    camera
}

fn view_of(camera: &OrbitCamera, target_px: f32, min_side: f32) -> LodView {
    LodView::from_camera(
        camera,
        [VIEWPORT[0] as f32, VIEWPORT[1] as f32],
        target_px,
        min_side,
    )
}

fn unit_forward(rng: &mut Rng) -> [f32; 3] {
    loop {
        let v = [0; 3].map(|_| rng.range(-1.0, 1.0));
        let norm = v.iter().map(|c| c * c).sum::<f64>().sqrt();
        if norm > 0.1 {
            return v.map(|c| (c / norm) as f32);
        }
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalized(v: [f64; 3]) -> [f64; 3] {
    let norm = v.iter().map(|c| c * c).sum::<f64>().sqrt();
    v.map(|c| c / norm)
}

fn random_view(rng: &mut Rng) -> LodView {
    // A right-handed orthonormal basis, as the camera's.
    let forward = unit_forward(rng).map(f64::from);
    let right = loop {
        let candidate = cross(forward, unit_forward(rng).map(f64::from));
        if candidate.iter().map(|c| c * c).sum::<f64>() > 1e-3 {
            break normalized(candidate);
        }
    };
    let up = normalized(cross(forward, right));
    LodView {
        forward: forward.map(|c| c as f32),
        right: right.map(|c| c as f32),
        up: up.map(|c| c as f32),
        focal: rng.range(300.0, 3000.0) as f32,
        target_px: rng.range(0.5, 12.0) as f32,
        min_side: if rng.below(3) == 0 {
            rng.log_range(1e-4, 1.0) as f32
        } else {
            0.0
        },
    }
}

#[test]
fn edge_px_matches_the_camera() {
    let mut rng = Rng(1);
    let mut in_front = 0;
    let mut behind = 0;
    let mut worst = 0.0f64;
    let mut attempts = 0;
    while in_front < 10_000 || behind < 100 {
        attempts += 1;
        assert!(attempts < 2_000_000, "too few cubes in front of the camera");
        let camera = random_camera(&mut rng);
        let position = camera.position();
        let forward = camera.forward();
        let view = view_of(&camera, 1.0, 0.0);
        for _ in 0..4 {
            let spread = position.iter().map(|v| v.abs()).fold(1.0, f64::max) * 2.0;
            let side = rng.log_range(1e-3, 0.5) * spread;
            let lo: [f64; 3] =
                std::array::from_fn(|axis| position[axis] + rng.range(-1.0, 1.0) * spread);
            let zmin = (0..8)
                .map(|corner| {
                    (0..3)
                        .map(|axis| {
                            let c = lo[axis] + if corner >> axis & 1 == 1 { side } else { 0.0 };
                            (c - position[axis]) * forward[axis]
                        })
                        .sum::<f64>()
                })
                .fold(f64::INFINITY, f64::min);
            // Corner too close to the eye plane: f32 error on the depth is amplified into the pixels.
            if zmin.abs() < 1e-3 * spread {
                continue;
            }
            let bounds = SpatialBounds::new(lo, lo.map(|v| v + side)).expect("cube");
            let projected = camera.projected_voxel_edge_pixels(bounds, [0.0; 3], VIEWPORT);
            let relative: [f32; 3] = std::array::from_fn(|axis| (lo[axis] - position[axis]) as f32);
            let edge = f64::from(edge_px(&view, relative, side as f32));
            if zmin < 0.0 {
                assert!(
                    projected.is_infinite() && edge.is_infinite(),
                    "{edge} vs {projected}"
                );
                behind += 1;
                continue;
            }
            let diff = (edge - projected).abs() / projected;
            // f32 rounding of the far corner `lo + side` weighs `|lo| / side` times its epsilon.
            let reach = relative
                .iter()
                .map(|v| f64::from(v.abs()))
                .fold(0.0, f64::max)
                / side;
            worst = worst.max(diff / (1.0 + reach));
            assert!(
                diff <= 1e-4 + 1e-7 * reach,
                "edge_px {edge} vs projected {projected} (lo {lo:?}, side {side}, camera {camera:?})"
            );
            in_front += 1;
        }
    }
    eprintln!("worst relative difference over (1 + |lo| / side): {worst:e}");
}

/// `(lo, side)` of the cell `rel` of level `level` of a node with corner `anchor`.
fn chain_cell(node: &NodeLod, rel: [u32; 3], level: u32, side0: f32) -> ([f32; 3], f32) {
    let side = side0 / pow2(level);
    (cell_lo(node, rel, side), side)
}

/// Measurement: counts child/parent pairs where the child refines and the parent does not. The
/// actual projected edge is not monotone for chains crossing the eye plane, so only chains whose
/// root cell is fully in front of the eye and whose cells f32 resolves are counted; the draw rule does not rely on this.
#[test]
fn refines_is_monotone_from_child_to_parent() {
    let mut rng = Rng(2);
    let mut pairs = 0u64;
    let mut violations = 0u64;
    let mut first = None;
    for case in 0..4000 {
        let far = case % 4 == 0;
        let span = if far { 29 } else { rng.below(12) as u32 + 1 };
        let magnitude = if far { 1e4 } else { rng.log_range(0.1, 1e3) };
        let direction = unit_forward(&mut rng).map(f64::from);
        let node = NodeLod {
            anchor: direction.map(|c| (c * magnitude) as f32),
            node_depth: rng.below(8) as u8,
            active: u64::MAX,
            mask: [0; 8],
        };
        let side_fine = rng.log_range(1e-3, 1.0);
        let side0 = (side_fine * (1u64 << span) as f64) as f32;
        let mut view = random_view(&mut rng);
        let rel: [u32; 3] = [0; 3].map(|_| (rng.next() % (1u64 << span)) as u32);
        let (root_lo, root_side) = chain_cell(&node, [0; 3], 0, side0);
        if edge_px(&view, root_lo, root_side).is_infinite() {
            continue;
        }
        // Targets at the borders of the chain: the threshold crosses between two levels.
        for level in 0..=span {
            let (lo, side) = chain_cell(&node, rel.map(|r| r >> (span - level)), level, side0);
            if rng.below(2) == 0 {
                view.target_px =
                    edge_px(&view, lo, side) * (1.0 + (rng.unit() as f32 - 0.5) * 1e-3);
            }
            // Beyond ~1e6 cells of reach the f32 corners no longer resolve the cell: rounding noise.
            if lo.iter().map(|v| v.abs()).fold(0.0, f32::max) > 1e6 * side {
                continue;
            }
            let mut previous_refines = refines(&view, lo, side);
            for up in (0..level).rev() {
                let (plo, pside) = chain_cell(&node, rel.map(|r| r >> (span - up)), up, side0);
                let parent_refines = refines(&view, plo, pside);
                pairs += 1;
                if previous_refines && !parent_refines {
                    violations += 1;
                    first.get_or_insert_with(|| {
                        format!(
                            "child ({lo:?}, {side}, {level}) refines but parent ({plo:?}, {pside}, {up}) does not; view {view:?}"
                        )
                    });
                }
                previous_refines = parent_refines;
            }
        }
    }
    assert_eq!(
        violations,
        0,
        "{violations} of {pairs} pairs are not monotone; first: {}",
        first.unwrap_or_default()
    );
}

#[test]
fn a_cell_around_the_eye_refines() {
    let mut rng = Rng(3);
    for _ in 0..2000 {
        let mut view = random_view(&mut rng);
        view.target_px = 1.0;
        view.min_side = 0.0;
        let side = rng.log_range(0.01, 100.0) as f32;
        let inside: [f32; 3] = std::array::from_fn(|_| -side * rng.range(0.01, 0.99) as f32);
        // A cube whose nearest corner is behind the eye plane while the eye is outside it.
        let behind: [f32; 3] = std::array::from_fn(|axis| -view.forward[axis] * side * 0.25);
        for lo in [inside, behind] {
            let edge = edge_px(&view, lo, side);
            assert!(
                edge == f32::INFINITY,
                "edge {edge} for lo {lo:?} side {side}"
            );
            assert!(
                refines(&view, lo, side),
                "lo {lo:?} side {side} view {view:?}"
            );
            // At or below `min_side` it stops, still never NaN.
            view.min_side = side;
            assert!(!refines(&view, lo, side));
            view.min_side = 0.0;
        }
    }
}

#[test]
fn min_side_stops_refinement() {
    let mut rng = Rng(4);
    for _ in 0..2000 {
        let mut view = random_view(&mut rng);
        view.target_px = 0.0;
        view.min_side = rng.log_range(1e-3, 10.0) as f32;
        let side = view.min_side * rng.range(0.001, 1.0) as f32;
        let lo = [0; 3].map(|_| rng.range(-1e4, 1e4) as f32);
        assert!(!refines(&view, lo, side));
        assert!(!refines(&view, lo, view.min_side));
    }
}

fn mask_of(rng: &mut Rng, density: u64) -> [u64; 8] {
    std::array::from_fn(|_| {
        (0..64).fold(0u64, |word, bit| {
            word | u64::from(rng.below(density) == 0) << bit
        })
    })
}

fn mask_bit(mask: &[u64; 8], x: usize, y: usize, z: usize) -> bool {
    mask[z] >> (x + 8 * y) & 1 != 0
}

#[test]
fn masked_any_matches_brute_force() {
    let mut rng = Rng(5);
    for density in [1, 2, 7, 40, 200, 100_000] {
        for _ in 0..6 {
            let node_depth = rng.below(10) as u8;
            let node = NodeLod {
                anchor: [0.0; 3],
                node_depth,
                active: 0,
                mask: mask_of(&mut rng, density),
            };
            for level in 0..=5u32 {
                let depth = node_depth + level as u8;
                let cells = 1u32 << level;
                let samples: Vec<[u32; 3]> = if level <= 3 {
                    (0..cells.pow(3))
                        .map(|i| [i % cells, i / cells % cells, i / cells / cells])
                        .collect()
                } else {
                    (0..3000)
                        .map(|_| [0; 3].map(|_| rng.below(u64::from(cells)) as u32))
                        .collect()
                };
                for rel in samples {
                    let expected = if level >= 3 {
                        let shift = level - 3;
                        mask_bit(
                            &node.mask,
                            (rel[0] >> shift) as usize,
                            (rel[1] >> shift) as usize,
                            (rel[2] >> shift) as usize,
                        )
                    } else {
                        let span = 8 >> level;
                        (0..span).any(|dz| {
                            (0..span).any(|dy| {
                                (0..span).any(|dx| {
                                    mask_bit(
                                        &node.mask,
                                        rel[0] as usize * span + dx,
                                        rel[1] as usize * span + dy,
                                        rel[2] as usize * span + dz,
                                    )
                                })
                            })
                        })
                    };
                    assert_eq!(
                        masked_any(&node, depth, rel),
                        expected,
                        "density {density} level {level} rel {rel:?}"
                    );
                }
            }
        }
    }
}

struct Scene {
    name: &'static str,
    node: NodeLod,
    side0: f64,
}

fn active_bits(levels: &[u32]) -> u64 {
    levels.iter().fold(0, |bits, level| bits | 1 << level)
}

/// A scene of `kind`, its node corner eye-relative within a few node sides of the eye.
fn scene(kind: usize, rng: &mut Rng) -> Scene {
    let side0 = rng.log_range(1.0, 1e3);
    let node_depth = rng.below(8) as u8;
    let mut anchor = [0; 3].map(|_| rng.range(-3.0, 3.0) * side0);
    let (name, active, mask) = match kind {
        0 => ("three contiguous", active_bits(&[0, 1, 2]), [0; 8]),
        1 => ("non-contiguous", active_bits(&[0, 2, 5]), [0; 8]),
        2 => ("gap of 17", active_bits(&[0, 17, 20]), [0; 8]),
        3 => {
            // A finest depth at the mask depth or below it, as the gate requires.
            let mask = mask_of(rng, 6);
            ("partially masked", active_bits(&[0, 2, 3, 5]), mask)
        }
        4 => {
            let side_fine = side0 / 8.0;
            // The eye inside a cell of the finest depth.
            anchor = [0; 3].map(|_| -rng.range(0.0, 1.0) * side_fine - 3.0 * side_fine);
            ("eye inside a cell", active_bits(&[0, 1, 3]), [0; 8])
        }
        6 => {
            // `rel >= 2^24` is inexact as an `f32`; level 31 is the top of the span.
            ("top of the span", active_bits(&[0, 25, 31]), [0; 8])
        }
        _ => (
            "mask on every level",
            active_bits(&[1, 3, 4, 6]),
            mask_of(rng, 25),
        ),
    };
    Scene {
        name,
        node: NodeLod {
            anchor: anchor.map(|c| c as f32),
            node_depth,
            active,
            mask,
        },
        side0,
    }
}

const SCENE_KINDS: usize = 7;

/// `(depths drawing, first drawing depth, oracle's answer)`.
fn count_drawing(
    view: &LodView,
    scene: &Scene,
    point: [f64; 3],
) -> (usize, Option<u8>, Option<u8>) {
    let node = &scene.node;
    let node_lo = node.anchor.map(f64::from);
    let mut count = 0;
    let mut first = None;
    for level in 0..=u32::from(MAX_NODE_SPAN) {
        if node.active >> level & 1 == 0 {
            continue;
        }
        let side = scene.side0 / f64::from(pow2(level));
        let cells = (1u64 << level) as f64;
        let rel = std::array::from_fn(|axis| {
            ((point[axis] - node_lo[axis]) / side)
                .floor()
                .clamp(0.0, cells - 1.0) as u32
        });
        let depth = node.node_depth + level as u8;
        if draws(view, node, depth, rel, side as f32) {
            count += 1;
            first.get_or_insert(depth);
        }
    }
    (
        count,
        first,
        drawn_depth_at(view, node, point, node_lo, scene.side0),
    )
}

fn point_is_masked(scene: &Scene, point: [f64; 3]) -> bool {
    let cell = |axis: usize| {
        (((point[axis] - f64::from(scene.node.anchor[axis])) / scene.side0 * 8.0).floor() as usize)
            .min(7)
    };
    mask_bit(&scene.node.mask, cell(0), cell(1), cell(2))
}

#[test]
fn exactly_one_cell_draws_each_point() {
    let mut rng = Rng(6);
    let mut checked = 0;
    for pose in 0..1200 {
        let kind = pose % SCENE_KINDS;
        let scene = scene(kind, &mut rng);
        let mut view = random_view(&mut rng);
        view.min_side = if pose % 5 == 0 {
            (scene.side0 / 2f64.powi(4)) as f32
        } else {
            0.0
        };
        let points: Vec<[f64; 3]> = (0..64)
            .map(|_| {
                std::array::from_fn(|axis| {
                    f64::from(scene.node.anchor[axis]) + rng.unit() * scene.side0
                })
            })
            .collect();
        for band_border in [false, true] {
            if band_border {
                // Land the target within 1e-3 of the edge of some cell of the point's chain.
                let node_lo = scene.node.anchor.map(f64::from);
                let level = rng.below(6) as u32;
                let side = scene.side0 / f64::from(pow2(level));
                let cells = (1u64 << level) as f64;
                let rel = std::array::from_fn(|axis| {
                    ((points[0][axis] - node_lo[axis]) / side)
                        .floor()
                        .clamp(0.0, cells - 1.0) as u32
                });
                let edge = edge_px(&view, cell_lo(&scene.node, rel, side as f32), side as f32);
                view.target_px = edge * (1.0 + (rng.unit() as f32 - 0.5) * 2e-3);
            }
            for &point in &points {
                let (count, drawn, oracle) = count_drawing(&view, &scene, point);
                let expected = usize::from(!point_is_masked(&scene, point));
                assert_eq!(
                    count,
                    expected,
                    "{}: {count} depths draw {point:?} (masked {}); view {view:?} node {:?}",
                    scene.name,
                    expected == 0,
                    scene.node
                );
                assert_eq!(
                    oracle, drawn,
                    "{}: oracle disagrees with the per-depth loop",
                    scene.name
                );
                checked += 1;
            }
        }
    }
    assert!(checked >= 1000 * 64);
}
