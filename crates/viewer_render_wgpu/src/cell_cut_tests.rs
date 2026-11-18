//! Task 3 pass criteria: the level choice follows RQ-09, residency is all-or-nothing, and the
//! masks make the visible volume equal the covered volume with no overlap.

use std::collections::HashMap;

use crate::NodeCells;

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

/// The cells the fixture builds for node `id` (local frame, like `refinement_tests.rs`), and its
/// bounds in the dataset frame.
fn fixture_cells(fixture: &Fixture, id: NodeId) -> (SpatialBounds, NodeCells) {
    let local_grid = CellGrid {
        root_min: [0.0; 3],
        root_side: SIDE,
    };
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
    (bounds, cells)
}

/// Stored levels of every fixture node. Its range is the stored span, so every wanted depth is a
/// stored one.
fn fixture_nodes(fixture: &Fixture) -> HashMap<NodeId, CellCutNode> {
    (1..=9)
        .map(NodeId::new)
        .map(|id| {
            let (bounds, cells) = fixture_cells(fixture, id);
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
/// whose cube, at the octant point nearest the eye, would not refine (ADR 0054: its smallest of 8
/// children projects below `target`), else the finest.
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
        let half = side / 2.0;
        let mut smallest = f64::INFINITY;
        for child in 0..8_usize {
            let child_min: [f64; 3] =
                std::array::from_fn(|a| cube_min[a] + half * ((child >> a & 1) as f64));
            let bounds = SpatialBounds::new(child_min, child_min.map(|v| v + half)).unwrap();
            smallest = smallest.min(view.projected_voxel_edge_pixels(bounds, origin));
        }
        smallest
    };
    // Brute force: test every stored depth; the choice is the coarsest passing one.
    let passing: Vec<u8> = stored
        .iter()
        .copied()
        .filter(|d| edge(*d) < target)
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
    let fixture = fixture(HierarchySemantics::Replacement);
    let grid = dataset_grid(ORIGIN);
    let (mut checked, mut capped) = (0, 0);
    for id in (1..=9).map(NodeId::new) {
        if fixture.hierarchy.is_leaf(id) {
            continue;
        }
        let (bounds, cells) = fixture_cells(&fixture, id);
        let stored: Vec<u8> = cells.levels.iter().map(|level| level.depth).collect();
        let (first, last) = cells.buildable;
        assert!(
            last <= first + 5,
            "node {} builds {first}..={last}",
            id.get()
        );
        let nodes = HashMap::from([(
            id,
            CellCutNode {
                bounds,
                buildable: cells.buildable,
                resident: stored.clone(),
                stored,
            },
        )]);
        // Eye inside the node, looking at a point of it: the finest wish is the deepest depth.
        let mut camera = cameras().remove(0).1;
        let inside = bounds.min().map(|v| v + 0.25 * SIDE / 2.0);
        let target = bounds.min().map(|v| v + 0.5 * SIDE / 2.0);
        assert!(camera.set_pose(inside, target));
        for px in [0.01, 1.0] {
            let entries = run(&[id], &nodes, &camera, &grid, ORIGIN, px).unwrap();
            for wanted in entries[0].wanted.iter().flatten() {
                assert!(
                    *wanted <= first + 5,
                    "node {}: {wanted} at {px}px",
                    id.get()
                );
                checked += 1;
                capped += usize::from(*wanted == last);
            }
        }
    }
    assert!(checked > 0, "no octant was wanted");
    assert!(capped > 0, "no octant reached the end of its range");
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
    // The target is not reached while the wanted depth is not drawn (R3-Q3): a stage.
    for o in 0..8 {
        let wanted = entries[0].wanted[o].unwrap();
        assert_eq!(entries[0].stage[o], wanted > stored_finest, "octant {o}");
    }
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

/// Eye inside the geometric octant, data far in front or behind: the probe measures the data.
#[test]
fn a_probe_of_the_data_never_asks_the_finest_for_a_degenerate_box() {
    let huge = SpatialBounds::new([-1000.0; 3], [1000.0; 3]).unwrap();
    let grid = CellGrid {
        root_min: [-1000.0; 3],
        root_side: 2000.0,
    };
    let mut camera = cameras().remove(0).1;
    // Eye inside octant 6, looking along -x.
    assert!(camera.set_pose([500.0, 300.0, -200.0], [0.0, 300.0, -200.0]));
    let view = view(&camera);
    let at = |probe| octant_wanted_depth_in(&view, &grid, [0.0; 3], huge, 6, probe, (3, 9), 8.0);
    let data = |min: [f64; 3], side: f64| ProbeBox::Data(cube_box(min, side));
    // Octant 0 of a 200-cube holds exactly the data box: the geometric answer for it.
    let reference_node = SpatialBounds::new([0.0, 250.0, -250.0], [200.0, 450.0, -50.0]).unwrap();
    let reference = octant_wanted_depth(&view, &grid, [0.0; 3], reference_node, 0, (3, 9), 8.0);
    assert_eq!(
        at(ProbeBox::Geometric),
        9,
        "the geometric box reaches the eye"
    );
    assert!(reference < 9, "setup: the target is met at that distance");
    assert_eq!(at(data([0.0, 250.0, -250.0], 100.0)), reference);
    // All data behind the eye plane: the coarsest depth.
    assert_eq!(at(data([600.0, 250.0, -250.0], 100.0)), 3);
    assert_eq!(at(ProbeBox::Empty), 3);
}

fn cube_box(min: [f64; 3], side: f64) -> SpatialBounds {
    SpatialBounds::new(min, min.map(|v| v + side)).unwrap()
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
fn digest_tells_a_hidden_octant_from_depth_zero() {
    let entry = |wanted: Option<u8>| CellCutEntry {
        node: NodeId::new(1),
        wanted: [wanted; 8],
        drawn: [wanted; 8],
        stage: [false; 8],
        step: [None; 8],
        range: None,
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
    // R3-Q21: the closest resident level, a stage when coarser than wanted ...
    assert_eq!(resident_depth(5, &[3, 7, 4]), Some((4, true)));
    assert_eq!(resident_depth(5, &[4, 3]), Some((4, true)));
    // ... a tie goes to the finer one, which is no stage.
    assert_eq!(resident_depth(5, &[4, 6]), Some((6, false)));
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
        LodMode::Cell,
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
    ("root/fit/2px", 0x7d1169d9327e7815),
    ("root/fit/4px", 0x42f9363f7b47c3c8),
    ("root/fit/8px", 0x5153783bc29513cb),
    ("root/orbit/2px", 0x2fa845aa5f346e25),
    ("root/orbit/4px", 0xc94fe0686b55ff68),
    ("root/orbit/8px", 0xa8e7a617daf0527b),
    ("root/zoom/2px", 0x874f97c38baa823a),
    ("root/zoom/4px", 0xf0c6b585db2b31a5),
    ("root/zoom/8px", 0x19329f184c5ad560),
    ("root/pan/2px", 0x68ec0d2c4b5afa9f),
    ("root/pan/4px", 0x8f0c75bc680f980a),
    ("root/pan/8px", 0x7bbd7c33444d8725),
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
    ("mixed/fit/2px", 0x3d645e2f3ea980ba),
    ("mixed/fit/4px", 0xde418b595f37d282),
    ("mixed/fit/8px", 0xdb555f7c706e7e1e),
    ("mixed/orbit/2px", 0xa23a9cfb8cb4df9e),
    ("mixed/orbit/4px", 0x5301c3f944a73fce),
    ("mixed/orbit/8px", 0xb8531a79d519f47e),
    ("mixed/zoom/2px", 0xcfb3dd69675f533e),
    ("mixed/zoom/4px", 0xcfb3dd69675f533e),
    ("mixed/zoom/8px", 0x3d645e2f3ea980ba),
    ("mixed/pan/2px", 0x5ee017fede23060c),
    ("mixed/pan/4px", 0xcda82b8f251e4148),
    ("mixed/pan/8px", 0x9d1b8179261646b0),
    ("wide_chain/fit/2px", 0x6ddc9d7207e4afc9),
    ("wide_chain/fit/4px", 0xb47d3a451a1e318d),
    ("wide_chain/fit/8px", 0x53542e0c8f481245),
    ("wide_chain/orbit/2px", 0x403a506cb10ea159),
    ("wide_chain/orbit/4px", 0xe894eeff470f1055),
    ("wide_chain/orbit/8px", 0x89c1f98705838a65),
    ("wide_chain/zoom/2px", 0x666bc1dea4c36f31),
    ("wide_chain/zoom/4px", 0xd5ddb3e23053144d),
    ("wide_chain/zoom/8px", 0xf779a7f9f1450615),
    ("wide_chain/pan/2px", 0x016225300c8668c9),
    ("wide_chain/pan/4px", 0xae20372b87dfb69d),
    ("wide_chain/pan/8px", 0x932b0084c615954d),
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

/// `run` through the cell-mode path with the range inputs the cell pool passes (no `prev`: every
/// octant draws its closest resident level).
fn run_cell_mode(
    cut: &[NodeId],
    nodes: &HashMap<NodeId, CellCutNode>,
    camera: &OrbitCamera,
    grid: &CellGrid,
    origin: [f64; 3],
    target: f64,
) -> Vec<CellCutEntry> {
    run_cell_mode_from(cut, nodes, camera, grid, origin, target, None)
}

/// `run_cell_mode` stepping from `prev`, what each node drew in the last applied cut.
fn run_cell_mode_from(
    cut: &[NodeId],
    nodes: &HashMap<NodeId, CellCutNode>,
    camera: &OrbitCamera,
    grid: &CellGrid,
    origin: [f64; 3],
    target: f64,
    prev: Option<&PrevDrawn<'_>>,
) -> Vec<CellCutEntry> {
    let view = view(camera);
    cell_cut_ranged(
        cut,
        &|id| nodes.get(&id).cloned(),
        grid,
        LodMode::Cell,
        &|_, octant, data| {
            octant_wanted_depth(
                &view,
                grid,
                origin,
                data.bounds,
                octant,
                data.buildable,
                target,
            )
        },
        Some(&|_, data| {
            (
                view_range_lo(
                    &view,
                    grid,
                    origin,
                    Some(data.bounds),
                    data.buildable,
                    target,
                    None,
                )
                .unwrap_or(data.buildable.0),
                octants_in_front(&view, origin, data.bounds, [ProbeBox::Geometric; 8]),
            )
        }),
        prev,
    )
    .unwrap()
}

/// FNV-1a of `cell_cut_digest` (wanted, drawn, stage, mask) plus the fields it leaves out: `step`
/// and `range`.
fn cell_mode_digest(entries: &[CellCutEntry]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(&cell_cut_digest(entries).to_le_bytes());
    for entry in entries {
        for step in entry.step {
            feed(&step.map_or([0, 0], |depth| [1, depth]));
        }
        feed(&entry.range.map_or([0, 0, 0], |(lo, hi)| [1, lo, hi]));
    }
    hash
}

const CELL_MODE_GOLDEN: &[(&str, u64)] = &[
    ("root/fit/2px", 0xb7a6108de90de984),
    ("root/fit/4px", 0xd2299d92d9d7c4f8),
    ("root/fit/8px", 0x75547f15b3e6b04d),
    ("root/orbit/2px", 0xafbbad41b3073b15),
    ("root/orbit/4px", 0x68fbbe553bc0df38),
    ("root/orbit/8px", 0x95373c17ce63458c),
    ("root/zoom/2px", 0xac3f07980f6807b6),
    ("root/zoom/4px", 0x601f4a2add672d4a),
    ("root/zoom/8px", 0x34132421ab97fce2),
    ("root/pan/2px", 0xc29416bf891a7071),
    ("root/pan/4px", 0x685de424d4ecc74d),
    ("root/pan/8px", 0x32c6e06d86751818),
    ("children/fit/2px", 0x13bde445aeb00213),
    ("children/fit/4px", 0x13bde445aeb00213),
    ("children/fit/8px", 0x13bde445aeb00213),
    ("children/orbit/2px", 0x13bde445aeb00213),
    ("children/orbit/4px", 0x13bde445aeb00213),
    ("children/orbit/8px", 0x13bde445aeb00213),
    ("children/zoom/2px", 0x13bde445aeb00213),
    ("children/zoom/4px", 0x13bde445aeb00213),
    ("children/zoom/8px", 0x13bde445aeb00213),
    ("children/pan/2px", 0x13bde445aeb00213),
    ("children/pan/4px", 0x13bde445aeb00213),
    ("children/pan/8px", 0x13bde445aeb00213),
    ("mixed/fit/2px", 0x694d8cc7c5da4624),
    ("mixed/fit/4px", 0x1f4de304ed6044aa),
    ("mixed/fit/8px", 0xab84b87227b8b910),
    ("mixed/orbit/2px", 0xfc02a455261eb685),
    ("mixed/orbit/4px", 0x3210ab3c09ebb2c1),
    ("mixed/orbit/8px", 0xa3e5ba3f2093d275),
    ("mixed/zoom/2px", 0x8324526e8e5a7e4a),
    ("mixed/zoom/4px", 0xe77a3a919ae1de0f),
    ("mixed/zoom/8px", 0xeab80e4d1b711be2),
    ("mixed/pan/2px", 0xc7742bad8a78b466),
    ("mixed/pan/4px", 0x897ca9ee55c730a7),
    ("mixed/pan/8px", 0x59bd7b46b6b800c8),
    ("wide_chain/fit/2px", 0x0ea8f8bb1d4784b0),
    ("wide_chain/fit/4px", 0x44ccb8b4c6c48b69),
    ("wide_chain/fit/8px", 0x33d7fc184ec5f146),
    ("wide_chain/orbit/2px", 0x1bd3f5495a1e53d2),
    ("wide_chain/orbit/4px", 0xbd97125d9fc8eaa3),
    ("wide_chain/orbit/8px", 0x826477681b53cdda),
    ("wide_chain/zoom/2px", 0xae847aca4bfc48a0),
    ("wide_chain/zoom/4px", 0xda8655a91500bd9d),
    ("wide_chain/zoom/8px", 0x56237d61360ab14a),
    ("wide_chain/pan/2px", 0x4a6f77f53469c8c1),
    ("wide_chain/pan/4px", 0x2ad6c97ab3882a7b),
    ("wide_chain/pan/8px", 0x2891f51d489f0cbe),
];

/// The golden fixtures of `cell_cut_digest_golden`, through the cell-mode path. Deleting octant
/// mode must leave this table unchanged.
#[test]
fn cell_cut_digest_golden_in_cell_mode() {
    let nodes = fixture_nodes(&fixture(HierarchySemantics::Replacement));
    let grid = dataset_grid(ORIGIN);
    let mut actual = Vec::new();
    for (name, cut) in fixture_cuts() {
        for (camera_name, camera) in cameras() {
            for target in TARGETS {
                let entries = run_cell_mode(&cut, &nodes, &camera, &grid, ORIGIN, target);
                actual.push((
                    format!("{name}/{camera_name}/{target}px"),
                    cell_mode_digest(&entries),
                ));
            }
        }
    }
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
            let entries = run_cell_mode(&cut, &wide, &camera, &grid, ORIGIN, target);
            actual.push((
                format!("wide_chain/{camera_name}/{target}px"),
                cell_mode_digest(&entries),
            ));
        }
    }
    let matches = actual.len() == CELL_MODE_GOLDEN.len()
        && actual
            .iter()
            .zip(CELL_MODE_GOLDEN)
            .all(|(a, g)| a.0 == g.0 && a.1 == g.1);
    if !matches {
        let table: String = actual
            .iter()
            .map(|(name, hash)| format!("    (\"{name}\", {hash:#018x}),\n"))
            .collect();
        panic!("cell-mode digest table changed; actual:\n{table}");
    }
}

/// Every fixture of the cell-mode goldens: name, nodes, cut.
fn cell_mode_fixtures() -> Vec<(String, HashMap<NodeId, CellCutNode>, Vec<NodeId>)> {
    let replacement = fixture_nodes(&fixture(HierarchySemantics::Replacement));
    let mut fixtures: Vec<_> = fixture_cuts()
        .into_iter()
        .map(|(name, cut)| (name.to_string(), replacement.clone(), cut))
        .collect();
    let mut wide = chain(ORIGIN, 3, 0);
    for node in wide.values_mut() {
        let first = node.stored[0];
        node.stored = (first..first + 8).collect();
        node.buildable = (first, first + 7);
        node.resident = node.stored.clone();
    }
    fixtures.push(("wide_chain".to_string(), wide, ids(&[1, 2, 3])));
    fixtures
}

fn assert_cell_mode_table(actual: &[(String, u64)], golden: &[(&str, u64)]) {
    let matches = actual.len() == golden.len()
        && actual
            .iter()
            .zip(golden)
            .all(|(a, g)| a.0 == g.0 && a.1 == g.1);
    if !matches {
        let table: String = actual
            .iter()
            .map(|(name, hash)| format!("    (\"{name}\", {hash:#018x}),\n"))
            .collect();
        panic!("cell-mode digest table changed; actual:\n{table}");
    }
}

const CELL_PLAIN_GOLDEN: &[(&str, u64)] = &[
    ("root/fit/2px", 0xaef9278de422fa18),
    ("root/fit/4px", 0xc9795d92d4ea0be0),
    ("root/fit/8px", 0x7e082615b8d74e41),
    ("root/orbit/2px", 0xb85afe41b7e69ddd),
    ("root/orbit/4px", 0x604b7e5536d32620),
    ("root/orbit/8px", 0x8c838d17c9729a00),
    ("root/zoom/2px", 0xa38496980a71a6d6),
    ("root/zoom/4px", 0x68cf892ae254e4af),
    ("root/zoom/8px", 0x2b7aaf21a6be7b9c),
    ("root/pan/2px", 0xb9fef7bf8443b724),
    ("root/pan/4px", 0x5fc56024d0132c8a),
    ("root/pan/8px", 0x3b7df96d8b68ad99),
    ("children/fit/2px", 0x037adad8de9fe4bb),
    ("children/fit/4px", 0x037adad8de9fe4bb),
    ("children/fit/8px", 0x037adad8de9fe4bb),
    ("children/orbit/2px", 0x037adad8de9fe4bb),
    ("children/orbit/4px", 0x037adad8de9fe4bb),
    ("children/orbit/8px", 0x037adad8de9fe4bb),
    ("children/zoom/2px", 0x037adad8de9fe4bb),
    ("children/zoom/4px", 0x037adad8de9fe4bb),
    ("children/zoom/8px", 0x037adad8de9fe4bb),
    ("children/pan/2px", 0x037adad8de9fe4bb),
    ("children/pan/4px", 0x037adad8de9fe4bb),
    ("children/pan/8px", 0x037adad8de9fe4bb),
    ("mixed/fit/2px", 0x4faa2ba082dd7c5e),
    ("mixed/fit/4px", 0x61994f65d21e119c),
    ("mixed/fit/8px", 0x1d4a879a36dccefa),
    ("mixed/orbit/2px", 0x6ff2242df0e3ea77),
    ("mixed/orbit/4px", 0xa850a06eea9f0a23),
    ("mixed/orbit/8px", 0xa97a938e709d65eb),
    ("mixed/zoom/2px", 0x6fbaf3008d0c0e58),
    ("mixed/zoom/4px", 0x6fbaf3008d0c0e58),
    ("mixed/zoom/8px", 0x4faa2ba082dd7c5e),
    ("mixed/pan/2px", 0x30c7dedfc2986571),
    ("mixed/pan/4px", 0x435ad8cbd0999bb2),
    ("mixed/pan/8px", 0xd86263a68ac2fb37),
    ("wide_chain/fit/2px", 0x8c8444837cfa64a2),
    ("wide_chain/fit/4px", 0xe3bcd88762b1ce2f),
    ("wide_chain/fit/8px", 0xca00b8b6aacb2e20),
    ("wide_chain/orbit/2px", 0xbdfe2431a72fe990),
    ("wide_chain/orbit/4px", 0x02fecc09d5d4a459),
    ("wide_chain/orbit/8px", 0x8b0bf8214166cea4),
    ("wide_chain/zoom/2px", 0x981196c8652708ae),
    ("wide_chain/zoom/4px", 0x57ebc499b2d144f6),
    ("wide_chain/zoom/8px", 0x7dd6d7bde743beb4),
    ("wide_chain/pan/2px", 0x907628ca0a00650c),
    ("wide_chain/pan/4px", 0x64dd4a0b49245970),
    ("wide_chain/pan/8px", 0x31a8cc73280abf9e),
];

/// The plain `cell_cut` computes through `cell_cut_with` (no range input, no previous state).
/// It computes in cell mode: this pins what it returns.
#[test]
fn cell_cut_digest_golden_of_the_plain_cut_in_cell_mode() {
    let grid = dataset_grid(ORIGIN);
    let mut actual = Vec::new();
    for (name, nodes, cut) in cell_mode_fixtures() {
        for (camera_name, camera) in cameras() {
            for target in TARGETS {
                let view = view(&camera);
                let entries = cell_cut_with(
                    &cut,
                    &|id| nodes.get(&id).cloned(),
                    &grid,
                    LodMode::Cell,
                    &|_, octant, data| {
                        octant_wanted_depth(
                            &view,
                            &grid,
                            ORIGIN,
                            data.bounds,
                            octant,
                            data.buildable,
                            target,
                        )
                    },
                )
                .unwrap();
                actual.push((
                    format!("{name}/{camera_name}/{target}px"),
                    cell_mode_digest(&entries),
                ));
            }
        }
    }
    assert_cell_mode_table(&actual, CELL_PLAIN_GOLDEN);
}

const CELL_STEPPING_GOLDEN: &[(&str, u64)] = &[
    ("root/fit>orbit/8>2px/all_resident", 0xafbbad41b3073b15),
    ("root/fit>orbit/8>2px/only_drawn", 0xafbbad41b3073b15),
    ("root/fit>orbit/2>8px/all_resident", 0x95373c17ce63458c),
    ("root/fit>orbit/2>8px/only_drawn", 0x95373c17ce63458c),
    ("root/orbit>zoom/8>2px/all_resident", 0xac3f07980f6807b6),
    ("root/orbit>zoom/8>2px/only_drawn", 0xac3f07980f6807b6),
    ("root/orbit>zoom/2>8px/all_resident", 0x34132421ab97fce2),
    ("root/orbit>zoom/2>8px/only_drawn", 0x34132421ab97fce2),
    ("root/zoom>pan/8>2px/all_resident", 0xc29416bf891a7071),
    ("root/zoom>pan/8>2px/only_drawn", 0xc29416bf891a7071),
    ("root/zoom>pan/2>8px/all_resident", 0x32c6e06d86751818),
    ("root/zoom>pan/2>8px/only_drawn", 0x32c6e06d86751818),
    ("root/pan>fit/8>2px/all_resident", 0xb7a6108de90de984),
    ("root/pan>fit/8>2px/only_drawn", 0xb7a6108de90de984),
    ("root/pan>fit/2>8px/all_resident", 0x75547f15b3e6b04d),
    ("root/pan>fit/2>8px/only_drawn", 0x75547f15b3e6b04d),
    ("children/fit>orbit/8>2px/all_resident", 0x13bde445aeb00213),
    ("children/fit>orbit/8>2px/only_drawn", 0x13bde445aeb00213),
    ("children/fit>orbit/2>8px/all_resident", 0x13bde445aeb00213),
    ("children/fit>orbit/2>8px/only_drawn", 0x13bde445aeb00213),
    ("children/orbit>zoom/8>2px/all_resident", 0x13bde445aeb00213),
    ("children/orbit>zoom/8>2px/only_drawn", 0x13bde445aeb00213),
    ("children/orbit>zoom/2>8px/all_resident", 0x13bde445aeb00213),
    ("children/orbit>zoom/2>8px/only_drawn", 0x13bde445aeb00213),
    ("children/zoom>pan/8>2px/all_resident", 0x13bde445aeb00213),
    ("children/zoom>pan/8>2px/only_drawn", 0x13bde445aeb00213),
    ("children/zoom>pan/2>8px/all_resident", 0x13bde445aeb00213),
    ("children/zoom>pan/2>8px/only_drawn", 0x13bde445aeb00213),
    ("children/pan>fit/8>2px/all_resident", 0x13bde445aeb00213),
    ("children/pan>fit/8>2px/only_drawn", 0x13bde445aeb00213),
    ("children/pan>fit/2>8px/all_resident", 0x13bde445aeb00213),
    ("children/pan>fit/2>8px/only_drawn", 0x13bde445aeb00213),
    ("mixed/fit>orbit/8>2px/all_resident", 0xfc02a455261eb685),
    ("mixed/fit>orbit/8>2px/only_drawn", 0xfc02a455261eb685),
    ("mixed/fit>orbit/2>8px/all_resident", 0xa3e5ba3f2093d275),
    ("mixed/fit>orbit/2>8px/only_drawn", 0xa3e5ba3f2093d275),
    ("mixed/orbit>zoom/8>2px/all_resident", 0x8324526e8e5a7e4a),
    ("mixed/orbit>zoom/8>2px/only_drawn", 0x8324526e8e5a7e4a),
    ("mixed/orbit>zoom/2>8px/all_resident", 0xeab80e4d1b711be2),
    ("mixed/orbit>zoom/2>8px/only_drawn", 0xeab80e4d1b711be2),
    ("mixed/zoom>pan/8>2px/all_resident", 0xc7742bad8a78b466),
    ("mixed/zoom>pan/8>2px/only_drawn", 0xc7742bad8a78b466),
    ("mixed/zoom>pan/2>8px/all_resident", 0x59bd7b46b6b800c8),
    ("mixed/zoom>pan/2>8px/only_drawn", 0x59bd7b46b6b800c8),
    ("mixed/pan>fit/8>2px/all_resident", 0x694d8cc7c5da4624),
    ("mixed/pan>fit/8>2px/only_drawn", 0x694d8cc7c5da4624),
    ("mixed/pan>fit/2>8px/all_resident", 0xab84b87227b8b910),
    ("mixed/pan>fit/2>8px/only_drawn", 0xab84b87227b8b910),
    (
        "wide_chain/fit>orbit/8>2px/all_resident",
        0x8d3223155e8f482c,
    ),
    ("wide_chain/fit>orbit/8>2px/only_drawn", 0x788076b222402e2e),
    (
        "wide_chain/fit>orbit/2>8px/all_resident",
        0x682f9061b683c852,
    ),
    ("wide_chain/fit>orbit/2>8px/only_drawn", 0x7cee8d33d4ffc355),
    (
        "wide_chain/orbit>zoom/8>2px/all_resident",
        0xee2c298f8bcb4adb,
    ),
    ("wide_chain/orbit>zoom/8>2px/only_drawn", 0x620048accc8ff08d),
    (
        "wide_chain/orbit>zoom/2>8px/all_resident",
        0x1bac65e298f4eaa1,
    ),
    ("wide_chain/orbit>zoom/2>8px/only_drawn", 0xac5b719e1b7ceab5),
    ("wide_chain/zoom>pan/8>2px/all_resident", 0x05c921c96aa71456),
    ("wide_chain/zoom>pan/8>2px/only_drawn", 0x5ac037099eb560e3),
    ("wide_chain/zoom>pan/2>8px/all_resident", 0x2d8b2dd759a88572),
    ("wide_chain/zoom>pan/2>8px/only_drawn", 0x182bac955a61f15b),
    ("wide_chain/pan>fit/8>2px/all_resident", 0xe0c0be99c6cfcaed),
    ("wide_chain/pan>fit/8>2px/only_drawn", 0x80af7c903eae8489),
    ("wide_chain/pan>fit/2>8px/all_resident", 0x2f1e84445534ba62),
    ("wide_chain/pan>fit/2>8px/only_drawn", 0xeaba2ebeffca6ae0),
];

/// Two frames in cell mode: frame 2 steps from what frame 1 drew, as the pool passes it, with
/// every depth resident and with only the depths frame 1 drew resident.
#[test]
fn cell_cut_digest_golden_with_a_previous_cut_in_cell_mode() {
    let grid = dataset_grid(ORIGIN);
    let cams = cameras();
    let mut actual = Vec::new();
    let (mut stepping, mut staged) = (0, 0);
    for (name, nodes, cut) in cell_mode_fixtures() {
        for (i, (from, camera1)) in cams.iter().enumerate() {
            let (to, camera2) = &cams[(i + 1) % cams.len()];
            for (t1, t2) in [(8.0, 2.0), (2.0, 8.0)] {
                let frame1 = run_cell_mode(&cut, &nodes, camera1, &grid, ORIGIN, t1);
                let drawn: HashMap<NodeId, [Option<u8>; 8]> =
                    frame1.iter().map(|e| (e.node, e.drawn)).collect();
                let prev = |id: NodeId| PrevState {
                    drawn: drawn.get(&id).copied().unwrap_or([None; 8]),
                    timed_out: [false; 8],
                };
                let mut only_drawn = nodes.clone();
                for entry in &frame1 {
                    only_drawn.get_mut(&entry.node).unwrap().resident = entry.required_levels();
                }
                for (variant, resident) in [("all_resident", &nodes), ("only_drawn", &only_drawn)] {
                    let entries =
                        run_cell_mode_from(&cut, resident, camera2, &grid, ORIGIN, t2, Some(&prev));
                    stepping += entries
                        .iter()
                        .map(|e| e.step.iter().flatten().count())
                        .sum::<usize>();
                    staged += entries
                        .iter()
                        .map(|e| e.stage.iter().filter(|s| **s).count())
                        .sum::<usize>();
                    actual.push((
                        format!("{name}/{from}>{to}/{t1}>{t2}px/{variant}"),
                        cell_mode_digest(&entries),
                    ));
                }
            }
        }
    }
    assert!(
        stepping > 0 && staged > 0,
        "the golden no longer covers stepping: {stepping} steps, {staged} stages"
    );
    assert_cell_mode_table(&actual, CELL_STEPPING_GOLDEN);
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

/// `cell_cut_with` in cell mode over `nodes`, every octant `o` wanting `wanted[o]`.
fn cell_mode_cut(nodes: &HashMap<NodeId, CellCutNode>, wanted: [u8; 8]) -> CellCutEntry {
    cell_cut_with(
        &ids(&[1]),
        &|id| nodes.get(&id).cloned(),
        &dataset_grid(ORIGIN),
        LodMode::Cell,
        &|_, octant, _| wanted[octant],
    )
    .unwrap()
    .remove(0)
}

#[test]
fn cell_mode_activates_the_closest_resident_of_each_wanted_depth() {
    // The root's node depth is 0: octants want 1, 2, 6 and 7; the resident levels are 1, 3, 6, 7.
    // Depth 2 has a coarser (1) and a finer (3) resident level at the same distance: the finer.
    let nodes = root_with(&[0, 1, 2, 3, 6, 7], &[1, 3, 6, 7]);
    let entry = cell_mode_cut(&nodes, [1, 2, 6, 7, 7, 7, 7, 7]);
    assert_eq!(
        entry.drawn,
        [1, 3, 6, 7, 7, 7, 7, 7].map(Some),
        "closest resident, finer on a tie"
    );
    assert_eq!(entry.required_levels(), vec![1, 3, 6, 7]);
    assert_eq!(entry.wanted, [1, 2, 6, 7, 7, 7, 7, 7].map(Some));
}

#[test]
fn cell_mode_never_forces_one_depth_for_a_spanning_level() {
    // Same scene as `a_spanning_level_wins_...`: in cell mode octant 0 draws the spanning
    // level 0 and the others keep their own finer level.
    let nodes = root_with(&[0, 1, 3], &[0, 3]);
    let entry = cell_mode_cut(&nodes, [1, 3, 3, 3, 3, 3, 3, 3]);
    assert_eq!(entry.drawn, [0, 3, 3, 3, 3, 3, 3, 3].map(Some));
    assert_eq!(entry.draws(), vec![(0, 0x01), (3, 0xfe)]);
    assert_eq!(
        entry.stage,
        [true, false, false, false, false, false, false, false]
    );
}

#[test]
fn cell_mode_clamps_active_depths_to_the_node_span() {
    let node_depth = representative_depth(&dataset_grid(ORIGIN), root_bounds()).saturating_sub(3);
    // A leaf wanting node_depth + 40 gets the top of the range; the deeper level never draws.
    let top = node_depth + 31;
    let nodes = root_with(
        &[node_depth, top, node_depth + 45],
        &[node_depth, top, node_depth + 45],
    );
    let entry = cell_mode_cut(&nodes, [node_depth + 40; 8]);
    assert_eq!(entry.wanted, [Some(top); 8]);
    assert_eq!(entry.drawn, [Some(top); 8]);
    assert_eq!(entry.required_levels(), vec![top]);
    // A level below the node never draws: with only that one resident nothing is drawn, and
    // the wanted depth stays in range.
    let small = SpatialBounds::new(ORIGIN, ORIGIN.map(|v| v + SIDE / 4.0)).unwrap();
    let small_depth = representative_depth(&dataset_grid(ORIGIN), small).saturating_sub(3);
    assert!(small_depth >= 2, "{small_depth}");
    let below = small_depth - 1;
    let nodes = HashMap::from([(
        NodeId::new(1),
        CellCutNode {
            bounds: small,
            buildable: (below, small_depth + 5),
            stored: vec![below, small_depth + 2],
            resident: vec![below],
        },
    )]);
    let entry = cell_mode_cut(&nodes, [small_depth; 8]);
    assert_eq!(entry.wanted, [Some(small_depth); 8]);
    assert_eq!(entry.drawn, [None; 8]);
}

#[test]
fn cell_mode_ignores_children_resident_only_outside_their_span() {
    // Node 2 sits at node depth 1 and node 5 at 4; their only resident level (0) lies below the
    // node, so in cell mode they draw nothing and hide nothing, nor are they too fine.
    let grid = dataset_grid(ORIGIN);
    let mut nodes = chain(ORIGIN, 5, 0);
    for id in [2, 5] {
        let node = nodes.get_mut(&NodeId::new(id)).unwrap();
        node.stored.insert(0, 0);
        node.resident = vec![0];
    }
    let run_cell = |cut: &[u64]| {
        cell_cut_with(
            &ids(cut),
            &|id| nodes.get(&id).cloned(),
            &grid,
            LodMode::Cell,
            &|_, _, data| data.buildable.1,
        )
    };
    let entries = run_cell(&[1, 2]).unwrap();
    assert_eq!(entries[0].mask, [0; 8]);
    assert_eq!(entries[1].drawn, [None; 8]);
    assert!(run_cell(&[1, 5]).is_ok());
}

fn root_bounds() -> SpatialBounds {
    SpatialBounds::new(ORIGIN, ORIGIN.map(|v| v + SIDE)).unwrap()
}

/// ADR 0054: the target is a minimum. A wanted depth finer than the coarsest buildable one means
/// the next coarser cell refined, so the wanted cell, placed at the octant point nearest the eye,
/// measures at least the target (the eye behind the cell's plane counts as `+inf`).
#[test]
fn the_octant_wanted_depth_keeps_cells_at_least_the_target() {
    let huge = SpatialBounds::new([-1000.0; 3], [1000.0; 3]).unwrap();
    let grid = CellGrid {
        root_min: [-1000.0; 3],
        root_side: 2000.0,
    };
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1_u64 << 53) as f64
    };
    let mut finer = 0;
    for _ in 0..600 {
        let mut camera = cameras().remove(0).1;
        let eye: [f64; 3] = std::array::from_fn(|_| (next() * 2.0 - 1.0) * 1600.0);
        let at: [f64; 3] = std::array::from_fn(|_| (next() * 2.0 - 1.0) * 1000.0);
        if !camera.set_pose(eye, at) {
            continue;
        }
        let view = view(&camera);
        let target = [1.0, 4.0, 16.0][(next() * 3.0) as usize % 3];
        for octant in 0..8 {
            let wanted = octant_wanted_depth(&view, &grid, [0.0; 3], huge, octant, (3, 12), target);
            if wanted == 3 {
                continue;
            }
            finer += 1;
            let mid = [0.0; 3];
            let side = 2000.0 / f64::from(1_u32 << wanted);
            let cube_min: [f64; 3] = std::array::from_fn(|a| {
                let (lo, hi) = if octant >> (2 - a) & 1 == 1 {
                    (mid[a], 1000.0)
                } else {
                    (-1000.0, mid[a])
                };
                eye[a].clamp(lo, hi).min(hi - side).max(lo)
            });
            let cube = SpatialBounds::new(cube_min, cube_min.map(|v| v + side)).unwrap();
            let edge = view.projected_voxel_edge_pixels(cube, [0.0; 3]);
            assert!(
                edge >= target,
                "octant {octant} depth {wanted}: {edge} px below {target} (eye {eye:?})"
            );
        }
    }
    assert!(finer > 500, "{finer} finer octants");
}

// ---- Phase 2b task 1: the depth range of a node ----

fn huge_grid() -> CellGrid {
    CellGrid {
        root_min: [-1000.0; 3],
        root_side: 2000.0,
    }
}

/// Cell-mode entry of one node with every depth of `buildable` stored and resident, its octant
/// depths and range from the view as the pool computes them (origin `[0; 3]`).
fn ranged_entry(
    view: &ViewProjection,
    grid: &CellGrid,
    bounds: SpatialBounds,
    buildable: (u8, u8),
    target: f64,
) -> CellCutEntry {
    let stored: Vec<u8> = (buildable.0..=buildable.1).collect();
    let node = CellCutNode {
        bounds,
        buildable,
        resident: stored.clone(),
        stored,
    };
    cell_cut_ranged(
        &ids(&[1]),
        &|_| Some(node.clone()),
        grid,
        LodMode::Cell,
        &|_, octant, data| {
            octant_wanted_depth(view, grid, [0.0; 3], bounds, octant, data.buildable, target)
        },
        Some(&|_, data| {
            let probes = [ProbeBox::Geometric; 8];
            (
                view_range_lo(
                    view,
                    grid,
                    [0.0; 3],
                    Some(bounds),
                    data.buildable,
                    target,
                    None,
                )
                .unwrap_or(data.buildable.0),
                octants_in_front(view, [0.0; 3], bounds, probes),
            )
        }),
        None,
    )
    .unwrap()
    .remove(0)
}

fn posed(eye: [f64; 3], at: [f64; 3]) -> ViewProjection {
    let mut camera = cameras().remove(0).1;
    assert!(camera.set_pose(eye, at));
    view(&camera)
}

#[test]
fn the_far_wanted_depth_is_never_finer_than_the_near_one() {
    let grid = huge_grid();
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1_u64 << 53) as f64
    };
    let mut checked = 0;
    for _ in 0..800 {
        let eye: [f64; 3] = std::array::from_fn(|_| (next() * 2.0 - 1.0) * 1600.0);
        let at: [f64; 3] = std::array::from_fn(|_| (next() * 2.0 - 1.0) * 1000.0);
        let mut camera = cameras().remove(0).1;
        if !camera.set_pose(eye, at) {
            continue;
        }
        let view = view(&camera);
        let target = [1.0, 4.0, 16.0][(next() * 3.0) as usize % 3];
        // A random box inside the grid: the root or one of its octants.
        let side = 2000.0 / f64::from(1_u32 << ((next() * 3.0) as u32));
        let min: [f64; 3] = std::array::from_fn(|_| {
            -1000.0 + ((next() * 2000.0 / side).floor() * side).min(2000.0 - side)
        });
        let bounds = SpatialBounds::new(min, min.map(|v| v + side)).unwrap();
        let far =
            view_range_lo(&view, &grid, [0.0; 3], Some(bounds), (3, 12), target, None).unwrap_or(3);
        let front = octants_in_front(&view, [0.0; 3], bounds, [ProbeBox::Geometric; 8]);
        let near = (0..8)
            .filter(|o| front >> o & 1 == 1)
            .map(|o| octant_wanted_depth(&view, &grid, [0.0; 3], bounds, o, (3, 12), target))
            .max();
        if let Some(near) = near {
            assert!(
                far <= near,
                "far {far} > near {near}: eye {eye:?} at {at:?} box {bounds:?} target {target}"
            );
            checked += 1;
        }
    }
    assert!(checked > 300, "{checked} poses checked");
}

