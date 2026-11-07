//! R3 Task 10a2: the three steps of a cells update run as two simulated workers. The build side
//! (dataset: steps 1 and 3) and the render side (driver and strict target: step 2) talk through
//! the queues of the Task 10 protocol and a seeded random interleaving decides who steps next.
//! At rest the applied cut must be the one the in-process path reaches (`cutDigestAtRest`).
//! Real-data tests (`real_sim_*`, minutes each, so `#[ignore]`) need
//! `target/render-data/belvedere-v75.oct`; run them with
//! `R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --offline --profile perf -- --ignored real_sim`
//! (`R3_REQUIRE_REAL_DATA=1` makes the file mandatory). The `sim_synthetic_*` tests run always.

use super::cells_tests::node_block;
use super::*;
use crate::lod_driver::plan_cells_staged;
use crate::{CutRestProbe, PoolAck, add_cell_nodes, draw_cell_cut};
use std::collections::{HashMap, HashSet, VecDeque};
use viewer_render_wgpu::PackedNode;

/// Stand-in for one second of updates (R3 Task 10a2).
const MAX_UPDATES_ABOVE_TARGET: usize = 60;

/// Build to render, in order. `nodeCells` is never merged; `cellCut` is latest-wins on arrival.
enum Msg {
    Node(u64, Box<PackedNode>),
    /// Epoch, cut and support set of the `cellCut` message.
    Cut(u64, Vec<NodeId>, Vec<NodeId>),
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

#[derive(Clone, Copy, Default)]
struct Inject {
    pool_reset_at: Option<usize>,
    driver_replaced_at: Option<usize>,
    /// The view jumps at this move: the oracles drop the zone memory of the old view.
    camera_jump_at: Option<usize>,
    /// A lost pool or driver draws less than before by definition: until the first settled frame
    /// after the loss, the zone memory of the oracles starts over at every frame (holes and lost
    /// samples are still checked, and so is everything after the window).
    forget_zones_on_loss: bool,
}

#[derive(Default, Debug)]
struct SimStats {
    node_messages: usize,
    missing_reports: usize,
    /// Nodes an ack reported missing (also when a newer ack overwrote it) that were sent again.
    missing_resent: usize,
    evicted: usize,
    pool_resets: usize,
    driver_replacements: usize,
    max_above_target_run: usize,
    /// Updates until the stage first met the target.
    opening_updates: Option<usize>,
    updates: usize,
    parent_violations: usize,
    /// Page evictions that happened while a cut was still unacknowledged.
    evictions_during_ack_lag: usize,
}

struct Sim {
    /// `dataset` is the build side; `driver` and `target` are the render side.
    h: Harness,
    cpu_budget: u64,
    to_render: VecDeque<Msg>,
    cut: Option<(u64, Vec<NodeId>)>,
    /// The planner's nodes (built or not) per cut epoch sent: what the lost-sample oracle asks
    /// about the cut that is drawn, because the build side has usually planned further by then.
    plans: HashMap<u64, Vec<NodeId>>,
    applied: u64,
    digest: u64,
    // Render to build.
    ack: Option<PoolAck>,
    evict: Vec<NodeId>,
    pool_resets: usize,
    /// Build steps left before the build side learns of a `driverReplaced` (the message takes
    /// time; the builder keeps planning for the old driver meanwhile).
    replaced_in: Option<usize>,
    /// Inside the window that follows a loss: the zone oracles forget at every frame.
    loss_window: bool,
    /// The stage met the target since the opening or the last `driverReplaced`: from then on
    /// time above it counts as orbiting. The climb that follows an opening or a replacement
    /// (everything is built again from the coarse rung) is not orbiting.
    counting_above: bool,
    /// Nodes reported missing by a draw and not sent again yet.
    reported_missing: HashSet<NodeId>,
    /// Draws whose epoch had no recorded plan (the oracle would have no planner cut).
    plan_fallbacks: usize,
    probe: CutRestProbe,
    view_gen: u64,
    build_seen: u64,
    render_seen: u64,
    above_run: usize,
    rng: Rng,
    stats: SimStats,
}

impl Sim {
    fn new(mut h: Harness, cpu_budget: u64, seed: u64) -> Self {
        h.driver = driver_for(&h.dataset, cpu_budget);
        h.count_octant_coarsenings = true;
        Self {
            h,
            cpu_budget,
            to_render: VecDeque::new(),
            cut: None,
            plans: HashMap::new(),
            applied: 0,
            digest: 0,
            ack: None,
            evict: Vec::new(),
            pool_resets: 0,
            replaced_in: None,
            loss_window: false,
            plan_fallbacks: 0,
            reported_missing: HashSet::new(),
            counting_above: false,
            probe: CutRestProbe::default(),
            view_gen: 1,
            build_seen: 0,
            render_seen: 0,
            above_run: 0,
            rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
            stats: SimStats::default(),
        }
    }

