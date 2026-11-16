//! R3 Task 10b: the same two-worker simulation as the parent module, but the messages go through
//! the glue the bindings wrap (`BuilderCells`, `ViewerCells`): `nodeCells` are codec bytes applied
//! under a byte cap per frame, `cellCut` and the render-to-build messages are the ones of
//! `takeCellMessages`. The oracle is the parent's: the digest at rest must be the in-process one.
//! Real-data cases are `#[ignore]` like the parent's (`real_glue_*`).

use super::*;
use crate::cells_glue::{BuilderCells, CellMessages, InProcessCells, ViewerCells, driver_config};
use crate::scene_builder::{BuilderStep, SceneBuilderCore};
use std::rc::Rc;
use viewer_render_wgpu::decode_node_levels;

/// Build to render. `nodeCells` is never merged; `cellCut` is latest-wins at the viewer.
enum Wire {
    Node(u64, Vec<u8>),
    Levels(u64, Vec<u8>),
    /// `levelsRefused`.
    Refused(Vec<(NodeId, u8)>),
    Cut {
        epoch: u64,
        view_epoch: u64,
        cut: Vec<NodeId>,
        support: Vec<NodeId>,
        idle: bool,
        target_px: f64,
    },
}

#[derive(Default, Debug)]
struct GlueStats {
    node_messages: usize,
    missing_reports: usize,
    pool_resets_seen: usize,
    driver_replacements: usize,
}

struct GlueSim {
    /// `dataset`, `blocks`, `pages` and `target` are used; the harness driver is not.
    h: Harness,
    builder: BuilderCells,
    viewer: ViewerCells,
    cpu_budget: u64,
    byte_cap: usize,
    to_render: VecDeque<Wire>,
    inflight: CellMessages,
    inflight_levels: Option<Vec<(NodeId, u8)>>,
    inflight_delay: usize,
    /// The render side's view counter: what `viewEpoch()` returns.
    view_gen: u64,
    /// The host sends a view to the build side when it differs from the last one it sent.
    build_seen: u64,
    digest: u64,
    /// `cutDigestAtRest` as of the last step.
    rest: Option<u64>,
    rng: Rng,
    stats: GlueStats,
    mode: LodMode,
}

impl GlueSim {
    fn new(h: Harness, cpu_budget: u64, seed: u64, byte_cap: usize) -> Self {
        let config = driver_config(
            h.dataset.cell_grid(),
            h.dataset.coordinate_origin(),
            cpu_budget,
        );
        let builder = BuilderCells::new(h.config);
        let mut viewer = ViewerCells::new(config, byte_cap);
        viewer.camera_changed(1);
        Self {
            h,
            builder,
            viewer,
            cpu_budget,
            byte_cap,
            to_render: VecDeque::new(),
            inflight: CellMessages::default(),
            inflight_levels: None,
            inflight_delay: 0,
            view_gen: 1,
            build_seen: 0,
            digest: 0,
            rest: None,
            rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
            stats: GlueStats::default(),
            mode: LodMode::Octant,
        }
    }

    /// Runs the render side in `mode`, also after the driver is replaced.
    fn in_mode(mut self, mode: LodMode) -> Self {
        self.mode = mode;
        self.viewer.set_lod_mode(mode);
        self
    }

    /// A fresh render-side driver; the view counter lives in the viewer, not the driver.
    fn new_viewer(&self) -> ViewerCells {
        let mut viewer = ViewerCells::new(
            driver_config(
                self.h.dataset.cell_grid(),
                self.h.dataset.coordinate_origin(),
                self.cpu_budget,
            ),
            self.byte_cap,
        );
        viewer.set_lod_mode(self.mode);
        viewer.camera_changed(self.view_gen);
        viewer
    }

    fn build_step(&mut self, view: &ViewProjection) {
        if self.inflight_delay > 0 {
            self.inflight_delay -= 1;
        } else {
            if let Some(requests) = self.inflight_levels.take() {
                self.builder.level_requests(&mut self.h.dataset, &requests);
            }
            let messages = std::mem::take(&mut self.inflight);
            let ds = &mut self.h.dataset;
            if messages.driver_replaced {
                self.builder.driver_replaced(ds);
            }
            if messages.pool_reset {
                self.stats.pool_resets_seen += 1;
                self.builder.pool_reset(ds);
            }
            self.builder.evict(ds, &messages.evict);
            if let Some(ack) = messages.ack {
                self.stats.missing_reports += ack.missing.len();
                self.builder.pool_ack(ds, ack);
            }
        }
        if self.build_seen != self.view_gen {
            self.builder.mark_dirty();
        } else if !self.builder.is_dirty() {
            return;
        }
        self.build_seen = self.view_gen;
        self.builder.set_view_epoch(self.view_gen);
        let (blocks, pages) = (&mut self.h.blocks, &mut self.h.pages);
        let out = self
            .builder
            .plan(
                &mut self.h.dataset,
                view,
                self.h.target_px,
                true,
                false,
                Some(&mut ()),
                &mut |_: &mut (), id| blocks(id),
                &mut |_: &mut (), anchor| pages(anchor),
            )
            .unwrap_or_else(|error| panic!("plan: {error}"));
        for node in out.nodes {
            self.stats.node_messages += 1;
            self.to_render
                .push_back(Wire::Node(out.generation, node.bytes));
        }
        for node in out.levels {
            self.to_render
                .push_back(Wire::Levels(out.generation, node.bytes));
        }
        if !out.refused.is_empty() {
            self.to_render.push_back(Wire::Refused(out.refused));
        }
        self.to_render.push_back(Wire::Cut {
            epoch: out.cut_epoch,
            view_epoch: out.view_epoch,
            cut: out.cut,
            support: out.support,
            idle: out.idle,
            target_px: out.target_pixels,
        });
    }

    fn render_step(&mut self, view: &ViewProjection, take: usize) {
        for _ in 0..take {
            match self.to_render.pop_front() {
                Some(Wire::Node(generation, bytes)) => self
                    .viewer
                    .push_node(generation, bytes)
                    .unwrap_or_else(|error| panic!("push_node: {error}")),
                Some(Wire::Levels(generation, bytes)) => self
                    .viewer
                    .push_levels(generation, bytes)
                    .unwrap_or_else(|error| panic!("push_levels: {error}")),
                Some(Wire::Refused(refused)) => self.viewer.refuse_levels(&refused),
                Some(Wire::Cut {
                    epoch,
                    view_epoch,
                    cut,
                    support,
                    idle,
                    target_px,
                }) => self
                    .viewer
                    .push_cut(epoch, view_epoch, cut, support, idle, target_px),
                None => break,
            }
        }
        self.h.target.eye = view.camera().position();
        let report = self
            .viewer
            .frame(&mut self.h.target, view)
            .unwrap_or_else(|error| panic!("frame: {error}"));
        if let Some(report) = report {
            self.h.target.check_plan(&report.plan);
            self.digest = cell_cut_digest(&report.plan.entries);
        }
        let messages = self.viewer.take_messages();
        if messages.driver_replaced {
            self.inflight_delay = 1 + self.rng.below(4);
        }
        self.inflight.driver_replaced |= messages.driver_replaced;
        self.inflight.pool_reset |= messages.pool_reset;
        self.inflight.evict.extend(messages.evict);
        if messages.ack.is_some() {
            self.inflight.ack = messages.ack;
        }
        if let Some(requests) = self.viewer.take_level_requests() {
            self.inflight_levels = Some(requests);
        }
    }

