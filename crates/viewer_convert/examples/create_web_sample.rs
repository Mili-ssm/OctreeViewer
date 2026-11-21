//! Builds the web demo sample: a deterministic wave surface written as LAS and converted with the
//! converter's default options, so the output is the default format (V8) with `.octattrs`.

use std::{
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use las::{Builder, Color, Point, Writer, point::Format};
use viewer_convert::{ConvertOptions, convert_file};
use viewer_format::{FormatReader, FormatVersion, OctreeReader};

const SIDE: u32 = 768;
const MAX_BYTES: u64 = 8 * 1024 * 1024;

fn write_las(path: &std::path::Path, side: u32) -> Result<u64, Box<dyn Error>> {
    let spacing = 64.0 / f64::from(side);
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(2)?;
    let transform = |offset| las::Transform {
        scale: 0.001,
        offset,
    };
    builder.transforms = las::Vector {
        x: transform(0.0),
        y: transform(0.0),
        z: transform(0.0),
    };
    let mut writer = Writer::from_path(path, builder.into_header()?)?;
    for j in 0..side {
        for i in 0..side {
            let (x, y) = (f64::from(i) * spacing, f64::from(j) * spacing);
            #[expect(
                clippy::disallowed_methods,
                reason = "demo-surface generator, not the conversion output path"
            )]
            let z = ((0.19 * x).sin() * (0.16 * y).cos() * 2.0)
                + 2.0
                + 0.06 * (9.0 * x).sin() * (7.0 * y).cos();
            let channel = |value: f64| u16::from(value as u8) * 257;
            writer.write_point(Point {
                x,
                y,
                z,
                intensity: (x * 1000.0 + y * 8.0) as u16,
                classification: las::point::Classification::new(
                    ((x as u32 / 16 + y as u32 / 16) % 4) as u8,
                )?,
                color: Some(Color::new(
                    channel(40.0 + x * 3.0),
                    channel(45.0 + y * 3.0),
                    channel(120.0 + z * 25.0),
                )),
                ..Point::default()
            })?;
        }
    }
    writer.close()?;
    Ok(u64::from(side) * u64::from(side))
}

fn main() -> Result<(), Box<dyn Error>> {
    let output = env::args_os().nth(1).map_or_else(
        || PathBuf::from("examples/web-host/sample.oct"),
        PathBuf::from,
    );
    let scratch = env::temp_dir().join(format!("create_web_sample_{}", std::process::id()));
    let result = generate(&output, &scratch);
    let _ = fs::remove_dir_all(&scratch);
    result
}

fn generate(output: &Path, scratch: &Path) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir_all(scratch)?;
    let input = scratch.join("sample.las");
    // The converter refuses to overwrite, and the sample is meant to be regenerated.
    let attributes = output.with_extension("octattrs");
    for stale in [output, attributes.as_path()] {
        if stale.exists() {
            fs::remove_file(stale)?;
        }
    }
    let points = write_las(&input, SIDE)?;
    convert_file(&input, output, ConvertOptions::default().validate()?)?;
    let size = fs::metadata(output)?.len() + fs::metadata(&attributes)?.len();
    eprintln!("side {SIDE}: {points} points, {size} bytes");
    if size > MAX_BYTES {
        return Err(format!("sample is {size} bytes, over the {MAX_BYTES} byte guard").into());
    }
    // ADR 0047: the demo uses the latest stable format, which is the converter's default output.
    let version = FormatReader::new(fs::File::open(output)?)?.format_version();
    if version != FormatVersion::DEFAULT_OUTPUT {
        return Err(format!(
            "sample is {version:?}, expected the default output {:?}",
            FormatVersion::DEFAULT_OUTPUT
        )
        .into());
    }
    Ok(())
}
