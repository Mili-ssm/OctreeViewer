//! Task 9a2 pass criteria on the strict fake GPU: the driver executes the pool's plans without
//! hazards or holes, converges again after a reset or a lost pool, packs only non-empty levels,
//! checks the frame of the cells, and forgets nodes under its CPU budget.

use std::collections::{HashMap, HashSet};

use viewer_core::{NodeId, SpatialBounds};

use super::*;
use crate::{
    CellInstance, NodeCell, NodeCellLevel, OrbitCamera, POOL_PAGE_BYTES, VoxelBuildLimits,
    pack_level,
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
            let expected: [u32; crate::MASK_WORDS] =
                std::array::from_fn(|i| (entry.mask[i / 2] >> (32 * (i % 2))) as u32);
            assert_eq!(
                words[..crate::MASK_WORDS],
                expected,
                "mask of {:?}",
                active.key.node
            );
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
        buildable: (
            depths.iter().copied().min().unwrap_or(0),
            depths.iter().copied().max().unwrap_or(0),
        ),
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
    let drawn_levels: HashSet<(NodeId, u8)> = report
        .plan
        .active
        .iter()
        .map(|a| (a.key.node, a.key.depth))
        .collect();
    for (i, (id, node)) in fx.driver.nodes.iter_mut().enumerate() {
        for (depth, colors) in &mut node.colors {
            let k = i as f64 * 10.0 + f64::from(*depth) * 1000.0;
            *colors = CellColorRanges {
                intensity: Some([k as f32, k as f32 + 5.0]),
                z: Some([k, k + 1.0]),
            };
            // Only the levels drawn merge: a deeper level that is stored but not drawn would
            // widen the range the CPU path does not see (R3-Q32).
            if drawn_levels.contains(&(*id, *depth)) {
                expected = expected.merge(*colors);
            }
        }
    }
    fx.step(&ids(&CHILDREN), &cam, 4.0);
    assert_eq!(fx.gpu.color_ranges, Some(expected));
}

/// A fixture whose nodes arrive as messages of `generation`, like on the cells path.
fn generation_fixture(generation: u64) -> Fixture {
    let nodes = two_regions();
    let mut fx = Fixture::new(HashMap::new(), config(256, 4, u64::MAX));
    let mut sorted: Vec<_> = nodes.iter().collect();
    sorted.sort_by_key(|(id, _)| **id);
    for (id, node) in sorted {
        add_generation(&mut fx.driver, *id, node, generation).unwrap();
    }
    fx.nodes = nodes;
    fx
}

fn add_generation(
    driver: &mut CellDriver,
    id: NodeId,
    node: &PoolNode,
    generation: u64,
) -> Result<NodeAdded, NodeInputError> {
    let levels = node
        .stored
        .iter()
        .map(|depth| level_of(id, *depth, pages_of(node, *depth)))
        .collect();
    driver.add_packed_generation(
        generation,
        PackedNode {
            id,
            bounds: node.bounds,
            points: node.points,
            buildable: (
                node.stored.first().copied().unwrap_or(0),
                node.stored.last().copied().unwrap_or(0),
            ),
            levels,
        },
    )
}

#[test]
fn a_recoloured_node_is_rewritten_over_its_own_pages_without_a_hole() {
    let (old, new) = (cell_generation(5, 0), cell_generation(5, 1));
    let mut fx = generation_fixture(old);
    let cam = camera("fit");
    fx.run(10, &ids(&[2, 3]), &cam, 64.0);
    fx.run(60, &fine_cut(), &cam, 4.0);
    assert!(!fx.driver.needs_frame());
    let resident = fx.driver.pool().resident_pages();
    let nodes = fx.nodes.clone();
    let mut sorted: Vec<_> = nodes.iter().collect();
    sorted.sort_by_key(|(id, _)| **id);
    // Coarse nodes first: the first frames draw a mix of old and new nodes, never a hole
    // (`step` checks it on every frame, and the hazards of the fake GPU).
    for (id, node) in sorted {
        assert_eq!(
            add_generation(&mut fx.driver, *id, node, new).unwrap(),
            NodeAdded::Recolored {
                levels: node.stored.len()
            }
        );
        assert_eq!(fx.driver.foreign_node(new), None);
        let report = fx.step(&fine_cut(), &cam, 4.0);
        assert_eq!(report.plan.apply, CutApply::Applied);
        assert_eq!(fx.driver.pool().resident_pages(), resident);
    }
    for _ in 0..10 {
        fx.step(&fine_cut(), &cam, 4.0);
    }
    assert!(fx.gpu.rewrites > 0, "the resident pages were rewritten");
    assert!(!fx.driver.needs_frame());
    // A repeat of the new message is a repeat.
    let (id, node) = nodes.iter().next().unwrap();
    assert_eq!(
        add_generation(&mut fx.driver, *id, node, new).unwrap(),
        NodeAdded::AlreadyStored
    );
}