    fn step(&mut self, view: &ViewProjection) {
        if self.rng.below(2) == 0 {
            self.build_step(view);
        } else {
            let available = self.to_render.len();
            let take = self.rng.below(available + 1).min(3);
            self.render_step(view, take);
        }
        self.rest = self.viewer.cut_digest_at_rest(&self.h.target);
        if self.rest.is_some() {
            // The oracle: a digest at rest is only ever the one of a view the build side planned
            // for and finished (the glue has to know this from the echoed view epoch alone).
            assert!(
                self.build_seen == self.view_gen && !self.builder.is_dirty(),
                "digest at rest for a stale view: view {} planned {} dirty {}",
                self.view_gen,
                self.build_seen,
                self.builder.is_dirty()
            );
        }
    }

    /// Steps on the last view until the viewer publishes `cutDigestAtRest` on its own, the way
    /// the host reads it: the only signal is what the glue exposes (the cut's echoed view epoch
    /// included). It must be the digest of the last cut the viewer drew.
    fn settle(&mut self, view: &ViewProjection, max_iterations: usize) -> u64 {
        for _ in 0..max_iterations {
            self.step(view);
            if let Some(digest) = self.rest {
                assert_eq!(
                    digest, self.digest,
                    "the viewer's cutDigestAtRest is not the digest of its last drawn cut"
                );
                return digest;
            }
        }
        panic!(
            "no digest at rest after {max_iterations} iterations: {:?} dirty {} queue {} epoch {} \
             ack {}",
            self.stats,
            self.builder.is_dirty(),
            self.to_render.len(),
            self.h.dataset.cell_cut_epoch(),
            self.h.dataset.cell_ack_epoch(),
        )
    }

    fn run(&mut self, views: &[ViewProjection], inject: Inject, max_iterations: usize) -> u64 {
        let advance_every = 6;
        let mut moves = 0;
        for (at, view) in views.iter().enumerate().skip(1) {
            for iteration in 0..advance_every {
                if iteration == 0 {
                    self.view_gen += 1;
                    self.viewer.camera_changed(self.view_gen);
                    moves += 1;
                }
                if inject.pool_reset_at == Some(moves) && iteration == 1 {
                    self.h.target.drop_pool_for_test();
                    self.builder.mark_dirty();
                }
                if inject.driver_replaced_at == Some(moves) && iteration == 3 {
                    self.viewer = self.new_viewer();
                    self.h.target = StrictTarget::default();
                    self.to_render.clear();
                    self.inflight = CellMessages::default();
                    self.inflight_levels = None;
                    self.stats.driver_replacements += 1;
                }
                self.step(view);
                let _ = at;
            }
        }
        self.settle(views.last().unwrap(), max_iterations)
    }
}

impl StrictTarget {
    fn drop_pool_for_test(&mut self) {
        use viewer_render_wgpu::CellTarget;
        self.drop_pool();
    }
}

const GLUE_SEEDS: [u64; 3] = [1, 7, 42];

fn glue_synthetic_case(name: &str, views: Vec<ViewProjection>, inject: Inject, mode: LodMode) {
    let full = synthetic_cpu_bytes(&views, mode);
    let budget = full / 3;
    let expected = in_process_digest(
        synthetic_harness(1 << 20, 2.0).in_mode(mode),
        budget,
        &views,
    );
    let mut reports = 0;
    for (i, seed) in GLUE_SEEDS.iter().enumerate() {
        // A cap of one byte applies one node per frame; the other seeds apply bursts.
        let cap = if i == 0 { 1 } else { 256 << 10 };
        let mut sim =
            GlueSim::new(synthetic_harness(1 << 20, 2.0), budget, *seed, cap).in_mode(mode);
        let digest = sim.run(&views, inject, 200_000);
        assert_eq!(digest, expected, "{name} seed {seed} {mode:?}");
        assert!(sim.stats.node_messages > 0);
        reports += sim.stats.missing_reports;
    }
    // Which seeds hit the path depends on the interleaving: an ack naming the missing nodes can
    // be superseded before the builder reads it, or the builder learns of the replacement first.
    if inject.driver_replaced_at.is_some() {
        assert!(reports > 0, "the missing path was not exercised");
    }
}

/// Runs a glue simulation in both pool modes: the octant one keeps its name.
macro_rules! glue_both_modes {
    ($body:ident, $octant:ident, $cell:ident) => {
        #[test]
        fn $octant() {
            $body(LodMode::Octant);
        }

        #[test]
        fn $cell() {
            $body(LodMode::Cell);
        }
    };
}

fn glue_orbit_in(mode: LodMode) {
    glue_synthetic_case("orbit", synth_orbit(), Inject::default(), mode);
}

glue_both_modes!(
    glue_orbit_in,
    glue_synthetic_orbit_reaches_the_in_process_digest,
    glue_synthetic_orbit_reaches_the_in_process_digest_in_cell_mode
);

fn glue_dolly_in(mode: LodMode) {
    glue_synthetic_case("dolly", synth_dolly(), Inject::default(), mode);
}

glue_both_modes!(
    glue_dolly_in,
    glue_synthetic_dolly_reaches_the_in_process_digest,
    glue_synthetic_dolly_reaches_the_in_process_digest_in_cell_mode
);

fn glue_inject_in(mode: LodMode) {
    let inject = Inject {
        pool_reset_at: Some(5),
        driver_replaced_at: Some(10),
        forget_zones_on_loss: true,
        ..Inject::default()
    };
    glue_synthetic_case("orbit+inject", synth_orbit(), inject, mode);
}

glue_both_modes!(
    glue_inject_in,
    glue_synthetic_pool_reset_and_driver_replaced_mid_load_converge,
    glue_synthetic_pool_reset_and_driver_replaced_mid_load_converge_in_cell_mode
);

#[test]
fn the_viewer_stats_report_the_cut_apply_and_the_nodes_forgotten_for_the_cpu_budget() {
    use viewer_render_wgpu::CutApply;
    let views = synth_orbit();
    // The at-rest set now carries the levels built on demand (569 112 bytes, 206 200 before them,
    // same nodes): an eighth is the absolute budget a third was (about 70 000 bytes), the one at
    // which this seed forgets a node.
    let budget = synthetic_cpu_bytes(&views, LodMode::Octant) / 8;
    let mut sim = GlueSim::new(synthetic_harness(1 << 20, 2.0), budget, 7, 256 << 10);
    let before = sim.viewer.stats(&sim.h.target);
    assert_eq!((before.cut_apply, before.forgotten_total), (None, 0));
    sim.run(&views, Inject::default(), 200_000);
    let after = sim.viewer.stats(&sim.h.target);
    assert_eq!(after.cut_apply, Some(CutApply::Applied));
    assert!(
        after.forgotten_total > 0,
        "an eighth of the working set forgot nothing"
    );
    // Retained: a pool too small for the cut keeps the previous one (overflow).
    let mut sim = GlueSim::new(synthetic_harness(1 << 20, 2.0), budget, 7, 256 << 10);
    let mut config = driver_config(
        sim.h.dataset.cell_grid(),
        sim.h.dataset.coordinate_origin(),
        budget,
    );
    config.pool.capacity_pages = 2;
    sim.viewer = ViewerCells::new(config, 256 << 10);
    sim.viewer.camera_changed(sim.view_gen);
    let mut retained = false;
    for _ in 0..200 {
        sim.step(&views[0]);
        retained |= sim.viewer.stats(&sim.h.target).cut_apply == Some(CutApply::RetainedPrevious);
    }
    assert!(retained, "a two-page pool never retained the previous cut");
}