    fn build_idle(&self) -> bool {
        !self.h.dirty && self.build_seen == self.view_gen
    }

    /// Step 3 then step 1 in the build worker.
    fn build_step(&mut self, view: &ViewProjection) {
        let ds = &mut self.h.dataset;
        let mut wake = false;
        match self.replaced_in {
            Some(0) => {
                ds.driver_replaced();
                self.replaced_in = None;
                self.counting_above = false;
                self.above_run = 0;
                wake = true;
            }
            Some(left) => self.replaced_in = Some(left - 1),
            None => {}
        }
        for _ in 0..std::mem::take(&mut self.pool_resets) {
            ds.pool_reset();
        }
        let evicted = std::mem::take(&mut self.evict);
        wake |= !evicted.is_empty();
        ds.apply_evicted(&evicted);
        if let Some(ack) = self.ack.take() {
            wake |= !ack.missing.is_empty();
            self.stats.missing_reports += ack.missing.len();
            ds.apply_ack(ack);
        }
        if wake {
            self.h.dirty = true;
        }
        if !self.h.dirty && self.build_seen == self.view_gen {
            return;
        }
        self.build_seen = self.view_gen;
        let step = CellStep {
            view,
            target_pixels: self.h.target_px,
            config: self.h.config,
            can_load_blocks: true,
        };
        let planned = plan_cells_staged(
            &mut self.h.dataset,
            &step,
            &mut self.h.stage,
            &mut self.h.dirty,
            None,
            &mut self.h.blocks,
        )
        .unwrap_or_else(|error| panic!("plan: {error}"));
        self.stats.updates += 1;
        // The climb of the opening ladder is not orbiting: runs count once the target was met.
        if planned.stage_pixels > self.h.target_px {
            if self.counting_above {
                self.above_run += 1;
                self.stats.max_above_target_run =
                    self.stats.max_above_target_run.max(self.above_run);
            }
        } else {
            self.stats.opening_updates.get_or_insert(self.stats.updates);
            self.counting_above = true;
            self.above_run = 0;
        }
        let update = planned.update;
        if let Some(plan) = &self.h.dataset.visible_plan {
            self.plans
                .insert(update.cut_epoch, plan.node_ids().to_vec());
        }
        for (generation, node) in update.new_nodes {
            // R3-Q22: a node is never sent before its parents.
            if !self.h.dataset.ancestors_without_cells(node.id).is_empty() {
                self.stats.parent_violations += 1;
            }
            self.stats.node_messages += 1;
            self.to_render
                .push_back(Msg::Node(generation, Box::new(node)));
        }
        self.to_render
            .push_back(Msg::Cut(update.cut_epoch, update.cut, update.support));
        for anchor in update.stats.pending_page_anchors {
            let page = (self.h.pages)(anchor).unwrap_or_else(|e| panic!("page {anchor:?}: {e}"));
            match self.h.dataset.graft_page(page) {
                Ok(()) | Err(LodDatasetError::PageBudget { .. }) => {}
                Err(error) => panic!("graft of {anchor:?}: {error}"),
            }
        }
    }

    /// Receives up to `take` messages, then draws if there is something to draw (step 2).
    fn render_step(&mut self, view: &ViewProjection, take: usize) {
        let mut got = false;
        for _ in 0..take {
            match self.to_render.pop_front() {
                Some(Msg::Node(generation, node)) => {
                    self.stats.missing_resent +=
                        usize::from(self.reported_missing.remove(&node.id));
                    add_cell_nodes(&mut self.h.driver, [(generation, *node)])
                        .unwrap_or_else(|error| panic!("nodeCells: {error}"));
                    got = true;
                }
                Some(Msg::Cut(epoch, cut, support)) => {
                    self.h.driver.cut_received();
                    self.h.driver.set_support(&support);
                    self.cut = Some((epoch, cut));
                    got = true;
                }
                None => break,
            }
        }
        let Some((epoch, cut)) = &self.cut else {
            return;
        };
        if !(got
            || self.h.driver.needs_frame_on(&self.h.target)
            || self.render_seen != self.view_gen)
        {
            return;
        }
        self.render_seen = self.view_gen;
        self.h.target.eye = view.camera().position();
        let draw = draw_cell_cut(
            &mut self.h.driver,
            &mut self.h.target,
            *epoch,
            cut,
            view,
            self.h.target_px,
        )
        .unwrap_or_else(|error| panic!("frame: {error}"));
        self.h.target.check_plan(&draw.report.plan);
        self.digest = cell_cut_digest(&draw.report.plan.entries);
        let number = self.stats.updates;
        self.h.planner_of_drawn_cut = self.plans.get(epoch).cloned();
        self.plan_fallbacks += usize::from(self.h.planner_of_drawn_cut.is_none());
        if self.loss_window {
            self.h.forget_zones();
        }
        self.h
            .check_drawn_cut(number, &draw.report.plan, self.digest);
        self.applied = *epoch;
        self.plans.retain(|e, _| e >= epoch);
        self.stats.evicted += draw.evicted.len();
        self.evict.extend(draw.evicted);
        if draw.pool_reset {
            self.pool_resets += 1;
        }
        // Latest wins, as `poolAck` does: `missing` is recomputed from the cut of every draw, so
        // a node an overwritten ack named comes back in the next one while a cut names it.
        self.reported_missing
            .extend(draw.ack.missing.iter().copied());
        self.ack = Some(draw.ack);
        if self.loss_window && self.settled() {
            self.loss_window = false;
        }
    }

