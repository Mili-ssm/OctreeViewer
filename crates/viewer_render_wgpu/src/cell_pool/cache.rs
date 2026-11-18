//! Per-node depth cache: the context of a depth query and its refresh.

use super::*;

pub(super) fn distance_sq(eye: [f64; 3], bounds: SpatialBounds, origin: [f64; 3]) -> f64 {
    (0..3)
        .map(|a| {
            let d = eye[a].clamp(bounds.min()[a] - origin[a], bounds.max()[a] - origin[a]) - eye[a];
            d * d
        })
        .sum()
}

/// `NodeDepths::depth` of a node.
pub(super) fn effective_depth(pose: Pose, bounds: SpatialBounds, origin: [f64; 3]) -> f64 {
    let offset: [f64; 3] = std::array::from_fn(|a| {
        pose.eye[a].clamp(bounds.min()[a] - origin[a], bounds.max()[a] - origin[a]) - pose.eye[a]
    });
    let depth: f64 = (0..3).map(|a| offset[a] * pose.forward[a]).sum();
    let distance = offset.iter().map(|v| v * v).sum::<f64>().sqrt();
    if depth <= 0.0 {
        0.0
    } else {
        depth * depth / distance
    }
}

pub(super) fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// Everything one frame computes from the arguments of `frame`.
pub(super) struct Ctx<'a> {
    pub(super) grid: &'a CellGrid,
    pub(super) origin: [f64; 3],
    pub(super) depths: &'a FastMap<NodeId, NodeDepths>,
    pub(super) evaluations: Cell<u32>,
}

impl Ctx<'_> {
    /// `octant_wanted_depth` of the octant for `data.buildable`, from the cache of the full
    /// range: the coarsest depth that meets the target, else the finest, so a range cut short by
    /// a cap gets the same depth capped at its finest.
    pub(super) fn depth(&self, id: NodeId, octant: usize, data: &CellCutNode) -> u8 {
        self.depths[&id].depths[octant].min(data.buildable.1)
    }

    /// `lo` of the node (`node_view_range`) and its octants in front of the eye, capped like `depth`.
    pub(super) fn far(&self, id: NodeId, data: &CellCutNode) -> (u8, u8) {
        let (far, front) = self.depths[&id].far;
        (far.min(data.buildable.1), front)
    }
}

