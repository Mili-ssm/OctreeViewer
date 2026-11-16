use std::collections::{BTreeSet, HashMap, HashSet};

use thiserror::Error;

use crate::{
    BlockId, LodSelection, LodSelectionConfig, LodSelectionError, SpatialFrustum, SpatialHierarchy,
    SpatialNode,
};

/// Opaque identity for one scheduled block read. A cancelled ticket is never revived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestTicket {
    id: u64,
    block_id: BlockId,
}

impl RequestTicket {
    pub const fn id(self) -> u64 {
        self.id
    }

    pub const fn block_id(self) -> BlockId {
        self.block_id
    }
}

/// A block read released by the scheduler, with its current distance priority.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduledBlockRequest {
    ticket: RequestTicket,
    priority_distance: f64,
}

impl ScheduledBlockRequest {
    pub const fn ticket(self) -> RequestTicket {
        self.ticket
    }

    pub const fn priority_distance(self) -> f64 {
        self.priority_distance
    }
}

/// Results returned when a read ticket finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestCompletion {
    /// The block matched the ticket and remains selected by the current view.
    Delivered,
    /// The block was read, but its ticket was cancelled or is no longer selected.
    Discarded,
    /// The current request failed. Retry only after an explicit call to [`BlockRequestScheduler::retry`].
    Failed,
    /// The reader returned a block with an ID different from the ticket.
    UnexpectedBlock { expected: BlockId, actual: BlockId },
}

/// Observable lifecycle state for one block under the current view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestState {
    NotSelected,
    Queued,
    InFlight,
    CancelledInFlight,
    Delivered,
    Failed,
}

/// Plans visible block reads while leaving execution to the caller's runtime.
///
/// Create one scheduler per dataset. Call [`Self::cancel_all`] before replacing the
/// dataset. Active synchronous reads cannot be interrupted by this type; cancelled
/// tickets remain active until the caller reports their completion, and their results
/// are discarded.
pub struct BlockRequestScheduler {
    max_in_flight: usize,
    next_ticket_id: u64,
    pending: BTreeSet<PriorityKey>,
    pending_by_block: HashMap<BlockId, PriorityKey>,
    selected: HashSet<BlockId>,
    priorities: HashMap<BlockId, f64>,
    active: HashMap<u64, ActiveRequest>,
    active_by_block: HashMap<BlockId, u64>,
    completed: HashSet<BlockId>,
    failed: HashSet<BlockId>,
}

impl BlockRequestScheduler {
    pub fn new(max_in_flight: usize) -> Result<Self, SchedulerError> {
        if max_in_flight == 0 {
            return Err(SchedulerError::ZeroConcurrencyLimit);
        }
        Ok(Self {
            max_in_flight,
            next_ticket_id: 1,
            pending: BTreeSet::new(),
            pending_by_block: HashMap::new(),
            selected: HashSet::new(),
            priorities: HashMap::new(),
            active: HashMap::new(),
            active_by_block: HashMap::new(),
            completed: HashSet::new(),
            failed: HashSet::new(),
        })
    }

    /// Selects visible payloads, deduplicates their blocks and reconciles queued reads.
    ///
    /// Block priority is the distance from the camera to the closest node AABB that
    /// references it. Invalid camera coordinates or LOD input leave the current plan
    /// unchanged. The scheduler does not prefetch outside the selected visible cut.
    pub fn update_view(
        &mut self,
        hierarchy: &SpatialHierarchy,
        frustum: &SpatialFrustum,
        lod_config: LodSelectionConfig,
        camera_position: [f64; 3],
        projected_error_pixels: impl Fn(&SpatialNode) -> f64,
    ) -> Result<ViewUpdate, SchedulerError> {
        if camera_position
            .iter()
            .any(|coordinate| !coordinate.is_finite())
        {
            return Err(SchedulerError::InvalidCameraPosition(camera_position));
        }
        let selection = hierarchy.select_lod(frustum, lod_config, projected_error_pixels)?;
        let mut desired = HashMap::<BlockId, f64>::new();
        for node_id in selection.node_ids() {
            let node = hierarchy
                .node(*node_id)
                .expect("LOD selection refers to a hierarchy node");
            let distance = distance_to_bounds(camera_position, node);
            for block_id in node.blocks() {
                desired
                    .entry(*block_id)
                    .and_modify(|current| *current = current.min(distance))
                    .or_insert(distance);
            }
        }

        let cancelled = self.reconcile(desired);
        Ok(ViewUpdate {
            selection,
            cancelled,
        })
    }

