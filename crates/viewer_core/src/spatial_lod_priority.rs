use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
    mem::size_of,
};

use super::{
    HierarchySemantics, LodSelection, LodSelectionConfig, LodSelectionError, NodeId,
    SpatialFrustum, SpatialHierarchy, SpatialNode,
};

/// Default additional priority storage, separate from traversal and output allocations.
pub const DEFAULT_LOD_PRIORITY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LodPriorityFallback {
    /// A validated single-chain hierarchy has no competing priorities.
    LinearHierarchy,
    ScratchLimit,
    AllocationFailed,
    /// Restart the original selector to preserve validation/error precedence.
    Validation,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LodPriorityStats {
    /// Maximum observed heap + deferred-vector capacity times candidate size.
    /// Excludes common selection/traversal buffers and allocator overhead/transients.
    pub peak_priority_bytes: usize,
    pub fallback: Option<LodPriorityFallback>,
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    id: NodeId,
    error: f64,
    children_count: u64,
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.error
            .partial_cmp(&other.error)
            .expect("candidate errors are validated")
            .then_with(|| other.id.cmp(&self.id))
    }
}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Candidate {}

#[derive(Default)]
struct Scratch {
    pending: Vec<NodeId>,
    children: Vec<NodeId>,
}

const NO_SLOT: usize = usize::MAX;

struct Slot {
    id: NodeId,
    next: usize,
    expanded: bool,
}

/// Selection order as an indexed singly linked list, so expanding a candidate costs
/// O(children) instead of the O(frontier) `position` + `splice` of a flat vector.
///
/// A candidate stays in place as an `expanded` tombstone with its children linked right
/// after it. Additive output keeps tombstones (parents stay selected); the frontier and
/// the replacement output skip them. Both equal the flat-vector splices they replace.
struct Cut {
    slots: Vec<Slot>,
    index: HashMap<NodeId, usize>,
}

impl Cut {
    fn new(initial: &[NodeId]) -> Self {
        let mut cut = Self {
            slots: Vec::with_capacity(initial.len()),
            index: HashMap::with_capacity(initial.len()),
        };
        for (position, id) in initial.iter().enumerate() {
            cut.slots.push(Slot {
                id: *id,
                next: if position + 1 == initial.len() {
                    NO_SLOT
                } else {
                    position + 1
                },
                expanded: false,
            });
            cut.index.insert(*id, position);
        }
        cut
    }

    fn expand(&mut self, id: NodeId, children: &[NodeId]) {
        let slot = self.index[&id];
        self.slots[slot].expanded = true;
        let mut next = self.slots[slot].next;
        for child in children.iter().rev() {
            self.slots.push(Slot {
                id: *child,
                next,
                expanded: false,
            });
            next = self.slots.len() - 1;
            self.index.insert(*child, next);
        }
        self.slots[slot].next = next;
    }

    /// Slots in selection order; slot 0 is always the head (it is never relinked).
    fn ordered(&self) -> impl Iterator<Item = &Slot> {
        let mut current = if self.slots.is_empty() { NO_SLOT } else { 0 };
        std::iter::from_fn(move || {
            let slot = self.slots.get(current)?;
            current = slot.next;
            Some(slot)
        })
    }

    fn frontier(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.ordered()
            .filter(|slot| !slot.expanded)
            .map(|slot| slot.id)
    }
}

struct Priorities<'a> {
    ready: BinaryHeap<Candidate>,
    blocked: Vec<Candidate>,
    budget: usize,
    max_growth: u64,
    stats: &'a mut LodPriorityStats,
}

