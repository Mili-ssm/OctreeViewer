//! Task 3 pass criteria: the level choice follows RQ-09, residency is all-or-nothing, and the
//! masks make the visible volume equal the covered volume with no overlap.

use std::collections::HashMap;

use viewer_core::{HierarchySemantics, NodeId, PointBlock, SpatialBounds};

use super::*;
use crate::{
    NodeCellLevel, NodeCellSource, OrbitCamera, PointNodeCellSource, VoxelBuildLimits,
    scene_digest_tests::{Fixture, ORIGIN, SIDE, VIEWPORT, cameras, fixture, ids},
};

const TARGETS: [f64; 3] = [2.0, 4.0, 8.0];

/// (name, grid origin, coordinate origin, nodes).
type Synthetic = (
    &'static str,
    [f64; 3],
    [f64; 3],
    HashMap<NodeId, CellCutNode>,
);

fn view(camera: &OrbitCamera) -> ViewProjection {
    ViewProjection::new(*camera, VIEWPORT, 1.0, VoxelBuildLimits::default())
}

fn dataset_grid(root_min: [f64; 3]) -> CellGrid {
    CellGrid {
        root_min,
        root_side: SIDE,
    }
}

/// Stored levels of every fixture node, built like `refinement_tests.rs` (local frame).
fn fixture_nodes(fixture: &Fixture) -> HashMap<NodeId, CellCutNode> {
    let local_grid = CellGrid {
        root_min: [0.0; 3],
        root_side: SIDE,
    };
    (1..=9)
        .map(NodeId::new)
        .map(|id| {
            let node = fixture.hierarchy.node(id).expect("node");
            let bounds = node.bounds();
            let local = SpatialBounds::new(
                std::array::from_fn(|axis| bounds.min()[axis] - ORIGIN[axis]),
                std::array::from_fn(|axis| bounds.max()[axis] - ORIGIN[axis]),
            )
            .unwrap();
            let blocks: Vec<PointBlock> = node
                .blocks()
                .iter()
                .map(|block| fixture.cache.peek(*block).expect("block").clone())
                .collect();
            let cells = PointNodeCellSource::default()
                .node_cells(
                    &local_grid,
                    id,
                    local,
                    node.spacing(),
                    fixture.hierarchy.is_leaf(id),
                    &blocks,
                )
                .expect("cells");
            let stored: Vec<u8> = cells.levels.iter().map(|level| level.depth).collect();
            (
                id,
                CellCutNode {
                    bounds,
                    buildable: (stored[0], *stored.last().expect("levels")),
                    resident: stored.clone(),
                    stored,
                },
            )
        })
        .collect()
}

fn run(
    cut: &[NodeId],
    nodes: &HashMap<NodeId, CellCutNode>,
    camera: &OrbitCamera,
    grid: &CellGrid,
    origin: [f64; 3],
    target: f64,
) -> Result<Vec<CellCutEntry>, CellCutError> {
    cell_cut(
        cut,
        &|id| nodes.get(&id).cloned(),
        &view(camera),
        grid,
        origin,
        target,
    )
}

fn fixture_cuts() -> Vec<(&'static str, Vec<NodeId>)> {
    vec![
        ("root", ids(&[1])),
        ("children", ids(&[2, 3, 4, 5, 6, 7, 8, 9])),
        ("mixed", ids(&[1, 2, 3])),
    ]
}

/// Brute-force oracle with its own cube placement, in the dataset frame: the first stored depth
/// whose cube, at the octant point nearest the eye, meets `target`, else the finest (every
/// coarser stored depth fails by construction).
fn oracle_octant(
    view: &ViewProjection,
    origin: [f64; 3],
    root_side: f64,
    bounds: SpatialBounds,
    octant: usize,
    stored: &[u8],
    target: f64,
) -> u8 {
    let eye: [f64; 3] =
        std::array::from_fn(|a| view.camera().position()[a] / view.visual_scale() + origin[a]);
    // Octant box: axis 0 (x) is the highest bit of the octant index.
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for a in 0..3 {
        let mid = (bounds.min()[a] + bounds.max()[a]) / 2.0;
        if (octant >> (2 - a)) & 1 == 1 {
            (lo[a], hi[a]) = (mid, bounds.max()[a]);
        } else {
            (lo[a], hi[a]) = (bounds.min()[a], mid);
        }
    }
    let edge = |depth: u8| -> f64 {
        let side = root_side / f64::from(1_u32 << depth);
        let mut cube_min = [0.0; 3];
        for a in 0..3 {
            let mut v = eye[a];
            if v < lo[a] {
                v = lo[a];
            }
            if v > hi[a] {
                v = hi[a];
            }
            // Shift so the cube stays inside the octant when it fits.
            if v + side > hi[a] {
                v = hi[a] - side;
            }
            if v < lo[a] {
                v = lo[a];
            }
            cube_min[a] = v;
        }
        let cube = SpatialBounds::new(cube_min, cube_min.map(|v| v + side)).unwrap();
        view.projected_voxel_edge_pixels(cube, origin)
    };
    // Brute force: test every stored depth; the choice is the coarsest passing one.
    let passing: Vec<u8> = stored
        .iter()
        .copied()
        .filter(|d| edge(*d) <= target)
        .collect();
    passing.first().copied().unwrap_or(*stored.last().unwrap())
}

/// Checks every non-masked octant of `entries` against the oracle (with the spanning-level
/// promotion applied by hand). Returns `(octants checked, octants below the finest level)`.
#[allow(clippy::too_many_arguments)]
fn assert_octant_oracle(
    entries: &[CellCutEntry],
    nodes: &HashMap<NodeId, CellCutNode>,
    camera: &OrbitCamera,
    scale: f64,
    grid: &CellGrid,
    origin: [f64; 3],
    target: f64,
    label: &str,
) -> (usize, usize) {
    let view = ViewProjection::new(*camera, VIEWPORT, scale, VoxelBuildLimits::default());
    let (mut checked, mut coarse) = (0, 0);
    for entry in entries {
        let node = &nodes[&entry.node];
        // Independent of the library: the node side against the root side.
        let node_side = node.bounds.max()[0] - node.bounds.min()[0];
        let node_depth = (grid.root_side / node_side).log2().round() as u8;
        let range: Vec<u8> = (node.buildable.0..=node.buildable.1).collect();
        let finest = *range.last().unwrap();
        for o in 0..8 {
            let Some(wanted) = entry.wanted[o] else {
                continue;
            };
            let raw = oracle_octant(
                &view,
                origin,
                grid.root_side,
                node.bounds,
                o,
                &range,
                target,
            );
            let expected = if raw > node_depth {
                raw
            } else {
                range
                    .iter()
                    .copied()
                    .find(|d| *d > node_depth)
                    .unwrap_or(finest)
            };
            assert_eq!(
                wanted,
                expected,
                "{label}: node {} octant {o}",
                entry.node.get()
            );
            checked += 1;
            coarse += usize::from(wanted < finest);
        }
    }
    (checked, coarse)
}

