use super::cells::{MAX_CELL_CUT_GAP, cell_leaf};
use super::*;
use std::{collections::HashMap, io::Cursor};
use viewer_core::{
    ColorRgb, HierarchySemantics, Point, PointBatch, SpatialBounds, SpatialHierarchy,
};
use viewer_format::{
    CrsRecord, FormatMetadata, FormatReader, FormatReaderLimits, FormatVersion, FormatWriter,
    FormatWriterOptions, GlobalFrame, NodeCell, OctreeReader, PagedReader, SourcePointBlock,
};
use viewer_render_wgpu::PackedNode;
use viewer_render_wgpu::{
    CellDriver, CellPoolError, CellTarget, DriverConfig, MASK_WORDS, OrbitCamera, PackedLevel,
    PageEntry, PageFrame, PoolConfig, SceneBounds, ViewProjection, VoxelBuildLimits,
};

pub(super) const SIDE: f64 = 64.0;

/// Root (1) over its eight octants (2..=9); node 2 also has eight children (10..=17).
pub(super) fn octree_nodes() -> Vec<SpatialNode> {
    let cube = |min: [f64; 3], side: f64| SpatialBounds::new(min, min.map(|v| v + side)).unwrap();
    let node = |id: u64, parent: Option<u64>, level: u32, bounds| {
        SpatialNode::new(
            NodeId::new(id),
            parent.map(NodeId::new),
            level,
            bounds,
            200,
            0.0,
            vec![BlockId::new(100 + id)],
        )
        .unwrap()
    };
    let octant = |min: [f64; 3], side: f64, index: u64| {
        std::array::from_fn(|axis| min[axis] + side * ((index >> axis) & 1) as f64)
    };
    let mut nodes = vec![node(1, None, 0, cube([0.0; 3], SIDE))];
    for index in 0..8 {
        nodes.push(node(
            2 + index,
            Some(1),
            1,
            cube(octant([0.0; 3], SIDE / 2.0, index), SIDE / 2.0),
        ));
    }
    for index in 0..8 {
        nodes.push(node(
            10 + index,
            Some(2),
            2,
            cube(octant([0.0; 3], SIDE / 4.0, index), SIDE / 4.0),
        ));
    }
    nodes
}

pub(super) fn octree_dataset(
    nodes: Vec<SpatialNode>,
    semantics: HierarchySemantics,
    global_frame: bool,
) -> LodDataset {
    let total = nodes.iter().map(SpatialNode::point_count).sum();
    let hierarchy =
        SpatialHierarchy::new_with_semantics(nodes, Vec::new(), Some(total), semantics).unwrap();
    LodDataset::new([0.0; 3], hierarchy, 64 << 20)
        .unwrap()
        .with_global_frame(global_frame)
}

fn cell_dataset() -> LodDataset {
    octree_dataset(octree_nodes(), HierarchySemantics::Replacement, true)
}

/// 200 pseudo-random points inside the bounds of the node that owns `block`.
pub(super) fn node_block(dataset: &LodDataset, node: u64) -> PointBlock {
    let bounds = dataset
        .metadata
        .hierarchy()
        .node(NodeId::new(node))
        .unwrap()
        .bounds();
    let mut state = 0x9e37_79b9_u64 ^ node;
    let mut unit = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 40) as f64) / ((1_u64 << 24) as f64)
    };
    let points = (0..200)
        .map(|_| {
            let position = std::array::from_fn(|axis| {
                let (min, max) = (bounds.min()[axis], bounds.max()[axis]);
                (min + (max - min) * (0.01 + 0.98 * unit())) as f32
            });
            Point::new(position, ColorRgb::new(10, 120, 200))
        })
        .collect();
    PointBlock::new(
        BlockId::new(100 + node),
        PointBatch::new(points),
        Vec::new(),
    )
}

fn load_all(dataset: &mut LodDataset) {
    for node in (1..=17).collect::<Vec<u64>>() {
        let block = node_block(dataset, node);
        dataset.insert_block(block).unwrap();
    }
}

pub(super) fn view_at(eye: Option<[f64; 3]>) -> ViewProjection {
    let viewport = [800, 600];
    let mut camera = OrbitCamera::default();
    assert!(camera.fit(
        SceneBounds {
            min: [0.0; 3],
            max: [SIDE; 3],
        },
        viewport,
    ));
    if let Some(eye) = eye {
        assert!(camera.set_pose(eye, [SIDE / 2.0; 3]));
    }
    ViewProjection::new(camera, viewport, 1.0, VoxelBuildLimits::default())
}

/// A target that accepts everything, remembers whether it has a pool and the last `PageFrame` it
/// was given. `fail` makes its writes fail.
#[derive(Default)]
struct NullTarget {
    pool: bool,
    masks: u32,
    eye: [f64; 3],
    fail: bool,
    last_frame: Option<PageFrame>,
}

impl NullTarget {
    /// A target whose eye is the camera eye of `view`, as a renderer's would be.
    fn at(view: &ViewProjection) -> Self {
        Self {
            eye: view.camera().position(),
            ..Self::default()
        }
    }
}

impl CellTarget for NullTarget {
    type Error = CellPoolError;
    fn has_pool(&self) -> bool {
        self.pool
    }
    fn drop_pool(&mut self) {
        self.pool = false;
        self.masks = 0;
    }
    fn eye(&self) -> [f64; 3] {
        self.eye
    }
    fn upload_level(&mut self, _: &PackedLevel, _: &[u32]) -> Result<(), CellPoolError> {
        if self.fail {
            return Err(CellPoolError::NoPool);
        }
        self.pool = true;
        Ok(())
    }
    fn free_pages(&mut self, _: &[u32]) -> Result<(), CellPoolError> {
        Ok(())
    }
    fn alloc_mask_slot(&mut self) -> Result<Option<u32>, CellPoolError> {
        self.pool = true;
        self.masks += 1;
        Ok(Some(self.masks - 1))
    }
    fn set_node_mask(&mut self, _: u32, _: &[u32; MASK_WORDS]) -> Result<(), CellPoolError> {
        Ok(())
    }
    fn set_active_pages(
        &mut self,
        _: &[(u32, PageEntry)],
        frame: &PageFrame,
    ) -> Result<(), CellPoolError> {
        if self.fail {
            return Err(CellPoolError::NoPool);
        }
        self.last_frame = Some(*frame);
        self.pool = true;
        Ok(())
    }
}

pub(super) fn driver_for(dataset: &LodDataset, cpu_budget_bytes: u64) -> CellDriver {
    CellDriver::new(DriverConfig {
        pool: PoolConfig::default(),
        grid: dataset.cell_grid(),
        coordinate_origin: dataset.coordinate_origin(),
        cpu_budget_bytes,
    })
}

/// Builds the cells of `id` and hands them to the driver, as steps 1 and 2 do.
fn build_into(dataset: &mut LodDataset, driver: &mut CellDriver, id: NodeId) {
    let generation = dataset.cell_generation();
    let packed = dataset.build_node_cells(id).unwrap();
    crate::add_cell_nodes(driver, packed.map(|node| (generation, node))).unwrap();
}

/// A target node whose cells exist is in the plan; one whose blocks are only decoded is not.
fn plan_nodes(dataset: &mut LodDataset, px: f64) -> Vec<NodeId> {
    let (_, selection) = dataset.select_target(&view_at(None), px).unwrap();
    dataset.prepare_cell_plan(&selection).node_ids().to_vec()
}

