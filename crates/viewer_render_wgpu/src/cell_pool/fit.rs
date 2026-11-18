//! The fit of the wanted levels into the pool and the caps it sets.

use super::*;

/// `wanted` limited to the finest buildable depth at or below `limit` (cells of that depth or
/// coarser have a side of at least the continuity minimum); the coarsest buildable depth when all
/// are finer.
pub(super) fn continuity_depth(buildable: (u8, u8), wanted: u8, limit: u8) -> u8 {
    wanted.min(limit.clamp(buildable.0, buildable.1))
}

/// `buildable` with its finest depth lowered to the cap (never below its coarsest).
pub(super) fn capped_range(buildable: (u8, u8), cap: Option<&u8>) -> (u8, u8) {
    cap.map_or(buildable, |cap| {
        (buildable.0, buildable.1.min(*cap).max(buildable.0))
    })
}

/// `stored` truncated to depths at or below the cap.
///
/// Never empty while `stored` is not: a cap below every stored level (its depth was dropped by a
/// recolour) leaves the coarsest one.
pub(super) fn capped(stored: &[u8], cap: Option<&u8>) -> Vec<u8> {
    let kept: Vec<u8> = stored
        .iter()
        .copied()
        .filter(|depth| cap.is_none_or(|cap| depth <= cap))
        .collect();
    if kept.is_empty() {
        stored.first().copied().into_iter().collect()
    } else {
        kept
    }
}

