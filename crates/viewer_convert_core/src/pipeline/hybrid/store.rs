//! `HybridSpoolStore`: the `SpoolStore` contract over a RAM tier and a page tier.
//!
//! A spool is a list of RAM segments that doubles from `SEGMENT_MIN_BYTES` to
//! `SEGMENT_MAX_BYTES`; closing trims the last one. The RAM budget compares the sum of segment
//! capacities, not payload. Readers pin their spool (`delete` fails while one is alive) and copy
//! at most one I/O buffer per `read`, so no tier ever materialises a whole extent.
//!
//! When the retained RAM exceeds the budget, whole spools are spilled to pages (see
//! `Inner::spill_over_budget`). A spilled spool that is still open keeps a RAM tail of exactly
//! one page; every full tail is written as the next page and the partial one on close. The
//! budget is soft: with no victim (all pinned or already spilled) the store carries on over it.

use std::{
    collections::HashMap,
    io::{self, Read},
    sync::{Arc, Mutex},
};

use super::{
    pages::PageAllocator,
    tiers::{PageTier, RamTier, SegmentKey, storage_full},
};
use crate::{
    error::ConvertError,
    pipeline::spool_store::{
        LedgerTotals, Phase, SPOOL_IO_BUFFER_BYTES, SpoolAccounting, SpoolExtent, SpoolId,
        SpoolStore, entry_in, lifecycle_error, lock,
    },
};

pub const HYBRID_PAGE_BYTES: u32 = 256 * 1024;
pub const HYBRID_ZONE_BYTES: u64 = 1 << 30;
pub const SEGMENT_MIN_BYTES: u32 = 32 * 1024;
pub const SEGMENT_MAX_BYTES: u32 = 1 << 20;

#[derive(Clone, Copy, Debug)]
pub struct HybridConfig {
    pub ram_budget_bytes: u64,
    pub page_bytes: u32,
    pub zone_bytes: u64,
}

