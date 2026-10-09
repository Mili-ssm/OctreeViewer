use std::collections::{BTreeSet, HashSet};

use crate::{DecodedBlockCache, HierarchySemantics, LodSelection, NodeId, SpatialHierarchy};

/// Renderable block cut after reconciling an LOD target with decoded cache contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LodTransitionPlan {
    node_ids: Vec<NodeId>,
    block_ids: Vec<crate::BlockId>,
    point_count: u64,
    point_budget: u64,
    pending_blocks: Vec<crate::BlockId>,
    fallback_nodes: Vec<NodeId>,
    uncovered_nodes: Vec<NodeId>,
    budget_omitted_nodes: Vec<NodeId>,
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

/// Keeps the currently displayed cut stable while target blocks are loading.
///
/// Create one planner per dataset and call [`Self::reset`] when replacing it. The
/// planner protects its visible cache blocks from LRU eviction. If the cache cannot
/// hold the target alongside that protected cut, insertion is rejected and the
/// current cut remains visible until memory becomes available.
#[derive(Default)]
pub struct LodTransitionPlanner {
    active_nodes: Vec<NodeId>,
}

impl LodTransitionPlanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolves target nodes against cache residency without changing the active cut
    /// or cache protection. Call [`Self::commit`] after downstream consumers accept it.
    pub fn prepare(
        &self,
        hierarchy: &SpatialHierarchy,
        target: &LodSelection,
        cache: &DecodedBlockCache,
    ) -> LodTransitionPlan {
        let target_nodes = target.node_ids().iter().copied().collect::<HashSet<_>>();
        let active_nodes = self.active_nodes.iter().copied().collect::<HashSet<_>>();
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

        let mut result = if hierarchy.semantics() == HierarchySemantics::Replacement {
            resolve_replacement(
                hierarchy,
                hierarchy.root_id(),
                &target_nodes,
                &branch_nodes,
                &active_nodes,
                cache,
            )
        } else {
            resolve_additive(hierarchy, &target_nodes, cache)
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
        for omitted_id in &budget_omitted_nodes {
            for target_id in &target_nodes {
                if *target_id == *omitted_id
                    || is_descendant_of(hierarchy, *target_id, *omitted_id)
                    || is_descendant_of(hierarchy, *omitted_id, *target_id)
                {
                    uncovered_nodes.insert(*target_id);
                }
            }
        }
        let admitted_node_ids = admitted_nodes.iter().copied().collect::<HashSet<_>>();
        result
            .fallback_nodes
            .retain(|node_id| admitted_node_ids.contains(node_id));
        LodTransitionPlan {
            node_ids: admitted_nodes,
            block_ids: admitted_blocks.into_iter().collect(),
            point_count,
            point_budget: target.point_budget(),
            pending_blocks: result.pending_blocks.into_iter().collect(),
            fallback_nodes: result.fallback_nodes,
            uncovered_nodes: uncovered_nodes.into_iter().collect(),
            budget_omitted_nodes,
        }
    }

    /// Activates a previously prepared cut and protects its resident payloads.
    pub fn commit(&mut self, plan: &LodTransitionPlan, cache: &mut DecodedBlockCache) {
        self.active_nodes.clone_from(&plan.node_ids);
        cache.set_protected_blocks(plan.block_ids.iter().copied());
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

    /// Clears the active cut and releases its cache protection, for dataset replacement.
    pub fn reset(&mut self, cache: &mut DecodedBlockCache) {
        self.active_nodes.clear();
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

fn resolve_replacement(
    hierarchy: &SpatialHierarchy,
    node_id: NodeId,
    target_nodes: &HashSet<NodeId>,
    branch_nodes: &HashSet<NodeId>,
    active_nodes: &HashSet<NodeId>,
    cache: &DecodedBlockCache,
) -> ResolvedCut {
    if !branch_nodes.contains(&node_id) {
        return ResolvedCut {
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
        let active_descendants = active_nodes
            .iter()
            .copied()
            .filter(|active_id| {
                *active_id != node_id && is_descendant_of(hierarchy, *active_id, node_id)
            })
            .collect::<Vec<_>>();
        let available_descendants = active_descendants
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
        let descendants_available = !active_descendants.is_empty()
            && available_descendants.len() == active_descendants.len();
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
                active_nodes,
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
        replacement.pending_blocks.append(&mut pending_blocks);
        replacement.fallback_nodes.push(node_id);
        return replacement;
    }
    result
}

fn resolve_additive(
    hierarchy: &SpatialHierarchy,
    target_nodes: &HashSet<NodeId>,
    cache: &DecodedBlockCache,
) -> ResolvedCut {
    let mut result = ResolvedCut {
        complete: true,
        ..ResolvedCut::default()
    };
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
        } else {
            result.complete = false;
            result.pending_blocks.extend(missing);
            result.uncovered_nodes.push(*node_id);
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

fn is_descendant_of(hierarchy: &SpatialHierarchy, node_id: NodeId, ancestor_id: NodeId) -> bool {
    let mut current = hierarchy.node(node_id).and_then(|node| node.parent());
    while let Some(id) = current {
        if id == ancestor_id {
            return true;
        }
        current = hierarchy.node(id).and_then(|node| node.parent());
    }
    false
}
