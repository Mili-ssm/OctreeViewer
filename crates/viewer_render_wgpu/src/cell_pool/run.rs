//! The single active run of a node (plan task 10, amendments B1, B2, B6, B7, B11).
//!
//! `node_run` is pure: plain depths in, plain depths out. `CellPool::fill_node_runs` computes it
//! for the planned cut into the `node_*` fields of `CellCutEntry`; nobody reads them yet.

use super::{
    CellCutEntry, CellGrid, CellPool, FastMap, NodeId, PoolNode, STEP_WAIT_FRAMES, capped,
    capped_range, cell_depth_range,
};

/// What `node_run` needs of a node.
pub(super) struct RunInput<'a> {
    /// Fine end of the run the node drew before, WITHOUT its D5 depth (B3), or `before` (B5, C2)
    /// for a node that did not draw. `None`: no history, the target stands in for it.
    pub prev: Option<u8>,
    /// `[lo, hi]` of the node (`CellCutEntry::range`).
    pub range: (u8, u8),
    /// The finest depth the node can reach: `min(hi, finest stored or buildable depth, fit cap)`.
    pub target: u8,
    /// Usable depths of the node inside its cell span, ascending. Contract (B3): the D5 depth is
    /// NOT an input unless it is contiguous with the rest (then it belongs to the same run
    /// anyway), so the anchor never lands on it; see `without_detached_d5`.
    pub usable: &'a [u8],
    /// The pending step that has waited `STEP_WAIT_FRAMES` frames (B6, C1): only that level may
    /// make the anchor hop, so a wait never outlives the step it was counted for.
    pub waited_step: Option<u8>,
    /// The coarsest depth any octant of the node wants (B7).
    pub coarsest_wish: Option<u8>,
}

/// The node's run. `anchor` and `run` are `None` only when no depth is usable (A4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct NodeRun {
    pub anchor: Option<u8>,
    pub run: Option<(u8, u8)>,
    /// The run stops short of the target, or the node draws finer than its coarsest wish.
    pub stage: bool,
    /// The level that extends the run and is not usable yet (C1): `run.hi + 1` while the run
    /// stops short of the target, `run.lo - 1` while the coarsest wish is below `run.lo`. When
    /// both ends need one, see `pending_step`.
    pub step: Option<u8>,
}

/// The usable depth closest to `from` within `[min, max]` (ties: the coarser one).
fn closest(usable: &[u8], from: u8, min: u8, max: u8) -> Option<u8> {
    usable
        .iter()
        .copied()
        .filter(|depth| (min..=max).contains(depth))
        .min_by_key(|depth| (depth.abs_diff(from), *depth))
}

/// The maximal run of contiguous usable depths that contains `anchor`.
fn run_of(usable: &[u8], anchor: u8) -> (u8, u8) {
    let (mut lo, mut hi) = (anchor, anchor);
    while lo > 0 && usable.contains(&(lo - 1)) {
        lo -= 1;
    }
    while hi < u8::MAX && usable.contains(&(hi + 1)) {
        hi += 1;
    }
    (lo, hi)
}

/// B1 clip to `[min(lo, anchor), max(hi, anchor)]`, as `cell_active_depths` does today (`lo` and
/// `hi` widened by what is drawn). The text of B1 says `min(lo, run.lo)`, which would never clip
/// below `lo`: the run is cut at `lo` unless the anchor itself lies below it.
fn clipped(run: (u8, u8), anchor: u8, (lo, hi): (u8, u8)) -> (u8, u8) {
    (run.0.max(lo.min(anchor)), run.1.min(hi.max(anchor)))
}

/// The level each end of `run` waits for (`(fine, coarse)`), if any: the fine end while the run
/// stops short of `target` (the level above is not usable, else the run would include it), the
/// coarse end while the coarsest wish is below `run.lo` and the level under it is not usable.
fn steps(usable: &[u8], run: (u8, u8), target: u8, wish: Option<u8>) -> (Option<u8>, Option<u8>) {
    let fine = (run.1 < target && !usable.contains(&(run.1 + 1))).then_some(run.1 + 1);
    let coarse =
        (run.0 > 0 && wish.is_some_and(|wish| wish < run.0) && !usable.contains(&(run.0 - 1)))
            .then_some(run.0 - 1);
    (fine, coarse)
}