impl HybridConfig {
    pub const fn web(ram_budget_bytes: u64) -> Self {
        Self {
            ram_budget_bytes,
            page_bytes: HYBRID_PAGE_BYTES,
            zone_bytes: HYBRID_ZONE_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HybridStats {
    pub live_payload_bytes: u64,
    pub ram_retained_bytes: u64,
    pub ram_peak_bytes: u64,
    pub over_budget_peak_bytes: u64,
    pub spilled_bytes: u64,
    pub spilled_spools: u64,
    pub page_read_bytes: u64,
    pub pages_in_use: u32,
    pub pages_peak: u32,
    pub zones_released: u32,
}

struct Segment {
    key: SegmentKey,
    capacity: u32,
    used: u32,
    /// Offset of the segment's first byte inside the spool.
    start: u64,
}

struct Spool {
    phase: Phase,
    /// RAM segments; for a spilled spool only its tail (a single page-sized segment).
    segments: Vec<Segment>,
    len: u64,
    pins: u32,
    spilled: bool,
    /// Pages holding the first `paged_len` bytes (all full but the last one of a closed spool).
    pages: Vec<u32>,
    paged_len: u64,
    retained_hint: bool,
    rank_hint: Option<u32>,
    closed_seq: u64,
}

impl Spool {
    fn new() -> Self {
        Self {
            phase: Phase::Open,
            segments: Vec::new(),
            len: 0,
            pins: 0,
            spilled: false,
            pages: Vec::new(),
            paged_len: 0,
            retained_hint: false,
            rank_hint: None,
            closed_seq: 0,
        }
    }
}

struct State {
    spools: HashMap<SpoolId, Spool>,
    pages: PageAllocator,
    page_bytes: u32,
    next_key: u32,
    free_keys: Vec<u32>,
    budget: u64,
    written: u64,
    live_payload: u64,
    peak_payload: u64,
    ram_retained: u64,
    ram_peak: u64,
    over_budget_peak: u64,
    event_seq: u64,
    closed_seq: u64,
    spilled_bytes: u64,
    spilled_spools: u64,
    page_read_bytes: u64,
    /// Whether `SpoolAccounting` reports (the planner's `collect_diagnostics`).
    accounting: bool,
}

impl State {
    fn retain(&mut self, capacity: u32) {
        self.ram_retained += u64::from(capacity);
        self.ram_peak = self.ram_peak.max(self.ram_retained);
        self.over_budget_peak = self
            .over_budget_peak
            .max(self.ram_retained.saturating_sub(self.budget));
    }

    fn take_key(&mut self) -> Result<SegmentKey, ConvertError> {
        if let Some(key) = self.free_keys.pop() {
            return Ok(SegmentKey(key));
        }
        let key = self.next_key;
        self.next_key = key.checked_add(1).ok_or_else(|| {
            ConvertError::Io(io::Error::other("spool segment keys are exhausted"))
        })?;
        Ok(SegmentKey(key))
    }

    fn count_written(&mut self, n: usize) {
        self.written += n as u64;
        self.live_payload += n as u64;
        self.peak_payload = self.peak_payload.max(self.live_payload);
    }

    /// The next spool to spill: closed `retained` spools (largest first), closed spools with a
    /// consume rank (highest first), closed spools with no hint (oldest first), then open spools
    /// of at least one page (largest first). Pinned and spilled spools never qualify.
    fn pick_victim(&self) -> Option<SpoolId> {
        let page = u64::from(self.page_bytes);
        self.spools
            .iter()
            .filter(|(_, s)| s.pins == 0 && !s.spilled && s.len > 0 && !s.segments.is_empty())
            .filter_map(|(id, s)| {
                let key = match s.phase {
                    Phase::Closed if s.retained_hint => (0, u64::MAX - s.len),
                    Phase::Closed if s.rank_hint.is_some() => {
                        (1, u64::MAX - u64::from(s.rank_hint.unwrap_or(0)))
                    }
                    Phase::Closed => (2, s.closed_seq),
                    Phase::Open if s.len >= page => (3, u64::MAX - s.len),
                    _ => return None,
                };
                Some((key, *id))
            })
            .min()
            .map(|(_, id)| id)
    }
}

/// Copies `out.len()` bytes of the spool at `offset` out of its RAM segments.
fn read_ram<R: RamTier>(
    ram: &R,
    spool: &Spool,
    mut offset: u64,
    mut out: &mut [u8],
) -> Result<(), ConvertError> {
    while !out.is_empty() {
        let index = spool
            .segments
            .partition_point(|s| s.start + u64::from(s.used) <= offset);
        let segment = spool.segments.get(index).ok_or_else(|| {
            ConvertError::Io(io::Error::other("spool segments do not cover the range"))
        })?;
        let at = (offset - segment.start) as u32;
        let n = out.len().min((segment.used - at) as usize);
        ram.read(segment.key, at, &mut out[..n])?;
        offset += n as u64;
        out = &mut out[n..];
    }
    Ok(())
}

struct Inner<R, P> {
    state: Mutex<State>,
    ram: R,
    pages: P,
    /// One page of scratch: RAM segments are copied into it in I/O-buffer chunks, then written.
    staging: Mutex<Vec<u8>>,
}

pub struct HybridSpoolStore<R, P> {
    inner: Arc<Inner<R, P>>,
}

impl<R: RamTier + Send + Sync, P: PageTier + Send + Sync> HybridSpoolStore<R, P> {
    pub fn new(
        ram: R,
        pages: P,
        config: HybridConfig,
        diagnostics: bool,
    ) -> Result<Self, ConvertError> {
        let allocator =
            PageAllocator::new(pages.zone_count(), config.zone_bytes, config.page_bytes)?;
        Ok(Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    spools: HashMap::new(),
                    pages: allocator,
                    page_bytes: config.page_bytes,
                    next_key: 0,
                    free_keys: Vec::new(),
                    budget: config.ram_budget_bytes,
                    written: 0,
                    live_payload: 0,
                    peak_payload: 0,
                    ram_retained: 0,
                    ram_peak: 0,
                    over_budget_peak: 0,
                    event_seq: 0,
                    closed_seq: 0,
                    spilled_bytes: 0,
                    spilled_spools: 0,
                    page_read_bytes: 0,
                    accounting: diagnostics,
                }),
                ram,
                pages,
                staging: Mutex::new(vec![0; config.page_bytes as usize]),
            }),
        })
    }

    pub fn stats(&self) -> HybridStats {
        let state = lock(&self.inner.state);
        HybridStats {
            live_payload_bytes: state.live_payload,
            ram_retained_bytes: state.ram_retained,
            ram_peak_bytes: state.ram_peak,
            over_budget_peak_bytes: state.over_budget_peak,
            pages_in_use: state.pages.in_use(),
            pages_peak: state.pages.peak(),
            zones_released: state.pages.zones_released(),
            spilled_bytes: state.spilled_bytes,
            spilled_spools: state.spilled_spools,
            page_read_bytes: state.page_read_bytes,
        }
    }

    /// Spools created and not yet deleted.
    pub fn live_ids(&self) -> usize {
        let state = lock(&self.inner.state);
        state
            .spools
            .values()
            .filter(|spool| spool.phase != Phase::Deleted)
            .count()
    }

    /// RAM segments currently held by every spool.
    pub fn segment_count(&self) -> usize {
        let state = lock(&self.inner.state);
        state
            .spools
            .values()
            .map(|spool| spool.segments.len())
            .sum()
    }

    /// Capacities of the RAM segments of `id`, in order.
    pub fn segment_capacities(&self, id: SpoolId) -> Vec<u32> {
        let state = lock(&self.inner.state);
        state
            .spools
            .get(&id)
            .map_or_default(|spool| spool.segments.iter().map(|s| s.capacity).collect())
    }
}

fn alloc_error(capacity: u32, error: &ConvertError) -> ConvertError {
    ConvertError::Io(io::Error::new(
        io::ErrorKind::OutOfMemory,
        format!("could not allocate a {capacity}-byte spool segment: {error}"),
    ))
}

impl<R: RamTier, P: PageTier> Inner<R, P> {
    /// Spills victims until the retained RAM fits the budget or none is left. Without zones
    /// (RAM-only hosts such as Chrome `file://`) nothing spills and the budget stays soft.
    fn spill_over_budget(&self, state: &mut State) -> Result<(), ConvertError> {
        if self.pages.zone_count() == 0 {
            return Ok(());
        }
        while state.ram_retained > state.budget {
            let Some(id) = state.pick_victim() else {
                break;
            };
            self.spill_spool(state, id)?;
        }
        Ok(())
    }

    /// Frees `page`; releases its zone when that was the last live page.
    fn free_page(&self, allocator: &mut PageAllocator, page: u32) -> Result<(), ConvertError> {
        allocator
            .free(page)
            .map_or(Ok(()), |zone| self.pages.release(zone))
    }