/// Eye at `x = -900` looking along `+x` over `huge_grid`: view depth = `x + 900`.
fn along_x() -> ViewProjection {
    posed([-900.0, 0.0, 0.0], [0.0, 0.0, 0.0])
}

fn boxed(min: [f64; 3], max: [f64; 3]) -> Option<SpatialBounds> {
    Some(SpatialBounds::new(min, max).unwrap())
}

#[test]
fn a_region_behind_the_eye_has_no_view_range() {
    let (view, grid) = (along_x(), huge_grid());
    let behind = boxed([-1000.0, -100.0, -100.0], [-950.0, 100.0, 100.0]);
    assert_eq!(
        node_view_range(&view, &grid, [0.0; 3], behind, (3, 12), 4.0, None),
        None
    );
}

#[test]
fn an_empty_region_has_no_view_range() {
    let (view, grid) = (along_x(), huge_grid());
    let bounds = SpatialBounds::new([0.0; 3], [100.0; 3]).unwrap();
    assert_eq!(probe_region(bounds, [ProbeBox::Empty; 8]), None);
    assert_eq!(
        node_view_range(&view, &grid, [0.0; 3], None, (3, 12), 4.0, None),
        None
    );
}

#[test]
fn the_min_side_rule_caps_the_view_range() {
    let (view, grid) = (along_x(), huge_grid());
    let region = boxed([0.0, -50.0, -50.0], [100.0, 50.0, 50.0]);
    // A tiny target wants the finest depth; a minimum side of one depth-6 cell stops at 6.
    let free = node_view_range(&view, &grid, [0.0; 3], region, (3, 12), 0.001, None).unwrap();
    assert_eq!(free, (12, 12));
    let side = grid.root_side / 64.0;
    let capped =
        node_view_range(&view, &grid, [0.0; 3], region, (3, 12), 0.001, Some(side)).unwrap();
    assert_eq!(capped, (6, 6));
}

