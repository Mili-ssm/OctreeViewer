use glam::{DVec3, dcamera};
use viewer_core::{FrustumPlane, SpatialBounds, SpatialFrustum};

use crate::{SceneBounds, VoxelBuildLimits};

pub(crate) const VERTICAL_FOV: f64 = 45.0_f64.to_radians();
const MIN_PITCH: f64 = -89.0_f64.to_radians();
const MAX_PITCH: f64 = 89.0_f64.to_radians();
/// Largest far/near ratio of the render projection; keeps `Depth32Float` (non-reversed)
/// resolution usable at the far end.
const MAX_DEPTH_RATIO: f64 = 1.0e5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraFrame {
    /// Column-major matrix using WebGPU's zero-to-one depth range.
    pub view_projection: [[f32; 4]; 4],
    /// Local scene origin subtracted in the vertex shader before projection.
    pub render_origin: [f32; 4],
}

/// View-projection with the eye at the origin, for the eye-relative cell pool (Task 7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeFrame {
    /// Column-major, WebGPU depth range; it expects positions minus `eye`.
    pub view_projection: [[f32; 4]; 4],
    /// The camera eye in scaled local render units, in `f64`: the `PageFrame::eye_scaled` input.
    pub eye: [f64; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    target: DVec3,
    distance: f64,
    yaw: f64,
    pitch: f64,
    radius: f64,
    scene_center: DVec3,
    fitted: bool,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: DVec3::ZERO,
            distance: 5.0,
            yaw: 0.0,
            pitch: 0.45,
            radius: 1.0,
            scene_center: DVec3::ZERO,
            fitted: false,
        }
    }
}

/// Number of `f64` values in the `OrbitCamera` transport form.
pub const ORBIT_CAMERA_F64_LEN: usize = 11;

impl OrbitCamera {
    /// Lossless transport form for handing the camera to another worker as a `Float64Array`:
    /// `[target xyz, distance, yaw, pitch, radius, scene_center xyz, fitted (0 or 1)]`.
    pub fn to_f64_array(&self) -> [f64; ORBIT_CAMERA_F64_LEN] {
        let [tx, ty, tz] = self.target.to_array();
        let [cx, cy, cz] = self.scene_center.to_array();
        [
            tx,
            ty,
            tz,
            self.distance,
            self.yaw,
            self.pitch,
            self.radius,
            cx,
            cy,
            cz,
            if self.fitted { 1.0 } else { 0.0 },
        ]
    }

    /// Inverse of [`Self::to_f64_array`]; the round trip preserves every bit.
    pub fn from_f64_array(values: &[f64]) -> Option<Self> {
        let values: &[f64; ORBIT_CAMERA_F64_LEN] = values.try_into().ok()?;
        let fitted = match values[10] {
            0.0 => false,
            1.0 => true,
            _ => return None,
        };
        Some(Self {
            target: DVec3::new(values[0], values[1], values[2]),
            distance: values[3],
            yaw: values[4],
            pitch: values[5],
            radius: values[6],
            scene_center: DVec3::new(values[7], values[8], values[9]),
            fitted,
        })
    }

    pub fn target(&self) -> [f64; 3] {
        self.target.to_array()
    }

    pub fn position(&self) -> [f64; 3] {
        let offset = camera_direction(self.yaw, self.pitch) * self.distance;
        (self.target + offset).to_array()
    }

    /// Sets a camera pose in local render coordinates. Rejects invalid or unrenderable poses.
    pub fn set_pose(&mut self, position: [f64; 3], target: [f64; 3]) -> bool {
        if position
            .iter()
            .chain(&target)
            .any(|value| !value.is_finite())
        {
            return false;
        }
        let target = DVec3::from_array(target);
        let offset = DVec3::from_array(position) - target;
        let distance = offset.length();
        if target.abs().max_element() > f64::from(f32::MAX) * 0.1
            || !distance.is_finite()
            || distance < 0.001
            || distance > f64::from(f32::MAX) * 0.1
        {
            return false;
        }
        let pitch = (offset.z / distance).clamp(-1.0, 1.0).asin();
        let yaw = offset.x.atan2(offset.y).rem_euclid(std::f64::consts::TAU);
        self.target = target;
        self.distance = distance;
        self.yaw = yaw;
        self.pitch = pitch.clamp(MIN_PITCH, MAX_PITCH);
        self.radius = self.radius.max(0.001);
        self.fitted = true;
        true
    }

    /// Re-centers the orbit target while keeping the camera position fixed.
    pub fn set_target(&mut self, target: [f64; 3]) -> bool {
        if target.iter().any(|value| !value.is_finite())
            || target
                .iter()
                .any(|value| value.abs() > f64::from(f32::MAX) * 0.1)
        {
            return false;
        }
        let position = DVec3::from_array(self.position());
        let target = DVec3::from_array(target);
        let offset = position - target;
        let distance = offset.length();
        if !distance.is_finite() || distance < 0.001 || distance > f64::from(f32::MAX) * 0.1 {
            return false;
        }
        self.target = target;
        self.distance = distance;
        self.yaw = offset.x.atan2(offset.y).rem_euclid(std::f64::consts::TAU);
        self.pitch = (offset.z / distance)
            .clamp(-1.0, 1.0)
            .asin()
            .clamp(MIN_PITCH, MAX_PITCH);
        true
    }

    /// Translates camera and target together in local world coordinates.
    pub fn translate(&mut self, delta: [f64; 3]) -> bool {
        if delta.iter().any(|value| !value.is_finite()) {
            return false;
        }
        let target = self.target + DVec3::from_array(delta);
        let position = DVec3::from_array(self.position()) + DVec3::from_array(delta);
        if !target.is_finite()
            || !position.is_finite()
            || target.abs().max_element() > f64::from(f32::MAX) * 0.1
        {
            return false;
        }
        self.target = target;
        true
    }

    /// Flies the camera: `forward` along the view direction projected on the horizontal plane
    /// (derived from `yaw`, so it also works looking straight down), `right` along the screen's
    /// right, `up` along world Z. Amounts are in units of the orbit `distance`; target and eye
    /// move together.
    pub fn fly(&mut self, forward: f64, right: f64, up: f64) -> bool {
        if !forward.is_finite() || !right.is_finite() || !up.is_finite() {
            return false;
        }
        let (sin, cos) = self.yaw.sin_cos();
        let delta = (DVec3::new(-sin, -cos, 0.0) * forward
            + DVec3::new(-cos, sin, 0.0) * right
            + DVec3::Z * up)
            * self.distance;
        self.translate(delta.to_array())
    }

    pub fn set_bird_view(&mut self) {
        self.pitch = MAX_PITCH;
    }