/// The step `node_run` reports and may hop for when both ends wait (C1): the fine end first while
/// the view refines (`prev <= target`, or no history), the coarse end first when it coarsens
/// (`prev > target`), because the end that was drawn last is the one the view is moving. The
/// other end gets its turn once this one resolves.
fn pending_step(fine: Option<u8>, coarse: Option<u8>, refining: bool) -> Option<(bool, u8)> {
    let fine = fine.map(|step| (true, step));
    let coarse = coarse.map(|step| (false, step));
    if refining {
        fine.or(coarse)
    } else {
        coarse.or(fine)
    }
}

pub(super) fn node_run(input: &RunInput<'_>) -> NodeRun {
    let RunInput {
        prev,
        range,
        target,
        usable,
        waited_step,
        coarsest_wish,
    } = *input;
    // B2: closest to `prev` towards the target, else closest overall.
    let from = prev.unwrap_or(target);
    let (min, max) = (from.min(target), from.max(target));
    let Some(mut anchor) =
        closest(usable, from, min, max).or_else(|| closest(usable, from, 0, u8::MAX))
    else {
        return NodeRun::default();
    };
    let refining = from <= target;
    let mut run = clipped(run_of(usable, anchor), anchor, range);
    let (mut fine, mut coarse) = steps(usable, run, target, coarsest_wish);
    // B6, C1: the level the node waits for never arrives; hop to the nearest usable depth beyond
    // it, towards the target (never past it) or towards the coarsest wish (never past it): still
    // one run.
    if let Some((is_fine, step)) = pending_step(fine, coarse, refining)
        && waited_step == Some(step)
    {
        let hop = if is_fine {
            usable
                .iter()
                .copied()
                .filter(|depth| *depth > run.1 && *depth <= target)
                .min()
        } else {
            usable
                .iter()
                .copied()
                .filter(|depth| *depth < run.0 && coarsest_wish.is_none_or(|wish| *depth >= wish))
                .max()
        };
        if let Some(hop) = hop {
            anchor = hop;
            run = clipped(run_of(usable, anchor), anchor, range);
            (fine, coarse) = steps(usable, run, target, coarsest_wish);
        }
    }
    let stage = run.1 < target || coarsest_wish.is_some_and(|wish| wish < run.0);
    NodeRun {
        anchor: Some(anchor),
        run: Some(run),
        stage,
        step: pending_step(fine, coarse, refining).map(|(_, step)| step),
    }
}

/// `usable` without the D5 depth `d5` when no other usable depth is next to it: a detached D5
/// depth is a part apart that the emission adds (B3), and as an input it would let a hop land on
/// it. Next to a usable depth it is part of that run and stays; so does a D5 depth that is the
/// only usable one, because the node has nothing else to anchor on (A4).
fn without_detached_d5(usable: Vec<u8>, d5: Option<u8>) -> Vec<u8> {
    let Some(d5) = d5 else {
        return usable;
    };
    let next_to = |depth: u8| usable.contains(&depth);
    let attached = (d5 > 0 && next_to(d5 - 1)) || next_to(d5.saturating_add(1));
    if attached || usable == [d5] {
        usable
    } else {
        usable.into_iter().filter(|depth| *depth != d5).collect()
    }
}

