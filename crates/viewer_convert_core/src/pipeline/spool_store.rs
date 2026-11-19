//! Spool storage seam: one append-only byte store per `SpoolId`.
//!
//! Lifecycle per id: `create` → `append`* → `close` → (`len` | `reader`)* → `delete`. Ids are never
//! reused and any other order is an error, so a store with different physics (RAM segments, OPFS
//! files) can implement the same contract. The trait is synchronous and takes `&self`; `Sync`
//! is a requirement of the native planner, not of the trait.

use std::{
    collections::HashMap,
    io::{self, Read},
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::error::ConvertError;

/// Read buffer of spool scans and block reads.
pub const SPOOL_IO_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpoolId(pub u64);

impl SpoolId {
    pub const SOURCE: Self = Self(0);
}

/// A byte range of a closed spool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpoolExtent {
    pub offset: u64,
    pub len: u64,
}

/// One file per spool. Per id: create → append* → close → (len | reader)* → delete.
/// Ids are never reused; any other order is an error.
pub trait SpoolStore {
    type Reader: Read;
    /// `record_bytes` is the size of one record of this spool. Only stores that lay records out
    /// by byte plane need it; the others ignore it.
    fn create(&self, id: SpoolId, record_bytes: usize) -> Result<(), ConvertError>;
    fn append(&self, id: SpoolId, bytes: &[u8]) -> Result<(), ConvertError>;
    fn close(&self, id: SpoolId) -> Result<(), ConvertError>;
    fn len(&self, id: SpoolId) -> Result<u64, ConvertError>;
    fn reader(&self, id: SpoolId, extent: SpoolExtent) -> Result<Self::Reader, ConvertError>;
    fn delete(&self, id: SpoolId) -> Result<(), ConvertError>;

    /// The spool is only re-read in the block phase. Advisory and infallible, so a hint can
    /// neither change control flow nor error precedence; stores that ignore it are correct.
    fn hint_retained(&self, _id: SpoolId) {}

    /// Consumption order among the queued spools: the larger rank is consumed later. Advisory
    /// like `hint_retained`.
    fn hint_consume_rank(&self, _id: SpoolId, _rank: u32) {}
}

/// Totals the planner reports as diagnostics; `None` when the store does not account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerTotals {
    pub written: u64,
    pub peak_live: u64,
    /// Bytes as stored (after compression); equal to `written` for stores that do not compress.
    pub physical_written: u64,
    pub physical_peak_live: u64,
}

/// Diagnostics that live next to the bytes: the byte ledger and the ordering counter of
/// `NodeSpoolEvent`s across workers. Kept apart from `SpoolStore` so that trait stays minimal.
pub trait SpoolAccounting {
    fn next_event_seq(&self) -> Option<u64>;
    fn ledger_totals(&self) -> Option<LedgerTotals>;
}

/// What the spatial planner needs from a store shared by every branch.
pub trait PlanStore: SpoolStore + SpoolAccounting + Sync {}
impl<T: SpoolStore + SpoolAccounting + Sync> PlanStore for T {}

/// Spool byte totals shared by every spatial worker; updated on every append and delete.
#[derive(Default)]
pub struct SpoolLedger {
    pub written: AtomicU64,
    pub live: AtomicU64,
    pub peak: AtomicU64,
    /// Stored (compressed) counterparts of `written`, `live` and `peak`.
    pub physical_written: AtomicU64,
    pub physical_live: AtomicU64,
    pub physical_peak: AtomicU64,
    /// Orders `NodeSpoolEvent` starts and ends across workers.
    pub event_seq: AtomicU64,
}

impl SpoolLedger {
    pub fn with_live(bytes: u64) -> Self {
        Self {
            written: AtomicU64::new(bytes),
            live: AtomicU64::new(bytes),
            peak: AtomicU64::new(bytes),
            physical_written: AtomicU64::new(bytes),
            physical_live: AtomicU64::new(bytes),
            physical_peak: AtomicU64::new(bytes),
            ..Self::default()
        }
    }

    pub fn add(&self, bytes: u64) {
        self.written.fetch_add(bytes, Ordering::Relaxed);
        let live = self
            .live
            .fetch_add(bytes, Ordering::Relaxed)
            .saturating_add(bytes);
        self.peak.fetch_max(live, Ordering::Relaxed);
    }

