use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::BufReader,
    path::{Path, PathBuf},
    sync::mpsc,
    time::UNIX_EPOCH,
};

use las::PointDataBuilder;
use serde::Deserialize;
use viewer_core::{
    AttributeColumn, AttributeValues, BlockId, ColorRgb, CoordinateFrame, DatasetMetadata,
    HierarchySemantics, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    FormatMetadata, FormatWriter, FormatWriterOptions, OctreeWriter, SourcePointBlock,
};

use super::ept_checkpoint::{EptCheckpoint, EptCheckpointStage, EptJournalRecord, EptSourceFile};
use super::{CheckpointMode, ConvertError, ConvertOptions};
use crate::diagnostics::{ConversionDiagnostics, DiagnosticTimer};
use crate::las_source::{next_descriptor, omitted_dimensions, open_las_reader, source_schema};
use crate::options::{MAX_BLOCKS, input_batch_point_limit};
use crate::publish::{
    OutputValidation, create_temporary_pair, output_attributes_path, publish_output_pair,
    publish_validated_output_pair, validate_output_pair, visual_scale_for_bounds,
};
use crate::report::{ConversionProgress, ConversionReport, ConversionStage, emit_progress};

const MAX_EPT_NODES: usize = 1_000_000;
const MAX_EPT_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_EPT_HIERARCHY_PAGE_BYTES: u64 = 64 * 1024 * 1024;
const ESTIMATED_EPT_NODE_BYTES: u64 = 768;
const ESTIMATED_EPT_BLOCK_BYTES: u64 = 512;

#[derive(Deserialize)]
struct EptManifest {
    #[serde(rename = "dataType")]
    data_type: String,
    #[serde(rename = "hierarchyType")]
    hierarchy_type: String,
    points: u64,
    bounds: [f64; 6],
    schema: serde_json::Value,
    #[serde(default)]
    srs: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct EptKey {
    depth: u32,
    x: u32,
    y: u32,
    z: u32,
}

impl EptKey {
    fn parse(value: &str) -> Result<Self, ConvertError> {
        let values = value
            .split('-')
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ConvertError::InvalidEpt(format!("invalid hierarchy key {value:?}")))?;
        let [depth, x, y, z] = values.as_slice() else {
            return Err(ConvertError::InvalidEpt(format!(
                "hierarchy key must have four integers: {value:?}"
            )));
        };
        if *depth > 30 {
            return Err(ConvertError::InvalidEpt(format!(
                "hierarchy depth exceeds 30 in key {value:?}"
            )));
        }
        let side = 1_u32 << depth;
        if *x >= side || *y >= side || *z >= side {
            return Err(ConvertError::InvalidEpt(format!(
                "hierarchy coordinates exceed depth in key {value:?}"
            )));
        }
        Ok(Self {
            depth: *depth,
            x: *x,
            y: *y,
            z: *z,
        })
    }

    fn name(self) -> String {
        format!("{}-{}-{}-{}", self.depth, self.x, self.y, self.z)
    }

    fn parent(self) -> Option<Self> {
        (self.depth > 0).then(|| Self {
            depth: self.depth - 1,
            x: self.x / 2,
            y: self.y / 2,
            z: self.z / 2,
        })
    }
}

struct EptNode {
    key: EptKey,
    id: NodeId,
    parent: Option<NodeId>,
    point_count: u64,
    bounds: SpatialBounds,
    path: Option<PathBuf>,
    block_ids: Vec<BlockId>,
    point_limit: u64,
}

pub(super) fn is_ept_input(path: &Path) -> bool {
    path.is_dir() || path.file_name().is_some_and(|name| name == "ept.json")
}

