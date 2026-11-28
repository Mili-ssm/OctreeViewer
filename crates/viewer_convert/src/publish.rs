use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
};

use viewer_convert_core::{
    error::ConvertError,
    report::{ConversionProgress, ConversionStage, ConversionWorkerStats, emit_progress},
    written_block_ids,
};
use viewer_core::{BlockId, DatasetSource};
use viewer_format::FormatReader;

use crate::diagnostic_timer::DiagnosticTimer;
use crate::fs_util;
use viewer_convert_core::diagnostics::{ConversionDiagnostics, ValidationWorkerDiagnostics};
#[allow(unused_imports, reason = "old path of the pure limit checks")]
pub(crate) use viewer_convert_core::limits::{
    check_reader_attribute_ranges, visual_scale_for_bounds,
};

pub(crate) static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn output_block_ids(path: &Path) -> Result<(Vec<BlockId>, Vec<BlockId>), ConvertError> {
    written_block_ids(&FormatReader::new(File::open(path)?)?)
}

#[derive(Clone, Copy)]
pub(crate) struct OutputValidation<'a> {
    pub(crate) total_points: u64,
    pub(crate) block_ids: &'a [BlockId],
    pub(crate) source_block_ids: &'a [BlockId],
    pub(crate) file_length: u64,
    pub(crate) attributes_length: u64,
    pub(crate) requested_workers: usize,
    pub(crate) memory_budget_bytes: u64,
    pub(crate) reserved_memory_bytes: u64,
}

