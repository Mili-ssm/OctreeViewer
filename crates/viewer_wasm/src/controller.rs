use serde::Serialize;
use thiserror::Error;
use viewer_core::{
    ColorRgb, Point, PointBatch, SparseVoxelStore, VisibilityQuery, VoxelizationConfig,
    VoxelizationError,
};
use viewer_render_wgpu::{InstanceRaw, RenderScene};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewerConfig {
    pub voxel_size: f32,
    pub origin: [f32; 3],
    pub viewport_size: [u32; 2],
}

impl Default for ViewerConfig {
    fn default() -> Self {
        Self {
            voxel_size: 1.0,
            origin: [0.0, 0.0, 0.0],
            viewport_size: [1, 1],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewerStats {
    pub point_count: u64,
    pub voxel_count: usize,
    pub instance_count: usize,
    pub viewport_size: [u32; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RenderInstanceSnapshot {
    pub center: [f32; 3],
    pub size: f32,
    pub color: [f32; 4],
}

impl From<InstanceRaw> for RenderInstanceSnapshot {
    fn from(instance: InstanceRaw) -> Self {
        Self {
            center: instance.center,
            size: instance.size,
            color: instance.color,
        }
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum ViewerError {
    #[error(transparent)]
    InvalidVoxelization(#[from] VoxelizationError),
    #[error("point position must contain only finite coordinates, got {0:?}")]
    InvalidPointPosition([f32; 3]),
    #[error("positions length must be a multiple of 3, got {0}")]
    InvalidPositionComponentCount(usize),
    #[error("colors length must equal point count * 3, got {actual} for {point_count} points")]
    InvalidColorComponentCount { actual: usize, point_count: usize },
}

#[derive(Debug, Clone)]
pub struct ViewerController {
    data: ViewerData,
    voxel_config: VoxelizationConfig,
    scene: RenderScene,
    viewport_size: [u32; 2],
}

#[derive(Debug, Clone)]
enum ViewerData {
    RawPoints(Vec<Point>),
    Voxelized(SparseVoxelStore),
}

impl ViewerController {
    pub fn new(config: ViewerConfig) -> Result<Self, ViewerError> {
        let voxel_config = VoxelizationConfig::new(config.voxel_size, config.origin)?;

        Ok(Self {
            data: ViewerData::RawPoints(Vec::new()),
            voxel_config,
            scene: RenderScene::default(),
            viewport_size: config.viewport_size.map(|size| size.max(1)),
        })
    }

    pub fn replace_points(&mut self, positions: &[f32], colors: &[u8]) -> Result<(), ViewerError> {
        let batch = build_point_batch(positions, colors)?;
        self.replace_points_owned(batch.into_points());
        Ok(())
    }

    /// Replaces the raw scene by taking ownership of its point allocation.
    pub fn replace_points_owned(&mut self, points: Vec<Point>) {
        self.data = ViewerData::RawPoints(points);
        self.refresh_scene();
    }

    pub fn append_points(&mut self, positions: &[f32], colors: &[u8]) -> Result<(), ViewerError> {
        self.append_points_owned(build_point_batch(positions, colors)?.into_points())
    }

    /// Appends an owned point batch without retaining an intermediate copy.
    pub fn append_points_owned(&mut self, points: Vec<Point>) -> Result<(), ViewerError> {
        match &mut self.data {
            ViewerData::RawPoints(existing) => existing.extend(points),
            ViewerData::Voxelized(store) => store.insert_points(&points)?,
        }
        self.refresh_scene();
        Ok(())
    }

    pub fn replace_voxelized_points(
        &mut self,
        positions: &[f32],
        colors: &[u8],
    ) -> Result<(), ViewerError> {
        self.replace_voxelized_points_owned(build_point_batch(positions, colors)?.into_points())
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "public API named for ownership: callers hand over their buffer"
    )]
    pub fn replace_voxelized_points_owned(
        &mut self,
        points: Vec<Point>,
    ) -> Result<(), ViewerError> {
        let mut replacement = SparseVoxelStore::new(self.voxel_config);
        replacement.insert_points(&points)?;
        self.data = ViewerData::Voxelized(replacement);
        self.refresh_scene();
        Ok(())
    }

    pub fn append_voxelized_points(
        &mut self,
        positions: &[f32],
        colors: &[u8],
    ) -> Result<(), ViewerError> {
        self.append_voxelized_points_owned(build_point_batch(positions, colors)?.into_points())
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "public API named for ownership: callers hand over their buffer"
    )]
    pub fn append_voxelized_points_owned(&mut self, points: Vec<Point>) -> Result<(), ViewerError> {
        match &mut self.data {
            ViewerData::RawPoints(existing) => {
                let mut store = SparseVoxelStore::new(self.voxel_config);
                store.insert_points(existing)?;
                store.insert_points(&points)?;
                self.data = ViewerData::Voxelized(store);
            }
            ViewerData::Voxelized(store) => store.insert_points(&points)?,
        }
        self.refresh_scene();
        Ok(())
    }

    pub fn clear(&mut self) {
        self.data = ViewerData::RawPoints(Vec::new());
        self.refresh_scene();
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.viewport_size = [width.max(1), height.max(1)];
    }

    pub fn render_scene(&self) -> &RenderScene {
        &self.scene
    }

    pub fn stats(&self) -> ViewerStats {
        let scene = &self.scene;
        let (point_count, voxel_count) = match &self.data {
            ViewerData::RawPoints(points) => (points.len() as u64, scene.instance_count()),
            ViewerData::Voxelized(store) => (store.point_count(), store.voxel_count()),
        };

        ViewerStats {
            point_count,
            voxel_count,
            instance_count: scene.instance_count(),
            viewport_size: self.viewport_size,
        }
    }

    fn refresh_scene(&mut self) {
        self.scene = match &self.data {
            ViewerData::RawPoints(points) => RenderScene::from_points(points),
            ViewerData::Voxelized(store) => {
                let batch = store.build_visible_batch(VisibilityQuery::default());
                RenderScene::from_visible_batch(&batch)
            }
        };
    }
}

// Keep this layout validation and iteration compatible with the workspace's Rust 1.85 floor.
#[allow(
    clippy::manual_is_multiple_of,
    clippy::chunks_exact_to_as_chunks,
    reason = "the workspace Rust 1.85 floor has neither is_multiple_of nor as_chunks"
)]
fn build_point_batch(positions: &[f32], colors: &[u8]) -> Result<PointBatch, ViewerError> {
    if positions.len() % 3 != 0 {
        return Err(ViewerError::InvalidPositionComponentCount(positions.len()));
    }

    let point_count = positions.len() / 3;
    if colors.len() != point_count * 3 {
        return Err(ViewerError::InvalidColorComponentCount {
            actual: colors.len(),
            point_count,
        });
    }

    let points = positions
        .chunks_exact(3)
        .zip(colors.chunks_exact(3))
        .map(|(position, color)| {
            let position = [position[0], position[1], position[2]];
            if position.iter().any(|coordinate| !coordinate.is_finite()) {
                return Err(ViewerError::InvalidPointPosition(position));
            }
            Ok(Point::new(
                position,
                ColorRgb::new(color[0], color[1], color[2]),
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(PointBatch::new(points))
}

#[cfg(test)]
mod tests {
    use viewer_core::{ColorRgb, Point};

    use super::{ViewerConfig, ViewerController, ViewerError};

    #[test]
    fn replaces_points_and_builds_instances() {
        let mut viewer = ViewerController::new(ViewerConfig {
            voxel_size: 1.0,
            origin: [0.0, 0.0, 0.0],
            viewport_size: [640, 480],
        })
        .unwrap();

        viewer
            .replace_points(&[0.1, 0.1, 0.1, 1.2, 0.2, 0.1], &[255, 0, 0, 0, 255, 0])
            .unwrap();

        let stats = viewer.stats();
        assert_eq!(stats.point_count, 2);
        assert_eq!(stats.voxel_count, 2);
        assert_eq!(stats.instance_count, 2);
        assert_eq!(stats.viewport_size, [640, 480]);
        assert!(
            viewer.render_scene().instances()[0]
                .center
                .iter()
                .all(|coordinate| (*coordinate - 0.65).abs() < 1.0e-6)
        );
    }

    #[test]
    fn replaces_and_appends_owned_points_without_changing_the_dataset_contract() {
        let mut viewer = ViewerController::new(ViewerConfig::default()).unwrap();
        viewer.replace_points_owned(vec![Point::new([0.0; 3], ColorRgb::new(1, 2, 3))]);
        viewer
            .append_points_owned(vec![Point::new([1.0; 3], ColorRgb::new(4, 5, 6))])
            .unwrap();

        assert_eq!(viewer.stats().point_count, 2);
        assert_eq!(viewer.render_scene().instance_count(), 2);
    }

    #[test]
    fn rejects_invalid_position_components() {
        let mut viewer = ViewerController::new(ViewerConfig::default()).unwrap();

        let error = viewer
            .replace_points(&[0.0, 1.0], &[255, 255, 255])
            .unwrap_err();

        assert_eq!(error, ViewerError::InvalidPositionComponentCount(2));
    }

    #[test]
    fn rejects_invalid_color_components() {
        let mut viewer = ViewerController::new(ViewerConfig::default()).unwrap();

        let error = viewer
            .replace_points(&[0.0, 1.0, 2.0], &[255, 255])
            .unwrap_err();

        assert_eq!(
            error,
            ViewerError::InvalidColorComponentCount {
                actual: 2,
                point_count: 1,
            }
        );
    }

    #[test]
    fn failed_replacement_keeps_the_previous_scene() {
        let mut viewer = ViewerController::new(ViewerConfig::default()).unwrap();
        viewer
            .replace_points(&[0.0, 0.0, 0.0], &[12, 34, 56])
            .unwrap();

        assert!(matches!(
            viewer.replace_points(&[f32::NAN, 0.0, 0.0], &[1, 2, 3]),
            Err(ViewerError::InvalidPointPosition(_))
        ));
        assert_eq!(viewer.stats().point_count, 1);
        assert_eq!(viewer.stats().voxel_count, 1);
    }

    #[test]
    fn raw_points_render_as_voxels_and_explicit_resolution_can_reduce_density() {
        let mut viewer = ViewerController::new(ViewerConfig::default()).unwrap();
        let positions = [0.1, 0.1, 0.1, 0.2, 0.2, 0.2, 1.2, 0.1, 0.1];
        let colors = [255, 0, 0, 0, 255, 0, 0, 0, 255];

        viewer.replace_points(&positions, &colors).unwrap();
        assert_eq!(viewer.stats().point_count, 3);
        assert_eq!(viewer.stats().voxel_count, 2);
        assert_eq!(viewer.stats().instance_count, 2);

        viewer
            .replace_voxelized_points(&positions, &colors)
            .unwrap();
        assert_eq!(viewer.stats().point_count, 3);
        assert_eq!(viewer.stats().voxel_count, 2);
        assert_eq!(viewer.stats().instance_count, 2);
    }

    #[test]
    fn owned_voxel_appends_preserve_raw_and_voxelized_scenes_on_invalid_positions() {
        for voxelized in [false, true] {
            let mut viewer = ViewerController::new(ViewerConfig::default()).unwrap();
            let points = vec![Point::new([0.1; 3], ColorRgb::new(255, 0, 0))];
            if voxelized {
                viewer.replace_voxelized_points_owned(points).unwrap();
            } else {
                viewer.replace_points_owned(points);
            }
            let previous_scene = viewer.render_scene().clone();
            let previous_stats = viewer.stats();
            let invalid = vec![
                Point::new([1.1; 3], ColorRgb::new(0, 255, 0)),
                Point::new([f32::MAX; 3], ColorRgb::new(0, 0, 255)),
            ];
            assert!(viewer.append_voxelized_points_owned(invalid).is_err());
            assert_eq!(viewer.render_scene(), &previous_scene);
            assert_eq!(viewer.stats(), previous_stats);
            viewer
                .append_voxelized_points_owned(vec![Point::new([0.2; 3], ColorRgb::new(0, 255, 0))])
                .unwrap();
            assert_eq!(viewer.stats().point_count, 2);
            assert_eq!(viewer.stats().voxel_count, 1);
        }
    }

    #[test]
    fn appending_voxelized_points_converts_the_current_raw_data_atomically() {
        let mut viewer = ViewerController::new(ViewerConfig::default()).unwrap();
        viewer
            .replace_points(&[0.1, 0.1, 0.1], &[255, 0, 0])
            .unwrap();

        viewer
            .append_voxelized_points(&[0.2, 0.2, 0.2], &[0, 255, 0])
            .unwrap();
        assert_eq!(viewer.stats().point_count, 2);
        assert_eq!(viewer.stats().voxel_count, 1);
        assert_eq!(viewer.stats().instance_count, 1);

        let previous_scene = viewer.render_scene().clone();
        assert!(
            viewer
                .append_voxelized_points(&[f32::NAN, 0.0, 0.0], &[1, 2, 3])
                .is_err()
        );
        assert_eq!(viewer.render_scene(), &previous_scene);
    }
}