#[test]
fn the_cell_path_needs_a_global_frame_and_replacement_semantics() {
    let view = view_at(None);
    let cases = [
        // A v5 / v7.1-3 style dataset has no global frame.
        (octree_nodes(), HierarchySemantics::Replacement, false),
        (octree_nodes(), HierarchySemantics::Additive, true),
        (octree_nodes(), HierarchySemantics::Replacement, true),
    ];
    let expected = [false, false, true];
    for ((nodes, semantics, frame), supported) in cases.into_iter().zip(expected) {
        let mut dataset = octree_dataset(nodes, semantics, frame);
        load_all(&mut dataset);
        let mut driver = driver_for(&dataset, u64::MAX);
        assert_eq!(dataset.supports_cells(), supported);
        let result = dataset.update_cells(
            &view,
            64.0,
            CellUpdateConfig::default(),
            &mut driver,
            &mut NullTarget::default(),
            None,
            |_| Ok(None),
        );
        match (supported, result) {
            (true, Ok(update)) => assert!(update.stats.cut_nodes > 0),
            (false, Err(LodDatasetError::CellPathUnsupported)) => {}
            (_, other) => panic!(
                "unexpected result for supported={supported}: {:?}",
                other.err()
            ),
        }
    }
}

#[test]
fn a_node_counts_as_resident_for_the_plan_only_once_its_cells_exist() {
    let mut dataset = cell_dataset();
    load_all(&mut dataset);
    let mut driver = driver_for(&dataset, u64::MAX);
    let targets: Vec<NodeId> = (2..=9).map(NodeId::new).collect();
    // Every block is decoded and the plain planner would take the eight targets...
    let (_, selection) = dataset.select_target(&view_at(None), 64.0).unwrap();
    assert_eq!(selection.node_ids(), targets);
    assert_eq!(dataset.prepare_plan(&selection).node_ids(), targets);
    // ...but without cells the cell planner takes none of them.
    assert!(plan_nodes(&mut dataset, 64.0).is_empty());

    // With the root built, it covers the region as the fallback of every unbuilt target.
    build_into(&mut dataset, &mut driver, NodeId::new(1));
    assert_eq!(plan_nodes(&mut dataset, 64.0), [NodeId::new(1)]);

    // A target built afterwards joins the plan next to its fallback.
    build_into(&mut dataset, &mut driver, NodeId::new(3));
    assert_eq!(
        plan_nodes(&mut dataset, 64.0),
        [NodeId::new(1), NodeId::new(3)]
    );
}

/// ADR 0038: on the cells path a node with cells stays resident when its blocks leave the cache
/// and is not built again; the CPU path still drops a node whose block is gone.
#[test]
fn a_node_with_cells_stays_in_the_cut_after_its_blocks_leave_the_cache() {
    let mut dataset = cell_dataset();
    load_all(&mut dataset);
    let mut driver = driver_for(&dataset, u64::MAX);
    build_into(&mut dataset, &mut driver, NodeId::new(1));
    build_into(&mut dataset, &mut driver, NodeId::new(3));
    let evicted = [BlockId::new(101), BlockId::new(103)];
    for block in evicted {
        dataset.cache.remove(block);
    }

    // CPU path: the same cache loses the node from the plain plan.
    let (_, selection) = dataset.select_target(&view_at(None), 64.0).unwrap();
    assert!(
        !dataset
            .prepare_plan(&selection)
            .node_ids()
            .contains(&NodeId::new(3))
    );

    // Cells path: both stay in the plan and in the cut, and neither is built again.
    assert_eq!(
        plan_nodes(&mut dataset, 64.0),
        [NodeId::new(1), NodeId::new(3)]
    );
    let update = dataset
        .update_cells(
            &view_at(None),
            64.0,
            CellUpdateConfig::default(),
            &mut driver,
            &mut NullTarget::default(),
            None,
            |_| Ok(None),
        )
        .unwrap();
    assert!(
        update
            .stats
            .pending_block_ids
            .iter()
            .all(|b| !evicted.contains(b))
    );
    assert!(
        update
            .stats
            .built_nodes
            .iter()
            .all(|id| id.get() != 1 && id.get() != 3)
    );
    let plan = dataset.visible_plan.as_ref().unwrap();
    assert!(plan.node_ids().contains(&NodeId::new(3)));
    assert!(evicted.iter().all(|b| !dataset.cache.contains(*b)));
}

#[test]
fn at_most_the_configured_nodes_are_built_per_update_nearest_first() {
    let mut dataset = cell_dataset();
    load_all(&mut dataset);
    let mut driver = driver_for(&dataset, u64::MAX);
    // The eye hangs right beside node 9, the octant at the far corner.
    let view = view_at(Some([70.0, 68.0, 66.0]));
    let mut target = NullTarget::at(&view);
    let config = CellUpdateConfig {
        max_node_builds: 2,
        ..CellUpdateConfig::default()
    };
    let mut rounds = Vec::new();
    for round in 0..5 {
        let update = dataset
            .update_cells(&view, 64.0, config, &mut driver, &mut target, None, |_| {
                Ok(None)
            })
            .unwrap();
        // Nine nodes wait: the eight targets and the root, which goes first (R3-Q22).
        assert_eq!(
            update.stats.built_nodes.len(),
            if round < 4 { 2 } else { 1 }
        );
        assert_eq!(update.stats.build_capped, round < 4);
        assert_eq!(
            update.stats.build_deferred,
            7_usize.saturating_sub(2 * round)
        );
        rounds.push(update.stats.built_nodes);
    }
    // The root contains the eye, so it comes first: parents before children (R3-Q22). Distances
    // to the eye (axis gaps 6/38, 4/36, 2/34 to the high/low half) are all distinct: node 9, then
    // 5, 7, 8 (one far axis, z first), 3, 4, 6 (two), and node 2. The code breaks exact ties by
    // ascending `NodeId`.
    let ids = |list: &[u64]| list.iter().map(|id| NodeId::new(*id)).collect::<Vec<_>>();
    assert_eq!(
        rounds,
        [
            ids(&[1, 9]),
            ids(&[5, 7]),
            ids(&[8, 3]),
            ids(&[4, 6]),
            ids(&[2])
        ],
        "nearest first, ties by ascending NodeId"
    );
    let done = dataset
        .update_cells(&view, 64.0, config, &mut driver, &mut target, None, |_| {
            Ok(None)
        })
        .unwrap();
    assert_eq!(
        (done.stats.built_nodes.len(), done.stats.build_capped),
        (0, false)
    );
    assert_eq!(done.stats.cut_nodes, 8);
    // The driver wrote the frame of this view: its eye, origin, scale and grid.
    let frame = target.last_frame.expect("a frame was written");
    assert_eq!(frame.eye_scaled, view.camera().position());
    assert_eq!(frame.coordinate_origin, dataset.coordinate_origin());
    assert_eq!(frame.visual_scale, view.visual_scale());
    assert_eq!(frame.grid, dataset.cell_grid());
}

/// Loads blocks on demand and updates until the target is fully built and drawn.
fn run_to_completion(
    dataset: &mut LodDataset,
    driver: &mut CellDriver,
    target: &mut NullTarget,
    view: &ViewProjection,
    px: f64,
) -> Vec<CellUpdateStats> {
    let blocks: HashMap<BlockId, PointBlock> = (1..=17)
        .map(|node| (BlockId::new(100 + node), node_block(dataset, node)))
        .collect();
    let mut history = Vec::new();
    for _ in 0..60 {
        let update = dataset
            .update_cells(
                view,
                px,
                CellUpdateConfig::default(),
                driver,
                target,
                None,
                |id| Ok(blocks.get(&id).cloned()),
            )
            .unwrap();
        let done = update.stats.pending_block_ids.is_empty()
            && update.stats.build_deferred == 0
            && update.stats.built_nodes.is_empty()
            && !driver.needs_frame();
        history.push(update.stats);
        if done {
            return history;
        }
    }
    panic!("the update did not settle");
}

#[test]
fn updates_stream_blocks_build_cells_and_keep_the_gap_within_the_mask() {
    let mut dataset = cell_dataset();
    let mut driver = driver_for(&dataset, u64::MAX);
    let mut target = NullTarget::default();
    let view = view_at(None);
    let history = run_to_completion(&mut dataset, &mut driver, &mut target, &view, 16.0);
    assert!(history.iter().all(|stats| stats.built_nodes.len() <= 2));
    assert!(
        history
            .iter()
            .all(|stats| stats.max_gap <= MAX_CELL_CUT_GAP)
    );
    assert!(history.iter().all(|stats| !stats.descendant_too_fine));
    // Nodes 10..=17 and 3..=9 are the cut at 16 px.
    let last = history.last().unwrap();
    assert_eq!(last.cut_nodes, 15);
    assert_eq!(last.apply, viewer_render_wgpu::CutApply::Applied);
    assert_eq!(last.uncovered_nodes, 0);
    let mut drawn = dataset.cells.pool_drawn().to_vec();
    drawn.sort();
    let mut expected: Vec<NodeId> = (3..=17).map(NodeId::new).collect();
    expected.sort();
    assert_eq!(drawn, expected);
}

