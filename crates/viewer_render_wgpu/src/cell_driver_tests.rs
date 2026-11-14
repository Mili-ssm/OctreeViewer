//! Task 9a2 pass criteria on the strict fake GPU: the driver executes the pool's plans without
//! hazards or holes, converges again after a reset or a lost pool, packs only non-empty levels,
//! checks the frame of the cells, and forgets nodes under its CPU budget.

use std::collections::{HashMap, HashSet};

use viewer_core::{NodeId, SpatialBounds};

use super::*;
use crate::{
    NodeCell, NodeCellLevel, OrbitCamera, POOL_PAGE_BYTES, VoxelBuildLimits, pack_level,
    scene_digest_tests::{ORIGIN, SIDE, VIEWPORT, ids},
    test_support::*,
};

/// Pages of a level as the fixtures pack it: the first stored level has one page, the rest four.
fn pages_of(node: &PoolNode, depth: u8) -> usize {
    if depth == node.stored[0] { 1 } else { 4 }
}

fn level_of(node: NodeId, depth: u8, pages: usize) -> PackedLevel {
    fake_level(
        LevelKey {
            session: 0,
            node,
            depth,
            color_gen: 0,
        },
        pages,
    )
}

fn config(capacity_pages: u32, budget_pages: u64, cpu_budget_bytes: u64) -> DriverConfig {
    DriverConfig {
        pool: PoolConfig {
            capacity_pages,
            upload_budget_bytes: budget_pages * POOL_PAGE_BYTES,
            ..PoolConfig::default()
        },
        // The tests' grid and nodes are in the world frame and start at `ORIGIN`.
        grid: grid(),
        coordinate_origin: ORIGIN,
        cpu_budget_bytes,
    }
}

struct Fixture {
    driver: CellDriver,
    gpu: FakeGpu,
    nodes: HashMap<NodeId, PoolNode>,
    /// Visual scale of the views the frames are planned with.
    scale: f64,
}

impl Fixture {
    fn new(nodes: HashMap<NodeId, PoolNode>, config: DriverConfig) -> Self {
        let mut driver = CellDriver::new(config);
        let mut sorted: Vec<_> = nodes.iter().collect();
        sorted.sort_by_key(|(id, _)| **id);
        for (id, node) in sorted {
            let levels = node
                .stored
                .iter()
                .map(|depth| level_of(*id, *depth, pages_of(node, *depth)))
                .collect();
            let added = driver
                .add_packed(*id, node.bounds, node.points, levels)
                .unwrap();
            assert_eq!(
                added,
                NodeAdded::Stored {
                    levels: node.stored.len()
                }
            );
        }
        Self {
            driver,
            gpu: FakeGpu::default(),
            nodes,
            scale: 1.0,
        }
    }

    fn view(&self, camera: &OrbitCamera) -> ViewProjection {
        ViewProjection::new(*camera, VIEWPORT, self.scale, VoxelBuildLimits::default())
    }

    fn step(&mut self, cut: &[NodeId], camera: &OrbitCamera, target: f64) -> FrameReport {
        self.gpu.eye = camera.position();
        let view = self.view(camera);
        let report = self
            .driver
            .frame(&mut self.gpu, cut, &view, target)
            .expect("frame");
        assert_no_holes(&report.plan, &self.nodes);
        // The masks on the GPU are the cut's.
        for active in &report.plan.active {
            let entry = report
                .plan
                .entries
                .iter()
                .find(|entry| entry.node == active.key.node)
                .expect("active node is in the cut");
            let words = self.gpu.mask_words[&active.mask_slot];
            let expected: [u32; MASK_WORDS] =
                std::array::from_fn(|i| (entry.mask[i / 2] >> (32 * (i % 2))) as u32);
            assert_eq!(words, expected, "mask of {:?}", active.key.node);
        }
        // The table is the active pages, in the frame of the renderer.
        if let Some(frame) = self.gpu.page_frame {
            assert_eq!(frame.grid, self.driver.config.grid);
            assert_eq!(
                frame.coordinate_origin,
                self.driver.config.coordinate_origin
            );
            assert_eq!(frame.eye_scaled, camera.position());
            assert_eq!(self.gpu.table.len(), report.active_pages);
        }
        report
    }

    fn run(
        &mut self,
        frames: usize,
        cut: &[NodeId],
        camera: &OrbitCamera,
        target: f64,
    ) -> FrameReport {
        let mut report = self.step(cut, camera, target);
        for _ in 1..frames {
            report = self.step(cut, camera, target);
        }
        report
    }
}

