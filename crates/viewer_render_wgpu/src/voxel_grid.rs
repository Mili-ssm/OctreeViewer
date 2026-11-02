use std::collections::{BTreeMap, BTreeSet, VecDeque, btree_map::Entry};

use viewer_core::{NodeId, PointBlock, ResidentBlockSource, SpatialBounds, SpatialHierarchy};

use crate::{CellGrid, InstanceRaw, RenderScene, attributes::ScalarAttribute};

/// A global dyadic cell identity; source representatives can bound its finest resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct VoxelKey {
    depth: u8,
    coord: [u32; 3],
}

#[derive(Clone, Copy)]
struct LocalCutBounds {
    min: [f64; 3],
    max: [f64; 3],
    level: u32,
}

impl LocalCutBounds {
    fn contains(self, point: [f64; 3], root_max: [f64; 3]) -> bool {
        (0..3).all(|axis| {
            point[axis] >= self.min[axis]
                && (point[axis] < self.max[axis]
                    || (self.max[axis] == root_max[axis] && point[axis] <= self.max[axis]))
        })
    }
}

struct CutAccumulation<'a> {
    excluded: &'a [LocalCutBounds],
    root_max: [f64; 3],
    min_depth: u8,
    source_depth_limit: Option<u8>,
}

#[derive(Clone, Copy)]
struct AccumulationBudget {
    limits: VoxelBuildLimits,
    working_reservation_bytes: u64,
    accumulator_charge_bytes: u64,
}

const MAX_PROJECTED_EDGE_CACHE_ENTRIES: usize = 262_144;
const VOXEL_INSTANCE_SIZE: u64 = std::mem::size_of::<InstanceRaw>() as u64;
// Approximate tree entry, bounds vector, and allocator slack for mixed Replacement cuts.
const CUT_EXCLUSION_CHARGE_BYTES: u64 = 128;
const CUT_NODE_CHARGE_BYTES: u64 = 64;

/// Limits for voxel accumulation and its estimated temporary memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelBuildLimits {
    /// Maximum number of distinct renderable voxels retained by the builder.
    pub max_voxels: usize,
    /// Maximum estimated bytes for accumulation, projection/region reservation, and instances.
    /// This conservative model is not an RSS limit or a guarantee about allocator overhead.
    pub max_estimated_bytes: u64,
}

impl Default for VoxelBuildLimits {
    fn default() -> Self {
        Self {
            max_voxels: crate::DEFAULT_MAX_VISIBLE_VOXELS,
            max_estimated_bytes: 256 * 1024 * 1024,
        }
    }
}

/// Conservative builder memory estimates; these values do not measure process RSS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoxelBuildMemoryStats {
    /// Peak from accumulator charges plus the larger of working reservation or instances.
    pub estimated_peak_bytes: u64,
    /// Allocated instance-vector capacity at finish, in bytes.
    pub instance_capacity_bytes: u64,
    /// Maximum number of projected edge entries simultaneously retained.
    pub projection_cache_peak_entries: usize,
}

struct AdaptiveVoxelGrid {
    origin: [f64; 3],
    side: f64,
    target_pixels: f64,
    max_depth: u8,
    // Sorted source regions whose representative cells must not be refined, grouped by log2 of
    // the representatives per axis (3 for the fixed 8, more for a stored `v7.4-pre` spacing).
    representative_regions: Vec<(u8, Vec<VoxelKey>)>,
    projected_edge_cache_limit: usize,
    projected_edges: BTreeMap<VoxelKey, f64>,
    projected_edge_order: VecDeque<VoxelKey>,
    projected_edge_peak_entries: usize,
    last_resolved_cell: Option<(VoxelKey, bool, u8, u8)>,
}

impl AdaptiveVoxelGrid {
    fn bounds(&self, key: VoxelKey) -> SpatialBounds {
        let side = self.side / 2.0_f64.powi(i32::from(key.depth));
        let min = std::array::from_fn(|axis| self.origin[axis] + f64::from(key.coord[axis]) * side);
        let max = std::array::from_fn(|axis| {
            self.origin[axis] + (f64::from(key.coord[axis]) + 1.0) * side
        });
        SpatialBounds::new(min, max).expect("dyadic cells have ordered finite bounds")
    }

