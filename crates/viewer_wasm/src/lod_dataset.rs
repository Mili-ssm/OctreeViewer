use std::{collections::HashSet, sync::Arc};
use thiserror::Error;
use viewer_core::{
    BlockCacheStats, BlockId, CacheInsertRejection, CacheInsertResult, CoordinateFrame,
    DEFAULT_LOD_PRIORITY_BYTES, DatasetMetadata, DecodedBlockCache, LodSelection,
    LodSelectionConfig, LodSelectionError, LodTransitionPlan, LodTransitionPlanner, NodeId,
    PointBlock, SpatialBounds, SpatialFrustum, SpatialHierarchy, SpatialNode,
};
use viewer_render_wgpu::{
    GpuRendererError, OrbitCamera, SurfaceRenderer, VoxelBuildMemoryStats, VoxelSceneBuildError,
    build_voxel_scene_with_limits,
};

/// Decoded blocks and resident spatial metadata for one open dataset.
pub struct LodDataset {
    metadata: Arc<DatasetMetadata>,
    visual_scale: f64,
    cache: DecodedBlockCache,
    planner: LodTransitionPlanner,
    visible_plan: Option<LodTransitionPlan>,
    last_successful_build: Option<SuccessfulBuildContext>,
    visible_scene_metrics: (f64, f64, usize),
    visible_voxel_memory: VoxelBuildMemoryStats,
    refinement_work: LodWorkStats,
    cache_cycle: Option<CacheCycleDetector>,
    /// Blocks inserted since the last `update_view*` (async arrivals) and what they evicted.
    /// The next update judges them against its fresh target, as a synchronous load would be.
    arrived_blocks: u64,
    arrival_evictions: Vec<BlockId>,
    /// Last selection and the inputs it depends on; reused while camera, viewport and stage match.
    selection_memo: Option<SelectionMemo>,
    /// Plan prepared by the residency preflight, reusable by the update that follows it.
    plan_memo: Option<PlanMemo>,
    /// Bumped on every cache or planner mutation; a plan is valid only for the epoch it saw.
    state_epoch: u64,
}

struct SelectionMemo {
    camera: OrbitCamera,
    viewport: [u32; 2],
    error_pixels: f64,
    selection: Arc<LodSelection>,
}

struct PlanMemo {
    selection: Arc<LodSelection>,
    epoch: u64,
    plan: LodTransitionPlan,
}

/// Brent's cycle detection with one exact, normalized LRU checkpoint.
/// Only automatic loads with an unchanged build context/cut feed this detector.
struct CacheCycleDetector {
    checkpoint: Vec<(BlockId, usize, bool)>,
    power: u64,
    steps: u64,
    paused: bool,
    selection_error_pixels: f64,
    target_voxel_pixels: f64,
}

impl CacheCycleDetector {
    fn new(
        cache: &DecodedBlockCache,
        selection_error_pixels: f64,
        target_voxel_pixels: f64,
    ) -> Self {
        Self {
            checkpoint: cache.lru_entries().collect(),
            power: 1,
            steps: 0,
            paused: false,
            selection_error_pixels,
            target_voxel_pixels,
        }
    }

    fn observe(&mut self, cache: &DecodedBlockCache) -> bool {
        self.steps = self.steps.saturating_add(1);
        if cache.lru_entries().eq(self.checkpoint.iter().copied()) {
            self.paused = true;
            return true;
        }
        if self.steps == self.power {
            self.checkpoint.clear();
            self.checkpoint.extend(cache.lru_entries());
            self.power = self.power.saturating_mul(2);
            self.steps = 0;
        }
        false
    }
}

