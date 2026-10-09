use bytemuck::{Pod, Zeroable};
use viewer_core::{Point, VisibleVoxelBatch};

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneBounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderScene {
    instances: Vec<InstanceRaw>,
}

impl RenderScene {
    pub fn from_points(points: &[Point]) -> Self {
        let instances = points
            .iter()
            .map(|point| InstanceRaw {
                center: point.position,
                size: 1.0,
                color: point.color.to_linear_rgba(),
            })
            .collect();

        Self { instances }
    }

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

    /// Returns finite bounds that include each instance's full cube size.
    pub fn bounds(&self) -> Option<SceneBounds> {
        let first = self.instances.first()?;
        let first_half_size = f64::from(first.size) * 0.5;
        if !first.size.is_finite()
            || first.size <= 0.0
            || first
                .center
                .iter()
                .any(|coordinate| !coordinate.is_finite())
        {
            return None;
        }

        let mut bounds = SceneBounds {
            min: first
                .center
                .map(|coordinate| f64::from(coordinate) - first_half_size),
            max: first
                .center
                .map(|coordinate| f64::from(coordinate) + first_half_size),
        };
        for instance in &self.instances[1..] {
            if !instance.size.is_finite()
                || instance.size <= 0.0
                || instance
                    .center
                    .iter()
                    .any(|coordinate| !coordinate.is_finite())
            {
                return None;
            }
            let half_size = f64::from(instance.size) * 0.5;
            for axis in 0..3 {
                let center = f64::from(instance.center[axis]);
                bounds.min[axis] = bounds.min[axis].min(center - half_size);
                bounds.max[axis] = bounds.max[axis].max(center + half_size);
            }
        }
        Some(bounds)
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

struct CameraUniform {
    view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let local_position = input.instance_center - camera.render_origin.xyz
        + input.position * input.instance_size;

    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(local_position, 1.0);
    out.color = input.instance_color;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}
"#;

#[cfg(test)]
mod tests {
    use super::{InstanceRaw, RenderScene};

    #[test]
    fn scene_bounds_include_instance_extents_in_f64() {
        let scene = RenderScene {
            instances: vec![InstanceRaw {
                center: [1.0e8, -4.0, 2.0],
                size: 2.0,
                color: [1.0; 4],
            }],
        };

        let bounds = scene.bounds().unwrap();

        assert_eq!(bounds.min, [99_999_999.0, -5.0, 1.0]);
        assert_eq!(bounds.max, [100_000_001.0, -3.0, 3.0]);
    }

    #[test]
    fn empty_or_invalid_scenes_have_no_bounds() {
        assert_eq!(RenderScene::default().bounds(), None);
        let scene = RenderScene {
            instances: vec![InstanceRaw {
                center: [f32::NAN, 0.0, 0.0],
                size: 1.0,
                color: [1.0; 4],
            }],
        };
        assert_eq!(scene.bounds(), None);
    }

    #[test]
    fn voxel_shader_parses_and_validates() {
        let module = naga::front::wgsl::parse_str(super::VOXEL_SHADER_WGSL).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
}
