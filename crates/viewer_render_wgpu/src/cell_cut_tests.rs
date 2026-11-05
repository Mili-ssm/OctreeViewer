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

#[test]
fn cut_respects_target_unless_source_limited() {
    let nodes = fixture_nodes(&fixture(HierarchySemantics::Replacement));
    let grid = dataset_grid(ORIGIN);
    let (mut met, mut limited) = (0, 0);
    for (_, camera) in cameras() {
        let view = view(&camera);
        for target in TARGETS {
            for node in nodes.values() {
                let wanted = wanted_depth(&view, &grid, ORIGIN, node.bounds, &node.stored, target);
                let error = |depth: u8| {
                    view.projected_error_pixels(
                        node.bounds,
                        SIDE / 2.0_f64.powi(i32::from(depth)),
                        ORIGIN,
                    )
                };
                let finest = *node.stored.last().unwrap();
                if node.stored.iter().any(|depth| error(*depth) <= target) {
                    met += 1;
                    assert!(error(wanted) <= target);
                    // The first stored depth that meets it, not a finer one.
                    assert!(
                        node.stored
                            .iter()
                            .take_while(|depth| **depth < wanted)
                            .all(|depth| error(*depth) > target)
                    );
                } else {
                    limited += 1;
                    assert_eq!(wanted, finest);
                }
            }
        }
    }
    assert!(met > 0 && limited > 0, "met {met}, limited {limited}");
}

#[test]
fn pruned_depth_resolves_to_next_stored_finer() {
    let nodes = fixture_nodes(&fixture(HierarchySemantics::Replacement));
    let grid = dataset_grid(ORIGIN);
    let root = &nodes[&NodeId::new(1)];
    let full: Vec<u8> = (root.stored[0]..=root.stored[0] + 5).collect();
    let pruned = [full[0], full[5]];
    let mut differing = 0;
    for (_, camera) in cameras() {
        let view = view(&camera);
        for target in TARGETS {
            let w_full = wanted_depth(&view, &grid, ORIGIN, root.bounds, &full, target);
            let w_pruned = wanted_depth(&view, &grid, ORIGIN, root.bounds, &pruned, target);
            let expected = pruned.iter().copied().find(|d| *d >= w_full).unwrap();
            assert_eq!(w_pruned, expected);
            differing += usize::from(w_full != w_pruned);
        }
    }
    assert!(differing > 0, "no case exercised a pruned depth");
}

#[test]
fn camera_inside_node_returns_finest_depth() {
    let camera = cameras().remove(0).1;
    let view = view(&camera);
    let huge = SpatialBounds::new([-1000.0; 3], [1000.0; 3]).unwrap();
    let grid = CellGrid {
        root_min: [-1000.0; 3],
        root_side: 2000.0,
    };
    let stored = [3, 5, 9];
    assert!(
        !view
            .projected_error_pixels(huge, 2000.0 / 8.0, [0.0; 3])
            .is_nan()
    );
    for target in TARGETS {
        assert_eq!(
            wanted_depth(&view, &grid, [0.0; 3], huge, &stored, target),
            9
        );
    }
}

fn root_with(stored: &[u8], resident: &[u8]) -> HashMap<NodeId, CellCutNode> {
    let bounds = SpatialBounds::new(ORIGIN, ORIGIN.map(|v| v + SIDE)).unwrap();
    HashMap::from([(
        NodeId::new(1),
        CellCutNode {
            bounds,
            stored: stored.to_vec(),
            resident: resident.to_vec(),
        },
    )])
}

#[test]
fn missing_level_draws_coarsest_resident_as_stage() {
    assert_eq!(resident_depth(5, &[3, 5]), Some((5, false)));
    assert_eq!(resident_depth(5, &[4, 3]), Some((3, true)));
    assert_eq!(resident_depth(5, &[7]), Some((7, false)));
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
    assert_eq!(entries[0].drawn, [Some(3); 8]);
    assert_eq!(entries[0].stage, [true; 8]);
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
    ("root/fit/4px", 0x874f97c38baa823a),
    ("root/fit/8px", 0x874f97c38baa823a),
    ("root/orbit/2px", 0x874f97c38baa823a),
    ("root/orbit/4px", 0x874f97c38baa823a),
    ("root/orbit/8px", 0x874f97c38baa823a),
    ("root/zoom/2px", 0x874f97c38baa823a),
    ("root/zoom/4px", 0x874f97c38baa823a),
    ("root/zoom/8px", 0x874f97c38baa823a),
    ("root/pan/2px", 0x874f97c38baa823a),
    ("root/pan/4px", 0x874f97c38baa823a),
    ("root/pan/8px", 0x874f97c38baa823a),
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
    ("mixed/fit/2px", 0x41f79126f6703d85),
    ("mixed/fit/4px", 0x41f79126f6703d85),
    ("mixed/fit/8px", 0x41f79126f6703d85),
    ("mixed/orbit/2px", 0x41f79126f6703d85),
    ("mixed/orbit/4px", 0x41f79126f6703d85),
    ("mixed/orbit/8px", 0x41f79126f6703d85),
    ("mixed/zoom/2px", 0x41f79126f6703d85),
    ("mixed/zoom/4px", 0x41f79126f6703d85),
    ("mixed/zoom/8px", 0x41f79126f6703d85),
    ("mixed/pan/2px", 0x41f79126f6703d85),
    ("mixed/pan/4px", 0x41f79126f6703d85),
    ("mixed/pan/8px", 0x41f79126f6703d85),
    ("wide_chain/fit/2px", 0xd4ec69136f0fb2e1),
    ("wide_chain/fit/4px", 0x98e4833615d39f95),
    ("wide_chain/fit/8px", 0xc580e858432b8405),
    ("wide_chain/orbit/2px", 0x9b6893d765a13f65),
    ("wide_chain/orbit/4px", 0x80a0eebe3dc8d13d),
    ("wide_chain/orbit/8px", 0x0ec1fff7eb7087a1),
    ("wide_chain/zoom/2px", 0x5f91ab12d9793029),
    ("wide_chain/zoom/4px", 0xd6b265245fee5fd1),
    ("wide_chain/zoom/8px", 0x42b749cc4303d3a1),
    ("wide_chain/pan/2px", 0xd4ec69136f0fb2e1),
    ("wide_chain/pan/4px", 0x98e4833615d39f95),
    ("wide_chain/pan/8px", 0xc580e858432b8405),
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
    assert_eq!(entries[0].required_levels(), vec![3]);
    assert_eq!(entries[0].draws(), vec![(3, 0xff)]);
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