    fn projected_edge(
        &mut self,
        key: VoxelKey,
        project: &dyn Fn(SpatialBounds) -> f64,
    ) -> Result<f64, VoxelSceneBuildError> {
        if let Some(edge) = self.projected_edges.get(&key) {
            return Ok(*edge);
        }
        let edge = project(self.bounds(key));
        if self.projected_edge_cache_limit > 0 {
            if self.projected_edges.len() >= self.projected_edge_cache_limit
                && let Some(expired) = self.projected_edge_order.pop_front()
            {
                self.projected_edges.remove(&expired);
            }
            self.projected_edge_order.try_reserve(1).map_err(|_| {
                VoxelSceneBuildError::AllocationFailed {
                    requested_bytes: (self.projected_edge_order.len() + 1)
                        .saturating_mul(std::mem::size_of::<VoxelKey>())
                        as u64,
                }
            })?;
            self.projected_edges.insert(key, edge);
            self.projected_edge_order.push_back(key);
            self.projected_edge_peak_entries = self
                .projected_edge_peak_entries
                .max(self.projected_edges.len());
        }
        Ok(edge)
    }

    #[cfg(test)]
    fn key_for_point(
        &mut self,
        point: [f64; 3],
        project: &dyn Fn(SpatialBounds) -> f64,
    ) -> Result<Option<VoxelKey>, VoxelSceneBuildError> {
        self.key_for_point_with_depths(point, project, 0, self.max_depth)
    }

    fn key_for_point_with_depths(
        &mut self,
        point: [f64; 3],
        project: &dyn Fn(SpatialBounds) -> f64,
        min_depth: u8,
        max_depth: u8,
    ) -> Result<Option<VoxelKey>, VoxelSceneBuildError> {
        // Nearby source points often share a terminal cell. Match the exact integer mapping
        // used by descent, including clamping outside the root, before reusing its result.
        if let Some((key, visible, cached_min, cached_max)) = self.last_resolved_cell
            && cached_min == min_depth
            && cached_max == max_depth
            && self.key_at_depth(point, key.depth) == key
        {
            return Ok(visible.then_some(key));
        }
        let mut key = VoxelKey {
            depth: 0,
            coord: [0; 3],
        };
        loop {
            let edge = self.projected_edge(key, project)?;
            // Cull cells as soon as their bounds cannot contribute to the view. Deferring this
            // until scene construction would retain one accumulator per off-screen voxel.
            if edge <= 0.0 {
                self.last_resolved_cell = Some((key, false, min_depth, max_depth));
                return Ok(None);
            }
            // Beyond 24 levels f32 cannot reliably distinguish adjacent local cell faces.
            let representative_cell = self.representative_regions.iter().any(|(shift, regions)| {
                key.depth >= *shift
                    && regions
                        .binary_search(&VoxelKey {
                            depth: key.depth - shift,
                            coord: key.coord.map(|coord| coord >> shift),
                        })
                        .is_ok()
            });
            if (edge <= self.target_pixels && key.depth >= min_depth)
                || key.depth >= max_depth
                || (representative_cell && key.depth >= min_depth)
            {
                self.last_resolved_cell = Some((key, true, min_depth, max_depth));
                return Ok(Some(key));
            }
            key = self.key_at_depth(point, key.depth + 1);
        }
    }

    fn key_at_depth(&self, point: [f64; 3], depth: u8) -> VoxelKey {
        let count = 1_u32 << depth;
        VoxelKey {
            depth,
            coord: std::array::from_fn(|axis| {
                (((point[axis] - self.origin[axis]) / self.side * f64::from(count))
                    .floor()
                    .clamp(0.0, f64::from(count - 1))) as u32
            }),
        }
    }

    fn render_cell(&self, key: VoxelKey) -> Option<([f32; 3], f32)> {
        let bounds = self.bounds(key);
        let cell_size = self.side / 2.0_f64.powi(i32::from(key.depth));
        let center_exact = std::array::from_fn(|axis| bounds.min()[axis] + cell_size * 0.5);
        safe_render_cell_size(center_exact, cell_size)
    }
}

#[derive(Debug)]
pub struct VoxelScene {
    pub scene: RenderScene,
    pub min_edge_pixels: f64,
    pub max_edge_pixels: f64,
    pub oversized_voxels: usize,
    pub projected_edge_cache_entries: usize,
    /// Conservative estimates only; allocator overhead and process RSS are not measured.
    pub memory_stats: VoxelBuildMemoryStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoxelSceneBuildError {
    InstanceLimitExceeded { limit: usize },
    EstimatedMemoryLimitExceeded { required_bytes: u64, max_bytes: u64 },
    SizeOverflow,
    AllocationFailed { requested_bytes: u64 },
    MixedCutNotAligned,
}

impl std::fmt::Display for VoxelSceneBuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InstanceLimitExceeded { limit } => {
                write!(
                    formatter,
                    "voxelized scene exceeds the {limit}-voxel instance limit"
                )
            }
            Self::EstimatedMemoryLimitExceeded {
                required_bytes,
                max_bytes,
            } => write!(
                formatter,
                "voxel scene estimated memory {required_bytes} exceeds the {max_bytes}-byte limit"
            ),
            Self::SizeOverflow => write!(formatter, "voxel scene size calculation overflowed"),
            Self::AllocationFailed { requested_bytes } => write!(
                formatter,
                "failed to reserve {requested_bytes} bytes for voxel scene"
            ),
            Self::MixedCutNotAligned => write!(
                formatter,
                "mixed Replacement cut exceeds or does not align with the parent representative grid"
            ),
        }
    }
}

