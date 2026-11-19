use std::{mem::size_of, num::NonZeroU64};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::{GpuRendererError, gpu::ViewUniform};

pub(crate) const MIN_OCCLUSION_CANDIDATES: usize = 4_096;
const CULL_WORKGROUP_SIZE: u32 = 64;
const PYRAMID_WORKGROUP_SIZE: u32 = 8;

pub(crate) fn estimated_pyramid_bytes(viewport: [u32; 2]) -> u64 {
    let size = [
        viewport[0].max(1).next_power_of_two(),
        viewport[1].max(1).next_power_of_two(),
    ];
    let mip_count = size[0].max(size[1]).ilog2() + 1;
    pyramid_bytes(size, mip_count)
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CullParams {
    viewport: [u32; 2],
    pyramid_size: [u32; 2],
    instance_count: u32,
    mip_count: u32,
    depth_epsilon: f32,
    _padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DrawIndexedIndirectArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

pub(crate) struct HzbResources {
    depth_pipeline: wgpu::ComputePipeline,
    reduce_pipeline: wgpu::ComputePipeline,
    cull_pipeline: wgpu::ComputePipeline,
    indexed_pipeline: wgpu::RenderPipeline,
    camera_bind_group: wgpu::BindGroup,
    depth_bind_group: wgpu::BindGroup,
    reduce_bind_groups: Vec<wgpu::BindGroup>,
    cull_bind_group: Option<wgpu::BindGroup>,
    indexed_bind_group: Option<wgpu::BindGroup>,
    _pyramid_texture: wgpu::Texture,
    pyramid_view: wgpu::TextureView,
    visible_indices: Option<wgpu::Buffer>,
    indirect_args: Option<wgpu::Buffer>,
    params_buffer: wgpu::Buffer,
    viewport: [u32; 2],
    pyramid_size: [u32; 2],
    mip_count: u32,
    instance_count: usize,
    resource_bytes: u64,
}

impl HzbResources {
    pub(crate) async fn new(
        device: &wgpu::Device,
        adapter: &wgpu::Adapter,
        camera_buffer: &wgpu::Buffer,
        depth_texture: &wgpu::Texture,
        color_format: wgpu::TextureFormat,
        viewport: [u32; 2],
        max_texture_dimension_2d: u32,
    ) -> Result<Option<Self>, GpuRendererError> {
        let downlevel = adapter.get_downlevel_capabilities();
        let format_features = adapter.get_texture_format_features(wgpu::TextureFormat::R32Float);
        if !downlevel.flags.contains(
            wgpu::DownlevelFlags::COMPUTE_SHADERS | wgpu::DownlevelFlags::INDIRECT_EXECUTION,
        ) || !format_features
            .allowed_usages
            .contains(wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING)
            || !adapter
                .get_texture_format_features(wgpu::TextureFormat::Depth32Float)
                .allowed_usages
                .contains(wgpu::TextureUsages::TEXTURE_BINDING)
        {
            return Ok(None);
        }

        let viewport = [viewport[0].max(1), viewport[1].max(1)];
        let pyramid_size = [
            viewport[0].next_power_of_two(),
            viewport[1].next_power_of_two(),
        ];
        if pyramid_size
            .iter()
            .any(|side| *side > max_texture_dimension_2d)
        {
            return Ok(None);
        }
        let mip_count = pyramid_size[0].max(pyramid_size[1]).ilog2() + 1;
        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("octree-viewer-hzb-camera-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX
                    | wgpu::ShaderStages::FRAGMENT
                    | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(size_of::<ViewUniform>() as u64),
                },
                count: None,
            }],
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("octree-viewer-hzb-camera"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let pyramid_texture = create_pyramid_texture(device, pyramid_size, mip_count);
        let pyramid_view = pyramid_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("octree-viewer-hzb-all-mips"),
            usage: None,
            format: Some(wgpu::TextureFormat::R32Float),
            dimension: Some(wgpu::TextureViewDimension::D2),
            aspect: wgpu::TextureAspect::All,
            base_mip_level: 0,
            mip_level_count: Some(mip_count),
            base_array_layer: 0,
            array_layer_count: Some(1),
        });
        let depth_views = (0..mip_count)
            .map(|mip| {
                pyramid_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("octree-viewer-hzb-mip"),
                    usage: None,
                    format: Some(wgpu::TextureFormat::R32Float),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    aspect: wgpu::TextureAspect::All,
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    base_array_layer: 0,
                    array_layer_count: Some(1),
                })
            })
            .collect::<Vec<_>>();

        let depth_layout = texture_storage_layout(device, 0, true);
        let reduce_layout = texture_storage_layout(device, 2, false);
        let depth_pipeline = compute_pipeline(
            device,
            "octree-viewer-hzb-depth-pipeline",
            HZB_SHADER,
            "copy_depth",
            &[Some(&depth_layout)],
        );
        let reduce_pipeline = compute_pipeline(
            device,
            "octree-viewer-hzb-reduce-pipeline",
            HZB_SHADER,
            "reduce_depth",
            &[Some(&reduce_layout)],
        );
        let depth_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("octree-viewer-hzb-depth-bind-group"),
            layout: &depth_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &depth_texture.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&depth_views[0]),
                },
            ],
        });
        let reduce_bind_groups = depth_views
            .windows(2)
            .map(|views| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("octree-viewer-hzb-reduce-bind-group"),
                    layout: &reduce_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&views[0]),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&views[1]),
                        },
                    ],
                })
            })
            .collect();

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("octree-viewer-hzb-cull-params"),
            contents: bytemuck::bytes_of(&CullParams {
                viewport,
                pyramid_size,
                instance_count: 0,
                mip_count,
                depth_epsilon: 0.001,
                _padding: 0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let cull_layout = create_cull_layout(device);
        let cull_pipeline = compute_pipeline(
            device,
            "octree-viewer-hzb-cull-pipeline",
            CULL_SHADER,
            "cull_instances",
            &[Some(&uniform_layout), Some(&cull_layout)],
        );
        let indexed_layout = create_indexed_layout(device);
        let indexed_pipeline =
            create_indexed_pipeline(device, color_format, &uniform_layout, &indexed_layout);
        let validation_error = error_scope.pop().await;
        if let Some(error) = validation_error {
            return Err(GpuRendererError::PipelineValidation(error.to_string()));
        }

        Ok(Some(Self {
            depth_pipeline,
            reduce_pipeline,
            cull_pipeline,
            indexed_pipeline,
            camera_bind_group,
            depth_bind_group,
            reduce_bind_groups,
            cull_bind_group: None,
            indexed_bind_group: None,
            _pyramid_texture: pyramid_texture,
            pyramid_view,
            visible_indices: None,
            indirect_args: None,
            params_buffer,
            viewport,
            pyramid_size,
            mip_count,
            instance_count: 0,
            resource_bytes: pyramid_bytes(pyramid_size, mip_count),
        }))
    }

    pub(crate) fn can_prepare_scene(
        &self,
        limits: &wgpu::Limits,
        source_bytes: u64,
        instance_count: usize,
        available_bytes: u64,
    ) -> bool {
        let index_bytes = (instance_count as u64).saturating_mul(size_of::<u32>() as u64);
        source_bytes <= limits.max_storage_buffer_binding_size
            && index_bytes <= limits.max_storage_buffer_binding_size
            && index_bytes <= limits.max_buffer_size
            && self
                .resource_bytes
                .saturating_add(index_bytes)
                .saturating_add(size_of::<DrawIndexedIndirectArgs>() as u64)
                <= available_bytes
    }

    pub(crate) fn prepare_scene(
        &mut self,
        device: &wgpu::Device,
        source_buffer: &wgpu::Buffer,
        source_bytes: u64,
        instance_count: usize,
    ) {
        let index_bytes = (instance_count.max(1) as u64) * size_of::<u32>() as u64;
        let visible_indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("octree-viewer-hzb-visible-indices"),
            size: index_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        // cull() initializes every argument before dispatch/draw. Avoid mapped creation:
        // Chrome software WebGPU can reject this 20-byte mapped allocation.
        let indirect_args = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("octree-viewer-hzb-indirect-args"),
            size: size_of::<DrawIndexedIndirectArgs>() as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cull_layout = self.cull_pipeline.get_bind_group_layout(1);
        let cull_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("octree-viewer-hzb-cull-bind-group"),
            layout: &cull_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: source_buffer,
                        offset: 0,
                        size: NonZeroU64::new(source_bytes),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.pyramid_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: visible_indices.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: indirect_args.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.params_buffer.as_entire_binding(),
                },
            ],
        });
        let indexed_layout = self.indexed_pipeline.get_bind_group_layout(1);
        let indexed_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("octree-viewer-hzb-indexed-bind-group"),
            layout: &indexed_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: source_buffer,
                        offset: 0,
                        size: NonZeroU64::new(source_bytes),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: visible_indices.as_entire_binding(),
                },
            ],
        });
        self.visible_indices = Some(visible_indices);
        self.indirect_args = Some(indirect_args);
        self.cull_bind_group = Some(cull_bind_group);
        self.indexed_bind_group = Some(indexed_bind_group);
        self.instance_count = instance_count;
        self.resource_bytes = pyramid_bytes(self.pyramid_size, self.mip_count)
            + index_bytes
            + size_of::<DrawIndexedIndirectArgs>() as u64;
    }

    pub(crate) fn reset_scene(&mut self) {
        self.cull_bind_group = None;
        self.indexed_bind_group = None;
        self.visible_indices = None;
        self.indirect_args = None;
        self.instance_count = 0;
        self.resource_bytes = pyramid_bytes(self.pyramid_size, self.mip_count);
    }

    pub(crate) fn resize_targets(
        &mut self,
        device: &wgpu::Device,
        depth_texture: &wgpu::Texture,
        viewport: [u32; 2],
        max_texture_dimension_2d: u32,
    ) -> bool {
        let viewport = [viewport[0].max(1), viewport[1].max(1)];
        let pyramid_size = [
            viewport[0].next_power_of_two(),
            viewport[1].next_power_of_two(),
        ];
        if pyramid_size
            .iter()
            .any(|side| *side > max_texture_dimension_2d)
        {
            self.cull_bind_group = None;
            self.indexed_bind_group = None;
            self.visible_indices = None;
            self.indirect_args = None;
            self.instance_count = 0;
            self.resource_bytes = pyramid_bytes(self.pyramid_size, self.mip_count);
            return false;
        }
        let mip_count = pyramid_size[0].max(pyramid_size[1]).ilog2() + 1;
        let pyramid_texture = create_pyramid_texture(device, pyramid_size, mip_count);
        let pyramid_view = pyramid_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("octree-viewer-hzb-all-mips"),
            usage: None,
            format: Some(wgpu::TextureFormat::R32Float),
            dimension: Some(wgpu::TextureViewDimension::D2),
            aspect: wgpu::TextureAspect::All,
            base_mip_level: 0,
            mip_level_count: Some(mip_count),
            base_array_layer: 0,
            array_layer_count: Some(1),
        });
        let mip_views: Vec<_> = (0..mip_count)
            .map(|mip| {
                pyramid_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("octree-viewer-hzb-mip"),
                    usage: None,
                    format: Some(wgpu::TextureFormat::R32Float),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    aspect: wgpu::TextureAspect::All,
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    base_array_layer: 0,
                    array_layer_count: Some(1),
                })
            })
            .collect();
        self.depth_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("octree-viewer-hzb-depth-bind-group"),
            layout: &self.depth_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &depth_texture.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&mip_views[0]),
                },
            ],
        });
        self.reduce_bind_groups = mip_views
            .windows(2)
            .map(|views| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("octree-viewer-hzb-reduce-bind-group"),
                    layout: &self.reduce_pipeline.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&views[0]),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&views[1]),
                        },
                    ],
                })
            })
            .collect();
        #[expect(
            clippy::used_underscore_binding,
            reason = "the texture is only held so its views stay valid; the underscore says it is never read"
        )]
        {
            self._pyramid_texture = pyramid_texture;
        }
        self.pyramid_view = pyramid_view;
        self.viewport = viewport;
        self.pyramid_size = pyramid_size;
        self.mip_count = mip_count;
        self.cull_bind_group = None;
        self.indexed_bind_group = None;
        self.visible_indices = None;
        self.indirect_args = None;
        self.instance_count = 0;
        self.resource_bytes = pyramid_bytes(pyramid_size, mip_count);
        true
    }

    pub(crate) fn build_depth_pyramid(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("octree-viewer-hzb-build"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.depth_pipeline);
        pass.set_bind_group(0, &self.depth_bind_group, &[]);
        pass.dispatch_workgroups(
            self.pyramid_size[0].div_ceil(PYRAMID_WORKGROUP_SIZE),
            self.pyramid_size[1].div_ceil(PYRAMID_WORKGROUP_SIZE),
            1,
        );
        pass.set_pipeline(&self.reduce_pipeline);
        for (level, bind_group) in self.reduce_bind_groups.iter().enumerate() {
            pass.set_bind_group(0, bind_group, &[]);
            let width = (self.pyramid_size[0] >> (level + 1)).max(1);
            let height = (self.pyramid_size[1] >> (level + 1)).max(1);
            pass.dispatch_workgroups(
                width.div_ceil(PYRAMID_WORKGROUP_SIZE),
                height.div_ceil(PYRAMID_WORKGROUP_SIZE),
                1,
            );
        }
    }

    pub(crate) fn cull(&self, encoder: &mut wgpu::CommandEncoder, queue: &wgpu::Queue) {
        let Some(cull_bind_group) = &self.cull_bind_group else {
            return;
        };
        let Some(indirect_args) = &self.indirect_args else {
            return;
        };
        queue.write_buffer(
            indirect_args,
            0,
            bytemuck::bytes_of(&DrawIndexedIndirectArgs {
                index_count: CUBE_INDEX_COUNT,
                instance_count: 0,
                first_index: 0,
                base_vertex: 0,
                first_instance: 0,
            }),
        );
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::bytes_of(&CullParams {
                viewport: self.viewport,
                pyramid_size: self.pyramid_size,
                instance_count: self.instance_count as u32,
                mip_count: self.mip_count,
                depth_epsilon: 0.001,
                _padding: 0,
            }),
        );
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("octree-viewer-hzb-cull"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.cull_pipeline);
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(1, cull_bind_group, &[]);
        pass.dispatch_workgroups(
            (self.instance_count as u32).div_ceil(CULL_WORKGROUP_SIZE),
            1,
            1,
        );
    }

    pub(crate) fn indexed_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.indexed_pipeline
    }

    pub(crate) fn camera_bind_group(&self) -> &wgpu::BindGroup {
        &self.camera_bind_group
    }

    pub(crate) fn indexed_bind_group(&self) -> Option<&wgpu::BindGroup> {
        self.indexed_bind_group.as_ref()
    }

    pub(crate) fn indirect_args(&self) -> Option<&wgpu::Buffer> {
        self.indirect_args.as_ref()
    }

    pub(crate) fn requested_buffer_bytes(&self) -> u64 {
        self.params_buffer.size()
            + self.visible_indices.as_ref().map_or(0, wgpu::Buffer::size)
            + self.indirect_args.as_ref().map_or(0, wgpu::Buffer::size)
    }

    pub(crate) fn logical_texture_bytes(&self) -> u64 {
        pyramid_bytes(self.pyramid_size, self.mip_count)
    }

    pub(crate) fn resource_bytes(&self) -> u64 {
        self.resource_bytes
    }
}

