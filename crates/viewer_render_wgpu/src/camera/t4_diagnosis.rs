//! Ignored diagnostic: how the T4 metric change (87ef649) alters the LOD cut on a real index.
//!
//! Run with `BELVEDERE_OCT=/path/belvedere.oct cargo test -p viewer_render_wgpu --lib \
//! t4_diagnosis -- --ignored --nocapture`. Only the index is read; no point block is decoded.

use std::collections::BTreeMap;

use glam::DVec3;
use std::collections::HashMap;

use viewer_core::{
    BlockId, ColorRgb, FrustumPlane, LodSelection, LodSelectionConfig, LodTransitionPlanner, Point,
    PointBatch, PointBlock, ResidentBlockSource, SpatialBounds, SpatialFrustum, SpatialHierarchy,
    SpatialNode,
};
use viewer_format::FormatReader;

use super::OrbitCamera;
use crate::SceneBounds;

type Metric<'a> = &'a dyn Fn(&OrbitCamera, SpatialBounds, f64, [u32; 2]) -> f64;

/// Metric of ede7e0a: bounding-sphere nearest depth minus the error argument (`side / 8`).
fn old_metric(
    camera: &OrbitCamera,
    bounds: SpatialBounds,
    spacing: f64,
    viewport: [u32; 2],
) -> f64 {
    let center = DVec3::from_array(std::array::from_fn(|a| {
        (bounds.min()[a] + bounds.max()[a]) * 0.5
    }));
    let half_diagonal = DVec3::from_array(std::array::from_fn(|a| {
        (bounds.max()[a] - bounds.min()[a]) * 0.5
    }))
    .length();
    let (position, forward, _, _) = camera.basis();
    let depth =
        ((center - position).dot(forward) - half_diagonal - spacing).max(camera.clip_distances().0);
    scaled(viewport, spacing) / depth
}

/// The new metric with the old `- spacing` offset kept on the corner depth.
fn corner_minus_spacing(
    camera: &OrbitCamera,
    bounds: SpatialBounds,
    spacing: f64,
    viewport: [u32; 2],
) -> f64 {
    let depth = (corner_depth(camera, bounds) - spacing).max(camera.clip_distances().0);
    scaled(viewport, spacing) / depth
}

fn new_metric(
    camera: &OrbitCamera,
    bounds: SpatialBounds,
    spacing: f64,
    viewport: [u32; 2],
) -> f64 {
    camera.projected_error_pixels(bounds, spacing, [0.0; 3], viewport)
}

fn corner_depth(camera: &OrbitCamera, bounds: SpatialBounds) -> f64 {
    let (position, forward, _, _) = camera.basis();
    (0..8)
        .map(|i| {
            let c = DVec3::from_array(std::array::from_fn(|a| {
                if i & (1 << a) == 0 {
                    bounds.min()[a]
                } else {
                    bounds.max()[a]
                }
            }));
            (c - position).dot(forward)
        })
        .fold(f64::INFINITY, f64::min)
}

/// Numerator shared by both metrics: spacing * viewport height * off-axis / (2 tan(fov / 2)).
fn scaled(viewport: [u32; 2], spacing: f64) -> f64 {
    let aspect = f64::from(viewport[0]) / f64::from(viewport[1]);
    let tv = (super::VERTICAL_FOV * 0.5).tan();
    let th = tv * aspect;
    spacing * f64::from(viewport[1]) * (1.0 + tv * tv + th * th).sqrt() / (2.0 * tv)
}

fn side(bounds: SpatialBounds) -> f64 {
    (0..3)
        .map(|a| bounds.max()[a] - bounds.min()[a])
        .fold(0.0, f64::max)
}

fn local(bounds: SpatialBounds, (origin, vs): ([f64; 3], f64)) -> SpatialBounds {
    SpatialBounds::new(
        std::array::from_fn(|a| (bounds.min()[a] - origin[a]) * vs),
        std::array::from_fn(|a| (bounds.max()[a] - origin[a]) * vs),
    )
    .unwrap()
}

fn select(
    hierarchy: &SpatialHierarchy,
    camera: &OrbitCamera,
    frustum: &SpatialFrustum,
    frame: ([f64; 3], f64),
    viewport: [u32; 2],
    target: f64,
    metric: Metric,
) -> LodSelection {
    let config = LodSelectionConfig::new(target, u64::MAX).unwrap();
    hierarchy
        .select_lod_stable_projector(frustum, config, 1 << 20, |node: &SpatialNode| {
            let b = node.bounds();
            metric(camera, local(b, frame), side(b) * frame.1 / 8.0, viewport)
        })
        .unwrap()
        .0
}

