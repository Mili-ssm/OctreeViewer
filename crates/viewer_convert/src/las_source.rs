use std::{
    collections::BTreeMap,
    fs::{self, File},
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, MutexGuard, PoisonError},
    thread,
    time::Instant,
};

use las::Reader;
use rayon::prelude::*;
use viewer_convert_core::{
    blocks::{
        ConversionPlan, output_writer, prepare_planned_block,
        prepare_planned_block_with_diagnostics, write_prepared,
    },
    error::ConvertError,
    options::ESTIMATED_WORKING_BYTES_PER_POINT,
    report::{
        ConversionProgress, ConversionReport, ConversionStage, ConversionWorkerStats,
        aggregate_geometry_codec_diagnostics, emit_progress,
    },
    spatial::{PlannedBlock, build_spatial_plan_from_source},
};
use viewer_format::{BlockEncoder, OctreeWriter, PreparedOctreeWriter};

use crate::diagnostic_timer::DiagnosticTimer;
use crate::executor::ThreadedExecutor;
use crate::file_spool_store::FileSpoolStore;
use crate::pipeline::SystemClock;
use crate::publish::{
    OutputValidation, create_temporary_pair, output_attributes_path, publish_output_pair,
};
use crate::workspace::Workspace;
use crate::{ConversionDiagnostics, ConvertOptions};
use viewer_convert_core::pipeline::point_source::{
    ByteSource, LasFactory, LasPointSource, RangeSource,
};
use viewer_convert_core::pipeline::spool_store::SpoolStore;

use viewer_convert_core::source::{inspect_source, open_las_reader_from};

/// A local file as input bytes; `len` is the length measured when the file was first opened.
pub(crate) struct FileBytes {
    pub(crate) path: PathBuf,
    pub(crate) len: u64,
}

impl ByteSource for FileBytes {
    type Reader = File;

    fn open(&self) -> Result<File, ConvertError> {
        Ok(File::open(&self.path)?)
    }

    fn len(&self) -> u64 {
        self.len
    }
}

/// Bounds header-declared allocations by the file length and decoded LAZ chunk
/// table entries by the header, then opens the reader.
#[cfg(test)]
pub(crate) fn open_las_reader(path: &Path) -> Result<Reader, ConvertError> {
    open_las_reader_with_len(path).map(|(reader, _)| reader)
}

/// [`open_las_reader`] also returning the file length it measured.
fn open_las_reader_with_len(path: &Path) -> Result<(Reader, u64), ConvertError> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    Ok((open_las_reader_from(file, len)?, len))
}

/// Plans an LAS/LAZ conversion and reads, without encoding or writing any output, the
/// `leaf_count` largest source-point blocks (largest first) followed by the first `lod_count`
/// LOD blocks. The planning workspace is created next to `workspace_anchor` and removed on return.
pub(crate) fn plan_sample_blocks(
    input_path: &Path,
    workspace_anchor: &Path,
    options: ConvertOptions,
    leaf_count: usize,
    lod_count: usize,
) -> Result<Vec<viewer_format::SourcePointBlock>, ConvertError> {
    let (reader, len) = open_las_reader_with_len(input_path)?;
    let source = inspect_source(&reader)?.for_options(options)?;
    let workspace = Workspace::create(workspace_anchor)?;
    let store = FileSpoolStore::new(workspace.root(), false);
    let factory = LasFactory::new(FileBytes {
        path: input_path.to_owned(),
        len,
    });
    let spatial = build_spatial_plan_from_source(
        &mut RangeSource::Las(LasPointSource::full(reader)),
        &factory,
        &source,
        options,
        &store,
        &SystemClock,
        &ThreadedExecutor,
        &mut |_| true,
        false,
    )?;
    let plan = ConversionPlan::from_spatial(spatial, source.frame);
    let mut leaves = plan
        .blocks
        .iter()
        .filter(|block| block.is_source_points)
        .collect::<Vec<_>>();
    leaves.sort_by_key(|block| std::cmp::Reverse(block.point_count()));
    let lods = plan.blocks.iter().filter(|block| !block.is_source_points);
    leaves
        .into_iter()
        .take(leaf_count)
        .chain(lods.take(lod_count))
        .map(|block| block.read(&store, plan.frame))
        .collect()
}

