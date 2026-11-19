//! What the cut wants and what is requested: wanted cut, level ranges and requests.

use super::{
    BTreeSet, CellCutEntry, CellCutError, CellCutNode, CellGrid, CellPool, Ctx, FastMap, FastSet,
    LevelKey, NodeId, PAGE_SLOTS, PoolNode, capped, capped_range, cell_cut_ranged,
    cell_depth_range, in_cell_range, representative_depth, served_depth,
};

/// `ceil(min(points, 8^(depth - node_depth)) / 512)` pages, at least one.
pub(super) fn estimate_pages(points: u64, depth: u8, node_depth: u8) -> usize {
    let gap = u32::from(depth.saturating_sub(node_depth));
    let cells = if gap >= 21 {
        u64::MAX
    } else {
        1_u64 << (3 * gap)
    };
    (points.min(cells).div_ceil(PAGE_SLOTS as u64)).max(1) as usize
}

impl CellPool {
    /// Wanted entries with resident = stored (minus the offenders and below the caps). A
    /// `DescendantTooFine` adds its descendant to `offenders` and runs again.
    pub(super) fn wanted_cut(
        &self,
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        caps: &FastMap<NodeId, u8>,
        offenders: &mut BTreeSet<NodeId>,
        ctx: &Ctx,
    ) -> Result<Vec<CellCutEntry>, CellCutError> {
        loop {
            let lookup = |id: NodeId| {
                let node = nodes.get(&id)?;
                let stored = capped(&node.stored, caps.get(&id));
                let resident = if offenders.contains(&id) {
                    Vec::new()
                } else {
                    stored.clone()
                };
                Some(CellCutNode {
                    bounds: node.bounds,
                    buildable: capped_range(node.buildable, caps.get(&id)),
                    stored,
                    resident,
                })
            };
            ctx.evaluations.set(ctx.evaluations.get() + 1);
            // Cell mode: the entries carry each node's depth range (`range_depths`).
            match cell_cut_ranged(
                cut,
                &lookup,
                ctx.grid,
                &|id, o, data| ctx.depth(id, o, data),
                Some(&|id, data| ctx.far(id, data)),
                None,
            ) {
                Err(CellCutError::DescendantTooFine { descendant, .. })
                    if !offenders.contains(&descendant) =>
                {
                    offenders.insert(descendant);
                }
                other => return other,
            }
        }
    }

    /// Cell mode: the node range `[lo, top]` of an entry, coarse to fine. `lo` is the range's;
    /// `top` is its `hi` or the finest wanted octant depth, whichever is finer: `hi` ignores the
    /// octants behind or crossing the eye plane, whose (data-capped) wish is drawn all the same.
    pub(super) fn node_range(entry: &CellCutEntry) -> Option<(u8, u8)> {
        let (lo, hi) = entry.range?;
        let finest = entry.wanted.iter().flatten().copied().max();
        Some((lo, finest.map_or(hi, |wanted| hi.max(wanted))))
    }

    /// Cell mode: the depths of the node range (`node_range`) that the pool wants besides the
    /// octant ones: `(stored, missing)`. `stored` are the stored depths inside the range (fetched
    /// and protected); `missing` are the buildable depths not stored, requested on demand but
    /// never deeper than the spacing depth, so the single-point tail is not built for a range
    /// nobody wants (`requested_levels` adds the depths an octant wants, whatever their depth).
    /// The spacing depth is the finest stored depth of the node (the chain of the node
    /// ends at the first fully separated level, or where it is too sparse to page), or its
    /// representative depth when nothing is stored.
    pub(super) fn range_depths(
        &self,
        entry: &CellCutEntry,
        node: &PoolNode,
        caps: &FastMap<NodeId, u8>,
        grid: &CellGrid,
    ) -> (Vec<u8>, Vec<u8>) {
        let Some((lo, hi)) = Self::node_range(entry) else {
            return (Vec::new(), Vec::new());
        };
        let cap = caps.get(&entry.node);
        let stored = self.drawable_stored(&capped(&node.stored, cap), grid, node.bounds);
        let spacing = stored
            .last()
            .copied()
            .unwrap_or_else(|| representative_depth(grid, node.bounds));
        let buildable = capped_range(node.buildable, cap);
        let in_range: Vec<u8> = stored
            .iter()
            .copied()
            .filter(|depth| (lo..=hi).contains(depth))
            .collect();
        let missing = in_cell_range(
            &(lo.max(buildable.0)..=hi.min(buildable.1).min(spacing)).collect::<Vec<u8>>(),
            Self::node_depth(grid, node.bounds),
        )
        .into_iter()
        .filter(|depth| !stored.contains(depth))
        .collect::<Vec<u8>>();
        // Contiguous span (ADR 0053): every buildable depth between the coarsest and the finest
        // active one (drawn or resident) is wanted too, below the spacing depth as well.
        // `buildable` holds the sparse-page refusals; the minimum side only bounds the wishes.
        let node_depth = Self::node_depth(grid, node.bounds);
        let (span_lo, span_hi) = cell_depth_range(node_depth);
        // Active depths are the drawn ones and the resident ones inside the range (the pinned
        // coarsest level outside it is not active).
        let active: Vec<u8> = (lo.max(span_lo)..=hi.min(span_hi))
            .filter(|depth| self.is_resident(self.key(entry.node, *depth)))
            .chain(entry.required_levels())
            .collect();
        let mut missing = missing;
        if let (Some(first), Some(last)) = (active.iter().min(), active.iter().max()) {
            for depth in (*first).max(buildable.0)..=(*last).min(buildable.1) {
                if (span_lo..=span_hi).contains(&depth)
                    && !stored.contains(&depth)
                    && !missing.contains(&depth)
                {
                    missing.push(depth);
                }
            }
            missing.sort_unstable();
        }
        (in_range, missing)
    }

