//! Cells mode of `LodDataset` (R3 Task 9b): the planner sees a node as resident only once its cells
//! exist, a few nodes are built per update from their decoded blocks, and the cut goes to a
//! caller-owned `CellDriver`. The dataset holds no renderer: the caller passes the driver and the
//! `CellTarget` for the duration of one update.

use std::collections::HashSet;

use super::{
    LodDataset, LodDatasetError, LodTransitionPlan, NodeId, PointBlock, WorkClock,
    pending_page_anchors,
};
use viewer_core::{BlockId, LodSelection, ResidentBlockSource, SpatialBounds, SpatialHierarchy};
use viewer_render_wgpu::{
    CellDriver, CellDriverError, CellGrid, CellTarget, CutApply, FrameReport, NodeAdded,
    NodeCellSource, NodeInput, PointNodeCellSource, PoolCounters, ViewProjection,
    representative_depth,
};

/// Deepest ancestor-descendant gap, in geometric cell depths (`representative_depth`), a node
/// mask can express (`MAX_MASK_LEVELS` of the cut).
pub const MAX_CELL_CUT_GAP: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellUpdateConfig {
    /// Nodes whose cells are built per update, nearest to the eye first.
    pub max_node_builds: usize,
}

impl Default for CellUpdateConfig {
    fn default() -> Self {
        Self { max_node_builds: 2 }
    }
}

/// What one cells update did.
#[derive(Debug, Clone, PartialEq)]
pub struct CellUpdateStats {
    /// Nodes whose cells were built, in build order.
    pub built_nodes: Vec<NodeId>,
    /// Nodes still waiting for their cells after the cap was reached.
    pub build_deferred: usize,
    pub build_capped: bool,
    pub build_ms: f64,
    /// Nodes of the cut given to the driver (plan nodes with cells).
    pub cut_nodes: usize,
    /// Largest depth difference between a cut node and its nearest cut ancestor.
    pub max_gap: u32,
    /// Cut nodes whose gap is above 1.
    pub gaps_over_one: usize,
    /// The driver's real cut failed with `DescendantTooFine` (it retains the previous one).
    pub descendant_too_fine: bool,
    pub apply: CutApply,
    pub counters: PoolCounters,
    pub cpu_cell_bytes: u64,
    pub pending_block_ids: Vec<BlockId>,
    pub pending_page_anchors: Vec<NodeId>,
    pub uncovered_nodes: usize,
}

pub struct LodCellUpdate {
    pub stats: CellUpdateStats,
    pub report: FrameReport,
}

#[derive(Default)]
pub(super) struct CellState {
    /// Nodes whose cells are stored in the driver.
    built: HashSet<NodeId>,
    /// Nodes whose every level came out empty: resident for the plan, never in the cut. That
    /// leaves no hole: a node has no levels only when none of its points is finite (a finite
    /// point always lands in a cell), and such points draw nothing on any path.
    pub(super) empty: HashSet<NodeId>,
    /// Nodes of the cut the pool draws (applied or retained) after the last update. Replaced on
    /// every successful frame; cleared when a frame fails (the target may have dropped its
    /// pool) and by `pool_was_reset`. A gap error comes before the frame and changes nothing.
    pub(super) pool_drawn: Vec<NodeId>,
}

impl CellState {
    pub(super) fn pool_drawn(&self) -> &[NodeId] {
        &self.pool_drawn
    }

    pub(super) fn has_cells(&self, node: NodeId) -> bool {
        self.built.contains(&node) || self.empty.contains(&node)
    }
}

/// The block cache as the planner sees it in cells mode: a block is resident only when every node
/// owning it has its cells, so the planner covers unbuilt nodes with their ancestors.
struct CellReadySource<'a> {
    cache: &'a viewer_core::DecodedBlockCache,
    ready_blocks: &'a HashSet<BlockId>,
}