fn fine_cut() -> Vec<NodeId> {
    let mut cut = region_children(10);
    cut.extend(region_children(20));
    ids(&cut)
}

fn counters(report: &FrameReport) -> [u32; 4] {
    let c = report.counters();
    [
        c.stages,
        c.nodes_without_level,
        c.pool_coarsened,
        c.pool_overflow,
    ]
}

#[test]
fn reset_converges_back_to_the_same_cut_and_counters_without_holes() {
    let mut fx = Fixture::new(two_regions(), config(256, 4, u64::MAX));
    let cam = camera("fit");
    fx.run(10, &ids(&[2, 3]), &cam, 64.0);
    let before = fx.run(60, &fine_cut(), &cam, 4.0);
    assert_eq!(before.plan.apply, CutApply::Applied);
    assert!(fx.driver.pool().resident_pages() > 16);
    assert!(!fx.driver.needs_frame());

    // Explicit reset (device loss): the target starts fresh too.
    let events = fx.driver.reset();
    assert!(
        events
            .iter()
            .all(|event| matches!(event, CellPoolEvent::Evict(_)))
            && events.len() > 16
    );
    assert_eq!(fx.driver.pool().resident_pages(), 0);
    assert!(fx.driver.needs_frame());
    fx.gpu.lose_pool();

    let mut empty_frames = 0;
    let mut after = None;
    for _ in 0..80 {
        let report = fx.step(&fine_cut(), &cam, 4.0);
        assert!(report.reset_events.is_empty());
        empty_frames += usize::from(report.plan.entries.is_empty());
        after = Some(report);
    }
    let after = after.unwrap();
    assert!(empty_frames > 0);
    assert_eq!(after.plan.apply, CutApply::Applied);
    assert_eq!(after.plan.entries, before.plan.entries);
    assert_eq!(counters(&after), counters(&before));
    assert_eq!(after.active_pages, before.active_pages);
    assert!(!fx.driver.needs_frame());
    // Fresh mask slots from 0, no gaps.
    assert_eq!(fx.gpu.mask_words.len() as u32, fx.gpu.next_mask);
}

#[test]
fn a_target_that_lost_its_pool_makes_the_driver_reset_itself() {
    let mut fx = Fixture::new(two_regions(), config(256, 4, u64::MAX));
    let cam = camera("fit");
    fx.run(10, &ids(&[2, 3]), &cam, 64.0);
    let before = fx.run(60, &fine_cut(), &cam, 4.0);
    let resident = fx.driver.pool().resident_pages();

    // The renderer dropped its pool (colour mode change, new dataset): nobody calls `reset`.
    fx.gpu.lose_pool();
    let first = fx.step(&fine_cut(), &cam, 4.0);
    assert!(
        first.reset_events.len() > 16
            && first
                .reset_events
                .iter()
                .all(|event| matches!(event, CellPoolEvent::Evict(_)))
    );
    assert!(first.plan.entries.is_empty(), "nothing is drawn yet");
    let after = fx.run(80, &fine_cut(), &cam, 4.0);
    assert_eq!(after.plan.entries, before.plan.entries);
    assert_eq!(counters(&after), counters(&before));
    // The coarse cut (nodes 2 and 3, one page at their coarsest level each) was pinned by the
    // first frames and is not requested again after the reset: two pages fewer than before.
    assert_eq!(fx.driver.pool().resident_pages(), resident - 2);
    // A pool that never existed is not a loss: a driver with nothing uploaded stays quiet.
    let mut fresh = Fixture::new(two_regions(), config(256, 4, u64::MAX));
    for _ in 0..3 {
        let report = fresh.step(&ids(&[2, 3]), &cam, 64.0);
        assert!(report.reset_events.is_empty());
    }
}

#[test]
fn exact_sizes_reach_the_pool_when_a_node_is_added() {
    // 2 points: the pool's estimate is one page per level; the packed levels are far bigger, so
    // only the exact sizes make the fit coarsen on the very first frame.
    let nodes: HashMap<NodeId, PoolNode> = octant_nodes()
        .into_iter()
        .map(|(id, node)| (id, PoolNode { points: 2, ..node }))
        .collect();
    let mut driver = CellDriver::new(config(44, 4, u64::MAX));
    for (id, node) in &nodes {
        let levels = node
            .stored
            .iter()
            .map(|depth| level_of(*id, *depth, if *depth == 4 { 1 } else { 20 }))
            .collect();
        driver
            .add_packed(*id, node.bounds, node.points, levels)
            .unwrap();
    }
    let mut gpu = FakeGpu::default();
    let cam = camera("zoom");
    gpu.eye = cam.position();
    let report = driver
        .frame(&mut gpu, &ids(&CHILDREN), &view(&cam), 4.0)
        .unwrap();
    assert!(
        report.counters().pool_coarsened > 0,
        "{:?}",
        report.counters()
    );
}

