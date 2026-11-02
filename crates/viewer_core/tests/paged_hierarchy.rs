//! Growing and shrinking `SpatialHierarchy` with unloaded anchors (R5 P1).

use std::collections::HashSet;

use viewer_core::{
    BlockId, FrustumPlane, LodSelectionConfig, LodTransitionPlanner, NodeId, SpatialBounds,
    SpatialError, SpatialFrustum, SpatialHierarchy, SpatialNode,
};

/// Binary tree with heap numbering (children of `i` are `2i`, `2i + 1`), split along x.
const MAX_LEVEL: u32 = 6;
/// Pages hold two levels: the root page covers levels 0-2, others cover `anchor + 1..=anchor + 2`.
const PAGE_DEPTH: u32 = 2;

fn level(id: u64) -> u32 {
    63 - id.leading_zeros()
}

fn node(id: u64) -> SpatialNode {
    let level = level(id);
    let width = 64.0 / f64::from(1_u32 << level);
    let x = (id - (1 << level)) as f64 * width;
    SpatialNode::new(
        NodeId::new(id),
        (id > 1).then(|| NodeId::new(id / 2)),
        level,
        SpatialBounds::new([x, 0.0, 0.0], [x + width, 1.0, 1.0]).unwrap(),
        1,
        0.0,
        vec![BlockId::new(id)],
    )
    .unwrap()
}

/// Nodes of the page below `anchor` (parents first) and the anchors of its nested pages.
fn page(anchor: u64) -> (Vec<SpatialNode>, Vec<NodeId>) {
    let last = (level(anchor) + PAGE_DEPTH).min(MAX_LEVEL);
    let mut nodes = Vec::new();
    let mut anchors = Vec::new();
    let mut frontier = vec![anchor];
    for current in level(anchor) + 1..=last {
        frontier = frontier
            .iter()
            .flat_map(|id| [id * 2, id * 2 + 1])
            .collect();
        nodes.extend(frontier.iter().map(|id| node(*id)));
        if current == last && last < MAX_LEVEL {
            anchors.extend(frontier.iter().map(|id| NodeId::new(*id)));
        }
    }
    (nodes, anchors)
}

fn full() -> SpatialHierarchy {
    SpatialHierarchy::new(
        (1..1 << (MAX_LEVEL + 1)).map(node).collect(),
        Vec::new(),
        None,
    )
    .unwrap()
}

/// Root page resident; everything below it unloaded.
fn root_only() -> SpatialHierarchy {
    let (below, anchors) = page(1);
    let mut nodes = vec![node(1)];
    nodes.extend(below);
    let mut hierarchy = SpatialHierarchy::new(nodes, Vec::new(), None).unwrap();
    for anchor in anchors {
        hierarchy.mark_unloaded(anchor).unwrap();
    }
    hierarchy
}

fn frustum() -> SpatialFrustum {
    SpatialFrustum::new(vec![FrustumPlane::new([1.0, 0.0, 0.0], 1000.0).unwrap()]).unwrap()
}

fn config() -> LodSelectionConfig {
    LodSelectionConfig::new(1.0, u64::MAX).unwrap()
}

fn refine_everything(_: &SpatialNode) -> f64 {
    100.0
}

#[test]
fn unloaded_anchor_is_reported_and_not_resolution_limited() {
    let hierarchy = root_only();
    for selection in [
        hierarchy
            .select_lod(&frustum(), config(), refine_everything)
            .unwrap(),
        hierarchy
            .select_lod_stable_projector(&frustum(), config(), 1 << 20, refine_everything)
            .unwrap()
            .0,
    ] {
        assert!(selection.resolution_limited().is_empty());
        let mut anchors = selection.unloaded_anchors().to_vec();
        anchors.sort_unstable();
        assert_eq!(anchors, (4..8).map(NodeId::new).collect::<Vec<_>>());
    }
}

#[test]
fn true_leaves_stay_resolution_limited() {
    let selection = full()
        .select_lod(&frustum(), config(), refine_everything)
        .unwrap();
    assert!(selection.unloaded_anchors().is_empty());
    assert_eq!(selection.resolution_limited().len(), 64);
}

/// Loads every page breadth-first; the result must equal the hierarchy built whole.
fn load_all(hierarchy: &mut SpatialHierarchy) {
    let mut pending: Vec<NodeId> = (4..8).map(NodeId::new).collect();
    while let Some(anchor) = pending.pop() {
        let (nodes, anchors) = page(anchor.get());
        hierarchy.graft_page(anchor, nodes, &anchors).unwrap();
        pending.extend(anchors);
    }
}

#[test]
fn fully_grown_hierarchy_selects_like_the_whole_one() {
    let whole = full();
    let mut grown = root_only();
    load_all(&mut grown);
    assert_eq!(grown.stats(), whole.stats());
    assert_eq!(grown.block_ids(), whole.block_ids());
    for threshold in [0.5, 1.5, 3.0] {
        let project = |node: &SpatialNode| f64::from(MAX_LEVEL - node.level()) * threshold;
        let expected = whole.select_lod(&frustum(), config(), project).unwrap();
        let actual = grown.select_lod(&frustum(), config(), project).unwrap();
        assert_eq!(actual, expected);
        assert!(actual.unloaded_anchors().is_empty());
    }
}

