//! Test-only access to the actual viewer session; absent from normal builds.
use wasm_bindgen::prelude::*;

use super::WasmVoxelViewer;
use crate::lod_session::LodSession;

#[wasm_bindgen]
impl WasmVoxelViewer {
    #[wasm_bindgen(js_name = testReferenceSpacing)]
    pub fn test_reference_spacing(&self) -> Option<f64> {
        self.lod_session
            .as_ref()
            .map(LodSession::dataset)
            .and_then(|dataset| dataset.reference_spacing())
    }
}