#[test]
fn empty_levels_are_dropped_and_a_node_without_levels_is_not_given_to_the_pool() {
    let nodes = octant_nodes();
    let node = &nodes[&NodeId::new(2)];
    let mut driver = CellDriver::new(config(128, 4, u64::MAX));
    // An empty level at the coarsest depth, then a real one.
    let added = driver
        .add_packed(
            NodeId::new(2),
            node.bounds,
            node.points,
            vec![
                level_of(NodeId::new(2), 5, 2),
                level_of(NodeId::new(2), 4, 0),
            ],
        )
        .unwrap();
    assert_eq!(added, NodeAdded::Stored { levels: 1 });
    let bytes = driver.cpu_bytes();
    assert_eq!(
        bytes,
        (2 * PAGE_SLOTS * size_of::<crate::CellInstance>() + 2 * size_of::<PageInfo>()) as u64
    );
    // Nothing but empty levels.
    let other = &nodes[&NodeId::new(3)];
    let added = driver
        .add_packed(
            NodeId::new(3),
            other.bounds,
            other.points,
            vec![level_of(NodeId::new(3), 4, 0)],
        )
        .unwrap();
    assert_eq!(added, NodeAdded::NoLevels);
    assert!(!driver.has_node(NodeId::new(3)));
    assert_eq!(driver.cpu_bytes(), bytes);

    let mut gpu = FakeGpu::default();
    let cam = camera("zoom");
    gpu.eye = cam.position();
    // The node with one real level converges without a refused offer.
    for _ in 0..20 {
        driver
            .frame(&mut gpu, &ids(&[2]), &view(&cam), 4.0)
            .expect("frame");
    }
    assert!(driver.pool().resident_pages() > 0);
    // The empty node is unknown to the pool.
    let error = driver
        .frame(&mut gpu, &ids(&[3]), &view(&cam), 4.0)
        .unwrap_err();
    assert!(matches!(
        error,
        CellDriverError::Cut(CellCutError::UnknownNode(id)) if id == NodeId::new(3)
    ));
    // Repeated or foreign levels are refused.
    let again = driver.add_packed(
        NodeId::new(2),
        node.bounds,
        1,
        vec![level_of(NodeId::new(2), 4, 1)],
    );
    assert_eq!(again, Err(NodeInputError::Duplicate(NodeId::new(2))));
    let foreign = driver.add_packed(
        NodeId::new(4),
        node.bounds,
        1,
        vec![level_of(NodeId::new(5), 4, 1)],
    );
    assert_eq!(foreign, Err(NodeInputError::InvalidLevel(NodeId::new(4))));
}

fn node_cells(node: u64, depths: &[u8]) -> NodeCells {
    let cell = |coord: [u64; 3]| NodeCell {
        coord,
        rgb_sum: [100, 120, 140],
        intensity_sum: 0.0,
        intensity_count: 0,
        count: 1,
        classification: -1,
        z_min: 0.0,
        z_max: 0.0,
    };
    NodeCells {
        node: NodeId::new(node),
        levels: depths
            .iter()
            .map(|depth| NodeCellLevel {
                depth: *depth,
                // A level with no cells (a pruned depth) next to one with a few.
                cells: if *depth == 4 {
                    Vec::new()
                } else {
                    (0..3).map(|i| cell([i, 0, 0])).collect()
                },
                octant_offsets: None,
            })
            .collect(),
    }
}

