use std::io::{self, Seek, SeekFrom, Write};
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, HierarchySemantics, NodeId, SpatialBounds, SpatialHierarchy,
    SpatialNode,
};
use viewer_format::{FormatMetadata, FormatWriter, FormatWriterOptions, SourcePointBlock};
use wasm_bindgen::{JsCast, prelude::*};
use web_sys::Blob;

use crate::bindings::{BrowserBlobReader, js_error_to_io};
use crate::lod_representatives::{MAX_REPRESENTATIVE_POINTS, RepresentativeBuffer};

const CONVERSION_POINTS_PER_BLOCK: u64 = 16_384;
const MAX_CONVERSION_BLOCKS: u64 = 1_000_000;

/// Streams a LAS/LAZ Blob into an OPFS-backed `.oct` output inside a dedicated worker.
/// The output sink is a FileSystemSyncAccessHandle supplied by the host.
#[wasm_bindgen]
pub struct WasmLasOctConverter {
    reader: las::Reader,
    buffer: las::PointData,
    writer: Option<FormatWriter<OpfsWriter>>,
    processed_points: u64,
    total_points: u64,
    next_block_id: u64,
    representatives: RepresentativeBuffer,
    source_block_count: u64,
    failed: bool,
}

#[wasm_bindgen]
impl WasmLasOctConverter {
    #[wasm_bindgen(constructor)]
    pub fn new(input: Blob, output_handle: JsValue) -> Result<Self, JsValue> {
        let source = BrowserBlobReader::new(input, false)?;
        let reader = las::Reader::new(source)
            .map_err(|error| JsValue::from_str(&format!("LAS/LAZ header: {error}")))?;
        let header = reader.header();
        let bounds = header.bounds();
        let min = [bounds.min.x, bounds.min.y, bounds.min.z];
        let max = [bounds.max.x, bounds.max.y, bounds.max.z];
        let spatial_bounds = SpatialBounds::new(min, max)
            .map_err(|error| JsValue::from_str(&format!("LAS/LAZ bounds: {error}")))?;
        let frame = CoordinateFrame::new(min)
            .map_err(|error| JsValue::from_str(&format!("coordinate frame: {error}")))?;
        let total_points = header.number_of_points();
        let point_format = *header.point_format();
        let block_count = total_points.div_ceil(CONVERSION_POINTS_PER_BLOCK);
        let representative_count = total_points.min(MAX_REPRESENTATIVE_POINTS);
        let representative_block_count = representative_count.div_ceil(CONVERSION_POINTS_PER_BLOCK);
        if block_count.saturating_add(representative_block_count) > MAX_CONVERSION_BLOCKS {
            return Err(JsValue::from_str(
                "LAS/LAZ exceeds the browser converter block-index limit",
            ));
        }
        let coarse_block_ids = (block_count..block_count + representative_block_count)
            .map(BlockId::new)
            .collect();
        let full_resolution_block_ids = (0..block_count).map(BlockId::new).collect();
        let schema = las_attribute_schema(&point_format)?;
        let root = SpatialNode::new(
            NodeId::new(0),
            None,
            0,
            spatial_bounds,
            representative_count,
            bounds_extent(spatial_bounds),
            coarse_block_ids,
        )
        .map_err(|error| JsValue::from_str(&format!("LAS/LAZ hierarchy: {error}")))?;
        let full_resolution = SpatialNode::new(
            NodeId::new(1),
            Some(NodeId::new(0)),
            1,
            spatial_bounds,
            total_points,
            0.0,
            full_resolution_block_ids,
        )
        .map_err(|error| JsValue::from_str(&format!("LAS/LAZ hierarchy: {error}")))?;
        let hierarchy = SpatialHierarchy::new_with_semantics(
            vec![root, full_resolution],
            schema.clone(),
            Some(total_points),
            HierarchySemantics::Replacement,
        )
        .map_err(|error| JsValue::from_str(&format!("LAS/LAZ hierarchy: {error}")))?;
        let visual_scale = visual_scale(spatial_bounds)?;
        let metadata = FormatMetadata::new(
            DatasetMetadata::new(frame, hierarchy),
            None,
            Some(format!("browser conversion: full-resolution source points in bounded blocks with up to {MAX_REPRESENTATIVE_POINTS} deterministic reservoir samples for progressive display; extra bytes and CRS are not retained")),
            Vec::new(),
        )
        .and_then(|metadata| metadata.with_visual_scale(visual_scale))
        .map_err(|error| JsValue::from_str(&format!("output metadata: {error}")))?;
        let sink = OpfsWriter::new(output_handle)?;
        let options = FormatWriterOptions::new(0.01, 8 * 1024 * 1024)
            .map_err(|error| JsValue::from_str(&format!("output options: {error}")))?;
        let writer = FormatWriter::new(sink, metadata, options)
            .map_err(|error| JsValue::from_str(&format!("output file: {error}")))?;
        let buffer = las::PointDataBuilder::new()
            .for_header(reader.header())
            .build();

        Ok(Self {
            reader,
            buffer,
            writer: Some(writer),
            processed_points: 0,
            total_points,
            next_block_id: 0,
            representatives: RepresentativeBuffer::new(schema),
            source_block_count: block_count,
            failed: false,
        })
    }