pub(super) fn convert(
    input_path: &Path,
    output_path: &Path,
    options: ConvertOptions,
    checkpoint_mode: Option<CheckpointMode>,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    mut diagnostics: Option<&mut ConversionDiagnostics>,
) -> Result<ConversionReport, ConvertError> {
    if options.format_version().uses_global_frame() {
        return Err(ConvertError::UnsupportedForGlobalFormat(
            "an EPT dataset (additive hierarchy)",
        ));
    }
    if output_path.exists() && checkpoint_mode != Some(CheckpointMode::Resume) {
        return Err(ConvertError::OutputExists(output_path.to_owned()));
    }
    let attributes_output_path = output_attributes_path(output_path);
    if attributes_output_path.exists() && checkpoint_mode != Some(CheckpointMode::Resume) {
        return Err(ConvertError::OutputExists(attributes_output_path));
    }
    let mut checkpoint = checkpoint_mode
        .map(|mode| super::ept_checkpoint::EptCheckpoint::open(output_path, mode, options))
        .transpose()?;
    if (output_path.exists() || attributes_output_path.exists())
        && checkpoint
            .as_ref()
            .is_none_or(|c| c.stage() != EptCheckpointStage::OutputReady)
    {
        return Err(ConvertError::OutputExists(output_path.to_owned()));
    }
    if options.spatial_divisions() != 2
        || (options.spatial_workers_explicit() && options.spatial_workers() != 1)
    {
        return Err(ConvertError::UnsupportedEpt(
            "spatial partition options apply to LAS/LAZ input; EPT topology is preserved"
                .to_owned(),
        ));
    }

    let spatial_planning_timer = diagnostics.as_ref().map(|_| DiagnosticTimer::start());
    let manifest_path = if input_path.is_dir() {
        input_path.join("ept.json")
    } else {
        input_path.to_owned()
    };
    let root = manifest_path.parent().unwrap_or(Path::new("."));
    ensure_file_size(&manifest_path, MAX_EPT_MANIFEST_BYTES, "ept.json")?;
    let manifest_before = checkpoint
        .as_ref()
        .map(|_| source_file_identity(root, &manifest_path))
        .transpose()?;
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
    let manifest: EptManifest =
        serde_json::from_reader(BufReader::new(File::open(&manifest_path)?))
            .map_err(|error| ConvertError::InvalidEpt(format!("cannot parse ept.json: {error}")))?;
    let mut source_files = Vec::new();
    if let Some(before) = manifest_before {
        verify_source_file_identity(root, &manifest_path, &before)?;
        source_files.push(before);
    }
    if manifest.data_type != "laszip" {
        return Err(ConvertError::UnsupportedEpt(format!(
            "dataType {:?}; this version supports only laszip",
            manifest.data_type
        )));
    }
    if manifest.hierarchy_type != "json" {
        return Err(ConvertError::UnsupportedEpt(format!(
            "hierarchyType {:?}; this version supports only local JSON hierarchy pages",
            manifest.hierarchy_type
        )));
    }
    validate_schema(&manifest.schema)?;
    let bounds = SpatialBounds::new(
        [manifest.bounds[0], manifest.bounds[1], manifest.bounds[2]],
        [manifest.bounds[3], manifest.bounds[4], manifest.bounds[5]],
    )
    .map_err(|_| ConvertError::InvalidEpt("bounds are invalid".to_owned()))?;
    let frame = CoordinateFrame::new(bounds.min())?;
    // Even an empty EPT incurs at least one point of staging capacity and fixed overhead
    // in the existing estimate. Reject impossible budgets before parsing hierarchy pages.
    check_ept_memory_budget(1, 0, 0, options.max_memory_budget_bytes())?;
    let counts = read_hierarchy(
        root,
        options.max_memory_budget_bytes(),
        checkpoint.as_ref().map(|_| &mut source_files),
    )?;
    let root_key = EptKey {
        depth: 0,
        x: 0,
        y: 0,
        z: 0,
    };
    if !counts.contains_key(&root_key) {
        return Err(ConvertError::InvalidEpt(
            "hierarchy has no root node 0-0-0-0".to_owned(),
        ));
    }

    emit_progress(
        on_progress,
        ConversionProgress {
            stage: ConversionStage::PlanningMetadata,
            points_processed: 0,
            total_points: manifest.points,
            bytes_written: 0,
        },
        0,
    )?;
    let total = counts.values().try_fold(0_u64, |sum, count| {
        sum.checked_add(*count).ok_or(ConvertError::SizeOverflow)
    })?;
    if total != manifest.points {
        return Err(ConvertError::InvalidEpt(format!(
            "hierarchy sums to {total} points but ept.json declares {}",
            manifest.points
        )));
    }
    // Grow these planning structures only as each node passes the memory preflight below.
    let mut nodes = Vec::new();
    let mut expected_format: Option<las::point::Format> = None;
    let mut schema = None;
    let mut omitted = Vec::new();
    let mut next_block = 0_u64;
    let mut all_blocks = Vec::new();
    let mut memory_estimate = 0_u64;
    for (index, (&key, &count)) in counts.iter().enumerate() {
        let node_bounds = cell_bounds(bounds, key)?;
        let file_path = root.join("ept-data").join(format!("{}.laz", key.name()));
        let mut point_format = None;
        if count > 0 {
            if !file_path.is_file() {
                return Err(ConvertError::InvalidEpt(format!(
                    "node {} declares {count} points but {} is missing",
                    key.name(),
                    file_path.display()
                )));
            }
            let before = checkpoint
                .as_ref()
                .map(|_| source_file_identity(root, &file_path))
                .transpose()?;
            let reader = open_las_reader(&file_path)?;
            let header = reader.header();
            if let Some(before) = before {
                verify_source_file_identity(root, &file_path, &before)?;
                source_files.push(before);
            }
            if header.number_of_points() != count {
                return Err(ConvertError::InvalidEpt(format!(
                    "node {} declares {count} points but its LAZ header declares {}",
                    key.name(),
                    header.number_of_points()
                )));
            }
            let format = *header.point_format();
            if expected_format.is_some_and(|expected| {
                expected.has_gps_time != format.has_gps_time
                    || expected.has_color != format.has_color
                    || expected.has_nir != format.has_nir
                    || expected.is_extended != format.is_extended
                    || expected.has_waveform != format.has_waveform
                    || expected.extra_bytes != format.extra_bytes
            }) {
                return Err(ConvertError::InvalidEpt(
                    "node LAZ files use inconsistent point formats".to_owned(),
                ));
            }
            if expected_format.is_none() {
                omitted = omitted_dimensions(&format, header.has_crs_vlrs());
                schema = Some(source_schema(&format));
                expected_format = Some(format);
            }
            point_format = Some(format);
        }
        let record_length = point_format.map_or(34, |format| usize::from(format.len()).max(1));
        let point_limit = input_batch_point_limit(options.points_per_block(), record_length);
        let block_count = count.div_ceil(point_limit);
        let planned_block_count = next_block
            .checked_add(block_count)
            .ok_or(ConvertError::SizeOverflow)?;
        if planned_block_count > MAX_BLOCKS {
            return Err(ConvertError::TooManyBlocks(planned_block_count));
        }
        memory_estimate = check_ept_memory_budget(
            input_batch_point_limit(
                options.points_per_block(),
                expected_format.map_or(record_length, |format| usize::from(format.len())),
            ),
            u64::try_from(nodes.len())
                .map_err(|_| ConvertError::SizeOverflow)?
                .checked_add(1)
                .ok_or(ConvertError::SizeOverflow)?,
            planned_block_count,
            options.max_memory_budget_bytes(),
        )?;
        let block_ids = (0..block_count)
            .map(|_| {
                let id = BlockId::new(next_block);
                next_block += 1;
                id
            })
            .collect::<Vec<_>>();
        all_blocks.extend(block_ids.iter().copied());
        nodes.push(EptNode {
            key,
            id: NodeId::new(u64::try_from(index).map_err(|_| ConvertError::SizeOverflow)?),
            parent: None,
            point_count: count,
            bounds: node_bounds,
            path: (count > 0).then_some(file_path),
            block_ids,
            point_limit,
        });
    }
    let schema = schema.unwrap_or_default();
    let node_ids = nodes
        .iter()
        .map(|node| (node.key, node.id))
        .collect::<BTreeMap<_, _>>();
    for node in &mut nodes {
        node.parent = node
            .key
            .parent()
            .map(|parent| {
                node_ids.get(&parent).copied().ok_or_else(|| {
                    ConvertError::InvalidEpt(format!(
                        "node {} has no parent {}",
                        node.key.name(),
                        parent.name()
                    ))
                })
            })
            .transpose()?;
    }
    crate::publish::check_reader_attribute_ranges(nodes.len(), schema.len())?;
    let spatial_nodes = nodes
        .iter()
        .map(|node| {
            SpatialNode::new(
                node.id,
                node.parent,
                node.key.depth,
                node.bounds,
                node.point_count,
                node_diagonal(node.bounds),
                node.block_ids.clone(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let hierarchy = SpatialHierarchy::new_with_semantics(
        spatial_nodes,
        schema.clone(),
        Some(manifest.points),
        HierarchySemantics::Additive,
    )?;
    let crs = manifest
        .srs
        .and_then(|srs| serde_json::to_string(&srs).ok());
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(frame, hierarchy),
        crs,
        Some("EPT additive hierarchy; node payloads preserve source points; geometric error is the node-cell diagonal".to_owned()),
        Vec::new(),
    )?
    .with_visual_scale(visual_scale_for_bounds(bounds)?)?;
    source_files.sort_by(|left, right| left.path.cmp(&right.path));
    if source_files
        .windows(2)
        .any(|pair| pair[0].path == pair[1].path)
    {
        return Err(ConvertError::InvalidEpt(
            "duplicate path in EPT source inventory".to_owned(),
        ));
    }
    if let Some(checkpoint) = checkpoint.as_mut() {
        checkpoint.bind_source_files(source_files.clone())?;
    }
    verify_source_inventory(root, &source_files)?;
    if let (Some(diagnostics), Some(timer)) = (diagnostics.as_deref_mut(), spatial_planning_timer) {
        diagnostics.spatial_planning = Some(timer.finish());
    }

    if checkpoint
        .as_ref()
        .is_some_and(|checkpoint| checkpoint.stage() == EptCheckpointStage::OutputReady)
    {
        let ept_checkpoint = checkpoint.as_ref().expect("output-ready checkpoint exists");
        let candidate_path = ept_checkpoint.candidate_path();
        let candidate_attributes_path = ept_checkpoint.candidate_attributes_path();
        ept_checkpoint.verify_output_ready()?;
        let (point_count, block_count, output_validation) = ept_checkpoint
            .output_ready_details()
            .expect("output-ready checkpoint has report details");
        if point_count != manifest.points || block_count != all_blocks.len() as u64 {
            return Err(ConvertError::Checkpoint(super::CheckpointError::Mismatch));
        }
        verify_source_inventory(root, &source_files)?;
        let (output_bytes, output_validation) = publish_validated_output_pair(
            &candidate_path,
            &candidate_attributes_path,
            output_path,
            point_count,
            output_validation,
            on_progress,
        )?;
        checkpoint
            .take()
            .expect("output-ready checkpoint is present")
            .complete()?;
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
            estimated_peak_memory_bytes: memory_estimate,
            omitted_dimensions: omitted,
            block_preparation: None,
            output_validation,
        });
    }
    let mut temporary = if checkpoint.is_none() {
        Some(create_temporary_pair(output_path)?)
    } else {
        None
    };
    let primary_path = checkpoint.as_ref().map_or_else(
        || {
            temporary
                .as_ref()
                .expect("temporary pair exists")
                .geometry
                .path
                .clone()
        },
        EptCheckpoint::candidate_path,
    );
    let attributes_path = checkpoint.as_ref().map_or_else(
        || {
            temporary
                .as_ref()
                .expect("temporary pair exists")
                .attributes
                .path
                .clone()
        },
        EptCheckpoint::candidate_attributes_path,
    );
    let writer_options =
        FormatWriterOptions::new(options.max_quantization_error(), 64 * 1024 * 1024)
            .map_err(ConvertError::Format)?;
    let mut resume_cursor = (0_usize, 0_u64);
    let mut processed = 0_u64;
    let mut written_blocks = 0_u64;
    let mut writer = if let Some(checkpoint) = checkpoint.as_mut() {
        match checkpoint.stage() {
            EptCheckpointStage::Started => {
                let primary = File::create(&primary_path)?;
                let attributes = File::create(&attributes_path)?;
                let writer = FormatWriter::new_split_for_version(
                    options.format_version(),
                    primary,
                    attributes,
                    metadata,
                    writer_options,
                )?;
                checkpoint.start_writing(
                    fs::metadata(&primary_path)?.len(),
                    fs::metadata(&attributes_path)?.len(),
                )?;
                writer
            }
            EptCheckpointStage::Writing => {
                let primary = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&primary_path)?;
                let attributes = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&attributes_path)?;
                let mut writer = FormatWriter::resume_for_version(
                    options.format_version(),
                    primary,
                    Some(attributes),
                    metadata,
                    writer_options,
                )?;
                checkpoint.for_each_committed_record(|record| {
                    let Some(expected_id) = all_blocks.get(record.ordinal as usize) else {
                        return Err(super::CheckpointError::Invalid(
                            "journal references an unknown EPT block".to_owned(),
                        ));
                    };
                    let id = writer
                        .restore_committed_record(&record.writer_record)
                        .map_err(|error| super::CheckpointError::Invalid(error.to_string()))?;
                    if id != *expected_id {
                        return Err(super::CheckpointError::Invalid(
                            "journal block order does not match EPT plan".to_owned(),
                        ));
                    }
                    let (node_index, node) = nodes
                        .iter()
                        .enumerate()
                        .find(|(_, node)| node.block_ids.contains(&id))
                        .ok_or_else(|| {
                            super::CheckpointError::Invalid(
                                "journal block has no EPT node".to_owned(),
                            )
                        })?;
                    let block_index = node
                        .block_ids
                        .iter()
                        .position(|block_id| *block_id == id)
                        .expect("node contains block id");
                    let block_start = u64::try_from(block_index)
                        .map_err(|_| {
                            super::CheckpointError::Invalid("block index overflow".to_owned())
                        })?
                        .checked_mul(node.point_limit)
                        .ok_or_else(|| {
                            super::CheckpointError::Invalid("point offset overflow".to_owned())
                        })?;
                    let expected_offset = block_start
                        .checked_add(record.processed_points)
                        .ok_or_else(|| {
                            super::CheckpointError::Invalid("point offset overflow".to_owned())
                        })?;
                    if record.node_index != node_index as u64
                        || record.node_point_offset != expected_offset
                        || record.processed_points
                            != node
                                .point_count
                                .saturating_sub(block_start)
                                .min(node.point_limit)
                    {
                        return Err(super::CheckpointError::Invalid(
                            "journal source cursor does not match EPT plan".to_owned(),
                        ));
                    }
                    processed =
                        processed
                            .checked_add(record.processed_points)
                            .ok_or_else(|| {
                                super::CheckpointError::Invalid("point count overflow".to_owned())
                            })?;
                    written_blocks = written_blocks.checked_add(1).ok_or_else(|| {
                        super::CheckpointError::Invalid("block count overflow".to_owned())
                    })?;
                    if expected_offset == node.point_count {
                        resume_cursor = (node_index + 1, 0);
                    } else {
                        resume_cursor = (node_index, expected_offset);
                    }
                    Ok(())
                })?;
                checkpoint.truncate_uncommitted_artifacts()?;
                writer
            }
            EptCheckpointStage::OutputReady => {
                unreachable!("output-ready checkpoint was handled above")
            }
        }
    } else {
        let temporary = temporary.as_mut().expect("temporary pair exists");
        let file = temporary
            .geometry
            .file
            .take()
            .expect("temporary file exists");
        let attributes = temporary
            .attributes
            .file
            .take()
            .expect("temporary companion file exists");
        FormatWriter::new_split_for_version(
            options.format_version(),
            file,
            attributes,
            metadata,
            writer_options,
        )?
    };
    let largest_block_points = nodes
        .iter()
        .map(|node| node.point_count.min(options.points_per_block()))
        .max()
        .unwrap_or(1);
    let second_block_estimate = largest_block_points
        .checked_mul(192)
        .ok_or(ConvertError::SizeOverflow)?;
    let pipeline = options.pipelined_writes()
        && options
            .max_memory_budget_bytes()
            .saturating_sub(memory_estimate)
            >= second_block_estimate;
    let block_stream_timer = diagnostics.as_ref().map(|_| DiagnosticTimer::start());
    let produce_each = |consume: &mut dyn FnMut(
        SourcePointBlock,
        u64,
        usize,
        u64,
    ) -> Result<(), ConvertError>|
     -> Result<(), ConvertError> {
        for (node_index, node) in nodes.iter().enumerate().skip(resume_cursor.0) {
            let Some(path) = &node.path else { continue };
            if checkpoint_mode.is_some() {
                verify_source_inventory_entry(root, path, &source_files)?;
            }
            let mut reader = open_las_reader(path)?;
            let point_format = *reader.header().point_format();
            let mut data = PointDataBuilder::new().for_header(reader.header()).build();
            let mut node_processed = if node_index == resume_cursor.0 {
                resume_cursor.1
            } else {
                0
            };
            if node_processed > 0 {
                crate::las_source::seek_points(&mut reader, node_processed)?;
            }
            let first_block = usize::try_from(node_processed / node.point_limit)
                .map_err(|_| ConvertError::SizeOverflow)?;
            for block_id in node.block_ids.iter().skip(first_block) {
                let wanted = (node.point_count - node_processed).min(node.point_limit);
                let read = crate::las_source::fill_points(&mut reader, wanted, &mut data)?;
                if read != wanted {
                    return Err(ConvertError::PointCountMismatch {
                        expected: node.point_count,
                        actual: node_processed + read,
                    });
                }
                let source = point_data_to_block(
                    *block_id,
                    &data,
                    frame,
                    point_format,
                    node.bounds,
                    node.key,
                )?;
                let after_node_points = node_processed
                    .checked_add(read)
                    .ok_or(ConvertError::SizeOverflow)?;
                consume(source, read, node_index, after_node_points)?;
                node_processed += read;
            }
            if node_processed != node.point_count {
                return Err(ConvertError::PointCountMismatch {
                    expected: node.point_count,
                    actual: node_processed,
                });
            }
            if checkpoint_mode.is_some() {
                verify_source_inventory_entry(root, path, &source_files)?;
            }
        }
        Ok(())
    };
    let mut write_block = |block: SourcePointBlock,
                           count: u64,
                           node_index: usize,
                           node_point_offset: u64|
     -> Result<(), ConvertError> {
        let block_id = block.id();
        OctreeWriter::write_block(&mut writer, block)?;
        processed = processed
            .checked_add(count)
            .ok_or(ConvertError::SizeOverflow)?;
        if let Some(checkpoint) = checkpoint.as_mut() {
            let writer_record = writer.checkpoint_record(block_id)?;
            let primary_len = fs::metadata(&primary_path)?.len();
            let attributes_len = fs::metadata(&attributes_path)?.len();
            checkpoint.append_committed_block(EptJournalRecord {
                ordinal: written_blocks,
                node_index: node_index as u64,
                node_point_offset,
                processed_points: count,
                primary_len,
                attributes_len,
                writer_record,
            })?;
        }
        written_blocks = written_blocks
            .checked_add(1)
            .ok_or(ConvertError::SizeOverflow)?;
        emit_progress(
            on_progress,
            ConversionProgress {
                stage: ConversionStage::ReadingAndWritingBlocks,
                points_processed: processed,
                total_points: manifest.points,
                bytes_written: fs::metadata(&primary_path)?.len(),
            },
            processed,
        )
    };
    if pipeline {
        std::thread::scope(|scope| -> Result<(), ConvertError> {
            let (sender, receiver) = mpsc::sync_channel(0);
            scope.spawn(move || {
                let result = produce_each(&mut |block, count, node_index, node_point_offset| {
                    sender
                        .send(Ok((block, count, node_index, node_point_offset)))
                        .map_err(|_| {
                            ConvertError::InvalidEpt("pipeline consumer stopped".to_owned())
                        })
                });
                if let Err(error) = result {
                    // The receiver observes preparation failures and terminates without
                    // publishing the temporary output pair.
                    let _ = sender.send(Err(error));
                }
            });
            for item in receiver {
                let (block, count, node_index, node_point_offset) = item?;
                write_block(block, count, node_index, node_point_offset)?;
            }
            Ok(())
        })?;
    } else {
        produce_each(&mut |block, count, node_index, node_point_offset| {
            write_block(block, count, node_index, node_point_offset)
        })?;
    }
    if processed != manifest.points {
        return Err(ConvertError::PointCountMismatch {
            expected: manifest.points,
            actual: processed,
        });
    }
    if checkpoint_mode.is_some() {
        verify_source_inventory(root, &source_files)?;
    }
    if let (Some(diagnostics), Some(timer)) = (diagnostics.as_deref_mut(), block_stream_timer) {
        diagnostics.block_stream = Some(timer.finish());
    }
    let finalize_timer = diagnostics.as_ref().map(|_| DiagnosticTimer::start());
    let (file, attributes) = OctreeWriter::finish_pair(writer)?;
    file.sync_all()?;
    let attributes = attributes.expect("split writer returns companion");
    attributes.sync_all()?;
    drop(file);
    drop(attributes);
    if let (Some(diagnostics), Some(timer)) = (diagnostics.as_deref_mut(), finalize_timer) {
        diagnostics.finalize_and_sync_elapsed_ns = Some(timer.finish().elapsed_ns);
    }
    let file_length = fs::metadata(&primary_path)?.len();
    let attributes_length = fs::metadata(&attributes_path)?.len();
    let validation = OutputValidation {
        total_points: manifest.points,
        block_ids: &all_blocks,
        source_block_ids: &all_blocks,
        file_length,
        attributes_length,
        requested_workers: options.spatial_workers(),
        memory_budget_bytes: options.max_memory_budget_bytes(),
        reserved_memory_bytes: 0,
    };
    let (output_bytes, output_validation) = if checkpoint.is_some() {
        let validation_timer = diagnostics.as_ref().map(|_| DiagnosticTimer::start());
        let output_validation = validate_output_pair(
            &primary_path,
            &attributes_path,
            &validation,
            on_progress,
            diagnostics.as_deref_mut(),
        )?;
        if let (Some(diagnostics), Some(timer)) = (diagnostics.as_deref_mut(), validation_timer) {
            diagnostics.output_validation = Some(timer.finish());
        }
        verify_source_inventory(root, &source_files)?;
        checkpoint
            .as_mut()
            .expect("checkpoint is present")
            .mark_output_ready(manifest.points, written_blocks, output_validation)?;
        let published = publish_validated_output_pair(
            &primary_path,
            &attributes_path,
            output_path,
            manifest.points,
            output_validation,
            on_progress,
        )?;
        checkpoint
            .take()
            .expect("output-ready checkpoint is present")
            .complete()?;
        published
    } else {
        publish_output_pair(
            &primary_path,
            &attributes_path,
            output_path,
            validation,
            on_progress,
            diagnostics,
        )?
    };
    let _ = on_progress(ConversionProgress {
        stage: ConversionStage::Complete,
        points_processed: manifest.points,
        total_points: manifest.points,
        bytes_written: output_bytes,
    });
    Ok(ConversionReport {
        point_count: manifest.points,
        block_count: all_blocks.len() as u64,
        output_bytes,
        estimated_peak_memory_bytes: memory_estimate,
        omitted_dimensions: omitted,
        block_preparation: None,
        output_validation,
    })
}

