//! Local fallback ancestors, the cover rule and the zones that lost their level.

use super::*;

/// What stands in for the cut nodes that have no level (R3-Q19, R3-Q20).
#[derive(Clone, Default)]
pub(super) struct Fallbacks {
    /// Applied ancestors with a resident level that draw where a cut node has none.
    pub(super) ancestors: Vec<NodeId>,
    /// Applied descendants of an excluded node that were drawing: they keep drawing its zone.
    pub(super) descendants: Vec<NodeId>,
    /// Cut nodes with no level and nothing that draws their zone: left out of the real cut.
    pub(super) excluded: FastSet<NodeId>,
}

impl Fallbacks {
    /// The nodes added to the real cut, ancestors first.
    pub(super) fn extra(&self) -> Vec<NodeId> {
        self.ancestors
            .iter()
            .chain(&self.descendants)
            .copied()
            .collect()
    }
}

/// Where the cover of a node without a level comes from (R3-Q19); the fallback and the hold of
/// R3-Q21 share it, so that a held node always has an ancestor in the real cut.
pub(super) struct CoverRule {
    pub(super) in_cut: FastSet<NodeId>,
    /// Cells that hold a resident cut node more than `MAX_MASK_LEVELS` below them.
    pub(super) too_deep: FastSet<(u8, [u64; 3])>,
    /// Cells of the cut nodes that draw (`with_level`).
    pub(super) drawing: FastSet<(u8, [u64; 3])>,
    /// The applied cut was indexed for this grid, so its nodes can be ancestors.
    pub(super) applied_ok: bool,
}

pub(super) enum Cover {
    /// A cut node that draws lies over the node, at this cell: no fallback is needed.
    Cut {
        cell: (u8, [u64; 3]),
        gap: u8,
    },
    /// The nearest applied ancestor outside the cut that has a resident level.
    Fallback {
        ancestor: NodeId,
        gap: u8,
    },
    Nothing,
}

impl CoverRule {
    pub(super) fn new(
        pool: &CellPool,
        cut: &[NodeId],
        with_level: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
    ) -> Self {
        let mut too_deep = FastSet::default();
        let mut drawing = FastSet::default();
        for id in with_level {
            let f = Footprint::new(grid, nodes[id].bounds);
            for k in MAX_MASK_LEVELS + 1..=f.rep.saturating_sub(3) {
                too_deep.insert(f.ancestor(k));
            }
            drawing.insert((f.rep, f.lo));
        }
        Self {
            in_cut: cut.iter().copied().collect(),
            too_deep,
            drawing,
            applied_ok: !pool.applied_index.nodes.is_empty()
                && pool.applied_index.grid == Some(*grid),
        }
    }

    pub(super) fn cover(&self, pool: &CellPool, f: &Footprint) -> Cover {
        let mut nearest = None;
        for gap in f.gaps() {
            let cell = f.ancestor(gap);
            if self.drawing.contains(&cell) {
                return Cover::Cut { cell, gap };
            }
            if self.applied_ok
                && nearest.is_none()
                && let Some(&ancestor) = pool.applied_index.cells.get(&cell)
                && !self.in_cut.contains(&ancestor)
                && !self.too_deep.contains(&cell)
                && pool.applied_index.grid.as_ref().is_some_and(|grid| {
                    pool.has_resident_level(ancestor, &pool.applied_index.nodes[&ancestor], grid)
                })
            {
                nearest = Some(Cover::Fallback { ancestor, gap });
            }
        }
        nearest.unwrap_or(Cover::Nothing)
    }
}

/// Does `ancestor` (`gap` levels above `f`) draw, in the real cut `entries`, everywhere `f` lies?
/// A hidden octant (`wanted` is None) is drawn by the resident nodes that hide it. It reads
/// `drawn`, not the node anchor: the hold's evaluation calls it on entries `fill_node_runs` has
/// not reached yet (the run needs the hold's `before`).
pub(super) fn covers_in(
    entries: &[CellCutEntry],
    position: &FastMap<NodeId, usize>,
    ancestor: NodeId,
    f: &Footprint,
    gap: u8,
) -> bool {
    let Some(&index) = position.get(&ancestor) else {
        return false;
    };
    let parent = &entries[index];
    f.octants_in(gap)
        .all(|o| parent.drawn[o].is_some() || parent.wanted[o].is_none())
}

