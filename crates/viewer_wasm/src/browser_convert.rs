use std::io::{self, Seek, SeekFrom, Write};
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, HierarchySemantics, NodeId, SpatialBounds, SpatialHierarchy,
    SpatialNode, SpatialSubdivision,
};
use viewer_format::{
    FormatMetadata, FormatWriter, FormatWriterOptions, OctreeWriter, SourcePointBlock,
};
use wasm_bindgen::{JsCast, prelude::*};
use web_sys::Blob;

use crate::browser_io::{BrowserBlobReader, js_error_to_io};
use crate::lod_partition::partition_block;
use crate::lod_representatives::{MAX_REPRESENTATIVE_POINTS, VoxelRepresentativeBuffer};

const CONVERSION_POINTS_PER_BLOCK: u64 = 16_384;
const MAX_CONVERSION_BLOCKS: u64 = 1_000_000;
const FULL_RESOLUTION_DIVISIONS: u8 = 2;
const FULL_RESOLUTION_CELLS: usize = 8;

/// Streams a LAS/LAZ Blob into an OPFS-backed `.oct` output inside a dedicated worker.
/// The output sink is a FileSystemSyncAccessHandle supplied by the host.
#[wasm_bindgen]
pub struct WasmLasOctConverter {
    reader: las::Reader,
    input: Blob,
    output_handle: JsValue,
    attributes_handle: JsValue,
    buffer: las::PointData,
    writer: Option<FormatWriter<OpfsWriter>>,
    format_version: viewer_format::FormatVersion,
    processed_points: u64,
    total_points: u64,
    spatial_bounds: SpatialBounds,
    frame: CoordinateFrame,
    schema: Vec<AttributeDescriptor>,
    subdivision: SpatialSubdivision,
    cell_point_counts: Vec<u64>,
    cell_block_counts: Vec<u64>,
    cell_representatives: Vec<VoxelRepresentativeBuffer>,
    cell_block_offsets: Vec<u64>,
    next_cell_block: Vec<u64>,
    writing_full_resolution: bool,
    failed: bool,
}

#[wasm_bindgen]
impl WasmLasOctConverter {
    #[wasm_bindgen(constructor)]
    pub fn new(
        input: Blob,
        output_handle: JsValue,
        attributes_handle: JsValue,
    ) -> Result<Self, JsValue> {
        let source = BrowserBlobReader::new(input.clone(), false)?;
        preflight_blob(&input)?;
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
        if block_count
            .saturating_mul(FULL_RESOLUTION_CELLS as u64)
            .saturating_add(FULL_RESOLUTION_CELLS as u64)
            > MAX_CONVERSION_BLOCKS
        {
            return Err(JsValue::from_str(
                "LAS/LAZ exceeds the browser converter block-index limit",
            ));
        }
        let schema = las_attribute_schema(&point_format);
        let buffer = las::PointDataBuilder::new()
            .for_header(reader.header())
            .build();
        let subdivision = SpatialSubdivision::new(spatial_bounds, FULL_RESOLUTION_DIVISIONS)
            .map_err(|error| JsValue::from_str(&format!("LAS/LAZ partition: {error}")))?;
        let cell_representatives = (0..FULL_RESOLUTION_CELLS)
            .map(|cell| {
                let cell_bounds = subdivision
                    .cell_bounds(cell)
                    .map_err(|error| JsValue::from_str(&error.to_string()))?;
                VoxelRepresentativeBuffer::new(schema.clone(), cell_bounds, frame.origin())
                    .map_err(|error| JsValue::from_str(&error))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            reader,
            input,
            output_handle,
            attributes_handle,
            buffer,
            writer: None,
            format_version: viewer_format::FormatVersion::DEFAULT_OUTPUT,
            processed_points: 0,
            total_points,
            spatial_bounds,
            frame,
            cell_representatives,
            schema,
            subdivision,
            cell_point_counts: vec![0; FULL_RESOLUTION_CELLS],
            cell_block_counts: vec![0; FULL_RESOLUTION_CELLS],
            cell_block_offsets: vec![0; FULL_RESOLUTION_CELLS],
            next_cell_block: vec![0; FULL_RESOLUTION_CELLS],
            writing_full_resolution: false,
            failed: false,
        })
    }

