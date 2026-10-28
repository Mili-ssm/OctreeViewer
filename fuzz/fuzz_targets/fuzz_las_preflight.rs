#![no_main]

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use viewer_convert::{ConvertOptions, convert_file};
use viewer_core::las::preflight_las;

/// Scratch directory, preferably on tmpfs: one input and one output file reused per process.
fn scratch() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let base = if Path::new("/dev/shm").is_dir() {
            PathBuf::from("/dev/shm")
        } else {
            std::env::temp_dir()
        };
        let dir = base.join(format!("octree-fuzz-las-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    })
}

// The LAZ chunk-table check lives in `viewer_convert` (crate-private), so once the header
// preflight accepts the input the whole converter entry point runs: chunk-table check, `las`
// reader (laz + laz-parallel) and conversion. Errors are fine; a panic, abort or allocation
// above the libFuzzer limits after a successful preflight is a bypass.
fuzz_target!(|data: &[u8]| {
    let len = data.len() as u64;
    if preflight_las(&mut Cursor::new(data), len).is_err() {
        return;
    }
    let dir = scratch();
    let input = dir.join("in.laz");
    std::fs::write(&input, data).unwrap();
    let _ = convert_file(&input, &dir.join("out.oct"), ConvertOptions::default());
});
