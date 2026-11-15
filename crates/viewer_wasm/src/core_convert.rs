//! Browser converter over the shared `viewer_convert_core` serial path: the same planner and block
//! writer as the native CLI, with a Blob as input and two OPFS sync access handles as output.
//! Spools live in the hybrid store (JS RAM segments plus OPFS spill zones). Everything runs in one synchronous call inside a dedicated worker; cancelling means the host
//! terminates the worker and deletes the partial files.

use std::io;

use viewer_convert_core::{
    ByteSource, ConversionProgress, ConversionStage, ConvertError, ConvertOptions, HybridConfig,
    HybridSpoolStore, HybridStats, LasFactory, NoClock, SpoolEstimate, convert_serial,
    estimate_spools, inspect_source, validate_output_serial,
};
use viewer_format::{FormatVersion, RowOrder};
use wasm_bindgen::{JsCast, prelude::*};
use web_sys::Blob;

use crate::browser_io::BrowserBlobReader;
use crate::opfs::{OpfsReader, OpfsWriter};
use crate::spool_host::{JsPageTier, JsRamTier, SpoolHost};

/// Opens a fresh cached reader per call; `las::Reader` needs `Send + Sync`, which `JsValue` holds
/// on wasm32 without atomics (the same assumption `WasmLasOctConverter` makes).
struct BlobBytes {
    blob: Blob,
}

impl ByteSource for BlobBytes {
    type Reader = BrowserBlobReader;

    fn open(&self) -> Result<Self::Reader, ConvertError> {
        BrowserBlobReader::new(self.blob.clone(), false)
            .map_err(|error| io::Error::other(format!("{error:?}")).into())
    }

    fn len(&self) -> u64 {
        self.blob.size() as u64
    }
}

fn number(options: &JsValue, key: &str) -> Result<Option<f64>, JsValue> {
    if options.is_undefined() || options.is_null() {
        return Ok(None);
    }
    let value = js_sys::Reflect::get(options, &JsValue::from_str(key))?;
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    value
        .as_f64()
        .filter(|value| {
            value.is_finite() && *value >= 0.0 && (key == "maxError" || value.fract() == 0.0)
        })
        .map(Some)
        .ok_or_else(|| JsValue::from_str(&format!("option {key} must be a non-negative number")))
}

/// Builds the options from a JS object; absent keys keep the CLI defaults (R2). Keys:
/// `formatVersion`, `rowOrder` (`source`|`hybrid`; hybrid needs a v7 format; `v7` fixes hybrid and rejects source; `v7.N-pre` default to source),
/// `pointsPerBlock`, `maxError`, `spatialDivisions`, `maxMemoryMiB` and `spatialWorkers`. The
/// format resolves and checks the values it fixes, whatever the key order.
fn parse_options(options: &JsValue) -> Result<ConvertOptions, JsValue> {
    let defaults = ConvertOptions::default();
    let invalid = |error: ConvertError| JsValue::from_str(&error.to_string());
    let points =
        number(options, "pointsPerBlock")?.map_or(defaults.points_per_block(), |v| v as u64);
    let max_error = number(options, "maxError")?.unwrap_or(defaults.max_quantization_error());
    let memory = number(options, "maxMemoryMiB")?
        .map(|mib| (mib as u64).checked_mul(1024 * 1024))
        .map_or(Ok(defaults.max_memory_budget_bytes()), |bytes| {
            bytes.ok_or_else(|| JsValue::from_str("option maxMemoryMiB overflows bytes"))
        })?;
    let mut converted =
        ConvertOptions::with_memory_budget(points, max_error, memory).map_err(invalid)?;
    if let Some(workers) = number(options, "spatialWorkers")? {
        converted = converted
            .with_spatial_workers(workers as usize)
            .map_err(invalid)?;
    }
    if !options.is_undefined() && !options.is_null() {
        let version = js_sys::Reflect::get(options, &"formatVersion".into())?;
        if let Some(version) = version.as_string() {
            let version: FormatVersion = version.parse().map_err(JsValue::from_str)?;
            converted = converted.with_format_version(version).map_err(invalid)?;
        }
        if let Some(divisions) = number(options, "spatialDivisions")? {
            let divisions = u8::try_from(divisions as u64).unwrap_or(u8::MAX);
            converted = converted
                .with_spatial_divisions(divisions)
                .map_err(invalid)?;
        }
        let order = js_sys::Reflect::get(options, &"rowOrder".into())?;
        if !order.is_undefined() && !order.is_null() {
            let order: RowOrder = order
                .as_string()
                .ok_or_else(|| JsValue::from_str("option rowOrder must be a string"))?
                .parse()
                .map_err(|error: String| JsValue::from_str(&error))?;
            converted = converted.with_row_order(order).map_err(invalid)?;
        }
    }
    converted.validate().map_err(invalid)
}

