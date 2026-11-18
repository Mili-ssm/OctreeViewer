//! The frame planner: `frame` and `plan`.

use super::*;

impl CellPool {
    /// Plans one frame for `cut`. `UnknownNode` and `NoStoredLevels` are the caller's errors; a
    /// `DescendantTooFine` is not an error here, it retains the previous cut.
    pub fn frame(
        &mut self,
        cut: &[NodeId],
        node: &dyn Fn(NodeId) -> Option<PoolNode>,
        view: &ViewProjection,
        grid: &CellGrid,
        coordinate_origin: [f64; 3],
        target_px: f64,
    ) -> Result<FramePlan, CellCutError> {
        let mut nodes: FastMap<NodeId, PoolNode> =
            FastMap::with_capacity_and_hasher(cut.len(), Default::default());
        for id in cut {
            let data = node(*id).ok_or(CellCutError::UnknownNode(*id))?;
            if data.stored.is_empty() {
                return Err(CellCutError::NoStoredLevels(*id));
            }
            nodes.insert(*id, data);
        }
        self.frame += 1;
        self.carry = Carry::default();
        let eye: [f64; 3] = view
            .camera()
            .position()
            .map(|value| value / view.visual_scale());
        let camera = view.camera();
        let (position, target) = (camera.position(), camera.target());
        let heading: [f64; 3] = std::array::from_fn(|a| target[a] - position[a]);
        let length = heading.iter().map(|v| v * v).sum::<f64>().sqrt();
        let pose = Pose {
            eye,
            forward: heading.map(|v| if length > 0.0 { v / length } else { 0.0 }),
        };
        let settled = self.last_pose == Some(pose);
        self.last_pose = Some(pose);
        let mut hold: FastSet<NodeId> = self
            .hold
            .iter()
            .filter(|id| nodes.contains_key(id))
            .copied()
            .collect();
        let mut refreshed_total = 0;
        let mut previous: Vec<NodeId> = Vec::new();
        for pass in 1..=self.hold_passes + 1 {
            let found = self.fallbacks(cut, &nodes, grid, &hold);
            let fallback = found.extra();
            for id in &previous {
                nodes.remove(id);
            }
            for id in &fallback {
                nodes.insert(*id, self.applied_index.nodes[id].clone());
            }
            previous.clone_from(&fallback);
            let (refreshed, depths_changed, fallback_changed) = self.refresh_depths(
                cut,
                &fallback,
                &nodes,
                pose,
                view,
                grid,
                coordinate_origin,
                target_px,
                settled,
            );
            refreshed_total += refreshed;
            let depths = std::mem::take(&mut self.depths);
            let result = self.plan(
                cut,
                &found,
                &fallback,
                &nodes,
                &depths,
                (depths_changed, fallback_changed),
                &hold,
                match pass.cmp(&self.hold_passes) {
                    std::cmp::Ordering::Less => Round::Free,
                    std::cmp::Ordering::Equal => Round::Last,
                    std::cmp::Ordering::Greater => Round::Settled,
                },
                view,
                grid,
                coordinate_origin,
            );
            self.depths = depths;
            match result? {
                Pass::Done(mut plan) => {
                    self.hold = hold;
                    plan.counters.depth_refreshes = refreshed_total;
                    return Ok(*plan);
                }
                Pass::Redo(next) => hold = next,
            }
        }
        unreachable!("a settled pass never asks for another")
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn plan(
        &mut self,
        cut: &[NodeId],
        found: &Fallbacks,
        fallback: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        depths: &FastMap<NodeId, NodeDepths>,
        (depths_changed, fallback_changed): (bool, bool),
        hold: &FastSet<NodeId>,
        round: Round,
        view: &ViewProjection,
        grid: &CellGrid,
        coordinate_origin: [f64; 3],
    ) -> Result<Pass, CellCutError> {
        let ctx = Ctx {
            grid,
            origin: coordinate_origin,
            depths,
            evaluations: Cell::new(0),
            capped: {
                #[cfg(test)]
                {
                    !self.uncapped_wish
                }
                #[cfg(not(test))]
                {
                    true
                }
            },
        };
        let eye: [f64; 3] = view
            .camera()
            .position()
            .map(|value| value / view.visual_scale());
        let frame = self.frame;
        if self.sizes.len() > self.sizes_prune_at {
            let levels = &self.levels;
            self.sizes
                .retain(|key, _| nodes.contains_key(&key.node) || levels.contains_key(key));
            self.sizes_prune_at = (self.sizes.len() * 2).max(1024);
        }
        self.fresh.clear();
        let mut page_frees = std::mem::take(&mut self.carry.page_frees);
        self.release(frame, &mut page_frees);

        // Wanted cut and fit, reused while nothing they read has changed.
        // E7: the D5 depth of every node of the real cut with descendants in the wanted cut is
        // protected from eviction and requested before anything else; the fit counts its pages.
        let d5 = self.d5_keys(cut, &found.ancestors, nodes, grid);
        let reusable = !depths_changed
            && self.wanted_memo.as_ref().is_some_and(|memo| {
                let moved = distance(memo.eye, eye);
                memo.cut == cut
                    && memo.size_epoch == self.size_epoch
                    && memo.d5 == d5
                    && !(memo.had_caps && moved > DRIFT_FRACTION * memo.min_depth && moved > 0.0)
            });
        if !reusable {
            let mut offenders = BTreeSet::new();
            let (mut needed, mut requests, overflow) =
                self.fit(cut, nodes, &mut offenders, eye, &ctx, &d5)?;
            needed.retain(|key| !offenders.contains(&key.node));
            requests.retain(|key| !offenders.contains(&key.node));
            self.wanted_generation += 1;
            let min_depth = cut
                .iter()
                .map(|id| {
                    effective_depth(
                        Pose {
                            eye,
                            forward: self.last_pose.map_or([0.0; 3], |p| p.forward),
                        },
                        nodes[id].bounds,
                        coordinate_origin,
                    )
                })
                .fold(f64::INFINITY, f64::min);
            self.wanted_memo = Some(WantedMemo {
                cut: cut.to_vec(),
                size_epoch: self.size_epoch,
                generation: self.wanted_generation,
                needed: Rc::new(needed),
                requests: Rc::new(requests),
                offenders,
                overflow,
                eye,
                min_depth,
                had_caps: !self.caps.is_empty(),
                d5: d5.clone(),
            });
        }
        let memo = self.wanted_memo.as_ref().expect("set above");
        let (overflow, generation, needed, requests) = (
            memo.overflow,
            memo.generation,
            Rc::clone(&memo.needed),
            Rc::clone(&memo.requests),
        );
        if overflow && self.overflow_frame != frame {
            self.overflow += 1;
            self.overflow_frame = frame;
        }
        self.carry.wanted_reused.get_or_insert(reusable);
        let caps = self.caps.clone();

        // Real cut on the resident levels.
        let hold_digest = set_digest(hold);
        let real_reused = !fallback_changed
            && self.real_memo.as_ref().is_some_and(|memo| {
                memo.generation == generation
                    && memo.residency_epoch == self.residency_epoch
                    && memo.fallback == fallback
                    && memo.excluded == found.excluded
                    && memo.hold_digest == hold_digest
                    && memo.applied_digest == self.applied_digest
            });
        if !real_reused {
            // Excluded nodes (R3-Q20) have no level and nothing that draws them: out of the cut.
            let live: Vec<NodeId>;
            let live_cut: &[NodeId] = if found.excluded.is_empty() {
                cut
            } else {
                live = cut
                    .iter()
                    .filter(|id| !found.excluded.contains(id))
                    .copied()
                    .collect();
                &live
            };
            let joined: Vec<NodeId>;
            let real_cut: &[NodeId] = if fallback.is_empty() {
                live_cut
            } else {
                joined = live_cut.iter().chain(fallback).copied().collect();
                &joined
            };
            let kept = std::cell::RefCell::new(Vec::new());
            let lookup = |id: NodeId| {
                let data = nodes.get(&id)?;
                let stored = capped(&data.stored, caps.get(&id));
                let resident = if hold.contains(&id) {
                    Vec::new()
                } else {
                    stored
                        .iter()
                        .copied()
                        .filter(|depth| self.is_resident(self.key(id, *depth)))
                        .collect()
                };
                let mut node = CellCutNode {
                    bounds: data.bounds,
                    buildable: capped_range(data.buildable, caps.get(&id)),
                    stored,
                    resident,
                };
                if !hold.contains(&id)
                    && let Some(cap) = caps.get(&id)
                    && let Some(old) = self.finer_drawn(id, *cap, &node, &needed, &ctx)
                {
                    node.resident = old;
                    kept.borrow_mut().push(id);
                }
                Some(node)
            };
            ctx.evaluations.set(ctx.evaluations.get() + 1);
            let entries = match cell_cut_ranged(
                real_cut,
                &lookup,
                grid,
                &|id, o, data| ctx.depth(id, o, data),
                Some(&|id, data| ctx.far(id, data)),
                Some(&|id| match nodes.get(&id) {
                    Some(data) => self.prev_state_seeded(
                        id,
                        data.bounds,
                        capped_range(data.buildable, caps.get(&id)),
                        grid,
                    ),
                    None => self.prev_state(id),
                }),
            ) {
                Ok(entries) => Some(entries),
                Err(CellCutError::DescendantTooFine { .. }) => None,
                Err(other) => return Err(other),
            };
            let planned_missing = entries.as_deref().map_or(cut.len() as u32, |entries| {
                missing_levels(entries) + found.excluded.len() as u32
            });
            // An excluded node that is too fine for an ancestor of the cut is never requested (its
            // level would make the real cut fail with `DescendantTooFine`, and the ancestor,
            // which is requested, draws its zone): it must not keep the previous cut by itself.
            let waits_for_level = found.excluded.iter().any(|id| {
                self.wanted_memo
                    .as_ref()
                    .is_none_or(|memo| !memo.offenders.contains(id))
            });
            let (lost, blank) = entries.as_deref().map_or((0, false), |entries| {
                (
                    self.lost_zones(entries, cut, nodes, grid),
                    entries.is_empty() && waits_for_level && !self.applied.is_empty(),
                )
            });
            let (_, covered_octants) =
                entries.as_deref().map_or((cut.len() as u32, 0), |entries| {
                    uncovered_levels(entries, nodes, grid)
                });
            let eval = entries.as_deref().map_or_else(
                || HoldEval {
                    hold: hold.clone(),
                    settle: hold.clone(),
                    ..HoldEval::default()
                },
                |entries| {
                    self.eval_hold(entries, live_cut, nodes, grid, hold, &|id, o, data| {
                        ctx.depth(id, o, data)
                    })
                },
            );
            self.real_memo = Some(RealMemo {
                generation,
                residency_epoch: self.residency_epoch,
                entries,
                planned_missing,
                covered_octants,
                fallback: fallback.to_vec(),
                excluded: found.excluded.clone(),
                lost,
                blank,
                hold_digest,
                applied_digest: self.applied_digest,
                kept_finer: {
                    let mut kept = kept.into_inner();
                    kept.sort();
                    kept.dedup();
                    kept
                },
                next_hold: eval.hold,
                settle: eval.settle,
                hold_ancestors: eval.ancestors,
                applied: false,
            });
        }
        let memo = self.real_memo.as_mut().expect("set above");
        if round != Round::Settled && memo.next_hold != *hold {
            let next = if round == Round::Last {
                // No fixed point: keep held what is held in either set and has a covering
                // ancestor (a drawn octant never gets coarser), and count it.
                self.carry.unsettled = memo
                    .settle
                    .iter()
                    .filter(|id| hold.contains(id) != memo.next_hold.contains(id))
                    .count() as u32;
                memo.settle.clone()
            } else {
                memo.next_hold.clone()
            };
            if next != *hold {
                self.carry.page_frees = page_frees;
                self.carry.evaluations += ctx.evaluations.get();
                return Ok(Pass::Redo(next));
            }
        }
        let (too_fine, planned_missing, lost) =
            (memo.entries.is_none(), memo.planned_missing, memo.lost);
        let apply = if !too_fine && !overflow && memo.lost == 0 && !memo.blank {
            CutApply::Applied
        } else {
            CutApply::RetainedPrevious
        };
        // An applied cut loses no zone, and a held node has its zone drawn by an ancestor or by
        // descendants of the real cut (nodes without a level may wait: R3-Q20).
        debug_assert!(
            apply != CutApply::Applied
                || memo.entries.as_ref().is_none_or(|entries| {
                    let position: FastMap<NodeId, usize> = entries
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| (entry.node, index))
                        .collect();
                    memo.lost == 0
                        && !memo.blank
                        && hold
                            .iter()
                            .filter(|id| !found.excluded.contains(id))
                            .all(|id| {
                                let f = Footprint::new(grid, nodes[id].bounds);
                                // A node held by the D5 gate is covered by an ancestor of the
                                // real cut itself.
                                f.gaps().any(|gap| {
                                    entries.iter().any(|a| {
                                        let af = Footprint::new(grid, nodes[&a.node].bounds);
                                        (af.rep, af.lo) == f.ancestor(gap)
                                            && covers_in(entries, &position, a.node, &f, gap)
                                    })
                                }) || f.gaps().any(|gap| {
                                    self.applied_index
                                        .cells
                                        .get(&f.ancestor(gap))
                                        .is_some_and(|a| covers_in(entries, &position, *a, &f, gap))
                                }) || self.applied_index.below.get(&(f.rep, f.lo)).is_some_and(
                                    |list| list.iter().any(|(d, _)| position.contains_key(d)),
                                )
                            })
                }),
            "an applied cut loses a zone, or a held node has nothing drawing its zone"
        );
        // Levels in use: the drawn ones and, when retaining, the previous cut's.
        let mut protected: FastSet<LevelKey> = FastSet::default();
        let mut pinned: FastSet<LevelKey> = FastSet::default();
        let mut step_hops = 0;
        if apply == CutApply::Applied {
            if let Some(memo) = self.real_memo.as_ref().filter(|memo| !memo.applied)
                && let Some(entries) = memo.entries.as_deref()
            {
                step_hops = self.step_hops(entries);
            }
            let memo = self.real_memo.as_mut().expect("set above");
            let fresh = !memo.applied;
            memo.applied = true;
            self.applied_covered = memo.covered_octants;
            if fresh {
                self.applied = memo.entries.clone().expect("applied cut exists");
                self.applied_drawn = self
                    .applied
                    .iter()
                    .map(|entry| (entry.node, entry.drawn))
                    .collect();
                self.applied_window = self
                    .applied
                    .iter()
                    .filter_map(|entry| {
                        let (lo, hi) = self.applied_span.get(&entry.node)?;
                        Some((entry.node, (lo.saturating_sub(1), hi.saturating_add(1))))
                    })
                    .collect();
                self.applied_digest = drawn_digest(&self.applied);
                self.applied_cover.clone_from(&memo.hold_ancestors);
                self.index_applied(nodes, grid);
            }
        }
        // The wait of each stepping octant of what is on screen: every frame counts. At the
        // limit the cut is planned again, so the octants hop (`prev_state`).
        let mut waits = FastMap::default();
        let mut expired = false;
        for entry in &self.applied {
            for o in 0..8 {
                if let (Some(w), Some(drawn), Some(next)) =
                    (entry.wanted[o], entry.drawn[o], entry.step[o])
                {
                    let key = (drawn, w > drawn, next);
                    let frames = match self.step_waits.get(&(entry.node, o)) {
                        Some(&(waiting, frames)) if waiting == key => frames + 1,
                        // This frame is the first of the wait.
                        _ => 1,
                    };
                    expired |= frames == STEP_WAIT_FRAMES;
                    waits.insert((entry.node, o), (key, frames));
                }
            }
        }
        self.step_waits = waits;
        if expired {
            self.residency_epoch += 1;
        }
        for entry in &self.applied {
            for depth in entry.required_levels() {
                protected.insert(self.key(entry.node, depth));
            }
        }
        for id in cut.iter().chain(&found.ancestors) {
            // The coarsest level that can draw in the node's span, stored or not yet.
            let depth = {
                let node = &nodes[id];
                let stored = capped(&node.stored, caps.get(id));
                self.drawable_stored(&stored, grid, node.bounds)
                    .first()
                    .copied()
                    .or_else(|| {
                        let buildable = capped_range(node.buildable, caps.get(id));
                        self.coarsest_in_span(buildable, grid, node.bounds)
                    })
            };
            if let Some(depth) = depth {
                pinned.insert(self.key(*id, depth));
            }
        }
        if apply == CutApply::Applied {
            self.applied_pins = pinned.clone();
        } else {
            pinned.extend(self.applied_pins.iter().copied());
        }
        // Overflow keeps only the previous cut: nothing of the new one is requested.
        let no_levels = FastSet::default();
        let want: &FastSet<LevelKey> = if overflow { &no_levels } else { &needed };
        // ADR 0053: the adjacent depth each stepping octant waits for is requested right after
        // D5, and kept from eviction while it loads.
        let steps: FastSet<LevelKey> = if overflow {
            FastSet::default()
        } else {
            self.real_memo
                .as_ref()
                .and_then(|memo| memo.entries.as_deref())
                .into_iter()
                .flatten()
                .flat_map(|entry| {
                    entry
                        .step
                        .iter()
                        .flatten()
                        .map(|d| self.key(entry.node, *d))
                })
                .chain(
                    self.step_waits
                        .iter()
                        .filter(|((node, _), _)| nodes.contains_key(node))
                        .map(|((node, _), ((_, _, next), _))| self.key(*node, *next)),
                )
                .collect()
        };
        let stored_step = |key: &LevelKey| {
            nodes
                .get(&key.node)
                .is_some_and(|node| node.stored.contains(&key.depth))
        };
        protected.extend(want.iter().copied());
        protected.extend(steps.iter().filter(|key| stored_step(key)).copied());
        protected.extend(d5.iter().copied());
        protected.extend(pinned.iter().copied());
        self.pinned = pinned;

        // Cancel what nobody needs any more.
        let mut freed_now = Vec::new();
        let cancelled: Vec<LevelKey> = self
            .levels
            .iter()
            .filter(|(key, level)| !level.is_resident() && !protected.contains(key))
            .map(|(key, _)| *key)
            .collect();
        for key in cancelled {
            let before = self.known_pages(key);
            if let Some(level) = self.levels.remove(&key) {
                if before != self.known_pages(key) {
                    self.size_epoch += 1;
                }
                if let LevelState::Uploading { slots, .. } = level.state {
                    freed_now.extend(slots);
                }
                self.events.push(CellPoolEvent::Cancel(key));
            }
        }
        if !freed_now.is_empty() {
            self.pending.push((frame, freed_now));
        }

        // Use: LRU stamps.
        for key in &protected {
            if let Some(level) = self.levels.get_mut(key) {
                level.last_used = frame;
            }
        }

        // Requests.
        let mut order: Vec<(LevelKey, Need)> = Vec::new();
        let mut fetch_requests = Vec::new();
        let mut drawable: FastSet<NodeId> = FastSet::default();
        for key in self
            .levels
            .iter()
            .filter(|(key, l)| {
                l.is_resident() && caps.get(&key.node).is_none_or(|cap| key.depth <= *cap)
            })
            .map(|(k, _)| *k)
        {
            // A level outside the node's span never draws.
            if !nodes.get(&key.node).is_some_and(|node| {
                in_cell_range(&[key.depth], Self::node_depth(grid, node.bounds)).len() == 1
            }) {
                continue;
            }
            drawable.insert(key.node);
        }
        let mut wanted_keys: Vec<LevelKey> = if overflow {
            want.iter().copied().collect()
        } else {
            want.iter()
                .chain(&d5)
                .chain(steps.iter().filter(|key| stored_step(key)))
                .copied()
                .collect()
        };
        wanted_keys.sort();
        wanted_keys.dedup();
        for key in wanted_keys {
            let data = &nodes[&key.node];
            if let std::collections::hash_map::Entry::Vacant(slot) = self.levels.entry(key) {
                slot.insert(Level {
                    state: LevelState::Requested,
                    data: None,
                    last_used: frame,
                    last_active: 0,
                    refresh: None,
                });
                fetch_requests.push(key);
            }
            if !self.levels[&key].is_resident() {
                let coarsest_drawable = self
                    .drawable_stored(
                        &capped(&data.stored, caps.get(&key.node)),
                        grid,
                        data.bounds,
                    )
                    .first()
                    .copied();
                let first = d5.contains(&key)
                    || (!drawable.contains(&key.node) && coarsest_drawable == Some(key.depth));
                order.push((
                    key,
                    Need {
                        first,
                        step: steps.contains(&key),
                        distance: distance_sq(eye, data.bounds, coordinate_origin),
                    },
                ));
            }
        }

        // Coarse to fine (R3-Q19): the first level of the nodes that draw nothing, so no region
        // starves behind refinements elsewhere; then level depth, so a node's level is not
        // requested before the one its covering ancestor needs; then distance.
        order.sort_by(|(ka, a), (kb, b)| {
            (!a.first)
                .cmp(&!b.first)
                .then((!a.step).cmp(&!b.step))
                .then(ka.depth.cmp(&kb.depth))
                .then(a.distance.total_cmp(&b.distance))
                .then(ka.node.cmp(&kb.node))
        });
        let fresh: FastSet<LevelKey> = fetch_requests.iter().copied().collect();
        fetch_requests = order
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| fresh.contains(key))
            .collect();

