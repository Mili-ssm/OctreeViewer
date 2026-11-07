#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
//! R3 Task 10b: the per-message logic of the cells path, in plain Rust so native tests drive it.
//!
//! The wasm-bindgen wrappers (`bindings/cells.rs`) only convert between this module's types and
//! `js_sys` values. Three pieces:
//!
//! * [`BuilderCells`]: the build worker. Steps 1 and 3 of an update (`plan_cells_staged`,
//!   `nodeCells` encoding, `poolAck` / `evict` / `poolReset` / `driverReplaced`).
//! * [`ViewerCells`]: the render worker. Step 2: a queue of `nodeCells` applied under a byte cap
//!   per frame, the latest `cellCut`, the driver frame and the messages it sends back.
//! * [`InProcessCells`]: the viewer holding the dataset itself; the three steps run back to back.
//!
//! Colour modes (R3-Q23): cells are RGB only until Task 12. Any other mode switches the side off
//! (`set_active(false)`), the CPU path takes over, and back to RGB `set_active(true)` re-enables
//! the cells: the nodes are kept, only the pool is rebuilt.
//!
//! View epochs: the render side counts its views (`camera_changed(epoch)`, the `viewEpoch` of the
//! JS API); the host hands the epoch of the camera it sends to the build side, which echoes it in
//! the `cellCut` its plan produced. The render side reports a digest at rest only for a cut of
//! the latest view epoch, so a camera move can never publish the digest of the old view's cut.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
};

use viewer_core::{BlockId, NodeId, PointBlock};
use viewer_format::OctreeReader;
use viewer_render_wgpu::{
    CellDriver, CellGrid, CellTarget, DriverConfig, FrameReport, OrbitCamera, PoolConfig,
    PoolCounters, ViewProjection, VoxelBuildLimits, cell_cut_digest, decode_node_levels,
    encode_node_levels,
};

use crate::{
    CellPlanStats, CellUpdateConfig, CutRestProbe, LodDataset, LodDatasetError, PageInsert,
    PoolAck, add_cell_nodes, draw_cell_cut,
    lod_driver::{CellStep, plan_cells_staged, update_cells_staged},
};

/// `cpu_budget_bytes` of the driver in wasm: the stop criterion of T9b.
pub(crate) const WASM_CPU_BUDGET_BYTES: u64 = 512 << 20;

/// Bytes of `nodeCells` applied per frame (at least one node is always applied).
pub(crate) const DEFAULT_NODE_BYTES_PER_FRAME: usize = 4 << 20;

/// Most page-fetch rounds one build update runs before it returns what it has.
const MAX_PAGE_ROUNDS: usize = 32;

/// Same bounds as `WasmVoxelViewer::updateLod`.
pub(crate) const TARGET_PIXELS_RANGE: std::ops::RangeInclusive<f64> = 0.5..=16.0;

/// Bytes of the wasm linear memory (0 outside wasm32).
pub(crate) fn wasm_linear_memory_bytes() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        core::arch::wasm32::memory_size::<0>() as u64 * 65_536
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0
    }
}

pub(crate) fn driver_config(
    grid: CellGrid,
    coordinate_origin: [f64; 3],
    cpu_budget_bytes: u64,
) -> DriverConfig {
    DriverConfig {
        pool: PoolConfig::default(),
        grid,
        coordinate_origin,
        cpu_budget_bytes,
    }
}

/// A full page budget is not an error of the cells path: the cut keeps what is resident and the
/// pages are asked again (the page-fetch rounds are bounded).
fn graft_tolerating_budget(dataset: &mut LodDataset, page: PageInsert) -> Result<(), String> {
    match dataset.graft_page(page) {
        Ok(()) | Err(LodDatasetError::PageBudget { .. }) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// What the stats objects report about the cells path of either side.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CellsStats {
    /// Work is pending: a cut sent and not applied, queued nodes, or driver work.
    pub cells_busy: bool,
    /// The render worker must keep drawing (queued nodes, a pending camera move, driver work).
    pub needs_frame: bool,
    /// Latest view epoch: the render side's own, or the one the build side planned for.
    pub view_epoch: u64,
    /// Epoch of the last cut sent (build side) or received (render side).
    pub cut_epoch_sent: u64,
    /// Epoch of the last cut applied (render side) or acknowledged (build side).
    pub cut_epoch_applied: u64,
    pub queued_nodes: usize,
    pub queued_bytes: usize,
    pub cut_digest_at_rest: Option<u64>,
    pub counters: PoolCounters,
    pub cpu_cell_bytes: u64,
    pub wasm_linear_memory_bytes: u64,
}

// ---------------------------------------------------------------------------------------------
// Build side
// ---------------------------------------------------------------------------------------------

/// A node of a `nodeCells` message: the codec bytes of one node with all its levels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EncodedNode {
    pub bytes: Vec<u8>,
}

