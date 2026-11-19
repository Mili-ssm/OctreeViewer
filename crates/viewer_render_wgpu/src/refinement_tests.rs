//! Per-node cells refine the CPU voxel build exactly (R3 Task 2).
//!
//! For a cut and a view, the cells stored per node, restricted to the frustum, must tile the CPU
//! voxels of `build_voxel_scene_with_limits`: every cell lies in one voxel, every voxel holds at
//! least one cell, and the exact per-voxel sums equal the sums of its cells.
//!
//! Scope: replacement cuts without ancestor/descendant pairs (`[1]` and `[2..=9]`). Mixed cuts
//! need the per-node 8^3 mask of Task 3 and Additive hierarchies are not supported by the cell
//! path, so both are out.

use std::collections::{BTreeMap, BTreeSet};

use viewer_core::{HierarchySemantics, NodeId, PointBlock, SpatialBounds};

use crate::{
    CellGrid, NodeCell, NodeCellLevel, NodeCellSource, OrbitCamera, PointNodeCellSource,
    VoxelBuildLimits, build_voxel_scene_with_limits,
    scene_digest_tests::{Fixture, ORIGIN, VIEWPORT, cameras, fixture, ids},
    voxel_grid::{ExactVoxels, exact_voxels_with_limits},
};

const TARGETS: [f64; 4] = [1.0, 2.0, 4.0, 8.0];

#[derive(Clone, Copy)]
enum Pick {
    /// Coarsest stored level with depth >= `D_n` (RQ-09).
    Coarsest,
    Finest,
}

#[derive(Default)]
struct Totals {
    cases: usize,
    cpu_voxels: usize,
    cells_checked: usize,
    voxels_excluded: usize,
    cells_excluded: usize,
    source_density: usize,
}

fn overlaps(lo: [f64; 3], hi: [f64; 3], node: &SpatialBounds) -> bool {
    (0..3).all(|axis| hi[axis].min(node.max()[axis]) - lo[axis].max(node.min()[axis]) > 0.0)
}

fn local_bounds(bounds: SpatialBounds) -> SpatialBounds {
    SpatialBounds::new(
        std::array::from_fn(|axis| bounds.min()[axis] - ORIGIN[axis]),
        std::array::from_fn(|axis| bounds.max()[axis] - ORIGIN[axis]),
    )
    .unwrap()
}

fn cell_bounds(exact: &ExactVoxels, depth: u8, coord: [u32; 3]) -> SpatialBounds {
    let side = exact.side / 2.0_f64.powi(i32::from(depth));
    SpatialBounds::new(
        std::array::from_fn(|axis| exact.origin[axis] + f64::from(coord[axis]) * side),
        std::array::from_fn(|axis| exact.origin[axis] + (f64::from(coord[axis]) + 1.0) * side),
    )
    .unwrap()
}