#[test]
fn nodes_the_driver_forgot_are_built_again() {
    let mut dataset = cell_dataset();
    let mut driver = driver_for(&dataset, u64::MAX);
    let mut target = NullTarget::default();
    let view = view_at(None);
    run_to_completion(&mut dataset, &mut driver, &mut target, &view, 64.0);
    assert!(driver.has_node(NodeId::new(5)));

    // A driver without those nodes is what a forgotten node looks like from the dataset.
    let mut fresh = driver_for(&dataset, u64::MAX);
    let mut target = NullTarget::default();
    let history = run_to_completion(&mut dataset, &mut fresh, &mut target, &view, 64.0);
    let rebuilt: usize = history.iter().map(|stats| stats.built_nodes.len()).sum();
    // The eight targets and the root, their parent (R3-Q22).
    assert_eq!(rebuilt, 9);
    assert!(fresh.has_node(NodeId::new(5)));
    assert_eq!(history.last().unwrap().cut_nodes, 8);
}

#[test]
fn a_node_forgotten_under_the_cpu_budget_is_evicted_and_built_and_sent_again() {
    let view = view_at(None);
    let spare = NodeId::new(12);
    // Node 12 is built ahead and then sits in the driver outside every cut at 64 px, with no
    // level in the pool: a budget one byte below the nine targets plus 12 makes it the one to go.
    let (full, spare_bytes) = {
        let mut dataset = cell_dataset();
        let mut driver = driver_for(&dataset, u64::MAX);
        let mut target = NullTarget::default();
        run_to_completion(&mut dataset, &mut driver, &mut target, &view, 64.0);
        let before = driver.cpu_bytes();
        build_into(&mut dataset, &mut driver, spare);
        (before, driver.cpu_bytes() - before)
    };
    let mut dataset = cell_dataset();
    let mut driver = driver_for(&dataset, full + spare_bytes - 1);
    let mut target = NullTarget::at(&view);
    let blocks: HashMap<BlockId, PointBlock> = (1..=17)
        .map(|node| (BlockId::new(100 + node), node_block(&dataset, node)))
        .collect();
    dataset
        .insert_block(blocks[&BlockId::new(112)].clone())
        .unwrap();
    build_into(&mut dataset, &mut driver, spare);
    let (forgotten, spare_builds) = (std::cell::Cell::new(0), std::cell::Cell::new(0));
    let mut run = |px: f64, rounds: usize, dataset: &mut LodDataset, driver: &mut CellDriver| {
        for _ in 0..rounds {
            let update = dataset
                .update_cells(
                    &view,
                    px,
                    CellUpdateConfig::default(),
                    driver,
                    &mut target,
                    None,
                    |id| Ok(blocks.get(&id).cloned()),
                )
                .unwrap();
            let count = |ids: &[NodeId]| ids.iter().filter(|id| **id == spare).count();
            forgotten.set(forgotten.get() + count(&update.report.forgotten));
            spare_builds.set(spare_builds.get() + count(&update.stats.built_nodes));
            // What the driver reports forgotten is built again when needed, never a node the
            // dataset still believes the driver holds.
            for id in &update.report.forgotten {
                assert!(!dataset.cells.has_cells(*id));
            }
            if spare_builds.get() > 0 && !driver.needs_frame() {
                return;
            }
        }
    };
    run(64.0, 30, &mut dataset, &mut driver);
    assert_eq!(forgotten.get(), 1, "the idle node was not forgotten");
    assert!(!driver.has_node(spare) && !dataset.cells.has_cells(spare));
    // At 8 px node 12 is a target: evicted, so built and sent again (a repeat would be
    // `Duplicate` or ignored, a missing one `UnknownNode`).
    run(8.0, 80, &mut dataset, &mut driver);
    assert_eq!(spare_builds.get(), 1, "node 12 was not built again");
    assert!(driver.has_node(spare));
    // The one forget was of a node built outside a plan (role `Selected`), and its second build
    // was counted as a rebuild.
    let diag = dataset.cells.diag;
    assert_eq!(
        (
            diag.forgotten_selected,
            diag.forgotten_waiter,
            diag.forgotten_ancestor
        ),
        (1, 0, 0)
    );
    assert_eq!(diag.rebuilt_total, 1);
}

/// A chain: node n+1 is the first octant of node n, one level and one depth down.
fn chain_nodes() -> Vec<SpatialNode> {
    (0..5_u64)
        .map(|index| {
            SpatialNode::new(
                NodeId::new(index + 1),
                index.checked_sub(1).map(|p| NodeId::new(p + 1)),
                index as u32,
                SpatialBounds::new([0.0; 3], [SIDE / (1_u64 << index) as f64; 3]).unwrap(),
                10,
                0.0,
                vec![BlockId::new(index + 1)],
            )
            .unwrap()
        })
        .collect()
}

#[test]
fn a_cut_gap_above_the_mask_is_an_error_and_smaller_gaps_are_counted() {
    let dataset = octree_dataset(chain_nodes(), HierarchySemantics::Replacement, true);
    let ids = |list: &[u64]| list.iter().map(|id| NodeId::new(*id)).collect::<Vec<_>>();
    assert_eq!(dataset.cut_gaps(&ids(&[1, 2, 3])).unwrap(), (1, 0));
    assert_eq!(dataset.cut_gaps(&ids(&[1, 3, 4])).unwrap(), (2, 1));
    assert_eq!(dataset.cut_gaps(&ids(&[1, 4])).unwrap(), (3, 1));
    assert!(matches!(
        dataset.cut_gaps(&ids(&[1, 5])),
        Err(LodDatasetError::CellCutGap { gap: 4, .. })
    ));
}

#[test]
fn the_gap_is_the_geometric_depth_not_the_hierarchy_level() {
    // A child one level down whose bounds are 32 times smaller (a converter with 32 divisions
    // per level, say): the mask limit is about cell depth, so the gap is 5.
    let node = |id: u64, parent: Option<u64>, level: u32, side: f64| {
        SpatialNode::new(
            NodeId::new(id),
            parent.map(NodeId::new),
            level,
            SpatialBounds::new([0.0; 3], [side; 3]).unwrap(),
            10,
            0.0,
            vec![BlockId::new(id)],
        )
        .unwrap()
    };
    let dataset = octree_dataset(
        vec![node(1, None, 0, SIDE), node(2, Some(1), 1, SIDE / 32.0)],
        HierarchySemantics::Replacement,
        true,
    );
    assert!(matches!(
        dataset.cut_gaps(&[NodeId::new(1), NodeId::new(2)]),
        Err(LodDatasetError::CellCutGap { gap: 5, .. })
    ));
}

