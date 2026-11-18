//! Cell-mode-only entry seeds (removed with the cell mode).

use super::*;

impl CellPool {
    /// `prev_state`, except for a node that drew nothing in the applied cut: in cell mode its
    /// `prev` is the finest depth its drawing ancestor drew over its footprint (or, when it
    /// replaces drawn descendants, the coarsest depth they drew), clamped to its buildable range
    /// inside the span. The stepping rule then moves it one level from there, so it starts one
    /// level finer than its ancestor (one level coarser than its descendants) and goes on one
    /// level per frame (ADR 0053).
    pub(super) fn prev_state_seeded(
        &self,
        id: NodeId,
        bounds: SpatialBounds,
        buildable: (u8, u8),
        grid: &CellGrid,
    ) -> PrevState {
        let mut state = self.prev_state(id);
        if self.lod_mode != LodMode::Cell || self.drew_in_applied(id) {
            return state;
        }
        let (lo, hi) = cell_depth_range(Self::node_depth(grid, bounds));
        let (lo, hi) = (lo.max(buildable.0), hi.min(buildable.1));
        if lo > hi {
            return state;
        }
        let seed = self.entering_seed(bounds, grid);
        for (slot, seed) in state.drawn.iter_mut().zip(seed) {
            *slot = seed.map(|depth| depth.clamp(lo, hi));
        }
        state
    }

    /// Whether `id` drew anything in the applied cut (a node held with nothing resident did not).
    pub(super) fn drew_in_applied(&self, id: NodeId) -> bool {
        self.applied_drawn
            .get(&id)
            .is_some_and(|drawn| drawn.iter().any(Option::is_some))
    }

    /// Per octant, the depth a node entering the cut starts from (see `prev_state_seeded`).
    pub(super) fn entering_seed(&self, bounds: SpatialBounds, grid: &CellGrid) -> [Option<u8>; 8] {
        let mut seed = [None; 8];
        let index = &self.applied_index;
        if index.grid.as_ref() != Some(grid) {
            return seed;
        }
        let f = Footprint::new(grid, bounds);
        for gap in f.gaps() {
            let Some(&ancestor) = index.cells.get(&f.ancestor(gap)) else {
                continue;
            };
            let Some(drawn) = self.applied_drawn.get(&ancestor) else {
                continue;
            };
            for (o, slot) in seed.iter_mut().enumerate() {
                *slot = f
                    .octants_of(gap, o)
                    .filter_map(|a| self.drawn_ceiling(ancestor, drawn[a]))
                    .max();
            }
            if seed.iter().any(Option::is_some) {
                return seed;
            }
        }
        for (below, gap) in index.below.get(&(f.rep, f.lo)).into_iter().flatten() {
            let Some(drawn) = self.applied_drawn.get(below) else {
                continue;
            };
            let df = Footprint::new(grid, index.nodes[below].bounds);
            for (o, depth) in drawn.iter().enumerate() {
                for a in df.octants_of(*gap, o) {
                    if let Some(depth) = *depth {
                        seed[a] = Some(seed[a].map_or(depth, |was: u8| was.min(depth)));
                    }
                }
            }
        }
        seed
    }

    /// The depth the applied cut may have drawn over an octant of `id` where it drew `drawn`: in
    /// cell mode up to the finest active depth of the node (R3-Q21 needs what was actually drawn,
    /// not the octant's nominal depth).
    pub(super) fn drawn_ceiling(&self, id: NodeId, drawn: Option<u8>) -> Option<u8> {
        match self.lod_mode {
            LodMode::Octant => drawn,
            LodMode::Cell => {
                drawn.map(|depth| depth.max(self.applied_finest.get(&id).copied().unwrap_or(depth)))
            }
        }
    }

    /// Cell mode: the depths a node entering the cut starts from (`prev_state_seeded`) and the
    /// one its first step reaches, towards what each octant wants. They are fetched or built with
    /// the wanted depths, so the node can draw them instead of the resident level closest to its
    /// wanted depth.
    pub(super) fn seed_depths(
        &self,
        entry: &CellCutEntry,
        node: &PoolNode,
        caps: &FastMap<NodeId, u8>,
        grid: &CellGrid,
    ) -> Vec<u8> {
        if self.lod_mode != LodMode::Cell || self.drew_in_applied(entry.node) {
            return Vec::new();
        }
        let buildable = capped_range(node.buildable, caps.get(&entry.node));
        let prev = self.prev_state_seeded(entry.node, node.bounds, buildable, grid);
        let mut depths = Vec::new();
        for (seed, wanted) in prev.drawn.iter().zip(entry.wanted) {
            if let (Some(seed), Some(wanted)) = (*seed, wanted) {
                let step = match wanted.cmp(&seed) {
                    std::cmp::Ordering::Greater => seed + 1,
                    std::cmp::Ordering::Less => seed - 1,
                    std::cmp::Ordering::Equal => seed,
                };
                depths.extend([seed, step]);
            }
        }
        depths.retain(|depth| (buildable.0..=buildable.1).contains(depth));
        depths.sort_unstable();
        depths.dedup();
        depths
    }
}