fn point_data_to_block(
    id: BlockId,
    data: &las::PointData,
    frame: CoordinateFrame,
    format: las::point::Format,
    node_bounds: SpatialBounds,
    key: EptKey,
) -> Result<SourcePointBlock, ConvertError> {
    let positions: Vec<[f64; 3]> = data
        .x()
        .zip(data.y())
        .zip(data.z())
        .map(|((x, y), z)| {
            let position = [x, y, z];
            if position.iter().any(|coordinate| !coordinate.is_finite()) {
                return Err(ConvertError::InvalidEpt(
                    "node contains non-finite coordinates".to_owned(),
                ));
            }
            if (0..3).any(|axis| {
                position[axis] < node_bounds.min()[axis] || position[axis] > node_bounds.max()[axis]
            }) {
                return Err(ConvertError::InvalidEpt(format!(
                    "point in node {} lies outside its spatial cell",
                    key.name()
                )));
            }
            Ok(std::array::from_fn(|axis| {
                position[axis] - frame.origin()[axis]
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let positions = positions.into_iter().collect::<Vec<_>>();
    let count = positions.len();
    let intensities = data.intensity().collect::<Vec<_>>();
    let colors = if format.has_color {
        data.rgb()
            .ok_or(ConvertError::InvalidAttributeSchema)?
            .map(|(red, green, blue)| {
                ColorRgb::new((red >> 8) as u8, (green >> 8) as u8, (blue >> 8) as u8)
            })
            .collect()
    } else {
        intensities
            .iter()
            .map(|intensity| {
                let gray = (intensity >> 8) as u8;
                ColorRgb::new(gray, gray, gray)
            })
            .collect()
    };
    let mut descriptors = viewer_core::las::las_attribute_schema(
        format.has_gps_time,
        format.has_color,
        format.has_nir,
    )
    .into_iter();
    let mut attributes = vec![
        AttributeColumn::new(
            next_descriptor(&mut descriptors, "intensity"),
            AttributeValues::U16(intensities),
            count,
        )?,
        AttributeColumn::new(
            next_descriptor(&mut descriptors, "classification"),
            AttributeValues::U8(data.classification().collect()),
            count,
        )?,
        AttributeColumn::new(
            next_descriptor(&mut descriptors, "return_number"),
            AttributeValues::U8(data.return_number().collect()),
            count,
        )?,
        AttributeColumn::new(
            next_descriptor(&mut descriptors, "number_of_returns"),
            AttributeValues::U8(data.number_of_returns().collect()),
            count,
        )?,
        AttributeColumn::new(
            next_descriptor(&mut descriptors, "scan_angle_degrees"),
            AttributeValues::F32(data.scan_angle_degrees().collect()),
            count,
        )?,
        AttributeColumn::new(
            next_descriptor(&mut descriptors, "user_data"),
            AttributeValues::U8(data.user_data().collect()),
            count,
        )?,
        AttributeColumn::new(
            next_descriptor(&mut descriptors, "point_source_id"),
            AttributeValues::U16(data.point_source_id().collect()),
            count,
        )?,
    ];
    if format.has_gps_time {
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "gps_time"),
            AttributeValues::F64(
                data.gps_time()
                    .ok_or(ConvertError::InvalidAttributeSchema)?
                    .collect(),
            ),
            count,
        )?);
    }
    if format.has_color {
        let values = data
            .rgb()
            .ok_or(ConvertError::InvalidAttributeSchema)?
            .flat_map(|(red, green, blue)| [red, green, blue])
            .collect();
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "rgb"),
            AttributeValues::U16(values),
            count,
        )?);
    }
    if format.has_nir {
        attributes.push(AttributeColumn::new(
            next_descriptor(&mut descriptors, "nir"),
            AttributeValues::U16(
                data.nir()
                    .ok_or(ConvertError::InvalidAttributeSchema)?
                    .collect(),
            ),
            count,
        )?);
    }
    Ok(SourcePointBlock::new(id, positions, colors, attributes))
}

