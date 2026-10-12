use std::num::NonZeroU64;
use std::{collections::HashMap, mem::size_of, sync::mpsc};

use bytemuck::{Pod, Zeroable};
use viewer_core::{BlockId, DecodedBlockCache};
use wgpu::util::DeviceExt;

use crate::{
    CameraFrame, ColorMode, GpuBudget, InstanceRaw, OrbitCamera, RenderScene, RendererConfig,
    SceneBounds, VOXEL_SHADER_WGSL, VoxelBuildLimits,
    block_sync::ResidentBlockInfo,
    block_sync::plan_block_sync,
    budget::{buffer_capacity, check_peak_bytes, check_visible_instances},
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
const PICK_MAX_CANDIDATE_INSTANCES: usize = 65_536;
const MAX_FRAMEBUFFER_PIXELS: u64 = 4_194_304;
const MAX_FRAMEBUFFER_DIMENSION: u32 = 4_096;

#[derive(Debug, Clone, Copy)]
struct PickChunk {
    first_instance: usize,
    instance_count: usize,
    bounds: SceneBounds,
}

#[derive(Debug, Clone, Copy)]
struct PickCopy {
    staging_offset: u64,
    first_instance: usize,
    instance_count: usize,
}

pub struct PickReadback {
    buffer: wgpu::Buffer,
    copies: Vec<PickCopy>,
    ray: PickRay,
}

#[derive(Debug, Clone, Copy)]
struct PickRay {
    origin: glam::DVec3,
    direction: glam::DVec3,
    near_distance: f64,
    far_distance: f64,
}

impl PickReadback {
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
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
            })
            .await
            .ok_or(GpuRendererError::NoCompatibleAdapter)?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("octree-viewer-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                },
                None,
            )
            .await
            .map_err(|error| GpuRendererError::DeviceRequest(error.to_string()))?;

        let device_limits = device.limits();
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
            self.device.limits().max_buffer_size,
            self.gpu_budget.max_visible_instances,
        )?;
        let current_capacity = self
            .scene_buffer
            .as_ref()
            .map_or(0, |buffer| buffer.capacity);
        let target_capacity = buffer_capacity(
            bytes,
            size_of::<InstanceRaw>() as u64,
            self.device.limits().max_buffer_size,
        )?;
        let added_capacity = if target_capacity > current_capacity {
            target_capacity
        } else {
            0
        };
        check_peak_bytes(
            self.allocated_gpu_bytes()?,
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
                self.device.limits().max_buffer_size,
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
                self.device.limits().max_buffer_size,
            )?);
        }
        self.block_buffers.clear();
        self.visible_block_ids.clear();
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
        cache: &DecodedBlockCache,
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
            self.device.limits().max_buffer_size,
            self.gpu_budget,
            self.scene_buffer
                .as_ref()
                .map_or(0, |buffer| buffer.capacity),
        )?;
        let mut additions = HashMap::with_capacity(plan.added.len());
        for (id, scene) in &plan.added {
            let (bytes, count, bounds) = scene_upload_metadata(
                scene,
                self.device.limits().max_buffer_size,
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
                    self.device.limits().max_buffer_size,
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
            .checked_add(self.hzb.as_ref().map_or(0, |hzb| hzb.resource_bytes()))
            .and_then(|bytes| bytes.checked_add(self.hzb_count_readback.as_ref().map_or(0, |_| 4)))
            .ok_or(GpuRendererError::InvalidSceneSize)
    }

    fn prepare_hzb_scene(&mut self, source_bytes: u64) {
        self.hzb_reference_camera = None;
        let allocated_without_hzb = self
            .allocated_gpu_bytes()
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
                &self.device,
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
                    let view = buffer.slice(..).get_mapped_range();
                    self.visible_instance_count = *bytemuck::from_bytes::<u32>(&view) as usize;
                    drop(view);
                    buffer.unmap();
                }
            }
            Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                self.hzb_count_receiver = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
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
            .allocated_gpu_bytes()
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
        Ok(Some(PickReadback {
            buffer: staging,
            copies,
            ray: PickRay {
                origin: ray_origin,
                direction: ray_direction,
                near_distance,
                far_distance,
            },
        }))
    }

    pub fn resolve_pick(
        &self,
        readback: &PickReadback,
        mapped: &[u8],
    ) -> Result<Option<PickHit>, String> {
        resolve_pick_instances(
            readback.ray,
            &readback.copies,
            mapped,
            self.visual_scale,
            self.coordinate_origin,
        )
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

    /// Returns the current camera frustum in dataset coordinates.
    pub fn spatial_frustum(
        &self,
        coordinate_origin: [f64; 3],
    ) -> Option<viewer_core::SpatialFrustum> {
        let viewport = [self.surface_config.width, self.surface_config.height];
        let frustum = self.camera.spatial_frustum(viewport)?;
        let planes = frustum
            .planes()
            .iter()
            .map(|plane| {
                let normal = plane
                    .normal()
                    .map(|component| component * self.visual_scale);
                viewer_core::FrustumPlane::new(
                    normal,
                    plane.offset()
                        - (0..3)
                            .map(|axis| normal[axis] * coordinate_origin[axis])
                            .sum::<f64>(),
                )
                .ok()
            })
            .collect::<Option<Vec<_>>>()?;
        viewer_core::SpatialFrustum::new(planes).ok()
    }

    /// Projects dataset-space node error using the current camera and viewport.
    pub fn projected_error_pixels(
        &self,
        bounds: viewer_core::SpatialBounds,
        geometric_error: f64,
        coordinate_origin: [f64; 3],
    ) -> f64 {
        let min = std::array::from_fn(|axis| {
            (bounds.min()[axis] - coordinate_origin[axis]) * self.visual_scale
        });
        let max = std::array::from_fn(|axis| {
            (bounds.max()[axis] - coordinate_origin[axis]) * self.visual_scale
        });
        let Ok(bounds) = viewer_core::SpatialBounds::new(min, max) else {
            return f64::INFINITY;
        };
        self.camera.projected_error_pixels(
            bounds,
            geometric_error * self.visual_scale,
            [0.0; 3],
            [self.surface_config.width, self.surface_config.height],
        )
    }

    pub fn viewport_size(&self) -> [u32; 2] {
        [self.surface_config.width, self.surface_config.height]
    }

    pub fn projected_voxel_edge_pixels(
        &self,
        bounds: viewer_core::SpatialBounds,
        coordinate_origin: [f64; 3],
    ) -> f64 {
        let min = std::array::from_fn(|axis| {
            (bounds.min()[axis] - coordinate_origin[axis]) * self.visual_scale
        });
        let max = std::array::from_fn(|axis| {
            (bounds.max()[axis] - coordinate_origin[axis]) * self.visual_scale
        });
        let Ok(bounds) = viewer_core::SpatialBounds::new(min, max) else {
            return f64::INFINITY;
        };
        self.camera
            .projected_voxel_edge_pixels(bounds, [0.0; 3], self.viewport_size())
    }

    pub fn uploaded_bytes(&self) -> u64 {
        self.uploaded_bytes
    }

    pub fn resize(&mut self, width: u32, height: u32) -> FrameStatus {
        let [width, height] =
            bounded_framebuffer_size(width, height, self.device.limits().max_texture_dimension_2d);
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
            .allocated_gpu_bytes()
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
                &self.device,
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
    pub fn render(&mut self) -> Result<FrameStatus, wgpu::SurfaceError> {
        self.update_hzb_visible_count();
        if self.suspended {
            return Ok(FrameStatus::Suspended);
        }
        let Some(CameraFrame {
            view_projection,
            mut render_origin,
        }) = self
            .camera
            .frame([self.surface_config.width, self.surface_config.height])
        else {
            return Ok(FrameStatus::Suspended);
        };
        render_origin[3] = self.visual_scale as f32;
        let camera_key = (view_projection, render_origin);
        let use_hzb = self.hzb_reference_camera == Some(camera_key)
            && self
                .hzb
                .as_ref()
                .is_some_and(|hzb| hzb.indexed_bind_group().is_some());
        if !use_hzb {
            self.visible_instance_count = self
                .scene_buffer
                .as_ref()
                .map_or(0, |buffer| buffer.instance_count as usize);
        }
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Timeout) => return Ok(FrameStatus::SkippedTimeout),
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.surface_config);
                self.depth_texture = create_depth_texture(
                    &self.device,
                    &self.surface_config,
                    self.hzb_depth_supported,
                );
                self.refresh_hzb_targets();
                return Ok(FrameStatus::Reconfigured);
            }
            Err(error) => return Err(error),
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
        if use_hzb && let Some(hzb) = &self.hzb {
            hzb.cull(&mut encoder, &self.queue);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("octree-viewer-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
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
        self.queue.submit([encoder.finish()]);
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
        self.hzb_reference_camera = if self
            .hzb
            .as_ref()
            .is_some_and(|hzb| hzb.indexed_bind_group().is_some())
        {
            Some(camera_key)
        } else {
            None
        };
        let suboptimal = frame.suboptimal;
        frame.present();
        if suboptimal {
            self.surface.configure(&self.device, &self.surface_config);
            self.depth_texture =
                create_depth_texture(&self.device, &self.surface_config, self.hzb_depth_supported);
            self.refresh_hzb_targets();
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

fn build_pick_chunks(instances: &[InstanceRaw], visual_scale: f64) -> Vec<PickChunk> {
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

fn resolve_pick_instances(
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

fn ray_box_interval(
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
    device.push_error_scope(wgpu::ErrorFilter::Validation);
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
        bind_group_layouts: &[&uniform_layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("octree-viewer-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[
                wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<CubeVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
                },
                instance_buffer_layout(),
            ],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
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
        multiview: None,
        cache: None,
    });
    if let Some(error) = device.pop_error_scope().await {
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

#[cfg(test)]
mod pick_tests {
    use super::*;

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
