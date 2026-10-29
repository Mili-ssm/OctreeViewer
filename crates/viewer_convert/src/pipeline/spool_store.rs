//! Spool storage seam: one append-only byte store per `SpoolId`.
//!
//! Lifecycle per id: `create` → `append`* → `close` → (`len` | `reader`)* → `delete`. Ids are never
//! reused and any other order is an error, so a store with different physics (RAM segments, OPFS
//! files) can implement the same contract. The trait is synchronous and takes `&self`; `Sync`
//! is a requirement of the native planner, not of the trait.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::error::ConvertError;

/// Read buffer of spool scans and block reads.
pub(crate) const SPOOL_IO_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct SpoolId(pub u64);

impl SpoolId {
    pub const SOURCE: Self = Self(0);
}

/// A byte range of a closed spool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpoolExtent {
    pub offset: u64,
    pub len: u64,
}

/// One file per spool. Per id: create → append* → close → (len | reader)* → delete.
/// Ids are never reused; any other order is an error.
pub(crate) trait SpoolStore {
    type Reader: Read;
    fn create(&self, id: SpoolId) -> Result<(), ConvertError>;
    fn append(&self, id: SpoolId, bytes: &[u8]) -> Result<(), ConvertError>;
    fn close(&self, id: SpoolId) -> Result<(), ConvertError>;
    fn len(&self, id: SpoolId) -> Result<u64, ConvertError>;
    fn reader(&self, id: SpoolId, extent: SpoolExtent) -> Result<Self::Reader, ConvertError>;
    fn delete(&self, id: SpoolId) -> Result<(), ConvertError>;
}

/// Totals the planner reports as diagnostics; `None` when the store does not account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LedgerTotals {
    pub written: u64,
    pub peak_live: u64,
}

/// Diagnostics that live next to the bytes: the byte ledger and the ordering counter of
/// `NodeSpoolEvent`s across workers. Kept apart from `SpoolStore` so that trait stays minimal.
pub(crate) trait SpoolAccounting {
    fn next_event_seq(&self) -> Option<u64>;
    fn ledger_totals(&self) -> Option<LedgerTotals>;
}

/// What the spatial planner needs from a store shared by every branch.
pub(crate) trait PlanStore: SpoolStore + SpoolAccounting + Sync {}
impl<T: SpoolStore + SpoolAccounting + Sync> PlanStore for T {}

/// Spool byte totals shared by every spatial worker; updated on every append and delete.
#[derive(Default)]
struct SpoolLedger {
    written: AtomicU64,
    live: AtomicU64,
    peak: AtomicU64,
    /// Orders `NodeSpoolEvent` starts and ends across workers.
    event_seq: AtomicU64,
}

impl SpoolLedger {
    fn with_live(bytes: u64) -> Self {
        Self {
            written: AtomicU64::new(bytes),
            live: AtomicU64::new(bytes),
            peak: AtomicU64::new(bytes),
            ..Self::default()
        }
    }

    fn add(&self, bytes: u64) {
        self.written.fetch_add(bytes, Ordering::Relaxed);
        let live = self
            .live
            .fetch_add(bytes, Ordering::Relaxed)
            .saturating_add(bytes);
        self.peak.fetch_max(live, Ordering::Relaxed);
    }