#[test]
fn an_update_whose_cut_has_a_gap_above_the_mask_fails() {
    // The planner only pairs a fallback with its direct, grid-aligned children. A deep gap needs
    // a child that is one level down but 16 times smaller, which the alignment test allows when
    // its edges sit on the finer level grid: nodes 1..=5 are a chain of equal bounds (levels
    // 0..=4) and nodes 6 and 7 are small children of 5 at level 5, which node 5 allows because
    // its stored spacing (a v7.4-style 256 representatives per axis) reaches that deep. Node 7
    // has no block, so node 5 stands in for it and node 6, built, joins it in the cut four cell
    // depths below.
    let node = |id: u64, parent: Option<u64>, level: u32, min: [f64; 3], side: f64| {
        let spacing = if id == 5 { SIDE / 256.0 } else { side / 8.0 };
        SpatialNode::new(
            NodeId::new(id),
            parent.map(NodeId::new),
            level,
            SpatialBounds::new(min, min.map(|v| v + side)).unwrap(),
            10,
            0.0,
            vec![BlockId::new(id)],
        )
        .unwrap()
        .with_spacing(spacing)
        .unwrap()
    };
    let mut nodes: Vec<SpatialNode> = (1..=5)
        .map(|id| {
            node(
                id,
                id.checked_sub(2).map(|p| p + 1),
                id as u32 - 1,
                [0.0; 3],
                SIDE,
            )
        })
        .collect();
    nodes[0] = node(1, None, 0, [0.0; 3], SIDE);
    nodes.push(node(6, Some(5), 5, [0.0; 3], 4.0));
    nodes.push(node(7, Some(5), 5, [32.0, 0.0, 0.0], 4.0));
    let mut dataset = octree_dataset(nodes, HierarchySemantics::Replacement, true);
    for node in [5_u64, 6] {
        let bounds = dataset
            .metadata
            .hierarchy()
            .node(NodeId::new(node))
            .unwrap()
            .bounds();
        let points = (1..=10)
            .map(|i| {
                let at = |axis: usize| {
                    (bounds.min()[axis]
                        + (bounds.max()[axis] - bounds.min()[axis]) * f64::from(i) / 11.0)
                        as f32
                };
                Point::new([at(0), at(1), at(2)], ColorRgb::new(1, 2, 3))
            })
            .collect();
        dataset
            .insert_block(PointBlock::new(
                BlockId::new(node),
                PointBatch::new(points),
                Vec::new(),
            ))
            .unwrap();
    }
    let mut driver = driver_for(&dataset, u64::MAX);
    for node in [5_u64, 6] {
        build_into(&mut dataset, &mut driver, NodeId::new(node));
    }
    let view = view_at(None);
    let error = dataset
        .update_cells(
            &view,
            0.001,
            CellUpdateConfig::default(),
            &mut driver,
            &mut NullTarget::at(&view),
            None,
            |_| Ok(None),
        )
        .err()
        .expect("the cut has a gap of 4");
    assert!(
        matches!(error, LodDatasetError::CellCutGap { gap: 4, .. }),
        "{error}"
    );
}

/// Root (1) with two unloaded anchors 2 and 3, as in `lod_dataset_tests::paged_dataset`.
fn paged_cell_dataset(budget: usize) -> LodDataset {
    let node = |id: u64, parent: Option<u64>, level: u32, min: f64, max: f64| {
        SpatialNode::new(
            NodeId::new(id),
            parent.map(NodeId::new),
            level,
            SpatialBounds::new([min, 0.0, 0.0], [max, 8.0, 8.0]).unwrap(),
            10,
            0.0,
            vec![BlockId::new(id)],
        )
        .unwrap()
    };
    let hierarchy = SpatialHierarchy::new(
        vec![
            node(1, None, 0, 0.0, 8.0),
            node(2, Some(1), 1, 0.0, 4.0),
            node(3, Some(1), 1, 4.0, 8.0),
        ],
        Vec::new(),
        None,
    )
    .unwrap();
    LodDataset::new([0.0; 3], hierarchy, 1 << 20)
        .unwrap()
        .with_global_frame(true)
        .with_pages(&[NodeId::new(2), NodeId::new(3)], Box::new(|_| {}), budget)
        .unwrap()
}

fn page_below(anchor: u64, number: u32, child: u64, min: f64, max: f64) -> crate::PageInsert {
    let node = SpatialNode::new(
        NodeId::new(child),
        Some(NodeId::new(anchor)),
        2,
        SpatialBounds::new([min, 0.0, 0.0], [max, 4.0, 4.0]).unwrap(),
        5,
        0.0,
        vec![BlockId::new(child)],
    )
    .unwrap();
    crate::PageInsert {
        anchor: NodeId::new(anchor),
        number,
        nodes: vec![node],
        unloaded: Vec::new(),
        bytes: 100,
    }
}

#[test]
fn a_page_holding_a_node_the_pool_draws_is_not_evicted() {
    // `paged_dataset` of the other tests is a strip, not an octree, but eviction only looks at
    // node ids and pages, so the same shape serves: page 1 holds node 20.
    let mut dataset = paged_cell_dataset(150);
    dataset.graft_page(page_below(2, 1, 20, 0.0, 2.0)).unwrap();
    assert!(dataset.pinned_nodes().is_empty());

    // The pool still draws node 20 (also while it retains the previous cut): the page stays.
    dataset.cells.pool_drawn = vec![NodeId::new(20)];
    assert!(dataset.pinned_nodes().contains(&NodeId::new(20)));
    let error = dataset
        .graft_page(page_below(3, 2, 30, 4.0, 6.0))
        .unwrap_err();
    assert!(
        matches!(error, LodDatasetError::PageBudget { pinned: 100, .. }),
        "{error}"
    );
    assert!(dataset.references_block(BlockId::new(20)));

    // Once the pool lets go of it, the same graft evicts that page.
    dataset.cells.pool_drawn.clear();
    dataset.graft_page(page_below(3, 2, 30, 4.0, 6.0)).unwrap();
    assert!(!dataset.references_block(BlockId::new(20)));
}

#[test]
fn the_nodes_the_pool_draws_stay_pinned_when_nothing_else_names_them() {
    let mut dataset = cell_dataset();
    let mut driver = driver_for(&dataset, u64::MAX);
    let view = view_at(None);
    let mut target = NullTarget::at(&view);
    run_to_completion(&mut dataset, &mut driver, &mut target, &view, 16.0);
    let mut drawn = dataset.cells.pool_drawn().to_vec();
    drawn.sort();
    assert_eq!(drawn, (3..=17).map(NodeId::new).collect::<Vec<_>>());

    // Under `RetainedPrevious` the pool draws a cut the plan and the selection already left. The
    // fixtures never retain on their own, so drop the other pins by hand: only the pool's remain.
    dataset.planner.reset(&mut dataset.cache);
    dataset.visible_plan = None;
    dataset.invalidate_selection();
    let mut pinned = dataset.pinned_nodes();
    pinned.sort();
    assert_eq!(pinned, drawn);

    // A reset driver draws nothing, and a failed frame leaves no pin either.
    dataset.pool_reset();
    assert!(dataset.pinned_nodes().is_empty());
    let _ = driver.reset();
    dataset.cells.pool_drawn = drawn.clone();
    target.fail = true;
    let error = dataset.update_cells(
        &view,
        16.0,
        CellUpdateConfig::default(),
        &mut driver,
        &mut target,
        None,
        |_| Ok(None),
    );
    assert!(matches!(error, Err(LodDatasetError::CellDriver(_))));
    assert!(dataset.cells.pool_drawn().is_empty());
}

#[test]
fn a_block_shared_by_two_nodes_does_not_starve_the_node_that_is_in_the_cut() {
    // Node 10 (child of node 2, not in the cut at 64 px) reads the block of node 2.
    let mut nodes = octree_nodes();
    let shared = nodes
        .iter()
        .position(|n| n.id() == NodeId::new(10))
        .unwrap();
    let old = &nodes[shared];
    nodes[shared] = SpatialNode::new(
        old.id(),
        old.parent(),
        old.level(),
        old.bounds(),
        200,
        0.0,
        vec![BlockId::new(102)],
    )
    .unwrap();
    let mut dataset = octree_dataset(nodes, HierarchySemantics::Replacement, true);
    assert_eq!(
        dataset.block_owners[&BlockId::new(102)],
        [NodeId::new(2), NodeId::new(10)]
    );
    // The shared block holds points inside node 10, which lies inside node 2.
    let inner = node_block(&dataset, 10);
    for node in (1..=17).filter(|node| *node != 10) {
        let block = if node == 2 {
            PointBlock::new(BlockId::new(102), inner.points().clone(), Vec::new())
        } else {
            node_block(&dataset, node)
        };
        dataset.insert_block(block).unwrap();
    }
    let mut driver = driver_for(&dataset, u64::MAX);
    let view = view_at(None);
    let mut target = NullTarget::at(&view);
    let mut last = None;
    for _ in 0..30 {
        let update = dataset
            .update_cells(
                &view,
                64.0,
                CellUpdateConfig::default(),
                &mut driver,
                &mut target,
                None,
                |_| Ok(None),
            )
            .unwrap();
        let settled = update.stats.built_nodes.is_empty()
            && update.stats.build_deferred == 0
            && !driver.needs_frame();
        last = Some(update.stats);
        if settled {
            break;
        }
    }
    let last = last.unwrap();
    // The eight octants are drawn, node 2 among them, and node 10 got its cells as a co-owner.
    assert_eq!(last.cut_nodes, 8);
    assert_eq!(last.uncovered_nodes, 0);
    assert!(dataset.cells.has_cells(NodeId::new(10)));
    assert!(dataset.cells.pool_drawn().contains(&NodeId::new(2)));
}

