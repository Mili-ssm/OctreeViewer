use std::{
    env,
    fs::{self, File},
    path::Path,
    time::Instant,
};

use viewer_convert::{ConvertOptions, convert_file_with_progress};
use viewer_core::{DatasetSource, Point};
use viewer_format::{BlockCodec, BlockStorageStats, FormatReader};
use viewer_render_wgpu::InstanceRaw;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    match arguments.first().and_then(|value| value.to_str()) {
        Some("convert") if arguments.len() == 3 || arguments.len() == 4 => {
            let workers = arguments
                .get(3)
                .map(|value| value.to_string_lossy().parse())
                .transpose()?
                .unwrap_or(1);
            convert(Path::new(&arguments[1]), Path::new(&arguments[2]), workers)
        }
        Some("read") if arguments.len() == 2 || arguments.len() == 3 => {
            read(Path::new(&arguments[1]), arguments.get(2).map(Path::new))
        }
        _ => {
            eprintln!(
                "Usage:\n  cargo run --release -p viewer_convert --example real_dataset_bench -- convert INPUT.laz OUTPUT.oct [SPATIAL_WORKERS]\n  cargo run --release -p viewer_convert --example real_dataset_bench -- read INPUT.oct [SOURCE.laz]"
            );
            Err("invalid arguments".into())
        }
    }
}

fn convert(input: &Path, output: &Path, workers: usize) -> Result<(), Box<dyn std::error::Error>> {
    let input_bytes = fs::metadata(input)?.len();
    let started = Instant::now();
    let mut current_stage = None;
    let mut stage_started = started;
    let mut stage_times = Vec::new();
    let options = ConvertOptions::default().with_spatial_workers(workers)?;
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

fn read(output: &Path, input: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
    let output_bytes = fs::metadata(output)?.len();
    let started = Instant::now();
    let mut reader = FormatReader::new(File::open(output)?)?;
    let open_elapsed = started.elapsed();
    let stats = reader.block_storage_stats()?;
    let decode_started = Instant::now();
    let mut decoded_points = 0_u64;
    for block in &stats {
        let decoded = reader.read_block(block.id)?;
        decoded_points += decoded.points().len() as u64;
    }
    let decode_elapsed = decode_started.elapsed();
    println!("mode=read_all_blocks");
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
    let source_point_count = source_points(&reader);
    if let Some(input) = input {
        let input_bytes = fs::metadata(input)?.len();
        println!(
            "output_to_input_ratio={:.6}",
            ratio(output_bytes, input_bytes)
        );
    }
    summarize_stats(&stats, output_bytes, source_point_count);
    summarize_memory(source_point_count, &stats);
    Ok(())
}

fn summarize_output(
    output: &Path,
    input_bytes: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let output_bytes = fs::metadata(output)?.len();
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
    summarize_stats(&stats, output_bytes, source_point_count);
    summarize_memory(source_point_count, &stats);
    Ok(())
}

fn summarize_stats(stats: &[BlockStorageStats], file_bytes: u64, source_points: u64) {
    let stored_bytes = stats.iter().map(|block| block.stored_bytes).sum::<u64>();
    let raw_bytes = stats.iter().map(|block| block.raw_bytes).sum::<u64>();
    let payload_points = stats.iter().map(|block| block.point_count).sum::<u64>();
    let compressed_blocks = stats
        .iter()
        .filter(|block| block.codec == BlockCodec::Lz4)
        .count();
    let raw_blocks = stats
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
    println!("lz4_block_count={compressed_blocks}");
    println!("raw_block_count={raw_blocks}");
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
    let instance_bytes = std::mem::size_of::<InstanceRaw>() as u64;
    let block_point_limit = stats
        .iter()
        .map(|block| block.point_count)
        .max()
        .unwrap_or(0);
    println!("rust_point_size_bytes={point_bytes}");
    println!("render_instance_size_bytes={instance_bytes}");
    println!(
        "controller_full_cloud_cpu_bytes={}",
        source_points.saturating_mul(point_bytes + instance_bytes)
    );
    println!(
        "snapshot_copy_minimum_bytes={}",
        source_points.saturating_mul(instance_bytes)
    );
    println!(
        "hypothetical_full_gpu_instance_buffer_bytes={}",
        source_points.saturating_mul(instance_bytes)
    );
    println!("largest_payload_block_points={block_point_limit}");
    println!(
        "largest_block_point_plus_instance_bytes={}",
        block_point_limit.saturating_mul(point_bytes + instance_bytes)
    );
}

fn source_points(reader: &FormatReader<File>) -> u64 {
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