impl ResidentBlockSource for CellReadySource<'_> {
    fn peek(&self, id: BlockId) -> Option<&PointBlock> {
        if self.ready_blocks.contains(&id) {
            self.cache.peek(id)
        } else {
            None
        }
    }
}

impl LodDataset {
    /// Whether cells can be built for this dataset: global-frame formats with replacement
    /// semantics (R3-Q5: the others keep the CPU path).
    pub fn supports_cells(&self) -> bool {
        self.global_frame
            && self.metadata.hierarchy().semantics() == viewer_core::HierarchySemantics::Replacement
    }

    /// The cell grid in the dataset frame, for `DriverConfig::grid`.
    pub fn cell_grid(&self) -> CellGrid {
        let root = self.metadata.hierarchy().stats().root_bounds;
        CellGrid {
            root_min: root.min(),
            root_side: (0..3)
                .map(|axis| root.max()[axis] - root.min()[axis])
                .fold(0.0, f64::max),
        }
    }

    /// The dataset's `coordinate_origin`, for `DriverConfig::coordinate_origin`.
    pub fn coordinate_origin(&self) -> [f64; 3] {
        self.metadata.coordinate_frame().origin()
    }

    /// Blocks whose owners all have cells.
    fn cell_ready_blocks(&self) -> HashSet<BlockId> {
        let hierarchy = self.metadata.hierarchy();
        self.cells
            .built
            .iter()
            .chain(&self.cells.empty)
            .filter_map(|id| hierarchy.node(*id))
            .flat_map(|node| node.blocks().iter().copied())
            .filter(|block| {
                self.block_owners
                    .get(block)
                    .is_none_or(|owners| owners.iter().all(|owner| self.cells.has_cells(*owner)))
            })
            .collect()
    }

    /// Owners without cells of the blocks of selected nodes that have them. Limited to the
    /// selection so the closure cannot spread across the hierarchy away from the view.
    fn shared_block_waiters(&self, selection: &[NodeId]) -> Vec<NodeId> {
        let hierarchy = self.metadata.hierarchy();
        selection
            .iter()
            .filter(|id| self.cells.has_cells(**id))
            .filter_map(|id| hierarchy.node(*id))
            .flat_map(|node| node.blocks())
            .filter_map(|block| self.block_owners.get(block))
            .flatten()
            .copied()
            .filter(|owner| !self.cells.has_cells(*owner))
            .collect()
    }

    pub(super) fn prepare_cell_plan(&mut self, target: &LodSelection) -> LodTransitionPlan {
        self.refinement_work.prepares += 1;
        let planning_started = WorkClock::start();
        let ready_blocks = self.cell_ready_blocks();
        let source = CellReadySource {
            cache: &self.cache,
            ready_blocks: &ready_blocks,
        };
        let plan = self
            .planner
            .prepare(self.metadata.hierarchy(), target, &source);
        self.refinement_work.planning_ms += planning_started.elapsed_ms();
        plan
    }

    /// Whether the cut selected at `target_px` is complete with only built nodes resident: the
    /// ladder may start at the target stage.
    #[cfg(any(target_arch = "wasm32", test))]
    #[cfg_attr(not(test), allow(dead_code))] // only the ladder in `lod_driver` calls it, and that is test-only for now
    pub(crate) fn target_cells_are_ready(
        &mut self,
        view: &ViewProjection,
        target_px: f64,
    ) -> Result<bool, LodDatasetError> {
        let (_, selection) = self.select_target(view, target_px)?;
        let plan = self.prepare_cell_plan(&selection);
        Ok(super::resident_target_plan_is_complete(&selection, &plan))
    }

    /// Tells the dataset the caller reset the driver (`CellDriver::reset`): the pool draws
    /// nothing until the next update, so no node is pinned for it.
    pub fn pool_was_reset(&mut self) {
        self.cells.pool_drawn.clear();
    }

