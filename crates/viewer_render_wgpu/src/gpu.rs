use std::num::NonZeroU64;
use std::{collections::HashMap, mem::size_of};

use bytemuck::{Pod, Zeroable};
use viewer_core::{BlockId, DecodedBlockCache};
use wgpu::util::DeviceExt;

use crate::{
    CameraFrame, ColorMode, GpuBudget, InstanceRaw, OrbitCamera, RenderScene, RendererConfig,
    SceneBounds, VOXEL_SHADER_WGSL,
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
    surface: wgpu::Surface<'window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    depth_texture: wgpu::Texture,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    scene_buffer: Option<GpuInstanceBuffer>,
    block_buffers: HashMap<BlockId, GpuInstanceBuffer>,
    visible_block_ids: Vec<BlockId>,
    visible_instance_count: usize,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    clear_color: wgpu::Color,
    uploaded_bytes: u64,
    suspended: bool,
    camera: OrbitCamera,
    scene_bounds: Option<SceneBounds>,
    gpu_budget: GpuBudget,
    intensity_range: Option<[f32; 2]>,
    elevation_range: Option<[f32; 2]>,
    color_mode: ColorMode,
    depth_shading: bool,
    visual_scale: f64,
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
        let depth_texture = create_depth_texture(&device, &surface_config);
        surface.configure(&device, &surface_config);

        Ok(Self {
            surface,
            device,
            queue,
            surface_config,
            pipeline,
            depth_texture,
            vertex_buffer,
            index_buffer,
            scene_buffer: None,
            block_buffers: HashMap::new(),
            visible_block_ids: Vec::new(),
            visible_instance_count: 0,
            uniform_buffer,
            uniform_bind_group,
            clear_color: config.clear_color,
            uploaded_bytes: 0,
            suspended: width == 0 || height == 0,
            camera: OrbitCamera::default(),
            scene_bounds: None,
            gpu_budget: config.gpu_budget,
            intensity_range: None,
            elevation_range: None,
            color_mode: ColorMode::Rgb,
            depth_shading: true,
            visual_scale: 1.0,
        })
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_config.format
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
        self.visible_instance_count = scene.instance_count();
        self.uploaded_bytes = self.uploaded_bytes.saturating_add(bytes);
        self.scene_bounds = bounds;
        self.intensity_range = scene.intensity_range();
        self.elevation_range = scene.elevation_range();
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
        self.block_buffers
            .values()
            .try_fold(scene_capacity, |sum, buffer| {
                sum.checked_add(buffer.capacity)
                    .ok_or(GpuRendererError::InvalidSceneSize)
            })
    }

    /// Drops scene and block GPU data before opening another dataset.
    pub fn clear_dataset(&mut self) {
        self.scene_buffer = None;
        self.block_buffers.clear();
        self.visible_block_ids.clear();
        self.visible_instance_count = 0;
        self.scene_bounds = None;
        self.intensity_range = None;
        self.elevation_range = None;
        self.uploaded_bytes = 0;
        self.visual_scale = 1.0;
    }

    pub fn visible_block_count(&self) -> usize {
        self.visible_block_ids.len()
    }

    pub fn visible_instance_count(&self) -> usize {
        self.visible_instance_count
    }

    pub fn max_visible_instances(&self) -> usize {
        self.gpu_budget.max_visible_instances
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
        Ok(())
    }

    pub fn set_depth_shading(&mut self, enabled: bool) {
        self.depth_shading = enabled;
    }

    pub fn fit_scene(&mut self, scene: &RenderScene) -> bool {
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
            true
        } else {
            false
        }
    }

    pub fn orbit_camera(&mut self, delta_x: f64, delta_y: f64) -> bool {
        self.camera.orbit(delta_x, delta_y)
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
        self.depth_texture = create_depth_texture(&self.device, &self.surface_config);
        if !self.camera.is_fitted()
            && let Some(bounds) = self.display_bounds()
        {
            self.camera.fit(bounds, [width, height]);
        }
        self.suspended = false;
        FrameStatus::Reconfigured
    }

    /// Presents a frame with the current camera.
    pub fn render(&mut self) -> Result<FrameStatus, wgpu::SurfaceError> {
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
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Timeout) => return Ok(FrameStatus::SkippedTimeout),
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.surface_config);
                self.depth_texture = create_depth_texture(&self.device, &self.surface_config);
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
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            if let Some(buffer) = &self.scene_buffer {
                if buffer.instance_count > 0 {
                    pass.set_vertex_buffer(1, buffer.buffer.slice(..));
                    pass.draw_indexed(0..CUBE_INDICES.len() as u32, 0, 0..buffer.instance_count);
                }
            } else {
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
        self.queue.submit([encoder.finish()]);
        let suboptimal = frame.suboptimal;
        frame.present();
        if suboptimal {
            self.surface.configure(&self.device, &self.surface_config);
            self.depth_texture = create_depth_texture(&self.device, &self.surface_config);
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
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
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
) -> wgpu::Texture {
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
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
