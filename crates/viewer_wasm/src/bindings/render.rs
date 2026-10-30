use serde_wasm_bindgen::to_value;
use wasm_bindgen::prelude::*;

use super::WasmVoxelViewer;
use crate::{RenderInstanceSnapshot, lod_session::LodSession};
use viewer_render_wgpu::ColorMode;

const MAX_WASM_TRANSFER_INSTANCES: usize = 16_384;

#[wasm_bindgen]
impl WasmVoxelViewer {
    #[wasm_bindgen(js_name = setColorMode)]
    pub fn set_color_mode(&mut self, mode: String) -> Result<(), JsValue> {
        let mut selected_range = None;
        let selected_attribute = if let Some(selection) = mode.strip_prefix("attribute:") {
            let (index, component) = selection
                .split_once(':')
                .ok_or_else(|| JsValue::from_str("expected attribute:index:component"))?;
            let index = index
                .parse::<u32>()
                .map_err(|_| JsValue::from_str("invalid attribute index"))?;
            let component = component
                .parse::<u32>()
                .map_err(|_| JsValue::from_str("invalid attribute component"))?;
            let session = self
                .lod_session
                .as_ref()
                .ok_or_else(|| JsValue::from_str("no format dataset open"))?;
            let descriptor = session
                .dataset()
                .attribute_schema()
                .get(index as usize)
                .ok_or_else(|| JsValue::from_str("invalid attribute index"))?;
            if component >= descriptor.component_count() {
                return Err(JsValue::from_str("invalid attribute component"));
            }
            let (has_attributes, range) = if let Some(reader) = session.reader() {
                let range = reader
                    .attribute_range(index, component)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
                (reader.has_attributes(), range)
            } else if let Some(context) = &self.async_format_context {
                let range = context
                    .attribute_ranges
                    .get(index as usize)
                    .and_then(|components| components.get(component as usize))
                    .copied()
                    .flatten();
                (context.has_attributes, range)
            } else {
                return Err(JsValue::from_str("no format dataset open"));
            };
            if !has_attributes {
                return Err(JsValue::from_str("attach the attribute file first"));
            }
            selected_range = range;
            Some((index, component))
        } else {
            None
        };
        let mode = if selected_attribute.is_some() {
            Some(ColorMode::Rgb)
        } else {
            ColorMode::parse(&mode)
        }
        .ok_or_else(|| {
            JsValue::from_str("color mode must be rgb, intensity, elevation, or classification")
        })?;
        let renderer = self
            .gpu_renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        if renderer.color_mode() != mode || self.selected_attribute != selected_attribute {
            self.selected_attribute = selected_attribute;
            self.selected_attribute_range = selected_range;
            renderer.set_color_mode(mode);
            if let Some(dataset) = self.lod_session.as_mut().map(LodSession::dataset_mut) {
                renderer
                    .sync_visible_blocks(&[], dataset.resident_blocks())
                    .map_err(|error| JsValue::from_str(&error.to_string()))?;
                dataset.clear_loaded_blocks();
                self.reset_range_read_stats();
                self.lod_stage_pixels = Some(16.0);
                self.lod_dirty = true;
            }
        }
        Ok(())
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
