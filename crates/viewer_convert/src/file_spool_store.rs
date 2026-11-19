//! Native spool store: one file per spool under a work directory.
//!
//! `SOURCE` is stored raw (checkpoint and resume read and fingerprint it as is); every other spool
//! is a sequence of byte-plane LZ4 frames (see `spool_frame`), so its file is smaller than its
//! logical length. Offsets in `SpoolExtent` and `len` are always logical.

use std::{
    cell::RefCell,
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

use crate::spool_frame::{
    FRAME_HEADER_BYTES, FrameHeader, MAX_FRAME_RECORDS, decode_frame, encode_frame,
};

/// One sparse-index entry per this many frames. A per-frame table costs 16 bytes per frame, about
/// 0.05 % of the live logical bytes (~1.3 GB for ~2.7 TB of live spools at the 500 GB scale); a
/// header walk from the file start would make block reads quadratic on large leaf spools.
const INDEX_STRIDE_FRAMES: u64 = 64;
/// Encoded frames are written in batches of about this size.
const WRITE_BATCH_BYTES: usize = 1 << 20;

thread_local! {
    static ENCODED: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

struct FileEntry {
    phase: Phase,
    /// The file exists on disk (`SOURCE` always; the others from the first append).
    created: bool,
    /// Logical length, including the bytes still in `carry`.
    len: u64,
    /// Size of one record: frames hold whole records only.
    record_bytes: usize,
    /// `SOURCE` only: one writer from `create` to `close`.
    writer: Option<BufWriter<File>>,
    /// Appended bytes that do not yet fill a record (framed spools only).
    carry: Vec<u8>,
    /// Bytes on disk.
    physical_len: u64,
    frames: u64,
    /// `(logical, physical)` offset of the start of frames 0, 64, 128, ...
    sparse_index: Vec<(u64, u64)>,
    /// A write failed: the spool can only be deleted.
    failed: bool,
}

impl FileEntry {
    fn new(phase: Phase, created: bool, len: u64, record_bytes: usize) -> Self {
        Self {
            phase,
            created,
            len,
            record_bytes,
            writer: None,
            carry: Vec::new(),
            physical_len: 0,
            frames: 0,
            sparse_index: Vec::new(),
            failed: false,
        }
    }
}

#[derive(Default)]
struct OpenSpools {
    entries: HashMap<SpoolId, FileEntry>,
}

/// Where a framed append continues; taken from the entry under the lock, so the encoding and the
/// write run outside it (the lifecycle gives each spool a single writer).
struct FrameCursor {
    first: bool,
    record_bytes: usize,
    carry: Vec<u8>,
    logical: u64,
    physical: u64,
    frames: u64,
}

struct FramedAppend {
    carry: Vec<u8>,
    physical: u64,
    frames: u64,
    index: Vec<(u64, u64)>,
}

fn write_frames(path: &Path, cursor: FrameCursor, bytes: &[u8]) -> io::Result<FramedAppend> {
    let FrameCursor {
        first,
        record_bytes,
        mut carry,
        mut logical,
        mut physical,
        mut frames,
    } = cursor;
    let joined;
    let data = if carry.is_empty() {
        bytes
    } else {
        carry.extend_from_slice(bytes);
        joined = carry;
        &joined
    };
    let (body, tail) = data.split_at(data.len() - data.len() % record_bytes);
    let mut index = Vec::new();
    let mut options = OpenOptions::new();
    options.write(true);
    if first {
        options.create_new(true);
    } else {
        options.append(true);
    }
    let mut file = options.open(path)?;
    ENCODED.with_borrow_mut(|out| {
        out.clear();
        for chunk in body.chunks(MAX_FRAME_RECORDS * record_bytes) {
            if frames.is_multiple_of(INDEX_STRIDE_FRAMES) {
                index.push((logical, physical + out.len() as u64));
            }
            encode_frame(chunk, record_bytes, out);
            logical += chunk.len() as u64;
            frames += 1;
            if out.len() >= WRITE_BATCH_BYTES {
                file.write_all(out)?;
                physical += out.len() as u64;
                out.clear();
            }
        }
        file.write_all(out)?;
        physical += out.len() as u64;
        out.clear();
        Ok::<_, io::Error>(())
    })?;
    Ok(FramedAppend {
        carry: tail.to_vec(),
        physical,
        frames,
        index,
    })
}

/// Delivers exactly the requested logical bytes of a framed spool, decoding one frame at a time.
pub(crate) struct FramedReader {
    file: BufReader<File>,
    record_bytes: usize,
    /// Logical bytes still to deliver.
    remaining: u64,
    /// Logical bytes to drop from the first frame decoded.
    skip: u64,
    payload: Vec<u8>,
    frame: Vec<u8>,
    pos: usize,
    /// Kind of the first error: later reads fail the same way instead of skipping a frame.
    poisoned: Option<io::ErrorKind>,
}

fn ensure_sound(entry: &FileEntry, id: SpoolId, operation: &str) -> Result<(), ConvertError> {
    if entry.failed {
        Err(lifecycle_error(id, operation, "an earlier write failed"))
    } else {
        Ok(())
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

/// The header is untrusted: checked before any length is used to skip or allocate.
fn validate_header(header: FrameHeader, record_bytes: usize) -> io::Result<()> {
    let logical = header.logical_len as usize;
    if !logical.is_multiple_of(record_bytes) || logical / record_bytes > MAX_FRAME_RECORDS {
        return Err(invalid("spool frame length is not a valid record multiple"));
    }
    if header.stored_len > header.logical_len
        || (header.stored_len == header.logical_len) != header.raw
    {
        return Err(invalid("spool frame header is inconsistent"));
    }
    Ok(())
}

impl FramedReader {
    fn open(
        path: &Path,
        record_bytes: usize,
        start: (u64, u64),
        extent: SpoolExtent,
    ) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let (mut logical, physical) = start;
        file.seek(SeekFrom::Start(physical))?;
        let mut file = BufReader::with_capacity(SPOOL_IO_BUFFER_BYTES, file);
        if extent.len > 0 {
            loop {
                let header = read_header(&mut file)?;
                validate_header(header, record_bytes)?;
                let end = logical + u64::from(header.logical_len);
                if end > extent.offset {
                    file.seek_relative(-(FRAME_HEADER_BYTES as i64))?;
                    break;
                }
                file.seek_relative(i64::from(header.stored_len))?;
                logical = end;
            }
        }
        Ok(Self {
            file,
            record_bytes,
            remaining: extent.len,
            skip: extent.offset - logical,
            payload: Vec::new(),
            frame: Vec::new(),
            pos: 0,
            poisoned: None,
        })
    }

    fn load_frame(&mut self) -> io::Result<()> {
        let header = read_header(&mut self.file)?;
        validate_header(header, self.record_bytes)?;
        self.payload.resize(header.stored_len as usize, 0);
        self.file.read_exact(&mut self.payload).map_err(truncated)?;
        decode_frame(header, &self.payload, self.record_bytes, &mut self.frame)?;
        self.pos = usize::try_from(self.skip).unwrap_or(usize::MAX);
        self.skip = 0;
        if self.pos > self.frame.len() {
            return Err(invalid("spool frame ends before the requested offset"));
        }
        Ok(())
    }
}

fn truncated(error: io::Error) -> io::Error {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        invalid("spool file ends inside a frame")
    } else {
        error
    }
}

fn read_header(file: &mut BufReader<File>) -> io::Result<FrameHeader> {
    let mut bytes = [0; FRAME_HEADER_BYTES];
    file.read_exact(&mut bytes).map_err(truncated)?;
    Ok(FrameHeader::parse(bytes))
}

impl Read for FramedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(kind) = self.poisoned {
            return Err(kind.into());
        }
        if self.remaining == 0 || buf.is_empty() {
            return Ok(0);
        }
        while self.pos >= self.frame.len() {
            if let Err(error) = self.load_frame() {
                self.poisoned = Some(error.kind());
                return Err(error);
            }
        }
        let wanted = usize::try_from(self.remaining).unwrap_or(usize::MAX);
        let count = buf.len().min(self.frame.len() - self.pos).min(wanted);
        buf[..count].copy_from_slice(&self.frame[self.pos..self.pos + count]);
        self.pos += count;
        self.remaining -= count as u64;
        Ok(count)
    }
}