    /// Converts at most one bounded block. Call again until `done` is true.
    #[wasm_bindgen(js_name = convertNext)]
    pub fn convert_next(&mut self) -> Result<JsValue, JsValue> {
        if self.failed {
            return Err(JsValue::from_str(
                "conversion cannot continue after a failed block",
            ));
        }
        if self.processed_points < self.total_points
            && let Err(error) = self.convert_block()
        {
            self.failed = true;
            return Err(JsValue::from_str(&error));
        }
        let result = js_sys::Object::new();
        js_sys::Reflect::set(
            &result,
            &"processedPoints".into(),
            &JsValue::from_str(&self.processed_points.to_string()),
        )?;
        js_sys::Reflect::set(
            &result,
            &"totalPoints".into(),
            &JsValue::from_str(&self.total_points.to_string()),
        )?;
        js_sys::Reflect::set(
            &result,
            &"done".into(),
            &JsValue::from_bool(self.processed_points == self.total_points),
        )?;
        Ok(result.into())
    }

    /// Writes the final index/footer and flushes the OPFS access handle.
    pub fn finish(&mut self) -> Result<(), JsValue> {
        if self.failed || self.processed_points != self.total_points {
            return Err(JsValue::from_str("conversion is incomplete"));
        }
        if self.representatives.point_count() > 0 {
            let representative_buffer = std::mem::replace(
                &mut self.representatives,
                RepresentativeBuffer::new(Vec::new()),
            );
            let block = representative_buffer
                .into_block(BlockId::new(self.source_block_count))
                .map_err(|error| JsValue::from_str(&format!("coarse LOD block: {error}")))?;
            self.writer
                .as_mut()
                .ok_or_else(|| JsValue::from_str("conversion writer is closed"))?
                .write_block(block)
                .map_err(|error| {
                    JsValue::from_str(&format!("writing coarse LOD block: {error}"))
                })?;
        }
        let writer = self
            .writer
            .take()
            .ok_or_else(|| JsValue::from_str("conversion is already finished"))?;
        let mut sink = writer
            .finish()
            .map_err(|error| JsValue::from_str(&format!("finalizing .oct: {error}")))?;
        sink.flush()
            .map_err(|error| JsValue::from_str(&format!("flushing .oct: {error}")))
    }
}

