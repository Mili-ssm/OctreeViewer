use serde::Serialize;
use thiserror::Error;
use viewer_core::{
    ColorRgb, Point, PointBatch, SparseVoxelStore, VisibilityQuery, VoxelizationConfig,
    VoxelizationError,
};
use viewer_render_wgpu::{InstanceRaw, RenderScene, RendererConfig, WgpuRenderer};

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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RenderSceneSnapshot {
    pub instances: Vec<RenderInstanceSnapshot>,
}

impl From<&RenderScene> for RenderSceneSnapshot {
    fn from(scene: &RenderScene) -> Self {
        let instances = scene
            .instances()
            .iter()
            .map(|instance| RenderInstanceSnapshot::from(*instance))
            .collect();

        Self { instances }
    }
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
    renderer: WgpuRenderer,
}

#[derive(Debug, Clone)]
enum ViewerData {
    RawPoints(Vec<Point>),
    Voxelized(SparseVoxelStore),
}

impl ViewerController {
    pub fn new(config: ViewerConfig) -> Result<Self, ViewerError> {
        let voxel_config = VoxelizationConfig::new(config.voxel_size, config.origin)?;
        let mut renderer = WgpuRenderer::new(RendererConfig::default());
        renderer.resize(config.viewport_size[0], config.viewport_size[1]);

        Ok(Self {
            data: ViewerData::RawPoints(Vec::new()),
            voxel_config,
            renderer,
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
        let batch = build_point_batch(positions, colors)?;
        match &mut self.data {
            ViewerData::RawPoints(points) => points.extend(batch.into_points()),
            ViewerData::Voxelized(store) => store.insert_batch(&batch)?,
        }
        self.refresh_scene();
        Ok(())
    }

    /// Appends an owned point batch without retaining an intermediate copy.
    pub fn append_points_owned(&mut self, points: Vec<Point>) -> Result<(), ViewerError> {
        match &mut self.data {
            ViewerData::RawPoints(existing) => existing.extend(points),
            ViewerData::Voxelized(store) => store.insert_batch(&PointBatch::new(points))?,
        }
        self.refresh_scene();
        Ok(())
    }

    pub fn replace_voxelized_points(
        &mut self,
        positions: &[f32],
        colors: &[u8],
    ) -> Result<(), ViewerError> {
        let batch = build_point_batch(positions, colors)?;
        let mut replacement = SparseVoxelStore::new(self.voxel_config);
        replacement.insert_batch(&batch)?;
        self.data = ViewerData::Voxelized(replacement);
        self.refresh_scene();
        Ok(())
    }

    pub fn replace_voxelized_points_owned(
        &mut self,
        points: Vec<Point>,
    ) -> Result<(), ViewerError> {
        let mut replacement = SparseVoxelStore::new(self.voxel_config);
        replacement.insert_batch(&PointBatch::new(points))?;
        self.data = ViewerData::Voxelized(replacement);
        self.refresh_scene();
        Ok(())
    }

    pub fn append_voxelized_points(
        &mut self,
        positions: &[f32],
        colors: &[u8],
    ) -> Result<(), ViewerError> {
        let batch = build_point_batch(positions, colors)?;
        let mut replacement = match &self.data {
            ViewerData::RawPoints(points) => {
                let mut store = SparseVoxelStore::new(self.voxel_config);
                store.insert_batch(&PointBatch::new(points.clone()))?;
                store
            }
            ViewerData::Voxelized(store) => store.clone(),
        };
        replacement.insert_batch(&batch)?;
        self.data = ViewerData::Voxelized(replacement);
        self.refresh_scene();
        Ok(())
    }

    pub fn append_voxelized_points_owned(&mut self, points: Vec<Point>) -> Result<(), ViewerError> {
        let mut replacement = match &self.data {
            ViewerData::RawPoints(points) => {
                let mut store = SparseVoxelStore::new(self.voxel_config);
                store.insert_batch(&PointBatch::new(points.clone()))?;
                store
            }
            ViewerData::Voxelized(store) => store.clone(),
        };
        replacement.insert_batch(&PointBatch::new(points))?;
        self.data = ViewerData::Voxelized(replacement);
        self.refresh_scene();
        Ok(())
    }

    pub fn clear(&mut self) {
        self.data = ViewerData::RawPoints(Vec::new());
        self.refresh_scene();
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.renderer.resize(width, height);
    }

    pub fn render_scene(&self) -> &RenderScene {
        self.renderer.scene()
    }

    pub fn render_scene_snapshot(&self) -> RenderSceneSnapshot {
        RenderSceneSnapshot::from(self.renderer.scene())
    }

    pub fn stats(&self) -> ViewerStats {
        let scene = self.renderer.scene();
        let (point_count, voxel_count) = match &self.data {
            ViewerData::RawPoints(points) => (points.len() as u64, 0),
            ViewerData::Voxelized(store) => (store.point_count(), store.voxel_count()),
        };

        ViewerStats {
            point_count,
            voxel_count,
            instance_count: scene.instance_count(),
            viewport_size: self.renderer.viewport_size(),
        }
    }

    fn refresh_scene(&mut self) {
        match &self.data {
            ViewerData::RawPoints(points) => {
                self.renderer.update_points(points);
            }
            ViewerData::Voxelized(store) => {
                let batch = store.build_visible_batch(VisibilityQuery::default());
                self.renderer.update_scene(&batch);
            }
        }
    }
}

// Keep this layout validation and iteration compatible with the workspace's Rust 1.85 floor.
#[allow(clippy::manual_is_multiple_of, clippy::chunks_exact_to_as_chunks)]
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
        assert_eq!(stats.voxel_count, 0);
        assert_eq!(stats.instance_count, 2);
        assert_eq!(stats.viewport_size, [640, 480]);
        assert_eq!(viewer.render_scene().instances()[0].center, [0.1, 0.1, 0.1]);
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
        assert_eq!(viewer.stats().voxel_count, 0);
    }

    #[test]
    fn voxelization_is_an_explicit_density_reduction_path() {
        let mut viewer = ViewerController::new(ViewerConfig::default()).unwrap();
        let positions = [0.1, 0.1, 0.1, 0.2, 0.2, 0.2, 1.2, 0.1, 0.1];
        let colors = [255, 0, 0, 0, 255, 0, 0, 0, 255];

        viewer.replace_points(&positions, &colors).unwrap();
        assert_eq!(viewer.stats().point_count, 3);
        assert_eq!(viewer.stats().voxel_count, 0);
        assert_eq!(viewer.stats().instance_count, 3);

        viewer
            .replace_voxelized_points(&positions, &colors)
            .unwrap();
        assert_eq!(viewer.stats().point_count, 3);
        assert_eq!(viewer.stats().voxel_count, 2);
        assert_eq!(viewer.stats().instance_count, 2);
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
