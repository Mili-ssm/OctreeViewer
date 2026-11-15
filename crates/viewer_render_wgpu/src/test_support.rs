//! Shared fixtures of the cell pool and driver tests: the cameras, node layouts, the strict fake
//! GPU that fails on any hazard, and the hole oracle.

use std::collections::{HashMap, HashSet};

use viewer_core::{NodeId, SpatialBounds};

use crate::scene_digest_tests::{ORIGIN, SIDE, VIEWPORT, cameras};
use crate::{
    CellGrid, CellInstance, CellPool, CellPoolError, CellTarget, FramePlan, LevelKey, MASK_WORDS,
    OrbitCamera, PAGE_SLOTS, PackedLevel, PageEntry, PageFrame, PageInfo, PoolNode, ViewProjection,
    VoxelBuildLimits,
};

pub(crate) fn view(camera: &OrbitCamera) -> ViewProjection {
    ViewProjection::new(*camera, VIEWPORT, 1.0, VoxelBuildLimits::default())
}

pub(crate) fn grid() -> CellGrid {
    CellGrid {
        root_min: ORIGIN,
        root_side: SIDE,
    }
}

pub(crate) fn camera(name: &str) -> OrbitCamera {
    cameras()
        .into_iter()
        .find(|(n, _)| *n == name)
        .expect("camera")
        .1
}

pub(crate) fn cube(min: [f64; 3], side: f64) -> SpatialBounds {
    SpatialBounds::new(min, min.map(|v| v + side)).unwrap()
}

/// The eight children of the root (ids 2..=9) with stored depths 4..=7. With 2000 points a level
/// has 1 page at depth 4 and 4 pages deeper, which is exactly the pool's estimate.
pub(crate) fn octant_nodes() -> HashMap<NodeId, PoolNode> {
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
                    buildable: (4, 7),
                    points: 2000,
                },
            )
        })
        .collect()
}

pub(crate) const CHILDREN: [u64; 8] = [2, 3, 4, 5, 6, 7, 8, 9];

/// A level of empty pages at the grid corner. Each page names itself in `box_max` (page index,
/// depth, node), which `PageInfo::entry` copies to words 6 and 7 of its entry, so the fake GPU can
/// tell which page a table entry points at.
pub(crate) fn fake_level(key: LevelKey, pages: usize) -> PackedLevel {
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
            })
            .collect(),
    }
}

/// The (level, page) a fake entry names (see `fake_level`).
fn fake_entry_target(entry: &PageEntry) -> (LevelKey, usize) {
    let [.., word6, word7] = entry.0;
    (
        LevelKey {
            session: 0,
            node: NodeId::new(u64::from(word7 & 0xffff)),
            depth: (word6 >> 16) as u8,
            color_gen: 0,
        },
        (word6 & 0xffff) as usize,
    )
}

/// Executes plans like the GPU would and fails on any hazard: a page written while the current or
/// the previous frame can read it, a write over a page that is still owned, or an active level
/// whose pages are not all uploaded.
#[derive(Default)]
pub(crate) struct FakeGpu {
    pub(crate) owner: HashMap<u32, (LevelKey, usize)>,
    pub(crate) slot_of: HashMap<(LevelKey, usize), u32>,
    pub(crate) total: HashMap<LevelKey, usize>,
    pub(crate) prev_active: HashSet<u32>,
    pub(crate) masks: HashMap<u32, [u64; 8]>,
    // The `CellTarget` side, used by the driver tests: it mirrors a `SurfaceRenderer`, which has
    // no pool before the first upload and fails every other call without one.
    pub(crate) has_pool: bool,
    pub(crate) eye: [f64; 3],
    /// Slots written since the last page table; they must not be in it or in the previous one.
    pub(crate) frame_writes: Vec<u32>,
    pub(crate) mask_words: HashMap<u32, [u32; MASK_WORDS]>,
    pub(crate) next_mask: u32,
    /// The next `set_node_mask` fails once. The fake keeps its state (slots, masks) as a live
    /// renderer would, until `drop_pool`.
    pub(crate) fail_next_mask: bool,
    /// Pages rewritten in place over the same page (recolours).
    pub(crate) rewrites: usize,
    /// The last page table and the frame it came with.
    pub(crate) table: Vec<(u32, PageEntry)>,
    pub(crate) page_frame: Option<PageFrame>,
    /// The ranges of the last frame's cut (`CellTarget::set_color_ranges`).
    pub(crate) color_ranges: Option<crate::CellColorRanges>,
}