#[test]
fn a_burst_of_node_cells_is_applied_over_several_frames_in_order() {
    let mut h = synthetic_harness(1 << 20, 2.0);
    h.config = CellUpdateConfig {
        max_node_builds: 8,
        ..CellUpdateConfig::default()
    };
    let view = synth_orbit().remove(0);
    let mut builder = BuilderCells::new(h.config);
    let (mut nodes, mut last) = (Vec::new(), None);
    for _ in 0..40 {
        builder.mark_dirty();
        let (blocks, pages) = (&mut h.blocks, &mut h.pages);
        let out = builder
            .plan(
                &mut h.dataset,
                &view,
                2.0,
                true,
                false,
                Some(&mut ()),
                &mut |_: &mut (), id| blocks(id),
                &mut |_: &mut (), anchor| pages(anchor),
            )
            .unwrap();
        for node in &out.nodes {
            let id = decode_node_levels(&node.bytes).unwrap().id;
            nodes.push((out.generation, id, node.bytes.clone()));
        }
        last = Some((out.cut_epoch, out.cut, out.support));
        if nodes.len() >= 4 {
            break;
        }
    }
    assert!(nodes.len() >= 4, "only {} nodes built", nodes.len());
    let (epoch, cut, support) = last.unwrap();
    let config = driver_config(
        h.dataset.cell_grid(),
        h.dataset.coordinate_origin(),
        u64::MAX,
    );
    let mut viewer = ViewerCells::new(config, 1);
    let mut target = StrictTarget {
        eye: view.camera().position(),
        ..StrictTarget::default()
    };
    for (generation, _, bytes) in &nodes {
        viewer.push_node(*generation, bytes.clone()).unwrap();
    }
    viewer.push_cut(epoch, 0, cut, support, true, 2.0);
    for frame in 1..=nodes.len() {
        viewer.frame(&mut target, &view).unwrap();
        for (i, (_, id, _)) in nodes.iter().enumerate() {
            assert_eq!(
                viewer.driver().has_node(*id),
                i < frame,
                "frame {frame}: node {i} is applied out of order"
            );
        }
        let stats = viewer.stats(&target);
        assert!(stats.cells_busy, "frame {frame}: not busy with work queued");
        assert_eq!(stats.queued_nodes, nodes.len() - frame);
        if frame < nodes.len() {
            assert_eq!(stats.cut_epoch_applied, 0, "the cut jumped its nodes");
        }
    }
    let mut frames = 0;
    while viewer.cells_busy(&target) {
        viewer.frame(&mut target, &view).unwrap();
        frames += 1;
        assert!(frames < 200, "cellsBusy never cleared");
    }
    assert_eq!(viewer.stats(&target).cut_epoch_applied, epoch);
}

#[test]
fn the_builder_emits_no_scene_while_cells_are_present() {
    let h = synthetic_harness(1 << 20, 2.0);
    let view = synth_orbit().remove(0);
    let camera = view.camera();
    let scale = h.dataset.visual_scale();
    let mut cells = BuilderCells::new(h.config);
    let mut core = SceneBuilderCore::<()>::new(
        crate::lod_session::LodSession::External(h.dataset),
        true,
        VoxelBuildLimits::default(),
    );
    for _ in 0..2 {
        let step = core
            .update_with_cells(
                Some(&mut cells),
                camera,
                [800, 600],
                scale,
                2.0,
                |_: &mut (), _| Ok(None),
                |_: &mut (), _| Err("no pages".to_owned()),
            )
            .unwrap();
        assert!(matches!(step, BuilderStep::Cells(_)));
    }
}

#[test]
fn the_in_process_glue_reaches_the_in_process_digest() {
    let views = synth_orbit();
    let budget = synthetic_cpu_bytes(&views, LodMode::Octant);
    let expected = in_process_digest(synthetic_harness(1 << 20, 2.0), budget, &views);
    let mut h = synthetic_harness(1 << 20, 2.0);
    let mut cells = InProcessCells::new(&h.dataset, h.config);
    let mut target = StrictTarget::default();
    let mut rest = None;
    for (i, view) in views.iter().enumerate() {
        cells.mark_dirty();
        let last = i + 1 == views.len();
        for _ in 0..(if last { 20_000 } else { 3 }) {
            target.eye = view.camera().position();
            let (blocks, pages) = (&mut h.blocks, &mut h.pages);
            cells
                .update(
                    &mut h.dataset,
                    view,
                    2.0,
                    true,
                    &mut target,
                    Some(&mut ()),
                    false,
                    &mut |_: &mut (), id| blocks(id),
                    &mut |_: &mut (), anchor| pages(anchor),
                )
                .unwrap();
            let stats = cells.stats(&h.dataset, &target);
            if last && stats.cut_digest_at_rest.is_some() {
                rest = stats.cut_digest_at_rest;
                break;
            }
        }
    }
    assert_eq!(rest, Some(expected));
    // A camera move (`mark_dirty`) withdraws the digest until the next update ran.
    cells.mark_dirty();
    for _ in 0..3 {
        assert_eq!(cells.stats(&h.dataset, &target).cut_digest_at_rest, None);
    }
}

/// Runs `InProcessCells` updates on `view` until the cut is at rest or `max` updates ran.
fn in_process_updates(
    h: &mut Harness,
    cells: &mut InProcessCells,
    target: &mut StrictTarget,
    view: &ViewProjection,
    max: usize,
) -> usize {
    for round in 1..=max {
        target.eye = view.camera().position();
        let (blocks, pages) = (&mut h.blocks, &mut h.pages);
        cells
            .update(
                &mut h.dataset,
                view,
                2.0,
                true,
                target,
                Some(&mut ()),
                false,
                &mut |_: &mut (), id| blocks(id),
                &mut |_: &mut (), anchor| pages(anchor),
            )
            .unwrap();
        if cells.stats(&h.dataset, target).cut_digest_at_rest.is_some() {
            return round;
        }
    }
    max
}

#[test]
fn switching_mode_rewrites_the_page_table() {
    let views = synth_orbit();
    let view = views.last().unwrap();
    let mut h = synthetic_harness(1 << 20, 2.0);
    let mut cells = InProcessCells::new(&h.dataset, h.config);
    let mut target = StrictTarget::default();
    cells.mark_dirty();
    in_process_updates(&mut h, &mut cells, &mut target, view, 20_000);
    assert!(!target.table.is_empty(), "octant mode drew nothing");
    assert!(target.lod_view.is_none(), "octant mode passes no LOD view");
    for (mode, has_view) in [(LodMode::Cell, true), (LodMode::Octant, false)] {
        let (records, tables) = (target.records_written, target.tables_written);
        cells.set_lod_mode(mode);
        assert!(cells.is_dirty(), "{mode:?}: the switch asks for a frame");
        let rounds = in_process_updates(&mut h, &mut cells, &mut target, view, 20_000);
        // The rest probe was reset by the switch: a digest needs two equal reads of post-switch
        // frames, so a stale one from before the switch cannot be published at once.
        assert!(rounds >= 2, "{mode:?}: digest at rest after {rounds} round");
        assert!(
            target.tables_written > tables,
            "{mode:?}: no new page table"
        );
        assert!(
            target.records_written > records,
            "{mode:?}: node records were not rewritten"
        );
        assert_eq!(target.lod_view.is_some(), has_view, "{mode:?}");
        assert!(!target.table.is_empty(), "{mode:?}: the frame drew nothing");
    }
}

