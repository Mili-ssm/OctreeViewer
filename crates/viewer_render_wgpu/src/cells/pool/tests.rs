//! Task 7a pass criteria: residency is all-or-nothing, uploads are spread over frames, the fit
//! coarsens the farthest nodes first and settles, and no page is rewritten while it can be read.

use std::collections::{HashMap, HashSet};

use viewer_core::NodeId;

use super::hold::{Run, run_covers};
use super::*;
use crate::{
    OrbitCamera,
    cell_lod::{LodView, NodeLod, drawn_depth_at as twin_drawn_depth_at},
    scene_digest_tests::{ORIGIN, SIDE, ids},
    test_support::*,
};

struct Rig {
    pool: CellPool,
    gpu: FakeGpu,
    nodes: HashMap<NodeId, PoolNode>,
    delay: u64,
    /// Loader page counts that differ from the estimate: `shrink` packs one page less in levels
    /// above 1 page; `deep` forces the pages of depth-7 levels.
    shrink: bool,
    deep: Option<usize>,
    inflight: Vec<(u64, LevelKey)>,
    /// Nodes whose data is not delivered while they are here (a slow region).
    held: HashSet<NodeId>,
    /// Levels that never arrive while they are here (a refused or lost request).
    refused: HashSet<LevelKey>,
    frame: u64,
    /// The stepping oracle's wait per `(node, octant)` (see `Rig::check_stepping`).
    waits: HashMap<(NodeId, usize), StepWait>,
    /// `(node, octant)` pairs the cell-mode branch of the stepping oracle checked against a
    /// drawn depth: proves that branch ran.
    cell_checked: usize,
    /// `(node, octant)` pairs with a wanted depth that the cell-mode branch did not check, by
    /// reason: the size of what the oracle leaves unproven.
    cell_skipped: HashMap<&'static str, usize>,
}

impl Rig {
    fn new(
        nodes: HashMap<NodeId, PoolNode>,
        capacity_pages: u32,
        budget_pages: u64,
        delay: u64,
    ) -> Self {
        let mut pool = CellPool::new(PoolConfig {
            capacity_pages,
            upload_budget_bytes: budget_pages * POOL_PAGE_BYTES,
            ..PoolConfig::default()
        });
        // The fake pages sit at the grid corner: the tests that need the cap say so.
        pool.uncapped_wish = true;
        Self {
            pool,
            gpu: FakeGpu::default(),
            nodes,
            delay,
            shrink: false,
            deep: None,
            inflight: Vec::new(),
            held: HashSet::new(),
            refused: HashSet::new(),
            frame: 0,
            waits: HashMap::new(),
            cell_checked: 0,
            cell_skipped: HashMap::new(),
        }
    }

    /// Pages of a level as the loader packs it: the pool's estimate unless overridden.
    fn pages(&self, key: LevelKey) -> usize {
        let node = &self.nodes[&key.node];
        let node_depth = representative_depth(&grid(), node.bounds).saturating_sub(3);
        let estimate = estimate_pages(node.points, key.depth, node_depth);
        match self.deep {
            Some(pages) if key.depth == 7 => pages,
            _ if self.shrink => estimate.saturating_sub(1).max(1),
            _ => estimate,
        }
    }

    fn step(&mut self, cut: &[NodeId], camera: &OrbitCamera, target: f64) -> FramePlan {
        self.step_view(cut, &view(camera), target)
    }

    fn step_view(&mut self, cut: &[NodeId], view: &ViewProjection, target: f64) -> FramePlan {
        self.frame += 1;
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.inflight)
            .into_iter()
            .partition(|(at, _)| *at <= self.frame);
        self.inflight = later;
        for (at, key) in due {
            if self.held.contains(&key.node) || self.refused.contains(&key) {
                self.inflight.push((at, key));
                continue;
            }
            let pages = self.pages(key);
            self.pool.offer(key, fake_level(key, pages));
        }
        let before = self.stepping_before();
        let nodes = &self.nodes;
        let plan = self
            .pool
            .frame(
                cut,
                &|id| nodes.get(&id).cloned(),
                view,
                &grid(),
                ORIGIN,
                // These scenarios were calibrated when the target was a maximum edge; under
                // ADR 0054 (a minimum, cubes between N and 2N px) half the target selects the
                // same depths, so the literals below keep their meaning.
                target / 2.0,
            )
            .expect("frame");
        self.check_stepping(&plan, &before);
        self.gpu.apply(&plan, &self.pool);
        for key in &plan.fetch_requests {
            self.inflight.push((self.frame + self.delay, *key));
        }
        plan
    }

    fn run(
        &mut self,
        frames: usize,
        cut: &[NodeId],
        camera: &OrbitCamera,
        target: f64,
    ) -> FramePlan {
        let mut plan = self.step(cut, camera, target);
        for _ in 1..frames {
            plan = self.step(cut, camera, target);
        }
        plan
    }

    /// What the stepping oracle needs from before a frame is planned: what each octant drew in
    /// the applied cut and the levels resident (the lookup of the cut reads them).
    fn stepping_before(&mut self) -> SteppingBefore {
        if self.pool.applied_drawn.is_empty() {
            self.waits.clear();
        }
        SteppingBefore {
            drawn: self.pool.applied_drawn.clone(),
            resident: self
                .nodes
                .iter()
                .map(|(id, node)| {
                    let levels = node
                        .stored
                        .iter()
                        .copied()
                        .filter(|depth| self.pool.is_resident(self.pool.key(*id, *depth)))
                        .collect();
                    (*id, levels)
                })
                .collect(),
            waits: self.waits.clone(),
            seeds: self
                .nodes
                .iter()
                .filter(|(id, _)| !self.pool.drew_in_applied(**id))
                .map(|(id, node)| (*id, self.pool.entering_seed(node.bounds, &grid())))
                .collect(),
        }
    }

    /// The stepping rule of ADR 0053, restated from its text, against an applied cut: per `(node, octant)` the drawn depth equals the expected stepped depth (one level
    /// towards the wanted one through resident depths, never past it; after `STEP_WAIT_FRAMES`
    /// the resident depth of `prev..wanted` closest to `prev`; a lost `prev` draws the resident
    /// depth closest to it towards the wanted one), and it differs from `prev` by more than one
    /// level only in a hop the pool counts in `step_hops`. The rule runs over the levels inside
    /// the node's span (`in_cell_range`, buildable clamped to `cell_depth_range`); nodes under a
    /// fit cap or held are not checked. Every planned frame, applied or retained, advances the
    /// waits of what is on screen.
    fn check_stepping(&mut self, plan: &FramePlan, before: &SteppingBefore) {
        let (mut hops, mut waits) = (0, HashMap::new());
        for entry in &plan.entries {
            let node = &self.nodes[&entry.node];
            let stored = capped(&node.stored, self.pool.caps.get(&entry.node));
            let mut buildable = capped_range(node.buildable, self.pool.caps.get(&entry.node));
            let node_depth = CellPool::node_depth(&grid(), node.bounds);
            let stored = in_cell_range(&stored, node_depth);
            let (lo, hi) = cell_depth_range(node_depth);
            buildable = (buildable.0.max(lo), buildable.1.min(hi));
            if stored.is_empty() {
                let pairs = entry.wanted.iter().flatten().count();
                *self
                    .cell_skipped
                    .entry("no stored depth in the span")
                    .or_default() += pairs;
                continue;
            }
            let resident: Vec<u8> = stored
                .iter()
                .copied()
                .filter(|d| before.resident[&entry.node].contains(d))
                .collect();
            let exempt =
                self.pool.caps.contains_key(&entry.node) || self.pool.hold.contains(&entry.node);
            let mut prev = before.drawn.get(&entry.node).copied().unwrap_or([None; 8]);
            if let Some(seed) = before.seeds.get(&entry.node) {
                // An entering node starts from the depth its ancestor or descendants drew,
                // clamped to its buildable depths inside the span.
                let (lo, hi) = (buildable.0, buildable.1);
                if lo <= hi {
                    prev = seed.map(|depth| depth.map(|d| d.clamp(lo, hi)));
                }
            }
            for o in 0..8 {
                let Some(wanted) = entry.wanted[o] else {
                    continue;
                };
                let w = served_depth(&stored, wanted);
                let drawn = entry.drawn[o];
                let step = spec_next(drawn, w, buildable, &resident);
                if plan.apply != CutApply::Applied || exempt {
                    let reason = if plan.apply != CutApply::Applied {
                        "retained frame"
                    } else if self.pool.caps.contains_key(&entry.node) {
                        "fit cap"
                    } else {
                        "hold"
                    };
                    *self.cell_skipped.entry(reason).or_default() += 1;
                }
                if plan.apply == CutApply::Applied && !exempt {
                    let waited = before.waits.get(&(entry.node, o)).map_or(0, |(_, f)| *f);
                    let timed_out = waited >= STEP_WAIT_FRAMES;
                    let expected = spec_depth(prev[o], w, buildable, &resident, timed_out);
                    if drawn.is_some() {
                        self.cell_checked += 1;
                    }
                    assert_eq!(
                        drawn, expected,
                        "node {:?} octant {o}: prev {:?} wanted {w} resident {resident:?} \
                         waited {waited}",
                        entry.node, prev[o]
                    );
                    if let (Some(drawn), Some(was)) = (drawn, prev[o]) {
                        assert!(
                            (was.min(w)..=was.max(w)).contains(&drawn) || !resident.contains(&was),
                            "node {:?} octant {o}: {was} -> {drawn} passes the wanted depth {w}",
                            entry.node
                        );
                        if drawn.abs_diff(was) > 1 {
                            assert!(
                                timed_out || !resident.contains(&was),
                                "node {:?} octant {o}: {was} -> {drawn} without a timeout",
                                entry.node
                            );
                            hops += 1;
                        }
                    }
                }
                if let (Some(drawn), Some(step)) = (drawn, step) {
                    let key = (drawn, w > drawn, step);
                    let frames = match before.waits.get(&(entry.node, o)) {
                        Some((waiting, frames)) if *waiting == key => frames + 1,
                        _ => 1,
                    };
                    waits.insert((entry.node, o), (key, frames));
                }
            }
        }
        if plan.apply == CutApply::Applied {
            assert_eq!(plan.counters.step_hops, hops, "counted timeout hops");
        }
        self.waits = waits;
    }

    /// A cell-mode test that stepped this rig must have had the stepping oracle check something:
    /// without it a rig whose pairs are all exempt (caps, holds, retained frames) proves nothing.
    fn assert_stepping_was_checked(&self) {
        assert!(
            self.cell_checked > 0,
            "the cell-mode stepping oracle checked nothing; skipped {:?}",
            self.cell_skipped
        );
    }

    /// Pages owned on the GPU equal the pages of the resident levels and of the partial uploads.
    fn assert_gpu_matches_pool(&self) {
        let resident = self.pool.resident_pages() as usize;
        let partial: usize = self
            .gpu
            .partial()
            .iter()
            .map(|key| self.gpu.owner.values().filter(|(k, _)| k == key).count())
            .sum();
        assert_eq!(self.gpu.owner.len(), resident + partial);
    }
}

