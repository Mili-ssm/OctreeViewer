use std::{
    env,
    path::PathBuf,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use viewer_convert::{
    CheckpointMode, ConversionStage, ConvertOptions, convert_file_with_checkpoint,
    convert_file_with_progress,
};

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("viewer_convert: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.len() == 1 && arguments[0] == "--help" {
        print_help();
        return Ok(());
    }
    if arguments.len() < 2 {
        print_help();
        return Err("expected input and output paths".into());
    }
    let input = PathBuf::from(&arguments[0]);
    let output = PathBuf::from(&arguments[1]);
    let mut points_per_block = ConvertOptions::default().points_per_block();
    let mut max_error = ConvertOptions::default().max_quantization_error();
    let mut max_memory_bytes = ConvertOptions::default().max_memory_budget_bytes();
    let mut spatial_workers = None;
    let mut pipelined_writes = false;
    let mut spatial_divisions = ConvertOptions::default().spatial_divisions();
    let mut checkpoint_mode = None;
    let mut format_version = ConvertOptions::default().format_version();
    let mut index = 2;
    while index < arguments.len() {
        let flag = arguments[index].to_string_lossy();
        if flag == "--pipeline-writes" {
            pipelined_writes = true;
            index += 1;
            continue;
        }
        if flag == "--checkpoint" || flag == "--resume" {
            if checkpoint_mode.is_some() {
                return Err("--checkpoint and --resume cannot be combined".into());
            }
            checkpoint_mode = Some(if flag == "--checkpoint" {
                CheckpointMode::Create
            } else {
                CheckpointMode::Resume
            });
            index += 1;
            continue;
        }
        let value = arguments.get(index + 1).ok_or("missing value for option")?;
        match flag.as_ref() {
            "--format-version" => format_version = value.to_string_lossy().parse()?,
            "--points-per-block" => points_per_block = value.to_string_lossy().parse()?,
            "--max-error" => max_error = value.to_string_lossy().parse()?,
            "--max-memory-mib" => {
                max_memory_bytes = value
                    .to_string_lossy()
                    .parse::<u64>()?
                    .checked_mul(1024 * 1024)
                    .ok_or("memory budget overflows bytes")?;
            }
            "--spatial-workers" => spatial_workers = Some(value.to_string_lossy().parse()?),
            "--spatial-divisions" => spatial_divisions = value.to_string_lossy().parse()?,
            _ => return Err(format!("unknown option: {flag}").into()),
        }
        index += 2;
    }

    let mut options =
        ConvertOptions::with_memory_budget(points_per_block, max_error, max_memory_bytes)?
            .with_spatial_divisions(spatial_divisions)?
            .with_format_version(format_version)?;
    options = options.with_pipelined_writes(pipelined_writes);
    if let Some(workers) = spatial_workers {
        options = options.with_spatial_workers(workers)?;
    }
    if let Some(mode) = checkpoint_mode {
        eprintln!(
            "{} checkpoint for {}",
            match mode {
                CheckpointMode::Create => "Creating",
                CheckpointMode::Resume => "Resuming",
            },
            output.display()
        );
    }
    let cancellation_requested = Arc::new(AtomicBool::new(false));
    let signal_flag = Arc::clone(&cancellation_requested);
    ctrlc::set_handler(move || signal_flag.store(true, Ordering::Relaxed))?;

    let mut previous_stage = None;
    let mut next_percent = 0;
    let mut on_progress = |progress: viewer_convert::ConversionProgress| {
        if cancellation_requested.load(Ordering::Relaxed) {
            eprintln!(
                "{}",
                if checkpoint_mode.is_some() {
                    "Cancellation requested; stopping at a safe boundary and keeping completed checkpoints."
                } else {
                    "Cancellation requested; cleaning up before publishing output."
                }
            );
            return false;
        }
        if previous_stage != Some(progress.stage) {
            eprintln!("{:?}", progress.stage);
            previous_stage = Some(progress.stage);
            next_percent = 0;
        }
        if matches!(
            progress.stage,
            ConversionStage::PlanningSpatialIndex
                | ConversionStage::ReadingAndWritingBlocks
                | ConversionStage::ValidatingOutput
        ) && progress.total_points > 0
        {
            let percent = progress.points_processed.saturating_mul(100) / progress.total_points;
            if percent >= next_percent || percent == 100 {
                eprintln!(
                    "  {}/{} points ({} MiB written; {percent}%)",
                    progress.points_processed,
                    progress.total_points,
                    progress.bytes_written / (1024 * 1024)
                );
                next_percent = percent.saturating_add(10);
            }
        }
        true
    };
    let report = if let Some(mode) = checkpoint_mode {
        convert_file_with_checkpoint(&input, &output, options, mode, &mut on_progress)?
    } else {
        convert_file_with_progress(&input, &output, options, &mut on_progress)?
    };
    eprintln!(
        "Converted {} points in {} blocks ({} bytes; estimated controlled memory {} MiB) to {} + {}",
        report.point_count,
        report.block_count,
        report.output_bytes,
        report.estimated_peak_memory_bytes.div_ceil(1024 * 1024),
        output.display(),
        output.with_extension("octattrs").display()
    );
    if !report.omitted_dimensions.is_empty() {
        eprintln!("Not preserved: {}", report.omitted_dimensions.join(", "));
    }
    Ok(())
}

fn print_help() {
    eprintln!(
        "Usage: viewer_convert INPUT.las|INPUT.laz|INPUT.copc.laz|EPT_DIRECTORY|EPT_DIRECTORY/ept.json OUTPUT.oct [--format-version v7.1-pre|v5|v6.1-pre|v6.2-pre|v6.3-pre|v6.4-pre] [--points-per-block N] [--max-error E] [--max-memory-mib MIB] [--spatial-divisions 2|4|8] [--spatial-workers N] [--pipeline-writes] [--checkpoint|--resume]\n\
         Output: OUTPUT.oct (coordinates/index) + OUTPUT.octattrs (optional color/attributes). Reported bytes include both.\n\
         Defaults: 262144 points per block, maximum quantization error 0.01 in source units, estimated memory budget 512 MiB, 2 spatial divisions per axis.\n\
         --spatial-divisions sets the adaptive partition to 2x2x2, 4x4x4, or 8x8x8; the default is 2x2x2.\n\
         --spatial-workers uses 1 to 8 bounded workers for root subtrees; the default is min(4, available parallelism).\n\
         --pipeline-writes overlaps one preparation batch with ordered writes within the estimated memory budget (LAS/LAZ only).\n\
         --checkpoint preserves completed phases; repeat with --resume to continue the same output. Checkpoint and resume flags are mutually exclusive.\n\
         COPC points are read sequentially and replanned into an adaptive octree; the COPC hierarchy is not retained. Local EPT JSON datasets with dataType=laszip preserve their additive hierarchy; remote datasets and other codecs are unsupported. Checkpoint/resume and spatial repartition options apply only to LAS/LAZ inputs.\n\
         The converter preserves intensity and RGB16, and reports omitted LAS dimensions."
    );
}
