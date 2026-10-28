use wasm_bindgen::prelude::*;

use super::{
    WasmVoxelViewer,
    js_marshal::{points_from_typed_arrays, raw_scene_point_limit, viewer_error_to_js},
};

#[wasm_bindgen]
impl WasmVoxelViewer {
    #[wasm_bindgen(js_name = replacePoints)]
    pub fn replace_points(
        &mut self,
        positions: js_sys::Float32Array,
        colors: js_sys::Uint8Array,
    ) -> Result<(), JsValue> {
        let points = points_from_typed_arrays(&positions, &colors, raw_scene_point_limit())?;
        self.inner.replace_points_owned(points);
        self.gpu_scene_dirty = true;
        self.clear_lod_dataset();
        Ok(())
    }

    #[wasm_bindgen(js_name = appendPoints)]
    pub fn append_points(
        &mut self,
        positions: js_sys::Float32Array,
        colors: js_sys::Uint8Array,
    ) -> Result<(), JsValue> {
        let remaining_points = raw_scene_point_limit()
            .saturating_sub(usize::try_from(self.inner.stats().point_count).unwrap_or(usize::MAX));
        let points = points_from_typed_arrays(&positions, &colors, remaining_points)?;
        self.inner
            .append_points_owned(points)
            .map_err(viewer_error_to_js)?;
        self.gpu_scene_dirty = true;
        self.clear_lod_dataset();
        Ok(())
    }

    #[wasm_bindgen(js_name = replaceVoxelizedPoints)]
    pub fn replace_voxelized_points(
        &mut self,
        positions: js_sys::Float32Array,
        colors: js_sys::Uint8Array,
    ) -> Result<(), JsValue> {
        let points = points_from_typed_arrays(&positions, &colors, raw_scene_point_limit())?;
        self.inner
            .replace_voxelized_points_owned(points)
            .map_err(viewer_error_to_js)?;
        self.gpu_scene_dirty = true;
        self.clear_lod_dataset();
        Ok(())
    }

    #[wasm_bindgen(js_name = appendVoxelizedPoints)]
    pub fn append_voxelized_points(
        &mut self,
        positions: js_sys::Float32Array,
        colors: js_sys::Uint8Array,
    ) -> Result<(), JsValue> {
        let remaining_points = raw_scene_point_limit()
            .saturating_sub(usize::try_from(self.inner.stats().point_count).unwrap_or(usize::MAX));
        let points = points_from_typed_arrays(&positions, &colors, remaining_points)?;
        self.inner
            .append_voxelized_points_owned(points)
            .map_err(viewer_error_to_js)?;
        self.gpu_scene_dirty = true;
        self.clear_lod_dataset();
        Ok(())
    }
}