#[test]
#[allow(clippy::type_complexity)]
fn octant_choice_matches_the_brute_force_oracle() {
    let grid = dataset_grid(ORIGIN);
    let replacement = fixture_nodes(&fixture(HierarchySemantics::Replacement));
    let mut wide = chain(ORIGIN, 3, 0);
    for node in wide.values_mut() {
        let first = node.stored[0];
        node.stored = (first..first + 8).collect();
        node.buildable = (first, first + 7);
        node.resident = node.stored.clone();
    }
    let crs = [1.0e6; 3];
    let mut wide_crs = chain(crs, 3, 0);
    for node in wide_crs.values_mut() {
        let first = node.stored[0];
        node.stored = (first..first + 8).collect();
        node.buildable = (first, first + 7);
        node.resident = node.stored.clone();
    }
    // Hand-checked: chain node 3 is two halvings of the root (side 64 -> 16), also at 1e6.
    for nodes in [&wide, &wide_crs] {
        let b = nodes[&NodeId::new(3)].bounds;
        assert_eq!(b.max()[0] - b.min()[0], 16.0);
        assert_eq!((SIDE / 16.0).log2(), 2.0);
    }
    let (mut checked, mut coarse) = (0, 0);
    let cases: Vec<(&str, &HashMap<NodeId, CellCutNode>, CellGrid, [f64; 3])> = vec![
        ("fixture", &replacement, dataset_grid(ORIGIN), ORIGIN),
        ("wide", &wide, grid, ORIGIN),
        ("wide_crs", &wide_crs, dataset_grid(crs), crs),
    ];
    for (name, nodes, grid, origin) in cases {
        let cuts = if name == "fixture" {
            fixture_cuts()
        } else {
            vec![("chain", ids(&[1, 2, 3]))]
        };
        for (cut_name, cut) in cuts {
            for (camera_name, camera) in cameras() {
                for target in TARGETS {
                    let label = format!("{name}/{cut_name}/{camera_name}/{target}px");
                    let entries = run(&cut, nodes, &camera, &grid, origin, target).unwrap();
                    let (c, f) = assert_octant_oracle(
                        &entries, nodes, &camera, 1.0, &grid, origin, target, &label,
                    );
                    checked += c;
                    coarse += f;
                }
            }
        }
    }
    assert!(
        checked > 0 && coarse > 0,
        "checked {checked}, coarse {coarse}"
    );
}

#[test]
fn octants_of_one_node_can_choose_different_levels() {
    let grid = dataset_grid(ORIGIN);
    let nodes = root_with(&[1, 2, 3, 4, 5, 6], &[1, 2, 3, 4, 5, 6]);
    let mut differing = 0;
    for (_, camera) in cameras() {
        for target in [1.0, 2.0, 4.0, 8.0, 16.0] {
            let entries = run(&ids(&[1]), &nodes, &camera, &grid, ORIGIN, target).unwrap();
            differing += usize::from(entries[0].wanted.iter().any(|w| *w != entries[0].wanted[0]));
        }
    }
    assert!(differing > 0, "no case split the octants");
}

#[test]
fn a_pruned_depth_is_wanted_as_is() {
    let grid = dataset_grid(ORIGIN);
    let bounds = SpatialBounds::new(ORIGIN, ORIGIN.map(|v| v + SIDE)).unwrap();
    let mut nodes = root_with(&[3, 10], &[3, 10]);
    nodes.get_mut(&NodeId::new(1)).unwrap().buildable = (3, 10);
    let mut pruned_away = 0;
    for (_, camera) in cameras() {
        let view = view(&camera);
        for target in TARGETS {
            let entries = run(&ids(&[1]), &nodes, &camera, &grid, ORIGIN, target).unwrap();
            for (o, wanted) in entries[0].wanted.iter().enumerate() {
                let w_full = octant_wanted_depth(&view, &grid, ORIGIN, bounds, o, (3, 10), target);
                assert_eq!(*wanted, Some(w_full), "octant {o} at {target}px");
                pruned_away += usize::from(![3, 10].contains(&w_full));
            }
        }
    }
    assert!(pruned_away > 0, "no case wanted a depth that is not stored");
}

#[test]
fn internal_nodes_never_want_beyond_their_range() {
    let grid = dataset_grid(ORIGIN);
    let mut nodes = root_with(&[3, 4, 5, 6, 7, 8], &[3, 4, 5, 6, 7, 8]);
    // An internal node builds `first..=first + 5` and no deeper.
    nodes.get_mut(&NodeId::new(1)).unwrap().buildable = (3, 8);
    // Eye inside the node, a huge scale of detail: every octant would want the deepest depth.
    let mut camera = cameras().remove(0).1;
    assert!(camera.set_pose([SIDE / 4.0; 3], [SIDE / 2.0; 3]));
    for target in [0.01, 1.0] {
        let entries = run(&ids(&[1]), &nodes, &camera, &grid, ORIGIN, target).unwrap();
        for wanted in entries[0].wanted.iter().flatten() {
            assert!(*wanted <= 3 + 5, "{wanted} beyond the range at {target}px");
        }
    }
}