/// Cumulative synchronous work while refining this dataset's visible cut.
/// Times include blocking read waits and exclude asynchronous GPU completion.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LodWorkStats {
    pub update_calls: u64,
    /// Hierarchy selections actually run (a memoized selection is not counted).
    pub selections: u64,
    /// `LodTransitionPlanner::prepare` calls actually run (a reused plan is not counted).
    pub prepares: u64,
    pub block_load_calls: u64,
    pub loaded_blocks: u64,
    /// Evicted by this load and immediately missing from the requested target cut.
    pub evicted_pending_blocks: u64,
    /// Times the cache-cycle detector paused refinement (synchronous and async loads alike).
    pub cache_cycle_detections: u64,
    pub scene_builds: u64,
    pub voxel_source_points: u64,
    pub selection_ms: f64,
    pub planning_ms: f64,
    pub block_load_ms: f64,
    pub voxel_build_ms: f64,
    pub upload_ms: f64,
    pub reused_scene_updates: u64,
    pub reused_source_points: u64,
    pub unchanged_cut_builds: u64,
    pub reusable_scene_builds: u64,
    pub reusable_voxel_build_ms: f64,
    pub reusable_voxel_source_points: u64,
    /// Rebuild attempts whose actual block traversal extends the previous traversal exactly.
    pub prefix_rebuilds: u64,
    pub prefix_retained_source_points: u64,
    pub prefix_added_source_points: u64,
    /// Full rebuild time for those attempts, not time saved by incremental accumulation.
    pub prefix_voxel_build_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
