//! Native spool store: one file per spool under a work directory.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Mutex, atomic::Ordering},
};

use viewer_convert_core::ConvertError;
use viewer_convert_core::pipeline::spool_store::{
    LedgerTotals, Phase, SPOOL_IO_BUFFER_BYTES, SpoolAccounting, SpoolExtent, SpoolId, SpoolLedger,
    SpoolStore, entry_in, lifecycle_error, lock,
};

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
                .map_err(std::io::IntoInnerError::into_error)?
                .sync_all()?;
        }
        Ok(())
    }

    fn len(&self, id: SpoolId) -> Result<u64, ConvertError> {
        let mut open = lock(&self.open);
        Ok(entry_in(&mut open.entries, id, "len", Phase::Closed, |e| e.phase)?.len)
    }

    /// The extent is checked against the length recorded by `append` (like `MemSpoolStore`), so
    /// a planner bug is rejected here; a spool truncated on disk still ends the read early and
    /// the caller reports `PointCountMismatch`.
    fn reader(&self, id: SpoolId, extent: SpoolExtent) -> Result<Self::Reader, ConvertError> {
        {
            let mut open = lock(&self.open);
            let entry = entry_in(&mut open.entries, id, "read", Phase::Closed, |e| e.phase)?;
            if extent
                .offset
                .checked_add(extent.len)
                .is_none_or(|end| end > entry.len)
            {
                return Err(lifecycle_error(id, "read", "extent is out of range"));
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn message(result: Result<impl Sized, ConvertError>) -> String {
        result.err().expect("the call must be rejected").to_string()
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
