//! Shared fixtures of the cell pool and driver tests (here and in `viewer_wasm`, through the
//! `test-support` feature): the cameras, node layouts, the one strict fake GPU that fails on any
//! protocol hazard, and the hole oracle.

use std::collections::{HashMap, HashSet};

use viewer_core::{NodeId, SpatialBounds};

use crate::{
    CellGrid, CellInstance, CellPool, CellPoolError, CellTarget, FramePlan, LevelKey, NODE_WORDS,
    OrbitCamera, PAGE_SLOTS, PackedLevel, PageEntry, PageFrame, PageInfo, PoolNode, SceneBounds,
    ViewProjection, VoxelBuildLimits,
};

pub const ORIGIN: [f64; 3] = [1000.0, -2000.0, 50.0];
pub const SIDE: f64 = 64.0;
pub const VIEWPORT: [u32; 2] = [800, 600];

pub fn cameras() -> Vec<(&'static str, OrbitCamera)> {
    let bounds = SceneBounds {
        min: [0.0; 3],
        max: [SIDE; 3],
    };
    let fitted = || {
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(bounds, VIEWPORT));
        camera
    };
    let mut orbited = fitted();
    orbited.orbit(0.2, 0.1);
    let mut zoomed = fitted();
    zoomed.zoom(-0.7);
    let mut panned = fitted();
    panned.orbit(-0.1, -0.05);
    panned.pan(0.1, -0.05, VIEWPORT);
    vec![
        ("fit", fitted()),
        ("orbit", orbited),
        ("zoom", zoomed),
        ("pan", panned),
    ]
}

pub fn view(camera: &OrbitCamera) -> ViewProjection {
    ViewProjection::new(*camera, VIEWPORT, 1.0, VoxelBuildLimits::default())
}

pub fn grid() -> CellGrid {
    CellGrid {
        root_min: ORIGIN,
        root_side: SIDE,
    }
}

pub fn camera(name: &str) -> OrbitCamera {
    cameras()
        .into_iter()
        .find(|(n, _)| *n == name)
        .expect("camera")
        .1
}

pub fn cube(min: [f64; 3], side: f64) -> SpatialBounds {
    SpatialBounds::new(min, min.map(|v| v + side)).unwrap()
}

/// The eight children of the root (ids 2..=9) with stored depths 4..=7. With 2000 points a level
/// has 1 page at depth 4 and 4 pages deeper, which is exactly the pool's estimate.
pub fn octant_nodes() -> HashMap<NodeId, PoolNode> {
    let half = SIDE / 2.0;
    (0..8)
        .map(|index| {
            let min =
                std::array::from_fn(|axis| ORIGIN[axis] + half * f64::from((index >> axis) & 1));
            (
                NodeId::new(index as u64 + 2),
                PoolNode {
                    bounds: cube(min, half),
                    stored: vec![4, 5, 6, 7],
                    buildable: (4, 7),
                    points: 2000,
                    octant_data_bounds: None,
                },
            )
        })
        .collect()
}

pub const CHILDREN: [u64; 8] = [2, 3, 4, 5, 6, 7, 8, 9];

/// A level of empty pages at the grid corner. Each page names itself in `box_max` (page index,
/// depth, node), which `PageInfo::entry` copies to words 6 and 7 of its entry, so the fake GPU can
/// tell which page a table entry points at.
pub fn fake_level(key: LevelKey, pages: usize) -> PackedLevel {
    PackedLevel {
        node: key.node,
        depth: key.depth,
        instances: vec![CellInstance::EMPTY; pages * PAGE_SLOTS],
        pages: (0..pages)
            .map(|page| PageInfo {
                node: key.node,
                depth: key.depth,
                origin: [0; 3],
                first_cell: 0,
                cells: 0,
                box_max: [page as u16, u16::from(key.depth), key.node.get() as u16],
                flags: 0,
                rel_origin: [0; 3],
            })
            .collect(),
    }
}

/// A node record holding `mask` and nothing else (anchor, depth and mips zero).
pub fn mask_record(mask: &[u32; crate::MASK_WORDS]) -> [u32; NODE_WORDS] {
    let mut record = [0; NODE_WORDS];
    record[..crate::MASK_WORDS].copy_from_slice(mask);
    record
}

