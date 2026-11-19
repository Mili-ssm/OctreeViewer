//! Task 9a2: the T7b1 per-pixel oracle through the `CellDriver`, with a far coordinate origin
//! (about 10^6 per axis) and a visual scale of 0.0788, as a real dataset in the viewer.
//!
//! One node is built in the local frame of its points, on the local grid; the driver pools it on
//! the world grid. The reference is the CPU path drawing the cells the driver's own plan selects
//! (the cells the CPU twin of the per-cell LOD rule draws, from the records and the view the
//! driver gave the target), placed here in f64 from the world coordinates.

use super::*;
use std::collections::HashMap;

use crate::cell_driver::{CellDriver, CellTarget, DriverConfig, NodeInput};
use crate::{CellPoolError, NodeCells, PoolConfig};

const ORIGIN: [f64; 3] = [1_000_003.25, 999_998.5, 1_000_010.75];
const SCALE: f64 = 0.0788;
const NODE: u64 = 1;

/// A `CellRenderer` with the camera eye the driver draws for (a `SurfaceRenderer` has its own).
struct Target<'a> {
    pool: &'a mut CellRenderer,
    eye: [f64; 3],
    /// The last record of each node slot and the last LOD view the driver gave the target.
    records: &'a mut HashMap<u32, [u32; NODE_WORDS]>,
    view: &'a mut Option<crate::cell_lod::LodView>,
}

impl CellTarget for Target<'_> {
    type Error = CellPoolError;

    fn has_pool(&self) -> bool {
        true
    }

    fn drop_pool(&mut self) {
        unreachable!("the oracle target never fails");
    }

    fn eye(&self) -> [f64; 3] {
        self.eye
    }

    fn upload_level(&mut self, level: &PackedLevel, slots: &[u32]) -> Result<(), CellPoolError> {
        self.pool.upload_level(level, slots)
    }

    fn free_pages(&mut self, slots: &[u32]) -> Result<(), CellPoolError> {
        self.pool.free_pages(slots)
    }

    fn alloc_mask_slot(&mut self) -> Result<Option<u32>, CellPoolError> {
        Ok(self.pool.alloc_mask_slot())
    }

    fn set_node_record(
        &mut self,
        slot: u32,
        record: &[u32; NODE_WORDS],
    ) -> Result<(), CellPoolError> {
        self.records.insert(slot, *record);
        self.pool.set_node_record(slot, record)
    }

    fn set_lod_view(&mut self, view: Option<crate::cell_lod::LodView>) {
        *self.view = view;
        self.pool.set_lod_view(view);
    }

    fn set_active_pages(
        &mut self,
        pages: &[(u32, PageEntry)],
        _frame: &PageFrame,
    ) -> Result<(), CellPoolError> {
        self.pool.set_active_pages(pages)
    }
}

/// Random distinct cells of the given depths over the whole grid, unique colours.
fn node_cells(seed: u64, depths: &[(u8, usize)]) -> NodeCells {
    let mut rng = Rng(seed);
    let mut next_id = 0;
    let levels = depths
        .iter()
        .map(|&(depth, count)| {
            let span = 1_u64 << depth;
            let mut seen = HashSet::new();
            let mut coords = Vec::new();
            while coords.len() < count {
                let coord: [u64; 3] = std::array::from_fn(|_| rng.below(span));
                if seen.insert(coord) {
                    coords.push(coord);
                }
            }
            coords.sort_by_key(|coord| morton(*coord));
            NodeCellLevel {
                depth,
                cells: coords
                    .into_iter()
                    .map(|coord| {
                        next_id += 1;
                        cell(coord, next_id)
                    })
                    .collect(),
                octant_offsets: None,
            }
        })
        .collect();
    NodeCells {
        node: NodeId::new(NODE),
        levels,
        buildable: (
            depths.iter().map(|&(depth, _)| depth).min().unwrap_or(0),
            depths.iter().map(|&(depth, _)| depth).max().unwrap_or(0),
        ),
    }
}