#[test]
fn switching_mode_rewrites_the_page_table_through_the_glue() {
    let views = synth_orbit();
    let view = views.last().unwrap();
    let mut sim = GlueSim::new(synthetic_harness(1 << 20, 2.0), u64::MAX, 3, 1 << 20);
    sim.run(&views, Inject::default(), 200_000);
    assert!(!sim.h.target.table.is_empty(), "octant mode drew nothing");
    assert!(
        sim.h.target.lod_view.is_none(),
        "octant mode passes no LOD view"
    );
    for (mode, has_view) in [(LodMode::Cell, true), (LodMode::Octant, false)] {
        let (records, tables) = (sim.h.target.records_written, sim.h.target.tables_written);
        let nodes = sim.stats.node_messages;
        sim.mode = mode;
        sim.viewer.set_lod_mode(mode);
        // `settle` runs StrictTarget's plan checks on every frame drawn after the switch.
        sim.settle(view, 200_000);
        assert!(
            sim.h.target.tables_written > tables,
            "{mode:?}: no new page table"
        );
        assert!(
            sim.h.target.records_written > records,
            "{mode:?}: records not rewritten"
        );
        assert_eq!(sim.h.target.lod_view.is_some(), has_view, "{mode:?}");
        assert!(
            !sim.h.target.table.is_empty(),
            "{mode:?}: the frame drew nothing"
        );
        assert_eq!(
            sim.stats.node_messages, nodes,
            "{mode:?}: no node is built again"
        );
    }
}

/// The in-process path (`InProcessCells::update` is `update_cells_staged` plus page rounds, which
/// the synthetic cloud does not need): the views of the orbit, then updates on the last one.
/// Returns the deepest depth the last drawn cut draws, the octants it draws, and the bytes the
/// driver holds; `max_level_builds` is how many on-demand level builds an update may do.
fn in_process_drawn_depths(max_level_builds: usize) -> (u8, usize, u64) {
    use crate::cells_glue::WASM_CPU_BUDGET_BYTES;
    use crate::lod_driver::{CellStep, update_cells_staged};
    use viewer_render_wgpu::CellDriver;
    let views = synth_orbit();
    let mut h = synthetic_harness(1 << 20, 2.0);
    let config = CellUpdateConfig {
        max_level_builds,
        ..h.config
    };
    let mut driver = CellDriver::new(driver_config(
        h.dataset.cell_grid(),
        h.dataset.coordinate_origin(),
        WASM_CPU_BUDGET_BYTES,
    ));
    let mut target = StrictTarget::default();
    let (mut stage_pixels, mut dirty) = (None, true);
    let mut last = None;
    for (i, view) in views.iter().enumerate() {
        let rounds = if i + 1 == views.len() { 300 } else { 3 };
        for _ in 0..rounds {
            target.eye = view.camera().position();
            let step = CellStep {
                view,
                target_pixels: 2.0,
                config,
                can_load_blocks: true,
            };
            let blocks = &mut h.blocks;
            let done = update_cells_staged(
                &mut h.dataset,
                &step,
                &mut stage_pixels,
                &mut dirty,
                &mut driver,
                &mut target,
                None,
                blocks,
            )
            .unwrap();
            last = Some(done.update.report);
        }
    }
    let plan = last.unwrap().plan;
    let depths: Vec<u8> = plan
        .entries
        .iter()
        .flat_map(|entry| entry.drawn)
        .flatten()
        .collect();
    (
        depths.iter().copied().max().unwrap(),
        depths.len(),
        driver.cpu_bytes(),
    )
}

#[test]
fn update_cells_staged_serves_a_level_request() {
    // Without level builds a requested depth the nodes did not ship is never available: the cut
    // draws only what the chains hold.
    let (deepest_without, _, bytes_without) = in_process_drawn_depths(0);
    // With them the pruned depths are built, shipped to the driver and drawn by the cut.
    let (deepest, _, bytes) = in_process_drawn_depths(CellUpdateConfig::default().max_level_builds);
    assert!(
        bytes > bytes_without,
        "the levels were shipped to the driver"
    );
    assert!(
        deepest > deepest_without,
        "a depth absent from the chains is drawn: {deepest} against {deepest_without}"
    );
}

/// One builder and one viewer driven by hand (no simulated latency): `send` plans until the
/// builder is idle and hands everything to the viewer, `pump` runs viewer frames until it is
/// quiet.
struct Pair {
    h: Harness,
    builder: BuilderCells,
    viewer: ViewerCells,
    target: StrictTarget,
    view: ViewProjection,
    target_px: f64,
    /// Every (node, depth) the builder shipped through the level path, in order.
    shipped_levels: Vec<(NodeId, u8)>,
    /// Every (node, depth) of the chains the builder shipped as nodes.
    shipped_chains: Vec<(NodeId, u8)>,
    /// The last frame report of `pump` that drew.
    last_report: Option<viewer_render_wgpu::FrameReport>,
}

impl Pair {
    fn new() -> Self {
        Self::with_harness(synthetic_harness(1 << 20, 2.0))
    }

    fn with_harness(h: Harness) -> Self {
        let view = synth_orbit().remove(0);
        let config = driver_config(
            h.dataset.cell_grid(),
            h.dataset.coordinate_origin(),
            u64::MAX,
        );
        let target = StrictTarget {
            eye: view.camera().position(),
            ..StrictTarget::default()
        };
        Self {
            builder: BuilderCells::new(h.config),
            viewer: ViewerCells::new(config, 1 << 20),
            h,
            target,
            view,
            target_px: 2.0,
            shipped_levels: Vec::new(),
            shipped_chains: Vec::new(),
            last_report: None,
        }
    }

    fn plan(&mut self) -> crate::cells_glue::BuilderOutput {
        let (blocks, pages) = (&mut self.h.blocks, &mut self.h.pages);
        self.builder
            .plan(
                &mut self.h.dataset,
                &self.view,
                self.target_px,
                true,
                false,
                Some(&mut ()),
                &mut |_: &mut (), id| blocks(id),
                &mut |_: &mut (), anchor| pages(anchor),
            )
            .unwrap()
    }

    /// Plans until idle for `view_epoch`, sending nodes and the last cut when `deliver`.
    /// Returns the nodes' bytes in order (every round).
    fn send(&mut self, view_epoch: u64, deliver: bool) -> Vec<(u64, Vec<u8>)> {
        let mut nodes = self.send_round(view_epoch, deliver);
        // The viewer's requests for levels are served by more rounds until it asks for no more.
        while deliver && self.serve_requests() {
            nodes.extend(self.send_round(view_epoch, deliver));
        }
        nodes
    }

    /// Runs viewer frames until quiet and hands the driver's level requests to the builder.
    /// Whether it had any.
    fn serve_requests(&mut self) -> bool {
        self.pump();
        self.relay_level_requests()
    }

    fn send_round(&mut self, view_epoch: u64, deliver: bool) -> Vec<(u64, Vec<u8>)> {
        let (nodes, idle) = self.send_round_bounded(view_epoch, deliver, false);
        assert!(idle, "the builder never went idle");
        nodes
    }

    /// One round of at most 500 plans. Whether the builder went idle; when `always_cut`, every
    /// plan's cut is delivered (idle or not) so a builder that never settles still draws.
    fn send_round_bounded(
        &mut self,
        view_epoch: u64,
        deliver: bool,
        always_cut: bool,
    ) -> (Vec<(u64, Vec<u8>)>, bool) {
        self.builder.set_view_epoch(view_epoch);
        self.builder.mark_dirty();
        let mut nodes = Vec::new();
        for _ in 0..500 {
            let out = self.plan();
            for node in &out.nodes {
                nodes.push((out.generation, node.bytes.clone()));
                let chain = decode_node_levels(&node.bytes).unwrap();
                self.shipped_chains
                    .extend(chain.levels.iter().map(|level| (chain.id, level.depth)));
                if deliver {
                    self.viewer
                        .push_node(out.generation, node.bytes.clone())
                        .unwrap();
                }
            }
            if deliver {
                for node in &out.levels {
                    let levels = decode_node_levels(&node.bytes).unwrap();
                    self.shipped_levels
                        .extend(levels.levels.iter().map(|level| (levels.id, level.depth)));
                    self.viewer
                        .push_levels(out.generation, node.bytes.clone())
                        .unwrap();
                }
                self.viewer.refuse_levels(&out.refused);
            }
            if out.idle || always_cut {
                assert!(out.view_epoch == view_epoch, "the plan lost the view epoch");
                if deliver {
                    self.viewer.push_cut(
                        out.cut_epoch,
                        out.view_epoch,
                        out.cut,
                        out.support,
                        out.idle,
                        self.target_px,
                    );
                }
            }
            if out.idle {
                return (nodes, true);
            }
        }
        (nodes, false)
    }

