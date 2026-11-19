//! Only-refine hold and the D5 gate.

use super::*;

impl CellPool {
    /// D5 (E7): a node whose mask is set also has its D5 depth active, which covers the unmasked
    /// part of its coarse cells. `snapshot` reads the depth held at the last applied frame (it
    /// is still resident), which is what the GPU record holds on a retained frame: the caps
    /// may have dropped below it since. Otherwise it is computed from the current caps.
    pub(super) fn d5_active(&self, entry: &CellCutEntry, snapshot: bool) -> Option<u8> {
        if entry.mask.iter().all(|word| *word == 0) {
            return None;
        }
        let d5 = if snapshot {
            self.applied_d5.get(&entry.node).copied()
        } else {
            let node = self.applied_index.nodes.get(&entry.node)?;
            let grid = self.applied_index.grid.as_ref()?;
            self.d5_resident(entry.node, node, grid)
        }?;
        (self.is_resident(self.key(entry.node, d5))
            && !self.fresh.contains(&self.key(entry.node, d5)))
        .then_some(d5)
    }

    /// Stored depths of `node` that cell mode can use (inside its span, under the fit cap).
    pub(super) fn d5_stored(&self, id: NodeId, node: &PoolNode, grid: &CellGrid) -> Vec<u8> {
        self.drawable_stored(&capped(&node.stored, self.caps.get(&id)), grid, node.bounds)
    }

    /// The D5 depth of a node (E7): its smallest resident depth `>= rep` (`node_depth + 3`)
    /// inside the cell span.
    pub(super) fn d5_resident(&self, id: NodeId, node: &PoolNode, grid: &CellGrid) -> Option<u8> {
        let rep = representative_depth(grid, node.bounds);
        self.d5_stored(id, node, grid)
            .into_iter()
            .find(|depth| *depth >= rep && self.is_resident(self.key(id, *depth)))
    }

    /// The depth to request as the D5 depth of a node: the smallest stored depth `>= rep` and,
    /// when none is stored, the first buildable one inside the span.
    pub(super) fn d5_wanted(&self, id: NodeId, node: &PoolNode, grid: &CellGrid) -> Option<u8> {
        let rep = representative_depth(grid, node.bounds);
        self.d5_stored(id, node, grid)
            .into_iter()
            .find(|depth| *depth >= rep)
            .or_else(|| {
                let buildable = capped_range(node.buildable, self.caps.get(&id));
                let (lo, hi) = cell_depth_range(Self::node_depth(grid, node.bounds));
                let first = rep.max(lo).max(buildable.0);
                (first <= hi.min(buildable.1)).then_some(first)
            })
    }

    /// R3-Q21: the cut nodes of the real cut `entries` (computed with `hold` as the held set) that
    /// would coarsen a zone they take over, plus the D5 gate (`add_d5_gate`). What drew over a
    /// node's footprint before is the applied ancestor (`CoverRule`, the one of the fallback) and,
    /// on a zoom out, the applied descendants of the node. The node is judged as a whole by
    /// `run_covers`: it may take over iff the finest depth of its active run reaches
    /// `min(finest active depth of what drew there before, finest depth it wants)`. A node with a
    /// covering ancestor or covering descendants can be held; one that is already held keeps the
    /// hold only while they are in the real cut. With descendants the wanted depths are those of
    /// the node alone: its octants that they mask in `entries` would otherwise look finished.
    /// The tests judge the rule against the GPU twin (`cell_lod::drawn_depth_at`).
    pub(super) fn eval_hold(
        &self,
        entries: &[CellCutEntry],
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
        hold: &FastSet<NodeId>,
        wanted_depth: &dyn Fn(NodeId, usize, &CellCutNode) -> u8,
    ) -> HoldEval {
        let mut out = self.refine_hold(entries, cut, nodes, grid, hold, wanted_depth, Judge::Run);
        self.add_d5_gate(&mut out, entries, cut, nodes, grid);
        out
    }