impl WasmLasOctConverter {
    fn convert_block(&mut self) -> Result<(), String> {
        let requested =
            (self.total_points - self.processed_points).min(CONVERSION_POINTS_PER_BLOCK);
        self.reader
            .fill_points(requested, &mut self.buffer)
            .map_err(|error| format!("reading LAS/LAZ points: {error}"))?;
        if self.buffer.is_empty() {
            return Err(format!(
                "LAS/LAZ ended after {} of {} points",
                self.processed_points, self.total_points
            ));
        }
        let count = self.buffer.len();
        let mut positions = Vec::with_capacity(count);
        let mut colors = Vec::with_capacity(count);
        let mut intensity = Vec::with_capacity(count);
        let mut classification = Vec::with_capacity(count);
        let mut return_number = Vec::with_capacity(count);
        let mut number_of_returns = Vec::with_capacity(count);
        let mut scan_angle = Vec::with_capacity(count);
        let mut user_data = Vec::with_capacity(count);
        let mut point_source_id = Vec::with_capacity(count);
        let mut gps_time = Vec::with_capacity(count);
        let mut nir = Vec::with_capacity(count);
        let mut rgb = Vec::with_capacity(count.saturating_mul(3));
        let mut intensities = self.buffer.intensity();
        let mut classifications = self.buffer.classification();
        let mut return_numbers = self.buffer.return_number();
        let mut returns_per_pulse = self.buffer.number_of_returns();
        let mut scan_angles = self.buffer.scan_angle_degrees();
        let mut user_data_values = self.buffer.user_data();
        let mut source_ids = self.buffer.point_source_id();
        let mut gps_times = self.buffer.gps_time();
        let mut nir_values = self.buffer.nir();
        let mut rgb_values = self.buffer.rgb();
        for (((x, y), z), point_intensity) in self
            .buffer
            .x()
            .zip(self.buffer.y())
            .zip(self.buffer.z())
            .zip(&mut intensities)
        {
            if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                return Err("LAS/LAZ contains a non-finite position".into());
            }
            positions.push([x, y, z]);
            let class = classifications.next().ok_or("missing LAS classification")?;
            let rgb_point = rgb_values.as_mut().and_then(Iterator::next);
            let color = rgb_point.map_or_else(
                || {
                    let gray = (point_intensity >> 8) as u8;
                    ColorRgb::new(gray, gray, gray)
                },
                |(red, green, blue)| {
                    ColorRgb::new((red >> 8) as u8, (green >> 8) as u8, (blue >> 8) as u8)
                },
            );
            colors.push(color);
            intensity.push(point_intensity);
            classification.push(class);
            return_number.push(return_numbers.next().ok_or("missing return number")?);
            number_of_returns.push(returns_per_pulse.next().ok_or("missing return count")?);
            scan_angle.push(scan_angles.next().ok_or("missing scan angle")?);
            user_data.push(user_data_values.next().ok_or("missing user data")?);
            point_source_id.push(source_ids.next().ok_or("missing point source id")?);
            if let Some(values) = gps_times.as_mut() {
                gps_time.push(values.next().ok_or("missing GPS time")?);
            }
            if let Some(values) = nir_values.as_mut() {
                nir.push(values.next().ok_or("missing NIR value")?);
            }
            if let Some((red, green, blue)) = rgb_point {
                rgb.extend_from_slice(&[red, green, blue]);
            }
        }
        let mut columns = vec![
            attribute_column(
                "intensity",
                AttributeDataType::U16,
                1,
                AttributeValues::U16(intensity),
                count,
            )?,
            attribute_column(
                "classification",
                AttributeDataType::U8,
                1,
                AttributeValues::U8(classification),
                count,
            )?,
            attribute_column(
                "return_number",
                AttributeDataType::U8,
                1,
                AttributeValues::U8(return_number),
                count,
            )?,
            attribute_column(
                "number_of_returns",
                AttributeDataType::U8,
                1,
                AttributeValues::U8(number_of_returns),
                count,
            )?,
            attribute_column(
                "scan_angle_degrees",
                AttributeDataType::F32,
                1,
                AttributeValues::F32(scan_angle),
                count,
            )?,
            attribute_column(
                "user_data",
                AttributeDataType::U8,
                1,
                AttributeValues::U8(user_data),
                count,
            )?,
            attribute_column(
                "point_source_id",
                AttributeDataType::U16,
                1,
                AttributeValues::U16(point_source_id),
                count,
            )?,
        ];
        let point_format = *self.reader.header().point_format();
        if point_format.has_gps_time {
            columns.push(attribute_column(
                "gps_time",
                AttributeDataType::F64,
                1,
                AttributeValues::F64(gps_time),
                count,
            )?);
        }
        if point_format.has_color {
            columns.push(attribute_column(
                "rgb",
                AttributeDataType::U16,
                3,
                AttributeValues::U16(rgb),
                count,
            )?);
        }
        if point_format.has_nir {
            columns.push(attribute_column(
                "nir",
                AttributeDataType::U16,
                1,
                AttributeValues::U16(nir),
                count,
            )?);
        }
        let bounds = self.reader.header().bounds();
        let origin = [bounds.min.x, bounds.min.y, bounds.min.z];
        for position in &mut positions {
            for axis in 0..3 {
                position[axis] -= origin[axis];
            }
        }
        let block =
            SourcePointBlock::new(BlockId::new(self.next_block_id), positions, colors, columns);
        self.representatives.include_block(&block)?;
        self.writer
            .as_mut()
            .ok_or("conversion writer is closed")?
            .write_block(block)
            .map_err(|error| format!("writing .oct block: {error}"))?;
        self.processed_points += count as u64;
        self.next_block_id += 1;
        Ok(())
    }
}