#[test]
fn a_leaf_can_want_finer_than_its_chain() {
    let grid = dataset_grid(ORIGIN);
    let mut nodes = root_with(&[3, 4, 5], &[3, 4, 5]);
    // A leaf stores a short chain but can build deeper levels on demand.
    nodes.get_mut(&NodeId::new(1)).unwrap().buildable = (3, 9);
    let mut camera = cameras().remove(0).1;
    assert!(camera.set_pose([SIDE / 4.0; 3], [SIDE / 2.0; 3]));
    let entries = run(&ids(&[1]), &nodes, &camera, &grid, ORIGIN, 1.0).unwrap();
    let stored_finest = 5;
    assert!(
        entries[0]
            .wanted
            .iter()
            .flatten()
            .any(|w| *w > stored_finest),
        "{:?}",
        entries[0].wanted
    );
    // Residents keep drawing: the depth that serves it is still resident.
    assert!(
        entries[0]
            .drawn
            .iter()
            .flatten()
            .all(|d| *d <= stored_finest)
    );
    assert_eq!(entries[0].stage, [false; 8]);
}

#[test]
fn camera_inside_node_returns_finest_for_its_octant() {
    let huge = SpatialBounds::new([-1000.0; 3], [1000.0; 3]).unwrap();
    let grid = CellGrid {
        root_min: [-1000.0; 3],
        root_side: 2000.0,
    };
    let stored: Vec<u8> = (3..=9).collect();
    let mut camera = cameras().remove(0).1;
    // Eye inside the +x +y -z octant (index 4 + 2 + 0 = 6), looking along -x.
    assert!(camera.set_pose([500.0, 300.0, -200.0], [0.0, 300.0, -200.0]));
    let view = view(&camera);
    for target in TARGETS {
        let wanted: Vec<u8> = (0..8)
            .map(|o| octant_wanted_depth(&view, &grid, [0.0; 3], huge, o, (3, 9), target))
            .collect();
        assert_eq!(wanted[6], 9, "octant holding the eye");
        for (o, depth) in wanted.iter().enumerate() {
            let oracle = oracle_octant(&view, [0.0; 3], 2000.0, huge, o, &stored, target);
            assert_eq!(*depth, oracle, "octant {o} at {target}px");
        }
    }
}

#[test]
fn camera_inside_the_dataset_matches_the_oracle() {
    let grid = dataset_grid(ORIGIN);
    let mut nodes = chain(ORIGIN, 3, 0);
    for node in nodes.values_mut() {
        let first = node.stored[0];
        node.stored = (first..first + 8).collect();
        node.buildable = (first, first + 7);
        node.resident = node.stored.clone();
    }
    let mut camera = cameras().remove(0).1;
    // Local frame: the dataset spans 0..SIDE; the eye sits inside the root and node 2.
    assert!(camera.set_pose([10.0, 12.0, 9.0], [40.0, 30.0, 20.0]));
    let mut coarse = 0;
    for cut in [ids(&[1]), ids(&[1, 2, 3])] {
        for target in [1.0, 2.0, 4.0, 8.0] {
            let entries = run(&cut, &nodes, &camera, &grid, ORIGIN, target).unwrap();
            let (_, c) = assert_octant_oracle(
                &entries, &nodes, &camera, 1.0, &grid, ORIGIN, target, "inside",
            );
            coarse += c;
        }
    }
    assert!(coarse > 0, "every octant was source-limited");
}

#[test]
fn shifted_origin_and_visual_scale_do_not_change_the_levels() {
    const SHIFT: f64 = 1.0e6;
    const SCALE: f64 = 0.0788;
    let stored: Vec<u8> = (3..=11).collect();
    let make = |shift: f64| -> HashMap<NodeId, CellCutNode> {
        let mut nodes = chain(
            [ORIGIN[0] + shift, ORIGIN[1] + shift, ORIGIN[2] + shift],
            3,
            0,
        );
        for node in nodes.values_mut() {
            node.stored = stored.clone();
            node.buildable = (3, 11);
            node.resident = stored.clone();
        }
        nodes
    };
    let (plain, shifted) = (make(0.0), make(SHIFT));
    let origin = ORIGIN;
    let shifted_origin = origin.map(|v| v + SHIFT);
    let mut coarse = 0;
    for (name, camera) in cameras() {
        let mut scaled = camera;
        assert!(scaled.set_pose(
            camera.position().map(|v| v * SCALE),
            camera.target().map(|v| v * SCALE)
        ));
        for target in [1.0, 2.0, 4.0, 8.0] {
            let cut = ids(&[1, 2, 3]);
            let base = run(&cut, &plain, &camera, &dataset_grid(origin), origin, target).unwrap();
            let moved = cell_cut(
                &cut,
                &|id| shifted.get(&id).cloned(),
                &ViewProjection::new(scaled, VIEWPORT, SCALE, VoxelBuildLimits::default()),
                &dataset_grid(shifted_origin),
                shifted_origin,
                target,
            )
            .unwrap();
            for (a, b) in base.iter().zip(&moved) {
                assert_eq!(
                    a.wanted,
                    b.wanted,
                    "{name}/{target}px node {}",
                    a.node.get()
                );
                coarse += a.wanted.iter().flatten().filter(|d| **d < 11).count();
            }
        }
    }
    assert!(coarse > 0, "every octant was source-limited");
}

#[test]
fn fully_masked_octant_requests_no_level() {
    let camera = cameras().remove(0).1;
    let grid = dataset_grid(ORIGIN);
    // Node 3 is the +x child of the root: it hides the 4^3 block of octant 4 only.
    let all = fixture_nodes(&fixture(HierarchySemantics::Replacement));
    let entries = run(&ids(&[1, 3]), &all, &camera, &grid, ORIGIN, 4.0).unwrap();
    for o in 0..8 {
        assert_eq!(entries[0].wanted[o].is_none(), o == 4, "octant {o}");
        assert_eq!(entries[0].drawn[o].is_none(), o == 4, "octant {o}");
    }
    assert!(!entries[0].stage[4]);
    assert_eq!(entries[0].draws().iter().fold(0_u8, |b, d| b | d.1), 0xef);
    // The chain's lower child hides octant 0.
    let nodes = chain(ORIGIN, 2, 0);
    let entries = run(&ids(&[1, 2]), &nodes, &camera, &grid, ORIGIN, 4.0).unwrap();
    assert_eq!(entries[0].wanted[0], None);
    assert!(entries[0].wanted[1..].iter().all(Option::is_some));
}

