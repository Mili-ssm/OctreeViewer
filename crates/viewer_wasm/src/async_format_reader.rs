//! Async range access to split `.oct` files using browser Blob promises.
use std::{cell::Cell, io::Cursor, rc::Rc, sync::Arc};

use js_sys::{ArrayBuffer, Float32Array, Promise, Uint8Array};
use serde::Serialize;
use viewer_core::{AttributeValues, BlockId, DatasetSource, NodeId, PointBatch};
use viewer_format::{
    FormatReader, FormatReaderLimits, PageGraft, PagedCore, PayloadReadRange, ReadBudget,
    ReadReservation, ReadSelection, ReadSource, SPLIT_FOOTER_BYTES, SPLIT_HEADER_BYTES,
    SelectedRead, TAIL_READ_BYTES,
};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::JsFuture;
use web_sys::Blob;

use crate::{
    lod_dataset::WorkClock,
    lod_metadata::{crs_unit_js, dataset_unit},
};

type Reader = FormatReader<Cursor<Vec<u8>>>;
const DEFAULT_ASYNC_READ_BUDGET_BYTES: u32 = 256 * 1024 * 1024;

struct AsyncReaderShared {
    reader: Reader,
    primary: Blob,
    attributes: Option<Blob>,
    // The index reservation remains live while this shared reader or a pending read uses it.
    _metadata_reservation: ReadReservation,
}

#[derive(Serialize)]
struct RangeDescriptor {
    source: &'static str,
    offset: f64,
    length: f64,
}

#[derive(Serialize)]
struct ReadPlanDescriptor {
    ranges: Vec<RangeDescriptor>,
    stored_bytes: String,
    decoded_bytes: String,
    output_bytes: String,
    peak_bytes: String,
}

/// JS result shape: `{kind, values, pointCount, components?, name?, dataType?}`.
/// Positions are `Float32Array` xyz; geometry is `Float32Array` xyz + RGB bytes; colors are RGB bytes.
/// Attributes preserve their declared integer/float type in a JS typed array.
#[wasm_bindgen]
pub struct WasmAsyncFormatReader {
    shared: Arc<AsyncReaderShared>,
    budget: ReadBudget,
    plan: Option<viewer_format::ReadPlan>,
}

/// An independently owned LOD block request. Separate requests share the same immutable index
/// and read budget, so their Blob promises can overlap without borrowing the facade mutably.
#[wasm_bindgen]
pub struct WasmAsyncLodReadRequest {
    shared: Arc<AsyncReaderShared>,
    plans: Vec<viewer_format::ReadPlan>,
    /// Trailing plans whose columns travel with the block unpainted (`geometryAttributes`).
    carried: usize,
    reservation: Option<ReadReservation>,
    cancelled: Rc<Cell<bool>>,
    /// Per-request Blob-likes (a signal-scoped view of the range source) replacing the shared ones.
    sources: SourceOverride,
}

#[derive(Default)]
struct SourceOverride {
    primary: Option<Blob>,
    attributes: Option<Blob>,
}

/// Cancellation is logical: an active Blob promise resolves before its reservation is released.
#[wasm_bindgen]
pub struct WasmAsyncReadCancelToken {
    cancelled: Rc<Cell<bool>>,
}

/// Owns typed arrays and the request's quota until the worker inserts or discards the result.
#[wasm_bindgen]
pub struct WasmAsyncLodReadResult {
    value: Option<JsValue>,
    reservation: Option<ReadReservation>,
    read_ms: f64,
    decode_ms: f64,
}

/// A validated hierarchy page in flight between the reader and the dataset that grafts it.
#[wasm_bindgen]
pub struct WasmPageGraft {
    graft: Option<PageGraft>,
}

impl WasmPageGraft {
    pub(crate) fn take(&mut self) -> Result<PageGraft, JsValue> {
        self.graft
            .take()
            .ok_or_else(|| err("page graft was already used"))
    }
}

#[wasm_bindgen]
pub struct WasmAsyncReadBudget {
    inner: ReadBudget,
}

#[wasm_bindgen]
impl WasmAsyncReadBudget {
    #[wasm_bindgen(constructor)]
    pub fn new(limit_bytes: u32) -> WasmAsyncReadBudget {
        Self {
            inner: ReadBudget::new(u64::from(limit_bytes)),
        }
    }

    #[wasm_bindgen(js_name = clone)]
    #[must_use]
    pub fn clone_budget(&self) -> WasmAsyncReadBudget {
        Self {
            inner: self.inner.clone(),
        }
    }