fn read_hierarchy(
    root: &Path,
    memory_budget_bytes: u64,
    mut source_files: Option<&mut Vec<EptSourceFile>>,
) -> Result<BTreeMap<EptKey, u64>, ConvertError> {
    let hierarchy_dir = root.join("ept-hierarchy");
    let mut counts = BTreeMap::new();
    let mut pages = BTreeSet::new();
    let mut loaded_pages = BTreeSet::new();
    let mut positive_node_count = 0_u64;
    pages.insert(EptKey {
        depth: 0,
        x: 0,
        y: 0,
        z: 0,
    });
    while let Some(page_key) = pages.pop_first() {
        if !loaded_pages.insert(page_key) {
            continue;
        }
        let page_path = hierarchy_dir.join(format!("{}.json", page_key.name()));
        ensure_file_size(
            &page_path,
            MAX_EPT_HIERARCHY_PAGE_BYTES,
            "EPT hierarchy page",
        )?;
        let page_before = source_files
            .as_ref()
            .map(|_| source_file_identity(root, &page_path))
            .transpose()?;
        let page: BTreeMap<String, i64> =
            serde_json::from_reader(BufReader::new(File::open(&page_path)?)).map_err(|error| {
                ConvertError::InvalidEpt(format!(
                    "cannot parse hierarchy page {}: {error}",
                    page_path.display()
                ))
            })?;
        if let Some(before) = page_before {
            verify_source_file_identity(root, &page_path, &before)?;
            source_files
                .as_deref_mut()
                .expect("source inventory was requested")
                .push(before);
        }
        match page.get(&page_key.name()) {
            Some(count) if *count >= 0 => {}
            Some(_) => {
                return Err(ConvertError::InvalidEpt(format!(
                    "hierarchy page {} marks its own root as a page reference",
                    page_path.display()
                )));
            }
            None => {
                return Err(ConvertError::InvalidEpt(format!(
                    "hierarchy page {} does not contain its root node",
                    page_path.display()
                )));
            }
        }
        for (key_text, signed_count) in page {
            let key = EptKey::parse(&key_text)?;
            if signed_count < -1 {
                return Err(ConvertError::InvalidEpt(format!(
                    "invalid count {signed_count} for node {key_text}"
                )));
            }
            if signed_count == -1 {
                if loaded_pages.contains(&key) || !pages.insert(key) {
                    continue;
                }
                if pages.len() + loaded_pages.len() + counts.len() > MAX_EPT_NODES {
                    return Err(ConvertError::InvalidEpt(
                        "hierarchy exceeds the node limit".to_owned(),
                    ));
                }
                check_ept_hierarchy_memory_budget(
                    counts.len(),
                    pages.len(),
                    loaded_pages.len(),
                    positive_node_count,
                    memory_budget_bytes,
                )?;
                continue;
            }
            let count = signed_count as u64;
            match counts.insert(key, count) {
                Some(previous) if previous != count => {
                    return Err(ConvertError::InvalidEpt(format!(
                        "conflicting counts for node {key_text}"
                    )));
                }
                Some(_) => {}
                None if count > 0 => {
                    positive_node_count = positive_node_count
                        .checked_add(1)
                        .ok_or(ConvertError::SizeOverflow)?;
                }
                None => {}
            }
            if counts.len() > MAX_EPT_NODES {
                return Err(ConvertError::InvalidEpt(
                    "hierarchy exceeds the node limit".to_owned(),
                ));
            }
            check_ept_hierarchy_memory_budget(
                counts.len(),
                pages.len(),
                loaded_pages.len(),
                positive_node_count,
                memory_budget_bytes,
            )?;
        }
    }
    Ok(counts)
}