pub(crate) fn validate_output_pair(
    path: &Path,
    attributes_path: &Path,
    validation: &OutputValidation<'_>,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    diagnostics: Option<&mut ConversionDiagnostics>,
) -> Result<ConversionWorkerStats, ConvertError> {
    let collect_diagnostics = diagnostics.is_some();
    let OutputValidation {
        total_points,
        block_ids,
        source_block_ids,
        file_length,
        attributes_length,
        requested_workers,
        memory_budget_bytes,
        reserved_memory_bytes,
    } = *validation;
    let mut reader = FormatReader::new(File::open(path)?)?;
    reader.attach_attributes(File::open(attributes_path)?)?;
    let file_length = file_length
        .checked_add(attributes_length)
        .ok_or(ConvertError::SizeOverflow)?;
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
    let largest_raw_block = reader
        .block_storage_stats()?
        .into_iter()
        .map(|block| block.raw_bytes.max(block.logical_bytes))
        .max()
        .unwrap_or(0);
    let worker_bytes = largest_raw_block
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_add(1024 * 1024))
        .ok_or(ConvertError::SizeOverflow)?;
    let available_memory = memory_budget_bytes.saturating_sub(reserved_memory_bytes);
    let memory_limited_workers =
        usize::try_from(available_memory.checked_div(worker_bytes).unwrap_or(0))
            .unwrap_or(usize::MAX)
            .max(1);
    let worker_count = requested_workers
        .min(memory_limited_workers)
        .min(block_ids.len().max(1));
    let next_block = AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);
    let (sender, receiver) = mpsc::sync_channel::<ValidationMessage>(worker_count * 2);
    let worker_readers = (0..worker_count)
        .enumerate()
        .map(|(worker_index, _)| {
            let mut fork = reader.fork_reader(File::open(path)?);
            fork.attach_attributes(File::open(attributes_path)?)?;
            Ok((worker_index, fork))
        })
        .collect::<Result<Vec<_>, ConvertError>>()?;
    let mut validated_points = 0_u64;
    let mut first_error = None;
    let mut validation_workers = collect_diagnostics.then(|| Vec::with_capacity(worker_count));
    thread::scope(|scope| {
        for (worker_index, mut worker_reader) in worker_readers {
            let sender = sender.clone();
            let next_block = &next_block;
            let cancelled = &cancelled;
            let source_blocks = &source_blocks;
            scope.spawn(move || {
                let mut worker_diagnostics = if collect_diagnostics {
                    Some(ValidationWorkerDiagnostics {
                        worker_index,
                        blocks_read: 0,
                        points_read: 0,
                        read_elapsed_ns: 0,
                        max_decoded_block_estimated_bytes: 0,
                    })
                } else {
                    None
                };
                while !cancelled.load(Ordering::Relaxed) {
                    let index = next_block.fetch_add(1, Ordering::Relaxed);
                    let Some(block_id) = block_ids.get(index).copied() else {
                        break;
                    };
                    let read_started = collect_diagnostics.then(Instant::now);
                    let result = worker_reader.read_block(block_id).map(|block| {
                        let points = block.points().len() as u64;
                        if let Some(worker_diagnostics) = worker_diagnostics.as_mut() {
                            worker_diagnostics.blocks_read += 1;
                            worker_diagnostics.points_read += points;
                            worker_diagnostics.max_decoded_block_estimated_bytes =
                                worker_diagnostics.max_decoded_block_estimated_bytes.max(
                                    u64::try_from(block.estimated_memory_bytes())
                                        .unwrap_or(u64::MAX),
                                );
                        }
                        (source_blocks.contains(&block_id), points)
                    });
                    if let Some(started) = read_started
                        && let Some(worker_diagnostics) = worker_diagnostics.as_mut()
                    {
                        worker_diagnostics.read_elapsed_ns += started.elapsed().as_nanos();
                    }
                    let failed = result.is_err();
                    if sender.send(ValidationMessage::Block(result)).is_err() {
                        return;
                    }
                    if failed {
                        cancelled.store(true, Ordering::Relaxed);
                        break;
                    }
                }
                let _ = sender.send(ValidationMessage::Finished(worker_diagnostics));
            });
        }
        drop(sender);
        let mut finished_workers = 0;
        while finished_workers < worker_count {
            match receiver.recv() {
                Ok(ValidationMessage::Block(Ok((is_source, block_points)))) => {
                    if is_source {
                        match validated_points.checked_add(block_points) {
                            Some(total) => validated_points = total,
                            None => {
                                first_error.get_or_insert(ConvertError::SizeOverflow);
                            }
                        }
                    }
                    if first_error.is_none()
                        && let Err(error) = emit_progress(
                            on_progress,
                            ConversionProgress {
                                stage: ConversionStage::ValidatingOutput,
                                points_processed: validated_points,
                                total_points,
                                bytes_written: file_length,
                            },
                            validated_points,
                        )
                    {
                        cancelled.store(true, Ordering::Relaxed);
                        first_error = Some(error);
                    }
                }
                Ok(ValidationMessage::Block(Err(error))) => {
                    cancelled.store(true, Ordering::Relaxed);
                    first_error.get_or_insert(ConvertError::Format(error));
                }
                Ok(ValidationMessage::Finished(worker)) => {
                    if let (Some(workers), Some(worker)) = (&mut validation_workers, worker) {
                        workers.push(worker);
                    }
                    finished_workers += 1;
                }
                Err(_) => {
                    cancelled.store(true, Ordering::Relaxed);
                    first_error.get_or_insert_with(|| {
                        ConvertError::Io(io::Error::other(
                            "validation worker channel closed unexpectedly",
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
    if validated_points != total_points {
        return Err(ConvertError::PointCountMismatch {
            expected: total_points,
            actual: validated_points,
        });
    }
    if let (Some(diagnostics), Some(mut workers)) = (diagnostics, validation_workers) {
        workers.sort_by_key(|worker| worker.worker_index);
        diagnostics.validation_workers = Some(workers);
    }
    Ok(ConversionWorkerStats {
        worker_limit: worker_count,
        estimated_bytes_per_worker: worker_bytes,
    })
}

pub(crate) enum ValidationMessage {
    Block(Result<(bool, u64), viewer_format::FormatError>),
    Finished(Option<ValidationWorkerDiagnostics>),
}

pub(crate) fn publish_output_pair(
    temporary_path: &Path,
    attributes_temporary_path: &Path,
    output_path: &Path,
    validation: OutputValidation<'_>,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    mut diagnostics: Option<&mut ConversionDiagnostics>,
) -> Result<(u64, ConversionWorkerStats), ConvertError> {
    let validation_timer = diagnostics.as_ref().map(|_| DiagnosticTimer::start());
    let output_validation = validate_output_pair(
        temporary_path,
        attributes_temporary_path,
        &validation,
        on_progress,
        diagnostics.as_deref_mut(),
    )?;
    if let (Some(diagnostics), Some(timer)) = (diagnostics, validation_timer) {
        diagnostics.output_validation = Some(timer.finish());
    }
    publish_validated_output_pair(
        temporary_path,
        attributes_temporary_path,
        output_path,
        validation.total_points,
        output_validation,
        on_progress,
    )
}

pub(crate) fn publish_validated_output_pair(
    temporary_path: &Path,
    attributes_temporary_path: &Path,
    output_path: &Path,
    point_count: u64,
    output_validation: ConversionWorkerStats,
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
) -> Result<(u64, ConversionWorkerStats), ConvertError> {
    let attributes_path = output_attributes_path(output_path);
    let primary_bytes = fs::metadata(temporary_path)?.len();
    let attributes_bytes = fs::metadata(attributes_temporary_path)?.len();
    let output_bytes = primary_bytes
        .checked_add(attributes_bytes)
        .ok_or(ConvertError::SizeOverflow)?;
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
    fs::hard_link(attributes_temporary_path, &attributes_path).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists || attributes_path.exists() {
            ConvertError::OutputExists(attributes_path.clone())
        } else {
            ConvertError::Io(error)
        }
    })?;
    fs_util::sync_parent(output_path)?;
    if let Err(error) = fs::hard_link(temporary_path, output_path) {
        let _ = fs::remove_file(&attributes_path);
        return Err(
            if error.kind() == io::ErrorKind::AlreadyExists || output_path.exists() {
                ConvertError::OutputExists(output_path.to_owned())
            } else {
                ConvertError::Io(error)
            },
        );
    }
    fs_util::sync_parent(output_path)?;
    Ok((output_bytes, output_validation))
}

pub(crate) struct TemporaryOutput {
    pub(crate) path: PathBuf,
    pub(crate) file: Option<File>,
}

impl Drop for TemporaryOutput {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

pub(crate) fn create_temporary_output(output_path: &Path) -> Result<TemporaryOutput, ConvertError> {
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
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(ConvertError::Io(error)),
        }
    }
    Err(ConvertError::TemporaryPathUnavailable(
        output_path.to_owned(),
    ))
}

pub(crate) fn output_attributes_path(output_path: &Path) -> PathBuf {
    output_path.with_extension("octattrs")
}

pub(crate) struct TemporaryPair {
    pub(crate) geometry: TemporaryOutput,
    pub(crate) attributes: TemporaryOutput,
}

pub(crate) fn create_temporary_pair(output_path: &Path) -> Result<TemporaryPair, ConvertError> {
    Ok(TemporaryPair {
        geometry: create_temporary_output(output_path)?,
        attributes: create_temporary_output(&output_attributes_path(output_path))?,
    })
}