    /// [`Pair::send`] with a viewer frame after every plan round and the strict target's and
    /// the hole oracle's checks on every frame: the builder keeps planning while the viewer draws.
    fn send_checked(&mut self, view_epoch: u64) -> Vec<(u64, Vec<u8>)> {
        self.builder.set_view_epoch(view_epoch);
        self.builder.mark_dirty();
        let mut nodes = Vec::new();
        'plans: for _ in 0..500 {
            let out = self.plan();
            for node in &out.nodes {
                nodes.push((out.generation, node.bytes.clone()));
                self.viewer
                    .push_node(out.generation, node.bytes.clone())
                    .unwrap();
            }
            for node in &out.levels {
                self.viewer
                    .push_levels(out.generation, node.bytes.clone())
                    .unwrap();
            }
            self.viewer.refuse_levels(&out.refused);
            let idle = out.idle;
            self.viewer.push_cut(
                out.cut_epoch,
                out.view_epoch,
                out.cut,
                out.support,
                idle,
                2.0,
            );
            self.checked_frame();
            if self.relay_level_requests() {
                continue;
            }
            if idle {
                for _ in 0..500 {
                    self.checked_frame();
                    if self.relay_level_requests() {
                        continue 'plans;
                    }
                    if !self.viewer.cells_busy(&self.target) {
                        return nodes;
                    }
                }
                panic!("the viewer never went quiet");
            }
        }
        panic!("the builder never went idle");
    }

    /// The viewer's changed level requests go to the builder, which plans again. Whether any.
    fn relay_level_requests(&mut self) -> bool {
        let Some(requests) = self.viewer.take_level_requests() else {
            return false;
        };
        self.builder.level_requests(&mut self.h.dataset, &requests);
        true
    }

    fn checked_frame(&mut self) {
        let report = self.viewer.frame(&mut self.target, &self.view).unwrap();
        if let Some(report) = report {
            self.target.check_plan(&report.plan);
            let holes = CutView::new(&self.h.dataset, &report.plan).holes();
            assert!(
                holes.is_empty(),
                "holes: {:?}",
                &holes[..holes.len().min(3)]
            );
        }
    }

    fn pump(&mut self) {
        for frame in 0..500 {
            if let Some(report) = self.viewer.frame(&mut self.target, &self.view).unwrap() {
                self.last_report = Some(report);
            }
            if !self.viewer.cells_busy(&self.target) {
                return;
            }
            assert!(frame < 499, "the viewer never went quiet");
        }
    }

    /// `cutDigestAtRest` as the host polls it: twice in a row.
    fn rest(&self) -> Option<u64> {
        let _ = self.viewer.cut_digest_at_rest(&self.target);
        self.viewer.cut_digest_at_rest(&self.target)
    }
}

/// A leaf of 100 close pairs on a coarse lattice: the cell count stays at 100 from the lattice
/// spacing down to the pair gap, so `chain_keep_mask` drops every coarse level (#35).
fn close_pair_leaf() -> Harness {
    let bounds = SpatialBounds::new([0.0; 3], [SIDE; 3]).unwrap();
    let node = SpatialNode::new(
        NodeId::new(1),
        None,
        0,
        bounds,
        200,
        0.0,
        vec![BlockId::new(101)],
    )
    .unwrap();
    let dataset = octree_dataset(vec![node], HierarchySemantics::Replacement, true);
    let mut points = Vec::new();
    for x in 0..5 {
        for y in 0..5 {
            for z in 0..4 {
                let at = [x, y, z].map(|i| 6.0 + 12.0 * f64::from(i));
                for gap in [0.0, 0.01] {
                    points.push(Point::new(
                        [(at[0] + gap) as f32, at[1] as f32, at[2] as f32],
                        ColorRgb::new(10, 120, 200),
                    ));
                }
            }
        }
    }
    let block = PointBlock::new(BlockId::new(101), PointBatch::new(points), Vec::new());
    Harness::new(
        dataset,
        4.0,
        Box::new(move |_| Ok(Some(block.clone()))),
        Box::new(|_| Err("no pages".into())),
    )
}

/// What a bounded run of the close-pair leaf drew and shipped.
struct CloseRun {
    /// Depth the full, unpruned chain draws, per octant.
    expected: Vec<u8>,
    full_depths: Vec<u8>,
    /// Depths of the chain the node shipped (pruned).
    pruned: Vec<u8>,
    drawn: [Option<u8>; 8],
    /// Depths shipped through the level path.
    shipped_levels: Vec<u8>,
    idle: bool,
}

/// The #35 scenario: the close-pair leaf viewed from far at 4 px, a few request round-trips, with
/// `max_level_builds` on-demand builds per update. A builder that never settles (no builds) is
/// bounded and still reports what the viewer drew.
fn close_pair_run(max_level_builds: usize) -> CloseRun {
    use viewer_render_wgpu::{NodeCellSource, PointNodeCellSource, octant_wanted_depth};
    let id = NodeId::new(1);
    let target_px = 4.0;
    let mut pair = Pair::with_harness(close_pair_leaf());
    pair.target_px = target_px;
    pair.builder = BuilderCells::new(CellUpdateConfig {
        max_level_builds,
        ..pair.h.config
    });
    let c = SIDE / 2.0;
    pair.view = synth_view([c + 4.0 * SIDE, c + 0.5 * SIDE, c + 1.0 * SIDE], [c; 3]);
    pair.target.eye = pair.view.camera().position();

    let grid = pair.h.dataset.cell_grid();
    let origin = pair.h.dataset.coordinate_origin();
    let bounds = pair
        .h
        .dataset
        .metadata
        .hierarchy()
        .node(id)
        .unwrap()
        .bounds();
    let block = (pair.h.blocks)(BlockId::new(101)).unwrap().unwrap();
    let full = PointNodeCellSource { min_growth: 1.0 }
        .node_cells(&grid, id, bounds, None, true, &[block])
        .unwrap();
    let full_depths = full.levels.iter().map(|level| level.depth).collect();
    let expected = (0..8)
        .map(|octant| {
            octant_wanted_depth(
                &pair.view,
                &grid,
                origin,
                bounds,
                octant,
                full.buildable,
                target_px,
            )
        })
        .collect();

    let mut idle = false;
    for _ in 0..4 {
        idle = pair.send_round_bounded(0, true, true).1;
        pair.pump();
        if !pair.relay_level_requests() {
            break;
        }
    }
    let report = pair.last_report.take().expect("a frame drew");
    let entry = report.plan.entries.iter().find(|e| e.node == id).unwrap();
    let chain = |from: &[(NodeId, u8)]| {
        from.iter()
            .filter(|(node, _)| *node == id)
            .map(|(_, depth)| *depth)
            .collect::<Vec<u8>>()
    };
    CloseRun {
        expected,
        full_depths,
        pruned: chain(&pair.shipped_chains),
        drawn: entry.drawn,
        shipped_levels: chain(&pair.shipped_levels),
        idle,
    }
}

