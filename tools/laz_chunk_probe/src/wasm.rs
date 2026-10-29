//! Import-free exports driven by `run-wasm.mjs`; the host times each call itself because
//! `std::time::Instant` is unavailable on `wasm32-unknown-unknown`.

use std::io::Cursor;
use std::sync::Mutex;

use crate::{Fixture, RangeDecoder, decode_chunk_range};

const FAILED: u64 = u64::MAX;

static INPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static LOADED: Mutex<Option<Fixture>> = Mutex::new(None);

/// Allocates the fixture buffer for the host to fill; returns its address.
#[unsafe(no_mangle)]
pub extern "C" fn laz_probe_alloc(len: usize) -> *mut u8 {
    let mut input = INPUT.lock().unwrap();
    *input = vec![0; len];
    input.as_mut_ptr()
}

/// Parses the buffer written after `laz_probe_alloc`; returns the chunk count or `u64::MAX`.
#[unsafe(no_mangle)]
pub extern "C" fn laz_probe_load() -> u64 {
    let bytes = std::mem::take(&mut *INPUT.lock().unwrap());
    match Fixture::parse(bytes) {
        Ok(fixture) => {
            let chunks = fixture.index.table.len() as u64;
            *LOADED.lock().unwrap() = Some(fixture);
            chunks
        }
        Err(_) => FAILED,
    }
}

fn with_fixture(run: impl FnOnce(&Fixture) -> Option<u64>) -> u64 {
    LOADED
        .lock()
        .unwrap()
        .as_ref()
        .and_then(run)
        .unwrap_or(FAILED)
}

/// Points in the fixture.
#[unsafe(no_mangle)]
pub extern "C" fn laz_probe_points() -> u64 {
    with_fixture(|fixture| {
        Some(
            fixture
                .index
                .table
                .as_ref()
                .iter()
                .map(|entry| entry.point_count)
                .sum(),
        )
    })
}

/// Decodes all chunks with one decoder started at chunk 0; returns the FNV-1a of the output.
#[unsafe(no_mangle)]
pub extern "C" fn laz_probe_decode_batch() -> u64 {
    with_fixture(|fixture| {
        let mut out = Vec::new();
        let chunks = fixture.index.table.len();
        decode_chunk_range(
            &mut Cursor::new(&fixture.data),
            &fixture.index,
            0..chunks,
            &mut out,
        )
        .ok()?;
        Some(crate::fnv1a(&out))
    })
}

/// Decodes each chunk with its own freshly started decoder; returns the FNV-1a of the output.
#[unsafe(no_mangle)]
pub extern "C" fn laz_probe_decode_each() -> u64 {
    with_fixture(|fixture| {
        let mut out = Vec::new();
        for chunk in 0..fixture.index.table.len() {
            decode_chunk_range(
                &mut Cursor::new(&fixture.data),
                &fixture.index,
                chunk..chunk + 1,
                &mut out,
            )
            .ok()?;
        }
        Some(crate::fnv1a(&out))
    })
}

/// Builds a decoder per chunk (stream assembly + `LasZipDecompressor::new`) without decoding.
#[unsafe(no_mangle)]
pub extern "C" fn laz_probe_setup_each() -> u64 {
    with_fixture(|fixture| {
        let mut points = 0;
        for chunk in 0..fixture.index.table.len() {
            let decoder = RangeDecoder::new(
                &mut Cursor::new(&fixture.data),
                &fixture.index,
                chunk..chunk + 1,
            )
            .ok()?;
            points += decoder.points();
        }
        Some(points)
    })
}