#[test]
fn a_late_message_of_an_older_colour_generation_is_dropped() {
    let (g0, g1, g2) = (
        cell_generation(5, 0),
        cell_generation(5, 1),
        cell_generation(5, 2),
    );
    let mut fx = generation_fixture(g0);
    let cam = camera("fit");
    fx.run(10, &ids(&[2, 3]), &cam, 64.0);
    let nodes = fx.nodes.clone();
    let (id, node) = nodes.iter().find(|(id, _)| id.get() == 2).unwrap();
    let (other, other_node) = nodes.iter().find(|(id, _)| id.get() == 3).unwrap();
    add_generation(&mut fx.driver, *id, node, g2).unwrap();
    let rewrites = fx.gpu.rewrites;
    // The message of generation 1 comes after generation 2 was seen, for a stored node...
    assert_eq!(
        add_generation(&mut fx.driver, *id, node, g1).unwrap(),
        NodeAdded::Stale
    );
    // ...and for another stored node that still holds generation 0.
    assert_eq!(
        add_generation(&mut fx.driver, *other, other_node, g1).unwrap(),
        NodeAdded::Stale
    );
    fx.run(5, &ids(&[2, 3]), &cam, 64.0);
    assert!(fx.gpu.rewrites >= rewrites);
    // Another dataset is not a recolour.
    assert!(matches!(
        add_generation(&mut fx.driver, *id, node, cell_generation(6, 3)),
        Err(NodeInputError::Duplicate(_))
    ));
}

#[test]
fn a_recolour_with_another_shape_is_refused() {
    let mut fx = generation_fixture(cell_generation(5, 0));
    let node = fx.nodes[&NodeId::new(2)].clone();
    let mut levels: Vec<PackedLevel> = node
        .stored
        .iter()
        .map(|depth| level_of(NodeId::new(2), *depth, pages_of(&node, *depth) + 1))
        .collect();
    let packed = PackedNode {
        id: NodeId::new(2),
        bounds: node.bounds,
        points: node.points,
        buildable: node.buildable,
        levels: std::mem::take(&mut levels),
    };
    assert!(matches!(
        fx.driver
            .add_packed_generation(cell_generation(5, 1), packed),
        Err(NodeInputError::InvalidLevel(_))
    ));
}

#[test]
fn outdated_nodes_are_forgotten_before_current_ones() {
    let (old, new) = (cell_generation(5, 0), cell_generation(5, 1));
    let mut fx = generation_fixture(old);
    let nodes = fx.nodes.clone();
    // Every node but 2 is recoloured; none is in the cut or the pool, so only the order decides.
    for (id, node) in &nodes {
        if id.get() != 2 {
            add_generation(&mut fx.driver, *id, node, new).unwrap();
        }
    }
    let cam = camera("fit");
    fx.step(&[], &cam, 64.0);
    fx.driver.config.cpu_budget_bytes = fx.driver.cpu_bytes() - 1;
    let report = fx.step(&[], &cam, 64.0);
    assert_eq!(report.forgotten, ids(&[2]));
}

/// A node of `depths` (two pages each) in the generation, standing in for one built on demand.
fn packed_at(id: u64, depths: &[u8]) -> PackedNode {
    let node = NodeId::new(id);
    PackedNode {
        id: node,
        bounds: octant_nodes()[&node].bounds,
        points: 2000,
        buildable: (10, 14),
        levels: depths
            .iter()
            .map(|depth| level_of(node, *depth, 2))
            .collect(),
    }
}

fn stored_depths(driver: &CellDriver, id: u64) -> Vec<u8> {
    driver.nodes[&NodeId::new(id)].pool.stored.clone()
}

fn colour_depths(driver: &CellDriver, id: u64) -> Vec<u8> {
    driver.nodes[&NodeId::new(id)]
        .colors
        .iter()
        .map(|(depth, _)| *depth)
        .collect()
}