    /// E7, cell mode: a cut node that would mask an ancestor of the real cut stays held while
    /// that ancestor draws but has no resident D5 depth, so a partially masked coarse cell never
    /// leaves a hole. It looks at what the node would draw (its resident levels), not at the
    /// mask, so the held set does not depend on itself.
    pub(super) fn add_d5_gate(
        &self,
        out: &mut HoldEval,
        entries: &[CellCutEntry],
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
    ) {
        let mut cells: FastMap<(u8, [u64; 3]), NodeId> = FastMap::default();
        for entry in entries {
            let f = Footprint::new(grid, nodes[&entry.node].bounds);
            cells.entry((f.rep, f.lo)).or_insert(entry.node);
        }
        for entry in &entries[..cut.len()] {
            let id = entry.node;
            let data = &nodes[&id];
            if !self.has_resident_level(id, data, grid) {
                continue;
            }
            let f = Footprint::new(grid, data.bounds);
            for gap in f.gaps() {
                let Some(&ancestor) = cells.get(&f.ancestor(gap)) else {
                    continue;
                };
                let a = &nodes[&ancestor];
                if self.has_resident_level(ancestor, a, grid)
                    && self.d5_resident(ancestor, a, grid).is_none()
                {
                    out.hold.insert(id);
                    out.settle.insert(id);
                    out.ancestors.insert(ancestor);
                }
            }
        }
    }

    /// E7: the D5 depth (`d5_wanted`) of each node of `cut` or `ancestors` that has a node of
    /// `cut` under it (gap <= `MAX_MASK_LEVELS`, the span a mask can express).
    pub(super) fn d5_keys(
        &self,
        cut: &[NodeId],
        ancestors: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
    ) -> FastSet<LevelKey> {
        let mut cells: FastMap<(u8, [u64; 3]), NodeId> = FastMap::default();
        for id in cut.iter().chain(ancestors) {
            let f = Footprint::new(grid, nodes[id].bounds);
            cells.entry((f.rep, f.lo)).or_insert(*id);
        }
        let mut parents: FastSet<NodeId> = FastSet::default();
        for id in cut {
            let f = Footprint::new(grid, nodes[id].bounds);
            parents.extend(f.gaps().filter_map(|gap| cells.get(&f.ancestor(gap))));
        }
        parents
            .into_iter()
            .filter_map(|id| {
                let depth = self.d5_wanted(id, &nodes[&id], grid)?;
                Some(self.key(id, depth))
            })
            .collect()
    }

    /// The per-octant rule (deleted by plan task 11), kept for the tests that compare it with
    /// `run_covers`.
    #[cfg(test)]
    pub(super) fn eval_refine_hold(
        &self,
        entries: &[CellCutEntry],
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
        hold: &FastSet<NodeId>,
        wanted_depth: &dyn Fn(NodeId, usize, &CellCutNode) -> u8,
    ) -> HoldEval {
        self.refine_hold(
            entries,
            cut,
            nodes,
            grid,
            hold,
            wanted_depth,
            Judge::PerOctant,
        )
    }