    #[wasm_bindgen(js_name = reservedBytes)]
    pub fn reserved_bytes(&self) -> String {
        self.inner.reserved_bytes().to_string()
    }

    #[wasm_bindgen(js_name = limitBytes)]
    pub fn limit_bytes(&self) -> String {
        self.inner.limit_bytes().to_string()
    }
}

#[wasm_bindgen]
impl WasmAsyncReadCancelToken {
    pub fn cancel(&self) {
        self.cancelled.set(true);
    }
}

#[wasm_bindgen]
impl WasmAsyncLodReadResult {
    #[wasm_bindgen(getter)]
    pub fn value(&self) -> JsValue {
        self.value.clone().unwrap_or(JsValue::NULL)
    }

    /// Wall time awaiting Blob range reads (includes event-loop waits), for benchmarks.
    #[wasm_bindgen(getter, js_name = readMs)]
    pub fn read_ms(&self) -> f64 {
        self.read_ms
    }

    /// Time spent decoding payloads and building typed arrays, for benchmarks.
    #[wasm_bindgen(getter, js_name = decodeMs)]
    pub fn decode_ms(&self) -> f64 {
        self.decode_ms
    }

    /// Releases the typed arrays and shared quota after insertion or logical discard.
    pub fn release(&mut self) {
        self.value = None;
        self.reservation = None;
    }
}

#[wasm_bindgen]
impl WasmAsyncLodReadRequest {
    #[wasm_bindgen(js_name = cancelToken)]
    pub fn cancel_token(&self) -> WasmAsyncReadCancelToken {
        WasmAsyncReadCancelToken {
            cancelled: Rc::clone(&self.cancelled),
        }
    }

    /// Replaces the shared Blobs for this request only (the default stays when not called). The
    /// replacements must expose the same bytes; they let a source scope its reads to this request.
    #[wasm_bindgen(js_name = setSource)]
    pub fn set_source(&mut self, primary: Blob, attributes: Option<Blob>) {
        self.sources = SourceOverride {
            primary: Some(primary),
            attributes,
        };
    }

    #[wasm_bindgen(js_name = read)]
    pub async fn read(&mut self) -> Result<WasmAsyncLodReadResult, JsValue> {
        let mut plans = std::mem::take(&mut self.plans).into_iter();
        let first = plans
            .next()
            .ok_or_else(|| err("LOD read request is empty"))?;
        let first_selection = first.selection();
        let (first_result, mut read_ms, mut decode_ms) =
            read_plan(&self.shared, first, &self.cancelled, &self.sources).await?;
        let mut rest = Vec::with_capacity(plans.len());
        for plan in plans {
            if self.cancelled.get() {
                return Err(cancelled_error());
            }
            let (result, read, decode) =
                read_plan(&self.shared, plan, &self.cancelled, &self.sources).await?;
            read_ms += read;
            decode_ms += decode;
            rest.push(result);
        }
        let carried = rest.split_off(rest.len().saturating_sub(self.carried));
        let clock = WorkClock::start();
        let value = lod_result_to_js(first_selection, first_result, rest.pop(), carried)?;
        decode_ms += clock.elapsed_ms();
        Ok(WasmAsyncLodReadResult {
            value: Some(value),
            reservation: self.reservation.take(),
            read_ms,
            decode_ms,
        })
    }
}

