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
mod node_cells;
#[cfg(test)]
mod node_cells_tests;
mod scene;
#[cfg(test)]
mod scene_digest_tests;
mod voxel_grid;

pub use budget::{DEFAULT_MAX_VISIBLE_VOXELS, GpuBudget};
pub use camera::{CameraFrame, ORBIT_CAMERA_F64_LEN, OrbitCamera, ViewProjection};
pub use gpu::{BlockUploadStats, FrameStatus, GpuRendererError, GpuResourceStats, SurfaceRenderer};
pub use node_cells::{
    CHAIN_SPAN, CellGrid, MAX_CELL_DEPTH, NodeCell, NodeCellError, NodeCellLevel, NodeCellSource,
    NodeCells, PointNodeCellSource, node_cells_digest, representative_depth,
};
pub use scene::{
    ColorMode, InstanceRaw, RenderScene, RenderStats, SceneBounds, VOXEL_SHADER_WGSL,
    instance_buffer_layout,
};
pub use voxel_grid::{
    VoxelBuildLimits, VoxelBuildMemoryStats, VoxelScene, VoxelSceneBuildError, VoxelSceneBuilder,
    build_voxel_scene, build_voxel_scene_with_limit, build_voxel_scene_with_limits,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RendererConfig {
    pub clear_color: wgpu::Color,
    pub gpu_budget: GpuBudget,
    /// Independent CPU construction budget; estimated bytes are not a process RSS limit.
    pub voxel_build_limits: VoxelBuildLimits,
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
            voxel_build_limits: VoxelBuildLimits::default(),
        }
    }
}

mod attribute_color;
pub use attribute_color::colorize_attribute;
