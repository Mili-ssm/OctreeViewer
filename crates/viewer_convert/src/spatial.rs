use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
};

use las::{PointData, PointDataBuilder, Reader};
use viewer_core::{
    AttributeColumn, AttributeValues, BlockId, ColorRgb, DatasetMetadata, NodeId, SpatialBounds,
    SpatialHierarchy, SpatialNode, SpatialSubdivision,
};
use viewer_format::{FormatMetadata, SourcePointBlock};

use super::{ConvertError, ConvertOptions};
use crate::diagnostics::{SpatialPhaseDiagnostics, SpatialSpoolDiagnostics};
use crate::las_source::{SourceInfo, next_descriptor, omitted_dimensions, source_schema};
use crate::report::{ConversionProgress, ConversionStage, emit_progress};

const MAX_SPOOL_RECORD_BYTES: usize = 55;
const SPOOL_IO_BUFFER_BYTES: usize = 64 * 1024;
const CHILD_SPOOL_BUFFER_BYTES: usize = 32 * 1024;
const SAMPLE_GRID_SIZE: usize = 8;
const MAX_SPATIAL_DEPTH: u32 = 32;
const PLANNED_NODE_BYTES: u64 = 256;
const SPATIAL_WORKER_OVERHEAD_BYTES: u64 = 1024 * 1024;
const HEADER_BOUNDS_RELATIVE_TOLERANCE: f64 = 1.0e-7;
const HEADER_BOUNDS_MAX_TOLERANCE: f64 = 0.5;

/// All temporary files for a source share these attributes and transforms.
/// Source coordinates stay on the exact LAS integer grid; generated LOD centers use f64.
#[derive(Clone, Copy)]
struct SpoolLayout {
    transforms: las::Vector<las::Transform>,
    source_coordinates: bool,
    has_gps_time: bool,
    has_color: bool,
    has_nir: bool,
}

impl SpoolLayout {
    fn source(source: &SourceInfo) -> Self {
        Self {
            transforms: source.transforms,
            source_coordinates: true,
            has_gps_time: source.point_format.has_gps_time,
            has_color: source.point_format.has_color,
            has_nir: source.point_format.has_nir,
        }
    }

    fn lod(self) -> Self {
        Self {
            source_coordinates: false,
            ..self
        }
    }

    fn record_bytes(self) -> usize {
        let coordinates = if self.source_coordinates { 12 } else { 24 };
        coordinates
            + 15
            + usize::from(self.has_gps_time) * 8
            + usize::from(self.has_color) * 6
            + usize::from(self.has_nir) * 2
    }

    fn position(self, raw: [i32; 3]) -> [f64; 3] {
        [
            self.transforms.x.direct(raw[0]),
            self.transforms.y.direct(raw[1]),
            self.transforms.z.direct(raw[2]),
        ]
    }
}

#[derive(Clone, Copy)]
struct SpoolPoint {
    raw_position: [i32; 3],
    position: [f64; 3],
    intensity: u16,
    classification: u8,
    return_number: u8,
    returns_per_pulse: u8,
    scan_angle: f32,
    user_data: u8,
    point_source_id: u16,
    gps_time: f64,
    rgb: [u16; 3],
    nir: u16,
    display_color: ColorRgb,
}

pub(crate) struct PlannedBlock {
    pub id: BlockId,
    path: PathBuf,
    offset_points: u64,
    point_count: usize,
    layout: SpoolLayout,
    pub is_source_points: bool,
    // Blocks sharing a spool stay in construction order when streamed. Only the
    // last range may release it, after all reads in its worker batch have joined.
    final_block_in_spool: bool,
}

impl PlannedBlock {
    pub fn point_count(&self) -> usize {
        self.point_count
    }

    pub fn payload_bytes(&self) -> Result<u64, ConvertError> {
        u64::try_from(self.point_count)
            .ok()
            .and_then(|points| points.checked_mul(self.layout.record_bytes() as u64))
            .ok_or(ConvertError::SizeOverflow)
    }

    pub fn release_spool_after_write(&self) -> Result<(), ConvertError> {
        if self.final_block_in_spool {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }

    pub fn read(
        &self,
        frame: viewer_core::CoordinateFrame,
    ) -> Result<SourcePointBlock, ConvertError> {
        let byte_offset = self
            .offset_points
            .checked_mul(self.layout.record_bytes() as u64)
            .ok_or(ConvertError::SizeOverflow)?;
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(byte_offset))?;
        let mut reader = BufReader::with_capacity(SPOOL_IO_BUFFER_BYTES, file);
        let record_bytes = self.layout.record_bytes();
        let batch_capacity = (SPOOL_IO_BUFFER_BYTES / record_bytes).max(1);
        let mut record_buffer = vec![0; batch_capacity * record_bytes];
        let mut positions = Vec::with_capacity(self.point_count);
        let mut colors = Vec::with_capacity(self.point_count);
        let mut intensities = Vec::with_capacity(self.point_count);
        let mut classifications = Vec::with_capacity(self.point_count);
        let mut return_numbers = Vec::with_capacity(self.point_count);
        let mut returns_per_pulse = Vec::with_capacity(self.point_count);
        let mut scan_angles = Vec::with_capacity(self.point_count);
        let mut user_data = Vec::with_capacity(self.point_count);
        let mut point_source_ids = Vec::with_capacity(self.point_count);
        let mut gps_times = Vec::with_capacity(if self.layout.has_gps_time {
            self.point_count
        } else {
            0
        });
        let mut rgb_values = Vec::with_capacity(if self.layout.has_color {
            self.point_count.saturating_mul(3)
        } else {
            0
        });
        let mut nir_values = Vec::with_capacity(if self.layout.has_nir {
            self.point_count
        } else {
            0
        });
        let mut remaining = self.point_count;
        while remaining > 0 {
            let requested = remaining.min(batch_capacity);
            let buffer_bytes = requested * record_bytes;
            let records =
                read_records(&mut reader, &mut record_buffer[..buffer_bytes], self.layout)?;
            if records != requested {
                return Err(ConvertError::PointCountMismatch {
                    expected: self.point_count as u64,
                    actual: (self.point_count - remaining + records) as u64,
                });
            }
            for bytes in record_buffer[..buffer_bytes].chunks_exact(record_bytes) {
                let point = decode_point(bytes, self.layout);
                positions.push(std::array::from_fn(|axis| {
                    point.position[axis] - frame.origin()[axis]
                }));
                colors.push(point.display_color);
                intensities.push(point.intensity);
                classifications.push(point.classification);
                return_numbers.push(point.return_number);
                returns_per_pulse.push(point.returns_per_pulse);
                scan_angles.push(point.scan_angle);
                user_data.push(point.user_data);
                point_source_ids.push(point.point_source_id);
                if self.layout.has_gps_time {
                    gps_times.push(point.gps_time);
                }
                if self.layout.has_color {
                    rgb_values.extend_from_slice(&point.rgb);
                }
                if self.layout.has_nir {
                    nir_values.push(point.nir);
                }
            }
            remaining -= records;
        }

        let mut descriptors = viewer_core::las::las_attribute_schema(
            self.layout.has_gps_time,
            self.layout.has_color,
            self.layout.has_nir,
        )
        .into_iter();
        let mut attributes = Vec::with_capacity(10);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "intensity"),
            AttributeValues::U16(intensities),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "classification"),
            AttributeValues::U8(classifications),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "return_number"),
            AttributeValues::U8(return_numbers),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "number_of_returns"),
            AttributeValues::U8(returns_per_pulse),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "scan_angle_degrees"),
            AttributeValues::F32(scan_angles),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "user_data"),
            AttributeValues::U8(user_data),
            self.point_count,
        )?);
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "point_source_id"),
            AttributeValues::U16(point_source_ids),
            self.point_count,
        )?);
        if self.layout.has_gps_time {
            attributes.push(AttributeColumn::new(
                next_descriptor(&mut descriptors, "gps_time"),
                AttributeValues::F64(gps_times),
                self.point_count,
            )?);
        }
        if self.layout.has_color {
            attributes.push(AttributeColumn::new(
                next_descriptor(&mut descriptors, "rgb"),
                AttributeValues::U16(rgb_values),
                self.point_count,
            )?);
        }
        if self.layout.has_nir {
            attributes.push(AttributeColumn::new(
                next_descriptor(&mut descriptors, "nir"),
                AttributeValues::U16(nir_values),
                self.point_count,
            )?);
        }
        Ok(SourcePointBlock::new(
            self.id, positions, colors, attributes,
        ))
    }
}

pub(crate) struct SpatialPlan {
    pub metadata: FormatMetadata,
    pub blocks: Vec<PlannedBlock>,
    pub point_count: u64,
    pub estimated_peak_memory_bytes: u64,
    pub omitted_dimensions: Vec<String>,
    pub spool_diagnostics: Option<SpatialSpoolDiagnostics>,
    pub spatial_phases: Option<SpatialPhaseDiagnostics>,
}

pub(crate) struct Workspace {
    path: PathBuf,
    cleanup_on_drop: bool,
}