        // Missing levels the view wants and the node can build, coarse to fine like the fetches.
        // They hold no pool state: the level arrives as a stored one (`add_packed_levels`).
        let mut level_requests: Vec<LevelKey> = if overflow {
            Vec::new()
        } else {
            requests.iter().copied().collect()
        };
        for key in &d5 {
            let stored = nodes
                .get(&key.node)
                .is_some_and(|node| node.stored.contains(&key.depth));
            if !overflow && !stored && !level_requests.contains(key) {
                level_requests.push(*key);
            }
        }
        for key in &steps {
            if !overflow && !stored_step(key) && !level_requests.contains(key) {
                level_requests.push(*key);
            }
        }
        level_requests.sort_by(|a, b| {
            let distance =
                |key: &LevelKey| distance_sq(eye, nodes[&key.node].bounds, coordinate_origin);
            (!d5.contains(a))
                .cmp(&!d5.contains(b))
                .then((!steps.contains(a)).cmp(&!steps.contains(b)))
                .then(a.depth.cmp(&b.depth))
                .then(distance(a).total_cmp(&distance(b)))
                .then(a.node.cmp(&b.node))
        });

        // Eviction: on demand for what is missing, and down to the soft limit.
        let missing: usize = order
            .iter()
            .map(|(key, _)| {
                let level = &self.levels[key];
                match &level.state {
                    LevelState::Uploading { .. } => 0,
                    _ => level
                        .pages()
                        .unwrap_or_else(|| self.pages_of(*key, &nodes[&key.node], grid)),
                }
            })
            .sum();
        let soft = (self.usable() * SOFT_PERCENT / 100) as usize;
        let mut candidates: Vec<(u64, LevelKey)> = self
            .levels
            .iter()
            .filter(|(key, level)| level.is_resident() && !protected.contains(key))
            .map(|(key, level)| (level.last_used, *key))
            .collect();
        candidates.sort();
        for (_, key) in candidates {
            let short = self.free.len() + self.pending_pages() < missing;
            if !short && self.allocated_pages() <= soft {
                break;
            }
            self.evict_resident(key, frame);
        }
        // The finer levels kept for capped nodes (`finer_drawn`) are protected while they draw;
        // when the capped levels still lack pages after every other eviction, they are given up
        // (the real cut drops to the resident capped levels next frame, which unprotects them).
        if self.free.len() + self.pending_pages() < missing
            && let Some(memo) = self.real_memo.as_ref().filter(|memo| memo.applied)
            && !memo.kept_finer.is_empty()
        {
            let kept = memo.kept_finer.clone();
            for id in kept {
                if let Some(cap) = caps.get(&id) {
                    self.old_released.insert(id, *cap);
                }
            }
            self.residency_epoch += 1;
        }
        self.release(frame, &mut page_frees);