fn source_file_identity(root: &Path, path: &Path) -> Result<EptSourceFile, ConvertError> {
    let root = if root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        root
    };
    let relative = path
        .strip_prefix(root)
        .map_err(|_| ConvertError::InvalidEpt("source path escaped EPT root".to_owned()))?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified()?;
    let (mtime_secs, mtime_nanos) = match modified.duration_since(UNIX_EPOCH) {
        Ok(duration) => (
            i64::try_from(duration.as_secs()).map_err(|_| ConvertError::SizeOverflow)?,
            duration.subsec_nanos(),
        ),
        Err(error) => {
            let duration = error.duration();
            let seconds =
                i64::try_from(duration.as_secs()).map_err(|_| ConvertError::SizeOverflow)?;
            if duration.subsec_nanos() == 0 {
                (-seconds, 0)
            } else {
                (
                    seconds
                        .checked_neg()
                        .and_then(|s| s.checked_sub(1))
                        .ok_or(ConvertError::SizeOverflow)?,
                    1_000_000_000 - duration.subsec_nanos(),
                )
            }
        }
    };
    Ok(EptSourceFile {
        path: relative,
        bytes: metadata.len(),
        mtime_secs,
        mtime_nanos,
    })
}

fn verify_source_file_identity(
    root: &Path,
    path: &Path,
    expected: &EptSourceFile,
) -> Result<(), ConvertError> {
    if source_file_identity(root, path)? != *expected {
        return Err(ConvertError::InvalidEpt(format!(
            "source file metadata changed during conversion: {}",
            path.display()
        )));
    }
    Ok(())
}