#[test]
fn added_levels_extend_the_stored_chain_in_order() {
    let generation = cell_generation(5, 0);
    let mut driver = CellDriver::new(config(44, 4, u64::MAX));
    driver
        .add_packed_generation(generation, packed_at(2, &[10, 14]))
        .unwrap();
    let before = driver.cpu_bytes();
    let added = driver
        .add_packed_levels(generation, packed_at(2, &[12]))
        .unwrap();
    assert_eq!(added, LevelsAdded::Added(1));
    assert_eq!(stored_depths(&driver, 2), [10, 12, 14]);
    assert_eq!(colour_depths(&driver, 2), [10, 12, 14]);
    let id = NodeId::new(2);
    let level = level_of(id, 12, 2);
    assert_eq!(driver.cpu_bytes(), before + level_bytes(&level));
    assert_eq!(driver.pool.known_pages(driver.pool.key(id, 12)), Some(2));
    assert_eq!(driver.nodes[&id].levels.len(), 3);
    assert_eq!(driver.nodes[&id].pool.buildable, (10, 14));
}

#[test]
fn on_demand_levels_count_toward_the_cpu_budget() {
    let generation = cell_generation(5, 0);
    let mut fx = Fixture::new(HashMap::new(), config(256, 4, u64::MAX));
    for id in [2, 3] {
        fx.driver
            .add_packed_generation(generation, packed_at(id, &[10, 14]))
            .unwrap();
    }
    let at_rest = fx.driver.cpu_bytes();
    // The budget holds both nodes as stored; the levels added to one push it over.
    fx.driver.config.cpu_budget_bytes = at_rest;
    let cam = camera("fit");
    assert!(fx.step(&[], &cam, 64.0).forgotten.is_empty());
    fx.driver
        .add_packed_levels(generation, packed_at(2, &[12]))
        .unwrap();
    assert_eq!(
        fx.driver.cpu_bytes(),
        at_rest + level_bytes(&level_of(NodeId::new(2), 12, 2))
    );
    let report = fx.step(&[], &cam, 64.0);
    assert_eq!(report.forgotten.len(), 1, "one idle node goes");
    assert!(fx.driver.cpu_bytes() <= at_rest);
    // The bytes of the forgotten node include its on-demand level: nothing leaks.
    let kept = fx.driver.nodes.values().map(|node| node.bytes).sum::<u64>();
    assert_eq!(fx.driver.cpu_bytes(), kept);
}

#[test]
fn an_already_stored_depth_is_ignored() {
    let generation = cell_generation(5, 0);
    let mut driver = CellDriver::new(config(44, 4, u64::MAX));
    driver
        .add_packed_generation(generation, packed_at(2, &[10, 14]))
        .unwrap();
    let before = driver.cpu_bytes();
    assert_eq!(
        driver.add_packed_levels(generation, packed_at(2, &[14])),
        Ok(LevelsAdded::Ignored)
    );
    assert_eq!(driver.cpu_bytes(), before);
    assert_eq!(stored_depths(&driver, 2), [10, 14]);
}

#[test]
fn levels_for_a_forgotten_node_or_old_generation_are_ignored() {
    let (g0, g1) = (cell_generation(5, 0), cell_generation(5, 1));
    let mut driver = CellDriver::new(config(44, 4, u64::MAX));
    assert_eq!(
        driver.add_packed_levels(g0, packed_at(2, &[12])),
        Ok(LevelsAdded::Ignored),
        "unknown node"
    );
    driver
        .add_packed_generation(g0, packed_at(2, &[10, 14]))
        .unwrap();
    driver
        .add_packed_generation(g1, packed_at(3, &[10, 14]))
        .unwrap();
    // Node 2 still holds colour 0 and colour 1 was seen: its levels are old.
    assert_eq!(
        driver.add_packed_levels(g0, packed_at(2, &[12])),
        Ok(LevelsAdded::Ignored),
        "old generation"
    );
    assert_eq!(
        driver.add_packed_levels(g1, packed_at(2, &[12])),
        Ok(LevelsAdded::Ignored),
        "colour the node does not hold"
    );
    assert_eq!(stored_depths(&driver, 2), [10, 14]);
    assert_eq!(
        driver.add_packed_levels(g1, packed_at(3, &[12])),
        Ok(LevelsAdded::Added(1))
    );
}