#[wasm_bindgen]
impl WasmAsyncFormatReader {
    /// Opens split primary and optional attribute Blobs. Reads header/footer/index by ranges only.
    #[wasm_bindgen(js_name = open)]
    pub async fn open(
        primary: Blob,
        attributes: Option<Blob>,
        budget: Option<WasmAsyncReadBudget>,
    ) -> Result<WasmAsyncFormatReader, JsValue> {
        let budget = budget.map_or_else(
            || ReadBudget::new(u64::from(DEFAULT_ASYNC_READ_BUDGET_BYTES)),
            |budget| budget.inner,
        );
        let primary_len = blob_length(&primary)?;
        // One tail read serves both layouts: its last 40 bytes are the footer, and a paged file's
        // metadata block and root page sit right before it.
        let tail_len = primary_len.min(TAIL_READ_BYTES);
        if tail_len < SPLIT_FOOTER_BYTES {
            return Err(err("primary Blob is shorter than footer"));
        }
        let tail = read_range(&primary, primary_len - tail_len, tail_len).await?;
        let footer = tail[tail.len() - SPLIT_FOOTER_BYTES as usize..].to_vec();
        let (reader, metadata_reservation) = if PagedCore::is_paged_footer(&footer) {
            let limits = FormatReaderLimits::default();
            let location = PagedCore::locate_index(primary_len, &footer, &limits)
                .map_err(|error| open_error(error, None, &footer))?;
            let metadata_reservation = budget
                .try_reserve(location.length)
                .map_err(format_error)?
                .ok_or_else(|| {
                err("shared async read budget is unavailable for the resident format index")
            })?;
            let tail_start = primary_len - tail_len;
            let index = if location.offset >= tail_start {
                let start = (location.offset - tail_start) as usize;
                tail[start..tail.len() - SPLIT_FOOTER_BYTES as usize].to_vec()
            } else {
                read_range(&primary, location.offset, location.length).await?
            };
            let reader = Reader::paged_external(primary_len, &footer, &index, limits)
                .map_err(|error| open_error(error, None, &footer))?;
            (reader, metadata_reservation)
        } else {
            drop(tail);
            let header = read_range(&primary, 0, SPLIT_HEADER_BYTES).await?;
            let (offset, length) = Reader::split_index_range(
                primary_len,
                &header,
                &footer,
                FormatReaderLimits::default(),
            )
            .map_err(|error| open_error(error, Some(&header), &footer))?;
            let metadata_reservation = budget
                .try_reserve(length)
                .map_err(format_error)?
                .ok_or_else(|| {
                    err("shared async read budget is unavailable for the resident format index")
                })?;
            let index = read_range(&primary, offset, length).await?;
            let reader = Reader::from_split_buffers(
                primary_len,
                &header,
                &footer,
                &index,
                FormatReaderLimits::default(),
            )
            .map_err(|error| open_error(error, Some(&header), &footer))?;
            (reader, metadata_reservation)
        };
        let mut reader = reader;
        if let Some(blob) = attributes.as_ref() {
            let n = blob_length(blob)?;
            if n < SPLIT_HEADER_BYTES + SPLIT_FOOTER_BYTES {
                return Err(err("attribute Blob is shorter than header and footer"));
            }
            let ah = read_range(blob, 0, SPLIT_HEADER_BYTES).await?;
            let af = read_range(blob, n - SPLIT_FOOTER_BYTES, SPLIT_FOOTER_BYTES).await?;
            reader
                .attach_attribute_identity(n, &ah, &af)
                .map_err(format_error)?;
        }
        Ok(Self {
            shared: Arc::new(AsyncReaderShared {
                reader,
                primary,
                attributes,
                _metadata_reservation: metadata_reservation,
            }),
            budget,
            plan: None,
        })
    }

