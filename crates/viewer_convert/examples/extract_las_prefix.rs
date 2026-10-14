//! Bounded real-data fixture preparation; preserves source point format/transforms.
use las::{Builder, Reader, Writer};
use std::{fs::OpenOptions, io::BufWriter, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: extract_las_prefix INPUT.las|INPUT.laz OUTPUT.las POINTS".into());
    }
    let count = args[2].to_string_lossy().parse::<usize>()?;
    if count == 0 {
        return Err("positive point count required".into());
    }
    let output = PathBuf::from(&args[1]);
    if output.extension().and_then(|e| e.to_str()) != Some("las") {
        return Err("output must be an uncompressed .las fixture".into());
    }
    let mut reader = Reader::from_path(&args[0])?;
    let mut builder = Builder::from(reader.header().clone());
    builder.point_format.is_compressed = false;
    let header = builder.into_header()?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    let mut writer = Writer::new(BufWriter::new(file), header)?;
    let mut seen = [[false; 256]; 4];
    let mut written = 0;
    while written < count {
        let batch = reader.read_points((count - written).min(65_536) as u64)?;
        if batch.is_empty() {
            break;
        }
        for point in batch.points() {
            let point = point?;
            for (column, value) in [
                u8::from(point.classification),
                point.return_number,
                point.number_of_returns,
                point.user_data,
            ]
            .into_iter()
            .enumerate()
            {
                seen[column][usize::from(value)] = true;
            }
            writer.write_point(point)?;
            written += 1;
        }
    }
    writer.close()?;
    println!(
        "points={written} requested={count} classification_unique={} return_number_unique={} number_of_returns_unique={} user_data_unique={}",
        seen[0].iter().filter(|&&v| v).count(),
        seen[1].iter().filter(|&&v| v).count(),
        seen[2].iter().filter(|&&v| v).count(),
        seen[3].iter().filter(|&&v| v).count()
    );
    Ok(())
}