impl Priorities<'_> {
    fn reserve_one(&mut self, blocked: bool) -> Result<(), LodPriorityFallback> {
        let (len, capacity, other) = if blocked {
            (
                self.blocked.len(),
                self.blocked.capacity(),
                self.ready.capacity(),
            )
        } else {
            (
                self.ready.len(),
                self.ready.capacity(),
                self.blocked.capacity(),
            )
        };
        if len < capacity {
            return Ok(());
        }
        let allowed = (self.budget / size_of::<Candidate>()).saturating_sub(other);
        let target = capacity.saturating_mul(2).max(4).min(allowed);
        if target <= capacity {
            return Err(LodPriorityFallback::ScratchLimit);
        }
        let result = if blocked {
            self.blocked.try_reserve_exact(target - len)
        } else {
            self.ready.try_reserve_exact(target - len)
        };
        result.map_err(|_| LodPriorityFallback::AllocationFailed)?;
        let bytes = self
            .ready
            .capacity()
            .checked_add(self.blocked.capacity())
            .and_then(|capacity| capacity.checked_mul(size_of::<Candidate>()))
            .ok_or(LodPriorityFallback::ScratchLimit)?;
        self.stats.peak_priority_bytes = self.stats.peak_priority_bytes.max(bytes);
        if bytes > self.budget {
            return Err(LodPriorityFallback::ScratchLimit);
        }
        Ok(())
    }

    fn push(&mut self, candidate: Candidate) -> Result<(), LodPriorityFallback> {
        self.reserve_one(false)?;
        self.ready.push(candidate);
        Ok(())
    }

    fn activate(
        &mut self,
        hierarchy: &SpatialHierarchy,
        id: NodeId,
        frustum: &SpatialFrustum,
        tolerance: f64,
        project: &impl Fn(&SpatialNode) -> f64,
        scratch: &mut Scratch,
    ) -> Result<(), LodPriorityFallback> {
        let node = &hierarchy.nodes[&id];
        let error = checked_error(project(&node.metadata))?;
        if error <= tolerance {
            return Ok(());
        }
        hierarchy.visible_frontier_into(
            node.children.iter().copied(),
            frustum,
            &mut scratch.pending,
            &mut scratch.children,
        );
        if scratch.children.is_empty() {
            return Ok(());
        }
        let children_count = hierarchy
            .count_points(&scratch.children)
            .map_err(|_| LodPriorityFallback::Validation)?;
        let growth = match hierarchy.semantics {
            HierarchySemantics::Replacement => {
                children_count.saturating_sub(node.metadata.point_count)
            }
            HierarchySemantics::Additive => children_count,
        };
        // Retired candidates can keep this conservative maximum high: safe fallback beats
        // choosing before an overflow the original full-frontier scan would have reported.
        self.max_growth = self.max_growth.max(growth);
        self.push(Candidate {
            id,
            error,
            children_count,
        })
    }
}

fn checked_error(error: f64) -> Result<f64, LodPriorityFallback> {
    if error.is_nan() || error < 0.0 {
        Err(LodPriorityFallback::Validation)
    } else {
        Ok(error)
    }
}

fn candidate_points(hierarchy: &SpatialHierarchy, count: u64, candidate: Candidate) -> Option<u64> {
    match hierarchy.semantics {
        HierarchySemantics::Replacement => count
            .checked_sub(hierarchy.nodes[&candidate.id].metadata.point_count)
            .and_then(|count| count.checked_add(candidate.children_count)),
        HierarchySemantics::Additive => count.checked_add(candidate.children_count),
    }
}

impl SpatialHierarchy {
    /// Selects with incremental priorities for a projector stable per node during this call.
    ///
    /// Unlike `select_lod`, callback count/order is unspecified, including a possible restart.
    /// Side effects must not change projected values. The additional heap/deferred storage is
    /// budgeted by vector capacities; zero disables the optimization. Common traversal/output
    /// allocations, allocator overhead and transient reservation peaks are not bounded here.
    /// Failed reservations or ambiguous validation/overflow restart the original selector after
    /// dropping the attempt. Expansion is O(children); only releasing blocked candidates after a
    /// point-count drop still scans them linearly.
    pub fn select_lod_stable_projector(
        &self,
        frustum: &SpatialFrustum,
        config: LodSelectionConfig,
        priority_scratch_budget_bytes: usize,
        project: impl Fn(&SpatialNode) -> f64,
    ) -> Result<(LodSelection, LodPriorityStats), LodSelectionError> {
        let mut stats = LodPriorityStats::default();
        let attempt = if priority_scratch_budget_bytes == 0 {
            Err(LodPriorityFallback::ScratchLimit)
        } else if usize::try_from(self.stats.max_level)
            .ok()
            .and_then(|level| level.checked_add(1))
            == Some(self.stats.node_count)
        {
            // One node at every depth accounts for the whole validated tree.
            // A chain has no competing priorities and measured heap overhead dominates.
            Err(LodPriorityFallback::LinearHierarchy)
        } else {
            self.try_select_with_priorities(
                frustum,
                config,
                priority_scratch_budget_bytes,
                &project,
                &mut stats,
            )
        };
        match attempt {
            Ok(selection) => Ok((selection, stats)),
            Err(reason) => {
                stats.fallback = Some(reason);
                self.select_lod(frustum, config, project)
                    .map(|selection| (selection, stats))
            }
        }
    }