#[test]
fn graft_rejects_invalid_pages_atomically() {
    let mut hierarchy = root_only();
    let before = hierarchy.clone();
    let anchor = NodeId::new(4);
    let (mut nodes, anchors) = page(4);
    assert_eq!(
        hierarchy.graft_page(NodeId::new(2), nodes.clone(), &[]),
        Err(SpatialError::NotUnloadedAnchor(NodeId::new(2)))
    );
    nodes.push(node(2)); // Already resident.
    assert_eq!(
        hierarchy.graft_page(anchor, nodes, &anchors),
        Err(SpatialError::DuplicateNodeId(NodeId::new(2)))
    );
    let (mut nodes, _) = page(4);
    nodes.push(node(20)); // Descends from a different anchor.
    assert!(hierarchy.graft_page(anchor, nodes, &[]).is_err());
    assert_eq!(hierarchy, before);
}

#[test]
fn eviction_is_refused_for_retained_nodes_and_resident_descendant_pages() {
    let mut hierarchy = root_only();
    let (nodes, anchors) = page(4);
    hierarchy
        .graft_page(NodeId::new(4), nodes, &anchors)
        .unwrap();
    let (nodes, anchors) = page(16);
    hierarchy
        .graft_page(NodeId::new(16), nodes, &anchors)
        .unwrap();

    assert_eq!(
        hierarchy.evict_page(NodeId::new(4), &[]),
        Err(SpatialError::PageHasResidentDescendant {
            anchor: NodeId::new(4),
            descendant: NodeId::new(16)
        })
    );
    assert_eq!(
        hierarchy.evict_page(NodeId::new(16), &[NodeId::new(64)]),
        Err(SpatialError::PageNodeRetained {
            anchor: NodeId::new(16),
            node: NodeId::new(64)
        })
    );
    // A retained anchor itself is fine: it stays resident, only its children go.
    let removed = hierarchy
        .evict_page(NodeId::new(16), &[NodeId::new(16)])
        .unwrap();
    assert_eq!(removed.len(), 6);
    assert!(hierarchy.is_unloaded(NodeId::new(16)));
    assert!(hierarchy.node(NodeId::new(32)).is_none());
    hierarchy.evict_page(NodeId::new(4), &[]).unwrap();
    assert_eq!(hierarchy, root_only());
    assert_eq!(
        hierarchy.evict_page(NodeId::new(4), &[]),
        Err(SpatialError::NotGraftedPage(NodeId::new(4)))
    );
}

/// Deterministic xorshift so failures reproduce without a dev-dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn random_load_and_evict_sequences_leave_no_orphans() {
    for seed in 1..=20_u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut hierarchy = root_only();
        let mut planner = LodTransitionPlanner::new();
        let mut cache = viewer_core::DecodedBlockCache::new(usize::MAX);
        let mut evictions = 0;
        for _ in 0..300 {
            if rng.next() % 3 == 0 {
                // Plan against the current target so the planner retains a real cut.
                let depth = (rng.next() % 5) as u32;
                let target = hierarchy
                    .select_lod(&frustum(), config(), |node| {
                        if node.level() < depth { 100.0 } else { 0.0 }
                    })
                    .unwrap();
                let plan = planner.prepare(&hierarchy, &target, &cache);
                planner.commit(&plan, &mut cache);
            }
            let anchors: Vec<u64> = (2..1 << (MAX_LEVEL + 1))
                .filter(|id| level(*id).is_multiple_of(PAGE_DEPTH) && level(*id) < MAX_LEVEL)
                .collect();
            let anchor = NodeId::new(anchors[(rng.next() as usize) % anchors.len()]);
            if hierarchy.is_unloaded(anchor) {
                let (nodes, nested) = page(anchor.get());
                hierarchy.graft_page(anchor, nodes, &nested).unwrap();
            } else if hierarchy.node(anchor).is_some()
                && hierarchy
                    .evict_page(anchor, planner.retained_nodes())
                    .is_ok()
            {
                evictions += 1;
            }

            let ids: HashSet<NodeId> = (1..1 << (MAX_LEVEL + 1))
                .map(NodeId::new)
                .filter(|id| hierarchy.node(*id).is_some())
                .collect();
            assert_eq!(hierarchy.stats().node_count, ids.len());
            for id in &ids {
                let parent = hierarchy.node(*id).unwrap().parent();
                assert!(
                    parent.is_none_or(|parent| ids.contains(&parent)),
                    "orphan {id:?}"
                );
                if hierarchy.is_unloaded(*id) {
                    assert!(hierarchy.child_ids(*id).is_empty());
                }
            }
            for retained in planner.retained_nodes() {
                assert!(
                    hierarchy.node(*retained).is_some(),
                    "retained cut lost {retained:?}"
                );
            }
        }
        assert!(evictions > 0, "seed {seed} never evicted");
    }
}

/// An empty node whose page is missing may hide points below it, so it is reported rather than
/// skipped as an empty region.
#[test]
fn unloaded_anchor_without_points_is_not_dropped() {
    let (below, anchors) = page(1);
    let mut nodes = vec![node(1)];
    nodes.extend(below.into_iter().map(|n| {
        if n.id() == NodeId::new(4) {
            SpatialNode::new(n.id(), n.parent(), n.level(), n.bounds(), 0, 0.0, vec![]).unwrap()
        } else {
            n
        }
    }));
    let mut hierarchy = SpatialHierarchy::new(nodes, Vec::new(), None).unwrap();
    for anchor in anchors {
        hierarchy.mark_unloaded(anchor).unwrap();
    }
    for selection in [
        hierarchy
            .select_lod(&frustum(), config(), refine_everything)
            .unwrap(),
        hierarchy
            .select_lod_stable_projector(&frustum(), config(), 1 << 20, refine_everything)
            .unwrap()
            .0,
    ] {
        let mut anchors = selection.unloaded_anchors().to_vec();
        anchors.sort_unstable();
        assert_eq!(anchors, (4..8).map(NodeId::new).collect::<Vec<_>>());
    }
}
