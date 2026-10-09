use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use las::Reader;
use thiserror::Error;
use viewer_core::{
    AttributeDataType, AttributeDescriptor, BlockId, CoordinateFrame, DatasetSource, SpatialBounds,
};
use viewer_format::{FormatMetadata, FormatReader, FormatWriter, FormatWriterOptions};

mod checkpoint;
mod ept;
mod spatial;
use checkpoint::{Checkpoint, CheckpointStage, fingerprint_file};
pub use checkpoint::{CheckpointError, CheckpointMode};
use spatial::{PlannedBlock, SpatialPlan, Workspace, build_spatial_plan, spool_source};

const DEFAULT_POINTS_PER_BLOCK: u64 = 262_144;
const DEFAULT_MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
const MAX_POINTS_PER_BLOCK: u64 = 1_000_000;
const MAX_SPATIAL_WORKERS: usize = 4;
const ALLOWED_SPATIAL_DIVISIONS: [u8; 3] = [2, 4, 8];
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
    spatial_workers: usize,
    spatial_divisions: u8,
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
            spatial_workers: 1,
            spatial_divisions: 2,
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

    pub fn with_spatial_workers(mut self, workers: usize) -> Result<Self, ConvertError> {
        if !(1..=MAX_SPATIAL_WORKERS).contains(&workers) {
            return Err(ConvertError::InvalidSpatialWorkerCount(workers));
        }
        self.spatial_workers = workers;
        Ok(self)
    }

    pub const fn spatial_workers(self) -> usize {
        self.spatial_workers
    }

    pub fn with_spatial_divisions(mut self, divisions: u8) -> Result<Self, ConvertError> {
        if !ALLOWED_SPATIAL_DIVISIONS.contains(&divisions) {
            return Err(ConvertError::InvalidSpatialDivisions(divisions));
        }
        self.spatial_divisions = divisions;
        Ok(self)
    }

    pub const fn spatial_divisions(self) -> u8 {
        self.spatial_divisions
    }
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            points_per_block: DEFAULT_POINTS_PER_BLOCK,
            max_quantization_error: 0.01,
            max_memory_bytes: DEFAULT_MEMORY_BUDGET_BYTES,
            spatial_workers: 1,
            spatial_divisions: 2,
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
    PlanningSpatialIndex,
    BuildingSpatialHierarchy,
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
    frame: CoordinateFrame,
    point_count: u64,
    block_count: u64,
    blocks: Vec<PlannedBlock>,
    source_block_ids: Vec<BlockId>,
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
    #[error("spatial worker count must be between 1 and {MAX_SPATIAL_WORKERS}, got {0}")]
    InvalidSpatialWorkerCount(usize),
    #[error("spatial divisions per axis must be 2, 4, or 8, got {0}")]
    InvalidSpatialDivisions(u8),
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
    #[error("unsupported EPT dataset: {0}")]
    UnsupportedEpt(String),
    #[error("invalid EPT dataset: {0}")]
    InvalidEpt(String),
    #[error("conversion size or identifier count overflowed")]
    SizeOverflow,
    #[error("point {point_index} at {position:?} is outside the LAS/LAZ header bounds")]
    PointOutsideHeaderBounds {
        point_index: u64,
        position: [f64; 3],
    },
    #[error("output path already exists: {0}")]
    OutputExists(PathBuf),
    #[error("could not allocate a temporary output path beside {0}")]
    TemporaryPathUnavailable(PathBuf),
    #[error(transparent)]
    Checkpoint(#[from] CheckpointError),
}

/// Converts LAS/LAZ or a local laszip EPT dataset to the native format.
/// LAS/LAZ output uses replacement hierarchy samples; EPT output preserves its additive hierarchy.
/// Point payload memory is bounded by the configured block size and spatial-index workspace.
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
    convert_file_inner(input_path, output_path, options, None, &mut on_progress)
}