struct SuccessfulBuildContext {
    camera: OrbitCamera,
    viewport: [u32; 2],
    stage_voxel_pixels: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LodViewStats {
    pub visible_blocks: usize,
    pub visible_points: u64,
    pub rendered_voxels: usize,
    pub voxel_budget: usize,
    pub voxel_build_budget_bytes: u64,
    pub voxel_build_estimated_peak_bytes: u64,
    pub voxel_instance_capacity_bytes: u64,
    pub projection_cache_peak_entries: usize,
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
    pub refinement_work: LodWorkStats,
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
    #[error(
        "refinement paused after repeated decoded-cache state displaced target blocks; change the view or choose coarser detail"
    )]
    CacheCyclePaused,
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
            last_successful_build: None,
            visible_scene_metrics: (0.0, 0.0, 0),
            visible_voxel_memory: VoxelBuildMemoryStats::default(),
            refinement_work: LodWorkStats::default(),
            cache_cycle: None,
            arrived_blocks: 0,
            arrival_evictions: Vec::new(),
            selection_memo: None,
            plan_memo: None,
            state_epoch: 0,
        }
    }

    /// Selects the target cut for a view, or returns the previous selection when camera,
    /// viewport and stage error are identical (the hierarchy is immutable).
    fn resolve_target(
        &mut self,
        camera: &OrbitCamera,
        viewport: [u32; 2],
        error_pixels: f64,
        frustum: &SpatialFrustum,
        project: impl Fn(&SpatialNode) -> f64,
    ) -> Result<Arc<LodSelection>, LodSelectionError> {
        if let Some(memo) = &self.selection_memo
            && memo.camera == *camera
            && memo.viewport == viewport
            && memo.error_pixels == error_pixels
        {
            return Ok(Arc::clone(&memo.selection));
        }
        // Source points are LOD input, not rendered GPU instances. Bound the output scene later.
        let config = LodSelectionConfig::new(error_pixels, u64::MAX)?;
        self.refinement_work.selections += 1;
        let selection_started = WorkClock::start();
        let selection = self
            .metadata
            .hierarchy()
            .select_lod_stable_projector(frustum, config, DEFAULT_LOD_PRIORITY_BYTES, project)
            .map(|(selection, _priority_stats)| Arc::new(selection));
        self.refinement_work.selection_ms += selection_started.elapsed_ms();
        let selection = selection?;
        self.selection_memo = Some(SelectionMemo {
            camera: *camera,
            viewport,
            error_pixels,
            selection: Arc::clone(&selection),
        });
        Ok(selection)
    }

    /// Resolves a target against the cache, reusing the preflight's plan when nothing changed.
    fn resolve_plan(&mut self, target: &Arc<LodSelection>) -> LodTransitionPlan {
        if let Some(memo) = self.plan_memo.take()
            && memo.epoch == self.state_epoch
            && Arc::ptr_eq(&memo.selection, target)
        {
            return memo.plan;
        }
        self.prepare_plan(target)
    }

    /// Without a cache or planner change since `initial_plan`, preparing again is identical.
    fn replan_after_load(
        &mut self,
        initial_plan: LodTransitionPlan,
        initial_epoch: u64,
        target: &LodSelection,
    ) -> LodTransitionPlan {
        if self.state_epoch == initial_epoch {
            initial_plan
        } else {
            self.prepare_plan(target)
        }
    }

    fn prepare_plan(&mut self, target: &LodSelection) -> LodTransitionPlan {
        self.refinement_work.prepares += 1;
        let planning_started = WorkClock::start();
        let plan = self
            .planner
            .prepare(self.metadata.hierarchy(), target, &self.cache);
        self.refinement_work.planning_ms += planning_started.elapsed_ms();
        plan
    }

    /// Inserts an externally loaded (async) block. The cache-cycle detector is deliberately not
    /// reset here: a camera/target change resets it in `update_view*`, and the next update
    /// observes this arrival like a synchronous load.
    pub fn insert_block(&mut self, block: PointBlock) -> Result<usize, LodDatasetError> {
        let result = self.cache_block(block)?;
        self.arrived_blocks += 1;
        self.arrival_evictions
            .extend_from_slice(result.evicted_blocks());
        Ok(result.accounted_bytes())
    }

    /// Whether the detector has ever paused refinement for this dataset's loaded blocks.
    pub fn cache_cycle_detected(&self) -> bool {
        self.refinement_work.cache_cycle_detections > 0
    }

    /// Total blocks of the requested target cut evicted by loads or arrivals.
    pub fn pending_target_evictions(&self) -> u64 {
        self.refinement_work.evicted_pending_blocks
    }

    /// Feeds the detector after a load or async arrival; `Err` means refinement must pause.
    fn observe_cache_cycle(
        &mut self,
        loaded_block: bool,
        evicted_pending_blocks: u64,
        reusable_scene: bool,
        target_is_resident: bool,
        selection_error_pixels: f64,
        target_voxel_pixels: f64,
    ) -> Result<(), LodDatasetError> {
        if !reusable_scene || target_is_resident {
            self.cache_cycle = None;
        } else if loaded_block {
            if let Some(cycle) = self.cache_cycle.as_mut() {
                if cycle.observe(&self.cache) {
                    self.refinement_work.cache_cycle_detections += 1;
                    return Err(LodDatasetError::CacheCyclePaused);
                }
            } else if evicted_pending_blocks > 0 {
                self.cache_cycle = Some(CacheCycleDetector::new(
                    &self.cache,
                    selection_error_pixels,
                    target_voxel_pixels,
                ));
            }
        }
        Ok(())
    }

    fn cache_block(&mut self, block: PointBlock) -> Result<CacheInsertResult, LodDatasetError> {
        let id = block.id();
        if !self.metadata.hierarchy().references_block(id) {
            return Err(LodDatasetError::UnknownBlock(id));
        }
        self.state_epoch += 1;
        let result = self.cache.insert(block);
        if let Some(reason) = result.rejection() {
            return Err(LodDatasetError::CacheRejected(reason));
        }
        if self
            .visible_plan
            .as_ref()
            .is_some_and(|plan| plan.block_ids().contains(&id))
        {
            self.last_successful_build = None;
        }
        Ok(result)
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
        self.state_epoch += 1;
        self.planner.reset(&mut self.cache);
        self.cache.clear();
        self.visible_plan = None;
        self.last_successful_build = None;
        self.visible_scene_metrics = (0.0, 0.0, 0);
        self.visible_voxel_memory = VoxelBuildMemoryStats::default();
        self.refinement_work = LodWorkStats::default();
        self.cache_cycle = None;
        self.arrived_blocks = 0;
        self.arrival_evictions.clear();
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
            None,
            load_block,
        )
    }

    pub fn update_view_progressive(
        &mut self,
        renderer: &mut SurfaceRenderer<'_>,
        target_voxel_pixels: f64,
        selection_error_pixels: f64,
        stage_voxel_pixels: f64,
        async_priority_order: Option<&mut Vec<BlockId>>,
        mut load_block: impl FnMut(BlockId) -> Result<Option<PointBlock>, String>,
    ) -> Result<LodViewStats, LodDatasetError> {
        let build_context = SuccessfulBuildContext {
            camera: renderer.camera_snapshot(),
            viewport: renderer.viewport_size(),
            stage_voxel_pixels,
        };
        if self.last_successful_build.as_ref() != Some(&build_context)
            || self.cache_cycle.as_ref().is_some_and(|cycle| {
                cycle.selection_error_pixels != selection_error_pixels
                    || cycle.target_voxel_pixels != target_voxel_pixels
            })
        {
            self.cache_cycle = None;
        }
        if self.cache_cycle.as_ref().is_some_and(|cycle| cycle.paused) {
            return Err(LodDatasetError::CacheCyclePaused);
        }
        self.refinement_work.update_calls += 1;
        renderer.set_visual_scale(self.visual_scale)?;
        let frustum = renderer
            .spatial_frustum(self.metadata.coordinate_frame().origin())
            .ok_or(LodDatasetError::CameraUnavailable)?;
        let coordinate_origin = self.metadata.coordinate_frame().origin();
        let selection = self.resolve_target(
            &build_context.camera,
            build_context.viewport,
            selection_error_pixels,
            &frustum,
            |node| {
                // Camera and viewport remain immutable throughout this synchronous selection.
                renderer.projected_error_pixels(
                    node.bounds(),
                    bounds_side(node.bounds()) / 8.0,
                    coordinate_origin,
                )
            },
        );
        let target = match selection {
            Ok(target) => target,
            Err(error @ LodSelectionError::BaseCutExceedsBudget { .. }) => {
                renderer.sync_visible_blocks(&[], &self.cache)?;
                self.state_epoch += 1;
                self.planner.reset(&mut self.cache);
                self.visible_plan = None;
                self.last_successful_build = None;
                self.visible_scene_metrics = (0.0, 0.0, 0);
                self.visible_voxel_memory = VoxelBuildMemoryStats::default();
                return Err(LodDatasetError::Selection(error));
            }
            Err(error) => return Err(LodDatasetError::Selection(error)),
        };
        if !target.budget_limited().is_empty() {
            return Err(LodDatasetError::SourceDetailBudgetExceeded(
                target.budget_limited().len(),
            ));
        }
        let initial_plan = self.resolve_plan(&target);
        let planning_started = WorkClock::start();
        let initial_epoch = self.state_epoch;
        let next_block = if let Some(priority_order) = async_priority_order {
            let ordered = prioritized_pending_blocks(
                self.metadata.hierarchy(),
                initial_plan.pending_blocks(),
                &frustum,
                renderer,
                self.metadata.coordinate_frame().origin(),
            );
            priority_order.clear();
            priority_order.extend(ordered.iter().map(|(id, _)| *id));
            ordered.first().map(|(id, _)| *id)
        } else {
            highest_priority_pending_block(
                self.metadata.hierarchy(),
                initial_plan.pending_blocks(),
                &frustum,
                renderer,
                self.metadata.coordinate_frame().origin(),
            )
        };
        self.refinement_work.planning_ms += planning_started.elapsed_ms();
        let mut evicted_blocks = std::mem::take(&mut self.arrival_evictions);
        let mut loaded_block = std::mem::take(&mut self.arrived_blocks) > 0;
        if let Some(requested) = next_block {
            self.refinement_work.block_load_calls += 1;
            let load_started = WorkClock::start();
            let loaded = load_block(requested);
            self.refinement_work.block_load_ms += load_started.elapsed_ms();
            if let Some(block) = loaded.map_err(LodDatasetError::BlockRead)? {
                if block.id() != requested {
                    return Err(LodDatasetError::UnexpectedBlock {
                        requested,
                        actual: block.id(),
                    });
                }
                if let CacheInsertResult::Cached {
                    evicted_blocks: evicted,
                    ..
                } = self.cache_block(block)?
                {
                    evicted_blocks.extend(evicted);
                }
                self.refinement_work.loaded_blocks += 1;
                loaded_block = true;
            }
        }
        let plan = self.replan_after_load(initial_plan, initial_epoch, &target);
        let evicted_pending_blocks = evicted_blocks
            .iter()
            .filter(|id| plan.pending_blocks().binary_search(id).is_ok())
            .count() as u64;
        self.refinement_work.evicted_pending_blocks += evicted_pending_blocks;
        let local_frustum = renderer
            .spatial_frustum([0.0; 3])
            .ok_or(LodDatasetError::CameraUnavailable)?;
        if !plan.node_ids().is_empty() || plan.pending_blocks().is_empty() {
            let (same_visible_cut, reusable_scene) = reusable_build_match(
                self.visible_plan.as_ref().map(LodTransitionPlan::node_ids),
                self.visible_plan.as_ref().map(LodTransitionPlan::block_ids),
                plan.node_ids(),
                plan.block_ids(),
                self.last_successful_build.as_ref(),
                &build_context,
            );
            self.observe_cache_cycle(
                loaded_block,
                evicted_pending_blocks,
                reusable_scene,
                plan.pending_blocks().is_empty(),
                selection_error_pixels,
                target_voxel_pixels,
            )?;
            if reusable_scene {
                self.refinement_work.reused_scene_updates += 1;
                self.refinement_work.reused_source_points += plan.point_count();
            } else {
                self.refinement_work.scene_builds += 1;
                self.refinement_work.voxel_source_points += plan.point_count();
                if same_visible_cut {
                    self.refinement_work.unchanged_cut_builds += 1;
                }
                let prefix_points = prefix_extension_points(
                    self.metadata.hierarchy(),
                    &self.cache,
                    self.visible_plan.as_ref().map(LodTransitionPlan::node_ids),
                    plan.node_ids(),
                    self.last_successful_build.as_ref(),
                    &build_context,
                );
                let build_started = WorkClock::start();
                let voxel_scene_result = build_voxel_scene_with_limits(
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
                    renderer.voxel_build_limits(),
                );
                let build_elapsed_ms = build_started.elapsed_ms();
                self.refinement_work.voxel_build_ms += build_elapsed_ms;
                if let Some((retained, added)) = prefix_points {
                    self.refinement_work.prefix_rebuilds += 1;
                    self.refinement_work.prefix_retained_source_points += retained;
                    self.refinement_work.prefix_added_source_points += added;
                    self.refinement_work.prefix_voxel_build_ms += build_elapsed_ms;
                }
                let voxel_scene = voxel_scene_result?;
                let upload_started = WorkClock::start();
                let upload_result = renderer.upload_voxel_cut(&voxel_scene.scene, plan.block_ids());
                self.refinement_work.upload_ms += upload_started.elapsed_ms();
                upload_result?;
                self.visible_voxel_memory = voxel_scene.memory_stats;
                self.visible_scene_metrics = (
                    voxel_scene.min_edge_pixels,
                    voxel_scene.max_edge_pixels,
                    voxel_scene.oversized_voxels,
                );
            }
            // Even with identical geometry, publish new pending/coverage information and retain
            // the planner's cache protection. Only voxel construction and upload are omitted.
            self.state_epoch += 1;
            self.planner.commit(&plan, &mut self.cache);
            self.visible_plan = Some(plan.clone());
            self.last_successful_build = Some(build_context);
        }
        let visible_plan = self.visible_plan.as_ref().unwrap_or(&plan);
        let stats = LodViewStats {
            visible_blocks: visible_plan.block_ids().len(),
            visible_points: visible_plan.point_count(),
            rendered_voxels: renderer.visible_instance_count(),
            voxel_budget: renderer.voxel_build_limits().max_voxels,
            voxel_build_budget_bytes: renderer.voxel_build_limits().max_estimated_bytes,
            voxel_build_estimated_peak_bytes: self.visible_voxel_memory.estimated_peak_bytes,
            voxel_instance_capacity_bytes: self.visible_voxel_memory.instance_capacity_bytes,
            projection_cache_peak_entries: self.visible_voxel_memory.projection_cache_peak_entries,
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
            refinement_work: self.refinement_work.clone(),
        };
        Ok(stats)
    }

    /// Checks whether the target cut can be published using only blocks already in the cache.
    /// This preflight does not build a scene or change cache protection.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn target_cut_is_resident(
        &mut self,
        renderer: &mut SurfaceRenderer<'_>,
        target_voxel_pixels: f64,
    ) -> Result<bool, LodDatasetError> {
        renderer.set_visual_scale(self.visual_scale)?;
        let camera = renderer.camera_snapshot();
        let viewport = renderer.viewport_size();
        let coordinate_origin = self.metadata.coordinate_frame().origin();
        let frustum = renderer
            .spatial_frustum(coordinate_origin)
            .ok_or(LodDatasetError::CameraUnavailable)?;
        let target =
            self.resolve_target(&camera, viewport, target_voxel_pixels, &frustum, |node| {
                renderer.projected_error_pixels(
                    node.bounds(),
                    bounds_side(node.bounds()) / 8.0,
                    coordinate_origin,
                )
            })?;
        if !target.budget_limited().is_empty() {
            return Ok(false);
        }
        Ok(self.preflight_resident(target))
    }

    /// Whether `target` is fully resident. The update that follows at this stage reuses the
    /// prepared plan if the cache and planner are untouched in between.
    #[cfg(any(target_arch = "wasm32", test))]
    fn preflight_resident(&mut self, target: Arc<LodSelection>) -> bool {
        let plan = self.prepare_plan(&target);
        let complete = resident_target_plan_is_complete(&target, &plan);
        self.plan_memo = Some(PlanMemo {
            selection: target,
            epoch: self.state_epoch,
            plan,
        });
        complete
    }

    /// Current cache-reset-scoped work counters, including a failed scene update.
    pub fn refinement_work(&self) -> &LodWorkStats {
        &self.refinement_work
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn attribute_schema(&self) -> &[viewer_core::AttributeDescriptor] {
        self.metadata.hierarchy().attributes()
    }

    /// Dataset-lifetime activity; clearing payloads preserves the cache's counters.
    pub fn cache_stats(&self) -> BlockCacheStats {
        self.cache.stats()
    }

    pub fn cache_bytes(&self) -> usize {
        self.cache.used_bytes()
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct WorkClock(std::time::Instant);

#[cfg(not(target_arch = "wasm32"))]
impl WorkClock {
    pub(crate) fn start() -> Self {
        Self(std::time::Instant::now())
    }

    pub(crate) fn elapsed_ms(self) -> f64 {
        self.0.elapsed().as_secs_f64() * 1000.0
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) enum WorkClock {
    Performance(web_sys::Performance, f64),
    Date(f64),
}

#[cfg(target_arch = "wasm32")]
impl WorkClock {
    pub(crate) fn start() -> Self {
        use wasm_bindgen::JsCast;

        let global = js_sys::global();
        let performance = js_sys::Reflect::get(&global, &"performance".into())
            .ok()
            .and_then(|value| value.dyn_into::<web_sys::Performance>().ok());
        performance.map_or_else(
            || Self::Date(js_sys::Date::now()),
            |performance| Self::Performance(performance.clone(), performance.now()),
        )
    }

    pub(crate) fn elapsed_ms(self) -> f64 {
        match self {
            Self::Performance(performance, started) => (performance.now() - started).max(0.0),
            Self::Date(started) => (js_sys::Date::now() - started).max(0.0),
        }
    }
}

fn bounds_side(bounds: SpatialBounds) -> f64 {
    (0..3)
        .map(|axis| bounds.max()[axis] - bounds.min()[axis])
        .fold(0.0, f64::max)
}

fn reusable_build_match(
    visible_node_ids: Option<&[NodeId]>,
    visible_block_ids: Option<&[BlockId]>,
    next_node_ids: &[NodeId],
    next_block_ids: &[BlockId],
    previous_context: Option<&SuccessfulBuildContext>,
    next_context: &SuccessfulBuildContext,
) -> (bool, bool) {
    let same_cut =
        visible_node_ids == Some(next_node_ids) && visible_block_ids == Some(next_block_ids);
    let same_inputs = previous_context.is_some_and(|previous| {
        previous.camera == next_context.camera
            && previous.viewport == next_context.viewport
            && previous.stage_voxel_pixels == next_context.stage_voxel_pixels
    });
    (same_cut, same_cut && same_inputs)
}

fn prefix_extension_points(
    hierarchy: &SpatialHierarchy,
    cache: &DecodedBlockCache,
    previous_nodes: Option<&[NodeId]>,
    next_nodes: &[NodeId],
    previous_context: Option<&SuccessfulBuildContext>,
    next_context: &SuccessfulBuildContext,
) -> Option<(u64, u64)> {
    if previous_context != Some(next_context) {
        return None;
    }
    // Match the builder's traversal, including shared references, not sorted unique block IDs.
    fn blocks<'a>(
        hierarchy: &'a SpatialHierarchy,
        cache: &'a DecodedBlockCache,
        nodes: &'a [NodeId],
    ) -> impl Iterator<Item = (BlockId, u64)> + 'a {
        nodes
            .iter()
            .filter_map(|id| hierarchy.node(*id))
            .flat_map(|node| node.blocks())
            .filter_map(|id| {
                cache
                    .peek(*id)
                    .map(|block| (*id, u64::try_from(block.points().len()).unwrap_or(u64::MAX)))
            })
    }
    let mut next = blocks(hierarchy, cache, next_nodes);
    let mut previous = blocks(hierarchy, cache, previous_nodes?).peekable();
    previous.peek()?; // No retained work to save for an empty old scene.
    let mut retained = 0_u64;
    for (id, points) in previous {
        if next.next().map(|(next_id, _)| next_id) != Some(id) {
            return None;
        }
        retained = retained.saturating_add(points);
    }
    let (_, first_added) = next.next()?; // Identical scenes already have a separate reuse path.
    let added = next.fold(first_added, |sum, (_, points)| sum.saturating_add(points));
    Some((retained, added))
}

fn max_visible_level(hierarchy: &SpatialHierarchy, node_ids: &[NodeId]) -> Option<u32> {
    node_ids
        .iter()
        .filter_map(|id| hierarchy.node(*id).map(|node| node.level()))
        .max()
}

#[cfg(any(target_arch = "wasm32", test))]
fn resident_target_plan_is_complete(
    target: &viewer_core::LodSelection,
    plan: &LodTransitionPlan,
) -> bool {
    target.budget_limited().is_empty()
        && plan.pending_blocks().is_empty()
        && plan.uncovered_nodes().is_empty()
        && plan.budget_omitted_nodes().is_empty()
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

fn prioritized_pending_blocks(
    hierarchy: &SpatialHierarchy,
    pending: &[BlockId],
    frustum: &SpatialFrustum,
    renderer: &SurfaceRenderer<'_>,
    coordinate_origin: [f64; 3],
) -> Vec<(BlockId, f64)> {
    if pending.is_empty() {
        return Vec::new();
    }
    let pending_ids = pending.iter().copied().collect::<HashSet<_>>();
    let mut priorities = std::collections::HashMap::<BlockId, f64>::new();
    for node in hierarchy.query_frustum(frustum.clone()) {
        let priority = renderer.projected_error_pixels(
            node.bounds(),
            node.geometric_error(),
            coordinate_origin,
        );
        for block_id in node.blocks() {
            if pending_ids.contains(block_id) {
                priorities
                    .entry(*block_id)
                    .and_modify(|current| {
                        if priority.total_cmp(current).is_gt() {
                            *current = priority;
                        }
                    })
                    .or_insert(priority);
            }
        }
    }
    let mut ordered = priorities.into_iter().collect::<Vec<_>>();
    ordered.sort_unstable_by(|candidate, current| {
        if candidate.1.total_cmp(&current.1).is_eq() {
            candidate.0.cmp(&current.0)
        } else {
            current.1.total_cmp(&candidate.1)
        }
    });
    ordered
}

fn priority_precedes(candidate: (BlockId, f64), current: (BlockId, f64)) -> bool {
    candidate.1.total_cmp(&current.1).is_gt()
        || (candidate.1.total_cmp(&current.1).is_eq() && candidate.0 < current.0)
}

#[cfg(test)]
#[path = "lod_dataset_tests.rs"]
mod tests;