#[test]
fn the_buildable_span_clamps_both_ends_of_the_view_range() {
    let (view, grid) = (along_x(), huge_grid());
    let region = boxed([0.0, -50.0, -50.0], [100.0, 50.0, 50.0]);
    // Too coarse a target for the span: its coarsest depth; too fine: its finest.
    let coarse = node_view_range(&view, &grid, [0.0; 3], region, (8, 9), 1e6, None).unwrap();
    assert_eq!(coarse, (8, 8));
    let fine = node_view_range(&view, &grid, [0.0; 3], region, (8, 9), 0.001, None).unwrap();
    assert_eq!(fine, (9, 9));
}

#[test]
fn the_view_range_is_ordered_and_inside_the_span() {
    let grid = huge_grid();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1_u64 << 53) as f64
    };
    let mut checked = 0;
    for _ in 0..600 {
        let eye: [f64; 3] = std::array::from_fn(|_| (next() * 2.0 - 1.0) * 1600.0);
        let at: [f64; 3] = std::array::from_fn(|_| (next() * 2.0 - 1.0) * 1000.0);
        let mut camera = cameras().remove(0).1;
        if !camera.set_pose(eye, at) {
            continue;
        }
        let view = view(&camera);
        let side = 2000.0 / f64::from(1_u32 << ((next() * 3.0) as u32));
        let min: [f64; 3] = std::array::from_fn(|_| {
            -1000.0 + ((next() * 2000.0 / side).floor() * side).min(2000.0 - side)
        });
        let region = boxed(min, min.map(|v| v + side));
        let target = [1.0, 4.0, 16.0][(next() * 3.0) as usize % 3];
        if let Some((lo, hi)) =
            node_view_range(&view, &grid, [0.0; 3], region, (3, 12), target, None)
        {
            assert!(
                lo <= hi && (3..=12).contains(&lo) && hi <= 12,
                "{lo}..{hi}: eye {eye:?} at {at:?} region {region:?} target {target}"
            );
            checked += 1;
        }
    }
    assert!(checked > 300, "{checked} ranges checked");
}