impl FakeGpu {
    /// The renderer dropped its pool (or the device was lost): every slot and mask is gone.
    pub(crate) fn lose_pool(&mut self) {
        *self = Self {
            eye: self.eye,
            ..Self::default()
        };
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
        self.has_pool = true;
        assert_eq!(level.pages.len(), page_slots.len());
        let key = LevelKey {
            session: 0,
            node: level.node,
            depth: level.depth,
            color_gen: 0,
        };
        for (info, slot) in level.pages.iter().zip(page_slots) {
            let page = usize::from(info.box_max[0]);
            // A recolour rewrites the page its slot already holds: the queue orders the write
            // after the draws already submitted, so it is not a hazard.
            if self.owner.get(slot) == Some(&(key, page)) {
                self.rewrites += 1;
                continue;
            }
            assert!(!self.owner.contains_key(slot), "slot {slot} is still owned");
            // A part of a level carries its own pages: the index is the one `fake_level` wrote.
            self.owner.insert(*slot, (key, page));
            self.frame_writes.push(*slot);
        }
        Ok(())
    }

    fn free_pages(&mut self, page_slots: &[u32]) -> Result<(), CellPoolError> {
        if !self.has_pool {
            return Err(CellPoolError::NoPool);
        }
        for slot in page_slots {
            self.owner.remove(slot);
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

    fn set_node_mask(&mut self, slot: u32, mask: &[u32; MASK_WORDS]) -> Result<(), CellPoolError> {
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
        self.mask_words.insert(slot, *mask);
        Ok(())
    }

    fn set_color_ranges(&mut self, ranges: crate::CellColorRanges) {
        self.color_ranges = Some(ranges);
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
            let owned = self.owner.get(slot);
            assert!(owned.is_some(), "slot {slot} is active but not uploaded");
            // The entry must describe the page that lives in its slot.
            assert_eq!(
                owned.copied(),
                Some(fake_entry_target(entry)),
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
        self.table = pages.to_vec();
        self.page_frame = Some(*frame);
        Ok(())
    }
}

impl FakeGpu {
    pub(crate) fn apply(&mut self, plan: &FramePlan, pool: &CellPool) {
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
    pub(crate) fn partial(&self) -> Vec<LevelKey> {
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

pub(crate) fn side_of(bounds: SpatialBounds) -> f64 {
    bounds.max()[0] - bounds.min()[0]
}

/// The eight children of `parent`, with ids from `first_id`, in the order of the octants of
/// `octant_nodes`.
pub(crate) fn octants_of(
    parent: &PoolNode,
    first_id: u64,
    stored: &[u8],
) -> Vec<(NodeId, PoolNode)> {
    let half = side_of(parent.bounds) / 2.0;
    (0..8)
        .map(|index| {
            let min = std::array::from_fn(|axis| {
                parent.bounds.min()[axis] + half * ((index >> axis) & 1) as f64
            });
            (
                NodeId::new(first_id + index as u64),
                PoolNode {
                    bounds: cube(min, half),
                    stored: stored.to_vec(),
                    buildable: (stored[0], stored[stored.len() - 1]),
                    points: 2000,
                },
            )
        })
        .collect()
}

/// Nodes 2 and 3 (regions A and B), each with eight children: 10..=17 under 2, 20..=27 under 3.
pub(crate) fn two_regions() -> HashMap<NodeId, PoolNode> {
    let mut nodes = octant_nodes();
    nodes.retain(|id, _| id.get() <= 3);
    for (parent, first) in [(2, 10), (3, 20)] {
        let children = octants_of(&nodes[&NodeId::new(parent)], first, &[5, 6, 7]);
        nodes.extend(children);
    }
    nodes
}

pub(crate) fn region_children(first: u64) -> Vec<u64> {
    (first..first + 8).collect()
}

/// No hole in the plan: every unhidden octant of every entry is drawn by a resident level of its
/// own, or lies under an ancestor entry of the cut that draws there and does not hide it
/// (geometry computed here, from the bounds).
pub(crate) fn assert_no_holes(plan: &FramePlan, nodes: &HashMap<NodeId, PoolNode>) {
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