#[test]
fn a_close_pair_leaf_draws_at_its_spacing_from_far() {
    let run = close_pair_run(CellUpdateConfig::default().max_level_builds);
    assert!(!run.pruned.is_empty(), "the node was shipped");
    // The value first: every octant draws the depth the full chain would draw.
    for (octant, depth) in run.drawn.iter().enumerate() {
        assert_eq!(
            *depth,
            Some(run.expected[octant]),
            "octant {octant}: {:?}",
            run.drawn
        );
    }
    for depth in &run.expected {
        assert!(
            run.full_depths.contains(depth),
            "full chain {:?}",
            run.full_depths
        );
        assert!(
            !run.pruned.contains(depth),
            "depth {depth} is absent from the pruned chain {:?}",
            run.pruned
        );
        assert!(
            run.shipped_levels.contains(depth),
            "depth {depth} shipped on demand: {:?}",
            run.shipped_levels
        );
    }
    assert!(run.idle, "the builder settled");
}

/// A depth the build side refuses (a leaf merged at the deepest cell depth) must stop being
/// wanted across the worker split. No real view wants depth 63, so the refusal is injected on
/// the path the build worker uses (`levelsRefused`) for the deeper depths the viewer asks for;
/// the coarsest buildable depth is real and the builder serves it.
#[test]
fn a_refused_depth_stops_being_wanted_and_the_view_settles() {
    let mut pair = Pair::with_harness(close_pair_leaf());
    pair.target_px = 4.0;
    let c = SIDE / 2.0;
    pair.view = synth_view([c + 4.0 * SIDE, c + 0.5 * SIDE, c + 1.0 * SIDE], [c; 3]);
    pair.target.eye = pair.view.camera().position();
    pair.send_round(0, true);
    pair.pump();
    let first = pair
        .viewer
        .take_level_requests()
        .expect("the view wants a depth the chain does not hold");
    assert!(!first.is_empty());
    assert!(
        pair.rest().is_none(),
        "the wanted levels keep the view open"
    );
    // The builder is never told of these requests: the viewer hears only the refusals, each of
    // which lowers what the node can want (the first set is the finest wanted depth).
    let mut refused = first.clone();
    let mut rounds = 0;
    let served = loop {
        pair.viewer.refuse_levels(&refused);
        pair.pump();
        let Some(next) = pair.viewer.take_level_requests() else {
            break None;
        };
        rounds += 1;
        // Stop injecting after three rounds or once nothing more is wanted; what is left is served.
        if next.is_empty() || rounds == 3 {
            break Some(next);
        }
        refused = next;
    };
    let served = served.expect("the wanted set changed with every refusal");
    assert!(
        served.iter().all(|request| !first.contains(request)),
        "a refused depth is still listed: {served:?}"
    );
    // What is still wanted is buildable: the builder serves it and the view settles.
    pair.builder.level_requests(&mut pair.h.dataset, &served);
    pair.send(0, true);
    pair.pump();
    let listed = pair.viewer.take_level_requests().unwrap_or_default();
    assert!(
        listed.iter().all(|request| !first.contains(request)),
        "a refused depth is listed again: {listed:?}"
    );
    assert!(listed.is_empty(), "still wanted: {listed:?}");
    assert!(
        pair.rest().is_some(),
        "the view never settled (levels still wanted or work held)"
    );
    assert!(!pair.viewer.cells_busy(&pair.target), "a node stays held");
}

/// The producer side of a refusal: a request for a depth `node_cell_level` rejects
/// (`AbsoluteDepthExceeded`) leaves the builder as `BuilderOutput.refused` on the next plan, once.
#[test]
fn an_unservable_request_is_reported_as_refused_on_the_next_update() {
    let mut pair = Pair::with_harness(close_pair_leaf());
    let leaf = NodeId::new(1);
    pair.send_round(0, false);
    // Two distinct positions closer than a depth-63 cell, next to the origin where f32
    // resolves them: in range, but merged at the deepest depth.
    let block = BlockId::new(101);
    let at = |value: f32| [value; 3];
    let merged = PointBlock::new(
        block,
        PointBatch::new(vec![
            Point::new(at(1.0e-19), ColorRgb::new(1, 2, 3)),
            Point::new(at(2.0e-19), ColorRgb::new(4, 5, 6)),
        ]),
        Vec::new(),
    );
    pair.h.dataset.cache.remove(block);
    pair.h.dataset.insert_block(merged).unwrap();
    let depth = viewer_render_wgpu::MAX_CELL_DEPTH;
    pair.builder
        .level_requests(&mut pair.h.dataset, &[(leaf, depth)]);
    pair.builder.mark_dirty();
    let first = pair.plan();
    assert_eq!(first.refused, [(leaf, depth)]);
    assert!(first.levels.is_empty());
    // Reported once: later updates do not repeat it (a request asked again is answered again,
    // without a rebuild).
    pair.builder.mark_dirty();
    let second = pair.plan();
    assert!(second.refused.is_empty() && second.levels.is_empty());
}

/// A request for a depth so deep that the leaf's cells would take a page each (what a strong zoom
/// asked for, and trapped the build worker out of memory when packed) is refused, not built: the
/// refusal reaches the render side like any unservable depth, which then stops wanting it.
#[test]
fn a_level_too_sparse_to_page_is_refused_not_built() {
    let mut pair = Pair::with_harness(close_pair_leaf());
    let leaf = NodeId::new(1);
    pair.send_round(0, false);
    let block = BlockId::new(101);
    let mut state = 12_345_u64;
    let mut unit = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 40) as f32) / ((1_u64 << 24) as f32)
    };
    let points = (0..3_000)
        .map(|_| {
            let at = [0; 3].map(|_| 1.0 + unit() * (SIDE as f32 - 2.0));
            Point::new(at, ColorRgb::new(1, 2, 3))
        })
        .collect();
    pair.h.dataset.cache.remove(block);
    pair.h
        .dataset
        .insert_block(PointBlock::new(block, PointBatch::new(points), Vec::new()))
        .unwrap();
    pair.builder
        .level_requests(&mut pair.h.dataset, &[(leaf, 40)]);
    pair.builder.mark_dirty();
    let out = pair.plan();
    assert!(out.levels.is_empty(), "the sparse level was built");
    // The asked depth, then the first sparse one: the render side's maximum drops there at once.
    assert_eq!(out.refused[0], (leaf, 40));
    let [(_, first_sparse)] = out.refused[1..] else {
        panic!("expected the first sparse depth: {:?}", out.refused);
    };
    assert!((10..40).contains(&first_sparse), "{first_sparse}");
}

/// Negative control: this is the #35 bug, a leaf drawing its fine levels from far.
#[test]
fn without_on_demand_builds_a_close_pair_leaf_draws_too_fine() {
    let run = close_pair_run(0);
    assert!(
        run.shipped_levels.is_empty(),
        "no on-demand level was built"
    );
    for (octant, depth) in run.drawn.iter().enumerate() {
        let depth = depth.expect("the pruned chain still draws");
        assert!(
            depth > run.expected[octant],
            "octant {octant}: drawn {depth} is finer than {}",
            run.expected[octant]
        );
    }
}