    /// Distance from the camera to the node, in the camera's scaled local units.
    pub(super) fn distance_to_eye(&self, view: &ViewProjection, bounds: SpatialBounds) -> f64 {
        let origin = self.coordinate_origin();
        let eye = view.camera().position();
        let scale = view.visual_scale();
        (0..3)
            .map(|axis| {
                let min = (bounds.min()[axis] - origin[axis]) * scale;
                let max = (bounds.max()[axis] - origin[axis]) * scale;
                let d = eye[axis].clamp(min, max) - eye[axis];
                d * d
            })
            .sum::<f64>()
            .sqrt()
    }

    /// Builds the cells of `id` from its decoded blocks and hands them to the driver.
    pub(super) fn build_node_cells(
        &mut self,
        driver: &mut CellDriver,
        id: NodeId,
    ) -> Result<(), LodDatasetError> {
        let hierarchy = self.metadata.hierarchy();
        let Some(node) = hierarchy.node(id) else {
            return Ok(());
        };
        let blocks: Option<Vec<PointBlock>> = node
            .blocks()
            .iter()
            .map(|block| self.cache.peek(*block).cloned())
            .collect();
        let Some(blocks) = blocks else {
            return Ok(());
        };
        let origin = self.coordinate_origin();
        let world = self.cell_grid();
        let local_grid = CellGrid {
            root_min: std::array::from_fn(|axis| world.root_min[axis] - origin[axis]),
            root_side: world.root_side,
        };
        let local = SpatialBounds::new(
            std::array::from_fn(|axis| node.bounds().min()[axis] - origin[axis]),
            std::array::from_fn(|axis| node.bounds().max()[axis] - origin[axis]),
        )
        .map_err(|error| LodDatasetError::CellDriver(error.to_string()))?;
        let is_leaf = cell_leaf(hierarchy, id);
        let cells = PointNodeCellSource::default().node_cells(
            &local_grid,
            id,
            local,
            node.spacing(),
            is_leaf,
            &blocks,
        )?;
        let added = driver
            .add_node(NodeInput {
                id,
                bounds: node.bounds(),
                points: node.point_count(),
                local_grid,
                cells: &cells,
            })
            .map_err(|error| LodDatasetError::CellDriver(error.to_string()))?;
        match added {
            NodeAdded::Stored { .. } => self.cells.built.insert(id),
            NodeAdded::NoLevels => {
                debug_assert!(
                    blocks.iter().all(|block| block
                        .points()
                        .points()
                        .iter()
                        .all(|point| point.position.iter().any(|value| !value.is_finite()))),
                    "node {id:?} has no cell levels but has a finite point"
                );
                self.cells.empty.insert(id)
            }
        };
        Ok(())
    }

    /// Largest and over-one depth gaps between each cut node and its nearest cut ancestor. The
    /// gap is the difference of `representative_depth` on the world grid, which is what the node
    /// mask of the cut limits (not the hierarchy level).
    pub(super) fn cut_gaps(&self, cut: &[NodeId]) -> Result<(u32, usize), LodDatasetError> {
        let hierarchy = self.metadata.hierarchy();
        let grid = self.cell_grid();
        let depth =
            |node: &viewer_core::SpatialNode| u32::from(representative_depth(&grid, node.bounds()));
        let in_cut: HashSet<NodeId> = cut.iter().copied().collect();
        let (mut max_gap, mut over_one) = (0, 0);
        for id in cut {
            let Some(node) = hierarchy.node(*id) else {
                continue;
            };
            let mut parent = node.parent();
            while let Some(ancestor_id) = parent {
                let Some(ancestor) = hierarchy.node(ancestor_id) else {
                    break;
                };
                if in_cut.contains(&ancestor_id) {
                    let gap = depth(node).saturating_sub(depth(ancestor));
                    if gap > MAX_CELL_CUT_GAP {
                        return Err(LodDatasetError::CellCutGap {
                            ancestor: ancestor_id,
                            descendant: *id,
                            gap,
                            max: MAX_CELL_CUT_GAP,
                        });
                    }
                    max_gap = max_gap.max(gap);
                    over_one += usize::from(gap > 1);
                    break;
                }
                parent = ancestor.parent();
            }
        }
        Ok((max_gap, over_one))
    }