const CUBE_INDEX_COUNT: u32 = 36;

fn pyramid_bytes(size: [u32; 2], mip_count: u32) -> u64 {
    (0..mip_count)
        .map(|mip| u64::from((size[0] >> mip).max(1)) * u64::from((size[1] >> mip).max(1)) * 4)
        .sum()
}

fn create_pyramid_texture(device: &wgpu::Device, size: [u32; 2], mip_count: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("octree-viewer-hzb-pyramid"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: mip_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn texture_storage_layout(
    device: &wgpu::Device,
    binding_base: u32,
    depth_source: bool,
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("octree-viewer-hzb-texture-layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: binding_base,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: if depth_source {
                        wgpu::TextureSampleType::Depth
                    } else {
                        wgpu::TextureSampleType::Float { filterable: false }
                    },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: binding_base + 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::R32Float,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            },
        ],
    })
}

fn create_cull_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("octree-viewer-hzb-cull-layout"),
        entries: &[
            storage_entry(0, true, wgpu::ShaderStages::COMPUTE),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            storage_entry(2, false, wgpu::ShaderStages::COMPUTE),
            storage_entry(3, false, wgpu::ShaderStages::COMPUTE),
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(size_of::<CullParams>() as u64),
                },
                count: None,
            },
        ],
    })
}

