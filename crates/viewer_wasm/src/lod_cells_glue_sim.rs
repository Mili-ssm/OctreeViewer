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
            inflight_delay: 0,
            view_gen: 1,
            build_seen: 0,
            digest: 0,
            rest: None,
            rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
            stats: GlueStats::default(),
        }
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
        viewer.camera_changed(self.view_gen);
        viewer
    }

    fn build_step(&mut self, view: &ViewProjection) {
        if self.inflight_delay > 0 {
            self.inflight_delay -= 1;
        } else if !self.inflight.is_empty() {
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
        if !self.builder.is_active() {
            return;
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
                    self.stats.driver_replacements += 1;
                }
                self.step(view);
                let _ = at;
            }
        }
        self.settle(views.last().unwrap(), max_iterations)
    }

    /// R3-Q23: a colour mode other than RGB, then back. The CPU scene upload frees the pool.
    fn set_cells_active(&mut self, active: bool) {
        self.builder.set_active(&mut self.h.dataset, active);
        self.viewer.set_active(active);
        if !active {
            self.h.target.drop_pool_for_test();
        }
    }
}

impl StrictTarget {
    fn drop_pool_for_test(&mut self) {
        use viewer_render_wgpu::CellTarget;
        self.drop_pool();
    }
}

const GLUE_SEEDS: [u64; 3] = [1, 7, 42];

fn glue_synthetic_case(name: &str, views: Vec<ViewProjection>, inject: Inject) {
    let full = synthetic_cpu_bytes(&views);
    let budget = full / 3;
    let expected = in_process_digest(synthetic_harness(1 << 20, 2.0), budget, &views);
    for (i, seed) in GLUE_SEEDS.iter().enumerate() {
        // A cap of one byte applies one node per frame; the other seeds apply bursts.
        let cap = if i == 0 { 1 } else { 256 << 10 };
        let mut sim = GlueSim::new(synthetic_harness(1 << 20, 2.0), budget, *seed, cap);
        let digest = sim.run(&views, inject, 200_000);
        println!("glue {name} seed {seed}: {:?}", sim.stats);
        assert_eq!(digest, expected, "{name} seed {seed}");
        assert!(sim.stats.node_messages > 0);
        if inject.driver_replaced_at.is_some() {
            assert!(
                sim.stats.missing_reports > 0,
                "the missing path was not exercised: {:?}",
                sim.stats
            );
        }
    }
}

#[test]
fn glue_synthetic_orbit_reaches_the_in_process_digest() {
    glue_synthetic_case("orbit", synth_orbit(), Inject::default());
}

#[test]
fn glue_synthetic_dolly_reaches_the_in_process_digest() {
    glue_synthetic_case("dolly", synth_dolly(), Inject::default());
}

#[test]
fn glue_synthetic_pool_reset_and_driver_replaced_mid_load_converge() {
    let inject = Inject {
        pool_reset_at: Some(5),
        driver_replaced_at: Some(10),
        forget_zones_on_loss: true,
        ..Inject::default()
    };
    glue_synthetic_case("orbit+inject", synth_orbit(), inject);
}