#[test]
fn spanning_levels_promote_octants_to_the_next_buildable_level() {
    let camera = cameras().remove(0).1;
    let grid = dataset_grid(ORIGIN);
    // The node is at depth 0: a level at depth 0 spans octants, so a coarse choice moves on.
    let nodes = root_with(&[0, 2], &[0, 2]);
    let entries = run(&ids(&[1]), &nodes, &camera, &grid, ORIGIN, 1.0e6).unwrap();
    // Depth 1 is not stored but buildable: it is the first one past the spanning level.
    assert_eq!(entries[0].wanted, [Some(1); 8]);
    // Nothing finer than the spanning level: every octant uses it.
    let nodes = root_with(&[0], &[0]);
    let entries = run(&ids(&[1]), &nodes, &camera, &grid, ORIGIN, 1.0e6).unwrap();
    assert_eq!(entries[0].wanted, [Some(0); 8]);
    let entries = run(&ids(&[1]), &nodes, &camera, &grid, ORIGIN, 0.001).unwrap();
    assert_eq!(entries[0].wanted, [Some(0); 8]);
    assert_eq!(entries[0].draws(), vec![(0, 0xff)]);
}

#[test]
fn digest_tells_a_hidden_octant_from_depth_zero() {
    let entry = |wanted: Option<u8>| CellCutEntry {
        node: NodeId::new(1),
        wanted: [wanted; 8],
        drawn: [wanted; 8],
        stage: [false; 8],
        mask: [0; 8],
    };
    assert_ne!(
        cell_cut_digest(&[entry(None)]),
        cell_cut_digest(&[entry(Some(0))])
    );
    let mut mixed = entry(Some(0));
    mixed.wanted[3] = None;
    mixed.drawn[3] = None;
    assert_ne!(
        cell_cut_digest(&[mixed]),
        cell_cut_digest(&[entry(Some(0))])
    );
}

/// A level holding every `keep`-th cell of the node's `2^(depth - node_depth)` cube per axis in
/// Morton order, with octant offsets counted from integer coordinates.
fn synthetic_level(node_depth: u8, depth: u8, keep: usize) -> NodeCellLevel {
    let n = 1_u64 << (depth - node_depth);
    let half = n / 2;
    let mut per_octant: Vec<Vec<[u64; 3]>> = vec![Vec::new(); 8];
    let mut seen = 0_usize;
    for x in 0..n {
        for y in 0..n {
            for z in 0..n {
                seen += 1;
                if !seen.is_multiple_of(keep) {
                    continue;
                }
                let o = 4 * usize::from(x >= half)
                    + 2 * usize::from(y >= half)
                    + usize::from(z >= half);
                per_octant[o].push([x, y, z]);
            }
        }
    }
    let mut offsets = [0_u32; 9];
    let mut cells = Vec::new();
    for (o, list) in per_octant.iter().enumerate() {
        for coord in list {
            cells.push(crate::NodeCell {
                coord: *coord,
                ..level_cell()
            });
        }
        offsets[o + 1] = cells.len() as u32;
    }
    NodeCellLevel {
        depth,
        cells,
        octant_offsets: Some(offsets),
    }
}

/// Cover oracle for one node (id 1) at depth `node_depth` with levels `depths`, of which
/// `resident` are loaded. A level at depth <= `node_depth` is one cell spanning the node.
/// Returns `(cases with two or more drawn levels, cases drawing a spanning level)`.
fn assert_draw_cover(
    node_depth: u8,
    depths: &[u8],
    resident: &[u8],
    finest: u8,
    targets: &[f64],
    label: &str,
) -> (usize, usize) {
    let side = SIDE / f64::from(1_u32 << node_depth);
    let bounds = SpatialBounds::new(ORIGIN, ORIGIN.map(|v| v + side)).unwrap();
    let nodes = HashMap::from([(
        NodeId::new(1),
        CellCutNode {
            bounds,
            buildable: (depths[0], *depths.last().unwrap()),
            stored: depths.to_vec(),
            resident: resident.to_vec(),
        },
    )]);
    // The grid root is the cube of side SIDE at ORIGIN, so this node sits at `node_depth`.
    let grid = dataset_grid(ORIGIN);
    let levels: Vec<NodeCellLevel> = depths
        .iter()
        .map(|d| {
            if *d <= node_depth {
                NodeCellLevel {
                    depth: *d,
                    cells: vec![level_cell()],
                    octant_offsets: None,
                }
            } else {
                synthetic_level(node_depth, *d, 3)
            }
        })
        .collect();
    let present: HashMap<u8, std::collections::HashSet<[u64; 3]>> = levels
        .iter()
        .map(|l| (l.depth, l.cells.iter().map(|c| c.coord).collect()))
        .collect();
    let n = 1_u64 << (finest - node_depth);
    let (mut mixed, mut spanning_cases) = (0, 0);
    for (name, camera) in cameras() {
        for target in targets {
            let entries = run(&ids(&[1]), &nodes, &camera, &grid, ORIGIN, *target).unwrap();
            let entry = &entries[0];
            let draws = entry.draws();
            let has_spanning = draws.iter().any(|(d, _)| *d <= node_depth);
            assert!(
                !has_spanning || draws.len() == 1,
                "{label}/{name}/{target}px mixes a spanning level with another: {draws:?}"
            );
            for o in 0..8 {
                if let (Some(w), Some(d)) = (entry.wanted[o], entry.drawn[o]) {
                    assert_eq!(
                        entry.stage[o],
                        d < w,
                        "{label}/{name}/{target}px octant {o}"
                    );
                }
            }
            mixed += usize::from(draws.len() > 1);
            spanning_cases += usize::from(has_spanning);
            let mut covered: HashMap<[u64; 3], u32> = HashMap::new();
            for (depth, octants) in &draws {
                let level = levels.iter().find(|l| l.depth == *depth).unwrap();
                for range in octant_ranges(level, *octants) {
                    for cell in &level.cells[range] {
                        let (f, origin) = if *depth <= node_depth {
                            (n, [0; 3])
                        } else {
                            (1_u64 << (finest - depth), cell.coord)
                        };
                        for dx in 0..f {
                            for dy in 0..f {
                                for dz in 0..f {
                                    let c = origin.map(|v| v * f);
                                    *covered
                                        .entry([c[0] + dx, c[1] + dy, c[2] + dz])
                                        .or_default() += 1;
                                }
                            }
                        }
                    }
                }
            }
            // Oracle: a lattice cell is covered once iff its octant's drawn level has a cell on it.
            for x in 0..n {
                for y in 0..n {
                    for z in 0..n {
                        let o = 4 * usize::from(x >= n / 2)
                            + 2 * usize::from(y >= n / 2)
                            + usize::from(z >= n / 2);
                        let expected = entry.drawn[o].map_or(0, |depth| {
                            if depth <= node_depth {
                                1
                            } else {
                                let shift = finest - depth;
                                u32::from(present[&depth].contains(&[
                                    x >> shift,
                                    y >> shift,
                                    z >> shift,
                                ]))
                            }
                        });
                        let got = covered.get(&[x, y, z]).copied().unwrap_or(0);
                        assert_eq!(
                            got,
                            expected,
                            "{label}/{name}/{target}px cell {:?}",
                            [x, y, z]
                        );
                    }
                }
            }
        }
    }
    (mixed, spanning_cases)
}