    /// Returns metadata in the descriptor shape accepted by `WasmVoxelViewer.openLodDataset`.
    #[wasm_bindgen(js_name = lodDatasetDescriptor)]
    pub fn lod_dataset_descriptor(&self, cache_bytes: u32) -> Result<JsValue, JsValue> {
        let metadata = self.shared.reader.format_metadata();
        // World-space bounds and stored spacing, as the viewer consumes them.
        let dataset = metadata.viewer_dataset().map_err(format_error)?;
        let hierarchy = dataset.hierarchy();
        let descriptor = js_sys::Object::new();
        set(
            &descriptor,
            "coordinateOrigin",
            &number_array(&dataset.coordinate_frame().origin()),
        )?;
        set(
            &descriptor,
            "semantics",
            &JsValue::from_str(&format!("{:?}", hierarchy.semantics()).to_lowercase()),
        )?;
        if metadata.global_frame().is_some()
            && let Some(crs) = metadata.crs()
        {
            set(&descriptor, "crs", &JsValue::from_str(crs))?;
        }
        if let Some(frame) = metadata.global_frame() {
            let unit = dataset_unit(&frame.crs().linear_unit());
            set(&descriptor, "crsUnit", &crs_unit_js(&unit)?)?;
        }
        set(
            &descriptor,
            "cacheBytes",
            &JsValue::from_f64(f64::from(cache_bytes)),
        )?;
        set(
            &descriptor,
            "visualScale",
            &JsValue::from_f64(metadata.visual_scale()),
        )?;
        set(
            &descriptor,
            "hasAttributes",
            &JsValue::from_bool(self.shared.reader.has_attributes()),
        )?;
        if let Some(point_count) = hierarchy.stats().source_point_count {
            set(
                &descriptor,
                "sourcePointCount",
                &JsValue::from_str(&point_count.to_string()),
            )?;
        }

        // Extent of the data (tight in `v7.4-pre`; node bounds stay lattice cubes).
        let data_bounds = hierarchy.stats().data_bounds;
        set(&descriptor, "dataMin", &number_array(&data_bounds.min()))?;
        set(&descriptor, "dataMax", &number_array(&data_bounds.max()))?;

        let paged = self.shared.reader.page_source();
        // `nodeCount` is the declared total for a paged file (its hierarchy is not resident) and
        // `maxLevel` the deepest resident node; paged files carry no per-node descriptor.
        let node_count = paged.map_or_else(
            || hierarchy.stats().node_count as f64,
            |source| source.declared_nodes() as f64,
        );
        set(&descriptor, "paged", &JsValue::from_bool(paged.is_some()))?;
        set(&descriptor, "nodeCount", &JsValue::from_f64(node_count))?;
        set(
            &descriptor,
            "maxLevel",
            &JsValue::from_f64(f64::from(hierarchy.stats().max_level)),
        )?;
        let nodes = js_sys::Array::new();
        let mut pending = if paged.is_some() {
            Vec::new()
        } else {
            vec![hierarchy.root_id()]
        };
        while let Some(node_id) = pending.pop() {
            let node = hierarchy
                .node(node_id)
                .ok_or_else(|| err("hierarchy references a missing node"))?;
            let value = js_sys::Object::new();
            set(
                &value,
                "id",
                &JsValue::from_str(&node.id().get().to_string()),
            )?;
            let parent = node
                .parent()
                .map_or(JsValue::NULL, |id| JsValue::from_str(&id.get().to_string()));
            set(&value, "parent", &parent)?;
            set(&value, "level", &JsValue::from_f64(f64::from(node.level())))?;
            set(&value, "min", &number_array(&node.bounds().min()))?;
            set(&value, "max", &number_array(&node.bounds().max()))?;
            set(
                &value,
                "pointCount",
                &JsValue::from_str(&node.point_count().to_string()),
            )?;
            set(
                &value,
                "geometricError",
                &JsValue::from_f64(node.geometric_error()),
            )?;
            let blocks = js_sys::Array::new();
            for block_id in node.blocks() {
                blocks.push(&JsValue::from_str(&block_id.get().to_string()));
            }
            set(&value, "blocks", &blocks.into())?;
            if let Some(spacing) = node.spacing() {
                set(&value, "spacing", &JsValue::from_f64(spacing))?;
            }
            nodes.push(&value);
            pending.extend(hierarchy.child_ids(node.id()).iter().rev().copied());
        }
        set(&descriptor, "nodes", &nodes.into())?;

        let schema = js_sys::Array::new();
        let attribute_ranges = js_sys::Array::new();
        for (schema_index, attribute) in hierarchy.attributes().iter().enumerate() {
            let value = js_sys::Object::new();
            set(&value, "name", &JsValue::from_str(attribute.name()))?;
            set(
                &value,
                "dataType",
                &JsValue::from_str(&format!("{:?}", attribute.data_type()).to_lowercase()),
            )?;
            set(
                &value,
                "componentCount",
                &JsValue::from_f64(f64::from(attribute.component_count())),
            )?;
            schema.push(&value);

            let ranges = js_sys::Array::new();
            for component in 0..attribute.component_count() {
                let range = self
                    .shared
                    .reader
                    .attribute_range(schema_index as u32, component)
                    .map_err(format_error)?
                    .map_or(JsValue::NULL, |[min, max]| number_array(&[min, max]));
                ranges.push(&range);
            }
            attribute_ranges.push(&ranges);
        }
        set(&descriptor, "attributeSchema", &schema.into())?;
        set(&descriptor, "attributeRanges", &attribute_ranges.into())?;
        Ok(descriptor.into())
    }

    /// Fetches, validates and registers the hierarchy page hanging from `anchor` (a decimal node
    /// id). The result is grafted with `graftPage` on the viewer or builder that owns the
    /// hierarchy. Page reads have no `ReadBudget`: the dataset's page budget bounds them.
    #[wasm_bindgen(js_name = loadPage)]
    pub async fn load_page(&self, anchor: String) -> Result<WasmPageGraft, JsValue> {
        let id = NodeId::new(
            anchor
                .parse()
                .map_err(|_| err("anchor must be an unsigned 64-bit integer string"))?,
        );
        let source = self
            .shared
            .reader
            .page_source()
            .ok_or_else(|| err("the format is not paged"))?
            .clone();
        let range = source.page_range(id).map_err(format_error)?;
        let bytes = read_range(&self.shared.primary, range.offset, range.length).await?;
        let graft = source.accept_page(id, &bytes).map_err(format_error)?;
        Ok(WasmPageGraft { graft: Some(graft) })
    }

