use std::collections::{BTreeSet, HashMap, HashSet};

use crate::{
    DecodedBlockCache, HierarchySemantics, LodSelection, NodeId, ResidentBlockSource,
    SpatialHierarchy,
};

/// Renderable block cut after reconciling an LOD target with decoded cache contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LodTransitionPlan {
    node_ids: Vec<NodeId>,
    block_ids: Vec<crate::BlockId>,
    protected_block_ids: Vec<crate::BlockId>,
    point_count: u64,
    point_budget: u64,
    pending_blocks: Vec<crate::BlockId>,
    fallback_nodes: Vec<NodeId>,
    uncovered_nodes: Vec<NodeId>,
    budget_omitted_nodes: Vec<NodeId>,
    target_complete: bool,
}

impl LodTransitionPlan {
    pub fn node_ids(&self) -> &[NodeId] {
        &self.node_ids
    }

    pub fn block_ids(&self) -> &[crate::BlockId] {
        &self.block_ids
    }

    pub const fn point_count(&self) -> u64 {
        self.point_count
    }

    pub const fn point_budget(&self) -> u64 {
        self.point_budget
    }

    /// Missing target payloads, even when an available parent currently covers them.
    pub fn pending_blocks(&self) -> &[crate::BlockId] {
        &self.pending_blocks
    }

    /// Available fallback payloads used in place of an incomplete target refinement.
    pub fn fallback_nodes(&self) -> &[NodeId] {
        &self.fallback_nodes
    }

    /// Target regions whose full coverage cannot currently be established.
    pub fn uncovered_nodes(&self) -> &[NodeId] {
        &self.uncovered_nodes
    }

    /// Resolved nodes omitted because their resident payloads exceed the point budget.
    pub fn budget_omitted_nodes(&self) -> &[NodeId] {
        &self.budget_omitted_nodes
    }

    pub fn coverage_complete(&self) -> bool {
        self.uncovered_nodes.is_empty()
    }
}

/// Keeps the last fully covered cut available while a newer target is loading.
///
/// Create one planner per dataset and call [`Self::reset`] when replacing it.
/// Incomplete target plans retain the prior complete cut by region and protect
/// its resident payloads. The cache stays within its configured byte budget; if
/// it cannot admit a new block alongside that cut, insertion is rejected.
#[derive(Default)]
pub struct LodTransitionPlanner {
    retained_nodes: Vec<NodeId>,
}

