//! Task 7a pass criteria: residency is all-or-nothing, uploads are spread over frames, the fit
//! coarsens the farthest nodes first and settles, and no page is rewritten while it can be read.

use std::collections::{HashMap, HashSet};

use viewer_core::{NodeId, SpatialBounds};

use super::*;
use crate::{
    CellInstance, OrbitCamera, PageInfo, VoxelBuildLimits,
    scene_digest_tests::{ORIGIN, SIDE, VIEWPORT, cameras, ids},
};

fn view(camera: &OrbitCamera) -> ViewProjection {
    ViewProjection::new(*camera, VIEWPORT, 1.0, VoxelBuildLimits::default())
}

fn grid() -> CellGrid {
    CellGrid {
        root_min: ORIGIN,
        root_side: SIDE,
    }
}

fn camera(name: &str) -> OrbitCamera {
    cameras()
        .into_iter()
        .find(|(n, _)| *n == name)
        .expect("camera")
        .1
}

fn cube(min: [f64; 3], side: f64) -> SpatialBounds {
    SpatialBounds::new(min, min.map(|v| v + side)).unwrap()
}

/// The eight children of the root (ids 2..=9) with stored depths 4..=7. With 2000 points a level
/// has 1 page at depth 4 and 4 pages deeper, which is exactly the pool's estimate.
fn octant_nodes() -> HashMap<NodeId, PoolNode> {
    let half = SIDE / 2.0;
    (0..8)
        .map(|index| {
            let min =
                std::array::from_fn(|axis| ORIGIN[axis] + half * ((index >> axis) & 1) as f64);
            (
                NodeId::new(index as u64 + 2),
                PoolNode {
                    bounds: cube(min, half),
                    stored: vec![4, 5, 6, 7],
                    points: 2000,
                },
            )
        })
        .collect()
}

const CHILDREN: [u64; 8] = [2, 3, 4, 5, 6, 7, 8, 9];

fn fake_level(key: LevelKey, pages: usize) -> PackedLevel {
    let page = PageInfo {
        node: key.node,
        depth: key.depth,
        origin: [0; 3],
        first_cell: 0,
        cells: 0,
        box_max: [0; 3],
        flags: 0,
    };
    PackedLevel {
        node: key.node,
        depth: key.depth,
        instances: vec![CellInstance::EMPTY; pages * PAGE_SLOTS],
        pages: vec![page; pages],
    }
}

/// Executes plans like the GPU would and fails on any hazard: a page written while the current or
/// the previous frame can read it, a write over a page that is still owned, or an active level
/// whose pages are not all uploaded.
#[derive(Default)]
struct FakeGpu {
    owner: HashMap<u32, (LevelKey, usize)>,
    slot_of: HashMap<(LevelKey, usize), u32>,
    total: HashMap<LevelKey, usize>,
    prev_active: HashSet<u32>,
    masks: HashMap<u32, [u64; 8]>,
}

impl FakeGpu {
    fn apply(&mut self, plan: &FramePlan, pool: &CellPool) {
        for slot in &plan.page_frees {
            if let Some((key, page)) = self.owner.remove(slot) {
                self.slot_of.remove(&(key, page));
            }
        }
        for update in &plan.mask_updates {
            self.masks.insert(update.slot, update.mask);
        }
        let mut current = HashSet::new();
        for active in &plan.active {
            let entry = plan
                .entries
                .iter()
                .find(|entry| entry.node == active.key.node)
                .expect("active node is in the cut");
            assert_eq!(
                self.masks.get(&active.mask_slot),
                Some(&entry.mask),
                "mask of node {:?} on the GPU differs from its cut entry",
                active.key.node
            );
            let total = self.total[&active.key];
            for page in 0..total {
                let slot = self.slot_of.get(&(active.key, page)).unwrap_or_else(|| {
                    panic!("{:?} page {page} is active but not uploaded", active.key)
                });
                current.insert(*slot);
            }
        }
        for upload in &plan.uploads {
            let pages = pool
                .level(upload.key)
                .expect("uploaded level has data")
                .pages
                .len();
            self.total.insert(upload.key, pages);
            assert_eq!(upload.slots.len(), upload.page_range.len());
            for (page, slot) in upload.page_range.clone().zip(&upload.slots) {
                assert!(
                    !current.contains(slot) && !self.prev_active.contains(slot),
                    "slot {slot} is written while the current or previous frame reads it"
                );
                assert!(!self.owner.contains_key(slot), "slot {slot} is still owned");
                assert!(self.slot_of.insert((upload.key, page), *slot).is_none());
                self.owner.insert(*slot, (upload.key, page));
            }
        }
        self.prev_active = current;
    }