    /// Schedules an explicit block set, for example the pending payloads from a
    /// budget-filtered transition plan. Duplicate IDs keep their nearest supplied
    /// priority. All priorities are validated before the current queue is changed.
    pub fn update_blocks(
        &mut self,
        requests: impl IntoIterator<Item = (BlockId, f64)>,
    ) -> Result<RequestUpdate, SchedulerError> {
        let mut desired = HashMap::<BlockId, f64>::new();
        for (block_id, distance) in requests {
            if distance.is_nan() || distance < 0.0 {
                return Err(SchedulerError::InvalidPriorityDistance { block_id, distance });
            }
            desired
                .entry(block_id)
                .and_modify(|current| *current = current.min(distance))
                .or_insert(distance);
        }
        Ok(RequestUpdate {
            cancelled: self.reconcile(desired),
        })
    }

    fn reconcile(&mut self, desired: HashMap<BlockId, f64>) -> Vec<RequestTicket> {
        let desired_ids = desired.keys().copied().collect::<HashSet<_>>();
        let obsolete_pending = self
            .pending_by_block
            .keys()
            .filter(|block_id| !desired_ids.contains(block_id))
            .copied()
            .collect::<Vec<_>>();
        for block_id in obsolete_pending {
            self.remove_pending(block_id);
        }

        let mut cancelled = Vec::new();
        for request in self.active.values_mut() {
            if request.current && !desired_ids.contains(&request.ticket.block_id) {
                request.current = false;
                cancelled.push(request.ticket);
            }
        }
        cancelled.sort_unstable_by_key(|ticket| ticket.id);

        self.completed
            .retain(|block_id| desired_ids.contains(block_id));
        self.failed
            .retain(|block_id| desired_ids.contains(block_id));
        self.selected = desired_ids;
        self.priorities.clone_from(&desired);
        for (block_id, distance) in desired {
            if self.active_by_block.contains_key(&block_id)
                || self.completed.contains(&block_id)
                || self.failed.contains(&block_id)
            {
                continue;
            }
            self.insert_pending(block_id, distance);
        }

        cancelled
    }

    /// Releases the highest-priority pending request if a concurrency slot is free.
    pub fn take_next(&mut self) -> Result<Option<ScheduledBlockRequest>, SchedulerError> {
        if self.active.len() >= self.max_in_flight {
            return Ok(None);
        }
        let Some(key) = self.pending.iter().next().copied() else {
            return Ok(None);
        };
        self.admit(key)
    }

    /// Releases the highest-priority pending request among blocks already admitted by an
    /// external byte budget. This lets callers avoid reserving an operation slot while waiting
    /// for memory quota.
    pub fn take_next_admitted(
        &mut self,
        admitted_blocks: &[BlockId],
    ) -> Result<Option<ScheduledBlockRequest>, SchedulerError> {
        if self.active.len() >= self.max_in_flight {
            return Ok(None);
        }
        let Some(key) = self
            .pending
            .iter()
            .find(|key| admitted_blocks.contains(&key.block_id))
            .copied()
        else {
            return Ok(None);
        };
        self.admit(key)
    }

    fn admit(&mut self, key: PriorityKey) -> Result<Option<ScheduledBlockRequest>, SchedulerError> {
        let ticket_id = self.next_ticket_id;
        let next_id = ticket_id
            .checked_add(1)
            .ok_or(SchedulerError::TicketIdOverflow)?;
        self.pending.remove(&key);
        self.pending_by_block.remove(&key.block_id);
        self.next_ticket_id = next_id;
        let ticket = RequestTicket {
            id: ticket_id,
            block_id: key.block_id,
        };
        self.active.insert(
            ticket.id,
            ActiveRequest {
                ticket,
                current: true,
            },
        );
        self.active_by_block.insert(ticket.block_id, ticket.id);
        Ok(Some(ScheduledBlockRequest {
            ticket,
            priority_distance: key.distance,
        }))
    }

    /// Pending blocks in scheduler priority order.
    pub fn pending_blocks(&self) -> Vec<BlockId> {
        self.pending.iter().map(|key| key.block_id).collect()
    }