    /// Copies `n` bytes of the spool at `offset` through `staging` into a freshly allocated page.
    fn write_page(
        &self,
        spools: &HashMap<SpoolId, Spool>,
        allocator: &mut PageAllocator,
        id: SpoolId,
        offset: u64,
        staging: &mut [u8],
        n: usize,
    ) -> Result<u32, ConvertError> {
        let spool = &spools[&id];
        for (i, chunk) in staging[..n].chunks_mut(SPOOL_IO_BUFFER_BYTES).enumerate() {
            let at = offset + (i * SPOOL_IO_BUFFER_BYTES) as u64;
            read_ram(&self.ram, spool, at, chunk)?;
        }
        let page = allocator.alloc()?;
        let (zone, at) = allocator.locate(page);
        match self.pages.write(zone, at, &staging[..n]) {
            Ok(written) if written == n => Ok(page),
            Ok(written) => {
                let _ = self.free_page(allocator, page);
                Err(storage_full(&format!("spill zone {zone}"), written, n))
            }
            Err(error) => {
                let _ = self.free_page(allocator, page);
                Err(error)
            }
        }
    }

    /// Writes the first `n` bytes of the RAM tail of a spilled spool as its next page.
    fn push_tail_page(&self, state: &mut State, id: SpoolId, n: usize) -> Result<(), ConvertError> {
        let mut staging = lock(&self.staging);
        let offset = state.spools[&id].paged_len;
        let page = self.write_page(&state.spools, &mut state.pages, id, offset, &mut staging, n)?;
        let spool = state.spools.get_mut(&id).expect("spool exists");
        spool.pages.push(page);
        spool.paged_len += n as u64;
        state.spilled_bytes += n as u64;
        Ok(())
    }

    /// Allocates a page-sized tail holding the last `remainder` bytes of `id`.
    fn make_tail(
        &self,
        state: &mut State,
        id: SpoolId,
        start: u64,
        remainder: u64,
        staging: &mut [u8],
    ) -> Result<Segment, ConvertError> {
        let capacity = state.page_bytes;
        let key = state.take_key()?;
        if let Err(error) = self.ram.alloc(key, capacity) {
            state.free_keys.push(key.0);
            return Err(alloc_error(capacity, &error));
        }
        state.retain(capacity);
        let spool = &state.spools[&id];
        let mut copied = 0;
        let result = (|| {
            while copied < remainder {
                let n = (remainder - copied).min(SPOOL_IO_BUFFER_BYTES as u64) as usize;
                read_ram(&self.ram, spool, start + copied, &mut staging[..n])?;
                self.ram.write(key, copied as u32, &staging[..n])?;
                copied += n as u64;
            }
            Ok(())
        })();
        if let Err(error) = result {
            state.ram_retained -= u64::from(capacity);
            state.free_keys.push(key.0);
            let _ = self.ram.free(key);
            return Err(error);
        }
        Ok(Segment {
            key,
            capacity,
            used: remainder as u32,
            start,
        })
    }