#[test]
fn a_shared_block_is_resident_only_when_every_owner_has_cells_and_leaves_no_hole() {
    // Node 10 reads the block of node 2 (see the test above).
    let mut nodes = octree_nodes();
    let shared = nodes
        .iter()
        .position(|n| n.id() == NodeId::new(10))
        .unwrap();
    let old = &nodes[shared];
    nodes[shared] = SpatialNode::new(
        old.id(),
        old.parent(),
        old.level(),
        old.bounds(),
        200,
        0.0,
        vec![BlockId::new(102)],
    )
    .unwrap();
    let mut dataset = octree_dataset(nodes, HierarchySemantics::Replacement, true);
    let inner = node_block(&dataset, 10);
    for node in (1..=17).filter(|node| *node != 10) {
        let block = if node == 2 {
            PointBlock::new(BlockId::new(102), inner.points().clone(), Vec::new())
        } else {
            node_block(&dataset, node)
        };
        dataset.insert_block(block).unwrap();
    }
    let shared_block = BlockId::new(102);
    dataset.build_node_cells(NodeId::new(1)).unwrap();
    dataset.build_node_cells(NodeId::new(2)).unwrap();
    // Only one of the two owners has cells: the block is not resident for the planner, and the
    // cut falls back on the root instead of leaving a hole.
    assert!(dataset.cells.has_cells(NodeId::new(2)));
    assert!(!dataset.cell_ready_blocks().contains(&shared_block));
    let view = view_at(None);
    let (_, selection) = dataset.select_target(&view, 64.0).unwrap();
    let plan = dataset.prepare_cell_plan(&selection);
    assert!(!plan.node_ids().contains(&NodeId::new(2)));
    assert_eq!(plan.node_ids(), [NodeId::new(1)]);
    // The second owner gets its cells: the block turns resident and node 2 is drawable.
    dataset.build_node_cells(NodeId::new(10)).unwrap();
    assert!(dataset.cell_ready_blocks().contains(&shared_block));
    let plan = dataset.prepare_cell_plan(&selection);
    assert!(plan.node_ids().contains(&NodeId::new(2)));
}

/// Each of the eight octants (2..=9) has a child that reads the block of its parent (as a shared
/// block of a real file does): node 10 for octant 2 and nodes 20..=26 for octants 3..=9. At 64 px
/// the cut is the eight octants, and the eight children are co-owners of the block of a selected
/// node, so they are built as waiters although the cut never names them.
fn waiters_dataset() -> LodDataset {
    let mut nodes = octree_nodes();
    let corner = |nodes: &[SpatialNode], octant: u64| {
        let parent = nodes.iter().find(|n| n.id().get() == octant).unwrap();
        let min = parent.bounds().min();
        SpatialBounds::new(min, min.map(|v| v + SIDE / 4.0)).unwrap()
    };
    let waiter = |id: u64, parent: u64, bounds| {
        SpatialNode::new(
            NodeId::new(id),
            Some(NodeId::new(parent)),
            2,
            bounds,
            200,
            0.0,
            vec![BlockId::new(100 + parent)],
        )
        .unwrap()
    };
    let at = nodes.iter().position(|n| n.id().get() == 10).unwrap();
    nodes[at] = waiter(10, 2, corner(&nodes, 2));
    for octant in 3..=9 {
        let child = waiter(20 + octant - 3, octant, corner(&nodes, octant));
        nodes.push(child);
    }
    let mut dataset = octree_dataset(nodes, HierarchySemantics::Replacement, true);
    let mut waiters = vec![(2, 10)];
    waiters.extend((3..=9).map(|octant| (octant, 20 + octant - 3)));
    for (octant, child) in waiters {
        // The shared block holds points inside the child, which lies inside the octant.
        let inner = node_block(&dataset, child);
        let block = PointBlock::new(
            BlockId::new(100 + octant),
            inner.points().clone(),
            Vec::new(),
        );
        dataset.insert_block(block).unwrap();
    }
    for node in std::iter::once(1).chain(11..=17) {
        let block = node_block(&dataset, node);
        dataset.insert_block(block).unwrap();
    }
    dataset
}

/// Updates at 64 px until nothing is built, deferred or pending in the driver (at most `limit`).
/// Returns the stats of the last update and the nodes the driver forgot along the way.
fn run_waiters(
    dataset: &mut LodDataset,
    driver: &mut CellDriver,
    config: CellUpdateConfig,
    limit: usize,
) -> (CellUpdateStats, usize, bool) {
    let view = view_at(None);
    let mut target = NullTarget::at(&view);
    let (mut last, mut forgotten, mut settled) = (None, 0, false);
    for _ in 0..limit {
        let update = dataset
            .update_cells(&view, 64.0, config, driver, &mut target, None, |_| Ok(None))
            .unwrap();
        forgotten += update.report.forgotten.len();
        settled = update.stats.built_nodes.is_empty()
            && update.stats.build_deferred == 0
            && !driver.needs_frame();
        last = Some(update.stats);
        if settled {
            break;
        }
    }
    (last.unwrap(), forgotten, settled)
}

#[test]
fn built_waiters_are_kept_and_counted_so_a_budget_below_cut_plus_waiters_settles() {
    // Unbounded run: the bytes of the cut (the octants) and of the waiters (the children).
    let mut free = waiters_dataset();
    let mut driver = driver_for(&free, u64::MAX);
    let (last, forgotten, settled) =
        run_waiters(&mut free, &mut driver, CellUpdateConfig::default(), 60);
    assert!(settled && forgotten == 0 && last.cut_nodes == 8);
    let bytes = |ids: std::ops::RangeInclusive<u64>| -> u64 {
        ids.map(|id| free.cells.built_bytes(NodeId::new(id)).expect("built"))
            .sum()
    };
    let waiter_bytes = bytes(10..=10) + bytes(20..=26);
    let cut_bytes = bytes(1..=9);
    assert!(waiter_bytes > 0 && cut_bytes > 0);
    // A node of the cut is drawn only once its co-owners have cells, so the cut needs cut and
    // waiters together: the budget holds a part of them.
    let budget = (cut_bytes + waiter_bytes) * 7 / 10;
    let mut dataset = waiters_dataset();
    let mut driver = driver_for(&dataset, budget);
    let config = CellUpdateConfig {
        cpu_budget_bytes: Some(budget),
        ..CellUpdateConfig::default()
    };
    let (last, forgotten, settled) = run_waiters(&mut dataset, &mut driver, config, 200);
    assert!(
        settled,
        "never settled; forgotten {forgotten}, last {last:?}"
    );
    assert!(last.cpu_coarsened > 0, "the waiters were not blocked");
    assert!((1..8).contains(&last.cut_nodes) && last.uncovered_nodes == 0);
    assert!(driver.cpu_bytes() <= budget);
    // Settled for good: nothing is forgotten or built in further updates.
    let (_, later, _) = run_waiters(&mut dataset, &mut driver, config, 50);
    assert_eq!(later, 0, "forgetting went on after settling");
    assert_eq!(forgotten, 0, "a waiter was built and forgotten");
}