/// Converts a file while retaining completed phase checkpoints for explicit resume.
pub fn convert_file_with_checkpoint<F>(
    input_path: &Path,
    output_path: &Path,
    options: ConvertOptions,
    mode: CheckpointMode,
    mut on_progress: F,
) -> Result<ConversionReport, ConvertError>
where
    F: FnMut(ConversionProgress) -> bool,
{
    convert_file_inner(
        input_path,
        output_path,
        options,
        Some(mode),
        &mut on_progress,
    )
}

fn convert_file_inner(
    input_path: &Path,
    output_path: &Path,
    options: ConvertOptions,
    checkpoint_mode: Option<CheckpointMode>,
    on_progress: &mut impl FnMut(ConversionProgress) -> bool,
) -> Result<ConversionReport, ConvertError> {
    if ept::is_ept_input(input_path) {
        if checkpoint_mode.is_some() {
            return Err(ConvertError::UnsupportedEpt(
                "--checkpoint and --resume are not supported for EPT inputs".to_owned(),
            ));
        }
        return ept::convert(input_path, output_path, options, on_progress);
    }
    if output_path.exists() && checkpoint_mode != Some(CheckpointMode::Resume) {
        return Err(ConvertError::OutputExists(output_path.to_owned()));
    }

    emit_progress(
        on_progress,
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
    let mut checkpoint = if let Some(mode) = checkpoint_mode {
        let source_fingerprint = fingerprint_file(input_path)?;
        Some(Checkpoint::open(
            output_path,
            mode,
            source_fingerprint,
            options.points_per_block(),
            options.max_quantization_error(),
            options.max_memory_budget_bytes(),
            options.spatial_divisions(),
        )?)
    } else {
        None
    };
    if output_path.exists()
        && checkpoint.as_ref().is_none_or(|checkpoint| {
            !matches!(checkpoint.stage(), CheckpointStage::OutputReady { .. })
        })
    {
        return Err(ConvertError::OutputExists(output_path.to_owned()));
    }
    emit_progress(
        on_progress,
        ConversionProgress {
            stage: ConversionStage::PlanningMetadata,
            points_processed: 0,
            total_points: source.point_count,
            bytes_written: 0,
        },
        0,
    )?;
    if checkpoint
        .as_ref()
        .is_some_and(|checkpoint| matches!(checkpoint.stage(), CheckpointStage::OutputReady { .. }))
    {
        let checkpoint = checkpoint.expect("output-ready conversion has a checkpoint");
        checkpoint.verify_output()?;
        checkpoint.verify_input(input_path)?;
        let (candidate_fingerprint, point_count, block_count, estimated_memory_bytes) = checkpoint
            .output_ready_details()
            .expect("output-ready checkpoint has report details");
        let candidate_path = checkpoint.candidate_path();
        let (block_ids, source_block_ids) = output_block_ids(&candidate_path)?;
        let candidate_length = candidate_fingerprint.bytes;
        let output_bytes = if output_path.exists() {
            if fingerprint_file(output_path)? != candidate_fingerprint {
                return Err(ConvertError::OutputExists(output_path.to_owned()));
            }
            validate_output(
                output_path,
                point_count,
                &block_ids,
                &source_block_ids,
                candidate_length,
                on_progress,
            )?;
            candidate_length
        } else {
            publish_output(
                &candidate_path,
                output_path,
                point_count,
                &block_ids,
                &source_block_ids,
                candidate_length,
                on_progress,
            )?
        };
        checkpoint.complete()?;
        let _ = on_progress(ConversionProgress {
            stage: ConversionStage::Complete,
            points_processed: point_count,
            total_points: point_count,
            bytes_written: output_bytes,
        });
        return Ok(ConversionReport {
            point_count,
            block_count,
            output_bytes,
            estimated_peak_memory_bytes: estimated_memory_bytes,
            omitted_dimensions: omitted_dimensions(&source.point_format, source.has_crs),
        });
    }

    let workspace = if let Some(checkpoint) = checkpoint.as_ref() {
        Workspace::create_at(&checkpoint.root().join("plan"))?
    } else {
        Workspace::create(output_path)?
    };
    let source_spool = if let Some(checkpoint) = checkpoint.as_mut() {
        match checkpoint.stage() {
            CheckpointStage::Started => {
                let path = checkpoint.source_path();
                spool_source(&mut reader, &source, options, &path, on_progress)?;
                checkpoint.commit_source()?;
            }
            CheckpointStage::SourceReady(_) => checkpoint.verify_source()?,
            CheckpointStage::OutputReady { .. } => {
                unreachable!("output-ready checkpoint was handled above");
            }
        }
        let path = workspace.file("source.spool");
        if matches!(checkpoint.stage(), CheckpointStage::SourceReady(_)) {
            link_or_copy(&checkpoint.source_path(), &path)?;
        }
        path
    } else {
        let path = workspace.file("source.spool");
        spool_source(&mut reader, &source, options, &path, on_progress)?;
        path
    };
    let spatial_plan =
        build_spatial_plan(&source, options, &workspace, &source_spool, on_progress)?;
    let plan = ConversionPlan::from_spatial(spatial_plan, source.frame);
    let mut temporary = if checkpoint.is_none() {
        Some(create_temporary_output(output_path)?)
    } else {
        None
    };
    let temporary_path = if let Some(checkpoint) = checkpoint.as_ref() {
        let candidate = checkpoint.candidate_path();
        let _ = fs::remove_file(&candidate);
        candidate
    } else {
        temporary
            .as_ref()
            .expect("non-checkpoint conversion has a temporary output")
            .path
            .to_owned()
    };
    let file = if let Some(temporary) = temporary.as_mut() {
        temporary
            .file
            .take()
            .expect("temporary output file is present")
    } else {
        OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&temporary_path)?
    };
    let mut writer = FormatWriter::new(
        file,
        plan.metadata.clone(),
        FormatWriterOptions::new(options.max_quantization_error, 64 * 1024 * 1024)
            .map_err(ConvertError::Format)?,
    )?;
    stream_blocks(&mut writer, &plan, &temporary_path, on_progress)?;
    let output = writer.finish()?;
    output.sync_all()?;
    drop(output);
    let file_length = fs::metadata(&temporary_path)?.len();
    if let Some(checkpoint) = checkpoint.as_mut() {
        checkpoint.verify_input(input_path)?;
        checkpoint.commit_output(
            plan.point_count,
            plan.block_count,
            plan.estimated_peak_memory_bytes,
        )?;
    }
    let output_bytes = publish_output(
        &temporary_path,
        output_path,
        plan.point_count,
        &plan.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
        &plan.source_block_ids,
        file_length,
        on_progress,
    )?;
    drop(temporary);
    if let Some(checkpoint) = checkpoint {
        checkpoint.complete()?;
    }

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
    fn from_spatial(spatial: SpatialPlan, frame: CoordinateFrame) -> Self {
        let block_count = spatial.blocks.len() as u64;
        let source_block_ids = spatial
            .blocks
            .iter()
            .filter(|block| block.is_source_points)
            .map(|block| block.id)
            .collect();
        Self {
            metadata: spatial.metadata,
            frame,
            point_count: spatial.point_count,
            block_count,
            blocks: spatial.blocks,
            source_block_ids,
            omitted_dimensions: spatial.omitted_dimensions,
            estimated_peak_memory_bytes: spatial.estimated_peak_memory_bytes,
        }
    }
}