/// Executes plans like the GPU would and panics on any hazard of the pool protocol: a slot written
/// while it is still owned or while the current or the previous page table reads it, a slot freed
/// while the previous table reads it, a table naming a slot nobody uploaded or one that holds
/// another page than its entry, a duplicate table slot, and a node record for a mask slot nobody
/// allocated. Lifecycle conditions a live renderer reports (no pool, a page count that does not
/// match the slots) are `Err`s.
#[derive(Default)]
pub struct FakeGpu {
    /// The slot owner. `apply` keys it by the plan's level and page; the `CellTarget` side by the
    /// uploaded level (session and colour generation zero) and the page's order in `slots_of`.
    pub owner: HashMap<u32, (LevelKey, usize)>,
    pub slot_of: HashMap<(LevelKey, usize), u32>,
    pub total: HashMap<LevelKey, usize>,
    pub prev_active: HashSet<u32>,
    pub masks: HashMap<u32, [u64; 8]>,
    // The `CellTarget` side, used by the driver tests: it mirrors a `SurfaceRenderer`, which has
    // no pool before the first upload and fails every other call without one.
    pub has_pool: bool,
    pub eye: [f64; 3],
    /// Slots written since the last page table; they must not be in it or in the previous one.
    pub frame_writes: Vec<u32>,
    /// The last record written to each mask slot.
    pub mask_words: HashMap<u32, [u32; NODE_WORDS]>,
    /// Every record write, in order.
    pub record_writes: Vec<(u32, [u32; NODE_WORDS])>,
    pub next_mask: u32,
    /// The next `set_node_record` fails once. The fake keeps its state (slots, masks) as a live
    /// renderer would, until `drop_pool`.
    pub fail_next_mask: bool,
    /// Pages rewritten in place over the same page (recolours).
    pub rewrites: usize,
    /// Page writes, rewrites included.
    pub uploads_total: usize,
    /// The page the upload put in each slot.
    pub page_at: HashMap<u32, PageInfo>,
    /// Slots written per level, in upload order.
    pub slots_of: HashMap<(NodeId, u8), Vec<u32>>,
    /// Hash of the cells each slot holds (colours included), to tell recoloured pages apart.
    pub page_cells: HashMap<u32, u64>,
    /// The last page table and the frame it came with.
    pub table: Vec<(u32, PageEntry)>,
    pub page_frame: Option<PageFrame>,
    /// Page tables written.
    pub tables_written: usize,
    /// The ranges of the last frame's cut (`CellTarget::set_color_ranges`).
    pub color_ranges: Option<crate::CellColorRanges>,
    /// The view of every frame that reached the target (`CellTarget::set_lod_view`).
    pub lod_views: Vec<Option<crate::cells::lod::LodView>>,
}

impl FakeGpu {
    /// The renderer dropped its pool (or the device was lost): every slot and mask is gone.
    pub fn lose_pool(&mut self) {
        *self = Self {
            eye: self.eye,
            ..Self::default()
        };
    }

    /// What the target would show in colour: the sum over the active pages of a hash of the
    /// page's identity and cells. Independent of the slots the pages sit in.
    pub fn color_digest(&self) -> u64 {
        self.table.iter().fold(0_u64, |acc, (slot, _)| {
            let mut hasher = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(&format!("{:?}", self.page_at[slot]), &mut hasher);
            std::hash::Hash::hash(&self.page_cells[slot], &mut hasher);
            acc.wrapping_add(std::hash::Hasher::finish(&hasher))
        })
    }

    /// The hash of the cells of every active page, by (node, depth, page of the level).
    pub fn active_cells(&self) -> HashMap<(NodeId, u8, usize), u64> {
        let active: HashSet<u32> = self.table.iter().map(|(slot, _)| *slot).collect();
        let mut out = HashMap::new();
        for ((node, depth), slots) in &self.slots_of {
            for (page, slot) in slots.iter().enumerate() {
                if active.contains(slot) {
                    out.insert((*node, *depth, page), self.page_cells[slot]);
                }
            }
        }
        out
    }

    /// What the driver left in the target equals what the plan asked for: each active node's
    /// mask slot holds the mask of its cut entry, and the page table lists, level by level, every
    /// page that was uploaded for it with the entry that page computes for the plan's mask slot
    /// and octants.
    pub fn check_plan(&self, plan: &FramePlan) {
        let mut expected: Vec<(u32, PageEntry)> = Vec::new();
        for active in &plan.active {
            let entry = plan
                .entries
                .iter()
                .find(|entry| entry.node == active.key.node)
                .expect("an active node is in the cut");
            let words: [u32; crate::MASK_WORDS] =
                std::array::from_fn(|i| (entry.mask[i / 2] >> (32 * (i % 2))) as u32);
            assert_eq!(
                self.mask_words
                    .get(&active.mask_slot)
                    .map(|record| &record[..crate::MASK_WORDS]),
                Some(&words[..]),
                "mask slot {} of node {:?} differs from its cut entry",
                active.mask_slot,
                active.key.node
            );
            let frame = self.page_frame.as_ref().expect("a table was written");
            let slots = self
                .slots_of
                .get(&(active.key.node, active.key.depth))
                .unwrap_or_else(|| panic!("active level {:?} was never uploaded", active.key));
            for slot in slots {
                expected.push((
                    *slot,
                    self.page_at[slot].entry(
                        frame,
                        active.mask_slot,
                        u32::from(active.octant_bits),
                    ),
                ));
            }
        }
        assert_eq!(self.table, expected, "page table differs from the plan");
    }
}