impl std::error::Error for VoxelSceneBuildError {}

#[derive(Default)]
struct VoxelAccumulator {
    rgb: [u64; 3],
    count: u64,
    intensity_sum: f64,
    intensity_count: u64,
    classification: Option<f32>,
}

/// Incremental voxel accumulator for a single spatial cut.
///
/// Call [`Self::add_block`] as source blocks arrive, then [`Self::finish`] to produce one scene.
/// The source blocks can be dropped immediately after each call; only occupied voxels and a
/// bounded projection memo are retained. [`Self::new`] preserves the legacy CPU-only voxel limit;
/// pair [`Self::with_limits`] with [`Self::try_finish`] to enforce estimated memory and return
/// build errors.
pub struct VoxelSceneBuilder {
    grid: AdaptiveVoxelGrid,
    voxels: BTreeMap<VoxelKey, VoxelAccumulator>,
    limits: VoxelBuildLimits,
    working_reservation_bytes: u64,
    accumulator_charge_bytes: u64,
    error: Option<VoxelSceneBuildError>,
}

impl VoxelSceneBuilder {
    /// Creates a builder with a voxel-count limit and no estimated-memory limit.
    pub fn new(
        root: SpatialBounds,
        coordinate_origin: [f64; 3],
        target_pixels: f64,
        max_voxels: usize,
    ) -> Self {
        Self::with_limits(
            root,
            coordinate_origin,
            target_pixels,
            VoxelBuildLimits {
                max_voxels,
                max_estimated_bytes: u64::MAX,
            },
        )
    }

    /// Creates a builder with CPU and estimated-memory limits.
    ///
    /// Per-voxel charges use twice the `(VoxelKey, VoxelAccumulator)` size plus six pointer-sized
    /// words. Projection entries use twice `(VoxelKey, f64)`, six pointer-sized words, and four
    /// `VoxelKey`s for FIFO capacity allowance. These charges are conservative estimates, not RSS.
    pub fn with_limits(
        root: SpatialBounds,
        coordinate_origin: [f64; 3],
        target_pixels: f64,
        limits: VoxelBuildLimits,
    ) -> Self {
        let accumulator_charge_bytes = accumulator_charge_bytes();
        let projection_charge = projection_entry_charge_bytes();
        let cache_limit = usize::try_from(
            (limits.max_estimated_bytes / 8 / projection_charge)
                .min(MAX_PROJECTED_EDGE_CACHE_ENTRIES as u64),
        )
        .expect("projected cache limit is bounded by its entry cap");
        let working_reservation_bytes = u64::try_from(cache_limit)
            .expect("projected cache limit fits in u64")
            .checked_mul(projection_charge)
            .expect("bounded projected cache charge fits in u64");
        Self {
            grid: AdaptiveVoxelGrid {
                // Use local coordinates before subdivision to retain fine cells at geographic origins.
                origin: std::array::from_fn(|axis| root.min()[axis] - coordinate_origin[axis]),
                side: bounds_side(root).max(1.0e-5),
                target_pixels,
                max_depth: 24,
                representative_regions: Vec::new(),
                projected_edge_cache_limit: cache_limit,
                projected_edges: BTreeMap::new(),
                projected_edge_order: VecDeque::new(),
                projected_edge_peak_entries: 0,
                last_resolved_cell: None,
            },
            voxels: BTreeMap::new(),
            limits,
            working_reservation_bytes,
            accumulator_charge_bytes,
            error: None,
        }
    }

    pub fn add_block(
        &mut self,
        block: &PointBlock,
        project: &dyn Fn(SpatialBounds) -> f64,
    ) -> Result<(), VoxelSceneBuildError> {
        self.add_block_for_cut(block, project, &[], [f64::INFINITY; 3], 0, None)
    }

