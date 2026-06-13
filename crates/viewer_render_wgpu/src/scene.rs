use bytemuck::{Pod, Zeroable};
use viewer_core::VisibleVoxelBatch;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct InstanceRaw {
    pub center: [f32; 3],
    pub size: f32,
    pub color: [f32; 4],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderStats {
    pub instance_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderScene {
    instances: Vec<InstanceRaw>,
}

impl RenderScene {
    pub fn from_visible_batch(batch: &VisibleVoxelBatch) -> Self {
        let instances = batch
            .voxels()
            .iter()
            .map(|voxel| InstanceRaw {
                center: voxel.center,
                size: voxel.size,
                color: voxel.color_linear_rgba(),
            })
            .collect();

        Self { instances }
    }

    pub fn instances(&self) -> &[InstanceRaw] {
        &self.instances
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    pub fn stats(&self) -> RenderStats {
        RenderStats {
            instance_count: self.instance_count(),
        }
    }
}

pub fn instance_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![2 => Float32x3, 3 => Float32, 4 => Float32x4];

    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<InstanceRaw>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &ATTRIBUTES,
    }
}

pub const VOXEL_SHADER_WGSL: &str = r#"
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) instance_center: vec3<f32>,
    @location(3) instance_size: f32,
    @location(4) instance_color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> view_proj: mat4x4<f32>;

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let world_position = input.instance_center + input.position * input.instance_size;

    var out: VertexOutput;
    out.clip_position = view_proj * vec4<f32>(world_position, 1.0);
    out.color = input.instance_color;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}
"#;