    /// Selects an implemented storage revision without changing the default v5 constructor.
    #[wasm_bindgen(js_name = createForVersion)]
    pub fn create_for_version(
        input: Blob,
        output_handle: JsValue,
        attributes_handle: JsValue,
        version: String,
    ) -> Result<Self, JsValue> {
        let version: viewer_format::FormatVersion = version.parse().map_err(JsValue::from_str)?;
        version
            .ensure_writable()
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let mut converter = Self::new(input, output_handle, attributes_handle)?;
        converter.format_version = version;
        Ok(converter)
    }

    /// Converts at most one bounded block. Call again until `done` is true.
    #[wasm_bindgen(js_name = convertNext)]
    pub fn convert_next(&mut self) -> Result<JsValue, JsValue> {
        if self.failed {
            return Err(JsValue::from_str(
                "conversion cannot continue after a failed block",
            ));
        }
        if self.processed_points < self.total_points {
            if let Err(error) = self.convert_block() {
                self.failed = true;
                return Err(JsValue::from_str(&error));
            }
        } else if !self.writing_full_resolution
            && let Err(error) = self.begin_full_resolution_pass()
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
            &JsValue::from_bool(
                self.writing_full_resolution && self.processed_points == self.total_points,
            ),
        )?;
        js_sys::Reflect::set(
            &result,
            &"stage".into(),
            &JsValue::from_str(if self.writing_full_resolution {
                "writing"
            } else {
                "planning"
            }),
        )?;
        Ok(result.into())
    }

    /// Writes the final index/footer and flushes the OPFS access handle.
    pub fn finish(&mut self) -> Result<(), JsValue> {
        if self.failed
            || !self.writing_full_resolution
            || self.processed_points != self.total_points
        {
            return Err(JsValue::from_str("conversion is incomplete"));
        }
        let representative_base = self.cell_block_offsets.last().copied().unwrap_or(0)
            + self.cell_block_counts.last().copied().unwrap_or(0);
        for (cell, representatives) in self.cell_representatives.drain(..).enumerate() {
            if representatives.point_count() == 0 {
                continue;
            }
            let block = representatives
                .into_block(BlockId::new(representative_base + cell as u64))
                .map_err(|error| JsValue::from_str(&format!("coarse LOD block: {error}")))?;
            OctreeWriter::write_block(
                self.writer
                    .as_mut()
                    .ok_or_else(|| JsValue::from_str("conversion writer is closed"))?,
                block,
            )
            .map_err(|error| JsValue::from_str(&format!("writing coarse LOD block: {error}")))?;
        }
        let writer = self
            .writer
            .take()
            .ok_or_else(|| JsValue::from_str("conversion is already finished"))?;
        let (mut sink, attributes_sink) = OctreeWriter::finish_pair(writer)
            .map_err(|error| JsValue::from_str(&format!("finalizing .oct pair: {error}")))?;
        sink.flush()
            .map_err(|error| JsValue::from_str(&format!("flushing .oct: {error}")))?;
        if let Some(mut attributes_sink) = attributes_sink {
            attributes_sink
                .flush()
                .map_err(|error| JsValue::from_str(&format!("flushing .octattrs: {error}")))?;
        }
        Ok(())
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
        let point_format = *self.reader.header().point_format();
        let mut gps_time = point_format.has_gps_time.then(|| Vec::with_capacity(count));
        let mut nir = point_format.has_nir.then(|| Vec::with_capacity(count));
        let mut rgb = point_format
            .has_color
            .then(|| Vec::with_capacity(count.saturating_mul(3)));
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
                gps_time
                    .as_mut()
                    .ok_or("unexpected GPS time")?
                    .push(values.next().ok_or("missing GPS time")?);
            }
            if let Some(values) = nir_values.as_mut() {
                nir.as_mut()
                    .ok_or("unexpected NIR value")?
                    .push(values.next().ok_or("missing NIR value")?);
            }
            if let Some((red, green, blue)) = rgb_point {
                rgb.as_mut()
                    .ok_or("unexpected RGB value")?
                    .extend_from_slice(&[red, green, blue]);
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
        if point_format.has_gps_time {
            columns.push(attribute_column(
                "gps_time",
                AttributeDataType::F64,
                1,
                AttributeValues::F64(gps_time.ok_or("missing GPS values")?),
                count,
            )?);
        }
        if point_format.has_color {
            columns.push(attribute_column(
                "rgb",
                AttributeDataType::U16,
                3,
                AttributeValues::U16(rgb.ok_or("missing RGB values")?),
                count,
            )?);
        }
        if point_format.has_nir {
            columns.push(attribute_column(
                "nir",
                AttributeDataType::U16,
                1,
                AttributeValues::U16(nir.ok_or("missing NIR values")?),
                count,
            )?);
        }
        let world_positions = positions.clone();
        let bounds = self.reader.header().bounds();
        let origin = [bounds.min.x, bounds.min.y, bounds.min.z];
        for position in &mut positions {
            for axis in 0..3 {
                position[axis] -= origin[axis];
            }
        }
        let block = SourcePointBlock::new(BlockId::new(0), positions, colors, columns);
        let partitions = partition_block(&block, &world_positions, self.subdivision)?;
        if self.writing_full_resolution {
            for partition in partitions {
                let cell = partition.cell;
                let partition = partition.block;
                let local_index = self.next_cell_block[cell];
                if local_index >= self.cell_block_counts[cell] {
                    return Err("LAS/LAZ partition changed between conversion passes".into());
                }
                let block_id = self.cell_block_offsets[cell] + local_index;
                let partition = partition.with_id(BlockId::new(block_id));
                OctreeWriter::write_block(
                    self.writer.as_mut().ok_or("conversion writer is closed")?,
                    partition,
                )
                .map_err(|error| format!("writing .oct spatial block: {error}"))?;
                self.next_cell_block[cell] += 1;
            }
        } else {
            for partition in partitions {
                let cell = partition.cell;
                let cell_positions = partition.world_positions;
                let partition = partition.block;
                let count = partition.positions_local().len() as u64;
                self.cell_point_counts[cell] = self.cell_point_counts[cell]
                    .checked_add(count)
                    .ok_or("spatial point count overflow")?;
                self.cell_block_counts[cell] += 1;
                self.cell_representatives[cell].include_block(&partition, &cell_positions)?;
            }
        }
        self.processed_points += count as u64;
        Ok(())
    }

    fn begin_full_resolution_pass(&mut self) -> Result<(), String> {
        let mut nodes = Vec::with_capacity(FULL_RESOLUTION_CELLS + 1);
        let mut block_offset = 0_u64;
        for (cell, block_count) in self.cell_block_counts.iter().copied().enumerate() {
            self.cell_block_offsets[cell] = block_offset;
            block_offset = block_offset
                .checked_add(block_count)
                .ok_or("spatial block index overflow")?;
            let cell_bounds = self
                .subdivision
                .cell_bounds(cell)
                .map_err(|error| error.to_string())?;
            let block_ids = (self.cell_block_offsets[cell]..block_offset)
                .map(BlockId::new)
                .collect();
            nodes.push(
                SpatialNode::new(
                    NodeId::new(cell as u64 + 1),
                    Some(NodeId::new(0)),
                    1,
                    cell_bounds,
                    self.cell_point_counts[cell],
                    0.0,
                    block_ids,
                )
                .map_err(|error| error.to_string())?,
            );
        }
        let representative_count = self
            .cell_representatives
            .iter()
            .map(|representatives| representatives.point_count() as u64)
            .sum();
        let representative_blocks = self
            .cell_representatives
            .iter()
            .enumerate()
            .filter(|(_, representatives)| representatives.point_count() > 0)
            .map(|(cell, _)| BlockId::new(block_offset + cell as u64))
            .collect();
        nodes.push(
            SpatialNode::new(
                NodeId::new(0),
                None,
                0,
                self.spatial_bounds,
                representative_count,
                bounds_extent(self.spatial_bounds),
                representative_blocks,
            )
            .map_err(|error| error.to_string())?,
        );
        nodes.rotate_right(1);
        let hierarchy = SpatialHierarchy::new_with_semantics(
            nodes,
            self.schema.clone(),
            Some(self.total_points),
            HierarchySemantics::Replacement,
        )
        .map_err(|error| error.to_string())?;
        let scale = visual_scale(self.spatial_bounds).map_err(|error| format!("{error:?}"))?;
        let metadata = FormatMetadata::new(
            DatasetMetadata::new(self.frame, hierarchy),
            None,
            Some(format!("browser conversion: eight spatial full-resolution nodes with up to {MAX_REPRESENTATIVE_POINTS} voxel-center representatives; color and RGB are averaged per voxel and other attributes come from the nearest source point; extra bytes and CRS are not retained")),
            Vec::new(),
        )
        .and_then(|metadata| metadata.with_visual_scale(scale))
        .map_err(|error| error.to_string())?;
        let options =
            FormatWriterOptions::new(0.01, 8 * 1024 * 1024).map_err(|error| error.to_string())?;
        self.writer = Some(
            FormatWriter::new_split_for_version(
                self.format_version,
                OpfsWriter::new(self.output_handle.clone())
                    .map_err(|error| format!("output file: {error:?}"))?,
                OpfsWriter::new(self.attributes_handle.clone())
                    .map_err(|error| format!("attribute output file: {error:?}"))?,
                metadata,
                options,
            )
            .map_err(|error| format!("output file: {error}"))?,
        );
        preflight_blob(&self.input)
            .map_err(|error| format!("reopening LAS/LAZ input: {error:?}"))?;
        self.reader = las::Reader::new(
            BrowserBlobReader::new(self.input.clone(), false)
                .map_err(|error| format!("reopening LAS/LAZ input: {error:?}"))?,
        )
        .map_err(|error| format!("reopening LAS/LAZ input: {error}"))?;
        self.buffer = las::PointDataBuilder::new()
            .for_header(self.reader.header())
            .build();
        self.processed_points = 0;
        self.writing_full_resolution = true;
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

fn las_attribute_schema(format: &las::point::Format) -> Vec<AttributeDescriptor> {
    viewer_core::las::las_attribute_schema(format.has_gps_time, format.has_color, format.has_nir)
}

fn bounds_extent(bounds: SpatialBounds) -> f64 {
    let min = bounds.min();
    let max = bounds.max();
    (0..3)
        .map(|axis| max[axis] - min[axis])
        .fold(0.0_f64, f64::max)
}

fn visual_scale(bounds: SpatialBounds) -> Result<f64, JsValue> {
    viewer_core::las::visual_scale_for_bounds(bounds)
        .ok_or_else(|| JsValue::from_str("LAS/LAZ bounds exceed the viewer coordinate range"))
}

fn preflight_blob(blob: &Blob) -> Result<(), JsValue> {
    let mut reader = BrowserBlobReader::new(blob.clone(), false)?;
    let header_error = |error| JsValue::from_str(&format!("LAS/LAZ header: {error}"));
    let layout = viewer_core::las::preflight_las_layout(&mut reader, blob.size() as u64)
        .map_err(header_error)?;
    // The sequential decoder traps on a table that covers fewer points than the header declares.
    if let Some(layout) = layout {
        viewer_core::las::check_laz_chunk_table(&mut reader, layout).map_err(header_error)?;
    }
    Ok(())
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
