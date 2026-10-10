use std::collections::BTreeMap;

use viewer_core::{
    AttributeValues, DecodedBlockCache, NodeId, PointBlock, SpatialBounds, SpatialHierarchy,
};

use crate::{InstanceRaw, RenderScene};

/// A global dyadic cell identity; storage nodes never determine rendered cell size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct VoxelKey {
    depth: u8,
    coord: [u32; 3],
}

struct AdaptiveVoxelGrid<'a> {
    origin: [f64; 3],
    side: f64,
    target_pixels: f64,
    project: &'a dyn Fn(SpatialBounds) -> f64,
    projected_edges: BTreeMap<VoxelKey, f64>,
}

impl AdaptiveVoxelGrid<'_> {
    fn bounds(&self, key: VoxelKey) -> SpatialBounds {
        let side = self.side / 2.0_f64.powi(i32::from(key.depth));
        let min = std::array::from_fn(|axis| self.origin[axis] + f64::from(key.coord[axis]) * side);
        let max = std::array::from_fn(|axis| {
            self.origin[axis] + (f64::from(key.coord[axis]) + 1.0) * side
        });
        SpatialBounds::new(min, max).expect("dyadic cells have ordered finite bounds")
    }

    fn key_for_point(&mut self, point: [f64; 3]) -> Option<VoxelKey> {
        let mut key = VoxelKey {
            depth: 0,
            coord: [0; 3],
        };
        loop {
            let bounds = self.bounds(key);
            let edge = *self
                .projected_edges
                .entry(key)
                .or_insert_with(|| (self.project)(bounds));
            // Cull cells as soon as their bounds cannot contribute to the view. Deferring this
            // until scene construction would retain one accumulator per off-screen voxel.
            if edge <= 0.0 {
                return None;
            }
            // Beyond 24 levels f32 cannot reliably distinguish adjacent local cell faces.
            if edge <= self.target_pixels || key.depth >= 24 {
                return Some(key);
            }
            key.depth += 1;
            let count = 1_u32 << key.depth;
            key.coord = std::array::from_fn(|axis| {
                (((point[axis] - self.origin[axis]) / self.side * f64::from(count))
                    .floor()
                    .clamp(0.0, f64::from(count - 1))) as u32
            });
        }
    }
}

