//! Task 7a pass criteria: residency is all-or-nothing, uploads are spread over frames, the fit
//! coarsens the farthest nodes first and settles, and no page is rewritten while it can be read.

use std::collections::{HashMap, HashSet};

use viewer_core::NodeId;

use super::*;
use crate::{
    OrbitCamera,
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
    frame: u64,
}

impl Rig {
    fn new(
        nodes: HashMap<NodeId, PoolNode>,
        capacity_pages: u32,
        budget_pages: u64,
        delay: u64,
    ) -> Self {
        Self {
            pool: CellPool::new(PoolConfig {
                capacity_pages,
                upload_budget_bytes: budget_pages * POOL_PAGE_BYTES,
                ..PoolConfig::default()
            }),
            gpu: FakeGpu::default(),
            nodes,
            delay,
            shrink: false,
            deep: None,
            inflight: Vec::new(),
            held: HashSet::new(),
            frame: 0,
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
        self.frame += 1;
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.inflight)
            .into_iter()
            .partition(|(at, _)| *at <= self.frame);
        self.inflight = later;
        for (at, key) in due {
            if self.held.contains(&key.node) {
                self.inflight.push((at, key));
                continue;
            }
            let pages = self.pages(key);
            self.pool.offer(key, fake_level(key, pages));
        }
        let nodes = &self.nodes;
        let plan = self
            .pool
            .frame(
                cut,
                &|id| nodes.get(&id).cloned(),
                &view(camera),
                &grid(),
                ORIGIN,
                target,
            )
            .expect("frame");
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
    for frame in 0..80 {
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
    for _ in 0..60 {
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
            points: 2000,
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
    // Far away the cut draws coarse levels, so the fine one can go.
    rig.run(3, &cut, &cam, 64.0);
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
    // Node 2 at 4 px: depth 7 (4 pages) + depth 4 (1 page) in 8 pages; reuse needs the delay.
    let mut rig = Rig::new(octant_nodes(), 8, 8, 0);
    let cut = ids(&[2]);
    let cam = camera("zoom");
    let plan = rig.run(8, &cut, &cam, 4.0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    let key = rig.pool.key(NodeId::new(2), 7);
    assert!(plan.active.iter().any(|a| a.key == key));
    let slots: Vec<u32> = (0..4).map(|p| rig.gpu.slot_of[&(key, p)]).collect();
    // The level draws until frame `last`; far away it is unused and may go.
    let last = rig.frame;
    rig.step(&cut, &cam, 64.0);
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
                points: 2000,
            },
        ),
        (
            NodeId::new(20),
            PoolNode {
                bounds: cube(ORIGIN, SIDE / f64::from(1_u32 << (MAX_MASK_LEVELS + 1))),
                stored: vec![7, 8],
                points: 2000,
            },
        ),
        (
            NodeId::new(21),
            PoolNode {
                bounds: cube(ORIGIN, SIDE / f64::from(1_u32 << MAX_MASK_LEVELS)),
                stored: vec![6, 7],
                points: 2000,
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
        .map(|(slot, _)| *slot)
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
    rig.run(12, &cut, &zoom, 4.0);
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
    rig.run(8, &cut[..7], &zoom, 64.0);
    let key = rig.pool.key(NodeId::new(2), 7);
    assert!(rig.pool.evict(key));
    let plan = rig.step(&cut[..7], &zoom, 64.0);
    assert!(!plan.counters.real_reused);

    // A camera that stops is exact: it refreshes once, then everything is reused again.
    let moved = camera("fit");
    rig.step(&cut[..7], &moved, 64.0);
    let still = rig.run(3, &cut[..7], &moved, 64.0);
    assert!(still.counters.wanted_reused && still.counters.real_reused);
    assert!(still.counters.cut_evaluations <= 1);
}

fn full_depths(cam: &OrbitCamera, node: &PoolNode, target: f64) -> [u8; 8] {
    std::array::from_fn(|o| {
        octant_wanted_depth(
            &view(cam),
            &grid(),
            ORIGIN,
            node.bounds,
            o,
            &node.stored,
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
        cam.orbit(0.004, 0.002);
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
    for _ in 0..4000 {
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
        let hint = stored[(next(&mut state) * stored.len() as f64) as usize % stored.len()];
        let v = view(&cam);
        let full =
            octant_wanted_depth(&v, &grid(), ORIGIN, node.bounds, octant, &stored, target_px);
        let near = crate::cell_cut::octant_wanted_depth_near(
            &v,
            &grid(),
            ORIGIN,
            node.bounds,
            octant,
            &stored,
            target_px,
            hint,
        );
        assert_eq!(
            near, full,
            "eye {eye:?} target {target:?} stored {stored:?} hint {hint}"
        );
        checked += 1;
    }
    assert!(checked > 2000);
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
            4.0,
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
            points: 2000,
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
fn a_far_node_that_draws_nothing_is_not_starved_by_refinements_elsewhere() {
    // Node 8 draws its depth 4 and wants depth 5; node 2 (farther) draws nothing and stores
    // depth 6 and deeper. By absolute depth alone the refinement of node 8 would go first.
    let mut nodes = octant_nodes();
    nodes.get_mut(&NodeId::new(8)).unwrap().stored = vec![4, 5];
    nodes.get_mut(&NodeId::new(2)).unwrap().stored = vec![6, 7];
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
/// before the step: each drawn depth is checked against it and `resident_depth`, and no point may
/// be drawn by two entries.
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
                    let wanted = entry.wanted[octant].expect("a drawn octant is wanted");
                    assert_eq!(
                        crate::resident_depth(wanted, &resident[&entry.node]).map(|(d, _)| d),
                        Some(*depth),
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
    // now and redraws coarser, so the children are not held behind it.
    takes_over_at(512, 16.0, false, 15);
}

#[test]
fn a_child_whose_fit_cap_is_below_the_ancestor_depth_takes_over_at_the_cap() {
    // The pool cannot hold depth 6 for the farthest children: their cap (5) is the threshold,
    // and the nearest ones still want 6.
    takes_over_at(40, 8.0, true, 30);
}

#[test]
fn the_hold_follows_what_would_be_drawn_and_not_mere_residency() {
    // Node 10 wants 6 (the depth node 2 drew). With 5 and 7 resident the closest level below 6
    // is 5, so it is held although 7 is resident.
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    let cam = camera("fit");
    let before = rig.run(60, &ids(&[2, 3]), &cam, 8.0);
    assert_eq!(depth_of(&before, 2), [Some(6); 8], "setup");
    let mut cut = ids(&region_children(10));
    cut.push(NodeId::new(3));
    rig.held.insert(NodeId::new(10));
    seed_resident(&mut rig.pool, 10, &[5, 7]);
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
        (vec![5, 7], true),
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
            },
        );
    }
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
    let entries = cell_cut_with(
        cut,
        &|id| {
            let node = nodes.get(&id)?;
            Some(CellCutNode {
                bounds: node.bounds,
                stored: node.stored.clone(),
                resident: node
                    .stored
                    .iter()
                    .copied()
                    .filter(|d| rig.pool.is_resident(rig.pool.key(id, *d)))
                    .collect(),
            })
        },
        &grid,
        &|_, _, _| wanted,
    )
    .unwrap();
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
            points: 2000,
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
fn the_hold_threshold_is_the_depth_the_ancestor_drew_over_the_octant_of_the_child() {
    // Node 2 drew depth 6 over its octants 0..7 and 5 over octant 7. Child 10 (octant 0) has
    // level 5 and wants 7: short of the 6 drawn there, held. Child 17 (octant 7) is not: 5 is
    // what node 2 drew there.
    let mut rig = rig_with_coarse_children(None);
    let mut drawn = [Some(6); 8];
    drawn[7] = Some(5);
    rig.pool.applied_drawn.insert(NodeId::new(2), drawn);
    let (eval, _) = eval_with(&rig, &ids(&[10, 17]), 7);
    assert!(eval.hold.contains(&NodeId::new(10)));
    assert!(!eval.hold.contains(&NodeId::new(17)));
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
/// `resident_depth` gives for the wanted one over the levels that were resident, and no point may
/// be drawn by two entries.
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
                .inspect(|(drawn, wanted)| {
                    let levels = capped(&seen.resident[&entry.node], seen.caps.get(&entry.node));
                    assert_eq!(
                        crate::resident_depth(*wanted, &levels).map(|(d, _)| d),
                        Some(*drawn),
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
            points: 2000,
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
            points: 2000,
        },
    );
    nodes.insert(
        NodeId::new(30),
        PoolNode {
            bounds: cube(ORIGIN, SIDE / 32.0),
            stored: vec![9, 10],
            points: 2000,
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

/// An entry of `node` drawing the given octants at depth 7.
fn drawing(node: u64, octants: &[usize]) -> CellCutEntry {
    CellCutEntry {
        node: NodeId::new(node),
        wanted: [Some(7); 8],
        drawn: std::array::from_fn(|o| octants.contains(&o).then_some(7)),
        stage: [false; 8],
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
#[allow(clippy::type_complexity)]
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