impl CellPool {
    /// The `prev` of `node_run`: the finest depth that drew over the node's footprint before, from
    /// the first source that has a history. None of them counts the D5 depth (B3). In order: the
    /// fine end of the node's own applied run; the finest of its own applied per-octant depths
    /// other than D5 when it drew but has no run; the shared `before` of the hold (B5); the finest depth of the entering seed today's
    /// per-octant machinery starts from (`prev_state_seeded`). The last one matters because
    /// `before` is empty when the hold's cover rule finds no covering cell of the current cut
    /// (`Cover::Nothing`), while the seed reads the nearest applied ancestor (or descendants) and
    /// today's draw starts from it.
    pub(super) fn node_prev(
        &self,
        id: NodeId,
        node: &PoolNode,
        cap: Option<&u8>,
        before: &FastMap<NodeId, u8>,
        grid: &CellGrid,
    ) -> Option<u8> {
        self.applied_run
            .get(&id)
            .map(|(_, top)| *top)
            .or_else(|| {
                self.applied_drawn.get(&id).and_then(|drawn| {
                    let d5 = self.applied_d5.get(&id);
                    drawn
                        .iter()
                        .flatten()
                        .filter(|depth| Some(*depth) != d5)
                        .max()
                        .copied()
                })
            })
            .or_else(|| before.get(&id).copied())
            .or_else(|| {
                let buildable = capped_range(node.buildable, cap);
                self.prev_state_seeded(id, node.bounds, buildable, grid)
                    .drawn
                    .iter()
                    .flatten()
                    .max()
                    .copied()
            })
    }

    /// Fills the `node_*` fields of `entries` (the real cut, before it is applied). `prev` is what
    /// was drawn over the node's footprint before (`node_prev`). `given` is the resident list
    /// the cut used for each node (after hold, cap and `finer_drawn`; A4). The usable depths are
    /// those of that list `cell_active_depths` accepts over the node's cell span, less a detached D5 depth
    /// (`without_detached_d5`). The target is `min(hi, finest drawable stored or
    /// buildable depth under the fit cap)`.
    pub(super) fn fill_node_runs(
        &self,
        entries: &mut [CellCutEntry],
        given: &FastMap<NodeId, Vec<u8>>,
        before: &FastMap<NodeId, u8>,
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
    ) {
        for entry in entries {
            let (Some((lo, hi)), Some(node)) = (entry.range, nodes.get(&entry.node)) else {
                continue;
            };
            let id = entry.node;
            let node_depth = Self::node_depth(grid, node.bounds);
            let (span_lo, span_hi) = cell_depth_range(node_depth);
            let usable: Vec<u8> = (span_lo..=span_hi)
                .filter(|depth| {
                    given.get(&id).is_some_and(|list| list.contains(depth))
                        && self.is_usable(self.key(id, *depth))
                })
                .collect();
            let d5 = entry
                .mask
                .iter()
                .any(|word| *word != 0)
                .then(|| self.d5_resident(id, node, grid))
                .flatten();
            let usable = without_detached_d5(usable, d5);
            let cap = self.caps.get(&id);
            let (b_lo, b_hi) = capped_range(node.buildable, cap);
            let finest = self
                .drawable_stored(&capped(&node.stored, cap), grid, node.bounds)
                .into_iter()
                .chain((b_lo.max(span_lo) <= b_hi.min(span_hi)).then_some(b_hi.min(span_hi)))
                .max();
            let Some(target) = finest.map(|finest| finest.min(hi)) else {
                continue;
            };
            let out = node_run(&RunInput {
                prev: self.node_prev(id, node, cap, before, grid),
                range: (lo, hi),
                target,
                usable: &usable,
                waited_step: self
                    .node_waits
                    .get(&id)
                    .and_then(|(step, frames, _)| (*frames >= STEP_WAIT_FRAMES).then_some(*step)),
                coarsest_wish: entry.wanted.iter().flatten().min().copied(),
            });
            entry.node_target = Some(target);
            entry.node_anchor = out.anchor;
            entry.node_run = out.run;
            entry.node_stage = out.stage;
            entry.node_step = out.step;
        }
    }

