use std::{collections::HashSet, sync::Arc};
use thiserror::Error;
use viewer_core::{
    BlockId, CacheInsertRejection, CoordinateFrame, DatasetMetadata, DecodedBlockCache,
    LodSelectionConfig, LodSelectionError, LodTransitionPlan, LodTransitionPlanner, NodeId,
    PointBlock, SpatialBounds, SpatialFrustum, SpatialHierarchy,
};
use viewer_render_wgpu::{
    GpuRendererError, SurfaceRenderer, VoxelSceneBuildError, build_voxel_scene_with_limit,
};

/// Decoded blocks and resident spatial metadata for one open dataset.
pub struct LodDataset {
    metadata: Arc<DatasetMetadata>,
    visual_scale: f64,
    cache: DecodedBlockCache,
    planner: LodTransitionPlanner,
    visible_plan: Option<LodTransitionPlan>,
    visible_scene_metrics: (f64, f64, usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LodViewStats {
    pub visible_blocks: usize,
    pub visible_points: u64,
    pub rendered_voxels: usize,
    pub voxel_budget: usize,
    pub target_voxel_pixels: f64,
    pub lod_stage_pixels: f64,
    pub min_voxel_edge_pixels: f64,
    pub max_voxel_edge_pixels: f64,
    pub oversized_voxels: usize,
    pub max_visible_level: Option<u32>,
    pub cache_bytes: usize,
    pub cache_budget_bytes: usize,
    pub pending_block_ids: Vec<BlockId>,
    pub uncovered_nodes: usize,
    pub budget_omitted_nodes: usize,
}

#[derive(Debug, Error)]
pub enum LodDatasetError {
    #[error("LOD metadata coordinate origin must contain only finite values")]
    InvalidCoordinateOrigin,
    #[error("block {0:?} is not referenced by the dataset hierarchy")]
    UnknownBlock(BlockId),
    #[error("block reader returned {actual:?} for requested block {requested:?}")]
    UnexpectedBlock { requested: BlockId, actual: BlockId },
    #[error("block read failed: {0}")]
    BlockRead(String),
    #[error("requested detail exceeds the source-point safety limit in {0} regions")]
    SourceDetailBudgetExceeded(usize),
    #[error("camera viewport is zero-sized or unavailable")]
    CameraUnavailable,
    #[error("decoded block was not cached: {0:?}")]
    CacheRejected(CacheInsertRejection),
    #[error("LOD selection failed: {0}")]
    Selection(#[from] LodSelectionError),
    #[error("GPU cut update failed: {0}")]
    Gpu(#[from] GpuRendererError),
    #[error("voxel scene build failed: {0}")]
    VoxelScene(#[from] VoxelSceneBuildError),
}

impl LodDataset {
    pub fn new(
        coordinate_origin: [f64; 3],
        hierarchy: SpatialHierarchy,
        cache_bytes: usize,
    ) -> Result<Self, LodDatasetError> {
        let coordinate_frame = CoordinateFrame::new(coordinate_origin)
            .map_err(|_| LodDatasetError::InvalidCoordinateOrigin)?;
        Ok(Self::from_shared_metadata(
            Arc::new(DatasetMetadata::new(coordinate_frame, hierarchy)),
            cache_bytes,
        ))
    }

    pub fn from_shared_metadata(metadata: Arc<DatasetMetadata>, cache_bytes: usize) -> Self {
        Self::from_shared_metadata_and_visual_scale(metadata, 1.0, cache_bytes)
    }

    pub fn from_shared_metadata_and_visual_scale(
        metadata: Arc<DatasetMetadata>,
        visual_scale: f64,
        cache_bytes: usize,
    ) -> Self {
        Self {
            metadata,
            visual_scale,
            cache: DecodedBlockCache::new(cache_bytes),
            planner: LodTransitionPlanner::new(),
            visible_plan: None,
            visible_scene_metrics: (0.0, 0.0, 0),
        }
    }

    pub fn insert_block(&mut self, block: PointBlock) -> Result<usize, LodDatasetError> {
        let id = block.id();
        if !self.metadata.hierarchy().references_block(id) {
            return Err(LodDatasetError::UnknownBlock(id));
        }
        let result = self.cache.insert(block);
        if !result.cached {
            return Err(LodDatasetError::CacheRejected(
                result
                    .rejection
                    .expect("rejected cache insert includes a reason"),
            ));
        }
        Ok(result.accounted_bytes)
    }

    pub fn block_cache_budget_bytes(&self) -> usize {
        self.cache.max_bytes()
    }

    pub fn references_block(&self, id: BlockId) -> bool {
        self.metadata.hierarchy().references_block(id)
    }

    /// Drops decoded blocks and the current scene so a different attribute set can be loaded.
    pub fn clear_loaded_blocks(
        &mut self,
        renderer: &mut SurfaceRenderer<'_>,
    ) -> Result<(), LodDatasetError> {
        renderer.sync_visible_blocks(&[], &self.cache)?;
        self.planner.reset(&mut self.cache);
        self.cache.clear();
        self.visible_plan = None;
        self.visible_scene_metrics = (0.0, 0.0, 0);
        Ok(())
    }

    pub fn fit_camera(&self, renderer: &mut SurfaceRenderer<'_>) -> bool {
        if renderer.set_visual_scale(self.visual_scale).is_err() {
            return false;
        }
        renderer.fit_spatial_bounds(
            self.metadata.hierarchy().stats().root_bounds,
            self.metadata.coordinate_frame().origin(),
        )
    }

    pub fn update_view(
        &mut self,
        renderer: &mut SurfaceRenderer<'_>,
        target_voxel_pixels: f64,
    ) -> Result<LodViewStats, LodDatasetError> {
        self.update_view_with_loader(renderer, target_voxel_pixels, |_| Ok(None))
    }

    pub fn update_view_with_loader(
        &mut self,
        renderer: &mut SurfaceRenderer<'_>,
        target_voxel_pixels: f64,
        load_block: impl FnMut(BlockId) -> Result<Option<PointBlock>, String>,
    ) -> Result<LodViewStats, LodDatasetError> {
        self.update_view_progressive(
            renderer,
            target_voxel_pixels,
            target_voxel_pixels,
            target_voxel_pixels,
            load_block,
        )
    }

    pub fn update_view_progressive(
        &mut self,
        renderer: &mut SurfaceRenderer<'_>,
        target_voxel_pixels: f64,
        selection_error_pixels: f64,
        stage_voxel_pixels: f64,
        mut load_block: impl FnMut(BlockId) -> Result<Option<PointBlock>, String>,
    ) -> Result<LodViewStats, LodDatasetError> {
        renderer.set_visual_scale(self.visual_scale)?;
        // Source points are LOD input, not rendered GPU instances. Bound the output scene below.
        let point_budget = u64::MAX;
        let config = LodSelectionConfig::new(selection_error_pixels, point_budget)?;
        let frustum = renderer
            .spatial_frustum(self.metadata.coordinate_frame().origin())
            .ok_or(LodDatasetError::CameraUnavailable)?;
        let target = match self
            .metadata
            .hierarchy()
            .select_lod(&frustum, config, |node| {
                renderer.projected_error_pixels(
                    node.bounds(),
                    bounds_side(node.bounds()) / 8.0,
                    self.metadata.coordinate_frame().origin(),
                )
            }) {
            Ok(target) => target,
            Err(error @ LodSelectionError::BaseCutExceedsBudget { .. }) => {
                renderer.sync_visible_blocks(&[], &self.cache)?;
                self.planner.reset(&mut self.cache);
                self.visible_plan = None;
                self.visible_scene_metrics = (0.0, 0.0, 0);
                return Err(LodDatasetError::Selection(error));
            }
            Err(error) => return Err(LodDatasetError::Selection(error)),
        };
        if !target.budget_limited().is_empty() {
            return Err(LodDatasetError::SourceDetailBudgetExceeded(
                target.budget_limited().len(),
            ));
        }
        let initial_plan = self
            .planner
            .prepare(self.metadata.hierarchy(), &target, &self.cache);
        let next_block = highest_priority_pending_block(
            self.metadata.hierarchy(),
            initial_plan.pending_blocks(),
            &frustum,
            renderer,
            self.metadata.coordinate_frame().origin(),
        );
        if let Some(requested) = next_block
            && let Some(block) = load_block(requested).map_err(LodDatasetError::BlockRead)?
        {
            if block.id() != requested {
                return Err(LodDatasetError::UnexpectedBlock {
                    requested,
                    actual: block.id(),
                });
            }
            self.insert_block(block)?;
        }
        let plan = self
            .planner
            .prepare(self.metadata.hierarchy(), &target, &self.cache);
        let coordinate_origin = self.metadata.coordinate_frame().origin();
        let local_frustum = renderer
            .spatial_frustum([0.0; 3])
            .ok_or(LodDatasetError::CameraUnavailable)?;
        if !plan.node_ids().is_empty() || plan.pending_blocks().is_empty() {
            let voxel_scene = build_voxel_scene_with_limit(
                self.metadata.hierarchy(),
                plan.node_ids(),
                &self.cache,
                coordinate_origin,
                stage_voxel_pixels,
                &|bounds| {
                    if local_frustum.intersects(&bounds) {
                        renderer.projected_voxel_edge_pixels(bounds, [0.0; 3])
                    } else {
                        0.0
                    }
                },
                renderer.max_visible_instances(),
            )?;
            renderer.upload_voxel_cut(&voxel_scene.scene, plan.block_ids())?;
            self.planner.commit(&plan, &mut self.cache);
            self.visible_plan = Some(plan.clone());
            self.visible_scene_metrics = (
                voxel_scene.min_edge_pixels,
                voxel_scene.max_edge_pixels,
                voxel_scene.oversized_voxels,
            );
        }
        let visible_plan = self.visible_plan.as_ref().unwrap_or(&plan);
        let stats = LodViewStats {
            visible_blocks: visible_plan.block_ids().len(),
            visible_points: visible_plan.point_count(),
            rendered_voxels: renderer.visible_instance_count(),
            voxel_budget: renderer.max_visible_instances(),
            target_voxel_pixels,
            lod_stage_pixels: stage_voxel_pixels,
            min_voxel_edge_pixels: self.visible_scene_metrics.0,
            max_voxel_edge_pixels: self.visible_scene_metrics.1,
            oversized_voxels: self.visible_scene_metrics.2,
            max_visible_level: max_visible_level(
                self.metadata.hierarchy(),
                visible_plan.node_ids(),
            ),
            cache_bytes: self.cache.used_bytes(),
            cache_budget_bytes: self.cache.max_bytes(),
            pending_block_ids: plan.pending_blocks().to_vec(),
            uncovered_nodes: plan.uncovered_nodes().len(),
            budget_omitted_nodes: plan.budget_omitted_nodes().len(),
        };
        Ok(stats)
    }

    pub fn cache_bytes(&self) -> usize {
        self.cache.used_bytes()
    }
}

fn bounds_side(bounds: SpatialBounds) -> f64 {
    (0..3)
        .map(|axis| bounds.max()[axis] - bounds.min()[axis])
        .fold(0.0, f64::max)
}

fn max_visible_level(hierarchy: &SpatialHierarchy, node_ids: &[NodeId]) -> Option<u32> {
    node_ids
        .iter()
        .filter_map(|id| hierarchy.node(*id).map(|node| node.level()))
        .max()
}

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn next_lod_stage(
    current_stage: Option<f64>,
    target_pixels: f64,
    has_pending_blocks: bool,
    can_load_blocks: bool,
) -> (Option<f64>, bool) {
    if has_pending_blocks {
        return (current_stage, can_load_blocks);
    }
    let stage_pixels = current_stage.unwrap_or(16.0);
    if stage_pixels > target_pixels {
        return (
            Some((stage_pixels / 2.0).max(target_pixels)),
            can_load_blocks,
        );
    }
    (Some(stage_pixels), false)
}

fn highest_priority_pending_block(
    hierarchy: &SpatialHierarchy,
    pending: &[BlockId],
    frustum: &SpatialFrustum,
    renderer: &SurfaceRenderer<'_>,
    coordinate_origin: [f64; 3],
) -> Option<BlockId> {
    if pending.is_empty() {
        return None;
    }
    let pending_ids = pending.iter().copied().collect::<HashSet<_>>();
    let mut best = None;
    for node in hierarchy.query_frustum(frustum.clone()) {
        let priority = renderer.projected_error_pixels(
            node.bounds(),
            node.geometric_error(),
            coordinate_origin,
        );
        for block_id in node.blocks() {
            if pending_ids.contains(block_id) {
                let candidate = (*block_id, priority);
                let is_better = best.is_none_or(|current| priority_precedes(candidate, current));
                if is_better {
                    best = Some(candidate);
                }
            }
        }
    }
    best.map(|(block_id, _)| block_id)
}

fn priority_precedes(candidate: (BlockId, f64), current: (BlockId, f64)) -> bool {
    candidate.1.total_cmp(&current.1).is_gt()
        || (candidate.1.total_cmp(&current.1).is_eq() && candidate.0 < current.0)
}

#[cfg(test)]
mod tests {
    use viewer_core::{
        BlockId, ColorRgb, DecodedBlockCache, HierarchySemantics, NodeId, Point, PointBatch,
        PointBlock, SpatialBounds, SpatialHierarchy, SpatialNode,
    };

    use super::{LodDataset, LodDatasetError, bounds_side, next_lod_stage, priority_precedes};
    use viewer_render_wgpu::{RenderScene, build_voxel_scene};

    fn scene_for_test(
        hierarchy: &SpatialHierarchy,
        ids: &[NodeId],
        cache: &DecodedBlockCache,
        origin: [f64; 3],
    ) -> RenderScene {
        build_voxel_scene(hierarchy, ids, cache, origin, 1.0, &|bounds| {
            bounds_side(bounds) * 8.0
        })
        .scene
    }

    #[test]
    fn progressive_lod_advances_from_coarse_to_requested_pixel_stages() {
        let (stage, continue_loading) = next_lod_stage(None, 4.0, false, true);
        assert_eq!(stage, Some(8.0));
        assert!(continue_loading);

        let (stage, continue_loading) = next_lod_stage(stage, 4.0, false, true);
        assert_eq!(stage, Some(4.0));
        assert!(continue_loading);

        let (stage, continue_loading) = next_lod_stage(stage, 4.0, false, true);
        assert_eq!(stage, Some(4.0));
        assert!(!continue_loading);
    }

    #[test]
    fn progressive_lod_can_refine_to_half_pixel_target() {
        let mut stage = None;
        let mut stages = vec![16.0];
        loop {
            let (next, continue_loading) = next_lod_stage(stage, 0.5, false, true);
            if next == stage {
                assert!(!continue_loading);
                break;
            }
            stage = next;
            stages.push(stage.unwrap());
        }

        assert_eq!(stages, [16.0, 8.0, 4.0, 2.0, 1.0, 0.5]);
    }

    #[test]
    fn progressive_lod_waits_for_current_stage_blocks_and_stops_on_read_failure() {
        assert_eq!(next_lod_stage(None, 4.0, true, true), (None, true));
        assert_eq!(
            next_lod_stage(Some(8.0), 4.0, true, true),
            (Some(8.0), true)
        );
        assert_eq!(
            next_lod_stage(Some(8.0), 4.0, true, false),
            (Some(8.0), false)
        );
        assert_eq!(next_lod_stage(None, 16.0, false, true), (Some(16.0), false));
    }

    #[test]
    fn progressive_block_priority_prefers_larger_error_then_lower_block_id() {
        assert!(priority_precedes(
            (BlockId::new(2), 8.0),
            (BlockId::new(1), 4.0)
        ));
        assert!(priority_precedes(
            (BlockId::new(1), 4.0),
            (BlockId::new(2), 4.0)
        ));
        assert!(!priority_precedes(
            (BlockId::new(2), 4.0),
            (BlockId::new(1), 4.0)
        ));
    }

    fn dataset(cache_bytes: usize) -> LodDataset {
        let hierarchy = SpatialHierarchy::new_with_semantics(
            vec![
                SpatialNode::new(
                    NodeId::new(1),
                    None,
                    0,
                    SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                    1,
                    0.0,
                    vec![BlockId::new(10)],
                )
                .unwrap(),
            ],
            Vec::new(),
            Some(1),
            HierarchySemantics::Replacement,
        )
        .unwrap();
        LodDataset::new([0.0; 3], hierarchy, cache_bytes).unwrap()
    }

    fn block(id: u64) -> PointBlock {
        block_with_points(id, 1)
    }

    fn block_with_points(id: u64, count: usize) -> PointBlock {
        PointBlock::new(
            BlockId::new(id),
            PointBatch::new(
                (0..count)
                    .map(|_| Point::new([0.5; 3], ColorRgb::new(1, 2, 3)))
                    .collect(),
            ),
            Vec::new(),
        )
    }

    #[test]
    fn accepts_only_referenced_blocks_and_accounts_decoded_memory() {
        let payload_bytes = block(10).estimated_memory_bytes();
        let mut dataset = dataset(payload_bytes);
        assert!(matches!(
            dataset.insert_block(block(11)),
            Err(LodDatasetError::UnknownBlock(id)) if id == BlockId::new(11)
        ));
        let accounted_bytes = dataset.insert_block(block(10)).unwrap();
        assert_eq!(accounted_bytes, payload_bytes);
        assert_eq!(dataset.cache_bytes(), payload_bytes);
        assert!(matches!(
            dataset.insert_block(block_with_points(10, 2)),
            Err(LodDatasetError::CacheRejected(_))
        ));
    }

    #[test]
    fn reports_cache_budget_for_bounding_external_block_imports() {
        let dataset = dataset(4096);
        assert_eq!(dataset.block_cache_budget_bytes(), 4096);
    }

    #[test]
    fn reports_the_deepest_level_in_the_visible_cut() {
        let dataset = dataset(4096);
        assert_eq!(
            super::max_visible_level(dataset.metadata.hierarchy(), &[NodeId::new(1)]),
            Some(0)
        );
        assert_eq!(
            super::max_visible_level(dataset.metadata.hierarchy(), &[]),
            None
        );
    }

    #[test]
    fn replacement_voxels_use_integer_cells_and_deduplicate_source_points() {
        let hierarchy = dataset(4096).metadata.hierarchy().clone();
        let mut cache = DecodedBlockCache::new(4096);
        cache.insert(PointBlock::new(
            BlockId::new(10),
            PointBatch::new(vec![
                Point::new([0.13, 0.2, 0.2], ColorRgb::new(100, 0, 0)),
                Point::new([0.20, 0.2, 0.2], ColorRgb::new(0, 100, 0)),
                Point::new([0.25, 0.2, 0.2], ColorRgb::new(0, 0, 100)),
            ]),
            Vec::new(),
        ));

        let scene = scene_for_test(&hierarchy, &[NodeId::new(1)], &cache, [0.0; 3]);
        let instances = scene.instances();
        assert_eq!(instances.len(), 2);
        assert!(
            instances
                .iter()
                .all(|instance| instance.size < 0.125 && instance.size > 0.124)
        );
        let mut centers: Vec<_> = instances
            .iter()
            .map(|instance| instance.center[0])
            .collect();
        centers.sort_by(f32::total_cmp);
        assert_eq!(centers, [0.1875, 0.3125]);
        let half_size = f32::from_bits(instances[0].size.to_bits()) / 2.0;
        assert!(centers[0] + half_size <= centers[1] - half_size);
    }

    #[test]
    fn voxel_scene_converts_world_bounds_back_to_dataset_local_positions() {
        let origin = [100.0, 200.0, 300.0];
        let hierarchy = SpatialHierarchy::new_with_semantics(
            vec![
                SpatialNode::new(
                    NodeId::new(1),
                    None,
                    0,
                    SpatialBounds::new(origin, [101.0, 201.0, 301.0]).unwrap(),
                    1,
                    0.0,
                    vec![BlockId::new(10)],
                )
                .unwrap(),
            ],
            Vec::new(),
            Some(1),
            HierarchySemantics::Replacement,
        )
        .unwrap();
        let mut cache = DecodedBlockCache::new(4096);
        cache.insert(PointBlock::new(
            BlockId::new(10),
            PointBatch::new(vec![Point::new([0.5; 3], ColorRgb::new(20, 30, 40))]),
            Vec::new(),
        ));

        let scene = scene_for_test(&hierarchy, &[NodeId::new(1)], &cache, origin);

        assert_eq!(scene.instance_count(), 1);
        assert_eq!(scene.instances()[0].center, [0.5625; 3]);
        assert!(scene.instances()[0].center.iter().all(|value| *value < 1.0));
    }

    #[test]
    fn additive_ancestors_and_descendants_share_one_integer_grid() {
        let hierarchy = SpatialHierarchy::new_with_semantics(
            vec![
                SpatialNode::new(
                    NodeId::new(1),
                    None,
                    0,
                    SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                    1,
                    0.0,
                    vec![BlockId::new(10)],
                )
                .unwrap(),
                SpatialNode::new(
                    NodeId::new(2),
                    Some(NodeId::new(1)),
                    1,
                    SpatialBounds::new([0.0; 3], [0.5; 3]).unwrap(),
                    1,
                    0.0,
                    vec![BlockId::new(11)],
                )
                .unwrap(),
            ],
            Vec::new(),
            Some(2),
            HierarchySemantics::Additive,
        )
        .unwrap();
        let mut cache = DecodedBlockCache::new(4096);
        for id in [10, 11] {
            cache.insert(PointBlock::new(
                BlockId::new(id),
                PointBatch::new(vec![Point::new([0.2; 3], ColorRgb::new(20, 30, 40))]),
                Vec::new(),
            ));
        }

        let scene = scene_for_test(
            &hierarchy,
            &[NodeId::new(1), NodeId::new(2)],
            &cache,
            [0.0; 3],
        );
        assert_eq!(scene.instance_count(), 1);
        assert!(scene.instances()[0].size < 0.125);
    }
    #[test]
    fn source_leaf_depth_does_not_determine_voxel_size_or_limit_granularity() {
        let make = |id, parent: Option<u64>, level, min, max, blocks: Vec<u64>| {
            SpatialNode::new(
                NodeId::new(id),
                parent.map(NodeId::new),
                level,
                SpatialBounds::new(min, max).unwrap(),
                if blocks.is_empty() { 0 } else { 1 },
                0.0,
                blocks.into_iter().map(BlockId::new).collect(),
            )
            .unwrap()
        };
        let hierarchy = SpatialHierarchy::new_with_semantics(
            vec![
                make(1, None, 0, [0.0; 3], [2.0; 3], vec![]),
                make(2, Some(1), 1, [0.0; 3], [1.0; 3], vec![10]),
                make(3, Some(1), 1, [1.0, 0.0, 0.0], [2.0, 1.0, 1.0], vec![]),
                make(4, Some(3), 2, [1.0, 0.0, 0.0], [1.5, 0.5, 0.5], vec![11]),
            ],
            vec![],
            Some(2),
            HierarchySemantics::Replacement,
        )
        .unwrap();
        let mut cache = DecodedBlockCache::new(4096);
        for (id, point) in [(10, [0.2, 0.2, 0.5]), (11, [1.2, 0.2, 0.5])] {
            cache.insert(PointBlock::new(
                BlockId::new(id),
                PointBatch::new(vec![Point::new(point, ColorRgb::new(1, 2, 3))]),
                vec![],
            ));
        }
        let result = build_voxel_scene(
            &hierarchy,
            &[NodeId::new(2), NodeId::new(4)],
            &cache,
            [0.0; 3],
            1.0,
            &|bounds| bounds_side(bounds) * 32.0,
        );
        assert_eq!(result.scene.instance_count(), 2);
        assert_eq!(
            result.scene.instances()[0].size,
            result.scene.instances()[1].size
        );
        assert!(result.scene.instances()[0].size < 1.0 / 32.0);
        assert_eq!(result.oversized_voxels, 0);
        assert!((result.max_edge_pixels - 1.0).abs() < 1e-10);
    }

    #[test]
    fn adaptive_cells_obey_pixel_target_grow_with_distance_and_never_overlap() {
        let hierarchy = SpatialHierarchy::new_with_semantics(
            vec![
                SpatialNode::new(
                    NodeId::new(1),
                    None,
                    0,
                    SpatialBounds::new([0.0; 3], [2.0; 3]).unwrap(),
                    256,
                    0.0,
                    vec![BlockId::new(10)],
                )
                .unwrap(),
            ],
            vec![],
            Some(256),
            HierarchySemantics::Replacement,
        )
        .unwrap();
        let points = (0..16)
            .flat_map(|x| {
                (0..16).map(move |z| {
                    Point::new(
                        [x as f32 / 8.0, 0.25, z as f32 / 8.0],
                        ColorRgb::new(10, 20, 30),
                    )
                })
            })
            .collect();
        let mut cache = DecodedBlockCache::new(65536);
        cache.insert(PointBlock::new(
            BlockId::new(10),
            PointBatch::new(points),
            vec![],
        ));
        let project = |bounds: SpatialBounds| bounds_side(bounds) * 16.0 / (1.0 + bounds.min()[2]);
        for target in [1.0, 2.0, 4.0, 16.0] {
            let result = build_voxel_scene(
                &hierarchy,
                &[NodeId::new(1)],
                &cache,
                [0.0; 3],
                target,
                &project,
            );
            assert_eq!(result.oversized_voxels, 0);
            assert!(result.max_edge_pixels <= target + 1e-10);
            assert!(result.min_edge_pixels >= target / 2.0 - 1e-10);
            let instances = result.scene.instances();
            let near = instances
                .iter()
                .filter(|v| v.center[2] < 0.25)
                .map(|v| v.size)
                .fold(f32::INFINITY, f32::min);
            let far = instances
                .iter()
                .filter(|v| v.center[2] > 1.5)
                .map(|v| v.size)
                .fold(0.0_f32, f32::max);
            if target <= 4.0 {
                assert!(far > near);
            }
            for (i, a) in instances.iter().enumerate() {
                for b in &instances[i + 1..] {
                    let overlap = (0..3).all(|axis| {
                        (f64::from(a.center[axis]) - f64::from(b.center[axis])).abs()
                            < (f64::from(a.size) + f64::from(b.size)) * 0.5 - 1e-8
                    });
                    assert!(!overlap, "adaptive cells must not overlap");
                }
            }
        }
    }

    #[test]
    fn fine_local_cells_survive_a_large_geographic_origin() {
        let origin = [1.0e12; 3];
        let hierarchy = SpatialHierarchy::new_with_semantics(
            vec![
                SpatialNode::new(
                    NodeId::new(1),
                    None,
                    0,
                    SpatialBounds::new(origin, origin.map(|value| value + 1.0)).unwrap(),
                    2,
                    0.0,
                    vec![BlockId::new(10)],
                )
                .unwrap(),
            ],
            vec![],
            Some(2),
            HierarchySemantics::Replacement,
        )
        .unwrap();
        let mut cache = DecodedBlockCache::new(4096);
        cache.insert(PointBlock::new(
            BlockId::new(10),
            PointBatch::new(vec![
                Point::new([0.5; 3], ColorRgb::new(1, 2, 3)),
                Point::new([0.500002, 0.5, 0.5], ColorRgb::new(1, 2, 3)),
            ]),
            vec![],
        ));
        let result = build_voxel_scene(
            &hierarchy,
            &[NodeId::new(1)],
            &cache,
            origin,
            1.0,
            &|bounds| bounds_side(bounds) * 1_048_576.0,
        );
        assert_eq!(result.scene.instance_count(), 2);
        assert_eq!(result.oversized_voxels, 0);
        assert_ne!(
            result.scene.instances()[0].center,
            result.scene.instances()[1].center
        );
        assert!(
            result
                .scene
                .instances()
                .iter()
                .all(|cell| cell.size < 1.0e-6)
        );
    }
}