#[test]
fn cells_must_be_built_on_the_world_grid_in_the_local_frame() {
    let mut driver = CellDriver::new(config(128, 4, u64::MAX));
    let bounds = cube(ORIGIN, SIDE);
    let local = CellGrid {
        root_min: [0.0; 3],
        root_side: SIDE,
    };
    let cells = node_cells(2, &[3, 4, 5]);
    let input = |local_grid| NodeInput {
        id: NodeId::new(2),
        bounds,
        points: 100,
        local_grid,
        cells: &cells,
    };
    // The world grid itself (cells built before subtracting the origin), a shifted anchor and
    // another side are all refused, and nothing is stored.
    for found in [
        grid(),
        CellGrid {
            root_min: [0.5, 0.0, 0.0],
            root_side: SIDE,
        },
        CellGrid {
            root_side: SIDE * 1.001,
            ..local
        },
    ] {
        let error = driver.add_node(input(found)).unwrap_err();
        assert!(
            matches!(error, NodeInputError::GridMismatch { expected, .. } if expected == local),
            "{error:?}"
        );
        assert!(!driver.has_node(NodeId::new(2)));
    }
    // One ulp of the frame shift is not a mismatch; the empty level is dropped.
    let noisy = CellGrid {
        root_min: ORIGIN.map(|v| v * f64::EPSILON),
        root_side: SIDE,
    };
    assert_eq!(
        driver.add_node(input(noisy)),
        Ok(NodeAdded::Stored { levels: 2 })
    );
    assert!(driver.has_node(NodeId::new(2)));
    // Stored already: refused before anything is packed.
    assert_eq!(
        driver.add_node(input(local)),
        Err(NodeInputError::Duplicate(NodeId::new(2)))
    );
    // Cells of another node are refused.
    let mut wrong = input(local);
    wrong.id = NodeId::new(3);
    assert_eq!(
        driver.add_node(wrong),
        Err(NodeInputError::InvalidNode(NodeId::new(3)))
    );
}

#[test]
fn the_grid_check_rejects_half_a_cell_at_depth_29_and_takes_the_exact_shift() {
    let origin = [1.0e6; 3];
    let world = CellGrid {
        root_min: [1.0e6 + 0.37, 1.0e6 - 0.61, 1.0e6 + 0.25],
        root_side: SIDE,
    };
    let mut cfg = config(128, 4, u64::MAX);
    cfg.grid = world;
    cfg.coordinate_origin = origin;
    let mut driver = CellDriver::new(cfg);
    let exact = CellGrid {
        root_min: std::array::from_fn(|axis| world.root_min[axis] - origin[axis]),
        root_side: SIDE,
    };
    let cells = node_cells(2, &[3, 4, 5]);
    let bounds = cube(world.root_min, SIDE);
    let input = |local_grid| NodeInput {
        id: NodeId::new(2),
        bounds,
        points: 100,
        local_grid,
        cells: &cells,
    };
    let half_cell = SIDE / f64::from(1_u32 << 29) / 2.0;
    for axis in 0..3 {
        let mut off = exact;
        off.root_min[axis] += half_cell;
        let error = driver.add_node(input(off)).unwrap_err();
        assert!(
            matches!(error, NodeInputError::GridMismatch { .. }),
            "{error:?}"
        );
    }
    assert!(!driver.has_node(NodeId::new(2)));
    assert_eq!(
        driver.add_node(input(exact)),
        Ok(NodeAdded::Stored { levels: 2 })
    );
}

#[test]
fn the_page_table_is_built_on_the_world_grid_and_the_targets_eye() {
    // The grid anchor is a few units away from the coordinate origin, as in a real dataset.
    let anchor: [f64; 3] = std::array::from_fn(|axis| ORIGIN[axis] + [0.37, -0.61, 0.25][axis]);
    let mut shifted = config(128, 8, u64::MAX);
    shifted.grid.root_min = anchor;
    let mut fx = Fixture::new(octant_nodes(), shifted);
    let cam = camera("zoom");
    fx.run(40, &ids(&CHILDREN), &cam, 4.0);
    // Entries use the world anchor shifted by the origin (neither the anchor alone nor the local
    // grid): the fake pages sit at the cell origin 0, so each page is at the anchor minus the
    // origin minus the eye.
    let frame = fx.gpu.page_frame.unwrap();
    assert_eq!(frame.grid.root_min, anchor);
    assert_eq!(frame.coordinate_origin, ORIGIN);
    assert_eq!(frame.visual_scale, 1.0);
    assert!(!fx.gpu.table.is_empty());
    for (_, entry) in &fx.gpu.table {
        let expected: [f32; 3] =
            std::array::from_fn(|axis| (anchor[axis] - ORIGIN[axis] - cam.position()[axis]) as f32);
        assert_eq!(entry.origin(), expected);
    }
}