#[test]
fn lo_does_not_depend_on_how_far_off_axis_the_far_corner_is() {
    // Both regions end at view depth 1000: one on the axis, one far off to the side, where a cell
    // placed at the far corner would look larger (planar perspective stretches it).
    let (view, grid) = (along_x(), huge_grid());
    let centred = boxed([0.0, -50.0, -50.0], [100.0, 50.0, 50.0]);
    let aside = boxed([0.0, 700.0, -50.0], [100.0, 800.0, 50.0]);
    for target in [1.0, 4.0, 16.0] {
        let lo = |region| {
            node_view_range(&view, &grid, [0.0; 3], region, (3, 14), target, None)
                .unwrap()
                .0
        };
        assert_eq!(lo(centred), lo(aside), "target {target}");
    }
}

#[test]
fn a_node_straddling_the_eye_plane_has_a_sane_range() {
    // The eye sits beside the node, level with its middle, looking along +x: the node's lower-x
    // half is behind the eye plane, the upper-x half in front.
    let grid = huge_grid();
    let bounds = SpatialBounds::new([-1000.0; 3], [1000.0; 3]).unwrap();
    let view = posed([100.0, 3000.0, 0.0], [1000.0, 3000.0, 0.0]);
    let entry = ranged_entry(&view, &grid, bounds, (3, 12), 4.0);
    let (lo, hi) = entry.range.expect("range");
    assert!(lo <= hi && (3..=12).contains(&lo) && hi <= 12, "{lo}..{hi}");
}

