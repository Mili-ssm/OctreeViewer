#![expect(
    clippy::needless_pass_by_value,
    reason = "wasm-bindgen exports receive JS handles and owned values by value"
)]
use serde_wasm_bindgen::to_value;
use wasm_bindgen::prelude::*;

use super::attribute_availability;
use super::{RemoteDatasetFrame, WasmVoxelViewer};
use viewer_core::{BlockId, SpatialBounds};
use viewer_render_wgpu::{ColorMode, InstanceRaw, RenderScene};

use crate::{
    RenderInstanceSnapshot,
    lod_driver::{ColorSelection, resolve_color_selection},
    lod_session::LodSession,
};

const MAX_WASM_TRANSFER_INSTANCES: usize = 16_384;

#[wasm_bindgen]
impl WasmVoxelViewer {
    #[wasm_bindgen(js_name = setColorMode)]
    pub fn set_color_mode(&mut self, mode: String) -> Result<(), JsValue> {
        let selection = {
            let session = self.lod_session.as_ref();
            let async_context = self.async_format_context.as_ref();
            resolve_color_selection(
                &mode,
                session.map(|session| session.dataset().attribute_schema()),
                |index, component| attribute_availability(session, async_context, index, component),
            )
            .map_err(|message| JsValue::from_str(&message))?
        };
        let ColorSelection {
            mode,
            attribute: selected_attribute,
            attribute_range: selected_range,
        } = selection;
        // Shader modes on live cells only change the uniform: no cells rebuilt, no pool reset.
        let uniform_only = crate::cells_glue::shader_mode_only(
            self.cells_local.is_some(),
            self.selected_attribute.is_some(),
            selected_attribute.is_some(),
        );
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        if uniform_only {
            renderer.set_color_mode(mode);
            return Ok(());
        }
        if renderer.color_mode() != mode || self.selected_attribute != selected_attribute {
            self.selected_attribute = selected_attribute;
            self.selected_attribute_range = selected_range;
            renderer.set_color_mode(mode);
            if let Some(dataset) = self.lod_session.as_mut().map(LodSession::dataset_mut) {
                renderer
                    .sync_visible_blocks(&[], dataset.resident_blocks())
                    .map_err(|error| JsValue::from_str(&error.to_string()))?;
                dataset.clear_loaded_blocks();
                if let Some(cells) = self.cells_local.as_mut() {
                    // R3-Q33: a new attribute starts a new colour generation of the cells.
                    if dataset.set_cell_paint(selected_attribute) {
                        cells.mark_dirty();
                    }
                }
                self.last_scene_digest = None;
                self.reset_range_read_stats();
                self.lod_stage_pixels = Some(16.0);
                self.lod_dirty = true;
            }
        }
        Ok(())
    }

    /// Selects how the renderer shades the displayed scene when the LOD hierarchy lives in a
    /// build worker. `attribute:*` modes are painted into the colours by the builder, so they
    /// render as `rgb`.
    #[wasm_bindgen(js_name = setPresentationColorMode)]
    pub fn set_presentation_color_mode(&mut self, mode: String) -> Result<(), JsValue> {
        let mode = if mode.starts_with("attribute:") {
            ColorMode::Rgb
        } else {
            ColorMode::parse(&mode).ok_or_else(|| {
                JsValue::from_str("color mode must be rgb, intensity, elevation, or classification")
            })?
        };
        self.gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?
            .set_color_mode(mode);
        Ok(())
    }