        // Uploads, spread over frames, in the same order.
        let mut budget = (self.config.upload_budget_bytes / POOL_PAGE_BYTES).max(1) as usize;
        let mut uploads = Vec::new();
        let mut uploaded_pages = self.refresh_uploads(&mut budget, &mut uploads);
        let next_free = self.pending_pages_by(frame + 1);
        let mut blocked = false;
        for (key, _) in &order {
            if budget == 0 {
                break;
            }
            let Some(pages) = self.levels[key].pages() else {
                continue;
            };
            if matches!(self.levels[key].state, LevelState::Requested) {
                if blocked {
                    continue;
                }
                if self.free.len() < pages {
                    // Pages about to be released: nobody of lower priority takes the free ones.
                    blocked = self.free.len() + next_free >= pages;
                    continue;
                }
                let slots: Vec<u32> = (0..pages)
                    .map(|_| self.free.pop_first().expect("checked above"))
                    .collect();
                self.levels.get_mut(key).expect("level").state = LevelState::Uploading {
                    slots,
                    pages_done: 0,
                };
            }
            let level = self.levels.get_mut(key).expect("level");
            let LevelState::Uploading { slots, pages_done } = &mut level.state else {
                continue;
            };
            let count = budget.min(pages - *pages_done);
            let range = *pages_done..*pages_done + count;
            uploads.push(PageUpload {
                key: *key,
                slots: slots[range.clone()].to_vec(),
                page_range: range,
            });
            *pages_done += count;
            budget -= count;
            uploaded_pages += count;
            if *pages_done == pages {
                let slots = std::mem::take(slots);
                level.state = LevelState::Resident { slots };
                self.residency_epoch += 1;
                self.fresh.insert(*key);
            }
        }