fn attribute_column(
    name: &'static str,
    data_type: AttributeDataType,
    components: u8,
    values: AttributeValues,
    count: usize,
) -> Result<AttributeColumn, String> {
    let descriptor = AttributeDescriptor::new(name, data_type, u32::from(components))
        .map_err(|error| error.to_string())?;
    AttributeColumn::new(descriptor, values, count).map_err(|error| error.to_string())
}

fn las_attribute_schema(format: &las::point::Format) -> Result<Vec<AttributeDescriptor>, JsValue> {
    let mut schema = vec![
        AttributeDescriptor::new("intensity", AttributeDataType::U16, 1),
        AttributeDescriptor::new("classification", AttributeDataType::U8, 1),
        AttributeDescriptor::new("return_number", AttributeDataType::U8, 1),
        AttributeDescriptor::new("number_of_returns", AttributeDataType::U8, 1),
        AttributeDescriptor::new("scan_angle_degrees", AttributeDataType::F32, 1),
        AttributeDescriptor::new("user_data", AttributeDataType::U8, 1),
        AttributeDescriptor::new("point_source_id", AttributeDataType::U16, 1),
    ]
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .map_err(|error| JsValue::from_str(&format!("attribute schema: {error}")))?;
    if format.has_gps_time {
        schema.push(AttributeDescriptor::new("gps_time", AttributeDataType::F64, 1).unwrap());
    }
    if format.has_color {
        schema.push(AttributeDescriptor::new("rgb", AttributeDataType::U16, 3).unwrap());
    }
    if format.has_nir {
        schema.push(AttributeDescriptor::new("nir", AttributeDataType::U16, 1).unwrap());
    }
    Ok(schema)
}

fn bounds_extent(bounds: SpatialBounds) -> f64 {
    let min = bounds.min();
    let max = bounds.max();
    (0..3)
        .map(|axis| max[axis] - min[axis])
        .fold(0.0_f64, f64::max)
}

fn visual_scale(bounds: SpatialBounds) -> Result<f64, JsValue> {
    let extent = bounds_extent(bounds);
    let scale = if extent == 0.0 { 1.0 } else { 100.0 / extent };
    if !scale.is_finite() || scale <= 0.0 || !(scale as f32).is_finite() || scale as f32 == 0.0 {
        return Err(JsValue::from_str(
            "LAS/LAZ bounds exceed the viewer coordinate range",
        ));
    }
    Ok(scale)
}

struct OpfsWriter {
    handle: JsValue,
    position: u64,
}

impl OpfsWriter {
    fn new(handle: JsValue) -> Result<Self, JsValue> {
        Ok(Self {
            handle,
            position: 0,
        })
    }

    fn call(&self, name: &str, arguments: &[JsValue]) -> Result<JsValue, JsValue> {
        let method = js_sys::Reflect::get(&self.handle, &JsValue::from_str(name))?
            .dyn_into::<js_sys::Function>()?;
        let args = js_sys::Array::new();
        for argument in arguments {
            args.push(argument);
        }
        method.apply(&self.handle, &args)
    }
}

impl Write for OpfsWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let array = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
        array.copy_from(bytes);
        let options = js_sys::Object::new();
        js_sys::Reflect::set(
            &options,
            &JsValue::from_str("at"),
            &JsValue::from_f64(self.position as f64),
        )
        .map_err(js_error_to_io)?;
        let written = self
            .call("write", &[array.into(), options.into()])
            .map_err(js_error_to_io)?
            .as_f64()
            .ok_or_else(|| io::Error::other("OPFS write returned an invalid byte count"))?;
        if written < 0.0 || written > bytes.len() as f64 || written.fract() != 0.0 {
            return Err(io::Error::other(
                "OPFS write returned an invalid byte count",
            ));
        }
        let written = written as usize;
        self.position = self.position.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.call("flush", &[]).map_err(js_error_to_io)?;
        Ok(())
    }
}

impl Seek for OpfsWriter {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let size = if matches!(position, SeekFrom::End(_)) {
            self.call("getSize", &[])
                .map_err(js_error_to_io)?
                .as_f64()
                .ok_or_else(|| io::Error::other("OPFS getSize returned an invalid value"))?
                as u64
        } else {
            0
        };
        let next = match position {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
            SeekFrom::End(offset) => size.checked_add_signed(offset),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid OPFS seek"))?;
        self.position = next;
        Ok(next)
    }
}