#[test]
fn needs_frame_follows_requests_uploads_retention_and_new_nodes() {
    let nodes = octant_nodes();
    let mut fx = Fixture::new(nodes.clone(), config(128, 2, u64::MAX));
    let cam = camera("zoom");
    let cut = ids(&CHILDREN);
    assert!(fx.driver.needs_frame(), "new nodes");
    let mut quiet = 0;
    let mut saw_busy = false;
    for _ in 0..200 {
        let report = fx.step(&cut, &cam, 4.0);
        let pending = !report.plan.fetch_requests.is_empty()
            || !report.plan.uploads.is_empty()
            || report.plan.apply == CutApply::RetainedPrevious
            || report.plan.counters.excluded_nodes > 0
            || report.plan.counters.held_nodes > 0;
        assert_eq!(fx.driver.needs_frame(), pending);
        saw_busy |= pending;
        quiet = if pending { 0 } else { quiet + 1 };
        if quiet == 10 {
            break;
        }
    }
    assert!(saw_busy && quiet == 10, "the pool never settled");
    // A new node and a reset both ask for a frame.
    let extra = NodeId::new(99);
    let bounds = nodes[&NodeId::new(2)].bounds;
    fx.driver
        .add_packed(extra, bounds, 10, vec![level_of(extra, 4, 1)])
        .unwrap();
    assert!(fx.driver.needs_frame());
    fx.step(&cut, &cam, 4.0);
    assert!(!fx.driver.needs_frame());
    let _ = fx.driver.reset();
    assert!(fx.driver.needs_frame());
}

fn distance_from(eye: [f64; 3], node: &PoolNode) -> f64 {
    (0..3)
        .map(|a| {
            let d = eye[a].clamp(node.bounds.min()[a], node.bounds.max()[a]) - eye[a];
            d * d
        })
        .sum::<f64>()
        .sqrt()
}

/// Scale of a real dataset: the eye in the dataset frame is `position / SCALE + ORIGIN`.
const SCALE: f64 = 0.0788;

/// Bytes of a node of `octant_nodes` (1 + 4 + 4 + 4 pages).
fn octant_node_bytes() -> u64 {
    (13 * (PAGE_SLOTS * size_of::<crate::CellInstance>() + size_of::<PageInfo>())) as u64
}

#[test]
fn over_the_cpu_budget_the_farthest_idle_nodes_are_forgotten_and_resident_ones_kept() {
    let nodes = octant_nodes();
    let node_bytes = octant_node_bytes();
    let mut fx = Fixture::new(nodes.clone(), config(128, 8, u64::MAX));
    fx.scale = SCALE;
    let cam = camera("zoom");
    let eye: [f64; 3] = std::array::from_fn(|a| cam.position()[a] / SCALE + ORIGIN[a]);
    let mut by_distance: Vec<NodeId> = nodes.keys().copied().collect();
    by_distance.sort_by(|a, b| {
        distance_from(eye, &nodes[b])
            .total_cmp(&distance_from(eye, &nodes[a]))
            .then(a.cmp(b))
    });
    // The farthest node and the nearest are in the cut and become resident; then only the
    // nearest stays in the cut and the budget allows five nodes.
    let (far, near) = (by_distance[0], by_distance[7]);
    for _ in 0..30 {
        fx.step(&[far, near], &cam, 4.0);
    }
    assert_eq!(fx.driver.cpu_bytes(), 8 * node_bytes);
    fx.driver.config.cpu_budget_bytes = 5 * node_bytes;
    let report = fx.step(&[near], &cam, 4.0);
    // Three of the six idle nodes without levels go, farthest first. The farthest of all holds
    // levels in the pool and stays, whatever the budget.
    assert_eq!(report.forgotten, by_distance[1..4].to_vec());
    assert_eq!(fx.driver.cpu_bytes(), 5 * node_bytes);
    assert!(fx.driver.has_node(far) && fx.driver.has_node(near));
    for id in &report.forgotten {
        assert!(!fx.driver.has_node(*id));
    }
    // A forgotten node must be sent again before it is in a cut.
    let view = fx.view(&cam);
    let error = fx
        .driver
        .frame(&mut fx.gpu, &[report.forgotten[0]], &view, 4.0)
        .unwrap_err();
    assert!(matches!(
        error,
        CellDriverError::Cut(CellCutError::UnknownNode(_))
    ));
}