/// `Rig::stepping_before`.
struct SteppingBefore {
    drawn: FastMap<NodeId, [Option<u8>; 8]>,
    resident: HashMap<NodeId, Vec<u8>>,
    waits: HashMap<(NodeId, usize), StepWait>,
    /// Cell mode: where a node that drew nothing in the applied cut starts stepping from
    /// (`CellPool::entering_seed`, read before the frame because it depends on the applied cut).
    seeds: HashMap<NodeId, [Option<u8>; 8]>,
}

/// The depths strictly after `from` up to `w`, in the order of the walk from `from` to `w`.
fn walk(from: u8, w: u8) -> Vec<u8> {
    if w >= from {
        (from + 1..=w).collect()
    } else {
        (w..from).rev().collect()
    }
}

/// The depth the octant awaits next from `from`: the first servable one on the walk to `w`, `w`
/// itself when none is; `None` once `from` is `w`.
fn spec_next(from: Option<u8>, w: u8, buildable: (u8, u8), resident: &[u8]) -> Option<u8> {
    let from = from.filter(|from| *from != w)?;
    let servable = |d: &u8| resident.contains(d) || (buildable.0..=buildable.1).contains(d);
    Some(walk(from, w).into_iter().find(servable).unwrap_or(w))
}

/// The depth an octant that drew `prev` draws now (ADR 0053).
fn spec_depth(
    prev: Option<u8>,
    w: u8,
    buildable: (u8, u8),
    resident: &[u8],
    timed_out: bool,
) -> Option<u8> {
    let closest_to_w = || {
        resident
            .iter()
            .copied()
            .min_by_key(|d| (d.abs_diff(w), std::cmp::Reverse(*d)))
    };
    let first_resident = |from: u8| walk(from, w).into_iter().find(|d| resident.contains(d));
    match prev {
        None => closest_to_w(),
        Some(prev) if !resident.contains(&prev) => first_resident(prev).or_else(closest_to_w),
        Some(prev) if prev == w => Some(prev),
        Some(prev) => {
            let next = spec_next(Some(prev), w, buildable, resident).expect("prev != w");
            if resident.contains(&next) {
                Some(next)
            } else if timed_out {
                Some(first_resident(prev).unwrap_or(prev))
            } else {
                Some(prev)
            }
        }
    }
}

fn depth_of(plan: &FramePlan, node: u64) -> [Option<u8>; 8] {
    plan.entries
        .iter()
        .find(|entry| entry.node == NodeId::new(node))
        .expect("entry")
        .drawn
}

/// Explicit centre of the octant node `id` (ids 2..=9) in the frame relative to `ORIGIN`.
fn centre(id: u64) -> [f64; 3] {
    let index = id - 2;
    std::array::from_fn(|axis| SIDE / 2.0 * (((index >> axis) & 1) as f64 + 0.5))
}

/// Node ids ordered from the nearest to the farthest eye-to-centre distance, computed here.
fn by_distance(cam: &OrbitCamera, ids: &[u64]) -> Vec<u64> {
    let v = view(cam);
    let eye = v.camera().position().map(|p| p / v.visual_scale());
    let dist = |id: u64| -> f64 {
        let c = centre(id);
        (0..3).map(|a| (c[a] - eye[a]).powi(2)).sum()
    };
    let mut sorted = ids.to_vec();
    sorted.sort_by(|a, b| dist(*a).total_cmp(&dist(*b)));
    sorted
}

#[test]
fn levels_become_active_only_with_all_pages_and_uploads_span_frames() {
    // One page per frame: a 3-page level needs several frames, and the fake GPU rejects an
    // active level with a missing page.
    let mut rig = Rig::new(octant_nodes(), 128, 1, 1);
    rig.shrink = true;
    let cut = ids(&CHILDREN);
    let cam = camera("zoom");
    let mut spans: HashMap<LevelKey, usize> = HashMap::new();
    let mut first_complete = None;
    let mut max_stages = 0;
    let mut last = None;
    // Budget: one page per frame, and ADR 0053 makes every node load its whole chain 4..=7
    // (8 nodes x (1 + 3 x 3) pages, one less per level with `shrink`) before reaching depth 7.
    for frame in 0..100 {
        let plan = rig.step(&cut, &cam, 4.0);
        if frame == 0 {
            assert_eq!(
                plan.counters.nodes_without_level, 8,
                "nothing is resident yet"
            );
        }
        assert!(
            plan.counters.uploaded_bytes <= POOL_PAGE_BYTES,
            "frame {frame}"
        );
        for upload in &plan.uploads {
            *spans.entry(upload.key).or_default() += 1;
        }
        max_stages = max_stages.max(plan.counters.stages);
        if plan.counters.nodes_without_level == 0 && first_complete.is_none() {
            first_complete = Some(frame);
        }
        if first_complete.is_some() {
            assert_eq!(plan.counters.nodes_without_level, 0, "frame {frame}");
        }
        last = Some(plan);
    }
    assert!(first_complete.is_some(), "never covered every node");
    assert!(
        max_stages > 0,
        "coarse levels draw as stages before the fine ones arrive"
    );
    assert!(
        spans.values().any(|n| *n > 1),
        "no level spanned two frames"
    );
    let last = last.unwrap();
    assert_eq!(last.apply, CutApply::Applied);
    assert_eq!(last.counters.stages, 0);
    assert_eq!(last.counters.pool_overflow, 0);
    assert!(
        last.active
            .iter()
            .all(|a| a.key.depth == 7 || a.key.depth == 4)
    );
    rig.assert_gpu_matches_pool();
}