    fn add_block_for_cut(
        &mut self,
        block: &PointBlock,
        project: &dyn Fn(SpatialBounds) -> f64,
        excluded: &[LocalCutBounds],
        root_max: [f64; 3],
        min_depth: u8,
        source_depth_limit: Option<u8>,
    ) -> Result<(), VoxelSceneBuildError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let result = accumulate_block(
            block,
            &mut self.grid,
            &mut self.voxels,
            AccumulationBudget {
                limits: self.limits,
                working_reservation_bytes: self.working_reservation_bytes,
                accumulator_charge_bytes: self.accumulator_charge_bytes,
            },
            project,
            CutAccumulation {
                excluded,
                root_max,
                min_depth,
                source_depth_limit,
            },
        );
        if let Err(error) = result {
            self.error = Some(error);
            return Err(error);
        }
        Ok(())
    }

    /// Panicking convenience wrapper for [`Self::try_finish`].
    ///
    /// Use [`Self::try_finish`] when build failures must be returned to the caller.
    pub fn finish(self, project: &dyn Fn(SpatialBounds) -> f64) -> VoxelScene {
        self.try_finish(project).expect("voxel scene finish failed")
    }

    /// Finishes without publishing a partial scene after any earlier build error.
    ///
    /// `project` must return stable results for equal bounds throughout accumulation and finish.
    pub fn try_finish(
        self,
        project: &dyn Fn(SpatialBounds) -> f64,
    ) -> Result<VoxelScene, VoxelSceneBuildError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let Self {
            mut grid,
            voxels,
            limits,
            working_reservation_bytes,
            accumulator_charge_bytes,
            error: _,
        } = self;
        let projected_edge_cache_entries = grid.projected_edges.len();
        let projection_cache_peak_entries = grid.projected_edge_peak_entries;
        grid.projected_edges = BTreeMap::new();
        grid.projected_edge_order = VecDeque::new();
        grid.representative_regions = Vec::new();

        let count = voxels.len();
        let requested_instance_bytes = instance_allocation_bytes(count)?;
        let accumulator_bytes = checked_charge(count, accumulator_charge_bytes)?;
        let requested_peak_bytes = accumulator_bytes
            .checked_add(working_reservation_bytes.max(requested_instance_bytes))
            .ok_or(VoxelSceneBuildError::SizeOverflow)?;
        check_memory_limit(requested_peak_bytes, limits.max_estimated_bytes)?;
        let mut instances = reserve_instances(count)?;
        let instance_capacity_bytes = u64::try_from(instances.capacity())
            .ok()
            .and_then(|capacity| capacity.checked_mul(VOXEL_INSTANCE_SIZE))
            .ok_or(VoxelSceneBuildError::SizeOverflow)?;
        let estimated_peak_bytes = accumulator_bytes
            .checked_add(instance_capacity_bytes)
            .ok_or(VoxelSceneBuildError::SizeOverflow)?
            .max(
                accumulator_bytes
                    .checked_add(working_reservation_bytes)
                    .ok_or(VoxelSceneBuildError::SizeOverflow)?,
            );
        check_memory_limit(estimated_peak_bytes, limits.max_estimated_bytes)?;
        let mut min_edge_pixels = f64::INFINITY;
        let mut max_edge_pixels: f64 = 0.0;
        let mut oversized_voxels = 0;
        for (key, accumulator) in voxels {
            let edge = project(grid.bounds(key));
            if edge <= 0.0 {
                continue;
            }
            let Some((center, size)) = grid.render_cell(key) else {
                continue;
            };
            min_edge_pixels = min_edge_pixels.min(edge);
            max_edge_pixels = max_edge_pixels.max(edge);
            oversized_voxels += usize::from(edge > grid.target_pixels * (1.0 + 1.0e-6));
            let color = accumulator
                .rgb
                .map(|sum| ((sum + accumulator.count / 2) / accumulator.count).min(255) as u8);
            instances.push(InstanceRaw {
                center,
                size,
                color: viewer_core::ColorRgb::new(color[0], color[1], color[2]).to_linear_rgba(),
                intensity: accumulator.average_intensity(),
                classification: accumulator.classification.unwrap_or(-1.0),
            });
        }
        Ok(VoxelScene {
            scene: RenderScene::from_instances(instances),
            min_edge_pixels: if min_edge_pixels.is_finite() {
                min_edge_pixels
            } else {
                0.0
            },
            max_edge_pixels,
            oversized_voxels,
            projected_edge_cache_entries,
            memory_stats: VoxelBuildMemoryStats {
                estimated_peak_bytes,
                instance_capacity_bytes,
                projection_cache_peak_entries,
            },
        })
    }
}

/// Builds a voxel scene with the legacy CPU-only limit and no estimated-memory limit.
pub fn build_voxel_scene(
    hierarchy: &SpatialHierarchy,
    node_ids: &[NodeId],
    cache: &dyn ResidentBlockSource,
    coordinate_origin: [f64; 3],
    target_pixels: f64,
    project: &dyn Fn(SpatialBounds) -> f64,
) -> VoxelScene {
    build_voxel_scene_with_limit(
        hierarchy,
        node_ids,
        cache,
        coordinate_origin,
        target_pixels,
        project,
        usize::MAX,
    )
    .expect("voxel scene build failed")
}

