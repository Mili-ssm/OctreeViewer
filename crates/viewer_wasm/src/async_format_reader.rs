//! Async range access to split `.oct` files using browser Blob promises.
use std::io::Cursor;

use js_sys::{ArrayBuffer, Float32Array, Promise, Uint8Array};
use serde::Serialize;
use viewer_core::{AttributeValues, BlockId, PointBatch};
use viewer_format::{
    FormatReader, FormatReaderLimits, PayloadReadRange, ReadSelection, ReadSource,
    SPLIT_FOOTER_BYTES, SPLIT_HEADER_BYTES, SelectedRead,
};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::JsFuture;
use web_sys::Blob;

type Reader = FormatReader<Cursor<Vec<u8>>>;

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
    reader: Reader,
    primary: Blob,
    attributes: Option<Blob>,
    plan: Option<viewer_format::ReadPlan>,
}

#[wasm_bindgen]
impl WasmAsyncFormatReader {
    /// Opens split primary and optional attribute Blobs. Reads header/footer/index by ranges only.
    #[wasm_bindgen(js_name = open)]
    pub async fn open(
        primary: Blob,
        attributes: Option<Blob>,
    ) -> Result<WasmAsyncFormatReader, JsValue> {
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
            reader,
            primary,
            attributes,
            plan: None,
        })
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
        let plan = self.reader.plan_read(id, selected).map_err(format_error)?;
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
        for range in plan.ranges() {
            let blob = match range.source() {
                ReadSource::Primary => &self.primary,
                ReadSource::Attributes => self
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
            buffers.push(read_range(blob, range.offset(), range.stored_bytes()).await?);
        }
        let selected = self
            .reader
            .materialize_selected_from_buffers(plan, buffers)
            .map_err(format_error)?;
        selected_to_js(selected)
    }
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
fn err(message: &str) -> JsValue {
    JsValue::from_str(message)
}
fn format_error(error: impl std::fmt::Display) -> JsValue {
    err(&error.to_string())
}