#[test]
fn the_eye_inside_the_node_gives_lo_le_hi() {
    let grid = huge_grid();
    let bounds = SpatialBounds::new([-1000.0; 3], [1000.0; 3]).unwrap();
    for at in [
        [900.0, 0.0, 0.0],
        [0.0, -900.0, 100.0],
        [-500.0, 500.0, 500.0],
    ] {
        let view = posed([10.0, 20.0, 30.0], at);
        for target in [1.0, 4.0, 16.0] {
            let entry = ranged_entry(&view, &grid, bounds, (3, 12), target);
            let (lo, hi) = entry.range.expect("range");
            assert!(lo <= hi, "{lo}..{hi} at {at:?} target {target}");
        }
    }
}

#[test]
fn an_octant_behind_the_eye_does_not_widen_the_range() {
    // Octants 0..4 are behind the eye: they "want" the finest depth (12) only because their cube
    // measures +inf, so they must not set `hi`; the visible ones want 5, the far side 4.
    let nodes = root_with(&[3, 4, 5, 6, 12], &[3, 4, 5, 6, 12]);
    let range = |front: u8| {
        cell_cut_ranged(
            &ids(&[1]),
            &|id| nodes.get(&id).cloned(),
            &dataset_grid(ORIGIN),
            LodMode::Cell,
            &|_, octant, _| if octant < 4 { 12 } else { 5 },
            Some(&|_, _| (4, front)),
            None,
        )
        .unwrap()[0]
            .range
    };
    assert_eq!(range(0b1111_0000), Some((4, 5)));
    // None in front (every octant crosses the plane): hi = lo.
    assert_eq!(range(0), Some((4, 4)));
}