fn verify_source_inventory(root: &Path, files: &[EptSourceFile]) -> Result<(), ConvertError> {
    for expected in files {
        verify_source_file_identity(root, &root.join(&expected.path), expected)?;
    }
    Ok(())
}

fn verify_source_inventory_entry(
    root: &Path,
    path: &Path,
    files: &[EptSourceFile],
) -> Result<(), ConvertError> {
    let current = source_file_identity(root, path)?;
    let expected = files
        .iter()
        .find(|file| file.path == current.path)
        .ok_or_else(|| {
            ConvertError::InvalidEpt(format!(
                "source file is absent from checkpoint inventory: {}",
                path.display()
            ))
        })?;
    if current != *expected {
        return Err(ConvertError::InvalidEpt(format!(
            "source file metadata changed during conversion: {}",
            path.display()
        )));
    }
    Ok(())
}

fn check_ept_memory_budget(
    point_batch_limit: u64,
    node_count: u64,
    block_count: u64,
    limit: u64,
) -> Result<u64, ConvertError> {
    let requested = point_batch_limit
        .saturating_mul(192)
        .saturating_add(node_count.saturating_mul(ESTIMATED_EPT_NODE_BYTES))
        .saturating_add(block_count.saturating_mul(ESTIMATED_EPT_BLOCK_BYTES))
        .saturating_add(16 * 1024 * 1024);
    if requested > limit {
        return Err(ConvertError::MemoryBudgetExceeded { requested, limit });
    }
    Ok(requested)
}

