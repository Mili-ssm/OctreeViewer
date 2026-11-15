//! Native 4x reference for the wasm time gate: `native_hybrid INPUT OUTPUT.oct --divisions D
//! --points-per-block P --spool-budget-bytes B --zones-dir DIR [--store mem]` runs the serial core
//! path with one spatial worker over `HybridSpoolStore<VecRamTier, FilePageTier>` (or
//! `MemSpoolStore` with `--store mem`), so the wasm build and this build run the same code. The
//! input is read through a 256 KiB `BufReader`, like the browser's Blob cache. Prints one JSON
//! line; the output pair is written to `OUTPUT.oct` and `OUTPUT.octattrs`.
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use viewer_convert_core::{
    ByteSource, ConvertError, ConvertOptions, HybridConfig, HybridSpoolStore, HybridStats,
    LasFactory, MemSpoolStore, NoClock, PageTier, SpoolEstimate, VecRamTier, convert_serial,
    estimate_spools, inspect_source,
};

const READ_BUFFER_BYTES: usize = 256 * 1024;

struct FileBytes {
    path: PathBuf,
    len: u64,
}

impl ByteSource for FileBytes {
    type Reader = BufReader<File>;

    fn open(&self) -> Result<Self::Reader, ConvertError> {
        Ok(BufReader::with_capacity(
            READ_BUFFER_BYTES,
            File::open(&self.path)?,
        ))
    }

    fn len(&self) -> u64 {
        self.len
    }
}

/// Spill zones as plain files; positional I/O needs no shared cursor.
struct FilePageTier {
    zones: Vec<File>,
}

impl FilePageTier {
    fn create(dir: &Path, count: u32) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let zones = (0..count)
            .map(|index| {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(dir.join(format!("zone-{index}")))
            })
            .collect::<std::io::Result<_>>()?;
        Ok(Self { zones })
    }

    fn zone(&self, zone: u32) -> Result<&File, ConvertError> {
        self.zones.get(zone as usize).ok_or_else(|| {
            ConvertError::Io(std::io::Error::other(format!("zone {zone} does not exist")))
        })
    }
}

impl PageTier for FilePageTier {
    fn zone_count(&self) -> u32 {
        self.zones.len() as u32
    }

    fn write(&self, zone: u32, at: u64, bytes: &[u8]) -> Result<usize, ConvertError> {
        self.zone(zone)?.write_all_at(bytes, at)?;
        Ok(bytes.len())
    }

    fn read(&self, zone: u32, at: u64, out: &mut [u8]) -> Result<(), ConvertError> {
        Ok(self.zone(zone)?.read_exact_at(out, at)?)
    }

    fn release(&self, zone: u32) -> Result<(), ConvertError> {
        Ok(self.zone(zone)?.set_len(0)?)
    }
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn compress(state: &mut [u32; 8], block: &[u8]) {
    let mut w = [0_u32; 64];
    for (word, chunk) in w.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *slot = slot.wrapping_add(value);
    }
}

/// Streaming SHA-256 of a file (the tool has no hashing dependency, and outputs can be large).
fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut file = File::open(path)?;
    let mut buffer = vec![0_u8; 1 << 20];
    let mut pending = Vec::with_capacity(1 << 20);
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        pending.extend_from_slice(&buffer[..read]);
        let whole = pending.len() / 64 * 64;
        for block in pending[..whole].as_chunks::<64>().0 {
            compress(&mut state, block);
        }
        pending.drain(..whole);
    }
    pending.push(0x80);
    while pending.len() % 64 != 56 {
        pending.push(0);
    }
    pending.extend_from_slice(&(total * 8).to_be_bytes());
    for block in pending.as_chunks::<64>().0 {
        compress(&mut state, block);
    }
    Ok(state.iter().map(|word| format!("{word:08x}")).collect())
}

struct Args {
    input: PathBuf,
    output: PathBuf,
    divisions: u8,
    points_per_block: u64,
    budget: u64,
    zones_dir: PathBuf,
    mem: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut positional = Vec::new();
    let (mut divisions, mut ppb, mut budget, mut zones_dir, mut mem) =
        (None, None, None, None, false);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--divisions" => divisions = Some(value(&arg)?),
            "--points-per-block" => ppb = Some(value(&arg)?),
            "--spool-budget-bytes" => budget = Some(value(&arg)?),
            "--zones-dir" => zones_dir = Some(value(&arg)?),
            "--store" => match value(&arg)?.as_str() {
                "mem" => mem = true,
                "hybrid" => mem = false,
                other => return Err(format!("unknown store {other}")),
            },
            _ => positional.push(arg),
        }
    }
    let number = |name: &str, text: Option<String>| -> Result<u64, String> {
        text.ok_or_else(|| format!("{name} is required"))?
            .parse()
            .map_err(|_| format!("{name} must be an integer"))
    };
    let [input, output] = <[String; 2]>::try_from(positional)
        .map_err(|_| "expected INPUT and OUTPUT.oct".to_string())?;
    Ok(Args {
        input: input.into(),
        output: output.into(),
        divisions: u8::try_from(number("--divisions", divisions)?)
            .map_err(|_| "--divisions is too large".to_string())?,
        points_per_block: number("--points-per-block", ppb)?,
        budget: number("--spool-budget-bytes", budget)?,
        zones_dir: zones_dir.ok_or("--zones-dir is required")?.into(),
        mem,
    })
}

