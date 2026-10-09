use viewer_core::{
    BlockId, BlockRequestScheduler, FrustumPlane, HierarchySemantics, LodSelectionConfig, NodeId,
    RequestCompletion, SchedulerError, SpatialBounds, SpatialFrustum, SpatialHierarchy,
    SpatialNode,
};

fn box_frustum(min: f64, max: f64) -> SpatialFrustum {
    SpatialFrustum::new(vec![
        FrustumPlane::new([1.0, 0.0, 0.0], -min).unwrap(),
        FrustumPlane::new([-1.0, 0.0, 0.0], max).unwrap(),
        FrustumPlane::new([0.0, 1.0, 0.0], -min).unwrap(),
        FrustumPlane::new([0.0, -1.0, 0.0], max).unwrap(),
        FrustumPlane::new([0.0, 0.0, 1.0], -min).unwrap(),
        FrustumPlane::new([0.0, 0.0, -1.0], max).unwrap(),
    ])
    .unwrap()
}

fn node(id: u64, parent: Option<u64>, min: [f64; 3], max: [f64; 3], blocks: &[u64]) -> SpatialNode {
    SpatialNode::new(
        NodeId::new(id),
        parent.map(NodeId::new),
        u32::from(parent.is_some()),
        SpatialBounds::new(min, max).unwrap(),
        blocks.len() as u64,
        0.0,
        blocks.iter().copied().map(BlockId::new).collect(),
    )
    .unwrap()
}

fn two_leaf_hierarchy() -> SpatialHierarchy {
    SpatialHierarchy::new(
        vec![
            node(1, None, [0.0; 3], [10.0; 3], &[]),
            node(2, Some(1), [0.0; 3], [1.0; 3], &[11]),
            node(3, Some(1), [8.0; 3], [9.0; 3], &[22]),
        ],
        Vec::new(),
        None,
    )
    .unwrap()
}

fn update(
    scheduler: &mut BlockRequestScheduler,
    hierarchy: &SpatialHierarchy,
    min: f64,
    max: f64,
    camera: [f64; 3],
) -> Vec<u64> {
    scheduler
        .update_view(
            hierarchy,
            &box_frustum(min, max),
            LodSelectionConfig::new(1.0, 10).unwrap(),
            camera,
            |_| 0.0,
        )
        .unwrap()
        .selection()
        .node_ids()
        .iter()
        .map(|id| id.get())
        .collect()
}

#[test]
fn prioritizes_nearest_visible_blocks_and_enforces_in_flight_limit() {
    let hierarchy = two_leaf_hierarchy();
    let mut scheduler = BlockRequestScheduler::new(1).unwrap();
    assert_eq!(
        update(&mut scheduler, &hierarchy, 0.0, 10.0, [8.5; 3]),
        [2, 3]
    );
    assert_eq!(scheduler.pending_count(), 2);

    let nearest = scheduler.take_next().unwrap().unwrap();
    assert_eq!(nearest.ticket().block_id(), BlockId::new(22));
    assert_eq!(nearest.priority_distance(), 0.0);
    assert!(scheduler.take_next().unwrap().is_none());
    assert_eq!(scheduler.in_flight_count(), 1);

    assert_eq!(
        scheduler
            .complete(nearest.ticket(), Ok(BlockId::new(22)))
            .unwrap(),
        RequestCompletion::Delivered
    );
    assert_eq!(
        scheduler.take_next().unwrap().unwrap().ticket().block_id(),
        BlockId::new(11)
    );
}

#[test]
fn invalidating_a_delivered_block_requeues_it_when_the_view_still_needs_it() {
    let hierarchy = two_leaf_hierarchy();
    let mut scheduler = BlockRequestScheduler::new(1).unwrap();
    update(&mut scheduler, &hierarchy, 0.0, 2.0, [0.5; 3]);
    let request = scheduler.take_next().unwrap().unwrap();
    assert_eq!(request.ticket().block_id(), BlockId::new(11));
    assert_eq!(
        scheduler
            .complete(request.ticket(), Ok(BlockId::new(11)))
            .unwrap(),
        RequestCompletion::Delivered
    );

    assert!(scheduler.invalidate_completed(BlockId::new(11)));
    assert!(!scheduler.invalidate_completed(BlockId::new(11)));
    assert_eq!(scheduler.pending_count(), 1);
    assert_eq!(
        scheduler.take_next().unwrap().unwrap().ticket().block_id(),
        BlockId::new(11)
    );
}

#[test]
fn cached_visible_blocks_are_not_queued_until_the_cache_invalidates_them() {
    let hierarchy = two_leaf_hierarchy();
    let mut scheduler = BlockRequestScheduler::new(1).unwrap();
    update(&mut scheduler, &hierarchy, 0.0, 2.0, [0.5; 3]);
    assert!(scheduler.mark_cached(BlockId::new(11)));
    assert_eq!(scheduler.pending_count(), 0);
    assert!(scheduler.take_next().unwrap().is_none());

    assert!(scheduler.invalidate_completed(BlockId::new(11)));
    assert_eq!(scheduler.pending_count(), 1);
    assert_eq!(
        scheduler.take_next().unwrap().unwrap().ticket().block_id(),
        BlockId::new(11)
    );
}