fn check_ept_hierarchy_memory_budget(
    known_count_nodes: usize,
    pending_page_nodes: usize,
    loaded_page_nodes: usize,
    positive_count_nodes: u64,
    limit: u64,
) -> Result<(), ConvertError> {
    let page_node_count = u64::try_from(pending_page_nodes)
        .map_err(|_| ConvertError::SizeOverflow)?
        .checked_add(u64::try_from(loaded_page_nodes).map_err(|_| ConvertError::SizeOverflow)?)
        .ok_or(ConvertError::SizeOverflow)?;
    let known_node_count = u64::try_from(known_count_nodes)
        .map_err(|_| ConvertError::SizeOverflow)?
        .max(page_node_count);
    check_ept_memory_budget(1, known_node_count, positive_count_nodes, limit)?;
    Ok(())
}

fn validate_schema(schema: &serde_json::Value) -> Result<(), ConvertError> {
    let fields = schema
        .as_array()
        .ok_or_else(|| ConvertError::InvalidEpt("schema must be a JSON array".to_owned()))?;
    let mut names = BTreeSet::new();
    for field in fields {
        let object = field.as_object().ok_or_else(|| {
            ConvertError::InvalidEpt("schema entries must be JSON objects".to_owned())
        })?;
        let name = object
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                ConvertError::InvalidEpt("schema field has no string name".to_owned())
            })?;
        let data_type = object
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                ConvertError::InvalidEpt(format!("schema field {name:?} has no string type"))
            })?;
        let size = object
            .get("size")
            .and_then(serde_json::Value::as_u64)
            .filter(|size| *size > 0)
            .ok_or_else(|| {
                ConvertError::InvalidEpt(format!("schema field {name:?} has no positive size"))
            })?;
        if name.is_empty()
            || !matches!(data_type, "signed" | "unsigned" | "float" | "floating")
            || size > 8
            || !names.insert(name)
        {
            return Err(ConvertError::InvalidEpt(format!(
                "invalid or duplicate EPT schema field {name:?}"
            )));
        }
    }
    if !["X", "Y", "Z"].into_iter().all(|axis| names.contains(axis)) {
        return Err(ConvertError::InvalidEpt(
            "schema must describe X, Y, and Z coordinates".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_file_size(path: &Path, limit: u64, description: &str) -> Result<(), ConvertError> {
    let size = fs::metadata(path)?.len();
    if size > limit {
        return Err(ConvertError::InvalidEpt(format!(
            "{description} is {size} bytes, exceeding the {limit}-byte limit"
        )));
    }
    Ok(())
}

fn cell_bounds(root: SpatialBounds, key: EptKey) -> Result<SpatialBounds, ConvertError> {
    let divisor = f64::from(1_u32 << key.depth);
    let min = std::array::from_fn(|axis| {
        root.min()[axis]
            + (root.max()[axis] - root.min()[axis]) * f64::from([key.x, key.y, key.z][axis])
                / divisor
    });
    let max = std::array::from_fn(|axis| {
        root.min()[axis]
            + (root.max()[axis] - root.min()[axis]) * f64::from([key.x, key.y, key.z][axis] + 1)
                / divisor
    });
    SpatialBounds::new(min, max)
        .map_err(|_| ConvertError::InvalidEpt("node bounds are invalid".to_owned()))
}

fn node_diagonal(bounds: SpatialBounds) -> f64 {
    let size: [f64; 3] = std::array::from_fn(|axis| bounds.max()[axis] - bounds.min()[axis]);
    viewer_format::norm3(size)
}

#[cfg(test)]
#[path = "ept_tests.rs"]
mod tests;