impl LodTransitionPlanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolves target nodes against cache residency without changing the active cut
    /// or cache protection. Call [`Self::commit`] after downstream consumers accept it.
    pub fn prepare<S: ResidentBlockSource + ?Sized>(
        &self,
        hierarchy: &SpatialHierarchy,
        target: &LodSelection,
        cache: &S,
    ) -> LodTransitionPlan {
        let target_nodes = target.node_ids().iter().copied().collect::<HashSet<_>>();
        let retained = RetainedIndex::new(hierarchy, &self.retained_nodes);
        let mut branch_nodes = HashSet::new();
        for target_id in &target_nodes {
            let mut current = Some(*target_id);
            while let Some(id) = current {
                if !branch_nodes.insert(id) {
                    break;
                }
                current = hierarchy.node(id).and_then(|node| node.parent());
            }
        }
        let retain_previous = target.node_ids().iter().any(|node_id| {
            hierarchy.node(*node_id).is_some_and(|node| {
                node.blocks()
                    .iter()
                    .any(|block_id| !cache.contains(*block_id))
            })
        });

        let mut result = if hierarchy.semantics() == HierarchySemantics::Replacement {
            resolve_replacement(
                hierarchy,
                hierarchy.root_id(),
                &target_nodes,
                &branch_nodes,
                &retained,
                retain_previous,
                cache,
            )
        } else {
            resolve_additive(
                hierarchy,
                &target_nodes,
                &retained.nodes,
                retain_previous,
                cache,
            )
        };

        result.nodes.sort_unstable();
        result.fallback_nodes.sort_unstable();
        result.uncovered_nodes.sort_unstable();
        let mut admitted_nodes = Vec::new();
        let mut admitted_blocks = BTreeSet::new();
        let mut point_count = 0_u64;
        let mut budget_omitted_nodes = Vec::new();
        for node_id in result.nodes {
            let node = hierarchy
                .node(node_id)
                .expect("resolved cut refers to a hierarchy node");
            let mut candidate_blocks = HashSet::new();
            let new_blocks = node
                .blocks()
                .iter()
                .copied()
                .filter(|block_id| {
                    !admitted_blocks.contains(block_id) && candidate_blocks.insert(*block_id)
                })
                .collect::<Vec<_>>();
            let added_points = new_blocks.iter().fold(0_u64, |sum, block_id| {
                let block = cache
                    .peek(*block_id)
                    .expect("resolved cut blocks are resident");
                sum.saturating_add(u64::try_from(block.points().len()).unwrap_or(u64::MAX))
            });
            let Some(candidate_count) = point_count.checked_add(added_points) else {
                budget_omitted_nodes.push(node_id);
                continue;
            };
            if candidate_count > target.point_budget() {
                budget_omitted_nodes.push(node_id);
                continue;
            }
            point_count = candidate_count;
            admitted_nodes.push(node_id);
            admitted_blocks.extend(new_blocks);
        }
        let mut uncovered_nodes = result.uncovered_nodes.into_iter().collect::<BTreeSet<_>>();
        if !budget_omitted_nodes.is_empty() {
            // A target is uncovered when it is, contains or lies inside an omitted node.
            // One ancestor walk per node replaces the targets x omitted pairwise check.
            let omitted = budget_omitted_nodes.iter().copied().collect::<HashSet<_>>();
            for omitted_id in &omitted {
                if target_nodes.contains(omitted_id) {
                    uncovered_nodes.insert(*omitted_id);
                }
                for ancestor in ancestors(hierarchy, *omitted_id) {
                    if target_nodes.contains(&ancestor) {
                        uncovered_nodes.insert(ancestor);
                    }
                }
            }
            for target_id in &target_nodes {
                if ancestors(hierarchy, *target_id).any(|ancestor| omitted.contains(&ancestor)) {
                    uncovered_nodes.insert(*target_id);
                }
            }
        }
        let admitted_node_ids = admitted_nodes.iter().copied().collect::<HashSet<_>>();
        let mut protected_block_ids = admitted_nodes
            .iter()
            .filter_map(|id| hierarchy.node(*id))
            .filter(|node| {
                let mut parent = node.parent();
                while let Some(id) = parent {
                    if admitted_node_ids.contains(&id) {
                        return false;
                    }
                    parent = hierarchy.node(id).and_then(|ancestor| ancestor.parent());
                }
                true
            })
            .flat_map(|node| node.blocks().iter().copied())
            .collect::<BTreeSet<_>>();
        let target_complete = result.pending_blocks.is_empty()
            && uncovered_nodes.is_empty()
            && budget_omitted_nodes.is_empty();
        let capture_initial_cut = self.retained_nodes.is_empty()
            && uncovered_nodes.is_empty()
            && budget_omitted_nodes.is_empty();
        if target_complete || capture_initial_cut {
            for node_id in &admitted_nodes {
                if let Some(node) = hierarchy.node(*node_id) {
                    protected_block_ids.extend(node.blocks().iter().copied());
                }
            }
        } else {
            for node_id in &self.retained_nodes {
                if let Some(node) = hierarchy.node(*node_id) {
                    protected_block_ids.extend(
                        node.blocks()
                            .iter()
                            .copied()
                            .filter(|block_id| cache.contains(*block_id)),
                    );
                }
            }
        }
        result
            .fallback_nodes
            .retain(|node_id| admitted_node_ids.contains(node_id));
        LodTransitionPlan {
            node_ids: admitted_nodes,
            block_ids: admitted_blocks.into_iter().collect(),
            protected_block_ids: protected_block_ids.into_iter().collect(),
            point_count,
            point_budget: target.point_budget(),
            pending_blocks: result.pending_blocks.into_iter().collect(),
            fallback_nodes: result.fallback_nodes,
            uncovered_nodes: uncovered_nodes.into_iter().collect(),
            budget_omitted_nodes,
            target_complete,
        }
    }

    /// Activates a previously prepared cut and its cache protection.
    pub fn commit(&mut self, plan: &LodTransitionPlan, cache: &mut DecodedBlockCache) {
        if plan.target_complete || (self.retained_nodes.is_empty() && plan.coverage_complete()) {
            self.retained_nodes.clone_from(&plan.node_ids);
        }
        cache.set_protected_blocks(plan.protected_block_ids.iter().copied());
    }

    /// Resolves and immediately activates a cut. Prefer [`Self::prepare`] and
    /// [`Self::commit`] when a renderer must accept the cut before it becomes active.
    pub fn update(
        &mut self,
        hierarchy: &SpatialHierarchy,
        target: &LodSelection,
        cache: &mut DecodedBlockCache,
    ) -> LodTransitionPlan {
        let plan = self.prepare(hierarchy, target, cache);
        self.commit(&plan, cache);
        plan
    }

    /// Clears the retained cut and releases its cache protection for dataset replacement.
    pub fn reset(&mut self, cache: &mut DecodedBlockCache) {
        self.retained_nodes.clear();
        cache.set_protected_blocks([]);
    }
}