#[test]
fn entries_without_a_range_input_have_no_range() {
    let nodes = root_with(&[0, 1, 2, 3], &[0, 1, 2, 3]);
    let camera = cameras().remove(0).1;
    let entries = run(
        &ids(&[1]),
        &nodes,
        &camera,
        &dataset_grid(ORIGIN),
        ORIGIN,
        4.0,
    )
    .unwrap();
    assert!(entries.iter().all(|entry| entry.range.is_none()));
}

/// One octant drawing `prev` that wants `w`, over `stored` buildable in `3..=12`.
fn step_one(prev: Option<u8>, w: u8, stored: &[u8], resident: &[u8]) -> (Option<u8>, Option<u8>) {
    let (drawn, _, step) = drawn_levels(
        &[Some(w); 8],
        stored,
        (3, 12),
        resident,
        &PrevState {
            drawn: [prev; 8],
            timed_out: [false; 8],
        },
        // A node depth whose span holds every stored depth here.
        0,
        LodMode::Cell,
    );
    (drawn[0], step[0])
}

#[test]
fn stepping_never_jumps_past_the_wanted_depth() {
    let stored = [7, 8, 9, 10];
    // Drew 10, now wants 8: only {7, 10} resident keeps 10 and waits for 9.
    assert_eq!(
        step_one(Some(10), 8, &stored, &[7, 10]),
        (Some(10), Some(9))
    );
    assert_eq!(
        step_one(Some(10), 8, &stored, &[7, 9, 10]),
        (Some(9), Some(8))
    );
    // Then 9 -> 8 once 8 is resident, and it stops there.
    assert_eq!(
        step_one(Some(9), 8, &stored, &[7, 8, 9, 10]),
        (Some(8), None)
    );
    assert_eq!(
        step_one(Some(8), 8, &stored, &[7, 8, 9, 10]),
        (Some(8), None)
    );
}