/// Step 1 output, ready to be sent.
pub(crate) struct BuilderOutput {
    /// Generation of the dataset: stamped on every node of the message.
    pub generation: u64,
    pub cut_epoch: u64,
    /// The view epoch this plan was made for (see the module doc).
    pub view_epoch: u64,
    pub cut: Vec<NodeId>,
    pub support: Vec<NodeId>,
    /// In build order (parents first); never merged or reordered.
    pub nodes: Vec<EncodedNode>,
    /// The build side has nothing more to do for this view: with the render side having applied
    /// this epoch, the cut is at rest.
    pub idle: bool,
    pub target_pixels: f64,
    pub stage_pixels: f64,
    pub stats: CellPlanStats,
    pub async_priority_order: Vec<BlockId>,
}

/// The build worker's side of the cells path.
pub(crate) struct BuilderCells {
    active: bool,
    stage_pixels: Option<f64>,
    dirty: bool,
    target_pixels: Option<f64>,
    last_camera: Option<OrbitCamera>,
    /// Epoch of the view the host is sending now, and the one the last plan used.
    view_epoch: u64,
    planned_view_epoch: Option<u64>,
    config: CellUpdateConfig,
}

impl BuilderCells {
    pub(crate) fn new(config: CellUpdateConfig) -> Self {
        Self {
            active: true,
            stage_pixels: None,
            dirty: false,
            target_pixels: None,
            last_camera: None,
            view_epoch: 0,
            planned_view_epoch: None,
            config,
        }
    }

