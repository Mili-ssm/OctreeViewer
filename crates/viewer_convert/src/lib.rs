use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use las::{PointData, PointDataBuilder, Reader};
use thiserror::Error;
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, DatasetSource, NodeId, SpatialBounds, SpatialHierarchy,
    SpatialNode,
};
use viewer_format::{
    FormatMetadata, FormatReader, FormatWriter, FormatWriterOptions, SourcePointBlock,
};

const DEFAULT_POINTS_PER_BLOCK: u64 = 262_144;
const DEFAULT_MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
const MAX_POINTS_PER_BLOCK: u64 = 1_000_000;
const MAX_INPUT_BYTES_PER_BATCH: usize = 8 * 1024 * 1024;
const MAX_BLOCKS: u64 = 1_000_000;
const ESTIMATED_WORKING_BYTES_PER_POINT: u64 = 192;
const ESTIMATED_DIRECTORY_BYTES_PER_BLOCK: u64 = 512;
const ESTIMATED_FIXED_OVERHEAD_BYTES: u64 = 16 * 1024 * 1024;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConvertOptions {
    points_per_block: u64,
    max_quantization_error: f64,
    max_memory_bytes: u64,
}

impl ConvertOptions {
    pub fn new(points_per_block: u64, max_quantization_error: f64) -> Result<Self, ConvertError> {
        Self::with_memory_budget(
            points_per_block,
            max_quantization_error,
            DEFAULT_MEMORY_BUDGET_BYTES,
        )
    }

    pub fn with_memory_budget(
        points_per_block: u64,
        max_quantization_error: f64,
        max_memory_bytes: u64,
    ) -> Result<Self, ConvertError> {
        if points_per_block == 0 || points_per_block > MAX_POINTS_PER_BLOCK {
            return Err(ConvertError::InvalidBlockPointLimit(points_per_block));
        }
        if !max_quantization_error.is_finite() || max_quantization_error < 0.0 {
            return Err(ConvertError::InvalidQuantizationError(
                max_quantization_error,
            ));
        }
        if max_memory_bytes == 0 {
            return Err(ConvertError::InvalidMemoryBudget);
        }
        Ok(Self {
            points_per_block,
            max_quantization_error,
            max_memory_bytes,
        })
    }

    pub const fn points_per_block(self) -> u64 {
        self.points_per_block
    }

    pub const fn max_quantization_error(self) -> f64 {
        self.max_quantization_error
    }