    /// The render side has nothing in flight and drew the newest cut.
    fn settled(&self) -> bool {
        self.to_render.is_empty()
            && !self.needs_frame()
            && self.applied == self.h.dataset.cell_cut_epoch()
    }

    /// Whether the render worker must keep drawing: the driver has work (also after a lost
    /// pool) or messages wait.
    fn needs_frame(&self) -> bool {
        self.h.driver.needs_frame_on(&self.h.target) || !self.to_render.is_empty()
    }

    /// The render worker lost its pool (device lost).
    fn inject_pool_reset(&mut self) {
        use viewer_render_wgpu::CellTarget;
        self.h.target.drop_pool();
        self.h.last_cut = None;
        self.h.last_cut_digest = None;
        self.h.dirty = true;
        self.stats.pool_resets += 1;
    }

    /// The render worker has a new driver and target (another worker).
    fn inject_driver_replaced(&mut self) {
        self.h.driver = driver_for(&self.h.dataset, self.cpu_budget);
        self.h.target = StrictTarget::default();
        self.h.last_cut = None;
        self.h.last_cut_digest = None;
        self.cut = None;
        self.ack = None;
        self.evict.clear();
        self.pool_resets = 0;
        // Messages queued for the old driver die with it (the generation filter); the builder
        // learns of the replacement a few steps later and plans for the old driver until then.
        self.to_render.clear();
        self.replaced_in = Some(1 + self.rng.below(4));
        self.stats.driver_replacements += 1;
    }

