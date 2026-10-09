use std::collections::{BTreeSet, HashMap};

use viewer_core::{BlockId, DecodedBlockCache};

use crate::{GpuRendererError, RenderScene, SceneBounds};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResidentBlockInfo {
    pub id: BlockId,
    pub instance_count: usize,
    pub bounds: Option<SceneBounds>,
}

#[derive(Debug)]
pub(crate) struct BlockSyncPlan {
    pub selected: Vec<BlockId>,
    pub added: Vec<(BlockId, RenderScene)>,
    pub removed: Vec<BlockId>,
    pub visible_instance_count: usize,
    pub uploaded_bytes: u64,
    pub visible_bounds: Option<SceneBounds>,
}

pub(crate) fn plan_block_sync(
    resident: &[ResidentBlockInfo],
    requested_ids: &[BlockId],
    cache: &DecodedBlockCache,
    max_buffer_size: u64,
) -> Result<BlockSyncPlan, GpuRendererError> {
    let current_ids: BTreeSet<_> = resident.iter().map(|block| block.id).collect();
    let selected: Vec<_> = requested_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let selected_set: BTreeSet<_> = selected.iter().copied().collect();
    let resident_by_id: HashMap<_, _> = resident.iter().map(|block| (block.id, *block)).collect();
    let removed = current_ids.difference(&selected_set).copied().collect();

    let mut added = Vec::new();
    let mut visible_instance_count = 0_usize;
    let mut uploaded_bytes = 0_u64;
    let mut visible_bounds = None;
    for id in &selected {
        if let Some(block) = resident_by_id.get(id) {
            visible_instance_count = visible_instance_count
                .checked_add(block.instance_count)
                .ok_or(GpuRendererError::InvalidSceneSize)?;
            visible_bounds = merge_bounds(visible_bounds, block.bounds);
            continue;
        }

        let payload = cache
            .peek(*id)
            .ok_or(GpuRendererError::MissingBlockPayload(*id))?;
        let scene = RenderScene::from_points(payload.points().points());
        let instance_count = scene.instance_count();
        let bytes = instance_count
            .checked_mul(std::mem::size_of::<crate::InstanceRaw>())
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(GpuRendererError::InvalidSceneSize)?;
        if bytes > max_buffer_size || u32::try_from(instance_count).is_err() {
            return Err(GpuRendererError::InvalidSceneSize);
        }
        if instance_count != 0 && scene.bounds().is_none() {
            return Err(GpuRendererError::InvalidBlockData(*id));
        }
        visible_instance_count = visible_instance_count
            .checked_add(instance_count)
            .ok_or(GpuRendererError::InvalidSceneSize)?;
        uploaded_bytes = uploaded_bytes
            .checked_add(bytes)
            .ok_or(GpuRendererError::InvalidSceneSize)?;
        visible_bounds = merge_bounds(visible_bounds, scene.bounds());
        added.push((*id, scene));
    }

    Ok(BlockSyncPlan {
        selected,
        added,
        removed,
        visible_instance_count,
        uploaded_bytes,
        visible_bounds,
    })
}

