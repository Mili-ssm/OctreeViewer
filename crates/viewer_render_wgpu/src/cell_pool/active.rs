//! Active depths of a cut node in cell mode.

use super::run::{RunInput, node_run};
use super::*;

impl CellPool {
    /// The stored depths of a node that can draw: those inside the node's span (E2).
    pub(super) fn drawable_stored(
        &self,
        stored: &[u8],
        grid: &CellGrid,
        bounds: SpatialBounds,
    ) -> Vec<u8> {
        in_cell_range(stored, Self::node_depth(grid, bounds))
    }

    /// The run `entry` emits: its planned `node_run` (plan task 10, B4). Every depth of it was
    /// usable when the run was planned and the plan protects them from eviction, so it is the
    /// planned run; if one is not usable any more the run is computed again from the stored
    /// anchor over what is usable now (never a gap: the run stays contiguous).
    pub(super) fn emitted_run(&self, entry: &CellCutEntry, node_depth: u8) -> Option<(u8, u8)> {
        let run = entry.node_run?;
        let usable = |depth: u8| self.is_usable(self.key(entry.node, depth));
        if (run.0..=run.1).all(usable) {
            return Some(run);
        }
        let (span_lo, span_hi) = cell_depth_range(node_depth);
        let depths: Vec<u8> = (span_lo..=span_hi).filter(|depth| usable(*depth)).collect();
        node_run(&RunInput {
            prev: entry.node_anchor,
            range: entry.range?,
            target: entry.node_target?,
            usable: &depths,
            waited_step: None,
            coarsest_wish: entry.wanted.iter().flatten().min().copied(),
        })
        .run
    }

    /// The depths of `entry` emitted as active levels: the depths of its node run (see
    /// `emitted_run`; a held node has none), plus the node's D5 depth when its mask is set (see
    /// `d5_active`).
    pub(super) fn cell_active_depths(
        &self,
        entry: &CellCutEntry,
        node_depth: u8,
        new_d5: Option<u8>,
    ) -> Vec<u8> {
        let run: Vec<u8> = self
            .emitted_run(entry, node_depth)
            .map_or_else(Vec::new, |(lo, hi)| (lo..=hi).collect());
        let mut depths = in_cell_range(&run, node_depth);
        if let Some(d5) = new_d5
            && !depths.contains(&d5)
        {
            depths.push(d5);
            depths.sort_unstable();
        }
        depths
    }

    /// The coarsest buildable depth inside the node's span, which cell mode requests (and pins)
    /// when no stored level of the node can draw.
    pub(super) fn coarsest_in_span(
        &self,
        buildable: (u8, u8),
        grid: &CellGrid,
        bounds: SpatialBounds,
    ) -> Option<u8> {
        let (lo, hi) = cell_depth_range(Self::node_depth(grid, bounds));
        let lo = lo.max(buildable.0);
        (lo <= hi.min(buildable.1)).then_some(lo)
    }
}
