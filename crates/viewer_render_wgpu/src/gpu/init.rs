//! Surface creation, configuration and resize.

use super::{
    FrameStatus, SurfaceRenderer, device_loss,
    error::GpuRendererError,
    pipeline::{PipelineResources, create_pipeline_resources},
};
use std::{
    collections::HashMap,
    mem::size_of,
    sync::{Arc, Mutex},
};

use crate::{
    CellColorRanges, ColorMode, InstanceRaw, OrbitCamera, RendererConfig,
    cell_render::{cell_pool_pages, plan_device_limits},
};

pub(super) const MAX_FRAMEBUFFER_PIXELS: u64 = 4_194_304;
pub(super) const MAX_FRAMEBUFFER_DIMENSION: u32 = 4_096;

pub(super) fn bounded_framebuffer_size(
    width: u32,
    height: u32,
    device_max_dimension: u32,
) -> [u32; 2] {
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

pub(super) fn create_depth_texture(
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

        let device_loss_slot = Arc::new(Mutex::new(None));
        {
            let slot = Arc::clone(&device_loss_slot);
            device.set_device_lost_callback(move |reason, message| {
                if let Ok(mut slot) = slot.lock() {
                    slot.get_or_insert_with(|| device_loss(reason, message));
                }
            });
        }
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
            device_loss: device_loss_slot,
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
}
