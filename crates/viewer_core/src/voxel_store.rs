use std::collections::HashMap;

use glam::Vec3;
use thiserror::Error;

use crate::{ColorRgb, PointBatch, VoxelCoord};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoxelizationConfig {
    pub voxel_size: f32,
    pub origin: [f32; 3],
}

impl VoxelizationConfig {
    pub fn new(voxel_size: f32, origin: [f32; 3]) -> Result<Self, VoxelizationError> {
        if voxel_size <= 0.0 || !voxel_size.is_finite() {
            return Err(VoxelizationError::InvalidVoxelSize(voxel_size));
        }

        Ok(Self { voxel_size, origin })
    }

    fn origin_vec3(self) -> Vec3 {
        Vec3::from_array(self.origin)
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum VoxelizationError {
    #[error("voxel_size must be a positive finite number, got {0}")]
    InvalidVoxelSize(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Voxel {
    pub coord: VoxelCoord,
    pub center: [f32; 3],
    pub size: f32,
    pub color: ColorRgb,
    pub point_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelStoreStats {
    pub point_count: usize,
    pub voxel_count: usize,
}

#[derive(Debug, Clone)]
pub struct SparseVoxelStore {
    config: VoxelizationConfig,
    voxels: HashMap<VoxelCoord, VoxelAccumulator>,
    point_count: usize,
}

impl SparseVoxelStore {
    pub fn new(config: VoxelizationConfig) -> Self {
        Self {
            config,
            voxels: HashMap::new(),
            point_count: 0,
        }
    }

    pub fn config(&self) -> VoxelizationConfig {
        self.config
    }

    pub fn clear(&mut self) {
        self.voxels.clear();
        self.point_count = 0;
    }

    pub fn insert_batch(&mut self, batch: &PointBatch) {
        for point in batch.points() {
            let coord = self.quantize_position(point.position);
            let entry = self
                .voxels
                .entry(coord)
                .or_insert_with(VoxelAccumulator::default);
            entry.accumulate(point.color);
            self.point_count += 1;
        }
    }

    pub fn voxel_count(&self) -> usize {
        self.voxels.len()
    }

    pub fn point_count(&self) -> usize {
        self.point_count
    }

    pub fn stats(&self) -> VoxelStoreStats {
        VoxelStoreStats {
            point_count: self.point_count(),
            voxel_count: self.voxel_count(),
        }
    }

    pub fn voxels(&self) -> Vec<Voxel> {
        self.iter_voxels().collect()
    }

    pub fn iter_voxels(&self) -> impl Iterator<Item = Voxel> + '_ {
        self.voxels.iter().map(|(coord, accumulator)| Voxel {
            coord: *coord,
            center: self.voxel_center(*coord),
            size: self.config.voxel_size,
            color: accumulator.average_color(),
            point_count: accumulator.point_count,
        })
    }

    fn quantize_position(&self, position: [f32; 3]) -> VoxelCoord {
        let position = Vec3::from_array(position);
        let relative = (position - self.config.origin_vec3()) / self.config.voxel_size;

        VoxelCoord::new(
            relative.x.floor() as i32,
            relative.y.floor() as i32,
            relative.z.floor() as i32,
        )
    }

    fn voxel_center(&self, coord: VoxelCoord) -> [f32; 3] {
        let offset = Vec3::new(
            coord.x as f32 + 0.5,
            coord.y as f32 + 0.5,
            coord.z as f32 + 0.5,
        ) * self.config.voxel_size;

        (self.config.origin_vec3() + offset).to_array()
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct VoxelAccumulator {
    sum_r: u64,
    sum_g: u64,
    sum_b: u64,
    point_count: u32,
}

impl VoxelAccumulator {
    fn accumulate(&mut self, color: ColorRgb) {
        self.sum_r += u64::from(color.r);
        self.sum_g += u64::from(color.g);
        self.sum_b += u64::from(color.b);
        self.point_count += 1;
    }

    fn average_color(self) -> ColorRgb {
        let count = u64::from(self.point_count.max(1));

        ColorRgb::new(
            (self.sum_r / count) as u8,
            (self.sum_g / count) as u8,
            (self.sum_b / count) as u8,
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::{ColorRgb, Point, PointBatch, VisibilityQuery};

    use super::{SparseVoxelStore, VoxelCoord, VoxelizationConfig};

    fn sample_batch() -> PointBatch {
        vec![
            Point::new([0.1, 0.1, 0.1], ColorRgb::new(255, 0, 0)),
            Point::new([0.8, 0.7, 0.9], ColorRgb::new(0, 255, 0)),
            Point::new([1.3, 0.2, 0.2], ColorRgb::new(0, 0, 255)),
        ]
        .into()
    }

    #[test]
    fn quantizes_points_into_expected_voxels() {
        let config = VoxelizationConfig::new(1.0, [0.0, 0.0, 0.0]).unwrap();
        let mut store = SparseVoxelStore::new(config);

        store.insert_batch(&sample_batch());

        let mut coords = store
            .voxels()
            .into_iter()
            .map(|voxel| voxel.coord)
            .collect::<Vec<_>>();
        coords.sort_by_key(|coord| (coord.x, coord.y, coord.z));

        assert_eq!(
            coords,
            vec![VoxelCoord::new(0, 0, 0), VoxelCoord::new(1, 0, 0)]
        );
        assert_eq!(store.point_count(), 3);
        assert_eq!(store.voxel_count(), 2);
    }

    #[test]
    fn averages_colors_inside_the_same_voxel() {
        let config = VoxelizationConfig::new(2.0, [0.0, 0.0, 0.0]).unwrap();
        let mut store = SparseVoxelStore::new(config);
        let batch: PointBatch = vec![
            Point::new([0.1, 0.1, 0.1], ColorRgb::new(10, 20, 30)),
            Point::new([0.9, 0.9, 0.9], ColorRgb::new(30, 40, 50)),
        ]
        .into();

        store.insert_batch(&batch);

        let voxels = store.voxels();
        assert_eq!(voxels.len(), 1);
        assert_eq!(voxels[0].color, ColorRgb::new(20, 30, 40));
        assert_eq!(voxels[0].point_count, 2);
    }

    #[test]
    fn filters_visible_batch_by_bounds() {
        let config = VoxelizationConfig::new(1.0, [0.0, 0.0, 0.0]).unwrap();
        let mut store = SparseVoxelStore::new(config);

        store.insert_batch(&sample_batch());

        let visible = store.build_visible_batch(VisibilityQuery {
            bounds: Some(crate::Aabb::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])),
            limit: None,
        });

        assert_eq!(visible.len(), 1);
        assert_eq!(visible.voxels()[0].center, [0.5, 0.5, 0.5]);
    }
}