fn stats_json(stats: &HybridStats) -> String {
    format!(
        "\"livePayloadBytes\":{},\"ramRetainedBytes\":{},\"ramPeakBytes\":{},\"overBudgetPeakBytes\":{},\"spilledBytes\":{},\"spilledSpools\":{},\"pageReadBytes\":{},\"pagesInUse\":{},\"pagesPeak\":{},\"zonesReleased\":{}",
        stats.live_payload_bytes,
        stats.ram_retained_bytes,
        stats.ram_peak_bytes,
        stats.over_budget_peak_bytes,
        stats.spilled_bytes,
        stats.spilled_spools,
        stats.page_read_bytes,
        stats.pages_in_use,
        stats.pages_peak,
        stats.zones_released
    )
}

fn estimate_json(estimate: &SpoolEstimate) -> String {
    format!(
        "{{\"sourceSpoolBytes\":{},\"livePeakBytes\":{},\"nodes\":{},\"spilledPeakBytes\":{},\"zones\":{},\"outputBytes\":{},\"ramOnlyBytes\":{}}}",
        estimate.source_spool_bytes,
        estimate.live_peak_bytes,
        estimate.nodes,
        estimate.spilled_peak_bytes,
        estimate.zones,
        estimate.output_bytes,
        estimate.ram_only_bytes
    )
}

fn run(args: &Args) -> Result<String, Box<dyn std::error::Error>> {
    let options = ConvertOptions::new(args.points_per_block, 0.01)?
        .with_format_version("v5".parse()?)?
        .with_spatial_divisions(args.divisions)?
        .with_spatial_workers(1)?;
    let factory = LasFactory::new(FileBytes {
        path: args.input.clone(),
        len: std::fs::metadata(&args.input)?.len(),
    });
    let config = HybridConfig::web(args.budget);
    let estimate = estimate_spools(&inspect_source(&factory)?, options, config)?;
    let attributes_path = PathBuf::from(format!("{}attrs", args.output.display()));
    for path in [&args.output, &attributes_path] {
        let _ = std::fs::remove_file(path);
    }
    let create = |path: &Path| -> std::io::Result<BufWriter<File>> {
        Ok(BufWriter::new(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(path)?,
        ))
    };
    let (geometry, attributes) = (create(&args.output)?, create(&attributes_path)?);

    let started = Instant::now();
    let (output, stats) = if args.mem {
        let store = MemSpoolStore::new(false);
        let output = convert_serial(
            &factory,
            &store,
            &NoClock,
            options,
            geometry,
            attributes,
            &mut |_| true,
        )?;
        (output, HybridStats::default())
    } else {
        let pages = FilePageTier::create(&args.zones_dir, estimate.zones)?;
        let store = HybridSpoolStore::new(VecRamTier::new(), pages, config, false)?;
        let output = convert_serial(
            &factory,
            &store,
            &NoClock,
            options,
            geometry,
            attributes,
            &mut |_| true,
        )?;
        (output, store.stats())
    };
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let blocks = output.plan.block_count;
    // Flush the buffered writers before the files are hashed.
    for mut writer in [output.geometry, output.attributes] {
        std::io::Write::flush(&mut writer)?;
    }
    Ok(format!(
        "{{\"elapsedMs\":{elapsed_ms:.1},\"blocks\":{blocks},\"store\":\"{}\",{},\"estimate\":{},\"octSha256\":\"{}\",\"octattrsSha256\":\"{}\"}}",
        if args.mem { "mem" } else { "hybrid" },
        stats_json(&stats),
        estimate_json(&estimate),
        sha256_file(&args.output)?,
        sha256_file(&attributes_path)?
    ))
}

fn main() {
    let args = parse_args().unwrap_or_else(|message| {
        eprintln!("{message}");
        eprintln!(
            "usage: native_hybrid INPUT OUTPUT.oct --divisions D --points-per-block P --spool-budget-bytes B --zones-dir DIR [--store mem]"
        );
        std::process::exit(2);
    });
    match run(&args) {
        Ok(line) => println!("{line}"),
        Err(error) => {
            eprintln!("native_hybrid failed: {error}");
            std::process::exit(1);
        }
    }
}