fn run_case(
    fixture: &Fixture,
    cut: &[NodeId],
    camera: &OrbitCamera,
    target: f64,
    pick: Pick,
    label: &str,
    totals: &mut Totals,
) {
    let frustum = camera.spatial_frustum(VIEWPORT).expect("frustum");
    let project = |bounds: SpatialBounds| {
        if frustum.intersects(&bounds) {
            camera.projected_voxel_edge_pixels(bounds, [0.0; 3], VIEWPORT)
        } else {
            0.0
        }
    };
    let limits = VoxelBuildLimits::default();
    let exact = exact_voxels_with_limits(
        &fixture.hierarchy,
        cut,
        &fixture.cache,
        ORIGIN,
        target,
        &project,
        limits,
    )
    .expect("exact voxels");
    let public = build_voxel_scene_with_limits(
        &fixture.hierarchy,
        cut,
        &fixture.cache,
        ORIGIN,
        target,
        &project,
        limits,
    )
    .expect("scene");
    assert_eq!(
        exact.voxels.len(),
        public.scene.instance_count(),
        "{label}: hook and public path disagree"
    );

    let by_key: BTreeMap<(u8, [u32; 3]), usize> = exact
        .voxels
        .iter()
        .enumerate()
        .map(|(index, voxel)| ((voxel.depth, voxel.coord), index))
        .collect();
    // (4) no CPU voxel is an ancestor of another.
    for voxel in &exact.voxels {
        for depth in 0..voxel.depth {
            let shift = voxel.depth - depth;
            let ancestor = voxel.coord.map(|c| c >> shift);
            assert!(
                !by_key.contains_key(&(depth, ancestor)),
                "{label}: CPU voxel {:?} has CPU ancestor {:?}",
                (voxel.depth, voxel.coord),
                (depth, ancestor)
            );
        }
    }
    let excluded: Vec<bool> = exact
        .voxels
        .iter()
        .map(|voxel| voxel.projected_edge.is_infinite())
        .collect();
    totals.cases += 1;
    totals.cpu_voxels += exact.voxels.len();
    totals.voxels_excluded += excluded.iter().filter(|flag| **flag).count();

    let grid = CellGrid {
        root_min: exact.origin,
        root_side: exact.side,
    };
    let mut sums = vec![([0_u64; 3], 0_u64, 0_u64, 0_usize); exact.voxels.len()];
    let mut covered = vec![false; exact.voxels.len()];
    let mut seen = BTreeSet::new();
    for node_id in cut {
        let node = fixture.hierarchy.node(*node_id).expect("cut node");
        let local = local_bounds(node.bounds());
        let depth_needed = exact
            .voxels
            .iter()
            .filter(|voxel| {
                let lo = cell_bounds(&exact, voxel.depth, voxel.coord);
                overlaps(lo.min(), lo.max(), &local)
            })
            .map(|voxel| voxel.depth)
            .max();
        let is_leaf = fixture.hierarchy.is_leaf(*node_id);
        let blocks: Vec<PointBlock> = node
            .blocks()
            .iter()
            .map(|id| fixture.cache.peek(*id).expect("block").clone())
            .collect();
        let cells = PointNodeCellSource::default()
            .node_cells(&grid, *node_id, local, node.spacing(), is_leaf, &blocks)
            .expect("node cells");
        let needed = depth_needed.unwrap_or(0);
        let eligible: Vec<&NodeCellLevel> = cells
            .levels
            .iter()
            .filter(|level| level.depth >= needed)
            .collect();
        // A separated leaf stops at its source density (RQ-09): its finest level may be coarser
        // than the CPU voxels, so its cells may contain voxels instead of being contained.
        let source_density = eligible.is_empty() && is_leaf;
        let level = if source_density {
            cells.levels.iter().max_by_key(|level| level.depth)
        } else {
            match pick {
                Pick::Coarsest => eligible.iter().copied().min_by_key(|level| level.depth),
                Pick::Finest => eligible.iter().copied().max_by_key(|level| level.depth),
            }
        }
        .unwrap_or_else(|| {
            panic!(
                "{label}: node {} has no stored level >= D_n = {needed}",
                node_id.get()
            )
        });
        for cell in &level.cells {
            let coord = cell
                .coord
                .map(|c| u32::try_from(c).expect("coord fits u32"));
            let visible = project(cell_bounds(&exact, level.depth, coord)) > 0.0;
            // (4) no new cell is an ancestor-or-self of another.
            for depth in 0..=level.depth {
                let ancestor = coord.map(|c| c >> (level.depth - depth));
                assert!(
                    !seen.contains(&(depth, ancestor)),
                    "{label}: cell {:?} overlaps cell {:?}",
                    (level.depth, coord),
                    (depth, ancestor)
                );
            }
            seen.insert((level.depth, coord));
            let owners: Vec<usize> = (0..=level.depth)
                .filter_map(|depth| {
                    let shifted = coord.map(|c| c >> (level.depth - depth));
                    by_key.get(&(depth, shifted)).copied()
                })
                .collect();
            let describe = || (node_id.get(), level.depth, coord, *cell);
            match owners[..] {
                // (1) contained in exactly one CPU voxel.
                [owner] => {
                    if excluded[owner] {
                        totals.cells_excluded += 1;
                        continue;
                    }
                    totals.cells_checked += 1;
                    covered[owner] = true;
                    accumulate(&mut sums[owner], cell);
                }
                [] => {
                    let inside: Vec<usize> = exact
                        .voxels
                        .iter()
                        .enumerate()
                        .filter(|(_, voxel)| {
                            voxel.depth > level.depth
                                && voxel.coord.map(|c| c >> (voxel.depth - level.depth)) == coord
                        })
                        .map(|(index, _)| index)
                        .collect();
                    match inside[..] {
                        // Culled by the CPU: outside the frustum and in no voxel.
                        [] if !visible => {}
                        [] => panic!(
                            "{label}: visible cell {:?} lies in no CPU voxel",
                            describe()
                        ),
                        [voxel_index] if source_density => {
                            if excluded[voxel_index] {
                                totals.cells_excluded += 1;
                                continue;
                            }
                            let voxel = &exact.voxels[voxel_index];
                            assert!(
                                cell.rgb_sum == voxel.rgb
                                    && u64::from(cell.count) == voxel.count
                                    && u64::from(cell.intensity_count) == voxel.intensity_count,
                                "{label}: source-density cell {:?} differs from the CPU voxel {:?} it contains (rgb {:?} count {} intensity {})",
                                describe(),
                                (voxel.depth, voxel.coord),
                                voxel.rgb,
                                voxel.count,
                                voxel.intensity_count
                            );
                            assert!(
                                !covered[voxel_index],
                                "{label}: CPU voxel {:?} claimed twice",
                                (voxel.depth, voxel.coord)
                            );
                            covered[voxel_index] = true;
                            totals.source_density += 1;
                        }
                        _ => panic!(
                            "{label}: cell {:?} contains {} CPU voxels (source density: {source_density})",
                            describe(),
                            inside.len()
                        ),
                    }
                }
                _ => panic!(
                    "{label}: cell {:?} has {} containing CPU voxels",
                    describe(),
                    owners.len()
                ),
            }
        }
    }
    // (2) and (3): every kept voxel is covered and its contained cells sum to it exactly.
    for (index, voxel) in exact.voxels.iter().enumerate() {
        if excluded[index] {
            continue;
        }
        assert!(
            covered[index],
            "{label}: CPU voxel {:?} contains no new cell",
            (voxel.depth, voxel.coord)
        );
        let (rgb, count, intensity, cells) = sums[index];
        if cells == 0 {
            continue; // matched one-to-one as a source-density cell
        }
        assert!(
            rgb == voxel.rgb && count == voxel.count && intensity == voxel.intensity_count,
            "{label}: CPU voxel {:?} has rgb {:?} count {} intensity {}, cells sum to rgb {rgb:?} count {count} intensity {intensity} over {cells} cells",
            (voxel.depth, voxel.coord),
            voxel.rgb,
            voxel.count,
            voxel.intensity_count
        );
    }
}

