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

// R3-Q19: local fallback, coarse to fine and reset.

fn has_entry(plan: &FramePlan, node: u64) -> bool {
    plan.entries.iter().any(|e| e.node == NodeId::new(node))
}

fn coarse_resident(rig: &Rig, node: u64) -> bool {
    let depth = rig.nodes[&NodeId::new(node)].stored[0];
    rig.pool.is_resident(rig.pool.key(NodeId::new(node), depth))
}

#[test]
fn a_region_that_waits_for_a_level_does_not_block_an_unrelated_one() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 256, 64, 1);
    let cam = camera("fit");
    let before = rig.run(10, &ids(&[2, 3]), &cam, 64.0);
    assert_eq!(before.apply, CutApply::Applied);
    // Both regions refine; node 10 of region A never arrives while the rest does.
    let cut: Vec<NodeId> = region_children(10)
        .into_iter()
        .chain(region_children(20))
        .map(NodeId::new)
        .collect();
    rig.held.insert(NodeId::new(10));
    // B draws one frame after the frame that uploads its last page: the cut of that frame was
    // computed before, so `ready` (residency at the start of the frame) is what must apply.
    let (mut resident_after, mut first_drawn) = (None, None);
    for frame in 0..20 {
        let ready = region_children(20)
            .into_iter()
            .all(|c| coarse_resident(&rig, c));
        let plan = rig.step(&cut, &cam, 64.0);
        assert_no_holes(&plan, &nodes);
        assert!(!coarse_resident(&rig, 10));
        let drawn = region_children(20)
            .into_iter()
            .all(|c| plan.active.iter().any(|a| a.key.node == NodeId::new(c)));
        if resident_after.is_none()
            && region_children(20)
                .into_iter()
                .all(|c| coarse_resident(&rig, c))
        {
            resident_after = Some(frame);
        }
        if ready {
            // Same frame: region B draws its new levels although node 10 has none.
            assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
            assert!(drawn, "frame {frame}: region B is not drawn");
        }
        if drawn && first_drawn.is_none() {
            first_drawn = Some(frame);
        }
    }
    assert!(resident_after.is_some());
    assert_eq!(first_drawn, resident_after.map(|frame| frame + 1));
    let plan = rig.step(&cut, &cam, 64.0);
    assert_eq!(plan.apply, CutApply::Applied);
    // Region A keeps its ancestor, region B has none left, and node 10 is counted as a stage.
    assert!(has_entry(&plan, 2) && !has_entry(&plan, 3));
    assert_eq!(plan.entries.len(), cut.len() + 1);
    assert!(plan.counters.stages >= 8, "{}", plan.counters.stages);
    assert_eq!(plan.counters.nodes_without_level, 1);
}

#[test]
fn the_fallback_ancestor_leaves_on_the_first_frame_after_the_node_is_resident() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 256, 64, 1);
    let cam = camera("fit");
    rig.run(10, &ids(&[2, 3]), &cam, 64.0);
    let cut: Vec<NodeId> = region_children(10).into_iter().map(NodeId::new).collect();
    rig.held.insert(NodeId::new(10));
    rig.run(8, &cut, &cam, 64.0);
    rig.held.clear();
    let (mut with, mut without) = (0, 0);
    for frame in 0..12 {
        let resident = coarse_resident(&rig, 10);
        let plan = rig.step(&cut, &cam, 64.0);
        assert_no_holes(&plan, &nodes);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_eq!(has_entry(&plan, 2), !resident, "frame {frame}");
        if resident {
            without += 1;
            assert_eq!(plan.entries.len(), cut.len());
            assert_eq!(plan.counters.nodes_without_level, 0);
        } else {
            with += 1;
        }
    }
    assert!(with > 0 && without > 0, "{with} {without}");
}

#[test]
fn a_missing_node_without_an_ancestor_in_the_applied_cut_is_left_out_until_its_level_arrives() {
    let mut rig = Rig::new(octant_nodes(), 64, 8, 3);
    let cam = camera("fit");
    let held = rig.run(10, &ids(&[2, 3]), &cam, 64.0);
    assert_eq!(held.apply, CutApply::Applied);
    // Node 4 is not below any node of the applied cut: it waits outside the cut (R3-Q20) while
    // node 2 keeps drawing; node 3 left the cut and is not a loss.
    let mut waited = 0;
    let mut plan = rig.step(&ids(&[2, 4]), &cam, 64.0);
    while !has_entry(&plan, 4) {
        assert_eq!(plan.apply, CutApply::Applied);
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0], held.entries[0]);
        assert_eq!(plan.counters.nodes_without_level, 1);
        assert_eq!(plan.counters.lost_nodes, 0);
        waited += 1;
        plan = rig.step(&ids(&[2, 4]), &cam, 64.0);
        assert!(waited < 12);
    }
    assert!(waited > 0);
    assert_eq!(plan.apply, CutApply::Applied);
    assert_eq!(plan.entries.len(), 2);
    assert_eq!(plan.counters.nodes_without_level, 0);
}

#[test]
fn the_fallback_reaches_max_mask_levels_up_but_not_further() {
    let nodes = chain_nodes();
    let mut rig = Rig::new(nodes.clone(), 64, 8, 3);
    let cam = camera("fit");
    let held = rig.run(10, &ids(&[1]), &cam, 64.0);
    assert_eq!(held.apply, CutApply::Applied);
    // Node 20 is `MAX_MASK_LEVELS + 1` levels below the root: the root cannot hide it, so the cut
    // waits.
    let mut retained = 0;
    let mut plan = rig.step(&ids(&[20]), &cam, 64.0);
    while plan.apply == CutApply::RetainedPrevious {
        retained += 1;
        assert_eq!(plan.entries, held.entries);
        plan = rig.step(&ids(&[20]), &cam, 64.0);
        assert!(retained < 12);
    }
    assert!(retained > 0);
    assert!(!has_entry(&plan, 1));
    // Back to the root, then to node 21, `MAX_MASK_LEVELS` below it: the root draws until 21
    // arrives.
    rig.run(10, &ids(&[1]), &cam, 64.0);
    let mut seen_fallback = false;
    let mut seen_alone = false;
    for frame in 0..12 {
        let resident = coarse_resident(&rig, 21);
        let plan = rig.step(&ids(&[21]), &cam, 64.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_no_holes(&plan, &nodes);
        assert_eq!(has_entry(&plan, 1), !resident, "frame {frame}");
        if resident {
            seen_alone = true;
        } else {
            seen_fallback = true;
            assert!(plan.counters.stages > 0);
        }
    }
    assert!(seen_fallback && seen_alone);
}

#[test]
fn the_fallback_never_makes_the_real_cut_too_fine() {
    // Applied {root}; the cut is {Y, Z}: Y is a node one level below the root that never arrives
    // and Z is resident `MAX_MASK_LEVELS + 1` levels below it. The root would cover Y, but with Z
    // in the cut it cannot hide Z: the real cut would fail with `DescendantTooFine`.
    let mut nodes = chain_nodes();
    nodes.insert(
        NodeId::new(22),
        PoolNode {
            bounds: cube([ORIGIN[0] + SIDE / 2.0, ORIGIN[1], ORIGIN[2]], SIDE / 2.0),
            stored: vec![4, 5],
            buildable: (4, 5),
            points: 2000,
            octant_data_bounds: None,
        },
    );
    let mut rig = Rig::new(nodes.clone(), 64, 8, 1);
    let cam = camera("fit");
    // Z becomes resident first (alone), then the root is the applied cut.
    rig.run(8, &ids(&[20]), &cam, 64.0);
    let held = rig.run(10, &ids(&[1]), &cam, 64.0);
    assert_eq!(held.apply, CutApply::Applied);
    assert!(
        rig.pool.is_resident(rig.pool.key(NodeId::new(20), 7)),
        "setup"
    );
    rig.held.insert(NodeId::new(22));
    for frame in 0..10 {
        let plan = rig.step(&ids(&[22, 20]), &cam, 64.0);
        assert!(!plan.counters.descendant_too_fine, "frame {frame}");
        // No valid ancestor: the previous cut stays (a nearer valid ancestor would also do).
        assert_eq!(plan.apply, CutApply::RetainedPrevious, "frame {frame}");
        assert_eq!(plan.entries, held.entries, "frame {frame}");
    }
}

#[test]
fn a_resident_level_beyond_the_fit_cap_is_not_a_fallback() {
    // The root's only resident level is depth 4, but the fit capped it at 3 (as a previous frame's
    // fit could have): it does not draw, so it cannot cover node 21 while that waits.
    let nodes = chain_nodes();
    let mut rig = Rig::new(nodes.clone(), 64, 8, 1);
    let cam = camera("fit");
    let held = rig.run(12, &ids(&[1]), &cam, 1.0);
    assert_eq!(held.apply, CutApply::Applied);
    let (coarse, fine) = (
        rig.pool.key(NodeId::new(1), 3),
        rig.pool.key(NodeId::new(1), 4),
    );
    assert!(rig.pool.is_resident(fine), "setup: the root draws depth 4");
    rig.pool.levels.remove(&coarse);
    rig.pool.residency_epoch += 1;
    rig.pool.caps.insert(NodeId::new(1), 3);
    rig.held.insert(NodeId::new(21));
    let plan = rig.step(&ids(&[21]), &cam, 1.0);
    assert_eq!(plan.apply, CutApply::RetainedPrevious);
    assert_eq!(plan.entries, held.entries);
    assert!(!has_entry(&plan, 21));
}

#[test]
fn a_cap_on_a_dropped_depth_does_not_panic_after_a_recolour() {
    // A fit cap at depth 5 whose level a recolour dropped: the node now stores 6 and 7 only.
    let mut nodes = octant_nodes();
    let node = nodes.get_mut(&NodeId::new(2)).unwrap();
    node.stored = vec![6, 7];
    node.buildable = (6, 7);
    let mut rig = Rig::new(nodes, 128, 8, 0);
    let id = NodeId::new(2);
    rig.pool.caps.insert(id, 5);
    let cam = camera("zoom");
    let plan = rig.run(6, &ids(&[2]), &cam, 1.0);
    assert!(
        plan.entries[0]
            .drawn
            .iter()
            .flatten()
            .all(|d| [6, 7].contains(d))
    );
    // The driver reconciles the cap with what is stored: below every level it clamps to the
    // coarsest one, so no finer level comes back in.
    rig.pool.caps.insert(id, 5);
    rig.pool.reconcile_cap(id, &[6, 7]);
    assert_eq!(rig.pool.caps.get(&id), Some(&6));
    assert_eq!(capped(&[6, 7], rig.pool.caps.get(&id)), vec![6]);
    rig.pool.caps.insert(id, 7);
    rig.pool.reconcile_cap(id, &[4, 6]);
    assert_eq!(
        rig.pool.caps.get(&id),
        Some(&6),
        "clamped to the finest stored below it"
    );
    assert_eq!(capped(&[6, 7], Some(&5)), vec![6], "never empty");
}

#[test]
fn a_far_node_that_draws_nothing_is_not_starved_by_refinements_elsewhere() {
    // Node 8 draws its depth 4 and wants depth 5; node 2 (farther) draws nothing and stores
    // depth 6 and deeper. By absolute depth alone the refinement of node 8 would go first.
    let mut nodes = octant_nodes();
    nodes.get_mut(&NodeId::new(8)).unwrap().stored = vec![4, 5];
    nodes.get_mut(&NodeId::new(8)).unwrap().buildable = (4, 5);
    nodes.get_mut(&NodeId::new(2)).unwrap().stored = vec![6, 7];
    nodes.get_mut(&NodeId::new(2)).unwrap().buildable = (6, 7);
    let mut rig = Rig::new(nodes, 128, 1, 0);
    let cam = camera("zoom");
    assert_eq!(by_distance(&cam, &[2, 8]), vec![8, 2], "setup");
    rig.run(8, &ids(&[8]), &camera("fit"), 64.0);
    assert!(
        rig.pool.is_resident(rig.pool.key(NodeId::new(8), 4)),
        "setup"
    );
    let cut = ids(&[8, 2]);
    // One page per frame: the first frame with data uploads the first level of node 2.
    let requests = rig.step(&cut, &cam, 4.0).fetch_requests;
    let order: Vec<(u64, u8)> = requests.iter().map(|k| (k.node.get(), k.depth)).collect();
    assert!(
        order.contains(&(8, 5)) && order.contains(&(2, 6)),
        "{order:?}"
    );
    assert_eq!(order.first(), Some(&(2, 6)), "{order:?}");
    let uploads = rig.step(&cut, &cam, 4.0).uploads;
    let first = uploads.first().expect("an upload");
    assert_eq!(
        (first.key.node.get(), first.key.depth),
        (2, 6),
        "{uploads:?}"
    );
}

#[test]
fn requests_and_uploads_go_coarse_to_fine() {
    // Node 8 is the nearest and wants depth 7; node 2, farther, stores up to depth 5. By distance
    // alone the depth-7 level of node 8 would come first.
    let mut nodes = octant_nodes();
    nodes.get_mut(&NodeId::new(2)).unwrap().stored = vec![4, 5];
    nodes.get_mut(&NodeId::new(2)).unwrap().buildable = (4, 5);
    let mut rig = Rig::new(nodes, 128, 64, 0);
    let cut = ids(&[8, 2]);
    let cam = camera("zoom");
    assert_eq!(by_distance(&cam, &[2, 8]), vec![8, 2], "setup");
    // Far away both nodes want only their coarsest level.
    rig.run(4, &cut, &camera("fit"), 64.0);
    let requests = rig.step(&cut, &cam, 4.0).fetch_requests;
    let order: Vec<(u64, u8)> = requests.iter().map(|k| (k.node.get(), k.depth)).collect();
    assert!(
        order.contains(&(2, 5)) && order.contains(&(8, 7)),
        "{order:?}"
    );
    assert!(
        requests.windows(2).all(|w| w[0].depth <= w[1].depth),
        "{order:?}"
    );
    let started: Vec<u8> = rig
        .step(&cut, &cam, 4.0)
        .uploads
        .iter()
        .filter(|u| u.page_range.start == 0)
        .map(|u| u.key.depth)
        .collect();
    assert!(
        started.len() >= 2 && started.windows(2).all(|w| w[0] <= w[1]),
        "{started:?}"
    );

    // Nodes that draw nothing: the first level of a parent is requested before the first level of
    // its child, whatever the order of the cut.
    let mut rig = Rig::new(two_regions(), 128, 64, 0);
    let requests = rig.step(&ids(&[10, 2]), &cam, 4.0).fetch_requests;
    let at = |node: u64| requests.iter().position(|k| k.node.get() == node);
    assert!(
        at(2)
            .zip(at(10))
            .is_some_and(|(parent, child)| parent < child),
        "{requests:?}"
    );
}

#[test]
fn reset_evicts_every_level_and_the_pool_converges_back() {
    // `reset` assumes a fresh GPU (device lost, or the renderer dropped its pool), so the fake GPU
    // starts empty: there is no "same GPU" case to model.
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 256, 4, 2);
    let cam = camera("fit");
    let cut = ids(&[2, 3]);
    let fine = {
        let mut cut = region_children(10);
        cut.extend(region_children(20));
        ids(&cut)
    };
    rig.run(10, &cut, &cam, 64.0);
    let before = rig.run(60, &fine, &cam, 4.0);
    let resident = |rig: &Rig| -> Vec<LevelKey> {
        let mut keys: Vec<LevelKey> = rig
            .pool
            .levels
            .iter()
            .filter(|(_, l)| l.is_resident())
            .map(|(k, _)| *k)
            .collect();
        keys.sort();
        keys
    };
    let keys = resident(&rig);
    let parent_pages: u32 = keys
        .iter()
        .filter(|k| k.node.get() <= 3)
        .map(|k| rig.pool.level(*k).unwrap().pages.len() as u32)
        .sum();
    assert!(keys.len() > 16, "setup: {} levels", keys.len());

    let mut events = rig.pool.reset();
    events.sort_by_key(|event| match event {
        CellPoolEvent::Evict(key) | CellPoolEvent::Cancel(key) => *key,
    });
    let expected: Vec<CellPoolEvent> = keys.iter().map(|k| CellPoolEvent::Evict(*k)).collect();
    assert_eq!(events, expected);
    assert_eq!(rig.pool.resident_pages(), 0);
    // Every page is free at once: nothing is delayed.
    assert_eq!(rig.pool.free.len(), 256);
    assert!(rig.pool.pending.is_empty());
    rig.inflight.clear();
    rig.gpu = FakeGpu::default();

    let mut empty_frames = 0;
    let mut mask_slots = HashSet::new();
    let mut last = None;
    for _ in 0..80 {
        let plan = rig.step(&fine, &cam, 4.0);
        // Nothing is drawn before the cut is complete again, and never with a hole.
        assert_no_holes(&plan, &nodes);
        assert!(plan.page_frees.is_empty(), "the old GPU pages are gone");
        mask_slots.extend(plan.mask_updates.iter().map(|update| update.slot));
        if plan.entries.is_empty() {
            empty_frames += 1;
        }
        last = Some(plan);
    }
    assert!(empty_frames > 0);
    // A new renderer hands mask slots out from 0, with no gap.
    assert_eq!(
        mask_slots,
        (0..mask_slots.len() as u32).collect::<HashSet<u32>>()
    );
    let after = last.unwrap();
    assert_eq!(after.apply, CutApply::Applied);
    assert_eq!(after.entries, before.entries);
    // The coarse levels of the parents were not pinned by the cut any more.
    let children: Vec<LevelKey> = keys.iter().filter(|k| k.node.get() > 3).copied().collect();
    assert_eq!(resident(&rig), children);
    for (name, a, b) in [
        ("stages", after.counters.stages, before.counters.stages),
        (
            "nodes_without_level",
            after.counters.nodes_without_level,
            before.counters.nodes_without_level,
        ),
        (
            "pool_coarsened",
            after.counters.pool_coarsened,
            before.counters.pool_coarsened,
        ),
        (
            "pool_overflow",
            after.counters.pool_overflow,
            before.counters.pool_overflow,
        ),
        (
            "resident_pages",
            after.counters.resident_pages,
            before.counters.resident_pages - parent_pages,
        ),
    ] {
        assert_eq!(a, b, "{name}");
    }
    rig.assert_gpu_matches_pool();
}