fn stream_blocks(
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
    let mut processed = 0_u64;
    for block in &plan.blocks {
        writer.write_block(block.read(plan.frame)?)?;
        if block.is_source_points {
            processed += block.point_count() as u64;
        }
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
    if processed != plan.point_count {
        return Err(ConvertError::PointCountMismatch {
            expected: plan.point_count,
            actual: processed,
        });
    }
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

fn output_block_ids(path: &Path) -> Result<(Vec<BlockId>, Vec<BlockId>), ConvertError> {
    let reader = FormatReader::new(File::open(path)?)?;
    let block_ids = reader
        .block_storage_stats()?
        .into_iter()
        .map(|block| block.id)
        .collect();
    let hierarchy = reader.format_metadata().dataset().hierarchy();
    let nodes = hierarchy
        .query(hierarchy.stats().root_bounds)
        .collect::<Vec<_>>();
    let parent_ids = nodes
        .iter()
        .filter_map(|node| node.parent())
        .collect::<HashSet<_>>();
    let source_block_ids = nodes
        .iter()
        .filter(|node| !parent_ids.contains(&node.id()))
        .flat_map(|node| node.blocks().iter().copied())
        .collect();
    Ok((block_ids, source_block_ids))
}

fn link_or_copy(source: &Path, destination: &Path) -> Result<(), ConvertError> {
    match fs::hard_link(source, destination) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(source, destination)?;
            Ok(())
        }
    }
}