fn accumulate(sum: &mut ([u64; 3], u64, u64, usize), cell: &NodeCell) {
    for axis in 0..3 {
        sum.0[axis] += cell.rgb_sum[axis];
    }
    sum.1 += u64::from(cell.count);
    sum.2 += u64::from(cell.intensity_count);
    sum.3 += 1;
}

#[test]
fn refinement_relation_holds() {
    let fixture = fixture(HierarchySemantics::Replacement);
    let cuts = [ids(&[1]), ids(&[2, 3, 4, 5, 6, 7, 8, 9])];
    for (name, pick) in [("coarsest", Pick::Coarsest), ("finest", Pick::Finest)] {
        let mut totals = Totals::default();
        for cut in &cuts {
            for (index, (camera_name, camera)) in cameras().iter().enumerate() {
                for target in TARGETS {
                    let label = format!(
                        "[{name}] cut {:?} camera {index} ({camera_name}) target {target}",
                        cut.iter().map(|id| id.get()).collect::<Vec<_>>()
                    );
                    run_case(&fixture, cut, camera, target, pick, &label, &mut totals);
                }
            }
        }
        eprintln!(
            "refinement[{name}]: cases {}, cpu voxels {}, new cells checked {}, excluded (infinite edge) voxels {} cells {}, excluded (source density) {}",
            totals.cases,
            totals.cpu_voxels,
            totals.cells_checked,
            totals.voxels_excluded,
            totals.cells_excluded,
            totals.source_density
        );
        // Guards against a vacuous pass: 2 cuts x 4 cameras x 4 targets, all with checked cells.
        assert_eq!(totals.cases, 32, "{name}: unexpected case count");
        assert!(
            totals.cpu_voxels > 0 && totals.cells_checked > 0,
            "{name}: nothing compared"
        );
    }
}