#[test]
fn nodes_with_levels_in_the_pool_are_never_forgotten() {
    let mut fx = Fixture::new(octant_nodes(), config(128, 8, 0));
    let cam = camera("zoom");
    // Zero budget: only what is idle goes. Nodes 2 and 3 are in the cut and then hold levels.
    for _ in 0..30 {
        fx.step(&ids(&[2, 3]), &cam, 4.0);
    }
    assert!(fx.driver.has_node(NodeId::new(2)) && fx.driver.has_node(NodeId::new(3)));
    for id in 4..10 {
        assert!(!fx.driver.has_node(NodeId::new(id)), "node {id}");
    }
    // They leave the cut but their levels are still resident: kept, whatever the budget.
    let report = fx.step(&[], &cam, 4.0);
    assert!(report.forgotten.is_empty());
    assert!(fx.driver.has_node(NodeId::new(2)) && fx.driver.has_node(NodeId::new(3)));
}

#[test]
fn a_frame_after_a_target_error_recovers_when_the_target_drops_its_pool() {
    let cam = camera("fit");
    let run = |fail: bool| {
        let mut fx = Fixture::new(two_regions(), config(256, 4, u64::MAX));
        fx.run(10, &ids(&[2, 3]), &cam, 64.0);
        fx.gpu.fail_next_mask = fail;
        let mut errors = 0;
        let mut recovered = None;
        for _ in 0..120 {
            fx.gpu.eye = cam.position();
            let view = fx.view(&cam);
            match fx.driver.frame(&mut fx.gpu, &fine_cut(), &view, 4.0) {
                Err(CellDriverError::Target(_)) => {
                    errors += 1;
                    // The driver emptied the live renderer's pool; its reset meets a fresh target.
                    assert!(!fx.gpu.has_pool && fx.gpu.next_mask == 0);
                    assert!(fx.driver.needs_frame());
                    recovered = Some(fx.run(80, &fine_cut(), &cam, 4.0));
                    break;
                }
                Err(error) => panic!("{error}"),
                Ok(_) => {}
            }
        }
        if !fail {
            recovered = Some(fx.run(80, &fine_cut(), &cam, 4.0));
        }
        assert_eq!(errors, usize::from(fail));
        assert!(!fx.driver.needs_frame());
        assert_eq!(fx.gpu.mask_words.len() as u32, fx.gpu.next_mask);
        recovered.unwrap()
    };
    let control = run(false);
    let after = run(true);
    assert_eq!(after.plan.apply, CutApply::Applied);
    assert_eq!(after.plan.entries, control.plan.entries);
    assert_eq!(after.active_pages, control.active_pages);
    assert_eq!(counters(&after), counters(&control));
}

/// Max over 80 frames of the coarsening and overflow counters, and whether the driver still asks
/// for frames at the end.
fn pressure(capacity: u32) -> (u32, u32, bool, u32) {
    // Eight nodes of 13 pages each (104), a coarsest level of 1 page.
    let mut fx = Fixture::new(octant_nodes(), config(capacity, 8, u64::MAX));
    let cam = camera("zoom");
    let cut = ids(&CHILDREN);
    let (mut coarsened, mut overflow) = (0, 0);
    for _ in 0..80 {
        let report = fx.step(&cut, &cam, 4.0);
        coarsened = coarsened.max(report.counters().pool_coarsened);
        overflow = overflow.max(report.counters().pool_overflow);
    }
    (
        coarsened,
        overflow,
        fx.driver.needs_frame(),
        fx.driver.pool().resident_pages(),
    )
}

#[test]
fn a_cut_larger_than_the_pool_fits_coarser_levels_and_then_settles() {
    let (coarsened, overflow, needs_frame, resident) = pressure(64);
    // The fit chose coarser levels than the 104 pages the full cut would take.
    assert!((1..=64).contains(&resident), "{resident}");
    assert_eq!(overflow, 0, "coarsened {coarsened}");
    assert!(!needs_frame);
}

#[test]
fn a_cut_whose_coarsest_levels_do_not_fit_overflows() {
    // Eight nodes need eight pages even at their coarsest level; the pool has six.
    let (_, overflow, needs_frame, resident) = pressure(6);
    assert!(overflow > 0 && resident <= 6, "{overflow} {resident}");
    // The pool keeps retrying, so `needs_frame` stays true (documented on it); the viewer stops on
    // the overflow counter.
    assert!(needs_frame);
}