pub struct VoxelScene {
    pub scene: RenderScene,
    pub min_edge_pixels: f64,
    pub max_edge_pixels: f64,
    pub oversized_voxels: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoxelSceneBuildError {
    InstanceLimitExceeded { limit: usize },
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

pub fn build_voxel_scene(
    hierarchy: &SpatialHierarchy,
    node_ids: &[NodeId],
    cache: &DecodedBlockCache,
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

pub fn build_voxel_scene_with_limit(
    hierarchy: &SpatialHierarchy,
    node_ids: &[NodeId],
    cache: &DecodedBlockCache,
    coordinate_origin: [f64; 3],
    target_pixels: f64,
    project: &dyn Fn(SpatialBounds) -> f64,
    max_voxels: usize,
) -> Result<VoxelScene, VoxelSceneBuildError> {
    let root = hierarchy.stats().root_bounds;
    let mut grid = AdaptiveVoxelGrid {
        // Use local coordinates before subdivision to retain fine cells at geographic origins.
        origin: std::array::from_fn(|axis| root.min()[axis] - coordinate_origin[axis]),
        side: bounds_side(root).max(1.0e-5),
        target_pixels,
        project,
        projected_edges: BTreeMap::new(),
    };
    let mut voxels = BTreeMap::<VoxelKey, VoxelAccumulator>::new();
    for node_id in node_ids {
        let Some(node) = hierarchy.node(*node_id) else {
            continue;
        };
        for block_id in node.blocks() {
            let Some(block) = cache.peek(*block_id) else {
                continue;
            };
            accumulate_block(block, &mut grid, &mut voxels, max_voxels)?;
        }
    }
    let mut min_edge_pixels = f64::INFINITY;
    let mut max_edge_pixels: f64 = 0.0;
    let mut oversized_voxels = 0;
    let instances = voxels
        .into_iter()
        .filter_map(|(key, accumulator)| {
            let edge = grid.projected_edges[&key];
            if edge <= 0.0 {
                return None;
            }
            let bounds = grid.bounds(key);
            let cell_size = grid.side / 2.0_f64.powi(i32::from(key.depth));
            let center_exact = std::array::from_fn(|axis| bounds.min()[axis] + cell_size * 0.5);
            let (center, size) = safe_render_cell_size(center_exact, cell_size)?;
            min_edge_pixels = min_edge_pixels.min(edge);
            max_edge_pixels = max_edge_pixels.max(edge);
            oversized_voxels += usize::from(edge > target_pixels * (1.0 + 1.0e-6));
            let color = accumulator
                .rgb
                .map(|sum| ((sum + accumulator.count / 2) / accumulator.count).min(255) as u8);
            Some(InstanceRaw {
                center,
                size,
                color: viewer_core::ColorRgb::new(color[0], color[1], color[2]).to_linear_rgba(),
                intensity: accumulator.average_intensity(),
                classification: accumulator.classification.unwrap_or(-1.0),
            })
        })
        .collect();
    Ok(VoxelScene {
        scene: RenderScene::from_instances(instances),
        min_edge_pixels: if min_edge_pixels.is_finite() {
            min_edge_pixels
        } else {
            0.0
        },
        max_edge_pixels,
        oversized_voxels,
    })
}

fn accumulate_block(
    block: &PointBlock,
    grid: &mut AdaptiveVoxelGrid<'_>,
    voxels: &mut BTreeMap<VoxelKey, VoxelAccumulator>,
    max_voxels: usize,
) -> Result<(), VoxelSceneBuildError> {
    let points = block.points().points();
    let intensity = scalar_attribute(block, "intensity", points.len());
    let classification = scalar_attribute(block, "classification", points.len());
    for (index, point) in points.iter().enumerate() {
        if point.position.iter().any(|value| !value.is_finite()) {
            continue;
        }
        let Some(key) = grid.key_for_point(point.position.map(f64::from)) else {
            continue;
        };
        if !voxels.contains_key(&key) && voxels.len() >= max_voxels {
            return Err(VoxelSceneBuildError::InstanceLimitExceeded { limit: max_voxels });
        }
        let voxel = voxels.entry(key).or_default();
        if voxel.count == 0 {
            voxel.classification = classification
                .as_ref()
                .and_then(|values| values.get(index).copied());
        }
        if let Some(value) = intensity.as_ref().and_then(|values| values.get(index)) {
            voxel.intensity_sum += f64::from(*value);
            voxel.intensity_count += 1;
        }
        voxel.rgb[0] += u64::from(point.color.r);
        voxel.rgb[1] += u64::from(point.color.g);
        voxel.rgb[2] += u64::from(point.color.b);
        voxel.count += 1;
    }
    Ok(())
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

fn scalar_attribute(block: &PointBlock, name: &str, point_count: usize) -> Option<Vec<f32>> {
    let column = block.attributes().iter().find(|column| {
        column.descriptor().name().eq_ignore_ascii_case(name)
            && column.descriptor().component_count() == 1
    })?;
    let values: Vec<f64> = match column.values() {
        AttributeValues::U8(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::I8(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::U16(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::I16(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::U32(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::I32(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::U64(values) => values.iter().map(|value| *value as f64).collect(),
        AttributeValues::I64(values) => values.iter().map(|value| *value as f64).collect(),
        AttributeValues::F32(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::F64(values) => values.clone(),
    };
    if values.len() != point_count
        || values
            .iter()
            .any(|value| !value.is_finite() || value.abs() > f64::from(f32::MAX))
    {
        return None;
    }
    Some(values.into_iter().map(|value| value as f32).collect())
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
        AdaptiveVoxelGrid, VoxelKey, VoxelSceneBuildError, bounds_side,
        build_voxel_scene_with_limit,
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
            project: &project,
            projected_edges: Default::default(),
        };

        assert_eq!(grid.key_for_point([0.5; 3]), None);
        assert_eq!(projection_calls.get(), 1);
        assert!(grid.projected_edges.contains_key(&VoxelKey {
            depth: 0,
            coord: [0; 3],
        }));
    }

    #[test]
    fn visible_point_keeps_its_projected_cell() {
        let mut grid = AdaptiveVoxelGrid {
            origin: [0.0; 3],
            side: 1.0,
            target_pixels: 4.0,
            project: &|_| 2.0,
            projected_edges: Default::default(),
        };

        assert_eq!(
            grid.key_for_point([0.5; 3]),
            Some(VoxelKey {
                depth: 0,
                coord: [0; 3],
            })
        );
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
}