    pub(crate) fn ray(
        &self,
        pixel: [f64; 2],
        viewport: [u32; 2],
    ) -> Option<(DVec3, DVec3, f64, f64)> {
        if viewport.contains(&0)
            || pixel.iter().any(|value| !value.is_finite())
            || pixel[0] < 0.0
            || pixel[1] < 0.0
            || pixel[0] >= f64::from(viewport[0])
            || pixel[1] >= f64::from(viewport[1])
        {
            return None;
        }
        let aspect = f64::from(viewport[0]) / f64::from(viewport[1]);
        let ndc_x = 2.0 * (pixel[0] + 0.5) / f64::from(viewport[0]) - 1.0;
        let ndc_y = 1.0 - 2.0 * (pixel[1] + 0.5) / f64::from(viewport[1]);
        let forward = -camera_direction(self.yaw, self.pitch);
        let right = DVec3::new(self.yaw.cos(), -self.yaw.sin(), 0.0);
        let up = forward.cross(right).normalize();
        let direction = (forward
            + right * (ndc_x * aspect * (VERTICAL_FOV * 0.5).tan())
            + up * (ndc_y * (VERTICAL_FOV * 0.5).tan()))
        .normalize();
        let origin = self.target + camera_direction(self.yaw, self.pitch) * self.distance;
        let (near, far) = self.clip_distances();
        (origin.is_finite() && direction.is_finite() && near.is_finite() && far.is_finite())
            .then_some((origin, direction, near, far))
    }

    /// Fits a sphere around the scene bounds. Returns false without changing state if input is invalid.
    pub fn fit(&mut self, bounds: SceneBounds, viewport: [u32; 2]) -> bool {
        if viewport[0] == 0
            || viewport[1] == 0
            || bounds
                .min
                .iter()
                .chain(&bounds.max)
                .any(|value| !value.is_finite())
            || (0..3).any(|axis| bounds.min[axis] > bounds.max[axis])
        {
            return false;
        }

        let min = DVec3::from_array(bounds.min);
        let max = DVec3::from_array(bounds.max);
        let target = (min + max) * 0.5;
        let radius = (max - min).length() * 0.5;
        if !target.is_finite()
            || target.abs().max_element() > f64::from(f32::MAX) * 0.1
            || !radius.is_finite()
        {
            return false;
        }
        let radius = radius.max(0.001);
        let aspect = f64::from(viewport[0]) / f64::from(viewport[1]);
        let tan_vertical = (VERTICAL_FOV * 0.5).tan();
        let tan_horizontal = tan_vertical * aspect;
        let corners = bounds_corners(min, max, target);
        let mut best: Option<(f64, f64, f64)> = None;
        for pitch_degrees in [30.0_f64, 45.0, 60.0, 75.0] {
            let pitch = pitch_degrees.to_radians();
            for yaw_degrees in (0..360).step_by(45) {
                let yaw = f64::from(yaw_degrees).to_radians();
                let direction = camera_direction(yaw, pitch);
                let right = DVec3::new(yaw.cos(), -yaw.sin(), 0.0);
                let forward = -direction;
                let up = forward.cross(right).normalize();
                let mut distance = radius * 0.005;
                let mut max_depth_offset = f64::NEG_INFINITY;
                for corner in &corners {
                    let depth_offset = corner.dot(direction);
                    max_depth_offset = max_depth_offset.max(depth_offset);
                    let horizontal = corner.dot(right).abs() / tan_horizontal;
                    let vertical = corner.dot(up).abs() / tan_vertical;
                    distance = distance.max(depth_offset + horizontal);
                    distance = distance.max(depth_offset + vertical);
                }
                distance = distance.max(max_depth_offset + radius * 0.005) * 1.04;
                let occupancy = projected_bounds_area(
                    &corners,
                    direction,
                    right,
                    up,
                    distance,
                    tan_horizontal,
                    tan_vertical,
                );
                if occupancy.is_finite()
                    && best.is_none_or(|(best_occupancy, _, _)| occupancy > best_occupancy)
                {
                    best = Some((occupancy, yaw, pitch));
                }
            }
        }
        let Some((_, yaw, pitch)) = best else {
            return false;
        };
        let direction = camera_direction(yaw, pitch);
        let right = DVec3::new(yaw.cos(), -yaw.sin(), 0.0);
        let up = (-direction).cross(right).normalize();
        let mut distance = radius * 0.005;
        let mut max_depth_offset = f64::NEG_INFINITY;
        for corner in &corners {
            let depth_offset = corner.dot(direction);
            max_depth_offset = max_depth_offset.max(depth_offset);
            distance = distance.max(depth_offset + corner.dot(right).abs() / tan_horizontal);
            distance = distance.max(depth_offset + corner.dot(up).abs() / tan_vertical);
        }
        distance = distance.max(max_depth_offset + radius * 0.005) * 1.04;
        if !distance.is_finite() || distance > f64::from(f32::MAX) * 0.1 {
            return false;
        }

        self.target = target;
        self.radius = radius;
        self.scene_center = target;
        self.distance = distance;
        self.yaw = yaw;
        self.pitch = pitch;
        self.fitted = true;
        true
    }

    /// Rotates by normalized pointer deltas; one full viewport width is one full turn.
    pub fn orbit(&mut self, delta_x: f64, delta_y: f64) -> bool {
        if !delta_x.is_finite() || !delta_y.is_finite() {
            return false;
        }
        self.yaw = (self.yaw - delta_x.rem_euclid(1.0) * std::f64::consts::TAU)
            .rem_euclid(std::f64::consts::TAU);
        self.pitch = (self.pitch + delta_y.clamp(-1.0, 1.0) * std::f64::consts::PI)
            .clamp(MIN_PITCH, MAX_PITCH);
        true
    }

    /// Pans by pointer deltas normalized to the viewport dimensions.
    pub fn pan(&mut self, delta_x: f64, delta_y: f64, viewport: [u32; 2]) -> bool {
        if !delta_x.is_finite() || !delta_y.is_finite() || viewport[0] == 0 || viewport[1] == 0 {
            return false;
        }
        let aspect = f64::from(viewport[0]) / f64::from(viewport[1]);
        let right = DVec3::new(self.yaw.cos(), -self.yaw.sin(), 0.0);
        let direction = DVec3::new(
            -self.yaw.sin() * self.pitch.cos(),
            -self.yaw.cos() * self.pitch.cos(),
            -self.pitch.sin(),
        );
        let up = direction.cross(right).normalize();
        let half_height = self.distance * (VERTICAL_FOV * 0.5).tan();
        let pan =
            right * (-delta_x * half_height * 2.0 * aspect) + up * (delta_y * half_height * 2.0);
        if !pan.is_finite() {
            return false;
        }
        let target = self.target + pan;
        if !target.is_finite() || target.abs().max_element() > f64::from(f32::MAX) * 0.1 {
            return false;
        }
        self.target = target;
        true
    }

    /// Applies an exponential zoom delta; positive values move away from the scene.
    pub fn zoom(&mut self, delta: f64) -> bool {
        if !delta.is_finite() {
            return false;
        }
        let factor = delta.clamp(-20.0, 20.0).exp();
        let max_distance = (self.radius * 1.0e6)
            .max(1.0)
            .min(f64::from(f32::MAX) * 0.1);
        let min_distance = (self.radius * 0.01).max(0.001).min(max_distance);
        self.distance = (self.distance * factor).clamp(min_distance, max_distance);
        true
    }

