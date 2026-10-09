mod block_sync;
mod camera;
mod gpu;
mod scene;

pub use camera::{CameraFrame, OrbitCamera};
pub use gpu::{BlockUploadStats, FrameStatus, GpuRendererError, SurfaceRenderer};
pub use scene::{
    InstanceRaw, RenderScene, RenderStats, SceneBounds, VOXEL_SHADER_WGSL, instance_buffer_layout,
};

use viewer_core::{Point, VisibleVoxelBatch};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RendererConfig {
    pub clear_color: wgpu::Color,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            clear_color: wgpu::Color {
                r: 0.04,
                g: 0.06,
                b: 0.08,
                a: 1.0,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct WgpuRenderer {
    config: RendererConfig,
    scene: RenderScene,
    viewport_size: [u32; 2],
    surface_format: Option<wgpu::TextureFormat>,
}

impl WgpuRenderer {
    pub fn new(config: RendererConfig) -> Self {
        Self {
            config,
            scene: RenderScene::default(),
            viewport_size: [1, 1],
            surface_format: None,
        }
    }

    pub fn config(&self) -> RendererConfig {
        self.config
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.viewport_size = [width.max(1), height.max(1)];
    }

    pub fn viewport_size(&self) -> [u32; 2] {
        self.viewport_size
    }

    pub fn set_surface_format(&mut self, surface_format: wgpu::TextureFormat) {
        self.surface_format = Some(surface_format);
    }

    pub fn surface_format(&self) -> Option<wgpu::TextureFormat> {
        self.surface_format
    }

    pub fn update_scene(&mut self, batch: &VisibleVoxelBatch) -> &RenderScene {
        self.scene = RenderScene::from_visible_batch(batch);
        &self.scene
    }

    pub fn update_points(&mut self, points: &[Point]) -> &RenderScene {
        self.scene = RenderScene::from_points(points);
        &self.scene
    }

    pub fn scene(&self) -> &RenderScene {
        &self.scene
    }
}