/// The cells the pass draws, as the CPU twin of the per-cell rule takes them: those whose active
/// depth draws them from the node record the target holds (`cell_lod::draws`, f32 like the GPU).
fn expected_instances(
    cells: &NodeCells,
    world: &CellGrid,
    bounds: SpatialBounds,
    report: &crate::FrameReport,
    records: &HashMap<u32, [u32; NODE_WORDS]>,
    view: &crate::cell_lod::LodView,
) -> Vec<InstanceRaw> {
    let mut out = Vec::new();
    for active in &report.plan.active {
        let record = &records[&active.mask_slot];
        let node = crate::cell_lod::NodeLod {
            anchor: std::array::from_fn(|axis| f32::from_bits(record[16 + axis])),
            node_depth: record[19] as u8,
            active: u64::from(record[20]) | u64::from(record[21]) << 32,
            mask: std::array::from_fn(|word| {
                u64::from(record[2 * word]) | u64::from(record[2 * word + 1]) << 32
            }),
        };
        let level = cells
            .levels
            .iter()
            .find(|level| level.depth == active.key.depth)
            .unwrap();
        let side = world.root_side / (1_u64 << level.depth) as f64;
        for cell in &level.cells {
            // The cell relative to the node corner, and its side in the render frame.
            let rel = std::array::from_fn(|axis| {
                let lo = crate::node_cells::node_range(world, bounds, level.depth, axis).0;
                (cell.coord[axis] - lo) as u32
            });
            if !crate::cell_lod::draws(view, &node, level.depth, rel, (side * SCALE) as f32) {
                continue;
            }
            let rgb = cell.rgb_sum.map(|c| c as u8);
            out.push(InstanceRaw {
                center: std::array::from_fn(|axis| {
                    // World position, then the render frame: minus the origin, times the scale.
                    ((world.root_min[axis] + (cell.coord[axis] as f64 + 0.5) * side - ORIGIN[axis])
                        * SCALE) as f32
                }),
                size: (side * SCALE) as f32,
                color: ColorRgb::new(rgb[0], rgb[1], rgb[2]).to_linear_rgba(),
                intensity: -1.0,
                classification: -1.0,
            });
        }
    }
    out
}

#[test]
fn the_driver_draws_like_the_cpu_path_at_a_far_origin_on_every_adapter() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        // The scene fits the unit cube of the render frame, where `test_camera` looks.
        let side = 1.0 / SCALE;
        // The grid anchor is not the coordinate origin, as in a real dataset: the cells are built
        // on the world grid shifted by the origin.
        // Binary fractions: other offsets (0.37, -0.61, 0.25 ...) move the cube edges to subpixel
        // positions where the AMD rasteriser's silhouette samples exceed the oracle's one-sample
        // boundary slack (a depth gap of 1e-5 against 3e-6), which is not a driver defect.
        let anchor = [0.125, -0.25, 0.0625];
        let world = CellGrid {
            root_min: std::array::from_fn(|axis| ORIGIN[axis] + anchor[axis]),
            root_side: side,
        };
        let local = CellGrid {
            root_min: std::array::from_fn(|axis| world.root_min[axis] - ORIGIN[axis]),
            root_side: side,
        };
        let bounds = SpatialBounds::new(world.root_min, world.root_min.map(|v| v + side)).unwrap();
        let cells = node_cells(3, &[(3, 300), (4, 1200), (5, 3000)]);
        let mut driver = CellDriver::new(DriverConfig {
            pool: PoolConfig {
                capacity_pages: POOL_PAGES_FOR_TESTS,
                ..PoolConfig::default()
            },
            grid: world,
            coordinate_origin: ORIGIN,
            cpu_budget_bytes: u64::MAX,
        });
        driver
            .add_node(NodeInput {
                id: NodeId::new(NODE),
                bounds,
                points: 10_000,
                local_grid: local,
                cells: &cells,
            })
            .unwrap();
        let camera = test_camera();
        let view = crate::ViewProjection::new(
            camera,
            [WIDTH, HEIGHT],
            SCALE,
            crate::VoxelBuildLimits::default(),
        );
        let mut pool = new_pool(h);
        let (mut records, mut lod_view) = (HashMap::new(), None);
        let mut last = None;
        for _ in 0..60 {
            let mut target = Target {
                pool: &mut pool,
                eye: camera.position(),
                records: &mut records,
                view: &mut lod_view,
            };
            let report = driver
                .frame(&mut target, &[NodeId::new(NODE)], &view, 20.0)
                .unwrap();
            let settled = !driver.needs_frame();
            last = Some(report);
            if settled {
                break;
            }
        }
        let report = last.unwrap();
        assert!(!driver.needs_frame(), "the pool never settled");
        assert_eq!(report.plan.apply, crate::CutApply::Applied);
        let depths: Vec<u8> = report.plan.active.iter().map(|a| a.key.depth).collect();
        assert!(!depths.is_empty());
        println!("# adapter {}: active depths {depths:?}", h.label);
        let lod_view = lod_view.expect("the driver gives the target a view in cell mode");
        let instances = expected_instances(&cells, &world, bounds, &report, &records, &lod_view);
        println!(
            "# adapter {}: {} cells drawn by the twin",
            h.label,
            instances.len()
        );
        let stats = run_oracle_against(h, &mut pool, &instances, &camera, false);
        println!("# adapter {}: driver oracle {stats:?}", h.label);
        true
    });
}

