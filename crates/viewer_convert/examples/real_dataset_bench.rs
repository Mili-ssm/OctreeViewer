use std::{
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

use viewer_convert::{ConvertOptions, convert_file_with_progress};
use viewer_core::{DatasetSource, Point};
use viewer_format::{BlockCodec, BlockPayloadStorageStats, BlockStorageStats, FormatReader};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    match arguments.first().and_then(|value| value.to_str()) {
        Some("convert") if (3..=5).contains(&arguments.len()) => {
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
            convert(
                Path::new(&arguments[1]),
                Path::new(&arguments[2]),
                workers,
                divisions,
            )
        }
        Some("read") if arguments.len() == 2 || arguments.len() == 3 => read(
            Path::new(&arguments[1]),
            arguments.get(2).map(Path::new),
            ReadMode::All,
        ),
        Some("read-geometry") if arguments.len() == 2 || arguments.len() == 3 => read(
            Path::new(&arguments[1]),
            arguments.get(2).map(Path::new),
            ReadMode::Geometry,
        ),
        Some("read-attribute") if (3..=4).contains(&arguments.len()) => {
            let schema_index = arguments[2].to_string_lossy().parse()?;
            read(
                Path::new(&arguments[1]),
                arguments.get(3).map(Path::new),
                ReadMode::Attribute(schema_index),
            )
        }
        _ => {
            eprintln!(
                "Usage:\n  cargo run --release -p viewer_convert --example real_dataset_bench -- convert INPUT.laz OUTPUT.oct [SPATIAL_WORKERS [SPATIAL_DIVISIONS]]\n  cargo run --release -p viewer_convert --example real_dataset_bench -- read INPUT.oct [SOURCE.laz]\n  cargo run --release -p viewer_convert --example real_dataset_bench -- read-geometry INPUT.oct [SOURCE.laz]\n  cargo run --release -p viewer_convert --example real_dataset_bench -- read-attribute INPUT.oct SCHEMA_INDEX [SOURCE.laz]"
            );
            Err("invalid arguments".into())
        }
    }
}

fn convert(
    input: &Path,
    output: &Path,
    workers: usize,
    divisions: u8,
) -> Result<(), Box<dyn std::error::Error>> {
    let input_bytes = fs::metadata(input)?.len();
    let started = Instant::now();
    let mut current_stage = None;
    let mut stage_started = started;
    let mut stage_times = Vec::new();
    let options = ConvertOptions::default()
        .with_spatial_workers(workers)?
        .with_spatial_divisions(divisions)?;
    let report = convert_file_with_progress(input, output, options, |progress| {
        if current_stage != Some(progress.stage) {
            if let Some(stage) = current_stage {
                stage_times.push((stage, stage_started.elapsed()));
            }
            current_stage = Some(progress.stage);
            stage_started = Instant::now();
        }
        true
    })?;
    if let Some(stage) = current_stage {
        stage_times.push((stage, stage_started.elapsed()));
    }
    let elapsed = started.elapsed();
    println!("mode=convert");
    println!("spatial_workers={workers}");
    println!("spatial_divisions={divisions}");
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
    for (stage, duration) in stage_times {
        println!("stage_{stage:?}_ms={:.3}", duration.as_secs_f64() * 1000.0);
    }
    for dimension in report.omitted_dimensions {
        println!("omitted_dimension={dimension}");
    }
    summarize_output(output, Some(input_bytes))
}

#[derive(Clone, Copy)]
enum ReadMode {
    All,
    Geometry,
    Attribute(u32),
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
) -> Result<(), Box<dyn std::error::Error>> {
    let geometry_bytes = fs::metadata(output)?.len();
    let attributes_bytes = fs::metadata(output.with_extension("octattrs"))
        .map(|m| m.len())
        .unwrap_or(0);
    let output_bytes = geometry_bytes
        .checked_add(attributes_bytes)
        .ok_or("size overflow")?;
    let started = Instant::now();
    let bytes_read = Arc::new(AtomicU64::new(0));
    let mut reader = FormatReader::new(CountingReader::new(
        File::open(output)?,
        Arc::clone(&bytes_read),
    ))?;
    if !matches!(mode, ReadMode::Geometry) && attributes_bytes > 0 {
        reader.attach_attributes(CountingReader::new(
            File::open(output.with_extension("octattrs"))?,
            Arc::clone(&bytes_read),
        ))?;
    }
    if matches!(mode, ReadMode::All) && !reader.has_attributes() {
        return Err("full-payload benchmark requires the .octattrs companion; use read-geometry for XYZ only".into());
    }
    let open_elapsed = started.elapsed();
    let payload_read_start = bytes_read.load(Ordering::Relaxed);
    let stats = reader.block_storage_stats()?;
    let decode_started = Instant::now();
    let mut decoded_points = 0_u64;
    for block in &stats {
        decoded_points += match mode {
            ReadMode::All => reader.read_block(block.id)?.points().len() as u64,
            ReadMode::Geometry => reader.read_positions(block.id)?.len() as u64,
            ReadMode::Attribute(schema_index) => {
                reader.read_attribute(block.id, schema_index)?;
                block.point_count
            }
        };
    }
    let decode_elapsed = decode_started.elapsed();
    let payload_bytes_read = bytes_read
        .load(Ordering::Relaxed)
        .saturating_sub(payload_read_start);
    println!(
        "mode={}",
        match mode {
            ReadMode::All => "read_all_blocks",
            ReadMode::Geometry => "read_geometry_only",
            ReadMode::Attribute(_) => "read_one_attribute",
        }
    );
    println!("geometry_file_bytes={geometry_bytes}");
    println!("attribute_file_bytes={attributes_bytes}");
    println!("output_bytes={output_bytes}");
    if let Some(input) = input {
        println!("input_bytes={}", fs::metadata(input)?.len());
    }
    println!("open_index_ms={:.3}", open_elapsed.as_secs_f64() * 1000.0);
    println!(
        "read_decode_crc_all_blocks_ms={:.3}",
        decode_elapsed.as_secs_f64() * 1000.0
    );
    println!("decoded_payload_points={decoded_points}");
    println!("payload_bytes_read={payload_bytes_read}");
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