/// Builds with a voxel-count limit and no estimated-memory limit.
pub fn build_voxel_scene_with_limit(
    hierarchy: &SpatialHierarchy,
    node_ids: &[NodeId],
    cache: &dyn ResidentBlockSource,
    coordinate_origin: [f64; 3],
    target_pixels: f64,
    project: &dyn Fn(SpatialBounds) -> f64,
    max_voxels: usize,
) -> Result<VoxelScene, VoxelSceneBuildError> {
    build_voxel_scene_with_limits(
        hierarchy,
        node_ids,
        cache,
        coordinate_origin,
        target_pixels,
        project,
        VoxelBuildLimits {
            max_voxels,
            max_estimated_bytes: u64::MAX,
        },
    )
}

/// Builds with voxel-count and conservative estimated-memory limits.
///
/// Replacement representatives cap their occupied regions at their source resolution; all
/// blocks follow the same terminal rule. Unusually dispersed external payloads use a global cap.
/// `project` must return stable results for equal bounds throughout scene construction.
pub fn build_voxel_scene_with_limits(
    hierarchy: &SpatialHierarchy,
    node_ids: &[NodeId],
    cache: &dyn ResidentBlockSource,
    coordinate_origin: [f64; 3],
    target_pixels: f64,
    project: &dyn Fn(SpatialBounds) -> f64,
    limits: VoxelBuildLimits,
) -> Result<VoxelScene, VoxelSceneBuildError> {
    let selected =
        (hierarchy.semantics() == viewer_core::HierarchySemantics::Replacement).then(|| {
            node_ids
                .iter()
                .copied()
                .filter(|id| {
                    hierarchy.node(*id).is_some_and(|node| {
                        !node.blocks().is_empty()
                            && node
                                .blocks()
                                .iter()
                                .all(|block_id| cache.contains(*block_id))
                    })
                })
                .collect::<BTreeSet<_>>()
        });
    let exclusions = selected.as_ref().map_or_else(BTreeMap::new, |selected| {
        replacement_cut_exclusions(hierarchy, selected, coordinate_origin)
    });
    let mut builder = VoxelSceneBuilder::with_limits(
        hierarchy.stats().root_bounds,
        coordinate_origin,
        target_pixels,
        limits,
    );
    validate_mixed_cut(&builder.grid, hierarchy, &exclusions, coordinate_origin)?;
    let excluded_regions = exclusions.values().try_fold(0_usize, |sum, bounds| {
        sum.checked_add(bounds.len())
            .ok_or(VoxelSceneBuildError::SizeOverflow)
    })?;
    let cut_charge = checked_charge(excluded_regions, CUT_EXCLUSION_CHARGE_BYTES)?
        .checked_add(checked_charge(exclusions.len(), CUT_NODE_CHARGE_BYTES)?)
        .ok_or(VoxelSceneBuildError::SizeOverflow)?;
    builder.working_reservation_bytes = builder
        .working_reservation_bytes
        .checked_add(cut_charge)
        .ok_or(VoxelSceneBuildError::SizeOverflow)?;
    check_memory_limit(
        builder.working_reservation_bytes,
        limits.max_estimated_bytes,
    )?;
    configure_representative_regions(&mut builder, hierarchy, node_ids, cache, &exclusions)?;
    let root_max = std::array::from_fn(|axis| {
        hierarchy.stats().root_bounds.max()[axis] - coordinate_origin[axis]
    });
    for node_id in node_ids {
        let Some(node) = hierarchy.node(*node_id) else {
            continue;
        };
        let excluded = exclusions.get(node_id).map_or(&[][..], Vec::as_slice);
        let selected_ancestor = selected
            .as_ref()
            .is_some_and(|selected| has_selected_ancestor(hierarchy, *node_id, selected));
        let min_depth = excluded
            .iter()
            .map(|bounds| bounds.level)
            .max()
            .unwrap_or(0)
            .max(if selected_ancestor { node.level() } else { 0 })
            .min(24) as u8;
        if min_depth > builder.grid.max_depth {
            return Err(VoxelSceneBuildError::MixedCutNotAligned);
        }
        let source_depth_limit =
            (!excluded.is_empty()).then(|| representative_depth(&builder.grid, node));
        for block_id in node.blocks() {
            let Some(block) = cache.peek(*block_id) else {
                continue;
            };
            builder.add_block_for_cut(
                block,
                project,
                excluded,
                root_max,
                min_depth,
                source_depth_limit,
            )?;
        }
    }
    builder.try_finish(project)
}