    pub fn frame(&self, viewport: [u32; 2]) -> Option<CameraFrame> {
        if viewport[0] == 0 || viewport[1] == 0 {
            return None;
        }
        let aspect = f64::from(viewport[0]) / f64::from(viewport[1]);
        let offset = DVec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
        ) * self.distance;
        let (near, far) = self.clip_distances();
        let view = dcamera::rh::view::look_at_mat4(offset, DVec3::ZERO, DVec3::Z);
        // WebGPU uses Y-up and depth [0, 1], unlike the Vulkan projection helper.
        let projection = dcamera::rh::proj::directx::perspective(VERTICAL_FOV, aspect, near, far);
        let matrix = (projection * view).as_mat4().to_cols_array_2d();
        let origin = self.target.as_vec3().to_array();
        if matrix.iter().flatten().any(|value| !value.is_finite())
            || origin.iter().any(|value| !value.is_finite())
        {
            return None;
        }
        Some(CameraFrame {
            view_projection: matrix,
            render_origin: [origin[0], origin[1], origin[2], 0.0],
        })
    }

    /// Same view and projection as [`Self::frame`] but looking from the origin towards `-offset`,
    /// so a point is projected as `position - eye`. Cells keep their precision near the eye
    /// because the subtraction happens in `f64` on the CPU, not in the matrix.
    pub fn eye_frame(&self, viewport: [u32; 2]) -> Option<EyeFrame> {
        if viewport[0] == 0 || viewport[1] == 0 {
            return None;
        }
        let aspect = f64::from(viewport[0]) / f64::from(viewport[1]);
        let offset = DVec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
        ) * self.distance;
        let (near, far) = self.clip_distances();
        let view = dcamera::rh::view::look_at_mat4(DVec3::ZERO, -offset, DVec3::Z);
        let projection = dcamera::rh::proj::directx::perspective(VERTICAL_FOV, aspect, near, far);
        let matrix = (projection * view).as_mat4().to_cols_array_2d();
        let eye = self.position();
        if matrix.iter().flatten().any(|value| !value.is_finite())
            || eye.iter().any(|value| !value.is_finite())
        {
            return None;
        }
        Some(EyeFrame {
            view_projection: matrix,
            eye,
        })
    }

    /// Builds the current view frustum in local render coordinates.
    pub fn spatial_frustum(&self, viewport: [u32; 2]) -> Option<SpatialFrustum> {
        self.spatial_frustum_with_near(viewport, self.clip_distances().0)
    }

    fn spatial_frustum_with_near(&self, viewport: [u32; 2], near: f64) -> Option<SpatialFrustum> {
        if viewport[0] == 0 || viewport[1] == 0 {
            return None;
        }
        let (position, forward, right, up) = self.basis();
        let aspect = f64::from(viewport[0]) / f64::from(viewport[1]);
        let horizontal_half_fov = ((VERTICAL_FOV * 0.5).tan() * aspect).atan();
        let far = self.clip_distances().1;
        let side_planes = [
            forward * horizontal_half_fov.cos() + right * horizontal_half_fov.sin(),
            forward * horizontal_half_fov.cos() - right * horizontal_half_fov.sin(),
            forward * (VERTICAL_FOV * 0.5).cos() + up * (VERTICAL_FOV * 0.5).sin(),
            forward * (VERTICAL_FOV * 0.5).cos() - up * (VERTICAL_FOV * 0.5).sin(),
        ];
        let mut planes = side_planes
            .into_iter()
            .map(|normal| plane(normal, position))
            .collect::<Option<Vec<_>>>()?;
        planes.push(plane(forward, position + forward * near)?);
        planes.push(plane(-forward, position + forward * far)?);
        SpatialFrustum::new(planes).ok()
    }

    /// Screen-space error in pixels of a node whose representative cell has edge `spacing`
    /// (dataset units): the single metric behind LOD selection and pending-block priority.
    ///
    /// The cell is assumed to sit at the node's nearest point, so the depth is the minimum
    /// view-space depth over the eight AABB corners (clamped to `lod_min_depth`), and the
    /// off-axis factor bounds the stretch of an edge at the viewport corners. This keeps the
    /// result an upper bound of `projected_voxel_edge_pixels` for any such cell in the node.
    pub fn projected_error_pixels(
        &self,
        bounds: SpatialBounds,
        spacing: f64,
        coordinate_origin: [f64; 3],
        viewport: [u32; 2],
    ) -> f64 {
        if viewport[0] == 0 || viewport[1] == 0 || !spacing.is_finite() || spacing < 0.0 {
            return f64::INFINITY;
        }
        // Hierarchy bounds use dataset coordinates; the renderer camera uses local ones.
        let (position, forward, _, _) = self.basis();
        let nearest_depth = (0..8)
            .map(|index| {
                let corner = DVec3::from_array(std::array::from_fn(|axis| {
                    (if index & (1 << axis) == 0 {
                        bounds.min()[axis]
                    } else {
                        bounds.max()[axis]
                    }) - coordinate_origin[axis]
                }));
                (corner - position).dot(forward)
            })
            .fold(f64::INFINITY, f64::min)
            .max(self.lod_min_depth());
        let aspect = f64::from(viewport[0]) / f64::from(viewport[1]);
        let tan_vertical = (VERTICAL_FOV * 0.5).tan();
        let tan_horizontal = tan_vertical * aspect;
        let off_axis_scale = (1.0 + tan_vertical.powi(2) + tan_horizontal.powi(2)).sqrt();
        spacing * f64::from(viewport[1]) * off_axis_scale / (2.0 * tan_vertical * nearest_depth)
    }

    /// Maximum length of the twelve projected cube edges, in framebuffer pixels.
    /// Uses actual perspective depth, independently of geometric-error bounds.
    pub fn projected_voxel_edge_pixels(
        &self,
        bounds: SpatialBounds,
        coordinate_origin: [f64; 3],
        viewport: [u32; 2],
    ) -> f64 {
        if viewport.contains(&0) {
            return f64::INFINITY;
        }
        let (position, forward, right, up) = self.basis();
        let focal = f64::from(viewport[1]) / (2.0 * (VERTICAL_FOV * 0.5).tan());
        let mut projected = [[0.0; 2]; 8];
        for (index, pixel) in projected.iter_mut().enumerate() {
            let corner = DVec3::from_array(std::array::from_fn(|axis| {
                (if index & (1 << axis) == 0 {
                    bounds.min()[axis]
                } else {
                    bounds.max()[axis]
                }) - coordinate_origin[axis]
            }));
            let offset = corner - position;
            let depth = offset.dot(forward);
            if depth <= 0.0 {
                return f64::INFINITY;
            }
            *pixel = [
                focal * offset.dot(right) / depth,
                focal * offset.dot(up) / depth,
            ];
        }
        let mut maximum: f64 = 0.0;
        for index in 0..8 {
            for axis in 0..3 {
                let other = index ^ (1 << axis);
                maximum = maximum.max(
                    (projected[index][0] - projected[other][0])
                        .hypot(projected[index][1] - projected[other][1]),
                );
            }
        }
        maximum
    }

    /// Unit view direction, in the same units as `position`.
    pub(crate) fn forward(&self) -> [f64; 3] {
        self.basis().1.to_array()
    }

    /// Unit `(right, up)` image axes, in the same units as `position`.
    pub(crate) fn right_up(&self) -> ([f64; 3], [f64; 3]) {
        let (_, _, right, up) = self.basis();
        (right.to_array(), up.to_array())
    }

    fn basis(&self) -> (DVec3, DVec3, DVec3, DVec3) {
        let offset = DVec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
        ) * self.distance;
        let position = self.target + offset;
        let forward = -offset.normalize();
        let right = DVec3::new(self.yaw.cos(), -self.yaw.sin(), 0.0).normalize();
        let up = forward.cross(right).normalize();
        (position, forward, right, up)
    }

    /// Depth below which the LOD error stops growing. It deliberately ignores the render near
    /// plane: lowering that one must not make selection refine ~100x more near the camera.
    fn lod_min_depth(&self) -> f64 {
        let (position, forward, _, _) = self.basis();
        let center_depth = (self.scene_center - position).dot(forward);
        (center_depth - self.radius * 1.2)
            .max(self.radius * 0.001)
            .max(0.001)
    }

    /// `lod_min_depth` for the LOD probes of the cell cut.
    pub(crate) fn lod_near_depth(&self) -> f64 {
        self.lod_min_depth()
    }

    pub fn clip_distances(&self) -> (f64, f64) {
        // Panning/recentering moves the orbit pivot, not the dataset bounds.
        let (position, forward, _, _) = self.basis();
        let center_depth = (self.scene_center - position).dot(forward);
        // The bounding sphere is the nearest-geometry bound the camera knows; inside it the
        // near plane drops to the depth-precision floor so ground at ~1 m is not clipped.
        let far = (center_depth + self.radius * 2.0).max(1.0e-3);
        let near = (center_depth - self.radius * 1.2).max(far / MAX_DEPTH_RATIO);
        (near, far.max(near * 2.0))
    }

    pub fn is_fitted(&self) -> bool {
        self.fitted
    }
}