    pub const fn max_memory_budget_bytes(self) -> u64 {
        self.max_memory_bytes
    }
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            points_per_block: DEFAULT_POINTS_PER_BLOCK,
            max_quantization_error: 0.01,
            max_memory_bytes: DEFAULT_MEMORY_BUDGET_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionReport {
    pub point_count: u64,
    pub block_count: u64,
    pub output_bytes: u64,
    pub estimated_peak_memory_bytes: u64,
    pub omitted_dimensions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionStage {
    InspectingInput,
    PlanningMetadata,
    ReadingAndWritingBlocks,
    ValidatingOutput,
    PublishingOutput,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversionProgress {
    pub stage: ConversionStage,
    pub points_processed: u64,
    pub total_points: u64,
    pub bytes_written: u64,
}

struct SourceInfo {
    bounds: SpatialBounds,
    frame: CoordinateFrame,
    point_count: u64,
    record_length: usize,
    point_format: las::point::Format,
    has_crs: bool,
}

struct ConversionPlan {
    metadata: FormatMetadata,
    schema: Vec<AttributeDescriptor>,
    bounds: SpatialBounds,
    frame: CoordinateFrame,
    point_count: u64,
    block_count: u64,
    points_per_block: u64,
    omitted_dimensions: Vec<String>,
    estimated_peak_memory_bytes: u64,
}

#[derive(Debug, Error)]
pub enum ConvertError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("LAS/LAZ error: {0}")]
    Las(#[from] las::Error),
    #[error(transparent)]
    Format(#[from] viewer_format::FormatError),
    #[error(transparent)]
    Spatial(#[from] viewer_core::SpatialError),
    #[error(transparent)]
    CoordinateFrame(#[from] viewer_core::CoordinateFrameError),
    #[error(transparent)]
    Dataset(#[from] viewer_core::DatasetError),
    #[error(transparent)]
    Metadata(#[from] viewer_format::MetadataError),
    #[error("points per block must be between 1 and {MAX_POINTS_PER_BLOCK}, got {0}")]
    InvalidBlockPointLimit(u64),
    #[error("maximum quantization error must be finite and non-negative, got {0}")]
    InvalidQuantizationError(f64),
    #[error("memory budget must be greater than zero")]
    InvalidMemoryBudget,
    #[error("LAS dimensions do not match the planned output attribute schema")]
    InvalidAttributeSchema,
    #[error("estimated controlled memory is {requested} bytes, exceeding budget {limit} bytes")]
    MemoryBudgetExceeded { requested: u64, limit: u64 },
    #[error("conversion cancelled after processing {points_processed} points")]
    Cancelled { points_processed: u64 },
    #[error("LAS/LAZ header bounds are invalid")]
    InvalidHeaderBounds,
    #[error("LAS/LAZ point count requires {0} blocks, exceeding the limit of {MAX_BLOCKS}")]
    TooManyBlocks(u64),
    #[error("LAS/LAZ point data ended after {actual} points; header declared {expected}")]
    PointCountMismatch { expected: u64, actual: u64 },
    #[error("point {point_index} at {position:?} is outside the LAS/LAZ header bounds")]
    PointOutsideHeaderBounds {
        point_index: u64,
        position: [f64; 3],
    },
    #[error("output path already exists: {0}")]
    OutputExists(PathBuf),
    #[error("could not allocate a temporary output path beside {0}")]
    TemporaryPathUnavailable(PathBuf),
}

/// Converts LAS/LAZ to the native v1 format using one root node and sequential blocks.
/// Point payload memory is bounded by the configured block size and LAS record buffer.
pub fn convert_file(
    input_path: &Path,
    output_path: &Path,
    options: ConvertOptions,
) -> Result<ConversionReport, ConvertError> {
    convert_file_with_progress(input_path, output_path, options, |_| true)
}

/// Converts a file and reports progress after each phase and completed block.
/// Return `false` from the callback to cancel before the output is published.
pub fn convert_file_with_progress<F>(
    input_path: &Path,
    output_path: &Path,
    options: ConvertOptions,
    mut on_progress: F,
) -> Result<ConversionReport, ConvertError>
where
    F: FnMut(ConversionProgress) -> bool,
{
    if output_path.exists() {
        return Err(ConvertError::OutputExists(output_path.to_owned()));
    }

    emit_progress(
        &mut on_progress,
        ConversionProgress {
            stage: ConversionStage::InspectingInput,
            points_processed: 0,
            total_points: 0,
            bytes_written: 0,
        },
        0,
    )?;
    let mut reader = Reader::from_path(input_path)?;
    let source = inspect_source(&reader)?;
    emit_progress(
        &mut on_progress,
        ConversionProgress {
            stage: ConversionStage::PlanningMetadata,
            points_processed: 0,
            total_points: source.point_count,
            bytes_written: 0,
        },
        0,
    )?;
    let plan = ConversionPlan::build(source, options)?;
    let mut temporary = create_temporary_output(output_path)?;
    let temporary_path = temporary.path.to_owned();
    let file = temporary
        .file
        .take()
        .expect("temporary output file is present");
    let mut writer = FormatWriter::new(
        file,
        plan.metadata.clone(),
        FormatWriterOptions::new(options.max_quantization_error, 64 * 1024 * 1024)
            .map_err(ConvertError::Format)?,
    )?;
    stream_blocks(
        &mut reader,
        &mut writer,
        &plan,
        &temporary_path,
        &mut on_progress,
    )?;
    let output = writer.finish()?;
    output.sync_all()?;
    drop(output);
    let file_length = fs::metadata(&temporary_path)?.len();
    let output_bytes = publish_output(
        &temporary_path,
        output_path,
        plan.point_count,
        file_length,
        &mut on_progress,
    )?;
    drop(temporary);

    let _ = on_progress(ConversionProgress {
        stage: ConversionStage::Complete,
        points_processed: plan.point_count,
        total_points: plan.point_count,
        bytes_written: output_bytes,
    });

    Ok(ConversionReport {
        point_count: plan.point_count,
        block_count: plan.block_count,
        output_bytes,
        estimated_peak_memory_bytes: plan.estimated_peak_memory_bytes,
        omitted_dimensions: plan.omitted_dimensions,
    })
}

fn inspect_source(reader: &Reader) -> Result<SourceInfo, ConvertError> {
    let header = reader.header();
    let source_bounds = header.bounds();
    let min = [
        source_bounds.min.x,
        source_bounds.min.y,
        source_bounds.min.z,
    ];
    let max = [
        source_bounds.max.x,
        source_bounds.max.y,
        source_bounds.max.z,
    ];
    let bounds = SpatialBounds::new(min, max).map_err(|_| ConvertError::InvalidHeaderBounds)?;
    Ok(SourceInfo {
        bounds,
        frame: CoordinateFrame::new(min)?,
        point_count: header.number_of_points(),
        record_length: usize::from(header.point_format().len()).max(1),
        point_format: *header.point_format(),
        has_crs: header.has_crs_vlrs(),
    })
}

impl ConversionPlan {
    fn build(source: SourceInfo, options: ConvertOptions) -> Result<Self, ConvertError> {
        let max_points_by_input_bytes =
            (MAX_INPUT_BYTES_PER_BATCH / source.record_length).max(1) as u64;
        let points_per_block = options.points_per_block.min(max_points_by_input_bytes);
        let block_count = source.point_count.div_ceil(points_per_block);
        if block_count > MAX_BLOCKS || block_count > u64::from(u32::MAX) {
            return Err(ConvertError::TooManyBlocks(block_count));
        }
        let estimated_peak_memory_bytes = options
            .points_per_block
            .min(max_points_by_input_bytes)
            .checked_mul(ESTIMATED_WORKING_BYTES_PER_POINT)
            .and_then(|working| {
                block_count
                    .checked_mul(ESTIMATED_DIRECTORY_BYTES_PER_BLOCK)
                    .and_then(|directory| working.checked_add(directory))
            })
            .and_then(|total| total.checked_add(ESTIMATED_FIXED_OVERHEAD_BYTES))
            .ok_or(ConvertError::MemoryBudgetExceeded {
                requested: u64::MAX,
                limit: options.max_memory_bytes,
            })?;
        if estimated_peak_memory_bytes > options.max_memory_bytes {
            return Err(ConvertError::MemoryBudgetExceeded {
                requested: estimated_peak_memory_bytes,
                limit: options.max_memory_bytes,
            });
        }

        let block_ids = (0..block_count).map(BlockId::new).collect::<Vec<_>>();
        let root = SpatialNode::new(
            NodeId::new(0),
            None,
            0,
            source.bounds,
            source.point_count,
            0.0,
            block_ids,
        )?;
        let schema = source_schema(&source.point_format);
        let hierarchy =
            SpatialHierarchy::new(vec![root], schema.clone(), Some(source.point_count))?;
        let metadata = FormatMetadata::new(
            DatasetMetadata::new(source.frame, hierarchy),
            None,
            Some("sequential LAS/LAZ import; flat root; no spatial LOD".to_owned()),
            Vec::new(),
        )?;
        let omitted_dimensions = omitted_dimensions(&source.point_format, source.has_crs);

        Ok(Self {
            metadata,
            schema,
            bounds: source.bounds,
            frame: source.frame,
            point_count: source.point_count,
            block_count,
            points_per_block,
            omitted_dimensions,
            estimated_peak_memory_bytes,
        })
    }
}

fn stream_blocks(
    reader: &mut Reader,
    writer: &mut FormatWriter<File>,
    plan: &ConversionPlan,
    temporary_path: &Path,
    on_progress: &mut impl FnMut(ConversionProgress) -> bool,
) -> Result<(), ConvertError> {
    emit_progress(
        on_progress,
        ConversionProgress {
            stage: ConversionStage::ReadingAndWritingBlocks,
            points_processed: 0,
            total_points: plan.point_count,
            bytes_written: fs::metadata(temporary_path)?.len(),
        },
        0,
    )?;
    let mut point_data = PointDataBuilder::new().for_header(reader.header()).build();
    let mut processed = 0_u64;
    while processed < plan.point_count {
        let requested = plan.points_per_block.min(plan.point_count - processed);
        let read = reader.fill_points(requested, &mut point_data)?;
        if read != requested {
            return Err(ConvertError::PointCountMismatch {
                expected: plan.point_count,
                actual: processed + read,
            });
        }
        let count = usize::try_from(read).map_err(|_| ConvertError::PointCountMismatch {
            expected: plan.point_count,
            actual: processed + read,
        })?;
        let positions = point_positions(&point_data, plan.frame, plan.bounds, processed)?;
        let intensities = point_data.intensity().collect::<Vec<_>>();
        let classifications = point_data.classification().collect::<Vec<_>>();
        let return_numbers = point_data.return_number().collect::<Vec<_>>();
        let returns_per_pulse = point_data.number_of_returns().collect::<Vec<_>>();
        let scan_angles = point_data.scan_angle_degrees().collect::<Vec<_>>();
        let user_data = point_data.user_data().collect::<Vec<_>>();
        let point_source_ids = point_data.point_source_id().collect::<Vec<_>>();
        let gps_times = point_data.gps_time().map(Iterator::collect::<Vec<_>>);
        let nir_values = point_data.nir().map(Iterator::collect::<Vec<_>>);
        let (colors, rgb_values) = match point_data.rgb() {
            Some(values) => {
                let mut colors = Vec::with_capacity(count);
                let mut rgb_values = Vec::with_capacity(count.saturating_mul(3));
                for (red, green, blue) in values {
                    colors.push(ColorRgb::new(
                        (red >> 8) as u8,
                        (green >> 8) as u8,
                        (blue >> 8) as u8,
                    ));
                    rgb_values.extend_from_slice(&[red, green, blue]);
                }
                (colors, Some(rgb_values))
            }
            None => {
                let colors = intensities
                    .iter()
                    .map(|intensity| {
                        let gray = (intensity >> 8) as u8;
                        ColorRgb::new(gray, gray, gray)
                    })
                    .collect();
                (colors, None)
            }
        };
        let mut attributes = Vec::with_capacity(plan.schema.len());
        let mut schema_index = 0;
        push_attribute(
            &mut attributes,
            &plan.schema,
            &mut schema_index,
            AttributeValues::U16(intensities),
            count,
        )?;
        push_attribute(
            &mut attributes,
            &plan.schema,
            &mut schema_index,
            AttributeValues::U8(classifications),
            count,
        )?;
        push_attribute(
            &mut attributes,
            &plan.schema,
            &mut schema_index,
            AttributeValues::U8(return_numbers),
            count,
        )?;
        push_attribute(
            &mut attributes,
            &plan.schema,
            &mut schema_index,
            AttributeValues::U8(returns_per_pulse),
            count,
        )?;
        push_attribute(
            &mut attributes,
            &plan.schema,
            &mut schema_index,
            AttributeValues::F32(scan_angles),
            count,
        )?;
        push_attribute(
            &mut attributes,
            &plan.schema,
            &mut schema_index,
            AttributeValues::U8(user_data),
            count,
        )?;
        push_attribute(
            &mut attributes,
            &plan.schema,
            &mut schema_index,
            AttributeValues::U16(point_source_ids),
            count,
        )?;
        if let Some(gps_times) = gps_times {
            push_attribute(
                &mut attributes,
                &plan.schema,
                &mut schema_index,
                AttributeValues::F64(gps_times),
                count,
            )?;
        }
        if let Some(rgb_values) = rgb_values {
            push_attribute(
                &mut attributes,
                &plan.schema,
                &mut schema_index,
                AttributeValues::U16(rgb_values),
                count,
            )?;
        }
        if let Some(nir_values) = nir_values {
            push_attribute(
                &mut attributes,
                &plan.schema,
                &mut schema_index,
                AttributeValues::U16(nir_values),
                count,
            )?;
        }
        writer.write_block(SourcePointBlock::new(
            BlockId::new(processed / plan.points_per_block),
            positions,
            colors,
            attributes,
        ))?;
        processed += read;
        emit_progress(
            on_progress,
            ConversionProgress {
                stage: ConversionStage::ReadingAndWritingBlocks,
                points_processed: processed,
                total_points: plan.point_count,
                bytes_written: fs::metadata(temporary_path)?.len(),
            },
            processed,
        )?;
    }
    Ok(())
}

fn push_attribute(
    attributes: &mut Vec<AttributeColumn>,
    schema: &[AttributeDescriptor],
    schema_index: &mut usize,
    values: AttributeValues,
    point_count: usize,
) -> Result<(), ConvertError> {
    let descriptor = schema
        .get(*schema_index)
        .ok_or(ConvertError::InvalidAttributeSchema)?
        .clone();
    attributes.push(AttributeColumn::new(descriptor, values, point_count)?);
    *schema_index += 1;
    Ok(())
}

fn source_schema(format: &las::point::Format) -> Vec<AttributeDescriptor> {
    let mut schema = vec![
        descriptor("intensity", AttributeDataType::U16, 1),
        descriptor("classification", AttributeDataType::U8, 1),
        descriptor("return_number", AttributeDataType::U8, 1),
        descriptor("number_of_returns", AttributeDataType::U8, 1),
        descriptor("scan_angle_degrees", AttributeDataType::F32, 1),
        descriptor("user_data", AttributeDataType::U8, 1),
        descriptor("point_source_id", AttributeDataType::U16, 1),
    ];
    if format.has_gps_time {
        schema.push(descriptor("gps_time", AttributeDataType::F64, 1));
    }
    if format.has_color {
        schema.push(descriptor("rgb", AttributeDataType::U16, 3));
    }
    if format.has_nir {
        schema.push(descriptor("nir", AttributeDataType::U16, 1));
    }
    schema
}

fn descriptor(name: &str, data_type: AttributeDataType, components: u32) -> AttributeDescriptor {
    AttributeDescriptor::new(name, data_type, components).expect("static attribute schema is valid")
}

fn point_positions(
    data: &PointData,
    frame: CoordinateFrame,
    bounds: SpatialBounds,
    processed: u64,
) -> Result<Vec<[f64; 3]>, ConvertError> {
    let mut positions = Vec::with_capacity(data.len());
    let coordinates = data.x().zip(data.y()).zip(data.z());
    for (offset, ((x, y), z)) in coordinates.enumerate() {
        let world = [x, y, z];
        let local = std::array::from_fn(|axis| world[axis] - frame.origin()[axis]);
        if world.iter().any(|coordinate| !coordinate.is_finite())
            || local.iter().any(|coordinate| !coordinate.is_finite())
        {
            return Err(ConvertError::CoordinateFrame(
                viewer_core::CoordinateFrameError::PositionOutOfRange(local),
            ));
        }
        if (0..3).any(|axis| world[axis] < bounds.min()[axis] || world[axis] > bounds.max()[axis]) {
            return Err(ConvertError::PointOutsideHeaderBounds {
                point_index: processed + offset as u64,
                position: world,
            });
        }
        positions.push(local);
    }
    Ok(positions)
}

fn omitted_dimensions(format: &las::point::Format, has_crs: bool) -> Vec<String> {
    let mut dimensions = vec![
        "synthetic/key-point/withheld/overlap flags".to_owned(),
        "scan direction and flight-line edge flags".to_owned(),
    ];
    if format.is_extended {
        dimensions.push("scanner channel".to_owned());
    }
    if format.has_waveform {
        dimensions.push("waveform data".to_owned());
    }
    if format.extra_bytes > 0 {
        dimensions.push("extra point bytes".to_owned());
    }
    if has_crs {
        dimensions.push("CRS/WKT metadata".to_owned());
    }
    dimensions
}

fn validate_output(
    path: &Path,
    total_points: u64,
    file_length: u64,
    on_progress: &mut impl FnMut(ConversionProgress) -> bool,
) -> Result<(), ConvertError> {
    let mut reader = FormatReader::new(File::open(path)?)?;
    let root = reader
        .metadata()
        .hierarchy()
        .node(NodeId::new(0))
        .expect("converter emits the root node");
    let block_ids = root.blocks().to_vec();
    emit_progress(
        on_progress,
        ConversionProgress {
            stage: ConversionStage::ValidatingOutput,
            points_processed: 0,
            total_points,
            bytes_written: file_length,
        },
        0,
    )?;
    let mut validated_points = 0;
    for block_id in block_ids {
        validated_points += reader.read_block(block_id)?.points().len() as u64;
        emit_progress(
            on_progress,
            ConversionProgress {
                stage: ConversionStage::ValidatingOutput,
                points_processed: validated_points,
                total_points,
                bytes_written: file_length,
            },
            validated_points,
        )?;
    }
    Ok(())
}

fn publish_output(
    temporary_path: &Path,
    output_path: &Path,
    point_count: u64,
    file_length: u64,
    on_progress: &mut impl FnMut(ConversionProgress) -> bool,
) -> Result<u64, ConvertError> {
    validate_output(temporary_path, point_count, file_length, on_progress)?;
    let output_bytes = fs::metadata(temporary_path)?.len();
    emit_progress(
        on_progress,
        ConversionProgress {
            stage: ConversionStage::PublishingOutput,
            points_processed: point_count,
            total_points: point_count,
            bytes_written: output_bytes,
        },
        point_count,
    )?;
    fs::hard_link(temporary_path, output_path).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists || output_path.exists() {
            ConvertError::OutputExists(output_path.to_owned())
        } else {
            ConvertError::Io(error)
        }
    })?;
    Ok(output_bytes)
}

fn emit_progress(
    on_progress: &mut impl FnMut(ConversionProgress) -> bool,
    progress: ConversionProgress,
    points_processed: u64,
) -> Result<(), ConvertError> {
    if on_progress(progress) {
        Ok(())
    } else {
        Err(ConvertError::Cancelled { points_processed })
    }
}

struct TemporaryOutput {
    path: PathBuf,
    file: Option<File>,
}

impl Drop for TemporaryOutput {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

fn create_temporary_output(output_path: &Path) -> Result<TemporaryOutput, ConvertError> {
    let parent = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = output_path
        .file_name()
        .ok_or_else(|| ConvertError::TemporaryPathUnavailable(output_path.to_owned()))?;
    for _ in 0..100 {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = file_name.to_os_string();
        temporary_name.push(format!(".{}.{}.tmp", std::process::id(), counter));
        let path = parent.join(temporary_name);
        match OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                return Ok(TemporaryOutput {
                    path,
                    file: Some(file),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(ConvertError::Io(error)),
        }
    }
    Err(ConvertError::TemporaryPathUnavailable(
        output_path.to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use las::{Builder, Color, Point, Writer, point::Format};
    use viewer_core::{AttributeValues, BlockId, DatasetSource, NodeId};
    use viewer_format::FormatReader;

    use super::{
        ConversionStage, ConvertError, ConvertOptions, convert_file, convert_file_with_progress,
    };

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "viewer-convert-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn fixture(path: &std::path::Path, compressed: bool, colored: bool, count: usize) {
        let mut builder = Builder::from((1, 2));
        builder.point_format = Format::new(if colored { 2 } else { 0 }).unwrap();
        builder.point_format.is_compressed = compressed;
        let header = builder.into_header().unwrap();
        let mut writer = Writer::from_path(path, header).unwrap();
        for index in 0..count {
            let mut point = Point {
                x: 1_000_000.0 + index as f64 * 0.25,
                y: 2_000_000.0 + (index % 3) as f64,
                z: 50.0 + (index % 5) as f64,
                intensity: (index as u16).wrapping_mul(10_000),
                return_number: 2,
                number_of_returns: 3,
                classification: las::point::Classification::Ground,
                scan_angle: index as f32 - 1.0,
                user_data: index as u8 + 7,
                point_source_id: 100 + index as u16,
                ..Point::default()
            };
            if colored {
                point.color = Some(Color::new(0x1200, 0xAB00, 0xFF00));
            }
            writer.write_point(point).unwrap();
        }
    }

    fn extended_fixture(path: &std::path::Path) {
        let mut builder = Builder::from((1, 4));
        builder.point_format = Format::new(8).unwrap();
        let header = builder.into_header().unwrap();
        let mut writer = Writer::from_path(path, header).unwrap();
        writer
            .write_point(Point {
                x: 10.0,
                y: 20.0,
                z: 30.0,
                intensity: 1234,
                return_number: 2,
                number_of_returns: 3,
                classification: las::point::Classification::Ground,
                scan_angle: -1.25,
                user_data: 9,
                point_source_id: 77,
                gps_time: Some(123.5),
                color: Some(Color::new(0x1200, 0xAB00, 0xFF00)),
                nir: Some(0xCAFE),
                ..Point::default()
            })
            .unwrap();
    }

    #[test]
    fn converts_laz_in_bounded_blocks_and_preserves_intensity_and_rgb16() {
        let dir = test_dir();
        let input = dir.join("source.laz");
        let output = dir.join("result.oct");
        fixture(&input, true, true, 5);
        let mut progress = Vec::new();
        let report = convert_file_with_progress(
            &input,
            &output,
            ConvertOptions::new(2, 0.01).unwrap(),
            |event| {
                progress.push(event);
                true
            },
        )
        .unwrap();
        assert_eq!(report.point_count, 5);
        assert_eq!(report.block_count, 3);
        assert!(report.estimated_peak_memory_bytes <= 512 * 1024 * 1024);
        assert!(
            !report
                .omitted_dimensions
                .contains(&"classification".to_owned())
        );
        assert!(
            report
                .omitted_dimensions
                .iter()
                .any(|name| name.contains("flags"))
        );
        assert_eq!(
            progress.first().unwrap().stage,
            ConversionStage::InspectingInput
        );
        assert!(progress.iter().any(|event| {
            event.stage == ConversionStage::ReadingAndWritingBlocks
                && event.points_processed == report.point_count
                && event.bytes_written > 24
        }));
        let completed = progress.last().unwrap();
        assert_eq!(completed.stage, ConversionStage::Complete);
        assert_eq!(completed.bytes_written, report.output_bytes);

        let mut reader = FormatReader::new(fs::File::open(output).unwrap()).unwrap();
        let hierarchy = reader.metadata().hierarchy();
        assert_eq!(hierarchy.node(NodeId::new(0)).unwrap().blocks().len(), 3);
        for (index, block_id) in [BlockId::new(0), BlockId::new(1), BlockId::new(2)]
            .into_iter()
            .enumerate()
        {
            let block = reader.read_block(block_id).unwrap();
            let intensity = block.attributes()[0].values();
            let AttributeValues::U16(values) = intensity else {
                panic!("intensity must remain u16");
            };
            assert_eq!(values[0], (index as u16 * 2).wrapping_mul(10_000));
            let AttributeValues::U8(classification) = block.attributes()[1].values() else {
                panic!("classification must remain u8");
            };
            assert_eq!(classification, &[2, 2][..block.points().len()]);
            let AttributeValues::U8(return_numbers) = block.attributes()[2].values() else {
                panic!("return number must remain u8");
            };
            assert!(return_numbers.iter().all(|value| *value == 2));
            let AttributeValues::U8(number_of_returns) = block.attributes()[3].values() else {
                panic!("number of returns must remain u8");
            };
            assert!(number_of_returns.iter().all(|value| *value == 3));
            let AttributeValues::U8(user_data) = block.attributes()[5].values() else {
                panic!("user data must remain u8");
            };
            assert_eq!(user_data[0], index as u8 * 2 + 7);
            let AttributeValues::U16(source_ids) = block.attributes()[6].values() else {
                panic!("point source ID must remain u16");
            };
            assert_eq!(source_ids[0], 100 + (index as u16 * 2));
            let AttributeValues::U16(rgb) = block.attributes()[7].values() else {
                panic!("RGB must remain u16");
            };
            assert_eq!(&rgb[..3], &[0x1200, 0xAB00, 0xFF00]);
            assert_eq!(block.points().points()[0].color.r, 0x12);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn converts_las_without_rgb_and_uses_intensity_for_display_color() {
        let dir = test_dir();
        let input = dir.join("source.las");
        let output = dir.join("result.oct");
        fixture(&input, false, false, 3);
        let report = convert_file(&input, &output, ConvertOptions::new(2, 0.01).unwrap()).unwrap();
        assert_eq!(report.block_count, 2);

        let mut reader = FormatReader::new(fs::File::open(output).unwrap()).unwrap();
        let last = reader.read_block(BlockId::new(1)).unwrap();
        let AttributeValues::U16(intensity) = last.attributes()[0].values() else {
            panic!("intensity must remain u16");
        };
        assert_eq!(intensity, &[20_000]);
        assert_eq!(
            last.points().points()[0].color,
            viewer_core::ColorRgb::new(78, 78, 78)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preserves_optional_gps_time_rgb_and_nir_dimensions() {
        let dir = test_dir();
        let input = dir.join("extended.las");
        let output = dir.join("result.oct");
        extended_fixture(&input);
        let report = convert_file(&input, &output, ConvertOptions::default()).unwrap();
        assert!(
            report
                .omitted_dimensions
                .iter()
                .any(|name| name == "scanner channel")
        );

        let mut reader = FormatReader::new(fs::File::open(output).unwrap()).unwrap();
        let block = reader.read_block(BlockId::new(0)).unwrap();
        let value = |name: &str| {
            block
                .attributes()
                .iter()
                .find(|column| column.descriptor().name() == name)
                .unwrap()
                .values()
        };
        let AttributeValues::F64(gps) = value("gps_time") else {
            panic!("GPS time must remain f64");
        };
        assert_eq!(gps, &[123.5]);
        let AttributeValues::U16(nir) = value("nir") else {
            panic!("NIR must remain u16");
        };
        assert_eq!(nir, &[0xCAFE]);
        let AttributeValues::U16(rgb) = value("rgb") else {
            panic!("RGB must remain u16");
        };
        assert_eq!(rgb, &[0x1200, 0xAB00, 0xFF00]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keeps_existing_outputs_unchanged_and_cleans_temporary_files_on_failure() {
        let dir = test_dir();
        let input = dir.join("source.las");
        let output = dir.join("result.oct");
        fixture(&input, false, false, 3);
        fs::write(&output, b"existing").unwrap();
        assert!(matches!(
            convert_file(&input, &output, ConvertOptions::default()),
            Err(ConvertError::OutputExists(_))
        ));
        assert_eq!(fs::read(&output).unwrap(), b"existing");

        fs::remove_file(&output).unwrap();
        let mut truncated = fs::read(&input).unwrap();
        truncated.pop();
        fs::write(&input, truncated).unwrap();
        assert!(convert_file(&input, &output, ConvertOptions::default()).is_err());
        assert!(!output.exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn memory_budget_rejects_the_plan_before_creating_an_output() {
        let dir = test_dir();
        let input = dir.join("source.las");
        let output = dir.join("result.oct");
        fixture(&input, false, false, 3);
        let options = ConvertOptions::with_memory_budget(2, 0.01, 1).unwrap();

        assert!(matches!(
            convert_file(&input, &output, options),
            Err(ConvertError::MemoryBudgetExceeded { limit: 1, .. })
        ));
        assert!(!output.exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancellation_removes_the_temporary_file_without_publishing_output() {
        let dir = test_dir();
        let input = dir.join("source.las");
        let output = dir.join("result.oct");
        fixture(&input, false, false, 5);
        let mut cancelled = false;
        let result = convert_file_with_progress(
            &input,
            &output,
            ConvertOptions::new(2, 0.01).unwrap(),
            |event| {
                if event.stage == ConversionStage::ReadingAndWritingBlocks
                    && event.points_processed >= 2
                {
                    cancelled = true;
                    false
                } else {
                    true
                }
            },
        );

        assert!(cancelled);
        assert!(matches!(
            result,
            Err(ConvertError::Cancelled {
                points_processed: 2
            })
        ));
        assert!(!output.exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