#[test]
fn a_camera_move_withdraws_the_digest_at_rest_until_the_new_views_cut_is_applied() {
    let mut pair = Pair::new();
    pair.send(0, true);
    pair.pump();
    let digest = pair.rest().expect("the first view is at rest");

    // The camera moves: the viewer redraws, but the cut it holds is the old view's.
    pair.viewer.camera_changed(1);
    for _ in 0..3 {
        pair.viewer.frame(&mut pair.target, &pair.view).unwrap();
        assert_eq!(pair.rest(), None, "the old view's digest was published");
    }
    assert_eq!(pair.viewer.stats(&pair.target).view_epoch, 1);

    // A cut planned for an older epoch does not bring it back either.
    pair.send(0, true);
    pair.pump();
    assert_eq!(
        pair.rest(),
        None,
        "a cut of view epoch 0 counted for epoch 1"
    );

    pair.send(1, true);
    pair.pump();
    assert_eq!(pair.rest(), Some(digest));
}

#[test]
fn a_new_view_epoch_makes_the_builder_plan_again_even_with_the_same_camera() {
    let mut pair = Pair::new();
    let camera = pair.view.camera();
    let scale = pair.h.dataset.visual_scale();
    let limits = VoxelBuildLimits::default();
    let begin = |pair: &mut Pair| {
        pair.builder
            .begin_update(&pair.h.dataset, camera, [800, 600], scale, 2.0, limits)
            .unwrap();
    };
    pair.builder.set_view_epoch(4);
    begin(&mut pair);
    pair.send(4, false);
    assert!(!pair.builder.is_dirty());
    begin(&mut pair);
    assert!(
        !pair.builder.is_dirty(),
        "same camera and epoch: nothing to plan"
    );
    pair.builder.set_view_epoch(5);
    begin(&mut pair);
    assert!(pair.builder.is_dirty(), "a new view epoch needs a new plan");
}

#[test]
fn shader_colour_modes_keep_the_cells_and_the_pool() {
    use viewer_render_wgpu::ColorMode;
    let mut pair = Pair::new();
    pair.send(0, true);
    pair.pump();
    let digest = pair.rest().expect("at rest");
    let _ = pair.viewer.take_messages();
    // The renderer's colour mode: what the uniform holds. The bindings' `setColorMode` takes the
    // same decision (`shader_mode_only`) and, when it is true, only sets this.
    let generation = pair.h.dataset.cell_generation();
    for mode in [
        ColorMode::Intensity,
        ColorMode::Elevation,
        ColorMode::Classification,
        ColorMode::Rgb,
    ] {
        // An attribute on either side is a rebuild (new colour generation), never a uniform write.
        assert!(!crate::cells_glue::shader_mode_only(true, true, false));
        assert!(!crate::cells_glue::shader_mode_only(true, false, true));
        assert!(crate::cells_glue::shader_mode_only(true, false, false));
        assert!(!crate::cells_glue::shader_mode_only(false, false, false));
        let built = pair.send(0, true);
        assert!(built.is_empty(), "{mode:?} rebuilt {} nodes", built.len());
        for _ in 0..5 {
            pair.viewer.frame(&mut pair.target, &pair.view).unwrap();
            assert!(
                !pair.viewer.take_messages().pool_reset,
                "{mode:?} reset the pool"
            );
        }
        assert_eq!(pair.rest(), Some(digest));
        assert_eq!(pair.h.dataset.cell_generation(), generation, "no recolour");
    }
}

#[test]
fn a_malformed_node_is_refused_at_the_message_and_leaves_the_queue_alone() {
    let mut pair = Pair::new();
    let nodes = pair.send(0, false);
    let (generation, good) = nodes.first().cloned().expect("a node was built");
    assert!(pair.viewer.push_node(generation, vec![1, 2, 3]).is_err());
    assert!(
        pair.viewer
            .push_node(generation, good[..good.len() / 2].to_vec())
            .is_err()
    );
    let stats = pair.viewer.stats(&pair.target);
    assert_eq!((stats.queued_nodes, stats.queued_bytes), (0, 0));
    pair.viewer.push_node(generation, good).unwrap();
    assert_eq!(pair.viewer.stats(&pair.target).queued_nodes, 1);
}

fn glue_real_case(
    name: &str,
    orbit: bool,
    budget_percent: u64,
    seeds: &[u64],
    inject: Inject,
    mode: LodMode,
) {
    let open = || open_real("belvedere-v7.oct", 4.0, 256 << 20).map(|real| real.harness);
    let Some(real) = open_real("belvedere-v7.oct", 4.0, 256 << 20) else {
        return;
    };
    let views = real_views(&real, orbit);
    let mut unlimited = real.harness.in_mode(mode);
    unlimited.driver = driver_for(&unlimited.dataset, u64::MAX);
    for view in &views {
        unlimited.frame(view);
    }
    unlimited.settle(views.last().unwrap(), 3, 20_000);
    let budget = unlimited.driver.cpu_bytes() * budget_percent / 100;
    let expected = in_process_digest(open().unwrap().in_mode(mode), budget, &views);
    for seed in seeds {
        let mut sim = GlueSim::new(open().unwrap(), budget, *seed, 4 << 20).in_mode(mode);
        let digest = sim.run(&views, inject, 400_000);
        assert_eq!(
            digest, expected,
            "belvedere glue {name} seed {seed} {mode:?}"
        );
    }
}

/// Like `glue_both_modes!`, for the real-data runs (ignored: minutes each).
macro_rules! real_glue_both_modes {
    ($body:ident, $octant:ident, $cell:ident) => {
        #[test]
        #[ignore = "minutes on real data: R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile perf -- --ignored real_glue"]
        fn $octant() {
            $body(LodMode::Octant);
        }

        #[test]
        #[ignore = "minutes on real data: R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile perf -- --ignored real_glue"]
        fn $cell() {
            $body(LodMode::Cell);
        }
    };
}

fn real_glue_orbit_in(mode: LodMode) {
    glue_real_case("orbit", true, 50, &[1, 7], Inject::default(), mode);
}

real_glue_both_modes!(
    real_glue_orbit_in,
    real_glue_belvedere_orbit_reaches_the_in_process_digest,
    real_glue_belvedere_orbit_reaches_the_in_process_digest_in_cell_mode
);

fn real_glue_dolly_in(mode: LodMode) {
    glue_real_case("dolly", false, 90, &[3], Inject::default(), mode);
}

real_glue_both_modes!(
    real_glue_dolly_in,
    real_glue_belvedere_dolly_reaches_the_in_process_digest,
    real_glue_belvedere_dolly_reaches_the_in_process_digest_in_cell_mode
);

fn real_glue_inject_in(mode: LodMode) {
    let inject = Inject {
        pool_reset_at: Some(4),
        driver_replaced_at: Some(8),
        forget_zones_on_loss: true,
        ..Inject::default()
    };
    glue_real_case("orbit+inject", true, 50, &[5], inject, mode);
}

real_glue_both_modes!(
    real_glue_inject_in,
    real_glue_belvedere_pool_reset_and_driver_replaced_mid_load_converge,
    real_glue_belvedere_pool_reset_and_driver_replaced_mid_load_converge_in_cell_mode
);

