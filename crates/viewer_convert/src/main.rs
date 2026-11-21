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

#[expect(
    clippy::needless_pass_by_value,
    reason = "the arguments are handed over by `main`"
)]
fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.len() == 1 && arguments[0] == "--help" {
        print_help();
        return Ok(());
    }
    if arguments.first().is_some_and(|word| word == "verify") && arguments.len() == 2 {
        return verify(&PathBuf::from(&arguments[1]));
    }
    if arguments.len() < 2 {
        print_help();
        return Err("expected input and output paths".into());
    }
    let input = PathBuf::from(&arguments[0]);
    let output = PathBuf::from(&arguments[1]);
    let mut max_error = ConvertOptions::default().max_quantization_error();
    let mut max_memory_bytes = ConvertOptions::default().max_memory_budget_bytes();
    let mut spatial_workers = None;
    let mut pipelined_writes = false;
    let mut page_target_bytes = None;
    let mut index = 2;
    while index < arguments.len() {
        let flag = arguments[index].to_string_lossy();
        if flag == "--pipeline-writes" {
            pipelined_writes = true;
            index += 1;
            continue;
        }
        if flag == "--checkpoint" || flag == "--resume" {
            return Err(format!(
                "option {flag} was removed with V8 (ADR 0061): conversions are not resumable; rerun the conversion from the start"
            )
            .into());
        }
        if flag == "--attr-codec" {
            return Err(
                "option --attr-codec was removed (ADR 0060): attribute payloads are stored with LZ4 only"
                    .into(),
            );
        }
        if matches!(
            flag.as_ref(),
            "--format-version" | "--row-order" | "--points-per-block" | "--spatial-divisions"
        ) {
            return Err(
                format!("option {flag} was removed: V8 is the only format and fixes it").into(),
            );
        }
        let value = arguments.get(index + 1).ok_or("missing value for option")?;
        match flag.as_ref() {
            // Hidden: tests and benchmarks that need several pages in a small file.
            "--page-target-bytes" => page_target_bytes = Some(value.to_string_lossy().parse()?),
            "--max-error" => max_error = value.to_string_lossy().parse()?,
            "--max-memory-mib" => {
                max_memory_bytes = value
                    .to_string_lossy()
                    .parse::<u64>()?
                    .checked_mul(1024 * 1024)
                    .ok_or("memory budget overflows bytes")?;
            }
            "--spatial-workers" => spatial_workers = Some(value.to_string_lossy().parse()?),
            _ => return Err(format!("unknown option: {flag}").into()),
        }
        index += 2;
    }

    // V8 fixes the format, points per block, spatial divisions and row order.
    let mut options = ConvertOptions::with_memory_budget(max_error, max_memory_bytes)?;
    options = options.with_pipelined_writes(pipelined_writes);
    if let Some(bytes) = page_target_bytes {
        options = options.with_page_target_bytes(bytes);
    }
    if let Some(workers) = spatial_workers {
        options = options.with_spatial_workers(workers)?;
    }
    let cancellation_requested = Arc::new(AtomicBool::new(false));
    let signal_flag = Arc::clone(&cancellation_requested);
    ctrlc::set_handler(move || signal_flag.store(true, Ordering::Relaxed))?;

    let mut previous_stage = None;
    let mut next_percent = 0;
    let mut on_progress = |progress: viewer_convert::ConversionProgress| {
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
    let report = convert_file_with_progress(&input, &output, options, &mut on_progress)?;
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

/// `viewer_convert verify FILE.oct`: reads every page and runs the global checks.
fn verify(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let report = viewer_convert::verify_file(path)
        .map_err(|error| format!("verification of {} failed: {error}", path.display()))?;
    match report.paged {
        Some(paged) => println!(
            "OK {}: {} nodes, {} blocks, {} pages (max {} B; by levels k=1..5: {:?}), index {} B",
            report.version,
            paged.nodes,
            paged.blocks,
            paged.pages,
            paged.max_page_bytes,
            paged.pages_by_levels,
            paged.index_bytes
        ),
        None => println!(
            "OK {}: {} blocks (the whole index is checked when the file is opened)",
            report.version, report.block_count
        ),
    }
    Ok(())
}

fn print_help() {
    eprintln!(
        "Usage: viewer_convert INPUT.las|INPUT.laz|INPUT.copc.laz OUTPUT.oct [--max-error E] [--max-memory-mib MIB] [--spatial-workers N] [--pipeline-writes]\n\
         Output: OUTPUT.oct (coordinates/index) + OUTPUT.octattrs (optional color/attributes). Reported bytes include both.\n\
         viewer_convert verify FILE.oct reads every index page of a paged file and runs the global checks (and the companion footer when FILE.octattrs exists); exit code 0 if valid, 1 otherwise.\n\
         The output is always V8, which fixes 64000 points per block, 2 spatial divisions per axis and hybrid row order (blocks whose geometry shrinks are re-sorted by Morton key); --format-version, --row-order, --points-per-block, --spatial-divisions and --attr-codec (attributes are LZ4 only, ADR 0060) were removed and are refused.\n\
         Defaults: maximum quantization error 0.01 in source units, estimated memory budget 512 MiB.\n\
         --spatial-workers uses 1 to 8 bounded workers for root subtrees; the default is min(4, available parallelism).\n\
         --pipeline-writes overlaps one preparation batch with ordered writes within the estimated memory budget.\n\
         COPC points are read sequentially and replanned into an adaptive octree; the COPC hierarchy is not retained. EPT input and --checkpoint/--resume were removed with V8 (ADR 0061).\n\
         The converter preserves intensity, classification, returns, scan angle, user data and point source ID, plus GPS time, RGB16 and NIR when present, and reports omitted LAS dimensions."
    );
}