fn validate_mixed_cut(
    grid: &AdaptiveVoxelGrid,
    hierarchy: &SpatialHierarchy,
    exclusions: &BTreeMap<NodeId, Vec<LocalCutBounds>>,
    coordinate_origin: [f64; 3],
) -> Result<(), VoxelSceneBuildError> {
    for (parent_id, children) in exclusions {
        let Some(parent) = hierarchy.node(*parent_id) else {
            continue;
        };
        let depth = representative_depth(grid, parent);
        let parent_min: [f64; 3] =
            std::array::from_fn(|axis| parent.bounds().min()[axis] - coordinate_origin[axis]);
        let parent_max: [f64; 3] =
            std::array::from_fn(|axis| parent.bounds().max()[axis] - coordinate_origin[axis]);
        for child in children {
            if child.level > u32::from(depth) {
                return Err(VoxelSceneBuildError::MixedCutNotAligned);
            }
            let scale =
                2.0_f64.powi(i32::try_from(child.level).expect("depth is at most 24")) / grid.side;
            for axis in 0..3 {
                for edge in [child.min[axis], child.max[axis]] {
                    if edge <= parent_min[axis] || edge >= parent_max[axis] {
                        continue;
                    }
                    let cell = (edge - grid.origin[axis]) * scale;
                    if !cell.is_finite() || (cell - cell.round()).abs() > 1.0e-6 {
                        return Err(VoxelSceneBuildError::MixedCutNotAligned);
                    }
                }
            }
        }
    }
    Ok(())
}

fn replacement_cut_exclusions(
    hierarchy: &SpatialHierarchy,
    selected: &BTreeSet<NodeId>,
    coordinate_origin: [f64; 3],
) -> BTreeMap<NodeId, Vec<LocalCutBounds>> {
    let mut result = BTreeMap::<NodeId, Vec<LocalCutBounds>>::new();
    for node_id in selected {
        let Some(node) = hierarchy.node(*node_id) else {
            continue;
        };
        let mut parent = node.parent();
        while let Some(parent_id) = parent {
            if selected.contains(&parent_id) {
                result.entry(parent_id).or_default().push(LocalCutBounds {
                    min: std::array::from_fn(|axis| {
                        node.bounds().min()[axis] - coordinate_origin[axis]
                    }),
                    max: std::array::from_fn(|axis| {
                        node.bounds().max()[axis] - coordinate_origin[axis]
                    }),
                    level: node.level(),
                });
                break;
            }
            parent = hierarchy
                .node(parent_id)
                .and_then(|ancestor| ancestor.parent());
        }
    }
    result
}

fn has_selected_ancestor(
    hierarchy: &SpatialHierarchy,
    node_id: NodeId,
    selected: &BTreeSet<NodeId>,
) -> bool {
    let mut parent = hierarchy.node(node_id).and_then(|node| node.parent());
    while let Some(id) = parent {
        if selected.contains(&id) {
            return true;
        }
        parent = hierarchy.node(id).and_then(|node| node.parent());
    }
    false
}

/// Configure one immutable terminal rule for all points, using occupied coarse regions only.
/// A region may also conservatively limit a neighbor; it never generates new occupied cells.
fn configure_representative_regions(
    builder: &mut VoxelSceneBuilder,
    hierarchy: &SpatialHierarchy,
    node_ids: &[NodeId],
    cache: &dyn ResidentBlockSource,
    exclusions: &BTreeMap<NodeId, Vec<LocalCutBounds>>,
) -> Result<(), VoxelSceneBuildError> {
    if hierarchy.semantics() != viewer_core::HierarchySemantics::Replacement {
        return Ok(());
    }
    let coarse_nodes = || {
        node_ids
            .iter()
            .filter_map(|id| hierarchy.node(*id))
            .filter(|node| {
                !hierarchy.is_leaf(node.id())
                    && !node.blocks().is_empty()
                    && !exclusions.contains_key(&node.id())
            })
    };
    let mut global_depth = 24;
    let mut regional = true;
    let mut count = 0_usize;
    for node in coarse_nodes() {
        global_depth = global_depth.min(representative_depth(&builder.grid, node));
        if regional {
            if let Some((_, _, length)) = representative_regions(&builder.grid, node, cache) {
                count = count
                    .checked_add(length)
                    .ok_or(VoxelSceneBuildError::SizeOverflow)?;
            } else {
                regional = false;
            }
        }
    }
    if !regional {
        builder.grid.max_depth = global_depth;
        return Ok(());
    }
    let requested_bytes = checked_charge(count, std::mem::size_of::<VoxelKey>() as u64)?;
    check_memory_limit(
        builder
            .working_reservation_bytes
            .checked_add(requested_bytes)
            .ok_or(VoxelSceneBuildError::SizeOverflow)?,
        builder.limits.max_estimated_bytes,
    )?;
    let mut regions = Vec::new();
    regions
        .try_reserve_exact(count)
        .map_err(|_| VoxelSceneBuildError::AllocationFailed { requested_bytes })?;
    for node in coarse_nodes() {
        let (shift, keys, length) = representative_regions(&builder.grid, node, cache)
            .expect("coarse regions were validated before allocation");
        regions.extend(keys[..length].iter().map(|key| (shift, *key)));
    }
    regions.sort_unstable();
    regions.dedup();
    let mut grouped: Vec<(u8, Vec<VoxelKey>)> = Vec::new();
    for (shift, key) in regions {
        match grouped.last_mut() {
            Some((last, keys)) if *last == shift => keys.push(key),
            _ => grouped.push((shift, vec![key])),
        }
    }
    // The region vector and projection cache are freed before instance allocation at finish.
    builder.working_reservation_bytes = builder
        .working_reservation_bytes
        .checked_add(checked_charge(
            count,
            std::mem::size_of::<VoxelKey>() as u64,
        )?)
        .ok_or(VoxelSceneBuildError::SizeOverflow)?;
    check_memory_limit(
        builder.working_reservation_bytes,
        builder.limits.max_estimated_bytes,
    )?;
    builder.grid.representative_regions = grouped;
    Ok(())
}

