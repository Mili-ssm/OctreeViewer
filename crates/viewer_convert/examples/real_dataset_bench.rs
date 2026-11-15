use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use viewer_convert::{
    ConversionDiagnostics, ConversionProgress, ConvertOptions, convert_file_with_diagnostics,
    convert_file_with_progress,
};
use viewer_core::{DatasetSource, Point};
use viewer_format::{
    BlockCodec, BlockPayloadKind, BlockPayloadStorageStats, BlockStorageStats, FormatReader,
    ReadSelection, SelectedRead,
};

#[path = "support/conversion_metrics.rs"]
mod conversion_metrics;
use conversion_metrics::ConversionMetrics;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    match arguments.first().and_then(|value| value.to_str()) {
        Some(command @ ("convert" | "convert-profile")) if (3..=9).contains(&arguments.len()) => {
            let workers = arguments
                .get(3)
                .map(|value| value.to_string_lossy().parse())
                .transpose()?
                .unwrap_or(1);
            let divisions = arguments
                .get(4)
                .map(|value| value.to_string_lossy().parse())
                .transpose()?
                .unwrap_or(2);
            let points_per_block = arguments
                .get(5)
                .map(|value| value.to_string_lossy().parse())
                .transpose()?
                .unwrap_or(ConvertOptions::default().points_per_block());
            let format_version = arguments
                .get(6)
                .map(|value| {
                    value
                        .to_string_lossy()
                        .parse::<viewer_format::FormatVersion>()
                })
                .transpose()?
                .unwrap_or(viewer_format::FormatVersion::V5);
            let root_ranges = arguments
                .get(7)
                .map(|value| value.to_string_lossy().parse())
                .transpose()?
                .unwrap_or(0);
            convert(
                Path::new(&arguments[1]),
                Path::new(&arguments[2]),
                workers,
                divisions,
                points_per_block,
                format_version,
                root_ranges,
                command == "convert-profile",
            )
        }
        Some("read") if arguments.len() == 2 || arguments.len() == 3 => read(
            Path::new(&arguments[1]),
            arguments.get(2).map(Path::new),
            ReadMode::All,
            ReadOrder::BlockId,
        ),
        Some("read-geometry") if arguments.len() == 2 || arguments.len() == 3 => read(
            Path::new(&arguments[1]),
            arguments.get(2).map(Path::new),
            ReadMode::Geometry,
            ReadOrder::BlockId,
        ),
        Some("read-attribute") if (3..=4).contains(&arguments.len()) => {
            let schema_index = arguments[2].to_string_lossy().parse()?;
            read(
                Path::new(&arguments[1]),
                arguments.get(3).map(Path::new),
                ReadMode::Attribute(schema_index),
                ReadOrder::BlockId,
            )
        }
        Some(command @ ("read-random" | "read-sample"))
            if (if command == "read-sample" {
                5..=6
            } else {
                4..=5
            })
            .contains(&arguments.len()) =>
        {
            let seed = arguments[2].to_string_lossy().parse()?;
            let count = if command == "read-sample" {
                Some(arguments[3].to_string_lossy().parse::<usize>()?)
            } else {
                None
            };
            let mode_index = if count.is_some() { 4 } else { 3 };
            let mode = match (
                arguments[mode_index].to_str(),
                arguments.get(mode_index + 1),
            ) {
                (Some("all"), None) => ReadMode::All,
                (Some("geometry"), None) => ReadMode::Geometry,
                (Some("attribute"), Some(index)) => {
                    ReadMode::Attribute(index.to_string_lossy().parse()?)
                }
                _ => {
                    return Err(
                        "random mode must be all, geometry, or attribute SCHEMA_INDEX".into(),
                    );
                }
            };
            read(
                Path::new(&arguments[1]),
                None,
                mode,
                match count {
                    Some(count) => ReadOrder::Sample { seed, count },
                    None => ReadOrder::Shuffled(seed),
                },
            )
        }
        _ => {
            eprintln!(
                "Usage:\n  cargo run --release -p viewer_convert --example real_dataset_bench -- convert INPUT.laz OUTPUT.oct [SPATIAL_WORKERS [SPATIAL_DIVISIONS [POINTS_PER_BLOCK [FORMAT_VERSION]]]]\n  cargo run --release -p viewer_convert --example real_dataset_bench -- read INPUT.oct [SOURCE.laz]\n  cargo run --release -p viewer_convert --example real_dataset_bench -- read-geometry INPUT.oct [SOURCE.laz]\n  cargo run --release -p viewer_convert --example real_dataset_bench -- read-attribute INPUT.oct SCHEMA_INDEX [SOURCE.laz]"
            );
            eprintln!(
                "  real_dataset_bench read-random INPUT.oct SEED all|geometry|attribute [SCHEMA_INDEX]\n  real_dataset_bench read-sample INPUT.oct SEED COUNT all|geometry|attribute [SCHEMA_INDEX]"
            );
            eprintln!(
                "  real_dataset_bench convert-profile INPUT.laz OUTPUT.oct [SPATIAL_WORKERS [SPATIAL_DIVISIONS [POINTS_PER_BLOCK [FORMAT_VERSION]]]] (Linux process I/O)"
            );
            Err("invalid arguments".into())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn convert(
    input: &Path,
    output: &Path,
    workers: usize,
    divisions: u8,
    points_per_block: u64,
    format_version: viewer_format::FormatVersion,
    root_ranges: usize,
    process_io: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let input_bytes = fs::metadata(input)?.len();
    let started = Instant::now();
    let mut metrics = ConversionMetrics::new(process_io)?;
    let mut metrics_error = None;
    let options = ConvertOptions::new(
        points_per_block,
        ConvertOptions::default().max_quantization_error(),
    )?
    .with_format_version(format_version)?
    .with_spatial_workers(workers)?
    .with_spatial_divisions(divisions)?
    .with_root_partition_ranges(root_ranges)?;
    let mut on_progress = |progress: ConversionProgress| {
        if let Err(error) = metrics.observe(progress.stage) {
            metrics_error = Some(error);
            return false;
        }
        true
    };
    let mut diagnostics: Option<ConversionDiagnostics> = None;
    let conversion = if process_io {
        convert_file_with_diagnostics(input, output, options, &mut on_progress).map(
            |(report, profile)| {
                diagnostics = Some(profile);
                report
            },
        )
    } else {
        convert_file_with_progress(input, output, options, &mut on_progress)
    };
    if let Some(error) = metrics_error {
        return Err(error.into());
    }
    let report = conversion?;
    metrics.finish()?;
    let elapsed = started.elapsed();
    println!("mode=convert");
    println!("format_version={}", options.format_version());
    println!("header_envelope={}", viewer_format::HEADER_ENVELOPE_VERSION);
    if let Some(diagnostics) = diagnostics.as_ref() {
        print_internal_window("spatial_planning", diagnostics.spatial_planning);
        print_internal_window("block_stream", diagnostics.block_stream);
        print_internal_window("output_validation", diagnostics.output_validation);
        if let Some(spools) = diagnostics.spatial_spools.as_ref() {
            println!(
                "internal_spatial_spool_source_payload_written_bytes={}",
                spools.source_payload_written_bytes
            );
            println!(
                "internal_spatial_spool_source_payload_scanned_bytes={}",
                spools.source_payload_scanned_bytes
            );
            println!(
                "internal_spatial_spool_child_payload_written_bytes={}",
                spools.child_payload_written_bytes
            );
            println!(
                "internal_spatial_spool_child_payload_scanned_bytes={}",
                spools.child_payload_scanned_bytes
            );
            println!(
                "internal_spatial_spool_lod_payload_written_bytes={}",
                spools.lod_payload_written_bytes
            );
        }
        if let Some(phases) = diagnostics.spatial_phases.as_ref() {
            println!(
                "internal_spatial_phase_root_las_fill_elapsed_ns={}",
                phases.root_las_fill_elapsed_ns
            );
            println!(
                "internal_spatial_phase_spool_scan_read_elapsed_ns={}",
                phases.spool_scan_read_elapsed_ns
            );
            println!(
                "internal_spatial_phase_spool_partition_and_sample_elapsed_ns={}",
                phases.spool_partition_and_sample_elapsed_ns
            );
            println!(
                "internal_spatial_phase_lod_sample_write_elapsed_ns={}",
                phases.lod_sample_write_elapsed_ns
            );
            println!(
                "internal_spatial_spool_bytes_written={}",
                phases.spool_bytes_written
            );
            println!(
                "internal_spatial_peak_live_spool_bytes={}",
                phases.peak_live_spool_bytes
            );
            println!(
                "internal_spatial_root_child_spool_bytes_written={}",
                phases
                    .root_child_spool_bytes_written
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            );
            // One line per node: node,parent|-,depth,input,lod,leaf,all_equal,elapsed_ns,start,end,
            // child;child;...
            for event in &phases.node_spool_events {
                println!(
                    "internal_spatial_node_event={},{},{},{},{},{},{},{},{},{},{}",
                    event.node_id,
                    event
                        .parent_id
                        .map_or_else(|| "-".to_owned(), |parent| parent.to_string()),
                    event.depth,
                    event.input_spool_bytes,
                    event.lod_bytes,
                    u8::from(event.is_leaf),
                    u8::from(event.became_leaf_all_equal),
                    event.elapsed_ns,
                    event.start_seq,
                    event.end_seq,
                    event
                        .child_spool_bytes
                        .iter()
                        .map(u64::to_string)
                        .collect::<Vec<_>>()
                        .join(";")
                );
            }
        }
        if let Some(bytes) = diagnostics.block_stream_source_payload_read_bytes {
            println!("internal_block_stream_source_payload_read_bytes={bytes}");
        }
        if let Some(bytes) = diagnostics.block_stream_lod_payload_read_bytes {
            println!("internal_block_stream_lod_payload_read_bytes={bytes}");
        }
        if let Some(blocks) = diagnostics.block_stream_max_prepared_blocks {
            println!("internal_block_stream_max_prepared_blocks={blocks}");
        }
        if let Some(bytes) = diagnostics.block_stream_max_encoded_payload_capacity_per_block_bytes {
            println!("internal_block_stream_max_encoded_payload_capacity_per_block_bytes={bytes}");
        }
        if let Some(bytes) = diagnostics.block_stream_max_encoded_payload_capacity_per_batch_bytes {
            println!("internal_block_stream_max_encoded_payload_capacity_per_batch_bytes={bytes}");
        }
        if let Some(workers) = diagnostics.validation_workers.as_ref() {
            for worker in workers {
                println!(
                    "internal_validation_worker_{}_blocks_read={}",
                    worker.worker_index, worker.blocks_read
                );
                println!(
                    "internal_validation_worker_{}_points_read={}",
                    worker.worker_index, worker.points_read
                );
                println!(
                    "internal_validation_worker_{}_read_elapsed_ns={}",
                    worker.worker_index, worker.read_elapsed_ns
                );
                println!(
                    "internal_validation_worker_{}_max_decoded_block_estimated_bytes={}",
                    worker.worker_index, worker.max_decoded_block_estimated_bytes
                );
            }
        }
        if let Some(elapsed_ns) = diagnostics.block_preparation_elapsed_ns {
            println!("internal_block_preparation_elapsed_ns={elapsed_ns}");
        }
        if let Some(elapsed_ns) = diagnostics.ordered_write_and_release_elapsed_ns {
            println!("internal_ordered_write_and_release_elapsed_ns={elapsed_ns}");
        }
        if let Some(elapsed_ns) = diagnostics.finalize_and_sync_elapsed_ns {
            println!("internal_finalize_and_sync_elapsed_ns={elapsed_ns}");
        }
        if let Some(candidates) = diagnostics.geometry_candidates.as_ref() {
            for candidate in candidates {
                let name = match candidate.kind {
                    viewer_format::GeometryCandidateKind::Raw => "raw".to_string(),
                    viewer_format::GeometryCandidateKind::Delta => "delta".to_string(),
                    viewer_format::GeometryCandidateKind::DeltaMorton => "delta_morton".to_string(),
                    viewer_format::GeometryCandidateKind::Occupancy {
                        residual_bits,
                        own_octant,
                    } => format!("occupancy_{residual_bits}_own_{}", u8::from(own_octant)),
                    viewer_format::GeometryCandidateKind::CompactOccupancy { residual_bits } => {
                        format!("compact_occupancy_{residual_bits}")
                    }
                };
                let prefix = format!("internal_geometry_candidate_{name}");
                println!("{prefix}_attempts={}", candidate.attempts);
                println!(
                    "{prefix}_encoding_elapsed_ns={}",
                    candidate.encoding_elapsed_ns
                );
                println!(
                    "{prefix}_compression_elapsed_ns={}",
                    candidate.compression_elapsed_ns
                );
                println!("{prefix}_raw_bytes={}", candidate.raw_bytes);
                println!("{prefix}_compressed_bytes={}", candidate.compressed_bytes);
                println!("{prefix}_selected_winners={}", candidate.selected_winners);
                println!(
                    "{prefix}_selected_raw_bytes={}",
                    candidate.selected_raw_bytes
                );
                println!(
                    "{prefix}_selected_compressed_bytes={}",
                    candidate.selected_compressed_bytes
                );
            }
        }
        if let Some(builds) = diagnostics.occupancy_builds.as_ref() {
            for build in builds {
                println!(
                    "internal_occupancy_build_bits_{}_attempts={}",
                    build.residual_bits, build.attempts
                );
                println!(
                    "internal_occupancy_build_bits_{}_elapsed_ns={}",
                    build.residual_bits, build.elapsed_ns
                );
            }
        }
    }
    println!("spatial_workers={workers}");
    println!("rayon_pool_threads={}", rayon::current_num_threads());
    if let Some(preparation) = report.block_preparation {
        println!("preparation_batch_blocks={}", preparation.worker_limit);
        println!(
            "preparation_thread_limit={}",
            preparation.worker_limit.min(rayon::current_num_threads())
        );
        println!(
            "preparation_estimated_bytes_per_worker={}",
            preparation.estimated_bytes_per_worker
        );
    } else {
        println!("preparation_batch_blocks=not_batched_this_invocation");
    }
    println!(
        "validation_workers={}",
        report.output_validation.worker_limit
    );
    println!(
        "validation_estimated_bytes_per_worker={}",
        report.output_validation.estimated_bytes_per_worker
    );
    println!("spatial_divisions={divisions}");
    println!("points_per_block={points_per_block}");
    println!("input_bytes={input_bytes}");
    println!("output_bytes={}", report.output_bytes);
    println!("source_points={}", report.point_count);
    println!("conversion_blocks={}", report.block_count);
    println!(
        "controlled_memory_estimate_bytes={}",
        report.estimated_peak_memory_bytes
    );
    println!(
        "conversion_elapsed_ms={:.3}",
        elapsed.as_secs_f64() * 1000.0
    );
    println!(
        "source_points_per_second={:.3}",
        report.point_count as f64 / elapsed.as_secs_f64()
    );
    metrics.print()?;
    for dimension in report.omitted_dimensions {
        println!("omitted_dimension={dimension}");
    }
    summarize_output(output, Some(input_bytes))
}

fn print_internal_window(name: &str, window: Option<viewer_convert::DiagnosticWindow>) {
    let Some(window) = window else {
        println!("internal_{name}_available=false");
        return;
    };
    println!("internal_{name}_available=true");
    println!("internal_{name}_elapsed_ns={}", window.elapsed_ns);
    println!(
        "internal_{name}_start_unix_ns={}",
        window
            .start_unix_ns
            .map_or_else(|| "null".to_owned(), |value| value.to_string())
    );
    println!(
        "internal_{name}_end_unix_ns={}",
        window
            .end_unix_ns
            .map_or_else(|| "null".to_owned(), |value| value.to_string())
    );
}

#[derive(Clone, Copy)]
enum ReadMode {
    All,
    Geometry,
    Attribute(u32),
}

#[derive(Clone, Copy)]
enum ReadOrder {
    BlockId,
    Shuffled(u64),
    Sample { seed: u64, count: usize },
}

/// Sampling takes a prefix of the existing reproducible permutation.
/// Zero requests measures metadata only; oversized requests fail before payload I/O.
fn select_blocks<T>(blocks: &mut [T], order: ReadOrder) -> Result<&[T], &'static str> {
    let count = match order {
        ReadOrder::Sample { count, .. } => count,
        _ => blocks.len(),
    };
    if count > blocks.len() {
        return Err("requested block count exceeds dataset block count");
    }
    match order {
        ReadOrder::Shuffled(seed) | ReadOrder::Sample { seed, .. } => shuffle(blocks, seed),
        ReadOrder::BlockId => {}
    }
    Ok(&blocks[..count])
}

/// Deterministic Fisher-Yates permutation using a fixed SplitMix64 sequence.
/// The fixed generator and modulo mapping preserve block order across platforms.
fn shuffle<T>(values: &mut [T], mut seed: u64) {
    for index in (1..values.len()).rev() {
        seed = seed.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = seed;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^= value >> 31;
        values.swap(index, (value % (index as u64 + 1)) as usize);
    }
}

struct CountingReader<R> {
    inner: R,
    bytes_read: Arc<AtomicU64>,
}

impl<R> CountingReader<R> {
    fn new(inner: R, bytes_read: Arc<AtomicU64>) -> Self {
        Self { inner, bytes_read }
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.bytes_read.fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}

impl<R: Seek> Seek for CountingReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

fn read(
    output: &Path,
    input: Option<&Path>,
    mode: ReadMode,
    order: ReadOrder,
) -> Result<(), Box<dyn std::error::Error>> {
    let geometry_bytes = fs::metadata(output)?.len();
    let attributes_bytes = fs::metadata(output.with_extension("octattrs"))
        .map(|m| m.len())
        .unwrap_or(0);
    let output_bytes = geometry_bytes
        .checked_add(attributes_bytes)
        .ok_or("size overflow")?;
    let started = Instant::now();
    let geometry_bytes_read = Arc::new(AtomicU64::new(0));
    let attribute_bytes_read = Arc::new(AtomicU64::new(0));
    let mut reader = FormatReader::new(CountingReader::new(
        File::open(output)?,
        Arc::clone(&geometry_bytes_read),
    ))?;
    let index_open_elapsed = started.elapsed();
    let attach_started = Instant::now();
    if !matches!(mode, ReadMode::Geometry) && attributes_bytes > 0 {
        reader.attach_attributes(CountingReader::new(
            File::open(output.with_extension("octattrs"))?,
            Arc::clone(&attribute_bytes_read),
        ))?;
    }
    let attach_elapsed = attach_started.elapsed();
    if matches!(mode, ReadMode::All) && !reader.has_attributes() {
        return Err("full-payload benchmark requires the .octattrs companion; use read-geometry for XYZ only".into());
    }
    let open_elapsed = started.elapsed();
    let metadata_geometry_bytes = geometry_bytes_read.load(Ordering::Relaxed);
    let metadata_attribute_bytes = attribute_bytes_read.load(Ordering::Relaxed);
    let mut stats = reader.block_storage_stats()?;
    let selection_started = Instant::now();
    let selected = select_blocks(&mut stats, order)?;
    let selection_elapsed = selection_started.elapsed();
    let order_checksum = selected
        .iter()
        .fold(0xcbf29ce484222325_u64, |checksum, block| {
            (checksum ^ block.id.get()).wrapping_mul(0x100000001b3)
        });
    // The benchmark retains one u64 per block for exact nearest-rank percentiles.
    // These samples and the stats vectors are benchmark overhead, not reader memory.
    let mut block_latencies_ns = Vec::with_capacity(selected.len());
    let decode_started = Instant::now();
    let mut decoded_points = 0_u64;
    let read_selection = match mode {
        ReadMode::All => None,
        // Historical read-geometry measures XYZ only and deliberately omits the companion.
        ReadMode::Geometry => Some(ReadSelection::Positions),
        ReadMode::Attribute(schema_index) => Some(ReadSelection::Attribute { schema_index }),
    };
    let mut max_estimated_read_payload_peak_bytes = 0;
    let mut planned_payload_bytes = 0_u64;
    for block in selected {
        let block_started = Instant::now();
        decoded_points += if let Some(selection) = read_selection {
            let plan = reader.plan_read(block.id, selection)?;
            let peak = plan.memory().peak_bytes();
            max_estimated_read_payload_peak_bytes = max_estimated_read_payload_peak_bytes.max(peak);
            planned_payload_bytes = planned_payload_bytes
                .checked_add(plan.memory().stored_bytes())
                .ok_or("planned payload byte count overflow")?;
            // Check the per-read estimate, and consume/drop each result before the next block.
            // This does not reserve an aggregate concurrent quota or enforce an RSS limit.
            match reader.read_selected(block.id, selection, peak)? {
                SelectedRead::Positions(points) => points.len() as u64,
                SelectedRead::Attribute(_) => block.point_count,
                _ => return Err("unexpected selective read result".into()),
            }
        } else {
            reader.read_block(block.id)?.points().len() as u64
        };
        block_latencies_ns
            .push(u64::try_from(block_started.elapsed().as_nanos()).unwrap_or(u64::MAX));
    }
    let decode_elapsed = decode_started.elapsed();
    let geometry_payload_bytes_read = geometry_bytes_read
        .load(Ordering::Relaxed)
        .saturating_sub(metadata_geometry_bytes);
    let attribute_payload_bytes_read = attribute_bytes_read
        .load(Ordering::Relaxed)
        .saturating_sub(metadata_attribute_bytes);
    let payload_bytes_read = geometry_payload_bytes_read + attribute_payload_bytes_read;
    if read_selection.is_some() && planned_payload_bytes != payload_bytes_read {
        return Err("selective read fetched a different byte count from its plan".into());
    }
    block_latencies_ns.sort_unstable();
    println!(
        "mode={}",
        match mode {
            ReadMode::All => "read_all_blocks",
            ReadMode::Geometry => "read_geometry_only",
            ReadMode::Attribute(_) => "read_one_attribute",
        }
    );
    println!("geometry_file_bytes={geometry_bytes}");
    println!(
        "read_order={}",
        match order {
            ReadOrder::BlockId => "block_id",
            ReadOrder::Shuffled(_) => "shuffled",
            ReadOrder::Sample { .. } => "sampled",
        }
    );
    if let ReadOrder::Shuffled(seed) | ReadOrder::Sample { seed, .. } = order {
        println!("shuffle_seed={seed}");
    }
    println!("block_order_checksum={order_checksum}");
    println!("requested_block_count={}", selected.len());
    println!(
        "selection_ms={:.3}",
        selection_elapsed.as_secs_f64() * 1000.0
    );
    println!("storage_summary_scope=whole_dataset");
    println!("attribute_file_bytes={attributes_bytes}");
    println!("output_bytes={output_bytes}");
    if let Some(input) = input {
        println!("input_bytes={}", fs::metadata(input)?.len());
    }
    println!("open_index_ms={:.3}", open_elapsed.as_secs_f64() * 1000.0);
    // Retain open_index_ms as the legacy total; split it for v5 comparisons.
    println!(
        "open_primary_index_ms={:.3}",
        index_open_elapsed.as_secs_f64() * 1000.0
    );
    println!(
        "attach_attributes_ms={:.3}",
        if matches!(mode, ReadMode::Geometry) || attributes_bytes == 0 {
            0.0
        } else {
            attach_elapsed.as_secs_f64() * 1000.0
        }
    );
    println!("serialized_index_bytes={}", reader.index_size_bytes());
    println!(
        "block_id_catalog_bytes={}",
        size_of_val(reader.metadata().hierarchy().block_ids())
    );
    println!("metadata_geometry_bytes_read={metadata_geometry_bytes}");
    println!("metadata_attribute_bytes_read={metadata_attribute_bytes}");
    println!("geometry_payload_bytes_read={geometry_payload_bytes_read}");
    println!("attribute_payload_bytes_read={attribute_payload_bytes_read}");
    for percentile in [50, 95] {
        if let Some(latency) = percentile_ns(&block_latencies_ns, percentile) {
            println!(
                "block_read_decode_crc_p{percentile}_ms={:.6}",
                latency as f64 / 1_000_000.0
            );
        }
    }
    println!(
        "read_decode_crc_selected_blocks_ms={:.3}",
        decode_elapsed.as_secs_f64() * 1000.0
    );
    if !matches!(order, ReadOrder::Sample { .. }) {
        println!(
            "read_decode_crc_all_blocks_ms={:.3}",
            decode_elapsed.as_secs_f64() * 1000.0
        );
    }
    println!("decoded_payload_points={decoded_points}");
    println!("payload_bytes_read={payload_bytes_read}");
    if read_selection.is_some() {
        println!("planned_payload_bytes={planned_payload_bytes}");
        println!("max_estimated_read_payload_peak_bytes={max_estimated_read_payload_peak_bytes}");
        println!("read_plan_included_in_timing=true");
        println!("read_memory_basis=payload_buffers_excludes_metadata_cache_allocator_control");
    }
    let source_point_count = source_points(&reader);
    if let Some(input) = input {
        let input_bytes = fs::metadata(input)?.len();
        println!(
            "output_to_input_ratio={:.6}",
            ratio(output_bytes, input_bytes)
        );
    }
    summarize_stats(
        &stats,
        &reader.block_payload_storage_stats()?,
        output_bytes,
        source_point_count,
    );
    summarize_memory(source_point_count, &stats);
    Ok(())
}

fn summarize_output(
    output: &Path,
    input_bytes: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let geometry_bytes = fs::metadata(output)?.len();
    let attributes_bytes = fs::metadata(output.with_extension("octattrs"))
        .map(|m| m.len())
        .unwrap_or(0);
    let output_bytes = geometry_bytes
        .checked_add(attributes_bytes)
        .ok_or("size overflow")?;
    let reader = FormatReader::new(File::open(output)?)?;
    println!("geometry_file_bytes={geometry_bytes}");
    println!("attribute_file_bytes={attributes_bytes}");
    println!("serialized_index_bytes={}", reader.index_size_bytes());
    let stats = reader.block_storage_stats()?;
    println!("output_size_bytes={output_bytes}");
    if let Some(input_bytes) = input_bytes {
        println!(
            "output_to_input_ratio={:.6}",
            ratio(output_bytes, input_bytes)
        );
    }
    let source_point_count = source_points(&reader);
    summarize_stats(
        &stats,
        &reader.block_payload_storage_stats()?,
        output_bytes,
        source_point_count,
    );
    summarize_memory(source_point_count, &stats);
    Ok(())
}

fn summarize_stats(
    stats: &[BlockStorageStats],
    payloads: &[BlockPayloadStorageStats],
    file_bytes: u64,
    source_points: u64,
) {
    let stored_bytes = stats.iter().map(|block| block.stored_bytes).sum::<u64>();
    let raw_bytes = stats.iter().map(|block| block.raw_bytes).sum::<u64>();
    let payload_points = stats.iter().map(|block| block.point_count).sum::<u64>();
    let compressed_payloads = payloads
        .iter()
        .filter(|block| block.codec == BlockCodec::Lz4)
        .count();
    let raw_payloads = payloads
        .iter()
        .filter(|block| block.codec == BlockCodec::None)
        .count();
    let largest_raw_block = stats.iter().map(|block| block.raw_bytes).max().unwrap_or(0);
    let largest_stored_block = stats
        .iter()
        .map(|block| block.stored_bytes)
        .max()
        .unwrap_or(0);
    println!("payload_block_count={}", stats.len());
    println!("payload_points_including_lod={payload_points}");
    println!("source_points={source_points}");
    println!(
        "lod_sample_points={}",
        payload_points.saturating_sub(source_points)
    );
    println!("lz4_payload_count={compressed_payloads}");
    println!("raw_payload_count={raw_payloads}");
    println!("raw_payload_bytes={raw_bytes}");
    println!(
        "logical_payload_bytes={}",
        stats.iter().map(|b| b.logical_bytes).sum::<u64>()
    );
    println!(
        "geometry_delta_block_count={}",
        stats
            .iter()
            .filter(|b| b.geometry_encoding == viewer_format::GeometryEncoding::DeltaVarint)
            .count()
    );
    println!(
        "geometry_occupied_block_count={}",
        stats
            .iter()
            .filter(|b| b.geometry_encoding == viewer_format::GeometryEncoding::Occupied)
            .count()
    );
    println!(
        "geometry_compact_occupied_block_count={}",
        stats
            .iter()
            .filter(|b| { b.geometry_encoding == viewer_format::GeometryEncoding::CompactOccupied })
            .count()
    );
    println!(
        "geometry_raw_block_count={}",
        stats
            .iter()
            .filter(|b| b.geometry_encoding == viewer_format::GeometryEncoding::RawU32)
            .count()
    );
    println!("stored_payload_bytes={stored_bytes}");
    println!(
        "payload_compression_ratio={:.6}",
        ratio(stored_bytes, raw_bytes)
    );
    println!(
        "payload_compression_factor={:.6}",
        ratio(raw_bytes, stored_bytes)
    );
    println!(
        "payload_size_reduction_percent={:.3}",
        (1.0 - ratio(stored_bytes, raw_bytes)) * 100.0
    );
    println!(
        "format_overhead_bytes={}",
        file_bytes.saturating_sub(stored_bytes)
    );
    println!("largest_raw_block_bytes={largest_raw_block}");
    println!("largest_stored_block_bytes={largest_stored_block}");
    let mut by_kind = BTreeMap::<String, (u64, u64)>::new();
    for payload in payloads {
        let key = match &payload.kind {
            BlockPayloadKind::Geometry => "geometry".to_owned(),
            BlockPayloadKind::DisplayColor => "display_rgb".to_owned(),
            BlockPayloadKind::Attribute { schema_index, .. } => format!("attribute_{schema_index}"),
        };
        let sizes = by_kind.entry(key).or_default();
        sizes.0 += payload.stored_bytes;
        sizes.1 += payload.raw_bytes;
    }
    for (kind, (stored, raw)) in by_kind {
        println!("{kind}_stored_payload_bytes={stored}");
        println!("{kind}_raw_payload_bytes={raw}");
    }
}

/// Nearest-rank percentile of sorted nanosecond samples; empty runs have no latency.
fn percentile_ns(sorted_samples: &[u64], percentile: usize) -> Option<u64> {
    assert!((1..=100).contains(&percentile));
    if sorted_samples.is_empty() {
        return None;
    }
    let rank = sorted_samples
        .len()
        .saturating_mul(percentile)
        .div_ceil(100);
    Some(sorted_samples[rank - 1])
}

fn summarize_memory(source_points: u64, stats: &[BlockStorageStats]) {
    let point_bytes = std::mem::size_of::<Point>() as u64;
    let block_point_limit = stats
        .iter()
        .map(|block| block.point_count)
        .max()
        .unwrap_or(0);
    println!("rust_point_size_bytes={point_bytes}");
    println!(
        "hypothetical_full_point_storage_bytes={}",
        source_points.saturating_mul(point_bytes)
    );
    println!("largest_payload_block_points={block_point_limit}");
    println!(
        "largest_block_point_storage_bytes={}",
        block_point_limit.saturating_mul(point_bytes)
    );
}

fn source_points<R: Read + Seek>(reader: &FormatReader<R>) -> u64 {
    reader
        .format_metadata()
        .dataset()
        .hierarchy()
        .stats()
        .source_point_count
        .unwrap_or(0)
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

#[cfg(test)]
mod tests {
    use super::{CountingReader, ReadOrder, percentile_ns, select_blocks, shuffle};
    use std::{
        io::{Cursor, Read, Seek, SeekFrom},
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
    };

    #[test]
    fn shuffle_is_reproducible_and_visits_each_block_once() {
        let mut empty: [u64; 0] = [];
        shuffle(&mut empty, 42);
        let mut single = [7];
        shuffle(&mut single, 42);
        assert_eq!(single, [7]);
        let mut ids = [0, 1, 2, 3, 4, 5, 6, 7];
        shuffle(&mut ids, 42);
        assert_eq!(ids, [3, 1, 6, 2, 4, 0, 7, 5]);
        let mut repeated = [0, 1, 2, 3, 4, 5, 6, 7];
        shuffle(&mut repeated, 42);
        assert_eq!(ids, repeated);
        repeated = [0, 1, 2, 3, 4, 5, 6, 7];
        shuffle(&mut repeated, 0);
        assert_ne!(ids, repeated);
        ids.sort_unstable();
        assert_eq!(ids, [0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn samples_are_nested_prefixes_without_duplicates_and_cover_boundaries() {
        let original = [0, 1, 2, 3, 4, 5, 6, 7];
        for count in 0..=original.len() {
            let mut ids = original;
            let selected = select_blocks(&mut ids, ReadOrder::Sample { seed: 42, count }).unwrap();
            assert_eq!(selected, &[3, 1, 6, 2, 4, 0, 7, 5][..count]);
        }
        let mut ids = original;
        assert!(select_blocks(&mut ids, ReadOrder::Sample { seed: 42, count: 9 }).is_err());
        assert_eq!(ids, original);
        let mut empty: [u64; 0] = [];
        assert!(
            select_blocks(&mut empty, ReadOrder::Sample { seed: 0, count: 0 })
                .unwrap()
                .is_empty()
        );
        assert!(select_blocks(&mut empty, ReadOrder::Sample { seed: 0, count: 1 }).is_err());
        assert_eq!(
            select_blocks(&mut ids, ReadOrder::BlockId).unwrap(),
            original
        );
    }

    #[test]
    fn percentiles_use_nearest_rank_and_handle_empty_or_single_runs() {
        assert_eq!(percentile_ns(&[], 95), None);
        assert_eq!(percentile_ns(&[17], 50), Some(17));
        assert_eq!(percentile_ns(&[17], 95), Some(17));
        let samples: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile_ns(&samples, 50), Some(50));
        assert_eq!(percentile_ns(&samples, 95), Some(95));
        assert_eq!(percentile_ns(&[10, 20, 30], 50), Some(20));
        assert_eq!(percentile_ns(&[10, 20, 30], 95), Some(30));
    }

    #[test]
    fn counters_count_actual_reads_and_rereads_without_counting_seeks() {
        let first_count = Arc::new(AtomicU64::new(0));
        let second_count = Arc::new(AtomicU64::new(0));
        let mut first = CountingReader::new(Cursor::new(vec![1, 2, 3]), Arc::clone(&first_count));
        let mut second = CountingReader::new(Cursor::new(vec![4]), Arc::clone(&second_count));
        let mut buffer = [0; 8];
        assert_eq!(first.read(&mut buffer).unwrap(), 3);
        first.seek(SeekFrom::Start(1)).unwrap();
        assert_eq!(first.read(&mut buffer).unwrap(), 2);
        assert_eq!(first.read(&mut buffer).unwrap(), 0);
        assert_eq!(second.read(&mut buffer).unwrap(), 1);
        assert_eq!(first_count.load(Ordering::Relaxed), 5);
        assert_eq!(second_count.load(Ordering::Relaxed), 1);
    }
}