    /// Plans a block result with one shared reservation, ready for independent async execution.
    /// `positionsAttribute` returns XYZ and one typed column as a single quota-owned result.
    /// `geometryAttributes` (the cells path) returns XYZ and RGB, the optional `schemaIndex`
    /// column as `attribute` and every `carriedIndices` column in `attributes`.
    #[wasm_bindgen(js_name = planLodRead)]
    pub fn plan_lod_read(
        &self,
        block_id: u64,
        selection: &str,
        schema_index: Option<u32>,
        carried_indices: Option<Vec<u32>>,
    ) -> Result<Option<WasmAsyncLodReadRequest>, JsValue> {
        let id = BlockId::new(block_id);
        let mut plans = Vec::with_capacity(2);
        let mut carried = 0;
        match selection {
            "geometry" => plans.push(
                self.shared
                    .reader
                    .plan_read(id, ReadSelection::Geometry)
                    .map_err(format_error)?,
            ),
            "positions" => plans.push(
                self.shared
                    .reader
                    .plan_read(id, ReadSelection::Positions)
                    .map_err(format_error)?,
            ),
            "positionsAttribute" => {
                let schema_index = schema_index.ok_or_else(|| err("schemaIndex required"))?;
                plans.push(
                    self.shared
                        .reader
                        .plan_read(id, ReadSelection::Positions)
                        .map_err(format_error)?,
                );
                if let Some(schema_index) = self.painted_column(id, schema_index)? {
                    plans.push(
                        self.shared
                            .reader
                            .plan_read(id, ReadSelection::Attribute { schema_index })
                            .map_err(format_error)?,
                    );
                }
            }
            "geometryAttributes" => {
                let plan = |selection| {
                    self.shared
                        .reader
                        .plan_read(id, selection)
                        .map_err(format_error)
                };
                plans.push(plan(ReadSelection::Geometry)?);
                let painted = match schema_index {
                    Some(schema_index) => self.painted_column(id, schema_index)?,
                    None => None,
                };
                if let Some(schema_index) = painted {
                    plans.push(plan(ReadSelection::Attribute { schema_index })?);
                }
                // The painted column already travels with the block: a substitute that is also
                // a carried column (intensity) is read once.
                for schema_index in carried_indices
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|index| Some(*index) != painted)
                {
                    // A carried column a LOD block omits is left out, as the synchronous loader does.
                    if !self
                        .shared
                        .reader
                        .attribute_present(id, schema_index)
                        .map_err(format_error)?
                    {
                        continue;
                    }
                    plans.push(plan(ReadSelection::Attribute { schema_index })?);
                    carried += 1;
                }
            }
            _ => {
                return Err(err(
                    "selection must be geometry, positions, positionsAttribute, or geometryAttributes",
                ));
            }
        }
        let required = lod_request_peak_bytes(&plans)?;
        let Some(reservation) = self.budget.try_reserve(required).map_err(format_error)? else {
            return Ok(None);
        };
        Ok(Some(WasmAsyncLodReadRequest {
            shared: Arc::clone(&self.shared),
            plans,
            carried,
            reservation: Some(reservation),
            cancelled: Rc::new(Cell::new(false)),
            sources: SourceOverride::default(),
        }))
    }

    /// Plans one supported selection and returns the exact ranges to fetch. The plan remains
    /// private to this reader and must be consumed by `materialize` before another plan is made.
    #[wasm_bindgen(js_name = planRead)]
    pub fn plan_read(
        &mut self,
        block_id: u64,
        selection: &str,
        schema_index: Option<u32>,
    ) -> Result<JsValue, JsValue> {
        if self.plan.is_some() {
            return Err(err(
                "consume the pending plan with read before planning another",
            ));
        }
        let id = BlockId::new(block_id);
        let selected = match selection {
            "positions" => ReadSelection::Positions,
            "geometry" => ReadSelection::Geometry,
            "displayColors" => ReadSelection::DisplayColors,
            "attribute" => ReadSelection::Attribute {
                schema_index: schema_index.ok_or_else(|| err("schemaIndex required"))?,
            },
            _ => {
                return Err(err(
                    "selection must be positions, geometry, displayColors, or attribute",
                ));
            }
        };
        let plan = self
            .shared
            .reader
            .plan_read(id, selected)
            .map_err(format_error)?;
        let ranges = plan
            .ranges()
            .iter()
            .map(range_descriptor)
            .collect::<Result<Vec<_>, _>>()?;
        let memory = plan.memory();
        let descriptor = ReadPlanDescriptor {
            ranges,
            stored_bytes: memory.stored_bytes().to_string(),
            decoded_bytes: memory.decoded_bytes().to_string(),
            output_bytes: memory.output_bytes().to_string(),
            peak_bytes: memory.peak_bytes().to_string(),
        };
        let value = serde_wasm_bindgen::to_value(&descriptor).map_err(|e| err(&e.to_string()))?;
        self.plan = Some(plan);
        Ok(value)
    }

    /// Fetches the ranges from the plan and materializes its validated selection.
    #[wasm_bindgen(js_name = read)]
    pub async fn read(&mut self) -> Result<JsValue, JsValue> {
        let plan = self
            .plan
            .take()
            .ok_or_else(|| err("call planRead before read"))?;
        let mut buffers = Vec::with_capacity(plan.ranges().len());
        let not_cancelled = Cell::new(false);
        for range in plan.ranges() {
            buffers.push(
                read_range_for(
                    &self.shared,
                    range,
                    &not_cancelled,
                    &SourceOverride::default(),
                )
                .await?,
            );
        }
        let selected = self
            .shared
            .reader
            .materialize_selected_from_buffers(plan, buffers)
            .map_err(format_error)?;
        selected_to_js(selected)
    }
}

