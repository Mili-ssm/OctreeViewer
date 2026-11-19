//! Page bookkeeping: offers, uploads, evictions, delayed frees and the mask slots.

use super::*;

impl CellPool {
    /// Delivers the data of a requested level. A level nobody wants any more, one that already has
    /// data, an empty one (no pages) or one larger than the pool is dropped (`false`). An `Arc`
    /// is shared, not copied.
    pub fn offer(&mut self, key: LevelKey, level: impl Into<Arc<PackedLevel>>) -> bool {
        let level = level.into();
        let capacity = self.config.capacity_pages as usize;
        match self.levels.get(&key) {
            Some(entry)
                if matches!(entry.state, LevelState::Requested)
                    && entry.data.is_none()
                    && !level.pages.is_empty()
                    && level.pages.len() <= capacity =>
            {
                let before = self.known_pages(key);
                let pages = level.pages.len();
                if let Some(entry) = self.levels.get_mut(&key) {
                    entry.data = Some(level);
                }
                if before != Some(pages) {
                    self.size_epoch += 1;
                }
                true
            }
            _ => false,
        }
    }

    /// Replaces the data of a level with a recoloured copy of the same cells (colour generation
    /// change, R3-Q33). Same page count is a precondition (`false` otherwise, nothing changes).
    /// A level the pool does not hold needs nothing: it is requested from the new data. A level
    /// not resident yet restarts its upload from the new data; a resident one keeps drawing the
    /// old colours until the next frame writes the new ones over its page slots (see
    /// `refresh_uploads`): the page table does not change, so no gap opens.
    pub fn refresh_level(&mut self, key: LevelKey, level: impl Into<Arc<PackedLevel>>) -> bool {
        let level = level.into();
        let Some(entry) = self.levels.get_mut(&key) else {
            return true;
        };
        if entry
            .pages()
            .is_some_and(|pages| pages != level.pages.len())
        {
            return false;
        }
        match &mut entry.state {
            LevelState::Requested => {
                if entry.data.is_some() {
                    entry.data = Some(level);
                }
            }
            LevelState::Uploading { pages_done, .. } => {
                *pages_done = 0;
                entry.data = Some(level);
            }
            LevelState::Resident { .. } => entry.refresh = Some(level),
        }
        true
    }

    /// Writes the pending recolours over the page slots of their resident levels, coarse to
    /// fine and one node whole at a time (a node never draws old and new colours together). The
    /// budget is checked before each node, so a node larger than what is left still goes in one
    /// frame. Returns the pages written.
    pub(super) fn refresh_uploads(
        &mut self,
        budget: &mut usize,
        uploads: &mut Vec<PageUpload>,
    ) -> usize {
        let mut nodes: Vec<(u8, NodeId)> = Vec::new();
        for (key, level) in &self.levels {
            if level.refresh.is_some() {
                match nodes.iter_mut().find(|(_, id)| *id == key.node) {
                    Some((depth, _)) => *depth = (*depth).min(key.depth),
                    None => nodes.push((key.depth, key.node)),
                }
            }
        }
        nodes.sort();
        let mut written = 0;
        for (_, node) in nodes {
            if *budget == 0 {
                break;
            }
            let mut keys: Vec<LevelKey> = self
                .levels
                .iter()
                .filter(|(key, level)| key.node == node && level.refresh.is_some())
                .map(|(key, _)| *key)
                .collect();
            keys.sort();
            for key in keys {
                let level = self.levels.get_mut(&key).expect("level");
                let (Some(data), LevelState::Resident { slots }) =
                    (level.refresh.take(), &level.state)
                else {
                    continue;
                };
                let pages = data.pages.len();
                uploads.push(PageUpload {
                    key,
                    slots: slots.clone(),
                    page_range: 0..pages,
                });
                level.data = Some(data);
                *budget = budget.saturating_sub(pages);
                written += pages;
            }
        }
        written
    }

    /// Exact page counts of the levels of `node` as `(depth, pages)`, known before any upload. The
    /// fit uses them instead of the estimate until the data itself arrives; they must equal the
    /// size `pack_level` gives. Sizes of nodes that leave the cut and hold no level are dropped
    /// eventually.
    pub fn set_level_sizes(&mut self, node: NodeId, sizes: &[(u8, usize)]) {
        for (depth, pages) in sizes {
            let key = self.key(node, *depth);
            let before = self.known_pages(key);
            self.sizes.insert(key, (*pages).max(1));
            if self.known_pages(key) != before {
                self.size_epoch += 1;
            }
        }
    }

    /// The size of a level when known: its data, else its exact size.
    pub(crate) fn known_pages(&self, key: LevelKey) -> Option<usize> {
        self.levels
            .get(&key)
            .and_then(Level::pages)
            .or_else(|| self.sizes.get(&key).copied())
    }

