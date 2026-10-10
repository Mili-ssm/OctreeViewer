use bytemuck::{Pod, Zeroable};
use viewer_core::{
    Point, PointBatch, PointBlock, SparseVoxelStore, VisibleVoxelBatch, VoxelizationConfig,
};

use crate::attributes::ScalarAttribute;

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
    pub fn from_instances(instances: Vec<InstanceRaw>) -> Self {
        let elevation_range = position_elevation_range(&instances);
        let intensity_range = value_range(
            instances
                .iter()
                .map(|instance| instance.intensity)
                .filter(|value| *value >= 0.0),
        );
        Self {
            instances,
            intensity_range,
            elevation_range,
        }
    }

    pub fn from_points(points: &[Point]) -> Self {
        let points: Vec<_> = points
            .iter()
            .copied()
            .filter(|point| point.position.iter().all(|value| value.is_finite()))
            .collect();
        let point_size = point_glyph_size(&points);
        let origin = points.iter().fold([f32::INFINITY; 3], |mut min, point| {
            for (axis, value) in point.position.iter().enumerate() {
                min[axis] = min[axis].min(*value);
            }
            min
        });
        let instances = VoxelizationConfig::new(point_size, origin)
            .ok()
            .and_then(|config| {
                let mut store = SparseVoxelStore::new(config);
                store.insert_batch(&PointBatch::new(points)).ok()?;
                let mut voxels: Vec<_> = store.iter_voxels().collect();
                voxels.sort_by_key(|voxel| (voxel.coord.x, voxel.coord.y, voxel.coord.z));
                Some(
                    voxels
                        .into_iter()
                        .filter_map(|voxel| {
                            let size = safe_cell_size(voxel.center, voxel.size)?;
                            Some(InstanceRaw {
                                center: voxel.center,
                                size,
                                color: voxel.color.to_linear_rgba(),
                                intensity: -1.0,
                                classification: -1.0,
                            })
                        })
                        .collect(),
                )
            })
            .unwrap_or_default();
        Self::from_instances(instances)
    }

    pub fn from_visible_batch(batch: &VisibleVoxelBatch) -> Self {
        let instances: Vec<_> = batch
            .voxels()
            .iter()
            .filter_map(|voxel| {
                let size = safe_cell_size(voxel.center, voxel.size)?;
                Some(InstanceRaw {
                    center: voxel.center,
                    size,
                    color: voxel.color_linear_rgba(),
                    intensity: -1.0,
                    classification: -1.0,
                })
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
        let point_size = point_glyph_size(points);
        let intensity = ScalarAttribute::find(block, "intensity");
        let classification = ScalarAttribute::find(block, "classification");
        let instances: Vec<_> = points
            .iter()
            .enumerate()
            .map(|(index, point)| InstanceRaw {
                center: point.position,
                size: point_size,
                color: point.color.to_linear_rgba(),
                intensity: intensity
                    .and_then(|values| values.value_at(index))
                    .unwrap_or(-1.0),
                classification: classification
                    .and_then(|values| values.value_at(index))
                    .unwrap_or(-1.0),
            })
            .collect();
        let intensity_range = intensity
            .and_then(|_| value_range(instances.iter().map(|instance| instance.intensity)));
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

fn point_glyph_size(points: &[Point]) -> f32 {
    const DEGENERATE_GLYPH_SIZE: f32 = 1.0e-5;
    const SAMPLE_SIZE: usize = 256;

    if points.len() < 2 {
        return DEGENERATE_GLYPH_SIZE;
    }

    let mut sample = Vec::with_capacity(SAMPLE_SIZE.min(points.len()));
    let mut random_state = 0x6a09_e667_f3bc_c909_u64;
    let mut finite_count = 0_usize;
    for point in points {
        let position = point.position;
        if position.iter().any(|value| !value.is_finite()) {
            continue;
        }
        let index = finite_count;
        finite_count += 1;
        let position = position.map(f64::from);
        if sample.len() < SAMPLE_SIZE {
            sample.push(position);
        } else {
            random_state = splitmix64(random_state);
            let replacement = (random_state % (index as u64 + 1)) as usize;
            if replacement < SAMPLE_SIZE {
                sample[replacement] = position;
            }
        }
    }

    if sample.len() < 2 {
        return DEGENERATE_GLYPH_SIZE;
    }

    let mut ranges = [0.0_f64; 3];
    for axis in 0..3 {
        let mut coordinates: Vec<_> = sample.iter().map(|position| position[axis]).collect();
        coordinates.sort_by(f64::total_cmp);
        let (low, high) = if finite_count > SAMPLE_SIZE {
            let tail = (coordinates.len() - 1) / 20;
            (coordinates[tail], coordinates[coordinates.len() - 1 - tail])
        } else {
            (coordinates[0], coordinates[coordinates.len() - 1])
        };
        ranges[axis] = high - low;
    }
    ranges.sort_by(f64::total_cmp);
    let largest_extent = ranges[2];
    if largest_extent <= 0.0 || !largest_extent.is_finite() {
        DEGENERATE_GLYPH_SIZE
    } else {
        let dimension = ranges
            .iter()
            .filter(|extent| **extent > largest_extent * 1.0e-4)
            .count();
        let estimated_count = finite_count.saturating_sub(1).max(1) as f64;
        let sample_extent_correction = if finite_count > SAMPLE_SIZE {
            1.25
        } else {
            1.0
        };
        (largest_extent * sample_extent_correction / estimated_count.powf(1.0 / dimension as f64))
            .clamp(f64::from(DEGENERATE_GLYPH_SIZE), f64::from(f32::MAX)) as f32
    }
}

fn safe_cell_size(center: [f32; 3], size: f32) -> Option<f32> {
    let rounding_bound = center
        .iter()
        .map(|value| {
            let magnitude = value.abs();
            let bits = magnitude.to_bits();
            let lower = if bits == 0 {
                0.0
            } else {
                magnitude - f32::from_bits(bits - 1)
            };
            let upper = bits
                .checked_add(1)
                .map(f32::from_bits)
                .filter(|next| next.is_finite())
                .map_or(lower, |next| next - magnitude);
            f64::from(lower.max(upper)) / 2.0
        })
        .fold(0.0, f64::max);
    let safe_size = f64::from(size) * (1.0 - 1.0e-6) - 2.0 * rounding_bound;
    (safe_size.is_finite() && safe_size > 0.0 && safe_size <= f64::from(f32::MAX))
        .then_some(safe_size as f32)
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn value_range(mut values: impl Iterator<Item = f32>) -> Option<[f32; 2]> {
    let first = values.next()?;
    Some(values.fold([first, first], |[min, max], value| {
        [min.min(value), max.max(value)]
    }))
}

fn position_elevation_range(instances: &[InstanceRaw]) -> Option<[f32; 2]> {
    value_range(instances.iter().map(|instance| instance.center[2]))
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
    @location(1) normal: vec3<f32>,
    @location(2) depth: f32,
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
    let local_position = input.instance_center * camera.render_origin.w - camera.render_origin.xyz
        + input.position * input.instance_size * camera.style.y * camera.render_origin.w;

    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(local_position, 1.0);
    out.depth = out.clip_position.z / out.clip_position.w;
    out.normal = input.normal;
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
    if camera.intensity_range.z > 0.5 {
        let light_direction = normalize(vec3<f32>(-0.35, -0.45, 0.82));
        let face_light = 0.68 + 0.32 * max(dot(normalize(input.normal), light_direction), 0.0);
        let depth_light = 1.0 - 0.12 * clamp(input.depth, 0.0, 1.0);
        return vec4<f32>(input.color.rgb * face_light * depth_light, input.color.a);
    }
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
        assert!((scene.instances()[0].size - 3.0).abs() < 1e-6);
    }

    #[test]
    fn point_block_intensity_range_includes_finite_negative_values() {
        let block = PointBlock::new(
            BlockId::new(1),
            PointBatch::new(vec![
                Point::new([0.0; 3], ColorRgb::new(0, 0, 0)),
                Point::new([1.0; 3], ColorRgb::new(0, 0, 0)),
            ]),
            vec![
                AttributeColumn::new(
                    AttributeDescriptor::new("intensity", AttributeDataType::F32, 1).unwrap(),
                    AttributeValues::F32(vec![-2.0, 3.0]),
                    2,
                )
                .unwrap(),
            ],
        );

        let scene = RenderScene::from_point_block(&block);

        assert_eq!(scene.intensity_range(), Some([-2.0, 3.0]));
    }

    #[test]
    fn point_glyph_size_scales_with_cloud_extent_and_density() {
        let points = vec![
            Point::new([0.0; 3], ColorRgb::new(1, 2, 3)),
            Point::new([0.1, 0.0, 0.0], ColorRgb::new(1, 2, 3)),
            Point::new([0.0, 0.1, 0.0], ColorRgb::new(1, 2, 3)),
            Point::new([0.0, 0.0, 0.1], ColorRgb::new(1, 2, 3)),
        ];

        let scene = RenderScene::from_points(&points);

        assert!((scene.instances()[0].size - 0.1 / 3.0_f32.cbrt()).abs() < 1e-6);
    }

    #[test]
    fn estimated_glyphs_approximate_regular_two_and_three_dimensional_spacing() {
        let two_dimensional = (0..64)
            .flat_map(|x| {
                (0..64).map(move |y| Point::new([x as f32, y as f32, 0.0], ColorRgb::new(1, 2, 3)))
            })
            .collect::<Vec<_>>();
        let three_dimensional = (0..16)
            .flat_map(|x| {
                (0..16).flat_map(move |y| {
                    (0..16).map(move |z| {
                        Point::new([x as f32, y as f32, z as f32], ColorRgb::new(1, 2, 3))
                    })
                })
            })
            .collect::<Vec<_>>();

        for (name, points) in [("2D", &two_dimensional), ("3D", &three_dimensional)] {
            let scene = RenderScene::from_points(points);
            assert!(
                (scene.instances()[0].size - 1.0).abs() < 0.25,
                "{name} estimated spacing={}",
                scene.instances()[0].size
            );
        }
    }

    #[test]
    fn sampled_spacing_is_deterministic_and_robust_to_outliers() {
        let grid = (0..64)
            .flat_map(|x| {
                (0..64).map(move |y| Point::new([x as f32, y as f32, 0.0], ColorRgb::new(1, 2, 3)))
            })
            .collect::<Vec<_>>();
        let baseline = RenderScene::from_points(&grid).instances()[0].size;
        let with_outlier = grid
            .iter()
            .copied()
            .chain([Point::new(
                [100_000.0, -100_000.0, 0.0],
                ColorRgb::new(1, 2, 3),
            )])
            .collect::<Vec<_>>();

        let outlier_size = RenderScene::from_points(&with_outlier).instances()[0].size;

        assert!(
            (outlier_size - baseline).abs() < 0.25,
            "size={outlier_size}"
        );
        assert_eq!(
            baseline,
            RenderScene::from_points(&grid).instances()[0].size
        );
    }

    #[test]
    fn coincident_points_use_a_small_visible_glyph() {
        let points = vec![
            Point::new([0.0; 3], ColorRgb::new(1, 2, 3)),
            Point::new([0.0; 3], ColorRgb::new(4, 5, 6)),
        ];

        let scene = RenderScene::from_points(&points);

        assert!(scene.instances()[0].size < 1.0e-5);
        assert!(scene.instances()[0].size > 0.9e-5);
    }

    #[test]
    fn raw_points_are_collapsed_into_integer_grid_cells() {
        let points = [
            Point::new([0.1, 0.1, 0.1], viewer_core::ColorRgb::new(255, 0, 0)),
            Point::new([0.1, 0.1, 0.1], viewer_core::ColorRgb::new(0, 0, 255)),
        ];

        let scene = RenderScene::from_points(&points);

        assert_eq!(scene.instance_count(), 1);
        assert!(scene.instances()[0].size > 0.0);
    }
}