impl CellPool {
    /// Recomputes the cached octant depths of the cut nodes and the fallback nodes `extra` that
    /// need it. Returns how many were recomputed and, apart for the cut and for `extra`, whether
    /// any depth, or the node data they came from, differs from before.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn refresh_depths(
        &mut self,
        cut: &[NodeId],
        extra: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        pose: Pose,
        view: &ViewProjection,
        grid: &CellGrid,
        origin: [f64; 3],
        target_px: f64,
        settled: bool,
    ) -> (u32, bool, bool) {
        let key = ViewKey {
            target_px,
            viewport: view.viewport(),
            visual_scale: view.visual_scale(),
            grid: *grid,
            origin,
            min_side: self.min_side,
        };
        if self.depth_key != Some(key) {
            self.depth_key = Some(key);
            self.depths.clear();
        }
        self.depths.retain(|id, _| nodes.contains_key(id));
        let (mut refreshed, mut changed, mut extra_changed) = (0, false, false);
        let aspect = f64::from(view.viewport()[0]) / f64::from(view.viewport()[1].max(1));
        let tan_corner = (VERTICAL_FOV * 0.5).tan() * (1.0 + aspect * aspect).sqrt();
        let rotation_limit = ROTATION_SHARE / tan_corner;
        let finest_allowed = self
            .min_side
            .map(|side| representative_depth_for_spacing(grid, side));
        let extra_set: FastSet<NodeId> = extra.iter().copied().collect();
        let mut due: Vec<NodeId> = Vec::new();
        let mut early: Vec<(f64, NodeId)> = Vec::new();
        for id in cut.iter().chain(extra) {
            let data = &nodes[id];
            let current = self.depths.get(id).filter(|cached| cached.node == *data);
            let Some(cached) = current else {
                due.push(*id);
                continue;
            };
            let moved = distance(cached.pose.eye, pose.eye);
            let turned = distance(cached.pose.forward, pose.forward);
            if moved == 0.0 && turned == 0.0 {
                // A hinted entry of a camera that has stopped is redone in full, once: every
                // such entry is due on the first settled frame, so a still pose is exact.
                if settled && !cached.exact {
                    due.push(*id);
                }
                continue;
            }
            // Share of the node's own limit already used; unbounded when the eye is on the node.
            let used = (moved / (cached.scale * DRIFT_FRACTION * cached.depth))
                .max(turned / (cached.scale * rotation_limit));
            if settled || used.is_nan() || used >= 1.0 {
                due.push(*id);
            } else if used >= EARLY_SHARE {
                early.push((used, *id));
            }
        }
        // Nodes close to their limit go first, worst first, within a per-frame budget: a rotating
        // camera would otherwise bring every node to its limit on the same frame.
        early.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        due.extend(
            early
                .into_iter()
                .take((cut.len() / EARLY_FRACTION).max(8))
                .map(|(_, id)| id),
        );
        for id in due {
            let data = &nodes[&id];
            let was = self.depths.get(&id).filter(|cached| cached.node == *data);
            refreshed += 1;
            // A drift refresh starts from the cached answer; a new node or a settled camera
            // searches in full, so a still pose is exactly `octant_wanted_depth`.
            let hints = (!settled)
                .then_some(was)
                .flatten()
                .map(|cached| cached.depths);
            let exact = hints.is_none();
            // Octant mode keeps the geometric probe: it still has the degenerate-probe hole (a box
            // reaching the eye asks for the finest depth), tracked as a follow-up. Only cell
            // mode probes the data extent and gives behind or empty octants their siblings' wish.
            let cell = self.lod_mode == LodMode::Cell;
            let probe = |octant: usize| {
                if cell {
                    data.probe_box(octant)
                } else {
                    ProbeBox::Geometric
                }
            };
            let depths = std::array::from_fn(|octant| match hints {
                Some(hints) => octant_wanted_depth_near(
                    view,
                    grid,
                    origin,
                    data.bounds,
                    octant,
                    probe(octant),
                    data.buildable,
                    target_px,
                    hints[octant],
                ),
                None => octant_wanted_depth_in(
                    view,
                    grid,
                    origin,
                    data.bounds,
                    octant,
                    probe(octant),
                    data.buildable,
                    target_px,
                ),
            });
            let depths = if cell {
                behind_wanted(
                    depths,
                    behind_octants(
                        view,
                        grid,
                        origin,
                        data.bounds,
                        std::array::from_fn(|octant| data.probe_box(octant)),
                    ),
                )
            } else {
                depths
            };
            let depths = match finest_allowed {
                Some(limit) => depths.map(|depth| continuity_depth(data.buildable, depth, limit)),
                None => depths,
            };
            let far = if self.lod_mode == LodMode::Cell {
                let probes = std::array::from_fn(|octant| data.probe_box(octant));
                // `lo` of the node's range: the on-axis bound of `node_view_range` (ADR 0055
                // plan, task 4); a node wholly behind the eye or without data wants the coarsest
                // depth. Its `hi_c` is not used yet: `hi` stays as the octants want it (task 5).
                let far = node_view_range(
                    view,
                    grid,
                    origin,
                    probe_region(data.bounds, probes),
                    data.buildable,
                    target_px,
                    self.min_side,
                )
                .map_or(data.buildable.0, |(lo, _)| lo);
                (
                    finest_allowed
                        .map_or(far, |limit| continuity_depth(data.buildable, far, limit)),
                    octants_in_front(view, origin, data.bounds, probes),
                )
            } else {
                (0, 0)
            };
            let differs = was.is_none_or(|cached| cached.depths != depths || cached.far != far);
            if extra_set.contains(&id) {
                extra_changed |= differs;
            } else {
                changed |= differs;
            }
            let hashed = id.get().wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40;
            self.depths.insert(
                id,
                NodeDepths {
                    node: data.clone(),
                    depths,
                    far,
                    pose,
                    depth: effective_depth(pose, data.bounds, origin),
                    exact,
                    scale: 0.5 + 0.5 * hashed as f64 / (1_u64 << 24) as f64,
                },
            );
        }
        (refreshed, changed, extra_changed)
    }
}
