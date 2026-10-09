use std::sync::Arc;
use thiserror::Error;
use viewer_core::{
    BlockId, CacheInsertRejection, CoordinateFrame, DatasetMetadata, DecodedBlockCache,
    LodSelectionConfig, LodSelectionError, LodTransitionPlanner, PointBlock, SpatialHierarchy,
};
use viewer_render_wgpu::{GpuRendererError, SurfaceRenderer};

/// Decoded blocks and resident spatial metadata for one open dataset.
pub struct LodDataset {
    metadata: Arc<DatasetMetadata>,
    cache: DecodedBlockCache,
    planner: LodTransitionPlanner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LodViewStats {
    pub visible_blocks: usize,
    pub visible_points: u64,
    pub point_budget: u64,
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
    #[error("camera viewport is zero-sized or unavailable")]
    CameraUnavailable,
    #[error("decoded block was not cached: {0:?}")]
    CacheRejected(CacheInsertRejection),
    #[error("LOD selection failed: {0}")]
    Selection(#[from] LodSelectionError),
    #[error("GPU cut update failed: {0}")]
    Gpu(#[from] GpuRendererError),
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
        Self {
            metadata,
            cache: DecodedBlockCache::new(cache_bytes),
            planner: LodTransitionPlanner::new(),
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

    pub fn fit_camera(&self, renderer: &mut SurfaceRenderer<'_>) -> bool {
        renderer.fit_spatial_bounds(
            self.metadata.hierarchy().stats().root_bounds,
            self.metadata.coordinate_frame().origin(),
        )
    }

    pub fn update_view(
        &mut self,
        renderer: &mut SurfaceRenderer<'_>,
        max_error_pixels: f64,
        point_budget: u64,
    ) -> Result<LodViewStats, LodDatasetError> {
        self.update_view_with_loader(renderer, max_error_pixels, point_budget, |_| Ok(None))
    }

    pub fn update_view_with_loader(
        &mut self,
        renderer: &mut SurfaceRenderer<'_>,
        max_error_pixels: f64,
        point_budget: u64,
        mut load_block: impl FnMut(BlockId) -> Result<Option<PointBlock>, String>,
    ) -> Result<LodViewStats, LodDatasetError> {
        let config = LodSelectionConfig::new(max_error_pixels, point_budget)?;
        let frustum = renderer
            .spatial_frustum(self.metadata.coordinate_frame().origin())
            .ok_or(LodDatasetError::CameraUnavailable)?;
        let target = match self
            .metadata
            .hierarchy()
            .select_lod(&frustum, config, |node| {
                renderer.projected_error_pixels(
                    node.bounds(),
                    node.geometric_error(),
                    self.metadata.coordinate_frame().origin(),
                )
            }) {
            Ok(target) => target,
            Err(error @ LodSelectionError::BaseCutExceedsBudget { .. }) => {
                renderer.sync_visible_blocks(&[], &self.cache)?;
                self.planner.reset(&mut self.cache);
                return Err(LodDatasetError::Selection(error));
            }
            Err(error) => return Err(LodDatasetError::Selection(error)),
        };
        let initial_plan = self
            .planner
            .prepare(self.metadata.hierarchy(), &target, &self.cache);
        for requested in initial_plan.pending_blocks() {
            let Some(block) = load_block(*requested).map_err(LodDatasetError::BlockRead)? else {
                continue;
            };
            if block.id() != *requested {
                return Err(LodDatasetError::UnexpectedBlock {
                    requested: *requested,
                    actual: block.id(),
                });
            }
            self.insert_block(block)?;
        }
        let plan = self
            .planner
            .prepare(self.metadata.hierarchy(), &target, &self.cache);
        renderer.sync_visible_blocks(plan.block_ids(), &self.cache)?;
        self.planner.commit(&plan, &mut self.cache);
        let stats = LodViewStats {
            visible_blocks: plan.block_ids().len(),
            visible_points: plan.point_count(),
            point_budget: plan.point_budget(),
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

#[cfg(test)]
mod tests {
    use viewer_core::{
        BlockId, ColorRgb, HierarchySemantics, NodeId, Point, PointBatch, PointBlock,
        SpatialBounds, SpatialHierarchy, SpatialNode,
    };

    use super::{LodDataset, LodDatasetError};

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
}