#[test]
fn a_refused_offer_does_not_keep_the_node_from_being_forgotten() {
    // A pool of 4 pages and a level of 8: the pool requests it and then refuses the data.
    let id = NodeId::new(2);
    let node = octant_nodes().remove(&id).unwrap();
    let mut driver = CellDriver::new(config(4, 4, 0));
    driver
        .add_packed(id, node.bounds, node.points, vec![level_of(id, 4, 8)])
        .unwrap();
    // The driver gives the pool exact sizes, which it would never request; a wrong one (1 page)
    // stands for any way the pool may refuse a level it asked for.
    driver.pool.set_level_sizes(id, &[(4, 1)]);
    // A live pool from earlier frames, which the failed frame must drop.
    let mut gpu = FakeGpu {
        has_pool: true,
        ..FakeGpu::default()
    };
    let cam = camera("zoom");
    gpu.eye = cam.position();
    let error = driver.frame(&mut gpu, &[id], &view(&cam), 4.0).unwrap_err();
    assert!(
        matches!(error, CellDriverError::OfferRefused(key) if key.node == id),
        "{error:?}"
    );
    // The pool planned a frame the target never saw: the frame fails like a target error.
    assert!(!gpu.has_pool() && driver.needs_frame());
    // Nothing of it is in the pool, so it is idle and over the (zero) budget it goes.
    assert_eq!(driver.nodes[&id].live, 0);
    let report = driver.frame(&mut gpu, &[], &view(&cam), 4.0).unwrap();
    assert_eq!(report.forgotten, vec![id]);
    assert!(gpu.has_pool() || report.plan.uploads.is_empty());
}

#[test]
fn needs_frame_stays_true_while_nodes_wait_outside_the_cut_or_held() {
    // Zoom out from the children of node 2 to node 2 itself at 4 px: node 2 waits (excluded, then
    // held until its wanted level is resident) while the children keep drawing.
    let nodes = two_regions();
    let mut fx = Fixture::new(nodes, config(512, 2, u64::MAX));
    let cam = camera("fit");
    fx.run(60, &ids(&region_children(10)), &cam, 4.0);
    assert!(!fx.driver.needs_frame());
    let (mut excluded, mut held) = (false, false);
    for _ in 0..80 {
        let report = fx.step(&ids(&[2]), &cam, 4.0);
        let counters = report.plan.counters;
        excluded |= counters.excluded_nodes > 0;
        held |= counters.held_nodes > 0;
        if counters.excluded_nodes > 0 || counters.held_nodes > 0 {
            assert!(fx.driver.needs_frame());
        }
    }
    assert!(excluded && held, "the scenario must wait both ways");
    assert!(!fx.driver.needs_frame());
}

#[test]
fn nodes_that_arrived_after_the_latched_cut_are_not_forgotten_until_the_next_cut() {
    let mut fx = Fixture::new(octant_nodes(), config(128, 8, 0));
    let cam = camera("zoom");
    fx.gpu.eye = cam.position();
    let view = fx.view(&cam);
    // The fixture's nodes arrived after the last latched cut (none): the cut here does not name
    // them. Zero budget, so every idle node outside the cut would be forgotten.
    let cut: Vec<NodeId> = Vec::new();
    fx.driver
        .frame_latched(&mut fx.gpu, &cut, &view, 4.0, &mut |_| {})
        .expect("frame");
    assert!(
        (2..10).all(|id| fx.driver.has_node(NodeId::new(id))),
        "arrivals after the latched cut are spared"
    );
    // The next cut is latched: the same nodes are fair game.
    fx.driver.cut_received();
    let report = fx
        .driver
        .frame_latched(&mut fx.gpu, &cut, &view, 4.0, &mut |_| {})
        .expect("frame");
    assert_eq!(report.forgotten.len(), 8);
    assert!(!fx.driver.has_node(NodeId::new(2)));
}

#[test]
fn support_nodes_are_not_forgotten_while_the_cut_does_not_name_them() {
    let mut fx = Fixture::new(octant_nodes(), config(128, 8, 0));
    let cam = camera("zoom");
    fx.gpu.eye = cam.position();
    let view = fx.view(&cam);
    // Zero budget and a cut that names nothing: only the support set survives.
    fx.driver.cut_received();
    fx.driver.set_support(&ids(&[2, 3]));
    let report = fx
        .driver
        .frame_latched(&mut fx.gpu, &[], &view, 4.0, &mut |_| {})
        .expect("frame");
    assert_eq!(report.forgotten.len(), 6);
    assert!(fx.driver.has_node(NodeId::new(2)) && fx.driver.has_node(NodeId::new(3)));
    // The next cut replaces the set.
    fx.driver.cut_received();
    fx.driver.set_support(&[]);
    let report = fx
        .driver
        .frame_latched(&mut fx.gpu, &[], &view, 4.0, &mut |_| {})
        .expect("frame");
    assert_eq!(report.forgotten.len(), 2);
}