impl CellPool {
    /// R3-Q11. Returns the levels the wanted entries need after the fit (the stored ones that
    /// serve them), the wanted depths that are not stored (to request), and whether even the coarsest levels overflow the usable
    /// capacity.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn fit(
        &mut self,
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        offenders: &mut BTreeSet<NodeId>,
        eye: [f64; 3],
        ctx: &Ctx,
        d5: &FastSet<LevelKey>,
    ) -> Result<(FastSet<LevelKey>, FastSet<LevelKey>, bool), CellCutError> {
        let usable = self.usable();
        let (high, low) = (usable * HIGH_PERCENT / 100, usable * LOW_PERCENT / 100);
        let mut caps = std::mem::take(&mut self.caps);
        caps.retain(|id, _| nodes.contains_key(id));
        let mut entries = self.wanted_cut(cut, nodes, &caps, offenders, ctx)?;
        let (mut needed, mut total) = self.needed_and_pages(&entries, nodes, &caps, ctx.grid, d5);
        // Lifting the caps needs the uncapped cut to fit under `low`. This runs only when the
        // wanted cut is recomputed (cut, sizes or depths changed), never on a reused frame; sizes
        // are not monotone in depth, so the capped total says nothing about the uncapped one.
        if !caps.is_empty() {
            let none = FastMap::default();
            let open = self.wanted_cut(cut, nodes, &none, offenders, ctx)?;
            let (open_needed, open_total) =
                self.needed_and_pages(&open, nodes, &none, ctx.grid, d5);
            if open_total <= low {
                caps.clear();
                entries = open;
                (needed, total) = (open_needed, open_total);
            }
        }
        if total > high {
            // Farthest first, ordered once; coarsened in blocks of an eighth of the cut.
            let mut order: Vec<NodeId> = cut.to_vec();
            order.sort_by(|a, b| {
                let (da, db) = (
                    distance_sq(eye, nodes[a].bounds, ctx.origin),
                    distance_sq(eye, nodes[b].bounds, ctx.origin),
                );
                db.total_cmp(&da).then(a.cmp(b))
            });
            // `entries` follow the order of `cut`.
            let position: FastMap<NodeId, usize> =
                cut.iter().enumerate().map(|(i, id)| (*id, i)).collect();
            let block = (order.len() / 8).max(1);
            let mut cursor = 0;
            while total > high {
                let mut taken = 0;
                let mut scanned = 0;
                while taken < block && scanned < order.len() {
                    let id = order[cursor % order.len()];
                    cursor += 1;
                    scanned += 1;
                    let entry = &entries[position[&id]];
                    let Some(top) = entry.wanted.iter().flatten().max().copied() else {
                        continue;
                    };
                    let stored = capped(&nodes[&id].stored, caps.get(&id));
                    let stored = self.drawable_stored(&stored, ctx.grid, nodes[&id].bounds);
                    if stored.is_empty() {
                        continue;
                    }
                    let top = served_depth(&stored, top);
                    // E7: a cap never goes below the node's D5 depth (it is protected and its
                    // pages are already counted): below it the gate of its descendants waits for
                    // a depth that is no longer requested (`d5_wanted` reads the capped levels).
                    let floor = d5
                        .iter()
                        .filter(|key| key.node == id)
                        .map(|key| key.depth)
                        .max();
                    // The finest level below `top` that saves pages: smaller, or already needed
                    // (dropping `top` then frees it outright). Estimates can tie, and a tie must
                    // not stop the fit: the cut would overflow with nothing coarsened.
                    let size =
                        |depth: u8| self.pages_of(self.key(id, depth), &nodes[&id], ctx.grid);
                    if let Some(below) = stored
                        .iter()
                        .copied()
                        .filter(|d| {
                            floor.is_none_or(|floor| *d >= floor)
                                && *d < top
                                && (size(*d) < size(top) || needed.contains(&self.key(id, *d)))
                        })
                        .max()
                    {
                        caps.insert(id, below);
                        taken += 1;
                    }
                }
                if taken == 0 {
                    break;
                }
                entries = self.wanted_cut(cut, nodes, &caps, offenders, ctx)?;
                (needed, total) = self.needed_and_pages(&entries, nodes, &caps, ctx.grid, d5);
            }
        }
        self.old_released
            .retain(|id, cap| caps.get(id) == Some(cap));
        let requests = self.requested_levels(&entries, nodes, &caps, ctx.grid);
        self.caps = caps;
        Ok((needed, requests, total > usable))
    }

    /// R3-Q21 under a fit cap: a capped node (`node`: its levels below `cap`) that lacks, in an
    /// octant whose wanted level is requested, a resident level at or above that wanted depth
    /// keeps drawing the levels above the cap it drew in the last applied cut (while resident),
    /// instead of dropping to a coarser resident level until the capped level arrives. It is per
    /// node (the lookup cannot express octants): the kept levels replace the node's resident
    /// list, so every octant of the node draws a finer or equal level than before. The
    /// kept levels are protected as part of the applied cut; once the capped levels are resident
    /// they stop drawing and become evictable. `old_released` ends the stand-in when the capped
    /// level cannot get pages.
    pub(super) fn finer_drawn(
        &self,
        id: NodeId,
        cap: u8,
        node: &CellCutNode,
        needed: &FastSet<LevelKey>,
        ctx: &Ctx,
    ) -> Option<Vec<u8>> {
        if self.old_released.get(&id) == Some(&cap) {
            return None;
        }
        let drawn = self.applied_drawn.get(&id)?;
        let mut old: Vec<u8> = drawn
            .iter()
            .flatten()
            .copied()
            .filter(|depth| *depth > cap && self.is_resident(self.key(id, *depth)))
            .collect();
        old.sort_unstable();
        old.dedup();
        if old.is_empty() {
            return None;
        }
        // Cell mode reasons about the depths it can use: stored levels and wanted depth are
        // restricted to the node's span, as `cell_cut_with` does.
        let (stored, span) = {
            let (lo, hi) = cell_depth_range(Self::node_depth(ctx.grid, node.bounds));
            let (lo, hi) = (lo.max(node.buildable.0), hi.min(node.buildable.1));
            (
                in_cell_range(&node.stored, Self::node_depth(ctx.grid, node.bounds)),
                (lo <= hi).then_some((lo, hi)),
            )
        };
        if stored.is_empty() {
            return None;
        }
        let lacks = (0..8).any(|octant| {
            let depth = ctx.depth(id, octant, node);
            let depth = span.map_or(depth, |(lo, hi)| depth.clamp(lo, hi));
            let wanted = served_depth(&stored, depth);
            needed.contains(&self.key(id, wanted)) && !node.resident.iter().any(|d| *d >= wanted)
        });
        lacks.then_some(old)
    }

    /// The levels of a node were dropped (a recolour): a fit cap that points at a depth no longer
    /// stored moves to the finest stored depth below it, or goes if there is none.
    pub(crate) fn reconcile_cap(&mut self, id: NodeId, stored: &[u8]) {
        let Some(cap) = self.caps.get(&id).copied() else {
            return;
        };
        if stored.contains(&cap) {
            return;
        }
        // Keep the cap: removing it would let finer levels back in and undo the fit. With no
        // stored depth below it, it clamps to the coarsest one (the node draws that and no finer).
        let below = stored.iter().copied().filter(|depth| *depth < cap).max();
        if let Some(depth) = below.or_else(|| stored.iter().copied().min()) {
            self.caps.insert(id, depth);
        }
        self.size_epoch += 1;
    }

    /// Does any stored level of the node, below its fit cap, have pages resident? Only the
    /// levels inside the node's span count (E2).
    pub(super) fn has_resident_level(&self, id: NodeId, node: &PoolNode, grid: &CellGrid) -> bool {
        let cap = self.caps.get(&id);
        let resident = |depth: &u8| {
            cap.is_none_or(|cap| *depth <= *cap) && self.is_resident(self.key(id, *depth))
        };
        self.drawable_stored(&node.stored, grid, node.bounds)
            .iter()
            .any(resident)
    }
}
