//! Residency of hierarchy pages (R5): which pages are grafted, how many bytes they account for,
//! and in which order they are dropped. The budget is separate from the decoded-block cache.

use viewer_core::{NodeId, SpatialNode};

/// A validated page ready to graft below `anchor`; `bytes` is its encoded size.
pub struct PageInsert {
    pub anchor: NodeId,
    /// Page number; the pages' nodes carry it in the high half of their ids.
    pub number: u32,
    pub nodes: Vec<SpatialNode>,
    /// Nodes of the page whose own pages are not loaded.
    pub unloaded: Vec<NodeId>,
    pub bytes: usize,
}

/// Called with the anchor of every page the dataset evicts, so the reader can forget its blocks.
pub type PageRelease = Box<dyn Fn(NodeId)>;

/// Default page budget: pages are at most 512 KB encoded and cost a few times that resident.
pub const DEFAULT_PAGE_BUDGET_BYTES: usize = 64 * 1024 * 1024;

pub(crate) struct PageResidency {
    release: PageRelease,
    budget_bytes: usize,
    used_bytes: usize,
    /// Grafted pages, least recently used first.
    order: Vec<(NodeId, u32, usize)>,
    evictions: u64,
}

impl PageResidency {
    pub(crate) fn new(release: PageRelease, budget_bytes: usize) -> Self {
        Self {
            release,
            budget_bytes,
            used_bytes: 0,
            order: Vec::new(),
            evictions: 0,
        }
    }

    pub(crate) fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    pub(crate) fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    pub(crate) fn pages(&self) -> usize {
        self.order.len()
    }

    pub(crate) fn evictions(&self) -> u64 {
        self.evictions
    }

    /// Whether `bytes` more fit without evicting.
    pub(crate) fn fits(&self, bytes: usize) -> bool {
        self.used_bytes.saturating_add(bytes) <= self.budget_bytes
    }

    /// Resident pages, least recently used first.
    pub(crate) fn lru(&self) -> Vec<(NodeId, u32)> {
        self.order.iter().map(|(a, n, _)| (*a, *n)).collect()
    }

    pub(crate) fn insert(&mut self, anchor: NodeId, number: u32, bytes: usize) {
        self.used_bytes += bytes;
        self.order.push((anchor, number, bytes));
    }

    /// Marks page `number` as the most recently used.
    pub(crate) fn touch(&mut self, number: u32) {
        if let Some(position) = self.order.iter().position(|(_, n, _)| *n == number)
            && position + 1 != self.order.len()
        {
            let entry = self.order.remove(position);
            self.order.push(entry);
        }
    }

    pub(crate) fn remove(&mut self, anchor: NodeId) {
        if let Some(position) = self.order.iter().position(|(id, _, _)| *id == anchor) {
            let (_, _, bytes) = self.order.remove(position);
            self.used_bytes -= bytes;
            self.evictions += 1;
            (self.release)(anchor);
        }
    }

    /// Bytes of the pages `pinned` rejected for eviction, for the budget error.
    pub(crate) fn bytes_of(&self, anchors: &[NodeId]) -> usize {
        self.order
            .iter()
            .filter(|(id, _, _)| anchors.contains(id))
            .map(|(_, _, bytes)| *bytes)
            .sum()
    }
}