#[test]
fn cancels_obsolete_reads_and_discards_stale_results_without_freeing_slots_early() {
    let hierarchy = two_leaf_hierarchy();
    let mut scheduler = BlockRequestScheduler::new(1).unwrap();
    update(&mut scheduler, &hierarchy, 0.0, 10.0, [8.5; 3]);
    let old = scheduler.take_next().unwrap().unwrap().ticket();

    let changed = scheduler
        .update_view(
            &hierarchy,
            &box_frustum(0.0, 2.0),
            LodSelectionConfig::new(1.0, 10).unwrap(),
            [0.5; 3],
            |_| 0.0,
        )
        .unwrap();
    assert_eq!(changed.cancelled(), [old]);
    assert_eq!(scheduler.cancelled_in_flight_count(), 1);
    assert!(scheduler.take_next().unwrap().is_none());

    assert_eq!(
        scheduler.complete(old, Ok(BlockId::new(22))).unwrap(),
        RequestCompletion::Discarded
    );
    assert_eq!(
        scheduler.take_next().unwrap().unwrap().ticket().block_id(),
        BlockId::new(11)
    );
}

#[test]
fn a_cancelled_ticket_does_not_revive_when_the_view_returns_to_its_block() {
    let hierarchy = two_leaf_hierarchy();
    let mut scheduler = BlockRequestScheduler::new(1).unwrap();
    update(&mut scheduler, &hierarchy, 0.0, 10.0, [8.5; 3]);
    let old = scheduler.take_next().unwrap().unwrap().ticket();

    update(&mut scheduler, &hierarchy, 0.0, 2.0, [0.5; 3]);
    update(&mut scheduler, &hierarchy, 0.0, 10.0, [8.5; 3]);
    assert!(scheduler.take_next().unwrap().is_none());

    assert_eq!(
        scheduler.complete(old, Ok(BlockId::new(22))).unwrap(),
        RequestCompletion::Discarded
    );
    let retried = scheduler.take_next().unwrap().unwrap().ticket();
    assert_ne!(retried, old);
    assert_eq!(retried.block_id(), old.block_id());
}

#[test]
fn deduplicates_blocks_shared_by_selected_nodes_and_allows_explicit_retry() {
    let hierarchy = SpatialHierarchy::new(
        vec![
            node(1, None, [0.0; 3], [10.0; 3], &[]),
            node(2, Some(1), [0.0; 3], [5.0; 3], &[44]),
            node(3, Some(1), [5.0, 0.0, 0.0], [10.0; 3], &[44]),
        ],
        Vec::new(),
        None,
    )
    .unwrap();
    let mut scheduler = BlockRequestScheduler::new(2).unwrap();
    update(&mut scheduler, &hierarchy, 0.0, 10.0, [4.0, 0.5, 0.5]);
    assert_eq!(scheduler.pending_count(), 1);
    let request = scheduler.take_next().unwrap().unwrap();
    assert_eq!(
        scheduler
            .complete(request.ticket(), Ok(BlockId::new(45)))
            .unwrap(),
        RequestCompletion::UnexpectedBlock {
            expected: BlockId::new(44),
            actual: BlockId::new(45),
        }
    );
    assert_eq!(scheduler.pending_count(), 0);
    assert!(scheduler.retry(BlockId::new(44)));
    let retry = scheduler.take_next().unwrap().unwrap();
    assert_eq!(retry.ticket().block_id(), BlockId::new(44));
    assert_eq!(
        scheduler.complete(retry.ticket(), Err(())).unwrap(),
        RequestCompletion::Failed
    );
    assert!(!scheduler.retry(BlockId::new(45)));
    assert!(scheduler.retry(BlockId::new(44)));
}

#[test]
fn cancel_all_keeps_active_slots_until_the_read_is_reported() {
    let hierarchy = two_leaf_hierarchy();
    let mut scheduler = BlockRequestScheduler::new(1).unwrap();
    update(&mut scheduler, &hierarchy, 0.0, 10.0, [8.5; 3]);
    let request = scheduler.take_next().unwrap().unwrap().ticket();

    assert_eq!(scheduler.cancel_all(), [request]);
    assert_eq!(scheduler.cancel_all(), []);
    assert_eq!(scheduler.in_flight_count(), 1);
    assert_eq!(scheduler.pending_count(), 0);
    assert_eq!(
        scheduler.complete(request, Ok(request.block_id())).unwrap(),
        RequestCompletion::Discarded
    );
    assert!(matches!(
        scheduler.complete(request, Ok(request.block_id())),
        Err(SchedulerError::UnknownTicket(_))
    ));
    assert_eq!(scheduler.in_flight_count(), 0);
}

#[test]
fn invalid_updates_leave_the_current_queue_unchanged() {
    let hierarchy = two_leaf_hierarchy();
    assert!(matches!(
        BlockRequestScheduler::new(0),
        Err(SchedulerError::ZeroConcurrencyLimit)
    ));
    let mut scheduler = BlockRequestScheduler::new(1).unwrap();
    update(&mut scheduler, &hierarchy, 0.0, 10.0, [8.5; 3]);
    assert!(matches!(
        scheduler.update_view(
            &hierarchy,
            &box_frustum(0.0, 10.0),
            LodSelectionConfig::new(1.0, 10).unwrap(),
            [f64::NAN, 0.0, 0.0],
            |_| 0.0,
        ),
        Err(SchedulerError::InvalidCameraPosition(_))
    ));
    assert_eq!(scheduler.pending_count(), 2);
}

#[test]
fn additive_hierarchy_schedules_ancestor_and_descendant_payloads() {
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![
            node(1, None, [0.0; 3], [10.0; 3], &[1]),
            node(2, Some(1), [0.0; 3], [5.0; 3], &[2]),
        ],
        Vec::new(),
        Some(2),
        HierarchySemantics::Additive,
    )
    .unwrap();
    let mut scheduler = BlockRequestScheduler::new(2).unwrap();
    let selected = scheduler
        .update_view(
            &hierarchy,
            &box_frustum(0.0, 10.0),
            LodSelectionConfig::new(1.0, 2).unwrap(),
            [1.0; 3],
            |_| 2.0,
        )
        .unwrap();
    assert_eq!(selected.selection().point_count(), 2);
    assert_eq!(scheduler.pending_count(), 2);
}