    /// Levels with some pages uploaded but not all.
    fn partial(&self) -> Vec<LevelKey> {
        let mut written: HashMap<LevelKey, usize> = HashMap::new();
        for (key, _) in self.owner.values() {
            *written.entry(*key).or_default() += 1;
        }
        written
            .into_iter()
            .filter(|(key, count)| *count < self.total[key])
            .map(|(key, _)| key)
            .collect()
    }
}

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
        for (_, key) in due {
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
fn uploads_start_with_nodes_that_draw_nothing_and_then_go_nearest_first() {
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
        assert_eq!(plan.counters.nodes_without_level, 0);
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

/// Nodes of the `RetainedPrevious` scenario: the root, a node 4 levels below it (too fine for the
/// mask) and one 3 levels below it.
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
                bounds: cube(ORIGIN, SIDE / 16.0),
                stored: vec![7, 8],
                points: 2000,
            },
        ),
        (
            NodeId::new(21),
            PoolNode {
                bounds: cube(ORIGIN, SIDE / 8.0),
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
    // Together with the root it is 4 levels below it: the mask cannot express it.
    let held = alone.entries.clone();
    for _ in 0..10 {
        let plan = rig.step(&ids(&[1, 20]), &cam, 64.0);
        assert_eq!(plan.apply, CutApply::RetainedPrevious);
        assert!(plan.counters.descendant_too_fine);
        assert_eq!(plan.entries, held);
        assert_eq!(plan.counters.nodes_without_level, 0);
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
    // Node 21 is 3 levels below the root: the cut applies once its coarsest level is resident.
    let mut applied_at = None;
    for frame in 0..12 {
        let plan = rig.step(&ids(&[1, 21]), &cam, 64.0);
        if plan.apply == CutApply::Applied {
            applied_at.get_or_insert(frame);
            assert_eq!(plan.counters.nodes_without_level, 0);
            assert_eq!(plan.entries.len(), 2);
            assert!(!plan.counters.descendant_too_fine);
        } else {
            assert!(applied_at.is_none(), "left the applied state again");
            assert_eq!(plan.entries, held);
        }
    }
    assert!(matches!(applied_at, Some(n) if n > 0), "{applied_at:?}");
}

#[test]
fn a_new_cut_replaces_an_incomplete_one_but_waits_for_a_complete_one() {
    let mut rig = Rig::new(octant_nodes(), 64, 8, 3);
    let cam = camera("fit");
    // Nothing is resident for [2, 3]: it applies with holes, since there is nothing to keep.
    let first = rig.step(&ids(&[2, 3]), &cam, 64.0);
    assert_eq!(first.apply, CutApply::Applied);
    assert_eq!(first.counters.nodes_without_level, 2);
    // That cut is incomplete, so a different one replaces it at once.
    let second = rig.step(&ids(&[4, 5]), &cam, 64.0);
    assert_eq!(second.apply, CutApply::Applied);
    assert_eq!(second.counters.nodes_without_level, 2);
    let done = rig.run(8, &ids(&[4, 5]), &cam, 64.0);
    assert_eq!(done.counters.nodes_without_level, 0);
    // The applied cut is complete now: [2, 3] waits for its levels.
    let wait = rig.step(&ids(&[2, 3]), &cam, 64.0);
    assert_eq!(wait.apply, CutApply::RetainedPrevious);
    assert_eq!(wait.entries, done.entries);
    let back = rig.run(8, &ids(&[2, 3]), &cam, 64.0);
    assert_eq!(back.apply, CutApply::Applied);
    assert_eq!(back.counters.nodes_without_level, 0);
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
