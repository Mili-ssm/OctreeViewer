use std::collections::{BTreeSet, HashSet};

use crate::{DecodedBlockCache, HierarchySemantics, LodSelection, NodeId, SpatialHierarchy};

/// Renderable block cut after reconciling an LOD target with decoded cache contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LodTransitionPlan {
    node_ids: Vec<NodeId>,
    block_ids: Vec<crate::BlockId>,
    pending_blocks: Vec<crate::BlockId>,
    fallback_nodes: Vec<NodeId>,
    uncovered_nodes: Vec<NodeId>,
}

impl LodTransitionPlan {
    pub fn node_ids(&self) -> &[NodeId] {
        &self.node_ids
    }

    pub fn block_ids(&self) -> &[crate::BlockId] {
        &self.block_ids
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

    /// Resolves target nodes against cache residency and protects the returned cut.
    pub fn update(
        &mut self,
        hierarchy: &SpatialHierarchy,
        target: &LodSelection,
        cache: &mut DecodedBlockCache,
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
        let plan = LodTransitionPlan {
            node_ids: result.nodes,
            block_ids: result.block_ids.into_iter().collect(),
            pending_blocks: result.pending_blocks.into_iter().collect(),
            fallback_nodes: result.fallback_nodes,
            uncovered_nodes: result.uncovered_nodes,
        };
        self.active_nodes.clone_from(&plan.node_ids);
        cache.set_protected_blocks(plan.block_ids.iter().copied());
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
    block_ids: BTreeSet<crate::BlockId>,
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
            return resolved_node(node_id, node.blocks().iter().copied());
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
                    .then_some((active_id, active_node.blocks()))
            })
            .collect::<Vec<_>>();
        let descendants_available = !active_descendants.is_empty()
            && available_descendants.len() == active_descendants.len();
        let mut result = ResolvedCut {
            nodes: available_descendants
                .iter()
                .map(|(active_id, _)| *active_id)
                .collect(),
            pending_blocks: std::mem::take(&mut pending_blocks),
            // A finer cut from the previous frustum may not cover newly visible space.
            complete: false,
            ..ResolvedCut::default()
        };
        for (_, blocks) in available_descendants {
            result.block_ids.extend(blocks.iter().copied());
        }
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
        let mut replacement = resolved_node(node_id, node.blocks().iter().copied());
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
            result.block_ids.extend(node.blocks().iter().copied());
        } else {
            result.complete = false;
            result.pending_blocks.extend(missing);
            result.uncovered_nodes.push(*node_id);
        }
    }
    result
}

fn resolved_node(node_id: NodeId, blocks: impl IntoIterator<Item = crate::BlockId>) -> ResolvedCut {
    ResolvedCut {
        nodes: vec![node_id],
        block_ids: blocks.into_iter().collect(),
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
        combined.block_ids.extend(result.block_ids);
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
