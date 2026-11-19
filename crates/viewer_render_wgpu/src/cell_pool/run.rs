//! The single active run of a node (plan task 10, amendments B1, B2, B6, B7, B11).
//!
//! `node_run` is pure: plain depths in, plain depths out. `CellPool::fill_node_runs` computes it
//! for the planned cut into the `node_*` fields of `CellCutEntry`; nobody reads them yet.

use super::*;

/// What `node_run` needs of a node.
pub(super) struct RunInput<'a> {
    /// Fine end of the run the node drew before, WITHOUT its D5 depth (B3), or `before` (B5) for a
    /// node that did not draw. `None`: no history, the target stands in for it.
    pub prev: Option<u8>,
    /// `[lo, hi]` of the node (`CellCutEntry::range`).
    pub range: (u8, u8),
    /// The finest depth the node can reach: `min(hi, finest stored or buildable depth, fit cap)`.
    pub target: u8,
    /// Usable depths of the node inside its cell span, ascending. Contract (B3): the D5 depth is
    /// NOT an input unless it is contiguous with the rest, so the anchor never lands on it; the
    /// caller leaves it out.
    pub usable: &'a [u8],
    /// The level above the run has waited `STEP_WAIT_FRAMES` frames (B6).
    pub waited: bool,
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
    /// The depth that extends the run upward (`run.hi + 1`); only when the fine end is what
    /// stages the node and that depth is not usable (B11).
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
/// `hi` widened by what is drawn). B1's text says `min(lo, run.lo)`, which would clip nothing
/// below; see the report of task 10b.
fn clipped(run: (u8, u8), anchor: u8, (lo, hi): (u8, u8)) -> (u8, u8) {
    (run.0.max(lo.min(anchor)), run.1.min(hi.max(anchor)))
}

pub(super) fn node_run(input: &RunInput<'_>) -> NodeRun {
    let RunInput {
        prev,
        range,
        target,
        usable,
        waited,
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
    let mut run = clipped(run_of(usable, anchor), anchor, range);
    // B6: the level above the run never arrives; hop to the nearest usable run finer than it,
    // still one run.
    if waited
        && run.1 < target
        && let Some(hop) = usable
            .iter()
            .copied()
            .filter(|depth| *depth > run.1 && *depth <= target)
            .min()
    {
        anchor = hop;
        run = clipped(run_of(usable, anchor), anchor, range);
    }
    let fine_end = run.1 < target;
    let stage = fine_end || coarsest_wish.is_some_and(|wish| wish < run.0);
    let step =
        (fine_end && !usable.contains(&run.1.saturating_add(1))).then_some(run.1.saturating_add(1));
    NodeRun {
        anchor: Some(anchor),
        run: Some(run),
        stage,
        step,
    }
}

impl CellPool {
    /// Fills the `node_*` fields of `entries` (the real cut, before it is applied). `prev` is the
    /// fine end of the applied run without the D5 depth (B3) and, for a node that did not draw,
    /// `before` (B5, `hold::before_finest`). The usable depths are those `cell_active_depths`
    /// accepts over the node's cell span. The target is `min(hi, finest drawable stored or
    /// buildable depth under the fit cap)`.
    pub(super) fn fill_node_runs(
        &self,
        entries: &mut [CellCutEntry],
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
                .filter(|depth| self.is_usable(self.key(id, *depth)))
                .collect();
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
                prev: self
                    .applied_run
                    .get(&id)
                    .map(|(_, top)| *top)
                    .or_else(|| before.get(&id).copied()),
                range: (lo, hi),
                target,
                usable: &usable,
                waited: self
                    .node_waits
                    .get(&id)
                    .is_some_and(|(_, frames)| *frames >= STEP_WAIT_FRAMES),
                coarsest_wish: entry.wanted.iter().flatten().min().copied(),
            });
            entry.node_anchor = out.anchor;
            entry.node_run = out.run;
            entry.node_stage = out.stage;
            entry.node_step = out.step;
        }
    }

    /// Advances the waits of the nodes of the applied cut whose run is staged by its fine end.
    pub(super) fn advance_node_waits(&mut self) {
        let mut waits = FastMap::default();
        for entry in &self.applied {
            if let (Some(_), Some((_, top))) = (entry.node_step, entry.node_run) {
                let frames = match self.node_waits.get(&entry.node) {
                    Some(&(waiting, frames)) if waiting == top => frames + 1,
                    _ => 1,
                };
                waits.insert(entry.node, (top, frames));
            }
        }
        self.node_waits = waits;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(prev: Option<u8>, range: (u8, u8), target: u8, usable: &'a [u8]) -> RunInput<'a> {
        RunInput {
            prev,
            range,
            target,
            usable,
            waited: false,
            coarsest_wish: None,
        }
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
        // The run 5..=14 above hi 8 is cut at 8 when the anchor is below it.
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
            waited: true,
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
            waited: true,
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
            waited: true,
            ..input(Some(7), (5, 14), 14, &[5, 6, 7])
        });
        assert_eq!(out.run, Some((5, 7)));
        assert_eq!(out.step, Some(8));
        // Nor does it move a node that reached its target.
        let out = node_run(&RunInput {
            waited: true,
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
        assert_eq!(out.step, None, "B11: no step down");
    }

    #[test]
    fn step_is_only_for_the_fine_end() {
        // Target reached: no step even if the next depth is not usable.
        let out = node_run(&input(Some(9), (5, 9), 9, &[8, 9]));
        assert!(!out.stage && out.step.is_none());
    }
}