    /// Advances the waits of the nodes of the applied cut that wait for a level (`node_step`),
    /// in place: `(step, frames, stamp)`; the nodes not stamped this call are dropped.
    pub(super) fn advance_node_waits(&mut self) {
        self.node_waits_stamp += 1;
        let stamp = self.node_waits_stamp;
        for entry in &self.applied {
            let Some(step) = entry.node_step else {
                continue;
            };
            match self.node_waits.get_mut(&entry.node) {
                // The same step as the previous call: one more frame.
                Some(wait) if wait.0 == step && wait.2 + 1 == stamp => {
                    wait.1 += 1;
                    wait.2 = stamp;
                }
                _ => {
                    self.node_waits.insert(entry.node, (step, 1, stamp));
                }
            }
        }
        self.node_waits.retain(|_, wait| wait.2 == stamp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(prev: Option<u8>, range: (u8, u8), target: u8, usable: &[u8]) -> RunInput<'_> {
        RunInput {
            prev,
            range,
            target,
            usable,
            waited_step: None,
            coarsest_wish: None,
        }
    }

    #[test]
    fn a_detached_d5_depth_is_not_usable_but_an_attached_one_is() {
        assert_eq!(
            without_detached_d5(vec![5, 6, 7, 8, 13], Some(13)),
            [5, 6, 7, 8]
        );
        assert_eq!(
            without_detached_d5(vec![5, 6, 7, 8, 9], Some(9)),
            [5, 6, 7, 8, 9]
        );
        assert_eq!(without_detached_d5(vec![5, 13], None), [5, 13]);
        assert_eq!(without_detached_d5(vec![13], Some(13)), [13]);
        // The hop of the example (own 5..=8, D5 13, target 13, waiting for 9) finds nothing to
        // land on, where with 13 as an input it would jump to a run of (13, 13).
        let usable = without_detached_d5(vec![5, 6, 7, 8, 13], Some(13));
        let out = node_run(&RunInput {
            waited_step: Some(9),
            ..input(Some(8), (5, 13), 13, &usable)
        });
        assert_eq!(out.run, Some((5, 8)));
        assert_eq!(out.step, Some(9));
    }

    #[test]
    fn zoom_out_fast_keeps_the_run_of_prev() {
        // B1: prev 12, hi 8: the run is 10..=12, not empty and not a jump to 5.
        let out = node_run(&input(Some(12), (8, 8), 8, &[5, 10, 11, 12]));
        assert_eq!(out.anchor, Some(12));
        assert_eq!(out.run, Some((10, 12)));
        assert!(!out.stage && out.step.is_none());
    }

    #[test]
    fn anchor_is_closest_to_prev_towards_the_target() {
        // Zoom in: 8 is closer to prev 7 than 5, and lies towards the target.
        let usable = [5, 8, 9];
        assert_eq!(
            node_run(&input(Some(7), (5, 9), 9, &usable)).anchor,
            Some(8)
        );
        // Tie between 6 and 8 around prev 7 (both usable, 7 is not): the coarser one.
        assert_eq!(
            node_run(&input(Some(7), (5, 9), 9, &[6, 8])).anchor,
            Some(8),
            "towards the target first: 6 lies behind prev"
        );
        assert_eq!(
            node_run(&input(Some(7), (5, 9), 7, &[6, 8])).anchor,
            Some(6),
            "target = prev: both sides tie, the coarser one"
        );
    }

    #[test]
    fn evicted_prev_falls_back_to_the_closest_overall() {
        // B2: prev 12 is gone and nothing usable lies in [8, 12]: the closest overall.
        let out = node_run(&input(Some(12), (8, 8), 8, &[5, 6]));
        assert_eq!(out.anchor, Some(6));
        assert_eq!(out.run, Some((6, 6)), "clipped at the anchor, below lo");
        // Prev gone but a depth lies between prev and the target: that one, not a coarser one.
        let out = node_run(&input(Some(12), (8, 8), 8, &[5, 10, 11]));
        assert_eq!(out.anchor, Some(11));
    }

    #[test]
    fn no_history_uses_the_target() {
        let out = node_run(&input(None, (5, 14), 14, &[5, 13, 14]));
        assert_eq!(out.anchor, Some(14));
        assert_eq!(out.run, Some((13, 14)));
    }

    #[test]
    fn no_usable_depth_gives_nothing() {
        let out = node_run(&input(Some(9), (5, 14), 14, &[]));
        assert_eq!(out, NodeRun::default());
    }

    #[test]
    fn run_is_clipped_above_hi_but_never_empty() {
        // The usable 5..=10 above hi 8 is cut at 8 when the anchor is below it.
        let out = node_run(&input(Some(6), (5, 8), 8, &[5, 6, 7, 8, 9, 10]));
        assert_eq!(out.run, Some((5, 8)));
        // Nor does it draw below `lo`: 3..=5 are usable, the node spans 5 only.
        let out = node_run(&input(Some(5), (5, 5), 5, &[3, 4, 5]));
        assert_eq!(out.run, Some((5, 5)));
        // An anchor above hi keeps itself: `max(hi, anchor)`.
        let out = node_run(&input(Some(12), (5, 8), 8, &[11, 12, 13]));
        assert_eq!(out.run, Some((11, 12)));
    }

    #[test]
    fn zoom_in_waits_with_the_run_at_the_coarse_side() {
        // Only 5 and 14 usable, prev 5: the anchor stays at 5, staged, asking for 6.
        let usable = [5, 14];
        let out = node_run(&input(Some(5), (5, 14), 14, &usable));
        assert_eq!((out.anchor, out.run), (Some(5), Some((5, 5))));
        assert!(out.stage);
        assert_eq!(out.step, Some(6));
        // Prev below 5: same.
        assert_eq!(
            node_run(&input(Some(3), (5, 14), 14, &usable)).anchor,
            Some(5)
        );
        // The waited flag hops to 14: one run, no gap, no stage.
        let out = node_run(&RunInput {
            waited_step: Some(6),
            ..input(Some(5), (5, 14), 14, &usable)
        });
        assert_eq!((out.anchor, out.run), (Some(14), Some((14, 14))));
        assert!(!out.stage && out.step.is_none());
    }

    #[test]
    fn hop_goes_to_the_nearest_depth_of_the_finer_run() {
        // The 8 never arrives: 10..=14 are resident; the hop lands on 10 and the run takes 10..=14.
        let usable = [5, 6, 7, 10, 11, 12, 13, 14];
        let out = node_run(&RunInput {
            waited_step: Some(8),
            ..input(Some(7), (5, 14), 14, &usable)
        });
        assert_eq!((out.anchor, out.run), (Some(10), Some((10, 14))));
        assert!(!out.stage);
        // Without the flag the node waits at 7, asking for 8.
        let out = node_run(&input(Some(7), (5, 14), 14, &usable));
        assert_eq!(out.run, Some((5, 7)));
        assert_eq!(out.step, Some(8));
    }

    #[test]
    fn the_waited_flag_does_nothing_without_a_finer_run() {
        let out = node_run(&RunInput {
            waited_step: Some(8),
            ..input(Some(7), (5, 14), 14, &[5, 6, 7])
        });
        assert_eq!(out.run, Some((5, 7)));
        assert_eq!(out.step, Some(8));
        // Nor does it move a node that reached its target.
        let out = node_run(&RunInput {
            waited_step: Some(8),
            ..input(Some(7), (5, 7), 7, &[5, 6, 7, 12])
        });
        assert_eq!(out.run, Some((5, 7)));
    }

    #[test]
    fn stage_also_follows_the_coarsest_wish() {
        // B7: the run reaches the target but draws finer than the coarsest wish.
        let out = node_run(&RunInput {
            coarsest_wish: Some(11),
            ..input(Some(12), (10, 12), 12, &[10, 11, 12])
        });
        assert!(!out.stage, "the run reaches down to 10, below the wish 11");
        let out = node_run(&RunInput {
            coarsest_wish: Some(7),
            ..input(Some(12), (12, 12), 12, &[12])
        });
        assert!(out.stage, "run.lo 12 is finer than the wish 7");
        assert_eq!(
            out.step,
            Some(11),
            "C1: the level under the run is asked for"
        );
    }

    #[test]
    fn step_is_only_for_the_fine_end() {
        // Target reached: no step even if the next depth is not usable.
        let out = node_run(&input(Some(9), (5, 9), 9, &[8, 9]));
        assert!(!out.stage && out.step.is_none());
    }

    #[test]
    fn the_hop_never_goes_past_the_target() {
        // B6 guard: 12 is usable but lies beyond the target 9; the hop has nowhere to go.
        let out = node_run(&RunInput {
            waited_step: Some(8),
            ..input(Some(7), (5, 14), 9, &[5, 6, 7, 12])
        });
        assert_eq!(out.run, Some((5, 7)));
        assert_eq!(out.step, Some(8));
    }

    #[test]
    fn stage_is_exact_at_the_wish_boundary() {
        // B7: a wish equal to `run.lo` is not finer than the wish.
        let out = node_run(&RunInput {
            coarsest_wish: Some(10),
            ..input(Some(12), (10, 12), 12, &[10, 11, 12])
        });
        assert!(!out.stage && out.step.is_none());
        let out = node_run(&RunInput {
            coarsest_wish: Some(9),
            ..input(Some(12), (9, 12), 12, &[10, 11, 12])
        });
        assert!(out.stage, "one level below run.lo");
        assert_eq!(out.step, Some(9));
    }

    #[test]
    fn zoom_out_asks_for_the_coarse_level_and_hops_after_the_wait() {
        // C1, node 34: range (7, 7), usable 7, 11, 12, prev 12, coarsest wish 7.
        let node = input(Some(12), (7, 7), 7, &[7, 11, 12]);
        let node = RunInput {
            coarsest_wish: Some(7),
            ..node
        };
        let out = node_run(&node);
        assert_eq!((out.anchor, out.run), (Some(12), Some((11, 12))));
        assert!(out.stage, "draws finer than the wish");
        assert_eq!(out.step, Some(10), "asks for the level under the run");
        // A wait that was counted for another level does not hop.
        let out = node_run(&RunInput {
            waited_step: Some(8),
            ..node
        });
        assert_eq!(out.run, Some((11, 12)));
        // The wait for 10 expired: hop to the nearest usable depth coarser than the run.
        let out = node_run(&RunInput {
            waited_step: Some(10),
            ..node
        });
        assert_eq!((out.anchor, out.run), (Some(7), Some((7, 7))));
        assert!(!out.stage && out.step.is_none());
    }

    #[test]
    fn the_coarse_hop_never_goes_past_the_wish() {
        // Only 5 lies coarser than the run, and the wish is 8: no hop.
        let out = node_run(&RunInput {
            waited_step: Some(10),
            coarsest_wish: Some(8),
            ..input(Some(12), (8, 12), 12, &[5, 11, 12])
        });
        assert_eq!(out.run, Some((11, 12)));
        assert_eq!(out.step, Some(10));
    }

    #[test]
    fn both_ends_waiting_the_end_the_view_is_moving_goes_first() {
        // Run (7, 7) is short of the target 12 (needs 8) and the wish 5 is below it (needs 6).
        let usable = [7, 12];
        let both = |prev| RunInput {
            coarsest_wish: Some(5),
            ..input(Some(prev), (5, 12), 12, &usable)
        };
        // prev 7 <= target 12: refining, the fine end first.
        assert_eq!(node_run(&both(7)).step, Some(8));
        // The coarse step is the one that hops while it is the pending one.
        let out = node_run(&RunInput {
            waited_step: Some(8),
            ..both(7)
        });
        assert_eq!(
            out.run,
            Some((12, 12)),
            "the fine hop, then the coarse step is next"
        );
        assert_eq!(out.step, Some(11));
        // Coarsening (prev above the target): the coarse end first.
        let out = node_run(&RunInput {
            coarsest_wish: Some(5),
            ..input(Some(12), (5, 9), 9, &[7, 8, 12])
        });
        assert_eq!(out.run, Some((12, 12)));
        assert_eq!(out.step, Some(11));
    }
}