    /// Moves a whole spool to pages. An open spool keeps its sub-page remainder in a RAM tail.
    fn spill_spool(&self, state: &mut State, id: SpoolId) -> Result<(), ConvertError> {
        let mut staging = lock(&self.staging);
        let page_bytes = u64::from(state.page_bytes);
        let spool = &state.spools[&id];
        let (len, open) = (spool.len, spool.phase == Phase::Open);
        let page_count = if open {
            len / page_bytes
        } else {
            len.div_ceil(page_bytes)
        };
        let paged_len = if open { page_count * page_bytes } else { len };
        let mut pages = Vec::new();
        let mut result = Ok(());
        for i in 0..page_count {
            let offset = i * page_bytes;
            let n = (len - offset).min(page_bytes) as usize;
            match self.write_page(&state.spools, &mut state.pages, id, offset, &mut staging, n) {
                Ok(page) => pages.push(page),
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        let mut tail = None;
        if result.is_ok() && len > paged_len {
            match self.make_tail(state, id, paged_len, len - paged_len, &mut staging) {
                Ok(segment) => tail = Some(segment),
                Err(error) => result = Err(error),
            }
        }
        if let Err(error) = result {
            for page in pages {
                let _ = self.free_page(&mut state.pages, page);
            }
            return Err(error);
        }
        let spool = state.spools.get_mut(&id).expect("spool exists");
        let old = std::mem::take(&mut spool.segments);
        spool.segments.extend(tail);
        spool.pages = pages;
        spool.paged_len = paged_len;
        spool.spilled = true;
        state.spilled_bytes += paged_len;
        state.spilled_spools += 1;
        self.free_segments(state, old)
    }

    /// Frees RAM segments already detached from their spool; reports the first failure.
    fn free_segments(&self, state: &mut State, segments: Vec<Segment>) -> Result<(), ConvertError> {
        let mut first_error = None;
        for segment in segments {
            state.ram_retained -= u64::from(segment.capacity);
            state.free_keys.push(segment.key.0);
            if let Err(error) = self.ram.free(segment.key) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Appends to an open spool that is already spilled: bytes go to the one-page RAM tail.
    fn append_tail(
        &self,
        state: &mut State,
        id: SpoolId,
        rest: &[u8],
    ) -> Result<usize, ConvertError> {
        let capacity = state.page_bytes;
        if state.spools[&id].segments.is_empty() {
            let start = state.spools[&id].paged_len;
            let key = state.take_key()?;
            if let Err(error) = self.ram.alloc(key, capacity) {
                state.free_keys.push(key.0);
                return Err(alloc_error(capacity, &error));
            }
            state.retain(capacity);
            state
                .spools
                .get_mut(&id)
                .expect("spool exists")
                .segments
                .push(Segment {
                    key,
                    capacity,
                    used: 0,
                    start,
                });
            self.spill_over_budget(state)?;
        }
        let spool = state.spools.get_mut(&id).expect("spool exists");
        let tail = spool
            .segments
            .last_mut()
            .expect("the tail was just ensured");
        let n = ((tail.capacity - tail.used) as usize).min(rest.len());
        self.ram.write(tail.key, tail.used, &rest[..n])?;
        tail.used += n as u32;
        spool.len += n as u64;
        let full = tail.used == tail.capacity;
        state.count_written(n);
        if full {
            self.push_tail_page(state, id, capacity as usize)?;
            let spool = state.spools.get_mut(&id).expect("spool exists");
            let tail = spool.segments.last_mut().expect("the tail exists");
            tail.used = 0;
            tail.start = spool.paged_len;
        }
        Ok(n)
    }

    /// Closes a spilled spool: the partial tail becomes the last page and the tail is freed.
    fn close_spilled(&self, state: &mut State, id: SpoolId) -> Result<(), ConvertError> {
        let used = state.spools[&id].segments.last().map_or(0, |s| s.used);
        if used > 0 {
            self.push_tail_page(state, id, used as usize)?;
        }
        let tail = std::mem::take(&mut state.spools.get_mut(&id).expect("spool exists").segments);
        self.free_segments(state, tail)
    }
}

impl<R: RamTier + Send + Sync, P: PageTier + Send + Sync> SpoolStore for HybridSpoolStore<R, P> {
    type Reader = HybridReader<R, P>;

    fn create(&self, id: SpoolId, _record_bytes: usize) -> Result<(), ConvertError> {
        let mut state = lock(&self.inner.state);
        if state.spools.contains_key(&id) {
            return Err(lifecycle_error(id, "create", "id was already used"));
        }
        state.spools.insert(id, Spool::new());
        Ok(())
    }

    fn append(&self, id: SpoolId, bytes: &[u8]) -> Result<(), ConvertError> {
        let mut guard = lock(&self.inner.state);
        let state = &mut *guard;
        entry_in(&mut state.spools, id, "append", Phase::Open, |s| s.phase)?;
        let mut rest = bytes;
        while !rest.is_empty() {
            let spool = &state.spools[&id];
            if spool.spilled {
                let n = self.inner.append_tail(state, id, rest)?;
                rest = &rest[n..];
                continue;
            }
            if spool.segments.last().is_none_or(|s| s.used == s.capacity) {
                let capacity = spool.segments.last().map_or(SEGMENT_MIN_BYTES, |s| {
                    s.capacity.saturating_mul(2).min(SEGMENT_MAX_BYTES)
                });
                let start = spool.len;
                let key = state.take_key()?;
                if let Err(error) = self.inner.ram.alloc(key, capacity) {
                    state.free_keys.push(key.0);
                    return Err(alloc_error(capacity, &error));
                }
                state.retain(capacity);
                state
                    .spools
                    .get_mut(&id)
                    .expect("spool exists")
                    .segments
                    .push(Segment {
                        key,
                        capacity,
                        used: 0,
                        start,
                    });
                // The spool itself may be the victim, so re-read its state.
                self.inner.spill_over_budget(state)?;
                continue;
            }
            let spool = state.spools.get_mut(&id).expect("spool exists");
            let segment = spool.segments.last_mut().expect("a segment exists");
            let n = ((segment.capacity - segment.used) as usize).min(rest.len());
            self.inner
                .ram
                .write(segment.key, segment.used, &rest[..n])?;
            segment.used += n as u32;
            spool.len += n as u64;
            state.count_written(n);
            rest = &rest[n..];
        }
        Ok(())
    }

    fn close(&self, id: SpoolId) -> Result<(), ConvertError> {
        let mut guard = lock(&self.inner.state);
        let state = &mut *guard;
        let spool = entry_in(&mut state.spools, id, "close", Phase::Open, |s| s.phase)?;
        spool.phase = Phase::Closed;
        state.closed_seq += 1;
        spool.closed_seq = state.closed_seq;
        if spool.spilled {
            return self.inner.close_spilled(state, id);
        }
        if let Some(segment) = spool.segments.last_mut() {
            let trimmed = segment.used.div_ceil(SEGMENT_MIN_BYTES) * SEGMENT_MIN_BYTES;
            if trimmed < segment.capacity {
                self.inner.ram.shrink(segment.key, trimmed)?;
                state.ram_retained -= u64::from(segment.capacity - trimmed);
                segment.capacity = trimmed;
            }
        }
        // The closed spool is now a candidate, which matters when it was the only one.
        self.inner.spill_over_budget(state)
    }

    fn len(&self, id: SpoolId) -> Result<u64, ConvertError> {
        let mut state = lock(&self.inner.state);
        Ok(entry_in(&mut state.spools, id, "len", Phase::Closed, |s| s.phase)?.len)
    }

    fn reader(&self, id: SpoolId, extent: SpoolExtent) -> Result<Self::Reader, ConvertError> {
        let mut state = lock(&self.inner.state);
        let spool = entry_in(&mut state.spools, id, "read", Phase::Closed, |s| s.phase)?;
        let end = extent
            .offset
            .checked_add(extent.len)
            .filter(|end| *end <= spool.len)
            .ok_or_else(|| lifecycle_error(id, "read", "extent is out of range"))?;
        spool.pins += 1;
        Ok(HybridReader {
            inner: Arc::clone(&self.inner),
            id,
            position: extent.offset,
            end,
        })
    }

    fn delete(&self, id: SpoolId) -> Result<(), ConvertError> {
        let mut guard = lock(&self.inner.state);
        let state = &mut *guard;
        let spool = entry_in(&mut state.spools, id, "delete", Phase::Closed, |s| s.phase)?;
        if spool.pins > 0 {
            return Err(lifecycle_error(id, "delete", "spool has a live reader"));
        }
        spool.phase = Phase::Deleted;
        let segments = std::mem::take(&mut spool.segments);
        let pages = std::mem::take(&mut spool.pages);
        let len = std::mem::take(&mut spool.len);
        state.live_payload -= len;
        let mut first_error = None;
        for page in pages {
            if let Err(error) = self.inner.free_page(&mut state.pages, page) {
                first_error.get_or_insert(error);
            }
        }
        if let Err(error) = self.inner.free_segments(state, segments) {
            first_error.get_or_insert(error);
        }
        first_error.map_or(Ok(()), Err)
    }

    fn hint_retained(&self, id: SpoolId) {
        if let Some(spool) = lock(&self.inner.state).spools.get_mut(&id) {
            spool.retained_hint = true;
        }
    }

    fn hint_consume_rank(&self, id: SpoolId, rank: u32) {
        if let Some(spool) = lock(&self.inner.state).spools.get_mut(&id) {
            spool.rank_hint = Some(rank);
        }
    }
}

impl<R, P> SpoolAccounting for HybridSpoolStore<R, P> {
    fn next_event_seq(&self) -> Option<u64> {
        let mut state = lock(&self.inner.state);
        let accounting = state.accounting;
        let seq = state.event_seq;
        state.event_seq += 1;
        accounting.then_some(seq)
    }

    fn ledger_totals(&self) -> Option<LedgerTotals> {
        let state = lock(&self.inner.state);
        state.accounting.then_some(LedgerTotals {
            written: state.written,
            peak_live: state.peak_payload,
            physical_written: state.written,
            physical_peak_live: state.peak_payload,
        })
    }
}

/// Reads an extent of a closed spool. It pins the spool until dropped.
pub struct HybridReader<R, P> {
    inner: Arc<Inner<R, P>>,
    id: SpoolId,
    position: u64,
    end: u64,
}

impl<R: RamTier, P: PageTier> Read for HybridReader<R, P> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.end - self.position;
        if buf.is_empty() || remaining == 0 {
            return Ok(0);
        }
        let mut guard = lock(&self.inner.state);
        let state = &mut *guard;
        let spool = state
            .spools
            .get(&self.id)
            .ok_or_else(|| io::Error::other("pinned spool vanished"))?;
        let limit = buf
            .len()
            .min(SPOOL_IO_BUFFER_BYTES)
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let into_io = |error: ConvertError| match error {
            ConvertError::Io(error) => error,
            other => io::Error::other(other.to_string()),
        };
        if self.position < spool.paged_len {
            let page_bytes = u64::from(state.page_bytes);
            let index = self.position / page_bytes;
            let page_end = ((index + 1) * page_bytes).min(spool.paged_len);
            let n = limit.min(usize::try_from(page_end - self.position).unwrap_or(usize::MAX));
            let page = *spool
                .pages
                .get(index as usize)
                .ok_or_else(|| io::Error::other("spool pages do not cover the extent"))?;
            let (zone, base) = state.pages.locate(page);
            self.inner
                .pages
                .read(zone, base + self.position % page_bytes, &mut buf[..n])
                .map_err(into_io)?;
            state.page_read_bytes += n as u64;
            self.position += n as u64;
            return Ok(n);
        }
        let index = spool
            .segments
            .partition_point(|s| s.start + u64::from(s.used) <= self.position);
        let segment = spool
            .segments
            .get(index)
            .ok_or_else(|| io::Error::other("spool segments do not cover the extent"))?;
        let at = (self.position - segment.start) as u32;
        let n = limit.min((segment.used - at) as usize);
        self.inner
            .ram
            .read(segment.key, at, &mut buf[..n])
            .map_err(into_io)?;
        self.position += n as u64;
        Ok(n)
    }
}

impl<R, P> Drop for HybridReader<R, P> {
    fn drop(&mut self) {
        let mut state = lock(&self.inner.state);
        if let Some(spool) = state.spools.get_mut(&self.id) {
            spool.pins -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{
        hybrid::tiers::{VecPageTier, VecRamTier},
        spool_store::MemSpoolStore,
    };

    type Store = HybridSpoolStore<VecRamTier, VecPageTier>;

    fn store() -> Store {
        let config = HybridConfig::web(u64::MAX);
        HybridSpoolStore::new(VecRamTier::new(), VecPageTier::new(2, None), config, true).unwrap()
    }

    /// xorshift64: a seeded sequence with no dependency.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self, bound: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % bound
        }
    }

    fn read_all(reader: &mut impl Read) -> Vec<u8> {
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        out
    }

    fn outcome<T>(result: Result<T, ConvertError>) -> Result<(), String> {
        result.map(drop).map_err(|error| error.to_string())
    }

    fn config(budget: u64, page_bytes: u32, zone_bytes: u64) -> HybridConfig {
        HybridConfig {
            ram_budget_bytes: budget,
            page_bytes,
            zone_bytes,
        }
    }

    fn spilling_store(budget: u64, zones: u32, quota: Option<u64>, page: u32, zone: u64) -> Store {
        HybridSpoolStore::new(
            VecRamTier::new(),
            VecPageTier::new(zones, quota),
            config(budget, page, zone),
            true,
        )
        .unwrap()
    }

    fn is_spilled(store: &Store, id: u64) -> bool {
        lock(&store.inner.state).spools[&SpoolId(id)].spilled
    }

    #[test]
    fn matches_mem_store_on_a_random_operation_sequence() {
        random_sequence(&store(), 0x9E37_79B9_7F4A_7C15);
    }

    #[test]
    fn without_zones_nothing_spills_and_the_budget_stays_soft() {
        let store = spilling_store(0, 0, None, HYBRID_PAGE_BYTES, HYBRID_ZONE_BYTES);
        random_sequence(&store, 0x2545_F491_4F6C_DD1D);
        let stats = store.stats();
        assert_eq!(stats.spilled_spools, 0, "{stats:?}");
        assert!(stats.over_budget_peak_bytes > 0, "{stats:?}");
    }

    #[test]
    fn spilling_matches_mem_store_on_random_operation_sequences() {
        for (budget, seed) in [
            (64 * 1024, 0xDEAD_BEEF_1234_5678),
            (0, 0x0123_4567_89AB_CDEF),
        ] {
            // Small zones exercise page lists, tails and zone release.
            let hybrid = spilling_store(budget, 2048, None, 64 * 1024, 1 << 20);
            random_sequence(&hybrid, seed);
            let stats = hybrid.stats();
            assert!(
                stats.spilled_spools > 5 && stats.page_read_bytes > 0,
                "{stats:?}"
            );
            assert!(stats.zones_released > 0, "{stats:?}");
            assert!(hybrid.inner.ram.max_read_len() <= SPOOL_IO_BUFFER_BYTES);
            assert!(hybrid.inner.pages.max_read_len() <= SPOOL_IO_BUFFER_BYTES);
        }
    }

    fn random_sequence(hybrid: &Store, seed: u64) {
        let mem = MemSpoolStore::new(true);
        let mut rng = Rng(seed);
        let mut next_id = 1u64;
        let mut data = Vec::new();
        let mut compared_reads = 0;
        for _ in 0..4000 {
            // Mostly existing ids, sometimes an unknown one, to compare errors too.
            let id = SpoolId(1 + rng.next(next_id + 1));
            match rng.next(8) {
                0 => {
                    let id = SpoolId(next_id);
                    next_id += 1;
                    assert_eq!(outcome(hybrid.create(id, 1)), outcome(mem.create(id, 1)));
                }
                1 | 2 => {
                    let size = match rng.next(6) {
                        0 => 0,
                        1 => 1_500_000 + rng.next(1_000_000) as usize,
                        2 => 30_000 + rng.next(10_000) as usize,
                        _ => rng.next(5_000) as usize,
                    };
                    data.clear();
                    data.extend((0..size).map(|_| rng.next(256) as u8));
                    assert_eq!(
                        outcome(hybrid.append(id, &data)),
                        outcome(mem.append(id, &data))
                    );
                }
                3 => assert_eq!(outcome(hybrid.close(id)), outcome(mem.close(id))),
                4 => {
                    let (a, b) = (hybrid.len(id), mem.len(id));
                    assert_eq!(a.as_ref().ok(), b.as_ref().ok());
                    assert_eq!(outcome(a), outcome(b));
                }
                5 | 6 => {
                    let total = mem.len(id).unwrap_or(0);
                    let offset = rng.next(total + 2);
                    let extent = SpoolExtent {
                        offset,
                        len: rng.next(total + 2),
                    };
                    match (hybrid.reader(id, extent), mem.reader(id, extent)) {
                        (Ok(mut a), Ok(mut b)) => {
                            compared_reads += 1;
                            assert_eq!(read_all(&mut a), read_all(&mut b));
                        }
                        (a, b) => assert_eq!(outcome(a), outcome(b)),
                    }
                }
                _ => assert_eq!(outcome(hybrid.delete(id)), outcome(mem.delete(id))),
            }
        }
        assert!(compared_reads > 50, "{compared_reads}");
        assert_eq!(hybrid.stats().live_payload_bytes, mem.stats().live_bytes);
        assert_eq!(hybrid.ledger_totals(), mem.ledger_totals());
        for raw in 1..next_id {
            let id = SpoolId(raw);
            assert_eq!(outcome(hybrid.close(id)), outcome(mem.close(id)));
            assert_eq!(outcome(hybrid.delete(id)), outcome(mem.delete(id)));
        }
        assert_eq!((hybrid.live_ids(), hybrid.segment_count()), (0, 0));
        let stats = hybrid.stats();
        assert_eq!((stats.pages_in_use, stats.ram_retained_bytes), (0, 0));
        assert_eq!(hybrid.inner.ram.segment_count(), 0);
    }

    fn filled(store: &Store, id: u64, len: usize) -> SpoolId {
        let id = SpoolId(id);
        store.create(id, 1).unwrap();
        let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        store.append(id, &bytes).unwrap();
        store.close(id).unwrap();
        id
    }

    #[test]
    fn tier_reads_never_exceed_the_io_buffer() {
        let store = store();
        let id = filled(&store, 1, 3_000_000);
        let mut reader = store
            .reader(
                id,
                SpoolExtent {
                    offset: 5,
                    len: 2_999_000,
                },
            )
            .unwrap();
        let mut big = vec![0; 4 << 20];
        let mut total = 0;
        loop {
            let n = reader.read(&mut big).unwrap();
            if n == 0 {
                break;
            }
            total += n;
        }
        assert_eq!(total, 2_999_000);
        assert!(store.inner.ram.max_read_len() <= SPOOL_IO_BUFFER_BYTES);
    }

    #[test]
    fn segments_double_up_to_the_cap_and_the_last_is_trimmed_on_close() {
        let store = store();
        let id = SpoolId(1);
        store.create(id, 1).unwrap();
        // 32+64+128+256+512 KiB = 992 KiB, then 1 MiB segments.
        store.append(id, &vec![7; 992 * 1024 + 1]).unwrap();
        let k = 1024;
        assert_eq!(
            store.segment_capacities(id),
            [32 * k, 64 * k, 128 * k, 256 * k, 512 * k, 1024 * k]
        );
        store.append(id, &vec![7; 1024 * 1024]).unwrap();
        assert_eq!(store.segment_capacities(id).len(), 7);
        store.close(id).unwrap();
        // The sixth segment fills up and the seventh keeps 1 byte: one 32 KiB step.
        let caps = store.segment_capacities(id);
        assert_eq!(
            caps[..6],
            [32 * k, 64 * k, 128 * k, 256 * k, 512 * k, 1024 * k]
        );
        assert_eq!(caps[6], 32 * k);
        assert_eq!(
            store.stats().ram_retained_bytes,
            caps.iter().map(|c| u64::from(*c)).sum::<u64>()
        );
        assert_eq!(store.inner.ram.segment_count(), 7);
    }

    #[test]
    fn delete_fails_while_a_reader_is_alive() {
        let store = store();
        let id = filled(&store, 1, 100);
        let reader = store
            .reader(
                id,
                SpoolExtent {
                    offset: 0,
                    len: 100,
                },
            )
            .unwrap();
        let error = store.delete(id).unwrap_err().to_string();
        assert!(error.contains("spool has a live reader"), "{error}");
        drop(reader);
        store.delete(id).unwrap();
        assert_eq!((store.live_ids(), store.segment_count()), (0, 0));
        assert_eq!(store.stats().ram_retained_bytes, 0);
        assert_eq!(store.inner.ram.segment_count(), 0);
    }

    #[test]
    fn freed_segment_keys_are_reused() {
        let store = store();
        let a = filled(&store, 1, 10);
        store.delete(a).unwrap();
        filled(&store, 2, 10);
        assert_eq!(lock(&store.inner.state).next_key, 1);
    }

    #[test]
    fn a_spilled_input_is_read_from_pages_while_its_children_spill() {
        let store = spilling_store(0, 8, None, 64 * 1024, 1 << 22);
        let mut rng = Rng(42);
        let input: Vec<u8> = (0..1_300_000).map(|_| rng.next(256) as u8).collect();
        let id = SpoolId(1);
        store.create(id, 1).unwrap();
        store.append(id, &input).unwrap();
        store.close(id).unwrap();
        let extent = SpoolExtent {
            offset: 0,
            len: input.len() as u64,
        };
        let mut reader = store.reader(id, extent).unwrap();
        let mut seen = Vec::new();
        let mut chunk = vec![0; 20_000];
        let before = store.stats().spilled_spools;
        for child in 10..40u64 {
            let child_id = SpoolId(child);
            let bytes = vec![child as u8; 30_000 + rng.next(60_000) as usize];
            store.create(child_id, 1).unwrap();
            for piece in bytes.chunks(7_000) {
                store.append(child_id, piece).unwrap();
            }
            store.close(child_id).unwrap();
            let n = reader.read(&mut chunk).unwrap();
            seen.extend_from_slice(&chunk[..n]);
        }
        reader.read_to_end(&mut seen).unwrap();
        assert_eq!(seen, input);
        assert!(store.stats().spilled_spools > before + 10);
        drop(reader);
        store.delete(id).unwrap();
    }

    #[test]
    fn a_pinned_ram_input_is_skipped_by_the_policy() {
        let store = spilling_store(u64::MAX, 8, None, 64 * 1024, 1 << 22);
        let input: Vec<u8> = (0..300_000u32).map(|i| (i % 249) as u8).collect();
        let id = SpoolId(1);
        store.create(id, 1).unwrap();
        store.append(id, &input).unwrap();
        store.close(id).unwrap();
        let extent = SpoolExtent {
            offset: 0,
            len: input.len() as u64,
        };
        let mut reader = store.reader(id, extent).unwrap();
        // From here every allocation is over budget.
        lock(&store.inner.state).budget = 0;
        let mut seen = Vec::new();
        let mut chunk = vec![0; 10_000];
        for child in 10..60u64 {
            let child_id = SpoolId(child);
            store.create(child_id, 1).unwrap();
            for piece in vec![child as u8; 90_000].chunks(5_000) {
                store.append(child_id, piece).unwrap();
                assert!(!is_spilled(&store, 1), "the pinned input was spilled");
            }
            store.close(child_id).unwrap();
            let n = reader.read(&mut chunk).unwrap();
            seen.extend_from_slice(&chunk[..n]);
        }
        reader.read_to_end(&mut seen).unwrap();
        assert_eq!(seen, input);
        let stats = store.stats();
        assert!(stats.spilled_spools >= 50, "{stats:?}");
        assert!(stats.over_budget_peak_bytes >= 300_000, "{stats:?}");
        drop(reader);
        // Unpinned, the input is a normal victim at the next allocation.
        store.create(SpoolId(99), 1).unwrap();
        store.append(SpoolId(99), &[1]).unwrap();
        assert!(is_spilled(&store, 1));
        assert_eq!(read_all(&mut store.reader(id, extent).unwrap()), input);
    }

    #[test]
    fn a_short_page_write_is_a_storage_full_error() {
        let store = spilling_store(0, 1, Some(100_000), 256 * 1024, 1 << 30);
        let id = SpoolId(1);
        store.create(id, 1).unwrap();
        store.append(id, &vec![1; 300_000]).unwrap();
        let ConvertError::Io(error) = store.close(id).unwrap_err() else {
            panic!("expected an I/O error");
        };
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
        assert_eq!(
            error.to_string(),
            "storage is full writing spill zone 0: wrote 100000 of 262144 bytes"
        );
        assert_eq!(store.stats().pages_in_use, 0);
    }

    #[test]
    fn exhausted_zones_are_reported() {
        // One zone of two 64 KiB pages.
        let store = spilling_store(0, 1, None, 64 * 1024, 128 * 1024);
        filled(&store, 1, 40_000);
        filled(&store, 2, 40_000);
        let id = SpoolId(3);
        store.create(id, 1).unwrap();
        store.append(id, &vec![9; 40_000]).unwrap();
        let error = store.close(id).unwrap_err().to_string();
        assert!(error.contains("spill zones exhausted"), "{error}");
    }

    #[test]
    fn empty_zones_are_released_and_low_pages_are_reused() {
        let store = spilling_store(0, 3, None, 64 * 1024, 128 * 1024);
        for id in 1..=4 {
            filled(&store, id, 40_000);
        }
        let stats = store.stats();
        assert_eq!((stats.pages_in_use, stats.pages_peak), (4, 4));
        assert_eq!(store.inner.pages.zone_len(1), 64 * 1024 + 40_000);
        // Pages 2 and 3 fill zone 1.
        store.delete(SpoolId(3)).unwrap();
        assert_eq!(store.stats().zones_released, 0);
        store.delete(SpoolId(4)).unwrap();
        assert_eq!(store.stats().zones_released, 1);
        assert_eq!(store.inner.pages.zone_len(1), 0);
        // Page 0 frees up and is reused before any higher one.
        store.delete(SpoolId(1)).unwrap();
        filled(&store, 5, 40_000);
        filled(&store, 6, 40_000);
        let stats = store.stats();
        assert_eq!((stats.pages_in_use, stats.pages_peak), (3, 4));
        assert_eq!(lock(&store.inner.state).spools[&SpoolId(5)].pages, [0]);
        assert_eq!(lock(&store.inner.state).spools[&SpoolId(6)].pages, [2]);
    }

    #[test]
    fn victims_follow_the_four_tier_order() {
        let store = spilling_store(u64::MAX, 4, None, 64 * 1024, 1 << 22);
        // Closed without a hint: 5 closes before 6.
        filled(&store, 5, 10_000);
        filled(&store, 6, 20_000);
        // Consume ranks: the highest goes first.
        filled(&store, 3, 10_000);
        filled(&store, 4, 10_000);
        store.hint_consume_rank(SpoolId(3), 5);
        store.hint_consume_rank(SpoolId(4), 9);
        // Retained: the largest goes first, and it beats a rank.
        filled(&store, 1, 30_000);
        filled(&store, 2, 50_000);
        store.hint_retained(SpoolId(1));
        store.hint_retained(SpoolId(2));
        store.hint_consume_rank(SpoolId(2), 100);
        // Open spools of at least a page: the largest goes first; a small one never.
        for (id, len) in [(7, 70_000), (8, 100_000), (9, 1_000)] {
            store.create(SpoolId(id), 1).unwrap();
            store.append(SpoolId(id), &vec![1; len]).unwrap();
        }
        // A pinned retained spool is skipped.
        filled(&store, 10, 90_000);
        store.hint_retained(SpoolId(10));
        let pin = store
            .reader(SpoolId(10), SpoolExtent { offset: 0, len: 1 })
            .unwrap();
        let mut order = Vec::new();
        loop {
            let mut guard = lock(&store.inner.state);
            let Some(id) = guard.pick_victim() else { break };
            store.inner.spill_spool(&mut guard, id).unwrap();
            order.push(id.0);
        }
        assert_eq!(order, [2, 1, 4, 3, 5, 6, 8, 7]);
        assert!(!is_spilled(&store, 10) && !is_spilled(&store, 9));
        drop(pin);
    }

    #[test]
    fn an_open_spilled_spool_keeps_a_one_page_tail() {
        let store = spilling_store(0, 2, None, 64 * 1024, 1 << 22);
        let mem = MemSpoolStore::new(false);
        let id = SpoolId(1);
        store.create(id, 1).unwrap();
        mem.create(id, 1).unwrap();
        let data: Vec<u8> = (0..400_000u32).map(|i| (i % 253) as u8).collect();
        for piece in data.chunks(50_000) {
            store.append(id, piece).unwrap();
            mem.append(id, piece).unwrap();
        }
        assert!(is_spilled(&store, 1));
        assert_eq!(store.segment_capacities(id), [64 * 1024]);
        assert_eq!(store.stats().ram_retained_bytes, 64 * 1024);
        store.close(id).unwrap();
        mem.close(id).unwrap();
        assert_eq!(store.segment_count(), 0);
        assert_eq!(store.stats().ram_retained_bytes, 0);
        let extent = SpoolExtent {
            offset: 3,
            len: 399_000,
        };
        assert_eq!(
            read_all(&mut store.reader(id, extent).unwrap()),
            read_all(&mut mem.reader(id, extent).unwrap())
        );
    }
}