/// Where a node sits in the grid, as `cell_cut` places it.
pub(super) struct Footprint {
    pub(super) rep: u8,
    pub(super) lo: [u64; 3],
    pub(super) hi: [u64; 3],
}

impl Footprint {
    pub(super) fn new(grid: &CellGrid, bounds: SpatialBounds) -> Self {
        let rep = representative_depth(grid, bounds);
        let range: [(u64, u64); 3] =
            std::array::from_fn(|axis| node_range(grid, bounds, rep, axis));
        Self {
            rep,
            lo: range.map(|(lo, _)| lo),
            hi: range.map(|(_, hi)| hi),
        }
    }

    /// Ancestor gaps that can exist and be masked.
    pub(super) fn gaps(&self) -> std::ops::RangeInclusive<u8> {
        1..=self.rep.saturating_sub(3).min(MAX_MASK_LEVELS)
    }

    /// Cell key of the ancestor `k` levels up.
    pub(super) fn ancestor(&self, k: u8) -> (u8, [u64; 3]) {
        (self.rep - k, self.lo.map(|c| (c >> k) & !7))
    }

    /// Octants of the ancestor `k` levels up that this node's span touches.
    pub(super) fn octants_in(&self, k: u8) -> impl Iterator<Item = usize> {
        self.octants_span(k, self.lo, self.hi)
    }

    /// Octants of the ancestor `k` levels up that the given `octant` of this node touches.
    pub(super) fn octants_of(&self, k: u8, octant: usize) -> impl Iterator<Item = usize> {
        let (lo, hi) = self.octant_range(octant);
        self.octants_span(k, lo, hi)
    }

    /// Cells (inclusive range, at this node's depth) of the given `octant` of this node.
    pub(super) fn octant_range(&self, octant: usize) -> ([u64; 3], [u64; 3]) {
        let (mut lo, mut hi) = (self.lo, self.hi);
        for axis in 0..3 {
            let size = self.hi[axis] - self.lo[axis] + 1;
            if size >= 2 {
                let middle = self.lo[axis] + size / 2;
                if octant >> (2 - axis) & 1 == 1 {
                    lo[axis] = middle;
                } else {
                    hi[axis] = middle - 1;
                }
            }
        }
        (lo, hi)
    }

    pub(super) fn octants_span(
        &self,
        k: u8,
        lo: [u64; 3],
        hi: [u64; 3],
    ) -> impl Iterator<Item = usize> + use<> {
        let base = self.ancestor(k).1;
        let span: [(usize, usize); 3] = std::array::from_fn(|axis| {
            (
                ((lo[axis] >> k) - base[axis]).min(7) as usize,
                ((hi[axis] >> k) - base[axis]).min(7) as usize,
            )
        });
        (span[0].0 >> 2..=span[0].1 >> 2).flat_map(move |x| {
            (span[1].0 >> 2..=span[1].1 >> 2).flat_map(move |y| {
                (span[2].0 >> 2..=span[2].1 >> 2).map(move |z| x << 2 | y << 1 | z)
            })
        })
    }
}

/// Per cell, the things (nodes or entry indices) under it and how many levels below.
pub(super) type Below<T> = FastMap<(u8, [u64; 3]), Vec<(T, u8)>>;

/// An octant of an applied node whose zone has to stay drawn.
pub(super) struct Zone<'a> {
    pub(super) f: &'a Footprint,
    /// Cells of the node that its own children hid in the applied cut.
    pub(super) mask: &'a [u64; 8],
    pub(super) octant: usize,
    /// The cut nodes (and levels below the node) whose cells are the only ones that have to stay
    /// drawn; `None`: all the visible cells of the octant.
    pub(super) need: Option<&'a [(NodeId, u8)]>,
}