#[test]
fn a_lost_pool_keeps_the_driver_busy_until_the_next_frame_resets_it() {
    let mut fx = Fixture::new(two_regions(), config(256, 4, u64::MAX));
    let cam = camera("fit");
    fx.run(60, &fine_cut(), &cam, 4.0);
    assert!(!fx.driver.needs_frame_on(&fx.gpu), "at rest with a pool");
    fx.gpu.lose_pool();
    assert!(!fx.driver.needs_frame(), "the driver itself knows nothing");
    assert!(fx.driver.needs_frame_on(&fx.gpu), "the lost pool is work");
    fx.step(&fine_cut(), &cam, 4.0);
    assert!(
        fx.driver.needs_frame_on(&fx.gpu),
        "the reset left work to do"
    );
}

#[test]
fn a_node_of_another_generation_is_foreign() {
    let mut fx = Fixture::new(octant_nodes(), config(128, 8, u64::MAX));
    // Stored without a generation: foreign to every dataset.
    assert_eq!(fx.driver.foreign_node(7), Some(NodeId::new(2)));
    fx.driver = CellDriver::new(config(128, 8, u64::MAX));
    assert_eq!(fx.driver.foreign_node(7), None);
}

#[test]
fn color_ranges_are_read_from_packed_levels_in_absolute_f64_heights() {
    // Cells of depth 3 on a grid 1e6 above the origin: heights are the cell centres, exact.
    let grid = CellGrid {
        root_min: [0.0, 0.0, 1_000_000.0],
        root_side: 8.0,
    };
    let bounds = SpatialBounds::new(grid.root_min, grid.root_min.map(|v| v + 8.0)).unwrap();
    let cell = |z: u64, intensity: Option<f64>| NodeCell {
        coord: [1, 1, z],
        rgb_sum: [1, 1, 1],
        intensity_sum: intensity.unwrap_or(0.0),
        intensity_count: u32::from(intensity.is_some()),
        count: 1,
        classification: -1,
        z_min: 0.0,
        z_max: 0.0,
    };
    let level = NodeCellLevel {
        depth: 3,
        cells: vec![cell(2, Some(40.0)), cell(5, None), cell(7, Some(900.0))],
        octant_offsets: None,
    };
    let packed = pack_level(&grid, bounds, NodeId::new(1), &level);
    let ranges = CellColorRanges::of_levels(&grid, &[packed]);
    assert_eq!(ranges.z, Some([1_000_002.5, 1_000_007.5]));
    assert_eq!(ranges.intensity, Some([40.0, 900.0]));
    // Levels without cells and without intensity add nothing.
    assert_eq!(
        CellColorRanges::of_levels(&grid, &[level_of(NodeId::new(2), 3, 2)]),
        CellColorRanges::default()
    );
    let other = CellColorRanges {
        intensity: Some([10.0, 20.0]),
        z: Some([1_000_001.0, 1_000_003.0]),
    };
    let merged = ranges.merge(other);
    assert_eq!(merged.z, Some([1_000_001.0, 1_000_007.5]));
    assert_eq!(merged.intensity, Some([10.0, 900.0]));
    assert_eq!(CellColorRanges::default().merge(other), other);
}

#[test]
fn the_frame_hands_the_target_the_ranges_of_the_drawn_nodes_only() {
    let mut fx = Fixture::new(octant_nodes(), config(128, 8, u64::MAX));
    let cam = camera("zoom");
    let report = fx.run(40, &ids(&CHILDREN), &cam, 4.0);
    let drawn: HashSet<NodeId> = report.plan.active.iter().map(|a| a.key.node).collect();
    assert!(!drawn.is_empty());
    let mut expected = CellColorRanges::default();
    for (i, (id, node)) in fx.driver.nodes.iter_mut().enumerate() {
        let k = i as f64 * 10.0;
        node.colors = CellColorRanges {
            intensity: Some([k as f32, k as f32 + 5.0]),
            z: Some([k, k + 1.0]),
        };
        if drawn.contains(id) {
            expected = expected.merge(node.colors);
        }
    }
    fx.step(&ids(&CHILDREN), &cam, 4.0);
    assert_eq!(fx.gpu.color_ranges, Some(expected));
}