    pub fn remove(&self, bytes: u64) {
        debug_assert!(self.live.load(Ordering::Relaxed) >= bytes);
        let _ = self
            .live
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                Some(live.saturating_sub(bytes))
            });
    }

    pub fn add_physical(&self, bytes: u64) {
        self.physical_written.fetch_add(bytes, Ordering::Relaxed);
        let live = self
            .physical_live
            .fetch_add(bytes, Ordering::Relaxed)
            .saturating_add(bytes);
        self.physical_peak.fetch_max(live, Ordering::Relaxed);
    }

    pub fn remove_physical(&self, bytes: u64) {
        debug_assert!(self.physical_live.load(Ordering::Relaxed) >= bytes);
        let _ = self
            .physical_live
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                Some(live.saturating_sub(bytes))
            });
    }

    pub fn totals(&self) -> LedgerTotals {
        LedgerTotals {
            written: self.written.load(Ordering::Relaxed),
            peak_live: self.peak.load(Ordering::Relaxed),
            physical_written: self.physical_written.load(Ordering::Relaxed),
            physical_peak_live: self.physical_peak.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Open,
    Closed,
    Deleted,
}

pub fn lifecycle_error(id: SpoolId, operation: &str, reason: &str) -> ConvertError {
    ConvertError::Io(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("spool {}: cannot {operation}: {reason}", id.0),
    ))
}

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Looks `id` up and requires it to be in `want`.
#[expect(
    clippy::implicit_hasher,
    reason = "internal helper; every caller uses the default hasher"
)]
pub fn entry_in<'a, T>(
    entries: &'a mut HashMap<SpoolId, T>,
    id: SpoolId,
    operation: &str,
    want: Phase,
    phase_of: impl Fn(&T) -> Phase,
) -> Result<&'a mut T, ConvertError> {
    let entry = entries
        .get_mut(&id)
        .ok_or_else(|| lifecycle_error(id, operation, "unknown id"))?;
    let phase = phase_of(entry);
    if phase == want {
        Ok(entry)
    } else {
        Err(lifecycle_error(
            id,
            operation,
            &format!("spool is {phase:?}, expected {want:?}"),
        ))
    }
}

struct MemSpool {
    phase: Phase,
    data: Vec<u8>,
    /// Stride the caller declared at `create`; `close` checks the spool holds whole records, so a
    /// caller that passes the wrong size fails the tests instead of silently mis-striding.
    record_bytes: usize,
}

#[derive(Default)]
struct MemSpools {
    spools: HashMap<SpoolId, MemSpool>,
    written: u64,
    live_bytes: u64,
    peak_bytes: u64,
    live_spools: usize,
    peak_spools: usize,
    event_seq: u64,
    /// Whether `SpoolAccounting` reports (the planner's `collect_diagnostics`).
    accounting: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemSpoolStats {
    pub live_bytes: u64,
    pub peak_bytes: u64,
    pub live_spools: usize,
    pub peak_spools: usize,
}

/// In-memory store that enforces the whole lifecycle; it models the footprint of a RAM-backed
/// implementation (a spool is live from `create` to `delete`) and backs the tests.
pub struct MemSpoolStore {
    state: Mutex<MemSpools>,
}

impl MemSpoolStore {
    /// Bytes and spools are always counted (`stats`); `diagnostics` only gates `SpoolAccounting`.
    pub fn new(diagnostics: bool) -> Self {
        Self {
            state: Mutex::new(MemSpools {
                accounting: diagnostics,
                ..MemSpools::default()
            }),
        }
    }

    pub fn stats(&self) -> MemSpoolStats {
        let state = lock(&self.state);
        MemSpoolStats {
            live_bytes: state.live_bytes,
            peak_bytes: state.peak_bytes,
            live_spools: state.live_spools,
            peak_spools: state.peak_spools,
        }
    }
}

impl SpoolStore for MemSpoolStore {
    type Reader = io::Cursor<Vec<u8>>;

    fn create(&self, id: SpoolId, record_bytes: usize) -> Result<(), ConvertError> {
        let mut state = lock(&self.state);
        if state.spools.contains_key(&id) {
            return Err(lifecycle_error(id, "create", "id was already used"));
        }
        state.spools.insert(
            id,
            MemSpool {
                phase: Phase::Open,
                data: Vec::new(),
                record_bytes,
            },
        );
        state.live_spools += 1;
        state.peak_spools = state.peak_spools.max(state.live_spools);
        Ok(())
    }

    fn append(&self, id: SpoolId, bytes: &[u8]) -> Result<(), ConvertError> {
        let mut state = lock(&self.state);
        let spool = entry_in(&mut state.spools, id, "append", Phase::Open, |s| s.phase)?;
        spool.data.extend_from_slice(bytes);
        let added = bytes.len() as u64;
        state.written += added;
        state.live_bytes += added;
        state.peak_bytes = state.peak_bytes.max(state.live_bytes);
        Ok(())
    }

    fn close(&self, id: SpoolId) -> Result<(), ConvertError> {
        let mut state = lock(&self.state);
        let spool = entry_in(&mut state.spools, id, "close", Phase::Open, |s| s.phase)?;
        if spool.record_bytes == 0 || !spool.data.len().is_multiple_of(spool.record_bytes) {
            return Err(lifecycle_error(
                id,
                "close",
                "length is not a whole number of records",
            ));
        }
        spool.phase = Phase::Closed;
        Ok(())
    }