    /// Distinct levels the entries need: the wanted ones plus the pinned coarsest of every node.
    pub(super) fn needed_levels(
        &self,
        entries: &[CellCutEntry],
        nodes: &FastMap<NodeId, PoolNode>,
        caps: &FastMap<NodeId, u8>,
        grid: &CellGrid,
    ) -> FastSet<LevelKey> {
        let mut set = FastSet::default();
        for entry in entries {
            let node = &nodes[&entry.node];
            // Cell mode only sees the stored levels inside the node's span.
            let stored = capped(&node.stored, caps.get(&entry.node));
            let stored = self.drawable_stored(&stored, grid, node.bounds);
            if let Some(coarsest) = stored.first().copied() {
                set.insert(self.key(entry.node, coarsest));
            }
            // A wanted depth is fetched as the stored one that serves it; when it is not stored
            // itself it is also requested on demand (`requested_levels`).
            if !stored.is_empty() {
                for depth in entry.wanted.iter().flatten() {
                    set.insert(self.key(entry.node, served_depth(&stored, *depth)));
                }
            }
            for depth in self.range_depths(entry, node, caps, grid).0 {
                set.insert(self.key(entry.node, depth));
            }
            for depth in self.seed_depths(entry, node, caps, grid) {
                if stored.contains(&depth) {
                    set.insert(self.key(entry.node, depth));
                }
            }
        }
        set
    }

    /// Cell mode: the range depths of the entries that are buildable but not stored (see
    /// `range_depths`).
    pub(super) fn range_requests(
        &self,
        entries: &[CellCutEntry],
        nodes: &FastMap<NodeId, PoolNode>,
        caps: &FastMap<NodeId, u8>,
        grid: &CellGrid,
    ) -> FastSet<LevelKey> {
        let mut set = FastSet::default();
        for entry in entries {
            for depth in self.range_depths(entry, &nodes[&entry.node], caps, grid).1 {
                set.insert(self.key(entry.node, depth));
            }
        }
        set
    }

    /// The wanted depths that are buildable but not stored: the levels to request on demand.
    pub(super) fn requested_levels(
        &self,
        entries: &[CellCutEntry],
        nodes: &FastMap<NodeId, PoolNode>,
        caps: &FastMap<NodeId, u8>,
        grid: &CellGrid,
    ) -> FastSet<LevelKey> {
        let mut set = self.range_requests(entries, nodes, caps, grid);
        for entry in entries {
            let node = &nodes[&entry.node];
            let stored = capped(&node.stored, caps.get(&entry.node));
            let stored = self.drawable_stored(&stored, grid, node.bounds);
            let buildable = capped_range(node.buildable, caps.get(&entry.node));
            // Cell mode: no stored level can draw, so the coarsest buildable one inside the
            // span is requested first (it is also what the node pins).
            if stored.is_empty()
                && let Some(depth) = self.coarsest_in_span(buildable, grid, node.bounds)
            {
                set.insert(self.key(entry.node, depth));
            }
            for depth in self.seed_depths(entry, node, caps, grid) {
                if !stored.contains(&depth) {
                    set.insert(self.key(entry.node, depth));
                }
            }
            for depth in entry.wanted.iter().flatten() {
                if !stored.contains(depth) {
                    debug_assert!(
                        (buildable.0..=buildable.1).contains(depth),
                        "a wanted depth is stored or buildable"
                    );
                    set.insert(self.key(entry.node, *depth));
                }
            }
        }
        set
    }

    /// The levels the entries need and their pages.
    pub(super) fn needed_and_pages(
        &self,
        entries: &[CellCutEntry],
        nodes: &FastMap<NodeId, PoolNode>,
        caps: &FastMap<NodeId, u8>,
        grid: &CellGrid,
        d5: &FastSet<LevelKey>,
    ) -> (FastSet<LevelKey>, u64) {
        let mut needed = self.needed_levels(entries, nodes, caps, grid);
        // Cell mode: the D5 levels of the gated ancestors take pages too, so the fit counts them.
        needed.extend(d5.iter().copied());
        // Cell mode: the range depths requested on demand take pages once built, so the fit
        // counts them at their estimate (they are not in `needed`: nothing to fetch yet).
        let requested = self.range_requests(entries, nodes, caps, grid);
        let total = needed
            .iter()
            .chain(requested.iter().filter(|key| !needed.contains(key)))
            .map(|key| self.pages_of(*key, &nodes[&key.node], grid) as u64)
            .sum();
        (needed, total)
    }
}
