//! Spool tiers backed by the JS object `createSpoolHost()` (`examples/web-host/spool-tiers.mjs`):
//! pooled `ArrayBuffer` segments for RAM and sync access handles for the spill zones. The import
//! is structural (no snippet, no `inline_js`) and every byte crosses as a view of wasm memory
//! that is only valid during the call.

use std::io;

use viewer_convert_core::{
    ConvertError, PageTier, RamTier, SegmentKey, pipeline::hybrid::storage_full,
    pipeline::spool_store::PlanStore,
};
use wasm_bindgen::prelude::*;

use crate::browser_io::js_message;

#[wasm_bindgen]
extern "C" {
    /// Object from `createSpoolHost()` (spool-tiers.mjs).
    pub type SpoolHost;

    #[wasm_bindgen(method, getter, js_name = zoneCount)]
    fn zone_count(this: &SpoolHost) -> u32;
    #[wasm_bindgen(method, catch, js_name = ramAlloc)]
    fn ram_alloc(this: &SpoolHost, key: u32, capacity: u32) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch, js_name = ramWrite)]
    fn ram_write(this: &SpoolHost, key: u32, at: u32, bytes: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch, js_name = ramRead)]
    fn ram_read(this: &SpoolHost, key: u32, at: u32, out: &mut [u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch, js_name = ramShrink)]
    fn ram_shrink(this: &SpoolHost, key: u32, capacity: u32) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch, js_name = ramFree)]
    fn ram_free(this: &SpoolHost, key: u32) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch, js_name = pageWrite)]
    fn page_write(this: &SpoolHost, zone: u32, at: f64, bytes: &[u8]) -> Result<f64, JsValue>;
    #[wasm_bindgen(method, catch, js_name = pageRead)]
    fn page_read(this: &SpoolHost, zone: u32, at: f64, out: &mut [u8]) -> Result<f64, JsValue>;
    #[wasm_bindgen(method, catch, js_name = zoneRelease)]
    fn zone_release(this: &SpoolHost, zone: u32) -> Result<(), JsValue>;
}

pub(crate) struct JsRamTier(pub(crate) SpoolHost);
pub(crate) struct JsPageTier(pub(crate) SpoolHost);

// `JsValue` is `Send + Sync` only without `atomics`; enabling them must break the build here
// instead of silently making the store unsound.
const _: fn() = || {
    fn plan_store<T: PlanStore>() {}
    plan_store::<viewer_convert_core::HybridSpoolStore<JsRamTier, JsPageTier>>();
};

fn message_of(error: &JsValue) -> String {
    js_message(error).unwrap_or_else(|| "unknown JavaScript error".to_owned())
}

fn host_error(what: &str, error: &JsValue) -> ConvertError {
    ConvertError::Io(io::Error::other(format!("{what}: {}", message_of(error))))
}

impl RamTier for JsRamTier {
    fn alloc(&self, key: SegmentKey, capacity: u32) -> Result<(), ConvertError> {
        self.0.ram_alloc(key.0, capacity).map_err(|error| {
            ConvertError::Io(io::Error::new(
                io::ErrorKind::OutOfMemory,
                format!(
                    "could not allocate a {capacity}-byte spool segment: {}",
                    message_of(&error)
                ),
            ))
        })
    }

    fn write(&self, key: SegmentKey, at: u32, bytes: &[u8]) -> Result<(), ConvertError> {
        self.0
            .ram_write(key.0, at, bytes)
            .map_err(|error| host_error("spool RAM write failed", &error))
    }

    fn read(&self, key: SegmentKey, at: u32, out: &mut [u8]) -> Result<(), ConvertError> {
        self.0
            .ram_read(key.0, at, out)
            .map_err(|error| host_error("spool RAM read failed", &error))
    }

    fn shrink(&self, key: SegmentKey, capacity: u32) -> Result<(), ConvertError> {
        self.0
            .ram_shrink(key.0, capacity)
            .map_err(|error| host_error("spool RAM shrink failed", &error))
    }

    fn free(&self, key: SegmentKey) -> Result<(), ConvertError> {
        self.0
            .ram_free(key.0)
            .map_err(|error| host_error("spool RAM free failed", &error))
    }
}

/// A host byte count must be an integer within `limit`.
fn count(value: f64, limit: usize) -> Option<usize> {
    (value >= 0.0 && value <= limit as f64 && value.fract() == 0.0).then_some(value as usize)
}

impl PageTier for JsPageTier {
    fn zone_count(&self) -> u32 {
        self.0.zone_count()
    }

    fn write(&self, zone: u32, at: u64, bytes: &[u8]) -> Result<usize, ConvertError> {
        let written = self
            .0
            .page_write(zone, at as f64, bytes)
            .map_err(|error| host_error(&format!("spill zone {zone} write failed"), &error))?;
        let Some(written) = count(written, bytes.len()) else {
            return Err(ConvertError::Io(io::Error::other(format!(
                "spill zone {zone} write returned an invalid byte count"
            ))));
        };
        if written < bytes.len() {
            return Err(storage_full(
                &format!("spill zone {zone}"),
                written,
                bytes.len(),
            ));
        }
        Ok(written)
    }

    fn read(&self, zone: u32, at: u64, out: &mut [u8]) -> Result<(), ConvertError> {
        let wanted = out.len();
        let read = self
            .0
            .page_read(zone, at as f64, out)
            .map_err(|error| host_error(&format!("spill zone {zone} read failed"), &error))?;
        if count(read, wanted) != Some(wanted) {
            return Err(ConvertError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("short read from spill zone {zone}: {read} of {wanted} bytes"),
            )));
        }
        Ok(())
    }

    fn release(&self, zone: u32) -> Result<(), ConvertError> {
        self.0
            .zone_release(zone)
            .map_err(|error| host_error(&format!("spill zone {zone} release failed"), &error))
    }
}