        // Masks and active levels of the cut being drawn.
        let mut mask_updates = Vec::new();
        let mut active = Vec::new();
        if apply == CutApply::Applied {
            let live: FastSet<NodeId> = self.applied.iter().map(|entry| entry.node).collect();
            let gone: Vec<NodeId> = self
                .mask_slots
                .keys()
                .filter(|id| !live.contains(id))
                .copied()
                .collect();
            for id in gone {
                if let Some(old) = self.mask_slots.remove(&id) {
                    self.free_masks.insert(old.slot);
                }
            }
            let mut finest = FastMap::default();
            let mut spans = FastMap::default();
            let mut d5_depths = FastMap::default();
            for index in 0..self.applied.len() {
                let (id, mask) = (self.applied[index].node, self.applied[index].mask);
                let bounds = nodes
                    .get(&id)
                    .or_else(|| self.applied_index.nodes.get(&id))
                    .map(|node| node.bounds);
                debug_assert!(bounds.is_some(), "a node of the applied cut has bounds");
                let Some(bounds) = bounds else {
                    continue;
                };
                let node_depth = Self::node_depth(grid, bounds);
                let d5 = self.d5_active(&self.applied[index], false);
                if let Some(d5) = d5 {
                    d5_depths.insert(id, d5);
                }
                // Exactly the depths emitted below as `ActiveLevel`s of this node.
                let active_bits = self
                    .cell_active_depths(&self.applied[index], node_depth, d5)
                    .into_iter()
                    .fold(0_u64, |bits, depth| bits | 1 << (depth - node_depth));
                if active_bits != 0 {
                    let top = node_depth + (63 - active_bits.leading_zeros()) as u8;
                    finest.insert(id, top);
                    spans.insert(id, (node_depth + active_bits.trailing_zeros() as u8, top));
                }
                let next = |slot: u32| MaskSlot {
                    slot,
                    mask,
                    active: active_bits,
                    node_depth,
                    stale: false,
                };
                let update = |slot: u32| MaskUpdate {
                    slot,
                    mask,
                    active: active_bits,
                    node_depth,
                    node_lo: std::array::from_fn(|axis| {
                        crate::node_cells::node_range(grid, bounds, node_depth, axis).0
                    }),
                };
                match self.mask_slots.get(&id) {
                    // A node is sent when its mask, active depths or node depth change.
                    Some(old)
                        if !old.stale
                            && old.mask == mask
                            && (old.active, old.node_depth) == (active_bits, node_depth) => {}
                    Some(old) => {
                        let slot = old.slot;
                        mask_updates.push(update(slot));
                        self.mask_slots.insert(id, next(slot));
                    }
                    None => {
                        let slot = self.alloc_mask();
                        mask_updates.push(update(slot));
                        self.mask_slots.insert(id, next(slot));
                    }
                }
            }
            self.applied_d5 = d5_depths;
            self.applied_finest = finest;
            self.applied_span = spans;
        }
        let mut applied_levels = FastSet::default();
        for entry in &self.applied {
            let Some(MaskSlot { slot, .. }) = self.mask_slots.get(&entry.node) else {
                continue;
            };
            let cell_depths = {
                // Retained frames keep the D5 depth the records were last written with.
                let d5 = self.d5_active(entry, true);
                let node_depth = nodes
                    .get(&entry.node)
                    .or_else(|| self.applied_index.nodes.get(&entry.node))
                    .map(|node| Self::node_depth(grid, node.bounds));
                node_depth.map_or_else(Vec::new, |d| self.cell_active_depths(entry, d, d5))
            };
            // Active depths the node does not draw itself (its D5 depth) come last.
            let extra = {
                let drawn: Vec<u8> = entry.required_levels();
                cell_depths
                    .iter()
                    .filter(|depth| !drawn.contains(depth))
                    .map(|depth| (*depth, 0xff))
                    .collect::<Vec<_>>()
            };
            for (depth, _) in entry.draws().into_iter().chain(extra) {
                if !cell_depths.contains(&depth) {
                    continue;
                }
                let key = self.key(entry.node, depth);
                if !self.is_resident(key) {
                    continue;
                }
                applied_levels.insert(key);
                active.push(ActiveLevel {
                    key,
                    // The GPU decides per cell which of the node's active levels draws.
                    octant_bits: 0xff,
                    mask_slot: *slot,
                });
            }
        }
        for key in &applied_levels {
            if let Some(level) = self.levels.get_mut(key) {
                level.last_active = frame;
            }
        }
        self.applied_levels = applied_levels;

