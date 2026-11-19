use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    Point,
};
use wasm_bindgen::prelude::*;

use crate::ViewerError;
use viewer_render_wgpu::RendererConfig;

const WASM_POINT_BATCH_POINTS: usize = 16_384;

pub(super) fn raw_scene_point_limit() -> usize {
    RendererConfig::default().gpu_budget.max_visible_instances
}

pub(super) fn parse_block_id(value: &str) -> Result<BlockId, JsValue> {
    value
        .parse::<u64>()
        .map(BlockId::new)
        .map_err(|_| JsValue::from_str("blockId must be an unsigned 64-bit integer string"))
}

pub(super) fn attribute_column_from_js(
    schema: &[AttributeDescriptor],
    value: &JsValue,
    point_count: usize,
) -> Result<AttributeColumn, JsValue> {
    let name = js_sys::Reflect::get(value, &"name".into())?
        .as_string()
        .ok_or_else(|| JsValue::from_str("attribute result is missing its name"))?;
    let descriptor = schema
        .iter()
        .find(|descriptor| descriptor.name() == name)
        .cloned()
        .ok_or_else(|| JsValue::from_str("attribute result is absent from the dataset schema"))?;
    let data = js_sys::Reflect::get(value, &"values".into())?;
    let values = match descriptor.data_type() {
        AttributeDataType::U8 if data.is_instance_of::<js_sys::Uint8Array>() => {
            AttributeValues::U8(js_sys::Uint8Array::new(&data).to_vec())
        }
        AttributeDataType::I8 if data.is_instance_of::<js_sys::Int8Array>() => {
            AttributeValues::I8(js_sys::Int8Array::new(&data).to_vec())
        }
        AttributeDataType::U16 if data.is_instance_of::<js_sys::Uint16Array>() => {
            AttributeValues::U16(js_sys::Uint16Array::new(&data).to_vec())
        }
        AttributeDataType::I16 if data.is_instance_of::<js_sys::Int16Array>() => {
            AttributeValues::I16(js_sys::Int16Array::new(&data).to_vec())
        }
        AttributeDataType::U32 if data.is_instance_of::<js_sys::Uint32Array>() => {
            AttributeValues::U32(js_sys::Uint32Array::new(&data).to_vec())
        }
        AttributeDataType::I32 if data.is_instance_of::<js_sys::Int32Array>() => {
            AttributeValues::I32(js_sys::Int32Array::new(&data).to_vec())
        }
        AttributeDataType::U64 if data.is_instance_of::<js_sys::BigUint64Array>() => {
            AttributeValues::U64(js_sys::BigUint64Array::new(&data).to_vec())
        }
        AttributeDataType::I64 if data.is_instance_of::<js_sys::BigInt64Array>() => {
            AttributeValues::I64(js_sys::BigInt64Array::new(&data).to_vec())
        }
        AttributeDataType::F32 if data.is_instance_of::<js_sys::Float32Array>() => {
            AttributeValues::F32(js_sys::Float32Array::new(&data).to_vec())
        }
        AttributeDataType::F64 if data.is_instance_of::<js_sys::Float64Array>() => {
            AttributeValues::F64(js_sys::Float64Array::new(&data).to_vec())
        }
        _ => {
            return Err(JsValue::from_str(
                "attribute typed array does not match the dataset schema",
            ));
        }
    };
    AttributeColumn::new(descriptor, values, point_count)
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

pub(super) fn attribute_column_payload_bytes(column: &AttributeColumn) -> Result<usize, JsValue> {
    let value_bytes = match column.values() {
        AttributeValues::U8(values) => values.len().checked_mul(std::mem::size_of::<u8>()),
        AttributeValues::I8(values) => values.len().checked_mul(std::mem::size_of::<i8>()),
        AttributeValues::U16(values) => values.len().checked_mul(std::mem::size_of::<u16>()),
        AttributeValues::I16(values) => values.len().checked_mul(std::mem::size_of::<i16>()),
        AttributeValues::U32(values) => values.len().checked_mul(std::mem::size_of::<u32>()),
        AttributeValues::I32(values) => values.len().checked_mul(std::mem::size_of::<i32>()),
        AttributeValues::U64(values) => values.len().checked_mul(std::mem::size_of::<u64>()),
        AttributeValues::I64(values) => values.len().checked_mul(std::mem::size_of::<i64>()),
        AttributeValues::F32(values) => values.len().checked_mul(std::mem::size_of::<f32>()),
        AttributeValues::F64(values) => values.len().checked_mul(std::mem::size_of::<f64>()),
    }
    .ok_or_else(|| JsValue::from_str("attribute size exceeds the cache budget"))?;
    value_bytes
        .checked_add(column.descriptor().name().len())
        .ok_or_else(|| JsValue::from_str("attribute size exceeds the cache budget"))
}

pub(super) fn points_from_typed_arrays(
    positions: &js_sys::Float32Array,
    colors: &js_sys::Uint8Array,
    max_points: usize,
) -> Result<Vec<Point>, JsValue> {
    let position_components = positions.length() as usize;
    let color_components = colors.length() as usize;
    if !position_components.is_multiple_of(3) || color_components != position_components {
        return Err(JsValue::from_str(
            "positions and colors must contain three values per point",
        ));
    }
    let point_count = position_components / 3;
    if point_count > max_points {
        return Err(JsValue::from_str(&format!(
            "point batch has {point_count} points, limit is {max_points}"
        )));
    }
    let mut points = Vec::new();
    points
        .try_reserve_exact(point_count)
        .map_err(|_| JsValue::from_str("unable to allocate decoded point batch"))?;
    let staged_points = point_count.min(WASM_POINT_BATCH_POINTS);
    let mut position_batch = vec![0.0_f32; staged_points * 3];
    let mut color_batch = vec![0_u8; staged_points * 3];
    let mut point_offset = 0;
    while point_offset < point_count {
        let batch_points = (point_count - point_offset).min(WASM_POINT_BATCH_POINTS);
        let batch_components = batch_points * 3;
        let component_start = (point_offset * 3) as u32;
        let component_end = component_start + batch_components as u32;
        positions
            .subarray(component_start, component_end)
            .copy_to(&mut position_batch[..batch_components]);
        colors
            .subarray(component_start, component_end)
            .copy_to(&mut color_batch[..batch_components]);
        for (position, color) in position_batch[..batch_components]
            .as_chunks::<3>()
            .0
            .iter()
            .zip(color_batch[..batch_components].as_chunks::<3>().0.iter())
        {
            if position.iter().any(|value| !value.is_finite()) {
                return Err(JsValue::from_str("block positions must be finite"));
            }
            points.push(Point::new(
                [position[0], position[1], position[2]],
                ColorRgb::new(color[0], color[1], color[2]),
            ));
        }
        point_offset += batch_points;
    }
    Ok(points)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a `map_err` function path, which hands the error over by value"
)]
pub(super) fn viewer_error_to_js(error: ViewerError) -> JsValue {
    JsValue::from_str(&error.to_string())
}