fn lod_request_peak_bytes(plans: &[viewer_format::ReadPlan]) -> Result<u64, JsValue> {
    let mut result_bytes = 0_u64;
    let mut peak_bytes = 0_u64;
    for plan in plans {
        let memory = plan.memory();
        // Blob.arrayBuffer and Vec<u8> coexist; materialized Rust output and the JS typed-array
        // copy coexist as well. Reserve conservatively until the viewer accepts the result.
        let plan_peak = memory
            .peak_bytes()
            .checked_add(memory.stored_bytes())
            .and_then(|bytes| bytes.checked_add(memory.output_bytes()))
            .ok_or_else(|| err("async LOD read estimate overflow"))?;
        let held_peak = result_bytes
            .checked_add(plan_peak)
            .ok_or_else(|| err("async LOD read estimate overflow"))?;
        peak_bytes = peak_bytes.max(held_peak);
        result_bytes = result_bytes
            .checked_add(memory.output_bytes())
            .ok_or_else(|| err("async LOD result estimate overflow"))?;
    }
    // The viewer copies JS outputs into Point/Attribute vectors before retaining its cache block.
    let insertion_peak = result_bytes
        .checked_mul(3)
        .ok_or_else(|| err("async LOD insertion estimate overflow"))?;
    Ok(peak_bytes.max(insertion_peak))
}

async fn read_plan(
    shared: &AsyncReaderShared,
    plan: viewer_format::ReadPlan,
    cancelled: &Cell<bool>,
    sources: &SourceOverride,
) -> Result<(SelectedRead, f64, f64), JsValue> {
    let mut buffers = Vec::with_capacity(plan.ranges().len());
    let read_clock = WorkClock::start();
    for range in plan.ranges() {
        if cancelled.get() {
            return Err(cancelled_error());
        }
        buffers.push(read_range_for(shared, range, cancelled, sources).await?);
    }
    if cancelled.get() {
        return Err(cancelled_error());
    }
    let read_ms = read_clock.elapsed_ms();
    let decode_clock = WorkClock::start();
    let selected = shared
        .reader
        .materialize_selected_from_buffers(plan, buffers)
        .map_err(format_error)?;
    Ok((selected, read_ms, decode_clock.elapsed_ms()))
}

async fn read_range_for(
    shared: &AsyncReaderShared,
    range: &PayloadReadRange,
    cancelled: &Cell<bool>,
    sources: &SourceOverride,
) -> Result<Vec<u8>, JsValue> {
    if cancelled.get() {
        return Err(cancelled_error());
    }
    let blob = match range.source() {
        ReadSource::Primary => sources.primary.as_ref().unwrap_or(&shared.primary),
        ReadSource::Attributes => sources
            .attributes
            .as_ref()
            .or(shared.attributes.as_ref())
            .ok_or_else(|| err("attribute Blob required by this selection"))?,
    };
    let end = range
        .offset()
        .checked_add(range.stored_bytes())
        .ok_or_else(|| err("payload range overflow"))?;
    if end > blob_length(blob)? {
        return Err(err("payload range exceeds Blob length"));
    }
    let slice = blob.slice_with_f64_and_f64(range.offset() as f64, end as f64)?;
    let promise: Promise = slice.array_buffer();
    let value = JsFuture::from(promise).await?;
    if cancelled.get() {
        return Err(cancelled_error());
    }
    let buffer: ArrayBuffer = value
        .dyn_into()
        .map_err(|_| err("Blob.arrayBuffer returned a non-ArrayBuffer"))?;
    let array = Uint8Array::new(&buffer);
    if u64::from(array.length()) != range.stored_bytes() {
        return Err(err("Blob returned an unexpected range length"));
    }
    Ok(array.to_vec())
}