#[test]
fn a_node_without_finite_points_is_resident_but_never_drawn() {
    let mut dataset = cell_dataset();
    for node in 1..=17 {
        let block = if node == 5 {
            let points = (0..4)
                .map(|_| Point::new([f32::NAN; 3], ColorRgb::new(1, 2, 3)))
                .collect();
            PointBlock::new(BlockId::new(105), PointBatch::new(points), Vec::new())
        } else {
            node_block(&dataset, node)
        };
        dataset.insert_block(block).unwrap();
    }
    let mut driver = driver_for(&dataset, u64::MAX);
    let view = view_at(None);
    let mut target = NullTarget::at(&view);
    let mut last = None;
    for _ in 0..30 {
        let update = dataset
            .update_cells(
                &view,
                64.0,
                CellUpdateConfig::default(),
                &mut driver,
                &mut target,
                None,
                |_| Ok(None),
            )
            .unwrap();
        let settled = update.stats.built_nodes.is_empty() && !driver.needs_frame();
        last = Some(update.stats);
        if settled {
            break;
        }
    }
    let last = last.unwrap();
    assert!(dataset.cells.empty.contains(&NodeId::new(5)));
    assert!(!driver.has_node(NodeId::new(5)));
    // The planner counts it as covered; the other seven octants are the cut.
    assert_eq!((last.cut_nodes, last.uncovered_nodes), (7, 0));
}

#[test]
fn a_leaf_is_a_resident_node_without_children_never_an_unloaded_anchor() {
    // A paged v7.5 file with a root, two children and a grandchild, one page per node.
    let root_level = 40;
    let root_prefix = [(1_u64 << 39) + 3, (1 << 39) + 4, (1 << 39) + 5];
    let tree = [
        (0_u64, None, 0_u32, root_prefix, true),
        (1, Some(0), 1, root_prefix.map(|p| p << 1), true),
        (2, Some(0), 1, root_prefix.map(|p| (p << 1) | 1), false),
        (3, Some(1), 2, root_prefix.map(|p| p << 2), false),
    ];
    let origin = root_prefix.map(|p| viewer_format::cell_origin_quanta(root_level, p));
    let cells = tree
        .iter()
        .map(|(id, _, depth, prefix, has_children)| {
            let level = (root_level + depth) as i32;
            let spacing = if *has_children {
                2.0_f64.powi(41 - level)
            } else {
                2.0_f64.powi((49 - level).min(-5))
            };
            (NodeId::new(*id), NodeCell::new(*prefix, spacing))
        })
        .collect();
    let frame = GlobalFrame::new(
        root_level,
        root_prefix,
        origin,
        origin.map(|q| q + 100 * 32_768),
        CrsRecord::new(
            "EPSG".to_owned(),
            32632,
            viewer_format::CrsRaw::Wkt(
                "PROJCS[\"WGS 84 / UTM zone 32N\",AUTHORITY[\"EPSG\",\"32632\"]]".into(),
            ),
        )
        .unwrap(),
        None,
        cells,
    )
    .unwrap();
    let mut spatial = Vec::new();
    let mut blocks = Vec::new();
    for (id, parent, depth, prefix, _) in &tree {
        let bounds = frame.node_bounds(*depth, *prefix).unwrap();
        spatial.push(
            SpatialNode::new(
                NodeId::new(*id),
                parent.map(NodeId::new),
                *depth,
                bounds,
                2,
                0.0,
                vec![BlockId::new(id + 1)],
            )
            .unwrap(),
        );
        let at = |fraction: f64| {
            std::array::from_fn(|axis| {
                bounds.min()[axis] + (bounds.max()[axis] - bounds.min()[axis]) * fraction
            })
        };
        blocks.push(SourcePointBlock::new(
            BlockId::new(id + 1),
            vec![at(0.25), at(0.75)],
            vec![ColorRgb::new(1, 2, 3); 2],
            Vec::new(),
        ));
    }
    let hierarchy = SpatialHierarchy::new(spatial, Vec::new(), Some(8)).unwrap();
    let metadata = FormatMetadata::new(
        viewer_core::DatasetMetadata::new(
            viewer_core::CoordinateFrame::new(frame.frame_origin()).unwrap(),
            hierarchy,
        ),
        None,
        None,
        Vec::new(),
    )
    .unwrap()
    .with_global_frame(frame)
    .unwrap();
    let options = FormatWriterOptions::new(0.01, 1 << 24)
        .unwrap()
        .with_page_target_bytes(1);
    let mut writer = FormatWriter::new_split_for_version(
        FormatVersion::V7_PRE5,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata,
        options,
    )
    .unwrap();
    for block in blocks {
        writer.write_block(block).unwrap();
    }
    let (primary, _attributes) = writer.finish_pair().unwrap();
    let bytes = primary.into_inner();

    let mut paged = PagedReader::open(Cursor::new(bytes.clone())).unwrap();
    let mut has_children = HashMap::new();
    let mut pages = vec![paged.root_page().clone()];
    let mut next = 0;
    while next < pages.len() {
        let page = pages[next].clone();
        next += 1;
        for node in page.nodes() {
            has_children.insert(node.id(), node.has_children());
        }
        for (anchor, pointer) in page.child_pages() {
            pages.push(paged.read_child_page(anchor, pointer).unwrap());
        }
    }
    assert_eq!(has_children.len(), 4);
    assert!(pages.len() > 1, "the fixture must span several pages");

    // Lazy open: only the root page is resident and its anchors are unloaded.
    let mut reader =
        FormatReader::open_lazy(Cursor::new(bytes), FormatReaderLimits::default()).unwrap();
    assert!(reader.format_version().uses_global_frame());
    let source = reader.page_source().expect("paged").clone();
    let mut dataset = LodDataset::from_shared_metadata(
        reader.format_metadata().viewer_dataset().unwrap(),
        1 << 20,
    )
    .with_global_frame(reader.format_version().uses_global_frame())
    .with_pages(source.root_unloaded(), Box::new(|_| {}), 1 << 20)
    .unwrap();
    assert!(dataset.supports_cells());

    // Compares the production rule with the file's own flag for every resident node.
    let check = |dataset: &LodDataset| {
        let hierarchy = dataset.metadata.hierarchy();
        let mut checked = 0;
        for (id, children) in &has_children {
            if hierarchy.node(*id).is_none() {
                continue;
            }
            assert_eq!(cell_leaf(hierarchy, *id), !children, "node {id:?}");
            checked += 1;
        }
        checked
    };
    let root_page_nodes = check(&dataset);
    assert!(root_page_nodes >= 1);
    // At least one root-page node is an anchor: `is_leaf` alone would call it a leaf.
    let hierarchy = dataset.metadata.hierarchy();
    assert!(has_children.iter().any(|(id, children)| {
        *children && hierarchy.is_leaf(*id) && hierarchy.is_unloaded(*id)
    }));

    // Graft every child page: the nodes of those pages are checked too.
    let mut anchors: Vec<NodeId> = source.root_unloaded().to_vec();
    let mut number = 0;
    while let Some(anchor) = anchors.pop() {
        let graft = reader.read_page(anchor).unwrap();
        anchors.extend(graft.unloaded.iter().copied());
        number += 1;
        dataset
            .graft_page(crate::PageInsert {
                anchor,
                number: graft.number,
                nodes: graft.nodes,
                unloaded: graft.unloaded,
                bytes: graft.encoded_len,
            })
            .unwrap();
    }
    assert!(number >= 1, "the fixture must have child pages");
    assert_eq!(check(&dataset), has_children.len());
}

