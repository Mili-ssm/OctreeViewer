use std::collections::{BTreeMap, VecDeque, btree_map::Entry};

use viewer_core::{NodeId, PointBlock, ResidentBlockSource, SpatialBounds, SpatialHierarchy};

use crate::{InstanceRaw, RenderScene, attributes::ScalarAttribute};

/// A global dyadic cell identity; source representatives can bound its finest resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct VoxelKey {
    depth: u8,
    coord: [u32; 3],
}

const MAX_PROJECTED_EDGE_CACHE_ENTRIES: usize = 262_144;
const VOXEL_INSTANCE_SIZE: u64 = std::mem::size_of::<InstanceRaw>() as u64;

/// Limits for voxel accumulation and its estimated temporary memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelBuildLimits {
    /// Maximum number of distinct renderable voxels retained by the builder.
    pub max_voxels: usize,
    /// Maximum estimated bytes for accumulation, projection-cache reservation, and instances.
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
    /// Peak from estimated accumulator charges plus the larger of cache reservation or instances.
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
    projected_edge_cache_limit: usize,
    projected_edges: BTreeMap<VoxelKey, f64>,
    projected_edge_order: VecDeque<VoxelKey>,
    projected_edge_peak_entries: usize,
    last_resolved_cell: Option<(VoxelKey, bool)>,
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

    fn key_for_point(
        &mut self,
        point: [f64; 3],
        project: &dyn Fn(SpatialBounds) -> f64,
    ) -> Result<Option<VoxelKey>, VoxelSceneBuildError> {
        // Nearby source points often share a terminal cell. Match the exact integer mapping
        // used by descent, including clamping outside the root, before reusing its result.
        if let Some((key, visible)) = self.last_resolved_cell
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
                self.last_resolved_cell = Some((key, false));
                return Ok(None);
            }
            // Beyond 24 levels f32 cannot reliably distinguish adjacent local cell faces.
            if edge <= self.target_pixels || key.depth >= self.max_depth {
                self.last_resolved_cell = Some((key, true));
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
    cache_reservation_bytes: u64,
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
        let cache_reservation_bytes = u64::try_from(cache_limit)
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
                projected_edge_cache_limit: cache_limit,
                projected_edges: BTreeMap::new(),
                projected_edge_order: VecDeque::new(),
                projected_edge_peak_entries: 0,
                last_resolved_cell: None,
            },
            voxels: BTreeMap::new(),
            limits,
            cache_reservation_bytes,
            accumulator_charge_bytes,
            error: None,
        }
    }

    pub fn add_block(
        &mut self,
        block: &PointBlock,
        project: &dyn Fn(SpatialBounds) -> f64,
    ) -> Result<(), VoxelSceneBuildError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let result = accumulate_block(
            block,
            &mut self.grid,
            &mut self.voxels,
            self.limits,
            self.cache_reservation_bytes,
            self.accumulator_charge_bytes,
            project,
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
            cache_reservation_bytes,
            accumulator_charge_bytes,
            error: _,
        } = self;
        let projected_edge_cache_entries = grid.projected_edges.len();
        let projection_cache_peak_entries = grid.projected_edge_peak_entries;
        grid.projected_edges = BTreeMap::new();
        grid.projected_edge_order = VecDeque::new();

        let count = voxels.len();
        let requested_instance_bytes = instance_allocation_bytes(count)?;
        let accumulator_bytes = checked_charge(count, accumulator_charge_bytes)?;
        let requested_peak_bytes = accumulator_bytes
            .checked_add(cache_reservation_bytes.max(requested_instance_bytes))
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
                    .checked_add(cache_reservation_bytes)
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
    .expect("unbounded voxel scene build cannot exceed usize::MAX instances")
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
    let mut builder = VoxelSceneBuilder::with_limits(
        hierarchy.stats().root_bounds,
        coordinate_origin,
        target_pixels,
        limits,
    );
    // Replacement parents contain one representative per 8x8x8 cell, not dense
    // geometry. Keep a common dyadic floor until their children replace them;
    // a common floor also prevents overlaps at boundaries of mixed source cuts.
    if hierarchy.semantics() == viewer_core::HierarchySemantics::Replacement {
        for node_id in node_ids {
            if let Some(node) = hierarchy.node(*node_id)
                && !hierarchy.is_leaf(*node_id)
                && !node.blocks().is_empty()
            {
                let source_edge = bounds_side(node.bounds()) / 8.0;
                let depth = (builder.grid.side / source_edge)
                    .log2()
                    .floor()
                    .clamp(0.0, 24.0) as u8;
                builder.grid.max_depth = builder.grid.max_depth.min(depth);
            }
        }
    }
    for node_id in node_ids {
        let Some(node) = hierarchy.node(*node_id) else {
            continue;
        };
        for block_id in node.blocks() {
            let Some(block) = cache.peek(*block_id) else {
                continue;
            };
            builder.add_block(block, project)?;
        }
    }
    builder.try_finish(project)
}