    /// One update of the cells path: select at `target_px`, read at most one block, build the
    /// cells of up to `config.max_node_builds` target nodes (nearest first), plan with only
    /// built nodes resident and run one driver frame on `target`.
    ///
    /// The driver and the target are the caller's; nodes the driver forgets are built again when
    /// the cut needs them. On an error the update stops where it failed: blocks read and cells
    /// built so far stay (cache, driver and `built` are updated as they happen), but the plan is
    /// not committed, `visible_plan` keeps the previous one and the pool pins change only as
    /// `pool_drawn` documents.
    #[allow(clippy::too_many_arguments)]
    pub fn update_cells<T: CellTarget>(
        &mut self,
        view: &ViewProjection,
        target_px: f64,
        config: CellUpdateConfig,
        driver: &mut CellDriver,
        target: &mut T,
        async_priority_order: Option<&mut Vec<BlockId>>,
        load_block: impl FnMut(BlockId) -> Result<Option<PointBlock>, String>,
    ) -> Result<LodCellUpdate, LodDatasetError> {
        self.update_cells_at_stage(
            view,
            target_px,
            target_px,
            config,
            driver,
            target,
            async_priority_order,
            load_block,
        )
    }

    /// [`LodDataset::update_cells`] selecting the nodes at `stage_px` (a rung of the px ladder,
    /// see `lod_driver::update_cells_staged`) while the driver still chooses cell levels for
    /// `target_px`. Coarser nodes get their cells first and stay as the fallback of the finer
    /// ones (R3-Q19).
    ///
    /// The caller owns the ladder: when the camera moves it resets `stage_pixels = None` and sets
    /// `dirty = true`; otherwise the nodes of a new region load at a fine stage without their
    /// parents' cells as fallback (see `lod_driver::update_cells_staged`).
    #[allow(clippy::too_many_arguments)]
    pub fn update_cells_at_stage<T: CellTarget>(
        &mut self,
        view: &ViewProjection,
        stage_px: f64,
        target_px: f64,
        config: CellUpdateConfig,
        driver: &mut CellDriver,
        target: &mut T,
        async_priority_order: Option<&mut Vec<BlockId>>,
        mut load_block: impl FnMut(BlockId) -> Result<Option<PointBlock>, String>,
    ) -> Result<LodCellUpdate, LodDatasetError> {
        if !self.supports_cells() {
            return Err(LodDatasetError::CellPathUnsupported);
        }
        self.refinement_work.update_calls += 1;
        // The driver is the truth about stored cells (it forgets, a caller may swap it).
        self.cells.built.retain(|id| driver.has_node(*id));
        let coordinate_origin = self.coordinate_origin();
        let (frustum, selection) = self.select_target(view, stage_px)?;

        let initial_plan = self.prepare_cell_plan(&selection);
        // Blocks that are decoded but wait for their cells are not read again.
        let to_read: Vec<BlockId> = initial_plan
            .pending_blocks()
            .iter()
            .copied()
            .filter(|block| !self.cache.contains(*block))
            .collect();
        let next_block = self.next_block_to_load(
            view,
            &frustum,
            &to_read,
            initial_plan.uncovered_nodes(),
            async_priority_order,
        );
        let mut evicted_blocks = std::mem::take(&mut self.arrival_evictions);
        self.arrived_blocks = 0;
        if let Some(requested) = next_block {
            self.load_block_step(requested, &mut load_block, &mut evicted_blocks)?;
        }

        // Nodes with all their blocks decoded and no cells yet, nearest first: the target nodes
        // and the co-owners of a block shared with a node that already has cells (a block turns
        // resident only when all its owners have cells, so they would wait forever otherwise).
        let waiters = self.shared_block_waiters(selection.node_ids());
        let hierarchy = self.metadata.hierarchy();
        let mut seen = HashSet::new();
        // (waiter, distance, id): the selected nodes take the build slots before the waiters.
        let mut candidates: Vec<(bool, f64, NodeId)> = Vec::new();
        let selected = selection.node_ids().len();
        for (index, id) in selection.node_ids().iter().chain(&waiters).enumerate() {
            if !seen.insert(*id) || self.cells.has_cells(*id) {
                continue;
            }
            if driver.has_node(*id) {
                self.cells.built.insert(*id);
                continue;
            }
            let Some(node) = hierarchy.node(*id) else {
                continue;
            };
            if !node.blocks().is_empty() && node.blocks().iter().all(|b| self.cache.contains(*b)) {
                candidates.push((
                    index >= selected,
                    self.distance_to_eye(view, node.bounds()),
                    *id,
                ));
            }
        }
        candidates.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
        let build_started = WorkClock::start();
        let take = candidates.len().min(config.max_node_builds);
        let mut built_nodes = Vec::with_capacity(take);
        for (_, _, id) in &candidates[..take] {
            self.build_node_cells(driver, *id)?;
            built_nodes.push(*id);
        }
        let build_ms = build_started.elapsed_ms();
        let build_deferred = candidates.len() - take;
        if take > 0 || next_block.is_some() {
            self.state_epoch += 1;
        }

        let plan = self.prepare_cell_plan(&selection);
        let evicted_pending = evicted_blocks
            .iter()
            .filter(|id| plan.pending_blocks().binary_search(id).is_ok())
            .count() as u64;
        self.refinement_work.evicted_pending_blocks += evicted_pending;
        let cut: Vec<NodeId> = plan
            .node_ids()
            .iter()
            .copied()
            .filter(|id| driver.has_node(*id))
            .collect();
        let (max_gap, gaps_over_one) = self.cut_gaps(&cut)?;
        let report = driver
            .frame(target, &cut, view, target_px)
            .map_err(|error| {
                // A failed frame may have emptied the target's pool (or the driver reset it
                // first): keep no pin for what it drew before.
                self.cells.pool_drawn.clear();
                match error {
                    CellDriverError::Cut(error) => LodDatasetError::CellCut(error),
                    other => LodDatasetError::CellDriver(other.to_string()),
                }
            })?;

        self.state_epoch += 1;
        self.planner.commit(&plan, &mut self.cache);
        self.touch_pages(plan.node_ids());
        self.cells.pool_drawn = report.plan.entries.iter().map(|entry| entry.node).collect();
        let pending_anchors = pending_page_anchors(self.metadata.hierarchy(), &selection, |node| {
            view.projected_error_pixels(
                node.bounds(),
                node.representative_spacing(),
                coordinate_origin,
            )
        });
        let stats = CellUpdateStats {
            built_nodes,
            build_deferred,
            build_capped: build_deferred > 0,
            build_ms,
            cut_nodes: cut.len(),
            max_gap,
            gaps_over_one,
            descendant_too_fine: report.counters().descendant_too_fine,
            apply: report.plan.apply,
            counters: *report.counters(),
            cpu_cell_bytes: driver.cpu_bytes(),
            pending_block_ids: plan.pending_blocks().to_vec(),
            pending_page_anchors: pending_anchors,
            uncovered_nodes: plan.uncovered_nodes().len(),
        };
        self.visible_plan = Some(plan);
        Ok(LodCellUpdate { stats, report })
    }
}

/// Whether `id` is a leaf for its cell levels: a page anchor that is not resident has children
/// we cannot see, so it is not a leaf.
pub(super) fn cell_leaf(hierarchy: &SpatialHierarchy, id: NodeId) -> bool {
    hierarchy.is_leaf(id) && !hierarchy.is_unloaded(id)
}