fn descriptor(name: &str, data_type: AttributeDataType, components: u32) -> AttributeDescriptor {
    AttributeDescriptor::new(name, data_type, components).expect("static attribute schema is valid")
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
    block_ids: &[BlockId],
    source_block_ids: &[BlockId],
    file_length: u64,
    on_progress: &mut impl FnMut(ConversionProgress) -> bool,
) -> Result<(), ConvertError> {
    let mut reader = FormatReader::new(File::open(path)?)?;
    let source_blocks = source_block_ids.iter().copied().collect::<HashSet<_>>();
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
        let block_points = reader.read_block(*block_id)?.points().len() as u64;
        if source_blocks.contains(block_id) {
            validated_points += block_points;
        }
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
    if validated_points != total_points {
        return Err(ConvertError::PointCountMismatch {
            expected: total_points,
            actual: validated_points,
        });
    }
    Ok(())
}

fn publish_output(
    temporary_path: &Path,
    output_path: &Path,
    point_count: u64,
    block_ids: &[BlockId],
    source_block_ids: &[BlockId],
    file_length: u64,
    on_progress: &mut impl FnMut(ConversionProgress) -> bool,
) -> Result<u64, ConvertError> {
    validate_output(
        temporary_path,
        point_count,
        block_ids,
        source_block_ids,
        file_length,
        on_progress,
    )?;
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
    let parent = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    File::open(parent)?.sync_all()?;
    Ok(output_bytes)
}