/// Edge of the representative cell placed at the AABB corner nearest to the camera: the worst
/// case `oversizedVoxels` can see for that node.
fn nearest_cell_edge(camera: &OrbitCamera, b: SpatialBounds, viewport: [u32; 2]) -> f64 {
    let spacing = side(b) / 8.0;
    let (_, forward, _, _) = camera.basis();
    let cell_min: [f64; 3] = std::array::from_fn(|a| {
        if forward[a] > 0.0 {
            b.min()[a]
        } else {
            b.max()[a] - spacing
        }
    });
    let cell = SpatialBounds::new(cell_min, cell_min.map(|v| v + spacing)).unwrap();
    camera.projected_voxel_edge_pixels(cell, [0.0; 3], viewport)
}

/// Residency stand-in: one-point blocks, so the planner resolves the cut but no payload is built.
struct Resident(HashMap<BlockId, PointBlock>);

impl ResidentBlockSource for Resident {
    fn peek(&self, id: BlockId) -> Option<&PointBlock> {
        self.0.get(&id)
    }
}

/// Loads the pending blocks of `target` one by one in `priority` order, as the viewer does
/// (one block per update), and reports the source points of the scene rebuilt after each load.
fn simulate_load(
    hierarchy: &SpatialHierarchy,
    target: &LodSelection,
    resident: &mut Resident,
    priority: &dyn Fn(&SpatialNode) -> f64,
) -> (u64, u64, u64, u64) {
    let planner = LodTransitionPlanner::new();
    let (mut builds, mut total, mut max_scene, mut first_loaded) = (0_u64, 0_u64, 0_u64, 0_u64);
    loop {
        let plan = planner.prepare(hierarchy, target, &*resident);
        let scene: u64 = plan
            .node_ids()
            .iter()
            .map(|id| hierarchy.node(*id).unwrap().point_count())
            .sum();
        if builds > 0 {
            total += scene;
            max_scene = max_scene.max(scene);
        }
        let next = plan
            .pending_blocks()
            .iter()
            .map(|block| {
                let node = target
                    .node_ids()
                    .iter()
                    .map(|id| hierarchy.node(*id).unwrap())
                    .find(|node| node.blocks().contains(block))
                    .unwrap();
                (*block, priority(node))
            })
            .reduce(|best, c| {
                if c.1.total_cmp(&best.1).is_gt() || (c.1 == best.1 && c.0 < best.0) {
                    c
                } else {
                    best
                }
            });
        let Some((block, _)) = next else {
            return (builds, total, max_scene, first_loaded);
        };
        if builds < 50 {
            first_loaded += target
                .node_ids()
                .iter()
                .map(|id| hierarchy.node(*id).unwrap())
                .find(|node| node.blocks().contains(&block))
                .unwrap()
                .point_count();
        }
        resident.0.insert(
            block,
            PointBlock::new(
                block,
                PointBatch::new(vec![Point::new([0.0; 3], ColorRgb::new(0, 0, 0))]),
                Vec::new(),
            ),
        );
        builds += 1;
    }
}

