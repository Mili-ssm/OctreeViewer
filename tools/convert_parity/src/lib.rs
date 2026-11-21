//! wasm32 half of the conversion parity check: converts LAS/LAZ bytes held in memory with the
//! serial core path and hands the output pair back to Node, which hashes it and compares it with
//! the bytes `viewer_convert` writes natively (`native_convert`).
//!
//! The ABI is plain integers over linear memory so the module needs no wasm-bindgen glue.

use std::cell::RefCell;
use std::io::Cursor;
use std::sync::Arc;

use viewer_convert_core::{
    ConvertOptions, LasFactory, MemBytes, MemSpoolStore, NoClock, convert_serial,
    validate_output_serial,
};

#[derive(Default)]
struct Outcome {
    geometry: Vec<u8>,
    attributes: Vec<u8>,
    error: String,
    block_count: u64,
}

thread_local! {
    static OUTCOME: RefCell<Outcome> = RefCell::new(Outcome::default());
}

fn with_outcome<T>(read: impl FnOnce(&Outcome) -> T) -> T {
    OUTCOME.with(|outcome| read(&outcome.borrow()))
}

/// Returns a buffer of `len` bytes that the host fills with the input file.
#[unsafe(no_mangle)]
pub extern "C" fn convert_parity_alloc(len: usize) -> *mut u8 {
    let mut buffer = vec![0_u8; len].into_boxed_slice();
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

fn run(input: &[u8], workers: usize) -> Result<Outcome, String> {
    let mut options = ConvertOptions::default();
    if workers != 0 {
        options = options
            .with_spatial_workers(workers)
            .map_err(|error| error.to_string())?;
    }
    let factory = LasFactory::new(MemBytes(Arc::from(input)));
    let store = MemSpoolStore::new(false);
    let output = convert_serial(
        &factory,
        &store,
        &NoClock,
        options,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        &mut |_| true,
    )
    .map_err(|error| error.to_string())?;
    let geometry = output.geometry.into_inner();
    let attributes = output.attributes.into_inner();
    validate_output_serial(
        Cursor::new(geometry.as_slice()),
        Cursor::new(attributes.as_slice()),
        &output.plan,
        &mut |_| true,
    )
    .map_err(|error| error.to_string())?;
    Ok(Outcome {
        geometry,
        attributes,
        error: String::new(),
        block_count: output.plan.block_count,
    })
}

/// Converts and validates the buffer from `convert_parity_alloc` as V8 (its fixed parameters);
/// `workers == 0` keeps the core default. Returns 1 on success; on failure returns 0 and the
/// message is at `error_ptr`.
///
/// # Safety
/// `ptr` must come from `convert_parity_alloc(len)` and stay unmodified during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn convert_parity_run(ptr: *const u8, len: usize, workers: u32) -> u32 {
    // SAFETY: the caller passes a buffer from `convert_parity_alloc` of exactly `len` bytes.
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    let result = run(input, workers as usize);
    let succeeded = result.is_ok();
    OUTCOME.with(|outcome| {
        *outcome.borrow_mut() = result.unwrap_or_else(|error| Outcome {
            error,
            ..Outcome::default()
        });
    });
    u32::from(succeeded)
}

#[unsafe(no_mangle)]
pub extern "C" fn convert_parity_block_count() -> u64 {
    with_outcome(|outcome| outcome.block_count)
}

#[unsafe(no_mangle)]
pub extern "C" fn convert_parity_geometry_ptr() -> *const u8 {
    with_outcome(|outcome| outcome.geometry.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn convert_parity_geometry_len() -> usize {
    with_outcome(|outcome| outcome.geometry.len())
}

#[unsafe(no_mangle)]
pub extern "C" fn convert_parity_attributes_ptr() -> *const u8 {
    with_outcome(|outcome| outcome.attributes.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn convert_parity_attributes_len() -> usize {
    with_outcome(|outcome| outcome.attributes.len())
}

#[unsafe(no_mangle)]
pub extern "C" fn convert_parity_error_ptr() -> *const u8 {
    with_outcome(|outcome| outcome.error.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn convert_parity_error_len() -> usize {
    with_outcome(|outcome| outcome.error.len())
}