fn emit_progress(
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
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
        io::{Read, Seek, SeekFrom, Write},
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use las::{Builder, Color, Point, Writer, point::Format};
    use viewer_core::{AttributeValues, BlockId, DatasetSource, NodeId};
    use viewer_format::FormatReader;

    use super::{
        CheckpointError, CheckpointMode, ConversionStage, ConvertError, ConvertOptions,
        convert_file, convert_file_with_checkpoint, convert_file_with_progress,
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
        assert!(report.block_count >= 4);
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
        let (root_block_ids, root_error, node_count) = {
            let hierarchy = reader.metadata().hierarchy();
            let root = hierarchy.node(NodeId::new(0)).unwrap();
            (
                root.blocks().to_vec(),
                root.geometric_error(),
                hierarchy.stats().node_count,
            )
        };
        assert_eq!(root_block_ids.len(), 1);
        assert!(root_error > 0.0);
        assert!(node_count > 1);
        let root_sample = reader.read_block(root_block_ids[0]).unwrap();
        assert!(root_sample.points().len() <= 512);
        for index in 0..5 {
            let source_position = [index as f64 * 0.25, (index % 3) as f64, (index % 5) as f64];
            let nearest_distance = root_sample
                .points()
                .points()
                .iter()
                .map(|sample| {
                    (0..3)
                        .map(|axis| {
                            (source_position[axis] - f64::from(sample.position[axis])).powi(2)
                        })
                        .sum::<f64>()
                        .sqrt()
                })
                .fold(f64::INFINITY, f64::min);
            assert!(nearest_distance <= root_error);
        }
        let mut all_source_ids = std::collections::BTreeSet::new();
        let mut source_values = [false; 5];
        for block_number in 0..report.block_count {
            let block_id = BlockId::new(block_number);
            let block = reader.read_block(block_id).unwrap();
            let intensity = block.attributes()[0].values();
            let AttributeValues::U16(values) = intensity else {
                panic!("intensity must remain u16");
            };
            let AttributeValues::U8(classification) = block.attributes()[1].values() else {
                panic!("classification must remain u8");
            };
            assert!(classification.iter().all(|value| *value == 2));
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
            let AttributeValues::U16(source_ids) = block.attributes()[6].values() else {
                panic!("point source ID must remain u16");
            };
            for ((source_id, intensity), user_data) in source_ids.iter().zip(values).zip(user_data)
            {
                all_source_ids.insert(*source_id);
                let index = usize::from(*source_id - 100);
                if index < source_values.len()
                    && *intensity == index as u16 * 10_000
                    && *user_data == index as u8 + 7
                {
                    source_values[index] = true;
                }
            }
            let AttributeValues::U16(rgb) = block.attributes()[7].values() else {
                panic!("RGB must remain u16");
            };
            let (colors, remainder) = rgb.as_chunks::<3>();
            assert!(remainder.is_empty());
            assert!(
                colors
                    .iter()
                    .all(|color| color == &[0x1200, 0xAB00, 0xFF00])
            );
            assert!(
                block
                    .points()
                    .points()
                    .iter()
                    .all(|point| point.color == viewer_core::ColorRgb::new(0x12, 0xAB, 0xFF))
            );
        }
        assert_eq!(all_source_ids, (100..105).collect());
        assert!(source_values.into_iter().all(|found| found));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn converts_las_without_rgb_and_uses_intensity_for_display_color() {
        let dir = test_dir();
        let input = dir.join("source.las");
        let output = dir.join("result.oct");
        fixture(&input, false, false, 3);
        let report = convert_file(&input, &output, ConvertOptions::new(2, 0.01).unwrap()).unwrap();

        let mut reader = FormatReader::new(fs::File::open(output).unwrap()).unwrap();
        let mut found_last_point = false;
        for block_number in 0..report.block_count {
            let block = reader.read_block(BlockId::new(block_number)).unwrap();
            let AttributeValues::U16(intensity) = block.attributes()[0].values() else {
                panic!("intensity must remain u16");
            };
            if intensity.contains(&20_000)
                && block
                    .points()
                    .points()
                    .iter()
                    .any(|point| point.color == viewer_core::ColorRgb::new(78, 78, 78))
            {
                found_last_point = true;
            }
        }
        assert!(found_last_point);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bounded_spatial_workers_preserve_the_serial_output() {
        let dir = test_dir();
        let input = dir.join("source.las");
        let serial_output = dir.join("serial.oct");
        let parallel_output = dir.join("parallel.oct");
        fixture(&input, false, true, 200);
        let serial_options = ConvertOptions::new(16, 0.01)
            .unwrap()
            .with_spatial_divisions(4)
            .unwrap();
        let parallel_options = serial_options.with_spatial_workers(2).unwrap();

        let serial = convert_file(&input, &serial_output, serial_options).unwrap();
        let parallel = convert_file(&input, &parallel_output, parallel_options).unwrap();

        assert_eq!(parallel.point_count, serial.point_count);
        assert_eq!(parallel.block_count, serial.block_count);
        assert_eq!(parallel.output_bytes, serial.output_bytes);
        assert_eq!(
            parallel.estimated_peak_memory_bytes,
            serial.estimated_peak_memory_bytes + 1024 * 1024 + 512 * 1024
        );
        assert_eq!(
            fs::read(serial_output).unwrap(),
            fs::read(parallel_output).unwrap()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parallel_worker_limits_are_validated_and_budget_is_respected() {
        let dir = test_dir();
        let input = dir.join("source.las");
        let output = dir.join("parallel.oct");
        fixture(&input, false, false, 100);
        let options = ConvertOptions::with_memory_budget(10, 0.01, 17 * 1024 * 1024)
            .unwrap()
            .with_spatial_workers(2)
            .unwrap();

        assert!(matches!(
            ConvertOptions::default().with_spatial_workers(0),
            Err(ConvertError::InvalidSpatialWorkerCount(0))
        ));
        assert!(matches!(
            ConvertOptions::default().with_spatial_workers(5),
            Err(ConvertError::InvalidSpatialWorkerCount(5))
        ));
        assert!(matches!(
            ConvertOptions::default().with_spatial_divisions(3),
            Err(ConvertError::InvalidSpatialDivisions(3))
        ));
        assert!(matches!(
            convert_file(&input, &output, options),
            Err(ConvertError::MemoryBudgetExceeded { .. })
        ));
        assert!(!output.exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancellation_during_parallel_spatial_planning_does_not_publish_output() {
        let dir = test_dir();
        let input = dir.join("source.las");
        let output = dir.join("parallel.oct");
        fixture(&input, false, false, 100);
        let options = ConvertOptions::new(10, 0.01)
            .unwrap()
            .with_spatial_workers(2)
            .unwrap();
        let mut hierarchy_updates = 0;

        let result = convert_file_with_progress(&input, &output, options, |event| {
            if event.stage == ConversionStage::BuildingSpatialHierarchy {
                hierarchy_updates += 1;
                hierarchy_updates < 12
            } else {
                true
            }
        });

        assert!(matches!(result, Err(ConvertError::Cancelled { .. })));
        assert_eq!(hierarchy_updates, 12);
        assert!(!output.exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
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
    fn identical_positions_remain_full_resolution_without_deep_empty_splits() {
        let dir = test_dir();
        let input = dir.join("identical.las");
        let output = dir.join("result.oct");
        let header = Builder::from((1, 2)).into_header().unwrap();
        let mut writer = Writer::from_path(&input, header).unwrap();
        for intensity in [1, 2, 3, 4, 5] {
            writer
                .write_point(Point {
                    x: 10.0,
                    y: 20.0,
                    z: 30.0,
                    intensity,
                    ..Point::default()
                })
                .unwrap();
        }
        drop(writer);

        let report = convert_file(&input, &output, ConvertOptions::new(2, 0.01).unwrap()).unwrap();
        assert_eq!(report.point_count, 5);
        assert_eq!(report.block_count, 3);
        let mut reader = FormatReader::new(fs::File::open(output).unwrap()).unwrap();
        let root_blocks = {
            let hierarchy = reader.metadata().hierarchy();
            assert_eq!(hierarchy.stats().node_count, 1);
            let root = hierarchy.node(NodeId::new(0)).unwrap();
            assert_eq!(root.point_count(), 5);
            root.blocks().to_vec()
        };
        assert_eq!(root_blocks.len(), 3);
        let mut intensities = Vec::new();
        for block_id in root_blocks {
            let block = reader.read_block(block_id).unwrap();
            let AttributeValues::U16(values) = block.attributes()[0].values() else {
                panic!("intensity must remain u16");
            };
            intensities.extend_from_slice(values);
        }
        intensities.sort_unstable();
        assert_eq!(intensities, [1, 2, 3, 4, 5]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lod_voxels_average_color_and_copy_other_attributes_from_nearest_point() {
        let dir = test_dir();
        let input = dir.join("colors.las");
        let output = dir.join("result.oct");
        let mut builder = Builder::from((1, 2));
        builder.point_format = Format::new(2).unwrap();
        let mut writer = Writer::from_path(&input, builder.into_header().unwrap()).unwrap();
        for (position, intensity, source_id, color) in [
            ([0.05, 0.05, 0.05], 10, 11, Color::new(0, 0, 0)),
            (
                [0.06, 0.06, 0.06],
                20,
                22,
                Color::new(u16::MAX, u16::MAX, u16::MAX),
            ),
            ([1.0, 1.0, 1.0], 30, 33, Color::new(0, 0, 0)),
        ] {
            writer
                .write_point(Point {
                    x: position[0],
                    y: position[1],
                    z: position[2],
                    intensity,
                    point_source_id: source_id,
                    color: Some(color),
                    ..Point::default()
                })
                .unwrap();
        }
        drop(writer);

        convert_file(&input, &output, ConvertOptions::new(1, 0.01).unwrap()).unwrap();
        let mut reader = FormatReader::new(fs::File::open(output).unwrap()).unwrap();
        let (root_block_id, max_level) = {
            let hierarchy = reader.metadata().hierarchy();
            (
                hierarchy.node(NodeId::new(0)).unwrap().blocks()[0],
                hierarchy.stats().max_level,
            )
        };
        assert!(max_level > 1);
        let root_sample = reader.read_block(root_block_id).unwrap();
        let averaged_sample = root_sample
            .points()
            .points()
            .iter()
            .position(|point| point.color == viewer_core::ColorRgb::new(128, 128, 128))
            .expect("the two clustered source colors are averaged");
        let AttributeValues::U16(intensity) = root_sample.attributes()[0].values() else {
            panic!("intensity must remain u16");
        };
        assert_eq!(intensity[averaged_sample], 20);
        let AttributeValues::U16(source_ids) = root_sample.attributes()[6].values() else {
            panic!("point source ID must remain u16");
        };
        assert_eq!(source_ids[averaged_sample], 22);
        let AttributeValues::U16(rgb) = root_sample.attributes()[7].values() else {
            panic!("RGB must remain u16");
        };
        let rgb = &rgb[averaged_sample * 3..averaged_sample * 3 + 3];
        assert_eq!(rgb, &[0x8000; 3]);
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

        let rebuilt = convert_file(&input, &output, ConvertOptions::new(2, 0.01).unwrap())
            .expect("a cancelled conversion can be rebuilt from its source");
        assert_eq!(rebuilt.point_count, 5);
        assert!(output.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn resumes_from_the_completed_source_spool_and_rejects_changed_options() {
        let dir = test_dir();
        let input = dir.join("source.las");
        let output = dir.join("result.oct");
        let expected = dir.join("expected.oct");
        let mut checkpoint_dir = output.as_os_str().to_owned();
        checkpoint_dir.push(".checkpoint");
        let checkpoint_dir = PathBuf::from(checkpoint_dir);
        fixture(&input, false, false, 5);
        let options = ConvertOptions::new(2, 0.01)
            .unwrap()
            .with_spatial_workers(2)
            .unwrap();

        let cancelled = convert_file_with_checkpoint(
            &input,
            &output,
            options,
            CheckpointMode::Create,
            |event| event.stage != ConversionStage::BuildingSpatialHierarchy,
        );
        assert!(matches!(cancelled, Err(ConvertError::Cancelled { .. })));
        assert!(checkpoint_dir.join("source.spool").is_file());
        assert!(!output.exists());

        let changed_options = ConvertOptions::new(3, 0.01).unwrap();
        assert!(matches!(
            convert_file_with_checkpoint(
                &input,
                &output,
                changed_options,
                CheckpointMode::Resume,
                |_| true
            ),
            Err(ConvertError::Checkpoint(CheckpointError::Mismatch))
        ));
        assert!(checkpoint_dir.join("source.spool").is_file());

        let mut reread_source = false;
        let resume_options = options.with_spatial_workers(1).unwrap();
        let report = convert_file_with_checkpoint(
            &input,
            &output,
            resume_options,
            CheckpointMode::Resume,
            |event| {
                reread_source |= event.stage == ConversionStage::PlanningSpatialIndex;
                true
            },
        )
        .unwrap();
        assert_eq!(report.point_count, 5);
        assert!(!reread_source);
        assert!(output.exists());
        assert!(!checkpoint_dir.exists());

        convert_file(&input, &expected, options).unwrap();
        assert_eq!(fs::read(output).unwrap(), fs::read(expected).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn resumes_validation_after_output_is_published_but_checkpoint_cleanup_did_not_run() {
        let dir = test_dir();
        let input = dir.join("source.las");
        let output = dir.join("result.oct");
        let expected = dir.join("expected.oct");
        let mut checkpoint_dir = output.as_os_str().to_owned();
        checkpoint_dir.push(".checkpoint");
        let checkpoint_dir = PathBuf::from(checkpoint_dir);
        fixture(&input, false, false, 5);
        let options = ConvertOptions::new(2, 0.01).unwrap();

        let cancelled = convert_file_with_checkpoint(
            &input,
            &output,
            options,
            CheckpointMode::Create,
            |event| event.stage != ConversionStage::ValidatingOutput,
        );
        assert!(matches!(cancelled, Err(ConvertError::Cancelled { .. })));
        let candidate = checkpoint_dir.join("candidate.oct");
        assert!(candidate.is_file());
        let candidate_bytes = fs::read(&candidate).unwrap();
        let mut corrupted = candidate_bytes.clone();
        *corrupted.last_mut().unwrap() ^= 1;
        fs::write(&candidate, corrupted).unwrap();
        assert!(matches!(
            convert_file_with_checkpoint(&input, &output, options, CheckpointMode::Resume, |_| {
                true
            }),
            Err(ConvertError::Checkpoint(CheckpointError::Invalid(_)))
        ));
        assert!(candidate.exists());
        fs::write(&candidate, candidate_bytes).unwrap();
        fs::hard_link(&candidate, &output).unwrap();

        let report =
            convert_file_with_checkpoint(&input, &output, options, CheckpointMode::Resume, |_| {
                true
            })
            .unwrap();
        assert_eq!(report.point_count, 5);
        assert!(!checkpoint_dir.exists());

        convert_file(&input, &expected, options).unwrap();
        assert_eq!(fs::read(output).unwrap(), fs::read(expected).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_changed_input_and_preserves_the_checkpoint() {
        let dir = test_dir();
        let input = dir.join("source.las");
        let output = dir.join("result.oct");
        let mut checkpoint_dir = output.as_os_str().to_owned();
        checkpoint_dir.push(".checkpoint");
        let checkpoint_dir = PathBuf::from(checkpoint_dir);
        fixture(&input, false, false, 5);
        let options = ConvertOptions::new(2, 0.01).unwrap();

        let cancelled = convert_file_with_checkpoint(
            &input,
            &output,
            options,
            CheckpointMode::Create,
            |event| event.stage != ConversionStage::BuildingSpatialHierarchy,
        );
        assert!(matches!(cancelled, Err(ConvertError::Cancelled { .. })));
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&input)
            .unwrap();
        file.seek(SeekFrom::End(-1)).unwrap();
        let mut last_byte = [0];
        file.read_exact(&mut last_byte).unwrap();
        last_byte[0] ^= 1;
        file.seek(SeekFrom::End(-1)).unwrap();
        file.write_all(&last_byte).unwrap();
        file.sync_all().unwrap();

        assert!(matches!(
            convert_file_with_checkpoint(&input, &output, options, CheckpointMode::Resume, |_| {
                true
            }),
            Err(ConvertError::Checkpoint(CheckpointError::Mismatch))
        ));
        assert!(checkpoint_dir.join("source.spool").is_file());
        assert!(!output.exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
