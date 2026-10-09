use std::{
    env,
    path::PathBuf,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use viewer_convert::{ConversionStage, ConvertOptions, convert_file_with_progress};

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
    let mut index = 2;
    while index < arguments.len() {
        let flag = arguments[index].to_string_lossy();
        let value = arguments.get(index + 1).ok_or("missing value for option")?;
        match flag.as_ref() {
            "--points-per-block" => points_per_block = value.to_string_lossy().parse()?,
            "--max-error" => max_error = value.to_string_lossy().parse()?,
            "--max-memory-mib" => {
                max_memory_bytes = value
                    .to_string_lossy()
                    .parse::<u64>()?
                    .checked_mul(1024 * 1024)
                    .ok_or("memory budget overflows bytes")?;
            }
            _ => return Err(format!("unknown option: {flag}").into()),
        }
        index += 2;
    }

    let options =
        ConvertOptions::with_memory_budget(points_per_block, max_error, max_memory_bytes)?;
    let cancellation_requested = Arc::new(AtomicBool::new(false));
    let signal_flag = Arc::clone(&cancellation_requested);
    ctrlc::set_handler(move || signal_flag.store(true, Ordering::Relaxed))?;

    let mut previous_stage = None;
    let mut next_percent = 0;
    let report = convert_file_with_progress(&input, &output, options, |progress| {
        if cancellation_requested.load(Ordering::Relaxed) {
            eprintln!("Cancellation requested; cleaning up before publishing output.");
            return false;
        }
        if previous_stage != Some(progress.stage) {
            eprintln!("{:?}", progress.stage);
            previous_stage = Some(progress.stage);
            next_percent = 0;
        }
        if matches!(
            progress.stage,
            ConversionStage::ReadingAndWritingBlocks | ConversionStage::ValidatingOutput
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
    })?;
    eprintln!(
        "Converted {} points in {} blocks ({} bytes; estimated controlled memory {} MiB) to {}",
        report.point_count,
        report.block_count,
        report.output_bytes,
        report.estimated_peak_memory_bytes.div_ceil(1024 * 1024),
        output.display()
    );
    if !report.omitted_dimensions.is_empty() {
        eprintln!("Not preserved: {}", report.omitted_dimensions.join(", "));
    }
    Ok(())
}

fn print_help() {
    eprintln!(
        "Usage: viewer_convert INPUT.las|INPUT.laz OUTPUT.oct [--points-per-block N] [--max-error E] [--max-memory-mib MIB]\n\
         Defaults: 262144 points per block, maximum quantization error 0.01 in source units, estimated memory budget 512 MiB.\n\
         The converter writes one root node, preserves intensity and RGB16, and reports omitted LAS dimensions."
    );
}