/// Depth of a node's representative cells: 8 per axis, or its stored spacing (`v7.4-pre`).
fn representative_depth(grid: &AdaptiveVoxelGrid, node: &viewer_core::SpatialNode) -> u8 {
    crate::representative_depth_for_spacing(
        &CellGrid {
            root_min: grid.origin,
            root_side: grid.side,
        },
        node.representative_spacing(),
    )
}

/// log2 of the representatives per axis: 3 for the fixed 8, derived from the spacing otherwise.
fn representatives_per_axis_log2(node: &viewer_core::SpatialNode) -> u8 {
    let Some(spacing) = node.spacing() else {
        return 3;
    };
    let bounds = node.bounds();
    let side = (0..3)
        .map(|axis| bounds.max()[axis] - bounds.min()[axis])
        .fold(0.0, f64::max);
    (side / spacing).log2().round().clamp(0.0, 24.0) as u8
}

/// Up to eight occupied regions per source node bound setup work and retained metadata.
/// Arbitrarily dispersed external payloads exceeding that cap use the old global floor.
fn representative_regions(
    grid: &AdaptiveVoxelGrid,
    node: &viewer_core::SpatialNode,
    cache: &dyn ResidentBlockSource,
) -> Option<(u8, [VoxelKey; 8], usize)> {
    let depth = representative_depth(grid, node);
    let shift = representatives_per_axis_log2(node);
    let mut keys = [VoxelKey {
        depth: 0,
        coord: [0; 3],
    }; 8];
    let mut length = 0;
    if depth == 24 {
        return Some((shift, keys, length));
    }
    let region_depth = depth.checked_sub(shift)?;
    for block in node.blocks().iter().filter_map(|id| cache.peek(*id)) {
        for point in block.points().points() {
            if point.position.iter().any(|v| !v.is_finite()) {
                continue;
            }
            let key = grid.key_at_depth(point.position.map(f64::from), region_depth);
            if !keys[..length].contains(&key) {
                if length == keys.len() {
                    return None;
                }
                keys[length] = key;
                length += 1;
            }
        }
    }
    Some((shift, keys, length))
}