#[test]
fn draws_cover_each_lattice_cell_of_the_drawn_octants_once() {
    let targets = [1.0, 2.0, 4.0, 8.0, 16.0, 24.0, 32.0, 48.0];
    // Splitting levels only, root node.
    let (mixed, _) = assert_draw_cover(0, &[3, 4, 5, 6], &[3, 4, 5, 6], 6, &targets, "split");
    assert!(mixed > 0, "no case drew two levels at once");
    // A resident spanning level at node depth 0 and 1, while finer wanted levels are absent.
    // (R3-Q21: it is the closest level only when it is the only one coarser than the wanted.)
    assert_draw_cover(0, &[0, 3, 4, 5], &[0, 3, 4], 5, &targets, "span0_mixed");
    let (_, spanning) = assert_draw_cover(0, &[0, 3, 4, 5], &[0], 5, &targets, "span0");
    assert!(spanning > 0, "no case drew the spanning level");
    assert_draw_cover(1, &[1, 3, 4, 5], &[1, 3, 4], 5, &targets, "span1_mixed");
    let (_, spanning) = assert_draw_cover(1, &[1, 3, 4, 5], &[1], 5, &targets, "span1");
    assert!(spanning > 0, "no case drew the spanning level");
    assert_draw_cover(1, &[1, 3, 4, 5], &[1, 3, 4, 5], 5, &targets, "span1_all");
}

#[test]
fn a_spanning_resident_level_is_the_only_level_a_node_draws() {
    let camera = cameras().remove(0).1;
    let grid = dataset_grid(ORIGIN);
    let mut forced = 0;
    for (stored, resident) in [
        (vec![0, 2], vec![0, 2]),
        (vec![0, 2], vec![0]),
        (vec![0, 2, 6], vec![0, 2]),
        (vec![0, 2, 6], vec![0]),
        (vec![0, 2, 6], vec![0, 6]),
    ] {
        let nodes = root_with(&stored, &resident);
        for (_, camera) in cameras().into_iter().chain([("c", camera)]) {
            for target in [1.0, 4.0, 16.0, 32.0, 1.0e6] {
                let entries = run(&ids(&[1]), &nodes, &camera, &grid, ORIGIN, target).unwrap();
                let draws = entries[0].draws();
                if draws.iter().any(|(d, _)| *d == 0) {
                    assert_eq!(
                        draws.len(),
                        1,
                        "{stored:?}/{resident:?}/{target}px: {draws:?}"
                    );
                    forced += usize::from(entries[0].wanted.iter().any(|w| *w != Some(0)));
                }
            }
        }
    }
    assert!(
        forced > 0,
        "no case forced the spanning level over wanted finer ones"
    );
}

fn root_with(stored: &[u8], resident: &[u8]) -> HashMap<NodeId, CellCutNode> {
    let bounds = SpatialBounds::new(ORIGIN, ORIGIN.map(|v| v + SIDE)).unwrap();
    HashMap::from([(
        NodeId::new(1),
        CellCutNode {
            bounds,
            buildable: stored
                .first()
                .zip(stored.last())
                .map_or((0, 0), |(a, b)| (*a, *b)),
            stored: stored.to_vec(),
            resident: resident.to_vec(),
        },
    )])
}

#[test]
fn missing_level_draws_the_closest_resident_level() {
    assert_eq!(resident_depth(5, &[3, 5]), Some((5, false)));
    // R3-Q21: the finest of the coarser levels ...
    assert_eq!(resident_depth(5, &[4, 3]), Some((4, true)));
    assert_eq!(resident_depth(5, &[3, 7, 4]), Some((4, true)));
    // ... else the coarsest of the finer ones, which meets the target (not a stage).
    assert_eq!(resident_depth(5, &[7]), Some((7, false)));
    assert_eq!(resident_depth(5, &[9, 7]), Some((7, false)));
    assert_eq!(resident_depth(5, &[]), None);
    let camera = cameras().remove(0).1;
    let nodes = root_with(&[3, 4, 5], &[4, 3]);
    let entries = run(
        &ids(&[1]),
        &nodes,
        &camera,
        &dataset_grid(ORIGIN),
        ORIGIN,
        0.001,
    )
    .unwrap();
    assert_eq!(entries[0].wanted, [Some(5); 8]);
    assert_eq!(entries[0].drawn, [Some(4); 8]);
    assert_eq!(entries[0].stage, [true; 8]);
}

#[test]
fn the_closest_level_avoids_coarsening_within_a_node() {
    // Octants 0..4 want depth 4 (resident), octants 4..8 want depth 6 (not resident): the second
    // half draws depth 4 too, not the coarsest depth 3 (R3-Q21).
    let nodes = root_with(&[3, 4, 6], &[3, 4]);
    let entries = cell_cut_with(
        &ids(&[1]),
        &|id| nodes.get(&id).cloned(),
        &dataset_grid(ORIGIN),
        &|_, octant, _| if octant < 4 { 4 } else { 6 },
    )
    .unwrap();
    assert_eq!(entries[0].drawn, [Some(4); 8]);
    assert_eq!(
        entries[0].stage,
        [false, false, false, false, true, true, true, true]
    );
    assert_eq!(entries[0].draws(), vec![(4, 0xff)]);
}