/// An EPT dataset is its `ept.json` or a directory containing it; its input was removed with V8
/// (ADR 0061). Any other directory is not an input at all (`InputIsDirectory`).
fn is_ept_input(path: &Path) -> bool {
    if path.is_dir() {
        path.join("ept.json").is_file()
    } else {
        path.file_name().is_some_and(|name| name == "ept.json")
    }
}

pub(crate) fn convert_file_inner(
    input_path: &Path,
    output_path: &Path,
    options: ConvertOptions,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    mut diagnostics: Option<&mut ConversionDiagnostics>,
) -> Result<ConversionReport, ConvertError> {
    crate::allocator::limit_retention();
    if output_path == output_attributes_path(output_path) {
        return Err(ConvertError::ConflictingOutputPaths);
    }
    if is_ept_input(input_path) {
        return Err(ConvertError::EptRemoved);
    }
    if input_path.is_dir() {
        return Err(ConvertError::InputIsDirectory(input_path.to_owned()));
    }
    let requested_attributes_path = output_attributes_path(output_path);
    if output_path.exists() {
        return Err(ConvertError::OutputExists(output_path.to_owned()));
    }
    if requested_attributes_path.exists() {
        return Err(ConvertError::OutputExists(requested_attributes_path));
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
    let (reader, len) = open_las_reader_with_len(input_path)?;
    let source = inspect_source(&reader)?.for_options(options)?;
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

    let workspace = Workspace::create(output_path)?;
    let spatial_timer = diagnostics.as_ref().map(|_| DiagnosticTimer::start());
    let spool_store = FileSpoolStore::new(workspace.root(), diagnostics.is_some());
    let factory = LasFactory::new(FileBytes {
        path: input_path.to_owned(),
        len,
    });
    let spatial_plan = build_spatial_plan_from_source(
        &mut RangeSource::Las(LasPointSource::full(reader)),
        &factory,
        &source,
        options,
        &spool_store,
        &SystemClock,
        &ThreadedExecutor,
        on_progress,
        diagnostics.is_some(),
    )?;
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
    let mut temporary = create_temporary_pair(output_path)?;
    let temporary_path = temporary.geometry.path.clone();
    let file = temporary
        .geometry
        .file
        .take()
        .expect("temporary output file is present");
    let attributes_file = temporary
        .attributes
        .file
        .take()
        .expect("temporary attributes file is present");
    let (mut writer, encoder) = output_writer(options, &plan.metadata, file, attributes_file)?;
    let block_preparation = stream_blocks(
        &mut writer,
        &encoder,
        &plan,
        &spool_store,
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
    let attributes_path = temporary.attributes.path.clone();
    let attributes_length = fs::metadata(&attributes_path)?.len();
    // `v7.5-pre` numbers blocks by page when it writes them, so read the ids back from the file.
    let (output_block_ids, source_block_ids) = crate::publish::output_block_ids(&temporary_path)?;
    let validation = OutputValidation {
        total_points: plan.point_count,
        block_ids: &output_block_ids,
        source_block_ids: &source_block_ids,
        file_length,
        attributes_length,
        requested_workers: options.spatial_workers(),
        memory_budget_bytes: options.max_memory_budget_bytes(),
        reserved_memory_bytes: plan.later_stages_reserved_bytes,
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
        spatial_workers_used: plan.spatial_workers_used,
        omitted_dimensions: plan.omitted_dimensions,
        block_preparation,
        output_validation,
    })
}

pub(crate) struct StreamBlockOptions<'a> {
    temporary_path: &'a Path,
    requested_workers: usize,
    pipelined_writes: bool,
    memory_budget_bytes: u64,
    on_progress: &'a mut dyn FnMut(ConversionProgress) -> bool,
    diagnostics: Option<&'a mut ConversionDiagnostics>,
}

