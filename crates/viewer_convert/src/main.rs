use std::{env, path::PathBuf, process::ExitCode};

use viewer_convert::{ConvertOptions, convert_file};

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
    let mut index = 2;
    while index < arguments.len() {
        let flag = arguments[index].to_string_lossy();
        let value = arguments.get(index + 1).ok_or("missing value for option")?;
        match flag.as_ref() {
            "--points-per-block" => points_per_block = value.to_string_lossy().parse()?,
            "--max-error" => max_error = value.to_string_lossy().parse()?,
            _ => return Err(format!("unknown option: {flag}").into()),
        }
        index += 2;
    }

    let options = ConvertOptions::new(points_per_block, max_error)?;
    let report = convert_file(&input, &output, options)?;
    eprintln!(
        "Converted {} points in {} blocks ({} bytes) to {}",
        report.point_count,
        report.block_count,
        report.output_bytes,
        output.display()
    );
    if !report.omitted_dimensions.is_empty() {
        eprintln!("Not preserved: {}", report.omitted_dimensions.join(", "));
    }
    Ok(())
}

fn print_help() {
    eprintln!(
        "Usage: viewer_convert INPUT.las|INPUT.laz OUTPUT.oct [--points-per-block N] [--max-error E]\n\
         Defaults: 262144 points per block, maximum quantization error 0.01 in source units.\n\
         The converter writes one root node, preserves intensity and RGB16, and reports omitted LAS dimensions."
    );
}
