//! Active depths of a cut node in cell mode.

use super::*;

impl CellPool {
    /// The stored depths of a node that can draw: all of them in octant mode, those inside the
    /// node's span in cell mode (E2). Cell-mode callers only: octant callers use `stored` itself.
    pub(super) fn drawable_stored(
        &self,
        stored: &[u8],
        grid: &CellGrid,
        bounds: SpatialBounds,
    ) -> Vec<u8> {
        match self.lod_mode {
            LodMode::Octant => stored.to_vec(),
            LodMode::Cell => in_cell_range(stored, Self::node_depth(grid, bounds)),
        }
    }

    /// Cell mode: the depths of `entry` emitted as active levels: the drawn ones that are
    /// resident and inside the node's span (a retained cut may hold depths of another mode or
    /// span), plus the node's D5 depth when its mask is set (see `d5_active`).
    pub(super) fn cell_active_depths(
        &self,
        entry: &CellCutEntry,
        node_depth: u8,
        new_d5: Option<u8>,
    ) -> Vec<u8> {
        let usable = |depth: u8| {
            let key = self.key(entry.node, depth);
            self.is_resident(key) && !self.fresh.contains(&key)
        };
        let mut wanted = entry.required_levels();
        if self.active_rule == ActiveRule::Contiguous {
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
                    wanted
                        .extend((anchor.saturating_add(1)..=hi).take_while(|depth| usable(*depth)));
                }
                wanted.sort_unstable();
                wanted.dedup();
            }
        } else {
            // A node that draws nothing (held, nothing resident in its span) stays out entirely.
            // The span of active depths moves at most one level per applied cut at each end (ADR
            // 0053): the range residents are only taken inside the window `applied_window` of a node
            // that was already active (the drawn depths are always active).
            if let Some((lo, hi)) = entry.range
                && !wanted.is_empty()
            {
                // A node entering the cut has no window: it starts from the depths it draws, one
                // level either side, and widens from there one level per frame.
                let (wlo, whi) = self
                    .applied_window
                    .get(&entry.node)
                    .copied()
                    .unwrap_or_else(|| {
                        let drawn = entry.required_levels();
                        let first = drawn.first().copied().unwrap_or(u8::MAX);
                        let last = drawn.last().copied().unwrap_or(0);
                        (first.saturating_sub(1), last.saturating_add(1))
                    });
                // The active set stays contiguous up to the finest drawn depth (ADR 0053): an octant
                // that holds the eye draws beyond `hi`, and a gap makes the per-cell rule jump from
                // the range to that depth, whose far cells measure a fraction of a pixel.
                let hi = hi.max(wanted.last().copied().unwrap_or(hi));
                wanted.extend(lo.max(wlo)..=hi.min(whi));
                wanted.sort_unstable();
                wanted.dedup();
            }
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
