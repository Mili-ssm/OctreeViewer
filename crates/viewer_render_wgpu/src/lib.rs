#![forbid(unsafe_code)]
//! CPU scene construction, camera projection and budgeted WebGPU presentation.
//! `RenderScene` and `VoxelSceneBuilder` prepare geometry; `SurfaceRenderer` owns GPU state.

mod attributes;
mod block_sync;
mod budget;
mod camera;
#[doc(hidden)]
pub mod cell_bench;
mod cell_codec;
mod cell_cull;
mod cell_cut;
mod cell_driver;
#[doc(hidden)]
pub mod cell_lod;
mod cell_pages;
#[cfg(test)]
mod cell_pages_tests;
mod cell_pick;
mod cell_pool;
mod cell_render;
#[cfg(test)]
mod cell_render_tests;
mod frame_timer;
mod gpu;
#[cfg(not(target_arch = "wasm32"))]
mod headless;
mod hzb;
#[cfg(not(target_arch = "wasm32"))]
pub use headless::{OffscreenImage, render_offscreen};
mod node_cells;
#[cfg(test)]
mod node_cells_tests;
#[cfg(test)]
mod refinement_tests;
mod scene;
#[cfg(test)]
mod scene_digest_tests;
#[cfg(test)]
mod test_support;
mod voxel_grid;

pub use budget::{DEFAULT_MAX_VISIBLE_VOXELS, GpuBudget};
pub use camera::{CameraFrame, EyeFrame, ORBIT_CAMERA_F64_LEN, OrbitCamera, ViewProjection};
pub use cell_codec::{CODEC_VERSION, CodecError, decode_node_levels, encode_node_levels};
pub use cell_cut::{
    CellCutEntry, CellCutError, CellCutNode, cell_cut, cell_cut_digest, octant_ranges,
    octant_wanted_depth, resident_depth,
};
pub use cell_driver::{
    CellColorRanges, CellDriver, CellDriverError, CellTarget, DriverConfig, FramePhase,
    FrameReport, LevelsAdded, NodeAdded, NodeInput, NodeInputError, PackedNode, cell_generation,
    pack_node, packed_node_bytes,
};
pub use cell_pages::{
    CELL_PAGES_WGSL, CellInstance, DecodedCell, FLAG_SPANS_OCTANTS, MAX_INTENSITY, MAX_PAGE_EXTENT,
    MISSING_INTENSITY, PAGE_HAS_CLASS, PAGE_HAS_INTENSITY, PAGE_SLOTS, PackedLevel, PageEntry,
    PageFrame, PageInfo, decode_cell, pack_level, srgb_to_linear_table,
};
pub use cell_pool::{
    ActiveLevel, CellPool, CellPoolEvent, CutApply, DEFAULT_POOL_PAGES,
    DEFAULT_UPLOAD_BUDGET_BYTES, FramePlan, HIGH_PERCENT, LOW_PERCENT, LevelKey, LodMode,
    MaskUpdate, POOL_PAGE_BYTES, PageUpload, PoolConfig, PoolCounters, PoolNode, usable_pages,
};
pub use cell_render::{CellLimitMode, CellPoolError, MASK_WORDS, NODE_WORDS};
pub use frame_timer::GpuFrameStats;
pub use gpu::{
    BlockUploadStats, DeviceLoss, FrameStatus, GpuRendererError, GpuResourceStats, SurfaceRenderer,
};
pub use node_cells::{
    CHAIN_MIN_GROWTH, CHAIN_SPAN, CellGrid, MAX_CELL_DEPTH, NodeCell, NodeCellError, NodeCellLevel,
    NodeCellSource, NodeCells, PointNodeCellSource, chain_keep_mask, node_cell_level,
    node_cells_digest, representative_depth, representative_depth_for_spacing,
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
    /// Device limits for the cell pool; `DownlevelOnly` is for tests of the CPU-only fallback.
    pub cell_limits: CellLimitMode,
    /// Budget of the cell pool's own resources, which also bounds its size (ADR 0040); the pool is
    /// the largest that fits it and the device limits, else the CPU path draws.
    pub cell_pool_budget_bytes: u64,
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
            cell_limits: CellLimitMode::default(),
            cell_pool_budget_bytes: cell_render::DEFAULT_CELL_POOL_BUDGET_BYTES,
        }
    }
}

mod attribute_color;
pub use attribute_color::colorize_attribute;