/// Do the drawn octants of the entries `below` (index, levels below the node) cover every cell of
/// the octant of `zone` that has to stay drawn? The union is measured at the finest resolution
/// among the boxes, so one descendant drawing a sub-cell does not cover the whole octant.
pub(super) fn octant_covered(
    zone: &Zone,
    below: &[(usize, u8)],
    entries: &[CellCutEntry],
    nodes: &FastMap<NodeId, PoolNode>,
    grid: &CellGrid,
) -> bool {
    pub(super) type Box3 = ([u64; 3], [u64; 3]);
    let f = zone.f;
    let scale = below
        .iter()
        .map(|(_, k)| *k)
        .chain(zone.need.into_iter().flatten().map(|(_, k)| *k))
        .max()
        .unwrap_or(0);
    // Half-open boxes in units of a cell of the deepest node, relative to the block of `f`.
    let boxed = |k: u8, (lo, hi): ([u64; 3], [u64; 3])| -> Box3 {
        let origin = f.lo.map(|c| c << k);
        let shift = scale - k;
        (
            std::array::from_fn(|a| lo[a].saturating_sub(origin[a]) << shift),
            std::array::from_fn(|a| (hi[a] + 1).saturating_sub(origin[a]) << shift),
        )
    };
    let mut covers: Vec<Box3> = Vec::new();
    for (index, k) in below {
        let entry = &entries[*index];
        let d = Footprint::new(grid, nodes[&entry.node].bounds);
        for o2 in (0..8).filter(|o2| entry.claims(*o2)) {
            covers.push(boxed(*k, d.octant_range(o2)));
        }
    }
    let needs: Option<Vec<Box3>> = zone.need.map(|list| {
        list.iter()
            .map(|(id, k)| {
                let d = Footprint::new(grid, nodes[id].bounds);
                boxed(*k, (d.lo, d.hi))
            })
            .collect()
    });
    let region: [(u64, u64); 3] = std::array::from_fn(|axis| {
        let start = 4 * (zone.octant >> (2 - axis) & 1) as u64;
        (start << scale, (start + 4) << scale)
    });
    // Compressed grid of the box edges inside the region: each piece is in or out of every box.
    let edges: [Vec<u64>; 3] = std::array::from_fn(|axis| {
        let (lo, hi) = region[axis];
        let mut edges = vec![lo, hi];
        for (bl, bh) in covers.iter().chain(needs.iter().flatten()) {
            edges.extend(
                [bl[axis], bh[axis]]
                    .into_iter()
                    .filter(|e| (lo..=hi).contains(e)),
            );
        }
        edges.sort_unstable();
        edges.dedup();
        edges
    });
    let inside = |boxes: &[Box3], at: [u64; 3]| {
        boxes
            .iter()
            .any(|(bl, bh)| (0..3).all(|a| bl[a] <= at[a] && at[a] < bh[a]))
    };
    for x in edges[0].windows(2) {
        for y in edges[1].windows(2) {
            for z in edges[2].windows(2) {
                let at = [x[0], y[0], z[0]];
                let cell = at.map(|c| (c >> scale) as usize);
                if zone.mask[cell[2]] >> (cell[0] + 8 * cell[1]) & 1 == 1
                    || needs.as_ref().is_some_and(|needs| !inside(needs, at))
                {
                    continue;
                }
                if !inside(&covers, at) {
                    return false;
                }
            }
        }
    }
    true
}

pub(super) fn missing_levels(entries: &[CellCutEntry]) -> u32 {
    entries
        .iter()
        .filter(|entry| (0..8).any(|o| entry.wanted[o].is_some() && !entry.claims(o)))
        .count() as u32
}