/// Converts a LAS/LAZ Blob into the `.oct` / `.octattrs` pair behind two OPFS sync access handles.
#[wasm_bindgen]
pub struct WasmCoreConverter {
    input: Blob,
    geometry: JsValue,
    attributes: JsValue,
    options: ConvertOptions,
    spool_budget: u64,
    on_progress: Option<js_sys::Function>,
    spool_host: SpoolHost,
}

#[wasm_bindgen]
impl WasmCoreConverter {
    /// `options` is a plain object (see `parse_options`) plus `spoolBudgetBytes`, the RAM the
    /// spools may retain before spilling to the zones of `spool_host`. `on_progress` receives
    /// `{ stage, processedPoints, totalPoints }` and may return `false` to stop the conversion.
    #[wasm_bindgen(constructor)]
    pub fn new(
        input: Blob,
        geometry_handle: JsValue,
        attributes_handle: JsValue,
        options: JsValue,
        on_progress: Option<js_sys::Function>,
        spool_host: SpoolHost,
    ) -> Result<WasmCoreConverter, JsValue> {
        Ok(Self {
            options: parse_options(&options)?,
            spool_budget: spool_budget(&options)?,
            spool_host,
            input,
            geometry: geometry_handle,
            attributes: attributes_handle,
            on_progress,
        })
    }

    /// Reads the LAS/LAZ header of `input` and returns the spool estimate for `options` as
    /// `{ sourceSpoolBytes, livePeakBytes, nodes, spilledPeakBytes, zones, outputBytes,
    /// ramOnlyBytes }`, so the host can open `zones` spill zones before converting.
    pub fn estimate(input: Blob, options: JsValue) -> Result<JsValue, JsValue> {
        let parsed = parse_options(&options)?;
        let budget = spool_budget(&options)?;
        let factory = LasFactory::new(BlobBytes { blob: input });
        let source = inspect_source(&factory)
            .map_err(|error| conversion_error("reading the source header failed", &error))?;
        let estimate = estimate_spools(&source, parsed, HybridConfig::web(budget))
            .map_err(|error| conversion_error("spool estimate failed", &error))?;
        estimate_object(estimate)
    }

    /// Converts, flushes and validates by reading the handles back. Returns
    /// `{ pointCount, blockCount, spool }` (point count as a string: it may exceed 2^53; `spool`
    /// holds the `HybridStats` counters). A full output or spill zone throws an `Error` named
    /// `QuotaExceededError`.
    pub fn convert(&mut self) -> Result<JsValue, JsValue> {
        let callback = self.on_progress.clone();
        let mut report = move |progress: ConversionProgress| -> bool {
            let Some(callback) = &callback else {
                return true;
            };
            let event = js_sys::Object::new();
            let fields = [
                ("stage", JsValue::from_str(stage_name(progress.stage))),
                (
                    "processedPoints",
                    JsValue::from_f64(progress.points_processed as f64),
                ),
                (
                    "totalPoints",
                    JsValue::from_f64(progress.total_points as f64),
                ),
            ];
            for (key, value) in fields {
                let _ = js_sys::Reflect::set(&event, &JsValue::from_str(key), &value);
            }
            callback
                .call1(&JsValue::NULL, &event)
                .map_or(true, |keep| keep != JsValue::FALSE)
        };
        let factory = LasFactory::new(BlobBytes {
            blob: self.input.clone(),
        });
        let store = HybridSpoolStore::new(
            JsRamTier(self.spool_host.clone().unchecked_into()),
            JsPageTier(self.spool_host.clone().unchecked_into()),
            HybridConfig::web(self.spool_budget),
            false,
        )
        .map_err(|error| conversion_error("spool setup failed", &error))?;
        let output = convert_serial(
            &factory,
            &store,
            &NoClock,
            self.options,
            OpfsWriter::new(self.geometry.clone())?,
            OpfsWriter::new(self.attributes.clone())?,
            &mut report,
        )
        .map_err(|error| conversion_error("conversion failed", &error))?;
        let (mut geometry, mut attributes) = (output.geometry, output.attributes);
        std::io::Write::flush(&mut geometry)
            .and_then(|()| std::io::Write::flush(&mut attributes))
            .map_err(|error| conversion_error("flushing output", &ConvertError::Io(error)))?;
        validate_output_serial(
            OpfsReader::new(self.geometry.clone()),
            OpfsReader::new(self.attributes.clone()),
            &output.plan,
            &mut report,
        )
        .map_err(|error| conversion_error("output validation failed", &error))?;
        let result = js_sys::Object::new();
        js_sys::Reflect::set(
            &result,
            &"pointCount".into(),
            &JsValue::from_str(&output.plan.point_count.to_string()),
        )?;
        js_sys::Reflect::set(
            &result,
            &"blockCount".into(),
            &JsValue::from_f64(output.plan.block_count as f64),
        )?;
        js_sys::Reflect::set(&result, &"spool".into(), &stats_object(store.stats())?)?;
        Ok(result.into())
    }
}