    /// Steps along `views` (one view every few iterations) in a seeded interleaving of both
    /// workers, then keeps going on the last one until the digest is published at rest.
    fn run(&mut self, views: &[ViewProjection], inject: Inject, max_iterations: usize) -> u64 {
        let mut at = 0;
        let advance_every = 6;
        let mut moves = 0;
        for iteration in 0..max_iterations {
            if at + 1 < views.len() && iteration % advance_every == 0 {
                at += 1;
                self.view_gen += 1;
                self.h.dirty = true;
                moves += 1;
            }
            if inject.camera_jump_at == Some(moves) && iteration % advance_every == 0 {
                self.h.camera_moved();
            }
            if inject.pool_reset_at == Some(moves) && iteration % advance_every == 1 {
                self.inject_pool_reset();
                self.loss_window |= inject.forget_zones_on_loss;
            }
            if inject.driver_replaced_at == Some(moves) && iteration % advance_every == 3 {
                self.inject_driver_replaced();
                self.loss_window |= inject.forget_zones_on_loss;
            }
            let view = &views[at];
            let evictions_before = self.h.dataset.page_stats().2;
            if self.rng.below(2) == 0 {
                self.build_step(view);
            } else {
                let available = self.to_render.len();
                let take = self.rng.below(available + 1).min(3);
                self.render_step(view, take);
            }
            if self.h.dataset.page_stats().2 > evictions_before
                && self.h.dataset.cell_ack_epoch() < self.h.dataset.cell_cut_epoch()
            {
                self.stats.evictions_during_ack_lag += 1;
            }
            if at + 1 == views.len() {
                let needs = self.needs_frame();
                let idle = self.build_idle();
                if let Some(digest) =
                    self.probe
                        .poll(&self.h.dataset, idle, self.applied, needs, self.digest)
                {
                    return digest;
                }
            }
        }
        panic!(
            "no digest at rest after {max_iterations} iterations: {:?} stage {:?} dirty {} queue {} \
             epoch {} applied {} ack {}",
            self.stats,
            self.h.stage,
            self.h.dirty,
            self.to_render.len(),
            self.h.dataset.cell_cut_epoch(),
            self.applied,
            self.h.dataset.cell_ack_epoch(),
        )
    }
}

/// The in-process run over the same views: a few frames per view, then settle on the last.
fn in_process_digest(mut h: Harness, cpu_budget: u64, views: &[ViewProjection]) -> u64 {
    h.driver = driver_for(&h.dataset, cpu_budget);
    h.count_octant_coarsenings = true;
    for view in views {
        for _ in 0..3 {
            h.frame(view);
        }
    }
    h.settle(views.last().unwrap(), 3, 20_000);
    h.totals.drawn_digest
}

fn check_stats(label: &str, sim: &Sim) {
    println!(
        "{label}: {:?} | zone coarsenings {} lost samples {}",
        sim.stats, sim.h.zone_coarsenings, sim.h.lost_samples
    );
    assert_eq!(
        sim.stats.parent_violations, 0,
        "a node was sent before its parent"
    );
    assert_eq!(
        sim.plan_fallbacks, 0,
        "a drawn cut had no recorded plan for the lost-sample oracle"
    );
    assert!(
        sim.stats.opening_updates.is_some(),
        "the stage never met the target, so the above-target bound proves nothing"
    );
    assert_eq!(sim.h.lost_samples, 0);
    assert_eq!(
        sim.h.zone_coarsenings, 0,
        "a zone was drawn coarser than before"
    );
    assert!(
        sim.stats.max_above_target_run <= MAX_UPDATES_ABOVE_TARGET,
        "the stage stayed above the target for {} updates",
        sim.stats.max_above_target_run
    );
}

// ---------------------------------------------------------------------------------------------
// Synthetic
// ---------------------------------------------------------------------------------------------

fn synth_view(eye: [f64; 3], target: [f64; 3]) -> ViewProjection {
    let mut camera = OrbitCamera::default();
    assert!(camera.fit(
        SceneBounds {
            min: [0.0; 3],
            max: [SIDE; 3],
        },
        [800, 600],
    ));
    assert!(camera.set_pose(eye, target));
    ViewProjection::new(camera, [800, 600], 1.0, VoxelBuildLimits::default())
}

fn synth_orbit() -> Vec<ViewProjection> {
    let c = SIDE / 2.0;
    (0..=24)
        .map(|i| {
            let a = f64::from(i) * std::f64::consts::TAU / 48.0;
            synth_view(
                [
                    c + 0.9 * SIDE * a.cos(),
                    c + 0.9 * SIDE * a.sin(),
                    c + 0.3 * SIDE,
                ],
                [c; 3],
            )
        })
        .collect()
}

fn synth_dolly() -> Vec<ViewProjection> {
    let c = SIDE / 2.0;
    (0..=16)
        .map(|i| {
            let d = 3.0 - 2.4 * f64::from(i) / 16.0;
            synth_view(
                [c + d * SIDE, c + 0.2 * d * SIDE, c + 0.3 * d * SIDE],
                [c; 3],
            )
        })
        .collect()
}

fn synth_sim(seed: u64, budget: u64, pages: usize, px: f64) -> Sim {
    Sim::new(synthetic_harness(pages, px), budget, seed)
}

/// Bytes the in-process run keeps at rest on the synthetic tree.
fn synthetic_cpu_bytes(views: &[ViewProjection]) -> u64 {
    let mut h = synthetic_harness(1 << 20, 2.0);
    for view in views {
        h.frame(view);
    }
    h.settle(views.last().unwrap(), 3, 2_000);
    h.driver.cpu_bytes()
}

fn synthetic_case(name: &str, views: Vec<ViewProjection>, seeds: &[u64], inject: Inject) {
    let full = synthetic_cpu_bytes(&views);
    // Small: a third of what the whole run keeps; idle nodes are forgotten and built again.
    let budget = full / 3;
    let expected = in_process_digest(synthetic_harness(1 << 20, 2.0), budget, &views);
    for seed in seeds {
        let mut sim = synth_sim(*seed, budget, 1 << 20, 2.0);
        let digest = sim.run(&views, inject, 200_000);
        check_stats(&format!("synthetic {name} seed {seed}"), &sim);
        assert_eq!(digest, expected, "{name} seed {seed}");
        assert!(sim.stats.node_messages > 0);
        if inject.driver_replaced_at.is_some() {
            // The builder planned for the old driver until it learned of the replacement: the new
            // one reported the nodes it lacked, and they came back.
            assert!(
                sim.stats.missing_reports > 0 && sim.stats.missing_resent > 0,
                "the missing path was not exercised: {:?}",
                sim.stats
            );
        }
    }
}

const SEEDS: [u64; 4] = [1, 7, 42, 1234];

#[test]
fn sim_synthetic_orbit_reaches_the_in_process_digest() {
    synthetic_case("orbit", synth_orbit(), &SEEDS, Inject::default());
}

#[test]
fn sim_synthetic_dolly_reaches_the_in_process_digest() {
    synthetic_case("dolly", synth_dolly(), &SEEDS, Inject::default());
}

#[test]
fn sim_synthetic_pool_reset_and_driver_replaced_mid_load_converge() {
    let inject = Inject {
        pool_reset_at: Some(5),
        driver_replaced_at: Some(10),
        forget_zones_on_loss: true,
        ..Inject::default()
    };
    synthetic_case("orbit+inject", synth_orbit(), &SEEDS, inject);
    synthetic_case("dolly+inject", synth_dolly(), &SEEDS[..2], inject);
}

#[test]
fn sim_synthetic_with_a_tight_page_budget_leaves_no_hole_while_acks_lag() {
    // One page of 100 bytes at a time (150-byte budget). A and B (the two anchors of the
    // synthetic region) each need their own page; the two together need 200 bytes, so the
    // whole orbit cannot settle on this budget. The view stays on A, jumps to B and the page of
    // A is evicted while the cuts that name its nodes are still unacknowledged.
    let (a, b) = synthetic_views();
    let (a_moves, b_moves) = (8, 8);
    let views: Vec<ViewProjection> = std::iter::repeat_n(a, a_moves)
        .chain(std::iter::repeat_n(b, b_moves))
        .collect();
    let inject = Inject {
        camera_jump_at: Some(a_moves),
        ..Inject::default()
    };
    let expected = {
        let mut h = synthetic_harness(150, 2.0);
        h.driver = driver_for(&h.dataset, u64::MAX);
        h.count_octant_coarsenings = true;
        h.settle(&a, 3, 20_000);
        h.camera_moved();
        h.settle(&b, 3, 20_000);
        h.totals.drawn_digest
    };
    for seed in SEEDS {
        let mut sim = synth_sim(seed, u64::MAX, 150, 2.0);
        let digest = sim.run(&views, inject, 200_000);
        check_stats(&format!("synthetic pages seed {seed}"), &sim);
        assert_eq!(digest, expected, "seed {seed}");
        assert!(sim.h.dataset.page_stats().2 > 0, "no page was evicted");
        assert!(
            sim.stats.evictions_during_ack_lag > 0,
            "no page was evicted while a cut was unacknowledged"
        );
    }
}

#[test]
fn the_lost_sample_oracle_asks_the_plan_of_the_cut_that_is_drawn() {
    // Seed 7 on Belvedere reported 34 lost samples that were not lost: a region the old cut drew
    // through a fallback ancestor left the new cut because it was out of view when that cut was
    // planned, and the oracle asked the newest plan, planned after the view moved back (the
    // build side runs ahead of the drawer). Here the view goes from A to B (the opposite
    // corner) in one thread: the A plan as "planner" would flag the regions A left.
    let (a, b) = synthetic_views();
    let run = |stale: bool| {
        let mut h = synthetic_harness(1 << 20, 2.0);
        h.soft_checks = true;
        h.settle(&a, 3, 600);
        if stale {
            let plan = h.dataset.visible_plan.as_ref().expect("a plan at A");
            h.planner_of_drawn_cut = Some(plan.node_ids().to_vec());
        }
        h.settle(&b, 3, 600);
        h.lost_samples
    };
    assert_eq!(run(false), 0, "the plan of the drawn cut flagged a loss");
    assert!(
        run(true) > 0,
        "the oracle does not see a stale plan covering a dropped region"
    );
}

// ---------------------------------------------------------------------------------------------
// Focused: pins, parents first, the ladder
// ---------------------------------------------------------------------------------------------

#[test]
fn an_unacknowledged_cut_stays_pinned_until_its_ack() {
    let mut h = synthetic_harness(1 << 20, 2.0);
    let view = synthetic_views().0;
    let step = CellStep {
        view: &view,
        target_pixels: 2.0,
        config: h.config,
        can_load_blocks: true,
    };
    let mut planned = None;
    for _ in 0..400 {
        let p = plan_cells_staged(
            &mut h.dataset,
            &step,
            &mut h.stage,
            &mut h.dirty,
            None,
            &mut h.blocks,
        )
        .unwrap();
        if !p.update.cut.is_empty() {
            planned = Some(p.update);
            break;
        }
    }
    let update = planned.expect("a cut with nodes");
    // No ack yet: the pool may be drawing the cut, so its nodes are pinned although the last
    // ack (none) drew nothing.
    assert!(h.dataset.cells.pool_drawn().is_empty());
    let pinned = h.dataset.pinned_nodes();
    for node in &update.cut {
        assert!(
            pinned.contains(node),
            "{node:?} is not pinned before the ack"
        );
    }
    let ack = PoolAck {
        cut_epoch: update.cut_epoch,
        drawn: Vec::new(),
        missing: Vec::new(),
    };
    h.dataset.apply_ack(ack);
    assert!(h.dataset.cells.pool_drawn().is_empty());
}

#[test]
fn a_page_named_only_by_an_unacknowledged_cut_is_not_evicted_during_the_ack_lag() {
    // One page of 100 bytes at a time. The cut drawn at A names a node of page 1; nothing is
    // ever acknowledged. The view then moves to B at a coarse rung that needs no page: the
    // retained cut, the visible plan and the selection no longer name the node, and only the
    // unacknowledged cut of A does. The page of B's anchor must not evict it.
    let mut h = synthetic_harness(150, 2.0);
    let (a, b) = synthetic_views();
    let config = h.config;
    let mut named = None;
    for _ in 0..400 {
        let update = h
            .dataset
            .plan_cell_update(&a, 2.0, config, None, &mut h.blocks)
            .unwrap();
        for anchor in update.stats.pending_page_anchors.clone() {
            let page = (h.pages)(anchor).unwrap();
            let _ = h.dataset.graft_page(page);
        }
        named = update
            .cut
            .iter()
            .find(|n| (10..18).contains(&n.get()))
            .copied();
        if named.is_some() {
            break;
        }
    }
    let node = named.expect("the cut at A names a node of page 1");
    let named_elsewhere = |h: &Harness| {
        let ds = &h.dataset;
        ds.planner.retained_nodes().contains(&node)
            || ds
                .visible_plan
                .as_ref()
                .is_some_and(|plan| plan.node_ids().contains(&node))
            || ds
                .selection_memo
                .as_ref()
                .is_some_and(|memo| memo.selection.node_ids().contains(&node))
    };
    for _ in 0..400 {
        h.dataset
            .plan_cell_update(&b, 16.0, config, None, &mut h.blocks)
            .unwrap();
        if !named_elsewhere(&h) {
            break;
        }
    }
    assert!(!named_elsewhere(&h), "the plan at B still names {node:?}");
    assert!(h.dataset.cells.pool_drawn().is_empty());
    let error = h
        .dataset
        .graft_page((h.pages)(NodeId::new(9)).unwrap())
        .unwrap_err();
    assert!(
        matches!(error, LodDatasetError::PageBudget { .. }),
        "the page of {node:?} was offered for eviction and taken: {error}"
    );
    assert!(h.dataset.metadata.hierarchy().node(node).is_some());
    // Once the last cut is acknowledged (drawing nothing), the page can go.
    h.dataset.apply_ack(PoolAck {
        cut_epoch: h.dataset.cell_cut_epoch(),
        drawn: Vec::new(),
        missing: Vec::new(),
    });
    h.dataset
        .graft_page((h.pages)(NodeId::new(9)).unwrap())
        .expect("no pin is left after the ack");
    assert!(h.dataset.metadata.hierarchy().node(node).is_none());
}

#[test]
fn a_digest_is_not_at_rest_right_after_the_pool_was_lost() {
    let views = synth_orbit();
    let mut sim = synth_sim(7, u64::MAX, 1 << 20, 2.0);
    let digest = sim.run(&views, Inject::default(), 200_000);
    let poll = |sim: &mut Sim| {
        let idle = sim.build_idle();
        let needs = sim.needs_frame();
        sim.probe
            .poll(&sim.h.dataset, idle, sim.applied, needs, sim.digest)
    };
    assert_eq!(poll(&mut sim), Some(digest), "at rest before the loss");
    // The queues are empty and the driver does not know yet: only the target does.
    sim.inject_pool_reset();
    assert!(sim.to_render.is_empty() && sim.h.dataset.cell_ack_epoch() == sim.applied);
    for _ in 0..3 {
        assert_eq!(poll(&mut sim), None, "a lost pool is not a rest");
    }
    let last = views.len() - 1;
    assert_eq!(sim.run(&views[last..], Inject::default(), 200_000), digest);
}

/// A harness whose blocks of nodes 10..=17 (the children of node 2) are held back until the
/// returned flag is set: the cut stays on node 2 and its ancestor while they "load".
fn held_children_harness(
    pages: u32,
    cpu_budget: u64,
) -> (Harness, std::rc::Rc<std::cell::Cell<bool>>) {
    let mut h = synthetic_harness(1 << 20, 2.0);
    h.driver = CellDriver::new(viewer_render_wgpu::DriverConfig {
        pool: viewer_render_wgpu::PoolConfig {
            capacity_pages: pages,
            ..viewer_render_wgpu::PoolConfig::default()
        },
        grid: h.dataset.cell_grid(),
        coordinate_origin: h.dataset.coordinate_origin(),
        cpu_budget_bytes: cpu_budget,
    });
    let release = std::rc::Rc::new(std::cell::Cell::new(false));
    let gate = std::rc::Rc::clone(&release);
    let mut inner = std::mem::replace(&mut h.blocks, Box::new(|_| Ok(None)));
    h.blocks = Box::new(move |id| {
        if (110..118).contains(&id.get()) && !gate.get() {
            return Ok(None);
        }
        inner(id)
    });
    (h, release)
}

#[test]
fn ancestors_built_ahead_of_the_cut_are_not_rebuilt_under_a_tight_cpu_budget() {
    // Pool capacities from a single page up: levels of the ancestors leave the pool, so they are
    // candidates for `forget` while their descendants still wait for blocks.
    let (a, _) = synthetic_views();
    let mut forgotten = 0;
    for pages in 1..=9 {
        let (mut h, release) = held_children_harness(pages, 1);
        for _ in 0..60 {
            h.frame(&a);
        }
        release.set(true);
        h.dirty = true;
        for _ in 0..200 {
            h.frame(&a);
        }
        let rebuilt: Vec<_> = h.totals.builds.iter().filter(|(_, n)| **n > 1).collect();
        assert!(
            rebuilt.is_empty(),
            "pool of {pages} pages: nodes built more than once: {rebuilt:?}"
        );
        assert!(
            h.totals.builds.contains_key(&NodeId::new(10)),
            "the held children were never built"
        );
        forgotten += h.totals.forgotten;
    }
    assert!(
        forgotten > 0,
        "the budget never forced a forget, so nothing was tested"
    );
}

#[test]
fn blocks_in_view_are_read_while_the_cache_barely_holds_the_cut_and_the_chain() {
    // Nine blocks is the least this view settles with: the blocks of the built cut are protected
    // in the cache, and the chain of ancestors (nodes 1 and 2) and the block being read share
    // what is left. Ancestor reads and in-view reads evict each other there.
    let (a, _) = synthetic_views();
    let block_bytes = {
        let h = synthetic_harness(1 << 20, 2.0);
        node_block(&h.dataset, 2).estimated_memory_bytes()
    };
    let mut free = synthetic_harness(1 << 20, 2.0);
    free.settle(&a, 3, 4_000);
    for blocks in [9, 10] {
        let mut h = synthetic_harness_with_cache(1 << 20, 2.0, blocks * block_bytes + 1);
        h.settle(&a, 3, 4_000);
        assert_eq!(
            h.totals.drawn_digest, free.totals.drawn_digest,
            "{blocks} blocks"
        );
        assert!(h.totals.builds.contains_key(&NodeId::new(10)));
    }
}

#[test]
fn a_node_is_never_built_before_its_parent() {
    let mut h = synthetic_harness(1 << 20, 2.0);
    let view = synthetic_views().0;
    let config = CellUpdateConfig { max_node_builds: 1 };
    // Straight at the target rung (no ladder): the selected leaves wait for their ancestors.
    let mut order: Vec<NodeId> = Vec::new();
    for _ in 0..300 {
        let update = h
            .dataset
            .plan_cell_update(&view, 2.0, config, None, &mut h.blocks)
            .unwrap();
        order.extend(update.new_nodes.iter().map(|(_, node)| node.id));
        for anchor in update.stats.pending_page_anchors.clone() {
            let page = (h.pages)(anchor).unwrap();
            let _ = h.dataset.graft_page(page);
        }
        if update.stats.built_nodes.is_empty()
            && update.stats.pending_block_ids.is_empty()
            && update.stats.pending_page_anchors.is_empty()
        {
            break;
        }
    }
    assert!(order.len() > 8, "built {order:?}");
    let hierarchy = h.dataset.metadata.hierarchy();
    for (at, id) in order.iter().enumerate() {
        let mut parent = hierarchy.node(*id).and_then(SpatialNode::parent);
        while let Some(up) = parent {
            let Some(p) = hierarchy.node(up) else { break };
            assert!(
                p.blocks().is_empty() || order[..at].contains(&up),
                "{id:?} was built before its ancestor {up:?}: {order:?}"
            );
            parent = p.parent();
        }
    }
}

#[test]
fn orbiting_inside_a_loaded_region_keeps_the_ladder_and_a_jump_restarts_it() {
    let mut h = synthetic_harness(1 << 20, 2.0);
    let (a, b) = synthetic_views();
    h.settle(&a, 3, 600);
    assert_eq!(h.stage, Some(2.0));
    // A small move inside the loaded region: the nodes in view already have cells.
    let nudged = synth_view([17.0, 16.0, 16.0], [-20.0; 3]);
    h.camera_moved();
    h.frame(&nudged);
    assert_eq!(h.stage, Some(2.0), "an orbit step restarted the ladder");
    // The opposite corner: no node in view has cells or an ancestor with cells... except the
    // root, which every selection descends from, so look where the root is not an ancestor
    // either: a fresh dataset with nothing built.
    let mut fresh = synthetic_harness(1 << 20, 2.0);
    fresh.stage = Some(2.0);
    fresh.dirty = false;
    fresh.frame(&b);
    assert_eq!(
        fresh.totals.stages.first(),
        Some(&16.0),
        "a jump must restart the ladder at the coarse rung"
    );
}

// ---------------------------------------------------------------------------------------------
// Real data
// ---------------------------------------------------------------------------------------------

fn real_views(real: &Real, orbit: bool) -> Vec<ViewProjection> {
    let (centre, side) = real.anchors_near([0.5, 0.5, 0.5])[0];
    (0..=12)
        .map(|i| {
            let t = f64::from(i);
            let offset = if orbit {
                let a = t * std::f64::consts::TAU / 48.0;
                [side * 0.25 * a.sin(), -side * 0.25 * a.cos(), side * 0.12]
            } else {
                let k = 4.0 - 3.0 * t / 12.0;
                [0.0, -side * 0.2 * k, side * 0.12 * k]
            };
            real.view(centre, offset)
        })
        .collect()
}

/// `budget_percent` is the share of the bytes an unlimited run keeps. Orbit: 50%, so idle nodes
/// are forgotten and built again. Dolly: 90%, because the view at the end of the dolly needs
/// about 54.8 MB of live nodes (the unlimited run keeps 53.0 MB before the last frames): at 80%
/// or less the in-process run itself never settles, it forgets and rebuilds two nodes per frame
/// at stage 8 (a finding of R3 Task 10a2: the budget has no hysteresis below the working set).
fn real_case(name: &str, orbit: bool, budget_percent: u64, seeds: &[u64], inject: Inject) {
    let open = || open_real("belvedere-v75.oct", 4.0, 256 << 20);
    let Some(real) = open() else { return };
    let views = real_views(&real, orbit);
    // A fraction of what an unlimited run keeps.
    let mut unlimited = real.harness;
    unlimited.driver = driver_for(&unlimited.dataset, u64::MAX);
    for view in &views {
        unlimited.frame(view);
    }
    unlimited.settle(views.last().unwrap(), 3, 20_000);
    let budget = unlimited.driver.cpu_bytes() * budget_percent / 100;
    println!(
        "belvedere {name}: unlimited run keeps {} bytes, budget {budget}",
        unlimited.driver.cpu_bytes()
    );
    let expected = in_process_digest(open().unwrap().harness, budget, &views);
    for seed in seeds {
        let mut sim = Sim::new(open().unwrap().harness, budget, *seed);
        let digest = sim.run(&views, inject, 400_000);
        check_stats(&format!("belvedere {name} seed {seed}"), &sim);
        println!("belvedere {name} seed {seed}: digest {digest} in-process {expected}");
        assert_eq!(digest, expected, "belvedere {name} seed {seed}");
    }
}

#[test]
#[ignore = "minutes on real data: R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile perf -- --ignored real_sim"]
fn real_sim_belvedere_orbit_reaches_the_in_process_digest() {
    real_case("orbit", true, 50, &[1, 7], Inject::default());
}

#[test]
#[ignore = "minutes on real data: R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile perf -- --ignored real_sim"]
fn real_sim_belvedere_dolly_reaches_the_in_process_digest() {
    real_case("dolly", false, 90, &[3], Inject::default());
}

#[test]
#[ignore = "minutes on real data: R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile perf -- --ignored real_sim"]
fn real_sim_belvedere_pool_reset_and_driver_replaced_mid_load_converge() {
    let inject = Inject {
        pool_reset_at: Some(4),
        driver_replaced_at: Some(8),
        forget_zones_on_loss: true,
        ..Inject::default()
    };
    real_case("orbit+inject", true, 50, &[5], inject);
}
