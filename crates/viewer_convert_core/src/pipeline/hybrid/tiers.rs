//! Storage tiers of the hybrid store. They know nothing about spools: a RAM tier holds byte
//! segments by key, a page tier holds zones addressed by byte offset. No read exceeds
//! `SPOOL_IO_BUFFER_BYTES`, so a backend never has to materialise a whole extent.

use std::{
    collections::HashMap,
    io,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use crate::{error::ConvertError, pipeline::spool_store::lock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SegmentKey(pub u32);

pub trait RamTier {
    fn alloc(&self, key: SegmentKey, capacity: u32) -> Result<(), ConvertError>;
    fn write(&self, key: SegmentKey, at: u32, bytes: &[u8]) -> Result<(), ConvertError>;
    fn read(&self, key: SegmentKey, at: u32, out: &mut [u8]) -> Result<(), ConvertError>;
    /// Trims a segment to `capacity` (called when its spool closes).
    fn shrink(&self, key: SegmentKey, capacity: u32) -> Result<(), ConvertError>;
    fn free(&self, key: SegmentKey) -> Result<(), ConvertError>;
}

pub trait PageTier {
    fn zone_count(&self) -> u32;
    /// Returns the bytes written; a value below `bytes.len()` means quota or disk full.
    fn write(&self, zone: u32, at: u64, bytes: &[u8]) -> Result<usize, ConvertError>;
    /// A short read is an error.
    fn read(&self, zone: u32, at: u64, out: &mut [u8]) -> Result<(), ConvertError>;
    /// The zone has no live pages: truncate it to zero.
    fn release(&self, zone: u32) -> Result<(), ConvertError>;
}

pub fn storage_full(what: &str, written: usize, wanted: usize) -> ConvertError {
    ConvertError::Io(io::Error::new(
        io::ErrorKind::StorageFull,
        format!("storage is full writing {what}: wrote {written} of {wanted} bytes"),
    ))
}

fn invalid(message: String) -> ConvertError {
    ConvertError::Io(io::Error::new(io::ErrorKind::InvalidInput, message))
}

fn span(at: u64, len: usize) -> Option<std::ops::Range<usize>> {
    let start = usize::try_from(at).ok()?;
    Some(start..start.checked_add(len)?)
}

/// In-memory `RamTier`; it records the longest read so tests can bound it.
#[derive(Default)]
pub struct VecRamTier {
    segments: Mutex<HashMap<u32, Vec<u8>>>,
    max_read: AtomicUsize,
}

impl VecRamTier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn max_read_len(&self) -> usize {
        self.max_read.load(Ordering::Relaxed)
    }

    pub fn segment_count(&self) -> usize {
        lock(&self.segments).len()
    }
}

impl RamTier for VecRamTier {
    fn alloc(&self, key: SegmentKey, capacity: u32) -> Result<(), ConvertError> {
        let mut segments = lock(&self.segments);
        if segments.contains_key(&key.0) {
            return Err(invalid(format!("segment {} is already allocated", key.0)));
        }
        segments.insert(key.0, vec![0; capacity as usize]);
        Ok(())
    }

    fn write(&self, key: SegmentKey, at: u32, bytes: &[u8]) -> Result<(), ConvertError> {
        let mut segments = lock(&self.segments);
        let segment = segments
            .get_mut(&key.0)
            .ok_or_else(|| invalid(format!("segment {} is not allocated", key.0)))?;
        let range = span(u64::from(at), bytes.len())
            .filter(|range| range.end <= segment.len())
            .ok_or_else(|| invalid(format!("write past the end of segment {}", key.0)))?;
        segment[range].copy_from_slice(bytes);
        Ok(())
    }

    fn read(&self, key: SegmentKey, at: u32, out: &mut [u8]) -> Result<(), ConvertError> {
        self.max_read.fetch_max(out.len(), Ordering::Relaxed);
        let segments = lock(&self.segments);
        let segment = segments
            .get(&key.0)
            .ok_or_else(|| invalid(format!("segment {} is not allocated", key.0)))?;
        let range = span(u64::from(at), out.len())
            .filter(|range| range.end <= segment.len())
            .ok_or_else(|| invalid(format!("read past the end of segment {}", key.0)))?;
        out.copy_from_slice(&segment[range]);
        Ok(())
    }

    fn shrink(&self, key: SegmentKey, capacity: u32) -> Result<(), ConvertError> {
        let mut segments = lock(&self.segments);
        let segment = segments
            .get_mut(&key.0)
            .ok_or_else(|| invalid(format!("segment {} is not allocated", key.0)))?;
        if capacity as usize > segment.len() {
            return Err(invalid(format!("segment {} cannot grow", key.0)));
        }
        segment.truncate(capacity as usize);
        segment.shrink_to_fit();
        Ok(())
    }

    fn free(&self, key: SegmentKey) -> Result<(), ConvertError> {
        lock(&self.segments)
            .remove(&key.0)
            .map(drop)
            .ok_or_else(|| invalid(format!("segment {} is not allocated", key.0)))
    }
}

/// In-memory `PageTier`; `quota` caps the total bytes held across zones.
pub struct VecPageTier {
    zones: Mutex<Vec<Vec<u8>>>,
    quota: Option<u64>,
}

impl VecPageTier {
    pub fn new(zone_count: u32, quota: Option<u64>) -> Self {
        Self {
            zones: Mutex::new(vec![Vec::new(); zone_count as usize]),
            quota,
        }
    }

    /// Bytes currently held by `zone`.
    pub fn zone_len(&self, zone: u32) -> usize {
        lock(&self.zones).get(zone as usize).map_or(0, Vec::len)
    }
}

impl PageTier for VecPageTier {
    fn zone_count(&self) -> u32 {
        lock(&self.zones).len() as u32
    }

    fn write(&self, zone: u32, at: u64, bytes: &[u8]) -> Result<usize, ConvertError> {
        let mut zones = lock(&self.zones);
        let total: u64 = zones.iter().map(|z| z.len() as u64).sum();
        let held = zones
            .get_mut(zone as usize)
            .ok_or_else(|| invalid(format!("zone {zone} does not exist")))?;
        let room = self
            .quota
            .map_or(u64::MAX, |quota| quota.saturating_sub(total));
        let start = usize::try_from(at).map_err(|_| invalid("offset is too large".into()))?;
        let growth = (start + bytes.len()).saturating_sub(held.len());
        let allowed_end = if growth as u64 > room {
            held.len() + room as usize
        } else {
            start + bytes.len()
        };
        let written = allowed_end.saturating_sub(start).min(bytes.len());
        if written == 0 {
            return Ok(0);
        }
        if held.len() < start + written {
            held.resize(start + written, 0);
        }
        held[start..start + written].copy_from_slice(&bytes[..written]);
        Ok(written)
    }

    fn read(&self, zone: u32, at: u64, out: &mut [u8]) -> Result<(), ConvertError> {
        let zones = lock(&self.zones);
        let held = zones
            .get(zone as usize)
            .ok_or_else(|| invalid(format!("zone {zone} does not exist")))?;
        let range = span(at, out.len())
            .filter(|range| range.end <= held.len())
            .ok_or_else(|| invalid(format!("short read from zone {zone}")))?;
        out.copy_from_slice(&held[range]);
        Ok(())
    }

    fn release(&self, zone: u32) -> Result<(), ConvertError> {
        let mut zones = lock(&self.zones);
        let held = zones
            .get_mut(zone as usize)
            .ok_or_else(|| invalid(format!("zone {zone} does not exist")))?;
        *held = Vec::new();
        Ok(())
    }
}