#[test]
fn reset_cancels_requests_and_evicts_uploading_levels() {
    let mut rig = Rig::new(octant_nodes(), 128, 1, 4);
    let cam = camera("zoom");
    let cut = ids(&CHILDREN);
    rig.run(14, &cut, &cam, 4.0);
    let (mut evict, mut cancel) = (Vec::new(), Vec::new());
    for (key, level) in &rig.pool.levels {
        match level.state {
            LevelState::Requested => cancel.push(*key),
            _ => evict.push(*key),
        }
    }
    assert!(!evict.is_empty() && !cancel.is_empty(), "setup");
    assert!(!rig.gpu.partial().is_empty(), "setup: an upload in flight");
    let events = rig.pool.reset();
    let evicted: HashSet<LevelKey> = events
        .iter()
        .filter_map(|e| match e {
            CellPoolEvent::Evict(k) => Some(*k),
            CellPoolEvent::Cancel(_) => None,
        })
        .collect();
    let cancelled: HashSet<LevelKey> = events
        .iter()
        .filter_map(|e| match e {
            CellPoolEvent::Cancel(k) => Some(*k),
            CellPoolEvent::Evict(_) => None,
        })
        .collect();
    assert_eq!(evicted, evict.into_iter().collect());
    assert_eq!(cancelled, cancel.into_iter().collect());
    assert!(rig.pool.levels.is_empty());
    // The GPU is fresh, so the slots of the half-uploaded levels are free at once, with no
    // `page_frees`, and the pool keeps all its pages.
    assert_eq!(rig.pool.free.len(), 128);
    assert!(rig.pool.pending.is_empty());
    rig.inflight.clear();
    rig.gpu = FakeGpu::default();
    for _ in 0..4 {
        assert!(rig.step(&cut, &cam, 4.0).page_frees.is_empty());
    }
    rig.assert_gpu_matches_pool();
    assert_eq!(
        rig.pool.free.len() + rig.pool.pending_pages() + rig.gpu.owner.len(),
        128
    );
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

/// Does the mask of `node` hide `point`?
fn masked_at(
    plan: &FramePlan,
    nodes: &HashMap<NodeId, PoolNode>,
    node: u64,
    point: [f64; 3],
) -> bool {
    let entry = plan
        .entries
        .iter()
        .find(|entry| entry.node == NodeId::new(node))
        .expect("entry");
    let at = cell_of(nodes[&entry.node].bounds, point).expect("point in node");
    entry.mask[at[2]] >> (at[0] + 8 * at[1]) & 1 == 1
}

/// Resident levels of the nodes, as the cut of the next `step` reads them (caps applied).
fn resident_levels(rig: &Rig) -> HashMap<NodeId, Vec<u8>> {
    rig.nodes
        .iter()
        .map(|(id, node)| {
            let levels = capped(&node.stored, rig.pool.caps.get(id))
                .into_iter()
                .filter(|depth| rig.pool.is_resident(rig.pool.key(*id, *depth)))
                .collect();
            (*id, levels)
        })
        .collect()
}

/// Depth the plan really draws at `point`: the unhidden octant of an entry that has a resident
/// level in `active` there (geometry computed here). `resident` is `resident_levels` taken
/// before the step: each drawn depth must be one of them, and no point may be drawn by two
/// entries.
fn drawn_at(
    plan: &FramePlan,
    nodes: &HashMap<NodeId, PoolNode>,
    resident: &HashMap<NodeId, Vec<u8>>,
    point: [f64; 3],
) -> Option<u8> {
    let depths: Vec<u8> = plan
        .entries
        .iter()
        .filter_map(|entry| {
            let at = cell_of(nodes[&entry.node].bounds, point)?;
            if entry.mask[at[2]] >> (at[0] + 8 * at[1]) & 1 == 1 {
                return None;
            }
            let octant = ((at[0] / 4) << 2) | ((at[1] / 4) << 1) | (at[2] / 4);
            plan.active
                .iter()
                .any(|a| a.key.node == entry.node && a.octant_bits >> octant & 1 == 1)
                .then(|| entry.drawn[octant])
                .flatten()
                .inspect(|depth| {
                    // The octant may be stepping (ADR 0053): it draws a resident level, and the
                    // stepping rule has its own tests.
                    assert!(
                        resident[&entry.node].contains(depth),
                        "node {:?} octant {octant}",
                        entry.node
                    );
                })
        })
        .collect();
    assert!(depths.len() <= 1, "{point:?} is drawn by {depths:?}");
    depths.first().copied()
}

/// `rig.run` that also returns the resident levels the last frame planned with.
fn run_with_resident(
    rig: &mut Rig,
    frames: usize,
    cut: &[NodeId],
    camera: &OrbitCamera,
    target: f64,
) -> (FramePlan, HashMap<NodeId, Vec<u8>>) {
    rig.run(frames - 1, cut, camera, target);
    let resident = resident_levels(rig);
    (rig.step(cut, camera, target), resident)
}

fn resident_at(rig: &Rig, node: u64, depth: u8) -> bool {
    rig.pool.is_resident(rig.pool.key(NodeId::new(node), depth))
}

/// Region A drew depth 6 (8 px) and refines: in the fallback variant the cut becomes its eight
/// children (node 2 is a fallback ancestor), in the other it keeps node 2 and adds child 10.
fn only_refine(in_cut: bool) {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    let cam = camera("fit");
    let (before, resident) = run_with_resident(&mut rig, 60, &ids(&[2, 3]), &cam, 8.0);
    assert_eq!(before.apply, CutApply::Applied);
    assert_eq!(depth_of(&before, 2), [Some(6); 8], "setup");
    let points = sample_points(nodes[&NodeId::new(2)].bounds);
    let base: Vec<Option<u8>> = points
        .iter()
        .map(|p| drawn_at(&before, &nodes, &resident, *p))
        .collect();
    assert!(base.iter().all(|d| *d == Some(6)), "setup");
    let (cut, watched): (Vec<NodeId>, Vec<u64>) = if in_cut {
        (ids(&[2, 10, 3]), vec![10])
    } else {
        let mut cut = ids(&region_children(10));
        cut.push(NodeId::new(3));
        (cut, region_children(10))
    };
    let (mut saw_held, mut saw_taken_over) = (false, false);
    for frame in 0..40 {
        let ready: Vec<bool> = watched.iter().map(|c| resident_at(&rig, *c, 6)).collect();
        let coarse: Vec<bool> = watched.iter().map(|c| resident_at(&rig, *c, 5)).collect();
        let resident = resident_levels(&rig);
        let plan = rig.step(&cut, &cam, 8.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_no_holes(&plan, &nodes);
        assert!(!plan.counters.descendant_too_fine, "frame {frame}");
        assert_eq!(plan.counters.hold_unsettled, 0, "frame {frame}");
        // No zone is ever drawn coarser than before.
        for (point, was) in points.iter().zip(&base) {
            let now = drawn_at(&plan, &nodes, &resident, *point);
            assert!(now >= *was, "frame {frame}: {point:?} {now:?} < {was:?}");
        }
        // A child draws exactly when its level 6 (the ancestor's depth and its wanted one) is
        // resident, and a coarser one is held (it neither draws nor hides node 2).
        let mut missing = 0;
        let mut held = 0;
        for (index, child) in watched.iter().enumerate() {
            let drawn = depth_of(&plan, *child);
            assert_eq!(
                drawn[0].is_some(),
                ready[index],
                "frame {frame} node {child}"
            );
            let centre = sample_points(nodes[&NodeId::new(*child)].bounds)[13];
            if has_entry(&plan, 2) {
                assert_eq!(
                    masked_at(&plan, &nodes, 2, centre),
                    ready[index],
                    "frame {frame} node {child}"
                );
            }
            missing += usize::from(!ready[index]);
            held += usize::from(coarse[index] && !ready[index]);
            saw_taken_over |= ready[index];
        }
        saw_held |= held > 0;
        assert_eq!(plan.counters.held_nodes as usize, held, "frame {frame}");
        assert_eq!(plan.counters.nodes_without_level as usize, missing);
        assert_eq!(has_entry(&plan, 2), in_cut || missing > 0, "frame {frame}");
        if missing == 0 {
            assert_eq!(plan.counters.stages, 0);
            assert_eq!(plan.counters.fallback_pages, 0);
        } else {
            assert!(plan.counters.stages as usize >= 8 * held, "frame {frame}");
            let pages = rig.pages(rig.pool.key(NodeId::new(2), 6)) as u32;
            let covers = if in_cut { held > 0 } else { true };
            assert_eq!(plan.counters.fallback_pages, if covers { pages } else { 0 });
        }
    }
    assert!(saw_held && saw_taken_over);
}

#[test]
fn a_child_whose_first_level_is_coarser_than_the_ancestor_drew_is_held() {
    only_refine(false);
}

#[test]
fn a_child_in_the_cut_with_its_ancestor_is_held_the_same_way() {
    only_refine(true);
}

/// `two_regions` whose children of node 2 also store depth 4, so they have a level coarser than
/// what the ancestor drew to be held with.
fn two_regions_with_coarse_children() -> HashMap<NodeId, PoolNode> {
    let mut nodes = two_regions();
    let parent = nodes[&NodeId::new(2)].clone();
    nodes.extend(octants_of(&parent, 10, &[4, 5, 6, 7]));
    nodes
}

/// Region A drew depth 6 at 8 px; its children (which also store depth 4) replace it at
/// `target` px with room for `capacity` pages. A child is held when what it would draw is below
/// `min(what node 2 drew, what the child wants)`; the test recomputes that from the resident
/// levels (`resident_depth`) and the plan's wanted depths, and checks the image never gets
/// coarser than the ancestor drew while the children take over within `limit` frames.
fn takes_over_at(capacity: u32, target: f64, expect_held: bool, limit: u64) {
    let nodes = two_regions_with_coarse_children();
    let mut rig = Rig::new(nodes.clone(), capacity, 3, 1);
    let cam = camera("fit");
    let mut previous = rig.run(60, &ids(&[2, 3]), &cam, 8.0);
    assert_eq!(depth_of(&previous, 2), [Some(6); 8], "setup");
    let mut cut = ids(&region_children(10));
    cut.push(NodeId::new(3));
    let (mut gone, mut saw_held) = (None, false);
    for frame in 0..60 {
        let resident = resident_levels(&rig);
        let plan = rig.step(&cut, &cam, target);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_no_holes(&plan, &nodes);
        let mut held = 0;
        for child in region_children(10) {
            // Node 2 drew this child's zone at its octant (child bits are x, y, z from the low).
            let index = child - 10;
            let octant = ((index & 1) << 2 | (index >> 1 & 1) << 1 | (index >> 2 & 1)) as usize;
            let ancestor = has_entry(&previous, 2).then(|| depth_of(&previous, 2)[octant]);
            let entry = plan.entries.iter().find(|e| e.node.get() == child).unwrap();
            let id = entry.node;
            let wanted = entry.wanted[0].unwrap();
            let would = crate::resident_depth(wanted, &resident[&id]).map(|(d, _)| d);
            let short = match (ancestor.flatten(), would) {
                (Some(a), Some(d)) => d < a.min(wanted),
                _ => false,
            };
            held += usize::from(short);
            assert_eq!(entry.drawn[0].is_some(), would.is_some() && !short);
        }
        assert_eq!(plan.counters.held_nodes as usize, held, "frame {frame}");
        saw_held |= held > 0 && has_entry(&plan, 2);
        if !has_entry(&plan, 2) && gone.is_none() {
            gone = Some(frame);
            assert_eq!(plan.counters.nodes_without_level, 0);
        }
        previous = plan;
    }
    assert_eq!(saw_held, expect_held, "a child was held while node 2 drew");
    assert!(gone.is_some_and(|frame| frame < limit), "{gone:?}");
}

#[test]
fn a_child_that_wants_less_than_the_ancestor_drew_takes_over_at_its_wanted_level() {
    // At 16 px the children want depth 5 < the 6 the ancestor drew; the ancestor itself wants 4
    // now. While its level 4 is not resident it keeps drawing 6 (R3-Q21: the image only refines,
    // a finer level meets the target), so the children whose only level is 4 are held behind it
    // until their 5 arrives, and then take over at their wanted level.
    takes_over_at(512, 16.0, true, 15);
}

#[test]
fn a_child_whose_fit_cap_is_below_the_ancestor_depth_takes_over_at_the_cap() {
    // The pool cannot hold depth 6 for the farthest children: their cap (5) is the threshold,
    // and the nearest ones still want 6.
    takes_over_at(40, 8.0, true, 30);
}

#[test]
fn the_hold_follows_what_would_be_drawn_and_not_mere_residency() {
    // Node 10 wants 6 (the depth node 2 drew). With only 5 resident (the closest level, below
    // 6) it is held; levels of the node that are not resident do not count.
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    let cam = camera("fit");
    let before = rig.run(60, &ids(&[2, 3]), &cam, 8.0);
    assert_eq!(depth_of(&before, 2), [Some(6); 8], "setup");
    let mut cut = ids(&region_children(10));
    cut.push(NodeId::new(3));
    rig.held.insert(NodeId::new(10));
    seed_resident(&mut rig.pool, 10, &[5]);
    let points = sample_points(nodes[&NodeId::new(10)].bounds);
    for frame in 0..12 {
        let resident = resident_levels(&rig);
        let plan = rig.step(&cut, &cam, 8.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert!(rig.pool.hold.contains(&NodeId::new(10)), "frame {frame}");
        assert_eq!(depth_of(&plan, 10), [None; 8]);
        for point in &points {
            let now = drawn_at(&plan, &nodes, &resident, *point);
            assert!(now >= Some(6), "frame {frame}");
        }
    }
    rig.held.clear();
    let mut taken_over = false;
    for _ in 0..12 {
        let resident = resident_levels(&rig);
        let plan = rig.step(&cut, &cam, 8.0);
        assert!(
            points
                .iter()
                .all(|p| drawn_at(&plan, &nodes, &resident, *p) >= Some(6))
        );
        taken_over |= depth_of(&plan, 10)[0] == Some(6);
    }
    assert!(taken_over);
}

#[test]
fn the_closest_resident_level_decides_whether_a_node_is_held() {
    // Node 2 drew 6 and node 10 wants 6.
    for (resident, held) in [
        (vec![5_u8], true),
        (vec![5, 7], false),
        (vec![4, 5], true),
        (vec![6], false),
        (vec![5, 6], false),
        (vec![7], false),
        (vec![7, 9], false),
    ] {
        let mut rig = rig_with_coarse_children(None);
        let key = rig.pool.key(NodeId::new(10), 5);
        rig.pool.levels.remove(&key);
        seed_resident(&mut rig.pool, 10, &resident);
        let (eval, _) = eval_with(&rig, &ids(&[10]), 6);
        assert_eq!(eval.hold.contains(&NodeId::new(10)), held, "{resident:?}");
    }
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

/// A rig that drew `[2, 3]` (node 2 at depth 6) and whose children 10 and 17 of node 2 hold a
/// resident level 5.
fn rig_with_coarse_children(extra: Option<(u64, PoolNode)>) -> Rig {
    let mut nodes = two_regions();
    nodes.extend(extra.map(|(id, node)| (NodeId::new(id), node)));
    let mut rig = Rig::new(nodes, 512, 3, 1);
    rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
    for child in [10, 17] {
        seed_resident(&mut rig.pool, child, &[5]);
    }
    rig.pool.residency_epoch += 1;
    rig
}

#[test]
fn only_a_node_with_an_ancestor_the_fallback_would_use_can_be_held() {
    let rig = rig_with_coarse_children(None);
    let (eval, fallback) = eval_with(&rig, &ids(&[10]), 7);
    assert!(eval.hold.contains(&NodeId::new(10)));
    assert_eq!(
        fallback,
        ids(&[2]),
        "the held node's ancestor is in the real cut"
    );
    // A resident cut node four levels below node 2 rules it out as a fallback (it would make the
    // real cut too fine), so the node cannot be held on it either: it would draw nothing.
    let deep = (
        50,
        PoolNode {
            bounds: cube([ORIGIN[0] + SIDE / 4.0, ORIGIN[1], ORIGIN[2]], SIDE / 32.0),
            stored: vec![8, 9, 10],
            buildable: (8, 10),
            points: 2000,
            octant_data_bounds: None,
        },
    );
    let mut rig = rig_with_coarse_children(Some(deep));
    seed_resident(&mut rig.pool, 50, &[8]);
    let (eval, fallback) = eval_with(&rig, &ids(&[10, 50]), 7);
    assert!(
        eval.hold.is_empty() && eval.settle.is_empty(),
        "{:?}",
        eval.hold
    );
    assert!(fallback.is_empty());
}

#[test]
fn in_cell_mode_the_hold_threshold_is_the_finest_depth_active_in_the_ancestor() {
    // The GPU picks the depth per cell among the active ones of a node, so node 2 may have drawn
    // its finest active depth (6) over an octant whose nominal depth is 5. Child 10 stores 5 and
    // 6, has only 5 resident and wants 6: short of what was really drawn there, held.
    let nodes = two_regions_with_coarse_children();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
    let child = nodes[&NodeId::new(10)].clone();
    seed_resident(&mut rig.pool, 10, &[child.stored[0]]);
    rig.pool.residency_epoch += 1;
    let nominal = child.stored[0];
    rig.pool
        .applied_drawn
        .insert(NodeId::new(2), [Some(nominal); 8]);
    rig.pool.applied_finest.insert(NodeId::new(2), nominal + 1);
    let (eval, _) = eval_with(&rig, &ids(&[10]), nominal + 1);
    assert!(eval.hold.contains(&NodeId::new(10)));
}

#[test]
fn the_hold_threshold_counts_the_d5_depth_above_the_run_of_the_ancestor() {
    // Node 2 drew its run at the nominal depth (5) and its D5 depth two levels above (7): the
    // GPU can draw 7 over a cell whose own run refines, so child 10 (5 resident, wants 6) would
    // coarsen it and is held. The run alone (B3) would say 5 drew before and release it; that
    // is only for the anchor of `node_run`, and `before` for the anchor stays without D5.
    let nodes = two_regions_with_coarse_children();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
    let child = nodes[&NodeId::new(10)].clone();
    seed_resident(&mut rig.pool, 10, &[child.stored[0]]);
    rig.pool.residency_epoch += 1;
    let nominal = child.stored[0];
    rig.pool
        .applied_drawn
        .insert(NodeId::new(2), [Some(nominal); 8]);
    rig.pool.applied_finest.insert(NodeId::new(2), nominal + 2);
    rig.pool
        .applied_run
        .insert(NodeId::new(2), (nominal, nominal));
    let id = NodeId::new(10);
    let (eval, _) = eval_with(&rig, &ids(&[10]), nominal + 1);
    assert!(eval.hold.contains(&id), "the D5 depth drew over it");
    assert_eq!(
        eval.before.get(&id),
        Some(&nominal),
        "the anchor's `before` does not count D5"
    );
    rig.pool.applied_finest.insert(NodeId::new(2), nominal);
    let (eval, _) = eval_with(&rig, &ids(&[10]), nominal + 1);
    assert!(!eval.hold.contains(&id), "without the D5 depth it releases");
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

/// What the judges say about one case. `eval_holds` is the production decision (`eval_hold`),
/// `old_holds` the per-octant hold of before (`eval_refine_hold`), `gate_releases` is `run_covers`
/// on the real run, `oracle` releases it (the twin).
struct Verdict {
    eval_holds: bool,
    old_holds: bool,
    gate_releases: bool,
    oracle: Result<(), String>,
    run: Option<Run>,
}

/// Where `eval_refine_hold` and `run_covers` part ways.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Parts {
    /// `eval_refine_hold` holds, the gate (and the twin) release: the incumbent is conservative.
    EvalHoldsLonger,
    /// `eval_refine_hold` releases, the gate holds: the node would coarsen a zone.
    EvalReleasesEarly,
}

fn parts(verdict: &Verdict) -> Option<Parts> {
    match (verdict.old_holds, verdict.gate_releases) {
        (true, true) => Some(Parts::EvalHoldsLonger),
        (false, false) => Some(Parts::EvalReleasesEarly),
        _ => None,
    }
}

const FARS: [Far; 2] = [Far::Wide, Far::Near];

/// Zoom in: node 2 drew with nominal depth `finest - 1` and finest active depth `finest` (`low`:
/// half of its octants drew `finest - 2`, so its active set is `finest - 2..=finest`); its child
/// 10 (stored 4..=7) enters the cut with the resident levels `resident` and wants `wish`.
fn zoom_in_case((finest, low, wish, resident): (u8, bool, u8, &[u8]), far: Far) -> Verdict {
    let mut rig = Rig::new(two_regions_with_coarse_children(), 512, 3, 1);
    rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
    set_applied(&mut rig, 2, finest);
    if low {
        let drawn = std::array::from_fn(|o| Some(finest - 1 - u8::from(o >= 4)));
        rig.pool.applied_drawn.insert(NodeId::new(2), drawn);
    }
    set_resident(&mut rig, 10, resident);
    let probe = refine_probe(&mut rig, 10, wish, far);
    let grid = grid();
    let ancestor_depth = CellPool::node_depth(&grid, rig.nodes[&NodeId::new(2)].bounds);
    assert_eq!(
        probe.node_depth,
        ancestor_depth + 1,
        "node 10 is a child of 2"
    );
    let before = Owner {
        node_depth: ancestor_depth,
        lo: [0.0, 0.0, 10.0],
        active: (finest - 1 - u8::from(low)..=finest).collect(),
    };
    let after = Owner {
        node_depth: probe.node_depth,
        lo: before.child_lo(0),
        active: probe.active.clone(),
    };
    let samples: Vec<_> = (0..8)
        .map(|o| (before.clone(), after.sub_centre(o)))
        .collect();
    let run = probe.run();
    let low_run = finest - 1 - u8::from(low);
    Verdict {
        eval_holds: probe.held,
        old_holds: probe.old_held,
        gate_releases: run_covers(run, &[(low_run, finest)], wish),
        oracle: twin_releases(&after, wish, &samples),
        run,
    }
}

/// Zoom out: the children 10..=17 of node 2 drew with nominal depth `finest - 1` and finest
/// active depth `finest` (`mixed`: only child 10 had `finest`, the others 5, so the octants of
/// node 2 differ); node 2 (stored 4..=7, never loaded) has `resident` levels and wants `wish`.
fn zoom_out_case((finest, mixed, wish, resident): (u8, bool, u8, &[u8]), far: Far) -> Verdict {
    let (mut rig, _, _) = zoom_out_rig();
    set_resident(&mut rig, 2, resident);
    let finest_of = |child: usize| if mixed && child != 10 { 5 } else { finest };
    for child in 10..18 {
        set_applied(&mut rig, child as u64, finest_of(child));
    }
    let probe = refine_probe(&mut rig, 2, wish, far);
    let grid = grid();
    let child_depth = CellPool::node_depth(&grid, rig.nodes[&NodeId::new(10)].bounds);
    assert_eq!(
        child_depth,
        probe.node_depth + 1,
        "10..=17 are children of 2"
    );
    let after = Owner {
        node_depth: probe.node_depth,
        lo: [0.0, 0.0, 10.0],
        active: probe.active.clone(),
    };
    let mut samples = Vec::new();
    let mut runs = Vec::new();
    for child in 10..18 {
        let f = finest_of(child);
        let owner = Owner {
            node_depth: child_depth,
            lo: after.child_lo(child - 10),
            active: vec![f - 1, f],
        };
        samples.extend((0..8).map(|o| (owner.clone(), owner.sub_centre(o))));
        runs.push((f - 1, f));
    }
    let run = probe.run();
    Verdict {
        eval_holds: probe.held,
        old_holds: probe.old_held,
        gate_releases: run_covers(run, &runs, wish),
        oracle: twin_releases(&after, wish, &samples),
        run,
    }
}

/// `(finest depth the ancestor drew, wish of the child, resident levels of the child, held)`.
/// The observed rule, with `m = min(finest, wish)` and `d` the depth the child would draw
/// (seeded at `finest`, one step towards the wish, else the resident level closest to the wish):
/// the child is held iff `d < m`. The `finest` is the finest ACTIVE depth of the ancestor, not
/// the nominal depth it drew (always `finest - 1` here), and the child may take over below what
/// it wants: reaching `finest` is enough, one level more is not asked.
const ZOOM_IN: &[(u8, u8, &[u8], bool)] = &[
    (6, 7, &[5], true),
    (6, 7, &[4, 5], true),
    (6, 7, &[6], false),
    (6, 7, &[7], false),
    (6, 7, &[4, 7], false),
    (6, 7, &[5, 7], false),
    (6, 6, &[5], true),
    (6, 6, &[4, 5], true),
    (6, 6, &[6], false),
    (6, 6, &[4, 7], false),
    // The wish below what the ancestor drew bounds the threshold: it takes over at the wish.
    (6, 5, &[4], true),
    (6, 5, &[4, 7], true),
    (6, 5, &[5], false),
    (6, 5, &[5, 7], false),
    (7, 7, &[6], true),
    (7, 7, &[4, 5], true),
    (7, 7, &[7], false),
    (7, 7, &[4, 7], false),
    (7, 4, &[4], false),
    (5, 7, &[4], true),
    (5, 7, &[5], false),
    // The ancestor drew finer than the child can build (7): the wish bounds it again.
    (8, 7, &[6], true),
    (8, 7, &[7], false),
];

/// Zoom-in rows whose ancestor has octants that drew different depths (`low`): `(finest, wish,
/// resident, held)` as in `ZOOM_IN`, the ancestor's active set being `finest - 2..=finest`.
const ZOOM_IN_LOW: &[(u8, u8, &[u8], bool)] = &[(6, 7, &[5], true)];

/// The zoom-in rows where `eval_refine_hold` and `run_covers` differ, and how. The twin oracle
/// agrees with `run_covers` on every row (`assert_judged`).
const ZOOM_IN_PARTS: &[(InCase, Parts)] = &[];

/// The zoom-out rows where `eval_refine_hold` and `run_covers` differ, and how.
const ZOOM_OUT_PARTS: &[(OutCase, Parts)] = &[
    ((7, true, 7, &[4, 6, 7]), Parts::EvalHoldsLonger),
    ((7, true, 7, &[4, 7]), Parts::EvalHoldsLonger),
    ((7, true, 6, &[4, 6]), Parts::EvalHoldsLonger),
    ((7, true, 6, &[4, 6, 7]), Parts::EvalHoldsLonger),
    ((6, true, 6, &[4, 6]), Parts::EvalHoldsLonger),
];

/// `(finest, low, wish, resident)` of a zoom-in row, see `zoom_in_case`.
type InCase = (u8, bool, u8, &'static [u8]);
/// `(finest, mixed, wish, resident)` of a zoom-out row, see `zoom_out_case`.
type OutCase = (u8, bool, u8, &'static [u8]);

/// Checks one verdict: the production decision releases the node iff the twin oracle does (and
/// iff `run_covers` does), and `eval_refine_hold` held the node iff `held`. Returns where
/// `eval_refine_hold` and the gate part ways.
fn assert_judged(case: &str, verdict: &Verdict, held: bool) -> Option<Parts> {
    let run = verdict.run;
    assert!(run.is_some(), "{case}: the node must have a run");
    assert_eq!(
        verdict.old_holds, held,
        "{case}: eval_refine_hold, run {run:?}"
    );
    assert_eq!(
        verdict.oracle.is_ok(),
        !verdict.eval_holds,
        "{case}: eval_hold and the twin disagree, run {run:?}: {:?}",
        verdict.oracle
    );
    assert_eq!(
        verdict.gate_releases, !verdict.eval_holds,
        "{case}: eval_hold and run_covers disagree, run {run:?}"
    );
    parts(verdict)
}

/// A row listed in a `*_PARTS` table parts ways in the direction given under some active rule
/// and range and never in the other; any other row never does. The active run depends on the
/// rule and the range, `eval_refine_hold` does not.
fn assert_parts<C: std::fmt::Debug + PartialEq>(
    case: &C,
    table: &[(C, Parts)],
    seen: &[Option<Parts>],
) {
    let expected = table.iter().find(|(c, _)| c == case).map(|(_, p)| *p);
    match expected {
        None => assert!(seen.iter().all(Option::is_none), "{case:?}: {seen:?}"),
        Some(p) => assert!(
            seen.contains(&Some(p)) && seen.iter().all(|s| s.is_none() || *s == Some(p)),
            "{case:?}: expected {p:?}, saw {seen:?}"
        ),
    }
}

/// Judges one case under every range.
fn judge(case: &str, held: bool, run: &dyn Fn(Far) -> Verdict) -> Vec<Option<Parts>> {
    FARS.iter()
        .map(|far| assert_judged(&format!("{far:?} {case}"), &run(*far), held))
        .collect()
}

#[test]
fn zoom_in_a_child_is_held_while_it_would_draw_below_the_ancestors_finest_depth() {
    for (finest, wish, resident, held) in ZOOM_IN {
        let case: InCase = (*finest, false, *wish, *resident);
        let seen = judge(&format!("{case:?}"), *held, &|far| zoom_in_case(case, far));
        assert_parts(&case, ZOOM_IN_PARTS, &seen);
    }
}

#[test]
fn zoom_in_with_octants_that_drew_different_depths_is_judged_by_the_twin() {
    for (finest, wish, resident, held) in ZOOM_IN_LOW {
        let case: InCase = (*finest, true, *wish, *resident);
        let seen = judge(&format!("{case:?}"), *held, &|far| zoom_in_case(case, far));
        assert_parts(&case, ZOOM_IN_PARTS, &seen);
    }
}

#[test]
fn zoom_out_a_node_is_held_while_it_would_draw_below_what_its_descendants_drew() {
    for (finest, mixed, wish, resident, held) in ZOOM_OUT {
        let case: OutCase = (*finest, *mixed, *wish, *resident);
        let seen = judge(&format!("{case:?}"), *held, &|far| zoom_out_case(case, far));
        assert_parts(&case, ZOOM_OUT_PARTS, &seen);
    }
}

/// `(finest depth of the descendants, mixed, wish of the node, resident levels, held)`. The
/// observed rule: with `m = min(finest over its octants, wish)` and `d` the depth the node would
/// draw (seeded at the coarsest nominal depth of the descendants over the octant, one step
/// towards the wish), the node is held iff some octant has `d < m`.
const ZOOM_OUT: &[(u8, bool, u8, &[u8], bool)] = &[
    (6, false, 7, &[5], true),
    (6, false, 7, &[4, 5], true),
    (6, false, 7, &[5, 7], true),
    (6, false, 7, &[6], false),
    (6, false, 7, &[7], false),
    (6, false, 7, &[4, 7], false),
    (6, false, 6, &[5], true),
    (6, false, 6, &[5, 7], true),
    (6, false, 6, &[6], false),
    (6, false, 6, &[4, 7], false),
    (6, false, 5, &[4], true),
    (6, false, 5, &[4, 7], true),
    (6, false, 5, &[5], false),
    (6, false, 5, &[5, 7], false),
    (7, false, 7, &[6], true),
    (7, false, 7, &[4, 5], true),
    (7, false, 7, &[7], false),
    (7, false, 7, &[5, 7], false),
    (7, false, 6, &[5], true),
    (7, false, 6, &[6], false),
    (7, false, 6, &[5, 7], false),
    (5, false, 7, &[4], true),
    (5, false, 7, &[5], false),
    (7, false, 4, &[4], false),
    // One octant over a descendant that drew 7, the rest 5: the finest one counts.
    (7, true, 7, &[6], true),
    (7, true, 7, &[5, 6], true),
    (7, true, 7, &[7], false),
    (7, true, 5, &[5], false),
    // Per-octant mixed: one descendant drew `finest`, the others 5, the node has gaps in its
    // levels (review of task 8: node 2 stored 4..=7, resident {4, 6, 7}).
    (7, true, 7, &[4, 6, 7], true),
    (7, true, 6, &[4, 6], true),
    (7, true, 6, &[4, 6, 7], true),
    (7, true, 7, &[4, 7], true),
    (7, true, 5, &[4, 5], false),
    (6, true, 7, &[4, 5], true),
    (6, true, 7, &[4, 5, 7], true),
    (6, true, 6, &[4, 6], true),
    (6, true, 6, &[4, 5], true),
    // The descendants drew 5 and the node holds the levels above: its run end follows the
    // resident levels (`the_range_moves_the_run_ends`).
    (5, false, 7, &[5, 6, 7], false),
    (5, true, 7, &[4, 5, 6, 7], false),
];

/// `eval_hold` for `id` as if the previous plan already held it. `covering` are the nodes whose
/// entries `cell_cut` appends after the cut because they draw its zone meanwhile (the ancestor on
/// a zoom in, the descendants on a zoom out).
fn eval_held(rig: &Rig, id: u64, covering: &[u64], wish: u8) -> HoldEval {
    let id = NodeId::new(id);
    let nodes: FastMap<NodeId, PoolNode> = rig
        .nodes
        .iter()
        .map(|(id, node)| (*id, node.clone()))
        .collect();
    let mut entries = entries_with(rig, &[id], wish);
    set_range(rig, &mut entries[0], Far::Wide);
    for node in covering {
        entries.extend(entries_with(rig, &[NodeId::new(*node)], wish));
    }
    let held: FastSet<NodeId> = [id].into_iter().collect();
    rig.pool
        .eval_hold(&entries, &[id], &nodes, &grid(), &held, &|_, _, _| wish)
}

/// The hysteresis branch of `eval_refine_hold` (`if held && ...`): a node held by the previous
/// plan is judged like a fresh one while something draws its zone (so the table rows hold for
/// it too, and it stays in `settle`); once nothing draws there it is released.
#[test]
fn a_node_already_held_is_judged_like_a_fresh_one_while_its_zone_is_drawn() {
    for (finest, wish, resident, held) in ZOOM_IN {
        let mut rig = Rig::new(two_regions_with_coarse_children(), 512, 3, 1);
        rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
        set_applied(&mut rig, 2, *finest);
        set_resident(&mut rig, 10, resident);
        let case = format!("zoom in {finest} {wish} {resident:?}");
        let covered = eval_held(&rig, 10, &[2], *wish);
        assert_eq!(covered.hold.contains(&NodeId::new(10)), *held, "{case}");
        assert!(covered.settle.contains(&NodeId::new(10)), "{case}");
        let alone = eval_held(&rig, 10, &[], *wish);
        assert!(alone.hold.is_empty() && alone.settle.is_empty(), "{case}");
    }
    for (finest, mixed, wish, resident, held) in ZOOM_OUT {
        let (mut rig, _, _) = zoom_out_rig();
        set_resident(&mut rig, 2, resident);
        for child in 10..18 {
            let f = if *mixed && child != 10 { 5 } else { *finest };
            set_applied(&mut rig, child, f);
        }
        let case = format!("zoom out {finest} {mixed} {wish} {resident:?}");
        let children: Vec<u64> = (10..18).collect();
        let covered = eval_held(&rig, 2, &children, *wish);
        // The rows `eval_refine_hold` held longer than the twin needs are released now.
        let held = *held
            && !ZOOM_OUT_PARTS
                .iter()
                .any(|(c, _)| *c == (*finest, *mixed, *wish, *resident));
        assert_eq!(covered.hold.contains(&NodeId::new(2)), held, "{case}");
        assert!(covered.settle.contains(&NodeId::new(2)), "{case}");
        let alone = eval_held(&rig, 2, &[], *wish);
        assert!(alone.hold.is_empty() && alone.settle.is_empty(), "{case}");
    }
}

/// With a `range` on the entry its width changes the run of the node (every resident level up
/// to the range end is active), so the judges see more than the drawn levels.
#[test]
fn the_range_moves_the_run_ends() {
    let runs = |case: OutCase| -> std::collections::BTreeSet<Option<Run>> {
        FARS.iter()
            .map(|far| zoom_out_case(case, *far).run)
            .collect()
    };
    // The coarse end moves with the width of the range.
    assert_eq!(
        runs((6, false, 7, &[4, 5])),
        [Some((4, 5)), Some((5, 5))].into()
    );
    // The fine end reaches every resident level: descendants drew 5, the node has 5, 6 and 7.
    assert_eq!(runs((5, false, 7, &[5, 6, 7])), [Some((5, 7))].into());
}

#[test]
fn octants_of_a_node_touch_the_ancestor_octants_below_them() {
    let f = Footprint {
        rep: 6,
        lo: [0; 3],
        hi: [15; 3],
    };
    assert_eq!(f.octants_in(1).count(), 8);
    for octant in 0..8 {
        assert_eq!(f.octants_of(1, octant).collect::<Vec<_>>(), vec![octant]);
    }
    // From two levels up the whole node lies in one octant of the ancestor.
    for octant in 0..8 {
        assert_eq!(f.octants_of(2, octant).collect::<Vec<_>>(), vec![0]);
    }
}

#[test]
fn a_hold_that_does_not_settle_stays_held_and_never_draws_coarser() {
    // One pass cannot reach a fixed point whenever the held set changes: the nodes held in the
    // input or in the new set stay held (so the image never gets coarser) and are counted.
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    rig.pool.hold_passes = 1;
    let cam = camera("fit");
    let (before, resident) = run_with_resident(&mut rig, 60, &ids(&[2, 3]), &cam, 8.0);
    assert_eq!(before.counters.hold_unsettled, 0);
    let points = sample_points(nodes[&NodeId::new(2)].bounds);
    let base: Vec<Option<u8>> = points
        .iter()
        .map(|p| drawn_at(&before, &nodes, &resident, *p))
        .collect();
    let mut cut = ids(&region_children(10));
    cut.push(NodeId::new(3));
    let (mut unsettled, mut held) = (0, Vec::new());
    for frame in 0..40 {
        let resident = resident_levels(&rig);
        let plan = rig.step(&cut, &cam, 8.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_no_holes(&plan, &nodes);
        for (point, was) in points.iter().zip(&base) {
            let now = drawn_at(&plan, &nodes, &resident, *point);
            assert!(now >= *was, "frame {frame}: {point:?} {now:?} < {was:?}");
        }
        unsettled += plan.counters.hold_unsettled;
        held.push(plan.counters.held_nodes);
    }
    assert!(unsettled > 0, "the hold always settled");
    assert!(held.windows(2).all(|w| w[0] <= w[1]), "{held:?}");
    assert!(held[30..].iter().all(|h| *h == held[39]), "{held:?}");
}

#[test]
fn the_real_cut_is_not_reused_when_the_applied_depths_changed() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes, 512, 3, 1);
    let cam = camera("fit");
    rig.run(60, &ids(&[2, 3]), &cam, 8.0);
    assert!(rig.step(&ids(&[2, 3]), &cam, 8.0).counters.real_reused);
    rig.pool.applied_digest ^= 1;
    assert!(!rig.step(&ids(&[2, 3]), &cam, 8.0).counters.real_reused);
}

#[test]
fn reset_with_held_nodes_converges_back() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    let cam = camera("fit");
    rig.run(60, &ids(&[2, 3]), &cam, 8.0);
    let mut cut = ids(&region_children(10));
    cut.push(NodeId::new(3));
    let mut frames = 0;
    while rig.step(&cut, &cam, 8.0).counters.held_nodes == 0 {
        frames += 1;
        assert!(frames < 20, "setup: nothing was held");
    }
    assert!(!rig.pool.hold.is_empty());
    let _ = rig.pool.reset();
    assert!(rig.pool.hold.is_empty() && rig.pool.applied_drawn.is_empty());
    assert_eq!(rig.pool.applied_digest, 0);
    rig.inflight.clear();
    rig.gpu = FakeGpu::default();
    let mut last = None;
    for _ in 0..80 {
        let plan = rig.step(&cut, &cam, 8.0);
        assert_no_holes(&plan, &nodes);
        last = Some(plan);
    }
    let plan = last.unwrap();
    assert_eq!(plan.apply, CutApply::Applied);
    assert_eq!(plan.entries.len(), cut.len());
    assert_eq!(plan.counters.held_nodes, 0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    assert_eq!(plan.counters.fallback_pages, 0);
    assert!(rig.pool.hold.is_empty());
    rig.assert_gpu_matches_pool();
}

// R3-Q20: nodes without a level wait outside the cut.

/// What the pool held when a frame was planned: the resident levels before the step and the caps
/// after it (the caps the plan was cut with).
struct Seen {
    resident: HashMap<NodeId, Vec<u8>>,
    caps: FastMap<NodeId, u8>,
}

fn observe(rig: &mut Rig, cut: &[NodeId], camera: &OrbitCamera, target: f64) -> (FramePlan, Seen) {
    let resident = rig
        .nodes
        .iter()
        .map(|(id, node)| {
            let levels = node
                .stored
                .iter()
                .copied()
                .filter(|depth| rig.pool.is_resident(rig.pool.key(*id, *depth)))
                .collect();
            (*id, levels)
        })
        .collect();
    let plan = rig.step(cut, camera, target);
    let caps = rig.pool.caps.clone();
    (plan, Seen { resident, caps })
}

/// `(drawn, wanted)` depth of the entry that really draws `point` (not masked, octant in
/// `active`); the geometry is computed here from the bounds. The drawn depth must be the one
/// level that was resident (the exact stepped depth is checked per octant by `Rig::step`), and no
/// point may be drawn by two entries.
fn drawn_and_wanted_at(
    plan: &FramePlan,
    nodes: &HashMap<NodeId, PoolNode>,
    seen: &Seen,
    point: [f64; 3],
) -> Option<(u8, u8)> {
    let hits: Vec<(u8, u8)> = plan
        .entries
        .iter()
        .filter_map(|entry| {
            let at = cell_of(nodes[&entry.node].bounds, point)?;
            if entry.mask[at[2]] >> (at[0] + 8 * at[1]) & 1 == 1 {
                return None;
            }
            let octant = ((at[0] / 4) << 2) | ((at[1] / 4) << 1) | (at[2] / 4);
            plan.active
                .iter()
                .any(|a| a.key.node == entry.node && a.octant_bits >> octant & 1 == 1)
                .then(|| Some((entry.drawn[octant]?, entry.wanted[octant]?)))
                .flatten()
                .inspect(|(drawn, _)| {
                    let levels = capped(&seen.resident[&entry.node], seen.caps.get(&entry.node));
                    // The expected stepped depth is `Rig::check_stepping`'s; here, that the
                    // level was resident, and that it is never past the wanted depth.
                    assert!(
                        levels.contains(drawn),
                        "node {:?} octant {octant}",
                        entry.node
                    );
                })
        })
        .collect();
    assert!(hits.len() <= 1, "{point:?} is drawn by {hits:?}");
    hits.first().copied()
}

/// What `points` drew: the drawn depth per point.
fn snapshot(
    plan: &FramePlan,
    nodes: &HashMap<NodeId, PoolNode>,
    seen: &Seen,
    points: &[[f64; 3]],
) -> Vec<Option<u8>> {
    points
        .iter()
        .map(|p| drawn_and_wanted_at(plan, nodes, seen, *p).map(|(drawn, _)| drawn))
        .collect()
}

/// No sample that was drawn is lost, and none is drawn below `min(before, wanted now)` (R3-Q21).
fn assert_kept(
    plan: &FramePlan,
    nodes: &HashMap<NodeId, PoolNode>,
    seen: &Seen,
    points: &[[f64; 3]],
    before: &[Option<u8>],
    label: &str,
) {
    for (point, was) in points.iter().zip(before) {
        let Some(was) = was else { continue };
        let (now, wanted) = drawn_and_wanted_at(plan, nodes, seen, *point)
            .unwrap_or_else(|| panic!("{label}: {point:?} was drawn and is lost"));
        assert!(
            now >= (*was).min(wanted),
            "{label}: {point:?} {now} < min({was}, {wanted})"
        );
    }
}

#[test]
fn the_first_rung_streams_in_one_node_per_frame_without_retaining() {
    let nodes = octant_nodes();
    let mut rig = Rig::new(nodes.clone(), 128, 8, 1);
    let cam = camera("fit");
    let mut seen_before: Vec<(u64, Vec<[f64; 3]>)> = Vec::new();
    for frame in 0..30 {
        let count = (frame + 1).min(CHILDREN.len());
        let cut = ids(&CHILDREN[..count]);
        let ready: Vec<bool> = CHILDREN[..count]
            .iter()
            .map(|id| coarse_resident(&rig, *id))
            .collect();
        let (plan, seen) = observe(&mut rig, &cut, &cam, 64.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_eq!(plan.counters.lost_nodes, 0, "frame {frame}");
        let mut waiting = 0;
        for (index, id) in CHILDREN[..count].iter().enumerate() {
            // A node draws from the frame after its coarsest level is resident.
            assert_eq!(
                has_entry(&plan, *id),
                ready[index],
                "frame {frame} node {id}"
            );
            for point in sample_points(nodes[&NodeId::new(*id)].bounds) {
                assert_eq!(
                    drawn_and_wanted_at(&plan, &nodes, &seen, point).is_some(),
                    ready[index],
                    "frame {frame} node {id} {point:?}"
                );
            }
            waiting += usize::from(!ready[index]);
        }
        assert_eq!(plan.counters.nodes_without_level as usize, waiting);
        assert_eq!(plan.counters.excluded_nodes as usize, waiting);
        assert_eq!(plan.counters.hold_age_max > 0, waiting > 0, "frame {frame}");
        // Drawn nodes never stop drawing.
        for (id, points) in &seen_before {
            for point in points {
                assert!(
                    drawn_and_wanted_at(&plan, &nodes, &seen, *point).is_some(),
                    "frame {frame}: node {id} lost"
                );
            }
        }
        seen_before = CHILDREN[..count]
            .iter()
            .zip(&ready)
            .filter(|(_, ready)| **ready)
            .map(|(id, _)| (*id, sample_points(nodes[&NodeId::new(*id)].bounds)))
            .collect();
    }
    let last = rig.step(&ids(&CHILDREN), &cam, 64.0);
    assert_eq!(last.counters.nodes_without_level, 0);
    assert_eq!(last.counters.hold_age_max, 0);
    assert_eq!(last.entries.len(), CHILDREN.len());
}

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

/// Zooming out to node 2 at `target` px. At 64 px node 2 wants its coarsest level (4); at 4 px it
/// wants 7, finer than its coarsest, so the children keep drawing after node 2 has a level until
/// the level they drew is resident (R3-Q21).
fn zoom_out(target: f64) {
    let (mut rig, nodes, cam) = zoom_out_rig();
    let points = sample_points(nodes[&NodeId::new(2)].bounds);
    let (before_plan, seen) = observe(&mut rig, &ids(&region_children(10)), &cam, 4.0);
    let before = snapshot(&before_plan, &nodes, &seen, &points);
    assert!(before.iter().all(|d| *d == Some(7)), "setup");
    let cut = ids(&[2]);
    let done = |rig: &Rig| {
        if target < 8.0 {
            resident_at(rig, 2, 7)
        } else {
            coarse_resident(rig, 2)
        }
    };
    let (mut waiting, mut taken, mut held_with_level) = (0_u32, false, false);
    for frame in 0..60 {
        let ready = done(&rig);
        let coarse = coarse_resident(&rig, 2);
        let (plan, seen) = observe(&mut rig, &cut, &cam, target);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert!(!plan.counters.descendant_too_fine, "frame {frame}");
        assert_eq!(plan.counters.lost_nodes, 0, "frame {frame}");
        assert_no_holes(&plan, &nodes);
        assert_kept(
            &plan,
            &nodes,
            &seen,
            &points,
            &before,
            &format!("frame {frame}"),
        );
        if ready {
            taken = true;
            assert!(
                has_entry(&plan, 2) && plan.entries.len() == 1,
                "frame {frame}"
            );
            assert_eq!(plan.counters.nodes_without_level, 0);
            assert_eq!(plan.counters.hold_age_max, 0);
        } else {
            waiting += 1;
            assert!(
                !has_entry(&plan, 2) || plan.entries.len() == 9,
                "frame {frame}"
            );
            assert_eq!(plan.counters.nodes_without_level, 1, "frame {frame}");
            assert_eq!(
                plan.counters.excluded_nodes + plan.counters.held_nodes,
                1,
                "frame {frame}"
            );
            assert_eq!(plan.counters.hold_age_max, waiting, "frame {frame}");
            held_with_level |= coarse && plan.counters.held_nodes == 1;
            assert!(!taken, "frame {frame}");
        }
    }
    assert!(waiting > 0 && taken);
    // With a wanted level finer than the coarsest, the node sits held while it has a level.
    assert_eq!(held_with_level, target < 8.0);
    // At a fixed pose the hold stays empty once it has emptied: no flicker, nothing stepping.
    for frame in 0..50 {
        let plan = rig.step(&cut, &cam, target);
        assert_eq!(plan.counters.held_nodes, 0, "rest frame {frame}");
        assert!(rig.pool.hold.is_empty(), "rest frame {frame}");
        assert_eq!(plan.counters.stages, 0, "rest frame {frame}");
    }
}

#[test]
fn zoom_out_keeps_the_descendants_drawing_until_the_coarse_node_is_resident() {
    zoom_out(64.0);
}

#[test]
fn zoom_out_keeps_the_descendants_until_the_node_has_the_level_they_drew() {
    zoom_out(4.0);
}

#[test]
fn a_pose_jump_to_a_disjoint_cut_retains_only_while_the_first_levels_arrive() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 8, 1);
    let cam = camera("fit");
    let a = ids(&region_children(10));
    rig.run(60, &a, &cam, 4.0);
    let held = rig.step(&a, &cam, 4.0);
    let b = ids(&region_children(20));
    let (mut retained, mut applied_once) = (0, false);
    for frame in 0..20 {
        let ready = region_children(20)
            .into_iter()
            .any(|c| coarse_resident(&rig, c));
        let plan = rig.step(&b, &cam, 4.0);
        if ready {
            applied_once = true;
        }
        if applied_once {
            assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
            assert_eq!(plan.counters.lost_nodes, 0);
        } else {
            // Nothing of the new cut draws yet: the old image stays, and it is not a "loss".
            assert_eq!(plan.apply, CutApply::RetainedPrevious, "frame {frame}");
            assert_eq!(plan.entries, held.entries);
            // The counters describe the retained cut: none of the new nodes is in it.
            assert_eq!(plan.counters.excluded_nodes, 8, "frame {frame}");
            assert_eq!(plan.counters.nodes_without_level, 8, "frame {frame}");
            retained += 1;
        }
    }
    assert!((1..=4).contains(&retained), "{retained}");
}

#[test]
fn a_pose_jump_keeps_the_zones_that_stay_in_the_cut() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 8, 1);
    let cam = camera("fit");
    rig.run(60, &ids(&region_children(10)), &cam, 4.0);
    let points = sample_points(nodes[&NodeId::new(10)].bounds);
    let (held, seen) = observe(&mut rig, &ids(&region_children(10)), &cam, 4.0);
    let before = snapshot(&held, &nodes, &seen, &points);
    assert!(before.iter().all(Option::is_some), "setup");
    let mut cut = vec![NodeId::new(10)];
    cut.extend(ids(&region_children(20)));
    let mut b_drawn = false;
    for frame in 0..20 {
        let (plan, seen) = observe(&mut rig, &cut, &cam, 4.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_eq!(plan.counters.lost_nodes, 0);
        assert_kept(
            &plan,
            &nodes,
            &seen,
            &points,
            &before,
            &format!("frame {frame}"),
        );
        b_drawn |= has_entry(&plan, 20);
    }
    assert!(b_drawn);
}

#[test]
fn a_cap_drop_loses_no_sample_and_never_draws_below_the_need() {
    let nodes = octant_nodes();
    let mut rig = Rig::new(nodes.clone(), 44, 64, 2);
    let cam = camera("fit");
    // The six farthest nodes fit at depth 7; adding the two nearest makes the fit coarsen the
    // farthest ones, which are drawing.
    let near = by_distance(&cam, &CHILDREN);
    let first = ids(&near[2..]);
    rig.run(30, &first, &cam, 4.0);
    let (held, seen) = observe(&mut rig, &first, &cam, 4.0);
    let points: Vec<[f64; 3]> = near[2..]
        .iter()
        .flat_map(|id| sample_points(nodes[&NodeId::new(*id)].bounds))
        .collect();
    let before = snapshot(&held, &nodes, &seen, &points);
    assert!(before.iter().all(|d| *d == Some(7)), "setup");
    let (mut dropped, mut waited) = (false, false);
    let mut last = None;
    for frame in 0..60 {
        let (plan, seen) = observe(&mut rig, &ids(&CHILDREN), &cam, 4.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_eq!(plan.counters.lost_nodes, 0, "frame {frame}");
        assert_kept(
            &plan,
            &nodes,
            &seen,
            &points,
            &before,
            &format!("frame {frame}"),
        );
        dropped |= snapshot(&plan, &nodes, &seen, &points)
            .iter()
            .any(|d| d.is_some_and(|d| d < 7));
        waited |= plan.counters.excluded_nodes > 0;
        last = Some(plan);
    }
    assert!(dropped && waited);
    // No node starves under the cap: every child ends with an entry.
    let last = last.unwrap();
    for child in CHILDREN {
        assert!(has_entry(&last, child), "node {child} never entered");
    }
}

#[test]
fn a_zone_nothing_can_cover_retains_the_previous_cut_and_counts_the_loss() {
    // Node 20 is `MAX_MASK_LEVELS + 1` levels below the root, which is in the cut without a level:
    // the node cannot stand in for the root and nothing above it draws. Node 22 is unrelated.
    let mut nodes = chain_nodes();
    nodes.insert(
        NodeId::new(22),
        PoolNode {
            bounds: cube([ORIGIN[0] + SIDE / 2.0, ORIGIN[1], ORIGIN[2]], SIDE / 2.0),
            stored: vec![4, 5],
            buildable: (4, 5),
            points: 2000,
            octant_data_bounds: None,
        },
    );
    let mut rig = Rig::new(nodes.clone(), 64, 8, 1);
    let cam = camera("fit");
    rig.run(10, &ids(&[20, 22]), &cam, 64.0);
    let held = rig.step(&ids(&[20, 22]), &cam, 64.0);
    assert!(has_entry(&held, 20) && has_entry(&held, 22), "setup");
    let (mut retained, mut taken) = (0, false);
    for frame in 0..12 {
        let ready = coarse_resident(&rig, 1);
        let plan = rig.step(&ids(&[1, 22]), &cam, 64.0);
        if frame == 0 {
            assert_eq!(
                plan.fetch_requests.first(),
                Some(&rig.pool.key(NodeId::new(1), 3))
            );
        }
        if ready {
            taken = true;
            assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
            assert_eq!(plan.counters.lost_nodes, 0);
            assert!(has_entry(&plan, 1) && !has_entry(&plan, 20));
        } else {
            assert_eq!(plan.apply, CutApply::RetainedPrevious, "frame {frame}");
            assert_eq!(plan.counters.lost_nodes, 1, "frame {frame}");
            assert_eq!(plan.entries, held.entries);
            assert_eq!(plan.counters.nodes_without_level, 1);
            retained += 1;
        }
    }
    assert!(retained > 0 && taken);
}

#[test]
fn the_hold_age_grows_while_a_child_is_held_and_resets_after() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    let cam = camera("fit");
    rig.run(60, &ids(&[2, 3]), &cam, 8.0);
    let mut cut = ids(&region_children(10));
    cut.push(NodeId::new(3));
    let (mut streak, mut peak) = (0_u32, 0_u32);
    for frame in 0..40 {
        let plan = rig.step(&cut, &cam, 8.0);
        if plan.counters.held_nodes > 0 {
            streak += 1;
            // The oldest hold is at least as old as the run of frames with something held.
            assert!(plan.counters.hold_age_max >= 1, "frame {frame}");
            assert!(plan.counters.hold_age_max <= streak, "frame {frame}");
            peak = peak.max(plan.counters.hold_age_max);
        } else {
            assert_eq!(plan.counters.hold_age_max, 0, "frame {frame}");
            streak = 0;
        }
    }
    assert!(peak >= 2, "{peak}");
}

#[test]
fn reset_with_excluded_nodes_and_descendants_converges_back() {
    let (mut rig, nodes, cam) = zoom_out_rig();
    let cut = ids(&[2]);
    let plan = rig.step(&cut, &cam, 64.0);
    assert_eq!(plan.counters.excluded_nodes, 1);
    assert_eq!(plan.entries.len(), 8, "setup: the descendants stand in");
    assert!(!rig.pool.wait_ages.is_empty());
    let _ = rig.pool.reset();
    assert!(rig.pool.wait_ages.is_empty() && rig.pool.fallback_memo.is_none());
    assert!(rig.pool.real_memo.is_none() && rig.pool.applied.is_empty());
    rig.inflight.clear();
    rig.gpu = FakeGpu::default();
    let mut last = None;
    for _ in 0..40 {
        let plan = rig.step(&cut, &cam, 64.0);
        assert_no_holes(&plan, &nodes);
        last = Some(plan);
    }
    let plan = last.unwrap();
    assert_eq!(plan.apply, CutApply::Applied);
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.counters.excluded_nodes, 0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    assert_eq!(plan.counters.hold_age_max, 0);
    rig.assert_gpu_matches_pool();
}

/// `chain_nodes` plus node 22, unrelated to the chain, and node 30 inside node 20.
fn chain_with_extras() -> HashMap<NodeId, PoolNode> {
    let mut nodes = chain_nodes();
    nodes.insert(
        NodeId::new(22),
        PoolNode {
            bounds: cube([ORIGIN[0] + SIDE / 2.0, ORIGIN[1], ORIGIN[2]], SIDE / 2.0),
            stored: vec![4, 5],
            buildable: (4, 5),
            points: 2000,
            octant_data_bounds: None,
        },
    );
    nodes.insert(
        NodeId::new(30),
        PoolNode {
            bounds: cube(ORIGIN, SIDE / 32.0),
            stored: vec![9, 10],
            buildable: (9, 10),
            points: 2000,
            octant_data_bounds: None,
        },
    );
    nodes
}

#[test]
fn an_excluded_node_too_fine_for_its_ancestor_waits_without_being_requested() {
    // Node 20 is `MAX_MASK_LEVELS + 1` levels below the root: both wait outside the cut while
    // node 22 (applied, an octant of the root) stands in for the root. The root, which is
    // requested, draws the zone as soon as it arrives; node 20 is not requested (its level would
    // make the real cut fail with `DescendantTooFine`) and keeps nothing waiting.
    let mut rig = Rig::new(chain_with_extras(), 64, 8, 1);
    let cam = camera("fit");
    rig.run(10, &ids(&[22]), &cam, 64.0);
    let mut waited = 0;
    let mut arrived = None;
    for frame in 0..12 {
        let plan = rig.step(&ids(&[1, 20]), &cam, 64.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert!(
            plan.fetch_requests
                .iter()
                .all(|k| k.node != NodeId::new(20))
        );
        if has_entry(&plan, 1) {
            arrived = Some(plan);
            break;
        }
        assert!(has_entry(&plan, 22), "frame {frame}: 22 stands in");
        waited += 1;
    }
    let plan = arrived.expect("the root arrives and draws");
    assert!(waited > 0);
    assert!(!has_entry(&plan, 22) && !plan.counters.descendant_too_fine);
    // Node 20 is drawn by the root, coarser: a stage, not a node waiting for a level, so the
    // counters settle (T11's "target reached" and the driver's `busy` depend on them).
    assert_eq!(plan.counters.excluded_nodes, 0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    assert!(plan.counters.stages >= 8);
    assert_eq!(plan.counters.hold_age_max, 0);
    for _ in 0..5 {
        let plan = rig.step(&ids(&[1, 20]), &cam, 64.0);
        assert_eq!(plan.apply, CutApply::Applied);
        assert!(
            plan.fetch_requests
                .iter()
                .all(|k| k.node != NodeId::new(20))
        );
        assert_eq!(plan.counters.nodes_without_level, 0);
    }
}

#[test]
fn a_descendant_under_a_node_that_draws_does_not_stand_in_when_the_mask_cannot_hide_it() {
    // Applied {30}, with the root resident. The cut {root, 20} excludes node 20 (nothing within
    // `MAX_MASK_LEVELS` draws it); node 30, applied under it, is `MAX_MASK_LEVELS + 2` levels
    // below the root, which draws: as a stand-in it would make the real cut `DescendantTooFine`.
    let mut rig = Rig::new(chain_with_extras(), 64, 8, 1);
    let cam = camera("fit");
    rig.run(10, &ids(&[1]), &cam, 64.0);
    let alone = rig.run(15, &ids(&[30]), &cam, 64.0);
    assert!(has_entry(&alone, 30) && alone.entries.len() == 1, "setup");
    for frame in 0..6 {
        let plan = rig.step(&ids(&[1, 20]), &cam, 64.0);
        assert!(!plan.counters.descendant_too_fine, "frame {frame}");
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert!(
            has_entry(&plan, 1) && !has_entry(&plan, 30),
            "frame {frame}"
        );
    }
}

/// An entry of `node` drawing the given octants at depth 7. The anchor is set as the pool does
/// (presence readers read it): a depth when some octant draws.
fn drawing(node: u64, octants: &[usize]) -> CellCutEntry {
    CellCutEntry {
        node: NodeId::new(node),
        wanted: [Some(7); 8],
        drawn: std::array::from_fn(|o| octants.contains(&o).then_some(7)),
        stage: [false; 8],
        step: [None; 8],
        range: None,
        node_anchor: (!octants.is_empty()).then_some(7),
        node_target: None,
        node_run: None,
        node_stage: false,
        node_step: None,
        mask: [0; 8],
    }
}

#[test]
fn a_zone_covered_by_one_sub_cell_of_a_descendant_is_lost() {
    let mut rig = Rig::new(two_regions(), 512, 8, 1);
    let cam = camera("fit");
    let applied = rig.run(30, &ids(&[2]), &cam, 4.0);
    assert!(
        has_entry(&applied, 2) && applied.entries[0].drawn.iter().all(Option::is_some),
        "setup"
    );
    let nodes: FastMap<NodeId, PoolNode> = rig
        .nodes
        .iter()
        .map(|(id, node)| (*id, node.clone()))
        .collect();
    let cut = ids(&[2]);
    let lost = |entries: &[CellCutEntry]| rig.pool.lost_zones(entries, &cut, &nodes, &grid());
    // One octant of one child covers a sub-cell of the zone node 2 drew.
    assert!(lost(&[drawing(10, &[0])]) > 0);
    // Every octant of one child still leaves the other seven children's zones.
    assert!(lost(&[drawing(10, &[0, 1, 2, 3, 4, 5, 6, 7])]) > 0);
    // The eight children drawing everything cover the zone.
    let all: Vec<CellCutEntry> = region_children(10)
        .into_iter()
        .map(|c| drawing(c, &[0, 1, 2, 3, 4, 5, 6, 7]))
        .collect();
    assert_eq!(lost(&all), 0);
}

/// `(drawn, wanted)` at `point` like `drawn_and_wanted_at`, without the `resident_depth` oracle: a
/// capped node may keep drawing a level above its cap (R3-Q21), so the drawn level must be
/// resident in the pool and drawn by an active level instead.
fn drawn_depth_at(
    plan: &FramePlan,
    nodes: &HashMap<NodeId, PoolNode>,
    rig: &Rig,
    point: [f64; 3],
) -> Option<(u8, u8)> {
    let hits: Vec<(u8, u8)> = plan
        .entries
        .iter()
        .filter_map(|entry| {
            let at = cell_of(nodes[&entry.node].bounds, point)?;
            if entry.mask[at[2]] >> (at[0] + 8 * at[1]) & 1 == 1 {
                return None;
            }
            let octant = ((at[0] / 4) << 2) | ((at[1] / 4) << 1) | (at[2] / 4);
            let drawn = entry.drawn[octant]?;
            plan.active
                .iter()
                .any(|a| {
                    a.key.node == entry.node
                        && a.key.depth == drawn
                        && a.octant_bits >> octant & 1 == 1
                })
                .then(|| {
                    assert!(rig.pool.is_resident(rig.pool.key(entry.node, drawn)));
                    (
                        drawn,
                        entry.wanted[octant].expect("a drawn octant is wanted"),
                    )
                })
        })
        .collect();
    assert!(hits.len() <= 1, "{point:?} is drawn by {hits:?}");
    hits.first().copied()
}

/// Six children draw depth 7 (5 pages each with the pinned level 4). Returns the rig, the camera,
/// the six ids (nearest last), their samples and what they drew. The capacity leaves the caps that
/// a test inserts in place: the six at depth 7 sit between the 80 % and 90 % thresholds.
#[expect(
    clippy::type_complexity,
    reason = "a test rig returning a tuple of fixtures; a named type would be used once"
)]
fn cap_drop_rig(
    capacity: u32,
    delay: u64,
) -> (
    Rig,
    OrbitCamera,
    Vec<NodeId>,
    Vec<[f64; 3]>,
    Vec<Option<u8>>,
) {
    let nodes = octant_nodes();
    let mut rig = Rig::new(nodes.clone(), capacity, 64, delay);
    let cam = camera("fit");
    let near = by_distance(&cam, &CHILDREN);
    let cut = ids(&near[2..]);
    rig.run(30, &cut, &cam, 4.0);
    let points: Vec<[f64; 3]> = cut
        .iter()
        .flat_map(|id| sample_points(nodes[id].bounds))
        .collect();
    let plan = rig.step(&cut, &cam, 4.0);
    let before: Vec<Option<u8>> = points
        .iter()
        .map(|p| drawn_depth_at(&plan, &nodes, &rig, *p).map(|(d, _)| d))
        .collect();
    assert!(before.iter().all(|d| *d == Some(7)), "setup");
    (rig, cam, cut, points, before)
}

/// Steps `frames` frames and counts the samples drawn below `min(before, wanted now)` per frame.
fn cap_drop_run(
    rig: &mut Rig,
    cam: &OrbitCamera,
    cut: &[NodeId],
    points: &[[f64; 3]],
    before: &[Option<u8>],
    frames: usize,
) -> Vec<(FramePlan, usize)> {
    let nodes = rig.nodes.clone();
    let mut out = Vec::new();
    for frame in 0..frames {
        rig.pool.size_epoch += u64::from(frame == 0);
        let plan = rig.step(cut, cam, 4.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_eq!(plan.counters.lost_nodes, 0, "frame {frame}");
        assert_no_holes(&plan, &nodes);
        let mut coarsened = 0;
        for (point, was) in points.iter().zip(before) {
            let (now, wanted) = drawn_depth_at(&plan, &nodes, rig, *point)
                .unwrap_or_else(|| panic!("frame {frame}: {point:?} lost"));
            coarsened += usize::from(now < was.unwrap().min(wanted));
        }
        out.push((plan, coarsened));
    }
    out
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

#[test]
fn a_cap_drop_keeps_the_finer_level_until_the_capped_one_is_resident() {
    let (mut rig, cam, cut, points, before) = cap_drop_rig(40, 3);
    let node = cut[0];
    let key6 = rig.pool.key(node, 6);
    assert!(
        !rig.pool.is_resident(key6),
        "setup: depth 6 is not resident"
    );
    rig.pool.caps.insert(node, 6);
    let frames = cap_drop_run(&mut rig, &cam, &cut, &points, &before, 20);
    assert_eq!(rig.pool.caps.get(&node), Some(&6), "setup: the cap stays");
    assert!(
        frames.iter().all(|(_, coarsened)| *coarsened == 0),
        "no frame draws below min(before, wanted now)"
    );
    // The node keeps drawing 7 while 6 is missing, switches the frame after it is resident, and
    // the old level is then released.
    let mut switched = None;
    for (frame, (plan, _)) in frames.iter().enumerate() {
        let drawn = depth_drawn(plan, node);
        if switched.is_none() && drawn == [6] {
            switched = Some(frame);
        }
        if switched.is_some() {
            assert_eq!(drawn, [6], "frame {frame}");
        } else {
            assert_eq!(drawn, [7], "frame {frame}");
        }
    }
    let switched = switched.expect("the node switches to the capped level");
    assert!(switched >= 2, "the capped level takes frames to arrive");
    assert!(
        !rig.pool.is_resident(rig.pool.key(node, 7)),
        "old level released"
    );
    assert_eq!(rig.pool.overflow, 0);
}

#[test]
fn a_cap_drop_whose_capped_level_is_resident_switches_at_once() {
    // The frame planned right after the capped level became resident (while the finer one is
    // still resident and drew the frame before) already draws the capped level: no extra frame.
    let (mut rig, cam, cut, points, before) = cap_drop_rig(40, 1);
    let node = cut[0];
    rig.pool.caps.insert(node, 6);
    let mut waited = 0;
    loop {
        let frames = cap_drop_run(&mut rig, &cam, &cut, &points, &before, 1);
        let resident = rig.pool.is_resident(rig.pool.key(node, 6));
        if resident {
            assert!(
                rig.pool.is_resident(rig.pool.key(node, 7)),
                "setup: both resident"
            );
            assert_eq!(depth_drawn(&frames[0].0, node), [7]);
            break;
        }
        waited += 1;
        assert!(waited < 10, "the capped level never arrives");
    }
    let frames = cap_drop_run(&mut rig, &cam, &cut, &points, &before, 1);
    assert_eq!(depth_drawn(&frames[0].0, node), [6], "immediate switch");
    assert_eq!(frames[0].1, 0);
}

#[test]
fn a_cap_drop_in_a_tight_pool_releases_the_old_level_and_completes() {
    let (mut rig, cam, cut, points, before) = cap_drop_rig(40, 2);
    for id in &cut {
        rig.pool.caps.insert(*id, 6);
    }
    let frames = cap_drop_run(&mut rig, &cam, &cut, &points, &before, 40);
    // The six capped levels (24 pages) do not fit beside the six depth-7 levels: the old levels
    // are given up when the capped ones need their pages, so coarser frames are allowed (counted)
    // but the transition ends and nothing overflows.
    let last = &frames.last().unwrap().0;
    for id in &cut {
        assert_eq!(depth_drawn(last, *id), [6], "node {id:?}");
    }
    assert_eq!(rig.pool.overflow, 0);
    assert!(
        frames
            .iter()
            .all(|(plan, _)| plan.counters.pool_overflow == 0)
    );
    println!(
        "tight pool coarsened samples per frame: {:?}",
        frames.iter().map(|(_, c)| *c).collect::<Vec<_>>()
    );
}

/// Depth `plan` draws in every octant of node 1, which must be one.
fn drawn_depth(plan: &FramePlan) -> u8 {
    let drawn = depth_of(plan, 1);
    let first = drawn[0].expect("drawn");
    assert!(drawn.iter().all(|d| *d == Some(first)), "{drawn:?}");
    first
}

#[test]
fn continuity_caps_the_drawn_level_without_a_stage_and_literal_mode_is_unchanged() {
    let cut = ids(&[1]);
    let cam = camera("fit");
    let mut literal = Rig::new(root_node(), 64, 16, 1);
    let plan = literal.run(12, &cut, &cam, 1.0);
    assert_eq!(drawn_depth(&plan), 5);
    let literal_entries = plan.entries.clone();

    // Cells of depth 4 have side SIDE / 16: the minimum admits depths 3 and 4 only.
    let mut rig = Rig::new(root_node(), 64, 16, 1);
    rig.pool.set_min_side(Some(SIDE / 16.0));
    let plan = rig.run(12, &cut, &cam, 1.0);
    assert_eq!(drawn_depth(&plan), 4);
    assert_eq!(plan.counters.stages, 0, "the cap is the wanted look");
    assert_eq!(plan.counters.pool_coarsened, 0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    assert_eq!(plan.entries[0].wanted, [Some(4); 8]);
    assert_covers_once(&plan.entries[0]);
    assert!(!rig.pool.is_resident(rig.pool.key(NodeId::new(1), 5)));

    // Off again: the literal cut comes back, the same one a pool that never had it draws.
    rig.pool.set_min_side(None);
    let plan = rig.run(12, &cut, &cam, 1.0);
    assert_eq!(plan.entries, literal_entries);
}

#[test]
fn the_continuity_cap_uses_the_buildable_range() {
    let cut = ids(&[1]);
    let cam = camera("fit");
    // A minimum between two depths rounds up the side: depth 4 is SIDE / 16, 3 is SIDE / 8. Depth
    // 4 is not stored (3 and 5 are) but it is buildable, so the cap lands on it.
    for (min, expect) in [
        (SIDE / 16.0, 4),
        (SIDE / 12.0, 3),
        (SIDE / 8.0, 3),
        (SIDE, 3),
        (SIDE * 4.0, 3),
        (SIDE / 32.0, 5),
        (SIDE / 1024.0, 7),
    ] {
        let mut nodes = root_node();
        let node = nodes.get_mut(&NodeId::new(1)).unwrap();
        node.stored = vec![3, 5];
        node.buildable = (3, 7);
        let mut rig = Rig::new(nodes, 64, 16, 1);
        rig.pool.set_min_side(Some(min));
        let plan = rig.run(12, &cut, &cam, 1.0);
        assert_eq!(plan.entries[0].wanted, [Some(expect); 8], "min {min}");
        // Cells at the capped depth are the finest the minimum side admits (RQ-09): what draws is
        // the stored level that serves it, never finer than the first stored level at or above;
        // a depth that is not stored is also requested.
        let served = if expect <= 3 { 3 } else { 5 };
        assert_eq!(drawn_depth(&plan), served, "min {min}");
        let stored = [3, 5].contains(&expect);
        let requested: Vec<u8> = plan.level_requests.iter().map(|key| key.depth).collect();
        assert_eq!(
            requested,
            if stored { vec![] } else { vec![expect] },
            "min {min}"
        );
    }
    // Every buildable depth is finer than the minimum: the coarsest one is wanted.
    let mut fine = root_node();
    let node = fine.get_mut(&NodeId::new(1)).unwrap();
    node.stored = vec![6, 7];
    node.buildable = (6, 7);
    let mut rig = Rig::new(fine, 64, 16, 1);
    rig.pool.set_min_side(Some(SIDE / 4.0));
    let plan = rig.run(12, &cut, &cam, 1.0);
    assert_eq!(drawn_depth(&plan), 6);
    assert_eq!(plan.entries[0].wanted, [Some(6); 8]);
}

#[test]
fn a_wanted_depth_outside_the_chain_is_requested_and_residents_keep_drawing() {
    let cut = ids(&[1]);
    let mut nodes = root_node();
    let node = nodes.get_mut(&NodeId::new(1)).unwrap();
    node.stored = vec![3, 5];
    node.buildable = (3, 7);
    let mut rig = Rig::new(nodes, 64, 16, 1);
    // The finest stored level is resident and drawn.
    let plan = rig.run(12, &cut, &camera("zoom"), 1.0);
    let wanted = plan.entries[0].wanted[0].expect("wanted");
    assert!(
        wanted > 5,
        "setup: the view wants a depth that is not stored"
    );
    assert_eq!(drawn_depth(&plan), 5);
    assert!(rig.pool.is_resident(rig.pool.key(NodeId::new(1), 5)));
    let key = rig.pool.key(NodeId::new(1), wanted);
    for _ in 0..4 {
        let plan = rig.step(&cut, &camera("zoom"), 1.0);
        assert_eq!(plan.level_requests, [key]);
        assert!(plan.fetch_requests.iter().all(|k| k.depth <= 5));
        assert_eq!(plan.apply, CutApply::Applied);
        assert_eq!(drawn_depth(&plan), 5, "the resident level keeps drawing");
        assert_covers_once(&plan.entries[0]);
    }
}

#[test]
fn a_fit_cap_keeps_the_resident_finer_level_while_the_capped_one_is_pending() {
    let cut = ids(&[1]);
    let mut nodes = root_node();
    let node = nodes.get_mut(&NodeId::new(1)).unwrap();
    node.stored = vec![3, 5, 7];
    node.buildable = (3, 9);
    let mut rig = Rig::new(nodes, 64, 16, 1);
    let plan = rig.run(14, &cut, &camera("zoom"), 1.0);
    assert_eq!(drawn_depth(&plan), 7, "setup");
    assert!(
        !rig.pool.is_resident(rig.pool.key(NodeId::new(1), 5)),
        "setup"
    );
    // The fit caps the node below the resident level; the capped level is not resident yet.
    rig.pool.caps.insert(NodeId::new(1), 5);
    let plan = rig.step(&cut, &camera("zoom"), 1.0);
    assert!(
        rig.pool.caps.contains_key(&NodeId::new(1)),
        "the cap stands"
    );
    assert_eq!(drawn_depth(&plan), 7, "the finer resident level stands in");
    assert_eq!(plan.apply, CutApply::Applied);
}

#[test]
fn toggling_continuity_requests_only_levels_of_the_pyramid_and_leaves_no_hole() {
    let cut = ids(&[1]);
    let cam = camera("fit");
    let mut rig = Rig::new(root_node(), 64, 16, 2);
    rig.run(12, &cut, &cam, 1.0);
    for min in [Some(SIDE / 16.0), None, Some(SIDE / 8.0), None] {
        rig.pool.set_min_side(min);
        for frame in 0..10 {
            let plan = rig.step(&cut, &cam, 1.0);
            assert_eq!(plan.counters.nodes_without_level, 0, "{min:?} {frame}");
            assert_covers_once(&plan.entries[0]);
            assert!(
                plan.fetch_requests
                    .iter()
                    .all(|key| rig.nodes[&key.node].stored.contains(&key.depth))
            );
        }
    }
}

#[test]
fn the_minimum_side_is_in_dataset_units_whatever_the_visual_scale() {
    // The pool works in dataset units (the eye is divided by the visual scale), like the grid and
    // the reference spacing, so a scaled view must pick the same levels as an unscaled one.
    let scale = 2.0;
    let mut camera = OrbitCamera::default();
    let bounds = crate::SceneBounds {
        min: [0.0; 3],
        max: [SIDE * scale; 3],
    };
    assert!(camera.fit(bounds, crate::scene_digest_tests::VIEWPORT));
    let scaled = ViewProjection::new(
        camera,
        crate::scene_digest_tests::VIEWPORT,
        scale,
        crate::VoxelBuildLimits::default(),
    );
    let cut = ids(&[1]);
    let min = SIDE / 16.0;
    let mut literal = Rig::new(root_node(), 64, 16, 1);
    let mut rig = Rig::new(root_node(), 64, 16, 1);
    rig.pool.set_min_side(Some(min));
    let (mut lit, mut cont) = (None, None);
    for _ in 0..12 {
        lit = Some(literal.step_view(&cut, &scaled, 1.0));
        cont = Some(rig.step_view(&cut, &scaled, 1.0));
    }
    assert_eq!(drawn_depth(&lit.unwrap()), 5);
    let depth = drawn_depth(&cont.unwrap());
    assert_eq!(depth, 4);
    assert!(SIDE / f64::from(1_u32 << depth) >= min);
}

#[test]
fn a_tight_pool_still_reports_its_coarsening_with_continuity_on_and_off() {
    // Two sibling-level fixtures: the pool fit coarsens the far octant nodes; continuity at
    // depth 6 (above every stored level but the finest) must neither hide nor add to it.
    let cut = ids(&CHILDREN);
    let cam = camera("zoom");
    let mut counts = Vec::new();
    for min in [None, Some(SIDE / 64.0), Some(SIDE / 16.0)] {
        let mut rig = Rig::new(octant_nodes(), 44, 4, 2);
        rig.pool.set_min_side(min);
        let plan = rig.run(240, &cut, &cam, 4.0);
        assert_eq!(plan.apply, CutApply::Applied, "{min:?}");
        counts.push((plan.counters.pool_coarsened, plan.counters.stages));
    }
    // The literal run is the pool-pressure baseline of `a_still_pose_converges_...`: 2 nodes.
    assert_eq!(counts[0], (2, 0));
    // A minimum finer than the wanted depth caps nothing: the same coarsening, no stages.
    assert_eq!(counts[1], counts[0]);
    // A cap that lowers the wanted depth may relieve the pool but never shows up as a stage, and
    // what is still coarsened is the pool's doing.
    assert_eq!(counts[2].1, 0);
    assert!(counts[2].0 <= counts[0].0, "{counts:?}");
}

#[test]
fn mask_updates_carry_active_and_node_depth_in_cell_mode() {
    let cam = camera("fit");
    let node_depth = representative_depth(&grid(), cube(ORIGIN, SIDE)).saturating_sub(3);
    let sent = |plan: &FramePlan| {
        plan.mask_updates
            .iter()
            .map(|u| (u.slot, u.active, u.node_depth))
            .collect::<Vec<_>>()
    };
    let mut rig = Rig::new(root_node(), 64, 8, 0);
    let mut updates = Vec::new();
    let mut plan = rig.step(&ids(&[1]), &cam, 64.0);
    for _ in 0..12 {
        updates.extend(sent(&plan));
        plan = rig.step(&ids(&[1]), &cam, 64.0);
    }
    let entry = &plan.entries[0];
    let active = entry
        .required_levels()
        .into_iter()
        .fold(0_u64, |bits, depth| bits | 1 << (depth - node_depth));
    assert_ne!(active, 0);
    let last = updates.last().expect("a node record is sent");
    assert_eq!((last.1, last.2), (active, node_depth), "{updates:?}");
    // One active level per active depth, drawn whole by the GPU's per-cell choice.
    assert!(!plan.active.is_empty());
    assert!(plan.active.iter().all(|level| level.octant_bits == 0xff));
    assert_eq!(plan.active.len(), active.count_ones() as usize);
    // Steady frames send nothing.
    assert!(plan.mask_updates.is_empty());
}

#[test]
fn cell_mode_clamps_the_pool_to_the_node_span() {
    // A leaf of node depth >= 2 whose only stored (and resident) level lies below the node.
    let bounds = cube(ORIGIN, SIDE / 4.0);
    let node_depth = representative_depth(&grid(), bounds).saturating_sub(3);
    assert!(node_depth >= 2);
    let below = node_depth - 1;
    let nodes = HashMap::from([(
        NodeId::new(1),
        PoolNode {
            bounds,
            stored: vec![below],
            buildable: (below, node_depth + 6),
            points: 500,
            octant_data_bounds: None,
        },
    )]);
    let cam = camera("fit");
    let mut rig = Rig::new(nodes, 64, 8, 0);
    seed_resident(&mut rig.pool, 1, &[below]);
    let plan = rig.step(&ids(&[1]), &cam, 64.0);
    assert!(plan.active.is_empty(), "{:?}", plan.active);
    assert!(
        plan.level_requests
            .contains(&rig.pool.key(NodeId::new(1), node_depth)),
        "{:?}",
        plan.level_requests
    );
    assert_eq!(
        plan.level_requests.first().map(|key| key.depth),
        Some(node_depth)
    );
}

/// A leaf whose first stored level lies below its node depth, so no cell can draw it.
fn leaf_below_its_node() -> (HashMap<NodeId, PoolNode>, u8, u8) {
    let bounds = cube(ORIGIN, SIDE / 4.0);
    let node_depth = representative_depth(&grid(), bounds).saturating_sub(3);
    assert!(node_depth >= 2);
    let below = node_depth - 1;
    let nodes = HashMap::from([(
        NodeId::new(1),
        PoolNode {
            bounds,
            stored: vec![below, node_depth + 2],
            buildable: (below, node_depth + 6),
            points: 500,
            octant_data_bounds: None,
        },
    )]);
    (nodes, node_depth, below)
}

#[test]
fn cell_mode_mask_updates_match_the_emitted_active_levels() {
    let (nodes, node_depth, below) = leaf_below_its_node();
    let cam = camera("fit");
    let mut rig = Rig::new(nodes, 64, 8, 0);
    // A resident level below the node (never drawn) and the requested ones as they arrive.
    seed_resident(&mut rig.pool, 1, &[below]);
    let mut checked = 0;
    for _ in 0..12 {
        let plan = rig.step(&ids(&[1]), &cam, 64.0);
        for update in &plan.mask_updates {
            let bits = plan
                .active
                .iter()
                .filter(|level| level.mask_slot == update.slot)
                .fold(0_u64, |bits, level| {
                    bits | 1 << (level.key.depth - node_depth)
                });
            assert_eq!(update.active, bits, "{update:?} vs {:?}", plan.active);
            checked += 1;
        }
    }
    assert!(checked > 0);
}

#[test]
fn cell_mode_fit_cap_reasons_about_the_depths_the_node_can_use() {
    // A capped leaf drew its resident level `near` (above the cap) and now wants a depth below
    // the node, which cell mode clamps to the node depth: it lacks the in-span level that serves
    // it, so `near` keeps drawing. The stored level below the node must not serve the want.
    let bounds = cube(ORIGIN, SIDE / 4.0);
    let node_depth = representative_depth(&grid(), bounds).saturating_sub(3);
    assert!(node_depth >= 2);
    let (below, cap, near) = (node_depth - 1, node_depth + 1, node_depth + 3);
    let id = NodeId::new(1);
    let node = PoolNode {
        bounds,
        stored: vec![below, cap, near],
        buildable: (below, near),
        points: 500,
        octant_data_bounds: None,
    };
    let mut pool = CellPool::new(PoolConfig::default());
    seed_resident(&mut pool, 1, &[below, near]);
    pool.applied_drawn.insert(id, [Some(near); 8]);
    let depths = FastMap::from_iter([(
        id,
        NodeDepths {
            node: node.clone(),
            depths: [below; 8],
            far: (0, 0),
            hi_d: None,
            epoch: 0,
            pose: Pose {
                eye: [0.0; 3],
                forward: [0.0, 0.0, 1.0],
            },
            depth: 0.0,
            exact: true,
            scale: 1.0,
        },
    )]);
    let grid = grid();
    let ctx = Ctx {
        grid: &grid,
        origin: ORIGIN,
        depths: &depths,
        evaluations: Cell::new(0),
        capped: true,
    };
    let cut_node = CellCutNode {
        bounds,
        buildable: (below, cap),
        stored: vec![below, cap],
        resident: vec![below],
    };
    // The level that serves the clamped want (the node depth is not stored: `cap` serves it).
    let needed = FastSet::from_iter([pool.key(id, cap)]);
    assert_eq!(
        pool.finer_drawn(id, cap, &cut_node, &needed, &ctx),
        Some(vec![near])
    );
}

/// A cell-mode rig that never delivers a level (delay far beyond the test), so residency is
/// exactly what the test seeds.
fn d5_rig(nodes: HashMap<NodeId, PoolNode>) -> Rig {
    Rig::new(nodes, 512, 8, 10_000)
}

/// One frame of the pool alone: the fake GPU cannot follow levels that were seeded by hand.
fn d5_step(rig: &mut Rig, cut: &[NodeId], camera: &OrbitCamera) -> FramePlan {
    d5_step_at(rig, cut, camera, 64.0)
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

fn mask_of(plan: &FramePlan, node: u64) -> [u64; 8] {
    plan.entries
        .iter()
        .find(|entry| entry.node == NodeId::new(node))
        .expect("entry")
        .mask
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

/// Parent 1 (node depth 0, rep 3) with a coarse resident level 2 only; child 21 (rep 6) fully
/// resident. The D5 depth of the parent is 4.
fn d5_pair() -> Rig {
    let mut rig = d5_rig(HashMap::from([
        (NodeId::new(1), d5_node(SIDE, &[2, 4, 5])),
        (NodeId::new(21), d5_node(SIDE / 8.0, &[6, 7])),
    ]));
    seed_resident(&mut rig.pool, 1, &[2]);
    seed_resident(&mut rig.pool, 21, &[6, 7]);
    rig
}

#[test]
fn children_wait_for_the_parent_d5_level() {
    let cam = camera("fit");
    let cut = ids(&[1, 21]);
    let mut rig = d5_pair();
    let plan = d5_step(&mut rig, &cut, &cam);
    assert_eq!(plan.apply, CutApply::Applied);
    // The child is held: it neither draws nor masks the parent, which draws its whole zone.
    assert_eq!(plan.counters.held_nodes, 1);
    assert_eq!(mask_of(&plan, 1), [0; 8]);
    assert!(plan.active.iter().all(|l| l.key.node != NodeId::new(21)));
    assert!(plan.active.iter().any(|l| l.key.node == NodeId::new(1)));
    // The parent's D5 depth is requested first.
    assert_eq!(
        plan.fetch_requests.first(),
        Some(&rig.pool.key(NodeId::new(1), 4)),
        "{:?}",
        plan.fetch_requests
    );
    // It stays so while the level is missing, without oscillating.
    for _ in 0..3 {
        let plan = d5_step(&mut rig, &cut, &cam);
        assert_eq!(plan.counters.held_nodes, 1);
        assert_eq!(mask_of(&plan, 1), [0; 8]);
    }
    seed_more(&mut rig, 1, &[4]);
    let plan = d5_step(&mut rig, &cut, &cam);
    assert_eq!(plan.counters.held_nodes, 0);
    assert_ne!(mask_of(&plan, 1), [0; 8]);
    assert!(plan.active.iter().any(|l| l.key.node == NodeId::new(21)));
}

/// `add_d5_gate` in `eval_hold` on a multi-node cut: the child 21 is held, and node 1 recorded as
/// its covering ancestor, while node 1 draws but has no resident D5 depth, whether or not the
/// applied cut already drew node 1. Node 21 has levels deeper than anything node 1 drew, so the
/// refine hold releases it: only the gate holds it (without `add_d5_gate` every leg fails).
/// That the hold ends once the D5 depth is resident is `children_wait_for_the_parent_d5_level`.
#[test]
fn the_d5_gate_holds_a_child_while_its_ancestor_has_no_d5_depth() {
    let (child, parent) = (NodeId::new(21), NodeId::new(1));
    let cam = camera("fit");
    for applied in [false, true] {
        let mut rig = d5_pair();
        if applied {
            let plan = d5_step(&mut rig, &ids(&[1]), &cam);
            assert_eq!(plan.apply, CutApply::Applied);
            assert!(!rig.pool.applied_drawn.is_empty(), "setup");
        }
        for wish in [5, 7] {
            let (eval, _) = eval_with(&rig, &ids(&[1, 21]), wish);
            let case = format!("applied {applied} wish {wish}");
            assert_eq!(eval.hold, FastSet::from_iter([child]), "{case}");
            assert_eq!(eval.settle, FastSet::from_iter([child]), "{case}");
            assert_eq!(eval.ancestors, FastSet::from_iter([parent]), "{case}");
        }
    }
}

#[test]
fn the_d5_request_comes_first() {
    let cam = camera("fit");
    let cut = ids(&[1, 21, 22]);
    let mut rig = d5_rig(HashMap::from([
        (NodeId::new(1), d5_node(SIDE, &[2, 4, 5])),
        (NodeId::new(21), d5_node(SIDE / 8.0, &[6, 7])),
        // A node that already draws (level 5) and wants its coarser level 3, shallower than the D5
        // depth (4): by depth alone that request would come before the parent's.
        (
            NodeId::new(22),
            PoolNode {
                bounds: cube([ORIGIN[0] + SIDE / 2.0, ORIGIN[1], ORIGIN[2]], SIDE / 2.0),
                stored: vec![3, 5],
                buildable: (3, 5),
                points: 2000,
                octant_data_bounds: None,
            },
        ),
    ]));
    seed_resident(&mut rig.pool, 1, &[2]);
    seed_resident(&mut rig.pool, 21, &[6, 7]);
    seed_resident(&mut rig.pool, 22, &[5]);
    let plan = d5_step_at(&mut rig, &cut, &cam, 1.0e6);
    let d5 = rig.pool.key(NodeId::new(1), 4);
    let other = rig.pool.key(NodeId::new(22), 3);
    let position = |key| plan.fetch_requests.iter().position(|k| *k == key);
    assert!(position(other).is_some(), "{:?}", plan.fetch_requests);
    assert_eq!(position(d5), Some(0), "{:?}", plan.fetch_requests);
}

#[test]
fn the_pool_fit_counts_the_d5_level() {
    let cam = camera("fit");
    let cut = ids(&[1, 21]);
    let nodes = HashMap::from([
        (NodeId::new(1), d5_node(SIDE, &[2, 4, 5])),
        (NodeId::new(21), d5_node(SIDE / 8.0, &[6, 7])),
    ]);
    let mut rig = Rig::new(nodes, 64, 8, 10_000);
    seed_resident(&mut rig.pool, 1, &[2]);
    seed_resident(&mut rig.pool, 21, &[6, 7]);
    rig.pool
        .set_level_sizes(NodeId::new(1), &[(2, 1), (4, 22), (5, 22)]);
    // A coarse want: node 1 wants level 2 only, so its D5 level 4 is not wanted by the entry.
    d5_step_at(&mut rig, &cut, &cam, 1.0e6);
    let memo = rig.pool.wanted_memo.as_ref().expect("fit ran");
    // The D5 level takes pages (a fetch under pressure), so the fit must count it.
    let d5 = rig.pool.key(NodeId::new(1), 4);
    assert!(memo.needed.contains(&d5), "{:?}", memo.needed);
}

#[test]
fn the_d5_depth_joins_the_active_set_when_the_mask_is_set() {
    let cam = camera("fit");
    let cut = ids(&[1, 21]);
    let mut rig = d5_pair();
    // A resident level above the D5 depth that nothing else makes active.
    seed_more(&mut rig, 1, &[4]);
    // A coarse want: the entry draws level 2 only, so level 4 is active only for the gate.
    let plan = d5_step_at(&mut rig, &cut, &cam, 1.0e6);
    assert_ne!(mask_of(&plan, 1), [0; 8]);
    let entry = plan
        .entries
        .iter()
        .find(|e| e.node == NodeId::new(1))
        .unwrap();
    assert!(
        !entry.required_levels().contains(&4),
        "{:?}",
        entry.required_levels()
    );
    let active = active_depths(&plan, &rig, 1);
    assert!(active.contains(&4), "{active:?}");
    let update = plan
        .mask_updates
        .iter()
        .find(|u| Some(u.slot) == rig.pool.mask_slots.get(&NodeId::new(1)).map(|s| s.slot))
        .expect("update");
    assert_ne!(update.active & (1 << 4), 0, "{update:?}");
}

#[test]
fn the_d5_gate_settles_within_hold_passes() {
    let cam = camera("fit");
    let cut = ids(&[1, 2, 21]);
    let mut rig = d5_rig(HashMap::from([
        (NodeId::new(1), d5_node(SIDE, &[2, 4, 5])),
        (NodeId::new(2), d5_node(SIDE / 2.0, &[2, 5, 6])),
        (NodeId::new(21), d5_node(SIDE / 8.0, &[6, 7])),
    ]));
    seed_resident(&mut rig.pool, 1, &[2]);
    seed_resident(&mut rig.pool, 2, &[2]);
    seed_resident(&mut rig.pool, 21, &[6, 7]);
    for _ in 0..3 {
        let plan = d5_step(&mut rig, &cut, &cam);
        assert_eq!(plan.apply, CutApply::Applied);
        assert_eq!(plan.counters.hold_unsettled, 0);
        assert_eq!(plan.counters.held_nodes, 2);
        assert_eq!(mask_of(&plan, 1), [0; 8]);
        assert_eq!(mask_of(&plan, 2), [0; 8]);
    }
    // The grandparent's D5 releases the parent; the child still waits for the parent's.
    seed_more(&mut rig, 1, &[4]);
    let plan = d5_step(&mut rig, &cut, &cam);
    assert_eq!(plan.counters.hold_unsettled, 0);
    assert_eq!(plan.counters.held_nodes, 1);
    assert_ne!(mask_of(&plan, 1), [0; 8]);
    seed_more(&mut rig, 2, &[5]);
    let plan = d5_step(&mut rig, &cut, &cam);
    assert_eq!(plan.counters.held_nodes, 0);
    assert_eq!(plan.counters.hold_unsettled, 0);
    assert_ne!(mask_of(&plan, 2), [0; 8]);
}

#[test]
fn a_retained_frame_keeps_the_d5_depth_its_record_was_written_with() {
    let cam = camera("fit");
    let cut = ids(&[1, 21]);
    let mut rig = d5_pair();
    seed_more(&mut rig, 1, &[4]);
    let plan = d5_step_at(&mut rig, &cut, &cam, 1.0e6);
    assert_eq!(plan.apply, CutApply::Applied);
    assert_ne!(mask_of(&plan, 1), [0; 8]);
    let applied = active_depths(&plan, &rig, 1);
    assert!(applied.contains(&4), "{applied:?}");
    // The sizes grow past the pool: the fit caps node 1 below the D5 depth and the frame
    // overflows, so it retains the previous cut (masks are not rewritten).
    rig.pool
        .set_level_sizes(NodeId::new(1), &[(2, 300), (4, 600), (5, 600)]);
    rig.pool
        .set_level_sizes(NodeId::new(21), &[(6, 300), (7, 600)]);
    let plan = d5_step_at(&mut rig, &cut, &cam, 64.0);
    assert_eq!(
        plan.apply,
        CutApply::RetainedPrevious,
        "setup {:?} {:?}",
        plan.counters,
        plan.entries
    );
    assert!(plan.mask_updates.is_empty());
    assert_eq!(active_depths(&plan, &rig, 1), applied);
}

/// ADR 0054: the length minimum bounds the wanted depth, even for a leaf the eye is next to and
/// whose source density is far finer.
#[test]
fn min_side_bounds_the_wanted_depth() {
    let min_side = 1e-4;
    // SIDE / 2^19 is the finest side that is not below the minimum.
    let finest_allowed = 19;
    assert!(SIDE / f64::from(1_u32 << finest_allowed) >= min_side);
    assert!(SIDE / f64::from(1_u32 << (finest_allowed + 1)) < min_side);
    let mut nodes = root_node();
    nodes.get_mut(&NodeId::new(1)).unwrap().buildable = (3, 25);
    for cap in [None, Some(min_side)] {
        let mut rig = Rig::new(nodes.clone(), 64, 16, 1);
        rig.pool.set_min_side(cap);
        for eye in [
            [ORIGIN[0] + 20.0, ORIGIN[1] + 30.0, ORIGIN[2] + 10.0],
            [ORIGIN[0] - 0.01, ORIGIN[1] + 30.0, ORIGIN[2] + 10.0],
        ] {
            let mut cam = camera("fit");
            assert!(cam.set_pose(eye, [ORIGIN[0] + 32.0, ORIGIN[1] + 32.0, ORIGIN[2] + 32.0]));
            let plan = rig.run(12, &ids(&[1]), &cam, 4.0);
            for wanted in plan.entries[0].wanted.iter().flatten() {
                match cap {
                    Some(_) => assert!(*wanted <= finest_allowed, "wanted {wanted}"),
                    None => assert!(*wanted > finest_allowed, "setup: uncapped wanted {wanted}"),
                }
            }
        }
    }
}

#[test]
fn cell_mode_activates_resident_depths_across_the_range() {
    // Residents at every stored depth: the active set also holds the resident depths between
    // the node's far (lo) and near (hi) depths that no octant draws.
    let cam = camera("zoom");
    let nodes = HashMap::from([(
        NodeId::new(1),
        d5_node(SIDE, &[2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]),
    )]);
    let mut rig = Rig::new(nodes, 4096, 64, 0);
    seed_resident(&mut rig.pool, 1, &[2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    rig.pool.residency_epoch += 1;
    let plan = d5_step_at(&mut rig, &ids(&[1]), &cam, 2.0);
    let entry = &plan.entries[0];
    let (lo, hi) = entry.range.expect("a cell-mode node has a range");
    assert!(lo < hi, "{lo}..{hi}");
    let required = entry.required_levels();
    assert!(required.iter().all(|d| *d <= hi));
    assert!(
        (lo..=hi).any(|d| !required.contains(&d)),
        "the range reaches depths no octant draws: {lo}..{hi} vs {required:?}"
    );
    let expected: Vec<u8> = (lo..=hi)
        .chain(required.iter().copied())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(active_depths(&plan, &rig, 1), expected);
}

#[test]
fn cell_mode_fetches_stored_depths_across_the_range() {
    // Only the coarsest depth is resident: every other stored depth inside the node's range is
    // fetched, not only the octants' wanted ones.
    let cam = camera("zoom");
    let stored = [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    let nodes = HashMap::from([(NodeId::new(1), d5_node(SIDE, &stored))]);
    let mut rig = Rig::new(nodes, 4096, 64, 0);
    seed_resident(&mut rig.pool, 1, &[2]);
    rig.pool.residency_epoch += 1;
    let plan = d5_step_at(&mut rig, &ids(&[1]), &cam, 2.0);
    let (lo, hi) = plan.entries[0].range.expect("a cell-mode node has a range");
    assert!(lo < hi, "{lo}..{hi}");
    let wanted = plan.entries[0].required_levels();
    let fetched: Vec<u8> = plan.fetch_requests.iter().map(|key| key.depth).collect();
    for depth in (lo..=hi).filter(|d| *d != 2) {
        assert!(
            fetched.contains(&depth),
            "depth {depth} of {lo}..{hi}: {fetched:?}"
        );
    }
    assert!(
        (lo..=hi).any(|d| !wanted.contains(&d)),
        "the range reaches depths no octant wants: {lo}..{hi} vs {wanted:?}"
    );
    // Coarse to fine.
    assert!(
        fetched.windows(2).all(|pair| pair[0] <= pair[1]),
        "{fetched:?}"
    );
}

/// A node whose range reaches below its stored chain: depths 7 and 8 are buildable, not stored,
/// and above the spacing depth (the finest stored one, 9); 10 and 11 are below it.
fn range_gap_rig() -> Rig {
    let mut node = d5_node(SIDE, &[2, 3, 4, 9]);
    node.buildable = (2, 14);
    node.points = 512 * 64;
    let mut rig = Rig::new(HashMap::from([(NodeId::new(1), node)]), 4096, 64, 0);
    seed_resident(&mut rig.pool, 1, &[2]);
    rig.pool.residency_epoch += 1;
    rig
}

#[test]
fn cell_mode_requests_missing_depths_across_the_range_down_to_spacing() {
    let mut rig = range_gap_rig();
    let plan = d5_step_at(&mut rig, &ids(&[1]), &camera("zoom"), 2.0);
    let (lo, hi) = plan.entries[0].range.expect("a cell-mode node has a range");
    assert_eq!((lo, hi), (7, 11), "setup");
    let requested: Vec<u8> = plan.level_requests.iter().map(|key| key.depth).collect();
    for depth in [7, 8] {
        assert!(requested.contains(&depth), "{depth} missing: {requested:?}");
    }
    // 10 lies in the range beyond the spacing depth and no octant wants it: not built.
    assert!(!requested.contains(&10), "{requested:?}");
    // Octant (near) depths keep their behaviour: 11 is wanted by an octant.
    assert!(requested.contains(&11), "{requested:?}");
    // Coarse to fine.
    assert_eq!(requested, [7, 8, 11]);
    // Not stored, so nothing is fetched for them.
    let fetched: Vec<u8> = plan.fetch_requests.iter().map(|key| key.depth).collect();
    assert!(
        !fetched.contains(&7) && !fetched.contains(&8),
        "{fetched:?}"
    );
}

/// Node with stored {2, 3, 4, 9}, buildable 2..=14; `resident` are seeded before the frame.
fn gap_rig_with(resident: &[u8]) -> Rig {
    let mut node = d5_node(SIDE, &[2, 3, 4, 9]);
    node.buildable = (2, 14);
    node.points = 512 * 64;
    let mut rig = Rig::new(HashMap::from([(NodeId::new(1), node)]), 4096, 64, 0);
    seed_resident(&mut rig.pool, 1, resident);
    rig.pool.residency_epoch += 1;
    rig
}

#[test]
fn cell_mode_fills_gaps_between_active_depths() {
    // 7 and 11 are resident and active (range 7..=11): 8 and 10 lie between them, so they are
    // requested although 10 is below the spacing depth 9; 9 is stored and fetched instead.
    let mut rig = gap_rig_with(&[2, 7, 11]);
    let plan = d5_step_at(&mut rig, &ids(&[1]), &camera("zoom"), 2.0);
    let requested: Vec<u8> = plan.level_requests.iter().map(|key| key.depth).collect();
    for depth in [8, 10] {
        assert!(requested.contains(&depth), "{depth} missing: {requested:?}");
    }
    assert!(!requested.contains(&9), "{requested:?}");
    assert!(
        !requested.contains(&12),
        "nothing past the finest active depth: {requested:?}"
    );
}

#[test]
fn cell_mode_fills_gaps_below_spacing_for_a_leaf() {
    // A leaf whose stored chain ends at 9 and whose resident depths are 9 and 11: 10 is below
    // the spacing depth and gets built because it lies between two active depths.
    let mut rig = gap_rig_with(&[2, 9, 11]);
    let plan = d5_step_at(&mut rig, &ids(&[1]), &camera("zoom"), 2.0);
    let requested: Vec<u8> = plan.level_requests.iter().map(|key| key.depth).collect();
    assert!(requested.contains(&10), "{requested:?}");
    assert!(!requested.contains(&12), "{requested:?}");
}

#[test]
fn a_node_requests_its_range_up_to_spacing_and_the_wanted_depths() {
    // The octants wish only the ends of the range (7 and 11) and `hi` stops at 10: the node range
    // is `[7, 11]`. Every depth up to the spacing depth (9, stored) is requested or fetched, the
    // wanted 11 beyond it is requested, and the unwanted 10 is not.
    let mut rig = range_gap_rig();
    let plan = d5_step_at(&mut rig, &ids(&[1]), &camera("zoom"), 2.0);
    let mut entry = plan.entries[0].clone();
    entry.wanted = [None; 8];
    entry.wanted[0] = Some(7);
    entry.wanted[1] = Some(11);
    entry.range = Some((7, 10));
    assert_eq!(CellPool::node_range(&entry), Some((7, 11)));
    let nodes: FastMap<NodeId, PoolNode> =
        rig.nodes.iter().map(|(id, n)| (*id, n.clone())).collect();
    let none = FastMap::default();
    let entries = [entry];
    let key = |depth: u8| rig.pool.key(NodeId::new(1), depth);
    let requested = rig.pool.requested_levels(&entries, &nodes, &none, &grid());
    for depth in [7, 8, 11] {
        assert!(requested.contains(&key(depth)), "{depth}: {requested:?}");
    }
    // 9 is stored, so it is fetched as the level that serves the range, not requested; 10 is
    // beyond the spacing depth and nobody wants it.
    assert!(!requested.contains(&key(9)), "{requested:?}");
    assert!(!requested.contains(&key(10)), "{requested:?}");
    let needed = rig.pool.needed_levels(&entries, &nodes, &none, &grid());
    assert!(needed.contains(&key(9)), "{needed:?}");
}

#[test]
fn the_fit_counts_requested_range_depths() {
    let mut rig = range_gap_rig();
    let plan = d5_step_at(&mut rig, &ids(&[1]), &camera("zoom"), 2.0);
    let nodes: FastMap<NodeId, PoolNode> =
        rig.nodes.iter().map(|(id, n)| (*id, n.clone())).collect();
    let none = FastMap::default();
    let (needed, total) =
        rig.pool
            .needed_and_pages(&plan.entries, &nodes, &none, &grid(), &FastSet::default());
    let pages = |depth: u8| {
        rig.pool.pages_of(
            rig.pool.key(NodeId::new(1), depth),
            &nodes[&NodeId::new(1)],
            &grid(),
        )
    };
    let needed_pages: usize = needed
        .iter()
        .map(|key| rig.pool.pages_of(*key, &nodes[&key.node], &grid()))
        .sum();
    for depth in [7, 8] {
        assert!(!needed.contains(&rig.pool.key(NodeId::new(1), depth)));
    }
    // The counted set is the parent's: the spacing-limited range depths. The wanted depth 11
    // lies beyond the spacing depth and is requested but not counted, as before.
    assert_eq!(total as usize, needed_pages + pages(7) + pages(8));
}

// ADR 0053: stepping through pruned residents, timeouts and hops. `Rig::step` checks every frame
// against the stepping oracle, so these scenarios only pin down when the hops happen.

/// Every octant of node 2 wants the same depth, so a hop counts once per octant.
const OCTANTS: u32 = 8;

/// One frame of node 2 alone in the zoom camera: `(drawn, step, step_hops)` of its octant 0.
fn step_node_2(rig: &mut Rig, target: f64) -> (Option<u8>, Option<u8>, u32) {
    let plan = rig.step(&ids(&[2]), &camera("zoom"), target);
    plan.entries.first().map_or((None, None, 0), |e| {
        (e.drawn[0], e.step[0], plan.counters.step_hops)
    })
}

/// Node 2 draws depth 7 (target 4) with every depth resident; `gone` are evicted and `refused`
/// never arrive again, then the view wants depth 4 (target 64) or 5 (target 32).
fn node_2_drawing_7(gone: &[u8], refused: &[u8]) -> Rig {
    let mut nodes = octant_nodes();
    let node = nodes.get_mut(&NodeId::new(2)).expect("node 2");
    node.stored = vec![3, 4, 5, 6, 7];
    node.buildable = (3, 7);
    let mut rig = Rig::new(nodes, 256, 64, 0);
    // Walk the depths up, so that each one is resident.
    for target in [64.0, 32.0, 16.0, 4.0] {
        for _ in 0..30 {
            step_node_2(&mut rig, target);
        }
    }
    assert_eq!(step_node_2(&mut rig, 4.0), (Some(7), None, 0), "setup");
    for depth in gone {
        assert!(rig.pool.evict(rig.pool.key(NodeId::new(2), *depth)));
    }
    rig.refused = refused
        .iter()
        .map(|depth| rig.pool.key(NodeId::new(2), *depth))
        .collect();
    rig
}

#[test]
fn a_level_that_never_arrives_is_skipped_after_the_wait_with_one_counted_hop() {
    // Depths 6 and 5 are gone and refused: 7 waits for 6, then hops to the closest resident (4).
    let mut rig = node_2_drawing_7(&[5, 6], &[5, 6]);
    for frame in 0..STEP_WAIT_FRAMES {
        assert_eq!(
            step_node_2(&mut rig, 64.0),
            (Some(7), Some(6), 0),
            "frame {frame}"
        );
    }
    assert_eq!(
        step_node_2(&mut rig, 64.0),
        (Some(4), None, OCTANTS),
        "the hop"
    );
    assert_eq!(step_node_2(&mut rig, 64.0), (Some(4), None, 0));
}

#[test]
fn the_wait_survives_a_wanted_depth_that_flips_towards_the_same_side() {
    // Wanted 4 and 5 alternate every frame (both below the drawn 7, awaiting 6): one wait.
    let mut rig = node_2_drawing_7(&[5, 6], &[5, 6]);
    let targets = [64.0, 32.0];
    for frame in 0..STEP_WAIT_FRAMES as usize {
        let (drawn, step, hops) = step_node_2(&mut rig, targets[frame % 2]);
        assert_eq!((drawn, step, hops), (Some(7), Some(6), 0), "frame {frame}");
    }
    let (drawn, _, hops) = step_node_2(&mut rig, targets[0]);
    assert_eq!(
        (drawn, hops),
        (Some(4), OCTANTS),
        "the wait did not restart"
    );
}

#[test]
fn each_hop_gets_its_own_wait_and_a_refused_level_never_reads_as_rest() {
    // 6 and 4 are gone and refused, 5 stays: 7 hops to 5 after one wait, then 4 (still wanted,
    // never delivered) is waited for in turn, and the octant stays pending.
    let mut rig = node_2_drawing_7(&[6, 4], &[6, 4]);
    for frame in 0..STEP_WAIT_FRAMES {
        assert_eq!(
            step_node_2(&mut rig, 64.0),
            (Some(7), Some(6), 0),
            "frame {frame}"
        );
    }
    assert_eq!(
        step_node_2(&mut rig, 64.0),
        (Some(5), Some(4), OCTANTS),
        "first hop"
    );
    for frame in 0..3 * STEP_WAIT_FRAMES {
        assert_eq!(
            step_node_2(&mut rig, 64.0),
            (Some(5), Some(4), 0),
            "frame {frame}"
        );
    }
}

#[test]
fn an_adjacent_level_arriving_during_the_wait_is_taken_and_restarts_the_wait() {
    // 6 arrives at frame 10: 7 -> 6 normally; 5 never arrives, so the next hop comes a full wait
    // after reaching 6, to the resident 4.
    let mut rig = node_2_drawing_7(&[5, 6], &[5, 6]);
    for _ in 0..10 {
        step_node_2(&mut rig, 64.0);
    }
    rig.refused.remove(&rig.pool.key(NodeId::new(2), 6));
    let mut frames = Vec::new();
    for _ in 0..3 * STEP_WAIT_FRAMES {
        frames.push(step_node_2(&mut rig, 64.0));
    }
    let reached_6 = frames.iter().position(|f| f.0 == Some(6)).expect("6 drawn");
    assert!(reached_6 < 4, "{reached_6}");
    assert!(
        frames[..reached_6]
            .iter()
            .all(|f| f.0 == Some(7) && f.2 == 0)
    );
    let reached_4 = frames.iter().position(|f| f.0 == Some(4)).expect("4 drawn");
    assert_eq!(
        reached_4 - reached_6,
        STEP_WAIT_FRAMES as usize,
        "{frames:?}"
    );
    assert_eq!(frames.iter().map(|f| f.2).sum::<u32>(), OCTANTS);
}

#[test]
fn retained_frames_advance_the_step_wait() {
    // 7 waits for 6, which never arrives. A pose jump to a cut that cannot draw yet keeps the
    // previous one on screen (`RetainedPrevious`); those frames count towards the wait, so back
    // on node 2 the octants hop at once to the resident 4.
    let mut rig = node_2_drawing_7(&[5, 6], &[5, 6]);
    assert_eq!(
        step_node_2(&mut rig, 64.0),
        (Some(7), Some(6), 0),
        "the wait starts"
    );
    rig.held.extend([NodeId::new(6), NodeId::new(7)]);
    for frame in 1..STEP_WAIT_FRAMES {
        let plan = rig.step(&ids(&[6, 7]), &camera("zoom"), 64.0);
        assert_eq!(plan.apply, CutApply::RetainedPrevious, "frame {frame}");
        assert_eq!(plan.counters.step_hops, 0, "frame {frame}");
    }
    assert_eq!(
        step_node_2(&mut rig, 64.0),
        (Some(4), None, OCTANTS),
        "the retained frames waited"
    );
}

#[test]
fn a_node_activates_the_contiguous_resident_run_of_its_range() {
    // Only depth 9 serves the node when it enters the cut (range 7..=11), so it draws 9 alone.
    // A range depth beyond a residency gap stays inactive (ADR 0055, proposed): with 10 missing,
    // 11 would make the per-cell rule jump over it. Once the range is resident it is all
    // active at once: no window of one level per frame.
    let nodes = HashMap::from([(
        NodeId::new(1),
        d5_node(SIDE, &[2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]),
    )]);
    let mut rig = Rig::new(nodes, 4096, 64, 10_000);
    seed_resident(&mut rig.pool, 1, &[2, 9]);
    rig.pool.residency_epoch += 1;
    let cam = camera("zoom");
    let plan = d5_step_at(&mut rig, &ids(&[1]), &cam, 2.0);
    assert_eq!(plan.entries[0].range, Some((7, 11)), "setup");
    assert_eq!(active_depths(&plan, &rig, 1), [9], "entering the cut");
    seed_more(&mut rig, 1, &[11]);
    let gapped = active_depths(&d5_step_at(&mut rig, &ids(&[1]), &cam, 2.0), &rig, 1);
    assert_eq!(gapped, [9], "11 lies beyond the gap at 10");
    seed_more(&mut rig, 1, &[7, 8, 10]);
    let full = active_depths(&d5_step_at(&mut rig, &ids(&[1]), &cam, 2.0), &rig, 1);
    assert_eq!(full, [7, 8, 9, 10, 11], "the whole range is resident");
}

/// Children of node 2 (ids 10..=17) that store depths 4..=10, with node 2 drawing depth 6 under
/// `[2, 3]` first. Returns the rig and the cut of the children.
fn children_entering_below_node_2() -> (Rig, Vec<NodeId>) {
    let mut nodes = two_regions_with_coarse_children();
    for child in region_children(10) {
        let node = nodes.get_mut(&NodeId::new(child)).expect("child");
        node.stored = (4..=10).collect();
        node.buildable = (4, 10);
    }
    let mut rig = Rig::new(nodes, 4096, 64, 0);
    let plan = rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
    let drawn = depth_of(&plan, 2)
        .into_iter()
        .flatten()
        .max()
        .expect("node 2 draws");
    assert_eq!(drawn, 6, "setup");
    let mut cut = ids(&region_children(10));
    cut.push(NodeId::new(3));
    (rig, cut)
}

/// The first plan in which every child draws something, and what each child drew in it.
fn first_drawn_children(rig: &mut Rig, cut: &[NodeId], target: f64) -> (FramePlan, Vec<u8>) {
    for _ in 0..30 {
        let plan = rig.step(cut, &camera("fit"), target);
        let drawn: Vec<Vec<u8>> = region_children(10)
            .into_iter()
            .map(|child| {
                plan.entries
                    .iter()
                    .find(|e| e.node.get() == child)
                    .map_or_else(Vec::new, |e| e.drawn.iter().flatten().copied().collect())
            })
            .collect();
        if drawn.iter().all(|d| !d.is_empty()) {
            return (plan, drawn.into_iter().flatten().collect());
        }
    }
    panic!("the children never drew");
}

#[test]
fn the_stepping_oracle_runs_in_cell_mode() {
    // The children enter at 6 or 7 and want 10 through resident levels: they step one level per
    // frame, and the cell branch of the oracle checks every one of those frames.
    let (mut rig, cut) = children_entering_below_node_2();
    let before = rig.cell_checked;
    let (_, first) = first_drawn_children(&mut rig, &cut, 0.25);
    let mut deepest = first.into_iter().max().expect("children draw");
    let mut steps = 0;
    for _ in 0..30 {
        let plan = rig.step(&cut, &camera("fit"), 0.25);
        let now = region_children(10)
            .into_iter()
            .flat_map(|child| depth_of(&plan, child))
            .flatten()
            .max()
            .expect("children draw");
        if now > deepest {
            assert_eq!(now, deepest + 1, "one level per frame");
            steps += 1;
        }
        deepest = deepest.max(now);
    }
    assert!(steps > 0, "no drawn depth stepped towards the wanted one");
    assert!(
        rig.cell_checked > before,
        "the cell branch of the oracle checked nothing"
    );
    rig.assert_stepping_was_checked();
}

#[test]
fn an_entering_leaf_starts_one_level_below_its_ancestor() {
    // Node 2 drew depth 6: its children enter at 6 or 7 (one level from it) although they want
    // 10 and the level is as available as the others.
    let (mut rig, cut) = children_entering_below_node_2();
    let (_, drawn) = first_drawn_children(&mut rig, &cut, 0.25);
    assert!(drawn.iter().all(|d| (6..=7).contains(d)), "{drawn:?}");
}

#[test]
fn a_leaving_node_hands_back_one_level() {
    // The children drew from depth 7 and the cut returns to node 2: it restarts one level below
    // their coarsest drawn depth, not at the level closest to what it wants.
    let (mut rig, cut) = children_entering_below_node_2();
    first_drawn_children(&mut rig, &cut, 0.25);
    // Node 2 has left the cut by now; the children are still at their first depth.
    let last = rig.step(&cut, &camera("fit"), 0.25);
    let coarsest = region_children(10)
        .into_iter()
        .flat_map(|child| depth_of(&last, child))
        .flatten()
        .min()
        .expect("children draw");
    let mut plan = rig.step(&ids(&[2, 3]), &camera("fit"), 64.0);
    while !has_entry(&plan, 2) {
        plan = rig.step(&ids(&[2, 3]), &camera("fit"), 64.0);
    }
    let first: Vec<u8> = depth_of(&plan, 2).into_iter().flatten().collect();
    assert!(
        !first.is_empty() && first.iter().all(|d| *d + 1 == coarsest),
        "{first:?}"
    );
}

/// The eight octant nodes with stored (and buildable) depths 4..=12: in the zoom camera the
/// near ones get a range of several depths.
fn wide_octant_nodes() -> HashMap<NodeId, PoolNode> {
    let mut nodes = octant_nodes();
    for node in nodes.values_mut() {
        node.stored = (4..=12).collect();
        node.buildable = (4, 12);
    }
    nodes
}

/// What the wide-range scenarios must have exercised: nodes whose range has `lo < hi` and an
/// active level that is range-only (active, but not the depth any octant draws).
fn count_range_depths(plan: &FramePlan) -> usize {
    plan.entries
        .iter()
        .filter_map(|entry| entry.range.filter(|(lo, hi)| lo < hi).map(|_| entry))
        .map(|entry| {
            let drawn = entry.required_levels();
            plan.active
                .iter()
                .filter(|level| level.key.node == entry.node && !drawn.contains(&level.key.depth))
                .count()
        })
        .sum()
}

#[test]
fn an_active_range_depth_is_never_evicted_under_pressure() {
    let mut rig = Rig::new(wide_octant_nodes(), 80, 2, 1);
    let cut = ids(&CHILDREN);
    let (mut evictions, mut range_depths_active, mut previously_active) = (0, 0, 0);
    let mut previous: HashSet<LevelKey> = HashSet::new();
    for frame in 0..80 {
        let mut cam = camera("zoom");
        cam.orbit(0.004 * f64::from(frame), 0.0);
        let plan = rig.step(&cut, &cam, 2.0);
        range_depths_active += count_range_depths(&plan);
        for event in &plan.events {
            let CellPoolEvent::Evict(key) = event else {
                continue;
            };
            evictions += 1;
            // An active level that leaves the wanted set may go; one inside the range the cut
            // still gives its node would be active again at once, so it must be kept.
            if !previous.contains(key) {
                continue;
            }
            previously_active += 1;
            let in_range = plan.entries.iter().any(|entry| {
                entry.node == key.node
                    && entry
                        .range
                        .is_some_and(|(lo, hi)| (lo..=hi).contains(&key.depth))
            });
            assert!(
                !in_range,
                "frame {frame}: {key:?} was active, is still in its node's range and was evicted"
            );
        }
        previous = plan.active.iter().map(|level| level.key).collect();
    }
    // Guards: the pool really was under pressure and range-only depths really were active.
    assert!(evictions > 0, "no eviction happened");
    assert!(
        range_depths_active > 0,
        "no range-only active depth occurred"
    );
    assert!(previously_active > 0, "no active level was ever evicted");
    rig.assert_gpu_matches_pool();
    rig.assert_stepping_was_checked();
}

#[test]
fn cap_clamps_the_range_and_fit_reports_it_with_wide_ranges() {
    // The eight near nodes want ranges of several depths in the zoom camera, which do not fit:
    // the fit caps the farthest nodes first and reports it, and no capped node keeps an active
    // depth finer than its cap.
    let mut rig = Rig::new(wide_octant_nodes(), 80, 64, 1);
    let cut = ids(&CHILDREN);
    let cam = camera("zoom");
    let near = by_distance(&cam, &CHILDREN);
    let (mut reported, mut capped_range_depths) = (0, 0);
    for frame in 0..40 {
        let plan = rig.step(&cut, &cam, 2.0);
        if plan.counters.pool_coarsened == 0 {
            continue;
        }
        reported += 1;
        assert_eq!(
            plan.coarsened_nodes.len(),
            plan.counters.pool_coarsened as usize,
            "frame {frame}"
        );
        // Farthest first: the farthest node is capped, and walking from the farthest to the
        // nearest node the caps never get coarser (an uncapped node in between is allowed:
        // the fit skips a node whose coarsening saves no pages).
        let farthest = *near.last().expect("nodes");
        assert!(
            plan.coarsened_nodes.contains(&NodeId::new(farthest)),
            "frame {frame}: {:?}",
            plan.coarsened_nodes
        );
        let caps: Vec<u8> = near
            .iter()
            .rev()
            .filter_map(|id| rig.pool.caps.get(&NodeId::new(*id)).copied())
            .collect();
        assert!(caps.is_sorted(), "frame {frame}: caps far to near {caps:?}");
        for key in plan.fetch_requests.iter().chain(&plan.level_requests) {
            if let Some(cap) = rig.pool.caps.get(&key.node) {
                assert!(
                    key.depth <= *cap,
                    "frame {frame}: {key:?} requested above cap {cap}"
                );
            }
        }
        for entry in &plan.entries {
            let Some(cap) = rig.pool.caps.get(&entry.node) else {
                continue;
            };
            let drawn = entry.required_levels();
            for level in plan.active.iter().filter(|l| l.key.node == entry.node) {
                assert!(
                    level.key.depth <= *cap,
                    "frame {frame}: {:?} above cap {cap}",
                    level.key
                );
                if entry.range.is_some_and(|(lo, hi)| lo < hi) && !drawn.contains(&level.key.depth)
                {
                    capped_range_depths += 1;
                }
            }
        }
    }
    assert!(reported > 0, "the fit never reported a coarsening");
    assert!(
        capped_range_depths > 0,
        "no capped node had a range-only active depth"
    );
    rig.assert_stepping_was_checked();
}

#[test]
fn every_active_depth_is_fully_uploaded_with_a_wide_range() {
    // One page per frame: levels spend several frames partially uploaded, while their node's
    // range already holds resident depths. No frame may activate a level that lacks pages.
    let mut rig = Rig::new(wide_octant_nodes(), 4096, 1, 1);
    let cut = ids(&CHILDREN);
    let cam = camera("zoom");
    let (mut range_depths_active, mut partial) = (0, 0);
    for frame in 0..200 {
        let plan = rig.step(&cut, &cam, 2.0);
        range_depths_active += count_range_depths(&plan);
        let active: HashSet<LevelKey> = plan.active.iter().map(|level| level.key).collect();
        for key in &active {
            assert!(
                rig.pool.is_resident(*key),
                "frame {frame}: {key:?} is not resident"
            );
            let pages = rig.pool.level(*key).expect("level").pages.len();
            let uploaded = (0..pages)
                .filter(|page| rig.gpu.slot_of.contains_key(&(*key, *page)))
                .count();
            assert_eq!(uploaded, pages, "frame {frame}: {key:?} lacks pages");
        }
        partial += rig
            .pool
            .levels
            .iter()
            .filter(|(key, level)| {
                !active.contains(key)
                    && matches!(&level.state, LevelState::Uploading { slots, pages_done }
                        if *pages_done < slots.len())
            })
            .count();
    }
    assert!(
        range_depths_active > 0,
        "no range-only active depth occurred"
    );
    assert!(partial > 0, "no level was ever observed partially uploaded");
    rig.assert_stepping_was_checked();
}

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

#[test]
fn hold_threshold_is_the_finest_active_depth_end_to_end() {
    // Root 1 draws, in the zoom camera, a range of depths 7..=11 whose octant over child 2 draws
    // depth 8 but whose finest active depth is 11 (the GPU picks per cell among the active
    // ones). Child 2 wants 12: with only 9 or 10 resident it would draw below what the ancestor
    // showed there, so it stays held; it takes over once it can draw the ancestor's finest
    // active depth (11), although that is below the 12 it wants.
    let mut rig = hold_rig();
    let zoom = camera("zoom");
    let mut plan = d5_step_at(&mut rig, &ids(&[1]), &zoom, 2.0);
    for _ in 0..8 {
        plan = d5_step_at(&mut rig, &ids(&[1]), &zoom, 2.0);
    }
    let root = NodeId::new(1);
    let (lo, hi) = plan.entries[0].range.expect("a cell-mode node has a range");
    let finest = rig.pool.applied_finest[&root];
    // Child 2 sits under octant 0 of the root.
    let nominal = rig.pool.applied_drawn[&root][0].expect("octant 0 draws");
    // Guards: a wide range whose finest active depth is finer than the octant's drawn one.
    assert!(lo < hi, "range {lo}..{hi}");
    assert!(active_depths(&plan, &rig, 1).len() > 1);
    assert!(nominal + 2 < finest, "nominal {nominal}, finest {finest}");
    let cut = ids(&CHILDREN);
    let child = NodeId::new(2);
    for (resident, held) in [
        (vec![nominal + 1], true),
        (vec![finest - 1], true),
        (vec![finest], false),
    ] {
        seed_more(&mut rig, 2, &resident);
        for frame in 0..2 {
            let plan = d5_step_at(&mut rig, &cut, &zoom, 0.125);
            let entry = plan
                .entries
                .iter()
                .find(|e| e.node == child)
                .expect("child");
            let wanted = entry.wanted[0].expect("child wants a depth");
            assert!(finest < wanted, "the child wants {wanted}");
            assert_eq!(
                rig.pool.hold.contains(&child),
                held,
                "resident {resident:?} frame {frame}"
            );
            assert_eq!(entry.drawn[0].is_some(), !held, "{resident:?}");
            assert_eq!(rig.pool.applied_finest[&root], finest, "threshold moved");
        }
    }
}

#[test]
fn real_memo_is_recomputed_when_only_the_active_span_changes() {
    // Root 1 shows depths 7..=11 and child 2 (resident 10, wants 12) is held under its finest
    // active depth 11 until the memo of the real cut is reused. Then root's level 11 goes away:
    // the finest active depth falls to 10 with the pose, the cut and every level of the child
    // unchanged. The next frame must read the new span and release the child; the memo must not
    // keep the decision taken under the old one.
    let mut rig = hold_rig();
    let zoom = camera("zoom");
    let root = NodeId::new(1);
    let child = NodeId::new(2);
    let mut plan = d5_step_at(&mut rig, &ids(&[1]), &zoom, 2.0);
    for _ in 0..8 {
        plan = d5_step_at(&mut rig, &ids(&[1]), &zoom, 2.0);
    }
    let finest = rig.pool.applied_finest[&root];
    assert!(active_depths(&plan, &rig, 1).len() > 1, "no wide range");
    seed_more(&mut rig, 2, &[finest - 1]);
    let cut = ids(&CHILDREN);
    let mut reused = false;
    for frame in 0..10 {
        let plan = d5_step_at(&mut rig, &cut, &zoom, 0.125);
        reused |= plan.counters.real_reused;
        assert!(rig.pool.hold.contains(&child), "frame {frame}: held");
    }
    // Guards: the decision was memoised, and it depends on the span (finest - 1 is below it).
    assert!(reused, "the real cut was never reused");
    assert_eq!(rig.pool.applied_finest[&root], finest);
    let gone = rig.pool.key(root, finest);
    assert!(rig.pool.levels.remove(&gone).is_some());
    rig.pool.residency_epoch += 1;
    // The frame that applies the smaller span still judges against what was on screen; the
    // next one must judge against the new span.
    let first = d5_step_at(&mut rig, &cut, &zoom, 0.125);
    assert!(!first.counters.real_reused, "stale real cut");
    assert!(rig.pool.applied_finest[&root] < finest);
    let plan = d5_step_at(&mut rig, &cut, &zoom, 0.125);
    assert!(!rig.pool.hold.contains(&child), "still held");
    let entry = plan
        .entries
        .iter()
        .find(|e| e.node == child)
        .expect("child");
    assert_eq!(entry.drawn[0], Some(finest - 1));
}

#[test]
fn requests_rank_first_step_then_coarse_to_fine_nearest_first() {
    // Eight octant nodes store depths {4, 12} (both resident) and can build the ones between; in
    // the zoom camera the near ones want a range of depths. Every depth of the node ranges is
    // requested from one source, so apart from the step depths the requests rank by depth, coarse
    // to fine, then nearest node first (and by id on an exact tie).
    let mut nodes = octant_nodes();
    for node in nodes.values_mut() {
        node.stored = vec![4, 12];
        node.buildable = (4, 12);
    }
    let mut rig = d5_rig(nodes);
    for id in CHILDREN {
        seed_resident(&mut rig.pool, id, &[4, 12]);
    }
    rig.pool.residency_epoch += 1;
    // Off the symmetry axis, so no two nodes of a pair are at the same distance.
    let mut zoom = camera("zoom");
    zoom.orbit(-0.03, 0.02);
    let cut = ids(&CHILDREN);
    let mut plan = d5_step_at(&mut rig, &cut, &zoom, 2.0);
    for _ in 0..3 {
        plan = d5_step_at(&mut rig, &cut, &zoom, 2.0);
    }
    let requests: Vec<(u64, u8)> = plan
        .level_requests
        .iter()
        .map(|key| (key.node.get(), key.depth))
        .collect();
    let near = by_distance(&zoom, &CHILDREN);
    let at = |id: u64| near.iter().position(|n| *n == id).expect("node");
    // The nearer node of each pair has the higher id, so the id tie-break cannot give the order.
    assert!(at(3) < at(2) && at(9) < at(8), "{near:?}");
    // Guard: a node asks for several depths, so the order within a node is exercised.
    assert!(
        requests.iter().filter(|r| r.0 == 9).count() > 1,
        "{requests:?}"
    );
    assert!(requests.is_sorted_by_key(|r| r.1), "{requests:?}");
    // Within a depth the nearer node of a pair goes first (here the ranking is by the
    // distance to the node box, which `by_distance` only approximates for the other nodes).
    let first = |node: u64, depth: u8| requests.iter().position(|r| *r == (node, depth));
    // Both nodes of a pair ask for these depths (the restored rule: up to the spacing depth,
    // plus what an octant wants).
    for (nearer, farther, depths) in [(9, 8, 8..=11), (3, 2, 7..=8)] {
        for depth in depths {
            let a = first(nearer, depth).expect("nearer requested");
            let b = first(farther, depth).expect("farther requested");
            assert!(a < b, "depth {depth}: {requests:?}");
        }
    }
}

// ---- Data bound of the node range (ADR 0055 plan, task 5; stored, not used) -----------------

/// Replaces the pages of every resident level of `id` by one cell containing the local point
/// `at` (the page count is kept: the upload bookkeeping does not change).
fn put_pages_at(rig: &mut Rig, id: NodeId, at: [f64; 3]) {
    for (key, level) in &mut rig.pool.levels {
        let Some(data) = level.data.as_mut().filter(|_| key.node == id) else {
            continue;
        };
        let side = SIDE / 2.0_f64.powi(i32::from(key.depth));
        let mut packed = (**data).clone();
        for page in &mut packed.pages {
            page.origin = at.map(|v| (v / side) as u64);
            page.box_max = [0; 3];
        }
        *data = Arc::new(packed);
    }
}

/// Local points of the zoom camera (eye at (32, 71.9, 71.9) looking at the centre): one cell
/// right in front of the eye, at view depth ~12, and one at the far side, at view depth ~102.
/// Both lie in node 9 and node 3 of `octant_nodes`.
const NEAR_POINT: [f64; 3] = [33.0, 63.5, 63.5];
const FAR_POINT: [f64; 3] = [33.0, 0.2, 0.2];
const NEAR_NODE: u64 = 9;
const FAR_NODE: u64 = 3;
/// Pool target: depths 4 to 7 change with the occupied depth (6 at ~12, 5 at ~102).
const BOUND_TARGET: f64 = 32.0;

/// `NodeDepths::hi_d` of `id` as the pure functions give it at `view`: the least over the
/// resident levels of `max(bound of the level, level depth)`, not below `lo`.
fn bound_from_pages(rig: &Rig, id: NodeId, view: &ViewProjection) -> Option<u8> {
    let node = &rig.nodes[&id];
    let (probes, _) = masked_probes(
        view,
        &grid(),
        ORIGIN,
        node.bounds,
        &std::array::from_fn(|o| node.probe_box(o)),
    );
    let region = probe_region(node.bounds, &probes);
    let lo = rig.pool.depths[&id].far.0;
    (node.buildable.0..=node.buildable.1)
        .filter_map(|depth| {
            let level = rig.pool.level(rig.pool.key(id, depth))?;
            let occupied =
                crate::nearest_occupied_depth(view, &grid(), ORIGIN, depth, &level.pages)?;
            let (_, hi) = crate::cells::cut::node_view_range_at(
                view,
                &grid(),
                ORIGIN,
                region,
                node.buildable,
                BOUND_TARGET / 2.0,
                rig.pool.min_side(),
                Some(occupied),
            )?;
            Some(hi.max(depth))
        })
        .min()
        .map(|bound| bound.max(lo))
}

/// A settled rig at the zoom camera whose node `id` holds one cell at `at` in every level.
fn bound_rig(id: u64, at: [f64; 3]) -> Rig {
    let mut rig = Rig::new(octant_nodes(), 256, 64, 0);
    rig.run(40, &ids(&CHILDREN), &camera("zoom"), BOUND_TARGET);
    put_pages_at(&mut rig, NodeId::new(id), at);
    // A new entry is computed from the pages as they are now.
    rig.pool.depths.remove(&NodeId::new(id));
    rig.step(&ids(&CHILDREN), &camera("zoom"), BOUND_TARGET);
    rig
}

#[test]
fn the_data_bound_is_the_least_over_the_resident_levels_of_hi_at_their_nearest_page() {
    let (near, far) = (NodeId::new(NEAR_NODE), NodeId::new(FAR_NODE));
    let zoom = view(&camera("zoom"));
    let rig = bound_rig(NEAR_NODE, NEAR_POINT);
    let entry = &rig.pool.depths[&near];
    let bound = rig.pool.data_bound(near).expect("a page in view");
    assert_eq!(Some(bound), bound_from_pages(&rig, near, &zoom));
    assert!(entry.far.0 <= bound && bound <= rig.nodes[&near].buildable.1);
    let far_rig = bound_rig(FAR_NODE, FAR_POINT);
    let far_bound = far_rig.pool.data_bound(far).expect("a page in view");
    assert_eq!(Some(far_bound), bound_from_pages(&far_rig, far, &zoom));
    // The nearer the occupied cell, the finer the bound.
    assert!(far_bound < bound, "{far_bound} against {bound}");
}

#[test]
fn the_data_bound_does_not_depend_on_the_levels_over_it() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let id = NodeId::new(NEAR_NODE);
    let mut rig = bound_rig(NEAR_NODE, FAR_POINT);
    let bound = rig.pool.data_bound(id).expect("bound");
    let top = rig.nodes[&id].buildable.1;
    assert!(bound < top, "setup: {bound} against {top}");
    // The levels over the bound leave: it does not change (they could not hold the least).
    let evaluations = rig.pool.bound_evaluations;
    for depth in (bound + 1)..=top {
        let (key, frame) = (rig.pool.key(id, depth), rig.pool.frame);
        rig.pool.evict_resident(key, frame);
    }
    rig.pool.uncapped_wish = false;
    rig.step(&cut, &zoom, BOUND_TARGET);
    assert!(
        rig.pool.bound_evaluations > evaluations,
        "setup: looked at again"
    );
    assert_eq!(rig.pool.data_bound(id), Some(bound));
    // The whole resident set leaves and comes back in the pool's own order, with the same pages:
    // the same bound, wherever the order of arrival went.
    let keys: Vec<LevelKey> = (rig.nodes[&id].buildable.0..=bound)
        .map(|depth| rig.pool.key(id, depth))
        .collect();
    for key in &keys {
        let frame = rig.pool.frame;
        rig.pool.evict_resident(*key, frame);
    }
    rig.step(&cut, &zoom, BOUND_TARGET);
    rig.run(8, &cut, &zoom, BOUND_TARGET);
    put_pages_at(&mut rig, id, FAR_POINT);
    rig.pool.residency_epoch += 1;
    rig.step(&cut, &zoom, BOUND_TARGET);
    assert_eq!(rig.pool.data_bound(id), Some(bound));
}

#[test]
fn a_level_arriving_inside_the_bound_can_lower_it_and_the_wishes_follow() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let id = NodeId::new(NEAR_NODE);
    let mut rig = bound_rig(NEAR_NODE, NEAR_POINT);
    rig.pool.uncapped_wish = false;
    let before = rig.pool.data_bound(id).expect("bound");
    let coarse = rig.nodes[&id].buildable.0;
    assert!(coarse < before, "setup: {coarse} against {before}");
    // The coarsest level now has its data far from the eye: it alone gives a lower bound.
    let key = rig.pool.key(id, coarse);
    let level = rig.pool.levels.get_mut(&key).expect("level");
    let mut packed = (**level.data.as_ref().expect("data")).clone();
    for page in &mut packed.pages {
        page.origin = FAR_POINT.map(|v| (v / (SIDE / 2.0_f64.powi(i32::from(coarse)))) as u64);
    }
    level.data = Some(Arc::new(packed));
    rig.pool.residency_epoch += 1;
    let mut plan = rig.step(&cut, &zoom, BOUND_TARGET);
    let after = rig.pool.data_bound(id).expect("bound");
    assert!(after < before, "setup: {after} against {before}");
    assert_eq!(
        after,
        bound_from_pages(&rig, id, &view(&zoom)).expect("pages")
    );
    assert!(
        !plan.counters.wanted_reused,
        "the lowered bound redoes the wanted cut"
    );
    let mut wish = None;
    for _ in 0..4 {
        wish = wish.max(finest_wish_and_request(&plan, id).0);
        plan = rig.step(&cut, &zoom, BOUND_TARGET);
    }
    assert_eq!(wish, Some(after), "the wish did not follow the bound");
}

#[test]
fn a_resident_level_with_no_page_in_view_bounds_the_node_at_its_own_depth() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let id = NodeId::new(NEAR_NODE);
    let (first, last) = (4_u8, 7_u8);
    // Run with the levels `resident` only, the ones in `unseen` holding their pages far outside
    // the view; return the bound and the finest wish of the node.
    let run = |resident: &[u8], unseen: &[u8]| {
        let mut rig = bound_rig(NEAR_NODE, FAR_POINT);
        rig.pool.uncapped_wish = false;
        for depth in first..=last {
            let (key, frame) = (rig.pool.key(id, depth), rig.pool.frame);
            if !resident.contains(&depth) {
                rig.pool.evict_resident(key, frame);
            } else if unseen.contains(&depth) {
                let side = SIDE / 2.0_f64.powi(i32::from(depth));
                let level = rig.pool.levels.get_mut(&key).expect("level");
                let mut packed = (**level.data.as_ref().expect("data")).clone();
                for page in &mut packed.pages {
                    page.origin = [1.0e9; 3].map(|v| (v / side) as u64);
                }
                level.data = Some(Arc::new(packed));
            }
        }
        rig.pool.residency_epoch += 1;
        let plan = rig.step(&cut, &zoom, BOUND_TARGET);
        let lo = rig.pool.depths[&id].far.0;
        (
            rig.pool.data_bound(id).expect("a resident level bounds"),
            lo,
            finest_wish_and_request(&plan, id).0,
        )
    };
    // {4, 5} with only level 4 in view, against {5, 6} with none: the coarsest level with no box
    // in view gives the bound whichever coarser level is also resident.
    let (with_view, lo, wish) = run(&[4, 5], &[5]);
    let (without_view, _, _) = run(&[5, 6], &[5, 6]);
    assert_eq!(
        with_view, without_view,
        "the bound follows the resident set"
    );
    assert_eq!(with_view, lo.max(5));
    assert!(
        wish.is_some() && wish <= Some(with_view),
        "wish {wish:?}, bound {with_view}"
    );
}

#[test]
fn a_still_camera_at_rest_computes_no_data_bound_and_a_moved_one_does() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let mut rig = bound_rig(NEAR_NODE, NEAR_POINT);
    rig.run(6, &cut, &zoom, BOUND_TARGET);
    let settled = rig.pool.bound_evaluations;
    assert!(settled > 0, "the bound was computed while settling");
    rig.run(8, &cut, &zoom, BOUND_TARGET);
    assert_eq!(rig.pool.bound_evaluations, settled, "no work at rest");
    rig.step(&cut, &camera("fit"), BOUND_TARGET);
    assert!(
        rig.pool.bound_evaluations > settled,
        "a pose change computes it again"
    );
}

#[test]
fn the_cached_depths_look_at_the_data_bound_again_when_a_resident_level_changes() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let id = NodeId::new(NEAR_NODE);
    let mut rig = bound_rig(NEAR_NODE, NEAR_POINT);
    let refreshes = |rig: &mut Rig| rig.step(&cut, &zoom, BOUND_TARGET).counters.depth_refreshes;
    // Nothing changes at a fixed pose: no refresh, no work.
    rig.run(4, &cut, &zoom, BOUND_TARGET);
    let evaluations = rig.pool.bound_evaluations;
    assert_eq!(refreshes(&mut rig), 0);
    assert_eq!(rig.pool.bound_evaluations, evaluations);
    // A level that leaves the pool, whichever it is, makes the bound be looked at again without
    // redoing the entry.
    let key = rig.pool.key(id, rig.nodes[&id].buildable.0);
    let frame = rig.pool.frame;
    rig.pool.evict_resident(key, frame);
    assert_eq!(refreshes(&mut rig), 0);
    assert!(rig.pool.bound_evaluations > evaluations);
}

#[test]
fn the_active_set_has_no_gap_between_a_drawn_depth_and_the_range() {
    // (range, drawn, resident, expected active depths).
    type Case = ((u8, u8), u8, &'static [u8], &'static [u8]);
    let cases: [Case; 4] = [
        // Drawn coarser than the range: depth 11 is resident, usable and inside the range.
        ((12, 12), 10, &[2, 7, 10, 11, 12], &[10, 11, 12]),
        // Drawn finer than the range.
        ((8, 9), 11, &[2, 8, 9, 10, 11], &[8, 9, 10, 11]),
        // A residency gap between the drawn depth and the range: the run stops at it.
        ((12, 12), 10, &[2, 10, 12], &[10]),
        // A drawn depth far from the range end (a hop of several levels): the run reaches it.
        (
            (12, 12),
            7,
            &[2, 7, 8, 9, 10, 11, 12],
            &[7, 8, 9, 10, 11, 12],
        ),
    ];
    for (n, (range, drawn, resident, expected)) in cases.into_iter().enumerate() {
        let mut nodes = octant_nodes();
        nodes.insert(
            NodeId::new(1),
            d5_node(SIDE, &(2..=12).collect::<Vec<u8>>()),
        );
        let mut rig = d5_rig(nodes);
        seed_resident(&mut rig.pool, 1, resident);
        let entry = CellCutEntry {
            node: NodeId::new(1),
            wanted: [Some(7); 8],
            drawn: [Some(drawn); 8],
            stage: [false; 8],
            step: [None; 8],
            range: Some(range),
            node_anchor: None,
            node_target: None,
            node_run: None,
            node_stage: false,
            node_step: None,
            mask: [0; 8],
        };
        let node_depth = CellPool::node_depth(&grid(), rig.nodes[&NodeId::new(1)].bounds);
        let active = rig.pool.cell_active_depths(&entry, node_depth, None);
        assert_eq!(active, expected, "case {n}");
    }
}

// ---- The wish is capped at the data bound (ADR 0055 plan, task 7a) --------------------------

/// The finest depth any octant of `id` wishes in `plan`, and the finest depth requested for it.
fn finest_wish_and_request(plan: &FramePlan, id: NodeId) -> (Option<u8>, Option<u8>) {
    let wish = plan
        .entries
        .iter()
        .filter(|entry| entry.node == id)
        .flat_map(|entry| entry.wanted.iter().flatten().copied())
        .max();
    let request = plan
        .fetch_requests
        .iter()
        .filter(|key| key.node == id)
        .map(|key| key.depth)
        .max();
    (wish, request)
}

#[test]
fn a_node_wishes_and_requests_no_depth_finer_than_its_data_bound() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let id = NodeId::new(NEAR_NODE);
    // The eye node (its octants cross the eye plane and wish the finest depth) holds data only
    // on the far side: its bound is 5 and it wishes 7.
    let mut rig = bound_rig(NEAR_NODE, FAR_POINT);
    rig.pool.uncapped_wish = false;
    let bound = rig.pool.data_bound(id).expect("a page in view");
    let top = rig.nodes[&id].buildable.1;
    assert!(
        bound < top,
        "setup: the bound {bound} must leave room below {top}"
    );
    // The levels finer than the bound go away; the pool must not fetch them again.
    let finer: Vec<LevelKey> = ((bound + 1)..=top).map(|d| rig.pool.key(id, d)).collect();
    for key in finer {
        let frame = rig.pool.frame;
        rig.pool.evict_resident(key, frame);
    }
    let (mut wish, mut request) = (None, None);
    for _ in 0..12 {
        let plan = rig.step(&cut, &zoom, BOUND_TARGET);
        let (w, r) = finest_wish_and_request(&plan, id);
        wish = wish.max(w);
        request = request.max(r);
    }
    assert!(
        wish.is_some() && wish <= Some(bound),
        "wish {wish:?}, bound {bound}"
    );
    assert!(request <= Some(bound), "request {request:?}, bound {bound}");
}

#[test]
fn at_a_fixed_pose_no_level_is_requested_twice_or_cancelled_more_than_twice() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let mut rig = Rig::new(octant_nodes(), 256, 64, 2);
    rig.pool.uncapped_wish = false;
    let (mut requested, mut cancelled): (HashMap<LevelKey, u32>, HashMap<LevelKey, u32>) =
        Default::default();
    for _ in 0..200 {
        let plan = rig.step(&cut, &zoom, BOUND_TARGET);
        for key in &plan.fetch_requests {
            *requested.entry(*key).or_default() += 1;
        }
        for event in &plan.events {
            if let CellPoolEvent::Cancel(key) = event {
                *cancelled.entry(*key).or_default() += 1;
            }
        }
    }
    assert!(!requested.is_empty(), "setup: levels were requested");
    let repeated: Vec<_> = requested.iter().filter(|(_, n)| **n > 1).collect();
    assert!(repeated.is_empty(), "requested twice: {repeated:?}");
    let cancels: Vec<_> = cancelled.iter().filter(|(_, n)| **n > 2).collect();
    assert!(cancels.is_empty(), "cancelled more than twice: {cancels:?}");
}

#[test]
fn run_covers_compares_the_run_end_with_the_finest_drawn_before_capped_by_the_wish() {
    // Zoom in: one ancestor run `[5, 6]`; the child takes over once its run reaches 6, not 7.
    let ancestor = [(5, 6)];
    assert!(!run_covers(Some((4, 5)), &ancestor, 8));
    assert!(run_covers(Some((4, 6)), &ancestor, 8));
    assert!(run_covers(Some((6, 6)), &ancestor, 8));
    assert!(run_covers(Some((4, 9)), &ancestor, 8));
    // The wish bounds the threshold, also when the ancestor drew finer than the child can build.
    assert!(run_covers(Some((4, 5)), &ancestor, 5));
    assert!(!run_covers(Some((4, 4)), &ancestor, 5));
    assert!(run_covers(Some((4, 5)), &[(5, 9)], 5));
    // Zoom out: the finest of the descendants counts, whichever it is.
    let descendants = [(3, 5), (6, 8), (4, 4)];
    assert!(!run_covers(Some((4, 7)), &descendants, 9));
    assert!(run_covers(Some((4, 8)), &descendants, 9));
    assert!(run_covers(Some((4, 7)), &descendants, 7));
    // Nothing drawn before, or nothing of the node to draw: not held by this rule.
    assert!(run_covers(Some((4, 4)), &[], 9));
    assert!(run_covers(None, &ancestor, 9));
}

/// Where the pool of the cap rows is small enough for the fit cap to stay (`fit` lifts the caps
/// when the uncapped cut fits under the low mark).
const CAP_POOL: u32 = 512;
const CAP_POINTS: u64 = 3_000_000;

/// What one frame of a fit-cap row saw.
#[derive(Debug)]
struct CapRow {
    /// The frame (from 0) on which the node came out of the hold, if it did.
    released: Option<usize>,
    /// The wish the oracle is run up to, shown in a failure.
    wish: u8,
    /// The runs `eval_hold` builds for the node (range `Wide` and `Near`).
    gate: [Option<Run>; 2],
    gate_releases: [bool; 2],
    emitted: Vec<u8>,
    cap_stayed: bool,
    oracle: Result<(), String>,
}

/// One zoom (`zoom_in`: child 10 over node 2; else node 2 over its children 10..=17) with a fit
/// cap on the entering node, `resident` levels of it (never delivered more), and, when `injected`,
/// the applied cut pretending the node already drew depth 7 (a level above the cap).
/// Each frame is replayed from scratch so the gate is read on the very state `plan` sees.
fn cap_row(
    zoom_in: bool,
    (cap, target, before_target, resident, injected): (u8, f64, f64, &[u8], bool),
    frames: usize,
) -> CapRow {
    let node = NodeId::new(if zoom_in { 10 } else { 2 });
    let cut = if zoom_in {
        let mut cut = ids(&region_children(10));
        cut.push(NodeId::new(3));
        cut
    } else {
        ids(&[2])
    };
    let setup = || {
        let (mut rig, cam) = if zoom_in {
            let rig = Rig::new(two_regions_with_coarse_children(), CAP_POOL, 3, 1);
            (rig, camera("fit"))
        } else {
            let rig = Rig::new(two_regions(), CAP_POOL, 3, 1);
            (rig, camera("fit"))
        };
        let last = if zoom_in {
            rig.run(60, &ids(&[2, 3]), &cam, before_target)
        } else {
            rig.run(60, &ids(&region_children(10)), &cam, before_target)
        };
        set_resident(&mut rig, node.get(), resident);
        for data in rig.nodes.values_mut() {
            data.points = CAP_POINTS;
        }
        rig.held.insert(node);
        rig.pool.caps.insert(node, cap);
        if injected {
            rig.pool.applied_drawn.insert(node, [Some(7); 8]);
        }
        (rig, cam, last)
    };
    let step = |rig: &mut Rig, cam: &OrbitCamera| d5_step_at(rig, &cut, cam, target / 2.0);
    // The wish of the node on this view, from a first frame of a throwaway rig.
    let wish = {
        let (mut rig, cam, _) = setup();
        let plan = step(&mut rig, &cam);
        plan.entries
            .iter()
            .find(|e| e.node == node)
            .and_then(|e| e.wanted.iter().flatten().max().copied())
            .unwrap_or(0)
    };
    let mut found = None;
    for frame in 0..frames {
        let (mut rig, cam, last) = setup();
        for _ in 0..frame {
            step(&mut rig, &cam);
        }
        let probes =
            [Far::Wide, Far::Near].map(|far| refine_probe(&mut rig, node.get(), wish, far));
        let owner_depth = probes[0].node_depth;
        let befores: Vec<Owner> = if zoom_in {
            vec![Owner {
                node_depth: owner_depth - 1,
                lo: [0.0, 0.0, 10.0],
                active: active_depths(&last, &rig, 2),
            }]
        } else {
            let top = Owner {
                node_depth: owner_depth,
                lo: [0.0, 0.0, 10.0],
                active: Vec::new(),
            };
            (10..18)
                .map(|child| Owner {
                    node_depth: owner_depth + 1,
                    lo: top.child_lo(child - 10),
                    active: active_depths(&last, &rig, child as u64),
                })
                .collect()
        };
        let plan = step(&mut rig, &cam);
        if depth_drawn(&plan, node).is_empty() {
            continue;
        }
        let emitted = active_depths(&plan, &rig, node.get());
        let lo = if zoom_in {
            befores[0].child_lo(0)
        } else {
            [0.0, 0.0, 10.0]
        };
        let after = Owner {
            node_depth: owner_depth,
            lo,
            active: emitted.clone(),
        };
        // Zoom in: the cells of the entering node; zoom out: those of each replaced descendant.
        let samples: Vec<_> = befores
            .iter()
            .flat_map(|before| {
                let at = if zoom_in { &after } else { before };
                (0..8).map(|o| (before.clone(), at.sub_centre(o)))
            })
            .collect();
        let gate = probes.each_ref().map(Probe::run);
        found = Some(CapRow {
            released: Some(frame),
            wish,
            gate_releases: gate.map(|run| {
                let before: Vec<Run> = befores
                    .iter()
                    .filter_map(|b| b.active.first().zip(b.active.last()).map(|(l, h)| (*l, *h)))
                    .collect();
                run_covers(run, &before, wish)
            }),
            gate,
            emitted,
            cap_stayed: rig.pool.caps.get(&node) == Some(&cap),
            oracle: twin_releases(&after, wish, &samples),
        });
        break;
    }
    found.unwrap_or(CapRow {
        released: None,
        wish,
        gate: [None; 2],
        gate_releases: [false; 2],
        emitted: Vec::new(),
        cap_stayed: true,
        oracle: Ok(()),
    })
}

/// R3-Q21 under a fit cap: when `eval_hold` releases a node whose depths are capped, the active set
/// `plan` really emits for it (`finer_drawn` may replace its resident levels with finer ones
/// above the cap) draws no sampled cell coarser than what drew there before, judged by the GPU
/// twin, and reaches at least the finest depth of the run the gate assumed.
#[test]
fn a_capped_node_the_gate_releases_draws_nothing_coarser_than_before_with_the_set_plan_emits() {
    let (mut rows, mut zoom_in_rows, mut zoom_out_rows, mut above_cap) = (0, 0, 0, 0);
    let mut failures = Vec::new();
    for zoom_in in [true, false] {
        // `(target now, target before)`: what the node wishes and what drew before.
        for (target, before_target) in [(4.0, 8.0), (4.0, 16.0)] {
            for cap in [4u8, 5, 6] {
                for resident in [&[4u8, 5, 6, 7][..], &[4, 7], &[5, 7], &[4, 5]] {
                    for injected in [false, true] {
                        let case = (cap, target, before_target, resident, injected);
                        let row = cap_row(zoom_in, case, 2);
                        let name = format!("zoom_in {zoom_in} {case:?} wish {}: {row:?}", row.wish);
                        // `fit` lifts a cap the uncapped cut fits under: not a capped row.
                        if !row.cap_stayed || row.released.is_none() {
                            continue;
                        }
                        rows += 1;
                        zoom_in_rows += usize::from(zoom_in);
                        zoom_out_rows += usize::from(!zoom_in);
                        above_cap += usize::from(row.emitted.last().is_some_and(|d| *d > cap));
                        let finest = row.emitted.last().copied();
                        let lower = row
                            .gate
                            .iter()
                            .flatten()
                            .any(|(_, hi)| finest.is_none_or(|f| f < *hi));
                        if row.oracle.is_err() || lower || row.gate_releases.contains(&false) {
                            failures.push(name);
                        }
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} rows:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(
        zoom_in_rows > 0 && zoom_out_rows > 0,
        "{zoom_in_rows} {zoom_out_rows}"
    );
    assert!(above_cap > 0, "no row drew a level above its cap");
    assert!(rows > 30, "{rows} rows");
}

/// Held with no escape (characterization). A wish that the node can never reach does not exist:
/// `cell_cut` clamps it to the node's buildable range (stored depths and fit cap), so a node
/// whose levels stop short of what the ancestor drew is released once it reaches its own finest
/// level (`min(what drew before, wish)`), by `eval_hold` and by the per-octant `eval_refine_hold`
/// alike. The only long hold is a level that is stored and buildable but not resident (never
/// delivered, e.g. refused): both judges hold it until it arrives.
#[test]
fn a_node_whose_wish_is_clamped_below_the_ancestor_is_released_and_a_missing_level_holds() {
    // `(stored, resident, ancestor finest, wish, held)` of child 10 in a zoom in.
    type Never = (&'static [u8], &'static [u8], u8, u8, bool);
    let rows: [Never; 3] = [
        // Stored up to 5, the ancestor drew 6: the wish clamps to 5, which is resident.
        (&[4, 5], &[4, 5], 6, 7, false),
        (&[4, 5], &[5], 6, 5, false),
        // The level the ancestor drew is stored and buildable but not resident: it must arrive.
        (&[4, 5, 6, 7], &[5], 6, 7, true),
    ];
    for (index, (stored, resident, finest, wish, held)) in rows.iter().enumerate() {
        for far in FARS {
            let mut rig = Rig::new(two_regions_with_coarse_children(), 512, 3, 1);
            rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
            set_applied(&mut rig, 2, *finest);
            {
                let node = rig.nodes.get_mut(&NodeId::new(10)).expect("node 10");
                node.stored = stored.to_vec();
                node.buildable = (stored[0], *stored.last().expect("stored"));
            }
            set_resident(&mut rig, 10, resident);
            let probe = refine_probe(&mut rig, 10, *wish, far);
            let case = format!("row {index} {far:?}: {probe:?}");
            assert_eq!(probe.held, *held, "{case}");
            assert_eq!(probe.old_held, *held, "{case}");
        }
    }
}

/// A node with data in octant 1 straight ahead and in octant 2 just off-screen to the side, seen
/// from `EYE` along +x, then from a pose turned by `YAW`: octant 2 enters the view at `YAW`.
const EYE: [f64; 3] = [-1.0, 32.0, 32.0];
/// Below the drift budget of the node (`ROTATION_SHARE / tan_corner` of the rotation).
const YAW: f64 = 0.01;

fn looking(yaw: f64) -> ViewProjection {
    let mut camera = camera("zoom");
    let at = [EYE[0] + 10.0 * yaw.cos(), EYE[1] + 10.0 * yaw.sin(), EYE[2]];
    assert!(camera.set_pose(EYE, at));
    view(&camera)
}

#[test]
fn a_residency_change_measures_the_octants_in_view_at_the_current_pose() {
    let (id, cut) = (NodeId::new(2), ids(&[2]));
    let at = |lo: [f64; 3], hi: [f64; 3]| {
        Some(
            SpatialBounds::new(
                std::array::from_fn(|a| ORIGIN[a] + lo[a]),
                std::array::from_fn(|a| ORIGIN[a] + hi[a]),
            )
            .unwrap(),
        )
    };
    let mut data = [None; 8];
    data[1] = at([40.0, 28.0, 28.0], [60.0, 31.0, 31.0]);
    // Its far depth sits within 0.15 of the view depth that decides "in view" at both poses.
    data[2] = at([2.0, 50.0, 28.0], [13.9, 60.0, 32.0]);
    let mut nodes = octant_nodes();
    let node = nodes.get_mut(&id).unwrap();
    node.bounds = cube(ORIGIN, SIDE);
    node.octant_data_bounds = Some(data);
    let mut rig = Rig::new(nodes, 256, 64, 0);
    let (before, after) = (looking(0.0), looking(YAW));
    let behind = |view: &ViewProjection| {
        let node = &rig.nodes[&id];
        masked_probes(
            view,
            &grid(),
            ORIGIN,
            node.bounds,
            &std::array::from_fn(|o| node.probe_box(o)),
        )
        .1
        .0
    };
    assert_eq!(
        behind(&before) >> 2 & 1,
        1,
        "setup: octant 2 is out of view"
    );
    assert_eq!(
        behind(&after) >> 2 & 1,
        0,
        "setup: octant 2 comes into view"
    );
    for _ in 0..40 {
        rig.step_view(&cut, &before, BOUND_TARGET);
    }
    // One cell right by the eye: the bound depends on the region only through what is in view.
    put_pages_at(&mut rig, id, [3.0, 32.0, 33.0]);
    // A level leaves: a residency change at the turned pose, which the node is not yet due for.
    let key = *rig
        .pool
        .levels
        .iter()
        .filter(|(key, level)| key.node == id && level.data.is_some())
        .map(|(key, _)| key)
        .max_by_key(|key| key.depth)
        .expect("a resident level");
    let frame = rig.pool.frame;
    rig.pool.evict_resident(key, frame);
    let pose = rig.pool.depths[&id].pose;
    rig.step_view(&cut, &after, BOUND_TARGET);
    assert_eq!(
        rig.pool.depths[&id].pose, pose,
        "setup: the node was not redone"
    );
    assert_eq!(
        rig.pool.data_bound(id),
        bound_from_pages(&rig, id, &after),
        "the bound of the pose it is evaluated at"
    );
}

/// Plans `frames` frames of the root of `hold_rig` and checks the `node_*` fields of each against
/// their definition. Returns the frames that carried a step and the last entry.
fn plan_and_check_runs(rig: &mut Rig, frames: usize) -> (usize, CellCutEntry) {
    let zoom = camera("zoom");
    let root = NodeId::new(1);
    let mut steps = 0;
    let mut last = None;
    for frame in 0..frames {
        let plan = d5_step_at(rig, &ids(&[1]), &zoom, 2.0);
        let entry = plan.entries.iter().find(|e| e.node == root).expect("root");
        let (anchor, run, target) = (
            entry.node_anchor.expect("an anchor"),
            entry.node_run.expect("a run"),
            entry.node_target.expect("a target"),
        );
        assert!((run.0..=run.1).contains(&anchor));
        assert!(run.1 <= target.max(anchor));
        let wish = entry.wanted.iter().flatten().min().copied();
        assert_eq!(
            entry.node_stage,
            run.1 < target || wish.is_some_and(|wish| wish < run.0),
            "frame {frame}: {entry:?}"
        );
        assert_eq!(
            entry.node_stage,
            entry.node_step.is_some(),
            "frame {frame}: a staged node asks for a level"
        );
        if let Some(step) = entry.node_step {
            steps += 1;
            assert!(step == run.1 + 1 || step + 1 == run.0, "frame {frame}");
        }
        last = Some(entry.clone());
    }
    (steps, last.expect("a frame was planned"))
}

#[test]
fn the_planned_cut_carries_the_node_run_and_the_pool_keeps_the_applied_run() {
    // Plumbing of plan task 10b: `fill_node_runs` fills the `node_*` fields of the planned
    // entries and `plan` stores the applied run (without D5) that the next frame reads as `prev`.
    let mut rig = hold_rig();
    let root = NodeId::new(1);
    let (_, entry) = plan_and_check_runs(&mut rig, 9);
    let applied = rig.pool.applied_run_of(root).expect("the root was applied");
    let own = rig.pool.cell_active_depths(
        &entry,
        CellPool::node_depth(&grid(), rig.nodes[&root].bounds),
        None,
    );
    assert_eq!(Some(applied), own.first().copied().zip(own.last().copied()));
}

#[test]
fn a_planned_node_waiting_for_a_level_is_staged_and_names_it() {
    // The same plumbing with a residency gap: nothing above depth 8 is resident, so the run
    // stops short of its target and the node asks for the level that extends it.
    let mut rig = hold_rig();
    for depth in 9..=14 {
        let key = rig.pool.key(NodeId::new(1), depth);
        rig.pool.levels.remove(&key);
    }
    rig.pool.residency_epoch += 1;
    let (steps, _) = plan_and_check_runs(&mut rig, 3);
    assert!(steps > 0, "a run short of its target has a step");
}

#[test]
fn a_node_entering_with_no_before_takes_its_prev_from_the_history() {
    // Task 10b, C1: the hold's `before` is empty when its cover rule finds no covering cell of
    // the cut (`Cover::Nothing`), yet today's per-octant machinery starts the entering node from
    // the depth its applied ancestor drew. `prev` follows that history and is never `None` when
    // there is one.
    let mut rig = hold_rig();
    let zoom = camera("zoom");
    for _ in 0..9 {
        d5_step_at(&mut rig, &ids(&[1]), &zoom, 2.0);
    }
    let root = NodeId::new(1);
    let child = NodeId::new(2);
    let finest = rig.pool.applied_finest[&root];
    let node = &rig.nodes[&child];
    assert!(
        !rig.pool.applied_drawn.contains_key(&child) && !rig.pool.applied_run.contains_key(&child),
        "setup: the child did not draw"
    );
    let empty = FastMap::default();
    assert_eq!(
        rig.pool.node_prev(child, node, None, &empty, &grid()),
        Some(finest),
        "the ancestor's finest drawn depth, from the entering seed"
    );
    // A `before` of the hold wins over the seed.
    let mut before = FastMap::default();
    before.insert(child, 9);
    assert_eq!(
        rig.pool.node_prev(child, node, None, &before, &grid()),
        Some(9)
    );
}
