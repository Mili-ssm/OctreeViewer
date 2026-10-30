//! `HybridSpoolStore`: the `SpoolStore` contract over a RAM tier and a page tier.
//!
//! A spool is a list of RAM segments that doubles from `SEGMENT_MIN_BYTES` to
//! `SEGMENT_MAX_BYTES`; closing trims the last one. The RAM budget compares the sum of segment
//! capacities, not payload. Readers pin their spool (`delete` fails while one is alive) and copy
//! at most one I/O buffer per `read`, so no tier ever materialises a whole extent.

use std::{
    collections::HashMap,
    io::{self, Read},
    sync::{Arc, Mutex},
};

use super::{
    pages::PageAllocator,
    tiers::{PageTier, RamTier, SegmentKey},
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
    segments: Vec<Segment>,
    len: u64,
    pins: u32,
}

struct State {
    spools: HashMap<SpoolId, Spool>,
    pages: PageAllocator,
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

    /// Seam of the spill policy: called after every change that grows the retained RAM.
    fn maybe_spill(&mut self) {}
}

struct Inner<R, P> {
    state: Mutex<State>,
    ram: R,
    #[allow(dead_code, reason = "read by the spill policy")]
    pages: P,
}

pub struct HybridSpoolStore<R, P> {
    inner: Arc<Inner<R, P>>,
}

fn invalid_config(message: &str) -> ConvertError {
    ConvertError::Io(io::Error::new(io::ErrorKind::InvalidInput, message))
}

impl<R: RamTier + Send + Sync, P: PageTier + Send + Sync> HybridSpoolStore<R, P> {
    pub fn new(
        ram: R,
        pages: P,
        config: HybridConfig,
        diagnostics: bool,
    ) -> Result<Self, ConvertError> {
        if pages.zone_count() == 0 {
            return Err(invalid_config("the page tier has no spill zones"));
        }
        let allocator =
            PageAllocator::new(pages.zone_count(), config.zone_bytes, config.page_bytes)?;
        Ok(Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    spools: HashMap::new(),
                    pages: allocator,
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
                    accounting: diagnostics,
                }),
                ram,
                pages,
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
            ..HybridStats::default()
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
            .map(|spool| spool.segments.iter().map(|s| s.capacity).collect())
            .unwrap_or_default()
    }
}

fn alloc_error(capacity: u32, error: &ConvertError) -> ConvertError {
    ConvertError::Io(io::Error::new(
        io::ErrorKind::OutOfMemory,
        format!("could not allocate a {capacity}-byte spool segment: {error}"),
    ))
}

impl<R: RamTier + Send + Sync, P: PageTier + Send + Sync> SpoolStore for HybridSpoolStore<R, P> {
    type Reader = HybridReader<R, P>;

    fn create(&self, id: SpoolId) -> Result<(), ConvertError> {
        let mut state = lock(&self.inner.state);
        if state.spools.contains_key(&id) {
            return Err(lifecycle_error(id, "create", "id was already used"));
        }
        state.spools.insert(
            id,
            Spool {
                phase: Phase::Open,
                segments: Vec::new(),
                len: 0,
                pins: 0,
            },
        );
        Ok(())
    }

    fn append(&self, id: SpoolId, bytes: &[u8]) -> Result<(), ConvertError> {
        let mut guard = lock(&self.inner.state);
        let state = &mut *guard;
        entry_in(&mut state.spools, id, "append", Phase::Open, |s| s.phase)?;
        let mut rest = bytes;
        while !rest.is_empty() {
            let spool = &state.spools[&id];
            let full = spool.segments.last().is_none_or(|s| s.used == s.capacity);
            if full {
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
            }
            let spool = state.spools.get_mut(&id).expect("spool exists");
            let segment = spool
                .segments
                .last_mut()
                .expect("a segment was just ensured");
            let n = ((segment.capacity - segment.used) as usize).min(rest.len());
            self.inner
                .ram
                .write(segment.key, segment.used, &rest[..n])?;
            segment.used += n as u32;
            spool.len += n as u64;
            state.written += n as u64;
            state.live_payload += n as u64;
            state.peak_payload = state.peak_payload.max(state.live_payload);
            rest = &rest[n..];
        }
        state.maybe_spill();
        Ok(())
    }

    fn close(&self, id: SpoolId) -> Result<(), ConvertError> {
        let mut guard = lock(&self.inner.state);
        let state = &mut *guard;
        let spool = entry_in(&mut state.spools, id, "close", Phase::Open, |s| s.phase)?;
        spool.phase = Phase::Closed;
        if let Some(segment) = spool.segments.last_mut() {
            let trimmed = segment.used.div_ceil(SEGMENT_MIN_BYTES) * SEGMENT_MIN_BYTES;
            if trimmed < segment.capacity {
                self.inner.ram.shrink(segment.key, trimmed)?;
                state.ram_retained -= u64::from(segment.capacity - trimmed);
                segment.capacity = trimmed;
            }
        }
        Ok(())
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
        let len = std::mem::take(&mut spool.len);
        state.live_payload -= len;
        let mut first_error = None;
        for segment in segments {
            state.ram_retained -= u64::from(segment.capacity);
            state.free_keys.push(segment.key.0);
            if let Err(error) = self.inner.ram.free(segment.key) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
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
        let state = lock(&self.inner.state);
        let spool = state
            .spools
            .get(&self.id)
            .ok_or_else(|| io::Error::other("pinned spool vanished"))?;
        let index = spool
            .segments
            .partition_point(|s| s.start + u64::from(s.used) <= self.position);
        let segment = spool
            .segments
            .get(index)
            .ok_or_else(|| io::Error::other("spool segments do not cover the extent"))?;
        let at = (self.position - segment.start) as u32;
        let n = buf
            .len()
            .min(SPOOL_IO_BUFFER_BYTES)
            .min((segment.used - at) as usize)
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        self.inner
            .ram
            .read(segment.key, at, &mut buf[..n])
            .map_err(|error| match error {
                ConvertError::Io(error) => error,
                other => io::Error::other(other.to_string()),
            })?;
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

    #[test]
    fn matches_mem_store_on_a_random_operation_sequence() {
        let hybrid = store();
        let mem = MemSpoolStore::new(true);
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
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
                    assert_eq!(outcome(hybrid.create(id)), outcome(mem.create(id)));
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
    }

    fn filled(store: &Store, id: u64, len: usize) -> SpoolId {
        let id = SpoolId(id);
        store.create(id).unwrap();
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
        store.create(id).unwrap();
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
}