/// R3-Q19 on a real cut: `(nodes, octants)` of the entries that have an unmasked octant without a
/// level and no ancestor in the cut that draws where the node lies (the nodes), and the unmasked
/// octants without a level of the nodes that do have such an ancestor (the octants, which draw a
/// coarser level: stages).
pub(super) fn uncovered_levels(
    entries: &[CellCutEntry],
    nodes: &FastMap<NodeId, PoolNode>,
    grid: &CellGrid,
) -> (u32, u32) {
    let missing = |entry: &CellCutEntry| {
        (0..8)
            .filter(|o| entry.wanted[*o].is_some() && !entry.claims(*o))
            .count() as u32
    };
    if entries.iter().all(|entry| missing(entry) == 0) {
        return (0, 0);
    }
    let feet: Vec<Footprint> = entries
        .iter()
        .map(|entry| Footprint::new(grid, nodes[&entry.node].bounds))
        .collect();
    let mut by_cell: FastMap<(u8, [u64; 3]), usize> = FastMap::default();
    for (index, f) in feet.iter().enumerate() {
        by_cell.entry((f.rep, f.lo)).or_insert(index);
    }
    let (mut uncovered, mut octants) = (0, 0);
    for (index, entry) in entries.iter().enumerate() {
        let count = missing(entry);
        if count == 0 {
            continue;
        }
        let covered = feet[index].gaps().any(|k| {
            let Some(&parent) = by_cell.get(&feet[index].ancestor(k)) else {
                return false;
            };
            let parent = &entries[parent];
            // A hidden octant (`wanted` is None) is drawn by the resident nodes that hide it.
            feet[index]
                .octants_in(k)
                .all(|o| parent.claims(o) || parent.wanted[o].is_none())
        });
        if covered {
            octants += count;
        } else {
            uncovered += 1;
        }
    }
    (uncovered, octants)
}

impl CellPool {
    /// R3-Q19: for each cut node without a resident level that no cut ancestor with a level
    /// covers, its nearest ancestor (gap <= 3) from the applied cut that has a resident level and
    /// lies at most `MAX_MASK_LEVELS` above every resident cut node under it (a deeper one would
    /// make the real cut fail with `DescendantTooFine`). Levels beyond the fit cap do not count.
    /// R3-Q20: a node with no such ancestor is excluded from the real cut, and the applied
    /// descendants of it that were drawing (gap <= 3) stand in for it until it is resident.
    /// The caps are those of the previous frame's fit; a change retains one frame at most.
    pub(super) fn fallbacks(
        &mut self,
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
        hold: &FastSet<NodeId>,
    ) -> Fallbacks {
        let hold_digest = set_digest(hold);
        if let Some(memo) = &self.fallback_memo
            && memo.cut == cut
            && memo.caps == self.caps
            && memo.residency_epoch == self.residency_epoch
            && memo.applied_epoch == self.applied_index.epoch
            && memo.applied_digest == self.applied_digest
            && memo.grid == *grid
            && memo.hold_digest == hold_digest
        {
            return memo.found.clone();
        }
        let found = self.compute_fallbacks(cut, nodes, grid, hold);
        self.fallback_memo = Some(FallbackMemo {
            cut: cut.to_vec(),
            caps: self.caps.clone(),
            residency_epoch: self.residency_epoch,
            applied_epoch: self.applied_index.epoch,
            grid: *grid,
            hold_digest,
            applied_digest: self.applied_digest,
            found: found.clone(),
        });
        found
    }

