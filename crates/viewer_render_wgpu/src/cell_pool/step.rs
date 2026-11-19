//! Step state of a node and the step waits.

use super::{CellCutEntry, CellPool, NodeId, PrevState, STEP_WAIT_FRAMES};

impl CellPool {
    /// What `id` drew in the last applied cut and which of its octants gave up stepping.
    pub(super) fn prev_state(&self, id: NodeId) -> PrevState {
        let mut state = PrevState {
            drawn: self.applied_drawn.get(&id).copied().unwrap_or([None; 8]),
            timed_out: [false; 8],
        };
        for (o, timed_out) in state.timed_out.iter_mut().enumerate() {
            *timed_out = self
                .step_waits
                .get(&(id, o))
                .is_some_and(|(_, frames)| *frames >= STEP_WAIT_FRAMES);
        }
        state
    }

    /// Octants of `entries` (a cut about to be applied) that jump more than one level from what
    /// they drew because they timed out or their previous level is not resident any more.
    pub(super) fn step_hops(&self, entries: &[CellCutEntry]) -> u32 {
        let mut hops = 0;
        for entry in entries {
            let prev = self.prev_state(entry.node);
            for o in 0..8 {
                if let (Some(was), Some(now)) = (prev.drawn[o], entry.drawn[o])
                    && was.abs_diff(now) > 1
                    && (prev.timed_out[o] || !self.is_resident(self.key(entry.node, was)))
                {
                    hops += 1;
                }
            }
        }
        hops
    }
}
