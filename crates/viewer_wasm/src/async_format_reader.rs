//! Async range access to split `.oct` files using browser Blob promises.
use std::{cell::Cell, io::Cursor, rc::Rc, sync::Arc};

use js_sys::{ArrayBuffer, Float32Array, Promise, Uint8Array};
use serde::Serialize;
use viewer_core::{AttributeValues, BlockId, PointBatch};
use viewer_format::{
    FormatReader, FormatReaderLimits, PayloadReadRange, ReadBudget, ReadReservation, ReadSelection,
    ReadSource, SPLIT_FOOTER_BYTES, SPLIT_HEADER_BYTES, SelectedRead,
};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::JsFuture;
use web_sys::Blob;

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
/// Positions are Float32Array xyz; geometry is Float32Array xyz + RGB bytes; colors are RGB bytes.
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
    reservation: Option<ReadReservation>,
    cancelled: Rc<Cell<bool>>,
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

    #[wasm_bindgen(js_name = read)]
    pub async fn read(&mut self) -> Result<WasmAsyncLodReadResult, JsValue> {
        let mut plans = std::mem::take(&mut self.plans).into_iter();
        let first = plans
            .next()
            .ok_or_else(|| err("LOD read request is empty"))?;
        let first_selection = first.selection();
        let first_result = read_plan(&self.shared, first, &self.cancelled).await?;
        let second_result = if let Some(second) = plans.next() {
            if self.cancelled.get() {
                return Err(err("LOD read request was cancelled"));
            }
            Some(read_plan(&self.shared, second, &self.cancelled).await?)
        } else {
            None
        };
        let value = lod_result_to_js(first_selection, first_result, second_result)?;
        Ok(WasmAsyncLodReadResult {
            value: Some(value),
            reservation: self.reservation.take(),
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
        let header = read_range(&primary, 0, SPLIT_HEADER_BYTES).await?;
        let footer = read_range(
            &primary,
            primary_len
                .checked_sub(SPLIT_FOOTER_BYTES)
                .ok_or_else(|| err("primary Blob is shorter than footer"))?,
            SPLIT_FOOTER_BYTES,
        )
        .await?;
        let (offset, length) =
            Reader::split_index_range(primary_len, &header, &footer, FormatReaderLimits::default())
                .map_err(format_error)?;
        let metadata_reservation = budget
            .try_reserve(length)
            .map_err(format_error)?
            .ok_or_else(|| {
                err("shared async read budget is unavailable for the resident format index")
            })?;
        let index = read_range(&primary, offset, length).await?;
        let mut reader = Reader::from_split_buffers(
            primary_len,
            &header,
            &footer,
            &index,
            FormatReaderLimits::default(),
        )
        .map_err(format_error)?;
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
        let dataset = metadata.dataset();
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

        let nodes = js_sys::Array::new();
        let mut pending = vec![hierarchy.root_id()];
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
                .map(|id| JsValue::from_str(&id.get().to_string()))
                .unwrap_or(JsValue::NULL);
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
                    .map(|[min, max]| number_array(&[min, max]))
                    .unwrap_or(JsValue::NULL);
                ranges.push(&range);
            }
            attribute_ranges.push(&ranges);
        }
        set(&descriptor, "attributeSchema", &schema.into())?;
        set(&descriptor, "attributeRanges", &attribute_ranges.into())?;
        Ok(descriptor.into())
    }

    /// Plans a block result with one shared reservation, ready for independent async execution.
    /// `positionsAttribute` returns XYZ and one typed column as a single quota-owned result.
    #[wasm_bindgen(js_name = planLodRead)]
    pub fn plan_lod_read(
        &self,
        block_id: u64,
        selection: &str,
        schema_index: Option<u32>,
    ) -> Result<Option<WasmAsyncLodReadRequest>, JsValue> {
        let id = BlockId::new(block_id);
        let mut plans = Vec::with_capacity(2);
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
                plans.push(
                    self.shared
                        .reader
                        .plan_read(id, ReadSelection::Attribute { schema_index })
                        .map_err(format_error)?,
                );
            }
            _ => {
                return Err(err(
                    "selection must be geometry, positions, or positionsAttribute",
                ));
            }
        }
        let required = lod_request_peak_bytes(&plans)?;
        let reservation = match self.budget.try_reserve(required).map_err(format_error)? {
            Some(reservation) => reservation,
            None => return Ok(None),
        };
        Ok(Some(WasmAsyncLodReadRequest {
            shared: Arc::clone(&self.shared),
            plans,
            reservation: Some(reservation),
            cancelled: Rc::new(Cell::new(false)),
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
            buffers.push(read_range_for(&self.shared, range, &not_cancelled).await?);
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
) -> Result<SelectedRead, JsValue> {
    let mut buffers = Vec::with_capacity(plan.ranges().len());
    for range in plan.ranges() {
        if cancelled.get() {
            return Err(err("LOD read request was cancelled"));
        }
        buffers.push(read_range_for(shared, range, cancelled).await?);
    }
    if cancelled.get() {
        return Err(err("LOD read request was cancelled"));
    }
    shared
        .reader
        .materialize_selected_from_buffers(plan, buffers)
        .map_err(format_error)
}

async fn read_range_for(
    shared: &AsyncReaderShared,
    range: &PayloadReadRange,
    cancelled: &Cell<bool>,
) -> Result<Vec<u8>, JsValue> {
    if cancelled.get() {
        return Err(err("LOD read request was cancelled"));
    }
    let blob = match range.source() {
        ReadSource::Primary => &shared.primary,
        ReadSource::Attributes => shared
            .attributes
            .as_ref()
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
        return Err(err("LOD read request was cancelled"));
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
fn err(message: &str) -> JsValue {
    JsValue::from_str(message)
}
fn format_error(error: impl std::fmt::Display) -> JsValue {
    err(&error.to_string())
}
