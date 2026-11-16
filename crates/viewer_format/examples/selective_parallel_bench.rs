//! Compare selective-read checksums with 1/2/4 readers. Timings are observations, not speedup
//! claims; request/catalog vectors are outside `ReadBudget`, as are runtime metadata and RSS.

use std::{
    fs::File,
    ops::ControlFlow,
    path::{Path, PathBuf},
    time::Instant,
};

use viewer_core::{AttributeValues, BlockId};
use viewer_format::{
    FormatReader, OctreeReader, ParallelReadStatus, ReadBudget, ReadSelection, SelectedRead,
    read_selected_parallel,
};

const MAX_WORKERS: usize = 8;
const MIB: u64 = 1024 * 1024;

#[derive(Clone, Copy)]
enum Mode {
    Xyz,
    Geometry,
    Rgb,
    Attribute(u32),
}

impl Mode {
    fn selection(self) -> ReadSelection {
        match self {
            Self::Xyz => ReadSelection::Positions,
            Self::Geometry => ReadSelection::Geometry,
            Self::Rgb => ReadSelection::DisplayColors,
            Self::Attribute(schema_index) => ReadSelection::Attribute { schema_index },
        }
    }

    fn name(self) -> String {
        match self {
            Self::Xyz => "xyz".to_owned(),
            Self::Geometry => "geometry".to_owned(),
            Self::Rgb => "rgb".to_owned(),
            Self::Attribute(schema_index) => format!("attribute:{schema_index}"),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = PathBuf::from(args.next().ok_or(
        "usage: selective_parallel_bench INPUT.oct xyz|geometry|rgb|attribute:N WORKERS BUDGET_MIB",
    )?);
    let mode = parse_mode(&args.next().ok_or("missing selection")?)?;
    let workers: usize = args.next().ok_or("missing worker count")?.parse()?;
    let budget_mib: u64 = args.next().ok_or("missing budget MiB")?.parse()?;
    if args.next().is_some() {
        return Err("unexpected extra arguments".into());
    }
    if !(1..=MAX_WORKERS).contains(&workers) {
        return Err("workers must be from 1 to 8".into());
    }
    if budget_mib == 0 {
        return Err("budget MiB must be greater than zero".into());
    }
    let budget_bytes = budget_mib
        .checked_mul(MIB)
        .ok_or("budget MiB overflows bytes")?;

    let base = FormatReader::new(File::open(&input)?)?;
    let version = base.format_version();
    let stats = base.block_storage_stats()?;
    let requests: Vec<_> = stats
        .iter()
        .map(|block| (block.id, mode.selection()))
        .collect();
    let attrs_path = attributes_path(&input);
    let attach_attributes = match std::fs::metadata(&attrs_path) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let mut readers = Vec::with_capacity(workers);
    for _ in 0..workers {
        let mut reader = base.fork_reader(File::open(&input)?);
        if attach_attributes {
            reader.attach_attributes(File::open(&attrs_path)?)?;
        }
        readers.push(reader);
    }

    let budget = ReadBudget::new(budget_bytes);
    let mut checksum = Fnv64::new();
    let mut blocks = 0_u64;
    let mut rows = 0_u64;
    let started = Instant::now();
    let progress = read_selected_parallel(
        &mut readers,
        &requests,
        &budget,
        |request_index, selected| {
            hash_selected(&mut checksum, requests[request_index].0, selected);
            blocks += 1;
            rows += selected_rows(selected) as u64;
            ControlFlow::Continue(())
        },
    )?;
    let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    if progress.status != ParallelReadStatus::Complete {
        return Err(format!(
            "read ended with {:?} after {} blocks",
            progress.status, progress.delivered
        )
        .into());
    }
    let reserved_after = budget.reserved_bytes();
    if reserved_after != 0 {
        return Err(format!("read completed with {reserved_after} reserved bytes").into());
    }

    println!(
        "format_version={version} mode={} workers={workers} budget_mib={budget_mib} peak_reserved_bytes={} reserved_after={reserved_after} status={:?} blocks={blocks} rows={rows} checksum={:016x} wall_ms={wall_ms:.3}",
        mode.name(),
        budget.peak_reserved_bytes(),
        progress.status,
        checksum.finish(),
    );
    Ok(())
}

fn parse_mode(text: &str) -> Result<Mode, Box<dyn std::error::Error>> {
    match text {
        "xyz" => Ok(Mode::Xyz),
        "geometry" => Ok(Mode::Geometry),
        "rgb" => Ok(Mode::Rgb),
        _ => {
            let schema_index = text
                .strip_prefix("attribute:")
                .ok_or("selection must be xyz, geometry, rgb, or attribute:N")?
                .parse::<u32>()?;
            Ok(Mode::Attribute(schema_index))
        }
    }
}

fn attributes_path(input: &Path) -> PathBuf {
    input.with_extension("octattrs")
}

fn selected_rows(read: &SelectedRead) -> usize {
    match read {
        SelectedRead::Positions(points) | SelectedRead::Geometry(points) => points.len(),
        SelectedRead::DisplayColors(colors) => colors.len(),
        SelectedRead::Attribute(column) => {
            let components = column.descriptor().component_count() as usize;
            column.values().len() / components
        }
    }
}

fn hash_selected(checksum: &mut Fnv64, id: BlockId, read: &SelectedRead) {
    checksum.write(&id.get().to_le_bytes());
    match read {
        SelectedRead::Positions(points) => {
            checksum.write(&[0]);
            for point in points.points() {
                for coordinate in point.position {
                    checksum.write(&coordinate.to_bits().to_le_bytes());
                }
            }
        }
        SelectedRead::Geometry(points) => {
            checksum.write(&[1]);
            for point in points.points() {
                for coordinate in point.position {
                    checksum.write(&coordinate.to_bits().to_le_bytes());
                }
                checksum.write(&[point.color.r, point.color.g, point.color.b]);
            }
        }
        SelectedRead::DisplayColors(colors) => {
            checksum.write(&[2]);
            for color in colors {
                checksum.write(&[color.r, color.g, color.b]);
            }
        }
        SelectedRead::Attribute(column) => {
            checksum.write(&[3]);
            hash_values(checksum, column.values());
        }
    }
}

fn hash_values(checksum: &mut Fnv64, values: &AttributeValues) {
    checksum.write(&[match values {
        AttributeValues::U8(_) => 0,
        AttributeValues::I8(_) => 1,
        AttributeValues::U16(_) => 2,
        AttributeValues::I16(_) => 3,
        AttributeValues::U32(_) => 4,
        AttributeValues::I32(_) => 5,
        AttributeValues::U64(_) => 6,
        AttributeValues::I64(_) => 7,
        AttributeValues::F32(_) => 8,
        AttributeValues::F64(_) => 9,
    }]);
    macro_rules! hash_values {
        ($values:expr) => {
            for value in $values {
                checksum.write(&value.to_le_bytes());
            }
        };
    }
    match values {
        AttributeValues::U8(values) => checksum.write(values),
        AttributeValues::I8(values) => hash_values!(values),
        AttributeValues::U16(values) => hash_values!(values),
        AttributeValues::I16(values) => hash_values!(values),
        AttributeValues::U32(values) => hash_values!(values),
        AttributeValues::I32(values) => hash_values!(values),
        AttributeValues::U64(values) => hash_values!(values),
        AttributeValues::I64(values) => hash_values!(values),
        AttributeValues::F32(values) => {
            for value in values {
                checksum.write(&value.to_bits().to_le_bytes());
            }
        }
        AttributeValues::F64(values) => {
            for value in values {
                checksum.write(&value.to_bits().to_le_bytes());
            }
        }
    }
}

struct Fnv64(u64);

impl Fnv64 {
    const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    const fn finish(&self) -> u64 {
        self.0
    }
}