#[derive(Default)]
struct ResolvedCut {
    nodes: Vec<NodeId>,
    pending_blocks: BTreeSet<crate::BlockId>,
    fallback_nodes: Vec<NodeId>,
    uncovered_nodes: Vec<NodeId>,
    complete: bool,
}

fn resolve_replacement<S: ResidentBlockSource + ?Sized>(
    hierarchy: &SpatialHierarchy,
    node_id: NodeId,
    target_nodes: &HashSet<NodeId>,
    branch_nodes: &HashSet<NodeId>,
    retained: &RetainedIndex,
    retain_previous: bool,
    cache: &S,
) -> ResolvedCut {
    if !branch_nodes.contains(&node_id) {
        let mut retained = retained
            .nodes
            .get(&node_id)
            .into_iter()
            .chain(retained.below(node_id))
            .copied()
            .filter(|retained_id| {
                hierarchy.node(*retained_id).is_some_and(|retained| {
                    retained
                        .blocks()
                        .iter()
                        .all(|block_id| cache.contains(*block_id))
                })
            })
            .collect::<Vec<_>>();
        retained.sort_unstable();
        if !retain_previous {
            retained.clear();
        }
        return ResolvedCut {
            nodes: retained,
            complete: true,
            ..ResolvedCut::default()
        };
    }

    let node = hierarchy
        .node(node_id)
        .expect("branch index refers to a hierarchy node");
    if target_nodes.contains(&node_id) {
        if node
            .blocks()
            .iter()
            .all(|block_id| cache.contains(*block_id))
        {
            return resolved_node(node_id);
        }

        let mut pending_blocks = node
            .blocks()
            .iter()
            .copied()
            .filter(|block_id| !cache.contains(*block_id))
            .collect::<BTreeSet<_>>();
        let retained_descendants = retained.below(node_id).to_vec();
        let available_descendants = retained_descendants
            .iter()
            .copied()
            .filter_map(|active_id| {
                let active_node = hierarchy.node(active_id)?;
                active_node
                    .blocks()
                    .iter()
                    .all(|block_id| cache.contains(*block_id))
                    .then_some(active_id)
            })
            .collect::<Vec<_>>();
        let descendants_available = !retained_descendants.is_empty()
            && available_descendants.len() == retained_descendants.len();
        let mut result = ResolvedCut {
            nodes: available_descendants,
            pending_blocks: std::mem::take(&mut pending_blocks),
            // A finer cut from the previous frustum may not cover newly visible space.
            complete: false,
            ..ResolvedCut::default()
        };
        if descendants_available {
            result.fallback_nodes.extend(result.nodes.iter().copied());
        }
        result.uncovered_nodes.push(node_id);
        return result;
    }

    let mut children = hierarchy
        .child_ids(node_id)
        .iter()
        .copied()
        .map(|child_id| {
            resolve_replacement(
                hierarchy,
                child_id,
                target_nodes,
                branch_nodes,
                retained,
                retain_previous,
                cache,
            )
        })
        .collect::<Vec<_>>();
    let children_complete = children.iter().all(|child| child.complete);
    let mut result = combine(children.drain(..));
    result.complete = children_complete;
    if !children_complete
        && node.point_count() > 0
        && node
            .blocks()
            .iter()
            .all(|block_id| cache.contains(*block_id))
    {
        let mut pending_blocks = std::mem::take(&mut result.pending_blocks);
        let mut replacement = resolved_node(node_id);
        replacement
            .nodes
            .extend(result.nodes.into_iter().filter(|child_id| {
                hierarchy.node(*child_id).is_some_and(|child| {
                    child.parent() == Some(node_id) && aligned_direct_child(hierarchy, node, child)
                })
            }));
        replacement.fallback_nodes.extend(result.fallback_nodes);
        replacement.pending_blocks.append(&mut pending_blocks);
        replacement.fallback_nodes.push(node_id);
        return replacement;
    }
    result
}