#[test]
fn recolor_drops_on_demand_levels() {
    let (g0, g1) = (cell_generation(5, 0), cell_generation(5, 1));
    let mut driver = CellDriver::new(config(44, 4, u64::MAX));
    driver
        .add_packed_generation(g0, packed_at(2, &[10, 14]))
        .unwrap();
    let base = driver.cpu_bytes();
    driver.add_packed_levels(g0, packed_at(2, &[12])).unwrap();
    assert_eq!(stored_depths(&driver, 2), [10, 12, 14]);
    assert_eq!(
        driver.add_packed_generation(g1, packed_at(2, &[10, 14])),
        Ok(NodeAdded::Recolored { levels: 2 })
    );
    assert_eq!(stored_depths(&driver, 2), [10, 14]);
    assert_eq!(colour_depths(&driver, 2), [10, 14]);
    assert_eq!(driver.nodes[&NodeId::new(2)].levels.len(), 2);
    assert_eq!(driver.cpu_bytes(), base);
    // A recolour of another shape is still refused and drops nothing.
    driver.add_packed_levels(g1, packed_at(2, &[12])).unwrap();
    let with_12 = driver.cpu_bytes();
    let mut odd = packed_at(2, &[10, 14]);
    odd.levels[0] = level_of(NodeId::new(2), 10, 3);
    assert_eq!(
        driver.add_packed_generation(cell_generation(5, 2), odd),
        Err(NodeInputError::InvalidLevel(NodeId::new(2)))
    );
    assert_eq!(stored_depths(&driver, 2), [10, 12, 14]);
    assert_eq!(driver.cpu_bytes(), with_12);
}

/// Packed levels of node 2 for a recolour: depths 5 to 7 as the fixture packs them, tagged.
fn recoloured_node_2(fx: &Fixture, tag: u32) -> PackedNode {
    let node = &fx.nodes[&NodeId::new(2)];
    let levels = node
        .stored
        .iter()
        .filter(|depth| **depth != 4)
        .map(|depth| {
            let mut level = level_of(NodeId::new(2), *depth, pages_of(node, *depth));
            level.instances[0].0[0] = tag;
            level
        })
        .collect();
    PackedNode {
        id: NodeId::new(2),
        bounds: node.bounds,
        points: node.points,
        buildable: (4, 7),
        levels,
    }
}

#[test]
fn a_dropped_on_demand_level_leaves_the_pool_and_a_drawn_one_is_refreshed_when_it_returns() {
    let (g0, g1) = (cell_generation(5, 0), cell_generation(5, 1));
    let cam = camera("fit");
    let id = NodeId::new(2);
    let first_levels = |fx: &Fixture, depth: u8| {
        let key = fx.driver.pool.key(id, depth);
        fx.driver
            .pool
            .level(key)
            .map(|level| level.instances[0].0[0])
    };

    // Not drawn when the node is recoloured: depth 4 (the "on-demand" one) is evicted.
    let mut fx = generation_fixture(g0);
    fx.run(10, &ids(&[2, 3]), &cam, 64.0);
    let key = fx.driver.pool.key(id, 4);
    assert!(fx.driver.pool.is_resident(key));
    fx.run(4, &ids(&[3]), &cam, 64.0);
    let tagged = recoloured_node_2(&fx, 1);
    assert_eq!(
        fx.driver.add_packed_generation(g1, tagged),
        Ok(NodeAdded::Recolored { levels: 3 })
    );
    assert!(!fx.driver.pool.is_resident(key), "the dropped depth left");
    fx.run(2, &ids(&[3]), &cam, 64.0);
    assert!(fx.driver.pool.level(key).is_none());

    // Drawn: it stays with the old colours until the depth returns with the new ones.
    let mut fx = generation_fixture(g0);
    fx.run(10, &ids(&[2, 3]), &cam, 64.0);
    assert!(fx.driver.pool.is_resident(key));
    assert_eq!(first_levels(&fx, 4), Some(CellInstance::EMPTY.0[0]));
    let tagged = recoloured_node_2(&fx, 1);
    fx.driver.add_packed_generation(g1, tagged).unwrap();
    assert!(
        fx.driver.pool.is_resident(key),
        "a drawn level is not evicted"
    );
    let mut back = PackedNode {
        levels: vec![level_of(id, 4, 1)],
        ..recoloured_node_2(&fx, 1)
    };
    back.levels[0].instances[0].0[0] = 9;
    assert_eq!(
        fx.driver.add_packed_levels(g1, back),
        Ok(LevelsAdded::Added(1))
    );
    fx.nodes = fx
        .nodes
        .iter()
        .map(|(id, node)| (*id, node.clone()))
        .collect();
    fx.run(3, &ids(&[2, 3]), &cam, 64.0);
    assert_eq!(
        first_levels(&fx, 4),
        Some(9),
        "the new colours replaced the old"
    );
}

/// Node 2 with only its coarsest level stored and depths up to 7 buildable, in `generation`.
fn on_demand_fixture(generation: u64) -> Fixture {
    let mut fx = Fixture::new(HashMap::new(), config(64, 4, u64::MAX));
    fx.nodes = octant_nodes();
    fx.driver
        .add_packed_generation(generation, on_demand_node(&[4], 0))
        .unwrap();
    fx
}

