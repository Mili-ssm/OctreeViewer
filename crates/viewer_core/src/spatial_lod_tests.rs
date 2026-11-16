use std::cell::{Cell, RefCell};

use super::*;

#[derive(Clone, Copy)]
enum CallbackMode {
    ByNodeId,
    Stateful,
}

fn node(
    id: u64,
    parent: Option<u64>,
    level: u32,
    bounds: SpatialBounds,
    points: u64,
) -> SpatialNode {
    SpatialNode::new(
        NodeId::new(id),
        parent.map(NodeId::new),
        level,
        bounds,
        points,
        0.0,
        (points > 0)
            .then_some(BlockId::new(id))
            .into_iter()
            .collect(),
    )
    .unwrap()
}

fn generated_hierarchy(semantics: HierarchySemantics, reverse_input: bool) -> SpatialHierarchy {
    const NODE_COUNT: usize = 31;
    let mut nodes = (0..NODE_COUNT)
        .map(|index| {
            let level = (index + 1).ilog2();
            let first_at_level = (1_usize << level) - 1;
            let slot = index - first_at_level;
            let divisions = 1_usize << level;
            let min_x = slot as f64 / divisions as f64;
            let max_x = (slot + 1) as f64 / divisions as f64;
            let point_count = if level == 0 || (index * 7 + 3) % 6 == 0 {
                0
            } else {
                (index % 4 + 1) as u64
            };
            let parent = if index == 0 {
                None
            } else {
                Some(NodeId::new(((index - 1) / 2) as u64 + 100))
            };
            SpatialNode::new(
                NodeId::new(index as u64 + 100),
                parent,
                level,
                SpatialBounds::new([min_x, 0.0, 0.0], [max_x, 1.0, 1.0]).unwrap(),
                point_count,
                0.0,
                (point_count > 0)
                    .then_some(BlockId::new(index as u64 + 1))
                    .into_iter()
                    .collect(),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let source_point_count = nodes.iter().map(SpatialNode::point_count).sum::<u64>();
    if reverse_input {
        nodes.reverse();
    }
    SpatialHierarchy::new_with_semantics(
        nodes,
        Vec::new(),
        (semantics == HierarchySemantics::Additive).then_some(source_point_count),
        semantics,
    )
    .unwrap()
}

fn test_frusta() -> Vec<SpatialFrustum> {
    vec![
        SpatialFrustum::new(vec![
            FrustumPlane::new([1.0, 0.0, 0.0], 0.0).unwrap(),
            FrustumPlane::new([-1.0, 0.0, 0.0], 1.0).unwrap(),
            FrustumPlane::new([0.0, 1.0, 0.0], 0.0).unwrap(),
            FrustumPlane::new([0.0, -1.0, 0.0], 1.0).unwrap(),
            FrustumPlane::new([0.0, 0.0, 1.0], 0.0).unwrap(),
            FrustumPlane::new([0.0, 0.0, -1.0], 1.0).unwrap(),
        ])
        .unwrap(),
        SpatialFrustum::new(vec![FrustumPlane::new([1.0, 0.0, 0.0], -2.0).unwrap()]).unwrap(),
        SpatialFrustum::new(vec![FrustumPlane::new([1.0, 0.0, 0.0], -0.5).unwrap()]).unwrap(),
    ]
}

fn callback_value(mode: CallbackMode, node: &SpatialNode, call_index: usize) -> f64 {
    let id = node.id().get();
    match mode {
        CallbackMode::ByNodeId => ((id.wrapping_mul(37) % 101) as f64) / 7.0,
        CallbackMode::Stateful => {
            (((id.wrapping_mul(11) + call_index as u64 * 17) % 47) as f64) / 5.0
        }
    }
}

fn compare_results(
    actual: Result<LodSelection, LodSelectionError>,
    reference: Result<LodSelection, LodSelectionError>,
) {
    match (actual, reference) {
        (Ok(actual), Ok(reference)) => assert_eq!(actual, reference),
        (
            Err(LodSelectionError::InvalidProjectedError {
                node: actual_node,
                error: actual_error,
            }),
            Err(LodSelectionError::InvalidProjectedError {
                node: reference_node,
                error: reference_error,
            }),
        ) => {
            assert_eq!(actual_node, reference_node);
            if actual_error.is_nan() {
                assert!(reference_error.is_nan());
            } else {
                assert_eq!(actual_error, reference_error);
            }
        }
        (Err(actual), Err(reference)) => assert_eq!(actual, reference),
        (actual, reference) => panic!("result mismatch: {actual:?} vs {reference:?}"),
    }
}

fn compare_selection(
    hierarchy: &SpatialHierarchy,
    frustum: &SpatialFrustum,
    config: LodSelectionConfig,
    mode: CallbackMode,
) {
    let actual_calls = Cell::new(0);
    let actual_trace = RefCell::new(Vec::new());
    let actual = hierarchy.select_lod(frustum, config, |node| {
        let call_index = actual_calls.get();
        actual_calls.set(call_index + 1);
        actual_trace.borrow_mut().push(node.id());
        callback_value(mode, node, call_index)
    });

    let reference_calls = Cell::new(0);
    let reference_trace = RefCell::new(Vec::new());
    let reference = reference_select_lod(hierarchy, frustum, config, |node| {
        let call_index = reference_calls.get();
        reference_calls.set(call_index + 1);
        reference_trace.borrow_mut().push(node.id());
        callback_value(mode, node, call_index)
    });

    assert_eq!(actual_calls.get(), reference_calls.get());
    assert_eq!(*actual_trace.borrow(), *reference_trace.borrow());
    compare_results(actual, reference);
}

#[test]
fn scratch_lod_matches_reference_for_generated_hierarchies_and_queries() {
    let frusta = test_frusta();
    for semantics in [
        HierarchySemantics::Replacement,
        HierarchySemantics::Additive,
    ] {
        for reverse_input in [false, true] {
            let hierarchy = generated_hierarchy(semantics, reverse_input);
            for frustum in &frusta {
                for budget in [0, 7, 31, u64::MAX] {
                    for tolerance in [0.0, 1.0] {
                        let config = LodSelectionConfig::new(tolerance, budget).unwrap();
                        compare_selection(&hierarchy, frustum, config, CallbackMode::ByNodeId);
                        compare_selection(&hierarchy, frustum, config, CallbackMode::Stateful);
                    }
                }
            }
        }
    }
}

#[test]
fn stable_projector_lod_matches_legacy_with_bounded_priority_memory() {
    let frusta = test_frusta();
    let budgets = [0, 24, 96, 4096, 4 * 1024 * 1024];
    for semantics in [
        HierarchySemantics::Replacement,
        HierarchySemantics::Additive,
    ] {
        for reverse_input in [false, true] {
            let hierarchy = generated_hierarchy(semantics, reverse_input);
            for frustum in &frusta {
                for point_budget in [0, 7, 31, u64::MAX] {
                    for tolerance in [0.0, 1.0] {
                        let config = LodSelectionConfig::new(tolerance, point_budget).unwrap();
                        for priority_budget in budgets {
                            let actual = hierarchy.select_lod_stable_projector(
                                frustum,
                                config,
                                priority_budget,
                                |node| callback_value(CallbackMode::ByNodeId, node, 0),
                            );
                            let legacy = hierarchy.select_lod(frustum, config, |node| {
                                callback_value(CallbackMode::ByNodeId, node, 0)
                            });
                            match actual {
                                Ok((selection, stats)) => {
                                    compare_results(Ok(selection), legacy);
                                    if stats.fallback.is_none() {
                                        assert!(stats.peak_priority_bytes <= priority_budget);
                                    }
                                    if priority_budget == 0 {
                                        assert!(stats.fallback.is_some());
                                    }
                                }
                                Err(error) => compare_results(Err(error), legacy),
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn stable_projector_lod_matches_legacy_for_ties_and_infinite_priorities() {
    let frustum = test_frusta().remove(0);
    let hierarchy = generated_hierarchy(HierarchySemantics::Replacement, true);
    let config = LodSelectionConfig::new(0.0, u64::MAX).unwrap();
    for (priority_budget, projector) in [(4096, 0), (4 * 1024 * 1024, 1)] {
        let project = |_node: &SpatialNode| match projector {
            0 => 1.0,
            _ => f64::INFINITY,
        };
        let actual =
            hierarchy.select_lod_stable_projector(&frustum, config, priority_budget, project);
        let legacy = hierarchy.select_lod(&frustum, config, project);
        match actual {
            Ok((selection, stats)) => {
                compare_results(Ok(selection), legacy);
                if stats.fallback.is_none() {
                    assert!(stats.peak_priority_bytes <= priority_budget);
                }
            }
            Err(error) => compare_results(Err(error), legacy),
        }
    }
}

#[test]
fn scratch_lod_preserves_invalid_error_and_overflow_results() {
    let leaf_bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let leaf =
        SpatialHierarchy::new(vec![node(9, None, 0, leaf_bounds, 1)], Vec::new(), None).unwrap();
    let frustum = test_frusta().remove(0);
    let config = LodSelectionConfig::new(0.0, 1).unwrap();
    for invalid_error in [f64::NAN, -1.0] {
        let actual_trace = RefCell::new(Vec::new());
        let actual = leaf.select_lod(&frustum, config, |node| {
            actual_trace.borrow_mut().push(node.id());
            invalid_error
        });
        let reference_trace = RefCell::new(Vec::new());
        let reference = reference_select_lod(&leaf, &frustum, config, |node| {
            reference_trace.borrow_mut().push(node.id());
            invalid_error
        });
        assert_eq!(*actual_trace.borrow(), *reference_trace.borrow());
        compare_results(actual, reference);
    }

    let overflow = SpatialHierarchy::new(
        vec![
            node(
                1,
                None,
                0,
                SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                0,
            ),
            node(
                2,
                Some(1),
                1,
                SpatialBounds::new([0.0; 3], [0.5, 1.0, 1.0]).unwrap(),
                u64::MAX,
            ),
            node(
                3,
                Some(1),
                1,
                SpatialBounds::new([0.5, 0.0, 0.0], [1.0; 3]).unwrap(),
                1,
            ),
        ],
        Vec::new(),
        None,
    )
    .unwrap();
    let trace = RefCell::new(Vec::new());
    let actual = overflow.select_lod(
        &frustum,
        LodSelectionConfig::new(1.0, u64::MAX).unwrap(),
        |node| {
            trace.borrow_mut().push(node.id());
            0.0
        },
    );
    let reference_trace = RefCell::new(Vec::new());
    let reference = reference_select_lod(
        &overflow,
        &frustum,
        LodSelectionConfig::new(1.0, u64::MAX).unwrap(),
        |node| {
            reference_trace.borrow_mut().push(node.id());
            0.0
        },
    );
    assert_eq!(*trace.borrow(), *reference_trace.borrow());
    compare_results(actual, reference);
}

#[test]
fn scratch_lod_preserves_stateful_final_validation_and_first_invalid_error() {
    let leaf = SpatialHierarchy::new(
        vec![node(
            9,
            None,
            0,
            SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
            1,
        )],
        Vec::new(),
        None,
    )
    .unwrap();
    let frustum = test_frusta().remove(0);
    let config = LodSelectionConfig::new(0.0, 1).unwrap();
    let actual_trace = RefCell::new(Vec::new());
    let actual_calls = Cell::new(0);
    let actual = leaf.select_lod(&frustum, config, |node| {
        actual_trace.borrow_mut().push(node.id());
        let call = actual_calls.get();
        actual_calls.set(call + 1);
        if call == 0 { 0.0 } else { f64::NAN }
    });
    let reference_trace = RefCell::new(Vec::new());
    let reference_calls = Cell::new(0);
    let reference = reference_select_lod(&leaf, &frustum, config, |node| {
        reference_trace.borrow_mut().push(node.id());
        let call = reference_calls.get();
        reference_calls.set(call + 1);
        if call == 0 { 0.0 } else { f64::NAN }
    });
    assert_eq!(actual_calls.get(), 2);
    assert_eq!(actual_calls.get(), reference_calls.get());
    assert_eq!(*actual_trace.borrow(), *reference_trace.borrow());
    compare_results(actual, reference);

    let two_leaves = SpatialHierarchy::new(
        vec![
            node(
                1,
                None,
                0,
                SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                0,
            ),
            node(
                2,
                Some(1),
                1,
                SpatialBounds::new([0.0; 3], [0.5, 1.0, 1.0]).unwrap(),
                1,
            ),
            node(
                3,
                Some(1),
                1,
                SpatialBounds::new([0.5, 0.0, 0.0], [1.0; 3]).unwrap(),
                1,
            ),
        ],
        Vec::new(),
        None,
    )
    .unwrap();
    let actual_trace = RefCell::new(Vec::new());
    let two_leaf_config = LodSelectionConfig::new(0.0, 2).unwrap();
    let actual = two_leaves.select_lod(&frustum, two_leaf_config, |node| {
        actual_trace.borrow_mut().push(node.id());
        if node.id() == NodeId::new(2) {
            -1.0
        } else {
            f64::NAN
        }
    });
    let reference_trace = RefCell::new(Vec::new());
    let reference = reference_select_lod(&two_leaves, &frustum, two_leaf_config, |node| {
        reference_trace.borrow_mut().push(node.id());
        if node.id() == NodeId::new(2) {
            -1.0
        } else {
            f64::NAN
        }
    });
    assert_eq!(*actual_trace.borrow(), vec![NodeId::new(2)]);
    assert_eq!(*actual_trace.borrow(), *reference_trace.borrow());
    compare_results(actual, reference);
}

#[test]
fn scratch_lod_preserves_candidate_overflow_even_when_candidate_is_not_best() {
    let hierarchy = SpatialHierarchy::new(
        vec![
            node(
                1,
                None,
                0,
                SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                0,
            ),
            node(
                2,
                Some(1),
                1,
                SpatialBounds::new([0.0; 3], [0.5, 1.0, 1.0]).unwrap(),
                u64::MAX - 1,
            ),
            node(
                3,
                Some(1),
                1,
                SpatialBounds::new([0.5, 0.0, 0.0], [1.0; 3]).unwrap(),
                1,
            ),
            node(
                4,
                Some(2),
                2,
                SpatialBounds::new([0.0; 3], [0.5, 1.0, 1.0]).unwrap(),
                u64::MAX - 1,
            ),
            node(
                5,
                Some(3),
                2,
                SpatialBounds::new([0.5, 0.0, 0.0], [1.0; 3]).unwrap(),
                2,
            ),
        ],
        Vec::new(),
        None,
    )
    .unwrap();
    let frustum = test_frusta().remove(0);
    let config = LodSelectionConfig::new(0.0, u64::MAX).unwrap();
    let actual_trace = RefCell::new(Vec::new());
    let actual = hierarchy.select_lod(&frustum, config, |node| {
        actual_trace.borrow_mut().push(node.id());
        if node.id() == NodeId::new(2) {
            100.0
        } else {
            10.0
        }
    });
    let reference_trace = RefCell::new(Vec::new());
    let reference = reference_select_lod(&hierarchy, &frustum, config, |node| {
        reference_trace.borrow_mut().push(node.id());
        if node.id() == NodeId::new(2) {
            100.0
        } else {
            10.0
        }
    });
    assert_eq!(
        actual_trace.borrow().as_slice(),
        [NodeId::new(2), NodeId::new(3)]
    );
    assert_eq!(*actual_trace.borrow(), *reference_trace.borrow());
    compare_results(actual, reference);
}

#[test]
fn visible_frontier_scratch_clears_residue_and_reuses_capacity_in_order() {
    let hierarchy = generated_hierarchy(HierarchySemantics::Replacement, true);
    let frustum = &test_frusta()[2];
    let expected = reference_visible_frontier(&hierarchy, [hierarchy.root], frustum);
    let mut pending = vec![NodeId::new(u64::MAX); 8];
    let mut frontier = vec![NodeId::new(u64::MAX); 8];
    pending.reserve(64);
    frontier.reserve(64);
    let pending_capacity = pending.capacity();
    let frontier_capacity = frontier.capacity();

    hierarchy.visible_frontier_into([hierarchy.root], frustum, &mut pending, &mut frontier);
    assert_eq!(pending, [] as [NodeId; 0]);
    assert_eq!(frontier, expected);
    assert!(
        hierarchy
            .nodes
            .values()
            .any(|node| node.metadata.point_count == 0)
    );
    assert_eq!(frontier.capacity(), frontier_capacity);
    assert_eq!(pending.capacity(), pending_capacity);
    let frontier_ptr = frontier.as_ptr();
    let pending_ptr = pending.as_ptr();

    pending.extend([NodeId::new(50), NodeId::new(51)]);
    frontier.extend([NodeId::new(52), NodeId::new(53)]);
    hierarchy.visible_frontier_into([hierarchy.root], frustum, &mut pending, &mut frontier);
    assert_eq!(pending, [] as [NodeId; 0]);
    assert_eq!(frontier, expected);
    assert_eq!(frontier.capacity(), frontier_capacity);
    assert_eq!(pending.capacity(), pending_capacity);
    assert_eq!(frontier.as_ptr(), frontier_ptr);
    assert_eq!(pending.as_ptr(), pending_ptr);
}

// Kept as an independent copy of select_lod at ffd4af9 for differential coverage.
fn reference_select_lod(
    hierarchy: &SpatialHierarchy,
    frustum: &SpatialFrustum,
    config: LodSelectionConfig,
    projected_error_pixels: impl Fn(&SpatialNode) -> f64,
) -> Result<LodSelection, LodSelectionError> {
    let mut selected = reference_visible_frontier(hierarchy, [hierarchy.root], frustum);
    let mut frontier = selected.clone();
    let mut point_count = reference_count_points(hierarchy, &selected)?;
    if point_count > config.point_budget {
        return Err(LodSelectionError::BaseCutExceedsBudget {
            required: point_count,
            budget: config.point_budget,
        });
    }

    loop {
        let mut best: Option<(usize, NodeId, f64, Vec<NodeId>, u64)> = None;
        let mut budget_limited = Vec::new();
        let mut resolution_limited = Vec::new();
        let mut unloaded_anchors = Vec::new();
        for (index, id) in frontier.iter().copied().enumerate() {
            let node = &hierarchy.nodes[&id].metadata;
            let error = projected_error_pixels(node);
            if error.is_nan() || error < 0.0 {
                return Err(LodSelectionError::InvalidProjectedError { node: id, error });
            }
            if error <= config.max_error_pixels {
                continue;
            }
            let children = reference_visible_frontier(
                hierarchy,
                hierarchy.nodes[&id].children.iter().copied(),
                frustum,
            );
            if children.is_empty() {
                if hierarchy.unloaded.contains(&id) {
                    unloaded_anchors.push(id);
                } else {
                    resolution_limited.push(id);
                }
                continue;
            }
            let children_count = reference_count_points(hierarchy, &children)?;
            let candidate_count = match hierarchy.semantics {
                HierarchySemantics::Replacement => point_count
                    .checked_sub(node.point_count)
                    .and_then(|count| count.checked_add(children_count))
                    .ok_or(LodSelectionError::PointCountOverflow(id))?,
                HierarchySemantics::Additive => point_count
                    .checked_add(children_count)
                    .ok_or(LodSelectionError::PointCountOverflow(id))?,
            };
            if candidate_count > config.point_budget {
                budget_limited.push(id);
                continue;
            }
            let should_replace = best.as_ref().is_none_or(|(_, best_id, best_error, _, _)| {
                error > *best_error || (error == *best_error && id < *best_id)
            });
            if should_replace {
                best = Some((index, id, error, children, candidate_count));
            }
        }

        let Some((index, _, _, replacement, new_point_count)) = best else {
            let mut max_error_pixels: f64 = 0.0;
            for id in &frontier {
                let error = projected_error_pixels(&hierarchy.nodes[id].metadata);
                if error.is_nan() || error < 0.0 {
                    return Err(LodSelectionError::InvalidProjectedError { node: *id, error });
                }
                max_error_pixels = max_error_pixels.max(error);
            }
            return Ok(LodSelection {
                node_ids: selected,
                point_count,
                point_budget: config.point_budget,
                max_error_pixels,
                budget_limited,
                resolution_limited,
                unloaded_anchors,
            });
        };

        match hierarchy.semantics {
            HierarchySemantics::Replacement => {
                selected.splice(index..=index, replacement.clone());
            }
            HierarchySemantics::Additive => {
                let selected_index = selected
                    .iter()
                    .position(|selected_id| *selected_id == frontier[index])
                    .expect("frontier payload is selected");
                selected.splice((selected_index + 1)..=selected_index, replacement.clone());
            }
        }
        frontier.splice(index..=index, replacement);
        point_count = new_point_count;
    }
}

fn reference_visible_frontier(
    hierarchy: &SpatialHierarchy,
    starts: impl IntoIterator<Item = NodeId>,
    frustum: &SpatialFrustum,
) -> Vec<NodeId> {
    let mut pending = starts.into_iter().collect::<Vec<_>>();
    pending.reverse();
    let mut frontier = Vec::new();
    while let Some(id) = pending.pop() {
        let indexed = &hierarchy.nodes[&id];
        if !frustum.intersects(&indexed.metadata.bounds) {
            continue;
        }
        if indexed.metadata.point_count > 0 {
            frontier.push(id);
        } else {
            pending.extend(indexed.children.iter().rev().copied());
        }
    }
    frontier
}

fn reference_count_points(
    hierarchy: &SpatialHierarchy,
    ids: &[NodeId],
) -> Result<u64, LodSelectionError> {
    ids.iter().try_fold(0_u64, |count, id| {
        count
            .checked_add(hierarchy.nodes[id].metadata.point_count)
            .ok_or(LodSelectionError::PointCountOverflow(*id))
    })
}
