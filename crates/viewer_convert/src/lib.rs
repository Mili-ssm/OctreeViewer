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
const ESTIMATED_WORKING_BYTES_PER_POINT: u64 = 128;
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

struct SourceInfo {
    bounds: SpatialBounds,
    frame: CoordinateFrame,
    point_count: u64,
    record_length: usize,
    point_format: las::point::Format,
    has_rgb: bool,
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
    #[error("estimated controlled memory is {requested} bytes, exceeding budget {limit} bytes")]
    MemoryBudgetExceeded { requested: u64, limit: u64 },
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
    if output_path.exists() {
        return Err(ConvertError::OutputExists(output_path.to_owned()));
    }

    let mut reader = Reader::from_path(input_path)?;
    let source = inspect_source(&reader)?;
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
    stream_blocks(&mut reader, &mut writer, &plan)?;
    let output = writer.finish()?;
    output.sync_all()?;
    drop(output);
    let output_bytes = publish_output(&temporary_path, output_path)?;
    drop(temporary);

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
        has_rgb: header.point_format().has_color,
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
        let schema = source_schema(source.has_rgb);
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
) -> Result<(), ConvertError> {
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
        attributes.push(AttributeColumn::new(
            plan.schema[0].clone(),
            AttributeValues::U16(intensities),
            count,
        )?);
        if let Some(rgb_values) = rgb_values {
            attributes.push(AttributeColumn::new(
                plan.schema[1].clone(),
                AttributeValues::U16(rgb_values),
                count,
            )?);
        }
        writer.write_block(SourcePointBlock::new(
            BlockId::new(processed / plan.points_per_block),
            positions,
            colors,
            attributes,
        ))?;
        processed += read;
    }
    Ok(())
}

fn source_schema(has_rgb: bool) -> Vec<AttributeDescriptor> {
    let mut schema = vec![
        AttributeDescriptor::new("intensity", AttributeDataType::U16, 1)
            .expect("static attribute schema is valid"),
    ];
    if has_rgb {
        schema.push(
            AttributeDescriptor::new("rgb", AttributeDataType::U16, 3)
                .expect("static attribute schema is valid"),
        );
    }
    schema
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
        "classification".to_owned(),
        "return number and number of returns".to_owned(),
        "scan angle".to_owned(),
        "user data".to_owned(),
        "point source ID".to_owned(),
    ];
    if format.has_gps_time {
        dimensions.push("GPS time".to_owned());
    }
    if format.has_nir {
        dimensions.push("near-infrared channel".to_owned());
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

fn validate_output(path: &Path) -> Result<(), ConvertError> {
    let mut reader = FormatReader::new(File::open(path)?)?;
    let root = reader
        .metadata()
        .hierarchy()
        .node(NodeId::new(0))
        .expect("converter emits the root node");
    let block_ids = root.blocks().to_vec();
    for block_id in block_ids {
        reader.read_block(block_id)?;
    }
    Ok(())
}

fn publish_output(temporary_path: &Path, output_path: &Path) -> Result<u64, ConvertError> {
    validate_output(temporary_path)?;
    let output_bytes = fs::metadata(temporary_path)?.len();
    fs::hard_link(temporary_path, output_path).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists || output_path.exists() {
            ConvertError::OutputExists(output_path.to_owned())
        } else {
            ConvertError::Io(error)
        }
    })?;
    Ok(output_bytes)
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

    use super::{ConvertError, ConvertOptions, convert_file};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "viewer-convert-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
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
                ..Point::default()
            };
            if colored {
                point.color = Some(Color::new(0x1200, 0xAB00, 0xFF00));
            }
            writer.write_point(point).unwrap();
        }
    }

    #[test]
    fn converts_laz_in_bounded_blocks_and_preserves_intensity_and_rgb16() {
        let dir = test_dir();
        let input = dir.join("source.laz");
        let output = dir.join("result.oct");
        fixture(&input, true, true, 5);
        let report = convert_file(&input, &output, ConvertOptions::new(2, 0.01).unwrap()).unwrap();
        assert_eq!(report.point_count, 5);
        assert_eq!(report.block_count, 3);
        assert!(report.estimated_peak_memory_bytes <= 512 * 1024 * 1024);
        assert!(
            report
                .omitted_dimensions
                .contains(&"classification".to_owned())
        );

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
            let AttributeValues::U16(rgb) = block.attributes()[1].values() else {
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
}
