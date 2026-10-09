use glam::{DMat4, DVec3};

use crate::SceneBounds;

const VERTICAL_FOV: f64 = 45.0_f64.to_radians();
const MIN_PITCH: f64 = -89.0_f64.to_radians();
const MAX_PITCH: f64 = 89.0_f64.to_radians();

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraFrame {
    /// Column-major matrix using WebGPU's zero-to-one depth range.
    pub view_projection: [[f32; 4]; 4],
    /// Local scene origin subtracted in the vertex shader before projection.
    pub render_origin: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    target: DVec3,
    distance: f64,
    yaw: f64,
    pitch: f64,
    radius: f64,
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
            fitted: false,
        }
    }
}

impl OrbitCamera {
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
        if !target.is_finite() || !radius.is_finite() {
            return false;
        }
        let radius = radius.max(0.001);
        let aspect = f64::from(viewport[0]) / f64::from(viewport[1]);
        let horizontal_fov = 2.0 * ((VERTICAL_FOV * 0.5).tan() * aspect).atan();
        let limiting_half_fov = (VERTICAL_FOV.min(horizontal_fov)) * 0.5;
        let distance = radius / limiting_half_fov.sin() * 1.08;
        if !distance.is_finite() || distance > f64::from(f32::MAX) * 0.1 {
            return false;
        }

        self.target = target;
        self.radius = radius;
        self.distance = distance;
        self.yaw = 0.0;
        self.pitch = 0.55;
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
        if !target.is_finite() {
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
        self.distance = (self.distance * factor).clamp(
            (self.radius * 0.01).max(0.001),
            (self.radius * 1.0e6).max(1.0),
        );
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
        let near = (self.distance - self.radius * 1.2)
            .max(self.radius * 0.001)
            .max(0.001);
        let far = (self.distance + self.radius * 2.0).max(near * 2.0);
        let view = DMat4::look_at_rh(offset, DVec3::ZERO, DVec3::Z);
        let projection = DMat4::perspective_rh(VERTICAL_FOV, aspect, near, far);
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

    pub fn is_fitted(&self) -> bool {
        self.fitted
    }
}

#[cfg(test)]
mod tests {
    use glam::Mat4;

    use super::OrbitCamera;
    use crate::SceneBounds;

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
}