impl CellTarget for FakeGpu {
    type Error = CellPoolError;

    fn has_pool(&self) -> bool {
        self.has_pool
    }

    fn drop_pool(&mut self) {
        self.lose_pool();
    }

    fn eye(&self) -> [f64; 3] {
        self.eye
    }

    fn upload_level(
        &mut self,
        level: &PackedLevel,
        page_slots: &[u32],
    ) -> Result<(), CellPoolError> {
        // The first upload creates the pool, but a failed one drops it again.
        if level.pages.len() != page_slots.len() {
            return Err(CellPoolError::PageCountMismatch {
                pages: level.pages.len(),
                slots: page_slots.len(),
            });
        }
        self.has_pool = true;
        let key = LevelKey {
            session: 0,
            node: level.node,
            depth: level.depth,
            color_gen: 0,
        };
        for (index, (slot, info)) in page_slots.iter().zip(&level.pages).enumerate() {
            let cells = &level.instances[index * PAGE_SLOTS..(index + 1) * PAGE_SLOTS];
            let hash = {
                let mut hasher = std::hash::DefaultHasher::new();
                for cell in cells {
                    std::hash::Hash::hash(&cell.0, &mut hasher);
                }
                std::hash::Hasher::finish(&hasher)
            };
            self.page_cells.insert(*slot, hash);
            self.uploads_total += 1;
            // A recolour rewrites the page its slot already holds: the queue orders the write
            // after the draws already submitted, so it is not a hazard.
            if self.owner.get(slot).map(|(owner, _)| owner) == Some(&key) {
                self.rewrites += 1;
                continue;
            }
            assert!(!self.owner.contains_key(slot), "slot {slot} is still owned");
            let slots = self.slots_of.entry((level.node, level.depth)).or_default();
            self.owner.insert(*slot, (key, slots.len()));
            slots.push(*slot);
            self.page_at.insert(*slot, info.clone());
            self.frame_writes.push(*slot);
        }
        Ok(())
    }

    fn free_pages(&mut self, page_slots: &[u32]) -> Result<(), CellPoolError> {
        if !self.has_pool {
            return Err(CellPoolError::NoPool);
        }
        for slot in page_slots {
            assert!(
                !self.prev_active.contains(slot),
                "slot {slot} is freed while the previous table reads it"
            );
            self.page_at.remove(slot);
            if let Some((key, _)) = self.owner.remove(slot)
                && let Some(slots) = self.slots_of.get_mut(&(key.node, key.depth))
            {
                slots.retain(|owned| owned != slot);
            }
        }
        Ok(())
    }

    fn alloc_mask_slot(&mut self) -> Result<Option<u32>, CellPoolError> {
        if !self.has_pool {
            return Err(CellPoolError::NoPool);
        }
        self.next_mask += 1;
        Ok(Some(self.next_mask - 1))
    }

    fn set_node_record(
        &mut self,
        slot: u32,
        record: &[u32; NODE_WORDS],
    ) -> Result<(), CellPoolError> {
        if !self.has_pool {
            return Err(CellPoolError::NoPool);
        }
        assert!(
            slot < self.next_mask,
            "mask slot {slot} was never allocated"
        );
        if std::mem::take(&mut self.fail_next_mask) {
            return Err(CellPoolError::NoPool);
        }
        self.mask_words.insert(slot, *record);
        self.record_writes.push((slot, *record));
        Ok(())
    }

    fn set_color_ranges(&mut self, ranges: crate::CellColorRanges) {
        self.color_ranges = Some(ranges);
    }

    fn set_lod_view(&mut self, view: Option<crate::cells::lod::LodView>) {
        self.lod_views.push(view);
    }