    /// The epoch of the view of the next update (`viewEpoch` of the render side's camera).
    pub(crate) fn set_view_epoch(&mut self, epoch: u64) {
        self.view_epoch = epoch;
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    /// Whether more updates are worth running without a new view.
    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// R3-Q23: any colour mode other than plain RGB sends the dataset down the CPU path; back to
    /// RGB re-enables the cells. The pool was dropped by the CPU scene, so nothing is drawn.
    pub(crate) fn set_active(&mut self, dataset: &mut LodDataset, active: bool) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            dataset.pool_reset();
            self.stage_pixels = None;
            self.dirty = true;
        }
    }

    /// A block or a page arrived: another update may refine further.
    pub(crate) fn block_inserted(&mut self) {
        self.dirty = self.target_pixels.is_some();
    }

    /// The view changed (or the caller wants a new plan).
    #[cfg(test)]
    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Validates an update request (the checks of `SceneBuilderCore::update`) and builds the view.
    pub(crate) fn begin_update(
        &mut self,
        dataset: &LodDataset,
        camera: OrbitCamera,
        viewport: [u32; 2],
        visual_scale: f64,
        target_pixels: f64,
        limits: VoxelBuildLimits,
    ) -> Result<ViewProjection, String> {
        if !TARGET_PIXELS_RANGE.contains(&target_pixels) {
            return Err("targetVoxelPixels must be finite and between 0.5 and 16".to_owned());
        }
        if viewport.contains(&0) {
            return Err("viewport must be non-empty".to_owned());
        }
        if visual_scale != dataset.visual_scale() {
            return Err("visualScale does not match the dataset".to_owned());
        }
        if self.target_pixels != Some(target_pixels) {
            self.stage_pixels = None;
            self.dirty = true;
        }
        if self.last_camera != Some(camera) {
            self.dirty = true;
        }
        self.last_camera = Some(camera);
        if self.planned_view_epoch != Some(self.view_epoch) {
            // A new view epoch needs a cut stamped with it, even if the camera is the same.
            self.dirty = true;
            self.planned_view_epoch = Some(self.view_epoch);
        }
        self.target_pixels = Some(target_pixels);
        Ok(ViewProjection::new(camera, viewport, visual_scale, limits))
    }

    /// Step 1 for `view`. With a reader, the pages the cut lacks are read and grafted and the
    /// update repeats until none is missing (the nodes of every round are sent); hosts that
    /// deliver pages themselves pass `reader = None`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn plan<R>(
        &mut self,
        dataset: &mut LodDataset,
        view: &ViewProjection,
        target_pixels: f64,
        can_load_blocks: bool,
        track_async_order: bool,
        mut reader: Option<&mut R>,
        load_block: &mut impl FnMut(&mut R, BlockId) -> Result<Option<PointBlock>, String>,
        load_page: &mut impl FnMut(&mut R, NodeId) -> Result<PageInsert, String>,
    ) -> Result<BuilderOutput, String> {
        let generation = dataset.cell_generation();
        let mut nodes = Vec::new();
        let mut rounds = 0;
        loop {
            let mut order = Vec::new();
            let step = CellStep {
                view,
                target_pixels,
                config: self.config,
                can_load_blocks,
            };
            let mut load = |id| match reader.as_deref_mut() {
                Some(reader) => load_block(reader, id),
                None => Ok(None),
            };
            let planned = plan_cells_staged(
                dataset,
                &step,
                &mut self.stage_pixels,
                &mut self.dirty,
                track_async_order.then_some(&mut order),
                &mut load,
            )
            .map_err(|error| error.to_string())?;
            rounds += 1;
            let update = planned.update;
            for (node_generation, node) in &update.new_nodes {
                debug_assert_eq!(*node_generation, generation);
                let bytes = encode_node_levels(node).map_err(|error| error.to_string())?;
                nodes.push(EncodedNode { bytes });
            }
            let pending = update.stats.pending_page_anchors.clone();
            let done = pending.is_empty() || rounds == MAX_PAGE_ROUNDS;
            let Some(reader) = reader.as_deref_mut().filter(|_| !done) else {
                return Ok(BuilderOutput {
                    generation,
                    cut_epoch: update.cut_epoch,
                    view_epoch: self.view_epoch,
                    cut: update.cut,
                    support: update.support,
                    nodes,
                    idle: !self.dirty,
                    target_pixels,
                    stage_pixels: planned.stage_pixels,
                    stats: update.stats,
                    async_priority_order: order,
                });
            };
            for anchor in pending {
                let page = load_page(reader, anchor)?;
                graft_tolerating_budget(dataset, page)?;
            }
        }
    }

    /// `poolReset` / `evict` / `poolAck` of one render-side batch, in the order the protocol
    /// needs: the reset first (it precedes the ack of the frame that reset the pool).
    pub(crate) fn pool_reset(&mut self, dataset: &mut LodDataset) {
        dataset.pool_reset();
        // The render driver keeps its nodes and re-uploads them, but a reset after a failed frame
        // may also have lost what that frame was adding: plan once more so `missing` comes back.
        self.dirty = true;
    }

    pub(crate) fn evict(&mut self, dataset: &mut LodDataset, nodes: &[NodeId]) {
        dataset.apply_evicted(nodes);
        self.dirty |= !nodes.is_empty();
    }

    pub(crate) fn pool_ack(&mut self, dataset: &mut LodDataset, ack: PoolAck) {
        self.dirty |= !ack.missing.is_empty();
        dataset.apply_ack(ack);
    }

    /// The render side has a new driver: every node sent so far is sent again.
    pub(crate) fn driver_replaced(&mut self, dataset: &mut LodDataset) {
        dataset.driver_replaced();
        self.dirty = true;
    }

    pub(crate) fn stats(&self, dataset: &LodDataset) -> CellsStats {
        CellsStats {
            cells_busy: self.active && self.dirty,
            view_epoch: self.view_epoch,
            cut_epoch_sent: dataset.cell_cut_epoch(),
            cut_epoch_applied: dataset.cell_ack_epoch(),
            wasm_linear_memory_bytes: wasm_linear_memory_bytes(),
            ..CellsStats::default()
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Render side
// ---------------------------------------------------------------------------------------------

enum Item {
    Node {
        generation: u64,
        bytes: Vec<u8>,
    },
    Cut {
        epoch: u64,
        view_epoch: u64,
        cut: Vec<NodeId>,
        support: Vec<NodeId>,
        idle: bool,
        target_pixels: f64,
    },
}

struct HeldCut {
    epoch: u64,
    view_epoch: u64,
    cut: Vec<NodeId>,
    idle: bool,
    target_pixels: f64,
}

/// What the render side owes the build side. `evict` is cumulative and in order; the ack is the
/// last one (latest wins).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CellMessages {
    pub ack: Option<PoolAck>,
    pub evict: Vec<NodeId>,
    pub pool_reset: bool,
    pub driver_replaced: bool,
}

impl CellMessages {
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The render worker's side of the cells path.
pub(crate) struct ViewerCells {
    driver: CellDriver,
    queue: VecDeque<Item>,
    queued_nodes: usize,
    queued_bytes: usize,
    byte_cap: usize,
    cut: Option<HeldCut>,
    received_epoch: u64,
    applied_epoch: u64,
    /// Latest view epoch (`camera_changed`); a cut at rest must carry it.
    view_epoch: u64,
    view_dirty: bool,
    active: bool,
    digest: Option<u64>,
    last_missing: usize,
    counters: PoolCounters,
    out: CellMessages,
    probe_last: Cell<Option<u64>>,
}

impl ViewerCells {
    /// A fresh driver: the build side is told (`driverReplaced`) with the first
    /// [`take_messages`](Self::take_messages) so it sends every node again.
    pub(crate) fn new(config: DriverConfig, byte_cap: usize) -> Self {
        Self {
            driver: CellDriver::new(config),
            queue: VecDeque::new(),
            queued_nodes: 0,
            queued_bytes: 0,
            byte_cap: byte_cap.max(1),
            cut: None,
            received_epoch: 0,
            applied_epoch: 0,
            view_epoch: 0,
            view_dirty: false,
            active: true,
            digest: None,
            last_missing: 0,
            counters: PoolCounters::default(),
            out: CellMessages {
                driver_replaced: true,
                ..CellMessages::default()
            },
            probe_last: Cell::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn driver(&self) -> &CellDriver {
        &self.driver
    }

    /// R3-Q23: see [`BuilderCells::set_active`]. Back to RGB the held cut is drawn again.
    pub(crate) fn set_active(&mut self, active: bool) {
        if self.active != active {
            self.active = active;
            self.view_dirty = true;
            self.probe_last.set(None);
        }
    }

    /// The view changed (camera move or resize) and `epoch` is its number: the page table is
    /// rewritten by the next frame and no digest is at rest until a cut of `epoch` is applied.
    pub(crate) fn camera_changed(&mut self, epoch: u64) {
        self.view_epoch = epoch;
        self.view_dirty = true;
        self.probe_last.set(None);
    }

    /// One `nodeCells` message. Queued; applied in order under the byte cap. A node that does
    /// not decode is refused here, so the caller learns of it at the message (the queue and the
    /// driver are untouched).
    pub(crate) fn push_node(&mut self, generation: u64, bytes: Vec<u8>) -> Result<(), String> {
        decode_node_levels(&bytes)
            .map_err(|error| format!("malformed nodeCells entry: {error}"))?;
        self.queued_nodes += 1;
        self.queued_bytes += bytes.len();
        self.queue.push_back(Item::Node { generation, bytes });
        Ok(())
    }

    /// One `cellCut` message. The last one wins: an earlier cut still waiting behind nodes is
    /// dropped. It takes effect when the nodes sent before it have been applied.
    pub(crate) fn push_cut(
        &mut self,
        epoch: u64,
        view_epoch: u64,
        cut: Vec<NodeId>,
        support: Vec<NodeId>,
        idle: bool,
        target_pixels: f64,
    ) {
        if epoch < self.received_epoch {
            return;
        }
        self.received_epoch = epoch;
        self.queue.retain(|item| matches!(item, Item::Node { .. }));
        self.queue.push_back(Item::Cut {
            epoch,
            view_epoch,
            cut,
            support,
            idle,
            target_pixels,
        });
        self.probe_last.set(None);
    }

    /// Whether the worker must keep drawing.
    pub(crate) fn needs_frame<T: CellTarget>(&self, target: &T) -> bool {
        self.active
            && (!self.queue.is_empty()
                || (self.cut.is_some() && self.view_dirty)
                || self.driver.needs_frame_on(target))
    }

    /// A cut sent and not applied, queued nodes, or driver work.
    pub(crate) fn cells_busy<T: CellTarget>(&self, target: &T) -> bool {
        self.active
            && (!self.queue.is_empty()
                || self.received_epoch > self.applied_epoch
                || self.driver.needs_frame_on(target))
    }

    /// Applies queued messages up to the byte cap, then draws the held cut when anything
    /// changed. Returns the driver's report when it drew.
    pub(crate) fn frame<T: CellTarget>(
        &mut self,
        target: &mut T,
        view: &ViewProjection,
    ) -> Result<Option<FrameReport>, String> {
        // R3-Q23: with the cells off the CPU scene owns the pool; the queue waits for the RGB mode
        // (nodes and cut stay queued, the driver keeps what it has).
        if !self.active {
            return Ok(None);
        }
        let mut got = false;
        let mut applied_bytes = 0usize;
        while let Some(front) = self.queue.front() {
            if matches!(front, Item::Node { .. }) && applied_bytes >= self.byte_cap {
                break;
            }
            match self.queue.pop_front() {
                Some(Item::Node { generation, bytes }) => {
                    self.queued_nodes -= 1;
                    self.queued_bytes -= bytes.len();
                    applied_bytes += bytes.len();
                    let node = decode_node_levels(&bytes).map_err(|error| error.to_string())?;
                    add_cell_nodes(&mut self.driver, [(generation, node)])
                        .map_err(|error| error.to_string())?;
                }
                Some(Item::Cut {
                    epoch,
                    view_epoch,
                    cut,
                    support,
                    idle,
                    target_pixels,
                }) => {
                    self.driver.cut_received();
                    self.driver.set_support(&support);
                    self.cut = Some(HeldCut {
                        epoch,
                        view_epoch,
                        cut,
                        idle,
                        target_pixels,
                    });
                }
                None => break,
            }
            got = true;
        }
        let Some(held) = &self.cut else {
            return Ok(None);
        };
        if !(got || self.view_dirty || self.driver.needs_frame_on(target)) {
            return Ok(None);
        }
        self.view_dirty = false;
        let draw = match draw_cell_cut(
            &mut self.driver,
            target,
            held.epoch,
            &held.cut,
            view,
            held.target_pixels,
        ) {
            Ok(draw) => draw,
            Err(error) => {
                // The target may have dropped its pool and the cut was not applied.
                self.out.pool_reset = true;
                return Err(error.to_string());
            }
        };
        self.applied_epoch = held.epoch;
        self.digest = Some(cell_cut_digest(&draw.report.plan.entries));
        self.last_missing = draw.ack.missing.len();
        self.counters = *draw.report.counters();
        self.out.evict.extend(draw.evicted);
        self.out.pool_reset |= draw.pool_reset;
        self.out.ack = Some(draw.ack);
        Ok(Some(draw.report))
    }

    /// The messages for the build side, in the order it must apply them: `driverReplaced`, then
    /// `poolReset`, then `evict`, then `poolAck`.
    pub(crate) fn take_messages(&mut self) -> CellMessages {
        std::mem::take(&mut self.out)
    }

    /// The digest of the applied cut once everything is at rest: the build side said the cut is
    /// final for the latest view epoch, it was applied and drawn for the current camera, nothing
    /// is queued, the driver has no work, no node was missing, and
    /// the same digest came out of two consecutive reads.
    pub(crate) fn cut_digest_at_rest<T: CellTarget>(&self, target: &T) -> Option<u64> {
        let held = self.cut.as_ref();
        let rest = self.active
            && held.is_some_and(|cut| {
                cut.idle && cut.epoch == self.applied_epoch && cut.view_epoch == self.view_epoch
            })
            && !self.view_dirty
            && self.received_epoch == self.applied_epoch
            && self.queue.is_empty()
            && !self.driver.needs_frame_on(target)
            && self.last_missing == 0;
        let digest = self.digest.filter(|_| rest);
        let Some(digest) = digest else {
            self.probe_last.set(None);
            return None;
        };
        let stable = self.probe_last.get() == Some(digest);
        self.probe_last.set(Some(digest));
        stable.then_some(digest)
    }

    pub(crate) fn stats<T: CellTarget>(&self, target: &T) -> CellsStats {
        CellsStats {
            cells_busy: self.cells_busy(target),
            needs_frame: self.needs_frame(target),
            view_epoch: self.view_epoch,
            cut_epoch_sent: self.received_epoch,
            cut_epoch_applied: self.applied_epoch,
            queued_nodes: self.queued_nodes,
            queued_bytes: self.queued_bytes,
            cut_digest_at_rest: self.cut_digest_at_rest(target),
            counters: self.counters,
            cpu_cell_bytes: self.driver.cpu_bytes(),
            wasm_linear_memory_bytes: wasm_linear_memory_bytes(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// In-process
// ---------------------------------------------------------------------------------------------

/// The viewer holding the dataset: the three steps back to back.
pub(crate) struct InProcessCells {
    driver: CellDriver,
    active: bool,
    stage_pixels: Option<f64>,
    dirty: bool,
    config: CellUpdateConfig,
    applied_epoch: u64,
    digest: u64,
    counters: PoolCounters,
    probe: RefCell<CutRestProbe>,
    /// Pages the last update needed and lacked (`pendingPageAnchors`).
    pending_pages: Vec<NodeId>,
}

/// Where a block read starts and ends, so the caller can account the browser reads it caused.
pub(crate) enum ReadPhase {
    Before,
    After,
}

/// What one in-process update reports besides the frame it drew.
pub(crate) struct InProcessUpdate {
    pub plan: CellPlanStats,
    pub async_priority_order: Vec<BlockId>,
    pub stats: CellsStats,
}

impl InProcessCells {
    pub(crate) fn new(dataset: &LodDataset, config: CellUpdateConfig) -> Self {
        Self {
            driver: CellDriver::new(driver_config(
                dataset.cell_grid(),
                dataset.coordinate_origin(),
                WASM_CPU_BUDGET_BYTES,
            )),
            active: true,
            stage_pixels: None,
            dirty: false,
            config,
            applied_epoch: 0,
            digest: 0,
            counters: PoolCounters::default(),
            probe: RefCell::new(CutRestProbe::default()),
            pending_pages: Vec::new(),
        }
    }

    pub(crate) fn pending_pages(&self) -> &[NodeId] {
        &self.pending_pages
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    /// R3-Q23, as [`BuilderCells::set_active`].
    pub(crate) fn set_active(&mut self, dataset: &mut LodDataset, active: bool) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            dataset.pool_reset();
            self.stage_pixels = None;
            self.dirty = true;
        }
    }

    /// The camera moved or the viewport changed: another update is worth running, and until it
    /// ran no digest is at rest (the probe sees `dirty`).
    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// An update failed: do not retry every frame until something changes.
    pub(crate) fn failed(&mut self) {
        self.dirty = false;
    }

    pub(crate) fn target_changed(&mut self) {
        self.stage_pixels = None;
    }

    /// One update with the page rounds of `update_lod_view_paged`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn update<R, T: CellTarget>(
        &mut self,
        dataset: &mut LodDataset,
        view: &ViewProjection,
        target_pixels: f64,
        can_load_blocks: bool,
        target: &mut T,
        mut reader: Option<&mut R>,
        track_async_order: bool,
        load_block: &mut impl FnMut(&mut R, BlockId) -> Result<Option<PointBlock>, String>,
        load_page: &mut impl FnMut(&mut R, NodeId) -> Result<PageInsert, String>,
    ) -> Result<(CellPlanStats, Vec<BlockId>), String> {
        let mut rounds = 0;
        loop {
            let mut order = Vec::new();
            let step = CellStep {
                view,
                target_pixels,
                config: self.config,
                can_load_blocks,
            };
            let mut load = |id| match reader.as_deref_mut() {
                Some(reader) => load_block(reader, id),
                None => Ok(None),
            };
            let done = update_cells_staged(
                dataset,
                &step,
                &mut self.stage_pixels,
                &mut self.dirty,
                &mut self.driver,
                target,
                track_async_order.then_some(&mut order),
                &mut load,
            )
            .map_err(|error: LodDatasetError| error.to_string())?;
            rounds += 1;
            self.applied_epoch = dataset.cell_cut_epoch();
            self.digest = cell_cut_digest(&done.update.report.plan.entries);
            self.counters = *done.update.report.counters();
            let stats = done.update.stats;
            let pending = stats.pending_page_anchors.clone();
            let plan_stats = CellPlanStats {
                built_nodes: stats.built_nodes,
                build_deferred: stats.build_deferred,
                build_capped: stats.build_capped,
                build_ms: stats.build_ms,
                cut_nodes: stats.cut_nodes,
                max_gap: stats.max_gap,
                gaps_over_one: stats.gaps_over_one,
                pending_block_ids: stats.pending_block_ids,
                pending_page_anchors: stats.pending_page_anchors,
                uncovered_nodes: stats.uncovered_nodes,
            };
            let Some(reader) = reader
                .as_deref_mut()
                .filter(|_| !pending.is_empty() && rounds < MAX_PAGE_ROUNDS)
            else {
                return Ok((plan_stats, order));
            };
            for anchor in pending {
                let page = load_page(reader, anchor)?;
                graft_tolerating_budget(dataset, page)?;
            }
        }
    }

    /// [`update`](Self::update) on the unpainted blocks the cells use (R3-Q23: RGB only), with
    /// the pending pages kept and the stats read after it. `read_hook` brackets every block read.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn update_view<R, E, T>(
        &mut self,
        dataset: &mut LodDataset,
        view: &ViewProjection,
        target_pixels: f64,
        target: &mut T,
        reader: Option<&mut R>,
        has_async_reader: bool,
        read_hook: &mut impl FnMut(ReadPhase),
        load_page: &mut impl FnMut(&mut R, NodeId) -> Result<PageInsert, String>,
    ) -> Result<InProcessUpdate, String>
    where
        R: OctreeReader<Error = E>,
        E: std::fmt::Display,
        T: CellTarget,
    {
        let can_load_blocks = reader.is_some() || has_async_reader;
        let mut load_block = |reader: &mut R, block_id| {
            read_hook(ReadPhase::Before);
            let loaded = crate::block_loading::load_render_block(
                reader,
                block_id,
                Some(viewer_render_wgpu::ColorMode::Rgb),
                None,
                None,
            )
            .map(Some);
            read_hook(ReadPhase::After);
            loaded
        };
        let (plan, async_priority_order) = self.update(
            dataset,
            view,
            target_pixels,
            can_load_blocks,
            target,
            reader,
            has_async_reader,
            &mut load_block,
            load_page,
        )?;
        self.pending_pages.clone_from(&plan.pending_page_anchors);
        let stats = self.stats(dataset, target);
        Ok(InProcessUpdate {
            plan,
            async_priority_order,
            stats,
        })
    }

    pub(crate) fn stats<T: CellTarget>(&self, dataset: &LodDataset, target: &T) -> CellsStats {
        let needs = self.driver.needs_frame_on(target);
        let rest = self.probe.borrow_mut().poll(
            dataset,
            !self.dirty,
            self.applied_epoch,
            needs,
            self.digest,
        );
        CellsStats {
            cells_busy: self.active && (self.dirty || needs),
            needs_frame: self.active && (self.dirty || needs),
            cut_epoch_sent: dataset.cell_cut_epoch(),
            cut_epoch_applied: self.applied_epoch,
            cut_digest_at_rest: rest,
            counters: self.counters,
            cpu_cell_bytes: self.driver.cpu_bytes(),
            wasm_linear_memory_bytes: wasm_linear_memory_bytes(),
            ..CellsStats::default()
        }
    }
}