#[test]
fn a_spanning_level_wins_also_when_other_octants_have_finer_resident_levels() {
    // Octant 0 wants depth 1 (above the node's detail), the others depth 3. With 0 and 3
    // resident, the closest level of octant 0 is the spanning 0 (nothing is coarser than 1 but
    // it), so the node draws 0 whole even though depth 3 is resident for the other octants.
    let nodes = root_with(&[0, 1, 3], &[0, 3]);
    let entries = cell_cut_with(
        &ids(&[1]),
        &|id| nodes.get(&id).cloned(),
        &dataset_grid(ORIGIN),
        &|_, octant, _| if octant == 0 { 1 } else { 3 },
    )
    .unwrap();
    assert_eq!(entries[0].wanted[0], Some(1));
    assert_eq!(entries[0].drawn, [Some(0); 8]);
    assert_eq!(entries[0].draws(), vec![(0, 0xff)]);
    assert_eq!(
        entries[0].stage,
        [true, true, true, true, true, true, true, true]
    );
}

#[test]
fn no_resident_level_is_not_drawn() {
    assert_eq!(resident_depth(5, &[]), None);
    let camera = cameras().remove(0).1;
    let nodes = root_with(&[3, 4, 5], &[]);
    let entries = run(
        &ids(&[1]),
        &nodes,
        &camera,
        &dataset_grid(ORIGIN),
        ORIGIN,
        0.001,
    )
    .unwrap();
    assert_eq!(entries[0].drawn, [None; 8]);
    assert_eq!(entries[0].stage, [false; 8]);
}

/// A chain of nested nodes at levels 0..levels, each the lower octant of its parent. Ids are
/// `level + 1`.
fn chain(root_min: [f64; 3], levels: u8, resident_from: u8) -> HashMap<NodeId, CellCutNode> {
    (0..levels)
        .map(|level| {
            let side = SIDE / 2.0_f64.powi(i32::from(level));
            let bounds = SpatialBounds::new(root_min, root_min.map(|value| value + side)).unwrap();
            let stored: Vec<u8> = (level + 3..level + 6).collect();
            let resident = if level >= resident_from {
                stored.clone()
            } else {
                vec![]
            };
            (
                NodeId::new(u64::from(level) + 1),
                CellCutNode {
                    bounds,
                    buildable: (level + 3, level + 5),
                    stored,
                    resident,
                },
            )
        })
        .collect()
}

#[test]
fn descendant_beyond_three_levels_is_rejected() {
    let camera = cameras().remove(0).1;
    let grid = dataset_grid(ORIGIN);
    let nodes = chain(ORIGIN, 5, 0);
    let at = |cut: &[u64]| run(&ids(cut), &nodes, &camera, &grid, ORIGIN, 4.0);
    // Three levels below is the limit.
    assert!(at(&[1, 4]).is_ok());
    assert_eq!(
        at(&[1, 5]),
        Err(CellCutError::DescendantTooFine {
            ancestor: NodeId::new(1),
            descendant: NodeId::new(5)
        })
    );
    // Also through an intermediate cut node that is itself within range.
    assert!(at(&[1, 2, 5]).is_err());
    // The same descendant without a resident level neither masks nor errors.
    let unloaded = chain(ORIGIN, 5, 0)
        .into_iter()
        .map(|(id, mut node)| {
            if id == NodeId::new(5) {
                node.resident.clear();
            }
            (id, node)
        })
        .collect();
    let entries = run(&ids(&[1, 5]), &unloaded, &camera, &grid, ORIGIN, 4.0).unwrap();
    assert_eq!(entries[0].mask, [0; 8]);
    assert_eq!(entries[1].drawn, [None; 8]);
}

#[test]
fn unknown_node_and_empty_levels_are_errors() {
    let camera = cameras().remove(0).1;
    let grid = dataset_grid(ORIGIN);
    let nodes = root_with(&[], &[]);
    assert_eq!(
        run(&ids(&[1]), &nodes, &camera, &grid, ORIGIN, 4.0),
        Err(CellCutError::NoStoredLevels(NodeId::new(1)))
    );
    assert_eq!(
        run(&ids(&[2]), &nodes, &camera, &grid, ORIGIN, 4.0),
        Err(CellCutError::UnknownNode(NodeId::new(2)))
    );
}

#[test]
fn mask_covers_the_descendant_octant() {
    let camera = cameras().remove(0).1;
    let nodes = chain(ORIGIN, 3, 0);
    let entries = run(
        &ids(&[1, 2, 3]),
        &nodes,
        &camera,
        &dataset_grid(ORIGIN),
        ORIGIN,
        4.0,
    )
    .unwrap();
    // Node 2 (k = 1) hides the 4^3 corner; node 3 (k = 2) lies inside it.
    let bits: u32 = entries[0].mask.iter().map(|w| w.count_ones()).sum();
    assert_eq!(bits, 64);
    let bits: u32 = entries[1].mask.iter().map(|w| w.count_ones()).sum();
    assert_eq!(bits, 64);
    assert_eq!(entries[2].mask, [0; 8]);
}

/// Lattice check of the Task 3 criterion; returns the lattice cells inspected.
fn assert_exact_cover(
    entries: &[CellCutEntry],
    nodes: &HashMap<NodeId, CellCutNode>,
    grid: &CellGrid,
    label: &str,
) -> usize {
    let placed: Vec<(u8, [(u64, u64); 3])> = entries
        .iter()
        .map(|entry| {
            let bounds = nodes[&entry.node].bounds;
            let rep = representative_depth(grid, bounds);
            (
                rep,
                std::array::from_fn(|axis| node_range(grid, bounds, rep, axis)),
            )
        })
        .collect();
    for (rep, range) in &placed {
        assert!(
            range.iter().all(|(lo, hi)| hi - lo == 7),
            "{label}: node not on the 8-cell grid at depth {rep}"
        );
    }
    let depth = placed.iter().map(|(rep, _)| *rep).max().unwrap();
    let side = 1_u64 << depth;
    let mut checked = 0;
    for z in 0..side {
        for y in 0..side {
            for x in 0..side {
                let coord = [x, y, z];
                // Independent oracle: the deepest drawn cut node containing the cell draws it.
                let mut expected: Option<(u8, usize)> = None;
                let mut drawers = Vec::new();
                for (index, (entry, (rep, range))) in entries.iter().zip(&placed).enumerate() {
                    let shift = depth - rep;
                    let local: [u64; 3] = std::array::from_fn(|axis| coord[axis] >> shift);
                    if !(0..3).all(|axis| (range[axis].0..=range[axis].1).contains(&local[axis])) {
                        continue;
                    }
                    if entry.drawn[0].is_none() {
                        continue;
                    }
                    if expected.is_none_or(|(deepest, _)| *rep > deepest) {
                        expected = Some((*rep, index));
                    }
                    let rel: [u64; 3] = std::array::from_fn(|axis| local[axis] - range[axis].0);
                    let bit = rel[0] + 8 * rel[1];
                    if entry.mask[rel[2] as usize] >> bit & 1 == 0 {
                        drawers.push(index);
                    }
                }
                // Exactly the deepest drawn node draws inside the union; nobody outside it.
                assert_eq!(
                    drawers,
                    expected
                        .map(|(_, index)| index)
                        .into_iter()
                        .collect::<Vec<_>>(),
                    "{label}: lattice cell {coord:?}"
                );
                checked += 1;
            }
        }
    }
    checked
}