fn merge_bounds(current: Option<SceneBounds>, next: Option<SceneBounds>) -> Option<SceneBounds> {
    match (current, next) {
        (Some(mut current), Some(next)) => {
            for axis in 0..3 {
                current.min[axis] = current.min[axis].min(next.min[axis]);
                current.max[axis] = current.max[axis].max(next.max[axis]);
            }
            Some(current)
        }
        (Some(bounds), None) | (None, Some(bounds)) => Some(bounds),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use viewer_core::{BlockId, ColorRgb, DecodedBlockCache, Point, PointBatch, PointBlock};

    use super::{ResidentBlockInfo, plan_block_sync};
    use crate::SceneBounds;

    fn block(id: u64, positions: &[[f32; 3]]) -> PointBlock {
        let points = positions
            .iter()
            .map(|position| Point::new(*position, ColorRgb::new(10, 20, 30)))
            .collect();
        PointBlock::new(BlockId::new(id), PointBatch::new(points), Vec::new())
    }

    fn cache(blocks: impl IntoIterator<Item = PointBlock>) -> DecodedBlockCache {
        let mut cache = DecodedBlockCache::new(1024 * 1024);
        for block in blocks {
            assert!(cache.insert(block).cached);
        }
        cache
    }

    #[test]
    fn repeated_and_intersecting_cuts_upload_only_new_blocks() {
        let cache = cache([
            block(1, &[[0.0, 0.0, 0.0]]),
            block(2, &[[2.0, 0.0, 0.0], [3.0, 0.0, 0.0]]),
            block(3, &[[4.0, 0.0, 0.0]]),
        ]);
        let cache_stats = cache.stats();
        let a = plan_block_sync(&[], &[BlockId::new(1), BlockId::new(2)], &cache, 1024).unwrap();
        assert_eq!(cache.stats(), cache_stats);
        assert_eq!(a.selected, [BlockId::new(1), BlockId::new(2)]);
        assert_eq!(a.added.len(), 2);
        assert_eq!(a.removed, []);
        assert_eq!(a.visible_instance_count, 3);
        assert_eq!(
            a.uploaded_bytes,
            3 * std::mem::size_of::<crate::InstanceRaw>() as u64
        );

        let current = [
            ResidentBlockInfo {
                id: BlockId::new(1),
                instance_count: 1,
                bounds: Some(SceneBounds {
                    min: [-0.5; 3],
                    max: [0.5; 3],
                }),
            },
            ResidentBlockInfo {
                id: BlockId::new(2),
                instance_count: 2,
                bounds: Some(SceneBounds {
                    min: [1.5, -0.5, -0.5],
                    max: [3.5, 0.5, 0.5],
                }),
            },
        ];
        let same =
            plan_block_sync(&current, &[BlockId::new(2), BlockId::new(1)], &cache, 1024).unwrap();
        assert!(same.added.is_empty());
        assert!(same.removed.is_empty());
        assert_eq!(same.uploaded_bytes, 0);

        let next =
            plan_block_sync(&current, &[BlockId::new(2), BlockId::new(3)], &cache, 1024).unwrap();
        assert_eq!(next.added.len(), 1);
        assert_eq!(next.added[0].0, BlockId::new(3));
        assert_eq!(next.removed, [BlockId::new(1)]);
        assert_eq!(next.visible_instance_count, 3);
    }

    #[test]
    fn duplicate_ids_are_deduplicated_and_empty_cut_releases_everything() {
        let cache = cache([block(1, &[[0.0, 0.0, 0.0]])]);
        let selection = [BlockId::new(1), BlockId::new(1)];
        let first = plan_block_sync(&[], &selection, &cache, 1024).unwrap();
        assert_eq!(first.selected, [BlockId::new(1)]);

        let current = [ResidentBlockInfo {
            id: BlockId::new(1),
            instance_count: 1,
            bounds: Some(SceneBounds {
                min: [-0.5; 3],
                max: [0.5; 3],
            }),
        }];
        let empty = plan_block_sync(&current, &[], &cache, 1024).unwrap();
        assert!(empty.selected.is_empty());
        assert_eq!(empty.removed, [BlockId::new(1)]);
        assert_eq!(empty.visible_instance_count, 0);
        assert_eq!(empty.visible_bounds, None);
    }

    #[test]
    fn missing_or_oversize_block_keeps_the_previous_cut_unchanged() {
        let current = [ResidentBlockInfo {
            id: BlockId::new(1),
            instance_count: 1,
            bounds: Some(SceneBounds {
                min: [-0.5; 3],
                max: [0.5; 3],
            }),
        }];
        let empty_cache = DecodedBlockCache::new(1024);
        assert_eq!(
            plan_block_sync(&current, &[BlockId::new(9)], &empty_cache, 1024).unwrap_err(),
            crate::GpuRendererError::MissingBlockPayload(BlockId::new(9))
        );
        assert_eq!(current[0].id, BlockId::new(1));

        let cache = cache([block(2, &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]])]);
        assert!(plan_block_sync(&current, &[BlockId::new(2)], &cache, 1).is_err());
        assert_eq!(current[0].id, BlockId::new(1));
    }

    #[test]
    fn invalid_payload_does_not_produce_a_partial_sync_plan() {
        let cache = cache([block(7, &[[f32::INFINITY, 0.0, 0.0]])]);
        assert_eq!(
            plan_block_sync(&[], &[BlockId::new(7)], &cache, 1024).unwrap_err(),
            crate::GpuRendererError::InvalidBlockData(BlockId::new(7))
        );
    }
}
