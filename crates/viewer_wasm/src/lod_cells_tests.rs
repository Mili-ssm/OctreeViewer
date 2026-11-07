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

#[test]
fn at_most_the_configured_nodes_are_built_per_update_nearest_first() {
    let mut dataset = cell_dataset();
    load_all(&mut dataset);
    let mut driver = driver_for(&dataset, u64::MAX);
    // The eye hangs right beside node 9, the octant at the far corner.
    let view = view_at(Some([70.0, 68.0, 66.0]));
    let mut target = NullTarget::at(&view);
    let config = CellUpdateConfig { max_node_builds: 2 };
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
fn multi_owner_blocks_in_the_v75_datasets() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/render-data");
    for name in ["majadas-v75.oct", "belvedere-v75.oct"] {
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