#[test]
fn visible_volume_equals_covered_volume() {
    let replacement = fixture(HierarchySemantics::Replacement);
    let grid = dataset_grid(ORIGIN);
    let mut cases = 0;
    let mut cells = 0;

    let all = fixture_nodes(&replacement);
    let mut partial = all.clone();
    partial.get_mut(&NodeId::new(3)).unwrap().resident.clear();
    let mut scenarios: Vec<(String, Vec<NodeId>, &HashMap<NodeId, CellCutNode>)> = fixture_cuts()
        .into_iter()
        .map(|(name, cut)| (name.to_string(), cut, &all))
        .collect();
    scenarios.push(("mixed_node3_absent".into(), ids(&[1, 2, 3]), &partial));
    for (name, cut, nodes) in &scenarios {
        for (camera_name, camera) in cameras() {
            for target in TARGETS {
                let label = format!("{name}/{camera_name}/{target}px");
                let entries = run(cut, nodes, &camera, &grid, ORIGIN, target).unwrap();
                if name == "mixed_node3_absent" {
                    assert_eq!(entries[2].drawn, [None; 8]);
                    // Node 3 is the +x octant: its mask cells stay visible through the root.
                    assert_eq!(entries[0].mask[0] >> 4 & 1, 0, "{label}");
                    assert_eq!(entries[0].mask[0] & 1, 1, "{label}");
                    assert_eq!(
                        entries[0].mask.iter().map(|w| w.count_ones()).sum::<u32>(),
                        64
                    );
                }
                cells += assert_exact_cover(&entries, nodes, &grid, &label);
                cases += 1;
            }
        }
    }

    // Synthetic mirrors of the T0 fixtures: (name, grid origin, coordinate origin, nodes, cut).
    let crs = [1.0e6, 1.0e6, 1.0e6];
    let deep_leaf: HashMap<NodeId, CellCutNode> = {
        let mut nodes = chain(ORIGIN, 3, 0);
        // The level-2 leaf stores a long chain down to relative depth 16.
        nodes.get_mut(&NodeId::new(3)).unwrap().stored = vec![5, 8, 12, 18];
        nodes.get_mut(&NodeId::new(3)).unwrap().buildable = (5, 18);
        nodes.get_mut(&NodeId::new(3)).unwrap().resident = vec![5, 8, 12, 18];
        nodes
    };
    let synthetic: Vec<Synthetic> = vec![
        ("crs_root_level0", crs, crs, chain(crs, 3, 0)),
        ("local_cloud", [0.0; 3], [0.0; 3], chain([0.0; 3], 3, 0)),
        ("deep_leaf", ORIGIN, ORIGIN, deep_leaf),
    ];
    for (name, root_min, origin, nodes) in &synthetic {
        let grid = dataset_grid(*root_min);
        for cut in [ids(&[1, 2, 3]), ids(&[1, 3]), ids(&[2, 3])] {
            for (camera_name, camera) in cameras() {
                for target in TARGETS {
                    let label = format!("{name}/{cut:?}/{camera_name}/{target}px");
                    let entries = run(&cut, nodes, &camera, &grid, *origin, target).unwrap();
                    cells += assert_exact_cover(&entries, nodes, &grid, &label);
                    cases += 1;
                }
            }
        }
    }
    eprintln!("volume check: {cases} cases, {cells} lattice cells");
    assert!(cases >= 48 + 108);
}

const GOLDEN: &[(&str, u64)] = &[
    ("root/fit/2px", 0x874f97c38baa823a),
    ("root/fit/4px", 0x7d1169d9327e7815),
    ("root/fit/8px", 0x42f9363f7b47c3c8),
    ("root/orbit/2px", 0x874f97c38baa823a),
    ("root/orbit/4px", 0x2fa845aa5f346e25),
    ("root/orbit/8px", 0xc94fe0686b55ff68),
    ("root/zoom/2px", 0x874f97c38baa823a),
    ("root/zoom/4px", 0x874f97c38baa823a),
    ("root/zoom/8px", 0xf0c6b585db2b31a5),
    ("root/pan/2px", 0x874f97c38baa823a),
    ("root/pan/4px", 0x68ec0d2c4b5afa9f),
    ("root/pan/8px", 0x8f0c75bc680f980a),
    ("children/fit/2px", 0xa51da02388ee6c85),
    ("children/fit/4px", 0xa51da02388ee6c85),
    ("children/fit/8px", 0xa51da02388ee6c85),
    ("children/orbit/2px", 0xa51da02388ee6c85),
    ("children/orbit/4px", 0xa51da02388ee6c85),
    ("children/orbit/8px", 0xa51da02388ee6c85),
    ("children/zoom/2px", 0xa51da02388ee6c85),
    ("children/zoom/4px", 0xa51da02388ee6c85),
    ("children/zoom/8px", 0xa51da02388ee6c85),
    ("children/pan/2px", 0xa51da02388ee6c85),
    ("children/pan/4px", 0xa51da02388ee6c85),
    ("children/pan/8px", 0xa51da02388ee6c85),
    ("mixed/fit/2px", 0xcfb3dd69675f533e),
    ("mixed/fit/4px", 0x3d645e2f3ea980ba),
    ("mixed/fit/8px", 0xde418b595f37d282),
    ("mixed/orbit/2px", 0xcfb3dd69675f533e),
    ("mixed/orbit/4px", 0xa23a9cfb8cb4df9e),
    ("mixed/orbit/8px", 0x5301c3f944a73fce),
    ("mixed/zoom/2px", 0xcfb3dd69675f533e),
    ("mixed/zoom/4px", 0xcfb3dd69675f533e),
    ("mixed/zoom/8px", 0xcfb3dd69675f533e),
    ("mixed/pan/2px", 0xcfb3dd69675f533e),
    ("mixed/pan/4px", 0x5ee017fede23060c),
    ("mixed/pan/8px", 0xcda82b8f251e4148),
    ("wide_chain/fit/2px", 0x5782cd16bba4da29),
    ("wide_chain/fit/4px", 0x6ddc9d7207e4afc9),
    ("wide_chain/fit/8px", 0xb47d3a451a1e318d),
    ("wide_chain/orbit/2px", 0x1b83ad1575279081),
    ("wide_chain/orbit/4px", 0x403a506cb10ea159),
    ("wide_chain/orbit/8px", 0xe894eeff470f1055),
    ("wide_chain/zoom/2px", 0x8b1110b3ed120ded),
    ("wide_chain/zoom/4px", 0x63d9c91115e43e69),
    ("wide_chain/zoom/8px", 0x3e44e15bac392dc1),
    ("wide_chain/pan/2px", 0x6047938e45ba3559),
    ("wide_chain/pan/4px", 0x016225300c8668c9),
    ("wide_chain/pan/8px", 0xae20372b87dfb69d),
];

