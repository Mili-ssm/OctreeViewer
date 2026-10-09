use serde_wasm_bindgen::to_value;
use std::io::{self, Read, Seek, SeekFrom};
use wasm_bindgen::prelude::*;
use web_sys::{Blob, FileReaderSync};

use crate::{LasPointStream, RenderSceneSnapshot, ViewerConfig, ViewerController, ViewerError};

const FILE_CACHE_BYTES: u64 = 256 * 1024;

#[wasm_bindgen]
pub struct WasmLasImportSession {
    inner: LasPointStream,
}

#[wasm_bindgen]
impl WasmLasImportSession {
    /// Opens a LAS or LAZ Blob. Construct this class in a dedicated Web Worker.
    #[wasm_bindgen(constructor)]
    pub fn new(file: Blob) -> Result<WasmLasImportSession, JsValue> {
        let source = BrowserBlobReader::new(file)?;
        let inner =
            LasPointStream::new(source).map_err(|error| JsValue::from_str(&error.to_string()))?;
        Ok(Self { inner })
    }

    #[wasm_bindgen(js_name = readNext)]
    pub fn read_next(&mut self, requested_points: u32) -> Result<JsValue, JsValue> {
        let chunk = self
            .inner
            .read_next(u64::from(requested_points))
            .map_err(|error| JsValue::from_str(&error.to_string()))?;

        let result = js_sys::Object::new();
        let positions = js_sys::Float32Array::from(chunk.positions.as_slice());
        let colors = js_sys::Uint8Array::from(chunk.colors.as_slice());
        let origin = js_sys::Float64Array::from(chunk.origin.as_slice());

        js_sys::Reflect::set(&result, &"positions".into(), &positions)?;
        js_sys::Reflect::set(&result, &"colors".into(), &colors)?;
        js_sys::Reflect::set(&result, &"origin".into(), &origin)?;
        js_sys::Reflect::set(
            &result,
            &"processedPoints".into(),
            &JsValue::from_str(&chunk.processed_points.to_string()),
        )?;
        js_sys::Reflect::set(
            &result,
            &"totalPoints".into(),
            &JsValue::from_str(&chunk.total_points.to_string()),
        )?;
        js_sys::Reflect::set(&result, &"done".into(), &JsValue::from_bool(chunk.done))?;
        Ok(result.into())
    }
}

struct BrowserBlobReader {
    blob: Blob,
    file_reader: FileReaderSync,
    file_size: u64,
    position: u64,
    cached_start: u64,
    cache: Vec<u8>,
}

impl BrowserBlobReader {
    fn new(blob: Blob) -> Result<Self, JsValue> {
        let size = blob.size();
        if !size.is_finite() || size < 0.0 || size > u64::MAX as f64 {
            return Err(JsValue::from_str(
                "file size is outside the supported range",
            ));
        }

        Ok(Self {
            blob,
            file_reader: FileReaderSync::new()?,
            file_size: size as u64,
            position: 0,
            cached_start: u64::MAX,
            cache: Vec::new(),
        })
    }

    fn load_cache(&mut self) -> io::Result<()> {
        let start = self.position / FILE_CACHE_BYTES * FILE_CACHE_BYTES;
        let end = start.saturating_add(FILE_CACHE_BYTES).min(self.file_size);
        let slice = self
            .blob
            .slice_with_f64_and_f64(start as f64, end as f64)
            .map_err(js_error_to_io)?;
        let array_buffer = self
            .file_reader
            .read_as_array_buffer(&slice)
            .map_err(js_error_to_io)?;
        let array = js_sys::Uint8Array::new(&array_buffer);
        self.cache.resize((end - start) as usize, 0);
        array.copy_to(&mut self.cache);
        self.cached_start = start;
        Ok(())
    }
}

impl Read for BrowserBlobReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.position >= self.file_size {
            return Ok(0);
        }
        if self.cached_start == u64::MAX
            || self.position < self.cached_start
            || self.position >= self.cached_start + self.cache.len() as u64
        {
            self.load_cache()?;
        }

        let cache_offset = (self.position - self.cached_start) as usize;
        let count = output
            .len()
            .min(self.cache.len().saturating_sub(cache_offset));
        output[..count].copy_from_slice(&self.cache[cache_offset..cache_offset + count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for BrowserBlobReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.file_size) + i128::from(offset),
        };
        if !(0..=i128::from(u64::MAX)).contains(&next) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek position is outside the supported range",
            ));
        }
        self.position = next as u64;
        Ok(self.position)
    }
}

fn js_error_to_io(error: JsValue) -> io::Error {
    let message = error
        .as_string()
        .unwrap_or_else(|| "browser file read failed".to_owned());
    io::Error::other(message)
}

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
