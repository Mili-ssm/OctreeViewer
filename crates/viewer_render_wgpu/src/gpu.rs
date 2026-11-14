use std::num::NonZeroU64;
use std::{collections::HashMap, mem::size_of, sync::mpsc};

use bytemuck::{Pod, Zeroable};
use viewer_core::{BlockId, ResidentBlockSource};
use wgpu::util::DeviceExt;

use crate::{
    CameraFrame, CellColorRanges, CellPoolError, ColorMode, GpuBudget, InstanceRaw, OrbitCamera,
    RenderScene, RendererConfig, SceneBounds, VOXEL_SHADER_WGSL, VoxelBuildLimits,
    block_sync::ResidentBlockInfo,
    block_sync::plan_block_sync,
    budget::{buffer_capacity, check_peak_bytes, check_visible_instances},
    cell_driver::CellTarget,
    cell_pages::{PackedLevel, PageEntry, PageFrame},
    cell_pick::{PoolPick, prepare_pool_pick},
    cell_render::{
        CellColor, CellFrame, CellRenderer, MASK_WORDS, PageFrameStamp, cell_pool_pages,
        plan_device_limits, pool_resource_bytes,
    },
    instance_buffer_layout,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuRendererError {
    NoCompatibleAdapter,
    Offscreen(String),
    UnsupportedSurface,
    DeviceRequest(String),
    InvalidSceneSize,
    InvalidSceneData,
    InvalidVisualScale,
    InvalidGpuBudget,
    GpuBufferLimitExceeded {
        requested_bytes: u64,
        max_bytes: u64,
    },
    GpuVertexStrideLimitExceeded {
        required_bytes: u64,
        max_bytes: u64,
    },
    VisibleInstanceLimitExceeded {
        requested_instances: usize,
        max_instances: usize,
    },
    GpuBudgetExceeded {
        required_bytes: u64,
        max_bytes: u64,
    },
    InvalidBlockData(BlockId),
    MissingBlockPayload(BlockId),
    PipelineValidation(String),
    SurfaceAcquisition(String),
    CellPool(CellPoolError),
    /// The `PageFrame` of the page entries uses another coordinate origin or visual scale.
    PageFrameMismatch,
}

impl std::fmt::Display for GpuRendererError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offscreen(error) => write!(f, "offscreen rendering failed: {error}"),
            Self::NoCompatibleAdapter => f.write_str("no GPU adapter supports this surface"),
            Self::UnsupportedSurface => f.write_str("surface has no usable presentation format"),
            Self::DeviceRequest(error) => write!(f, "GPU device request failed: {error}"),
            Self::InvalidSceneSize => f.write_str("scene exceeds supported GPU buffer limits"),
            Self::InvalidSceneData => f.write_str("scene contains invalid render data"),
            Self::InvalidVisualScale => {
                f.write_str("visual scale must be positive, finite, and representable as f32")
            }
            Self::InvalidGpuBudget => f.write_str(
                "GPU budget must allow at least one instance and a nonzero instance count",
            ),
            Self::GpuBufferLimitExceeded {
                requested_bytes,
                max_bytes,
            } => write!(
                f,
                "GPU buffer requires {requested_bytes} bytes, device limit is {max_bytes}"
            ),
            Self::GpuVertexStrideLimitExceeded {
                required_bytes,
                max_bytes,
            } => write!(
                f,
                "instance vertex stride requires {required_bytes} bytes, device limit is {max_bytes}"
            ),
            Self::VisibleInstanceLimitExceeded {
                requested_instances,
                max_instances,
            } => write!(
                f,
                "visible selection requires {requested_instances} instances, configured limit is {max_instances}"
            ),
            Self::GpuBudgetExceeded {
                required_bytes,
                max_bytes,
            } => write!(
                f,
                "atomic GPU buffer update requires a peak of {required_bytes} bytes, configured budget is {max_bytes}"
            ),
            Self::InvalidBlockData(id) => write!(f, "block {id:?} contains invalid render data"),
            Self::MissingBlockPayload(id) => {
                write!(f, "block {id:?} is not resident in the CPU cache")
            }
            Self::PipelineValidation(error) => write!(f, "GPU pipeline is invalid: {error}"),
            Self::SurfaceAcquisition(error) => write!(f, "GPU surface acquisition failed: {error}"),
            Self::CellPool(error) => write!(f, "cell pool: {error}"),
            Self::PageFrameMismatch => f.write_str(
                "page frame does not match the renderer's coordinate origin and visual scale",
            ),
        }
    }
}