/// Spool RAM retained before spilling when the host gives no `spoolBudgetBytes`.
const DEFAULT_SPOOL_BUDGET_BYTES: u64 = 2 << 30;

fn spool_budget(options: &JsValue) -> Result<u64, JsValue> {
    Ok(number(options, "spoolBudgetBytes")?
        .map_or(DEFAULT_SPOOL_BUDGET_BYTES, |bytes| bytes as u64))
}

fn estimate_object(estimate: SpoolEstimate) -> Result<JsValue, JsValue> {
    let object = js_sys::Object::new();
    let fields = [
        ("sourceSpoolBytes", estimate.source_spool_bytes as f64),
        ("livePeakBytes", estimate.live_peak_bytes as f64),
        ("nodes", estimate.nodes as f64),
        ("spilledPeakBytes", estimate.spilled_peak_bytes as f64),
        ("zones", f64::from(estimate.zones)),
        ("outputBytes", estimate.output_bytes as f64),
        ("ramOnlyBytes", estimate.ram_only_bytes as f64),
    ];
    for (key, value) in fields {
        js_sys::Reflect::set(&object, &key.into(), &JsValue::from_f64(value))?;
    }
    Ok(object.into())
}

/// Out-of-space errors are thrown as `QuotaExceededError`, like the browser's own, so the host can
/// tell a full disk from a bug. The `io::Error` may sit under a format or spool error.
fn conversion_error(context: &str, error: &ConvertError) -> JsValue {
    let message = format!("{context}: {error}");
    let is_full = |io: &io::Error| io.kind() == io::ErrorKind::StorageFull;
    // `transparent` variants forward `source()`, so `Io` is matched before walking the chain.
    let mut full = matches!(error, ConvertError::Io(io) if is_full(io));
    let mut cause = std::error::Error::source(error);
    while let Some(current) = cause {
        full |= current.downcast_ref::<io::Error>().is_some_and(is_full);
        cause = current.source();
    }
    if full {
        let thrown = js_sys::Error::new(&message);
        thrown.set_name("QuotaExceededError");
        return thrown.into();
    }
    JsValue::from_str(&message)
}

fn stats_object(stats: HybridStats) -> Result<JsValue, JsValue> {
    let object = js_sys::Object::new();
    let fields = [
        ("livePayloadBytes", stats.live_payload_bytes as f64),
        ("ramRetainedBytes", stats.ram_retained_bytes as f64),
        ("ramPeakBytes", stats.ram_peak_bytes as f64),
        ("overBudgetPeakBytes", stats.over_budget_peak_bytes as f64),
        ("spilledBytes", stats.spilled_bytes as f64),
        ("spilledSpools", stats.spilled_spools as f64),
        ("pageReadBytes", stats.page_read_bytes as f64),
        ("pagesInUse", f64::from(stats.pages_in_use)),
        ("pagesPeak", f64::from(stats.pages_peak)),
        ("zonesReleased", f64::from(stats.zones_released)),
    ];
    for (key, value) in fields {
        js_sys::Reflect::set(&object, &key.into(), &JsValue::from_f64(value))?;
    }
    Ok(object.into())
}

fn stage_name(stage: ConversionStage) -> &'static str {
    match stage {
        ConversionStage::InspectingInput => "inspecting",
        ConversionStage::PlanningMetadata => "planning-metadata",
        ConversionStage::PlanningSpatialIndex => "planning",
        ConversionStage::BuildingSpatialHierarchy => "hierarchy",
        ConversionStage::ReadingAndWritingBlocks => "writing",
        ConversionStage::ValidatingOutput => "validating",
        ConversionStage::PublishingOutput => "publishing",
        ConversionStage::Complete => "complete",
    }
}