// ---------------------------------------------------------------------------------------------
// Cell mode end to end: the driver feeds a real renderer, the slot pass partitions the view
// ---------------------------------------------------------------------------------------------

/// A page the target holds: its node, depth and the absolute coordinates of its cells.
struct HeldPage {
    node: NodeId,
    depth: u8,
    cells: Vec<[u64; 3]>,
}

/// A `CellRenderer` that also remembers which cells each page slot holds.
struct LodTarget<'a> {
    pool: &'a mut CellRenderer,
    eye: [f64; 3],
    held: &'a mut HashMap<u32, HeldPage>,
}

impl CellTarget for LodTarget<'_> {
    type Error = CellPoolError;

    fn has_pool(&self) -> bool {
        true
    }

    fn drop_pool(&mut self) {
        unreachable!("the target never fails");
    }

    fn eye(&self) -> [f64; 3] {
        self.eye
    }

    fn upload_level(&mut self, level: &PackedLevel, slots: &[u32]) -> Result<(), CellPoolError> {
        for (index, (info, slot)) in level.pages.iter().zip(slots).enumerate() {
            let cells = (0..info.cells as usize)
                .map(|local| {
                    let rel = level.instances[index * PAGE_SLOTS + local].rel();
                    std::array::from_fn(|axis| info.origin[axis] + u64::from(rel[axis]))
                })
                .collect();
            self.held.insert(
                *slot,
                HeldPage {
                    node: level.node,
                    depth: level.depth,
                    cells,
                },
            );
        }
        self.pool.upload_level(level, slots)
    }

    fn free_pages(&mut self, slots: &[u32]) -> Result<(), CellPoolError> {
        for slot in slots {
            self.held.remove(slot);
        }
        self.pool.free_pages(slots)
    }

    fn alloc_mask_slot(&mut self) -> Result<Option<u32>, CellPoolError> {
        Ok(self.pool.alloc_mask_slot())
    }

    fn set_node_record(
        &mut self,
        slot: u32,
        record: &[u32; NODE_WORDS],
    ) -> Result<(), CellPoolError> {
        self.pool.set_node_record(slot, record)
    }

    fn set_lod_view(&mut self, view: Option<crate::cell_lod::LodView>) {
        self.pool.set_lod_view(view);
    }

    fn set_active_pages(
        &mut self,
        pages: &[(u32, PageEntry)],
        _frame: &PageFrame,
    ) -> Result<(), CellPoolError> {
        self.pool.set_active_pages(pages)
    }
}

/// What a drawn frame shows against the points of the resident levels.
#[derive(Default)]
struct Partition {
    points: usize,
    holes: usize,
    doubles: usize,
    failures: Vec<String>,
}

/// Draws the pool and checks that every cell of each given node's finest resident level, away from the
/// screen border, lies under exactly one drawn cell of any node (same or other depth). Cells hidden
/// under a masked parent are covered by the descendant that draws there, so the test data of a
/// descendant must be dense over its footprint.
fn check_partition(
    h: &Harness,
    pool: &mut CellRenderer,
    held: &HashMap<u32, HeldPage>,
    nodes: &[NodeId],
    camera: &OrbitCamera,
    label: &str,
) -> Partition {
    use super::lod::{read_drawn_slots, well_inside};
    render_cells(h, pool, camera, false);
    let mut drawn: HashMap<(u8, [u64; 3]), u32> = HashMap::new();
    for slot in read_drawn_slots(h, pool, 1) {
        let page = &held[&(slot / PAGE_SLOTS as u32)];
        *drawn
            .entry((page.depth, page.cells[slot as usize % PAGE_SLOTS]))
            .or_default() += 1;
    }
    let depths: std::collections::BTreeSet<u8> = drawn.keys().map(|(depth, _)| *depth).collect();
    let eye = camera.position();
    let matrix = camera.eye_frame([WIDTH, HEIGHT]).unwrap().view_projection;
    let mut out = Partition::default();
    for id in nodes {
        let Some(finest) = held
            .values()
            .filter(|page| page.node == *id)
            .map(|page| page.depth)
            .max()
        else {
            continue;
        };
        let side = 1.0 / (1_u64 << finest) as f64;
        for page in held.values().filter(|p| p.node == *id && p.depth == finest) {
            for coord in &page.cells {
                let point = coord.map(|c| (c as f64 + 0.5) * side);
                if !well_inside(&matrix, point, eye) {
                    continue;
                }
                out.points += 1;
                // Half-open cells: the point is the centre of a cell, so on a finer cell's edge.
                let hits: u32 = depths
                    .iter()
                    .map(|&depth| {
                        let key = point.map(|p| (p * (1_u64 << depth) as f64).floor() as u64);
                        drawn.get(&(depth, key)).copied().unwrap_or(0)
                    })
                    .sum();
                let problem = match hits {
                    0 => {
                        out.holes += 1;
                        "hole"
                    }
                    1 => continue,
                    _ => {
                        out.doubles += 1;
                        "double"
                    }
                };
                if out.failures.len() < 8 {
                    out.failures.push(format!(
                        "{problem}: {label} node {id:?} finest depth {finest} cell {coord:?}, \
                         drawn depths {depths:?}"
                    ));
                }
            }
        }
    }
    out
}

