//! glibc allocator tuning for the conversion.
//!
//! glibc raises its `mmap` threshold to the size of the largest buffer freed so far (up to
//! 32 MiB), so later large buffers (sample grids and LOD buffers of every worker) come from
//! per-thread arenas, which fragment and keep the freed pages: measured on Belvedere with a 256³
//! sample grid, 410 MB of RSS for a 150 MB live heap. A fixed 128 KiB threshold returns every
//! large buffer to the OS on `free` (152 MB RSS, same wall time).

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn limit_retention() {
    use std::sync::Once;

    unsafe extern "C" {
        fn mallopt(param: i32, value: i32) -> i32;
    }
    const M_MMAP_THRESHOLD: i32 = -3;
    const THRESHOLD_BYTES: i32 = 128 * 1024;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: `mallopt` only sets a process-wide allocator parameter; failure (0) just
        // leaves the default behavior.
        unsafe { mallopt(M_MMAP_THRESHOLD, THRESHOLD_BYTES) };
    });
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub(crate) fn limit_retention() {}
