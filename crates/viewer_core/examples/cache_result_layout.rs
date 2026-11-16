//! Reports target-specific inline layouts; this does not measure RSS or throughput.
use std::mem::{align_of, size_of};

use viewer_core::{BlockId, CacheInsertRejection, CacheInsertResult};

/// Field types of the result before the enum refactor, using Rust's default layout.
#[expect(dead_code, reason = "only its size and alignment are reported")]
struct PreviousCacheInsertResult {
    cached: bool,
    rejection: Option<CacheInsertRejection>,
    evicted_blocks: Vec<BlockId>,
    accounted_bytes: usize,
}

fn main() {
    println!(
        "arch={} pointer_bits={} previous_bytes={} previous_align={} enum_bytes={} enum_align={}",
        std::env::consts::ARCH,
        usize::BITS,
        size_of::<PreviousCacheInsertResult>(),
        align_of::<PreviousCacheInsertResult>(),
        size_of::<CacheInsertResult>(),
        align_of::<CacheInsertResult>(),
    );
}