/// Node 2 with one page per level at `depths`, buildable 4..=7; `tag` marks the first cell.
fn on_demand_node(depths: &[u8], tag: u32) -> PackedNode {
    let id = NodeId::new(2);
    PackedNode {
        id,
        bounds: octant_nodes()[&id].bounds,
        points: 2000,
        buildable: (4, 7),
        levels: depths
            .iter()
            .map(|depth| {
                let mut level = level_of(id, *depth, 1);
                level.instances[0].0[0] = tag;
                level
            })
            .collect(),
    }
}

#[test]
fn a_missing_level_is_requested_without_failing_the_frame() {
    let g0 = cell_generation(5, 0);
    let mut fx = on_demand_fixture(g0);
    let cam = camera("zoom");
    let cut = ids(&[2]);
    let report = fx.run(4, &cut, &cam, 1.0);
    let wanted = report.plan.entries[0].wanted[0].expect("wanted");
    assert!(wanted > 4, "setup: the view wants a buildable level");
    assert!(report.plan.fetch_requests.iter().all(|key| key.depth == 4));
    let id = NodeId::new(2);
    assert_eq!(fx.driver.take_level_requests(), Some(vec![(id, wanted)]));
    fx.step(&cut, &cam, 1.0);
    assert_eq!(fx.driver.take_level_requests(), None);
}

#[test]
fn an_arrived_level_is_offered_on_the_next_frame() {
    let g0 = cell_generation(5, 0);
    let mut fx = on_demand_fixture(g0);
    let cam = camera("zoom");
    let cut = ids(&[2]);
    let id = NodeId::new(2);
    let before = fx.run(4, &cut, &cam, 1.0);
    let wanted = before.plan.entries[0].wanted[0].expect("wanted");
    assert_eq!(fx.driver.take_level_requests(), Some(vec![(id, wanted)]));
    assert_eq!(before.plan.entries[0].drawn[0], Some(4));
    assert!(before.plan.entries[0].stage[0]);
    assert_eq!(
        fx.driver
            .add_packed_levels(g0, on_demand_node(&[wanted], 0)),
        Ok(LevelsAdded::Added(1))
    );
    let mut offered = false;
    let mut last = before;
    for _ in 0..4 {
        last = fx.step(&cut, &cam, 1.0);
        offered |= last
            .plan
            .fetch_requests
            .iter()
            .any(|key| (key.node, key.depth) == (id, wanted));
    }
    assert!(offered, "the arrived level is fetched and offered");
    assert_eq!(last.plan.entries[0].drawn[0], Some(wanted));
    assert!(!last.plan.entries[0].stage[0]);
    assert!(fx.driver.pool().is_resident(fx.driver.pool.key(id, wanted)));
    assert_eq!(fx.driver.take_level_requests(), Some(vec![]));
}

#[test]
fn a_level_added_and_dropped_is_requested_again_and_returns_with_the_new_colours() {
    let (g0, g1) = (cell_generation(5, 0), cell_generation(5, 1));
    let mut fx = on_demand_fixture(g0);
    let cam = camera("zoom");
    let cut = ids(&[2]);
    let id = NodeId::new(2);
    let wanted = fx.run(4, &cut, &cam, 1.0).plan.entries[0].wanted[0].expect("wanted");
    fx.driver
        .add_packed_levels(g0, on_demand_node(&[wanted], 0))
        .unwrap();
    fx.run(6, &cut, &cam, 1.0);
    let key = fx.driver.pool.key(id, wanted);
    assert!(fx.driver.pool.is_resident(key));
    let _ = fx.driver.take_level_requests();
    // The recolour drops the added depth: the node asks for it again while the old one draws.
    fx.driver
        .add_packed_generation(g1, on_demand_node(&[4], 1))
        .unwrap();
    assert_eq!(stored_depths(&fx.driver, 2), [4]);
    fx.run(2, &cut, &cam, 1.0);
    assert_eq!(fx.driver.take_level_requests(), Some(vec![(id, wanted)]));
    fx.driver
        .add_packed_levels(g1, on_demand_node(&[wanted], 9))
        .unwrap();
    fx.run(6, &cut, &cam, 1.0);
    let colour = fx
        .driver
        .pool
        .level(key)
        .map(|level| level.instances[0].0[0]);
    assert_eq!(colour, Some(9), "the new colours replaced the old");
    assert_eq!(fx.driver.take_level_requests(), Some(vec![]));
}