impl std::error::Error for GpuRendererError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameStatus {
    Presented,
    SkippedTimeout,
    Reconfigured,
    Suspended,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct ViewProjection {
    pub(crate) matrix: [[f32; 4]; 4],
    pub(crate) render_origin: [f32; 4],
    pub(crate) style: [f32; 4],
    pub(crate) intensity_range: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CubeVertex {
    position: [f32; 3],
    normal: [f32; 3],
}

struct GpuInstanceBuffer {
    buffer: wgpu::Buffer,
    capacity: u64,
    instance_count: u32,
    bounds: Option<SceneBounds>,
    intensity_range: Option<[f32; 2]>,
    elevation_range: Option<[f32; 2]>,
}

const PICK_CHUNK_INSTANCES: usize = 256;
pub(crate) const PICK_MAX_CANDIDATE_INSTANCES: usize = 65_536;
const MAX_FRAMEBUFFER_PIXELS: u64 = 4_194_304;
const MAX_FRAMEBUFFER_DIMENSION: u32 = 4_096;

#[derive(Debug, Clone, Copy)]
pub(crate) struct PickChunk {
    pub(crate) first_instance: usize,
    pub(crate) instance_count: usize,
    pub(crate) bounds: SceneBounds,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PickCopy {
    pub(crate) staging_offset: u64,
    pub(crate) first_instance: usize,
    pub(crate) instance_count: usize,
}

/// Staging copy of a CPU-path pick.
pub struct CpuPick {
    buffer: wgpu::Buffer,
    copies: Vec<PickCopy>,
    ray: PickRay,
    /// The units of the scene at `prepare_pick`: the host awaits the mapping before `resolve_pick`.
    visual_scale: f64,
    coordinate_origin: [f64; 3],
}

/// What `prepare_pick` copied; `resolve_pick` decides by the variant, not by the renderer's current
/// mode, because the host awaits the mapping between the two calls.
pub enum PickReadback {
    Cpu(CpuPick),
    Pool(PoolPick),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PickRay {
    pub(crate) origin: glam::DVec3,
    pub(crate) direction: glam::DVec3,
    pub(crate) near_distance: f64,
    pub(crate) far_distance: f64,
}

impl PickReadback {
    pub fn buffer(&self) -> &wgpu::Buffer {
        match self {
            Self::Cpu(pick) => &pick.buffer,
            Self::Pool(pick) => pick.buffer(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickHit {
    pub instance_index: usize,
    pub position: [f64; 3],
    pub voxel_center: [f64; 3],
    pub voxel_size: f64,
    pub distance: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockUploadStats {
    pub visible_block_count: usize,
    pub visible_instance_count: usize,
    pub uploaded_block_count: usize,
    pub removed_block_count: usize,
    pub uploaded_bytes: u64,
}

/// Requested sizes of currently owned GPU resources, not physical VRAM residency.
/// Textures use logical texel bytes; driver padding, swapchain, staging and pipelines are excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GpuResourceStats {
    pub instance_buffer_bytes: u64,
    pub fixed_buffer_bytes: u64,
    pub hzb_buffer_bytes: u64,
    pub depth_texture_bytes: u64,
    pub hzb_texture_bytes: u64,
    /// Buffers of the cell pool; 0 while the pool does not exist (it is created lazily).
    pub cell_pool_bytes: u64,
}

impl GpuResourceStats {
    pub fn total_bytes(self) -> u64 {
        self.instance_buffer_bytes
            + self.fixed_buffer_bytes
            + self.hzb_buffer_bytes
            + self.depth_texture_bytes
            + self.hzb_texture_bytes
            + self.cell_pool_bytes
    }
}

const CUBE_VERTICES: [CubeVertex; 8] = [
    CubeVertex {
        position: [-0.5, -0.5, -0.5],
        normal: [-1.0, -1.0, -1.0],
    },
    CubeVertex {
        position: [0.5, -0.5, -0.5],
        normal: [1.0, -1.0, -1.0],
    },
    CubeVertex {
        position: [0.5, 0.5, -0.5],
        normal: [1.0, 1.0, -1.0],
    },
    CubeVertex {
        position: [-0.5, 0.5, -0.5],
        normal: [-1.0, 1.0, -1.0],
    },
    CubeVertex {
        position: [-0.5, -0.5, 0.5],
        normal: [-1.0, -1.0, 1.0],
    },
    CubeVertex {
        position: [0.5, -0.5, 0.5],
        normal: [1.0, -1.0, 1.0],
    },
    CubeVertex {
        position: [0.5, 0.5, 0.5],
        normal: [1.0, 1.0, 1.0],
    },
    CubeVertex {
        position: [-0.5, 0.5, 0.5],
        normal: [-1.0, 1.0, 1.0],
    },
];

pub(crate) const CUBE_INDICES: [u16; 36] = [
    0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 2, 3, 7, 2, 7, 6, 1, 2, 6, 1, 6, 5, 3, 0,
    4, 3, 4, 7,
];

/// The pool is allocated alone: the CPU scene is released once the first level is uploaded, so
/// only the pool counts against the budget.
fn check_pool_creation_budget(pool_bytes: u64, max_bytes: u64) -> Result<(), GpuRendererError> {
    check_peak_bytes(0, pool_bytes, max_bytes).map_err(|_| {
        GpuRendererError::CellPool(CellPoolError::BudgetExceeded {
            required_bytes: pool_bytes,
            max_bytes,
        })
    })
}

/// Runs `upload` on the pool, creating it first when absent. If that call created the pool and
/// fails, the pool is dropped. Returns whether this call created the pool.
fn upload_creating_pool<P, E>(
    slot: &mut Option<P>,
    create: impl FnOnce() -> P,
    upload: impl FnOnce(&mut P) -> Result<(), E>,
) -> Result<bool, E> {
    let created = slot.is_none();
    let pool = slot.get_or_insert_with(create);
    if let Err(error) = upload(pool) {
        if created {
            *slot = None;
        }
        return Err(error);
    }
    Ok(created)
}

/// GPU renderer for a surface owned by the calling application.
pub struct SurfaceRenderer<'window> {
    adapter_info: wgpu::AdapterInfo,
    surface: wgpu::Surface<'window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    depth_texture: wgpu::Texture,
    hzb: Option<crate::hzb::HzbResources>,
    hzb_depth_supported: bool,
    hzb_reference_camera: Option<([[f32; 4]; 4], [f32; 4])>,
    hzb_scene_opaque: bool,
    hzb_count_readback: Option<wgpu::Buffer>,
    hzb_count_receiver: Option<mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
    frame_timer: Option<crate::frame_timer::FrameTimer>,
    /// Limits of the CPU path: `downlevel_defaults` with the adapter's resolution. Every CPU size
    /// and HZB decision uses these, not the (possibly raised) limits of the device.
    cpu_limits: wgpu::Limits,
    /// Pages of the pool this device runs (ADR 0040); `None`: the CPU path only.
    cell_pool_pages: Option<u32>,
    cell_pool_budget_bytes: u64,
    cell_pool: Option<CellRenderer>,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    scene_buffer: Option<GpuInstanceBuffer>,
    block_buffers: HashMap<BlockId, GpuInstanceBuffer>,
    visible_block_ids: Vec<BlockId>,
    visible_instance_count: usize,
    pick_chunks: Vec<PickChunk>,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    clear_color: wgpu::Color,
    uploaded_bytes: u64,
    suspended: bool,
    camera: OrbitCamera,
    scene_bounds: Option<SceneBounds>,
    gpu_budget: GpuBudget,
    voxel_build_limits: VoxelBuildLimits,
    intensity_range: Option<[f32; 2]>,
    elevation_range: Option<[f32; 2]>,
    color_mode: ColorMode,
    /// Ranges of the cut the cells draw, for the shader colour modes (`CellTarget::set_color_ranges`).
    cell_color_ranges: CellColorRanges,
    depth_shading: bool,
    visual_scale: f64,
    coordinate_origin: [f64; 3],
}

impl<'window> SurfaceRenderer<'window> {
    /// Initializes GPU resources for the supplied host-created surface.
    /// The instance used to create that surface must be passed here as well.
    pub async fn new(
        instance: &wgpu::Instance,
        surface: wgpu::Surface<'window>,
        config: RendererConfig,
        width: u32,
        height: u32,
    ) -> Result<Self, GpuRendererError> {
        let minimum_instance_capacity = (size_of::<InstanceRaw>() as u64)
            .checked_next_power_of_two()
            .ok_or(GpuRendererError::InvalidGpuBudget)?;
        if config.gpu_budget.max_visible_instances == 0
            || config.gpu_budget.max_bytes < minimum_instance_capacity
        {
            return Err(GpuRendererError::InvalidGpuBudget);
        }
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|_| GpuRendererError::NoCompatibleAdapter)?;

        let limits = plan_device_limits(&adapter, config.cell_limits);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("octree-viewer-device"),
                // Frame timing is optional: never fail device creation without it.
                required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
                required_limits: limits.required_limits,
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            })
            .await
            .map_err(|error| GpuRendererError::DeviceRequest(error.to_string()))?;

        let cell_pool_pages =
            cell_pool_pages(&adapter, &device.limits(), config.cell_pool_budget_bytes);
        let device_limits = limits.cpu_limits;
        let [width, height] =
            bounded_framebuffer_size(width, height, device_limits.max_texture_dimension_2d);
        let instance_stride = size_of::<InstanceRaw>() as u64;
        if instance_stride > device_limits.max_buffer_size {
            return Err(GpuRendererError::GpuBufferLimitExceeded {
                requested_bytes: instance_stride,
                max_bytes: device_limits.max_buffer_size,
            });
        }
        if instance_stride > u64::from(device_limits.max_vertex_buffer_array_stride) {
            return Err(GpuRendererError::GpuVertexStrideLimitExceeded {
                required_bytes: instance_stride,
                max_bytes: u64::from(device_limits.max_vertex_buffer_array_stride),
            });
        }

        let (surface_config, pipeline_format) = surface_config(&surface, &adapter, width, height)?;
        let PipelineResources {
            pipeline,
            uniform_buffer,
            uniform_bind_group,
            vertex_buffer,
            index_buffer,
        } = create_pipeline_resources(&device, pipeline_format).await?;
        let hzb_depth_supported = adapter
            .get_texture_format_features(wgpu::TextureFormat::Depth32Float)
            .allowed_usages
            .contains(wgpu::TextureUsages::TEXTURE_BINDING);
        let depth_texture = create_depth_texture(&device, &surface_config, hzb_depth_supported);
        let hzb_estimated_bytes =
            crate::hzb::estimated_pyramid_bytes([surface_config.width, surface_config.height]);
        let hzb = if hzb_estimated_bytes <= config.gpu_budget.max_bytes / 2 {
            crate::hzb::HzbResources::new(
                &device,
                &adapter,
                &uniform_buffer,
                &depth_texture,
                surface_config.format,
                [surface_config.width, surface_config.height],
                device_limits.max_texture_dimension_2d,
            )
            .await
            .ok()
            .flatten()
        } else {
            None
        };
        let hzb_count_readback = hzb.as_ref().map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("octree-viewer-hzb-count-readback"),
                size: size_of::<u32>() as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        surface.configure(&device, &surface_config);
        let frame_timer = crate::frame_timer::FrameTimer::new(&device, &queue);

        Ok(Self {
            adapter_info: adapter.get_info(),
            surface,
            device,
            queue,
            surface_config,
            pipeline,
            depth_texture,
            hzb,
            hzb_depth_supported,
            hzb_reference_camera: None,
            hzb_scene_opaque: false,
            hzb_count_readback,
            hzb_count_receiver: None,
            frame_timer,
            cpu_limits: device_limits,
            cell_pool_pages,
            cell_pool_budget_bytes: config.cell_pool_budget_bytes,
            cell_pool: None,
            vertex_buffer,
            index_buffer,
            scene_buffer: None,
            block_buffers: HashMap::new(),
            visible_block_ids: Vec::new(),
            visible_instance_count: 0,
            pick_chunks: Vec::new(),
            uniform_buffer,
            uniform_bind_group,
            clear_color: config.clear_color,
            uploaded_bytes: 0,
            suspended: width == 0 || height == 0,
            camera: OrbitCamera::default(),
            scene_bounds: None,
            gpu_budget: config.gpu_budget,
            voxel_build_limits: config.voxel_build_limits,
            intensity_range: None,
            elevation_range: None,
            color_mode: ColorMode::Rgb,
            cell_color_ranges: CellColorRanges::default(),
            depth_shading: true,
            visual_scale: 1.0,
            coordinate_origin: [0.0; 3],
        })
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_config.format
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    /// Uploads one scene and returns the bytes written. Existing GPU storage is reused when it fits.
    pub fn upload_scene(&mut self, scene: &RenderScene) -> Result<u64, GpuRendererError> {
        let (bytes, count, bounds) = scene_upload_metadata(
            scene,
            self.cpu_limits.max_buffer_size,
            self.gpu_budget.max_visible_instances,
        )?;
        let current_capacity = self
            .scene_buffer
            .as_ref()
            .map_or(0, |buffer| buffer.capacity);
        let target_capacity = buffer_capacity(
            bytes,
            size_of::<InstanceRaw>() as u64,
            self.cpu_limits.max_buffer_size,
        )?;
        let added_capacity = if target_capacity > current_capacity {
            target_capacity
        } else {
            0
        };
        // The pool is freed when the CPU path takes over, so it does not count here.
        check_peak_bytes(
            self.allocated_cpu_bytes()?,
            added_capacity,
            self.gpu_budget.max_bytes,
        )?;
        if bytes > current_capacity {
            self.scene_buffer = Some(create_instance_buffer(
                &self.device,
                &self.queue,
                scene,
                bytes,
                count,
                bounds,
                self.cpu_limits.max_buffer_size,
            )?);
        } else if let Some(buffer) = &mut self.scene_buffer {
            if bytes > 0 {
                self.queue
                    .write_buffer(&buffer.buffer, 0, bytemuck::cast_slice(scene.instances()));
            }
            buffer.instance_count = count;
            buffer.bounds = bounds;
            buffer.intensity_range = scene.intensity_range();
            buffer.elevation_range = scene.elevation_range();
        }
        if self.scene_buffer.is_none() {
            self.scene_buffer = Some(create_instance_buffer(
                &self.device,
                &self.queue,
                scene,
                bytes,
                count,
                bounds,
                self.cpu_limits.max_buffer_size,
            )?);
        }
        self.block_buffers.clear();
        self.visible_block_ids.clear();
        self.cell_pool = None;
        self.hzb_reference_camera = None;
        self.visible_instance_count = scene.instance_count();
        self.pick_chunks = build_pick_chunks(scene.instances(), self.visual_scale);
        self.uploaded_bytes = self.uploaded_bytes.saturating_add(bytes);
        self.scene_bounds = bounds;
        self.intensity_range = scene.intensity_range();
        self.elevation_range = scene.elevation_range();
        self.hzb_scene_opaque = scene
            .instances()
            .iter()
            .all(|instance| instance.color[3] >= 1.0);
        self.prepare_hzb_scene(bytes);
        Ok(bytes)
    }

    /// Uploads a voxelized LOD cut while retaining its source block IDs for diagnostics.
    pub fn upload_voxel_cut(
        &mut self,
        scene: &RenderScene,
        block_ids: &[BlockId],
    ) -> Result<u64, GpuRendererError> {
        let bytes = self.upload_scene(scene)?;
        self.visible_block_ids = block_ids.to_vec();
        Ok(bytes)
    }

    /// Synchronizes GPU residency to a deduplicated renderable block cut.
    /// IDs are dataset-local; call `clear_dataset` before using IDs from another dataset.
    /// The previous cut remains active if validation fails.
    pub fn sync_visible_blocks(
        &mut self,
        block_ids: &[BlockId],
        cache: &dyn ResidentBlockSource,
    ) -> Result<BlockUploadStats, GpuRendererError> {
        let resident: Vec<_> = self
            .block_buffers
            .iter()
            .map(|(id, buffer)| ResidentBlockInfo {
                id: *id,
                instance_count: buffer.instance_count as usize,
                capacity: buffer.capacity,
                bounds: buffer.bounds,
                intensity_range: buffer.intensity_range,
                elevation_range: buffer.elevation_range,
            })
            .collect();
        let plan = plan_block_sync(
            &resident,
            block_ids,
            cache,
            self.cpu_limits.max_buffer_size,
            self.gpu_budget,
            self.scene_buffer
                .as_ref()
                .map_or(0, |buffer| buffer.capacity),
        )?;
        let mut additions = HashMap::with_capacity(plan.added.len());
        for (id, scene) in &plan.added {
            let (bytes, count, bounds) = scene_upload_metadata(
                scene,
                self.cpu_limits.max_buffer_size,
                self.gpu_budget.max_visible_instances,
            )?;
            additions.insert(
                *id,
                create_instance_buffer(
                    &self.device,
                    &self.queue,
                    scene,
                    bytes,
                    count,
                    bounds,
                    self.cpu_limits.max_buffer_size,
                )?,
            );
        }

        self.block_buffers
            .retain(|id, _| plan.selected.binary_search(id).is_ok());
        self.block_buffers.extend(additions);
        self.visible_block_ids = plan.selected;
        self.visible_instance_count = plan.visible_instance_count;
        self.scene_bounds = plan.visible_bounds;
        self.intensity_range = plan.visible_intensity_range;
        self.elevation_range = plan.visible_elevation_range;
        self.scene_buffer = None;
        self.cell_pool = None;
        self.pick_chunks.clear();
        self.hzb_reference_camera = None;
        self.hzb_scene_opaque = false;
        if let Some(hzb) = &mut self.hzb {
            hzb.reset_scene();
        }
        self.uploaded_bytes = self.uploaded_bytes.saturating_add(plan.uploaded_bytes);

        Ok(BlockUploadStats {
            visible_block_count: self.visible_block_ids.len(),
            visible_instance_count: self.visible_instance_count,
            uploaded_block_count: plan.added.len(),
            removed_block_count: plan.removed.len(),
            uploaded_bytes: plan.uploaded_bytes,
        })
    }

    fn allocated_gpu_bytes(&self) -> Result<u64, GpuRendererError> {
        let scene_capacity = self
            .scene_buffer
            .as_ref()
            .map_or(0, |buffer| buffer.capacity);
        let scene_and_blocks =
            self.block_buffers
                .values()
                .try_fold(scene_capacity, |sum, buffer| {
                    sum.checked_add(buffer.capacity)
                        .ok_or(GpuRendererError::InvalidSceneSize)
                })?;
        scene_and_blocks
            .checked_add(self.cell_pool_bytes())
            .and_then(|bytes| {
                bytes.checked_add(self.hzb.as_ref().map_or(0, |hzb| hzb.resource_bytes()))
            })
            .and_then(|bytes| bytes.checked_add(self.hzb_count_readback.as_ref().map_or(0, |_| 4)))
            .ok_or(GpuRendererError::InvalidSceneSize)
    }

    /// Bytes the CPU path accounts for: everything but the cell pool, which is freed when the CPU
    /// path takes over and has its own creation check.
    fn allocated_cpu_bytes(&self) -> Result<u64, GpuRendererError> {
        Ok(self.allocated_gpu_bytes()? - self.cell_pool_bytes())
    }

    fn cell_pool_bytes(&self) -> u64 {
        self.cell_pool
            .as_ref()
            .map_or(0, CellRenderer::resource_bytes)
    }

    /// Whether the device meets the limits and features of the cell pool (R3-Q5). When false, only
    /// the CPU path is available and the cell methods return `CellPoolError::Unsupported`.
    pub fn cells_supported(&self) -> bool {
        self.cell_pool_pages.is_some()
    }

    /// Pages of the cell pool this device runs: 65,536 (512 MiB) or what its limits admit, at
    /// least 32,768 (ADR 0040). `None` when only the CPU path is available. The driver's
    /// `PoolConfig::capacity_pages` must be this value.
    pub fn cell_pool_pages(&self) -> Option<u32> {
        self.cell_pool_pages
    }

    /// Whether the cell pool exists, which is when `render` draws cells instead of the CPU scene.
    pub fn cells_active(&self) -> bool {
        self.cell_pool.is_some()
    }

    /// The camera eye in scaled local units, in `f64`: `PageFrame::eye_scaled` for the page
    /// entries of this frame.
    pub fn cell_eye(&self) -> [f64; 3] {
        self.camera.position()
    }

    /// Writes a packed level into the given page slots. The first call creates the pool and, once
    /// the upload succeeds, releases the CPU scene: the CPU path and the cells path are exclusive.
    /// If that first call fails, the new pool is dropped and the renderer stays as it was. The pool
    /// is freed by `upload_scene`, `upload_voxel_cut`, `sync_visible_blocks`, `clear_dataset` and
    /// `clear_cells`.
    pub fn upload_cell_level(
        &mut self,
        level: &PackedLevel,
        page_slots: &[u32],
    ) -> Result<(), GpuRendererError> {
        let Some(pool_pages) = self.cell_pool_pages else {
            return Err(GpuRendererError::CellPool(CellPoolError::Unsupported));
        };
        if self.cell_pool.is_none() {
            // The pages were chosen to fit this budget; the check guards the invariant.
            check_pool_creation_budget(
                pool_resource_bytes(pool_pages),
                self.cell_pool_budget_bytes,
            )?;
        }
        let device = &self.device;
        let queue = &self.queue;
        let format = self.surface_config.format;
        let created = upload_creating_pool(
            &mut self.cell_pool,
            || CellRenderer::new(device, queue, format, pool_pages),
            |pool| pool.upload_level(level, page_slots),
        )
        .map_err(GpuRendererError::CellPool)?;
        if created {
            self.release_cpu_scene();
        }
        Ok(())
    }

    /// Frees what only the CPU path draws from, so that in cells mode only the pool (plus the
    /// shared depth texture and uniforms) is resident. Bounds, ranges and the camera are kept.
    fn release_cpu_scene(&mut self) {
        self.scene_buffer = None;
        self.block_buffers.clear();
        self.visible_block_ids.clear();
        self.visible_instance_count = 0;
        self.pick_chunks.clear();
        self.hzb_reference_camera = None;
        self.hzb_scene_opaque = false;
        self.hzb_count_receiver = None;
        if let Some(hzb) = &mut self.hzb {
            hzb.reset_scene();
        }
    }

    fn existing_cell_pool(&mut self) -> Result<&mut CellRenderer, GpuRendererError> {
        self.cell_pool
            .as_mut()
            .ok_or(GpuRendererError::CellPool(CellPoolError::NoPool))
    }

    /// Makes the pages inert; do this before uploading other levels into the same slots.
    pub fn free_cell_pages(&mut self, page_slots: &[u32]) -> Result<(), GpuRendererError> {
        match &mut self.cell_pool {
            Some(pool) => pool
                .free_pages(page_slots)
                .map_err(GpuRendererError::CellPool),
            None => Err(GpuRendererError::CellPool(CellPoolError::NoPool)),
        }
    }

    pub fn alloc_cell_mask_slot(&mut self) -> Result<Option<u32>, GpuRendererError> {
        Ok(self.existing_cell_pool()?.alloc_mask_slot())
    }

    pub fn free_cell_mask_slot(&mut self, slot: u32) -> Result<(), GpuRendererError> {
        match &mut self.cell_pool {
            Some(pool) => pool
                .free_mask_slot(slot)
                .map_err(GpuRendererError::CellPool),
            None => Err(GpuRendererError::CellPool(CellPoolError::NoPool)),
        }
    }

    /// Bit `x + 8y + 64z` set: that cell of the node is replaced by finer ones and not drawn.
    pub fn set_cell_node_mask(
        &mut self,
        slot: u32,
        mask: &[u32; MASK_WORDS],
    ) -> Result<(), GpuRendererError> {
        self.existing_cell_pool()?
            .set_node_mask(slot, mask)
            .map_err(GpuRendererError::CellPool)
    }

    /// The pages to draw this frame, with eye-relative origins and octant bits (see
    /// `PageInfo::entry` and `cell_eye`). `frame` is the one the entries were built with: its
    /// coordinate origin and visual scale must be the renderer's, and its eye is kept for picking.
    pub fn set_active_cell_pages(
        &mut self,
        pages: &[(u32, PageEntry)],
        frame: &PageFrame,
    ) -> Result<(), GpuRendererError> {
        let (visual_scale, coordinate_origin) = (self.visual_scale, self.coordinate_origin);
        let pool = self.existing_cell_pool()?;
        let stamp = PageFrameStamp::of(frame);
        if !stamp.same_units(visual_scale, coordinate_origin) {
            return Err(GpuRendererError::PageFrameMismatch);
        }
        pool.set_active_pages(pages)
            .map_err(GpuRendererError::CellPool)?;
        pool.set_page_frame(stamp);
        Ok(())
    }

    /// Frees the pool; `render` draws the CPU scene again.
    pub fn clear_cells(&mut self) {
        self.cell_pool = None;
    }

    fn prepare_hzb_scene(&mut self, source_bytes: u64) {
        self.hzb_reference_camera = None;
        let allocated_without_hzb = self
            .allocated_cpu_bytes()
            .unwrap_or(self.gpu_budget.max_bytes)
            .saturating_sub(self.hzb.as_ref().map_or(0, |hzb| hzb.resource_bytes()));
        let available = self
            .gpu_budget
            .max_bytes
            .saturating_sub(allocated_without_hzb);
        let Some(hzb) = &mut self.hzb else {
            return;
        };
        let opaque = self.hzb_scene_opaque;
        let Some(buffer) = &self.scene_buffer else {
            hzb.reset_scene();
            return;
        };
        if !opaque
            || buffer.instance_count < crate::hzb::MIN_OCCLUSION_CANDIDATES as u32
            || !hzb.can_prepare_scene(
                &self.cpu_limits,
                source_bytes,
                buffer.instance_count as usize,
                available,
            )
        {
            hzb.reset_scene();
            return;
        }
        hzb.prepare_scene(
            &self.device,
            &buffer.buffer,
            source_bytes,
            buffer.instance_count as usize,
        );
    }

    /// Drops scene and block GPU data before opening another dataset.
    pub fn clear_dataset(&mut self) {
        self.scene_buffer = None;
        self.cell_pool = None;
        self.cell_color_ranges = CellColorRanges::default();
        self.hzb_reference_camera = None;
        self.hzb_scene_opaque = false;
        self.block_buffers.clear();
        self.visible_block_ids.clear();
        self.visible_instance_count = 0;
        self.pick_chunks.clear();
        self.scene_bounds = None;
        self.intensity_range = None;
        self.elevation_range = None;
        self.uploaded_bytes = 0;
        self.visual_scale = 1.0;
        if let Some(hzb) = &mut self.hzb {
            hzb.reset_scene();
        }
    }

    pub fn visible_block_count(&self) -> usize {
        self.visible_block_ids.len()
    }

    pub fn visible_instance_count(&self) -> usize {
        self.visible_instance_count
    }

    pub fn hzb_active(&self) -> bool {
        let Some(frame) = self.camera.frame(self.viewport_size()) else {
            return false;
        };
        let mut render_origin = frame.render_origin;
        render_origin[3] = self.visual_scale as f32;
        self.hzb_reference_camera == Some((frame.view_projection, render_origin))
            && self
                .hzb
                .as_ref()
                .is_some_and(|hzb| hzb.indexed_bind_group().is_some())
    }

    fn update_hzb_visible_count(&mut self) {
        let Some(receiver) = &self.hzb_count_receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(())) => {
                self.hzb_count_receiver = None;
                if let Some(buffer) = &self.hzb_count_readback {
                    if let Ok(view) = buffer.slice(..).get_mapped_range() {
                        self.visible_instance_count = *bytemuck::from_bytes::<u32>(&view) as usize;
                    }
                    buffer.unmap();
                }
            }
            Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                self.hzb_count_receiver = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    /// Percentiles of the GPU time per frame over the last samples; `None` when the device has no
    /// `timestamp-query` or nothing was sampled yet.
    pub fn gpu_frame_stats(&self) -> Option<crate::GpuFrameStats> {
        self.frame_timer.as_ref().and_then(|timer| timer.stats())
    }

    /// Turns the per-frame GPU timing on or off (off by default; no cost while off). A no-op
    /// without `timestamp-query`.
    pub fn set_gpu_frame_timing(&mut self, enabled: bool) {
        if let Some(timer) = &mut self.frame_timer {
            timer.set_enabled(enabled);
        }
    }

    /// Starts a new sampling window for `gpu_frame_stats`.
    pub fn reset_gpu_frame_stats(&mut self) {
        if let Some(timer) = &mut self.frame_timer {
            timer.reset();
        }
    }

    pub fn max_visible_instances(&self) -> usize {
        self.gpu_budget.max_visible_instances
    }

    pub fn voxel_build_limits(&self) -> VoxelBuildLimits {
        VoxelBuildLimits {
            max_voxels: self
                .voxel_build_limits
                .max_voxels
                .min(self.gpu_budget.max_visible_instances),
            ..self.voxel_build_limits
        }
    }

    pub fn set_color_mode(&mut self, mode: ColorMode) {
        self.color_mode = mode;
    }

    pub fn color_mode(&self) -> ColorMode {
        self.color_mode
    }

    pub fn set_visual_scale(&mut self, scale: f64) -> Result<(), GpuRendererError> {
        if !scale.is_finite() || scale <= 0.0 || !(scale as f32).is_finite() || scale as f32 == 0.0
        {
            return Err(GpuRendererError::InvalidVisualScale);
        }
        self.visual_scale = scale;
        self.hzb_reference_camera = None;
        Ok(())
    }

    pub fn set_depth_shading(&mut self, enabled: bool) {
        self.depth_shading = enabled;
    }

    pub fn fit_scene(&mut self, scene: &RenderScene) -> bool {
        self.coordinate_origin = [0.0; 3];
        self.scene_bounds = scene.bounds();
        self.display_bounds().is_some_and(|bounds| {
            self.camera.fit(
                bounds,
                [self.surface_config.width, self.surface_config.height],
            )
        })
    }

    fn display_bounds(&self) -> Option<SceneBounds> {
        let bounds = self.scene_bounds?;
        let min = bounds.min.map(|value| value * self.visual_scale);
        let max = bounds.max.map(|value| value * self.visual_scale);
        if min.iter().chain(&max).all(|value| value.is_finite()) {
            Some(SceneBounds { min, max })
        } else {
            None
        }
    }

    pub fn fit_visible_blocks(&mut self) -> bool {
        self.display_bounds().is_some_and(|bounds| {
            self.camera.fit(
                bounds,
                [self.surface_config.width, self.surface_config.height],
            )
        })
    }

    pub fn fit_spatial_bounds(
        &mut self,
        bounds: viewer_core::SpatialBounds,
        coordinate_origin: [f64; 3],
    ) -> bool {
        let local = SceneBounds {
            min: std::array::from_fn(|axis| bounds.min()[axis] - coordinate_origin[axis]),
            max: std::array::from_fn(|axis| bounds.max()[axis] - coordinate_origin[axis]),
        };
        let display = SceneBounds {
            min: local.min.map(|value| value * self.visual_scale),
            max: local.max.map(|value| value * self.visual_scale),
        };
        if display
            .min
            .iter()
            .chain(&display.max)
            .any(|value| !value.is_finite())
        {
            return false;
        }
        if self.camera.fit(
            display,
            [self.surface_config.width, self.surface_config.height],
        ) {
            self.scene_bounds = Some(local);
            self.coordinate_origin = coordinate_origin;
            true
        } else {
            false
        }
    }

    pub fn orbit_camera(&mut self, delta_x: f64, delta_y: f64) -> bool {
        self.camera.orbit(delta_x, delta_y)
    }

    /// Returns position and target in the source dataset's coordinate system.
    pub fn camera_pose(&self) -> ([f64; 3], [f64; 3]) {
        let to_dataset = |point: [f64; 3]| {
            std::array::from_fn(|axis| {
                point[axis] / self.visual_scale + self.coordinate_origin[axis]
            })
        };
        (
            to_dataset(self.camera.position()),
            to_dataset(self.camera.target()),
        )
    }

    /// Exact local camera state for detecting changes to voxel projection inputs.
    pub fn camera_snapshot(&self) -> OrbitCamera {
        self.camera
    }

    /// Sets the pose in source dataset coordinates.
    pub fn set_camera_pose(&mut self, position: [f64; 3], target: [f64; 3]) -> bool {
        let to_local = |point: [f64; 3]| {
            std::array::from_fn(|axis| {
                (point[axis] - self.coordinate_origin[axis]) * self.visual_scale
            })
        };
        let changed = self.camera.set_pose(to_local(position), to_local(target));
        if changed {
            self.hzb_reference_camera = None;
        }
        changed
    }

    pub fn set_orbit_target(&mut self, target: [f64; 3]) -> bool {
        let target = std::array::from_fn(|axis| {
            (target[axis] - self.coordinate_origin[axis]) * self.visual_scale
        });
        let changed = self.camera.set_target(target);
        if changed {
            self.hzb_reference_camera = None;
        }
        changed
    }

    pub fn translate_camera(&mut self, delta: [f64; 3]) -> bool {
        let delta = delta.map(|value| value * self.visual_scale);
        let changed = self.camera.translate(delta);
        if changed {
            self.hzb_reference_camera = None;
        }
        changed
    }

    pub fn set_bird_view(&mut self) {
        self.camera.set_bird_view();
        self.hzb_reference_camera = None;
    }

    /// Copies only ray-intersecting instance ranges into a bounded staging buffer.
    pub fn prepare_pick(&self, pixel: [f64; 2]) -> Result<Option<PickReadback>, String> {
        if let Some(pool) = &self.cell_pool {
            let Some((origin, direction, near_distance, far_distance)) =
                self.camera.ray(pixel, self.viewport_size())
            else {
                return Ok(None);
            };
            let headroom = self.gpu_budget.max_bytes.saturating_sub(
                self.allocated_gpu_bytes()
                    .map_err(|error| error.to_string())?,
            );
            return Ok(prepare_pool_pick(
                &self.device,
                &self.queue,
                pool,
                PickRay {
                    origin,
                    direction,
                    near_distance,
                    far_distance,
                },
                self.visual_scale,
                self.coordinate_origin,
                headroom,
            )?
            .map(PickReadback::Pool));
        }
        let Some(source) = self.scene_buffer.as_ref() else {
            return Ok(None);
        };
        let Some((ray_origin, ray_direction, near_distance, far_distance)) =
            self.camera.ray(pixel, self.viewport_size())
        else {
            return Ok(None);
        };
        let mut candidates = self
            .pick_chunks
            .iter()
            .filter_map(|chunk| {
                ray_bounds_entry(ray_origin, ray_direction, chunk.bounds)
                    .map(|entry| (entry, *chunk))
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.0.total_cmp(&right.0));
        let candidate_count = candidates
            .iter()
            .try_fold(0_usize, |total, (_, chunk)| {
                total.checked_add(chunk.instance_count)
            })
            .ok_or_else(|| "pick candidate count overflow".to_owned())?;
        check_pick_candidate_budget(candidate_count)?;
        if candidate_count == 0 {
            return Ok(None);
        }

        let instance_bytes = std::mem::size_of::<InstanceRaw>() as u64;
        let byte_len = u64::try_from(candidate_count)
            .ok()
            .and_then(|count| count.checked_mul(instance_bytes))
            .ok_or_else(|| "pick staging buffer size overflow".to_owned())?;
        let projected_gpu_bytes = self
            .allocated_cpu_bytes()
            .map_err(|error| error.to_string())?
            .saturating_add(byte_len);
        if projected_gpu_bytes > self.gpu_budget.max_bytes {
            return Err(format!(
                "pick staging buffer would exceed the configured GPU memory budget ({projected_gpu_bytes} > {})",
                self.gpu_budget.max_bytes
            ));
        }
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("octree-viewer-pick-staging"),
            size: byte_len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("octree-viewer-pick-copy"),
            });
        let mut copies = Vec::with_capacity(candidates.len());
        let mut staging_offset = 0_u64;
        for (_, chunk) in candidates {
            let size = u64::try_from(chunk.instance_count)
                .ok()
                .and_then(|count| count.checked_mul(instance_bytes))
                .ok_or_else(|| "pick chunk size overflow".to_owned())?;
            let source_offset = u64::try_from(chunk.first_instance)
                .ok()
                .and_then(|index| index.checked_mul(instance_bytes))
                .ok_or_else(|| "pick source offset overflow".to_owned())?;
            encoder.copy_buffer_to_buffer(
                &source.buffer,
                source_offset,
                &staging,
                staging_offset,
                size,
            );
            copies.push(PickCopy {
                staging_offset,
                first_instance: chunk.first_instance,
                instance_count: chunk.instance_count,
            });
            staging_offset += size;
        }
        self.queue.submit([encoder.finish()]);
        Ok(Some(PickReadback::Cpu(CpuPick {
            buffer: staging,
            copies,
            ray: PickRay {
                origin: ray_origin,
                direction: ray_direction,
                near_distance,
                far_distance,
            },
            visual_scale: self.visual_scale,
            coordinate_origin: self.coordinate_origin,
        })))
    }

    pub fn resolve_pick(
        &self,
        readback: &PickReadback,
        mapped: &[u8],
    ) -> Result<Option<PickHit>, String> {
        match readback {
            PickReadback::Cpu(pick) => resolve_pick_instances(
                pick.ray,
                &pick.copies,
                mapped,
                pick.visual_scale,
                pick.coordinate_origin,
            ),
            PickReadback::Pool(pick) => pick.resolve(mapped),
        }
    }

    pub fn pan_camera(&mut self, delta_x: f64, delta_y: f64) -> bool {
        self.camera.pan(
            delta_x,
            delta_y,
            [self.surface_config.width, self.surface_config.height],
        )
    }

    pub fn zoom_camera(&mut self, delta: f64) -> bool {
        self.camera.zoom(delta)
    }

    /// GPU-free snapshot of the view math used by LOD selection and voxel construction.
    pub fn view_projection(&self) -> crate::ViewProjection {
        crate::ViewProjection::new(
            self.camera,
            self.viewport_size(),
            self.visual_scale,
            self.voxel_build_limits(),
        )
    }

    pub fn visual_scale(&self) -> f64 {
        self.visual_scale
    }

    pub fn viewport_size(&self) -> [u32; 2] {
        [self.surface_config.width, self.surface_config.height]
    }

    /// Snapshots resources owned now; replaced resources may still be retained by GPU work.
    pub fn resource_stats(&self) -> GpuResourceStats {
        let instance_buffer_bytes = self.scene_buffer.as_ref().map_or(0, |b| b.buffer.size())
            + self
                .block_buffers
                .values()
                .map(|b| b.buffer.size())
                .sum::<u64>();
        GpuResourceStats {
            instance_buffer_bytes,
            fixed_buffer_bytes: self.vertex_buffer.size()
                + self.index_buffer.size()
                + self.uniform_buffer.size(),
            hzb_buffer_bytes: self
                .hzb
                .as_ref()
                .map_or(0, |hzb| hzb.requested_buffer_bytes())
                + self
                    .hzb_count_readback
                    .as_ref()
                    .map_or(0, |buffer| buffer.size()),
            depth_texture_bytes: u64::from(self.depth_texture.width())
                * u64::from(self.depth_texture.height())
                * 4,
            hzb_texture_bytes: self
                .hzb
                .as_ref()
                .map_or(0, |hzb| hzb.logical_texture_bytes()),
            cell_pool_bytes: self.cell_pool_bytes(),
        }
    }

    pub fn uploaded_bytes(&self) -> u64 {
        self.uploaded_bytes
    }

    pub fn resize(&mut self, width: u32, height: u32) -> FrameStatus {
        let [width, height] =
            bounded_framebuffer_size(width, height, self.cpu_limits.max_texture_dimension_2d);
        if width == 0 || height == 0 {
            self.suspended = true;
            return FrameStatus::Suspended;
        }
        if self.surface_config.width == width && self.surface_config.height == height {
            self.suspended = false;
            return FrameStatus::Presented;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        self.depth_texture =
            create_depth_texture(&self.device, &self.surface_config, self.hzb_depth_supported);
        self.refresh_hzb_targets();
        if let Some(pool) = &mut self.cell_pool {
            pool.invalidate_viewport();
        }
        if !self.camera.is_fitted()
            && let Some(bounds) = self.display_bounds()
        {
            self.camera.fit(bounds, [width, height]);
        }
        self.suspended = false;
        FrameStatus::Reconfigured
    }

    fn refresh_hzb_targets(&mut self) {
        self.hzb_reference_camera = None;
        let Some(hzb) = &mut self.hzb else {
            return;
        };
        // Drop per-scene index buffers before resizing. The old pyramid remains alive until
        // resize_targets replaces it, so account for both pyramids during that transition.
        hzb.reset_scene();
        let required_bytes = crate::hzb::estimated_pyramid_bytes([
            self.surface_config.width,
            self.surface_config.height,
        ]);
        // The pool counts here: it stays resident while the HZB targets are rebuilt.
        let resize_fits_budget = self
            .allocated_gpu_bytes()
            .and_then(|current_bytes| {
                check_peak_bytes(current_bytes, required_bytes, self.gpu_budget.max_bytes)
            })
            .is_ok();
        if required_bytes > self.gpu_budget.max_bytes / 2 || !resize_fits_budget {
            self.hzb = None;
            self.hzb_count_readback = None;
            self.hzb_count_receiver = None;
            return;
        }
        let resized = self.hzb.as_mut().is_some_and(|hzb| {
            hzb.resize_targets(
                &self.device,
                &self.depth_texture,
                [self.surface_config.width, self.surface_config.height],
                self.cpu_limits.max_texture_dimension_2d,
            )
        });
        if !resized {
            self.hzb = None;
            self.hzb_count_readback = None;
            self.hzb_count_receiver = None;
            return;
        }
        self.reprepare_hzb_scene();
    }

    fn reprepare_hzb_scene(&mut self) {
        self.hzb_reference_camera = None;
        let allocated_without_hzb = self
            .allocated_cpu_bytes()
            .unwrap_or(self.gpu_budget.max_bytes)
            .saturating_sub(self.hzb.as_ref().map_or(0, |hzb| hzb.resource_bytes()));
        let available = self
            .gpu_budget
            .max_bytes
            .saturating_sub(allocated_without_hzb);
        let Some(hzb) = &mut self.hzb else {
            return;
        };
        if !self.hzb_scene_opaque {
            hzb.reset_scene();
            return;
        }
        let Some(buffer) = &self.scene_buffer else {
            hzb.reset_scene();
            return;
        };
        if buffer.instance_count < crate::hzb::MIN_OCCLUSION_CANDIDATES as u32
            || !hzb.can_prepare_scene(
                &self.cpu_limits,
                u64::from(buffer.instance_count) * size_of::<InstanceRaw>() as u64,
                buffer.instance_count as usize,
                available,
            )
        {
            hzb.reset_scene();
            return;
        }
        hzb.prepare_scene(
            &self.device,
            &buffer.buffer,
            u64::from(buffer.instance_count) * size_of::<InstanceRaw>() as u64,
            buffer.instance_count as usize,
        );
    }

    /// Presents a frame with the current camera.
    pub fn render(&mut self) -> Result<FrameStatus, GpuRendererError> {
        self.update_hzb_visible_count();
        if let Some(timer) = &mut self.frame_timer {
            timer.collect(&self.device);
        }
        if self.suspended {
            return Ok(FrameStatus::Suspended);
        }
        let viewport = [self.surface_config.width, self.surface_config.height];
        // With a cell pool the cells are drawn eye-relative and the CPU scene and its HZB are idle.
        let use_cells = self.cell_pool.is_some();
        let (view_projection, mut render_origin) = if use_cells {
            let Some(frame) = self.camera.eye_frame(viewport) else {
                return Ok(FrameStatus::Suspended);
            };
            (frame.view_projection, [0.0; 4])
        } else {
            let Some(CameraFrame {
                view_projection,
                render_origin,
            }) = self.camera.frame(viewport)
            else {
                return Ok(FrameStatus::Suspended);
            };
            (view_projection, render_origin)
        };
        render_origin[3] = self.visual_scale as f32;
        let camera_key = (view_projection, render_origin);
        let use_hzb = !use_cells
            && self.hzb_reference_camera == Some(camera_key)
            && self
                .hzb
                .as_ref()
                .is_some_and(|hzb| hzb.indexed_bind_group().is_some());
        if !use_hzb && !use_cells {
            self.visible_instance_count = self
                .scene_buffer
                .as_ref()
                .map_or(0, |buffer| buffer.instance_count as usize);
        }
        let (frame, suboptimal) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => (frame, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            wgpu::CurrentSurfaceTexture::Timeout => return Ok(FrameStatus::SkippedTimeout),
            wgpu::CurrentSurfaceTexture::Occluded => return Ok(FrameStatus::Suspended),
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                self.depth_texture = create_depth_texture(
                    &self.device,
                    &self.surface_config,
                    self.hzb_depth_supported,
                );
                self.refresh_hzb_targets();
                if let Some(pool) = &mut self.cell_pool {
                    pool.invalidate_viewport();
                }
                return Ok(FrameStatus::Reconfigured);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err(GpuRendererError::SurfaceAcquisition(
                    "surface lost; recreate the renderer and surface".into(),
                ));
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(GpuRendererError::SurfaceAcquisition(
                    "surface validation failed".into(),
                ));
            }
        };
        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&ViewProjection {
                matrix: view_projection,
                render_origin,
                style: [
                    self.color_mode.shader_value(),
                    1.0,
                    self.elevation_range.map_or(0.0, |range| range[0]),
                    self.elevation_range.map_or(1.0, |range| range[1]),
                ],
                intensity_range: self
                    .intensity_range
                    .map_or([0.0, 1.0, f32::from(self.depth_shading), 0.0], |range| {
                        [range[0], range[1], f32::from(self.depth_shading), 0.0]
                    }),
            }),
        );
        let view = frame.texture.create_view(&Default::default());
        let depth_view = self.depth_texture.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("octree-viewer-frame"),
            });
        let timer_slot = self
            .frame_timer
            .as_mut()
            .and_then(|timer| timer.begin(&mut encoder));
        if use_cells && let Some(pool) = &mut self.cell_pool {
            let eye_z = pool
                .page_frame()
                .map_or_else(|| self.camera.position()[2], |stamp| stamp.eye_scaled[2]);
            pool.set_color(CellColor::for_frame(
                self.color_mode,
                self.cell_color_ranges,
                eye_z,
                self.visual_scale,
                self.coordinate_origin[2],
            ));
            pool.encode_cells(
                &mut encoder,
                &view,
                &depth_view,
                &CellFrame {
                    view_projection,
                    clear_color: self.clear_color,
                    depth_shading: self.depth_shading,
                    viewport,
                },
            );
        }
        if use_hzb && let Some(hzb) = &self.hzb {
            hzb.cull(&mut encoder, &self.queue);
        }
        if !use_cells {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("octree-viewer-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            if use_hzb {
                if let Some(hzb) = &self.hzb
                    && let (Some(bind_group), Some(indirect_args)) =
                        (hzb.indexed_bind_group(), hzb.indirect_args())
                {
                    pass.set_pipeline(hzb.indexed_pipeline());
                    pass.set_bind_group(0, hzb.camera_bind_group(), &[]);
                    pass.set_bind_group(1, bind_group, &[]);
                    pass.draw_indexed_indirect(indirect_args, 0);
                }
            } else {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            }
            if !use_hzb && let Some(buffer) = &self.scene_buffer {
                if buffer.instance_count > 0 {
                    pass.set_vertex_buffer(1, buffer.buffer.slice(..));
                    pass.draw_indexed(0..CUBE_INDICES.len() as u32, 0, 0..buffer.instance_count);
                }
            } else if !use_hzb {
                for block_id in &self.visible_block_ids {
                    if let Some(buffer) = self.block_buffers.get(block_id)
                        && buffer.instance_count > 0
                    {
                        pass.set_vertex_buffer(1, buffer.buffer.slice(..));
                        pass.draw_indexed(
                            0..CUBE_INDICES.len() as u32,
                            0,
                            0..buffer.instance_count,
                        );
                    }
                }
            }
        }
        if !use_hzb
            && !use_cells
            && let Some(hzb) = &self.hzb
            && hzb.indexed_bind_group().is_some()
        {
            hzb.build_depth_pyramid(&mut encoder);
            hzb.cull(&mut encoder, &self.queue);
        }
        if use_hzb
            && self.hzb_count_receiver.is_none()
            && let (Some(hzb), Some(readback)) = (&self.hzb, &self.hzb_count_readback)
            && let Some(indirect_args) = hzb.indirect_args()
        {
            encoder.copy_buffer_to_buffer(
                indirect_args,
                size_of::<u32>() as u64,
                readback,
                0,
                size_of::<u32>() as u64,
            );
        }
        if let (Some(timer), Some(slot)) = (&self.frame_timer, timer_slot) {
            timer.end(&mut encoder, slot);
        }
        self.queue.submit([encoder.finish()]);
        if let (Some(timer), Some(slot)) = (&mut self.frame_timer, timer_slot) {
            timer.submitted(slot);
        }
        if use_hzb
            && self.hzb_count_receiver.is_none()
            && let Some(readback) = &self.hzb_count_readback
        {
            let (sender, receiver) = mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
            self.hzb_count_receiver = Some(receiver);
        }
        self.hzb_reference_camera = if !use_cells
            && self
                .hzb
                .as_ref()
                .is_some_and(|hzb| hzb.indexed_bind_group().is_some())
        {
            Some(camera_key)
        } else {
            None
        };
        self.queue.present(frame);
        if suboptimal {
            self.surface.configure(&self.device, &self.surface_config);
            self.depth_texture =
                create_depth_texture(&self.device, &self.surface_config, self.hzb_depth_supported);
            self.refresh_hzb_targets();
            if let Some(pool) = &mut self.cell_pool {
                pool.invalidate_viewport();
            }
            return Ok(FrameStatus::Reconfigured);
        }
        Ok(FrameStatus::Presented)
    }
}