#[test]
#[ignore = "needs a real index: set BELVEDERE_OCT"]
fn t4_diagnosis() {
    let path = std::env::var("BELVEDERE_OCT").expect("BELVEDERE_OCT");
    let reader = FormatReader::new(std::fs::File::open(path).unwrap()).unwrap();
    let metadata = reader.format_metadata();
    let dataset = metadata.dataset();
    let hierarchy = dataset.hierarchy();
    let origin = dataset.coordinate_frame().origin();
    let vs = metadata.visual_scale();
    let frame = (origin, vs);
    let stats = hierarchy.stats();
    println!(
        "visual_scale {vs} nodes {} max_level {} source_points {:?} origin {origin:?} root side {:.3}",
        stats.node_count,
        stats.max_level,
        stats.source_point_count,
        side(stats.root_bounds)
    );

    // geometric_error of the hierarchy versus side / 8, by level.
    let mut by_level: BTreeMap<u32, (u64, f64, f64, f64, u64)> = BTreeMap::new();
    let mut stack = vec![hierarchy.root_id()];
    while let Some(id) = stack.pop() {
        let n = hierarchy.node(id).unwrap();
        let e = by_level
            .entry(n.level())
            .or_insert((0, f64::MAX, 0.0, 0.0, 0));
        e.0 += 1;
        e.1 = e.1.min(n.geometric_error());
        e.2 = e.2.max(n.geometric_error());
        e.3 += side(n.bounds()) / 8.0;
        e.4 += u64::from(hierarchy.is_leaf(id));
        stack.extend(hierarchy.child_ids(id));
    }
    for (level, (count, min, max, side8_sum, leaves)) in &by_level {
        println!(
            "level {level}: nodes {count} leaves {leaves} geometric_error [{min:.4}, {max:.4}] mean side/8 {:.4}",
            side8_sum / *count as f64
        );
    }

    let viewport = [1282_u32, 722];
    let root = local(stats.root_bounds, frame);
    let mut camera = OrbitCamera::default();
    assert!(camera.fit(
        SceneBounds {
            min: root.min(),
            max: root.max()
        },
        viewport
    ));
    let local_frustum = camera.spatial_frustum(viewport).unwrap();
    let planes = local_frustum
        .planes()
        .iter()
        .map(|p| {
            let n = p.normal().map(|c| c * vs);
            FrustumPlane::new(
                n,
                p.offset() - (0..3).map(|a| n[a] * origin[a]).sum::<f64>(),
            )
            .unwrap()
        })
        .collect();
    let frustum = SpatialFrustum::new(planes).unwrap();

    let metrics: [(&str, Metric); 3] = [
        ("old (A)", &old_metric),
        ("new (B)", &new_metric),
        ("corner-depth - spacing", &corner_minus_spacing),
    ];
    for target in [16.0, 8.0, 4.0] {
        println!("--- target {target} px, viewport {viewport:?}");
        for (name, metric) in &metrics {
            let sel = select(
                hierarchy, &camera, &frustum, frame, viewport, target, *metric,
            );
            let mut levels: BTreeMap<u32, u64> = BTreeMap::new();
            let (mut blocks, mut max_metric, mut max_edge, mut oversized) =
                (0_usize, 0.0_f64, 0.0_f64, 0_u64);
            for id in sel.node_ids() {
                let node = hierarchy.node(*id).unwrap();
                *levels.entry(node.level()).or_default() += 1;
                blocks += node.blocks().len();
                let b = node.bounds();
                let m = metric(&camera, local(b, frame), side(b) * vs / 8.0, viewport);
                let edge = nearest_cell_edge(&camera, local(b, frame), viewport);
                if !sel.resolution_limited().contains(id) {
                    max_metric = max_metric.max(m);
                }
                if edge.is_finite() {
                    max_edge = max_edge.max(edge);
                    oversized += u64::from(edge > target);
                }
            }
            println!(
                "{name:24} nodes {:6} points {:11} blocks {blocks:6} res_limited {:4} max_metric {max_metric:7.2} max_nearest_cell_edge {max_edge:7.2} cells_over_target {oversized} levels {levels:?}",
                sel.node_ids().len(),
                sel.point_count(),
                sel.resolution_limited().len(),
            );
        }
    }

    // Internal nodes carry a subsample, leaves the dense data: this is what the load order moves.
    for target_px in [8.0, 4.0] {
        let target = select(
            hierarchy,
            &camera,
            &frustum,
            frame,
            viewport,
            target_px,
            &new_metric,
        );
        let (mut internal, mut leaves) = ((0_u64, 0_u64), (0_u64, 0_u64));
        for id in target.node_ids() {
            let node = hierarchy.node(*id).unwrap();
            let slot = if hierarchy.is_leaf(*id) {
                &mut leaves
            } else {
                &mut internal
            };
            slot.0 += 1;
            slot.1 += node.point_count();
        }
        println!("cut {target_px} px: internal (nodes, points) {internal:?} leaves {leaves:?}");
    }

    // Load-order simulation: stage 8 px then stage 4 px from the same residency, per policy.
    let policies: [(&str, Metric, bool, bool); 4] = [
        (
            "A order (error from node.geometric_error)",
            &old_metric,
            true,
            false,
        ),
        ("B order (error from side / 8)", &new_metric, false, false),
        (
            "D order (B error per source point, shipped)",
            &new_metric,
            false,
            true,
        ),
        (
            "C order (new metric, node.geometric_error)",
            &new_metric,
            true,
            false,
        ),
    ];
    for (name, metric, real_error, per_point) in policies {
        let mut resident = Resident(HashMap::new());
        println!("--- load order: {name}");
        for target_px in [8.0, 4.0] {
            let target = select(
                hierarchy,
                &camera,
                &frustum,
                frame,
                viewport,
                target_px,
                &new_metric,
            );
            let (builds, total, max_scene, first_loaded) =
                simulate_load(hierarchy, &target, &mut resident, &|node| {
                    let spacing = if real_error {
                        node.geometric_error()
                    } else {
                        side(node.bounds()) / 8.0
                    };
                    let error =
                        metric(&camera, local(node.bounds(), frame), spacing * vs, viewport);
                    if per_point {
                        error / node.point_count().max(1) as f64
                    } else {
                        error
                    }
                });
            println!(
                "stage {target_px} px: builds {builds} cumulative_source_points {total} mean_per_build {} max_scene {max_scene} points_of_first_50_loaded {first_loaded}",
                total / builds.max(1)
            );
        }
    }
}
