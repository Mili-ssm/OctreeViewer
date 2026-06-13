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
    pub point_count: usize,
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
    #[error("positions length must be a multiple of 3, got {0}")]
    InvalidPositionComponentCount(usize),
    #[error("colors length must equal point count * 3, got {actual} for {point_count} points")]
    InvalidColorComponentCount { actual: usize, point_count: usize },
}

#[derive(Debug, Clone)]
pub struct ViewerController {
    store: SparseVoxelStore,
    renderer: WgpuRenderer,
}

impl ViewerController {
    pub fn new(config: ViewerConfig) -> Result<Self, ViewerError> {
        let voxel_config = VoxelizationConfig::new(config.voxel_size, config.origin)?;
        let mut renderer = WgpuRenderer::new(RendererConfig::default());
        renderer.resize(config.viewport_size[0], config.viewport_size[1]);

        Ok(Self {
            store: SparseVoxelStore::new(voxel_config),
            renderer,
        })
    }

    pub fn replace_points(&mut self, positions: &[f32], colors: &[u8]) -> Result<(), ViewerError> {
        self.store.clear();
        self.append_points(positions, colors)
    }

    pub fn append_points(&mut self, positions: &[f32], colors: &[u8]) -> Result<(), ViewerError> {
        let batch = build_point_batch(positions, colors)?;
        self.store.insert_batch(&batch);
        self.refresh_scene();
        Ok(())
    }

    pub fn clear(&mut self) {
        self.store.clear();
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

        ViewerStats {
            point_count: self.store.point_count(),
            voxel_count: self.store.voxel_count(),
            instance_count: scene.instance_count(),
            viewport_size: self.renderer.viewport_size(),
        }
    }

    fn refresh_scene(&mut self) {
        let batch = self.store.build_visible_batch(VisibilityQuery::default());
        self.renderer.update_scene(&batch);
    }
}

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
            Point::new(
                [position[0], position[1], position[2]],
                ColorRgb::new(color[0], color[1], color[2]),
            )
        })
        .collect();

    Ok(PointBatch::new(points))
}

#[cfg(test)]
mod tests {
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
}
