//! Cached synchronous Blob reads for dedicated browser workers.
use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::{Arc, Mutex},
};
use wasm_bindgen::{JsCast, prelude::*};
use web_sys::{Blob, FileReaderSync};

use crate::lod_dataset::WorkClock;

const FILE_CACHE_BYTES: u64 = 256 * 1024;

/// Aggregate fills of the fixed-size Blob cache, including repeated and failed attempts.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct BrowserReadStats {
    pub(crate) calls: u64,
    pub(crate) bytes: u64,
    pub(crate) elapsed_ms: f64,
}

pub(crate) struct BrowserBlobReader {
    blob: Blob,
    file_reader: FileReaderSync,
    file_size: u64,
    position: u64,
    cached_start: u64,
    cache: Vec<u8>,
    report_progress: bool,
    loaded_chunks: Vec<u64>,
    loaded_unique_bytes: u64,
    last_progress_at_ms: f64,
    read_stats: Option<Arc<Mutex<BrowserReadStats>>>,
}

impl BrowserBlobReader {
    pub(crate) fn new(blob: Blob, report_progress: bool) -> Result<Self, JsValue> {
        Self::build(blob, report_progress, None)
    }

    pub(crate) fn new_with_stats(
        blob: Blob,
        report_progress: bool,
        read_stats: Arc<Mutex<BrowserReadStats>>,
    ) -> Result<Self, JsValue> {
        Self::build(blob, report_progress, Some(read_stats))
    }

    fn build(
        blob: Blob,
        report_progress: bool,
        read_stats: Option<Arc<Mutex<BrowserReadStats>>>,
    ) -> Result<Self, JsValue> {
        let size = blob.size();
        if !size.is_finite() || size < 0.0 || size > u64::MAX as f64 {
            return Err(JsValue::from_str(
                "file size is outside the supported range",
            ));
        }

        let loaded_chunks = if report_progress {
            let chunk_count = (size as u64).div_ceil(FILE_CACHE_BYTES);
            let word_count = usize::try_from(chunk_count.div_ceil(u64::from(u64::BITS)))
                .map_err(|_| JsValue::from_str("file progress index is too large"))?;
            let mut chunks = Vec::new();
            chunks
                .try_reserve_exact(word_count)
                .map_err(|_| JsValue::from_str("unable to allocate file progress index"))?;
            chunks.resize(word_count, 0);
            chunks
        } else {
            Vec::new()
        };

        Ok(Self {
            blob,
            file_reader: FileReaderSync::new()?,
            file_size: size as u64,
            position: 0,
            cached_start: u64::MAX,
            cache: Vec::new(),
            report_progress,
            loaded_chunks,
            loaded_unique_bytes: 0,
            last_progress_at_ms: 0.0,
            read_stats,
        })
    }

    fn load_cache(&mut self) -> io::Result<()> {
        let start = self.position / FILE_CACHE_BYTES * FILE_CACHE_BYTES;
        let end = start.saturating_add(FILE_CACHE_BYTES).min(self.file_size);
        let started = self.read_stats.as_ref().map(|_| WorkClock::start());
        let loaded: io::Result<()> = (|| {
            let slice = self
                .blob
                .slice_with_f64_and_f64(start as f64, end as f64)
                .map_err(js_error_to_io)?;
            let array_buffer = self
                .file_reader
                .read_as_array_buffer(&slice)
                .map_err(js_error_to_io)?;
            let array = js_sys::Uint8Array::new(&array_buffer);
            self.cache.resize((end - start) as usize, 0);
            array.copy_to(&mut self.cache);
            Ok(())
        })();
        if let (Some(read_stats), Some(started)) = (&self.read_stats, started) {
            let mut stats = read_stats
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            stats.calls = stats.calls.saturating_add(1);
            stats.elapsed_ms += started.elapsed_ms();
            if loaded.is_ok() {
                stats.bytes = stats.bytes.saturating_add(end - start);
            }
        }
        loaded?;
        self.cached_start = start;
        if self.report_progress {
            let chunk_index = usize::try_from(start / FILE_CACHE_BYTES)
                .map_err(|_| io::Error::other("file progress index is too large"))?;
            let word = chunk_index / u64::BITS as usize;
            let mask = 1_u64 << (chunk_index % u64::BITS as usize);
            let seen = self.loaded_chunks[word] & mask != 0;
            if !seen {
                self.loaded_chunks[word] |= mask;
                self.loaded_unique_bytes += end - start;
            }
            let now = js_sys::Date::now();
            if seen || now - self.last_progress_at_ms < 100.0 {
                return Ok(());
            }
            self.last_progress_at_ms = now;
            let progress = js_sys::Object::new();
            let _ = js_sys::Reflect::set(
                &progress,
                &"type".into(),
                &JsValue::from_str("fileReadProgress"),
            );
            let _ = js_sys::Reflect::set(
                &progress,
                &"uniqueBytes".into(),
                &JsValue::from_str(&self.loaded_unique_bytes.to_string()),
            );
            let _ = js_sys::Reflect::set(
                &progress,
                &"fileBytes".into(),
                &JsValue::from_str(&self.file_size.to_string()),
            );
            let scope: web_sys::DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
            let _ = scope.post_message(&progress);
        }
        Ok(())
    }
}

impl Read for BrowserBlobReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.position >= self.file_size {
            return Ok(0);
        }
        if self.cached_start == u64::MAX
            || self.position < self.cached_start
            || self.position >= self.cached_start + self.cache.len() as u64
        {
            self.load_cache()?;
        }

        let cache_offset = (self.position - self.cached_start) as usize;
        let count = output
            .len()
            .min(self.cache.len().saturating_sub(cache_offset));
        output[..count].copy_from_slice(&self.cache[cache_offset..cache_offset + count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for BrowserBlobReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.file_size) + i128::from(offset),
        };
        if !(0..=i128::from(u64::MAX)).contains(&next) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek position is outside the supported range",
            ));
        }
        self.position = next as u64;
        Ok(self.position)
    }
}

/// The text of a thrown JS value: a string as is, otherwise `Error.message`.
pub(crate) fn js_message(error: &JsValue) -> Option<String> {
    error.as_string().or_else(|| {
        js_sys::Reflect::get(error, &"message".into())
            .ok()
            .and_then(|message| message.as_string())
    })
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a `map_err` function path, which hands the error over by value"
)]
pub(crate) fn js_error_to_io(error: JsValue) -> io::Error {
    io::Error::other(js_message(&error).unwrap_or_else(|| "browser file read failed".to_owned()))
}