#[test]
fn recolouring_across_the_build_render_split_leaves_no_hole_and_drops_late_old_nodes() {
    let paint = Rc::new(std::cell::Cell::new(false));
    let mut pair = Pair::with_harness(painting_harness(Rc::clone(&paint)));
    let rgb_nodes = pair.send_checked(0);
    let rgb_digest = pair.rest().expect("rgb at rest");
    let rgb_colors = pair.target.color_digest();
    let rgb_generation = rgb_nodes[0].0;
    assert!(!rgb_nodes.is_empty());

    let mut epoch = 0;
    for round in 0..2 {
        // rgb -> attribute: the builder and the viewer both keep going while it rebuilds.
        paint.set(true);
        assert!(pair.h.dataset.set_cell_paint(Some((0, 0))));
        pair.builder.recolored();
        let painted = pair.send_checked(epoch);
        assert!(!painted.is_empty(), "round {round}: nothing was rebuilt");
        assert!(
            painted
                .iter()
                .all(|(generation, _)| *generation > rgb_generation)
        );
        pair.pump();
        let painted_digest = pair.rest().expect("the attribute image at rest");
        let painted_colors = pair.target.color_digest();
        assert_ne!(painted_colors, rgb_colors, "round {round}");
        assert_eq!(painted_digest, rgb_digest, "same cut, other colours");

        // A late message of the previous generation (a slow worker) is dropped: the image does
        // not go back.
        let bytes = pair.viewer.stats(&pair.target).queued_bytes;
        assert_eq!(bytes, 0);
        for (generation, node) in &rgb_nodes {
            pair.viewer.push_node(*generation, node.clone()).unwrap();
        }
        epoch += 1;
        pair.viewer.camera_changed(epoch);
        pair.pump();
        assert_eq!(pair.target.color_digest(), painted_colors, "round {round}");

        // attribute -> rgb: the rgb image comes back exactly.
        paint.set(false);
        assert!(pair.h.dataset.set_cell_paint(None));
        pair.builder.recolored();
        pair.send_checked(epoch);
        assert_eq!(pair.target.color_digest(), rgb_colors, "round {round}");
        assert_eq!(pair.rest(), Some(rgb_digest));
    }
}

/// Every `(drawn depth, wanted depth)` of the frame, over all octants.
fn depths_of(report: &viewer_render_wgpu::FrameReport) -> Vec<(u8, u8)> {
    report
        .plan
        .entries
        .iter()
        .flat_map(|entry| (0..8).filter_map(|o| Some((entry.drawn[o]?, entry.wanted[o]?))))
        .collect()
}

/// Frames until the viewer is quiet; the last report that drew, checked like every frame.
fn settle_report(pair: &mut Pair) -> viewer_render_wgpu::FrameReport {
    let mut last = None;
    for _ in 0..500 {
        if let Some(report) = pair.viewer.frame(&mut pair.target, &pair.view).unwrap() {
            pair.target.check_plan(&report.plan);
            last = Some(report);
        }
        if !pair.viewer.cells_busy(&pair.target) {
            break;
        }
    }
    last.expect("a frame drew")
}

#[test]
fn continuity_limits_the_drawn_level_across_the_workers_without_rebuilding_or_holes() {
    use crate::cells_glue::Continuity;
    let probe = synthetic_harness(1 << 20, 2.0);
    let root_side = probe.dataset.cell_grid().root_side;
    // Cells of the minimum side are those of depth 3 (the synthetic pyramids reach deeper).
    let spacing = root_side / 8.0;
    let mut pair = Pair::with_harness(synthetic_harness_spaced(
        1 << 20,
        2.0,
        64 << 20,
        Some(spacing),
    ));
    pair.viewer = ViewerCells::new(
        driver_config(
            pair.h.dataset.cell_grid(),
            pair.h.dataset.coordinate_origin(),
            u64::MAX,
        ),
        1 << 20,
    )
    .with_reference_spacing(pair.h.dataset.reference_spacing());
    let sent = pair.send_checked(0);
    let stored: std::collections::HashMap<NodeId, (Vec<u8>, (u8, u8))> = sent
        .iter()
        .map(|(_, bytes)| {
            let node = decode_node_levels(bytes).unwrap();
            (
                node.id,
                (
                    node.levels.iter().map(|level| level.depth).collect(),
                    node.buildable,
                ),
            )
        })
        .collect();
    pair.pump();
    let literal_digest = pair.rest().expect("literal view at rest");
    // A frame at rest draws nothing; setting the literal mode again asks for one.
    pair.viewer.set_continuity(Continuity::default()).unwrap();
    let literal = settle_report(&mut pair);
    let finest_literal = depths_of(&literal).iter().map(|d| d.0).max().unwrap_or(0);
    assert!(
        finest_literal > 3,
        "the fixture must draw finer than the minimum"
    );
    let spacing_before = pair.h.dataset.reference_spacing();

    let received = pair.viewer.stats(&pair.target).nodes_received_total;
    pair.viewer
        .set_continuity(Continuity::new(true, None).unwrap())
        .unwrap();
    // Toggling sends and builds nothing: the same cut, the same cells.
    let built = pair.send_checked(0);
    assert!(built.is_empty(), "toggling rebuilt {} nodes", built.len());
    pair.viewer
        .set_continuity(Continuity::new(true, None).unwrap())
        .unwrap();
    let limited = settle_report(&mut pair);
    let stats = pair.viewer.stats(&pair.target);
    assert!(stats.continuity);
    assert_eq!(stats.continuity_min_side, Some(spacing));
    // No cell below the minimum side (depth 3), except in a node whose coarsest level is finer.
    let mut capped = 0;
    for entry in &limited.plan.entries {
        let (levels, buildable) = &stored[&entry.node];
        let allowed = levels
            .iter()
            .copied()
            .rfind(|depth| *depth <= 3)
            .unwrap_or(levels[0]);
        // The wanted depth may be any buildable one, stored or not.
        let allowed_wanted = (buildable.0..=buildable.1)
            .rfind(|depth| *depth <= 3)
            .unwrap_or(buildable.0);
        for o in 0..8 {
            let (Some(drawn), Some(wanted)) = (entry.drawn[o], entry.wanted[o]) else {
                continue;
            };
            assert!(drawn <= allowed, "node {:?} drew depth {drawn}", entry.node);
            assert!(wanted <= allowed_wanted, "the cap is the wanted look");
            capped += 1;
        }
    }
    assert!(capped > 0);
    assert_eq!(
        pair.viewer.stats(&pair.target).nodes_received_total,
        received
    );
    assert_eq!(pair.h.dataset.reference_spacing(), spacing_before);

    // A factor that overflows the minimum is refused and the mode stays as it was.
    let huge = Continuity::new(true, Some(f64::MAX)).unwrap();
    assert!(pair.viewer.set_continuity(huge).is_err());
    assert_eq!(
        pair.viewer.stats(&pair.target).continuity_min_side,
        Some(spacing)
    );

    // Off again: literal mode, the digest of the first pass.
    pair.viewer.set_continuity(Continuity::default()).unwrap();
    assert_eq!(pair.viewer.stats(&pair.target).continuity_min_side, None);
    pair.pump();
    assert_eq!(pair.rest(), Some(literal_digest));
}

#[test]
fn the_continuity_minimum_is_the_factor_times_the_reference_spacing_and_off_by_default() {
    use crate::cells_glue::Continuity;
    assert_eq!(Continuity::default().min_side(Some(0.25)), None);
    let on = Continuity::new(true, None).unwrap();
    assert_eq!(on.min_side(Some(0.25)), Some(0.25));
    assert_eq!(
        on.min_side(None),
        None,
        "a cloud without spacing stays literal"
    );
    let wide = Continuity::new(true, Some(2.0)).unwrap();
    assert_eq!(wide.min_side(Some(0.25)), Some(0.5));
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(Continuity::new(true, Some(bad)).is_err());
    }
    // A finite factor whose product overflows is an error, not a silent literal mode.
    let huge = Continuity::new(true, Some(f64::MAX)).unwrap();
    assert!(huge.checked_min_side(Some(4.0)).is_err());
    assert_eq!(huge.checked_min_side(None), Ok(None));
    assert_eq!(on.checked_min_side(Some(0.25)), Ok(Some(0.25)));
    assert_eq!(
        Continuity::default().checked_min_side(Some(f64::MAX)),
        Ok(None)
    );
}