        let cover: FastSet<NodeId> = fallback
            .iter()
            .chain(&self.applied_cover)
            .copied()
            .collect();
        let fallback_pages = self
            .applied
            .iter()
            .filter(|entry| cover.contains(&entry.node))
            .flat_map(|entry| {
                entry
                    .required_levels()
                    .into_iter()
                    .map(|depth| self.key(entry.node, depth))
            })
            .filter_map(|key| self.known_pages(key))
            .sum::<usize>() as u32;
        // Excluded nodes too fine for an ancestor of the cut are never requested (see
        // `waits_for_level`): the ancestor draws their zone, coarser, so they are a stage and not
        // a node waiting for a level (otherwise the counters and `busy` would never settle).
        let too_fine_excluded: FastSet<NodeId> =
            self.wanted_memo
                .as_ref()
                .map_or_else(FastSet::default, |memo| {
                    found
                        .excluded
                        .iter()
                        .filter(|id| memo.offenders.contains(id))
                        .copied()
                        .collect()
                });
        let waiting_excluded = found.excluded.len() as u32 - too_fine_excluded.len() as u32;
        // A held or excluded node that waited last frame too is one frame older.
        let waiting: FastMap<NodeId, u32> = hold
            .iter()
            .chain(
                found
                    .excluded
                    .iter()
                    .filter(|id| !too_fine_excluded.contains(id)),
            )
            .map(|id| (*id, self.wait_ages.get(id).map_or(1, |age| age + 1)))
            .collect();
        let age_max = waiting.values().copied().max().unwrap_or(0);
        self.wait_ages = waiting;
        // The cut being drawn: after a retained frame it is the previous one, so the cut nodes it
        // does not hold are the ones without a level.
        // A held node whose octants are all hidden by its descendants has no unmasked octant for
        // `missing_levels` to count, but it is still waiting.
        let hidden_held = if apply == CutApply::Applied {
            self.applied
                .iter()
                .filter(|e| hold.contains(&e.node) && e.wanted.iter().all(Option::is_none))
                .count() as u32
        } else {
            0
        };
        let absent = if apply == CutApply::Applied {
            waiting_excluded
        } else {
            cut.iter()
                .filter(|id| {
                    !self.applied_drawn.contains_key(id) && !too_fine_excluded.contains(id)
                })
                .count() as u32
        };
        let counters = PoolCounters {
            stages: self
                .applied
                .iter()
                .map(|entry| entry.stage.iter().filter(|s| **s).count() as u32)
                .sum::<u32>()
                + self.applied_covered
                + 8 * too_fine_excluded.len() as u32,
            nodes_without_level: if self.applied.is_empty() {
                planned_missing
            } else {
                missing_levels(&self.applied) + absent + hidden_held
            },
            pool_coarsened: caps.len() as u32,
            pool_overflow: self.overflow,
            resident_pages: self.resident_pages(),
            cut_evaluations: self.carry.evaluations + ctx.evaluations.get(),
            descendant_too_fine: too_fine,
            uploaded_bytes: uploaded_pages as u64 * POOL_PAGE_BYTES,
            depth_refreshes: 0,
            wanted_reused: self.carry.wanted_reused.unwrap_or(reusable),
            real_reused,
            held_nodes: hold.len() as u32,
            fallback_pages,
            hold_unsettled: self.carry.unsettled,
            excluded_nodes: absent,
            lost_nodes: if apply == CutApply::Applied { 0 } else { lost },
            hold_age_max: age_max,
            step_hops,
        };
        Ok(Pass::Done(Box::new(FramePlan {
            apply,
            entries: self.applied.clone(),
            fetch_requests,
            level_requests,
            uploads,
            page_frees,
            mask_updates,
            active,
            events: std::mem::take(&mut self.events),
            counters,
            coarsened_nodes: {
                let mut ids: Vec<NodeId> = caps.keys().copied().collect();
                ids.sort_unstable();
                ids
            },
        })))
    }
}
