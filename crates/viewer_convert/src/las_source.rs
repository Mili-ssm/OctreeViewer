use std::{
    fs::{self, File, OpenOptions},
    path::Path,
    sync::mpsc,
    thread,
    time::Instant,
};

use las::Reader;
use rayon::prelude::*;
use viewer_core::{AttributeDescriptor, BlockId, CoordinateFrame, SpatialBounds};
use viewer_format::{
    BlockEncoder, FormatBlockEncoder, FormatMetadata, FormatWriter, FormatWriterOptions,
    OctreeWriter, PreparedOctreeWriter,
};

use crate::checkpoint::{Checkpoint, CheckpointStage, fingerprint_file};
use crate::diagnostics::DiagnosticTimer;
use crate::ept;
use crate::error::ConvertError;
use crate::options::ESTIMATED_WORKING_BYTES_PER_POINT;
use crate::publish::{
    OutputValidation, create_temporary_pair, link_or_copy, output_attributes_path,
    output_block_ids, publish_output_pair, validate_output_pair, verify_candidate_visual_scale,
    visual_scale_for_bounds,
};
use crate::report::{
    ConversionProgress, ConversionReport, ConversionStage, ConversionWorkerStats,
    aggregate_geometry_codec_diagnostics, emit_progress,
};
use crate::spatial::{
    PlannedBlock, SpatialPlan, Workspace, build_spatial_plan, build_spatial_plan_from_reader,
    spool_source,
};
use crate::{CheckpointMode, ConversionDiagnostics, ConvertOptions};

pub(crate) struct SourceInfo {
    pub(crate) bounds: SpatialBounds,
    pub(crate) frame: CoordinateFrame,
    pub(crate) point_count: u64,
    pub(crate) record_length: usize,
    pub(crate) point_format: las::point::Format,
    pub(crate) transforms: las::Vector<las::Transform>,
    pub(crate) has_crs: bool,
}

pub(crate) struct ConversionPlan {
    pub(crate) metadata: FormatMetadata,
    pub(crate) frame: CoordinateFrame,
    pub(crate) point_count: u64,
    pub(crate) block_count: u64,
    pub(crate) blocks: Vec<PlannedBlock>,
    pub(crate) source_block_ids: Vec<BlockId>,
    pub(crate) omitted_dimensions: Vec<String>,
    pub(crate) estimated_peak_memory_bytes: u64,
}

/// Bounds header-declared allocations by the file length, then opens the reader.
pub(crate) fn open_las_reader(path: &Path) -> Result<Reader, ConvertError> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    viewer_core::las::preflight_las(&mut file, file_len)?;
    Ok(Reader::from_path(path)?)
}