    /// Drops a resident level that is neither pinned nor drawn by the applied (or retained) cut
    /// (device loss, tests, memory pressure from outside). Its pages return to the free list two
    /// frames after it last drew; a still-wanted level is requested again by the next frame.
    /// Other levels stay (`false`).
    pub fn evict(&mut self, key: LevelKey) -> bool {
        let drawn = self
            .applied
            .iter()
            .any(|entry| entry.node == key.node && entry.required_levels().contains(&key.depth));
        if drawn || self.pinned.contains(&key) || !self.is_resident(key) {
            return false;
        }
        self.evict_resident(key, self.frame);
        true
    }

    /// Empties the pool (device lost, or the renderer dropped its pool): no residency, no
    /// requests, no cut. The caller's GPU side is gone and starts fresh, so every page slot is free
    /// at once (no `page_frees`, no delayed release) and the mask slots restart from 0, like a new
    /// renderer hands them out; the next applied cut rewrites every mask. Returns the events of
    /// the levels that left: `Evict` for every resident or uploading level, `Cancel` for every
    /// pending request, after any event still queued. Page sizes given by `set_level_sizes` stay:
    /// they describe the data, not the residency. The next frames request again what the cut
    /// needs.
    #[must_use]
    pub fn reset(&mut self) -> Vec<CellPoolEvent> {
        let mut keys: Vec<LevelKey> = self.levels.keys().copied().collect();
        keys.sort();
        for key in keys {
            let Some(level) = self.levels.remove(&key) else {
                continue;
            };
            self.events.push(match level.state {
                LevelState::Requested => CellPoolEvent::Cancel(key),
                LevelState::Uploading { .. } | LevelState::Resident { .. } => {
                    CellPoolEvent::Evict(key)
                }
            });
        }
        self.free = (0..self.config.capacity_pages).collect();
        self.pending.clear();
        self.size_epoch += 1;
        self.residency_epoch += 1;
        self.caps.clear();
        self.wanted_memo = None;
        self.real_memo = None;
        self.fallback_memo = None;
        self.hold.clear();
        self.wait_ages.clear();
        self.applied_cover.clear();
        self.overflow_frame = 0;
        self.applied_drawn.clear();
        self.applied_finest.clear();
        self.applied_span.clear();
        self.applied_run.clear();
        self.node_waits.clear();
        self.step_waits.clear();
        self.applied_d5.clear();
        self.old_released.clear();
        self.applied_digest = 0;
        self.carry = Carry::default();
        self.applied.clear();
        self.applied_covered = 0;
        self.applied_levels.clear();
        self.applied_pins.clear();
        self.pinned.clear();
        self.applied_index.nodes.clear();
        self.applied_index.cells.clear();
        self.applied_index.below.clear();
        self.applied_index.epoch += 1;
        self.mask_slots.clear();
        self.free_masks.clear();
        self.next_mask = 0;
        std::mem::take(&mut self.events)
    }

    /// Evicts a resident level: its pages are free at `last_active + FREE_DELAY` (never before
    /// `now`), or at `now` when it never drew. A level still uploading is not evicted but
    /// cancelled by `plan`, and its pages are free that same frame (nothing drew them).
    pub(super) fn evict_resident(&mut self, key: LevelKey, now: u64) {
        let before = self.known_pages(key);
        let Some(level) = self.levels.remove(&key) else {
            return;
        };
        if before != self.known_pages(key) {
            self.size_epoch += 1;
        }
        if let LevelState::Resident { slots } = level.state {
            self.residency_epoch += 1;
            let free_at = if level.last_active == 0 {
                now
            } else {
                (level.last_active + FREE_DELAY).max(now)
            };
            self.pending.push((free_at, slots));
        }
        self.events.push(CellPoolEvent::Evict(key));
    }

    pub(super) fn release(&mut self, frame: u64, page_frees: &mut Vec<u32>) {
        let mut kept = Vec::new();
        for (at, slots) in std::mem::take(&mut self.pending) {
            if at <= frame {
                page_frees.extend(&slots);
                self.free.extend(slots);
            } else {
                kept.push((at, slots));
            }
        }
        self.pending = kept;
    }

    pub(super) fn pending_pages(&self) -> usize {
        self.pending.iter().map(|(_, slots)| slots.len()).sum()
    }

    /// Pending pages that are released by `frame`.
    pub(super) fn pending_pages_by(&self, frame: u64) -> usize {
        self.pending
            .iter()
            .filter(|(at, _)| *at <= frame)
            .map(|(_, slots)| slots.len())
            .sum()
    }

    pub(super) fn allocated_pages(&self) -> usize {
        self.config.capacity_pages as usize - self.free.len() - self.pending_pages()
    }

    pub(super) fn usable(&self) -> u64 {
        usable_pages(self.config.capacity_pages)
    }

    pub(super) fn pages_of(&self, key: LevelKey, node: &PoolNode, grid: &CellGrid) -> usize {
        self.known_pages(key).unwrap_or_else(|| {
            let node_depth = representative_depth(grid, node.bounds).saturating_sub(3);
            estimate_pages(node.points, key.depth, node_depth)
        })
    }

    pub(super) fn alloc_mask(&mut self) -> u32 {
        self.free_masks.pop_first().unwrap_or_else(|| {
            self.next_mask += 1;
            self.next_mask - 1
        })
    }
}