#[test]
fn a_refused_depth_lowers_what_the_node_can_want_and_the_view_settles() {
    let mut fx = on_demand_fixture(cell_generation(5, 0));
    let cam = camera("zoom");
    let cut = ids(&[2]);
    let id = NodeId::new(2);
    let first = fx.run(4, &cut, &cam, 1.0);
    let wanted = first.plan.entries[0].wanted[0].expect("wanted");
    assert!(wanted > 4, "setup: the view wants a buildable level");
    // The build side refuses every depth it is asked for, as for a leaf merged at the deepest
    // cell depth: each refusal lowers what the node can want.
    for _ in 0..4 {
        if let Some(requests) = fx.driver.take_level_requests() {
            fx.driver.refuse_levels(&requests);
        }
        fx.run(2, &cut, &cam, 1.0);
    }
    assert_eq!(fx.driver.nodes[&id].pool.buildable.1, 4);
    let last = fx.run(4, &cut, &cam, 1.0);
    assert_eq!(fx.driver.take_level_requests().unwrap_or_default(), vec![]);
    assert_eq!(last.plan.entries[0].drawn[0], Some(4));
    assert!(
        !last.plan.entries[0].stage[0],
        "the node draws what it wants"
    );
    assert!(last.plan.entries.iter().all(|entry| entry.node == id));
    assert!(!fx.driver.needs_frame(), "nothing is held or pending");
}

#[test]
fn reordered_level_requests_are_not_a_change() {
    let mut fx = on_demand_fixture(cell_generation(5, 0));
    let id = NodeId::new(2);
    fx.driver.level_requests = vec![(id, 6), (id, 7)];
    assert!(fx.driver.take_level_requests().is_some());
    fx.driver.level_requests = vec![(id, 7), (id, 6)];
    assert_eq!(fx.driver.take_level_requests(), None);
    fx.driver.level_requests = vec![(id, 7)];
    assert_eq!(fx.driver.take_level_requests(), Some(vec![(id, 7)]));
}

#[test]
fn anchors_follow_the_eye() {
    let mut fx = Fixture::new(octant_nodes(), config(128, 8, u64::MAX));
    let cam = camera("zoom");
    let report = fx.run(40, &ids(&CHILDREN), &cam, 4.0);
    let slots: HashMap<u32, NodeId> = report
        .plan
        .active
        .iter()
        .map(|active| (active.mask_slot, active.key.node))
        .collect();
    assert!(!slots.is_empty());
    let before = fx.gpu.mask_words.clone();
    fx.gpu.record_writes.clear();

    // The eye moves; the cut, the masks and the view stay as they are.
    let moved = std::array::from_fn(|axis| cam.position()[axis] + [0.5, -0.25, 0.125][axis]);
    fx.gpu.eye = moved;
    let view = fx.view(&cam);
    let report = fx
        .driver
        .frame(&mut fx.gpu, &ids(&CHILDREN), &view, 4.0)
        .expect("frame");
    assert!(report.plan.mask_updates.is_empty());

    assert_eq!(fx.gpu.mask_words.len(), before.len());
    let grid = fx.driver.config.grid;
    for (slot, node) in &slots {
        let (old, new) = (before[slot], fx.gpu.mask_words[slot]);
        for word in (0..NODE_WORDS).filter(|word| !(16..19).contains(word)) {
            assert_eq!(new[word], old[word], "word {word} of slot {slot}");
        }
        let depth = old[19] as u8;
        let bounds = fx.nodes[node].bounds;
        let side = grid.root_side / (1_u64 << depth) as f64;
        let expected: [f32; 3] = std::array::from_fn(|axis| {
            let lo = crate::node_cells::node_range(&grid, bounds, depth, axis).0;
            let corner = grid.root_min[axis] + lo as f64 * side;
            ((corner - ORIGIN[axis]) * fx.scale - moved[axis]) as f32
        });
        let anchor: [f32; 3] = std::array::from_fn(|axis| f32::from_bits(new[16 + axis]));
        assert_eq!(anchor, expected, "anchor of slot {slot}");
        assert_ne!(new[16..19], old[16..19]);
    }
    // Only live slots are written, one record each.
    let written: HashSet<u32> = fx.gpu.record_writes.iter().map(|(slot, _)| *slot).collect();
    assert_eq!(written, slots.keys().copied().collect::<HashSet<_>>());
    assert_eq!(fx.gpu.record_writes.len(), slots.len());
}