pub(crate) fn convert_file_inner(
    input_path: &Path,
    output_path: &Path,
    options: ConvertOptions,
    checkpoint_mode: Option<CheckpointMode>,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    mut diagnostics: Option<&mut ConversionDiagnostics>,
) -> Result<ConversionReport, ConvertError> {
    if output_path == output_attributes_path(output_path) {
        return Err(ConvertError::ConflictingOutputPaths);
    }
    if ept::is_ept_input(input_path) {
        return ept::convert(
            input_path,
            output_path,
            options,
            checkpoint_mode,
            on_progress,
            diagnostics.as_deref_mut(),
        );
    }
    let requested_attributes_path = output_attributes_path(output_path);
    if output_path.exists() && checkpoint_mode != Some(CheckpointMode::Resume) {
        return Err(ConvertError::OutputExists(output_path.to_owned()));
    }
    if requested_attributes_path.exists() && checkpoint_mode != Some(CheckpointMode::Resume) {
        return Err(ConvertError::OutputExists(
            requested_attributes_path.clone(),
        ));
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
    let mut reader = open_las_reader(input_path)?;
    let source = inspect_source(&reader)?;
    let expected_visual_scale = visual_scale_for_bounds(source.bounds)?;
    let mut checkpoint = if let Some(mode) = checkpoint_mode {
        let source_fingerprint = fingerprint_file(input_path)?;
        Some(Checkpoint::open(
            output_path,
            mode,
            source_fingerprint,
            options,
        )?)
    } else {
        None
    };
    if (output_path.exists() || requested_attributes_path.exists())
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
        let (
            candidate_fingerprint,
            attributes_fingerprint,
            point_count,
            block_count,
            estimated_memory_bytes,
        ) = checkpoint
            .output_ready_details()
            .expect("output-ready checkpoint has report details");
        let candidate_path = checkpoint.candidate_path();
        verify_candidate_visual_scale(&candidate_path, expected_visual_scale)?;
        let (block_ids, source_block_ids) = output_block_ids(&candidate_path)?;
        let candidate_length = candidate_fingerprint.bytes;
        let validation = OutputValidation {
            total_points: point_count,
            block_ids: &block_ids,
            source_block_ids: &source_block_ids,
            file_length: candidate_length,
            attributes_length: attributes_fingerprint.bytes,
            requested_workers: options.spatial_workers(),
            memory_budget_bytes: options.max_memory_budget_bytes(),
            reserved_memory_bytes: estimated_memory_bytes,
        };
        let attributes_path = output_attributes_path(output_path);
        if output_path.exists() && fingerprint_file(output_path)? != candidate_fingerprint {
            return Err(ConvertError::OutputExists(output_path.to_owned()));
        }
        if attributes_path.exists() && fingerprint_file(&attributes_path)? != attributes_fingerprint
        {
            return Err(ConvertError::OutputExists(attributes_path));
        }
        let (output_bytes, output_validation) = if output_path.exists() && attributes_path.exists()
        {
            let validation_timer = diagnostics.as_ref().map(|_| DiagnosticTimer::start());
            let output_validation = validate_output_pair(
                output_path,
                &attributes_path,
                &validation,
                on_progress,
                diagnostics.as_deref_mut(),
            )?;
            if let (Some(diagnostics), Some(timer)) = (diagnostics.as_deref_mut(), validation_timer)
            {
                diagnostics.output_validation = Some(timer.finish());
            }
            (
                candidate_length
                    .checked_add(attributes_fingerprint.bytes)
                    .ok_or(ConvertError::SizeOverflow)?,
                output_validation,
            )
        } else {
            publish_output_pair(
                &candidate_path,
                &checkpoint.candidate_attributes_path(),
                output_path,
                validation,
                on_progress,
                diagnostics.as_deref_mut(),
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
            block_preparation: None,
            output_validation,
        });
    }

    let workspace = if let Some(checkpoint) = checkpoint.as_ref() {
        Workspace::create_at(&checkpoint.root().join("plan"))?
    } else {
        Workspace::create(output_path)?
    };
    let spatial_timer = diagnostics.as_ref().map(|_| DiagnosticTimer::start());
    let mut source_spool_bytes_written = 0;
    let spatial_plan = if let Some(checkpoint) = checkpoint.as_mut() {
        match checkpoint.stage() {
            CheckpointStage::Started => {
                let path = checkpoint.source_path();
                source_spool_bytes_written = spool_source(
                    &mut reader,
                    &source,
                    options,
                    &path,
                    on_progress,
                    diagnostics.is_some(),
                )?
                .unwrap_or(0);
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
        build_spatial_plan(
            &source,
            options,
            &workspace,
            &path,
            on_progress,
            diagnostics.is_some(),
            source_spool_bytes_written,
        )?
    } else {
        build_spatial_plan_from_reader(
            &mut reader,
            input_path,
            &source,
            options,
            &workspace,
            on_progress,
            diagnostics.is_some(),
        )?
    };
    if let (Some(diagnostics), Some(spools)) = (
        diagnostics.as_deref_mut(),
        spatial_plan.spool_diagnostics.as_ref(),
    ) {
        diagnostics.spatial_spools = Some(spools.clone());
    }
    if let (Some(diagnostics), Some(phases)) = (
        diagnostics.as_deref_mut(),
        spatial_plan.spatial_phases.as_ref(),
    ) {
        diagnostics.spatial_phases = Some(phases.clone());
    }
    if let (Some(diagnostics), Some(timer)) = (diagnostics.as_deref_mut(), spatial_timer) {
        diagnostics.spatial_planning = Some(timer.finish());
    }
    let plan = ConversionPlan::from_spatial(spatial_plan, source.frame);
    let mut temporary = if checkpoint.is_none() {
        Some(create_temporary_pair(output_path)?)
    } else {
        None
    };
    let temporary_path = if let Some(checkpoint) = checkpoint.as_ref() {
        let candidate = checkpoint.candidate_path();
        let _ = fs::remove_file(&candidate);
        let _ = fs::remove_file(checkpoint.candidate_attributes_path());
        candidate
    } else {
        temporary
            .as_ref()
            .expect("non-checkpoint conversion has a temporary output")
            .geometry
            .path
            .to_owned()
    };
    let file = if let Some(temporary) = temporary.as_mut() {
        temporary
            .geometry
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
    let writer_options = FormatWriterOptions::new(options.max_quantization_error, 64 * 1024 * 1024)
        .map_err(ConvertError::Format)?;
    let attributes_file = if let Some(temporary) = temporary.as_mut() {
        temporary
            .attributes
            .file
            .take()
            .expect("temporary attributes file is present")
    } else {
        OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(
                checkpoint
                    .as_ref()
                    .expect("checkpoint candidate is present")
                    .candidate_attributes_path(),
            )?
    };
    let mut writer = FormatWriter::new_split_for_version(
        options.format_version(),
        file,
        attributes_file,
        plan.metadata.clone(),
        writer_options,
    )?;
    let encoder =
        FormatBlockEncoder::new(options.format_version(), &plan.metadata, writer_options)?;
    let block_preparation = stream_blocks(
        &mut writer,
        &encoder,
        &plan,
        StreamBlockOptions {
            temporary_path: &temporary_path,
            requested_workers: options.spatial_workers(),
            pipelined_writes: options.pipelined_writes(),
            memory_budget_bytes: options.max_memory_budget_bytes(),
            on_progress,
            diagnostics: diagnostics.as_deref_mut(),
        },
    )?;
    let finalize_started = Instant::now();
    let (output, attributes) = OctreeWriter::finish_pair(writer)?;
    output.sync_all()?;
    let attributes = attributes.expect("split writer returns its companion");
    attributes.sync_all()?;
    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        diagnostics.finalize_and_sync_elapsed_ns = Some(finalize_started.elapsed().as_nanos());
    }
    drop(output);
    drop(attributes);
    let file_length = fs::metadata(&temporary_path)?.len();
    let attributes_path = if let Some(checkpoint) = checkpoint.as_ref() {
        checkpoint.candidate_attributes_path()
    } else {
        temporary
            .as_ref()
            .expect("temporary pair exists")
            .attributes
            .path
            .clone()
    };
    let attributes_length = fs::metadata(&attributes_path)?.len();
    if let Some(checkpoint) = checkpoint.as_mut() {
        checkpoint.verify_input(input_path)?;
        checkpoint.commit_output(
            plan.point_count,
            plan.block_count,
            plan.estimated_peak_memory_bytes,
        )?;
    }
    let output_block_ids = plan.blocks.iter().map(|block| block.id).collect::<Vec<_>>();
    let validation = OutputValidation {
        total_points: plan.point_count,
        block_ids: &output_block_ids,
        source_block_ids: &plan.source_block_ids,
        file_length,
        attributes_length,
        requested_workers: options.spatial_workers(),
        memory_budget_bytes: options.max_memory_budget_bytes(),
        reserved_memory_bytes: plan.estimated_peak_memory_bytes,
    };
    let (output_bytes, output_validation) = publish_output_pair(
        &temporary_path,
        &attributes_path,
        output_path,
        validation,
        on_progress,
        diagnostics,
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
        block_preparation: Some(block_preparation),
        output_validation,
    })
}

pub(crate) fn inspect_source(reader: &Reader) -> Result<SourceInfo, ConvertError> {
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
        transforms: *header.transforms(),
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

pub(crate) struct StreamBlockOptions<'a> {
    temporary_path: &'a Path,
    requested_workers: usize,
    pipelined_writes: bool,
    memory_budget_bytes: u64,
    on_progress: &'a mut dyn FnMut(ConversionProgress) -> bool,
    diagnostics: Option<&'a mut ConversionDiagnostics>,
}

pub(crate) fn stream_blocks<W, E>(
    writer: &mut W,
    encoder: &E,
    plan: &ConversionPlan,
    options: StreamBlockOptions<'_>,
) -> Result<ConversionWorkerStats, ConvertError>
where
    E: BlockEncoder,
    W: PreparedOctreeWriter<Error = viewer_format::FormatError, PreparedBlock = E::PreparedBlock>
        + ?Sized,
{
    let StreamBlockOptions {
        temporary_path,
        requested_workers,
        pipelined_writes,
        memory_budget_bytes,
        on_progress,
        mut diagnostics,
    } = options;
    let stream_timer = diagnostics.as_deref().map(|_| DiagnosticTimer::start());
    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        diagnostics.block_stream_source_payload_read_bytes = Some(0);
        diagnostics.block_stream_lod_payload_read_bytes = Some(0);
        diagnostics.block_stream_max_prepared_blocks = Some(0);
        diagnostics.block_stream_max_encoded_payload_capacity_per_block_bytes = Some(0);
        diagnostics.block_stream_max_encoded_payload_capacity_per_batch_bytes = Some(0);
    }
    let mut block_preparation_ns = 0_u128;
    let mut ordered_write_ns = 0_u128;
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
    let largest_block_points = plan
        .blocks
        .iter()
        .map(|block| block.point_count() as u64)
        .max()
        .unwrap_or(1);
    let bytes_per_worker = largest_block_points
        .checked_mul(ESTIMATED_WORKING_BYTES_PER_POINT)
        .and_then(|bytes| bytes.checked_add(1024 * 1024))
        .ok_or(ConvertError::SizeOverflow)?;
    let available_memory = memory_budget_bytes.saturating_sub(plan.estimated_peak_memory_bytes);
    let memory_limited_workers = 1_usize
        .saturating_add(usize::try_from(available_memory / bytes_per_worker).unwrap_or(usize::MAX));
    // A rendezvous hand-off retains at most the batch being written and one
    // being prepared. Disable overlap when the budget only admits one block.
    let pipeline = pipelined_writes && memory_limited_workers >= 2;
    let batch_slots = if pipeline { 2 } else { 1 };
    let worker_count = requested_workers
        .min((memory_limited_workers / batch_slots).max(1))
        .min(plan.blocks.len().max(1));
    let collect_codec_diagnostics = diagnostics.is_some();
    let prepare = |batch: &[PlannedBlock]| {
        let started = Instant::now();
        let (prepared, metrics) = if collect_codec_diagnostics {
            let prepared_with_metrics = batch
                .par_iter()
                .map(|block| {
                    encoder
                        .prepare_with_diagnostics(block.read(plan.frame)?)
                        .map_err(ConvertError::Format)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let (prepared, metrics): (
                Vec<_>,
                Vec<Option<viewer_format::GeometryCodecDiagnostics>>,
            ) = prepared_with_metrics.into_iter().unzip();
            (prepared, Some(metrics))
        } else {
            let prepared = batch
                .par_iter()
                .map(|block| {
                    encoder
                        .prepare(block.read(plan.frame)?)
                        .map_err(ConvertError::Format)
                })
                .collect::<Result<Vec<_>, _>>()?;
            (prepared, None)
        };
        Ok((prepared, metrics, started.elapsed().as_nanos()))
    };
    process_batches(
        &plan.blocks,
        worker_count,
        pipeline,
        prepare,
        |batch, (prepared, codec_metrics, batch_preparation_ns)| {
            if let Some(diagnostics) = diagnostics.as_deref_mut() {
                for metrics in codec_metrics.into_iter().flatten().flatten() {
                    aggregate_geometry_codec_diagnostics(diagnostics, metrics);
                }
                let batch_capacity = prepared.iter().try_fold(0_u64, |total, block| {
                    let capacity = u64::try_from(encoder.encoded_payload_capacity_bytes(block))
                        .map_err(|_| ConvertError::SizeOverflow)?;
                    diagnostics.block_stream_max_encoded_payload_capacity_per_block_bytes = Some(
                        diagnostics
                            .block_stream_max_encoded_payload_capacity_per_block_bytes
                            .unwrap_or(0)
                            .max(capacity),
                    );
                    total
                        .checked_add(capacity)
                        .ok_or(ConvertError::SizeOverflow)
                })?;
                diagnostics.block_stream_max_prepared_blocks = Some(
                    diagnostics
                        .block_stream_max_prepared_blocks
                        .unwrap_or(0)
                        .max(u64::try_from(prepared.len()).unwrap_or(u64::MAX)),
                );
                diagnostics.block_stream_max_encoded_payload_capacity_per_batch_bytes = Some(
                    diagnostics
                        .block_stream_max_encoded_payload_capacity_per_batch_bytes
                        .unwrap_or(0)
                        .max(batch_capacity),
                );
                for block in batch {
                    let counter = if block.is_source_points {
                        &mut diagnostics.block_stream_source_payload_read_bytes
                    } else {
                        &mut diagnostics.block_stream_lod_payload_read_bytes
                    };
                    let total = counter.get_or_insert(0);
                    *total = total
                        .checked_add(block.payload_bytes()?)
                        .ok_or(ConvertError::SizeOverflow)?;
                }
            }
            block_preparation_ns = block_preparation_ns.saturating_add(batch_preparation_ns);
            let write_started = Instant::now();
            for (block, prepared) in batch.iter().zip(prepared) {
                writer.write_prepared_block(prepared)?;
                // The whole batch has finished reading before any shared spool is removed.
                block.release_spool_after_write()?;
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
            ordered_write_ns = ordered_write_ns.saturating_add(write_started.elapsed().as_nanos());
            Ok(())
        },
    )?;
    if processed != plan.point_count {
        return Err(ConvertError::PointCountMismatch {
            expected: plan.point_count,
            actual: processed,
        });
    }
    if let (Some(diagnostics), Some(timer)) = (diagnostics, stream_timer) {
        diagnostics.block_stream = Some(timer.finish());
        diagnostics.block_preparation_elapsed_ns = Some(block_preparation_ns);
        diagnostics.ordered_write_and_release_elapsed_ns = Some(ordered_write_ns);
    }
    Ok(ConversionWorkerStats {
        worker_limit: worker_count,
        estimated_bytes_per_worker: bytes_per_worker,
    })
}

/// A zero-capacity channel bounds overlap to two batches. Dropping the receiver
/// on cancellation/error wakes the producer before the scoped thread is joined.
pub(crate) fn process_batches<T: Sync, P: Send>(
    items: &[T],
    width: usize,
    pipeline: bool,
    prepare: impl Fn(&[T]) -> Result<P, ConvertError> + Sync,
    mut consume: impl FnMut(&[T], P) -> Result<(), ConvertError>,
) -> Result<(), ConvertError> {
    if !pipeline {
        for batch in items.chunks(width) {
            consume(batch, prepare(batch)?)?;
        }
        return Ok(());
    }
    thread::scope(|scope| {
        let (sender, receiver) = mpsc::sync_channel(0);
        let prepare = &prepare;
        scope.spawn(move || {
            for batch in items.chunks(width) {
                let result = prepare(batch);
                let failed = result.is_err();
                if sender.send(result).is_err() || failed {
                    break;
                }
            }
        });
        for (batch, prepared) in items.chunks(width).zip(receiver) {
            consume(batch, prepared?)?;
        }
        Ok(())
    })
}

pub(crate) fn source_schema(format: &las::point::Format) -> Vec<AttributeDescriptor> {
    viewer_core::las::las_attribute_schema(format.has_gps_time, format.has_color, format.has_nir)
}

pub(crate) fn next_descriptor(
    descriptors: &mut impl Iterator<Item = AttributeDescriptor>,
) -> AttributeDescriptor {
    descriptors
        .next()
        .expect("LAS attribute schema covers every column")
}

pub(crate) fn omitted_dimensions(format: &las::point::Format, has_crs: bool) -> Vec<String> {
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
