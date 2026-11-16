//! Task 9a2: the T7b1 per-pixel oracle through the `CellDriver`, with a far coordinate origin
//! (about 10^6 per axis) and a visual scale of 0.0788, as a real dataset in the viewer.
//!
//! One node is built in the local frame of its points, on the local grid; the driver pools it on
//! the world grid. The reference is the CPU path drawing the cells the driver's own plan selects
//! (octant bits and mask), computed here in f64 from the world coordinates.

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
        self.pool.set_node_record(slot, record)
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

/// The cells the pass draws, as the CPU path takes them: octant bit set and mask bit clear.
fn expected_instances(
    cells: &NodeCells,
    world: &CellGrid,
    report: &crate::FrameReport,
) -> Vec<InstanceRaw> {
    let entry = &report.plan.entries[0];
    let mut out = Vec::new();
    for active in &report.plan.active {
        let level = cells
            .levels
            .iter()
            .find(|level| level.depth == active.key.depth)
            .unwrap();
        let side = world.root_side / (1_u64 << level.depth) as f64;
        for cell in &level.cells {
            let m = cell.coord.map(|c| (c >> (level.depth - 3)) as usize);
            let octant = 4 * (m[0] >> 2) + 2 * (m[1] >> 2) + (m[2] >> 2);
            let bit = m[0] + 8 * m[1] + 64 * m[2];
            if active.octant_bits >> octant & 1 == 0 || entry.mask[bit / 64] >> (bit % 64) & 1 == 1
            {
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
        let mut last = None;
        for _ in 0..60 {
            let mut target = Target {
                pool: &mut pool,
                eye: camera.position(),
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
        let instances = expected_instances(&cells, &world, &report);
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

#[test]
fn the_driver_in_cell_mode_partitions_the_view() {
    use super::lod::{LodNodeScene, random_pose, read_drawn_slots, well_inside};
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
            LodNodeScene::random(&mut rng, unit, 1, [0, 1, 0], &[0, 2, 4, 6], 0, 1200),
            LodNodeScene::random(&mut rng, unit, 1, [1, 0, 0], &[1, 3, 5], 0, 500),
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
        driver.set_lod_mode(crate::LodMode::Cell);

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
            render_cells(h, &mut pool, camera, false);
            let drawn: Vec<(NodeId, u8, [u64; 3])> = read_drawn_slots(h, &pool, 1)
                .iter()
                .map(|slot| {
                    let page = &held[&(slot / PAGE_SLOTS as u32)];
                    (
                        page.node,
                        page.depth,
                        page.cells[*slot as usize % PAGE_SLOTS],
                    )
                })
                .collect();
            // The points: every cell of the finest resident level of each node.
            let eye = camera.position();
            let matrix = camera.eye_frame([WIDTH, HEIGHT]).unwrap().view_projection;
            for id in &cut {
                let finest = held
                    .values()
                    .filter(|page| page.node == *id)
                    .map(|page| page.depth)
                    .max()
                    .unwrap();
                for page in held.values().filter(|p| p.node == *id && p.depth == finest) {
                    let side = 1.0 / (1_u64 << finest) as f64;
                    for coord in &page.cells {
                        let point = coord.map(|c| (c as f64 + 0.5) * side);
                        if !well_inside(&matrix, point, eye) {
                            continue;
                        }
                        points += 1;
                        let hits = drawn
                            .iter()
                            .filter(|(node, depth, cell)| {
                                node == id
                                    && *depth <= finest
                                    && (0..3).all(|a| cell[a] == coord[a] >> (finest - depth))
                            })
                            .count();
                        let problem = match hits {
                            0 => {
                                holes += 1;
                                "hole"
                            }
                            1 => continue,
                            _ => {
                                doubles += 1;
                                "double"
                            }
                        };
                        if failures.len() < 8 {
                            failures.push(format!(
                                "{problem}: pose {pose} node {id:?} finest depth {finest} cell {coord:?}, \
                                 drawn depths of the node {:?}",
                                drawn
                                    .iter()
                                    .filter(|(node, ..)| node == id)
                                    .map(|(_, depth, _)| *depth)
                                    .collect::<std::collections::BTreeSet<_>>()
                            ));
                        }
                    }
                }
            }
        }
        println!(
            "# adapter {}: driver cell mode, {} poses, {points} points, {holes} holes, \
             {doubles} doubles, {multi_depth} poses with 2+ active depths of a node",
            h.label,
            poses.len()
        );
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert!(points > 500, "too few points checked: {points}");
        assert!(multi_depth > 0, "no pose drew two depths of a node");
        true
    });
}