fn accumulate_block(
    block: &PointBlock,
    grid: &mut AdaptiveVoxelGrid,
    voxels: &mut BTreeMap<VoxelKey, VoxelAccumulator>,
    budget: AccumulationBudget,
    project: &dyn Fn(SpatialBounds) -> f64,
    cut: CutAccumulation<'_>,
) -> Result<(), VoxelSceneBuildError> {
    let points = block.points().points();
    let intensity = ScalarAttribute::find(block, "intensity");
    let classification = ScalarAttribute::find(block, "classification");
    for (index, point) in points.iter().enumerate() {
        if point.position.iter().any(|value| !value.is_finite()) {
            continue;
        }
        let position = point.position.map(f64::from);
        if cut
            .excluded
            .iter()
            .any(|bounds| bounds.contains(position, cut.root_max))
        {
            continue;
        }
        let max_depth = cut
            .source_depth_limit
            .map_or(grid.max_depth, |depth| depth.min(grid.max_depth))
            .max(cut.min_depth);
        let Some(key) =
            grid.key_for_point_with_depths(position, project, cut.min_depth, max_depth)?
        else {
            continue;
        };
        let voxel_count = voxels.len();
        let voxel = match voxels.entry(key) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                // Do not retain cells that cannot be represented safely by the renderer.
                if grid.render_cell(key).is_none() {
                    continue;
                }
                let next_count = voxel_count
                    .checked_add(1)
                    .ok_or(VoxelSceneBuildError::SizeOverflow)?;
                if next_count > budget.limits.max_voxels {
                    return Err(VoxelSceneBuildError::InstanceLimitExceeded {
                        limit: budget.limits.max_voxels,
                    });
                }
                let accumulator_bytes =
                    checked_charge(next_count, budget.accumulator_charge_bytes)?;
                let instance_bytes = next_count
                    .checked_mul(std::mem::size_of::<InstanceRaw>())
                    .and_then(|bytes| u64::try_from(bytes).ok())
                    .ok_or(VoxelSceneBuildError::SizeOverflow)?;
                let required_bytes = accumulator_bytes
                    .checked_add(budget.working_reservation_bytes.max(instance_bytes))
                    .ok_or(VoxelSceneBuildError::SizeOverflow)?;
                check_memory_limit(required_bytes, budget.limits.max_estimated_bytes)?;
                entry.insert(VoxelAccumulator::default())
            }
        };
        if voxel.count == 0 {
            voxel.classification = classification.and_then(|values| values.value_at(index));
        }
        if let Some(value) = intensity.and_then(|values| values.value_at(index)) {
            voxel.intensity_sum += f64::from(value);
            voxel.intensity_count += 1;
        }
        voxel.rgb[0] += u64::from(point.color.r);
        voxel.rgb[1] += u64::from(point.color.g);
        voxel.rgb[2] += u64::from(point.color.b);
        voxel.count += 1;
    }
    Ok(())
}

fn accumulator_charge_bytes() -> u64 {
    (2 * std::mem::size_of::<(VoxelKey, VoxelAccumulator)>() + 6 * std::mem::size_of::<usize>())
        as u64
}

fn projection_entry_charge_bytes() -> u64 {
    (2 * std::mem::size_of::<(VoxelKey, f64)>()
        + 6 * std::mem::size_of::<usize>()
        + 4 * std::mem::size_of::<VoxelKey>()) as u64
}

fn checked_charge(count: usize, charge: u64) -> Result<u64, VoxelSceneBuildError> {
    u64::try_from(count)
        .ok()
        .and_then(|count| count.checked_mul(charge))
        .ok_or(VoxelSceneBuildError::SizeOverflow)
}

fn instance_allocation_bytes(count: usize) -> Result<u64, VoxelSceneBuildError> {
    count
        .checked_mul(std::mem::size_of::<InstanceRaw>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or(VoxelSceneBuildError::SizeOverflow)
}

fn reserve_instances(count: usize) -> Result<Vec<InstanceRaw>, VoxelSceneBuildError> {
    let requested_bytes = instance_allocation_bytes(count)?;
    let mut instances = Vec::new();
    instances
        .try_reserve_exact(count)
        .map_err(|_| VoxelSceneBuildError::AllocationFailed { requested_bytes })?;
    Ok(instances)
}

fn check_memory_limit(required_bytes: u64, max_bytes: u64) -> Result<(), VoxelSceneBuildError> {
    if required_bytes > max_bytes {
        Err(VoxelSceneBuildError::EstimatedMemoryLimitExceeded {
            required_bytes,
            max_bytes,
        })
    } else {
        Ok(())
    }
}

impl VoxelAccumulator {
    fn average_intensity(&self) -> f32 {
        if self.intensity_count == 0 {
            -1.0
        } else {
            (self.intensity_sum / self.intensity_count as f64) as f32
        }
    }
}

fn safe_render_cell_size(center: [f64; 3], cell_size: f64) -> Option<([f32; 3], f32)> {
    let center_f32 = center.map(|coordinate| coordinate as f32);
    let rounding_error = center
        .iter()
        .zip(center_f32)
        .map(|(exact, rounded)| (f64::from(rounded) - exact).abs())
        .fold(0.0, f64::max);
    let safe_size = cell_size * (1.0 - 1.0e-6) - 2.0 * rounding_error;
    (safe_size.is_finite() && safe_size > 0.0 && safe_size <= f64::from(f32::MAX))
        .then_some((center_f32, safe_size as f32))
}

fn bounds_side(bounds: SpatialBounds) -> f64 {
    (0..3)
        .map(|axis| bounds.max()[axis] - bounds.min()[axis])
        .fold(0.0, f64::max)
}

#[cfg(test)]
#[path = "voxel_grid_tests.rs"]
mod tests;
