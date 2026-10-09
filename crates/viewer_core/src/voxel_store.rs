use std::collections::HashMap;

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
        if origin.iter().any(|coordinate| !coordinate.is_finite()) {
            return Err(VoxelizationError::InvalidOrigin(origin));
        }

        Ok(Self { voxel_size, origin })
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum VoxelizationError {
    #[error("voxel_size must be a positive finite number, got {0}")]
    InvalidVoxelSize(f32),
    #[error("origin coordinates must all be finite, got {0:?}")]
    InvalidOrigin([f32; 3]),
    #[error("point position must contain only finite coordinates, got {0:?}")]
    InvalidPointPosition([f32; 3]),
    #[error("point position {position:?} cannot be represented in the voxel coordinate space")]
    VoxelCoordinateOutOfRange { position: [f32; 3] },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Voxel {
    pub coord: VoxelCoord,
    pub center: [f32; 3],
    pub size: f32,
    pub color: ColorRgb,
    pub point_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelStoreStats {
    pub point_count: u64,
    pub voxel_count: usize,
}

#[derive(Debug, Clone)]
pub struct SparseVoxelStore {
    config: VoxelizationConfig,
    voxels: HashMap<VoxelCoord, VoxelAccumulator>,
    point_count: u64,
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

    pub fn insert_batch(&mut self, batch: &PointBatch) -> Result<(), VoxelizationError> {
        // Validate before mutating so a malformed point cannot leave a partially inserted batch.
        for point in batch.points() {
            self.quantize_position(point.position)?;
        }

        for point in batch.points() {
            let coord = self
                .quantize_position(point.position)
                .expect("batch positions were validated above");
            let entry = self.voxels.entry(coord).or_default();
            entry.accumulate(point.color);
            self.point_count += 1;
        }
        Ok(())
    }

    pub fn voxel_count(&self) -> usize {
        self.voxels.len()
    }

    pub fn point_count(&self) -> u64 {
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

    fn quantize_position(&self, position: [f32; 3]) -> Result<VoxelCoord, VoxelizationError> {
        if position.iter().any(|coordinate| !coordinate.is_finite()) {
            return Err(VoxelizationError::InvalidPointPosition(position));
        }

        let relative = std::array::from_fn::<_, 3, _>(|axis| {
            ((f64::from(position[axis]) - f64::from(self.config.origin[axis]))
                / f64::from(self.config.voxel_size))
            .floor()
        });
        if relative.iter().any(|coordinate| {
            !coordinate.is_finite()
                || *coordinate < f64::from(i32::MIN)
                || *coordinate > f64::from(i32::MAX)
        }) {
            return Err(VoxelizationError::VoxelCoordinateOutOfRange { position });
        }
        if relative.iter().enumerate().any(|(axis, coordinate)| {
            let center = f64::from(self.config.origin[axis])
                + (coordinate + 0.5) * f64::from(self.config.voxel_size);
            !center.is_finite() || center.abs() > f64::from(f32::MAX)
        }) {
            return Err(VoxelizationError::VoxelCoordinateOutOfRange { position });
        }

        Ok(VoxelCoord::new(
            relative[0] as i32,
            relative[1] as i32,
            relative[2] as i32,
        ))
    }

    fn voxel_center(&self, coord: VoxelCoord) -> [f32; 3] {
        let coord = [coord.x, coord.y, coord.z];
        std::array::from_fn(|axis| {
            (f64::from(self.config.origin[axis])
                + (f64::from(coord[axis]) + 0.5) * f64::from(self.config.voxel_size))
                as f32
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct VoxelAccumulator {
    sum_r: u128,
    sum_g: u128,
    sum_b: u128,
    point_count: u64,
}

impl VoxelAccumulator {
    fn accumulate(&mut self, color: ColorRgb) {
        self.sum_r += u128::from(color.r);
        self.sum_g += u128::from(color.g);
        self.sum_b += u128::from(color.b);
        self.point_count += 1;
    }

    fn average_color(self) -> ColorRgb {
        let count = self.point_count.max(1);

        ColorRgb::new(
            (self.sum_r / u128::from(count)) as u8,
            (self.sum_g / u128::from(count)) as u8,
            (self.sum_b / u128::from(count)) as u8,
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

        store.insert_batch(&sample_batch()).unwrap();

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

        store.insert_batch(&batch).unwrap();

        let voxels = store.voxels();
        assert_eq!(voxels.len(), 1);
        assert_eq!(voxels[0].color, ColorRgb::new(20, 30, 40));
        assert_eq!(voxels[0].point_count, 2);
    }

    #[test]
    fn filters_visible_batch_by_bounds() {
        let config = VoxelizationConfig::new(1.0, [0.0, 0.0, 0.0]).unwrap();
        let mut store = SparseVoxelStore::new(config);

        store.insert_batch(&sample_batch()).unwrap();

        let visible = store.build_visible_batch(VisibilityQuery {
            bounds: Some(crate::Aabb::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])),
            limit: None,
        });

        assert_eq!(visible.len(), 1);
        assert_eq!(visible.voxels()[0].center, [0.5, 0.5, 0.5]);
    }
}