fn lod_result_to_js(
    first_selection: ReadSelection,
    first: SelectedRead,
    second: Option<SelectedRead>,
    carried: Vec<SelectedRead>,
) -> Result<JsValue, JsValue> {
    let first = selected_to_js(first)?;
    let object = js_sys::Object::new();
    let first_values = js_sys::Reflect::get(&first, &JsValue::from_str("values"))?;
    set(&object, "positions", &first_values)?;
    if first_selection == ReadSelection::Geometry {
        let colors = js_sys::Reflect::get(&first, &JsValue::from_str("colors"))?;
        set(&object, "colors", &colors)?;
    }
    if let Some(second) = second {
        let second = selected_to_js(second)?;
        if js_sys::Reflect::get(&second, &JsValue::from_str("kind"))?
            .as_string()
            .as_deref()
            != Some("attribute")
        {
            return Err(err("second LOD read result must be an attribute"));
        }
        set(&object, "attribute", &second)?;
    }
    if !carried.is_empty() {
        let columns = js_sys::Array::new();
        for column in carried {
            columns.push(&selected_to_js(column)?);
        }
        set(&object, "attributes", &columns.into())?;
    }
    let point_count = js_sys::Reflect::get(&first, &JsValue::from_str("pointCount"))?;
    set(&object, "pointCount", &point_count)?;
    Ok(object.into())
}