#[test]
fn stepping_refines_one_level_per_frame() {
    let stored = [3, 4, 5, 6, 7];
    let resident = [3, 4, 5, 6, 7];
    assert_eq!(step_one(Some(3), 7, &stored, &resident), (Some(4), Some(5)));
    assert_eq!(step_one(Some(4), 7, &stored, &resident), (Some(5), Some(6)));
    // The adjacent level is not resident yet: keep drawing, and ask for it.
    assert_eq!(
        step_one(Some(3), 7, &stored, &[3, 5, 7]),
        (Some(3), Some(4))
    );
    // A chain pruned to {3, 7} whose depths in between are buildable asks for the adjacent one.
    assert_eq!(step_one(Some(3), 7, &[3, 7], &[3, 7]), (Some(3), Some(4)));
}

#[test]
fn a_first_draw_uses_the_closest_resident() {
    let stored = [7, 8, 9, 10];
    assert_eq!(step_one(None, 8, &stored, &[7, 10]), (Some(7), Some(8)));
    // A previous depth that is no longer resident counts as no previous.
    assert_eq!(step_one(Some(9), 8, &stored, &[7, 10]), (Some(7), Some(8)));
}

#[test]
fn cell_mode_keeps_the_stepped_depth_active() {
    // A cell-mode node of depth 7 (span 7..=10) that drew 10 in every octant and wants 8: it keeps
    // drawing 10 while 9 is missing, and the step it waits for is reported.
    let (drawn, _, step) = drawn_levels(
        &[Some(8); 8],
        &[7, 8, 9, 10],
        (7, 10),
        &[7, 10],
        &PrevState {
            drawn: [Some(10); 8],
            timed_out: [false; 8],
        },
        7,
        LodMode::Cell,
    );
    assert_eq!(drawn, [Some(10); 8]);
    assert_eq!(step, [Some(9); 8]);
}