    fn set_active_pages(
        &mut self,
        pages: &[(u32, PageEntry)],
        frame: &PageFrame,
    ) -> Result<(), CellPoolError> {
        if !self.has_pool {
            return Err(CellPoolError::NoPool);
        }
        let mut current = HashSet::new();
        for (slot, entry) in pages {
            let page = self.page_at.get(slot);
            assert!(page.is_some(), "slot {slot} is active but not uploaded");
            // The entry must describe the page that lives in its slot: words 6.. (box, flags,
            // relative origin, depth) come from the page alone.
            assert_eq!(
                entry.0[6..],
                page.unwrap().entry(frame, 0, 0).0[6..],
                "slot {slot} holds another page than its table entry"
            );
            assert!(current.insert(*slot), "slot {slot} is active twice");
        }
        for slot in &self.frame_writes {
            assert!(
                !current.contains(slot) && !self.prev_active.contains(slot),
                "slot {slot} is written while the current or previous frame reads it"
            );
        }
        self.frame_writes.clear();
        self.prev_active = current;
        self.tables_written += 1;
        self.table = pages.to_vec();
        self.page_frame = Some(*frame);
        Ok(())
    }
}

impl FakeGpu {
    pub fn apply(&mut self, plan: &FramePlan, pool: &CellPool) {
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
    pub fn partial(&self) -> Vec<LevelKey> {
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

pub fn side_of(bounds: SpatialBounds) -> f64 {
    bounds.max()[0] - bounds.min()[0]
}

/// The eight children of `parent`, with ids from `first_id`, in the order of the octants of
/// `octant_nodes`.
pub fn octants_of(parent: &PoolNode, first_id: u64, stored: &[u8]) -> Vec<(NodeId, PoolNode)> {
    let half = side_of(parent.bounds) / 2.0;
    (0..8)
        .map(|index| {
            let min = std::array::from_fn(|axis| {
                parent.bounds.min()[axis] + half * f64::from((index >> axis) & 1)
            });
            (
                NodeId::new(first_id + index as u64),
                PoolNode {
                    bounds: cube(min, half),
                    stored: stored.to_vec(),
                    buildable: (stored[0], stored[stored.len() - 1]),
                    points: 2000,
                    octant_data_bounds: None,
                },
            )
        })
        .collect()
}

/// Nodes 2 and 3 (regions A and B), each with eight children: 10..=17 under 2, 20..=27 under 3.
pub fn two_regions() -> HashMap<NodeId, PoolNode> {
    let mut nodes = octant_nodes();
    nodes.retain(|id, _| id.get() <= 3);
    for (parent, first) in [(2, 10), (3, 20)] {
        let children = octants_of(&nodes[&NodeId::new(parent)], first, &[5, 6, 7]);
        nodes.extend(children);
    }
    nodes
}

pub fn region_children(first: u64) -> Vec<u64> {
    (first..first + 8).collect()
}

/// No hole in the plan: every unhidden octant of every entry is drawn by a resident level of its
/// own, or lies under an ancestor entry of the cut that draws there and does not hide it
/// (geometry computed here, from the bounds).
#[allow(
    clippy::implicit_hasher,
    reason = "test oracle over the std map the fixtures build"
)]
pub fn assert_no_holes(plan: &FramePlan, nodes: &HashMap<NodeId, PoolNode>) {
    let drawn_by = |node: NodeId, octant: usize| {
        plan.active
            .iter()
            .any(|a| a.key.node == node && a.octant_bits >> octant & 1 == 1)
    };
    for entry in &plan.entries {
        let bounds = nodes[&entry.node].bounds;
        for octant in (0..8).filter(|o| entry.wanted[*o].is_some()) {
            if entry.drawn[octant].is_some() {
                assert!(drawn_by(entry.node, octant), "{:?}", entry.node);
                continue;
            }
            let half = side_of(bounds) / 2.0;
            let point: [f64; 3] = std::array::from_fn(|a| {
                bounds.min()[a] + half * (((octant >> (2 - a)) & 1) as f64 + 0.5)
            });
            let covered = plan.entries.iter().any(|ancestor| {
                let outer = nodes[&ancestor.node].bounds;
                if ancestor.node == entry.node
                    || !(0..3).all(|a| {
                        outer.min()[a] <= bounds.min()[a] && bounds.max()[a] <= outer.max()[a]
                    })
                {
                    return false;
                }
                let cell = side_of(outer) / 8.0;
                let at: [usize; 3] =
                    std::array::from_fn(|a| (((point[a] - outer.min()[a]) / cell) as usize).min(7));
                let hidden = ancestor.mask[at[2]] >> (at[0] + 8 * at[1]) & 1 == 1;
                let octant_there = ((at[0] / 4) << 2) | ((at[1] / 4) << 1) | (at[2] / 4);
                !hidden
                    && ancestor.drawn[octant_there].is_some()
                    && drawn_by(ancestor.node, octant_there)
            });
            assert!(covered, "node {:?} octant {octant} is a hole", entry.node);
        }
    }
}