pub(crate) fn scene_upload_metadata(
    scene: &RenderScene,
    max_buffer_size: u64,
    max_visible_instances: usize,
) -> Result<(u64, u32, Option<SceneBounds>), GpuRendererError> {
    let bytes = scene
        .instance_count()
        .checked_mul(size_of::<InstanceRaw>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or(GpuRendererError::InvalidSceneSize)?;
    if bytes > max_buffer_size {
        return Err(GpuRendererError::GpuBufferLimitExceeded {
            requested_bytes: bytes,
            max_bytes: max_buffer_size,
        });
    }
    let instance_count =
        u32::try_from(scene.instance_count()).map_err(|_| GpuRendererError::InvalidSceneSize)?;
    check_visible_instances(scene.instance_count(), max_visible_instances)?;
    let bounds = scene.bounds();
    if instance_count > 0 && bounds.is_none() {
        return Err(GpuRendererError::InvalidSceneData);
    }
    Ok((bytes, instance_count, bounds))
}

fn create_instance_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &RenderScene,
    bytes: u64,
    instance_count: u32,
    bounds: Option<SceneBounds>,
    max_buffer_size: u64,
) -> Result<GpuInstanceBuffer, GpuRendererError> {
    let capacity = buffer_capacity(bytes, size_of::<InstanceRaw>() as u64, max_buffer_size)?;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("octree-viewer-instances"),
        size: capacity,
        usage: wgpu::BufferUsages::VERTEX
            | wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if bytes > 0 {
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(scene.instances()));
    }
    Ok(GpuInstanceBuffer {
        buffer,
        capacity,
        instance_count,
        bounds,
        intensity_range: scene.intensity_range(),
        elevation_range: scene.elevation_range(),
    })
}