#[test]
fn cell_cut_digest_golden() {
    let nodes = fixture_nodes(&fixture(HierarchySemantics::Replacement));
    let grid = dataset_grid(ORIGIN);
    let mut actual = Vec::new();
    for (name, cut) in fixture_cuts() {
        for (camera_name, camera) in cameras() {
            for target in TARGETS {
                let entries = run(&cut, &nodes, &camera, &grid, ORIGIN, target).unwrap();
                actual.push((
                    format!("{name}/{camera_name}/{target}px"),
                    cell_cut_digest(&entries),
                ));
            }
        }
    }
    // The fixture nodes store one or two levels, so every choice above is source-limited; a
    // wide synthetic chain pins the target-driven choice and the mask too.
    let mut wide = chain(ORIGIN, 3, 0);
    for node in wide.values_mut() {
        let first = node.stored[0];
        node.stored = (first..first + 8).collect();
        node.buildable = (first, first + 7);
        node.resident = node.stored.clone();
    }
    let cut = ids(&[1, 2, 3]);
    for (camera_name, camera) in cameras() {
        for target in TARGETS {
            let entries = run(&cut, &wide, &camera, &grid, ORIGIN, target).unwrap();
            actual.push((
                format!("wide_chain/{camera_name}/{target}px"),
                cell_cut_digest(&entries),
            ));
        }
    }
    let matches = actual.len() == GOLDEN.len()
        && actual
            .iter()
            .zip(GOLDEN)
            .all(|(a, g)| a.0 == g.0 && a.1 == g.1);
    if !matches {
        let table: String = actual
            .iter()
            .map(|(name, hash)| format!("    (\"{name}\", {hash:#018x}),\n"))
            .collect();
        panic!("digest table changed; actual:\n{table}");
    }
}

fn level_with_offsets(offsets: [u32; 9]) -> NodeCellLevel {
    NodeCellLevel {
        depth: 4,
        cells: Vec::new(),
        octant_offsets: Some(offsets),
    }
}

fn level_cell() -> crate::NodeCell {
    crate::NodeCell {
        coord: [0; 3],
        rgb_sum: [0; 3],
        intensity_sum: 0.0,
        intensity_count: 0,
        count: 1,
        classification: -1,
        z_min: 0.0,
        z_max: 0.0,
    }
}

#[test]
fn octant_ranges_coalesce_adjacent_octants() {
    // Octants 0, 1 and 3 hold 2 cells each, 2 is empty, 4..8 hold 1 each.
    let level = level_with_offsets([0, 2, 4, 4, 6, 7, 8, 9, 10]);
    assert_eq!(octant_ranges(&level, 0b0000_0011), vec![0..4]);
    assert_eq!(octant_ranges(&level, 0b0000_1011), vec![0..6]);
    assert_eq!(octant_ranges(&level, 0b0000_0111), vec![0..4]);
    assert_eq!(octant_ranges(&level, 0b1111_1111), vec![0..10]);
    assert_eq!(octant_ranges(&level, 0b0001_0001), vec![0..2, 6..7]);
    assert!(octant_ranges(&level, 0).is_empty());

    // A level with every cell in octant 7 is not a spanning level.
    let last = level_with_offsets([0, 0, 0, 0, 0, 0, 0, 0, 5]);
    assert!((0..7).all(|o| last.octant_range(o).is_empty()));
    assert_eq!(last.octant_range(7), 0..5);
    assert!(octant_ranges(&last, 1).is_empty());
    assert_eq!(octant_ranges(&last, 0x80), vec![0..5]);

    // A spanning level is the whole level for every octant, drawn once.
    let mut spanning = level_with_offsets([0; 9]);
    spanning.octant_offsets = None;
    spanning.cells = vec![level_cell(); 5];
    assert!((0..8).all(|o| spanning.octant_range(o) == (0..5)));
    assert_eq!(octant_ranges(&spanning, 1), vec![0..5]);
    assert_eq!(octant_ranges(&spanning, 0xff), vec![0..5]);
    assert!(octant_ranges(&spanning, 0).is_empty());
}

#[test]
fn draws_and_required_levels_on_uniform_entries() {
    let camera = cameras().remove(0).1;
    let nodes = root_with(&[3, 4, 5], &[4, 3]);
    let entries = run(
        &ids(&[1]),
        &nodes,
        &camera,
        &dataset_grid(ORIGIN),
        ORIGIN,
        0.001,
    )
    .unwrap();
    assert_eq!(entries[0].required_levels(), vec![4]);
    assert_eq!(entries[0].draws(), vec![(4, 0xff)]);
    let nodes = root_with(&[3, 4, 5], &[]);
    let entries = run(
        &ids(&[1]),
        &nodes,
        &camera,
        &dataset_grid(ORIGIN),
        ORIGIN,
        0.001,
    )
    .unwrap();
    assert!(entries[0].required_levels().is_empty());
    assert!(entries[0].draws().is_empty());
}
