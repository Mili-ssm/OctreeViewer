use std::num::NonZeroU64;
use std::{collections::HashMap, mem::size_of};

use bytemuck::{Pod, Zeroable};
use viewer_core::{BlockId, DecodedBlockCache};
use wgpu::util::DeviceExt;

use crate::{
    CameraFrame, InstanceRaw, OrbitCamera, RenderScene, RendererConfig, SceneBounds,
    VOXEL_SHADER_WGSL, block_sync::ResidentBlockInfo, block_sync::plan_block_sync,
    instance_buffer_layout,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuRendererError {
    NoCompatibleAdapter,
    UnsupportedSurface,
    DeviceRequest(String),
    InvalidSceneSize,
    InvalidSceneData,
    InvalidBlockData(BlockId),
    MissingBlockPayload(BlockId),
    PipelineValidation(String),
}

impl std::fmt::Display for GpuRendererError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCompatibleAdapter => f.write_str("no GPU adapter supports this surface"),
            Self::UnsupportedSurface => f.write_str("surface has no usable presentation format"),
            Self::DeviceRequest(error) => write!(f, "GPU device request failed: {error}"),
            Self::InvalidSceneSize => f.write_str("scene exceeds supported GPU buffer limits"),
            Self::InvalidSceneData => f.write_str("scene contains invalid render data"),
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
struct ViewProjection {
    matrix: [[f32; 4]; 4],
    render_origin: [f32; 4],
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

const CUBE_INDICES: [u16; 36] = [
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

        let (surface_config, pipeline_format) = surface_config(&surface, &adapter, width, height)?;
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
                visibility: wgpu::ShaderStages::VERTEX,
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
        let scene_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("octree-viewer-instances"),
            size: size_of::<InstanceRaw>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
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
            scene_buffer: Some(GpuInstanceBuffer {
                buffer: scene_buffer,
                capacity: size_of::<InstanceRaw>() as u64,
                instance_count: 0,
                bounds: None,
            }),
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
        })
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_config.format
    }

    /// Uploads one scene and returns the bytes written. Existing GPU storage is reused when it fits.
    pub fn upload_scene(&mut self, scene: &RenderScene) -> Result<u64, GpuRendererError> {
        let (bytes, count, bounds) =
            scene_upload_metadata(scene, self.device.limits().max_buffer_size)?;
        let current_capacity = self
            .scene_buffer
            .as_ref()
            .map_or(0, |buffer| buffer.capacity);
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
                bounds: buffer.bounds,
            })
            .collect();
        let plan = plan_block_sync(
            &resident,
            block_ids,
            cache,
            self.device.limits().max_buffer_size,
        )?;
        let mut additions = HashMap::with_capacity(plan.added.len());
        for (id, scene) in &plan.added {
            let (bytes, count, bounds) =
                scene_upload_metadata(scene, self.device.limits().max_buffer_size)?;
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

    /// Drops scene and block GPU data before opening another dataset.
    pub fn clear_dataset(&mut self) {
        self.scene_buffer = None;
        self.block_buffers.clear();
        self.visible_block_ids.clear();
        self.visible_instance_count = 0;
        self.scene_bounds = None;
        self.uploaded_bytes = 0;
    }

    pub fn visible_block_count(&self) -> usize {
        self.visible_block_ids.len()
    }

    pub fn visible_instance_count(&self) -> usize {
        self.visible_instance_count
    }

    pub fn fit_scene(&mut self, scene: &RenderScene) -> bool {
        self.scene_bounds = scene.bounds();
        self.scene_bounds.is_some_and(|bounds| {
            self.camera.fit(
                bounds,
                [self.surface_config.width, self.surface_config.height],
            )
        })
    }

    pub fn fit_visible_blocks(&mut self) -> bool {
        self.scene_bounds.is_some_and(|bounds| {
            self.camera.fit(
                bounds,
                [self.surface_config.width, self.surface_config.height],
            )
        })
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
            && let Some(bounds) = self.scene_bounds
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
            render_origin,
        }) = self
            .camera
            .frame([self.surface_config.width, self.surface_config.height])
        else {
            return Ok(FrameStatus::Suspended);
        };
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

fn scene_upload_metadata(
    scene: &RenderScene,
    max_buffer_size: u64,
) -> Result<(u64, u32, Option<SceneBounds>), GpuRendererError> {
    let bytes = scene
        .instance_count()
        .checked_mul(size_of::<InstanceRaw>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or(GpuRendererError::InvalidSceneSize)?;
    if bytes > max_buffer_size {
        return Err(GpuRendererError::InvalidSceneSize);
    }
    let instance_count =
        u32::try_from(scene.instance_count()).map_err(|_| GpuRendererError::InvalidSceneSize)?;
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
    let minimum_size = size_of::<InstanceRaw>() as u64;
    let required_size = bytes.max(minimum_size);
    let capacity = required_size
        .checked_next_power_of_two()
        .unwrap_or(max_buffer_size)
        .min(max_buffer_size);
    if capacity < required_size {
        return Err(GpuRendererError::InvalidSceneSize);
    }
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