pub(crate) fn build_pick_chunks(instances: &[InstanceRaw], visual_scale: f64) -> Vec<PickChunk> {
    instances
        .chunks(PICK_CHUNK_INSTANCES)
        .enumerate()
        .filter_map(|(chunk_index, chunk)| {
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for instance in chunk {
                let half = f64::from(instance.size) * visual_scale * 0.5;
                for axis in 0..3 {
                    let center = f64::from(instance.center[axis]) * visual_scale;
                    min[axis] = min[axis].min(((center - half) as f32).next_down());
                    max[axis] = max[axis].max(((center + half) as f32).next_up());
                }
            }
            if min.iter().chain(&max).any(|value| !value.is_finite()) {
                return None;
            }
            Some(PickChunk {
                first_instance: chunk_index * PICK_CHUNK_INSTANCES,
                instance_count: chunk.len(),
                bounds: SceneBounds {
                    min: min.map(f64::from),
                    max: max.map(f64::from),
                },
            })
        })
        .collect()
}

fn ray_bounds_entry(
    origin: glam::DVec3,
    direction: glam::DVec3,
    bounds: SceneBounds,
) -> Option<f64> {
    ray_box_entry(origin, direction, bounds.min, bounds.max)
}

fn check_pick_candidate_budget(candidate_count: usize) -> Result<(), String> {
    if candidate_count > PICK_MAX_CANDIDATE_INSTANCES {
        Err(format!(
            "pick ray intersects {candidate_count} candidate voxels; limit is {PICK_MAX_CANDIDATE_INSTANCES}"
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn resolve_pick_instances(
    ray: PickRay,
    copies: &[PickCopy],
    mapped: &[u8],
    visual_scale: f64,
    coordinate_origin: [f64; 3],
) -> Result<Option<PickHit>, String> {
    let instance_bytes = std::mem::size_of::<InstanceRaw>();
    let mut nearest: Option<(f64, usize, InstanceRaw)> = None;
    for copy in copies {
        let start = usize::try_from(copy.staging_offset)
            .map_err(|_| "pick staging offset is not addressable".to_owned())?;
        for relative_index in 0..copy.instance_count {
            let byte_start = start + relative_index * instance_bytes;
            let byte_end = byte_start + instance_bytes;
            let raw = mapped
                .get(byte_start..byte_end)
                .ok_or_else(|| "pick staging result is truncated".to_owned())?;
            let instance = bytemuck::pod_read_unaligned::<InstanceRaw>(raw);
            let half = f64::from(instance.size) * visual_scale * 0.5;
            let center_render = instance.center.map(|value| f64::from(value) * visual_scale);
            let min = center_render.map(|value| value - half);
            let max = center_render.map(|value| value + half);
            let Some((entry, exit)) = ray_box_interval(ray.origin, ray.direction, min, max) else {
                continue;
            };
            let distance = entry.max(ray.near_distance);
            if distance > exit || distance > ray.far_distance {
                continue;
            }
            if nearest.is_none_or(|(best, _, _)| distance < best) {
                nearest = Some((distance, copy.first_instance + relative_index, instance));
            }
        }
    }
    let Some((distance, instance_index, instance)) = nearest else {
        return Ok(None);
    };
    let center = instance.center.map(f64::from);
    let position = std::array::from_fn(|axis| {
        (ray.origin[axis] + ray.direction[axis] * distance) / visual_scale + coordinate_origin[axis]
    });
    let voxel_center = std::array::from_fn(|axis| center[axis] + coordinate_origin[axis]);
    Ok(Some(PickHit {
        instance_index,
        position,
        voxel_center,
        voxel_size: f64::from(instance.size),
        distance: distance / visual_scale,
    }))
}

fn bounded_framebuffer_size(width: u32, height: u32, device_max_dimension: u32) -> [u32; 2] {
    if width == 0 || height == 0 || device_max_dimension == 0 {
        return [0, 0];
    }
    let dimension_limit = device_max_dimension.min(MAX_FRAMEBUFFER_DIMENSION);
    let pixel_count = u64::from(width) * u64::from(height);
    let pixel_scale = (MAX_FRAMEBUFFER_PIXELS as f64 / pixel_count as f64)
        .sqrt()
        .min(1.0);
    let dimension_scale = (f64::from(dimension_limit) / f64::from(width))
        .min(f64::from(dimension_limit) / f64::from(height))
        .min(1.0);
    let scale = pixel_scale.min(dimension_scale);
    [
        ((f64::from(width) * scale).floor() as u32).max(1),
        ((f64::from(height) * scale).floor() as u32).max(1),
    ]
}

fn ray_box_entry(
    origin: glam::DVec3,
    direction: glam::DVec3,
    min: [f64; 3],
    max: [f64; 3],
) -> Option<f64> {
    ray_box_interval(origin, direction, min, max).map(|(near, _)| near)
}

pub(crate) fn ray_box_interval(
    origin: glam::DVec3,
    direction: glam::DVec3,
    min: [f64; 3],
    max: [f64; 3],
) -> Option<(f64, f64)> {
    let mut near = 0.0_f64;
    let mut far = f64::INFINITY;
    for axis in 0..3 {
        if direction[axis].abs() < 1.0e-15 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
            continue;
        }
        let inverse = 1.0 / direction[axis];
        let mut first = (min[axis] - origin[axis]) * inverse;
        let mut second = (max[axis] - origin[axis]) * inverse;
        if first > second {
            std::mem::swap(&mut first, &mut second);
        }
        near = near.max(first);
        far = far.min(second);
        if near > far {
            return None;
        }
    }
    far.is_finite().then_some((near, far))
}

fn surface_config(
    surface: &wgpu::Surface<'_>,
    adapter: &wgpu::Adapter,
    width: u32,
    height: u32,
) -> Result<(wgpu::SurfaceConfiguration, wgpu::TextureFormat), GpuRendererError> {
    let mut config = surface
        .get_default_config(adapter, width.max(1), height.max(1))
        .ok_or(GpuRendererError::UnsupportedSurface)?;
    let capabilities = surface.get_capabilities(adapter);
    config.format = capabilities
        .formats
        .iter()
        .copied()
        .find(wgpu::TextureFormat::is_srgb)
        .unwrap_or(config.format);
    config.present_mode = if capabilities
        .present_modes
        .contains(&wgpu::PresentMode::Fifo)
    {
        wgpu::PresentMode::Fifo
    } else {
        config.present_mode
    };
    Ok((config.clone(), config.format))
}

fn create_depth_texture(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
    sampleable: bool,
) -> wgpu::Texture {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
    if sampleable {
        usage |= wgpu::TextureUsages::TEXTURE_BINDING;
    }
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("octree-viewer-depth"),
        size: wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage,
        view_formats: &[],
    })
}

/// Shared shader, mesh and pipeline configuration for surface and offscreen rendering.
pub(crate) struct PipelineResources {
    pub pipeline: wgpu::RenderPipeline,
    pub uniform_buffer: wgpu::Buffer,
    pub uniform_bind_group: wgpu::BindGroup,
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
}

pub(crate) async fn create_pipeline_resources(
    device: &wgpu::Device,
    pipeline_format: wgpu::TextureFormat,
) -> Result<PipelineResources, GpuRendererError> {
    let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("octree-viewer-voxel-shader"),
        source: wgpu::ShaderSource::Wgsl(VOXEL_SHADER_WGSL.into()),
    });
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("octree-viewer-view-projection"),
        size: std::mem::size_of::<ViewProjection>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("octree-viewer-uniform-layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(std::mem::size_of::<ViewProjection>() as u64),
            },
            count: None,
        }],
    });
    let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("octree-viewer-uniforms"),
        layout: &uniform_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("octree-viewer-pipeline-layout"),
        bind_group_layouts: &[Some(&uniform_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("octree-viewer-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[
                Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<CubeVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
                }),
                Some(instance_buffer_layout()),
            ],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: pipeline_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    if let Some(error) = validation_scope.pop().await {
        return Err(GpuRendererError::PipelineValidation(error.to_string()));
    }

    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("octree-viewer-cube-vertices"),
        contents: bytemuck::cast_slice(&CUBE_VERTICES),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("octree-viewer-cube-indices"),
        contents: bytemuck::cast_slice(&CUBE_INDICES),
        usage: wgpu::BufferUsages::INDEX,
    });
    Ok(PipelineResources {
        pipeline,
        uniform_buffer,
        uniform_bind_group,
        vertex_buffer,
        index_buffer,
    })
}

