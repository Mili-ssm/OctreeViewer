#![expect(
    clippy::needless_pass_by_value,
    reason = "wasm-bindgen exports receive JS handles and owned values by value"
)]
use wasm_bindgen::prelude::*;

use super::WasmVoxelViewer;
use crate::lod_session::LodSession;

#[wasm_bindgen]
impl WasmVoxelViewer {
    #[wasm_bindgen(js_name = fitScene)]
    pub fn fit_scene(&mut self) -> Result<bool, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let fitted = if let Some(dataset) = self.lod_session.as_ref().map(LodSession::dataset) {
            dataset.fit_camera(renderer)
        } else if let Some(frame) = &self.remote_dataset_frame {
            renderer.fit_spatial_bounds(frame.bounds, frame.origin)
        } else {
            renderer.fit_scene(self.inner.render_scene())
        };
        self.camera_changed(fitted);
        Ok(fitted)
    }

    #[wasm_bindgen(js_name = orbit)]
    pub fn orbit(&mut self, delta_x: f64, delta_y: f64) -> Result<bool, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let changed = renderer.orbit_camera(delta_x, delta_y);
        self.camera_changed(changed);
        Ok(changed)
    }

    #[wasm_bindgen(js_name = pan)]
    pub fn pan(&mut self, delta_x: f64, delta_y: f64) -> Result<bool, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let changed = renderer.pan_camera(delta_x, delta_y);
        self.camera_changed(changed);
        Ok(changed)
    }

    #[wasm_bindgen(js_name = zoom)]
    pub fn zoom(&mut self, delta: f64) -> Result<bool, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let changed = renderer.zoom_camera(delta);
        self.camera_changed(changed);
        Ok(changed)
    }

    /// Epoch of the view `cameraTransport` returns now: bumped by every camera move and resize.
    /// Send it to the builder (`setViewEpoch`) with that camera.
    #[wasm_bindgen(js_name = viewEpoch)]
    pub fn view_epoch(&self) -> f64 {
        self.view_epoch as f64
    }

    /// Returns the camera pose in source dataset coordinates.
    #[wasm_bindgen(js_name = cameraPose)]
    pub fn camera_pose(&self) -> Result<JsValue, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_ref()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let (position, target) = renderer.camera_pose();
        let pose = js_sys::Object::new();
        js_sys::Reflect::set(
            &pose,
            &"position".into(),
            &js_sys::Float64Array::from(position.as_slice()),
        )?;
        js_sys::Reflect::set(
            &pose,
            &"target".into(),
            &js_sys::Float64Array::from(target.as_slice()),
        )?;
        Ok(pose.into())
    }

    /// Sets camera position and orbit target in source dataset coordinates.
    #[wasm_bindgen(js_name = setCameraPose)]
    pub fn set_camera_pose(
        &mut self,
        position: js_sys::Float64Array,
        target: js_sys::Float64Array,
    ) -> Result<bool, JsValue> {
        if position.length() != 3 || target.length() != 3 {
            return Err(JsValue::from_str(
                "camera position and target must have 3 coordinates",
            ));
        }
        let mut position_values = [0.0; 3];
        let mut target_values = [0.0; 3];
        position.copy_to(&mut position_values);
        target.copy_to(&mut target_values);
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let changed = renderer.set_camera_pose(position_values, target_values);
        self.camera_changed(changed);
        Ok(changed)
    }

    /// Moves the orbit center while preserving the camera position.
    #[wasm_bindgen(js_name = setOrbitTarget)]
    pub fn set_orbit_target(&mut self, target: js_sys::Float64Array) -> Result<bool, JsValue> {
        if target.length() != 3 {
            return Err(JsValue::from_str("camera target must have 3 coordinates"));
        }
        let mut target_values = [0.0; 3];
        target.copy_to(&mut target_values);
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let changed = renderer.set_orbit_target(target_values);
        self.camera_changed(changed);
        Ok(changed)
    }

    /// Translates the camera in source dataset coordinates.
    #[wasm_bindgen(js_name = moveCamera)]
    pub fn move_camera(&mut self, x: f64, y: f64, z: f64) -> Result<bool, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let changed = renderer.translate_camera([x, y, z]);
        self.camera_changed(changed);
        Ok(changed)
    }

    /// Flies the camera relative to the view: `forward` on the horizontal plane, `right` along the
    /// screen's right and `up` along dataset Z, all in units of the orbit distance.
    #[wasm_bindgen(js_name = flyCamera)]
    pub fn fly_camera(&mut self, forward: f64, right: f64, up: f64) -> Result<bool, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let changed = renderer.fly_camera(forward, right, up);
        self.camera_changed(changed);
        Ok(changed)
    }

    /// Points the camera down along the dataset's positive Z up axis.
    #[wasm_bindgen(js_name = setBirdView)]
    pub fn set_bird_view(&mut self) -> Result<(), JsValue> {
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        renderer.set_bird_view();
        self.camera_changed(true);
        Ok(())
    }
}

impl WasmVoxelViewer {
    /// The view `cameraTransport` describes is newer: a camera move or a resize. Bumps
    /// `viewEpoch`, which the host hands to the builder with the camera, so a cut can be told
    /// from the cuts of older views.
    pub(super) fn camera_changed(&mut self, changed: bool) {
        if !changed {
            return;
        }
        self.view_epoch += 1;
        if let Some(cells) = &mut self.cells_remote {
            cells.camera_changed(self.view_epoch);
        }
        if let Some(cells) = &mut self.cells_local {
            cells.mark_dirty();
        }
        if self.lod_session.is_some() {
            self.lod_dirty = self.lod_config.is_some();
        }
    }
}