/// The anchor the driver must write for a node: its corner in the frame of the renderer.
fn expected_anchor(
    fx: &Fixture,
    node: NodeId,
    node_depth: u8,
    origin: [f64; 3],
    eye: [f64; 3],
) -> [f32; 3] {
    let grid = fx.driver.config.grid;
    let side = grid.root_side / (1_u64 << node_depth) as f64;
    std::array::from_fn(|axis| {
        let lo = crate::node_cells::node_range(&grid, fx.nodes[&node].bounds, node_depth, axis).0;
        let corner = grid.root_min[axis] + lo as f64 * side;
        ((corner - origin[axis]) * fx.scale - eye[axis]) as f32
    })
}

/// Checks the anchor of every active slot against the frame `(origin, eye)`.
fn assert_anchors(fx: &Fixture, report: &FrameReport, origin: [f64; 3], eye: [f64; 3]) {
    for active in &report.plan.active {
        let words = fx.gpu.mask_words[&active.mask_slot];
        let expected = expected_anchor(fx, active.key.node, words[19] as u8, origin, eye);
        let anchor: [f32; 3] = std::array::from_fn(|axis| f32::from_bits(words[16 + axis]));
        assert_eq!(anchor, expected, "slot {}", active.mask_slot);
    }
}

#[test]
fn anchors_follow_the_origin_and_the_scale() {
    let mut fx = Fixture::new(octant_nodes(), config(128, 8, u64::MAX));
    let cam = camera("zoom");
    let report = fx.run(40, &ids(&CHILDREN), &cam, 4.0);
    assert!(!report.plan.active.is_empty());
    let eye = cam.position();
    assert_anchors(&fx, &report, ORIGIN, eye);
    let before = fx.gpu.mask_words.clone();

    // A new coordinate origin moves every corner; the eye, the cut and the masks stay.
    let origin = std::array::from_fn(|axis| ORIGIN[axis] + [0.5, -0.25, 0.125][axis]);
    fx.driver.config.coordinate_origin = origin;
    let view = fx.view(&cam);
    let report = fx
        .driver
        .frame(&mut fx.gpu, &ids(&CHILDREN), &view, 4.0)
        .expect("frame");
    assert!(report.plan.mask_updates.is_empty());
    assert_anchors(&fx, &report, origin, eye);
    for active in &report.plan.active {
        let slot = active.mask_slot;
        assert_ne!(
            fx.gpu.mask_words[&slot][16..19],
            before[&slot][16..19],
            "slot {slot}"
        );
    }

    // A new visual scale does too.
    let origin_before = fx.gpu.mask_words.clone();
    fx.scale = 2.0;
    let view = fx.view(&cam);
    let report = fx
        .driver
        .frame(&mut fx.gpu, &ids(&CHILDREN), &view, 4.0)
        .expect("frame");
    assert_anchors(&fx, &report, origin, eye);
    for active in &report.plan.active {
        let slot = active.mask_slot;
        assert_ne!(
            fx.gpu.mask_words[&slot][16..19],
            origin_before[&slot][16..19],
            "slot {slot}"
        );
    }
}

#[test]
fn records_are_rewritten_after_a_reset() {
    let mut fx = Fixture::new(octant_nodes(), config(128, 8, u64::MAX));
    let cam = camera("zoom");
    fx.run(40, &ids(&CHILDREN), &cam, 4.0);

    let events = fx.driver.reset();
    assert!(!events.is_empty());
    fx.gpu.lose_pool();
    // The eye moved meanwhile: a frame stamp surviving the reset would skip the anchors.
    let moved: [f64; 3] =
        std::array::from_fn(|axis| cam.position()[axis] + [0.5, 0.25, -0.125][axis]);
    let mut drawn = false;
    for _ in 0..60 {
        fx.gpu.eye = moved;
        let view = fx.view(&cam);
        let report = fx
            .driver
            .frame(&mut fx.gpu, &ids(&CHILDREN), &view, 4.0)
            .expect("frame");
        // Every page of the table reads a record that was written this life of the target.
        for (_, entry) in &fx.gpu.table {
            let slot = entry.0[4];
            assert!(fx.gpu.mask_words.contains_key(&slot), "slot {slot}");
        }
        assert_anchors(&fx, &report, ORIGIN, moved);
        drawn |= !report.plan.active.is_empty();
    }
    assert!(drawn);

    // Same when only the target lost its pool.
    fx.gpu.lose_pool();
    let mut last = None;
    for _ in 0..60 {
        fx.gpu.eye = cam.position();
        let view = fx.view(&cam);
        let report = fx
            .driver
            .frame(&mut fx.gpu, &ids(&CHILDREN), &view, 4.0)
            .expect("frame");
        assert_anchors(&fx, &report, ORIGIN, cam.position());
        last = Some(report);
    }
    assert!(!last.unwrap().plan.active.is_empty());
}