    pub(super) fn compute_fallbacks(
        &self,
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
        hold: &FastSet<NodeId>,
    ) -> Fallbacks {
        let (with_level, missing): (Vec<NodeId>, Vec<NodeId>) = cut.iter().partition(|id| {
            !hold.contains(*id) && self.has_resident_level(**id, &nodes[*id], grid)
        });
        let mut out = Fallbacks::default();
        if missing.is_empty() {
            return out;
        }
        let rule = CoverRule::new(self, cut, &with_level, nodes, grid);
        let mut seen: FastSet<NodeId> = FastSet::default();
        // Held nodes (R3-Q21) with nothing above them but applied descendants that drew there.
        let mut standing: Vec<NodeId> = Vec::new();
        for id in missing {
            let f = Footprint::new(grid, nodes[&id].bounds);
            match rule.cover(self, &f) {
                Cover::Fallback { ancestor, .. } => {
                    if seen.insert(ancestor) {
                        out.ancestors.push(ancestor);
                    }
                }
                Cover::Nothing => {
                    if rule.applied_ok
                        && hold.contains(&id)
                        && self.applied_index.below.contains_key(&(f.rep, f.lo))
                    {
                        standing.push(id);
                    } else {
                        out.excluded.insert(id);
                    }
                }
                Cover::Cut { .. } => {}
            }
        }
        if (!out.excluded.is_empty() || !standing.is_empty()) && rule.applied_ok {
            let cells: FastSet<(u8, [u64; 3])> = out
                .excluded
                .iter()
                .chain(&standing)
                .map(|id| {
                    let f = Footprint::new(grid, nodes[id].bounds);
                    (f.rep, f.lo)
                })
                .collect();
            // Cells that already draw in the real cut: a descendant under one of them, at any
            // distance, would draw the same zone twice or make the mask impossible
            // (`DescendantTooFine`), so it does not stand in.
            let mut shadow = rule.drawing.clone();
            for ancestor in &out.ancestors {
                let f = Footprint::new(grid, self.applied_index.nodes[ancestor].bounds);
                shadow.insert((f.rep, f.lo));
            }
            let mut found: Vec<NodeId> = self
                .applied
                .iter()
                .filter(|entry| {
                    let id = entry.node;
                    !rule.in_cut.contains(&id)
                        && !seen.contains(&id)
                        && entry.node_anchor.is_some()
                        && self.has_resident_level(id, &self.applied_index.nodes[&id], grid)
                        && {
                            let f = Footprint::new(grid, self.applied_index.nodes[&id].bounds);
                            f.gaps().any(|k| cells.contains(&f.ancestor(k)))
                                && !(1..=f.rep.saturating_sub(3))
                                    .any(|k| shadow.contains(&f.ancestor(k)))
                        }
                })
                .map(|entry| entry.node)
                .collect();
            found.sort();
            out.descendants = found;
            for id in standing {
                let f = Footprint::new(grid, nodes[&id].bounds);
                let stands = self
                    .applied_index
                    .below
                    .get(&(f.rep, f.lo))
                    .is_some_and(|list| list.iter().any(|(d, _)| out.descendants.contains(d)));
                if !stands {
                    out.excluded.insert(id);
                }
            }
        }
        out
    }

    /// Remembers the nodes of the applied cut for the fallback; the epoch moves only when the set
    /// or its data changes.
    pub(super) fn index_applied(&mut self, nodes: &FastMap<NodeId, PoolNode>, grid: &CellGrid) {
        let index = &mut self.applied_index;
        let same = index.grid == Some(*grid)
            && index.nodes.len() == self.applied.len()
            && self
                .applied
                .iter()
                .all(|entry| index.nodes.get(&entry.node) == nodes.get(&entry.node));
        if same {
            return;
        }
        index.epoch += 1;
        index.grid = Some(*grid);
        index.nodes.clear();
        index.cells.clear();
        index.below.clear();
        for entry in &self.applied {
            let data = &nodes[&entry.node];
            let f = Footprint::new(grid, data.bounds);
            for k in f.gaps() {
                index
                    .below
                    .entry(f.ancestor(k))
                    .or_default()
                    .push((entry.node, k));
            }
            index.cells.entry((f.rep, f.lo)).or_insert(entry.node);
            index.nodes.insert(entry.node, data.clone());
        }
    }