#[test]
fn the_viewer_stats_report_the_cut_apply_and_the_nodes_forgotten_for_the_cpu_budget() {
    use viewer_render_wgpu::CutApply;
    let views = synth_orbit();
    let budget = synthetic_cpu_bytes(&views) / 3;
    let mut sim = GlueSim::new(synthetic_harness(1 << 20, 2.0), budget, 7, 256 << 10);
    let before = sim.viewer.stats(&sim.h.target);
    assert_eq!((before.cut_apply, before.forgotten_total), (None, 0));
    sim.run(&views, Inject::default(), 200_000);
    let after = sim.viewer.stats(&sim.h.target);
    assert_eq!(after.cut_apply, Some(CutApply::Applied));
    assert!(
        after.forgotten_total > 0,
        "a third of the working set forgot nothing"
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
fn colour_mode_off_and_back_resumes_cells_without_a_reset_loop() {
    let views = synth_orbit();
    let budget = synthetic_cpu_bytes(&views);
    let expected = in_process_digest(synthetic_harness(1 << 20, 2.0), budget, &views);
    let mut sim = GlueSim::new(synthetic_harness(1 << 20, 2.0), budget, 3, 1 << 20);
    let last = views.last().unwrap();
    assert_eq!(sim.run(&views, Inject::default(), 200_000), expected);
    let epoch = sim.h.dataset.cell_cut_epoch();

    sim.set_cells_active(false);
    for _ in 0..200 {
        sim.step(last);
    }
    assert_eq!(
        sim.h.dataset.cell_cut_epoch(),
        epoch,
        "cells ran in CPU mode"
    );
    assert!(!sim.viewer.cells_busy(&sim.h.target));
    assert_eq!(sim.viewer.cut_digest_at_rest(&sim.h.target), None);

    sim.set_cells_active(true);
    let resets_before = sim.stats.pool_resets_seen;
    assert_eq!(
        sim.settle(last, 200_000),
        expected,
        "digest after the switch"
    );
    assert!(
        sim.stats.pool_resets_seen <= resets_before + 1,
        "reset loop: {:?}",
        sim.stats
    );
    let (epoch, resets) = (sim.h.dataset.cell_cut_epoch(), sim.stats.pool_resets_seen);
    for _ in 0..300 {
        sim.step(last);
    }
    assert_eq!(
        sim.h.dataset.cell_cut_epoch(),
        epoch,
        "the builder kept planning"
    );
    assert_eq!(
        sim.stats.pool_resets_seen, resets,
        "the pool kept resetting"
    );
}

#[test]
fn the_builder_emits_no_scene_in_cells_mode_and_scenes_again_in_cpu_mode() {
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
    let step = |core: &mut SceneBuilderCore<()>, cells: &mut BuilderCells| {
        core.update_with_cells(
            Some(cells),
            camera,
            [800, 600],
            scale,
            2.0,
            |_: &mut (), _| Ok(None),
            |_: &mut (), _| Err("no pages".to_owned()),
        )
        .unwrap()
    };
    assert!(matches!(step(&mut core, &mut cells), BuilderStep::Cells(_)));
    cells.set_active(core.session_mut().dataset_mut(), false);
    assert!(matches!(step(&mut core, &mut cells), BuilderStep::Scene(_)));
    cells.set_active(core.session_mut().dataset_mut(), true);
    assert!(matches!(step(&mut core, &mut cells), BuilderStep::Cells(_)));
}

#[test]
fn the_in_process_glue_reaches_the_in_process_digest() {
    let views = synth_orbit();
    let budget = synthetic_cpu_bytes(&views);
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

/// One builder and one viewer driven by hand (no simulated latency): `send` plans until the
/// builder is idle and hands everything to the viewer, `pump` runs viewer frames until it is
/// quiet.
struct Pair {
    h: Harness,
    builder: BuilderCells,
    viewer: ViewerCells,
    target: StrictTarget,
    view: ViewProjection,
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
        }
    }

    fn plan(&mut self) -> crate::cells_glue::BuilderOutput {
        let (blocks, pages) = (&mut self.h.blocks, &mut self.h.pages);
        self.builder
            .plan(
                &mut self.h.dataset,
                &self.view,
                2.0,
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
        self.builder.set_view_epoch(view_epoch);
        self.builder.mark_dirty();
        let mut nodes = Vec::new();
        for _ in 0..500 {
            let out = self.plan();
            for node in &out.nodes {
                nodes.push((out.generation, node.bytes.clone()));
                if deliver {
                    self.viewer
                        .push_node(out.generation, node.bytes.clone())
                        .unwrap();
                }
            }
            if out.idle {
                assert!(out.view_epoch == view_epoch, "the plan lost the view epoch");
                if deliver {
                    self.viewer.push_cut(
                        out.cut_epoch,
                        out.view_epoch,
                        out.cut,
                        out.support,
                        true,
                        2.0,
                    );
                }
                return nodes;
            }
        }
        panic!("the builder never went idle");
    }

    /// [`Pair::send`] with a viewer frame after every plan round and the strict target's and
    /// the hole oracle's checks on every frame: the builder keeps planning while the viewer draws.
    fn send_checked(&mut self, view_epoch: u64) -> Vec<(u64, Vec<u8>)> {
        self.builder.set_view_epoch(view_epoch);
        self.builder.mark_dirty();
        let mut nodes = Vec::new();
        for _ in 0..500 {
            let out = self.plan();
            for node in &out.nodes {
                nodes.push((out.generation, node.bytes.clone()));
                self.viewer
                    .push_node(out.generation, node.bytes.clone())
                    .unwrap();
            }
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
            if idle {
                for _ in 0..500 {
                    self.checked_frame();
                    if !self.viewer.cells_busy(&self.target) {
                        return nodes;
                    }
                }
                panic!("the viewer never went quiet");
            }
        }
        panic!("the builder never went idle");
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
            self.viewer.frame(&mut self.target, &self.view).unwrap();
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
fn queued_nodes_wait_while_the_cells_are_off_and_one_pool_reset_follows_the_switch_back() {
    let mut pair = Pair::new();
    let nodes = pair.send(0, true);
    pair.pump();
    let digest = pair.rest().expect("at rest before the switch");
    let _ = pair.viewer.take_messages();

    // The same nodes (a repeat is ignored by the driver) and a new cut wait in the queue.
    for (generation, bytes) in &nodes {
        pair.viewer.push_node(*generation, bytes.clone()).unwrap();
    }
    pair.send(0, true);
    let queued = pair.viewer.stats(&pair.target).queued_nodes;
    assert!(queued > 0);

    // Colour mode off: the CPU scene frees the pool; the driver gets nothing while it is off.
    pair.viewer.set_active(false);
    pair.target.drop_pool_for_test();
    for _ in 0..5 {
        assert!(
            pair.viewer
                .frame(&mut pair.target, &pair.view)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(pair.viewer.stats(&pair.target).queued_nodes, queued);
    assert!(
        pair.viewer.take_messages().is_empty(),
        "an inactive viewer reported to the builder"
    );

    // Back to RGB: the queue drains, the pool is rebuilt once and the cut is the same.
    pair.viewer.set_active(true);
    let mut resets = 0;
    for _ in 0..50 {
        pair.viewer.frame(&mut pair.target, &pair.view).unwrap();
        resets += usize::from(pair.viewer.take_messages().pool_reset);
    }
    assert_eq!(
        resets, 1,
        "the switch back must reset the pool exactly once"
    );
    assert_eq!(pair.viewer.stats(&pair.target).queued_nodes, 0);
    assert_eq!(pair.rest(), Some(digest));
}

#[test]
fn shader_colour_modes_keep_the_cells_and_the_pool() {
    use crate::cells_glue::cells_draw_mode;
    use viewer_render_wgpu::ColorMode;
    let mut pair = Pair::new();
    pair.send(0, true);
    pair.pump();
    let digest = pair.rest().expect("at rest");
    let _ = pair.viewer.take_messages();
    assert!(!cells_draw_mode(false, ColorMode::Intensity, false));
    assert!(!cells_draw_mode(false, ColorMode::Rgb, true));
    assert!(cells_draw_mode(true, ColorMode::Rgb, true));
    // The renderer's colour mode: what the uniform holds. The bindings' `setColorMode` takes the
    // same decision (`shader_mode_only`) and, when it is true, only sets this.
    let generation = pair.h.dataset.cell_generation();
    for mode in [
        ColorMode::Intensity,
        ColorMode::Elevation,
        ColorMode::Classification,
        ColorMode::Rgb,
    ] {
        // With the opt-in off the same change is not a uniform write: the CPU path takes over.
        if mode != ColorMode::Rgb {
            assert!(!crate::cells_glue::shader_mode_only(
                true, false, false, false, mode
            ));
        }
        // An attribute on either side is a rebuild (new colour generation), never a uniform write.
        assert!(!crate::cells_glue::shader_mode_only(
            true, true, false, true, mode
        ));
        assert!(!crate::cells_glue::shader_mode_only(
            true, false, true, true, mode
        ));
        assert!(crate::cells_glue::shader_mode_only(
            true, false, false, true, mode
        ));
        assert!(!crate::cells_glue::shader_mode_only(
            false, false, false, true, mode
        ));
        let uniform_mode = if crate::cells_glue::shader_mode_only(true, false, false, true, mode) {
            mode
        } else {
            ColorMode::Rgb
        };
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
        assert_eq!(uniform_mode, mode);
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

fn glue_real_case(name: &str, orbit: bool, budget_percent: u64, seeds: &[u64], inject: Inject) {
    let open = || open_real("belvedere-v75.oct", 4.0, 256 << 20);
    let Some(real) = open() else { return };
    let views = real_views(&real, orbit);
    let mut unlimited = real.harness;
    unlimited.driver = driver_for(&unlimited.dataset, u64::MAX);
    for view in &views {
        unlimited.frame(view);
    }
    unlimited.settle(views.last().unwrap(), 3, 20_000);
    let budget = unlimited.driver.cpu_bytes() * budget_percent / 100;
    let expected = in_process_digest(open().unwrap().harness, budget, &views);
    for seed in seeds {
        let mut sim = GlueSim::new(open().unwrap().harness, budget, *seed, 4 << 20);
        let digest = sim.run(&views, inject, 400_000);
        println!(
            "belvedere glue {name} seed {seed}: {digest} in-process {expected} {:?}",
            sim.stats
        );
        assert_eq!(digest, expected, "belvedere glue {name} seed {seed}");
    }
}

#[test]
#[ignore = "minutes on real data: R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile perf -- --ignored real_glue"]
fn real_glue_belvedere_orbit_reaches_the_in_process_digest() {
    glue_real_case("orbit", true, 50, &[1, 7], Inject::default());
}

#[test]
#[ignore = "minutes on real data: R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile perf -- --ignored real_glue"]
fn real_glue_belvedere_dolly_reaches_the_in_process_digest() {
    glue_real_case("dolly", false, 90, &[3], Inject::default());
}

#[test]
#[ignore = "minutes on real data: R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile perf -- --ignored real_glue"]
fn real_glue_belvedere_pool_reset_and_driver_replaced_mid_load_converge() {
    let inject = Inject {
        pool_reset_at: Some(4),
        driver_replaced_at: Some(8),
        forget_zones_on_loss: true,
        ..Inject::default()
    };
    glue_real_case("orbit+inject", true, 50, &[5], inject);
}

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