    fn try_select_with_priorities(
        &self,
        frustum: &SpatialFrustum,
        config: LodSelectionConfig,
        budget: usize,
        project: &impl Fn(&SpatialNode) -> f64,
        stats: &mut LodPriorityStats,
    ) -> Result<LodSelection, LodPriorityFallback> {
        let mut scratch = Scratch::default();
        let mut initial = Vec::new();
        self.visible_frontier_into([self.root], frustum, &mut scratch.pending, &mut initial);
        let mut cut = Cut::new(&initial);
        let mut point_count = self
            .count_points(&initial)
            .map_err(|_| LodPriorityFallback::Validation)?;
        if point_count > config.point_budget {
            return Err(LodPriorityFallback::Validation);
        }
        let mut priorities = Priorities {
            ready: BinaryHeap::new(),
            blocked: Vec::new(),
            budget,
            max_growth: 0,
            stats,
        };
        for id in &initial {
            priorities.activate(
                self,
                *id,
                frustum,
                config.max_error_pixels,
                project,
                &mut scratch,
            )?;
        }
        let mut winner_children = Vec::new();
        loop {
            if point_count.checked_add(priorities.max_growth).is_none() {
                return Err(LodPriorityFallback::Validation);
            }
            let Some(candidate) = priorities.ready.pop() else {
                break;
            };
            let new_count = candidate_points(self, point_count, candidate)
                .ok_or(LodPriorityFallback::Validation)?;
            if new_count > config.point_budget {
                priorities.reserve_one(true)?;
                priorities.blocked.push(candidate);
                continue;
            }
            self.visible_frontier_into(
                self.nodes[&candidate.id].children.iter().copied(),
                frustum,
                &mut scratch.pending,
                &mut winner_children,
            );
            cut.expand(candidate.id, &winner_children);
            if new_count < point_count {
                // Only candidates that now fit can be selected next; the others would be
                // popped and re-blocked without effect, so they stay parked. A candidate
                // whose total overflows is released so the usual validation fallback fires.
                let mut parked = 0;
                while parked < priorities.blocked.len() {
                    let blocked = priorities.blocked[parked];
                    if candidate_points(self, new_count, blocked)
                        .is_none_or(|count| count <= config.point_budget)
                    {
                        priorities.blocked.swap_remove(parked);
                        priorities.push(blocked)?;
                    } else {
                        parked += 1;
                    }
                }
            }
            point_count = new_count;
            for id in &winner_children {
                priorities.activate(
                    self,
                    *id,
                    frustum,
                    config.max_error_pixels,
                    project,
                    &mut scratch,
                )?;
            }
        }
        // One final scan preserves diagnostic order without storing per-leaf priorities.
        let mut budget_limited = Vec::new();
        let mut resolution_limited = Vec::new();
        let mut max_error_pixels: f64 = 0.0;
        let selected = match self.semantics {
            HierarchySemantics::Replacement => cut.frontier().collect::<Vec<_>>(),
            HierarchySemantics::Additive => cut.ordered().map(|slot| slot.id).collect(),
        };
        for id in cut.frontier() {
            let error = checked_error(project(&self.nodes[&id].metadata))?;
            max_error_pixels = max_error_pixels.max(error);
            if error <= config.max_error_pixels {
                continue;
            }
            self.visible_frontier_into(
                self.nodes[&id].children.iter().copied(),
                frustum,
                &mut scratch.pending,
                &mut scratch.children,
            );
            if scratch.children.is_empty() {
                resolution_limited.push(id);
            } else {
                let children_count = self
                    .count_points(&scratch.children)
                    .map_err(|_| LodPriorityFallback::Validation)?;
                let count = candidate_points(
                    self,
                    point_count,
                    Candidate {
                        id,
                        error,
                        children_count,
                    },
                )
                .ok_or(LodPriorityFallback::Validation)?;
                if count > config.point_budget {
                    budget_limited.push(id);
                }
            }
        }
        Ok(LodSelection {
            node_ids: selected,
            point_count,
            point_budget: config.point_budget,
            max_error_pixels,
            budget_limited,
            resolution_limited,
        })
    }
}
