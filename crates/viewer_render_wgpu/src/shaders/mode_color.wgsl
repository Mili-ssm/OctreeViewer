
fn shader_mode_color(
    mode: f32,
    base: vec4<f32>,
    intensity: f32,
    intensity_range: vec2<f32>,
    classification: f32,
    height_num: f32,
    height_span: f32,
) -> vec4<f32> {
    let rgb_intensity = dot(base.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    var color = base;
    if mode == 1.0 {
        let intensity_span = intensity_range.y - intensity_range.x;
        let scaled_intensity = clamp((intensity - intensity_range.x)
            / max(intensity_span, 0.000001), 0.0, 1.0);
        let normalized = select(rgb_intensity, select(0.5, scaled_intensity, intensity_span > 0.000001), intensity >= 0.0);
        color = vec4<f32>(vec3<f32>(normalized), 1.0);
    } else if mode == 2.0 {
        let height = clamp(height_num / max(height_span, 0.000001), 0.0, 1.0);
        color = vec4<f32>(mix(vec3<f32>(0.08, 0.25, 0.9), vec3<f32>(0.95, 0.22, 0.08), height), 1.0);
    } else if mode == 3.0 && classification >= 0.0 {
        let hue = fract(classification * 0.61803398875);
        let rgb = clamp(
            abs(fract(hue + vec3<f32>(0.0, 0.6666667, 0.3333333)) * 6.0 - 3.0) - 1.0,
            vec3<f32>(0.0),
            vec3<f32>(1.0),
        );
        color = vec4<f32>(rgb, 1.0);
    }
    return color;
}