fn aligned_direct_child(
    hierarchy: &SpatialHierarchy,
    parent: &crate::SpatialNode,
    child: &crate::SpatialNode,
) -> bool {
    if child.level() != parent.level().saturating_add(1) || child.level() > 24 {
        return false;
    }
    let root = hierarchy.stats().root_bounds;
    let side = (0..3)
        .map(|axis| root.max()[axis] - root.min()[axis])
        .fold(0.0, f64::max);
    if !side.is_finite() || side <= 0.0 {
        return false;
    }
    // The voxel builder rejects mixed cuts finer than the parent's native representatives
    // (8 per axis, or the stored spacing of `v7.4-pre`).
    let representative_depth = (side / parent.representative_spacing())
        .log2()
        .floor()
        .clamp(0.0, 24.0) as u32;
    if child.level() > representative_depth {
        return false;
    }
    let scale = 2.0_f64.powi(i32::try_from(child.level()).expect("depth is at most 24")) / side;
    (0..3).all(|axis| {
        [child.bounds().min()[axis], child.bounds().max()[axis]]
            .into_iter()
            .filter(|edge| {
                *edge > parent.bounds().min()[axis] && *edge < parent.bounds().max()[axis]
            })
            .all(|edge| {
                let cell = (edge - root.min()[axis]) * scale;
                cell.is_finite() && (cell - cell.round()).abs() <= 1.0e-6
            })
    })
}

fn resolve_additive<S: ResidentBlockSource + ?Sized>(
    hierarchy: &SpatialHierarchy,
    target_nodes: &HashSet<NodeId>,
    retained_nodes: &HashSet<NodeId>,
    retain_previous: bool,
    cache: &S,
) -> ResolvedCut {
    let mut result = ResolvedCut {
        complete: true,
        ..ResolvedCut::default()
    };
    let mut resolved = HashSet::new();
    for node_id in target_nodes {
        let node = hierarchy
            .node(*node_id)
            .expect("LOD selection refers to a hierarchy node");
        let missing = node
            .blocks()
            .iter()
            .copied()
            .filter(|block_id| !cache.contains(*block_id))
            .collect::<Vec<_>>();
        if missing.is_empty() {
            result.nodes.push(*node_id);
            resolved.insert(*node_id);
        } else {
            result.complete = false;
            result.pending_blocks.extend(missing);
            result.uncovered_nodes.push(*node_id);
        }
    }
    for node_id in retained_nodes.iter().filter(|_| retain_previous) {
        if resolved.contains(node_id) {
            continue;
        }
        let Some(node) = hierarchy.node(*node_id) else {
            continue;
        };
        if node
            .blocks()
            .iter()
            .all(|block_id| cache.contains(*block_id))
        {
            result.nodes.push(*node_id);
            resolved.insert(*node_id);
        }
    }
    result
}

fn resolved_node(node_id: NodeId) -> ResolvedCut {
    ResolvedCut {
        nodes: vec![node_id],
        complete: true,
        ..ResolvedCut::default()
    }
}

fn combine(results: impl IntoIterator<Item = ResolvedCut>) -> ResolvedCut {
    let mut combined = ResolvedCut {
        complete: true,
        ..ResolvedCut::default()
    };
    for result in results {
        combined.complete &= result.complete;
        combined.nodes.extend(result.nodes);
        combined.pending_blocks.extend(result.pending_blocks);
        combined.fallback_nodes.extend(result.fallback_nodes);
        combined.uncovered_nodes.extend(result.uncovered_nodes);
    }
    combined
}

/// Strict ancestors of `node_id`, nearest first.
fn ancestors(hierarchy: &SpatialHierarchy, node_id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    let mut current = hierarchy.node(node_id).and_then(|node| node.parent());
    std::iter::from_fn(move || {
        let id = current?;
        current = hierarchy.node(id).and_then(|node| node.parent());
        Some(id)
    })
}

/// The previous cut plus, per ancestor, its retained strict descendants, so region
/// queries do not rescan every retained node and walk its ancestry each time.
/// Each list keeps the iteration order of the retained set.
struct RetainedIndex {
    nodes: HashSet<NodeId>,
    below: HashMap<NodeId, Vec<NodeId>>,
}

impl RetainedIndex {
    fn new(hierarchy: &SpatialHierarchy, retained: &[NodeId]) -> Self {
        let nodes = retained.iter().copied().collect::<HashSet<_>>();
        let mut below = HashMap::<NodeId, Vec<NodeId>>::new();
        for node_id in &nodes {
            for ancestor in ancestors(hierarchy, *node_id) {
                below.entry(ancestor).or_default().push(*node_id);
            }
        }
        Self { nodes, below }
    }

    fn below(&self, ancestor: NodeId) -> &[NodeId] {
        self.below.get(&ancestor).map_or(&[], Vec::as_slice)
    }
}