    fn remove(&self, bytes: u64) {
        debug_assert!(self.live.load(Ordering::Relaxed) >= bytes);
        let _ = self
            .live
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                Some(live.saturating_sub(bytes))
            });
    }

    fn totals(&self) -> LedgerTotals {
        LedgerTotals {
            written: self.written.load(Ordering::Relaxed),
            peak_live: self.peak.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Open,
    Closed,
    Deleted,
}

fn lifecycle_error(id: SpoolId, operation: &str, reason: &str) -> ConvertError {
    ConvertError::Io(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("spool {}: cannot {operation}: {reason}", id.0),
    ))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Looks `id` up and requires it to be in `want`.
fn entry_in<'a, T>(
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

struct FileEntry {
    phase: Phase,
    /// The file exists on disk (`SOURCE` always; the others from the first append).
    created: bool,
    len: u64,
    /// `SOURCE` only: one writer from `create` to `close`.
    writer: Option<BufWriter<File>>,
}

#[derive(Default)]
struct OpenSpools {
    entries: HashMap<SpoolId, FileEntry>,
}

/// Native store: `SOURCE` is `source.spool`, any other id `spool-{id}.spool`, under `root`.
///
/// `SOURCE` keeps one `BufWriter` from `create` to `close`. The others create their file at the
/// first `append` (`create_new`) and reopen it in append mode on each later one, so the number of
/// open descriptors never grows with the number of live spools.
pub(crate) struct FileSpoolStore {
    root: PathBuf,
    ledger: Option<SpoolLedger>,
    open: Mutex<OpenSpools>,
}

impl FileSpoolStore {
    pub fn new(root: &Path, diagnostics: bool) -> Self {
        Self {
            root: root.to_owned(),
            ledger: diagnostics.then(SpoolLedger::default),
            open: Mutex::new(OpenSpools::default()),
        }
    }

    /// Checkpoint path: `SOURCE` already exists and is closed; the ledger starts at `source_len`.
    pub fn with_closed_source(root: &Path, source_len: u64, diagnostics: bool) -> Self {
        let store = Self {
            root: root.to_owned(),
            ledger: diagnostics.then(|| SpoolLedger::with_live(source_len)),
            open: Mutex::new(OpenSpools::default()),
        };
        let len = fs::metadata(store.path(SpoolId::SOURCE)).map_or(source_len, |meta| meta.len());
        lock(&store.open).entries.insert(
            SpoolId::SOURCE,
            FileEntry {
                phase: Phase::Closed,
                created: true,
                len,
                writer: None,
            },
        );
        store
    }

    fn path(&self, id: SpoolId) -> PathBuf {
        if id == SpoolId::SOURCE {
            self.root.join("source.spool")
        } else {
            self.root.join(format!("spool-{}.spool", id.0))
        }
    }
}

impl SpoolStore for FileSpoolStore {
    type Reader = io::Take<BufReader<File>>;

    fn create(&self, id: SpoolId) -> Result<(), ConvertError> {
        let mut open = lock(&self.open);
        if open.entries.contains_key(&id) {
            return Err(lifecycle_error(id, "create", "id was already used"));
        }
        let writer = if id == SpoolId::SOURCE {
            Some(BufWriter::new(File::create(self.path(id))?))
        } else {
            None
        };
        open.entries.insert(
            id,
            FileEntry {
                phase: Phase::Open,
                created: writer.is_some(),
                len: 0,
                writer,
            },
        );
        Ok(())
    }

    fn append(&self, id: SpoolId, bytes: &[u8]) -> Result<(), ConvertError> {
        let (first, writer) = {
            let mut open = lock(&self.open);
            let entry = entry_in(&mut open.entries, id, "append", Phase::Open, |e| e.phase)?;
            if bytes.is_empty() {
                return Ok(());
            }
            entry.len = entry
                .len
                .checked_add(bytes.len() as u64)
                .ok_or(ConvertError::SizeOverflow)?;
            let first = !entry.created;
            entry.created = true;
            (first, entry.writer.take())
        };
        if id == SpoolId::SOURCE {
            let mut writer = writer.expect("the SOURCE writer lives from create to close");
            let result = writer.write_all(bytes);
            lock(&self.open)
                .entries
                .get_mut(&id)
                .expect("the entry was found above")
                .writer = Some(writer);
            result?;
        } else {
            let mut options = OpenOptions::new();
            options.write(true);
            if first {
                options.create_new(true);
            } else {
                options.append(true);
            }
            options.open(self.path(id))?.write_all(bytes)?;
        }
        if let Some(ledger) = &self.ledger {
            ledger.add(bytes.len() as u64);
        }
        Ok(())
    }

    fn close(&self, id: SpoolId) -> Result<(), ConvertError> {
        let writer = {
            let mut open = lock(&self.open);
            let entry = entry_in(&mut open.entries, id, "close", Phase::Open, |e| e.phase)?;
            entry.phase = Phase::Closed;
            entry.writer.take()
        };
        if let Some(writer) = writer {
            writer
                .into_inner()
                .map_err(|error| error.into_error())?
                .sync_all()?;
        }
        Ok(())
    }

    fn len(&self, id: SpoolId) -> Result<u64, ConvertError> {
        let mut open = lock(&self.open);
        Ok(entry_in(&mut open.entries, id, "len", Phase::Closed, |e| e.phase)?.len)
    }

    /// An extent past the end of the file is not an error here: the reader just ends early and the
    /// caller reports `PointCountMismatch`, as reads of truncated spools always did.
    fn reader(&self, id: SpoolId, extent: SpoolExtent) -> Result<Self::Reader, ConvertError> {
        {
            let mut open = lock(&self.open);
            entry_in(&mut open.entries, id, "read", Phase::Closed, |e| e.phase)?;
        }
        let mut file = File::open(self.path(id))?;
        if extent.offset > 0 {
            file.seek(SeekFrom::Start(extent.offset))?;
        }
        Ok(BufReader::with_capacity(SPOOL_IO_BUFFER_BYTES, file).take(extent.len))
    }

    fn delete(&self, id: SpoolId) -> Result<(), ConvertError> {
        let (created, len) = {
            let mut open = lock(&self.open);
            let entry = entry_in(&mut open.entries, id, "delete", Phase::Closed, |e| e.phase)?;
            entry.phase = Phase::Deleted;
            (entry.created, entry.len)
        };
        if created {
            fs::remove_file(self.path(id))?;
        }
        if let Some(ledger) = &self.ledger {
            ledger.remove(len);
        }
        Ok(())
    }
}

impl SpoolAccounting for FileSpoolStore {
    fn next_event_seq(&self) -> Option<u64> {
        self.ledger
            .as_ref()
            .map(|ledger| ledger.event_seq.fetch_add(1, Ordering::Relaxed))
    }

    fn ledger_totals(&self) -> Option<LedgerTotals> {
        self.ledger.as_ref().map(SpoolLedger::totals)
    }
}

#[allow(dead_code)] // Used by tests and by the future web host.
struct MemSpool {
    phase: Phase,
    data: Vec<u8>,
}

#[derive(Default)]
#[allow(dead_code)] // Used by tests and by the future web host.
struct MemSpools {
    spools: HashMap<SpoolId, MemSpool>,
    written: u64,
    live_bytes: u64,
    peak_bytes: u64,
    live_spools: usize,
    peak_spools: usize,
    event_seq: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Used by tests and by the future web host.
pub(crate) struct MemSpoolStats {
    pub live_bytes: u64,
    pub peak_bytes: u64,
    pub live_spools: usize,
    pub peak_spools: usize,
}

/// In-memory store that enforces the whole lifecycle; it models the footprint of a RAM-backed
/// implementation (a spool is live from `create` to `delete`) and backs the tests.
#[allow(dead_code)] // Used by tests and by the future web host.
pub(crate) struct MemSpoolStore {
    state: Mutex<MemSpools>,
}

#[allow(dead_code)] // Used by tests and by the future web host.
impl MemSpoolStore {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(MemSpools::default()),
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

    fn create(&self, id: SpoolId) -> Result<(), ConvertError> {
        let mut state = lock(&self.state);
        if state.spools.contains_key(&id) {
            return Err(lifecycle_error(id, "create", "id was already used"));
        }
        state.spools.insert(
            id,
            MemSpool {
                phase: Phase::Open,
                data: Vec::new(),
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
        entry_in(&mut state.spools, id, "close", Phase::Open, |s| s.phase)?.phase = Phase::Closed;
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
        let seq = state.event_seq;
        state.event_seq += 1;
        Some(seq)
    }

    fn ledger_totals(&self) -> Option<LedgerTotals> {
        let state = lock(&self.state);
        Some(LedgerTotals {
            written: state.written,
            peak_live: state.peak_bytes,
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
        store.create(id).unwrap();
        store.append(id, bytes).unwrap();
        store.close(id).unwrap();
        id
    }

    #[test]
    fn mem_store_round_trips_extents_and_tracks_footprint() {
        let store = MemSpoolStore::new();
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
        let store = MemSpoolStore::new();
        let id = closed(&store, 1, b"a");
        assert!(message(store.append(id, b"b")).contains("append"));
    }

    #[test]
    fn mem_store_rejects_deleting_an_open_spool() {
        let store = MemSpoolStore::new();
        store.create(SpoolId(1)).unwrap();
        assert!(message(store.delete(SpoolId(1))).contains("delete"));
    }

    #[test]
    fn mem_store_never_reuses_an_id() {
        let store = MemSpoolStore::new();
        let id = closed(&store, 1, b"a");
        assert!(message(store.create(id)).contains("already used"));
        store.delete(id).unwrap();
        assert!(message(store.create(id)).contains("already used"));
        assert!(message(store.delete(id)).contains("delete"));
    }

    #[test]
    fn mem_store_rejects_reads_before_close_and_outside_the_extent() {
        let store = MemSpoolStore::new();
        store.create(SpoolId(1)).unwrap();
        let whole = SpoolExtent { offset: 0, len: 0 };
        assert!(message(store.reader(SpoolId(1), whole)).contains("read"));
        assert!(message(store.len(SpoolId(1))).contains("len"));
        store.append(SpoolId(1), b"abc").unwrap();
        store.close(SpoolId(1)).unwrap();
        let past_end = SpoolExtent { offset: 1, len: 3 };
        assert!(message(store.reader(SpoolId(1), past_end)).contains("out of range"));
    }

    #[test]
    fn file_store_matches_the_lifecycle_and_native_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), true);
        store.create(SpoolId::SOURCE).unwrap();
        store.append(SpoolId::SOURCE, b"source").unwrap();
        store.close(SpoolId::SOURCE).unwrap();
        store.create(SpoolId(7)).unwrap();
        assert!(!dir.path().join("spool-7.spool").exists(), "created lazily");
        store.append(SpoolId(7), b"ab").unwrap();
        store.append(SpoolId(7), b"cd").unwrap();
        assert!(
            message(store.reader(SpoolId(7), SpoolExtent { offset: 0, len: 4 })).contains("read")
        );
        store.close(SpoolId(7)).unwrap();
        assert_eq!(
            fs::read(dir.path().join("source.spool")).unwrap(),
            b"source"
        );
        let mut out = Vec::new();
        store
            .reader(SpoolId(7), SpoolExtent { offset: 1, len: 2 })
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, b"bc");
        assert!(message(store.append(SpoolId(7), b"e")).contains("append"));
        store.delete(SpoolId(7)).unwrap();
        store.delete(SpoolId::SOURCE).unwrap();
        assert!(message(store.create(SpoolId(7))).contains("already used"));
        assert_eq!(
            store.ledger_totals(),
            Some(LedgerTotals {
                written: 10,
                peak_live: 10
            })
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn file_store_resumes_from_a_closed_source() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("source.spool"), b"12345").unwrap();
        let store = FileSpoolStore::with_closed_source(dir.path(), 5, true);
        assert_eq!(store.len(SpoolId::SOURCE).unwrap(), 5);
        assert!(message(store.create(SpoolId::SOURCE)).contains("already used"));
        store.delete(SpoolId::SOURCE).unwrap();
        assert!(!dir.path().join("source.spool").exists());
    }
}
