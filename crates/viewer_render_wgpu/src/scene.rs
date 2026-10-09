use bytemuck::{Pod, Zeroable};
use viewer_core::{AttributeColumn, AttributeValues, Point, PointBlock, VisibleVoxelBatch};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct InstanceRaw {
    pub center: [f32; 3],
    pub size: f32,
    pub color: [f32; 4],
    pub intensity: f32,
    pub classification: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderStats {
    pub instance_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorMode {
    #[default]
    Rgb,
    Intensity,
    Elevation,
    Classification,
}

impl ColorMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rgb => "rgb",
            Self::Intensity => "intensity",
            Self::Elevation => "elevation",
            Self::Classification => "classification",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "rgb" => Some(Self::Rgb),
            "intensity" => Some(Self::Intensity),
            "elevation" => Some(Self::Elevation),
            "classification" => Some(Self::Classification),
            _ => None,
        }
    }

    pub(crate) const fn shader_value(self) -> f32 {
        match self {
            Self::Rgb => 0.0,
            Self::Intensity => 1.0,
            Self::Elevation => 2.0,
            Self::Classification => 3.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneBounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderScene {
    instances: Vec<InstanceRaw>,
    intensity_range: Option<[f32; 2]>,
    elevation_range: Option<[f32; 2]>,
}

impl RenderScene {
    pub fn from_points(points: &[Point]) -> Self {
        let instances: Vec<_> = points
            .iter()
            .map(|point| InstanceRaw {
                center: point.position,
                size: 1.0,
                color: point.color.to_linear_rgba(),
                intensity: -1.0,
                classification: -1.0,
            })
            .collect();

        let elevation_range = position_elevation_range(&instances);
        Self {
            instances,
            intensity_range: None,
            elevation_range,
        }
    }

    pub fn from_visible_batch(batch: &VisibleVoxelBatch) -> Self {
        let instances: Vec<_> = batch
            .voxels()
            .iter()
            .map(|voxel| InstanceRaw {
                center: voxel.center,
                size: voxel.size,
                color: voxel.color_linear_rgba(),
                intensity: -1.0,
                classification: -1.0,
            })
            .collect();

        let elevation_range = position_elevation_range(&instances);
        Self {
            instances,
            intensity_range: None,
            elevation_range,
        }
    }

    pub fn from_point_block(block: &PointBlock) -> Self {
        let points = block.points().points();
        let intensity = scalar_attribute(block.attributes(), "intensity", points.len());
        let classification = scalar_attribute(block.attributes(), "classification", points.len());
        let instances: Vec<_> = points
            .iter()
            .enumerate()
            .map(|(index, point)| InstanceRaw {
                center: point.position,
                size: 1.0,
                color: point.color.to_linear_rgba(),
                intensity: intensity.as_ref().map_or(-1.0, |values| values[index]),
                classification: classification.as_ref().map_or(-1.0, |values| values[index]),
            })
            .collect();
        let intensity_range = attribute_range(intensity.as_deref());
        let elevation_range = position_elevation_range(&instances);
        Self {
            instances,
            intensity_range,
            elevation_range,
        }
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

    pub const fn intensity_range(&self) -> Option<[f32; 2]> {
        self.intensity_range
    }

    pub const fn elevation_range(&self) -> Option<[f32; 2]> {
        self.elevation_range
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

fn scalar_attribute(
    columns: &[AttributeColumn],
    name: &str,
    point_count: usize,
) -> Option<Vec<f32>> {
    let column = columns.iter().find(|column| {
        column.descriptor().name().eq_ignore_ascii_case(name)
            && column.descriptor().component_count() == 1
    })?;
    let values = match column.values() {
        AttributeValues::U8(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::I8(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::U16(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::I16(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::U32(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::I32(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::U64(values) => values.iter().map(|value| *value as f64).collect(),
        AttributeValues::I64(values) => values.iter().map(|value| *value as f64).collect(),
        AttributeValues::F32(values) => values.iter().map(|value| f64::from(*value)).collect(),
        AttributeValues::F64(values) => values.clone(),
    };
    if values.len() != point_count
        || values
            .iter()
            .any(|value| !value.is_finite() || value.abs() > f64::from(f32::MAX))
    {
        return None;
    }
    Some(values.into_iter().map(|value| value as f32).collect())
}

fn attribute_range(values: Option<&[f32]>) -> Option<[f32; 2]> {
    let values = values?;
    let mut values = values.iter().copied();
    let first = values.next()?;
    Some(values.fold([first, first], |[min, max], value| {
        [min.min(value), max.max(value)]
    }))
}

fn position_elevation_range(instances: &[InstanceRaw]) -> Option<[f32; 2]> {
    let mut elevations = instances.iter().map(|instance| instance.center[2]);
    let first = elevations.next()?;
    Some(elevations.fold([first, first], |[min, max], z| [min.min(z), max.max(z)]))
}

pub fn instance_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        2 => Float32x3,
        3 => Float32,
        4 => Float32x4,
        5 => Float32,
        6 => Float32
    ];

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
    @location(5) instance_intensity: f32,
    @location(6) instance_classification: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

struct CameraUniform {
    view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    style: vec4<f32>,
    intensity_range: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let local_position = input.instance_center - camera.render_origin.xyz
        + input.position * input.instance_size * camera.style.y;

    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(local_position, 1.0);
    let rgb_intensity = dot(input.instance_color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    var color = input.instance_color;
    if camera.style.x == 1.0 {
        let intensity_span = camera.intensity_range.y - camera.intensity_range.x;
        let scaled_intensity = clamp((input.instance_intensity - camera.intensity_range.x)
            / max(intensity_span, 0.000001), 0.0, 1.0);
        let normalized = select(rgb_intensity, select(0.5, scaled_intensity, intensity_span > 0.000001), input.instance_intensity >= 0.0);
        color = vec4<f32>(vec3<f32>(normalized), 1.0);
    } else if camera.style.x == 2.0 {
        let height = clamp((input.instance_center.z - camera.style.z)
            / max(camera.style.w - camera.style.z, 0.000001), 0.0, 1.0);
        color = vec4<f32>(mix(vec3<f32>(0.08, 0.25, 0.9), vec3<f32>(0.95, 0.22, 0.08), height), 1.0);
    } else if camera.style.x == 3.0 && input.instance_classification >= 0.0 {
        let hue = fract(input.instance_classification * 0.61803398875);
        let rgb = clamp(
            abs(fract(hue + vec3<f32>(0.0, 0.6666667, 0.3333333)) * 6.0 - 3.0) - 1.0,
            vec3<f32>(0.0),
            vec3<f32>(1.0),
        );
        color = vec4<f32>(rgb, 1.0);
    }
    out.color = color;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}
"#;

#[cfg(test)]
mod tests {
    use viewer_core::{
        AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId,
        ColorRgb, Point, PointBatch, PointBlock,
    };

    use super::{ColorMode, InstanceRaw, RenderScene};

    #[test]
    fn scene_bounds_include_instance_extents_in_f64() {
        let scene = RenderScene {
            instances: vec![InstanceRaw {
                center: [1.0e8, -4.0, 2.0],
                size: 2.0,
                color: [1.0; 4],
                intensity: -1.0,
                classification: -1.0,
            }],
            intensity_range: None,
            elevation_range: Some([2.0, 2.0]),
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
                intensity: -1.0,
                classification: -1.0,
            }],
            intensity_range: None,
            elevation_range: None,
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

    #[test]
    fn color_modes_parse_only_supported_names() {
        assert_eq!(ColorMode::parse("rgb"), Some(ColorMode::Rgb));
        assert_eq!(
            ColorMode::parse("classification"),
            Some(ColorMode::Classification)
        );
        assert_eq!(ColorMode::parse("RGB"), None);
    }

    #[test]
    fn point_block_attributes_are_attached_and_intensity_range_is_computed() {
        let descriptor = |name| AttributeDescriptor::new(name, AttributeDataType::U16, 1).unwrap();
        let block = PointBlock::new(
            BlockId::new(1),
            PointBatch::new(vec![
                Point::new([0.0, 0.0, 2.0], ColorRgb::new(1, 2, 3)),
                Point::new([1.0, 0.0, 5.0], ColorRgb::new(4, 5, 6)),
            ]),
            vec![
                AttributeColumn::new(
                    descriptor("Intensity"),
                    AttributeValues::U16(vec![10, 90]),
                    2,
                )
                .unwrap(),
                AttributeColumn::new(
                    descriptor("classification"),
                    AttributeValues::U16(vec![2, 7]),
                    2,
                )
                .unwrap(),
            ],
        );

        let scene = RenderScene::from_point_block(&block);

        assert_eq!(scene.intensity_range(), Some([10.0, 90.0]));
        assert_eq!(scene.instances()[0].intensity, 10.0);
        assert_eq!(scene.instances()[1].classification, 7.0);
        assert_eq!(scene.instances()[1].center[2], 5.0);
        assert_eq!(scene.elevation_range(), Some([2.0, 5.0]));
    }
}