/// `SOURCE` is read raw, every other spool through its frames.
pub(crate) enum SpoolReader {
    Raw(io::Take<BufReader<File>>),
    Framed(FramedReader),
}

impl Read for SpoolReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Raw(reader) => reader.read(buf),
            Self::Framed(reader) => reader.read(buf),
        }
    }
}

/// Native store: `SOURCE` is `source.spool`, any other id `spool-{id}.spool`, under `root`.
///
/// `SOURCE` keeps one `BufWriter` from `create` to `close`. The others create their file at the
/// first `append` (`create_new`) and reopen it in append mode on each later one, so the number of
/// open descriptors never grows with the number of live spools. Encoding and writing happen outside
/// the `open` mutex, so concurrent workers compress in parallel.
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
    pub fn with_closed_source(
        root: &Path,
        source_len: u64,
        record_bytes: usize,
        diagnostics: bool,
    ) -> Self {
        let store = Self {
            root: root.to_owned(),
            ledger: diagnostics.then(|| SpoolLedger::with_live(source_len)),
            open: Mutex::new(OpenSpools::default()),
        };
        let len = fs::metadata(store.path(SpoolId::SOURCE)).map_or(source_len, |meta| meta.len());
        let mut entry = FileEntry::new(Phase::Closed, true, len, record_bytes);
        entry.physical_len = len;
        lock(&store.open).entries.insert(SpoolId::SOURCE, entry);
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
    type Reader = SpoolReader;

    fn create(&self, id: SpoolId, record_bytes: usize) -> Result<(), ConvertError> {
        if record_bytes == 0 {
            return Err(lifecycle_error(
                id,
                "create",
                "record size must be positive",
            ));
        }
        let mut open = lock(&self.open);
        if open.entries.contains_key(&id) {
            return Err(lifecycle_error(id, "create", "id was already used"));
        }
        let writer = if id == SpoolId::SOURCE {
            Some(BufWriter::new(File::create(self.path(id))?))
        } else {
            None
        };
        let mut entry = FileEntry::new(Phase::Open, writer.is_some(), 0, record_bytes);
        entry.writer = writer;
        open.entries.insert(id, entry);
        Ok(())
    }

    fn append(&self, id: SpoolId, bytes: &[u8]) -> Result<(), ConvertError> {
        let added = bytes.len() as u64;
        let (cursor, writer) = {
            let mut open = lock(&self.open);
            let entry = entry_in(&mut open.entries, id, "append", Phase::Open, |e| e.phase)?;
            ensure_sound(entry, id, "append")?;
            if bytes.is_empty() {
                return Ok(());
            }
            entry
                .len
                .checked_add(added)
                .ok_or(ConvertError::SizeOverflow)?;
            if id == SpoolId::SOURCE {
                (None, entry.writer.take())
            } else {
                let logical = entry.len - entry.carry.len() as u64;
                let cursor = FrameCursor {
                    first: !entry.created,
                    record_bytes: entry.record_bytes,
                    carry: std::mem::take(&mut entry.carry),
                    logical,
                    physical: entry.physical_len,
                    frames: entry.frames,
                };
                (Some(cursor), None)
            }
        };
        // The entry is updated only after the write, so a failure leaves nothing half counted.
        let result = if let Some(cursor) = cursor {
            let before = cursor.physical;
            write_frames(&self.path(id), cursor, bytes).map(|done| (Some(done), before))
        } else {
            let mut writer = writer.expect("the SOURCE writer lives from create to close");
            let result = writer.write_all(bytes);
            lock(&self.open)
                .entries
                .get_mut(&id)
                .expect("the entry was found above")
                .writer = Some(writer);
            result.map(|()| (None, 0))
        };
        let mut open = lock(&self.open);
        let entry = open
            .entries
            .get_mut(&id)
            .expect("the entry was found above");
        let physical_added = match result {
            Ok((Some(done), before)) => {
                entry.physical_len = done.physical;
                entry.frames = done.frames;
                entry.sparse_index.extend(done.index);
                entry.carry = done.carry;
                entry.created = true;
                done.physical - before
            }
            Ok((None, _)) => {
                entry.physical_len += added;
                added
            }
            Err(error) => {
                entry.failed = true;
                // The file may exist even though the first write failed.
                entry.created = entry.created || self.path(id).exists();
                return Err(error.into());
            }
        };
        entry.len += added;
        drop(open);
        if let Some(ledger) = &self.ledger {
            ledger.add(added);
            ledger.add_physical(physical_added);
        }
        Ok(())
    }

    fn close(&self, id: SpoolId) -> Result<(), ConvertError> {
        let writer = {
            let mut open = lock(&self.open);
            let entry = entry_in(&mut open.entries, id, "close", Phase::Open, |e| e.phase)?;
            ensure_sound(entry, id, "close")?;
            if !entry.carry.is_empty() {
                return Err(lifecycle_error(
                    id,
                    "close",
                    "length is not a whole number of records",
                ));
            }
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
        let entry = entry_in(&mut open.entries, id, "len", Phase::Closed, |e| e.phase)?;
        ensure_sound(entry, id, "len")?;
        Ok(entry.len)
    }

    /// The extent is checked against the length recorded by `append` (like `MemSpoolStore`), so
    /// a planner bug is rejected here. A raw spool truncated on disk still ends the read early and
    /// the caller reports `PointCountMismatch`; a framed one fails with `InvalidData`.
    fn reader(&self, id: SpoolId, extent: SpoolExtent) -> Result<Self::Reader, ConvertError> {
        let (record_bytes, start) = {
            let mut open = lock(&self.open);
            let entry = entry_in(&mut open.entries, id, "read", Phase::Closed, |e| e.phase)?;
            ensure_sound(entry, id, "read")?;
            if extent
                .offset
                .checked_add(extent.len)
                .is_none_or(|end| end > entry.len)
            {
                return Err(lifecycle_error(id, "read", "extent is out of range"));
            }
            let after = entry
                .sparse_index
                .partition_point(|&(logical, _)| logical <= extent.offset);
            let start = after
                .checked_sub(1)
                .map_or((0, 0), |at| entry.sparse_index[at]);
            (entry.record_bytes, start)
        };
        let path = self.path(id);
        if id == SpoolId::SOURCE {
            let mut file = File::open(path)?;
            if extent.offset > 0 {
                file.seek(SeekFrom::Start(extent.offset))?;
            }
            return Ok(SpoolReader::Raw(
                BufReader::with_capacity(SPOOL_IO_BUFFER_BYTES, file).take(extent.len),
            ));
        }
        Ok(SpoolReader::Framed(FramedReader::open(
            &path,
            record_bytes,
            start,
            extent,
        )?))
    }

    fn delete(&self, id: SpoolId) -> Result<(), ConvertError> {
        let (created, len, physical) = {
            let mut open = lock(&self.open);
            // A spool whose write failed never closes; it can still be deleted.
            if let Some(entry) = open.entries.get_mut(&id).filter(|e| e.failed) {
                entry.phase = Phase::Closed;
            }
            let entry = entry_in(&mut open.entries, id, "delete", Phase::Closed, |e| e.phase)?;
            entry.phase = Phase::Deleted;
            drop(std::mem::take(&mut entry.carry));
            drop(std::mem::take(&mut entry.sparse_index));
            (entry.created, entry.len, entry.physical_len)
        };
        if created {
            fs::remove_file(self.path(id))?;
        }
        if let Some(ledger) = &self.ledger {
            ledger.remove(len);
            ledger.remove_physical(physical);
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
        store.create(SpoolId::SOURCE, 1).unwrap();
        store.append(SpoolId::SOURCE, b"source").unwrap();
        store.close(SpoolId::SOURCE).unwrap();
        store.create(SpoolId(7), 1).unwrap();
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
        assert_eq!(
            fs::metadata(dir.path().join("spool-7.spool"))
                .unwrap()
                .len(),
            20
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
        assert!(message(store.create(SpoolId(7), 1)).contains("already used"));
        assert_eq!(
            store.ledger_totals(),
            Some(LedgerTotals {
                written: 10,
                peak_live: 10,
                // Source raw (6) + two 2-byte raw frames of 10 bytes each.
                physical_written: 26,
                physical_peak_live: 26,
            })
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn create_rejects_a_zero_record_size() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), false);
        assert!(message(store.create(SpoolId(3), 0)).contains("record size"));
    }

    #[test]
    fn file_store_resumes_from_a_closed_source() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("source.spool"), b"12345").unwrap();
        let store = FileSpoolStore::with_closed_source(dir.path(), 5, 1, true);
        assert_eq!(store.len(SpoolId::SOURCE).unwrap(), 5);
        assert!(message(store.create(SpoolId::SOURCE, 1)).contains("already used"));
        store.delete(SpoolId::SOURCE).unwrap();
        assert!(!dir.path().join("source.spool").exists());
    }

    const RECORD: usize = 42;

    /// Compressible records: a counter in the first bytes, constant tail.
    fn records(count: usize) -> Vec<u8> {
        let mut data = Vec::with_capacity(count * RECORD);
        for index in 0..count {
            let mut record = [0x5A_u8; RECORD];
            record[..8].copy_from_slice(&(index as u64 * 3).to_le_bytes());
            data.extend_from_slice(&record);
        }
        data
    }

    /// Logical start of every frame, read from the file's headers.
    fn frame_starts(path: &Path) -> Vec<usize> {
        let bytes = fs::read(path).unwrap();
        let (mut at, mut logical, mut starts) = (0, 0, Vec::new());
        while at < bytes.len() {
            let header = FrameHeader::parse(bytes[at..at + 8].try_into().unwrap());
            starts.push(logical);
            logical += header.logical_len as usize;
            at += 8 + header.stored_len as usize;
        }
        starts
    }

    fn framed(store: &FileSpoolStore, id: u64, data: &[u8], chunk: usize) -> SpoolId {
        let id = SpoolId(id);
        store.create(id, RECORD).unwrap();
        for part in data.chunks(chunk) {
            store.append(id, part).unwrap();
        }
        store.close(id).unwrap();
        id
    }

    fn read(store: &FileSpoolStore, id: SpoolId, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        store
            .reader(id, SpoolExtent { offset, len })
            .unwrap()
            .read_to_end(&mut out)?;
        Ok(out)
    }

    #[test]
    fn framed_spool_round_trips_unaligned_appends() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), false);
        let data = records(20_000);
        let id = framed(&store, 1, &data, 65_536);
        assert_eq!(store.len(id).unwrap(), data.len() as u64);
        assert_eq!(read(&store, id, 0, data.len() as u64).unwrap(), data);
        assert!(
            fs::metadata(dir.path().join("spool-1.spool"))
                .unwrap()
                .len()
                < data.len() as u64
        );
    }

    #[test]
    fn framed_reads_honour_offsets_inside_and_across_frames() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), false);
        // More than 64 frames, so the sparse index has several entries.
        let count = MAX_FRAME_RECORDS * 70 + 13;
        let data = records(count);
        let id = framed(&store, 1, &data, 65_536);
        assert!(lock(&store.open).entries[&id].sparse_index.len() >= 2);
        let end = data.len();
        // Exact sparse-index boundaries, the byte before them and a frame start that is not one.
        let index: Vec<usize> = lock(&store.open).entries[&id]
            .sparse_index
            .iter()
            .map(|&(logical, _)| logical as usize)
            .collect();
        assert!(
            index.len() >= 3 && index[1].is_multiple_of(RECORD),
            "{index:?}"
        );
        let mut boundary_cases = Vec::new();
        for &at in &index[1..3] {
            boundary_cases.push((at, RECORD * 3));
            boundary_cases.push((at - RECORD, RECORD * 3));
            boundary_cases.push((at - 1, RECORD + 2));
        }
        let starts = frame_starts(&dir.path().join("spool-1.spool"));
        for frame in [1, 65] {
            boundary_cases.push((starts[frame], RECORD * 5));
            boundary_cases.push((starts[frame] - RECORD, RECORD * 5));
        }
        let cases = boundary_cases.into_iter().chain([
            (0, 42 * 100),
            (42 * 100, 42 * 5),
            (42 * 8191, 42 * 3),
            (42 * 8192 + 42 * 5, 42 * 8192 * 2 + 17),
            (42 * 8192 * 64 + 42, 42 * 8192 + 42 * 9),
            (42 * 8192 * 64 + 1, 100),
            (end - 42, 42),
            (end - 42 * 8192 - 42 * 13, 42 * 8192 + 42 * 13),
            (end, 0),
            (7, 0),
        ]);
        for (offset, len) in cases {
            let got = read(&store, id, offset as u64, len as u64).unwrap();
            assert_eq!(got, data[offset..offset + len], "{offset}+{len}");
        }
    }

    #[test]
    fn ledger_counts_physical_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), true);
        let data = records(5_000);
        let id = framed(&store, 1, &data, 65_536);
        let totals = store.ledger_totals().unwrap();
        assert_eq!(totals.written, data.len() as u64);
        assert!(totals.physical_written < totals.written);
        assert_eq!(
            totals.physical_written,
            fs::metadata(dir.path().join("spool-1.spool"))
                .unwrap()
                .len()
        );
        let ledger = store.ledger.as_ref().unwrap();
        assert_eq!(ledger.live.load(Ordering::Relaxed), data.len() as u64);
        assert_eq!(
            ledger.physical_live.load(Ordering::Relaxed),
            totals.physical_written
        );
        store.delete(id).unwrap();
        assert_eq!(ledger.live.load(Ordering::Relaxed), 0);
        assert_eq!(ledger.physical_live.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn deleted_spools_release_their_frame_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), false);
        let id = framed(&store, 1, &records(MAX_FRAME_RECORDS * 3), 1 << 20);
        assert!(!lock(&store.open).entries[&id].sparse_index.is_empty());
        store.delete(id).unwrap();
        let open = lock(&store.open);
        assert_eq!(open.entries[&id].carry.capacity(), 0);
        assert_eq!(open.entries[&id].sparse_index.capacity(), 0);
    }

    #[test]
    fn close_rejects_a_partial_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), false);
        store.create(SpoolId(1), RECORD).unwrap();
        store.append(SpoolId(1), &[1; 10]).unwrap();
        assert!(message(store.close(SpoolId(1))).contains("whole number of records"));
    }

    #[test]
    fn source_spool_stays_raw() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), true);
        let data = records(1_000);
        store.create(SpoolId::SOURCE, RECORD).unwrap();
        store.append(SpoolId::SOURCE, &data).unwrap();
        store.close(SpoolId::SOURCE).unwrap();
        assert_eq!(fs::read(dir.path().join("source.spool")).unwrap(), data);
        assert_eq!(
            read(&store, SpoolId::SOURCE, 42, 84).unwrap(),
            data[42..126]
        );
        let totals = store.ledger_totals().unwrap();
        assert_eq!(totals.physical_written, totals.written);
    }

    #[test]
    fn truncated_spool_reports_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), false);
        let data = records(MAX_FRAME_RECORDS * 4);
        let id = framed(&store, 1, &data, 1 << 20);
        let path = dir.path().join("spool-1.spool");
        let size = fs::metadata(&path).unwrap().len();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(size / 2)
            .unwrap();
        let error = read(&store, id, 0, data.len() as u64).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        // A start offset past the surviving frames fails when the reader is opened.
        let opened = store.reader(
            id,
            SpoolExtent {
                offset: (data.len() - RECORD) as u64,
                len: RECORD as u64,
            },
        );
        assert!(opened.is_err());
    }

    #[test]
    fn concurrent_readers_of_one_spool_agree() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), false);
        let data = records(MAX_FRAME_RECORDS * 5 + 77);
        let id = framed(&store, 1, &data, 65_536);
        std::thread::scope(|scope| {
            for thread in 0..4_usize {
                let (store, data) = (&store, &data);
                scope.spawn(move || {
                    let offset = RECORD * (thread * 9_000 + 5);
                    let len = RECORD * 10_000;
                    let got = read(store, id, offset as u64, len as u64).unwrap();
                    assert_eq!(got, data[offset..offset + len]);
                });
            }
        });
    }

    #[test]
    fn a_failed_write_leaves_the_spool_deletable_and_the_ledger_balanced() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), true);
        let data = records(2_000);
        let id = SpoolId(1);
        store.create(id, RECORD).unwrap();
        store.append(id, &data[..RECORD * 1_000]).unwrap();
        let path = dir.path().join("spool-1.spool");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        if File::options().append(true).open(&path).is_ok() {
            return; // permissions are not enforced (running as root)
        }
        assert!(store.append(id, &data[RECORD * 1_000..]).is_err());
        assert!(message(store.append(id, b"x")).contains("earlier write failed"));
        assert!(message(store.close(id)).contains("earlier write failed"));
        store.delete(id).unwrap();
        assert!(!path.exists());
        let ledger = store.ledger.as_ref().unwrap();
        assert_eq!(ledger.live.load(Ordering::Relaxed), 0);
        assert_eq!(ledger.physical_live.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_corrupt_header_in_a_skipped_frame_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), false);
        let data = records(MAX_FRAME_RECORDS * 3);
        let id = framed(&store, 1, &data, 1 << 20);
        let path = dir.path().join("spool-1.spool");
        let mut bytes = fs::read(&path).unwrap();
        // The first frame is walked over to reach an offset in the second.
        let bad = u32::try_from(RECORD * 7 + 1).unwrap();
        bytes[..4].copy_from_slice(&bad.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        let opened = store.reader(
            id,
            SpoolExtent {
                offset: (RECORD * (MAX_FRAME_RECORDS + 5)) as u64,
                len: RECORD as u64,
            },
        );
        assert!(
            opened
                .err()
                .unwrap()
                .to_string()
                .contains("record multiple")
        );
    }

    #[test]
    fn a_reader_that_failed_keeps_failing() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), false);
        let data = records(MAX_FRAME_RECORDS * 3);
        let id = framed(&store, 1, &data, 1 << 20);
        let path = dir.path().join("spool-1.spool");
        let size = fs::metadata(&path).unwrap().len();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(size / 2)
            .unwrap();
        let mut reader = store
            .reader(
                id,
                SpoolExtent {
                    offset: 0,
                    len: data.len() as u64,
                },
            )
            .unwrap();
        let mut buf = vec![0; 1 << 20];
        let error = loop {
            if let Err(error) = reader.read(&mut buf) {
                break error;
            }
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(reader.read(&mut buf).is_err());
    }

    #[test]
    fn concurrent_appends_to_distinct_spools_read_back_identically() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSpoolStore::new(dir.path(), true);
        let datas: Vec<Vec<u8>> = (0..4).map(|n| records(20_000 + n * 1_001)).collect();
        std::thread::scope(|scope| {
            for (n, data) in datas.iter().enumerate() {
                let store = &store;
                scope.spawn(move || {
                    let chunk = if n == 0 { 65_536 } else { RECORD * 777 };
                    framed(store, n as u64 + 1, data, chunk);
                });
            }
        });
        for (n, data) in datas.iter().enumerate() {
            let id = SpoolId(n as u64 + 1);
            assert_eq!(read(&store, id, 0, data.len() as u64).unwrap(), *data);
        }
        let totals = store.ledger_totals().unwrap();
        assert_eq!(
            totals.written,
            datas.iter().map(|d| d.len() as u64).sum::<u64>()
        );
    }
}