/// Counts the blocks owned by more than one node in the v7.5 files of `target/render-data`
/// (`cargo test -p viewer_wasm --offline -- --ignored multi_owner --nocapture`).
#[test]
#[ignore = "needs the datasets of target/render-data"]
fn multi_owner_blocks_in_the_v7_datasets() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/render-data");
    for name in ["majadas-v7.oct", "belvedere-v7.oct"] {
        let path = dir.join(name);
        let Ok(file) = std::fs::File::open(&path) else {
            eprintln!("SKIP {name}: {} is missing", path.display());
            continue;
        };
        let mut paged = PagedReader::open(std::io::BufReader::new(file)).unwrap();
        let mut owners: HashMap<viewer_core::BlockId, usize> = HashMap::new();
        let mut pages = vec![paged.root_page().clone()];
        let mut next = 0;
        while next < pages.len() {
            let page = pages[next].clone();
            next += 1;
            for node in page.nodes() {
                for block in node.blocks() {
                    *owners.entry(block.id()).or_default() += 1;
                }
            }
            for (anchor, pointer) in page.child_pages() {
                pages.push(paged.read_child_page(anchor, pointer).unwrap());
            }
        }
        let shared = owners.values().filter(|count| **count > 1).count();
        eprintln!(
            "OWNERS {name}: {} blocks, {shared} with more than one owner, {} pages",
            owners.len(),
            pages.len()
        );
    }
}

#[test]
fn the_async_read_order_is_stable_while_nothing_arrives() {
    // The host reads the blocks of the order in flight and cancels the ones that leave it, so an
    // order that flips between updates (root chain, then the selection's blocks, then the chain
    // again) cancels every read before its bytes arrive.
    let mut dataset = cell_dataset();
    let view = view_at(None);
    let mut orders = Vec::new();
    for _ in 0..4 {
        let mut order = Vec::new();
        dataset
            .plan_cell_update(
                &view,
                64.0,
                CellUpdateConfig::default(),
                Some(&mut order),
                |_| Ok(None),
            )
            .unwrap();
        orders.push(order);
    }
    // Both the root chain and the selection are in flight, the root's blocks first (R3-Q22).
    let root_blocks = dataset
        .metadata
        .hierarchy()
        .node(NodeId::new(1))
        .unwrap()
        .blocks()
        .to_vec();
    assert!(!root_blocks.is_empty());
    assert!(orders[0].len() > root_blocks.len(), "{:?}", orders[0]);
    assert!(
        orders[0][..root_blocks.len()]
            .iter()
            .all(|block| root_blocks.contains(block)),
        "{:?}",
        orders[0]
    );
    for order in &orders {
        assert_eq!(order, &orders[0]);
    }
}

// ---------------------------------------------------------------------------------------------
// Levels built on demand (R3: the render side asks, the build side serves)
// ---------------------------------------------------------------------------------------------

/// A dataset whose cut nodes are all built (their cells taken from the updates, as a driver
/// would), with every block cached, and the packed nodes as they were sent.
fn built_for_levels() -> (LodDataset, HashMap<NodeId, PackedNode>, ViewProjection) {
    let mut dataset = cell_dataset();
    load_all(&mut dataset);
    let view = view_at(None);
    let mut sent = HashMap::new();
    for _ in 0..20 {
        let update = dataset
            .plan_cell_update(&view, 64.0, CellUpdateConfig::default(), None, |_| Ok(None))
            .unwrap();
        let done = update.new_nodes.is_empty();
        for (_, node) in update.new_nodes {
            sent.insert(node.id, node);
        }
        if done {
            return (dataset, sent, view);
        }
    }
    panic!("the cells did not settle");
}

/// `count` depths a leaf can build but did not ship, finest last.
fn unsent_depths(node: &PackedNode, count: usize) -> Vec<u8> {
    let stored: Vec<u8> = node.levels.iter().map(|level| level.depth).collect();
    (node.buildable.0..=node.buildable.1)
        .filter(|depth| !stored.contains(depth))
        .take(count)
        .collect()
}

fn plan_levels(
    dataset: &mut LodDataset,
    view: &ViewProjection,
    config: CellUpdateConfig,
) -> CellUpdate {
    dataset
        .plan_cell_update(view, 64.0, config, None, |_| Ok(None))
        .unwrap()
}

/// Updates until nothing is built any more; every packed node and level sent on the way.
fn run_levels(
    dataset: &mut LodDataset,
    view: &ViewProjection,
) -> (Vec<PackedNode>, Vec<(NodeId, u8)>) {
    let (mut nodes, mut levels) = (Vec::new(), Vec::new());
    for _ in 0..20 {
        let update = plan_levels(dataset, view, CellUpdateConfig::default());
        levels.extend(shipped_depths(&update));
        let done = update.new_nodes.is_empty();
        nodes.extend(update.new_nodes.into_iter().map(|(_, node)| node));
        if done {
            break;
        }
    }
    (nodes, levels)
}

fn shipped_depths(update: &CellUpdate) -> Vec<(NodeId, u8)> {
    update
        .new_levels
        .iter()
        .flat_map(|(_, node)| node.levels.iter().map(|level| (node.id, level.depth)))
        .collect()
}

#[test]
fn a_requested_level_is_built_and_shipped() {
    let (mut dataset, sent, view) = built_for_levels();
    let leaf = NodeId::new(3);
    let depth = unsent_depths(&sent[&leaf], 1)[0];
    dataset.apply_level_requests(&[(leaf, depth)]);
    let update = plan_levels(&mut dataset, &view, CellUpdateConfig::default());
    assert_eq!(shipped_depths(&update), [(leaf, depth)]);
    assert!(update.new_nodes.is_empty());
    // Served: the next update ships nothing.
    let next = plan_levels(&mut dataset, &view, CellUpdateConfig::default());
    assert!(next.new_levels.is_empty());
}

#[test]
fn a_newer_request_set_replaces_the_pending_one() {
    let (mut dataset, sent, view) = built_for_levels();
    let (a, b) = (NodeId::new(3), NodeId::new(4));
    let depth_a = unsent_depths(&sent[&a], 1)[0];
    let depth_b = unsent_depths(&sent[&b], 1)[0];
    dataset.apply_level_requests(&[(a, depth_a)]);
    dataset.apply_level_requests(&[(b, depth_b)]);
    let update = plan_levels(&mut dataset, &view, CellUpdateConfig::default());
    assert_eq!(shipped_depths(&update), [(b, depth_b)]);
}

#[test]
fn an_uncached_request_schedules_its_blocks_and_is_served_later() {
    let (mut dataset, sent, view) = built_for_levels();
    let leaf = NodeId::new(3);
    let depth = unsent_depths(&sent[&leaf], 1)[0];
    let block = BlockId::new(103);
    dataset.cache.remove(block);
    dataset.apply_level_requests(&[(leaf, depth)]);
    let update = plan_levels(&mut dataset, &view, CellUpdateConfig::default());
    assert!(update.new_levels.is_empty());
    assert!(
        update.stats.pending_block_ids.contains(&block),
        "the blocks of the requested node are not listed to read"
    );
    dataset.insert_block(node_block(&dataset, 3)).unwrap();
    let update = plan_levels(&mut dataset, &view, CellUpdateConfig::default());
    assert_eq!(shipped_depths(&update), [(leaf, depth)]);
}

#[test]
fn level_builds_respect_their_own_budget() {
    let (mut dataset, sent, view) = built_for_levels();
    let requests: Vec<(NodeId, u8)> = (3..=9)
        .map(NodeId::new)
        .flat_map(|id| {
            unsent_depths(&sent[&id], 2)
                .into_iter()
                .map(move |depth| (id, depth))
        })
        .take(12)
        .collect();
    assert_eq!(requests.len(), 12);
    dataset.apply_level_requests(&requests);
    // Node builds are capped to zero: the level budget is a separate one.
    let config = CellUpdateConfig {
        max_node_builds: 0,
        ..CellUpdateConfig::default()
    };
    let first = plan_levels(&mut dataset, &view, config);
    assert_eq!(shipped_depths(&first).len(), 8);
    assert!(first.stats.build_deferred >= 4);
    let second = plan_levels(&mut dataset, &view, config);
    assert_eq!(shipped_depths(&second).len(), 4);
    let mut all = shipped_depths(&first);
    all.extend(shipped_depths(&second));
    all.sort();
    let mut expected = requests;
    expected.sort();
    assert_eq!(all, expected);
}