    fn len(&self, id: SpoolId) -> Result<u64, ConvertError> {
        let mut state = lock(&self.state);
        let spool = entry_in(&mut state.spools, id, "len", Phase::Closed, |s| s.phase)?;
        Ok(spool.data.len() as u64)
    }

    fn reader(&self, id: SpoolId, extent: SpoolExtent) -> Result<Self::Reader, ConvertError> {
        let mut state = lock(&self.state);
        let spool = entry_in(&mut state.spools, id, "read", Phase::Closed, |s| s.phase)?;
        let range = usize::try_from(extent.offset)
            .ok()
            .zip(usize::try_from(extent.len).ok())
            .and_then(|(offset, len)| Some(offset..offset.checked_add(len)?))
            .filter(|range| range.end <= spool.data.len())
            .ok_or_else(|| lifecycle_error(id, "read", "extent is out of range"))?;
        Ok(io::Cursor::new(spool.data[range].to_vec()))
    }

    fn delete(&self, id: SpoolId) -> Result<(), ConvertError> {
        let mut state = lock(&self.state);
        let spool = entry_in(&mut state.spools, id, "delete", Phase::Closed, |s| s.phase)?;
        spool.phase = Phase::Deleted;
        let freed = std::mem::take(&mut spool.data).len() as u64;
        state.live_bytes -= freed;
        state.live_spools -= 1;
        Ok(())
    }
}

impl SpoolAccounting for MemSpoolStore {
    fn next_event_seq(&self) -> Option<u64> {
        let mut state = lock(&self.state);
        let accounting = state.accounting;
        let seq = state.event_seq;
        state.event_seq += 1;
        accounting.then_some(seq)
    }

    fn ledger_totals(&self) -> Option<LedgerTotals> {
        let state = lock(&self.state);
        state.accounting.then_some(LedgerTotals {
            written: state.written,
            peak_live: state.peak_bytes,
            physical_written: state.written,
            physical_peak_live: state.peak_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(result: Result<impl Sized, ConvertError>) -> String {
        result.err().expect("the call must be rejected").to_string()
    }

    fn closed(store: &MemSpoolStore, id: u64, bytes: &[u8]) -> SpoolId {
        let id = SpoolId(id);
        store.create(id, 1).unwrap();
        store.append(id, bytes).unwrap();
        store.close(id).unwrap();
        id
    }

    #[test]
    fn mem_store_round_trips_extents_and_tracks_footprint() {
        let store = MemSpoolStore::new(true);
        let a = closed(&store, 1, b"abcdef");
        let b = closed(&store, 2, b"xy");
        assert_eq!(store.len(a).unwrap(), 6);
        let mut out = Vec::new();
        store
            .reader(a, SpoolExtent { offset: 2, len: 3 })
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, b"cde");
        assert_eq!(
            store.stats(),
            MemSpoolStats {
                live_bytes: 8,
                peak_bytes: 8,
                live_spools: 2,
                peak_spools: 2
            }
        );
        store.delete(a).unwrap();
        store.delete(b).unwrap();
        assert_eq!(
            store.stats(),
            MemSpoolStats {
                live_bytes: 0,
                peak_bytes: 8,
                live_spools: 0,
                peak_spools: 2
            }
        );
    }

    #[test]
    fn mem_store_rejects_append_after_close() {
        let store = MemSpoolStore::new(true);
        let id = closed(&store, 1, b"a");
        assert!(message(store.append(id, b"b")).contains("append"));
    }

    #[test]
    fn mem_store_rejects_deleting_an_open_spool() {
        let store = MemSpoolStore::new(true);
        store.create(SpoolId(1), 1).unwrap();
        assert!(message(store.delete(SpoolId(1))).contains("delete"));
    }

    #[test]
    fn mem_store_never_reuses_an_id() {
        let store = MemSpoolStore::new(true);
        let id = closed(&store, 1, b"a");
        assert!(message(store.create(id, 1)).contains("already used"));
        store.delete(id).unwrap();
        assert!(message(store.create(id, 1)).contains("already used"));
        assert!(message(store.delete(id)).contains("delete"));
    }

    #[test]
    fn mem_store_rejects_reads_before_close_and_outside_the_extent() {
        let store = MemSpoolStore::new(true);
        store.create(SpoolId(1), 1).unwrap();
        let whole = SpoolExtent { offset: 0, len: 0 };
        assert!(message(store.reader(SpoolId(1), whole)).contains("read"));
        assert!(message(store.len(SpoolId(1))).contains("len"));
        store.append(SpoolId(1), b"abc").unwrap();
        store.close(SpoolId(1)).unwrap();
        let past_end = SpoolExtent { offset: 1, len: 3 };
        assert!(message(store.reader(SpoolId(1), past_end)).contains("out of range"));
    }
}