#[test]
fn stepping_skips_unservable_depths_and_times_out() {
    // 8 and 9 are neither resident nor buildable: the step from 10 towards 7 waits for 7 itself.
    assert_eq!(
        stepped_depth(Some(10), 7, (3, 6), &[10], false),
        Some((10, Some(7)))
    );
    // Timed out: a hop to the resident level of the interval closest to `prev`, never past the
    // wanted depth, and the octant stays pending (`step`) until it draws the wanted depth.
    assert_eq!(
        stepped_depth(Some(10), 8, (3, 12), &[6, 9, 10], true),
        Some((9, Some(8)))
    );
    assert_eq!(
        stepped_depth(Some(6), 8, (3, 12), &[6, 9, 10], true),
        Some((6, Some(7)))
    );
}

#[test]
fn a_timeout_hops_to_the_resident_closest_to_the_previous_depth() {
    // Drew 10, wants 4, residents {10, 6}: it waits for 9 first, and only after the wait hops to
    // 6 (not further than the closest resident), still pending towards 4.
    assert_eq!(
        stepped_depth(Some(10), 4, (3, 12), &[10, 6], false),
        Some((10, Some(9)))
    );
    assert_eq!(
        stepped_depth(Some(10), 4, (3, 12), &[10, 6], true),
        Some((6, Some(5)))
    );
    // 9 or 8 arriving meanwhile is taken instead of the farther 6.
    assert_eq!(
        stepped_depth(Some(10), 4, (3, 12), &[10, 9, 6], true),
        Some((9, Some(8)))
    );
    assert_eq!(
        stepped_depth(Some(10), 4, (3, 12), &[10, 8, 6], true),
        Some((8, Some(7)))
    );
    // Nothing resident in between: it keeps waiting, and stays pending.
    assert_eq!(
        stepped_depth(Some(10), 4, (3, 12), &[10], true),
        Some((10, Some(9)))
    );
}

#[test]
fn a_lost_previous_depth_draws_the_resident_closest_to_it_towards_the_wanted_one() {
    // 10 is no longer resident: 8 (closest to 10 inside 4..=10) and not 6 (closest to 4).
    assert_eq!(
        stepped_depth(Some(10), 4, (3, 12), &[6, 8], false),
        Some((8, Some(7)))
    );
    // Nothing between: the closest to the wanted depth.
    assert_eq!(
        stepped_depth(Some(10), 4, (3, 12), &[2, 12], false),
        Some((2, Some(3)))
    );
}