fn range_descriptor(range: &PayloadReadRange) -> Result<RangeDescriptor, JsValue> {
    Ok(RangeDescriptor {
        source: match range.source() {
            ReadSource::Primary => "primary",
            ReadSource::Attributes => "attributes",
        },
        offset: exact_js_number(range.offset())?,
        length: exact_js_number(range.stored_bytes())?,
    })
}
fn exact_js_number(n: u64) -> Result<f64, JsValue> {
    if n > 9_007_199_254_740_991 {
        return Err(err(
            "offset or length exceeds exact JavaScript integer range",
        ));
    }
    Ok(n as f64)
}
fn blob_length(blob: &Blob) -> Result<u64, JsValue> {
    let n = blob.size();
    if !n.is_finite() || n < 0.0 || n.fract() != 0.0 || n > 9_007_199_254_740_991.0 {
        return Err(err("Blob length is outside exact JavaScript integer range"));
    }
    Ok(n as u64)
}
async fn read_range(blob: &Blob, offset: u64, length: u64) -> Result<Vec<u8>, JsValue> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| err("Blob range overflow"))?;
    if end > blob_length(blob)? {
        return Err(err("Blob range exceeds Blob length"));
    }
    let slice = blob.slice_with_f64_and_f64(offset as f64, end as f64)?;
    let promise: Promise = slice.array_buffer();
    let value = JsFuture::from(promise).await?;
    let buffer: ArrayBuffer = value
        .dyn_into()
        .map_err(|_| err("Blob.arrayBuffer returned a non-ArrayBuffer"))?;
    let array = Uint8Array::new(&buffer);
    if u64::from(array.length()) != length {
        return Err(err("Blob returned an unexpected range length"));
    }
    Ok(array.to_vec())
}
fn selected_to_js(selected: SelectedRead) -> Result<JsValue, JsValue> {
    let object = js_sys::Object::new();
    match selected {
        SelectedRead::Positions(points) => {
            set(&object, "kind", &JsValue::from_str("positions"))?;
            set(
                &object,
                "pointCount",
                &JsValue::from_f64(points.len() as f64),
            )?;
            set(&object, "values", &positions(&points).into())?;
        }
        SelectedRead::Geometry(points) => {
            set(&object, "kind", &JsValue::from_str("geometry"))?;
            set(
                &object,
                "pointCount",
                &JsValue::from_f64(points.len() as f64),
            )?;
            let values = positions(&points);
            set(&object, "values", &values.into())?;
            let colors = Uint8Array::new_with_length(
                u32::try_from(points.len() * 3)
                    .map_err(|_| err("result too large for JS typed array"))?,
            );
            for (i, p) in points.points().iter().enumerate() {
                colors.set_index((i * 3) as u32, p.color.r);
                colors.set_index((i * 3 + 1) as u32, p.color.g);
                colors.set_index((i * 3 + 2) as u32, p.color.b);
            }
            set(&object, "colors", &colors.into())?;
        }
        SelectedRead::DisplayColors(colors) => {
            set(&object, "kind", &JsValue::from_str("displayColors"))?;
            let bytes = Uint8Array::new_with_length(
                u32::try_from(colors.len() * 3)
                    .map_err(|_| err("result too large for JS typed array"))?,
            );
            for (i, c) in colors.iter().enumerate() {
                bytes.set_index((i * 3) as u32, c.r);
                bytes.set_index((i * 3 + 1) as u32, c.g);
                bytes.set_index((i * 3 + 2) as u32, c.b);
            }
            set(&object, "values", &bytes.into())?;
            set(
                &object,
                "pointCount",
                &JsValue::from_f64(colors.len() as f64),
            )?;
        }
        SelectedRead::Attribute(column) => {
            set(&object, "kind", &JsValue::from_str("attribute"))?;
            set(
                &object,
                "name",
                &JsValue::from_str(column.descriptor().name()),
            )?;
            set(
                &object,
                "components",
                &JsValue::from_f64(f64::from(column.descriptor().component_count())),
            )?;
            set(
                &object,
                "dataType",
                &JsValue::from_str(&format!("{:?}", column.descriptor().data_type())),
            )?;
            set(
                &object,
                "pointCount",
                &JsValue::from_f64(
                    (column.values().len() / column.descriptor().component_count() as usize) as f64,
                ),
            )?;
            let values = match column.values() {
                AttributeValues::U8(v) => Uint8Array::from(v.as_slice()).into(),
                AttributeValues::I8(v) => js_sys::Int8Array::from(v.as_slice()).into(),
                AttributeValues::U16(v) => js_sys::Uint16Array::from(v.as_slice()).into(),
                AttributeValues::I16(v) => js_sys::Int16Array::from(v.as_slice()).into(),
                AttributeValues::U32(v) => js_sys::Uint32Array::from(v.as_slice()).into(),
                AttributeValues::I32(v) => js_sys::Int32Array::from(v.as_slice()).into(),
                AttributeValues::U64(v) => js_sys::BigUint64Array::from(v.as_slice()).into(),
                AttributeValues::I64(v) => js_sys::BigInt64Array::from(v.as_slice()).into(),
                AttributeValues::F32(v) => Float32Array::from(v.as_slice()).into(),
                AttributeValues::F64(v) => js_sys::Float64Array::from(v.as_slice()).into(),
            };
            set(&object, "values", &values)?;
        }
    }
    Ok(object.into())
}
fn positions(points: &PointBatch) -> Float32Array {
    let mut values = Vec::with_capacity(points.len() * 3);
    for p in points.points() {
        values.extend_from_slice(&p.position);
    }
    Float32Array::from(values.as_slice())
}
fn set(target: &js_sys::Object, key: &str, value: &JsValue) -> Result<(), JsValue> {
    js_sys::Reflect::set(target, &JsValue::from_str(key), value).map(|_| ())
}
fn number_array(values: &[f64]) -> JsValue {
    let array = js_sys::Array::new();
    for value in values {
        array.push(&JsValue::from_f64(*value));
    }
    array.into()
}
/// A cancelled request rejects with an `AbortError`, the same name an aborted range read carries,
/// so workers classify both as cancellations.
fn cancelled_error() -> JsValue {
    let error = js_sys::Error::new("LOD read request was cancelled");
    error.set_name("AbortError");
    error.into()
}
fn err(message: &str) -> JsValue {
    JsValue::from_str(message)
}
fn format_error(error: impl std::fmt::Display) -> JsValue {
    err(&error.to_string())
}

/// Error of a failed open: a retired family is named from the header and footer already read, so
/// the success path costs no request.
fn open_error(error: viewer_format::FormatError, header: Option<&[u8]>, footer: &[u8]) -> JsValue {
    format_error(viewer_format::explain_retired(error, header, Some(footer)))
}

impl WasmAsyncFormatReader {
    /// The column a block is painted with when `requested` is the selected attribute (see
    /// `substitute_column`, which `assemble_async_block` shares).
    fn painted_column(&self, id: BlockId, requested: u32) -> Result<Option<u32>, JsValue> {
        let reader = &self.shared.reader;
        crate::lod_driver::substitute_column(
            requested,
            reader.metadata().hierarchy().attributes(),
            |index| reader.attribute_present(id, index),
        )
        .map_err(format_error)
    }

    /// The metadata-only reader behind this facade (root page only when the format is paged).
    pub(crate) fn format_reader(&self) -> &Reader {
        &self.shared.reader
    }
}
