use serde_wasm_bindgen::to_value;
use wasm_bindgen::prelude::*;

use crate::{RenderSceneSnapshot, ViewerConfig, ViewerController, ViewerError};

#[wasm_bindgen]
pub struct WasmVoxelViewer {
    inner: ViewerController,
}

#[wasm_bindgen]
impl WasmVoxelViewer {
    #[wasm_bindgen(constructor)]
    pub fn new(voxel_size: f32) -> Result<WasmVoxelViewer, JsValue> {
        let inner = ViewerController::new(ViewerConfig {
            voxel_size,
            ..ViewerConfig::default()
        })
        .map_err(viewer_error_to_js)?;

        Ok(Self { inner })
    }

    #[wasm_bindgen(js_name = resize)]
    pub fn resize(&mut self, width: u32, height: u32) {
        self.inner.resize(width, height);
    }

    #[wasm_bindgen(js_name = replacePoints)]
    pub fn replace_points(&mut self, positions: Vec<f32>, colors: Vec<u8>) -> Result<(), JsValue> {
        self.inner
            .replace_points(&positions, &colors)
            .map_err(viewer_error_to_js)
    }

    #[wasm_bindgen(js_name = appendPoints)]
    pub fn append_points(&mut self, positions: Vec<f32>, colors: Vec<u8>) -> Result<(), JsValue> {
        self.inner
            .append_points(&positions, &colors)
            .map_err(viewer_error_to_js)
    }

    #[wasm_bindgen(js_name = clear)]
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    #[wasm_bindgen(js_name = stats)]
    pub fn stats(&self) -> Result<JsValue, JsValue> {
        to_value(&self.inner.stats()).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    #[wasm_bindgen(js_name = renderScene)]
    pub fn render_scene(&self) -> Result<JsValue, JsValue> {
        let scene = RenderSceneSnapshot::from(self.inner.render_scene());
        to_value(&scene).map_err(|error| JsValue::from_str(&error.to_string()))
    }
}

fn viewer_error_to_js(error: ViewerError) -> JsValue {
    JsValue::from_str(&error.to_string())
}