/// Contiguous chains: a node steps one level at a time (ADR 0053) through levels it stores.
#[test]
fn the_driver_in_cell_mode_partitions_the_view() {
    partitions_the_view(&[0, 1, 2, 3, 4, 5, 6], &[1, 2, 3, 4, 5]);
}

/// Gapped chains: nothing here builds the levels in between on demand, so each hop over a gap
/// is a timeout hop (`STEP_WAIT_FRAMES`) and the view still settles and partitions.
#[test]
fn the_driver_in_cell_mode_partitions_the_view_over_gapped_chains() {
    partitions_the_view(&[0, 2, 4, 6], &[1, 3, 5]);
}

fn partitions_the_view(first: &[u8], second: &[u8]) {
    use super::lod::{LodNodeScene, random_pose};
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let unit = CellGrid {
            root_min: [0.0; 3],
            root_side: 1.0,
        };
        let mut rng = Rng(0xd2_1ee);
        let scene = [
            LodNodeScene::random(&mut rng, unit, 1, [0, 1, 0], first, 0, 1200),
            LodNodeScene::random(&mut rng, unit, 1, [1, 0, 0], second, 0, 500),
        ];
        let mut driver = CellDriver::new(DriverConfig {
            pool: PoolConfig {
                capacity_pages: POOL_PAGES_FOR_TESTS,
                ..PoolConfig::default()
            },
            grid: unit,
            coordinate_origin: [0.0; 3],
            cpu_budget_bytes: u64::MAX,
        });
        let mut cut = Vec::new();
        for (index, node) in scene.iter().enumerate() {
            let id = NodeId::new(index as u64 + 1);
            let offsets = node.offsets.iter().map(|o| node.node_depth + o);
            let cells = NodeCells {
                node: id,
                levels: node.levels.clone(),
                buildable: (offsets.clone().min().unwrap(), offsets.max().unwrap()),
            };
            driver
                .add_node(NodeInput {
                    id,
                    bounds: node.bounds,
                    points: node.levels.iter().map(|l| l.cells.len() as u64).sum(),
                    local_grid: unit,
                    cells: &cells,
                })
                .unwrap();
            cut.push(id);
        }

        // A zoom in and out on a cell of the first node, then random poses.
        let focus = {
            let node = &scene[0];
            let finest = node.levels.last().unwrap();
            let side = node.grid.root_side / (1_u64 << finest.depth) as f64;
            std::array::from_fn(|a| (finest.cells[0].coord[a] as f64 + 0.5) * side)
        };
        let mut poses: Vec<(OrbitCamera, f64)> = Vec::new();
        let zoom = [3.0, 1.5, 0.7, 0.3, 0.12, 0.05, 0.12, 0.7, 3.0];
        for distance in zoom {
            let mut camera = test_camera();
            let position = std::array::from_fn(|a| focus[a] + [0.5, 0.3, 0.4][a] * distance);
            assert!(camera.set_pose(position, focus));
            poses.push((camera, 3.0));
        }
        while poses.len() < zoom.len() + 12 {
            if let Some((camera, view)) = random_pose(&mut rng, ([0.0; 3], [1.0; 3]), (0.5, 12.0)) {
                poses.push((camera, f64::from(view.target_px)));
            }
        }

        let mut pool = new_pool(h);
        let mut held = HashMap::new();
        let (mut points, mut holes, mut doubles, mut multi_depth) = (0, 0, 0, 0);
        let mut failures = Vec::new();
        for (pose, (camera, target_px)) in poses.iter().enumerate() {
            let view = crate::ViewProjection::new(
                *camera,
                [WIDTH, HEIGHT],
                1.0,
                crate::VoxelBuildLimits::default(),
            );
            let mut report = None;
            for _ in 0..300 {
                let mut target = LodTarget {
                    pool: &mut pool,
                    eye: camera.position(),
                    held: &mut held,
                };
                report = Some(driver.frame(&mut target, &cut, &view, *target_px).unwrap());
                if !driver.needs_frame() {
                    break;
                }
            }
            assert!(!driver.needs_frame(), "pose {pose}: the pool never settled");
            let report = report.unwrap();
            multi_depth += usize::from(cut.iter().any(|id| {
                report
                    .plan
                    .active
                    .iter()
                    .filter(|a| a.key.node == *id)
                    .count()
                    >= 2
            }));
            let found = check_partition(h, &mut pool, &held, &cut, camera, &format!("pose {pose}"));
            points += found.points;
            holes += found.holes;
            doubles += found.doubles;
            failures.extend(found.failures);
        }
        println!(
            "# adapter {}: driver cell mode, {} poses, {points} points, {holes} holes, \
             {doubles} doubles, {multi_depth} poses with 2+ active depths of a node",
            h.label,
            poses.len()
        );
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert!(points > 500, "too few points checked: {points}");
        // One run per node: on a chain with gaps a node's run is a single level, so only a
        // contiguous chain can draw two depths of a node (the other detached depth is D5).
        if first.windows(2).all(|pair| pair[1] == pair[0] + 1) {
            assert!(multi_depth > 0, "no pose drew two depths of a node");
        }
        true
    });
}

