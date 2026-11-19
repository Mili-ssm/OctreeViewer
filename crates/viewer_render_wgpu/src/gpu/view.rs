//! Camera, fitting and display settings.

use super::{SurfaceRenderer, error::GpuRendererError};

use crate::{ColorMode, OrbitCamera, RenderScene, SceneBounds, VoxelBuildLimits};

impl<'window> SurfaceRenderer<'window> {
    pub fn max_visible_instances(&self) -> usize {
        self.gpu_budget.max_visible_instances
    }

    pub fn voxel_build_limits(&self) -> VoxelBuildLimits {
        VoxelBuildLimits {
            max_voxels: self
                .voxel_build_limits
                .max_voxels
                .min(self.gpu_budget.max_visible_instances),
            ..self.voxel_build_limits
        }
    }

    pub fn set_color_mode(&mut self, mode: ColorMode) {
        self.color_mode = mode;
    }

    pub fn color_mode(&self) -> ColorMode {
        self.color_mode
    }

    pub fn set_visual_scale(&mut self, scale: f64) -> Result<(), GpuRendererError> {
        if !scale.is_finite() || scale <= 0.0 || !(scale as f32).is_finite() || scale as f32 == 0.0
        {
            return Err(GpuRendererError::InvalidVisualScale);
        }
        self.visual_scale = scale;
        self.hzb_reference_camera = None;
        Ok(())
    }

    pub fn set_depth_shading(&mut self, enabled: bool) {
        self.depth_shading = enabled;
    }

    pub fn fit_scene(&mut self, scene: &RenderScene) -> bool {
        self.coordinate_origin = [0.0; 3];
        self.scene_bounds = scene.bounds();
        self.display_bounds().is_some_and(|bounds| {
            self.camera.fit(
                bounds,
                [self.surface_config.width, self.surface_config.height],
            )
        })
    }

    pub(super) fn display_bounds(&self) -> Option<SceneBounds> {
        let bounds = self.scene_bounds?;
        let min = bounds.min.map(|value| value * self.visual_scale);
        let max = bounds.max.map(|value| value * self.visual_scale);
        if min.iter().chain(&max).all(|value| value.is_finite()) {
            Some(SceneBounds { min, max })
        } else {
            None
        }
    }

    pub fn fit_visible_blocks(&mut self) -> bool {
        self.display_bounds().is_some_and(|bounds| {
            self.camera.fit(
                bounds,
                [self.surface_config.width, self.surface_config.height],
            )
        })
    }

    pub fn fit_spatial_bounds(
        &mut self,
        bounds: viewer_core::SpatialBounds,
        coordinate_origin: [f64; 3],
    ) -> bool {
        let local = SceneBounds {
            min: std::array::from_fn(|axis| bounds.min()[axis] - coordinate_origin[axis]),
            max: std::array::from_fn(|axis| bounds.max()[axis] - coordinate_origin[axis]),
        };
        let display = SceneBounds {
            min: local.min.map(|value| value * self.visual_scale),
            max: local.max.map(|value| value * self.visual_scale),
        };
        if display
            .min
            .iter()
            .chain(&display.max)
            .any(|value| !value.is_finite())
        {
            return false;
        }
        if self.camera.fit(
            display,
            [self.surface_config.width, self.surface_config.height],
        ) {
            self.scene_bounds = Some(local);
            self.coordinate_origin = coordinate_origin;
            true
        } else {
            false
        }
    }

    pub fn orbit_camera(&mut self, delta_x: f64, delta_y: f64) -> bool {
        self.camera.orbit(delta_x, delta_y)
    }

    /// Returns position and target in the source dataset's coordinate system.
    pub fn camera_pose(&self) -> ([f64; 3], [f64; 3]) {
        let to_dataset = |point: [f64; 3]| {
            std::array::from_fn(|axis| {
                point[axis] / self.visual_scale + self.coordinate_origin[axis]
            })
        };
        (
            to_dataset(self.camera.position()),
            to_dataset(self.camera.target()),
        )
    }

    /// Exact local camera state for detecting changes to voxel projection inputs.
    pub fn camera_snapshot(&self) -> OrbitCamera {
        self.camera
    }

    /// Sets the pose in source dataset coordinates.
    pub fn set_camera_pose(&mut self, position: [f64; 3], target: [f64; 3]) -> bool {
        let to_local = |point: [f64; 3]| {
            std::array::from_fn(|axis| {
                (point[axis] - self.coordinate_origin[axis]) * self.visual_scale
            })
        };
        let changed = self.camera.set_pose(to_local(position), to_local(target));
        if changed {
            self.hzb_reference_camera = None;
        }
        changed
    }

    pub fn set_orbit_target(&mut self, target: [f64; 3]) -> bool {
        let target = std::array::from_fn(|axis| {
            (target[axis] - self.coordinate_origin[axis]) * self.visual_scale
        });
        let changed = self.camera.set_target(target);
        if changed {
            self.hzb_reference_camera = None;
        }
        changed
    }

    pub fn translate_camera(&mut self, delta: [f64; 3]) -> bool {
        let delta = delta.map(|value| value * self.visual_scale);
        let changed = self.camera.translate(delta);
        if changed {
            self.hzb_reference_camera = None;
        }
        changed
    }

    /// Flies the camera by amounts in units of the orbit distance (see `OrbitCamera::fly`).
    pub fn fly_camera(&mut self, forward: f64, right: f64, up: f64) -> bool {
        let changed = self.camera.fly(forward, right, up);
        if changed {
            self.hzb_reference_camera = None;
        }
        changed
    }

    pub fn set_bird_view(&mut self) {
        self.camera.set_bird_view();
        self.hzb_reference_camera = None;
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

    /// GPU-free snapshot of the view math used by LOD selection and voxel construction.
    pub fn view_projection(&self) -> crate::ViewProjection {
        crate::ViewProjection::new(
            self.camera,
            self.viewport_size(),
            self.visual_scale,
            self.voxel_build_limits(),
        )
    }

    pub fn visual_scale(&self) -> f64 {
        self.visual_scale
    }

    pub fn viewport_size(&self) -> [u32; 2] {
        [self.surface_config.width, self.surface_config.height]
    }
}