    /// The R3-Q21 hold ("only refine"), see the module docs. `judge` says when a node that would
    /// enter the cut is short of what drew over its footprint before.
    #[allow(clippy::too_many_arguments)]
    fn refine_hold(
        &self,
        entries: &[CellCutEntry],
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
        hold: &FastSet<NodeId>,
        wanted_depth: &dyn Fn(NodeId, usize, &CellCutNode) -> u8,
        judge: Judge,
    ) -> HoldEval {
        let mut out = HoldEval::default();
        if self.applied_drawn.is_empty() {
            return out;
        }
        let position: FastMap<NodeId, usize> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.node, index))
            .collect();
        let in_cut: FastSet<NodeId> = cut.iter().copied().collect();
        let mut rule: Option<CoverRule> = None;
        for entry in &entries[..cut.len()] {
            let id = entry.node;
            let data = &nodes[&id];
            let stored = capped(&data.stored, self.caps.get(&id));
            let resident: Vec<u8> = self
                .drawable_stored(&stored, grid, data.bounds)
                .into_iter()
                .filter(|depth| self.is_resident(self.key(id, *depth)))
                .collect();
            if resident.is_empty() {
                continue;
            }
            let f = Footprint::new(grid, data.bounds);
            // Applied descendants of the node that drew something (a zoom out).
            let below: Vec<(NodeId, u8)> = self
                .applied_index
                .below
                .get(&(f.rep, f.lo))
                .map(|list| {
                    list.iter()
                        .copied()
                        .filter(|(d, _)| {
                            !in_cut.contains(d)
                                && self
                                    .applied_drawn
                                    .get(d)
                                    .is_some_and(|drawn| drawn.iter().any(Option::is_some))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let has_above = f
                .gaps()
                .any(|k| self.applied_index.cells.contains_key(&f.ancestor(k)));
            if !has_above && below.is_empty() {
                continue;
            }
            let above = if has_above {
                let rule = rule.get_or_insert_with(|| {
                    let with_level: Vec<NodeId> = cut
                        .iter()
                        .copied()
                        .filter(|id| {
                            !hold.contains(id) && self.has_resident_level(*id, &nodes[id], grid)
                        })
                        .collect();
                    CoverRule::new(self, cut, &with_level, nodes, grid)
                });
                match rule.cover(self, &f) {
                    Cover::Fallback { ancestor, gap } => Some((ancestor, gap)),
                    Cover::Cut { cell, gap } => self
                        .applied_index
                        .cells
                        .get(&cell)
                        .map(|&ancestor| (ancestor, gap)),
                    Cover::Nothing => None,
                }
            } else {
                None
            };
            if above.is_none() && below.is_empty() {
                continue;
            }
            let held = hold.contains(&id);
            if held
                && !above.is_some_and(|(a, gap)| covers_in(entries, &position, a, &f, gap))
                && !below.iter().any(|(d, _)| position.contains_key(d))
            {
                continue;
            }
            // Per octant of the node: the deepest level that drew over it before.
            let mut limit: [Option<u8>; 8] = [None; 8];
            if let Some((ancestor, gap)) = above
                && let Some(drawn) = self.applied_drawn.get(&ancestor)
            {
                for (o, slot) in limit.iter_mut().enumerate() {
                    *slot = f
                        .octants_of(gap, o)
                        .filter_map(|a| self.drawn_ceiling(ancestor, drawn[a]))
                        .max();
                }
            }
            // The covering node may not have drawn over this zone before (it is new in the cut
            // or drew elsewhere): what drew there was the nearest applied ancestor above it.
            if let Some((_, gap)) = above {
                for k in f.gaps().filter(|k| *k > gap) {
                    let Some((far, drawn)) = self
                        .applied_index
                        .cells
                        .get(&f.ancestor(k))
                        .and_then(|a| Some((*a, self.applied_drawn.get(a)?)))
                    else {
                        continue;
                    };
                    for (o, slot) in limit.iter_mut().enumerate() {
                        if slot.is_none() {
                            *slot = f
                                .octants_of(k, o)
                                .filter_map(|a| self.drawn_ceiling(far, drawn[a]))
                                .max();
                        }
                    }
                }
            }
            for (d, k) in &below {
                let df = Footprint::new(grid, self.applied_index.nodes[d].bounds);
                for (o, drawn) in self.applied_drawn[d].iter().enumerate() {
                    if let Some(depth) = self.drawn_ceiling(*d, *drawn) {
                        for a in df.octants_of(*k, o) {
                            limit[a] = limit[a].max(Some(depth));
                        }
                    }
                }
            }
            let wanted = if below.is_empty() {
                entry.wanted
            } else {
                let solo = cell_cut_with(
                    &[id],
                    &|_| {
                        Some(CellCutNode {
                            bounds: data.bounds,
                            buildable: capped_range(data.buildable, self.caps.get(&id)),
                            stored: stored.clone(),
                            resident: resident.clone(),
                        })
                    },
                    grid,
                    wanted_depth,
                );
                solo.ok()
                    .and_then(|solo| solo.first().map(|e| e.wanted))
                    .unwrap_or(entry.wanted)
            };
            let (here, _, _) = drawn_levels(
                &wanted,
                &stored,
                capped_range(data.buildable, self.caps.get(&id)),
                &resident,
                &self.prev_state_seeded(
                    id,
                    data.bounds,
                    capped_range(data.buildable, self.caps.get(&id)),
                    grid,
                ),
                f.rep.saturating_sub(MAX_MASK_LEVELS),
            );
            let short = match judge {
                Judge::Run => {
                    // The active run of the node from its own resident levels under the cap, with
                    // no `new_d5`, against the finest depth that drew over any octant it wants
                    // before. `plan` may emit a different set once the node is released: under a
                    // fit cap `finer_drawn` swaps the resident levels for finer ones above the cap
                    // (tests: a capped node released by the gate), and `new_d5` adds a depth: the
                    // gate ignores both, and the twin tests show the emitted set draws nothing
                    // coarser. A node with nothing usable to draw yet is short.
                    let before = limit
                        .iter()
                        .zip(&wanted)
                        .filter(|(_, w)| w.is_some())
                        .filter_map(|(l, _)| *l)
                        .max();
                    let wish = wanted.iter().flatten().max();
                    match (before, wish) {
                        (Some(before), Some(wish)) => {
                            let run = CellCutEntry {
                                drawn: here,
                                ..entry.clone()
                            };
                            let depths = self.cell_active_depths(
                                &run,
                                f.rep.saturating_sub(MAX_MASK_LEVELS),
                                None,
                            );
                            match depths.first().zip(depths.last()) {
                                Some((lo, hi)) => {
                                    !run_covers(Some((*lo, *hi)), &[(0, before)], *wish)
                                }
                                None => true,
                            }
                        }
                        _ => false,
                    }
                }
                #[cfg(test)]
                Judge::PerOctant => (0..8).any(|o| {
                    // The raw wanted depth, not the stored one (R3-Q21): a child storing only a
                    // level coarser than what its parent draws does not take over until that
                    // level is built on demand.
                    wanted[o].is_some_and(|w| {
                        limit[o].is_some_and(|l| here[o].is_none_or(|d| d < l.min(w)))
                    })
                }),
            };
            if short {
                out.hold.insert(id);
            }
            if short || held {
                out.settle.insert(id);
                out.ancestors.extend(above.map(|(a, _)| a));
                out.ancestors.extend(below.iter().map(|(d, _)| *d));
            }
        }
        out
    }
}

/// The active run `[coarsest, finest]` of a node, as `CellPool::cell_active_depths` gives it.
pub(super) type Run = (u8, u8);

/// Residency gate over node-level runs (R3-Q21, plan tasks 8 and 9): whether a node
/// whose run would be `run` may take over without coarsening what `before` drew over its
/// footprint. `before` are the runs that drew there: the applied ancestor's on a zoom in, the
/// applied descendants' on a zoom out (the node replaces them). `wish` is the finest depth the
/// node wants.
///
/// States the GPU-rule condition: among a node's active depths a cell draws the coarsest one
/// that does not refine (`cell_lod::draws`), so the node coarsens a zone only if its finest active
/// depth `run.1` is below what the zone drew before (the finest of `before`: the finest ACTIVE
/// depth, not the nominal one) capped by what the view wants (`wish`). It may take over iff
/// `run.1 >= min(finest of before, wish)`. The two judges, this and the per-octant rule
/// (`eval_refine_hold`, deleted by plan task 11), are not ordered; what guarantees the gate is
/// the twin-judged tables of `cell_pool_tests` (see `ZOOM_OUT_PARTS`).
///
/// `before` empty (nothing drew there) releases the node. An empty run (`None`: nothing
/// drawable resident) also returns `true`: the caller decides it, and holds the node, because
/// it has nothing to draw and the covering cell must keep drawing the zone.
pub(super) fn run_covers(run: Option<Run>, before: &[Run], wish: u8) -> bool {
    let (Some((_, finest)), Some(drawn)) = (run, before.iter().map(|(_, hi)| *hi).max()) else {
        return true;
    };
    finest >= drawn.min(wish)
}

/// What decides that a node entering the cut is short of what drew there before.
#[derive(Clone, Copy)]
enum Judge {
    /// `run_covers` on the node's active run.
    Run,
    /// The per-octant seeded depths; task 11 deletes it.
    #[cfg(test)]
    PerOctant,
}