#[test]
fn uploads_give_every_node_its_first_level_before_refining_and_go_nearest_first() {
    let mut rig = Rig::new(octant_nodes(), 128, 1, 0);
    let cut = ids(&CHILDREN);
    let cam = camera("zoom");
    let mut started: Vec<(u64, u8)> = Vec::new();
    // Budget: one page per frame and the whole chain 4..=7 per node, 8 x (1 + 3 x 4) = 104 pages.
    for _ in 0..120 {
        for upload in rig.step(&cut, &cam, 4.0).uploads {
            if upload.page_range.start == 0 {
                started.push((upload.key.node.get(), upload.key.depth));
            }
        }
    }
    let near = by_distance(&cam, &CHILDREN);
    let expected: Vec<(u64, u8)> = near
        .iter()
        .map(|id| (*id, 4))
        // One level at a time (ADR 0053): each intermediate depth is a step, nearest first.
        .chain(near.iter().map(|id| (*id, 5)))
        .chain(near.iter().map(|id| (*id, 6)))
        .chain(near.iter().map(|id| (*id, 7)))
        .collect();
    assert_ne!(
        near,
        CHILDREN.to_vec(),
        "setup: the distance order must differ from the id order"
    );
    assert_eq!(started, expected);
}

#[test]
fn a_level_that_leaves_the_wanted_set_is_cancelled_and_its_slots_are_freed() {
    let mut rig = Rig::new(octant_nodes(), 128, 1, 0);
    let cut = ids(&CHILDREN);
    let cam = camera("fit");
    for _ in 0..16 {
        rig.step(&cut, &cam, 4.0);
    }
    assert!(
        !rig.gpu.partial().is_empty(),
        "setup: a level should be half uploaded"
    );
    // Far away every node wants depth 4 again: the unfinished depth-7 levels are cancelled.
    let mut freed = 0;
    let mut cancels = 0;
    let mut plan = rig.step(&cut, &cam, 64.0);
    for _ in 0..6 {
        freed += plan.page_frees.len();
        cancels += plan
            .events
            .iter()
            .filter(|e| matches!(e, CellPoolEvent::Cancel(_)))
            .count();
        plan = rig.step(&cut, &cam, 64.0);
    }
    assert!(rig.gpu.partial().is_empty(), "{:?}", rig.gpu.partial());
    assert!(freed > 0 && cancels > 0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    rig.assert_gpu_matches_pool();
    assert_eq!(
        rig.pool.free.len() + rig.pool.pending_pages() + rig.gpu.owner.len(),
        128
    );
}

#[test]
fn overflow_keeps_previous_complete_cut() {
    // Usable capacity is 7 pages; one node fits, eight coarsest levels (8 pages) do not.
    let mut rig = Rig::new(octant_nodes(), 8, 8, 1);
    let cam = camera("fit");
    let first = rig.run(8, &ids(&[2]), &cam, 64.0);
    assert_eq!(first.apply, CutApply::Applied);
    assert_eq!(first.counters.nodes_without_level, 0);
    let held = first.entries.clone();
    let active = first.active.clone();
    for n in 1..=10_u32 {
        let plan = rig.step(&ids(&CHILDREN), &cam, 64.0);
        assert_eq!(plan.apply, CutApply::RetainedPrevious);
        assert_eq!(plan.entries, held);
        assert_eq!(plan.active, active);
        assert_eq!(plan.counters.pool_overflow, n);
        // Seven of the eight nodes have no level (excluded, R3-Q20): the target is not reached.
        assert_eq!(plan.counters.nodes_without_level, 7);
        assert!(plan.fetch_requests.is_empty() && plan.uploads.is_empty());
    }
    let back = rig.step(&ids(&[2]), &cam, 64.0);
    assert_eq!(back.apply, CutApply::Applied);
}

/// Root node (id 1) with stored depths 3..=5.
fn root_node() -> HashMap<NodeId, PoolNode> {
    HashMap::from([(
        NodeId::new(1),
        PoolNode {
            bounds: cube(ORIGIN, SIDE),
            stored: vec![3, 4, 5],
            buildable: (3, 5),
            points: 2000,
            octant_data_bounds: None,
        },
    )])
}

/// T15 draw-cover oracle on full lattices: each finest cell (depth 5) is covered exactly once.
fn assert_covers_once(entry: &CellCutEntry) {
    const FINEST: u8 = 5;
    let n = 1_usize << FINEST;
    let mut covered = vec![0_u8; n * n * n];
    for (depth, octants) in entry.draws() {
        let cells = 1_usize << depth;
        let f = 1_usize << (FINEST - depth);
        for o in (0..8).filter(|o| octants >> o & 1 == 1) {
            let base = [o >> 2 & 1, o >> 1 & 1, o & 1].map(|bit| bit * cells / 2);
            for x in base[0]..base[0] + cells / 2 {
                for y in base[1]..base[1] + cells / 2 {
                    for z in base[2]..base[2] + cells / 2 {
                        for (dx, dy, dz) in (0..f)
                            .flat_map(|a| (0..f).flat_map(move |b| (0..f).map(move |c| (a, b, c))))
                        {
                            covered[((x * f + dx) * n + y * f + dy) * n + z * f + dz] += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(
        covered.iter().all(|c| *c == 1),
        "a finest cell is not covered exactly once"
    );
}

#[test]
fn regraft_mid_cut_keeps_coverage() {
    let mut rig = Rig::new(root_node(), 64, 16, 3);
    let cut = ids(&[1]);
    let cam = camera("fit");
    // 1 px wants depth 5 everywhere.
    let plan = rig.run(12, &cut, &cam, 1.0);
    assert_eq!(plan.counters.stages, 0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    let wanted = rig.pool.key(NodeId::new(1), 5);
    let coarsest = rig.pool.key(NodeId::new(1), 3);
    assert!(rig.pool.is_resident(wanted));
    assert!(!rig.pool.evict(wanted), "a level the cut draws stays");
    assert!(!rig.pool.evict(coarsest), "the coarsest level is pinned");
    // Far away the cut draws coarse levels, so the fine one can go once they are resident (until
    // then it is the finer level that meets the target, R3-Q21).
    rig.run(12, &cut, &cam, 64.0);
    assert!(rig.pool.evict(wanted));
    let plan = rig.step(&cut, &cam, 64.0);
    assert_eq!(plan.events, vec![CellPoolEvent::Evict(wanted)]);
    rig.run(2, &cut, &cam, 64.0);
    // Back to 1 px: the level is missing, the cut stages it and it is grafted in again.
    let mut previous = u32::MAX;
    let mut peak = 0;
    let mut re_requested = false;
    let mut recovered = false;
    for frame in 0..20 {
        let plan = rig.step(&cut, &cam, 1.0);
        re_requested |= plan.fetch_requests.contains(&wanted);
        assert_eq!(plan.counters.nodes_without_level, 0, "frame {frame}");
        assert_covers_once(&plan.entries[0]);
        assert!(plan.counters.stages <= previous, "frame {frame}");
        previous = plan.counters.stages;
        peak = peak.max(plan.counters.stages);
        recovered |= plan.counters.stages == 0;
    }
    assert!(peak > 0, "the missing level must show as a stage");
    assert!(re_requested && recovered);
    let plan = rig.step(&cut, &cam, 1.0);
    assert_eq!(plan.counters.stages, 0);
    assert_covers_once(&plan.entries[0]);
}

#[test]
fn slots_of_an_evicted_level_return_two_frames_after_it_last_drew() {
    // Node 2 at 4 px: depth 7 (4 pages) + depth 4 (1 page); reuse needs the delay.
    // Stepping down from 7 also needs the pages of the levels in between (ADR 0053), so the pool
    // has room for them.
    let mut rig = Rig::new(octant_nodes(), 16, 16, 0);
    let cut = ids(&[2]);
    let cam = camera("zoom");
    let plan = rig.run(24, &cut, &cam, 4.0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    let key = rig.pool.key(NodeId::new(2), 7);
    assert!(plan.active.iter().any(|a| a.key == key));
    let slots: Vec<u32> = (0..4).map(|p| rig.gpu.slot_of[&(key, p)]).collect();
    // The level draws until frame `last`; far away the node steps down one level per frame
    // (ADR 0053) and then the level is unused and may go.
    let mut last = rig.frame;
    for _ in 0..8 {
        let plan = rig.step(&cut, &cam, 64.0);
        if plan.active.iter().any(|a| a.key == key) {
            last = rig.frame;
        } else {
            break;
        }
    }
    assert!(rig.pool.evict(key));
    // Pages return two frames after the last draw, not before, and they are not rewritten sooner.
    let mut freed_at = None;
    for _ in 0..3 {
        let plan = rig.step(&cut, &cam, 64.0);
        if freed_at.is_some() {
            continue;
        }
        if slots.iter().all(|s| plan.page_frees.contains(s)) {
            freed_at = Some(rig.frame);
        } else {
            assert!(slots.iter().all(|s| !plan.page_frees.contains(s)));
            assert!(
                plan.uploads
                    .iter()
                    .all(|u| u.slots.iter().all(|s| !slots.contains(s)))
            );
        }
    }
    assert_eq!(freed_at, Some(last + 2));
    let plan = rig.run(8, &cut, &cam, 4.0);
    assert_eq!(plan.counters.stages, 0);
    assert_eq!(plan.counters.nodes_without_level, 0);
}

#[test]
fn eviction_follows_lru_order_and_never_touches_pinned_levels() {
    // Usable capacity 11 pages: depths 7, 6 and 5 of node 2 cannot all stay next to depth 4.
    let mut rig = Rig::new(octant_nodes(), 12, 8, 0);
    let cut = ids(&[2]);
    let zoom = camera("zoom");
    let fit = camera("fit");
    let mut events = Vec::new();
    for (cam, target) in [(&zoom, 4.0), (&zoom, 16.0), (&fit, 16.0), (&zoom, 64.0)] {
        for frame in 0..8 {
            let plan = rig.step(&cut, cam, target);
            if rig.frame > 2 {
                assert_eq!(plan.counters.nodes_without_level, 0, "frame {frame}");
            }
            for event in plan.events {
                if let CellPoolEvent::Evict(key) = event {
                    events.push(key.depth);
                }
            }
        }
    }
    // Depth 7 was used first, then 6, then 5: that is the eviction order.
    assert_eq!(events, vec![7, 6]);
    assert!(rig.pool.is_resident(rig.pool.key(NodeId::new(2), 4)));
    rig.assert_gpu_matches_pool();
}

#[test]
fn fit_coarsens_the_farthest_nodes_first_and_releases_pages() {
    let nodes = octant_nodes();
    // Usable 39 pages: eight depth-7 levels (5 pages each with the pinned one) are 40.
    let mut rig = Rig::new(nodes, 44, 64, 2);
    let cut = ids(&CHILDREN);
    let cam = camera("fit");
    let plan = rig.run(30, &cut, &cam, 4.0);
    assert_eq!(plan.apply, CutApply::Applied);
    assert_eq!(plan.counters.pool_coarsened, 2);
    assert_eq!(plan.counters.nodes_without_level, 0);
    assert_eq!(plan.counters.stages, 0);
    // The two coarsened nodes are the two farthest from the eye (distances computed here).
    let near = by_distance(&cam, &CHILDREN);
    for (rank, id) in near.iter().enumerate() {
        let depth = depth_of(&plan, *id)[0];
        assert_eq!(
            depth,
            Some(if rank >= 6 { 4 } else { 7 }),
            "node {id} rank {rank}"
        );
    }
    let loaded = plan.counters.resident_pages;
    // Far away only depth 4 is wanted: the finer levels leave and the caps are lifted.
    let mut plan = plan;
    for _ in 0..60 {
        plan = rig.step(&cut, &cam, 64.0);
    }
    assert_eq!(plan.counters.pool_coarsened, 0);
    assert!(
        plan.counters.resident_pages < loaded,
        "{} !< {loaded}",
        plan.counters.resident_pages
    );
    assert!(u64::from(plan.counters.resident_pages) <= rig.pool.usable() * 80 / 100);
    rig.assert_gpu_matches_pool();
}

#[test]
fn a_still_pose_converges_within_120_frames_and_stays_stable() {
    let mut rig = Rig::new(octant_nodes(), 44, 4, 2);
    let cut = ids(&CHILDREN);
    let cam = camera("zoom");
    let mut settled = None;
    let mut digest = None;
    let mut last = None;
    for frame in 0..240 {
        let plan = rig.step(&cut, &cam, 4.0);
        let quiet =
            plan.uploads.is_empty() && plan.events.is_empty() && plan.fetch_requests.is_empty();
        let done =
            quiet && plan.apply == CutApply::Applied && plan.counters.nodes_without_level == 0;
        if done && settled.is_none() {
            settled = Some(frame);
        }
        if !quiet {
            settled = None;
        }
        if frame >= 120 {
            assert!(quiet, "frame {frame}");
            assert!(plan.counters.cut_evaluations <= 3, "frame {frame}");
            let now = (
                plan.entries.clone(),
                plan.counters.pool_coarsened,
                plan.counters.resident_pages,
            );
            assert_eq!(*digest.get_or_insert(now.clone()), now, "frame {frame}");
        }
        last = Some(plan);
    }
    assert!(matches!(settled, Some(n) if n <= 120), "{settled:?}");
    let last = last.unwrap();
    assert_eq!(last.apply, CutApply::Applied);
    assert_eq!(last.counters.nodes_without_level, 0);
    assert_eq!(last.counters.stages, 0);
    assert_eq!(digest.unwrap().1, 2);
}

#[test]
fn caps_persist_while_the_uncapped_total_sits_between_80_and_90_percent() {
    // Eight nodes of 8192 points: a node needs 1 + 16 pages at depth 7 and 1 + 8 at depth 5. Usable
    // 123: high 110, low 98. At 4 px (136 pages) four nodes drop to depth 5 (104); at 16 px the
    // uncapped total is 104, between the two thresholds, so the caps stay.
    let mut nodes = octant_nodes();
    for node in nodes.values_mut() {
        node.points = 8192;
    }
    let mut rig = Rig::new(nodes, 140, 64, 1);
    let cut = ids(&CHILDREN);
    let cam = camera("fit");
    let plan = rig.run(40, &cut, &cam, 4.0);
    assert_eq!(plan.counters.pool_coarsened, 4);
    let resident = plan.counters.resident_pages;
    for frame in 0..120 {
        let plan = rig.step(&cut, &cam, 16.0);
        assert_eq!(
            plan.counters.pool_coarsened, 4,
            "frame {frame}: caps must not be lifted"
        );
        if frame >= 60 {
            assert!(
                plan.uploads.is_empty() && plan.events.is_empty(),
                "frame {frame}"
            );
            assert!(plan.counters.resident_pages <= resident);
        }
    }
    // Well under 80 % the caps go.
    let plan = rig.run(10, &cut, &cam, 64.0);
    assert_eq!(plan.counters.pool_coarsened, 0);
}

#[test]
fn an_exact_size_above_the_estimate_triggers_coarsening_without_hazards() {
    // Estimates: 5 pages per node at depth 7, 40 in all, under the high mark of 41. The loader
    // packs 6 pages for every depth-7 level (48), so once they arrive the fit coarsens.
    let mut rig = Rig::new(octant_nodes(), 52, 64, 2);
    rig.deep = Some(6);
    let cut = ids(&CHILDREN);
    let cam = camera("fit");
    let first = rig.run(2, &cut, &cam, 4.0);
    assert_eq!(first.counters.pool_coarsened, 0);
    let mut coarsened_at = None;
    for frame in 2..60 {
        let plan = rig.step(&cut, &cam, 4.0);
        if plan.counters.pool_coarsened > 0 && coarsened_at.is_none() {
            coarsened_at = Some(frame);
        }
    }
    assert!(
        coarsened_at.is_some(),
        "the exact sizes never triggered the fit"
    );
    let plan = rig.step(&cut, &cam, 4.0);
    assert_eq!(plan.apply, CutApply::Applied);
    assert_eq!(plan.counters.nodes_without_level, 0);
    assert_eq!(plan.counters.stages, 0);
    assert!(u64::from(plan.counters.resident_pages) <= rig.pool.usable());
    rig.assert_gpu_matches_pool();
}

#[test]
fn a_level_waiting_for_pending_pages_blocks_lower_priority_levels() {
    // Node 8 is the nearest, node 2 the farthest (zoom). Both want depth 7: 4 and 2 pages.
    let mut nodes = octant_nodes();
    nodes.get_mut(&NodeId::new(2)).unwrap().points = 1000;
    let mut rig = Rig::new(nodes, 40, 64, 0);
    let cut = ids(&[8, 2]);
    let cam = camera("zoom");
    rig.step(&cut, &cam, 4.0);
    // Two free pages for the pinned levels... and then two more, while 4 pages are due next frame.
    rig.pool.free = (0..4).collect();
    rig.pool.pending = vec![(rig.frame + 2, vec![10, 11, 12, 13])];
    let two = rig.step(&cut, &cam, 4.0);
    let started: Vec<_> = two
        .uploads
        .iter()
        .map(|u| (u.key.node.get(), u.key.depth))
        .collect();
    assert!(
        started.contains(&(8, 4)) && started.contains(&(2, 4)),
        "{started:?}"
    );
    assert!(
        !started.iter().any(|(_, depth)| *depth == 7),
        "a lower priority level took the pages the nearest one waits for: {started:?}"
    );
    let three = rig.step(&cut, &cam, 4.0);
    let started: Vec<_> = three
        .uploads
        .iter()
        .map(|u| (u.key.node.get(), u.key.depth))
        .collect();
    assert_eq!(started.first(), Some(&(8, 7)), "{started:?}");
}

/// Nodes of the `RetainedPrevious` scenario: the root, a node `MAX_MASK_LEVELS + 1` levels below
/// it (too fine for the mask) and one `MAX_MASK_LEVELS` below it.
fn chain_nodes() -> HashMap<NodeId, PoolNode> {
    HashMap::from([
        (
            NodeId::new(1),
            PoolNode {
                bounds: cube(ORIGIN, SIDE),
                stored: vec![3, 4],
                buildable: (3, 4),
                points: 2000,
                octant_data_bounds: None,
            },
        ),
        (
            NodeId::new(20),
            PoolNode {
                bounds: cube(ORIGIN, SIDE / f64::from(1_u32 << (MAX_MASK_LEVELS + 1))),
                stored: vec![7, 8],
                buildable: (7, 8),
                points: 2000,
                octant_data_bounds: None,
            },
        ),
        (
            NodeId::new(21),
            PoolNode {
                bounds: cube(ORIGIN, SIDE / f64::from(1_u32 << MAX_MASK_LEVELS)),
                stored: vec![6, 7],
                buildable: (6, 7),
                points: 2000,
                octant_data_bounds: None,
            },
        ),
    ])
}

#[test]
fn a_cut_that_is_too_fine_is_retained_and_reported_until_the_cut_changes() {
    let mut rig = Rig::new(chain_nodes(), 64, 8, 1);
    let cam = camera("fit");
    // Node 20 alone is fine: it becomes resident.
    let alone = rig.run(8, &ids(&[20]), &cam, 64.0);
    assert_eq!(alone.apply, CutApply::Applied);
    assert_eq!(alone.counters.nodes_without_level, 0);
    assert!(!alone.counters.descendant_too_fine);
    // The root has no level yet: it is excluded (R3-Q20) and node 20 keeps drawing. Together with
    // the root's level it is `MAX_MASK_LEVELS + 1` levels below it: the mask cannot express it.
    let held = alone.entries.clone();
    let mut waited = 0;
    loop {
        let plan = rig.step(&ids(&[1, 20]), &cam, 64.0);
        if plan.counters.descendant_too_fine {
            break;
        }
        assert_eq!(plan.apply, CutApply::Applied);
        assert_eq!(plan.entries, held);
        assert_eq!(plan.counters.nodes_without_level, 1);
        waited += 1;
        assert!(waited < 12);
    }
    assert!(waited > 0);
    for _ in 0..10 {
        let plan = rig.step(&ids(&[1, 20]), &cam, 64.0);
        assert_eq!(plan.apply, CutApply::RetainedPrevious);
        assert!(plan.counters.descendant_too_fine);
        assert_eq!(plan.entries, held);
        assert_no_holes(&plan, &chain_nodes());
        // The retained cut does not draw the root, whatever the pool holds.
        assert_eq!(plan.counters.nodes_without_level, 1);
        assert_eq!(plan.counters.excluded_nodes, 1);
        assert!(
            plan.fetch_requests
                .iter()
                .all(|k| k.node != NodeId::new(20))
        );
    }
    assert!(
        rig.pool.is_resident(rig.pool.key(NodeId::new(20), 7)),
        "retained levels stay"
    );
    // Node 21 is `MAX_MASK_LEVELS` below the root, which is resident from the previous steps: the
    // cut applies at once (R3-Q19: the root draws where node 21 has no level); T7a waited for the
    // level. Every frame is complete, and what is missing only goes down.
    let nodes = chain_nodes();
    let mut previous = u32::MAX;
    for frame in 0..12 {
        let plan = rig.step(&ids(&[1, 21]), &cam, 64.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_eq!(plan.entries.len(), 2, "frame {frame}");
        assert_no_holes(&plan, &nodes);
        assert!(!plan.counters.descendant_too_fine, "frame {frame}");
        assert!(
            plan.counters.nodes_without_level <= previous,
            "frame {frame}"
        );
        if frame == 0 {
            assert_eq!(plan.counters.nodes_without_level, 1);
            assert!(plan.counters.stages > 0);
        }
        previous = plan.counters.nodes_without_level;
    }
    assert_eq!(previous, 0);
    let last = rig.step(&ids(&[1, 21]), &cam, 64.0);
    assert_eq!(last.counters.stages, 0);
}

#[test]
fn a_node_without_a_level_and_without_an_ancestor_waits_for_its_level() {
    let mut rig = Rig::new(octant_nodes(), 64, 8, 3);
    let cam = camera("fit");
    // R3-Q20: nothing is resident for [2, 3] and no ancestor covers them: they are excluded and
    // their coarsest levels requested. Nothing was drawn before, so nothing is lost.
    let first = rig.step(&ids(&[2, 3]), &cam, 64.0);
    assert_eq!(first.apply, CutApply::Applied);
    assert!(first.entries.is_empty());
    assert_eq!(first.counters.nodes_without_level, 2);
    assert_eq!(first.counters.excluded_nodes, 2);
    assert_eq!(first.fetch_requests.len(), 2);
    // A different cut does not draw anything either; its levels are requested.
    let second = rig.step(&ids(&[4, 5]), &cam, 64.0);
    assert_eq!(second.apply, CutApply::Applied);
    assert_eq!(second.counters.nodes_without_level, 2);
    let done = rig.run(8, &ids(&[4, 5]), &cam, 64.0);
    assert_eq!(done.counters.nodes_without_level, 0);
    // A pose jump to a cut disjoint from the drawn one, whose levels are not resident: the image
    // stays until the first coarsest level arrives, because the real cut would draw nothing.
    let wait = rig.step(&ids(&[6, 7]), &cam, 64.0);
    assert_eq!(wait.apply, CutApply::RetainedPrevious);
    assert_eq!(wait.entries, done.entries);
    assert_eq!(wait.counters.nodes_without_level, 2);
    let back = rig.run(8, &ids(&[6, 7]), &cam, 64.0);
    assert_eq!(back.apply, CutApply::Applied);
    assert_eq!(back.counters.nodes_without_level, 0);
    let swap = rig.step(&ids(&[4, 5]), &cam, 64.0);
    assert_eq!(swap.apply, CutApply::Applied);
    assert_eq!(swap.entries.len(), 2);
    assert_eq!(swap.counters.nodes_without_level, 0);
    assert_no_holes(&swap, &octant_nodes());
}

#[test]
fn overflow_on_the_first_frame_reports_the_missing_levels() {
    let mut rig = Rig::new(octant_nodes(), 8, 8, 1);
    let plan = rig.step(&ids(&CHILDREN), &camera("fit"), 64.0);
    assert_eq!(plan.apply, CutApply::RetainedPrevious);
    assert_eq!(plan.counters.pool_overflow, 1);
    assert_eq!(plan.counters.nodes_without_level, 8);
}

#[test]
fn mask_slots_follow_the_cut_and_pinned_levels_cover_masked_nodes() {
    let mut rig = Rig::new(chain_nodes(), 64, 8, 0);
    let cam = camera("fit");
    let plan = rig.run(10, &ids(&[1, 21]), &cam, 64.0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    // The root hides the cells under node 21; the fake GPU checked that it received that mask.
    assert!(plan.entries[0].mask.iter().any(|word| *word != 0));
    assert_eq!(rig.pool.mask_slots.len(), 2);
    let slots: HashSet<u32> = rig
        .pool
        .mask_slots
        .values()
        .map(|state| state.slot)
        .collect();
    assert_eq!(slots.len(), 2);
    // The child leaves the cut: its slot is freed and the root's mask is rewritten.
    let plan = rig.run(6, &ids(&[1]), &cam, 64.0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    assert_eq!(rig.pool.mask_slots.len(), 1);
    assert_eq!(rig.pool.free_masks.len(), 1);
    assert!(plan.entries[0].mask.iter().all(|word| *word == 0));
    // Both coarsest levels stay resident while their nodes are in the cut.
    assert!(rig.pool.is_resident(rig.pool.key(NodeId::new(1), 3)));
}

#[test]
fn exact_sizes_lift_a_false_coarsening() {
    // Usable 39 pages: with the estimate (4 pages per deep level) eight depth-7 levels need 40
    // and two nodes are coarsened, and the estimate of those two keeps the caps (the open total
    // stays above 31); the loader really packs depth 7 into 3 pages, 32 in all.
    let sizes = [(4_u8, 1_usize), (5, 4), (6, 4), (7, 3)];
    let run = |exact: bool| {
        let mut rig = Rig::new(octant_nodes(), 44, 64, 2);
        rig.deep = Some(3);
        if exact {
            for id in CHILDREN {
                rig.pool.set_level_sizes(NodeId::new(id), &sizes);
            }
        }
        rig.run(30, &ids(&CHILDREN), &camera("fit"), 4.0)
    };
    assert_eq!(run(false).counters.pool_coarsened, 2);
    let plan = run(true);
    assert_eq!(plan.counters.pool_coarsened, 0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    assert_eq!(plan.counters.stages, 0);
}

#[test]
fn a_steady_frame_calls_cell_cut_at_most_once_and_changes_invalidate_the_reuse() {
    let mut rig = Rig::new(octant_nodes(), 256, 64, 0);
    let cut = ids(&CHILDREN);
    let zoom = camera("zoom");
    rig.run(40, &cut, &zoom, 4.0);
    let steady = rig.step(&cut, &zoom, 4.0);
    assert!(steady.counters.cut_evaluations <= 1);
    assert!(steady.counters.wanted_reused && steady.counters.real_reused);
    assert_eq!(steady.counters.depth_refreshes, 0);

    // A new size for a level, a smaller cut and another target each redo the wanted cut.
    rig.pool.set_level_sizes(NodeId::new(2), &[(5, 3)]);
    assert!(!rig.step(&cut, &zoom, 4.0).counters.wanted_reused);
    assert!(rig.step(&cut, &zoom, 4.0).counters.wanted_reused);
    assert!(!rig.step(&cut[..7], &zoom, 4.0).counters.wanted_reused);
    let far = rig.step(&cut[..7], &zoom, 64.0);
    assert!(!far.counters.wanted_reused && far.counters.depth_refreshes > 0);

    // Residency changes redo the real cut (here also the wanted one: an evicted level is back to
    // its estimated size).
    rig.run(16, &cut[..7], &zoom, 64.0);
    let key = rig.pool.key(NodeId::new(2), 7);
    assert!(rig.pool.evict(key));
    let plan = rig.step(&cut[..7], &zoom, 64.0);
    assert!(!plan.counters.real_reused);

    // A camera that stops is exact: it refreshes once, then everything is reused again.
    let moved = camera("fit");
    rig.step(&cut[..7], &moved, 64.0);
    let still = rig.run(16, &cut[..7], &moved, 64.0);
    assert!(still.counters.wanted_reused && still.counters.real_reused);
    assert!(still.counters.cut_evaluations <= 1);
}

fn full_depths(cam: &OrbitCamera, node: &PoolNode, target: f64) -> [u8; 8] {
    std::array::from_fn(|o| {
        crate::cells::cut::octant_wanted_depth(
            &view(cam),
            &grid(),
            ORIGIN,
            node.bounds,
            o,
            node.buildable,
            target,
        )
    })
}

#[test]
fn a_still_pose_is_exact_after_a_moving_sequence() {
    let nodes = octant_nodes();
    let cut = ids(&CHILDREN);
    let mut pool = CellPool::new(PoolConfig {
        capacity_pages: 4096,
        ..PoolConfig::default()
    });
    let mut cam = camera("zoom");
    let run = |pool: &mut CellPool, cam: &OrbitCamera| {
        pool.frame(
            &cut,
            &|id| nodes.get(&id).cloned(),
            &view(cam),
            &grid(),
            ORIGIN,
            4.0,
        )
        .expect("frame");
    };
    for _ in 0..20 {
        cam.orbit(-0.03, -0.02);
        run(&mut pool, &cam);
    }
    // R3-Q18: one settled frame redoes every hinted entry with the full search.
    for _ in 0..2 {
        run(&mut pool, &cam);
    }
    for id in &cut {
        let cached = &pool.depths[id];
        assert!(cached.exact);
        assert_eq!(
            cached.depths,
            full_depths(&cam, &nodes[id], 4.0),
            "node {id:?}"
        );
    }
}

/// Deterministic xorshift for the property tests.
fn next(state: &mut u64) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 11) as f64 / (1_u64 << 53) as f64
}

#[test]
fn the_hinted_search_equals_the_full_search() {
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let nodes = octant_nodes();
    let mut checked = 0;
    for _ in 0..50_000 {
        let mut cam = camera("fit");
        // Eyes inside, next to and far from the octants, looking anywhere.
        let eye: [f64; 3] =
            std::array::from_fn(|a| ORIGIN[a] + SIDE * (next(&mut state) * 2.2 - 0.6));
        let target: [f64; 3] = std::array::from_fn(|a| ORIGIN[a] + SIDE * next(&mut state));
        if !cam.set_pose(eye, target) {
            continue;
        }
        let node = &nodes[&NodeId::new(2 + (next(&mut state) * 8.0) as u64 % 8)];
        let stored: Vec<u8> = (3..=12)
            .filter(|_| next(&mut state) < 0.6)
            .collect::<Vec<_>>();
        if stored.is_empty() {
            continue;
        }
        let target_px = [1.0, 4.0, 16.0][(next(&mut state) * 3.0) as usize % 3];
        let octant = (next(&mut state) * 8.0) as usize % 8;
        let range = (stored[0], *stored.last().unwrap());
        let width = usize::from(range.1 - range.0) + 1;
        let hint = range.0 + ((next(&mut state) * width as f64) as usize % width) as u8;
        let v = view(&cam);
        let full = crate::cells::cut::octant_wanted_depth(
            &v,
            &grid(),
            ORIGIN,
            node.bounds,
            octant,
            range,
            target_px,
        );
        let near = crate::cells::cut::octant_wanted_depth_near(
            &v,
            &grid(),
            ORIGIN,
            node.bounds,
            octant,
            crate::cells::cut::ProbeBox::Geometric,
            range,
            target_px,
            hint,
        );
        assert_eq!(
            near, full,
            "eye {eye:?} target {target:?} stored {stored:?} hint {hint}"
        );
        checked += 1;
    }
    // Measured with the rule of ADR 0054 (the smallest of the 8 children): 0 of 79 993 differ.
    assert!(checked > 40_000);
}

#[test]
fn caps_lift_when_the_capped_levels_are_bigger_than_the_uncapped_ones() {
    // Exact sizes only (nothing is offered). First depth 6 is smaller than depth 7, so the fit
    // caps nodes at 6; then depth 6 grows past depth 7 and the capped total (> low) is larger
    // than the uncapped one (<= low): the caps must lift.
    let nodes = octant_nodes();
    let cut = ids(&CHILDREN);
    let cam = camera("fit");
    let mut pool = CellPool::new(PoolConfig {
        capacity_pages: 44,
        ..PoolConfig::default()
    });
    let set = |pool: &mut CellPool, six: usize, seven: usize| {
        for id in CHILDREN {
            pool.set_level_sizes(NodeId::new(id), &[(4, 1), (5, 1), (6, six), (7, seven)]);
        }
    };
    let frame = |pool: &mut CellPool| {
        pool.frame(
            &cut,
            &|id| nodes.get(&id).cloned(),
            &view(&cam),
            &grid(),
            ORIGIN,
            // Calibrated for the old maximum target; 2 px under ADR 0054 selects the same depths.
            2.0,
        )
        .expect("frame")
    };
    set(&mut pool, 2, 4);
    let mut plan = frame(&mut pool);
    for _ in 0..4 {
        plan = frame(&mut pool);
    }
    assert!(plan.counters.pool_coarsened > 0);
    set(&mut pool, 9, 2);
    for _ in 0..4 {
        plan = frame(&mut pool);
    }
    assert_eq!(plan.counters.pool_coarsened, 0);
}

#[test]
fn an_eye_that_drifts_redoes_the_fit_while_caps_are_active() {
    let nodes = octant_nodes();
    let cut = ids(&CHILDREN);
    let mut pool = CellPool::new(PoolConfig {
        capacity_pages: 44,
        ..PoolConfig::default()
    });
    for id in CHILDREN {
        pool.set_level_sizes(NodeId::new(id), &[(4, 1), (5, 4), (6, 4), (7, 4)]);
    }
    let mut cam = camera("fit");
    let frame = |pool: &mut CellPool, cam: &OrbitCamera| {
        pool.frame(
            &cut,
            &|id| nodes.get(&id).cloned(),
            &view(cam),
            &grid(),
            ORIGIN,
            4.0,
        )
        .expect("frame")
    };
    for _ in 0..4 {
        frame(&mut pool, &cam);
    }
    let plan = frame(&mut pool, &cam);
    assert!(plan.counters.pool_coarsened > 0 && plan.counters.wanted_reused);
    // Dolly the eye by a tenth of the distance to the nearest node: past the 5 % limit, and
    // far too little to change any depth.
    let (eye, target) = (cam.position(), cam.target());
    let moved: [f64; 3] = std::array::from_fn(|a| eye[a] + (target[a] - eye[a]) * 0.1);
    assert!(cam.set_pose(moved, target));
    let plan = frame(&mut pool, &cam);
    assert!(!plan.counters.wanted_reused);
}

#[test]
fn the_drift_limit_bounds_the_edge_change_also_off_axis() {
    let mut cam = camera("fit");
    let eye = [
        ORIGIN[0] + SIDE * 0.5,
        ORIGIN[1] - SIDE * 2.0,
        ORIGIN[2] + SIDE * 0.5,
    ];
    // A cube whose centre is 60 degrees off the view axis, at distance 3 SIDE.
    let forward = [0.0, 1.0, 0.0];
    let centre: [f64; 3] = [
        eye[0] + 3.0 * SIDE * (60.0_f64.to_radians()).sin(),
        eye[1] + 3.0 * SIDE * (60.0_f64.to_radians()).cos(),
        eye[2],
    ];
    let side = SIDE / 64.0;
    let cube = cube(centre.map(|v| v - side / 2.0), side);
    let target = [eye[0], eye[1] + 1.0, eye[2]];
    assert!(cam.set_pose(eye, target));
    let pose = Pose { eye, forward };
    let bounds = cube;
    let depth = effective_depth(pose, bounds, [0.0; 3]);
    assert!(depth > 0.0);
    let base = view(&cam).projected_voxel_edge_pixels(bounds, [0.0; 3]);
    let step = DRIFT_FRACTION * depth;
    for direction in [
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
    ] {
        let moved: [f64; 3] = std::array::from_fn(|a| eye[a] + direction[a] * step);
        let mut other = cam;
        assert!(other.set_pose(
            moved,
            std::array::from_fn(|a| moved[a] + (target[a] - eye[a]))
        ));
        let edge = view(&other).projected_voxel_edge_pixels(bounds, [0.0; 3]);
        assert!(
            (edge / base - 1.0).abs() <= 0.05,
            "{direction:?}: {}",
            edge / base
        );
    }
}

mod data_bound;
mod fallback;
mod refine;
mod stepping;
mod wait;

// R3-Q19: local fallback, coarse to fine and reset.

fn has_entry(plan: &FramePlan, node: u64) -> bool {
    plan.entries.iter().any(|e| e.node == NodeId::new(node))
}

fn coarse_resident(rig: &Rig, node: u64) -> bool {
    let depth = rig.nodes[&NodeId::new(node)].stored[0];
    rig.pool.is_resident(rig.pool.key(NodeId::new(node), depth))
}

// R3-Q21: the image only refines.

fn sample_points(bounds: SpatialBounds) -> Vec<[f64; 3]> {
    let mut points = Vec::new();
    for x in [0.17, 0.5, 0.83] {
        for y in [0.17, 0.5, 0.83] {
            for z in [0.17, 0.5, 0.83] {
                let at = [x, y, z];
                points.push(std::array::from_fn(|a| {
                    bounds.min()[a] + at[a] * (bounds.max()[a] - bounds.min()[a])
                }));
            }
        }
    }
    points
}

/// Where the cell of `point` sits in the 8x8x8 grid of `bounds`, if the point is inside.
fn cell_of(bounds: SpatialBounds, point: [f64; 3]) -> Option<[usize; 3]> {
    if !(0..3).all(|a| bounds.min()[a] <= point[a] && point[a] < bounds.max()[a]) {
        return None;
    }
    let cell = side_of(bounds) / 8.0;
    Some(std::array::from_fn(|a| {
        (((point[a] - bounds.min()[a]) / cell) as usize).min(7)
    }))
}

fn resident_at(rig: &Rig, node: u64, depth: u8) -> bool {
    rig.pool.is_resident(rig.pool.key(NodeId::new(node), depth))
}

/// `two_regions` whose children of node 2 also store depth 4, so they have a level coarser than
/// what the ancestor drew to be held with.
fn two_regions_with_coarse_children() -> HashMap<NodeId, PoolNode> {
    let mut nodes = two_regions();
    let parent = nodes[&NodeId::new(2)].clone();
    nodes.extend(octants_of(&parent, 10, &[4, 5, 6, 7]));
    nodes
}

/// Marks the levels resident without uploading, to set up a state by hand.
fn seed_resident(pool: &mut CellPool, node: u64, depths: &[u8]) {
    for depth in depths {
        let key = pool.key(NodeId::new(node), *depth);
        pool.levels.insert(
            key,
            Level {
                state: LevelState::Resident { slots: Vec::new() },
                data: None,
                last_used: 0,
                last_active: 0,
                refresh: None,
            },
        );
    }
}

/// The entries `cell_cut` would draw for `cut` with the resident levels of the pool, every
/// octant wanting `wanted`.
fn entries_with(rig: &Rig, cut: &[NodeId], wanted: u8) -> Vec<CellCutEntry> {
    cell_cut_with(
        cut,
        &|id| {
            let node = rig.nodes.get(&id)?;
            Some(CellCutNode {
                bounds: node.bounds,
                buildable: node.buildable,
                stored: node.stored.clone(),
                resident: node
                    .stored
                    .iter()
                    .copied()
                    .filter(|d| rig.pool.is_resident(rig.pool.key(id, *d)))
                    .collect(),
            })
        },
        &grid(),
        &|_, _, _| wanted,
    )
    .unwrap()
}

/// `eval_hold` on `cut` as `cell_cut` would draw it with the resident levels of the pool, every
/// octant wanting `wanted`, and nothing held.
fn eval_with(rig: &Rig, cut: &[NodeId], wanted: u8) -> (HoldEval, Vec<NodeId>) {
    let grid = grid();
    let nodes: &FastMap<NodeId, PoolNode> = &rig
        .nodes
        .iter()
        .map(|(id, node)| (*id, node.clone()))
        .collect();
    let entries = entries_with(rig, cut, wanted);
    let none = FastSet::default();
    let eval = rig
        .pool
        .eval_hold(&entries, cut, nodes, &grid, &none, &|_, _, _| wanted);
    let fallback = rig
        .pool
        .compute_fallbacks(cut, nodes, &grid, &eval.hold)
        .ancestors;
    (eval, fallback)
}

/// Replaces the resident levels of `node` (stored 4..=7) by `depths`, without uploading.
fn set_resident(rig: &mut Rig, node: u64, depths: &[u8]) {
    for depth in 4..=7 {
        let key = rig.pool.key(NodeId::new(node), depth);
        rig.pool.levels.remove(&key);
    }
    seed_resident(&mut rig.pool, node, depths);
    rig.pool.residency_epoch += 1;
}

/// Pretends the applied cut drew `node` at `finest - 1` while its finest active depth was `finest`.
fn set_applied(rig: &mut Rig, node: u64, finest: u8) {
    rig.pool
        .applied_drawn
        .insert(NodeId::new(node), [Some(finest - 1); 8]);
    rig.pool.applied_finest.insert(NodeId::new(node), finest);
}

/// How wide a node's range is in a probe: `Wide` starts at the coarsest depth the node can draw
/// (the far end of `node_view_range` reaching the back of the node), `Near` is only the depth
/// the octants want. Either way `range = (far.min(near), near)` as `cell_cut_ranged` sets it.
#[derive(Clone, Copy, Debug)]
enum Far {
    Wide,
    Near,
}

/// What `refine_probe` saw: whether `eval_hold` (production) and `eval_refine_hold` (the
/// per-octant hold of before) hold the node, and the depths `cell_active_depths` emits for the
/// entry it would draw with.
#[derive(Debug)]
struct Probe {
    held: bool,
    old_held: bool,
    active: Vec<u8>,
    node_depth: u8,
}

impl Probe {
    /// The active run `[coarsest, finest]`.
    fn run(&self) -> Option<Run> {
        self.active
            .first()
            .zip(self.active.last())
            .map(|(lo, hi)| (*lo, *hi))
    }
}

/// Sets the `range` of `entry` as `cell_cut_ranged` would: `(far.min(near), near)`, see `Far`.
fn set_range(rig: &Rig, entry: &mut CellCutEntry, far: Far) {
    let data = &rig.nodes[&entry.node];
    let node_depth = CellPool::node_depth(&grid(), data.bounds);
    let buildable = capped_range(data.buildable, rig.pool.caps.get(&entry.node));
    let (lo, hi) = cell_depth_range(node_depth);
    let (lo, hi) = (lo.max(buildable.0), hi.min(buildable.1));
    let near = entry
        .wanted
        .iter()
        .flatten()
        .max()
        .map_or(hi, |wanted| (*wanted).clamp(lo, hi));
    let far = match far {
        Far::Wide => lo,
        Far::Near => near,
    };
    entry.range = Some((far.min(near), near));
}

/// `eval_hold` and `eval_refine_hold` on `entries` (the real cut, nothing held yet).
fn eval_pair(
    rig: &Rig,
    entries: &[CellCutEntry],
    cut: &[NodeId],
    wish: u8,
) -> (HoldEval, HoldEval) {
    let nodes: FastMap<NodeId, PoolNode> = rig
        .nodes
        .iter()
        .map(|(id, node)| (*id, node.clone()))
        .collect();
    let none = FastSet::default();
    let wanted = |_: NodeId, _: usize, _: &CellCutNode| wish;
    (
        rig.pool
            .eval_hold(entries, cut, &nodes, &grid(), &none, &wanted),
        rig.pool
            .eval_refine_hold(entries, cut, &nodes, &grid(), &none, &wanted),
    )
}

/// Whether `eval_hold` and `eval_refine_hold` hold `node` when every octant of it wants `wish`,
/// and the active depths it would take over with: the entry is built as `plan` would (the drawn
/// levels from the entering seed and a `range`), so the active depths see the range
/// residents.
fn refine_probe(rig: &mut Rig, node: u64, wish: u8, far: Far) -> Probe {
    let id = NodeId::new(node);
    let mut entries = entries_with(rig, &[id], wish);
    set_range(rig, &mut entries[0], far);
    let (eval, old) = eval_pair(rig, &entries, &[id], wish);
    let data = &rig.nodes[&id];
    let grid = grid();
    let stored = capped(&data.stored, rig.pool.caps.get(&id));
    let resident: Vec<u8> = rig
        .pool
        .drawable_stored(&stored, &grid, data.bounds)
        .into_iter()
        .filter(|depth| rig.pool.is_resident(rig.pool.key(id, *depth)))
        .collect();
    let buildable = capped_range(data.buildable, rig.pool.caps.get(&id));
    let f = Footprint::new(&grid, data.bounds);
    let (here, _, _) = drawn_levels(
        &[Some(wish); 8],
        &stored,
        buildable,
        &resident,
        &rig.pool
            .prev_state_seeded(id, data.bounds, buildable, &grid),
        f.rep.saturating_sub(MAX_MASK_LEVELS),
    );
    let node_depth = CellPool::node_depth(&grid, data.bounds);
    let mut entry = entries.remove(0);
    entry.drawn = here;
    let active = rig.pool.cell_active_depths(&entry, node_depth, None);
    Probe {
        held: eval.hold.contains(&id),
        old_held: old.hold.contains(&id),
        active,
        node_depth,
    }
}

/// A node as the GPU twin sees it: its depth, its eye-relative corner and its active depths.
#[derive(Clone)]
struct Owner {
    node_depth: u8,
    lo: [f64; 3],
    active: Vec<u8>,
}

impl Owner {
    /// The root cell has side 1; the eye is at the origin and the nodes sit 10 units ahead.
    fn side(&self) -> f64 {
        2f64.powi(-i32::from(self.node_depth))
    }

    /// Corner of the child `index` (bits x, y, z from the low) of the node, one level deeper.
    fn child_lo(&self, index: usize) -> [f64; 3] {
        std::array::from_fn(|axis| self.lo[axis] + self.side() / 2.0 * (index >> axis & 1) as f64)
    }

    fn sub_centre(&self, octant: usize) -> [f64; 3] {
        std::array::from_fn(|axis| {
            self.lo[axis] + self.side() / 2.0 * ((octant >> axis & 1) as f64 + 0.5)
        })
    }

    /// The depth the real per-cell rule (`cell_lod::drawn_depth_at`) draws `point` at when the
    /// view wants `wish` there: a cell refines iff its children are not shorter than a cell of
    /// depth `wish` (`min_side`; the pixel target is 0, so only that bound decides).
    fn drawn_at(&self, wish: u8, point: [f64; 3]) -> Option<u8> {
        let view = LodView {
            forward: [0.0, 0.0, 1.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            focal: 1000.0,
            target_px: 0.0,
            min_side: 2f32.powi(-i32::from(wish)),
        };
        let node = NodeLod {
            anchor: self.lo.map(|c| c as f32),
            node_depth: self.node_depth,
            active: self
                .active
                .iter()
                .fold(0, |mask, depth| mask | 1u64 << (depth - self.node_depth)),
            mask: [0; 8],
        };
        twin_drawn_depth_at(&view, &node, point, self.lo, self.side())
    }
}

/// The twin oracle: `Ok` iff `after` coarsens no sampled cell, for every wish `u <= wish`: it
/// draws at least `min(what `before` drew there, u)`. `Err` names the first cell that gets
/// coarser, with the depths.
fn twin_releases(after: &Owner, wish: u8, samples: &[(Owner, [f64; 3])]) -> Result<(), String> {
    for (before, point) in samples {
        for u in 0..=wish {
            let Some(drew) = before.drawn_at(u, *point) else {
                continue;
            };
            let draws = after.drawn_at(u, *point);
            if draws.is_none_or(|d| d < drew.min(u)) {
                return Err(format!(
                    "wish {u} at {point:?}: before {:?} drew {drew}, after {:?} draws {draws:?}",
                    before.active, after.active
                ));
            }
        }
    }
    Ok(())
}

const FARS: [Far; 2] = [Far::Wide, Far::Near];

// R3-Q20: nodes without a level wait outside the cut.

/// Region A's children drew at 4 px and node 2 (their parent) was never loaded; the tests then
/// zoom out to the cut `{2}`.
fn zoom_out_rig() -> (Rig, HashMap<NodeId, PoolNode>, OrbitCamera) {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    let cam = camera("fit");
    rig.run(60, &ids(&region_children(10)), &cam, 4.0);
    assert!(!coarse_resident(&rig, 2), "setup: node 2 was never loaded");
    (rig, nodes, cam)
}

fn depth_drawn(plan: &FramePlan, node: NodeId) -> Vec<u8> {
    let mut depths: Vec<u8> = plan
        .entries
        .iter()
        .filter(|e| e.node == node)
        .flat_map(|e| e.drawn.iter().flatten().copied())
        .collect();
    depths.sort_unstable();
    depths.dedup();
    depths
}

/// A cell-mode rig that never delivers a level (delay far beyond the test), so residency is
/// exactly what the test seeds.
fn d5_rig(nodes: HashMap<NodeId, PoolNode>) -> Rig {
    Rig::new(nodes, 512, 8, 10_000)
}

fn d5_step_at(rig: &mut Rig, cut: &[NodeId], camera: &OrbitCamera, target: f64) -> FramePlan {
    let nodes = &rig.nodes;
    rig.pool
        .frame(
            cut,
            &|id| nodes.get(&id).cloned(),
            &view(camera),
            &grid(),
            ORIGIN,
            target,
        )
        .expect("frame")
}

fn d5_node(side: f64, stored: &[u8]) -> PoolNode {
    PoolNode {
        bounds: cube(ORIGIN, side),
        stored: stored.to_vec(),
        buildable: (stored[0], *stored.last().unwrap()),
        points: 2000,
        octant_data_bounds: None,
    }
}

fn seed_more(rig: &mut Rig, node: u64, depths: &[u8]) {
    seed_resident(&mut rig.pool, node, depths);
    rig.pool.residency_epoch += 1;
}

fn active_depths(plan: &FramePlan, rig: &Rig, node: u64) -> Vec<u8> {
    let slot = plan
        .entries
        .iter()
        .find(|entry| entry.node == NodeId::new(node))
        .and_then(|entry| rig.pool.mask_slots.get(&entry.node))
        .map(|slot| slot.slot);
    let mut depths: Vec<u8> = plan
        .active
        .iter()
        .filter(|level| Some(level.mask_slot) == slot)
        .map(|level| level.key.depth)
        .collect();
    depths.sort_unstable();
    depths
}

// ADR 0053: stepping through pruned residents, timeouts and hops. `Rig::step` checks every frame
// against the stepping oracle, so these scenarios only pin down when the hops happen.

/// Root 1 (stored 2..=14) with its eight children 2..=9 (stored 5..=14). Only depths 2..=11 of
/// the root are resident, so its active range stops at 11 and stays there once it leaves the cut.
fn hold_rig() -> Rig {
    let mut nodes = octant_nodes();
    nodes.insert(
        NodeId::new(1),
        d5_node(SIDE, &(2..=14).collect::<Vec<u8>>()),
    );
    for id in CHILDREN {
        let child = nodes.get_mut(&NodeId::new(id)).expect("child");
        child.stored = (5..=14).collect();
        child.buildable = (5, 14);
    }
    let mut rig = d5_rig(nodes);
    seed_resident(&mut rig.pool, 1, &(2..=11).collect::<Vec<u8>>());
    rig.pool.residency_epoch += 1;
    rig
}