#[test]
fn a_reused_mask_slot_gets_the_full_record_of_its_new_node() {
    let mut fx = Fixture::new(octant_nodes(), config(128, 8, u64::MAX));
    let cam = camera("zoom");
    let first = fx.run(40, &ids(&[2, 3]), &cam, 4.0);
    let first_slots: HashSet<u32> = first.plan.active.iter().map(|a| a.mask_slot).collect();
    assert!(!first_slots.is_empty());

    // The cut changes to other nodes: the slots of 2 and 3 are freed and taken again.
    let mut latest: HashMap<u32, crate::MaskUpdate> = HashMap::new();
    let mut report = None;
    for _ in 0..60 {
        let step = fx.step(&ids(&[4]), &cam, 4.0);
        for update in &step.plan.mask_updates {
            latest.insert(update.slot, update.clone());
        }
        report = Some(step);
    }
    let report = report.unwrap();
    let slots: HashSet<u32> = report.plan.active.iter().map(|a| a.mask_slot).collect();
    assert!(!slots.is_empty());
    assert!(!slots.is_disjoint(&first_slots), "no slot was reused");
    let eye = cam.position();
    for active in &report.plan.active {
        let update = &latest[&active.mask_slot];
        let anchor = expected_anchor(&fx, active.key.node, update.node_depth, ORIGIN, eye);
        let expected = node_record(&NodeLod {
            anchor,
            node_depth: update.node_depth,
            active: update.active,
            mask: update.mask,
        });
        assert_eq!(
            fx.gpu.mask_words[&active.mask_slot], expected,
            "slot {}",
            active.mask_slot
        );
    }

    // Moving the eye rewrites only slots a node holds: the freed ones stay untouched.
    fx.gpu.record_writes.clear();
    fx.gpu.eye = std::array::from_fn(|axis| eye[axis] + 0.25);
    let view = fx.view(&cam);
    fx.driver
        .frame(&mut fx.gpu, &ids(&[4]), &view, 4.0)
        .expect("frame");
    let written: HashSet<u32> = fx.gpu.record_writes.iter().map(|(slot, _)| *slot).collect();
    let free: HashSet<u32> = fx.driver.pool().free_mask_slots().collect();
    assert!(!free.is_empty() && written.is_disjoint(&free));
    assert!(written.is_superset(&slots));
}

/// Pages of a node at its own corner plus a relative offset, as the packer writes them.
fn corner_level(fx: &Fixture, node: NodeId, depth: u8, rel: [u32; 3]) -> PackedLevel {
    let grid = fx.driver.config.grid;
    let bounds = fx.nodes[&node].bounds;
    let node_depth = 1;
    let mut level = level_of(node, depth, 1);
    for page in &mut level.pages {
        page.origin = std::array::from_fn(|axis| {
            let lo = crate::node_cells::node_range(&grid, bounds, node_depth, axis).0;
            (lo << (depth - node_depth)) + u64::from(rel[axis])
        });
        page.rel_origin = rel;
    }
    level
}

#[test]
fn anchor_plus_relative_origin_is_the_page_origin() {
    let nodes = octant_nodes();
    let mut fx = Fixture::new(HashMap::new(), config(128, 8, u64::MAX));
    fx.nodes = nodes.clone();
    let rel = [3, 5, 1];
    for (id, node) in &nodes {
        let levels = node
            .stored
            .iter()
            .map(|depth| corner_level(&fx, *id, *depth, rel))
            .collect();
        fx.driver
            .add_packed(*id, node.bounds, node.points, levels)
            .unwrap();
    }
    let cam = camera("zoom");
    // A scale other than 1 so the check is not a coincidence of the identity.
    fx.scale = 1.5;
    let report = fx.run(40, &ids(&CHILDREN), &cam, 4.0);
    assert!(!fx.gpu.table.is_empty());
    for (_, entry) in &fx.gpu.table {
        let words = fx.gpu.mask_words[&entry.0[4]];
        assert_eq!(words[19], 1, "node depth");
        let depth = entry.0[11];
        assert!(depth >= 4);
        let side = entry.side();
        let anchor: [f32; 3] = std::array::from_fn(|axis| f32::from_bits(words[16 + axis]));
        let origin = entry.origin();
        for axis in 0..3 {
            let rebuilt = anchor[axis] + entry.0[8 + axis] as f32 * side;
            let ulp = (origin[axis].abs().max(rebuilt.abs())) * f32::EPSILON;
            assert!(
                (rebuilt - origin[axis]).abs() <= ulp,
                "axis {axis}: {rebuilt} vs {}",
                origin[axis]
            );
        }
    }
    assert!(!report.plan.active.is_empty());
}
