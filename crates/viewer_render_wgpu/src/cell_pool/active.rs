//! Active depths of a cut node in cell mode.

use super::{CellCutEntry, CellGrid, CellPool, SpatialBounds, cell_depth_range, in_cell_range};

impl CellPool {
    /// The stored depths of a node that can draw: those inside the node's span (E2).
    #[expect(
        clippy::unused_self,
        reason = "a `CellPool` method kept beside `coarsest_in_span`; its callers hold the pool"
    )]
    pub(super) fn drawable_stored(
        &self,
        stored: &[u8],
        grid: &CellGrid,
        bounds: SpatialBounds,
    ) -> Vec<u8> {
        in_cell_range(stored, Self::node_depth(grid, bounds))
    }

    /// The depths of `entry` emitted as active levels: the drawn ones that are
    /// resident and inside the node's span (a retained cut may hold depths of another span), plus the node's D5 depth when its mask is set (see `d5_active`).
    pub(super) fn cell_active_depths(
        &self,
        entry: &CellCutEntry,
        node_depth: u8,
        new_d5: Option<u8>,
    ) -> Vec<u8> {
        let usable = |depth: u8| self.is_usable(self.key(entry.node, depth));
        let mut wanted = entry.required_levels();
        // The resident depths of the range that reach a drawn one without a residency gap
        // (ADR 0055): a range depth beyond a gap would make the per-cell rule jump
        // to it. A drawn depth that is not usable yet (fresh) anchors no run.
        let drawn = wanted.clone();
        if let (Some((lo, hi)), Some(first), Some(last)) =
            (entry.range, drawn.first(), drawn.last())
        {
            let (lo, hi) = (lo.min(*first), hi.max(*last));
            for anchor in drawn.iter().filter(|depth| usable(**depth)) {
                wanted.extend((lo..*anchor).rev().take_while(|depth| usable(*depth)));
                wanted.extend((anchor.saturating_add(1)..=hi).take_while(|depth| usable(*depth)));
            }
            wanted.sort_unstable();
            wanted.dedup();
        }
        let mut depths: Vec<u8> = in_cell_range(&wanted, node_depth)
            .into_iter()
            .filter(|depth| usable(*depth))
            .collect();
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
    #[expect(
        clippy::unused_self,
        reason = "a `CellPool` method kept beside `drawable_stored`; its callers hold the pool"
    )]
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