fn camera_direction(yaw: f64, pitch: f64) -> DVec3 {
    DVec3::new(
        yaw.sin() * pitch.cos(),
        yaw.cos() * pitch.cos(),
        pitch.sin(),
    )
}

fn bounds_corners(min: DVec3, max: DVec3, target: DVec3) -> [DVec3; 8] {
    std::array::from_fn(|index| {
        DVec3::new(
            if index & 1 == 0 { min.x } else { max.x },
            if index & 2 == 0 { min.y } else { max.y },
            if index & 4 == 0 { min.z } else { max.z },
        ) - target
    })
}

fn projected_bounds_area(
    corners: &[DVec3; 8],
    direction: DVec3,
    right: DVec3,
    up: DVec3,
    distance: f64,
    tan_horizontal: f64,
    tan_vertical: f64,
) -> f64 {
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for corner in corners {
        let depth = distance - corner.dot(direction);
        if !depth.is_finite() || depth <= 0.0 {
            return 0.0;
        }
        let x = corner.dot(right) / depth / tan_horizontal;
        let y = corner.dot(up) / depth / tan_vertical;
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    (max_x - min_x) * (max_y - min_y)
}

/// Everything the LOD and voxel-build path needs from a view, without any GPU state.
///
/// The camera lives in local render coordinates (dataset units minus the coordinate origin,
/// times `visual_scale`); hierarchy bounds and frusta are exposed in dataset or local units as
/// the callers need. Plain data, so it can be built and sent off the render thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewProjection {
    camera: OrbitCamera,
    viewport: [u32; 2],
    visual_scale: f64,
    build_limits: VoxelBuildLimits,
}

impl ViewProjection {
    pub fn new(
        camera: OrbitCamera,
        viewport: [u32; 2],
        visual_scale: f64,
        build_limits: VoxelBuildLimits,
    ) -> Self {
        Self {
            camera,
            viewport,
            visual_scale,
            build_limits,
        }
    }

    pub fn camera(&self) -> OrbitCamera {
        self.camera
    }

    pub fn viewport(&self) -> [u32; 2] {
        self.viewport
    }

    pub fn visual_scale(&self) -> f64 {
        self.visual_scale
    }

    /// Effective voxel construction limits (already capped by the GPU instance budget).
    pub fn voxel_build_limits(&self) -> VoxelBuildLimits {
        self.build_limits
    }

    /// Camera frustum relative to `coordinate_origin` (dataset units); `[0.0; 3]` gives the
    /// frustum in local scaled units.
    pub fn spatial_frustum(&self, coordinate_origin: [f64; 3]) -> Option<SpatialFrustum> {
        let frustum = self.camera.spatial_frustum(self.viewport)?;
        let planes = frustum
            .planes()
            .iter()
            .map(|plane| {
                let normal = plane
                    .normal()
                    .map(|component| component * self.visual_scale);
                FrustumPlane::new(
                    normal,
                    plane.offset()
                        - (0..3)
                            .map(|axis| normal[axis] * coordinate_origin[axis])
                            .sum::<f64>(),
                )
                .ok()
            })
            .collect::<Option<Vec<_>>>()?;
        SpatialFrustum::new(planes).ok()
    }

    fn local_bounds(
        &self,
        bounds: SpatialBounds,
        coordinate_origin: [f64; 3],
    ) -> Option<SpatialBounds> {
        let min = std::array::from_fn(|axis| {
            (bounds.min()[axis] - coordinate_origin[axis]) * self.visual_scale
        });
        let max = std::array::from_fn(|axis| {
            (bounds.max()[axis] - coordinate_origin[axis]) * self.visual_scale
        });
        SpatialBounds::new(min, max).ok()
    }

    /// Screen-space error of a node's representative cell (`spacing`, dataset units).
    pub fn projected_error_pixels(
        &self,
        bounds: SpatialBounds,
        spacing: f64,
        coordinate_origin: [f64; 3],
    ) -> f64 {
        let Some(bounds) = self.local_bounds(bounds, coordinate_origin) else {
            return f64::INFINITY;
        };
        self.camera.projected_error_pixels(
            bounds,
            spacing * self.visual_scale,
            [0.0; 3],
            self.viewport,
        )
    }

    pub fn projected_voxel_edge_pixels(
        &self,
        bounds: SpatialBounds,
        coordinate_origin: [f64; 3],
    ) -> f64 {
        let Some(bounds) = self.local_bounds(bounds, coordinate_origin) else {
            return f64::INFINITY;
        };
        self.camera
            .projected_voxel_edge_pixels(bounds, [0.0; 3], self.viewport)
    }
}

fn plane(normal: DVec3, point: DVec3) -> Option<FrustumPlane> {
    let normal = normal.normalize();
    FrustumPlane::new(normal.to_array(), -normal.dot(point)).ok()
}

#[cfg(test)]
mod t4_diagnosis;

#[cfg(test)]
mod tests {
    use glam::{DVec3, Mat4};

    use super::{ORBIT_CAMERA_F64_LEN, OrbitCamera};
    use crate::SceneBounds;