    /// Reports a read result and releases its concurrency slot.
    pub fn complete(
        &mut self,
        ticket: RequestTicket,
        result: Result<BlockId, ()>,
    ) -> Result<RequestCompletion, SchedulerError> {
        let active = self
            .active
            .remove(&ticket.id)
            .ok_or(SchedulerError::UnknownTicket(ticket.id))?;
        if active.ticket != ticket {
            self.active.insert(active.ticket.id, active);
            return Err(SchedulerError::TicketMismatch(ticket.id));
        }
        self.active_by_block.remove(&ticket.block_id);

        let completion = if !active.current || !self.selected.contains(&ticket.block_id) {
            RequestCompletion::Discarded
        } else {
            match result {
                Err(()) => {
                    self.failed.insert(ticket.block_id);
                    RequestCompletion::Failed
                }
                Ok(actual) if actual != ticket.block_id => {
                    self.failed.insert(ticket.block_id);
                    RequestCompletion::UnexpectedBlock {
                        expected: ticket.block_id,
                        actual,
                    }
                }
                Ok(_) => {
                    self.completed.insert(ticket.block_id);
                    RequestCompletion::Delivered
                }
            }
        };

        if completion == RequestCompletion::Discarded
            && !self.completed.contains(&ticket.block_id)
            && let Some(distance) = self.current_priority(ticket.block_id)
        {
            self.insert_pending(ticket.block_id, distance);
        }
        Ok(completion)
    }

    /// Explicitly queues a previously failed block if the current view still needs it.
    pub fn retry(&mut self, block_id: BlockId) -> bool {
        if !self.selected.contains(&block_id)
            || self.active_by_block.contains_key(&block_id)
            || self.completed.contains(&block_id)
            || !self.failed.remove(&block_id)
        {
            return false;
        }
        if let Some(key) = self.pending_by_block.get(&block_id).copied() {
            self.pending.remove(&key);
        }
        self.pending_by_block.remove(&block_id);
        self.insert_pending(block_id, self.priorities[&block_id]);
        true
    }

    /// Marks a selected request failed before it takes an in-flight ticket, for example when
    /// admission discovers that a single request cannot fit the byte budget.
    pub fn fail(&mut self, block_id: BlockId) -> bool {
        if !self.selected.contains(&block_id)
            || self.active_by_block.contains_key(&block_id)
            || self.completed.contains(&block_id)
        {
            return false;
        }
        self.remove_pending(block_id);
        self.failed.insert(block_id)
    }

    /// Forgets a delivered block after its decoded payload leaves the cache.
    /// If the current view still needs it, the block becomes pending again.
    pub fn invalidate_completed(&mut self, block_id: BlockId) -> bool {
        if !self.completed.remove(&block_id) {
            return false;
        }
        if self.selected.contains(&block_id)
            && !self.active_by_block.contains_key(&block_id)
            && !self.pending_by_block.contains_key(&block_id)
            && !self.failed.contains(&block_id)
            && let Some(distance) = self.current_priority(block_id)
        {
            self.insert_pending(block_id, distance);
        }
        true
    }

    /// Marks a currently selected block as already resident, removing queued rereads.
    pub fn mark_cached(&mut self, block_id: BlockId) -> bool {
        if !self.selected.contains(&block_id) {
            return false;
        }
        self.remove_pending(block_id);
        self.failed.remove(&block_id);
        self.completed.insert(block_id);
        true
    }

    /// Cancels queued work and marks all active work stale, for example on dataset close.
    pub fn cancel_all(&mut self) -> Vec<RequestTicket> {
        self.pending.clear();
        self.pending_by_block.clear();
        self.selected.clear();
        self.priorities.clear();
        self.completed.clear();
        self.failed.clear();
        let mut cancelled = Vec::new();
        for request in self.active.values_mut() {
            if request.current {
                request.current = false;
                cancelled.push(request.ticket);
            }
        }
        cancelled.sort_unstable_by_key(|ticket| ticket.id);
        cancelled
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Includes cancelled reads that the caller has not finished yet.
    pub fn in_flight_count(&self) -> usize {
        self.active.len()
    }

    pub fn cancelled_in_flight_count(&self) -> usize {
        self.active
            .values()
            .filter(|request| !request.current)
            .count()
    }

    pub fn failed_count(&self) -> usize {
        self.failed.len()
    }

    pub fn completed_count(&self) -> usize {
        self.completed.len()
    }

    pub fn request_state(&self, block_id: BlockId) -> RequestState {
        if let Some(request_id) = self.active_by_block.get(&block_id) {
            return if self
                .active
                .get(request_id)
                .is_some_and(|request| !request.current)
            {
                RequestState::CancelledInFlight
            } else {
                RequestState::InFlight
            };
        }
        if self.pending_by_block.contains_key(&block_id) {
            return RequestState::Queued;
        }
        if self.completed.contains(&block_id) {
            return RequestState::Delivered;
        }
        if self.failed.contains(&block_id) {
            return RequestState::Failed;
        }
        RequestState::NotSelected
    }

    pub fn ticket_is_current(&self, ticket: RequestTicket) -> bool {
        self.active
            .get(&ticket.id)
            .is_some_and(|active| active.ticket == ticket && active.current)
            && self.selected.contains(&ticket.block_id)
    }

    fn insert_pending(&mut self, block_id: BlockId, distance: f64) {
        let key = PriorityKey { distance, block_id };
        if let Some(previous) = self.pending_by_block.insert(block_id, key) {
            self.pending.remove(&previous);
        }
        self.pending.insert(key);
    }

    fn remove_pending(&mut self, block_id: BlockId) {
        if let Some(key) = self.pending_by_block.remove(&block_id) {
            self.pending.remove(&key);
        }
    }

    fn current_priority(&self, block_id: BlockId) -> Option<f64> {
        self.priorities.get(&block_id).copied()
    }
}

/// Result of reconciling a new camera/frustum selection.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewUpdate {
    selection: LodSelection,
    cancelled: Vec<RequestTicket>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestUpdate {
    cancelled: Vec<RequestTicket>,
}

impl RequestUpdate {
    pub fn cancelled(&self) -> &[RequestTicket] {
        &self.cancelled
    }
}

impl ViewUpdate {
    pub const fn selection(&self) -> &LodSelection {
        &self.selection
    }