fn create_indexed_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("octree-viewer-hzb-indexed-layout"),
        entries: &[
            storage_entry(0, true, wgpu::ShaderStages::VERTEX),
            storage_entry(1, true, wgpu::ShaderStages::VERTEX),
        ],
    })
}

fn storage_entry(
    binding: u32,
    read_only: bool,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn compute_pipeline(
    device: &wgpu::Device,
    label: &'static str,
    source: &'static str,
    entry_point: &'static str,
    layouts: &[Option<&wgpu::BindGroupLayout>],
) -> wgpu::ComputePipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: layouts,
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some(entry_point),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    })
}

fn create_indexed_pipeline(
    device: &wgpu::Device,
    color_format: wgpu::TextureFormat,
    uniform_layout: &wgpu::BindGroupLayout,
    indexed_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("octree-viewer-hzb-indexed-shader"),
        source: wgpu::ShaderSource::Wgsl(INDEXED_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("octree-viewer-hzb-indexed-pipeline-layout"),
        bind_group_layouts: &[Some(uniform_layout), Some(indexed_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("octree-viewer-hzb-indexed-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_indexed"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 24,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
            })],
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
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_indexed"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

const HZB_SHADER: &str = include_str!("shaders/hzb_reduce.wgsl");

const CULL_SHADER: &str = include_str!("shaders/hzb_cull.wgsl");

const INDEXED_SHADER: &str = include_str!("shaders/hzb_indexed.wgsl");

#[cfg(test)]
mod tests {
    use super::{CUBE_INDEX_COUNT, DrawIndexedIndirectArgs};

    #[test]
    fn hzb_shaders_parse_as_wgsl() {
        for source in [super::HZB_SHADER, super::CULL_SHADER, super::INDEXED_SHADER] {
            let module = naga::front::wgsl::parse_str(source).expect("valid HZB WGSL");
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .expect("valid HZB shader module");
        }
    }

    #[test]
    fn hzb_gpu_pipelines_validate_when_adapter_is_available() {
        pollster::block_on(async {
            let instance = wgpu::Instance::default();
            let Ok(adapter) = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: true,
                    ..Default::default()
                })
                .await
            else {
                return;
            };
            let Ok((device, _queue)) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("octree-viewer-hzb-test-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                    ..Default::default()
                })
                .await
            else {
                return;
            };
            let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("octree-viewer-hzb-test-camera"),
                size: size_of::<super::ViewUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM,
                mapped_at_creation: false,
            });
            let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("octree-viewer-hzb-test-depth"),
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let result = super::HzbResources::new(
                &device,
                &adapter,
                &camera_buffer,
                &depth_texture,
                wgpu::TextureFormat::Rgba8Unorm,
                [64, 64],
                device.limits().max_texture_dimension_2d,
            )
            .await;
            if let Err(error) = result {
                panic!("HZB pipeline and bind group validation: {error:?}");
            }
        });
    }

    #[test]
    fn hzb_culls_hidden_voxels_and_keeps_unoccluded_voxels() {
        pollster::block_on(async {
            use wgpu::util::DeviceExt;

            let instance = wgpu::Instance::default();
            let Ok(adapter) = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: true,
                    ..Default::default()
                })
                .await
            else {
                return;
            };
            let Ok((device, queue)) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("octree-viewer-hzb-cull-test-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                    ..Default::default()
                })
                .await
            else {
                return;
            };
            let uniform = super::ViewUniform {
                matrix: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
                render_origin: [0.0, 0.0, 0.0, 1.0],
                style: [0.0, 1.0, 0.0, 1.0],
                intensity_range: [0.0, 1.0, 0.0, 0.0],
            };
            let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("octree-viewer-hzb-cull-test-camera"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("octree-viewer-hzb-cull-test-depth"),
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let Some(mut hzb) = super::HzbResources::new(
                &device,
                &adapter,
                &camera_buffer,
                &depth_texture,
                wgpu::TextureFormat::Rgba8Unorm,
                [64, 64],
                device.limits().max_texture_dimension_2d,
            )
            .await
            .expect("HZB resource validation") else {
                return;
            };
            assert_eq!(
                hzb.requested_buffer_bytes(),
                size_of::<super::CullParams>() as u64
            );
            let pyramid_bytes = hzb.logical_texture_bytes();
            assert!(pyramid_bytes > 64 * 64 * 4);
            let voxels = [0.7, 0.1].map(|depth| crate::InstanceRaw {
                center: [0.0, 0.0, depth],
                size: 0.1,
                color: [1.0, 1.0, 1.0, 1.0],
                intensity: 0.0,
                classification: -1.0,
            });
            let source = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("octree-viewer-hzb-cull-test-voxel"),
                contents: bytemuck::cast_slice(&voxels),
                usage: wgpu::BufferUsages::STORAGE,
            });
            hzb.prepare_scene(
                &device,
                &source,
                (size_of::<crate::InstanceRaw>() * voxels.len()) as u64,
                voxels.len(),
            );

            assert_eq!(
                hzb.requested_buffer_bytes(),
                (size_of::<super::CullParams>()
                    + 2 * size_of::<u32>()
                    + size_of::<super::DrawIndexedIndirectArgs>()) as u64
            );
            assert_eq!(hzb.logical_texture_bytes(), pyramid_bytes);
            let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mut clear_encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let _pass = clear_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("octree-viewer-hzb-cull-test-occluder"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(0.3),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    occlusion_query_set: None,
                    timestamp_writes: None,
                    multiview_mask: None,
                });
            }
            queue.submit([clear_encoder.finish()]);

            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            hzb.build_depth_pyramid(&mut encoder);
            hzb.cull(&mut encoder, &queue);
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("octree-viewer-hzb-cull-test-count"),
                size: (size_of::<DrawIndexedIndirectArgs>() + size_of::<u32>()) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_buffer_to_buffer(
                hzb.indirect_args().expect("prepared indirect args"),
                0,
                &readback,
                0,
                size_of::<DrawIndexedIndirectArgs>() as u64,
            );
            encoder.copy_buffer_to_buffer(
                hzb.visible_indices
                    .as_ref()
                    .expect("prepared visible indices"),
                0,
                &readback,
                size_of::<DrawIndexedIndirectArgs>() as u64,
                size_of::<u32>() as u64,
            );
            queue.submit([encoder.finish()]);
            let (sender, receiver) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("GPU poll");
            receiver
                .recv()
                .expect("GPU mapping callback")
                .expect("GPU readback mapping");
            let mapped = readback
                .slice(..)
                .get_mapped_range()
                .expect("GPU mapped range");
            let visible = *bytemuck::from_bytes::<[u32; 6]>(&mapped);
            drop(mapped);
            readback.unmap();
            assert_eq!(visible, [CUBE_INDEX_COUNT, 1, 0, 0, 0, 1]);
            hzb.reset_scene();
            assert_eq!(
                hzb.requested_buffer_bytes(),
                size_of::<super::CullParams>() as u64
            );
            assert_eq!(hzb.logical_texture_bytes(), pyramid_bytes);
            hzb.prepare_scene(&device, &source, source.size(), 0);
            assert_eq!(
                hzb.requested_buffer_bytes(),
                (size_of::<super::CullParams>()
                    + size_of::<u32>()
                    + size_of::<super::DrawIndexedIndirectArgs>()) as u64
            );
        });
    }

    /// Regression: the pyramid is padded to powers of two but holds depth 1:1 with pixels,
    /// so a non-power-of-two viewport must be looked up without rescaling.
    #[test]
    fn hzb_lookup_matches_pixels_at_non_power_of_two_viewport() {
        pollster::block_on(async {
            use wgpu::util::DeviceExt;

            const VIEWPORT: [u32; 2] = [1280, 720];
            let instance = wgpu::Instance::default();
            let Ok(adapter) = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: true,
                    ..Default::default()
                })
                .await
            else {
                return;
            };
            let Ok((device, queue)) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("octree-viewer-hzb-npot-test-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                    ..Default::default()
                })
                .await
            else {
                return;
            };
            let uniform = super::ViewUniform {
                matrix: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
                render_origin: [0.0, 0.0, 0.0, 1.0],
                style: [0.0, 1.0, 0.0, 1.0],
                intensity_range: [0.0, 1.0, 0.0, 0.0],
            };
            let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("octree-viewer-hzb-npot-test-camera"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("octree-viewer-hzb-npot-test-depth"),
                size: wgpu::Extent3d {
                    width: VIEWPORT[0],
                    height: VIEWPORT[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let Some(mut hzb) = super::HzbResources::new(
                &device,
                &adapter,
                &camera_buffer,
                &depth_texture,
                wgpu::TextureFormat::Rgba8Unorm,
                VIEWPORT,
                device.limits().max_texture_dimension_2d,
            )
            .await
            .expect("HZB resource validation") else {
                return;
            };
            // Pixel x = 400 and x = 900 at y = 360; the occluder covers pixels x >= 600.
            let ndc_x = |pixel: f32| pixel / (VIEWPORT[0] as f32 / 2.0) - 1.0;
            let voxels = [400.0, 900.0].map(|pixel| crate::InstanceRaw {
                center: [ndc_x(pixel), 0.0, 0.7],
                size: 0.02,
                color: [1.0, 1.0, 1.0, 1.0],
                intensity: 0.0,
                classification: -1.0,
            });
            let source = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("octree-viewer-hzb-npot-test-voxel"),
                contents: bytemuck::cast_slice(&voxels),
                usage: wgpu::BufferUsages::STORAGE,
            });
            hzb.prepare_scene(
                &device,
                &source,
                (size_of::<crate::InstanceRaw>() * voxels.len()) as u64,
                voxels.len(),
            );

            let occluder_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("octree-viewer-hzb-npot-test-occluder"),
                source: wgpu::ShaderSource::Wgsl(
                    "@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                        let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
                        return vec4<f32>(p * 4.0 - 2.0, 0.3, 1.0);
                    }"
                    .into(),
                ),
            });
            let occluder_pipeline =
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("octree-viewer-hzb-npot-test-occluder"),
                    layout: None,
                    vertex: wgpu::VertexState {
                        module: &occluder_shader,
                        entry_point: Some("vs"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::Always),
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    multisample: wgpu::MultisampleState::default(),
                    fragment: None,
                    multiview_mask: None,
                    cache: None,
                });
            let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("octree-viewer-hzb-npot-test-occluder"),
                    color_attachments: &[],
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
                pass.set_pipeline(&occluder_pipeline);
                pass.set_scissor_rect(600, 0, VIEWPORT[0] - 600, VIEWPORT[1]);
                pass.draw(0..3, 0..1);
            }
            hzb.build_depth_pyramid(&mut encoder);
            hzb.cull(&mut encoder, &queue);
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("octree-viewer-hzb-npot-test-count"),
                size: (size_of::<DrawIndexedIndirectArgs>() + size_of::<u32>()) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_buffer_to_buffer(
                hzb.indirect_args().expect("prepared indirect args"),
                0,
                &readback,
                0,
                size_of::<DrawIndexedIndirectArgs>() as u64,
            );
            encoder.copy_buffer_to_buffer(
                hzb.visible_indices
                    .as_ref()
                    .expect("prepared visible indices"),
                0,
                &readback,
                size_of::<DrawIndexedIndirectArgs>() as u64,
                size_of::<u32>() as u64,
            );
            queue.submit([encoder.finish()]);
            let (sender, receiver) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("GPU poll");
            receiver
                .recv()
                .expect("GPU mapping callback")
                .expect("GPU readback mapping");
            let mapped = readback
                .slice(..)
                .get_mapped_range()
                .expect("GPU mapped range");
            let visible = *bytemuck::from_bytes::<[u32; 6]>(&mapped);
            drop(mapped);
            readback.unmap();
            // Oracle: voxel 0 sits over cleared depth (visible); voxel 1 behind the occluder.
            assert_eq!(visible, [CUBE_INDEX_COUNT, 1, 0, 0, 0, 0]);
        });
    }
}