pub(crate) fn stream_blocks<W, E, S>(
    writer: &mut W,
    encoder: &E,
    plan: &ConversionPlan,
    store: &S,
    options: StreamBlockOptions<'_>,
) -> Result<ConversionWorkerStats, ConvertError>
where
    E: BlockEncoder,
    S: SpoolStore + Sync,
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
    let available_memory = memory_budget_bytes.saturating_sub(plan.later_stages_reserved_bytes);
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
                    prepare_planned_block_with_diagnostics(encoder, block, store, plan.frame)
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
                .map(|block| prepare_planned_block(encoder, block, store, plan.frame))
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
                // Blocks are written in plan order, so every other reader of a shared spool is done.
                write_prepared(writer, block, prepared, store)?;
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

/// Prepares items on `width` threads and consumes them strictly in order. A sliding window
/// bounds the items prepared but not yet consumed (`2 * width`, or 1 when `width == 1` and
/// `!pipeline`, where the memory budget admits a single block), so one slow item delays only
/// the window instead of a whole batch. In-order consumption means a spool shared by several
/// items is released (by its final item) only after all its other readers were consumed.
/// An error or panic stops the workers before the scope joins them.
#[expect(
    clippy::items_after_statements,
    reason = "the helper types are declared next to the only code that uses them"
)]
pub(crate) fn process_batches<T: Sync, P: Send>(
    items: &[T],
    width: usize,
    pipeline: bool,
    prepare: impl Fn(&[T]) -> Result<P, ConvertError> + Sync,
    mut consume: impl FnMut(&[T], P) -> Result<(), ConvertError>,
) -> Result<(), ConvertError> {
    let width = width.max(1);
    let window = if pipeline || width > 1 { 2 * width } else { 1 };
    if window == 1 {
        for index in 0..items.len() {
            let item = &items[index..=index];
            consume(item, prepare(item)?)?;
        }
        return Ok(());
    }
    struct State<P> {
        next: usize,
        consumed: usize,
        stop: bool,
        ready: BTreeMap<usize, Result<P, ConvertError>>,
    }
    struct Shared<P> {
        state: Mutex<State<P>>,
        changed: Condvar,
    }
    impl<P> Shared<P> {
        fn lock(&self) -> MutexGuard<'_, State<P>> {
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }
        fn stop(&self) {
            self.lock().stop = true;
            self.changed.notify_all();
        }
    }
    // Stops the workers when the consumer returns early, and the consumer when a worker panics.
    struct StopOnDrop<'a, P>(&'a Shared<P>, bool);
    impl<P> Drop for StopOnDrop<'_, P> {
        fn drop(&mut self) {
            if self.1 || thread::panicking() {
                self.0.stop();
            }
        }
    }
    let shared = Shared {
        state: Mutex::new(State {
            next: 0,
            consumed: 0,
            stop: false,
            ready: BTreeMap::new(),
        }),
        changed: Condvar::new(),
    };
    thread::scope(|scope| {
        for _ in 0..width.min(items.len()) {
            let (shared, prepare) = (&shared, &prepare);
            scope.spawn(move || {
                let _guard = StopOnDrop(shared, false);
                loop {
                    let index = {
                        let mut state = shared.lock();
                        while !state.stop
                            && state.next < items.len()
                            && state.next >= state.consumed + window
                        {
                            state = shared
                                .changed
                                .wait(state)
                                .unwrap_or_else(PoisonError::into_inner);
                        }
                        if state.stop || state.next >= items.len() {
                            return;
                        }
                        state.next += 1;
                        state.next - 1
                    };
                    let result = prepare(&items[index..=index]);
                    shared.lock().ready.insert(index, result);
                    shared.changed.notify_all();
                }
            });
        }
        let _guard = StopOnDrop(&shared, true);
        for index in 0..items.len() {
            let prepared = {
                let mut state = shared.lock();
                loop {
                    if let Some(result) = state.ready.remove(&index) {
                        break result;
                    }
                    if state.stop {
                        // A worker panicked; the scope re-raises it when it joins.
                        return Ok(());
                    }
                    state = shared
                        .changed
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            };
            consume(&items[index..=index], prepared?)?;
            shared.lock().consumed = index + 1;
            shared.changed.notify_all();
        }
        Ok(())
    })
}
