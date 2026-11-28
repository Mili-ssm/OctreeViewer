#![no_main]

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use viewer_convert::{ConvertOptions, convert_file};
use viewer_convert_core::las_preflight::preflight_las;

/// Per-process scratch file stem, preferably on tmpfs. Files are removed after every iteration
/// because `fuzz_target!` has no teardown; nothing but two short-lived files is ever left behind.
fn scratch_stem() -> &'static Path {
    static STEM: OnceLock<PathBuf> = OnceLock::new();
    STEM.get_or_init(|| {
        let base = if Path::new("/dev/shm").is_dir() {
            PathBuf::from("/dev/shm")
        } else {
            std::env::temp_dir()
        };
        base.join(format!("octree-fuzz-las-{}", std::process::id()))
    })
}

// The LAZ chunk-table check runs inside `viewer_convert`'s `convert_file`, so once the header
// preflight accepts the input the whole converter entry point runs: chunk-table check, `las`
// reader (laz + laz-parallel) and conversion. Errors are fine; a panic, abort or allocation
// above the libFuzzer limits after a successful preflight is a bypass.
//
// `convert_file` contains panics raised inside the third-party LAZ decoder and returns them as
// errors, but libfuzzer-sys installs a panic hook that aborts before `catch_unwind` can run.
// Replace it once per process with a quiet hook. Invariant: a panic that is *not* caught by
// our code still unwinds into libfuzzer-sys's own `catch_unwind`, which aborts and reports it,
// so real bypasses stay visible.
fn allow_contained_panics() {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| std::panic::set_hook(Box::new(|_| {})));
}

fuzz_target!(|data: &[u8]| {
    allow_contained_panics();
    let len = data.len() as u64;
    if preflight_las(&mut Cursor::new(data), len).is_err() {
        return;
    }
    let stem = scratch_stem();
    let input = stem.with_extension("laz");
    let output = stem.with_extension("oct");
    std::fs::write(&input, data).unwrap();
    let _ = convert_file(&input, &output, ConvertOptions::default());
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(output.with_extension("octattrs"));
    let _ = std::fs::remove_file(&output);
});