impl Workspace {
    pub fn create(output_path: &Path) -> Result<Self, ConvertError> {
        let parent = output_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let file_name = output_path
            .file_name()
            .ok_or_else(|| ConvertError::TemporaryPathUnavailable(output_path.to_owned()))?;
        for _ in 0..100 {
            let counter =
                crate::publish::TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut name = file_name.to_os_string();
            name.push(format!(".{}.{}.work", std::process::id(), counter));
            let path = parent.join(name);
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        cleanup_on_drop: true,
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

    pub(crate) fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    pub fn create_at(path: &Path) -> Result<Self, ConvertError> {
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
        fs::create_dir(path)?;
        Ok(Self {
            path: path.to_owned(),
            cleanup_on_drop: true,
        })
    }

    fn create_child(path: &Path) -> Result<Self, ConvertError> {
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
        fs::create_dir(path)?;
        Ok(Self {
            path: path.to_owned(),
            cleanup_on_drop: false,
        })
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

pub(crate) fn build_spatial_plan(
    source: &SourceInfo,
    options: ConvertOptions,
    workspace: &Workspace,
    source_path: &Path,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    collect_spool_diagnostics: bool,
    source_spool_bytes_written: u64,
) -> Result<SpatialPlan, ConvertError> {
    build_spatial_plan_inner(
        source,
        options,
        workspace,
        SpatialInput::Spool(source_path),
        on_progress,
        collect_spool_diagnostics,
        source_spool_bytes_written,
    )
}

pub(crate) fn build_spatial_plan_from_reader(
    reader: &mut Reader,
    input_path: &Path,
    source: &SourceInfo,
    options: ConvertOptions,
    workspace: &Workspace,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    collect_spool_diagnostics: bool,
) -> Result<SpatialPlan, ConvertError> {
    build_spatial_plan_inner(
        source,
        options,
        workspace,
        SpatialInput::Las { reader, input_path },
        on_progress,
        collect_spool_diagnostics,
        0,
    )
}

enum SpatialInput<'a> {
    Spool(&'a Path),
    Las {
        reader: &'a mut Reader,
        input_path: &'a Path,
    },
}

fn build_spatial_plan_inner(
    source: &SourceInfo,
    options: ConvertOptions,
    workspace: &Workspace,
    input: SpatialInput<'_>,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    collect_spool_diagnostics: bool,
    source_spool_bytes_written: u64,
) -> Result<SpatialPlan, ConvertError> {
    let point_limit =
        crate::options::input_batch_point_limit(options.points_per_block(), source.record_length);
    check_memory_budget(
        estimate_memory(point_limit, 0, 0, options.spatial_divisions())?,
        options.max_memory_budget_bytes(),
    )?;
    let root_bounds = cube_bounds(source.bounds);
    let mut builder = SpatialBuilder {
        workspace,
        point_limit,
        memory_budget: options.max_memory_budget_bytes(),
        spatial_divisions: options.spatial_divisions(),
        frame: source.frame,
        layout: SpoolLayout::source(source),
        node_records: Vec::new(),
        output_blocks: Vec::new(),
        next_block_id: 0,
        on_progress,
        source_point_count: source.point_count,
        parallel_workers: options.spatial_workers(),
        workers_used: 1,
        spool_diagnostics: collect_spool_diagnostics.then(|| SpatialSpoolDiagnostics {
            source_payload_written_bytes: source_spool_bytes_written,
            ..SpatialSpoolDiagnostics::default()
        }),
        spatial_phases: collect_spool_diagnostics.then(SpatialPhaseDiagnostics::default),
    };
    match input {
        SpatialInput::Spool(path) => {
            builder.build_node(
                path.to_owned(),
                source.point_count,
                root_bounds,
                None,
                0,
                false,
            )?;
        }
        SpatialInput::Las { reader, input_path } => {
            builder.build_root_from_las(reader, input_path, source, options, root_bounds)?;
        }
    }

    let estimated_peak_memory_bytes = estimate_memory(
        point_limit,
        builder.node_records.len(),
        builder.output_blocks.len(),
        options.spatial_divisions(),
    )?
    .checked_add(
        (builder.workers_used.saturating_sub(1) as u64)
            .checked_mul(
                SPATIAL_WORKER_OVERHEAD_BYTES
                    .checked_add(child_spool_buffer_bytes(options.spatial_divisions())?)
                    .ok_or(ConvertError::SizeOverflow)?,
            )
            .ok_or(ConvertError::SizeOverflow)?,
    )
    .ok_or(ConvertError::SizeOverflow)?;
    check_memory_budget(
        estimated_peak_memory_bytes,
        options.max_memory_budget_bytes(),
    )?;

    let schema = source_schema(&source.point_format);
    crate::publish::check_reader_attribute_ranges(builder.node_records.len(), schema.len())?;
    let nodes = builder
        .node_records
        .iter()
        .map(|node| {
            SpatialNode::new(
                node.id,
                node.parent,
                node.level,
                node.bounds,
                node.point_count,
                node.geometric_error,
                node.blocks.clone(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let hierarchy = SpatialHierarchy::new(nodes, schema, Some(source.point_count))?;
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(source.frame, hierarchy),
        None,
        Some(format!(
            "adaptive {}x{}x{} spatial partition; full-resolution leaves; 8x8x8 voxel-center LOD samples; color averaged per voxel; other attributes from nearest source point",
            options.spatial_divisions(),
            options.spatial_divisions(),
            options.spatial_divisions()
        )),
        Vec::new(),
    )?
    .with_visual_scale(crate::publish::visual_scale_for_bounds(source.bounds)?)?;
    Ok(SpatialPlan {
        metadata,
        blocks: builder.output_blocks,
        point_count: source.point_count,
        estimated_peak_memory_bytes,
        omitted_dimensions: omitted_dimensions(&source.point_format, source.has_crs),
        spool_diagnostics: builder.spool_diagnostics,
        spatial_phases: builder.spatial_phases,
    })
}

struct NodeRecord {
    id: NodeId,
    parent: Option<NodeId>,
    level: u32,
    bounds: SpatialBounds,
    point_count: u64,
    geometric_error: f64,
    blocks: Vec<BlockId>,
}

struct RootChildTask {
    key: usize,
    path: PathBuf,
    point_count: u64,
    bounds: SpatialBounds,
}

struct ChildSpool {
    layout: SpoolLayout,
    path: PathBuf,
    point_count: u64,
    buffer: Vec<u8>,
    created: bool,
    payload_written_bytes: Option<u64>,
}

impl ChildSpool {
    fn new(path: PathBuf, layout: SpoolLayout, collect_diagnostics: bool) -> Self {
        Self {
            layout,
            path,
            point_count: 0,
            buffer: Vec::with_capacity(CHILD_SPOOL_BUFFER_BYTES),
            created: false,
            payload_written_bytes: collect_diagnostics.then_some(0),
        }
    }

    fn write_point(&mut self, point: &SpoolPoint) -> Result<(), ConvertError> {
        if self.buffer.len() + self.layout.record_bytes() > CHILD_SPOOL_BUFFER_BYTES {
            self.flush()?;
        }
        write_point(&mut self.buffer, point, self.layout)?;
        self.point_count += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), ConvertError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let mut options = OpenOptions::new();
        options.write(true);
        if self.created {
            options.append(true);
        } else {
            options.create_new(true);
        }
        let mut file = options.open(&self.path)?;
        file.write_all(&self.buffer)?;
        if let Some(bytes_written) = self.payload_written_bytes.as_mut() {
            *bytes_written = bytes_written
                .checked_add(self.buffer.len() as u64)
                .ok_or(ConvertError::SizeOverflow)?;
        }
        self.buffer.clear();
        self.created = true;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ConvertError> {
        self.flush()
    }
}

struct BranchOutput {
    node_records: Vec<NodeRecord>,
    output_blocks: Vec<PlannedBlock>,
    next_block_id: u64,
    spool_diagnostics: Option<SpatialSpoolDiagnostics>,
    spatial_phases: Option<SpatialPhaseDiagnostics>,
}

enum BranchMessage {
    Progress(ConversionProgress),
    Finished(usize, Result<BranchOutput, ConvertError>),
}

struct SpatialBuilder<'a> {
    workspace: &'a Workspace,
    point_limit: u64,
    memory_budget: u64,
    spatial_divisions: u8,
    frame: viewer_core::CoordinateFrame,
    layout: SpoolLayout,
    node_records: Vec<NodeRecord>,
    output_blocks: Vec<PlannedBlock>,
    next_block_id: u64,
    on_progress: &'a mut dyn FnMut(ConversionProgress) -> bool,
    source_point_count: u64,
    parallel_workers: usize,
    workers_used: usize,
    spool_diagnostics: Option<SpatialSpoolDiagnostics>,
    spatial_phases: Option<SpatialPhaseDiagnostics>,
}

#[derive(Clone, Copy)]
enum SpatialPhase {
    RootLasFill,
    SpoolScanRead,
    SpoolPartitionAndSample,
    LodSampleWrite,
}

impl SpatialBuilder<'_> {
    fn record_phase_elapsed(&mut self, started: Option<Instant>, phase: SpatialPhase) {
        let (Some(metrics), Some(started)) = (self.spatial_phases.as_mut(), started) else {
            return;
        };
        let elapsed = started.elapsed().as_nanos();
        let target = match phase {
            SpatialPhase::RootLasFill => &mut metrics.root_las_fill_elapsed_ns,
            SpatialPhase::SpoolScanRead => &mut metrics.spool_scan_read_elapsed_ns,
            SpatialPhase::SpoolPartitionAndSample => {
                &mut metrics.spool_partition_and_sample_elapsed_ns
            }
            SpatialPhase::LodSampleWrite => &mut metrics.lod_sample_write_elapsed_ns,
        };
        *target = target.saturating_add(elapsed);
    }

    fn add_source_spool_bytes(&mut self, bytes: u64) -> Result<(), ConvertError> {
        if let Some(diagnostics) = self.spool_diagnostics.as_mut() {
            diagnostics.source_payload_written_bytes = diagnostics
                .source_payload_written_bytes
                .checked_add(bytes)
                .ok_or(ConvertError::SizeOverflow)?;
        }
        Ok(())
    }

    fn add_scanned_spool_bytes(
        &mut self,
        records: usize,
        record_bytes: usize,
        child_spool: bool,
    ) -> Result<(), ConvertError> {
        let Some(diagnostics) = self.spool_diagnostics.as_mut() else {
            return Ok(());
        };
        let bytes = records
            .checked_mul(record_bytes)
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(ConvertError::SizeOverflow)?;
        let counter = if child_spool {
            &mut diagnostics.child_payload_scanned_bytes
        } else {
            &mut diagnostics.source_payload_scanned_bytes
        };
        *counter = counter
            .checked_add(bytes)
            .ok_or(ConvertError::SizeOverflow)?;
        Ok(())
    }

    fn record_child_spool_writes(
        &mut self,
        children: &[Option<ChildSpool>],
    ) -> Result<(), ConvertError> {
        let Some(diagnostics) = self.spool_diagnostics.as_mut() else {
            return Ok(());
        };
        let bytes = children.iter().flatten().try_fold(0_u64, |total, child| {
            total
                .checked_add(child.payload_written_bytes.unwrap_or(0))
                .ok_or(ConvertError::SizeOverflow)
        })?;
        diagnostics.child_payload_written_bytes = diagnostics
            .child_payload_written_bytes
            .checked_add(bytes)
            .ok_or(ConvertError::SizeOverflow)?;
        Ok(())
    }

    fn add_lod_spool_bytes(
        &mut self,
        points: usize,
        layout: SpoolLayout,
    ) -> Result<(), ConvertError> {
        if self.spool_diagnostics.is_none() {
            return Ok(());
        }
        let bytes = points
            .checked_mul(layout.record_bytes())
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(ConvertError::SizeOverflow)?;
        let diagnostics = self
            .spool_diagnostics
            .as_mut()
            .expect("diagnostics were checked above");
        diagnostics.lod_payload_written_bytes = diagnostics
            .lod_payload_written_bytes
            .checked_add(bytes)
            .ok_or(ConvertError::SizeOverflow)?;
        Ok(())
    }

    fn build_root_from_las(
        &mut self,
        reader: &mut Reader,
        input_path: &Path,
        source: &SourceInfo,
        options: ConvertOptions,
        bounds: SpatialBounds,
    ) -> Result<(), ConvertError> {
        let can_split = source.point_count > self.point_limit
            && cube_side(bounds) > 0.0
            && MAX_SPATIAL_DEPTH > 0;
        let root_id = NodeId::new(0);
        if !can_split {
            let spool_path = self.workspace.file("source.spool");
            if let Some(bytes) = spool_source(
                reader,
                source,
                options,
                &spool_path,
                self.on_progress,
                self.spool_diagnostics.is_some(),
            )? {
                self.add_source_spool_bytes(bytes)?;
            }
            self.build_node(spool_path, source.point_count, bounds, None, 0, false)?;
            return Ok(());
        }

        let child_count = usize::from(self.spatial_divisions).pow(3);
        let mut children = (0..child_count)
            .map(|_| None)
            .collect::<Vec<Option<ChildSpool>>>();
        let mut samples = std::iter::repeat_with(|| None)
            .take(SAMPLE_GRID_SIZE.pow(3))
            .collect::<Vec<Option<SampleAccumulator>>>();
        let mut source_bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 3];
        let subdivision = SpatialSubdivision::new(bounds, self.spatial_divisions)
            .expect("converter subdivisions are validated");
        let point_limit =
            crate::options::input_batch_point_limit(self.point_limit, source.record_length);
        let mut point_data = PointDataBuilder::new().for_header(reader.header()).build();
        let mut processed = 0_u64;
        while processed < source.point_count {
            let requested = point_limit.min(source.point_count - processed);
            let fill_started = self.spatial_phases.as_ref().map(|_| Instant::now());
            let read = crate::las_source::fill_points(reader, requested, &mut point_data)?;
            self.record_phase_elapsed(fill_started, SpatialPhase::RootLasFill);
            if read != requested {
                return Err(ConvertError::PointCountMismatch {
                    expected: source.point_count,
                    actual: processed + read,
                });
            }
            let classify_started = self.spatial_phases.as_ref().map(|_| Instant::now());
            visit_source_points(&point_data, source, processed, |point| {
                for (axis, axis_bounds) in source_bounds.iter_mut().enumerate() {
                    axis_bounds[0] = axis_bounds[0].min(point.position[axis]);
                    axis_bounds[1] = axis_bounds[1].max(point.position[axis]);
                }
                let child_key = subdivision.cell_index(point.position);
                let child = children[child_key].get_or_insert_with(|| {
                    ChildSpool::new(
                        self.workspace
                            .file(&format!("node-{}-child-{child_key}.spool", root_id.get())),
                        self.layout,
                        self.spool_diagnostics.is_some(),
                    )
                });
                child.write_point(&point)?;
                let voxel_key = sample_index(point.position, bounds);
                let sample = samples[voxel_key].get_or_insert_with(SampleAccumulator::default);
                sample.add(point, voxel_center(voxel_key, bounds));
                Ok(())
            })?;
            self.record_phase_elapsed(classify_started, SpatialPhase::SpoolPartitionAndSample);
            processed += read;
            emit_progress(
                self.on_progress,
                ConversionProgress {
                    stage: ConversionStage::PlanningSpatialIndex,
                    points_processed: processed,
                    total_points: source.point_count,
                    bytes_written: 0,
                },
                processed,
            )?;
        }

        if source_bounds
            .iter()
            .all(|axis_bounds| axis_bounds[0] == axis_bounds[1])
        {
            self.record_child_spool_writes(&children)?;
            for child in children.into_iter().flatten() {
                if child.created {
                    fs::remove_file(child.path)?;
                }
            }
            let mut source_reader = crate::las_source::open_las_reader(input_path)?;
            let spool_path = self.workspace.file("source.spool");
            let bytes = spool_source(
                &mut source_reader,
                source,
                options,
                &spool_path,
                self.on_progress,
                self.spool_diagnostics.is_some(),
            )?;
            if let Some(bytes) = bytes {
                self.add_source_spool_bytes(bytes)?;
            }
            self.build_node(spool_path, source.point_count, bounds, None, 0, false)?;
            return Ok(());
        }

        for child in children.iter_mut().flatten() {
            child.finish()?;
        }
        self.record_child_spool_writes(&children)?;
        let lod_write_started = self.spatial_phases.as_ref().map(|_| Instant::now());
        let sample_path = self.workspace.file("node-0.lod");
        let mut sample_writer = BufWriter::new(File::create(&sample_path)?);
        for (key, accumulator) in samples
            .iter()
            .enumerate()
            .filter_map(|(key, sample)| sample.as_ref().map(|accumulator| (key, accumulator)))
        {
            write_point(
                &mut sample_writer,
                &accumulator.finish(voxel_center(key, bounds)),
                self.layout.lod(),
            )?;
        }
        sample_writer
            .into_inner()
            .map_err(|error| error.into_error())?;
        self.record_phase_elapsed(lod_write_started, SpatialPhase::LodSampleWrite);

        let sample_count = samples.iter().filter(|sample| sample.is_some()).count();
        self.add_lod_spool_bytes(sample_count, self.layout.lod())?;
        let sample_block = if sample_count > 0 {
            let block_id = self.allocate_block_id()?;
            self.output_blocks.push(PlannedBlock {
                id: block_id,
                path: sample_path,
                offset_points: 0,
                point_count: sample_count,
                layout: self.layout.lod(),
                is_source_points: false,
                final_block_in_spool: true,
            });
            vec![block_id]
        } else {
            Vec::new()
        };
        self.ensure_memory(self.node_records.len() + 1, self.output_blocks.len())?;
        self.node_records.push(NodeRecord {
            id: root_id,
            parent: None,
            level: 0,
            bounds,
            point_count: sample_count as u64,
            geometric_error: (3.0_f64).sqrt() * cube_side(bounds) / SAMPLE_GRID_SIZE as f64 / 2.0
                + center_rounding_error(bounds, self.frame),
            blocks: sample_block,
        });
        self.emit_plan_progress()?;

        let child_tasks = self.child_tasks(bounds, children)?;
        self.process_child_tasks(root_id, 0, child_tasks)
    }

    fn build_node(
        &mut self,
        spool_path: PathBuf,
        source_count: u64,
        bounds: SpatialBounds,
        parent: Option<NodeId>,
        level: u32,
        input_is_child_spool: bool,
    ) -> Result<NodeId, ConvertError> {
        let id = NodeId::new(self.node_records.len() as u64);
        let can_split =
            source_count > self.point_limit && level < MAX_SPATIAL_DEPTH && cube_side(bounds) > 0.0;
        if !can_split {
            self.add_leaf(id, spool_path, source_count, bounds, parent, level)?;
            return Ok(id);
        }

        let child_count = usize::from(self.spatial_divisions).pow(3);
        let mut children = (0..child_count)
            .map(|_| None)
            .collect::<Vec<Option<ChildSpool>>>();
        let mut samples = std::iter::repeat_with(|| None)
            .take(SAMPLE_GRID_SIZE.pow(3))
            .collect::<Vec<Option<SampleAccumulator>>>();
        let sample_cell_size = cube_side(bounds) / SAMPLE_GRID_SIZE as f64;
        let sample_error = (3.0_f64).sqrt() * sample_cell_size / 2.0;
        let mut source_bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 3];
        let subdivision = SpatialSubdivision::new(bounds, self.spatial_divisions)
            .expect("converter subdivisions are validated");
        let mut scanned = 0_u64;
        let mut source_reader =
            BufReader::with_capacity(SPOOL_IO_BUFFER_BYTES, File::open(&spool_path)?);
        let record_bytes = self.layout.record_bytes();
        let records_per_batch = (SPOOL_IO_BUFFER_BYTES / record_bytes).max(1);
        let mut record_buffer = vec![0; records_per_batch * record_bytes];
        loop {
            let read_started = self.spatial_phases.as_ref().map(|_| Instant::now());
            let records = read_records(&mut source_reader, &mut record_buffer, self.layout)?;
            self.record_phase_elapsed(read_started, SpatialPhase::SpoolScanRead);
            if records == 0 {
                break;
            }
            self.add_scanned_spool_bytes(records, record_bytes, input_is_child_spool)?;
            let classify_started = self.spatial_phases.as_ref().map(|_| Instant::now());
            for bytes in record_buffer[..records * record_bytes].chunks_exact(record_bytes) {
                let point = decode_point(bytes, self.layout);
                scanned += 1;
                for (axis, axis_bounds) in source_bounds.iter_mut().enumerate() {
                    axis_bounds[0] = axis_bounds[0].min(point.position[axis]);
                    axis_bounds[1] = axis_bounds[1].max(point.position[axis]);
                }
                if scanned.is_multiple_of(self.point_limit) {
                    self.emit_plan_progress()?;
                }
                let child_key = subdivision.cell_index(point.position);
                let child = children[child_key].get_or_insert_with(|| {
                    ChildSpool::new(
                        self.workspace
                            .file(&format!("node-{}-child-{child_key}.spool", id.get())),
                        self.layout,
                        self.spool_diagnostics.is_some(),
                    )
                });
                child.write_point(&point)?;

                let voxel_key = sample_index(point.position, bounds);
                let center = voxel_center(voxel_key, bounds);
                let sample = samples[voxel_key].get_or_insert_with(SampleAccumulator::default);
                sample.add(point, center);
            }
            self.record_phase_elapsed(classify_started, SpatialPhase::SpoolPartitionAndSample);
        }
        if scanned != source_count {
            return Err(ConvertError::PointCountMismatch {
                expected: source_count,
                actual: scanned,
            });
        }
        for child in children.iter_mut().flatten() {
            child.finish()?;
        }
        self.record_child_spool_writes(&children)?;

        if source_bounds
            .iter()
            .all(|axis_bounds| axis_bounds[0] == axis_bounds[1])
        {
            for child in children.into_iter().flatten() {
                fs::remove_file(child.path)?;
            }
            self.add_leaf(id, spool_path, source_count, bounds, parent, level)?;
            return Ok(id);
        }

        let lod_write_started = self.spatial_phases.as_ref().map(|_| Instant::now());
        let sample_path = self.workspace.file(&format!("node-{}.lod", id.get()));
        let mut sample_writer = BufWriter::new(File::create(&sample_path)?);
        for (key, accumulator) in samples
            .iter()
            .enumerate()
            .filter_map(|(key, sample)| sample.as_ref().map(|accumulator| (key, accumulator)))
        {
            write_point(
                &mut sample_writer,
                &accumulator.finish(voxel_center(key, bounds)),
                self.layout.lod(),
            )?;
        }
        sample_writer
            .into_inner()
            .map_err(|error| error.into_error())?;
        self.record_phase_elapsed(lod_write_started, SpatialPhase::LodSampleWrite);
        let sample_count = samples.iter().filter(|sample| sample.is_some()).count();
        self.add_lod_spool_bytes(sample_count, self.layout.lod())?;
        fs::remove_file(&spool_path)?;

        let sample_block = if sample_count > 0 {
            let block_id = self.allocate_block_id()?;
            self.output_blocks.push(PlannedBlock {
                id: block_id,
                path: sample_path,
                offset_points: 0,
                point_count: sample_count,
                layout: self.layout.lod(),
                is_source_points: false,
                final_block_in_spool: true,
            });
            vec![block_id]
        } else {
            Vec::new()
        };
        self.ensure_memory(self.node_records.len() + 1, self.output_blocks.len())?;
        self.node_records.push(NodeRecord {
            id,
            parent,
            level,
            bounds,
            point_count: sample_count as u64,
            geometric_error: sample_error + center_rounding_error(bounds, self.frame),
            blocks: sample_block,
        });
        self.emit_plan_progress()?;

        let child_tasks = self.child_tasks(bounds, children)?;
        self.process_child_tasks(id, level, child_tasks)?;
        Ok(id)
    }

    fn child_tasks(
        &self,
        bounds: SpatialBounds,
        children: Vec<Option<ChildSpool>>,
    ) -> Result<Vec<RootChildTask>, ConvertError> {
        let mut tasks = Vec::new();
        let subdivision = SpatialSubdivision::new(bounds, self.spatial_divisions)
            .expect("converter subdivisions are validated");
        for (child_key, child) in children.into_iter().enumerate() {
            let Some(child) = child else {
                continue;
            };
            if child.point_count == 0 {
                if child.created {
                    fs::remove_file(child.path)?;
                }
                continue;
            }
            tasks.push(RootChildTask {
                key: child_key,
                path: child.path,
                point_count: child.point_count,
                bounds: subdivision
                    .cell_bounds(child_key)
                    .expect("child index is in range"),
            });
        }
        Ok(tasks)
    }

    fn process_child_tasks(
        &mut self,
        parent_id: NodeId,
        level: u32,
        child_tasks: Vec<RootChildTask>,
    ) -> Result<(), ConvertError> {
        if level == 0 && self.parallel_workers > 1 && child_tasks.len() > 1 {
            self.build_root_children_parallel(parent_id, child_tasks)?;
        } else {
            for child in child_tasks {
                self.build_node(
                    child.path,
                    child.point_count,
                    child.bounds,
                    Some(parent_id),
                    level + 1,
                    true,
                )?;
            }
        }
        Ok(())
    }

    fn build_root_children_parallel(
        &mut self,
        parent_id: NodeId,
        tasks: Vec<RootChildTask>,
    ) -> Result<(), ConvertError> {
        let mut tasks = VecDeque::from(tasks);
        while !tasks.is_empty() {
            let batch_count = self.parallel_workers.min(tasks.len());
            let batch = (0..batch_count)
                .map(|_| {
                    tasks
                        .pop_front()
                        .expect("batch size is bounded by queued tasks")
                })
                .collect::<Vec<_>>();
            let worker_memory_budget = branch_memory_budget(
                self.memory_budget,
                self.point_limit,
                self.node_records.len(),
                self.output_blocks.len(),
                batch_count,
                self.spatial_divisions,
            )?;
            self.workers_used = self.workers_used.max(batch_count);

            let cancelled = Arc::new(AtomicBool::new(false));
            let (sender, receiver) = mpsc::sync_channel::<BranchMessage>(batch_count * 4);
            let workspace = self.workspace;
            let point_limit = self.point_limit;
            let frame = self.frame;
            let layout = self.layout;
            let spatial_divisions = self.spatial_divisions;
            let source_point_count = self.source_point_count;
            let collect_spool_diagnostics = self.spool_diagnostics.is_some();
            let mut outputs = Vec::with_capacity(batch_count);
            let mut first_error = None;

            thread::scope(|scope| {
                for task in batch {
                    let cancelled = Arc::clone(&cancelled);
                    let sender = sender.clone();
                    scope.spawn(move || {
                        let key = task.key;
                        let result = (|| {
                            let branch_workspace = Workspace::create_child(
                                &workspace.file(&format!("root-branch-{key}")),
                            )?;
                            let mut report_progress = |progress| {
                                !cancelled.load(Ordering::Relaxed)
                                    && sender.send(BranchMessage::Progress(progress)).is_ok()
                                    && !cancelled.load(Ordering::Relaxed)
                            };
                            let mut builder = SpatialBuilder {
                                workspace: &branch_workspace,
                                point_limit,
                                memory_budget: worker_memory_budget,
                                spatial_divisions,
                                frame,
                                layout,
                                node_records: Vec::new(),
                                output_blocks: Vec::new(),
                                next_block_id: 0,
                                on_progress: &mut report_progress,
                                source_point_count,
                                parallel_workers: 1,
                                workers_used: 1,
                                spool_diagnostics: collect_spool_diagnostics
                                    .then(SpatialSpoolDiagnostics::default),
                                spatial_phases: collect_spool_diagnostics
                                    .then(SpatialPhaseDiagnostics::default),
                            };
                            builder.build_node(
                                task.path,
                                task.point_count,
                                task.bounds,
                                None,
                                1,
                                true,
                            )?;
                            Ok(BranchOutput {
                                node_records: builder.node_records,
                                output_blocks: builder.output_blocks,
                                next_block_id: builder.next_block_id,
                                spool_diagnostics: builder.spool_diagnostics,
                                spatial_phases: builder.spatial_phases,
                            })
                        })();
                        if result.is_err() {
                            cancelled.store(true, Ordering::Relaxed);
                        }
                        let _ = sender.send(BranchMessage::Finished(key, result));
                    });
                }
                drop(sender);

                let mut finished_workers = 0;
                while finished_workers < batch_count {
                    match receiver.recv() {
                        Ok(BranchMessage::Progress(progress)) => {
                            if first_error.is_none()
                                && let Err(error) = emit_progress(self.on_progress, progress, 0)
                            {
                                cancelled.store(true, Ordering::Relaxed);
                                first_error = Some(error);
                            }
                        }
                        Ok(BranchMessage::Finished(key, result)) => {
                            match result {
                                Ok(output) => outputs.push((key, output)),
                                Err(error) if first_error.is_none() => {
                                    cancelled.store(true, Ordering::Relaxed);
                                    first_error = Some(error);
                                }
                                Err(error @ ConvertError::Cancelled { .. }) => {
                                    first_error.get_or_insert(error);
                                }
                                Err(error) => {
                                    cancelled.store(true, Ordering::Relaxed);
                                    if matches!(
                                        first_error.as_ref(),
                                        Some(ConvertError::Cancelled { .. })
                                    ) {
                                        first_error = Some(error);
                                    }
                                }
                            }
                            finished_workers += 1;
                        }
                        Err(_) => {
                            cancelled.store(true, Ordering::Relaxed);
                            first_error.get_or_insert_with(|| {
                                ConvertError::Io(io::Error::other(
                                    "spatial worker channel closed unexpectedly",
                                ))
                            });
                            break;
                        }
                    }
                }
            });

            if let Some(error) = first_error {
                return Err(error);
            }
            outputs.sort_by_key(|(key, _)| *key);
            for (_, mut branch) in outputs {
                self.append_branch(parent_id, &mut branch)?;
            }
        }
        Ok(())
    }

    fn append_branch(
        &mut self,
        parent_id: NodeId,
        branch: &mut BranchOutput,
    ) -> Result<(), ConvertError> {
        let node_offset =
            u64::try_from(self.node_records.len()).map_err(|_| ConvertError::SizeOverflow)?;
        let block_offset = self.next_block_id;
        let next_block_id = block_offset
            .checked_add(branch.next_block_id)
            .ok_or(ConvertError::SizeOverflow)?;
        if next_block_id > crate::options::MAX_BLOCKS || next_block_id >= u64::from(u32::MAX) {
            return Err(ConvertError::TooManyBlocks(next_block_id));
        }
        for node in &mut branch.node_records {
            node.id = NodeId::new(
                node.id
                    .get()
                    .checked_add(node_offset)
                    .ok_or(ConvertError::SizeOverflow)?,
            );
            node.parent = Some(match node.parent {
                Some(parent) => NodeId::new(
                    parent
                        .get()
                        .checked_add(node_offset)
                        .ok_or(ConvertError::SizeOverflow)?,
                ),
                None => parent_id,
            });
            for block in &mut node.blocks {
                *block = BlockId::new(
                    block
                        .get()
                        .checked_add(block_offset)
                        .ok_or(ConvertError::SizeOverflow)?,
                );
            }
        }
        for block in &mut branch.output_blocks {
            block.id = BlockId::new(
                block
                    .id
                    .get()
                    .checked_add(block_offset)
                    .ok_or(ConvertError::SizeOverflow)?,
            );
        }
        self.node_records.append(&mut branch.node_records);
        self.output_blocks.append(&mut branch.output_blocks);
        if let (Some(target), Some(source)) = (
            self.spool_diagnostics.as_mut(),
            branch.spool_diagnostics.as_ref(),
        ) {
            target.source_payload_written_bytes = target
                .source_payload_written_bytes
                .checked_add(source.source_payload_written_bytes)
                .ok_or(ConvertError::SizeOverflow)?;
            target.source_payload_scanned_bytes = target
                .source_payload_scanned_bytes
                .checked_add(source.source_payload_scanned_bytes)
                .ok_or(ConvertError::SizeOverflow)?;
            target.child_payload_written_bytes = target
                .child_payload_written_bytes
                .checked_add(source.child_payload_written_bytes)
                .ok_or(ConvertError::SizeOverflow)?;
            target.child_payload_scanned_bytes = target
                .child_payload_scanned_bytes
                .checked_add(source.child_payload_scanned_bytes)
                .ok_or(ConvertError::SizeOverflow)?;
            target.lod_payload_written_bytes = target
                .lod_payload_written_bytes
                .checked_add(source.lod_payload_written_bytes)
                .ok_or(ConvertError::SizeOverflow)?;
        }
        if let (Some(target), Some(source)) =
            (self.spatial_phases.as_mut(), branch.spatial_phases.as_ref())
        {
            target.root_las_fill_elapsed_ns = target
                .root_las_fill_elapsed_ns
                .saturating_add(source.root_las_fill_elapsed_ns);
            target.spool_scan_read_elapsed_ns = target
                .spool_scan_read_elapsed_ns
                .saturating_add(source.spool_scan_read_elapsed_ns);
            target.spool_partition_and_sample_elapsed_ns = target
                .spool_partition_and_sample_elapsed_ns
                .saturating_add(source.spool_partition_and_sample_elapsed_ns);
            target.lod_sample_write_elapsed_ns = target
                .lod_sample_write_elapsed_ns
                .saturating_add(source.lod_sample_write_elapsed_ns);
        }
        self.next_block_id = next_block_id;
        Ok(())
    }

    fn allocate_block_id(&mut self) -> Result<BlockId, ConvertError> {
        if self.next_block_id >= crate::options::MAX_BLOCKS
            || self.next_block_id >= u64::from(u32::MAX)
        {
            return Err(ConvertError::TooManyBlocks(self.next_block_id + 1));
        }
        self.ensure_memory(self.node_records.len(), self.output_blocks.len() + 1)?;
        let id = BlockId::new(self.next_block_id);
        self.next_block_id += 1;
        Ok(id)
    }

    fn add_leaf(
        &mut self,
        id: NodeId,
        spool_path: PathBuf,
        source_count: u64,
        bounds: SpatialBounds,
        parent: Option<NodeId>,
        level: u32,
    ) -> Result<(), ConvertError> {
        let mut node_blocks = Vec::new();
        let mut offset_points = 0_u64;
        while offset_points < source_count {
            let count = self
                .point_limit
                .min(source_count - offset_points)
                .try_into()
                .map_err(|_| ConvertError::SizeOverflow)?;
            let block_id = self.allocate_block_id()?;
            node_blocks.push(block_id);
            self.output_blocks.push(PlannedBlock {
                id: block_id,
                path: spool_path.clone(),
                offset_points,
                point_count: count,
                layout: self.layout,
                is_source_points: true,
                final_block_in_spool: offset_points + count as u64 == source_count,
            });
            offset_points += count as u64;
        }
        if source_count == 0 {
            fs::remove_file(spool_path)?;
        }
        self.ensure_memory(self.node_records.len() + 1, self.output_blocks.len())?;
        self.node_records.push(NodeRecord {
            id,
            parent,
            level,
            bounds,
            point_count: source_count,
            geometric_error: 0.0,
            blocks: node_blocks,
        });
        self.emit_plan_progress()
    }

    fn ensure_memory(&self, node_count: usize, block_count: usize) -> Result<(), ConvertError> {
        check_memory_budget(
            estimate_memory(
                self.point_limit,
                node_count,
                block_count,
                self.spatial_divisions,
            )?,
            self.memory_budget,
        )
    }

    fn emit_plan_progress(&mut self) -> Result<(), ConvertError> {
        emit_progress(
            self.on_progress,
            ConversionProgress {
                stage: ConversionStage::BuildingSpatialHierarchy,
                points_processed: 0,
                total_points: self.source_point_count,
                bytes_written: 0,
            },
            0,
        )
    }
}

#[derive(Default)]
struct SampleAccumulator {
    count: u64,
    display_color: [u128; 3],
    rgb: [u128; 3],
    nearest_distance_squared: f64,
    nearest_point: Option<SpoolPoint>,
}

impl SampleAccumulator {
    fn add(&mut self, point: SpoolPoint, center: [f64; 3]) {
        self.count += 1;
        for axis in 0..3 {
            self.display_color[axis] += u128::from(point.display_color_component(axis));
            self.rgb[axis] += u128::from(point.rgb[axis]);
        }
        let delta_x = point.position[0] - center[0];
        let delta_y = point.position[1] - center[1];
        let delta_z = point.position[2] - center[2];
        let distance_squared = delta_x * delta_x + delta_y * delta_y + delta_z * delta_z;
        if self
            .nearest_point
            .is_none_or(|_| distance_squared < self.nearest_distance_squared)
        {
            self.nearest_distance_squared = distance_squared;
            self.nearest_point = Some(point);
        }
    }

    fn finish(&self, center: [f64; 3]) -> SpoolPoint {
        let mut point = self.nearest_point.expect("sample has a source point");
        point.position = center;
        point.display_color = ColorRgb::new(
            average(self.display_color[0], self.count) as u8,
            average(self.display_color[1], self.count) as u8,
            average(self.display_color[2], self.count) as u8,
        );
        point.rgb = std::array::from_fn(|axis| average(self.rgb[axis], self.count) as u16);
        point
    }
}

impl SpoolPoint {
    fn display_color_component(self, axis: usize) -> u8 {
        match axis {
            0 => self.display_color.r,
            1 => self.display_color.g,
            _ => self.display_color.b,
        }
    }
}

fn average(sum: u128, count: u64) -> u128 {
    (sum + u128::from(count / 2)) / u128::from(count)
}

pub(crate) fn spool_source(
    reader: &mut Reader,
    source: &SourceInfo,
    options: ConvertOptions,
    spool_path: &Path,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    collect_diagnostics: bool,
) -> Result<Option<u64>, ConvertError> {
    let point_limit =
        crate::options::input_batch_point_limit(options.points_per_block(), source.record_length);
    let mut writer = BufWriter::new(File::create(spool_path)?);
    let mut point_data = PointDataBuilder::new().for_header(reader.header()).build();
    let mut processed = 0_u64;
    while processed < source.point_count {
        let requested = point_limit.min(source.point_count - processed);
        let read = crate::las_source::fill_points(reader, requested, &mut point_data)?;
        if read != requested {
            return Err(ConvertError::PointCountMismatch {
                expected: source.point_count,
                actual: processed + read,
            });
        }
        write_source_points(&point_data, source, processed, &mut writer)?;
        processed += read;
        emit_progress(
            on_progress,
            ConversionProgress {
                stage: ConversionStage::PlanningSpatialIndex,
                points_processed: processed,
                total_points: source.point_count,
                bytes_written: 0,
            },
            processed,
        )?;
    }
    writer
        .into_inner()
        .map_err(|error| error.into_error())?
        .sync_all()?;
    if collect_diagnostics {
        Ok(Some(
            source
                .point_count
                .checked_mul(SpoolLayout::source(source).record_bytes() as u64)
                .ok_or(ConvertError::SizeOverflow)?,
        ))
    } else {
        Ok(None)
    }
}

fn write_source_points(
    data: &PointData,
    source: &SourceInfo,
    processed: u64,
    writer: &mut impl Write,
) -> Result<(), ConvertError> {
    let layout = SpoolLayout::source(source);
    let mut encoded = Vec::with_capacity(CHILD_SPOOL_BUFFER_BYTES);
    visit_source_points(data, source, processed, |point| {
        if encoded.len() + layout.record_bytes() > CHILD_SPOOL_BUFFER_BYTES {
            writer.write_all(&encoded)?;
            encoded.clear();
        }
        write_point(&mut encoded, &point, layout).map_err(ConvertError::Io)
    })?;
    if !encoded.is_empty() {
        writer.write_all(&encoded)?;
    }
    Ok(())
}

fn visit_source_points(
    data: &PointData,
    source: &SourceInfo,
    processed: u64,
    mut visitor: impl FnMut(SpoolPoint) -> Result<(), ConvertError>,
) -> Result<(), ConvertError> {
    let mut intensities = data.intensity();
    let mut classifications = data.classification();
    let mut return_numbers = data.return_number();
    let mut returns_per_pulse = data.number_of_returns();
    let mut scan_angles = data.scan_angle_degrees();
    let mut user_data = data.user_data();
    let mut point_source_ids = data.point_source_id();
    let mut gps_times = data.gps_time();
    let mut nir_values = data.nir();
    let mut rgb_values = data.rgb();
    if source.point_format.has_gps_time != gps_times.is_some()
        || source.point_format.has_nir != nir_values.is_some()
        || source.point_format.has_color != rgb_values.is_some()
    {
        return Err(ConvertError::InvalidAttributeSchema);
    }
    let layout = SpoolLayout::source(source);
    for (index, ((x, y), z)) in data.x_raw().zip(data.y_raw()).zip(data.z_raw()).enumerate() {
        let raw_position = [x, y, z];
        let position = layout.position(raw_position);
        let local = std::array::from_fn(|axis| position[axis] - source.frame.origin()[axis]);
        if position.iter().any(|coordinate| !coordinate.is_finite())
            || local.iter().any(|coordinate| !coordinate.is_finite())
        {
            return Err(ConvertError::CoordinateFrame(
                viewer_core::CoordinateFrameError::PositionOutOfRange(local),
            ));
        }
        if outside_header_bounds(position, source.bounds) {
            return Err(ConvertError::PointOutsideHeaderBounds {
                point_index: processed + index as u64,
                position,
            });
        }
        let intensity = intensities
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let classification = classifications
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let return_number = return_numbers
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let returns_per_pulse = returns_per_pulse
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let scan_angle = scan_angles
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let user_data = user_data
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let point_source_id = point_source_ids
            .next()
            .ok_or(ConvertError::InvalidAttributeSchema)?;
        let gps_time = gps_times
            .as_mut()
            .map(|values| values.next().ok_or(ConvertError::InvalidAttributeSchema))
            .transpose()?
            .unwrap_or(0.0);
        let nir = nir_values
            .as_mut()
            .map(|values| values.next().ok_or(ConvertError::InvalidAttributeSchema))
            .transpose()?
            .unwrap_or(0);
        let rgb = rgb_values
            .as_mut()
            .map(|values| values.next().ok_or(ConvertError::InvalidAttributeSchema))
            .transpose()?
            .map(|(red, green, blue)| [red, green, blue])
            .unwrap_or_default();
        let display_color = if source.point_format.has_color {
            ColorRgb::new(
                (rgb[0] >> 8) as u8,
                (rgb[1] >> 8) as u8,
                (rgb[2] >> 8) as u8,
            )
        } else {
            let gray = (intensity >> 8) as u8;
            ColorRgb::new(gray, gray, gray)
        };
        visitor(SpoolPoint {
            raw_position,
            position,
            intensity,
            classification,
            return_number,
            returns_per_pulse,
            scan_angle,
            user_data,
            point_source_id,
            gps_time,
            rgb,
            nir,
            display_color,
        })?;
    }
    if intensities.next().is_some()
        || classifications.next().is_some()
        || return_numbers.next().is_some()
        || returns_per_pulse.next().is_some()
        || scan_angles.next().is_some()
        || user_data.next().is_some()
        || point_source_ids.next().is_some()
        || gps_times
            .as_mut()
            .is_some_and(|values| values.next().is_some())
        || nir_values
            .as_mut()
            .is_some_and(|values| values.next().is_some())
        || rgb_values
            .as_mut()
            .is_some_and(|values| values.next().is_some())
    {
        return Err(ConvertError::InvalidAttributeSchema);
    }
    Ok(())
}

fn outside_header_bounds(position: [f64; 3], bounds: SpatialBounds) -> bool {
    (0..3).any(|axis| {
        let min = bounds.min()[axis];
        let max = bounds.max()[axis];
        let coordinate_magnitude = position[axis].abs().max(min.abs()).max(max.abs());
        let tolerance = (coordinate_magnitude * HEADER_BOUNDS_RELATIVE_TOLERANCE)
            .min(HEADER_BOUNDS_MAX_TOLERANCE);
        position[axis] < min - tolerance || position[axis] > max + tolerance
    })
}

fn write_point(writer: &mut impl Write, point: &SpoolPoint, layout: SpoolLayout) -> io::Result<()> {
    let mut record = [0_u8; MAX_SPOOL_RECORD_BYTES];
    let mut offset = 0;
    macro_rules! append_bytes {
        ($bytes:expr) => {{
            let bytes = $bytes;
            let end = offset + bytes.len();
            record[offset..end].copy_from_slice(&bytes);
            offset = end;
        }};
    }
    if layout.source_coordinates {
        for coordinate in point.raw_position {
            append_bytes!(coordinate.to_le_bytes());
        }
    } else {
        for coordinate in point.position {
            append_bytes!(coordinate.to_le_bytes());
        }
    }
    append_bytes!(point.intensity.to_le_bytes());
    append_bytes!([
        point.classification,
        point.return_number,
        point.returns_per_pulse,
    ]);
    append_bytes!(point.scan_angle.to_le_bytes());
    append_bytes!([point.user_data]);
    append_bytes!(point.point_source_id.to_le_bytes());
    if layout.has_gps_time {
        append_bytes!(point.gps_time.to_le_bytes());
    }
    if layout.has_color {
        for channel in point.rgb {
            append_bytes!(channel.to_le_bytes());
        }
    }
    if layout.has_nir {
        append_bytes!(point.nir.to_le_bytes());
    }
    append_bytes!([
        point.display_color.r,
        point.display_color.g,
        point.display_color.b,
    ]);
    writer.write_all(&record[..offset])
}

fn read_records(
    reader: &mut impl Read,
    buffer: &mut [u8],
    layout: SpoolLayout,
) -> io::Result<usize> {
    let mut bytes_read = 0;
    while bytes_read < buffer.len() {
        match reader.read(&mut buffer[bytes_read..]) {
            Ok(0) => break,
            Ok(count) => bytes_read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    let record_bytes = layout.record_bytes();
    if !bytes_read.is_multiple_of(record_bytes) {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    Ok(bytes_read / record_bytes)
}

#[cfg(test)]
fn read_point_if_present(
    reader: &mut impl Read,
    layout: SpoolLayout,
) -> io::Result<Option<SpoolPoint>> {
    let mut buffer = [0_u8; MAX_SPOOL_RECORD_BYTES];
    let bytes = &mut buffer[..layout.record_bytes()];
    let read = reader.read(&mut bytes[..1])?;
    if read == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut bytes[1..])?;
    Ok(Some(decode_point(bytes, layout)))
}

#[cfg(test)]
fn read_point(reader: &mut impl Read, layout: SpoolLayout) -> Result<SpoolPoint, ConvertError> {
    read_point_if_present(reader, layout)?
        .ok_or_else(|| ConvertError::Io(io::Error::from(io::ErrorKind::UnexpectedEof)))
}

fn decode_point(bytes: &[u8], layout: SpoolLayout) -> SpoolPoint {
    let mut offset = 0;
    let mut raw_position = [0; 3];
    let position = if layout.source_coordinates {
        for coordinate in &mut raw_position {
            *coordinate =
                i32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("fixed width"));
            offset += 4;
        }
        layout.position(raw_position)
    } else {
        std::array::from_fn(|_| read_f64(bytes, &mut offset))
    };
    let intensity = read_u16(bytes, &mut offset);
    let classification = bytes[offset];
    let return_number = bytes[offset + 1];
    let returns_per_pulse = bytes[offset + 2];
    offset += 3;
    let scan_angle = f32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("fixed width"));
    offset += 4;
    let user_data = bytes[offset];
    offset += 1;
    let point_source_id = read_u16(bytes, &mut offset);
    let gps_time = if layout.has_gps_time {
        read_f64(bytes, &mut offset)
    } else {
        0.0
    };
    let rgb = if layout.has_color {
        std::array::from_fn(|_| read_u16(bytes, &mut offset))
    } else {
        [0; 3]
    };
    let nir = if layout.has_nir {
        read_u16(bytes, &mut offset)
    } else {
        0
    };
    let display_color = ColorRgb::new(bytes[offset], bytes[offset + 1], bytes[offset + 2]);
    SpoolPoint {
        raw_position,
        position,
        intensity,
        classification,
        return_number,
        returns_per_pulse,
        scan_angle,
        user_data,
        point_source_id,
        gps_time,
        rgb,
        nir,
        display_color,
    }
}

fn read_u16(bytes: &[u8], offset: &mut usize) -> u16 {
    let value = u16::from_le_bytes([bytes[*offset], bytes[*offset + 1]]);
    *offset += 2;
    value
}

fn read_f64(bytes: &[u8], offset: &mut usize) -> f64 {
    let value = f64::from_le_bytes(bytes[*offset..*offset + 8].try_into().expect("fixed width"));
    *offset += 8;
    value
}

fn cube_bounds(bounds: SpatialBounds) -> SpatialBounds {
    let side = (0..3)
        .map(|axis| bounds.max()[axis] - bounds.min()[axis])
        .fold(0.0, f64::max);
    if side == 0.0 {
        return bounds;
    }
    let min = bounds.min();
    let max = std::array::from_fn(|axis| (min[axis] + side).max(bounds.max()[axis]));
    SpatialBounds::new(min, max).expect("finite source bounds produce finite cube bounds")
}

fn cube_side(bounds: SpatialBounds) -> f64 {
    (0..3)
        .map(|axis| bounds.max()[axis] - bounds.min()[axis])
        .fold(0.0, f64::max)
}

#[cfg(test)]
fn child_index(position: [f64; 3], bounds: SpatialBounds, divisions: u8) -> usize {
    SpatialSubdivision::new(bounds, divisions)
        .expect("converter subdivisions are validated")
        .cell_index(position)
}

#[cfg(test)]
fn child_bounds(bounds: SpatialBounds, child: usize, divisions: u8) -> SpatialBounds {
    SpatialSubdivision::new(bounds, divisions)
        .expect("converter subdivisions are validated")
        .cell_bounds(child)
        .expect("child index is in range")
}

fn sample_index(position: [f64; 3], bounds: SpatialBounds) -> usize {
    let side = cube_side(bounds);
    let mut cell = [0_usize; 3];
    for axis in 0..3 {
        let scaled =
            ((position[axis] - bounds.min()[axis]) / side * SAMPLE_GRID_SIZE as f64).floor();
        cell[axis] = (scaled as usize).min(SAMPLE_GRID_SIZE - 1);
    }
    (cell[0] * SAMPLE_GRID_SIZE + cell[1]) * SAMPLE_GRID_SIZE + cell[2]
}

fn voxel_center(index: usize, bounds: SpatialBounds) -> [f64; 3] {
    let cell = [
        index / (SAMPLE_GRID_SIZE * SAMPLE_GRID_SIZE),
        (index / SAMPLE_GRID_SIZE) % SAMPLE_GRID_SIZE,
        index % SAMPLE_GRID_SIZE,
    ];
    let cell_size = cube_side(bounds) / SAMPLE_GRID_SIZE as f64;
    std::array::from_fn(|axis| bounds.min()[axis] + (cell[axis] as f64 + 0.5) * cell_size)
}

fn center_rounding_error(bounds: SpatialBounds, frame: viewer_core::CoordinateFrame) -> f64 {
    (0..3)
        .map(|axis| {
            let min = bounds.min()[axis] - frame.origin()[axis];
            let max = bounds.max()[axis] - frame.origin()[axis];
            let coordinate = if min.abs() > max.abs() { min } else { max };
            let rounded = coordinate as f32;
            let magnitude_bits = rounded.abs().to_bits();
            let magnitude = f64::from(rounded.abs());
            let lower = if magnitude_bits == 0 {
                0.0
            } else {
                magnitude - f64::from(f32::from_bits(magnitude_bits - 1))
            };
            let upper = magnitude_bits
                .checked_add(1)
                .map(f32::from_bits)
                .filter(|next| next.is_finite())
                .map(|next| f64::from(next) - magnitude)
                .unwrap_or(lower);
            lower.max(upper) / 2.0
        })
        .map(|error| error * error)
        .sum::<f64>()
        .sqrt()
}

fn estimate_memory(
    point_limit: u64,
    node_count: usize,
    block_count: usize,
    spatial_divisions: u8,
) -> Result<u64, ConvertError> {
    let node_bytes = u64::try_from(node_count)
        .map_err(|_| ConvertError::SizeOverflow)?
        .checked_mul(PLANNED_NODE_BYTES)
        .ok_or(ConvertError::SizeOverflow)?;
    let block_bytes = u64::try_from(block_count)
        .map_err(|_| ConvertError::SizeOverflow)?
        .checked_mul(crate::options::ESTIMATED_DIRECTORY_BYTES_PER_BLOCK)
        .ok_or(ConvertError::SizeOverflow)?;
    let working_bytes = point_limit
        .checked_mul(crate::options::ESTIMATED_WORKING_BYTES_PER_POINT)
        .ok_or(ConvertError::SizeOverflow)?;
    let sample_accumulators = (SAMPLE_GRID_SIZE.pow(3) as u64)
        .checked_mul(256)
        .ok_or(ConvertError::SizeOverflow)?;
    let child_spool_buffers = child_spool_buffer_bytes(spatial_divisions)?;
    working_bytes
        .checked_add(node_bytes)
        .and_then(|bytes| bytes.checked_add(block_bytes))
        .and_then(|bytes| bytes.checked_add(sample_accumulators))
        .and_then(|bytes| bytes.checked_add(child_spool_buffers))
        .and_then(|bytes| bytes.checked_add(crate::options::ESTIMATED_FIXED_OVERHEAD_BYTES))
        .ok_or(ConvertError::SizeOverflow)
}

fn child_spool_buffer_bytes(spatial_divisions: u8) -> Result<u64, ConvertError> {
    u64::from(spatial_divisions)
        .checked_pow(3)
        .and_then(|children| children.checked_mul(CHILD_SPOOL_BUFFER_BYTES as u64))
        .ok_or(ConvertError::SizeOverflow)
}

fn branch_memory_budget(
    total_budget: u64,
    point_limit: u64,
    completed_nodes: usize,
    completed_blocks: usize,
    worker_count: usize,
    spatial_divisions: u8,
) -> Result<u64, ConvertError> {
    let base = estimate_memory(point_limit, 0, 0, spatial_divisions)?;
    let current = estimate_memory(
        point_limit,
        completed_nodes,
        completed_blocks,
        spatial_divisions,
    )?;
    let current_index = current.saturating_sub(base);
    let workers = u64::try_from(worker_count).map_err(|_| ConvertError::SizeOverflow)?;
    let worker_overhead = workers
        .checked_mul(SPATIAL_WORKER_OVERHEAD_BYTES)
        .ok_or(ConvertError::SizeOverflow)?;
    let additional_spool_buffers = workers
        .saturating_sub(1)
        .checked_mul(child_spool_buffer_bytes(spatial_divisions)?)
        .ok_or(ConvertError::SizeOverflow)?;
    let reserved = base
        .checked_add(
            current_index
                .checked_mul(2)
                .ok_or(ConvertError::SizeOverflow)?,
        )
        .and_then(|bytes| bytes.checked_add(worker_overhead))
        .and_then(|bytes| bytes.checked_add(additional_spool_buffers))
        .ok_or(ConvertError::SizeOverflow)?;
    let available =
        total_budget
            .checked_sub(reserved)
            .ok_or(ConvertError::MemoryBudgetExceeded {
                requested: reserved,
                limit: total_budget,
            })?;
    let concurrent_index_share = available / workers / 2;
    base.checked_add(concurrent_index_share)
        .ok_or(ConvertError::SizeOverflow)
}

fn check_memory_budget(requested: u64, limit: u64) -> Result<(), ConvertError> {
    if requested > limit {
        Err(ConvertError::MemoryBudgetExceeded { requested, limit })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod partition_tests {
    use super::*;

    fn spool_fixture(format_id: u8) -> (SourceInfo, SpoolPoint) {
        let transforms = las::Vector {
            x: las::Transform {
                scale: 0.000_003_7,
                offset: 1.0e16,
            },
            y: las::Transform {
                scale: 0.031_415_926_535,
                offset: -9.0e14,
            },
            z: las::Transform {
                scale: 0.000_123_456_789,
                offset: 8.0e12,
            },
        };
        let raw_position = [i32::MAX - 19, i32::MIN + 31, -123_456_789];
        let position = [
            transforms.x.direct(raw_position[0]),
            transforms.y.direct(raw_position[1]),
            transforms.z.direct(raw_position[2]),
        ];
        let source = SourceInfo {
            bounds: SpatialBounds::new(position, position).unwrap(),
            frame: viewer_core::CoordinateFrame::new(position).unwrap(),
            point_count: 1,
            record_length: las::point::Format::new(format_id).unwrap().len() as usize,
            point_format: las::point::Format::new(format_id).unwrap(),
            transforms,
            has_crs: false,
        };
        let point = SpoolPoint {
            raw_position,
            position,
            intensity: 45_678,
            classification: 7,
            return_number: 2,
            returns_per_pulse: 3,
            scan_angle: -12.5,
            user_data: 123,
            point_source_id: 12_345,
            gps_time: 123_456.789,
            rgb: [12_345, 23_456, 34_567],
            nir: 54_321,
            display_color: ColorRgb::new(71, 82, 93),
        };
        (source, point)
    }

    #[test]
    fn source_spools_preserve_exact_raw_xyz_with_large_origins() {
        let (source, fixture) = spool_fixture(3);
        let layout = SpoolLayout::source(&source);
        let mut las_bytes = vec![0; source.record_length];
        for (axis, coordinate) in fixture.raw_position.iter().enumerate() {
            las_bytes[axis * 4..axis * 4 + 4].copy_from_slice(&coordinate.to_le_bytes());
        }
        let data = PointDataBuilder::new()
            .with_format(source.point_format)
            .with_transforms(source.transforms)
            .build_from_bytes(las_bytes)
            .unwrap();
        let expected_position = [
            data.x().next().unwrap(),
            data.y().next().unwrap(),
            data.z().next().unwrap(),
        ];
        // The large origin loses low bits in f64; an inverse transform cannot recover the raw value.
        assert_ne!(
            source.transforms.x.inverse(expected_position[0]).ok(),
            Some(fixture.raw_position[0])
        );
        let mut bytes = Vec::new();
        write_source_points(&data, &source, 0, &mut bytes).unwrap();
        assert_eq!(bytes.len(), 41);
        assert_eq!(
            &bytes[..12],
            &fixture
                .raw_position
                .iter()
                .flat_map(|coordinate| coordinate.to_le_bytes())
                .collect::<Vec<_>>()[..]
        );
        let decoded = read_point(&mut bytes.as_slice(), layout).unwrap();
        assert_eq!(decoded.raw_position, fixture.raw_position);
        assert_eq!(
            decoded.position.map(f64::to_bits),
            expected_position.map(f64::to_bits)
        );
        let mut rewritten = Vec::new();
        write_point(&mut rewritten, &decoded, layout).unwrap();
        assert_eq!(bytes, rewritten);
    }

    #[test]
    fn child_spool_diagnostics_count_only_flushed_payload() {
        let (source, point) = spool_fixture(3);
        let layout = SpoolLayout::source(&source);
        let temp = tempfile::TempDir::new().unwrap();
        let workspace = Workspace::create_at(&temp.path().join("workspace")).unwrap();
        let mut child = ChildSpool::new(workspace.file("child.spool"), layout, true);

        child.write_point(&point).unwrap();
        assert_eq!(child.payload_written_bytes, Some(0));
        child.flush().unwrap();
        assert_eq!(
            child.payload_written_bytes,
            Some(layout.record_bytes() as u64)
        );
    }

    #[test]
    fn compact_layout_sizes_and_optional_attributes_match_source_format() {
        for (format_id, source_bytes, lod_bytes) in
            [(0, 27, 39), (2, 33, 45), (3, 41, 53), (8, 43, 55)]
        {
            let (source, fixture) = spool_fixture(format_id);
            let source_layout = SpoolLayout::source(&source);
            for (layout, expected_bytes) in [
                (source_layout, source_bytes),
                (source_layout.lod(), lod_bytes),
            ] {
                let mut bytes = Vec::new();
                write_point(&mut bytes, &fixture, layout).unwrap();
                assert_eq!(layout.record_bytes(), expected_bytes);
                assert_eq!(bytes.len(), expected_bytes);
                let decoded = read_point(&mut bytes.as_slice(), layout).unwrap();
                assert_eq!(decoded.intensity, fixture.intensity);
                assert_eq!(decoded.classification, fixture.classification);
                assert_eq!(decoded.return_number, fixture.return_number);
                assert_eq!(decoded.returns_per_pulse, fixture.returns_per_pulse);
                assert_eq!(decoded.scan_angle, fixture.scan_angle);
                assert_eq!(decoded.user_data, fixture.user_data);
                assert_eq!(decoded.point_source_id, fixture.point_source_id);
                assert_eq!(decoded.display_color, fixture.display_color);
                assert_eq!(
                    decoded.gps_time,
                    if layout.has_gps_time {
                        fixture.gps_time
                    } else {
                        0.0
                    }
                );
                assert_eq!(
                    decoded.rgb,
                    if layout.has_color {
                        fixture.rgb
                    } else {
                        [0; 3]
                    }
                );
                assert_eq!(decoded.nir, if layout.has_nir { fixture.nir } else { 0 });
            }
        }
    }

    #[test]
    fn synthetic_lod_centers_preserve_f64_bits_outside_las_grid() {
        let (source, fixture) = spool_fixture(3);
        let layout = SpoolLayout::source(&source).lod();
        let center = [
            0.123_456_789_012_345,
            -1.234_567_890_123_456,
            987_654.321_098_765_4,
        ];
        let mut sample = SampleAccumulator::default();
        sample.add(fixture, center);
        let synthetic = sample.finish(center);
        let mut bytes = Vec::new();
        write_point(&mut bytes, &synthetic, layout).unwrap();
        assert_eq!(bytes.len(), 53);
        let decoded = read_point(&mut bytes.as_slice(), layout).unwrap();
        assert_eq!(decoded.position.map(f64::to_bits), center.map(f64::to_bits));
        assert_eq!(decoded.gps_time, fixture.gps_time);
        assert_eq!(decoded.rgb, fixture.rgb);
        assert_eq!(decoded.display_color, fixture.display_color);
    }

    #[test]
    fn planned_blocks_seek_using_their_compact_record_layout() {
        let temp = tempfile::TempDir::new().unwrap();
        let workspace = Workspace::create(&temp.path().join("compact-spool.oct")).unwrap();
        for format_id in [0, 2, 3, 8] {
            let (source, fixture) = spool_fixture(format_id);
            let source_layout = SpoolLayout::source(&source);
            for layout in [source_layout, source_layout.lod()] {
                let mut second = fixture;
                second.raw_position[2] += 123;
                second.position = layout.position(second.raw_position);
                second.intensity += 1;
                let path = workspace.file("points.spool");
                let mut file = File::create(&path).unwrap();
                write_point(&mut file, &fixture, layout).unwrap();
                write_point(&mut file, &second, layout).unwrap();
                drop(file);
                let block = PlannedBlock {
                    id: BlockId::new(0),
                    path,
                    offset_points: 1,
                    point_count: 1,
                    layout,
                    is_source_points: layout.source_coordinates,
                    final_block_in_spool: true,
                }
                .read(source.frame)
                .unwrap();
                assert_eq!(
                    block.positions_local(),
                    &[std::array::from_fn(
                        |axis| second.position[axis] - source.frame.origin()[axis]
                    )]
                );
                assert_eq!(block.colors(), &[second.display_color]);
                assert_eq!(
                    block.attributes()[0].values(),
                    &AttributeValues::U16(vec![second.intensity])
                );
                assert_eq!(
                    block.attributes().len(),
                    7 + usize::from(layout.has_gps_time)
                        + usize::from(layout.has_color)
                        + usize::from(layout.has_nir)
                );
            }
        }
    }

    #[test]
    fn bulk_spool_reads_decode_records_and_reject_partial_tails() {
        for format_id in [0, 2, 3, 8] {
            let (source, fixture) = spool_fixture(format_id);
            let layout = SpoolLayout::source(&source);
            let mut expected = fixture;
            expected.raw_position[0] += 19;
            expected.position = layout.position(expected.raw_position);
            let mut bytes = Vec::new();
            write_point(&mut bytes, &fixture, layout).unwrap();
            write_point(&mut bytes, &expected, layout).unwrap();

            let mut reader = bytes.as_slice();
            let mut batch = vec![0; layout.record_bytes() * 3];
            assert_eq!(read_records(&mut reader, &mut batch, layout).unwrap(), 2);
            let decoded = batch
                .chunks_exact(layout.record_bytes())
                .map(|record| decode_point(record, layout))
                .collect::<Vec<_>>();
            assert_eq!(decoded[0].raw_position, fixture.raw_position);
            assert_eq!(decoded[1].raw_position, expected.raw_position);
            assert_eq!(decoded[1].intensity, expected.intensity);

            bytes.push(0);
            let mut truncated = bytes.as_slice();
            let error = read_records(&mut truncated, &mut batch, layout).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        }
    }

    #[test]
    fn header_bounds_allow_small_metadata_rounding_but_reject_real_outliers() {
        let bounds = SpatialBounds::new(
            [692_199.5, 4_285_716.0, 329.910_095_214_843_75],
            [692_838.687_5, 4_286_262.5, 359.015_899_658_203_1],
        )
        .unwrap();

        assert!(!outside_header_bounds(
            [692_399.511_1, 4_285_715.937_7, 335.174],
            bounds
        ));
        assert!(outside_header_bounds(
            [692_399.511_1, 4_285_715.0, 335.174],
            bounds
        ));
    }

    #[test]
    fn adaptive_cells_cover_centers_and_assign_internal_planes_to_upper_cell() {
        let bounds = SpatialBounds::new([-4.0, 2.0, -8.0], [4.0, 10.0, 0.0]).unwrap();
        for divisions in [2_u8, 4, 8] {
            let divisions_usize = usize::from(divisions);
            for x in 0..divisions_usize {
                for y in 0..divisions_usize {
                    for z in 0..divisions_usize {
                        let key = (x * divisions_usize + y) * divisions_usize + z;
                        let child = child_bounds(bounds, key, divisions);
                        let position = std::array::from_fn(|axis| {
                            (child.min()[axis] + child.max()[axis]) / 2.0
                        });
                        assert_eq!(child_index(position, bounds, divisions), key);
                        for axis in 0..3 {
                            assert!(position[axis] >= child.min()[axis]);
                            assert!(position[axis] <= child.max()[axis]);
                            if [x, y, z][axis] + 1 < divisions_usize {
                                let plane = child.max()[axis];
                                let mut on_plane = position;
                                on_plane[axis] = plane;
                                let next_key = child_index(on_plane, bounds, divisions);
                                let expected_cell = [x, y, z][axis] + 1;
                                let actual_cell = match axis {
                                    0 => next_key / (divisions_usize * divisions_usize),
                                    1 => (next_key / divisions_usize) % divisions_usize,
                                    _ => next_key % divisions_usize,
                                };
                                assert_eq!(actual_cell, expected_cell);
                            }
                        }
                    }
                }
            }
            assert_eq!(
                child_index(bounds.max(), bounds, divisions),
                divisions_usize.pow(3) - 1
            );
        }
    }

    #[test]
    fn binary_partition_keeps_midpoint_and_key_order() {
        let bounds = SpatialBounds::new([0.0; 3], [8.0; 3]).unwrap();
        assert_eq!(child_index([3.0; 3], bounds, 2), 0);
        assert_eq!(child_index([4.0; 3], bounds, 2), 7);
        let upper = child_bounds(bounds, 7, 2);
        assert_eq!(upper.min(), [4.0; 3]);
        assert_eq!(upper.max(), [8.0; 3]);
    }

    #[test]
    fn fanout_spool_memory_is_bounded_for_supported_divisions() {
        assert_eq!(child_spool_buffer_bytes(2).unwrap(), 8 * 32 * 1024);
        assert_eq!(child_spool_buffer_bytes(4).unwrap(), 64 * 32 * 1024);
        assert_eq!(child_spool_buffer_bytes(8).unwrap(), 512 * 32 * 1024);
    }
}