    pub fn cancelled(&self) -> &[RequestTicket] {
        &self.cancelled
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum SchedulerError {
    #[error("maximum in-flight request count must be greater than zero")]
    ZeroConcurrencyLimit,
    #[error("camera position must be finite, got {0:?}")]
    InvalidCameraPosition([f64; 3]),
    #[error(
        "request priority for block {block_id:?} must be non-negative and not NaN, got {distance}"
    )]
    InvalidPriorityDistance { block_id: BlockId, distance: f64 },
    #[error("request ticket ID overflow")]
    TicketIdOverflow,
    #[error("request ticket {0} is not active")]
    UnknownTicket(u64),
    #[error("request ticket {0} does not match its active request")]
    TicketMismatch(u64),
    #[error(transparent)]
    Lod(#[from] LodSelectionError),
}

struct ActiveRequest {
    ticket: RequestTicket,
    current: bool,
}

#[derive(Debug, Clone, Copy)]
struct PriorityKey {
    distance: f64,
    block_id: BlockId,
}

impl PartialEq for PriorityKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for PriorityKey {}

impl PartialOrd for PriorityKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PriorityKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.distance
            .total_cmp(&other.distance)
            .then_with(|| self.block_id.cmp(&other.block_id))
    }
}

fn distance_to_bounds(camera: [f64; 3], node: &SpatialNode) -> f64 {
    let bounds = node.bounds();
    let distance: [f64; 3] = std::array::from_fn(|axis| {
        if camera[axis] < bounds.min()[axis] {
            bounds.min()[axis] - camera[axis]
        } else if camera[axis] > bounds.max()[axis] {
            camera[axis] - bounds.max()[axis]
        } else {
            0.0
        }
    });
    distance[0].hypot(distance[1]).hypot(distance[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_admission_skips_unadmitted_priority_without_using_a_slot() {
        let mut scheduler = BlockRequestScheduler::new(2).unwrap();
        scheduler
            .update_blocks([
                (BlockId::new(3), 0.0),
                (BlockId::new(2), 1.0),
                (BlockId::new(1), 2.0),
            ])
            .unwrap();

        let admitted = scheduler
            .take_next_admitted(&[BlockId::new(2), BlockId::new(1)])
            .unwrap()
            .unwrap();
        assert_eq!(admitted.ticket().block_id(), BlockId::new(2));
        assert_eq!(scheduler.in_flight_count(), 1);
        assert_eq!(
            scheduler.pending_blocks(),
            vec![BlockId::new(3), BlockId::new(1)]
        );
    }

    #[test]
    fn failed_selected_blocks_need_an_explicit_retry() {
        let mut scheduler = BlockRequestScheduler::new(1).unwrap();
        let block_id = BlockId::new(7);
        scheduler.update_blocks([(block_id, 0.0)]).unwrap();
        let ticket = scheduler.take_next().unwrap().unwrap().ticket();
        assert_eq!(
            scheduler.complete(ticket, Err(())).unwrap(),
            RequestCompletion::Failed
        );
        assert_eq!(scheduler.pending_blocks(), [] as [BlockId; 0]);
        assert!(scheduler.retry(block_id));
        assert_eq!(scheduler.pending_blocks(), vec![block_id]);
    }
}