#[test]
fn pending_requests_survive_a_colour_change() {
    let (mut dataset, sent, view) = built_for_levels();
    let leaf = NodeId::new(3);
    let depth = unsent_depths(&sent[&leaf], 1)[0];
    dataset.apply_level_requests(&[(leaf, depth)]);
    assert!(dataset.has_pending_level_requests(true));
    assert!(dataset.set_cell_paint(Some((0, 0))));
    assert!(dataset.has_pending_level_requests(true));
    // The node is rebuilt with the new colours and the pending depth is built after it, with
    // them, without the render side asking again.
    load_all(&mut dataset);
    let (nodes, levels) = run_levels(&mut dataset, &view);
    assert!(nodes.iter().any(|node| node.id == leaf), "not rebuilt");
    assert!(
        levels.contains(&(leaf, depth)),
        "the pending depth was lost"
    );
    assert!(!dataset.has_pending_level_requests(true));
}

#[test]
fn a_rebuilt_node_keeps_its_on_demand_depths_with_the_new_colours() {
    let (mut dataset, sent, view) = built_for_levels();
    let leaf = NodeId::new(3);
    let depth = unsent_depths(&sent[&leaf], 1)[0];
    dataset.apply_level_requests(&[(leaf, depth)]);
    let update = plan_levels(&mut dataset, &view, CellUpdateConfig::default());
    assert_eq!(shipped_depths(&update), [(leaf, depth)]);
    assert!(dataset.set_cell_paint(Some((0, 0))));
    load_all(&mut dataset);
    let (nodes, _) = run_levels(&mut dataset, &view);
    let rebuilt = nodes
        .iter()
        .find(|node| node.id == leaf)
        .expect("the node was not rebuilt");
    assert!(rebuilt.levels.iter().any(|level| level.depth == depth));
}

#[test]
fn an_unservable_depth_is_not_retried() {
    let (mut dataset, sent, view) = built_for_levels();
    let leaf = NodeId::new(3);
    // Past the last depth the node can build: the range check refuses it.
    let depth = sent[&leaf].buildable.1 + 1;
    dataset.apply_level_requests(&[(leaf, depth)]);
    let first = plan_levels(&mut dataset, &view, CellUpdateConfig::default());
    assert!(first.new_levels.is_empty());
    assert!(!dataset.has_pending_level_requests(true));
    // Asked again by the render side: no rebuild, nothing shipped.
    dataset.apply_level_requests(&[(leaf, depth)]);
    assert!(!dataset.has_pending_level_requests(true));
    let again = plan_levels(&mut dataset, &view, CellUpdateConfig::default());
    assert!(again.new_levels.is_empty());
    assert!(again.new_nodes.is_empty());
}

#[test]
fn a_depth_still_merged_at_the_deepest_level_is_requested_once() {
    let (mut dataset, sent, view) = built_for_levels();
    let leaf = NodeId::new(3);
    // In range (`buildable.1 == MAX_CELL_DEPTH` for a leaf), but two distinct positions share a
    // cell there: `node_cell_level` fails with `AbsoluteDepthExceeded`.
    let depth = sent[&leaf].buildable.1;
    assert_eq!(depth, viewer_render_wgpu::MAX_CELL_DEPTH);
    // Two f32 positions closer than a depth-63 cell (7e-18 of the 64-wide root), next to the
    // origin where f32 resolves them (the block need not lie in the node's bounds for this).
    let block = BlockId::new(103);
    let at = |value: f32| [value; 3];
    let merged = PointBlock::new(
        block,
        PointBatch::new(vec![
            Point::new(at(1.0e-19), ColorRgb::new(1, 2, 3)),
            Point::new(at(2.0e-19), ColorRgb::new(4, 5, 6)),
        ]),
        Vec::new(),
    );
    dataset.cache.remove(block);
    dataset.insert_block(merged).unwrap();
    dataset.apply_level_requests(&[(leaf, depth)]);
    assert!(dataset.has_pending_level_requests(true));
    let first = plan_levels(&mut dataset, &view, CellUpdateConfig::default());
    assert!(first.new_levels.is_empty());
    assert!(!dataset.has_pending_level_requests(true));
    // Asked again, and after more updates: remembered, not rebuilt, nothing pending or shipped.
    dataset.apply_level_requests(&[(leaf, depth)]);
    assert!(!dataset.has_pending_level_requests(true));
    for _ in 0..3 {
        let again = plan_levels(&mut dataset, &view, CellUpdateConfig::default());
        assert!(again.new_levels.is_empty() && again.new_nodes.is_empty());
    }
}

#[test]
fn pending_levels_with_uncached_blocks_need_a_reader_to_be_work() {
    let (mut dataset, sent, _) = built_for_levels();
    let leaf = NodeId::new(3);
    let depth = unsent_depths(&sent[&leaf], 1)[0];
    dataset.cache.remove(BlockId::new(103));
    dataset.apply_level_requests(&[(leaf, depth)]);
    assert!(dataset.has_pending_level_requests(true));
    assert!(!dataset.has_pending_level_requests(false));
}

#[test]
fn cut_levels_are_built_under_a_tight_budget_and_count_toward_it() {
    let (mut dataset, sent, view) = built_for_levels();
    let leaf = NodeId::new(3);
    let depth = unsent_depths(&sent[&leaf], 1)[0];
    let tight = CellUpdateConfig {
        cpu_budget_bytes: Some(1),
        ..CellUpdateConfig::default()
    };
    let before = plan_levels(&mut dataset, &view, tight);
    assert!(before.new_levels.is_empty());
    let needed = before.stats.diag.cpu_needed_bytes;
    assert!(needed > 1, "the budget is not exceeded");
    dataset.apply_level_requests(&[(leaf, depth)]);
    let update = plan_levels(&mut dataset, &view, tight);
    assert_eq!(shipped_depths(&update), [(leaf, depth)]);
    let bytes: u64 = update
        .new_levels
        .iter()
        .map(|(_, node)| viewer_render_wgpu::packed_node_bytes(node))
        .sum();
    assert_eq!(update.stats.diag.cpu_needed_bytes, needed + bytes);
}

#[test]
fn a_held_non_cut_request_does_not_keep_the_worker_dirty() {
    let (mut dataset, sent, view) = built_for_levels();
    let tight = CellUpdateConfig {
        cpu_budget_bytes: Some(1),
        ..CellUpdateConfig::default()
    };
    plan_levels(&mut dataset, &view, tight);
    // A built node outside the cut: the root, which the cut's nodes replace (if it were in the
    // cut it would be built below and the test would fail).
    let outside = NodeId::new(1);
    let depth = unsent_depths(&sent[&outside], 1)[0];
    dataset.apply_level_requests(&[(outside, depth)]);
    for _ in 0..3 {
        let update = plan_levels(&mut dataset, &view, tight);
        assert!(update.new_levels.is_empty(), "built over the budget");
        assert!(!dataset.has_pending_level_requests(true), "held but dirty");
    }
}

#[test]
fn an_exempt_cut_request_is_retried_while_blocked() {
    let (mut dataset, sent, view) = built_for_levels();
    let tight = CellUpdateConfig {
        cpu_budget_bytes: Some(1),
        max_level_builds: 1,
        ..CellUpdateConfig::default()
    };
    let requests: Vec<(NodeId, u8)> = [NodeId::new(3), NodeId::new(4)]
        .into_iter()
        .map(|id| (id, unsent_depths(&sent[&id], 1)[0]))
        .collect();
    plan_levels(&mut dataset, &view, tight);
    dataset.apply_level_requests(&requests);
    let first = plan_levels(&mut dataset, &view, tight);
    assert_eq!(shipped_depths(&first).len(), 1);
    // Blocked, one cut level left by the build cap: another update serves it.
    assert!(dataset.has_pending_level_requests(true));
    let second = plan_levels(&mut dataset, &view, tight);
    assert_eq!(shipped_depths(&second).len(), 1);
    assert!(!dataset.has_pending_level_requests(true));
}
