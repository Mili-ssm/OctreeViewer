//! `Read`/`Write`/`Seek` adapters over `FileSystemSyncAccessHandle`. Each call passes a view of
//! wasm memory (no staging `Uint8Array`), so the handle must not retain it. Handles are cast with
//! `unchecked_into`: the Node fakes are plain objects, which `dyn_into` would reject.

use std::io::{self, Read, Seek, SeekFrom, Write};

use viewer_convert_core::{ConvertError, pipeline::hybrid::storage_full};
use wasm_bindgen::{JsCast, prelude::*};
use web_sys::{FileSystemReadWriteOptions, FileSystemSyncAccessHandle};

use crate::browser_io::js_error_to_io;

fn options_at(position: u64) -> FileSystemReadWriteOptions {
    let options = FileSystemReadWriteOptions::new();
    options.set_at(position as f64);
    options
}

/// A count returned by `read`/`write` must be an integer within the buffer.
fn checked_count(count: f64, limit: usize, what: &str) -> io::Result<usize> {
    if count < 0.0 || count > limit as f64 || count.fract() != 0.0 {
        return Err(io::Error::other(format!(
            "OPFS {what} returned an invalid byte count"
        )));
    }
    Ok(count as usize)
}

fn size_of(handle: &FileSystemSyncAccessHandle) -> io::Result<u64> {
    let size = handle.get_size().map_err(js_error_to_io)?;
    if size < 0.0 || size.fract() != 0.0 {
        return Err(io::Error::other("OPFS getSize returned an invalid value"));
    }
    Ok(size as u64)
}

fn seek_target(
    position: SeekFrom,
    current: u64,
    end: impl FnOnce() -> io::Result<u64>,
) -> io::Result<u64> {
    match position {
        SeekFrom::Start(offset) => Some(offset),
        SeekFrom::Current(offset) => current.checked_add_signed(offset),
        SeekFrom::End(offset) => end()?.checked_add_signed(offset),
    }
    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid OPFS seek"))
}

pub(crate) struct OpfsWriter {
    handle: FileSystemSyncAccessHandle,
    position: u64,
}

impl OpfsWriter {
    pub(crate) fn new(handle: JsValue) -> Result<Self, JsValue> {
        Ok(Self {
            handle: handle.unchecked_into(),
            position: 0,
        })
    }
}

impl Write for OpfsWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let written = self
            .handle
            .write_with_u8_array_and_options(bytes, &options_at(self.position))
            .map_err(js_error_to_io)?;
        let written = checked_count(written, bytes.len(), "write")?;
        if written < bytes.len() {
            // A short write means quota or disk exhausted, never a transient condition.
            return Err(match storage_full("output", written, bytes.len()) {
                ConvertError::Io(error) => error,
                other => io::Error::other(other.to_string()),
            });
        }
        self.position += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.handle.flush().map_err(js_error_to_io)
    }
}

impl Seek for OpfsWriter {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.position = seek_target(position, self.position, || size_of(&self.handle))?;
        Ok(self.position)
    }
}

pub(crate) struct OpfsReader {
    handle: FileSystemSyncAccessHandle,
    position: u64,
}

impl OpfsReader {
    pub(crate) fn new(handle: JsValue) -> Self {
        Self {
            handle: handle.unchecked_into(),
            position: 0,
        }
    }
}

impl Read for OpfsReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let len = buffer.len();
        let read = self
            .handle
            .read_with_u8_array_and_options(buffer, &options_at(self.position))
            .map_err(js_error_to_io)?;
        let read = checked_count(read, len, "read")?;
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for OpfsReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.position = seek_target(position, self.position, || size_of(&self.handle))?;
        Ok(self.position)
    }
}