    /// R3-Q20: the nodes of the applied cut that drew an octant which the real cut `entries` no
    /// longer draws and nothing covers: an ancestor of the real cut that draws there, or
    /// descendants of it that do. A node whose zone is no longer under any node of the planner's
    /// `cut` has left it and never counts. Built from the cells of the applied index, like the
    /// fallback; a descendant counts as covering an octant it touches.
    pub(super) fn lost_zones(
        &self,
        entries: &[CellCutEntry],
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
    ) -> u32 {
        if self.applied.is_empty() || self.applied_index.grid != Some(*grid) {
            return 0;
        }
        let position: FastMap<NodeId, usize> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.node, index))
            .collect();
        let drew = |entry: &CellCutEntry, o: usize| entry.claims(o);
        // Octants a new entry of the same node does not draw any more.
        let mut pending: Vec<(&CellCutEntry, [bool; 8])> = Vec::new();
        for old in &self.applied {
            let short: [bool; 8] = std::array::from_fn(|o| {
                drew(old, o)
                    && position.get(&old.node).is_none_or(|&index| {
                        let now = &entries[index];
                        !now.claims(o) && now.wanted[o].is_some()
                    })
            });
            if short.iter().any(|s| *s) {
                pending.push((old, short));
            }
        }
        if pending.is_empty() {
            return 0;
        }
        let feet = |id: &NodeId| -> Footprint {
            let bounds = self
                .applied_index
                .nodes
                .get(id)
                .map_or_else(|| nodes[id].bounds, |node| node.bounds);
            Footprint::new(grid, bounds)
        };
        // Cells with a cut node on them or over them, and per cell the octants a cut node below
        // it touches.
        let cut_set: FastSet<NodeId> = cut.iter().copied().collect();
        let mut cut_cells: FastSet<(u8, [u64; 3])> = FastSet::default();
        let mut over_cut: FastMap<(u8, [u64; 3]), [bool; 8]> = FastMap::default();
        let mut cut_under: Below<NodeId> = FastMap::default();
        for id in cut {
            let f = Footprint::new(grid, nodes[id].bounds);
            cut_cells.insert((f.rep, f.lo));
            for k in 1..=f.rep.saturating_sub(3) {
                cut_under.entry(f.ancestor(k)).or_default().push((*id, k));
                let touched = over_cut.entry(f.ancestor(k)).or_insert([false; 8]);
                for a in f.octants_in(k) {
                    touched[a] = true;
                }
            }
        }
        let mut by_cell: FastMap<(u8, [u64; 3]), usize> = FastMap::default();
        // Per wanted cell, the entries under it that draw and how many levels below.
        let mut under: Below<usize> = FastMap::default();
        for (index, entry) in entries.iter().enumerate() {
            let f = Footprint::new(grid, nodes[&entry.node].bounds);
            by_cell.entry((f.rep, f.lo)).or_insert(index);
        }
        let wanted_cells: FastSet<(u8, [u64; 3])> = pending
            .iter()
            .map(|(old, _)| {
                let f = feet(&old.node);
                (f.rep, f.lo)
            })
            .collect();
        for (index, entry) in entries.iter().enumerate() {
            if entry.node_anchor.is_none() {
                continue;
            }
            let f = Footprint::new(grid, nodes[&entry.node].bounds);
            for k in 1..=f.rep.saturating_sub(3) {
                let cell = f.ancestor(k);
                if wanted_cells.contains(&cell) {
                    under.entry(cell).or_default().push((index, k));
                }
            }
        }
        let mut lost = 0;
        for (old, short) in pending {
            let f = feet(&old.node);
            let cell = (f.rep, f.lo);
            let whole = cut_set.contains(&old.node)
                || (1..=f.rep.saturating_sub(3)).any(|k| cut_cells.contains(&f.ancestor(k)));
            let in_cut = |o: usize| whole || over_cut.get(&cell).is_some_and(|t| t[o]);
            let covered = |o: usize| {
                under.get(&cell).is_some_and(|list| {
                    let zone = Zone {
                        f: &f,
                        mask: &old.mask,
                        octant: o,
                        // Only the part of the octant under the cut has to stay drawn.
                        need: (!whole)
                            .then(|| cut_under.get(&cell).map_or(&[][..], |v| v.as_slice())),
                    };
                    octant_covered(&zone, list, entries, nodes, grid)
                }) || (1..=f.rep.saturating_sub(3)).any(|k| {
                    by_cell.get(&f.ancestor(k)).is_some_and(|&index| {
                        let parent = &entries[index];
                        f.octants_of(k, o)
                            .all(|a| parent.claims(a) || parent.wanted[a].is_none())
                    })
                })
            };
            if (0..8).any(|o| short[o] && in_cut(o) && !covered(o)) {
                lost += 1;
            }
        }
        lost
    }
}
