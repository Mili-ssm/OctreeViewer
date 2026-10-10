//! CPU scene construction, camera projection and budgeted WebGPU presentation.
//! `RenderScene` and `VoxelSceneBuilder` prepare geometry; `SurfaceRenderer` owns GPU state.

mod attributes;
mod block_sync;
mod budget;
mod camera;
mod gpu;
#[cfg(not(target_arch = "wasm32"))]
mod headless;
mod hzb;
#[cfg(not(target_arch = "wasm32"))]
pub use headless::{OffscreenImage, render_offscreen};
mod scene;
mod voxel_grid;

pub use budget::{DEFAULT_MAX_VISIBLE_VOXELS, GpuBudget};
pub use camera::{CameraFrame, OrbitCamera};
pub use gpu::{BlockUploadStats, FrameStatus, GpuRendererError, SurfaceRenderer};
pub use scene::{
    ColorMode, InstanceRaw, RenderScene, RenderStats, SceneBounds, VOXEL_SHADER_WGSL,
    instance_buffer_layout,
};
pub use voxel_grid::{
    VoxelScene, VoxelSceneBuildError, VoxelSceneBuilder, build_voxel_scene,
    build_voxel_scene_with_limit,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RendererConfig {
    pub clear_color: wgpu::Color,
    pub gpu_budget: GpuBudget,
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
            gpu_budget: GpuBudget::default(),
        }
    }
}