/// A parent and two children enter the cut (a zoom towards one child): the parent's mask is set
/// only once its D5 depth (>= `node_depth + 3`) is resident, and its cells away from the children
/// stay at coarser depths. The partition must hold on every drawn frame, not only at rest.
#[test]
fn the_driver_in_cell_mode_partitions_a_parent_and_its_children() {
    partitions_a_parent_and_its_children(&[0, 1, 2, 3, 4, 5], &[1, 2, 3, 4]);
}

/// The same over gapped chains, whose hops over a gap are timeout hops (ADR 0053).
#[test]
fn the_driver_in_cell_mode_partitions_a_parent_and_its_children_over_gapped_chains() {
    partitions_a_parent_and_its_children(&[0, 2, 3, 5], &[1, 2, 4]);
}

fn partitions_a_parent_and_its_children(parent: &[u8], children: &[u8]) {
    use super::lod::{LodNodeScene, random_pose};
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let unit = CellGrid {
            root_min: [0.0; 3],
            root_side: 1.0,
        };
        let mut rng = Rng(0x5e_edd5);
        // Children are dense over their footprint (60000 points over at most 16^3 cells), so a
        // parent cell hidden under a child is always covered by a drawn child cell.
        let scene = [
            LodNodeScene::random(&mut rng, unit, 1, [0, 0, 0], parent, 0, 1500),
            LodNodeScene::random(&mut rng, unit, 2, [0, 0, 0], children, 0, 60_000),
            LodNodeScene::random(&mut rng, unit, 2, [1, 1, 1], children, 0, 60_000),
        ];
        let mut driver = CellDriver::new(DriverConfig {
            pool: PoolConfig {
                capacity_pages: 16,
                ..PoolConfig::default()
            },
            grid: unit,
            coordinate_origin: [0.0; 3],
            cpu_budget_bytes: u64::MAX,
        });
        let mut cut = Vec::new();
        for (index, node) in scene.iter().enumerate() {
            let id = NodeId::new(index as u64 + 1);
            let offsets = node.offsets.iter().map(|o| node.node_depth + o);
            let cells = NodeCells {
                node: id,
                levels: node.levels.clone(),
                buildable: (offsets.clone().min().unwrap(), offsets.max().unwrap()),
            };
            driver
                .add_node(NodeInput {
                    id,
                    bounds: node.bounds,
                    points: node.levels.iter().map(|l| l.cells.len() as u64).sum(),
                    local_grid: unit,
                    cells: &cells,
                })
                .unwrap();
            cut.push(id);
        }

        // A zoom in and out on a cell of the first child, then random poses around the children.
        let focus = {
            let node = &scene[1];
            let finest = node.levels.last().unwrap();
            let side = node.grid.root_side / (1_u64 << finest.depth) as f64;
            std::array::from_fn(|a| (finest.cells[0].coord[a] as f64 + 0.5) * side)
        };
        let mut poses: Vec<(OrbitCamera, f64)> = Vec::new();
        let zoom = [3.0, 1.5, 0.7, 0.3, 0.12, 0.05, 0.12, 0.7, 3.0];
        for distance in zoom {
            let mut camera = test_camera();
            let position = std::array::from_fn(|a| focus[a] + [0.5, 0.3, 0.4][a] * distance);
            assert!(camera.set_pose(position, focus));
            // Far and coarse first, then ever closer and finer: levels stream in under the cut.
            poses.push((camera, if poses.len() < 3 { 150.0 } else { 2.0 }));
        }
        while poses.len() < zoom.len() + 15 {
            if let Some((camera, view)) = random_pose(&mut rng, ([0.0; 3], [0.5; 3]), (2.0, 150.0))
            {
                poses.push((camera, f64::from(view.target_px)));
            }
        }

        let mut pool = new_pool(h);
        let mut held = HashMap::new();
        let mut total = Partition::default();
        let (mut frames, mut multi_depth, mut d5, mut d5_partial) = (0, 0, 0, 0);
        let parent_rep = scene[0].node_depth + 3;
        for (pose, (camera, target_px)) in poses.iter().enumerate() {
            let view = crate::ViewProjection::new(
                *camera,
                [WIDTH, HEIGHT],
                1.0,
                crate::VoxelBuildLimits::default(),
            );
            let mut settled = false;
            let mut multi = false;
            for frame in 0..300 {
                let mut target = LodTarget {
                    pool: &mut pool,
                    eye: camera.position(),
                    held: &mut held,
                };
                let report = driver.frame(&mut target, &cut, &view, *target_px).unwrap();
                multi |= cut.iter().any(|id| {
                    report
                        .plan
                        .active
                        .iter()
                        .filter(|a| a.key.node == *id)
                        .count()
                        >= 2
                });
                let masked = report
                    .plan
                    .entries
                    .iter()
                    .find(|entry| entry.node == cut[0])
                    .is_some_and(|entry| entry.mask.iter().any(|word| *word != 0));
                let parent_depths = || {
                    report
                        .plan
                        .active
                        .iter()
                        .filter(|a| a.key.node == cut[0])
                        .map(|a| a.key.depth)
                };
                d5 += usize::from(masked && parent_depths().any(|d| d >= parent_rep));
                d5_partial += usize::from(
                    masked
                        && parent_depths().any(|d| d >= parent_rep)
                        && parent_depths().any(|d| d < parent_rep),
                );
                // A node left out of the applied cut (R3-Q20: nothing draws its zone yet, as on
                // the first frames) is not drawing, so it owes no partition.
                let in_cut: Vec<NodeId> = report.plan.entries.iter().map(|e| e.node).collect();
                let found = check_partition(
                    h,
                    &mut pool,
                    &held,
                    &in_cut,
                    camera,
                    &format!("pose {pose} frame {frame}"),
                );
                frames += 1;
                total.points += found.points;
                total.holes += found.holes;
                total.doubles += found.doubles;
                total.failures.extend(found.failures);
                if !driver.needs_frame() {
                    settled = true;
                    break;
                }
            }
            assert!(settled, "pose {pose}: the pool never settled");
            multi_depth += usize::from(multi);
        }
        println!(
            "# adapter {}: parent and children, {} poses, {frames} frames, {} points, {} holes, \
             {} doubles, {multi_depth} poses with 2+ active depths of a node, \
             {d5} frames with the D5 depth active ({d5_partial} with a coarser depth too)",
            h.label,
            poses.len(),
            total.points,
            total.holes,
            total.doubles,
        );
        assert!(total.failures.is_empty(), "{}", total.failures.join("\n"));
        assert!(
            total.points > 1000,
            "too few points checked: {}",
            total.points
        );
        assert!(
            multi_depth * 2 >= poses.len(),
            "only {multi_depth} of {} poses draw two depths of a node",
            poses.len()
        );
        assert!(
            d5 > 0,
            "no frame had a masked parent with its D5 depth active"
        );
        assert!(
            d5_partial > 0,
            "no frame had a partly masked parent drawing a depth below its D5"
        );
        true
    });
}