    #[test]
    fn f64_transport_round_trips_every_bit() {
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(bounds(), [800, 600]));
        camera.orbit(13.7, -4.2);
        camera.pan(3.3, 1.1, [800, 600]);
        camera.zoom(0.37);
        let packed = camera.to_f64_array();
        let restored = OrbitCamera::from_f64_array(&packed).unwrap();
        assert_eq!(restored, camera);
        let repacked = restored.to_f64_array();
        for (a, b) in packed.iter().zip(&repacked) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        let default = OrbitCamera::default();
        assert_eq!(
            OrbitCamera::from_f64_array(&default.to_f64_array()),
            Some(default)
        );
    }

    #[test]
    fn f64_transport_rejects_wrong_length_and_flags() {
        let packed = OrbitCamera::default().to_f64_array();
        assert!(OrbitCamera::from_f64_array(&packed[..ORBIT_CAMERA_F64_LEN - 1]).is_none());
        let mut bad = packed;
        bad[ORBIT_CAMERA_F64_LEN - 1] = 0.5;
        assert!(OrbitCamera::from_f64_array(&bad).is_none());
    }

    #[test]
    fn frame_preserves_webgpu_depth_range_and_screen_orientation() {
        let mut camera = OrbitCamera::default();
        let viewport = [800, 600];
        assert!(camera.fit(bounds(), viewport));
        let matrix = Mat4::from_cols_array_2d(&camera.frame(viewport).unwrap().view_projection);
        let (eye, forward, _, _) = camera.basis();
        let right = forward.cross(DVec3::Z).normalize();
        let up = right.cross(forward);
        let (near, far) = camera.clip_distances();
        let project = |point: DVec3| {
            let clip = matrix * (point - camera.target).as_vec3().extend(1.0);
            clip.truncate() / clip.w
        };
        for (distance, expected_depth) in [(near, 0.0), (far, 1.0)] {
            let ndc = project(eye + forward * distance);
            assert!(ndc.x.abs() < 1e-5 && ndc.y.abs() < 1e-5);
            assert!((ndc.z - expected_depth).abs() < 1e-5);
        }
        let center = eye + forward * ((near + far) * 0.5);
        assert!(project(center + up).y > 0.0);
        assert!(project(center + right).x > 0.0);
    }

    #[test]
    fn eye_frame_projects_like_frame_with_the_eye_at_the_origin() {
        let mut camera = OrbitCamera::default();
        let viewport = [800, 600];
        assert!(camera.fit(bounds(), viewport));
        assert!(camera.orbit(0.7, 0.2));
        let frame = Mat4::from_cols_array_2d(&camera.frame(viewport).unwrap().view_projection);
        let eye_frame = camera.eye_frame(viewport).unwrap();
        assert_eq!(eye_frame.eye, camera.position());
        let eye_matrix = Mat4::from_cols_array_2d(&eye_frame.view_projection);
        for point in [
            DVec3::new(0.3, -0.2, 0.1),
            DVec3::new(-1.0, 0.5, 0.25),
            DVec3::new(0.0, 0.0, -0.4),
        ] {
            let from_target = frame * (point - camera.target).as_vec3().extend(1.0);
            let from_eye = eye_matrix
                * (point - DVec3::from_array(eye_frame.eye))
                    .as_vec3()
                    .extend(1.0);
            let (a, b) = (from_target / from_target.w, from_eye / from_eye.w);
            assert!((a - b).abs().max_element() < 1e-4, "{a} vs {b}");
        }
        assert!(camera.eye_frame([0, 600]).is_none());
    }

    #[test]
    fn fly_moves_relative_to_the_view_and_scales_with_distance() {
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(bounds(), [800, 600]));
        let distance = camera.distance;
        let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9);
        for bird in [false, true] {
            if bird {
                camera.set_bird_view();
            }
            let pitch = camera.pitch;
            for yaw in [0.0, std::f64::consts::FRAC_PI_2, 2.3] {
                camera.yaw = yaw;
                let start = camera.target;
                let (sin, cos) = yaw.sin_cos();
                assert!(camera.fly(0.5, 0.0, 0.0));
                let moved = (camera.target - start) / distance;
                assert!(close(moved.to_array(), [-0.5 * sin, -0.5 * cos, 0.0]));
                // Right matches the screen's right (see `frame_preserves_..._screen_orientation`).
                let start = camera.target;
                assert!(camera.fly(0.0, 1.0, 0.0));
                let moved = (camera.target - start) / distance;
                let (_, forward, _, _) = camera.basis();
                let screen_right = forward.cross(DVec3::Z).normalize();
                assert!(close(moved.to_array(), screen_right.to_array()));
                let start = camera.target;
                assert!(camera.fly(0.0, 0.0, -2.0));
                let moved = camera.target - start;
                assert!(close(moved.to_array(), [0.0, 0.0, -2.0 * distance]));
            }
            assert_eq!((camera.distance, camera.pitch), (distance, pitch));
        }
        let before = camera.target;
        assert!(!camera.fly(f64::NAN, 0.0, 0.0));
        assert!(!camera.fly(0.0, f64::INFINITY, 0.0));
        assert_eq!(camera.target, before);
    }

    #[test]
    fn clipping_tracks_dataset_after_translation_rotation_and_bird_view() {
        let mut camera = OrbitCamera::default();
        let viewport = [800, 600];
        assert!(camera.fit(bounds(), viewport));
        assert!(camera.translate([0.0, 0.0, 100.0]));
        for bird in [false, true] {
            if bird {
                camera.set_bird_view();
            } else {
                camera.orbit(0.25, 0.1);
            }
            let frame = camera.frame(viewport).unwrap();
            let matrix = Mat4::from_cols_array_2d(&frame.view_projection);
            let center = (camera.scene_center - camera.target).as_vec3();
            let clip = matrix * center.extend(1.0);
            assert!(clip.w > 0.0);
            assert!(
                clip.z >= 0.0 && clip.z <= clip.w,
                "dataset clipped: {clip:?}"
            );
        }
    }

    #[test]
    fn camera_pose_round_trips_and_rejects_invalid_values_without_mutation() {
        let mut camera = OrbitCamera::default();
        assert!(camera.set_pose([12.0, -7.0, 4.0], [1.0, 2.0, 3.0]));
        let pose = (camera.position(), camera.target());
        assert!((DVec3::from_array(pose.0) - DVec3::new(12.0, -7.0, 4.0)).length() < 1e-10);
        assert_eq!(pose.1, [1.0, 2.0, 3.0]);
        assert!(!camera.set_pose([f64::NAN, 0.0, 0.0], [0.0; 3]));
        assert_eq!((camera.position(), camera.target()), pose);
        assert!(!camera.set_target([f64::INFINITY, 0.0, 0.0]));
        assert_eq!((camera.position(), camera.target()), pose);
    }

    #[test]
    fn pick_ray_points_through_center_and_rejects_invalid_viewport_pixels() {
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(bounds(), [800, 600]));
        let (origin, direction, _, _) = camera.ray([399.5, 299.5], [800, 600]).unwrap();
        let target_delta = camera.target - origin;
        assert!(direction.dot(target_delta.normalize()) > 0.999_999);
        assert!(camera.ray([-1.0, 10.0], [800, 600]).is_none());
        assert!(camera.ray([10.0, 10.0], [0, 600]).is_none());
    }

    fn bounds() -> SceneBounds {
        SceneBounds {
            min: [-20.0, -10.0, -5.0],
            max: [20.0, 10.0, 15.0],
        }
    }

    #[test]
    fn fits_bounds_in_landscape_and_portrait_viewports() {
        for viewport in [[1600, 900], [500, 900]] {
            let mut camera = OrbitCamera::default();
            assert!(camera.fit(bounds(), viewport));
            let frame = camera.frame(viewport).unwrap();
            let matrix = Mat4::from_cols_array_2d(&frame.view_projection);
            let origin = glam::Vec3::from_array(frame.render_origin[..3].try_into().unwrap());

            for x in [bounds().min[0], bounds().max[0]] {
                for y in [bounds().min[1], bounds().max[1]] {
                    for z in [bounds().min[2], bounds().max[2]] {
                        let relative = glam::Vec3::new(x as f32, y as f32, z as f32) - origin;
                        let clip = matrix * relative.extend(1.0);
                        let ndc = clip / clip.w;
                        assert!(ndc.x.abs() <= 1.0, "x={}", ndc.x);
                        assert!(ndc.y.abs() <= 1.0, "y={}", ndc.y);
                        assert!((0.0..=1.0).contains(&ndc.z), "z={}", ndc.z);
                    }
                }
            }
        }
    }

    #[test]
    fn fits_an_elongated_scene_tightly_without_clipping_its_corners() {
        let bounds = SceneBounds {
            min: [-50.0, -5.0, -5.0],
            max: [50.0, 5.0, 5.0],
        };
        let viewport = [1600, 900];
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(bounds, viewport));
        let frame = camera.frame(viewport).unwrap();
        let matrix = Mat4::from_cols_array_2d(&frame.view_projection);
        let origin = glam::Vec3::from_array(frame.render_origin[..3].try_into().unwrap());
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;

        for x in [bounds.min[0], bounds.max[0]] {
            for y in [bounds.min[1], bounds.max[1]] {
                for z in [bounds.min[2], bounds.max[2]] {
                    let relative = glam::Vec3::new(x as f32, y as f32, z as f32) - origin;
                    let clip = matrix * relative.extend(1.0);
                    let ndc = clip / clip.w;
                    assert!(ndc.x.abs() <= 1.0, "x={}", ndc.x);
                    assert!(ndc.y.abs() <= 1.0, "y={}", ndc.y);
                    assert!((0.0..=1.0).contains(&ndc.z), "z={}", ndc.z);
                    min_x = min_x.min(ndc.x);
                    max_x = max_x.max(ndc.x);
                    min_y = min_y.min(ndc.y);
                    max_y = max_y.max(ndc.y);
                }
            }
        }

        let screen_coverage = (max_x - min_x) * (max_y - min_y);
        assert!(screen_coverage > 0.1, "coverage={screen_coverage}");
    }

    #[test]
    fn camera_controls_are_finite_and_invalid_inputs_do_not_change_state() {
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(bounds(), [800, 600]));
        let original = camera;
        assert!(!camera.orbit(f64::NAN, 0.0));
        assert!(!camera.pan(0.0, 0.0, [0, 600]));
        assert!(!camera.zoom(f64::INFINITY));
        assert_eq!(camera, original);

        assert!(camera.orbit(1.0e6, 1.0e6));
        assert!(camera.orbit(f64::MAX, f64::MAX));
        assert!(camera.pan(0.2, -0.1, [800, 600]));
        assert!(!camera.pan(f64::MAX, 0.0, [800, 600]));
        assert!(camera.zoom(-10_000.0));
        assert!(camera.zoom(10_000.0));
        let frame = camera.frame([800, 600]).unwrap();
        assert!(
            frame
                .view_projection
                .iter()
                .flatten()
                .all(|value| value.is_finite())
        );
        assert!(frame.render_origin.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn camera_uses_local_origin_for_large_local_coordinates() {
        let bounds = SceneBounds {
            min: [99_999_990.0, -10.0, 0.0],
            max: [100_000_010.0, 10.0, 20.0],
        };
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(bounds, [1000, 800]));
        let frame = camera.frame([1000, 800]).unwrap();
        assert!((frame.render_origin[0] - 100_000_000.0).abs() < 1.0);

        let matrix = Mat4::from_cols_array_2d(&frame.view_projection);
        let origin = glam::Vec3::new(
            frame.render_origin[0],
            frame.render_origin[1],
            frame.render_origin[2],
        );
        let projected = matrix * (glam::Vec3::new(100_000_000.0, 0.0, 10.0) - origin).extend(1.0);
        assert!(projected.is_finite());
    }

    #[test]
    fn fits_a_degenerate_point_bounds_and_rejects_invalid_bounds_atomically() {
        let mut camera = OrbitCamera::default();
        let point_bounds = SceneBounds {
            min: [12.0, -5.0, 1.0],
            max: [12.0, -5.0, 1.0],
        };
        assert!(camera.fit(point_bounds, [800, 600]));
        let fitted = camera;
        assert!(camera.frame([800, 600]).is_some());

        let invalid_bounds = SceneBounds {
            min: [f64::NAN, 0.0, 0.0],
            max: [1.0, 1.0, 1.0],
        };
        assert!(!camera.fit(invalid_bounds, [800, 600]));
        assert_eq!(camera, fitted);
    }

    #[test]
    fn spatial_frustum_tracks_fitted_scene_and_projected_error_changes_with_depth() {
        use viewer_core::SpatialBounds;

        let scene = SceneBounds {
            min: [-1.0; 3],
            max: [1.0; 3],
        };
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(scene, [800, 600]));
        let frustum = camera.spatial_frustum([800, 600]).unwrap();
        let scene_bounds = SpatialBounds::new(scene.min, scene.max).unwrap();
        assert!(frustum.intersects(&scene_bounds));

        let nearby = SpatialBounds::new([-0.5; 3], [0.5; 3]).unwrap();
        let (_, forward, _, _) = camera.basis();
        let distant_center = camera.target + forward * 1000.0;
        let distant = SpatialBounds::new(
            (distant_center - glam::DVec3::splat(0.5)).to_array(),
            (distant_center + glam::DVec3::splat(0.5)).to_array(),
        )
        .unwrap();
        let nearby_error = camera.projected_error_pixels(nearby, 0.1, [0.0; 3], [800, 600]);
        let distant_error = camera.projected_error_pixels(distant, 0.1, [0.0; 3], [800, 600]);
        assert!(nearby_error > distant_error);
    }

    #[test]
    fn projected_cube_edges_match_perspective_distance_viewport_and_world_origin() {
        use viewer_core::SpatialBounds;
        let mut camera = OrbitCamera {
            pitch: 0.0,
            ..OrbitCamera::default()
        };
        let bounds = SpatialBounds::new([-0.5; 3], [0.5; 3]).unwrap();
        let edge = camera.projected_voxel_edge_pixels(bounds, [0.0; 3], [800, 600]);
        let focal = 600.0 / (2.0 * (super::VERTICAL_FOV * 0.5).tan());
        assert!((edge - focal / 4.5).abs() < 1e-10);
        assert!(
            (camera.projected_voxel_edge_pixels(bounds, [0.0; 3], [1600, 1200]) - edge * 2.0).abs()
                < 1e-10
        );
        let origin = [1_000_000.0, -2_000_000.0, 3_000_000.0];
        let translated =
            SpatialBounds::new(origin.map(|v| v - 0.5), origin.map(|v| v + 0.5)).unwrap();
        assert!(
            (camera.projected_voxel_edge_pixels(translated, origin, [800, 600]) - edge).abs()
                < 1e-10
        );
        camera.zoom(2.0_f64.ln());
        assert!(
            (camera.projected_voxel_edge_pixels(bounds, [0.0; 3], [800, 600]) - focal / 9.5).abs()
                < 1e-10
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "requires a native GPU or a software Vulkan adapter"]
    fn rasterized_cube_edges_match_camera_projection_at_two_resolutions_and_distances() {
        let scene = crate::RenderScene::from_instances(vec![crate::InstanceRaw {
            center: [0.0; 3],
            size: 1.0,
            color: [1.0, 0.4, 0.2, 1.0],
            intensity: -1.0,
            classification: -1.0,
        }]);
        let bounds = viewer_core::SpatialBounds::new([-0.5; 3], [0.5; 3]).unwrap();
        for (distance, viewport) in [(5.0, [257, 193]), (5.0, [514, 386]), (10.0, [257, 193])] {
            let camera = OrbitCamera {
                pitch: 0.0,
                fitted: true,
                distance,
                ..OrbitCamera::default()
            };
            let image =
                pollster::block_on(crate::render_offscreen(&scene, &camera, viewport)).unwrap();
            let background = &image.rgba[..3];
            let mut min = [u32::MAX; 2];
            let mut max = [0; 2];
            for (index, pixel) in image.rgba.as_chunks::<4>().0.iter().enumerate() {
                if (0..3).any(|axis| pixel[axis].abs_diff(background[axis]) > 1) {
                    let xy = [index as u32 % viewport[0], index as u32 / viewport[0]];
                    for axis in 0..2 {
                        min[axis] = min[axis].min(xy[axis]);
                        max[axis] = max[axis].max(xy[axis]);
                    }
                }
            }
            assert!(image.non_background_pixels > 0);
            let expected = camera.projected_voxel_edge_pixels(bounds, [0.0; 3], viewport);
            for axis in 0..2 {
                let rasterized = f64::from(max[axis] - min[axis] + 1);
                assert!(
                    (rasterized - expected).abs() <= 2.0,
                    "distance={distance}, viewport={viewport:?}, actual={rasterized}, expected={expected}"
                );
            }
        }
    }

    /// Previous selection metric, kept to prove the new one never selects more points: the
    /// bounding sphere's nearest depth, minus the error itself.
    fn legacy_projected_error_pixels(
        camera: &OrbitCamera,
        bounds: viewer_core::SpatialBounds,
        geometric_error: f64,
        viewport: [u32; 2],
    ) -> f64 {
        let center = DVec3::from_array(std::array::from_fn(|axis| {
            (bounds.min()[axis] + bounds.max()[axis]) * 0.5
        }));
        let half_diagonal = DVec3::from_array(std::array::from_fn(|axis| {
            (bounds.max()[axis] - bounds.min()[axis]) * 0.5
        }))
        .length();
        let (position, forward, _, _) = camera.basis();
        let nearest_depth = ((center - position).dot(forward) - half_diagonal - geometric_error)
            .max(camera.clip_distances().0);
        let aspect = f64::from(viewport[0]) / f64::from(viewport[1]);
        let tan_vertical = (super::VERTICAL_FOV * 0.5).tan();
        let tan_horizontal = tan_vertical * aspect;
        let off_axis_scale = (1.0 + tan_vertical.powi(2) + tan_horizontal.powi(2)).sqrt();
        geometric_error * f64::from(viewport[1]) * off_axis_scale
            / (2.0 * tan_vertical * nearest_depth)
    }

    /// Complete octree over `[0, 512]^3`; payload sizes grow with depth so refining adds points.
    fn octree(depth: u32) -> viewer_core::SpatialHierarchy {
        octree_with_side(depth, 512.0)
    }

    fn octree_with_side(depth: u32, root_side: f64) -> viewer_core::SpatialHierarchy {
        use viewer_core::{BlockId, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode};
        let mut nodes = Vec::new();
        let mut frontier = vec![(1_u64, None, [0.0; 3], root_side)];
        let mut next_id = 2;
        for level in 0..=depth {
            let mut children = Vec::new();
            for (id, parent, min, side) in frontier {
                let bounds = SpatialBounds::new(min, min.map(|v| v + side)).unwrap();
                nodes.push(
                    SpatialNode::new(
                        NodeId::new(id),
                        parent.map(NodeId::new),
                        level,
                        bounds,
                        10 * 4_u64.pow(level),
                        0.0,
                        vec![BlockId::new(id)],
                    )
                    .unwrap(),
                );
                for child in 0..8 {
                    let half = side / 2.0;
                    let corner = std::array::from_fn(|axis| {
                        min[axis] + if child & (1 << axis) == 0 { 0.0 } else { half }
                    });
                    children.push((next_id, Some(id), corner, half));
                    next_id += 1;
                }
            }
            frontier = children;
        }
        SpatialHierarchy::new(nodes, Vec::new(), None).unwrap()
    }

    /// Deterministic pseudo-random cameras that look at random points of the scene, so nodes
    /// land anywhere in the viewport, including its corners.
    fn random_views(count: usize) -> Vec<(OrbitCamera, [u32; 2])> {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut uniform = move |low: f64, high: f64| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            low + (high - low) * ((state >> 11) as f64 / (1_u64 << 53) as f64)
        };
        let viewports = [[800, 600], [1920, 1080], [300, 900], [2000, 500]];
        (0..count)
            .map(|index| {
                let viewport = viewports[index % viewports.len()];
                let mut camera = OrbitCamera::default();
                assert!(camera.fit(
                    SceneBounds {
                        min: [0.0; 3],
                        max: [512.0; 3],
                    },
                    viewport
                ));
                camera.yaw = uniform(-3.1, 3.1);
                camera.pitch = uniform(-1.4, 1.4);
                camera.distance = uniform(60.0, 1800.0);
                camera.target = DVec3::new(
                    uniform(0.0, 512.0),
                    uniform(0.0, 512.0),
                    uniform(0.0, 512.0),
                );
                (camera, viewport)
            })
            .collect()
    }

    fn select(
        hierarchy: &viewer_core::SpatialHierarchy,
        camera: &OrbitCamera,
        viewport: [u32; 2],
        target: f64,
        legacy: bool,
    ) -> Option<viewer_core::LodSelection> {
        let frustum = camera.spatial_frustum(viewport)?;
        let config = viewer_core::LodSelectionConfig::new(target, u64::MAX / 2).unwrap();
        hierarchy
            .select_lod(&frustum, config, |node| {
                let bounds = node.bounds();
                let side = bounds.max_side();
                if legacy {
                    legacy_projected_error_pixels(camera, bounds, side / 8.0, viewport)
                } else {
                    camera.projected_error_pixels(bounds, side / 8.0, [0.0; 3], viewport)
                }
            })
            .ok()
    }

    #[test]
    fn projected_error_bounds_the_edge_of_any_representative_cell_also_off_axis() {
        use viewer_core::SpatialBounds;
        let hierarchy = octree(3);
        let target = 6.0;
        let mut checked_cells = 0_usize;
        let mut selected_nodes = 0_usize;
        for (camera, viewport) in random_views(120) {
            let Some(selection) = select(&hierarchy, &camera, viewport, target, false) else {
                continue;
            };
            let (position, forward, right, up) = camera.basis();
            let near = camera.clip_distances().0;
            let tan_v = (super::VERTICAL_FOV * 0.5).tan();
            let tan_h = tan_v * f64::from(viewport[0]) / f64::from(viewport[1]);
            for id in selection.node_ids() {
                let node = hierarchy.node(*id).unwrap();
                let bounds = node.bounds();
                let side = bounds.max()[0] - bounds.min()[0];
                let spacing = side / 8.0;
                let error = camera.projected_error_pixels(bounds, spacing, [0.0; 3], viewport);
                let limited = selection.resolution_limited().contains(id);
                if !limited {
                    selected_nodes += 1;
                    assert!(error <= target, "selected node above target: {error}");
                }
                // Cells at the eight corners of the node plus its center.
                for index in 0..9 {
                    let cell_min: [f64; 3] = std::array::from_fn(|axis| {
                        if index == 8 {
                            (bounds.min()[axis] + bounds.max()[axis] - spacing) * 0.5
                        } else if index & (1 << axis) == 0 {
                            bounds.min()[axis]
                        } else {
                            bounds.max()[axis] - spacing
                        }
                    });
                    let cell = SpatialBounds::new(cell_min, cell_min.map(|value| value + spacing))
                        .unwrap();
                    let depths_and_offsets = (0..8).map(|corner| {
                        let point = DVec3::from_array(std::array::from_fn(|axis| {
                            if corner & (1 << axis) == 0 {
                                cell.min()[axis]
                            } else {
                                cell.max()[axis]
                            }
                        }));
                        let offset = point - position;
                        (offset.dot(forward), offset.dot(right), offset.dot(up))
                    });
                    // The off-axis factor is exact inside the view frustum only, and cells
                    // nearer than the near plane are clipped: neither is drawn by the builder.
                    let drawn = depths_and_offsets.into_iter().all(|(depth, x, y)| {
                        depth >= near && x.abs() <= depth * tan_h && y.abs() <= depth * tan_v
                    });
                    if !drawn {
                        continue;
                    }
                    let edge = camera.projected_voxel_edge_pixels(cell, [0.0; 3], viewport);
                    assert!(
                        edge <= error * (1.0 + 1e-9),
                        "edge {edge} above metric {error} (viewport {viewport:?})"
                    );
                    if !limited {
                        assert!(edge <= target * (1.0 + 1e-9), "edge {edge} above target");
                    }
                    checked_cells += 1;
                }
            }
        }
        assert!(
            selected_nodes > 300 && checked_cells > 1000,
            "{selected_nodes} nodes, {checked_cells} cells"
        );
    }

    #[test]
    fn corner_depth_metric_selects_no_more_points_than_the_bounding_sphere_metric() {
        let hierarchy = octree(3);
        let (mut legacy_total, mut new_total, mut strictly_fewer) = (0_u64, 0_u64, 0_usize);
        for (camera, viewport) in random_views(120) {
            for target in [2.0, 6.0, 20.0] {
                let new = select(&hierarchy, &camera, viewport, target, false);
                let legacy = select(&hierarchy, &camera, viewport, target, true);
                let (Some(new), Some(legacy)) = (new, legacy) else {
                    continue;
                };
                assert!(
                    new.point_count() <= legacy.point_count(),
                    "new {} > legacy {} (target {target}, viewport {viewport:?})",
                    new.point_count(),
                    legacy.point_count()
                );
                legacy_total += legacy.point_count();
                new_total += new.point_count();
                strictly_fewer += usize::from(new.point_count() < legacy.point_count());
            }
        }
        println!(
            "points selected: legacy {legacy_total}, new {new_total}, fewer in {strictly_fewer}"
        );
        assert!(new_total < legacy_total);
    }

    /// A camera 1 m above the ground of a ~41 km wide cloud: the render near plane must not
    /// clip the foreground, and the LOD selection must not grow versus the old near plane.
    #[test]
    fn near_plane_clears_ground_one_metre_away_without_inflating_selection() {
        let side = 40_960.0;
        let hierarchy = octree_with_side(5, side);
        let viewport = [1280, 720];
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(
            SceneBounds {
                min: [0.0; 3],
                max: [side; 3],
            },
            viewport
        ));
        assert!(camera.set_pose(
            [side / 2.0, side / 2.0, 1.0],
            [side / 2.0, side / 2.0 + 100.0, 1.0]
        ));
        let (near, far) = camera.clip_distances();
        let old_near = camera.lod_min_depth();
        // Ground seen at the bottom edge of the view is at this depth along the view axis.
        let nearest_ground = 1.0 / (super::VERTICAL_FOV * 0.5).tan();
        assert!(far / near <= super::MAX_DEPTH_RATIO * 1.000_001);
        assert!(near < nearest_ground, "near {near} clips ground");
        assert!(
            old_near > nearest_ground,
            "old near {old_near} did not clip"
        );

        let (position, forward, _, _) = camera.basis();
        let config = viewer_core::LodSelectionConfig::new(6.0, u64::MAX / 2).unwrap();
        let mut results = Vec::new();
        for plane_near in [old_near, near] {
            let frustum = camera
                .spatial_frustum_with_near(viewport, plane_near)
                .unwrap();
            let selection = hierarchy
                .select_lod(&frustum, config, |node| {
                    let bounds = node.bounds();
                    let edge = bounds.max()[0] - bounds.min()[0];
                    camera.projected_error_pixels(bounds, edge / 8.0, [0.0; 3], viewport)
                })
                .unwrap();
            results.push((selection.point_count(), selection.node_ids().len()));
            if plane_near == near {
                for node_id in selection.node_ids() {
                    let bounds = hierarchy.node(*node_id).unwrap().bounds();
                    let deepest = (0..8)
                        .map(|i| {
                            let corner = DVec3::from_array(std::array::from_fn(|axis| {
                                if i & (1 << axis) == 0 {
                                    bounds.min()[axis]
                                } else {
                                    bounds.max()[axis]
                                }
                            }));
                            (corner - position).dot(forward)
                        })
                        .fold(f64::NEG_INFINITY, f64::max);
                    assert!(deepest > near, "selected node entirely behind near plane");
                }
            }
        }
        println!(
            "old near {old_near:.3} new near {near:.3}; (points, nodes=blocks) old/new: {results:?}"
        );
        assert!(results[1].0 >= results[0].0, "new near plane lost coverage");
        assert!(
            results[1].0 as f64 <= results[0].0 as f64 * 1.10,
            "selection grew more than 10%: {results:?}"
        );
    }
}