    /// Prepares the renderer for a dataset whose hierarchy and blocks live in a build worker:
    /// drops the previous scene, adopts the dataset's visual scale and frames its root bounds.
    #[wasm_bindgen(js_name = beginRemoteDataset)]
    pub fn begin_remote_dataset(
        &mut self,
        visual_scale: f64,
        origin: js_sys::Float64Array,
        bounds_min: js_sys::Float64Array,
        bounds_max: js_sys::Float64Array,
    ) -> Result<bool, JsValue> {
        let vector = |values: &js_sys::Float64Array, name: &str| -> Result<[f64; 3], JsValue> {
            if values.length() != 3 {
                return Err(JsValue::from_str(&format!(
                    "{name} must have 3 coordinates"
                )));
            }
            let mut out = [0.0; 3];
            values.copy_to(&mut out);
            Ok(out)
        };
        let origin = vector(&origin, "origin")?;
        let bounds = SpatialBounds::new(
            vector(&bounds_min, "boundsMin")?,
            vector(&bounds_max, "boundsMax")?,
        )
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
        self.clear_lod_dataset();
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        renderer.clear_dataset();
        renderer.set_color_mode(ColorMode::Rgb);
        renderer
            .set_visual_scale(visual_scale)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let fitted = renderer.fit_spatial_bounds(bounds, origin);
        self.remote_dataset_frame = Some(RemoteDatasetFrame { bounds, origin });
        self.gpu_scene_dirty = false;
        Ok(fitted)
    }

