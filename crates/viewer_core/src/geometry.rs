use glam::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ColorRgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn to_linear_rgba(self) -> [f32; 4] {
        [
            srgb_channel_to_linear(self.r),
            srgb_channel_to_linear(self.g),
            srgb_channel_to_linear(self.b),
            1.0,
        ]
    }
}

fn srgb_channel_to_linear(channel: u8) -> f32 {
    let srgb = f32::from(channel) / 255.0;
    if srgb <= 0.04045 {
        srgb / 12.92
    } else {
        ((srgb + 0.055) / 1.055).powf(2.4)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub position: [f32; 3],
    pub color: ColorRgb,
}

impl Point {
    pub fn new(position: [f32; 3], color: ColorRgb) -> Self {
        Self { position, color }
    }

    pub fn position_vec3(self) -> Vec3 {
        Vec3::from_array(self.position)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PointBatch {
    points: Vec<Point>,
}

impl PointBatch {
    pub fn new(points: Vec<Point>) -> Self {
        Self { points }
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn points(&self) -> &[Point] {
        &self.points
    }
}

impl From<Vec<Point>> for PointBatch {
    fn from(points: Vec<Point>) -> Self {
        Self::new(points)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VoxelCoord {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl VoxelCoord {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub fn new(min: [f32; 3], max: [f32; 3]) -> Self {
        Self { min, max }
    }

    pub fn is_valid(&self) -> bool {
        (0..3).all(|axis| {
            self.min[axis].is_finite()
                && self.max[axis].is_finite()
                && self.min[axis] <= self.max[axis]
        })
    }

    pub fn contains_point(&self, point: [f32; 3]) -> bool {
        self.is_valid()
            && point.iter().all(|coordinate| coordinate.is_finite())
            && point[0] >= self.min[0]
            && point[0] <= self.max[0]
            && point[1] >= self.min[1]
            && point[1] <= self.max[1]
            && point[2] >= self.min[2]
            && point[2] <= self.max[2]
    }

    pub fn contains_aabb(&self, other: &Self) -> bool {
        self.is_valid()
            && other.is_valid()
            && (0..3)
                .all(|axis| other.min[axis] >= self.min[axis] && other.max[axis] <= self.max[axis])
    }

    pub fn intersects(&self, other: &Self) -> bool {
        self.is_valid()
            && other.is_valid()
            && (0..3)
                .all(|axis| self.min[axis] <= other.max[axis] && other.min[axis] <= self.max[axis])
    }
}

#[cfg(test)]
mod tests {
    use super::{Aabb, ColorRgb};

    #[test]
    fn converts_srgb_channels_to_linear_light() {
        let color = ColorRgb::new(0, 128, 255).to_linear_rgba();

        assert_eq!(color[0], 0.0);
        assert!((color[1] - 0.21586).abs() < 0.0001);
        assert_eq!(color[2], 1.0);
        assert_eq!(color[3], 1.0);
    }

    #[test]
    fn aabb_contains_points_and_boxes_on_inclusive_boundaries() {
        let bounds = Aabb::new([0.0; 3], [1.0; 3]);

        assert!(bounds.is_valid());
        assert!(bounds.contains_point([0.0, 0.5, 1.0]));
        assert!(bounds.contains_aabb(&Aabb::new([0.0; 3], [1.0; 3])));
        assert!(bounds.contains_aabb(&Aabb::new([0.25; 3], [0.75; 3])));
    }

    #[test]
    fn aabb_intersection_includes_touching_faces_and_excludes_separated_boxes() {
        let bounds = Aabb::new([0.0; 3], [1.0; 3]);

        assert!(bounds.intersects(&Aabb::new([1.0, 0.2, 0.2], [2.0, 0.8, 0.8])));
        assert!(!bounds.intersects(&Aabb::new([1.01, 0.2, 0.2], [2.0, 0.8, 0.8])));
    }

    #[test]
    fn invalid_or_non_finite_aabbs_are_rejected_by_spatial_queries() {
        let bounds = Aabb::new([0.0; 3], [1.0; 3]);
        let inverted = Aabb::new([0.0, 0.0, 0.0], [1.0, -1.0, 1.0]);
        let non_finite = Aabb::new([0.0; 3], [f32::INFINITY, 1.0, 1.0]);

        assert!(!inverted.is_valid());
        assert!(!non_finite.is_valid());
        assert!(!inverted.contains_point([0.5; 3]));
        assert!(!bounds.intersects(&inverted));
        assert!(!bounds.contains_aabb(&non_finite));
        assert!(!bounds.contains_point([f32::NAN, 0.5, 0.5]));
    }
}