fn accumulate_block(
    block: &PointBlock,
    grid: &mut AdaptiveVoxelGrid,
    voxels: &mut BTreeMap<VoxelKey, VoxelAccumulator>,
    limits: VoxelBuildLimits,
    cache_reservation_bytes: u64,
    accumulator_charge_bytes: u64,
    project: &dyn Fn(SpatialBounds) -> f64,
) -> Result<(), VoxelSceneBuildError> {
    let points = block.points().points();
    let intensity = ScalarAttribute::find(block, "intensity");
    let classification = ScalarAttribute::find(block, "classification");
    for (index, point) in points.iter().enumerate() {
        if point.position.iter().any(|value| !value.is_finite()) {
            continue;
        }
        let Some(key) = grid.key_for_point(point.position.map(f64::from), project)? else {
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
                if next_count > limits.max_voxels {
                    return Err(VoxelSceneBuildError::InstanceLimitExceeded {
                        limit: limits.max_voxels,
                    });
                }
                let accumulator_bytes = checked_charge(next_count, accumulator_charge_bytes)?;
                let instance_bytes = next_count
                    .checked_mul(std::mem::size_of::<InstanceRaw>())
                    .and_then(|bytes| u64::try_from(bytes).ok())
                    .ok_or(VoxelSceneBuildError::SizeOverflow)?;
                let required_bytes = accumulator_bytes
                    .checked_add(cache_reservation_bytes.max(instance_bytes))
                    .ok_or(VoxelSceneBuildError::SizeOverflow)?;
                check_memory_limit(required_bytes, limits.max_estimated_bytes)?;
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
mod tests {
    use std::cell::Cell;

    use super::{
        AdaptiveVoxelGrid, MAX_PROJECTED_EDGE_CACHE_ENTRIES, VoxelBuildLimits, VoxelKey,
        VoxelSceneBuildError, VoxelSceneBuilder, accumulator_charge_bytes, bounds_side,
        build_voxel_scene_with_limit, projection_entry_charge_bytes, reserve_instances,
    };
    use viewer_core::{
        BlockId, ColorRgb, DecodedBlockCache, HierarchySemantics, Point, PointBatch, PointBlock,
        SpatialBounds, SpatialHierarchy, SpatialNode,
    };

    #[test]
    fn point_outside_frustum_is_culled_before_voxel_accumulation() {
        let projection_calls = Cell::new(0);
        let project = |_: SpatialBounds| {
            projection_calls.set(projection_calls.get() + 1);
            0.0
        };
        let mut grid = AdaptiveVoxelGrid {
            origin: [0.0; 3],
            side: 1.0,
            target_pixels: 4.0,
            max_depth: 24,
            projected_edge_cache_limit: 0,
            projected_edges: Default::default(),
            projected_edge_order: Default::default(),
            projected_edge_peak_entries: 0,
            last_resolved_cell: None,
        };

        assert_eq!(grid.key_for_point([0.5; 3], &project).unwrap(), None);
        assert_eq!(projection_calls.get(), 1);
        assert!(grid.projected_edges.is_empty());
    }

    #[test]
    fn visible_point_keeps_its_projected_cell() {
        let mut grid = AdaptiveVoxelGrid {
            origin: [0.0; 3],
            side: 1.0,
            target_pixels: 4.0,
            max_depth: 24,
            projected_edge_cache_limit: MAX_PROJECTED_EDGE_CACHE_ENTRIES,
            projected_edges: Default::default(),
            projected_edge_order: Default::default(),
            projected_edge_peak_entries: 0,
            last_resolved_cell: None,
        };

        assert_eq!(
            grid.key_for_point([0.5; 3], &|_| 2.0).unwrap(),
            Some(VoxelKey {
                depth: 0,
                coord: [0; 3],
            })
        );
    }

    #[test]
    fn terminal_hint_reuses_result_with_projection_cache_disabled() {
        let projection_calls = Cell::new(0);
        let project = |_: SpatialBounds| {
            projection_calls.set(projection_calls.get() + 1);
            2.0
        };
        let mut grid = AdaptiveVoxelGrid {
            origin: [0.0; 3],
            side: 1.0,
            target_pixels: 4.0,
            max_depth: 24,
            projected_edge_cache_limit: 0,
            projected_edges: Default::default(),
            projected_edge_order: Default::default(),
            projected_edge_peak_entries: 0,
            last_resolved_cell: None,
        };

        assert!(grid.key_for_point([0.5; 3], &project).unwrap().is_some());
        assert!(grid.key_for_point([0.5; 3], &project).unwrap().is_some());
        assert_eq!(projection_calls.get(), 1);
        assert!(grid.projected_edges.is_empty());
    }

    #[test]
    fn terminal_hint_matches_full_descent_at_boundaries_and_with_eviction() {
        let bounds = SpatialBounds::new([-1.0; 3], [1.0; 3]).unwrap();
        let mut points = vec![[-2.0; 3], [-1.0; 3], [0.0; 3], [1.0; 3], [2.0; 3]];
        for face in [-0.5_f64, 0.0, 0.5] {
            points.extend([
                [face - f64::EPSILON; 3],
                [face; 3],
                [face + f64::EPSILON; 3],
            ]);
        }
        let mut seed = 17_u64;
        for _ in 0..2_000 {
            points.push(std::array::from_fn(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                (seed >> 32) as f64 / u32::MAX as f64 * 4.0 - 2.0
            }));
        }
        for cache_limit in [0, 1, 32] {
            for target in [0.5, 4.0, 16.0] {
                let mut hinted = VoxelSceneBuilder::new(bounds, [0.0; 3], target, 64).grid;
                let mut reference = VoxelSceneBuilder::new(bounds, [0.0; 3], target, 64).grid;
                hinted.projected_edge_cache_limit = cache_limit;
                reference.projected_edge_cache_limit = cache_limit;
                let project = |bounds: SpatialBounds| {
                    if bounds.min()[0] >= 0.5 {
                        0.0
                    } else {
                        bounds_side(bounds) * 64.0 / (1.0 + bounds.min()[2].abs())
                    }
                };
                for point in &points {
                    // Reset only the hint to exercise the original root-to-terminal descent.
                    reference.last_resolved_cell = None;
                    let expected = reference.key_for_point(*point, &project).unwrap();
                    assert_eq!(hinted.key_for_point(*point, &project).unwrap(), expected);
                    assert_eq!(hinted.key_for_point(*point, &project).unwrap(), expected);
                }
            }
        }
        // Nonfinite projected edges retain the existing depth-24 fallback.
        let mut grid = VoxelSceneBuilder::new(bounds, [0.0; 3], 4.0, 64).grid;
        let key = grid.key_for_point([0.25; 3], &|_| f64::NAN).unwrap();
        assert_eq!(key.unwrap().depth, 24);
        assert_eq!(grid.key_for_point([0.25; 3], &|_| f64::NAN).unwrap(), key);
    }

    #[test]
    fn terminal_hint_preserves_scene_colors_attributes_and_budget_errors() {
        use viewer_core::{
            AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues,
        };
        let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
        let project = |bounds: SpatialBounds| bounds_side(bounds) * 32.0;
        let make_block = |index: usize| {
            PointBlock::new(
                BlockId::new(index as u64),
                PointBatch::new(vec![Point::new(
                    [index as f32 / 200.0; 3],
                    ColorRgb::new(index as u8, 20, 90),
                )]),
                vec![
                    AttributeColumn::new(
                        AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap(),
                        AttributeValues::U16(vec![index as u16 * 3]),
                        1,
                    )
                    .unwrap(),
                    AttributeColumn::new(
                        AttributeDescriptor::new("classification", AttributeDataType::U8, 1)
                            .unwrap(),
                        AttributeValues::U8(vec![(index % 8) as u8]),
                        1,
                    )
                    .unwrap(),
                ],
            )
        };
        for limits in [
            VoxelBuildLimits::default(),
            VoxelBuildLimits {
                max_voxels: 1,
                ..VoxelBuildLimits::default()
            },
            VoxelBuildLimits {
                max_estimated_bytes: 256,
                ..VoxelBuildLimits::default()
            },
        ] {
            let mut hinted = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 4.0, limits);
            let mut reference = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 4.0, limits);
            for index in 0..100 {
                let block = make_block(index);
                reference.grid.last_resolved_cell = None;
                let expected = reference.add_block(&block, &project);
                assert_eq!(hinted.add_block(&block, &project), expected);
                if expected.is_err() {
                    break;
                }
            }
            match (hinted.try_finish(&project), reference.try_finish(&project)) {
                (Ok(actual), Ok(expected)) => {
                    assert_eq!(actual.scene, expected.scene);
                    assert_eq!(actual.min_edge_pixels, expected.min_edge_pixels);
                    assert_eq!(actual.max_edge_pixels, expected.max_edge_pixels);
                    assert_eq!(actual.oversized_voxels, 0);
                }
                (Err(actual), Err(expected)) => assert_eq!(actual, expected),
                _ => panic!("hint changed scene completion"),
            }
        }
    }

    #[test]
    fn incremental_builder_matches_cached_scene_output() {
        let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
        let hierarchy = SpatialHierarchy::new_with_semantics(
            vec![
                SpatialNode::new(
                    viewer_core::NodeId::new(1),
                    None,
                    0,
                    bounds,
                    3,
                    0.0,
                    vec![BlockId::new(1)],
                )
                .unwrap(),
            ],
            vec![],
            Some(3),
            HierarchySemantics::Replacement,
        )
        .unwrap();
        let block = PointBlock::new(
            BlockId::new(1),
            PointBatch::new(vec![
                Point::new([0.1, 0.1, 0.1], ColorRgb::new(1, 2, 3)),
                Point::new([0.8, 0.1, 0.1], ColorRgb::new(7, 8, 9)),
                Point::new([0.1, 0.8, 0.1], ColorRgb::new(10, 11, 12)),
            ]),
            vec![],
        );
        let mut cache = DecodedBlockCache::new(4096);
        cache.insert(block.clone());
        let project = |bounds| bounds_side(bounds) * 16.0;
        let expected = build_voxel_scene_with_limit(
            &hierarchy,
            &[viewer_core::NodeId::new(1)],
            &cache,
            [0.0; 3],
            4.0,
            &project,
            8,
        )
        .unwrap();

        let mut builder = super::VoxelSceneBuilder::new(bounds, [0.0; 3], 4.0, 8);
        builder.add_block(&block, &project).unwrap();
        let actual = builder.finish(&project);

        assert_eq!(actual.scene, expected.scene);
        assert_eq!(actual.min_edge_pixels, expected.min_edge_pixels);
        assert_eq!(actual.max_edge_pixels, expected.max_edge_pixels);
        assert_eq!(actual.oversized_voxels, expected.oversized_voxels);
    }

    #[test]
    fn alternate_resident_source_preserves_voxels_missing_blocks_and_limits() {
        struct ResidentBlocks(Vec<PointBlock>);
        impl viewer_core::ResidentBlockSource for ResidentBlocks {
            fn peek(&self, id: BlockId) -> Option<&PointBlock> {
                self.0.iter().find(|block| block.id() == id)
            }
        }
        let hierarchy = SpatialHierarchy::new(
            vec![
                SpatialNode::new(
                    viewer_core::NodeId::new(1),
                    None,
                    0,
                    SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                    3,
                    0.0,
                    vec![BlockId::new(1)],
                )
                .unwrap(),
            ],
            vec![],
            Some(3),
        )
        .unwrap();
        let blocks = ResidentBlocks(vec![block(vec![
            point([0.1; 3]),
            point([0.8, 0.1, 0.1]),
            point([0.1, 0.8, 0.1]),
        ])]);
        let mut cache = DecodedBlockCache::new(4096);
        cache.insert(blocks.0[0].clone());
        let stats = cache.stats();
        let recency = cache.lru_entries().collect::<Vec<_>>();
        let erased: &dyn viewer_core::ResidentBlockSource = &blocks;
        let nodes = [viewer_core::NodeId::new(1), viewer_core::NodeId::new(999)];
        let project = |bounds| bounds_side(bounds) * 16.0;
        for limits in [
            VoxelBuildLimits {
                max_voxels: 10,
                max_estimated_bytes: u64::MAX,
            },
            VoxelBuildLimits {
                max_voxels: 2,
                max_estimated_bytes: u64::MAX,
            },
            VoxelBuildLimits {
                max_voxels: 10,
                max_estimated_bytes: 0,
            },
        ] {
            let build = |source: &dyn viewer_core::ResidentBlockSource| {
                super::build_voxel_scene_with_limits(
                    &hierarchy, &nodes, source, [0.0; 3], 1.0, &project, limits,
                )
                .map(|result| result.scene)
            };
            let concrete = super::build_voxel_scene_with_limits(
                &hierarchy, &nodes, &blocks, [0.0; 3], 1.0, &project, limits,
            )
            .map(|result| result.scene);
            assert_eq!(concrete, build(&cache));
            assert_eq!(build(erased), build(&cache));
        }
        let absent = ResidentBlocks(vec![]);
        let empty = super::build_voxel_scene(&hierarchy, &nodes, &absent, [0.0; 3], 1.0, &project);
        assert_eq!(empty.scene.instance_count(), 0);
        assert_eq!(cache.stats(), stats);
        assert_eq!(cache.lru_entries().collect::<Vec<_>>(), recency);
    }

    #[test]
    fn voxel_scene_stops_at_the_rendered_instance_limit() {
        let hierarchy = SpatialHierarchy::new_with_semantics(
            vec![
                SpatialNode::new(
                    viewer_core::NodeId::new(1),
                    None,
                    0,
                    SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
                    3,
                    0.0,
                    vec![BlockId::new(1)],
                )
                .unwrap(),
            ],
            vec![],
            Some(3),
            HierarchySemantics::Replacement,
        )
        .unwrap();
        let mut cache = DecodedBlockCache::new(4096);
        cache.insert(PointBlock::new(
            BlockId::new(1),
            PointBatch::new(vec![
                Point::new([0.1, 0.1, 0.1], ColorRgb::new(1, 2, 3)),
                Point::new([0.8, 0.1, 0.1], ColorRgb::new(1, 2, 3)),
                Point::new([0.1, 0.8, 0.1], ColorRgb::new(1, 2, 3)),
            ]),
            vec![],
        ));

        let error = build_voxel_scene_with_limit(
            &hierarchy,
            &[viewer_core::NodeId::new(1)],
            &cache,
            [0.0; 3],
            1.0,
            &|bounds| bounds_side(bounds) * 16.0,
            2,
        )
        .err()
        .unwrap();

        assert_eq!(
            error,
            VoxelSceneBuildError::InstanceLimitExceeded { limit: 2 }
        );
    }

    fn block(points: Vec<Point>) -> PointBlock {
        PointBlock::new(BlockId::new(1), PointBatch::new(points), vec![])
    }

    fn point(position: [f32; 3]) -> Point {
        Point::new(position, ColorRgb::new(10, 20, 30))
    }

    #[test]
    fn memory_budget_accepts_exactly_one_voxel_and_rejects_the_next() {
        let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
        let exact_bytes =
            accumulator_charge_bytes() + std::mem::size_of::<super::InstanceRaw>() as u64;
        let limits = VoxelBuildLimits {
            max_voxels: 10,
            max_estimated_bytes: exact_bytes,
        };
        let project = |bounds| bounds_side(bounds) * 2.0;
        let mut exact = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 1.0, limits);
        exact
            .add_block(&block(vec![point([0.1; 3])]), &project)
            .unwrap();
        let scene = exact.try_finish(&project).unwrap();
        assert_eq!(scene.memory_stats.estimated_peak_bytes, exact_bytes);

        let mut over = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 1.0, limits);
        let error = over
            .add_block(&block(vec![point([0.1; 3]), point([0.8; 3])]), &project)
            .unwrap_err();
        assert_eq!(
            error,
            VoxelSceneBuildError::EstimatedMemoryLimitExceeded {
                required_bytes: 2 * accumulator_charge_bytes()
                    + 2 * std::mem::size_of::<super::InstanceRaw>() as u64,
                max_bytes: exact_bytes,
            }
        );
        assert_eq!(over.add_block(&block(vec![]), &project), Err(error));
        assert_eq!(over.try_finish(&project).unwrap_err(), error);
    }

    #[test]
    fn voxel_limit_is_independent_from_memory_budget() {
        let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
        let limits = VoxelBuildLimits {
            max_voxels: 1,
            max_estimated_bytes: 1024 * 1024,
        };
        let mut builder = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 1.0, limits);
        let error = builder
            .add_block(&block(vec![point([0.1; 3]), point([0.8; 3])]), &|bounds| {
                bounds_side(bounds) * 2.0
            })
            .unwrap_err();
        assert_eq!(
            error,
            VoxelSceneBuildError::InstanceLimitExceeded { limit: 1 }
        );
    }

    #[test]
    fn repeated_points_in_one_voxel_consume_one_accumulator_budget() {
        let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
        let limits = VoxelBuildLimits {
            max_voxels: 1,
            max_estimated_bytes: accumulator_charge_bytes()
                + std::mem::size_of::<super::InstanceRaw>() as u64,
        };
        let mut builder = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 4.0, limits);
        builder
            .add_block(&block(vec![point([0.1; 3]); 100]), &|_| 2.0)
            .unwrap();
        assert_eq!(
            builder.try_finish(&|_| 2.0).unwrap().scene.instance_count(),
            1
        );
    }

    #[test]
    fn low_budget_disables_projection_cache_and_records_estimated_stats() {
        let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
        let limits = VoxelBuildLimits {
            max_voxels: 1,
            max_estimated_bytes: projection_entry_charge_bytes() - 1,
        };
        let builder = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 4.0, limits);
        assert_eq!(builder.grid.projected_edge_cache_limit, 0);
        let scene = builder.try_finish(&|_| 1.0).unwrap();
        assert_eq!(scene.memory_stats.projection_cache_peak_entries, 0);
        assert_eq!(scene.memory_stats.estimated_peak_bytes, 0);
    }

    #[test]
    fn unrenderable_cell_does_not_consume_voxel_or_memory_budget() {
        let bounds = SpatialBounds::new([100_000_000.0; 3], [100_000_001.0; 3]).unwrap();
        let limits = VoxelBuildLimits {
            max_voxels: 0,
            max_estimated_bytes: 1024 * 1024,
        };
        let mut builder = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 4.0, limits);
        builder
            .add_block(&block(vec![point([100_000_000.0; 3])]), &|_| 1.0)
            .unwrap();
        let scene = builder.try_finish(&|_| 1.0).unwrap();
        assert_eq!(scene.scene.instance_count(), 0);
        assert_eq!(scene.memory_stats.instance_capacity_bytes, 0);
    }

    #[test]
    fn scene_is_identical_with_projection_cache_disabled_or_evicted() {
        let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
        let points = vec![
            point([0.1, 0.1, 0.1]),
            point([0.8, 0.1, 0.1]),
            point([0.1, 0.8, 0.1]),
            point([0.8, 0.8, 0.8]),
        ];
        let limits = VoxelBuildLimits {
            max_voxels: 32,
            max_estimated_bytes: 1024 * 1024,
        };
        let project = |bounds| bounds_side(bounds) * 8.0;
        let mut evicting = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 1.0, limits);
        evicting.grid.projected_edge_cache_limit = 1;
        evicting
            .add_block(&block(points.clone()), &project)
            .unwrap();
        let evicted = evicting.try_finish(&project).unwrap();
        assert!(evicted.memory_stats.projection_cache_peak_entries > 0);

        let mut uncached = VoxelSceneBuilder::with_limits(bounds, [0.0; 3], 1.0, limits);
        uncached.grid.projected_edge_cache_limit = 0;
        let uncached = uncached
            .add_block(&block(points), &project)
            .and_then(|()| uncached.try_finish(&project));
        assert_eq!(uncached.unwrap().scene, evicted.scene);
    }

    #[test]
    fn charge_overflow_is_reported_without_large_allocation() {
        assert_eq!(
            super::checked_charge(usize::MAX, u64::MAX),
            Err(VoxelSceneBuildError::SizeOverflow)
        );
    }

    #[test]
    fn impossible_instance_reservation_returns_an_error_without_allocating() {
        assert_eq!(
            reserve_instances(usize::MAX).unwrap_err(),
            VoxelSceneBuildError::SizeOverflow
        );
        // Vec rejects capacities above isize::MAX bytes before asking the allocator.
        let count = isize::MAX as usize / std::mem::size_of::<super::InstanceRaw>() + 1;
        assert_eq!(
            reserve_instances(count).unwrap_err(),
            VoxelSceneBuildError::AllocationFailed {
                requested_bytes: count as u64 * super::VOXEL_INSTANCE_SIZE,
            }
        );
    }
}