    /// Uploads the `InstanceRaw` bytes a `WasmSceneBuilder` produced as the displayed scene.
    /// `block_ids` are decimal strings, kept for diagnostics. Returns the bytes written.
    #[wasm_bindgen(js_name = uploadSceneInstances)]
    pub fn upload_scene_instances(
        &mut self,
        instances: js_sys::Uint8Array,
        block_ids: Vec<String>,
    ) -> Result<f64, JsValue> {
        let stride = std::mem::size_of::<InstanceRaw>();
        let length = instances.length() as usize;
        if !length.is_multiple_of(stride) {
            return Err(JsValue::from_str(
                "scene bytes are not a whole number of instances",
            ));
        }
        let mut scene = vec![<InstanceRaw as bytemuck::Zeroable>::zeroed(); length / stride];
        instances.copy_to(bytemuck::cast_slice_mut(&mut scene));
        let block_ids = block_ids
            .iter()
            .map(|id| {
                id.parse::<u64>()
                    .map(BlockId::new)
                    .map_err(|_| JsValue::from_str("block ids must be decimal integers"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        renderer
            .upload_voxel_cut(&RenderScene::from_instances(scene), &block_ids)
            .map(|bytes| bytes as f64)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Voxel construction limits a `WasmSceneBuilder` must use: `[maxVoxels, maxEstimatedBytes]`,
    /// already capped by this renderer's GPU instance budget.
    #[wasm_bindgen(js_name = voxelBuildLimits)]
    pub fn voxel_build_limits(&self) -> Result<js_sys::Float64Array, JsValue> {
        let limits = self
            .gpu_renderer
            .as_ref()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?
            .voxel_build_limits();
        Ok(js_sys::Float64Array::from(
            [limits.max_voxels as f64, limits.max_estimated_bytes as f64].as_slice(),
        ))
    }

    /// The camera in the lossless 11-number form `WasmSceneBuilder.update` takes.
    #[wasm_bindgen(js_name = cameraTransport)]
    pub fn camera_transport(&self) -> Result<js_sys::Float64Array, JsValue> {
        let camera = self
            .gpu_renderer
            .as_ref()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?
            .camera_snapshot();
        Ok(js_sys::Float64Array::from(camera.to_f64_array().as_slice()))
    }

    #[wasm_bindgen(js_name = setDepthShading)]
    pub fn set_depth_shading(&mut self, enabled: bool) -> Result<(), JsValue> {
        self.gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?
            .set_depth_shading(enabled);
        Ok(())
    }

    #[wasm_bindgen(js_name = renderSceneBatch)]
    /// Returns a bounded diagnostic page of the current CPU render scene.
    pub fn render_scene_batch(&self, start: u32, count: u32) -> Result<JsValue, JsValue> {
        if count == 0 || count as usize > MAX_WASM_TRANSFER_INSTANCES {
            return Err(JsValue::from_str(
                "renderSceneBatch count must be between 1 and 16384",
            ));
        }
        let instances = self.inner.render_scene().instances();
        let start = (start as usize).min(instances.len());
        let end = start.saturating_add(count as usize).min(instances.len());
        let scene = instances[start..end]
            .iter()
            .copied()
            .map(RenderInstanceSnapshot::from)
            .collect::<Vec<_>>();
        let result = js_sys::Object::new();
        js_sys::Reflect::set(&result, &"start".into(), &JsValue::from_f64(start as f64))?;
        js_sys::Reflect::set(
            &result,
            &"total".into(),
            &JsValue::from_f64(instances.len() as f64),
        )?;
        js_sys::Reflect::set(
            &result,
            &"instances".into(),
            &to_value(&scene).map_err(|error| JsValue::from_str(&error.to_string()))?,
        )?;
        Ok(result.into())
    }

    #[wasm_bindgen(js_name = renderFrame)]
    pub fn render_frame(&mut self) -> Result<String, JsValue> {
        self.refresh_lod_if_dirty()?;
        self.run_remote_cells_frame();
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        if self.gpu_scene_dirty && self.lod_session.is_none() {
            renderer
                .upload_scene(self.inner.render_scene())
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            self.gpu_scene_dirty = false;
        }
        renderer
            .render()
            .map(|status| match status {
                viewer_render_wgpu::FrameStatus::Presented => "presented",
                viewer_render_wgpu::FrameStatus::SkippedTimeout => "skipped_timeout",
                viewer_render_wgpu::FrameStatus::Reconfigured => "reconfigured",
                viewer_render_wgpu::FrameStatus::Suspended => "suspended",
            })
            .map(str::to_owned)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Picks the frontmost rendered voxel at a framebuffer pixel. Coordinates and voxel data are
    /// returned in the source dataset's coordinate system; `None` means that the ray hit no voxel.
    #[wasm_bindgen]
    pub async fn pick(&self, x: f64, y: f64) -> Result<JsValue, JsValue> {
        let readback = {
            let renderer = self
                .gpu_renderer
                .as_ref()
                .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
            renderer
                .prepare_pick([x, y])
                .map_err(|error| JsValue::from_str(&error))?
        };
        let Some(readback) = readback else {
            return Ok(JsValue::NULL);
        };
        let buffer = readback.buffer().clone();
        let map_buffer = buffer.clone();
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            map_buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| match result {
                    Ok(()) => {
                        let _ = resolve.call0(&JsValue::UNDEFINED);
                    }
                    Err(error) => {
                        let _ = reject
                            .call1(&JsValue::UNDEFINED, &JsValue::from_str(&error.to_string()));
                    }
                });
        });
        wasm_bindgen_futures::JsFuture::from(promise).await?;
        let mapped = buffer.slice(..).get_mapped_range().map_err(|error| {
            buffer.unmap();
            JsValue::from_str(&error.to_string())
        })?;
        let hit = self
            .gpu_renderer
            .as_ref()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?
            .resolve_pick(&readback, &mapped);
        drop(mapped);
        buffer.unmap();
        let hit = hit.map_err(|error| JsValue::from_str(&error))?;
        let Some(hit) = hit else {
            return Ok(JsValue::NULL);
        };
        let result = js_sys::Object::new();
        js_sys::Reflect::set(
            &result,
            &"instanceIndex".into(),
            &JsValue::from_f64(hit.instance_index as f64),
        )?;
        js_sys::Reflect::set(
            &result,
            &"position".into(),
            &js_sys::Float64Array::from(hit.position.as_slice()),
        )?;
        js_sys::Reflect::set(
            &result,
            &"voxelCenter".into(),
            &js_sys::Float64Array::from(hit.voxel_center.as_slice()),
        )?;
        js_sys::Reflect::set(
            &result,
            &"voxelSize".into(),
            &JsValue::from_f64(hit.voxel_size),
        )?;
        js_sys::Reflect::set(
            &result,
            &"distance".into(),
            &JsValue::from_f64(hit.distance),
        )?;
        Ok(result.into())
    }

    #[wasm_bindgen(js_name = viewportSize)]
    pub fn viewport_size(&self) -> Result<js_sys::Uint32Array, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_ref()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let [width, height] = renderer.viewport_size();
        Ok(js_sys::Uint32Array::from([width, height].as_slice()))
    }
}