/// The cell driver (`CellDriver`) writes through the renderer's public cell methods.
impl CellTarget for SurfaceRenderer<'_> {
    type Error = GpuRendererError;

    fn has_pool(&self) -> bool {
        self.cells_active()
    }

    fn drop_pool(&mut self) {
        self.clear_cells();
    }

    fn eye(&self) -> [f64; 3] {
        self.cell_eye()
    }

    fn upload_level(
        &mut self,
        level: &PackedLevel,
        page_slots: &[u32],
    ) -> Result<(), GpuRendererError> {
        self.upload_cell_level(level, page_slots)
    }

    fn free_pages(&mut self, page_slots: &[u32]) -> Result<(), GpuRendererError> {
        self.free_cell_pages(page_slots)
    }

    fn alloc_mask_slot(&mut self) -> Result<Option<u32>, GpuRendererError> {
        self.alloc_cell_mask_slot()
    }

    fn set_node_mask(
        &mut self,
        slot: u32,
        mask: &[u32; MASK_WORDS],
    ) -> Result<(), GpuRendererError> {
        self.set_cell_node_mask(slot, mask)
    }

    fn set_color_ranges(&mut self, ranges: CellColorRanges) {
        self.cell_color_ranges = ranges;
    }

    fn set_active_pages(
        &mut self,
        pages: &[(u32, PageEntry)],
        frame: &PageFrame,
    ) -> Result<(), GpuRendererError> {
        self.set_active_cell_pages(pages, frame)
    }
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    #[test]
    fn failed_first_upload_drops_the_new_pool_and_keeps_an_existing_one() {
        let mut slot: Option<u8> = None;
        assert_eq!(
            upload_creating_pool(&mut slot, || 1, |_| Err::<(), _>("bad")),
            Err("bad")
        );
        assert_eq!(slot, None);
        assert_eq!(
            upload_creating_pool(&mut slot, || 1, |_| Ok::<(), ()>(())),
            Ok(true)
        );
        assert_eq!(slot, Some(1));
        assert_eq!(
            upload_creating_pool(&mut slot, || 2, |_| Err::<(), _>("bad")),
            Err("bad")
        );
        assert_eq!(slot, Some(1));
        assert_eq!(
            upload_creating_pool(&mut slot, || 2, |_| Ok::<(), ()>(())),
            Ok(false)
        );
    }

    #[test]
    fn pool_creation_budget_counts_only_the_pool() {
        assert!(check_pool_creation_budget(100, 100).is_ok());
        assert!(matches!(
            check_pool_creation_budget(101, 100),
            Err(GpuRendererError::CellPool(CellPoolError::BudgetExceeded {
                required_bytes: 101,
                max_bytes: 100
            }))
        ));
    }

    #[test]
    fn framebuffer_limits_apply_to_direct_renderer_callers() {
        let [width, height] = bounded_framebuffer_size(12_000, 8_000, 16_384);
        assert!(u64::from(width) * u64::from(height) <= MAX_FRAMEBUFFER_PIXELS);
        assert!(width <= MAX_FRAMEBUFFER_DIMENSION);
        assert!(height <= MAX_FRAMEBUFFER_DIMENSION);
        assert_eq!(bounded_framebuffer_size(u32::MAX, 1, 8_192), [4_096, 1]);
        assert_eq!(bounded_framebuffer_size(0, 800, 8_192), [0, 0]);
        assert_eq!(bounded_framebuffer_size(800, 600, 1_024), [800, 600]);
    }

    #[test]
    fn pick_candidate_budget_is_strict_and_bounded() {
        assert!(check_pick_candidate_budget(PICK_MAX_CANDIDATE_INSTANCES).is_ok());
        assert!(check_pick_candidate_budget(PICK_MAX_CANDIDATE_INSTANCES + 1).is_err());
    }

    #[test]
    fn ray_box_entry_orders_front_hits_and_rejects_misses() {
        let origin = glam::DVec3::ZERO;
        let direction = glam::DVec3::NEG_Z;
        assert_eq!(
            ray_box_entry(origin, direction, [-1.0; 3], [1.0; 3]),
            Some(0.0)
        );
        assert_eq!(
            ray_box_entry(origin, direction, [-1.0, -1.0, -4.0], [1.0, 1.0, -3.0]),
            Some(3.0)
        );
        assert_eq!(
            ray_box_entry(origin, direction, [2.0, -1.0, -4.0], [3.0, 1.0, -3.0]),
            None
        );
        assert_eq!(
            ray_box_entry(origin, glam::DVec3::Z, [-1.0; 3], [1.0, 1.0, -3.0]),
            None
        );
    }

    #[test]
    fn pick_chunk_bounds_cover_cube_faces_and_keep_instance_ranges() {
        let instances = (0..PICK_CHUNK_INSTANCES + 1)
            .map(|index| InstanceRaw {
                center: [index as f32, 0.0, 2.0],
                size: 2.0,
                color: [1.0; 4],
                intensity: -1.0,
                classification: -1.0,
            })
            .collect::<Vec<_>>();
        let chunks = build_pick_chunks(&instances, 1.0);
        assert_eq!(chunks.len(), 2);
        assert_eq!(
            (chunks[0].first_instance, chunks[0].instance_count),
            (0, 256)
        );
        assert_eq!(
            (chunks[1].first_instance, chunks[1].instance_count),
            (256, 1)
        );
        assert!(chunks[0].bounds.min[0] < -1.0 && chunks[0].bounds.min[0] > -1.000001);
        assert!(chunks[0].bounds.min[1] < -1.0 && chunks[0].bounds.min[1] > -1.000001);
        assert!(chunks[0].bounds.min[2] < 1.0 && chunks[0].bounds.min[2] > 0.999999);
        assert!(chunks[0].bounds.max[0] > 256.0 && chunks[0].bounds.max[0] < 256.001);
    }

    #[test]
    fn pick_readback_selects_frontmost_voxel_and_restores_dataset_coordinates() {
        let instances = [
            InstanceRaw {
                center: [0.0, 0.0, -3.0],
                size: 1.0,
                color: [1.0; 4],
                intensity: -1.0,
                classification: -1.0,
            },
            InstanceRaw {
                center: [0.0, 0.0, -5.0],
                size: 1.0,
                color: [1.0; 4],
                intensity: -1.0,
                classification: -1.0,
            },
        ];
        let mapped = bytemuck::cast_slice(&instances);
        let copies = [PickCopy {
            staging_offset: 0,
            first_instance: 10,
            instance_count: 2,
        }];
        let hit = resolve_pick_instances(
            PickRay {
                origin: glam::DVec3::ZERO,
                direction: glam::DVec3::NEG_Z,
                near_distance: 6.0,
                far_distance: 10.0,
            },
            &copies,
            mapped,
            2.0,
            [1_000.0, 2_000.0, 3_000.0],
        )
        .unwrap()
        .unwrap();
        assert_eq!(hit.instance_index, 10);
        assert_eq!(hit.voxel_center, [1_000.0, 2_000.0, 2_997.0]);
        assert_eq!(hit.voxel_size, 1.0);
        assert_eq!(hit.position, [1_000.0, 2_000.0, 2_997.0]);
        assert_eq!(hit.distance, 3.0);
        assert!(
            resolve_pick_instances(
                PickRay {
                    origin: glam::DVec3::ZERO,
                    direction: glam::DVec3::X,
                    near_distance: 0.1,
                    far_distance: 10.0,
                },
                &copies,
                mapped,
                1.0,
                [0.0; 3],
            )
            .unwrap()
            .is_none()
        );
        assert!(
            resolve_pick_instances(
                PickRay {
                    origin: glam::DVec3::ZERO,
                    direction: glam::DVec3::NEG_Z,
                    near_distance: 0.1,
                    far_distance: 10.0,
                },
                &copies,
                &[],
                1.0,
                [0.0; 3],
            )
            .is_err()
        );
    }
}
