use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use viewer_format::{RowOrder, block_checkpoint_versions, crc32};

use crate::{
    CheckpointError, CheckpointMode, ConversionWorkerStats, ConvertOptions,
    checkpoint::fingerprint_file, fs_util::sync_parent,
};

const MANIFEST_SCHEMA: u32 = 1;
const EPT_PLAN_VERSION: u32 = 1;
const MAX_MANIFEST: u64 = 16 * 1024 * 1024;
const MAX_RECORD: u64 = 64 * 1024 * 1024;
const MAX_SOURCE_PATH: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EptSourceFile {
    pub path: String,
    pub bytes: u64,
    pub mtime_secs: i64,
    pub mtime_nanos: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EptJournalRecord {
    pub ordinal: u64,
    pub node_index: u64,
    pub node_point_offset: u64,
    pub processed_points: u64,
    pub primary_len: u64,
    pub attributes_len: u64,
    pub writer_record: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum EptCheckpointStage {
    Started,
    Writing,
    OutputReady,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Options {
    version: String,
    points_per_block: u64,
    quantization_bits: u64,
    memory: u64,
    workers: usize,
    workers_explicit: bool,
    divisions: u8,
    pipelined: bool,
    // Journals written before the option existed were source order, hence the default; this
    // keeps them resumable as source and rejects them as hybrid without a schema bump.
    #[serde(default = "default_row_order")]
    row_order: String,
}
fn default_row_order() -> String {
    RowOrder::Source.as_str().into()
}
impl From<ConvertOptions> for Options {
    fn from(o: ConvertOptions) -> Self {
        Self {
            version: o.format_version().to_string(),
            points_per_block: o.points_per_block(),
            quantization_bits: o.max_quantization_error().to_bits(),
            memory: o.max_memory_budget_bytes(),
            workers: o.spatial_workers(),
            workers_explicit: o.spatial_workers_explicit(),
            divisions: o.spatial_divisions(),
            pipelined: o.pipelined_writes(),
            row_order: o.row_order().as_str().into(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    schema: u32,
    plan_version: u32,
    output_version: String,
    options: Options,
    sources: Option<SourceCatalog>,
    stage: EptCheckpointStage,
    journal_len: u64,
    records: u64,
    points: u64,
    primary_len: u64,
    attributes_len: u64,
    ready: Option<Ready>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct SourceCatalog {
    count: u64,
    bytes: u64,
    fingerprint: Fingerprint,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Ready {
    primary: Fingerprint,
    attributes: Fingerprint,
    point_count: u64,
    block_count: u64,
    output_validation: StoredWorkerStats,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct StoredWorkerStats {
    worker_limit: usize,
    estimated_bytes_per_worker: u64,
}
impl From<ConversionWorkerStats> for StoredWorkerStats {
    fn from(value: ConversionWorkerStats) -> Self {
        Self {
            worker_limit: value.worker_limit,
            estimated_bytes_per_worker: value.estimated_bytes_per_worker,
        }
    }
}
impl From<StoredWorkerStats> for ConversionWorkerStats {
    fn from(value: StoredWorkerStats) -> Self {
        Self {
            worker_limit: value.worker_limit,
            estimated_bytes_per_worker: value.estimated_bytes_per_worker,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Fingerprint {
    bytes: u64,
    hash: u128,
}
impl From<crate::checkpoint::FileFingerprint> for Fingerprint {
    fn from(f: crate::checkpoint::FileFingerprint) -> Self {
        Self {
            bytes: f.bytes,
            hash: f.hash,
        }
    }
}

pub(crate) struct EptCheckpoint {
    root: PathBuf,
    manifest: Manifest,
    lock: Option<File>,
}

impl EptCheckpoint {
    pub(crate) fn open(
        output_path: &Path,
        mode: CheckpointMode,
        options: ConvertOptions,
    ) -> Result<Self, CheckpointError> {
        if !options.format_version().supports_block_checkpoint() {
            let supported: Vec<String> = block_checkpoint_versions()
                .map(|version| version.to_string())
                .collect();
            let message = match supported.split_last() {
                Some((last, rest)) if !rest.is_empty() => {
                    format!("EPT recovery supports only {}, and {last}", rest.join(", "))
                }
                Some((only, _)) => format!("EPT recovery supports only {only}"),
                None => "EPT recovery is not supported by any output revision".to_owned(),
            };
            return Err(CheckpointError::Invalid(message));
        }
        let root = checkpoint_path(output_path);
        match mode {
            CheckpointMode::Create => match fs::create_dir(&root) {
                Ok(()) => sync_parent(&root)?,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    return Err(CheckpointError::AlreadyExists(root));
                }
                Err(e) => return Err(e.into()),
            },
            CheckpointMode::Resume if !root.is_dir() => return Err(CheckpointError::Missing(root)),
            CheckpointMode::Resume => (),
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("lock"))?;
        match lock.try_lock() {
            Ok(()) => (),
            Err(std::fs::TryLockError::WouldBlock) => return Err(CheckpointError::Locked),
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
        let mp = root.join("manifest.json");
        let manifest = match mode {
            CheckpointMode::Create => Manifest {
                schema: MANIFEST_SCHEMA,
                plan_version: EPT_PLAN_VERSION,
                output_version: options.format_version().to_string(),
                options: options.into(),
                sources: None,
                stage: EptCheckpointStage::Started,
                journal_len: 0,
                records: 0,
                points: 0,
                primary_len: 0,
                attributes_len: 0,
                ready: None,
            },
            CheckpointMode::Resume => {
                let m = read_manifest(&mp)?;
                if m.schema != MANIFEST_SCHEMA
                    || m.plan_version != EPT_PLAN_VERSION
                    || m.output_version != options.format_version().to_string()
                    || m.options != Options::from(options)
                {
                    return Err(CheckpointError::Mismatch);
                }
                m
            }
        };
        let mut c = Self {
            root,
            manifest,
            lock: Some(lock),
        };
        if mode == CheckpointMode::Create {
            c.persist()?;
        }
        Ok(c)
    }
    pub(crate) fn candidate_path(&self) -> PathBuf {
        self.root.join("candidate.oct")
    }
    pub(crate) fn candidate_attributes_path(&self) -> PathBuf {
        self.root.join("candidate.octattrs")
    }
    pub(crate) fn journal_path(&self) -> PathBuf {
        self.root.join("journal.bin")
    }
    pub(crate) fn stage(&self) -> EptCheckpointStage {
        self.manifest.stage
    }
    pub(crate) fn bind_source_files(
        &mut self,
        mut files: Vec<EptSourceFile>,
    ) -> Result<(), CheckpointError> {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        if files.windows(2).any(|w| w[0].path == w[1].path) {
            return Err(invalid("duplicate source path in catalog"));
        }
        if let Some(old) = self.manifest.sources {
            let (catalog, decoded) = compare_source_catalog(&self.sources_path(), &files)?;
            if catalog != old || !decoded {
                return Err(CheckpointError::Mismatch);
            }
        } else {
            let catalog = write_source_catalog(&self.root, &files)?;
            self.manifest.sources = Some(catalog);
            self.persist()?;
        }
        Ok(())
    }
    fn sources_path(&self) -> PathBuf {
        self.root.join("sources.bin")
    }
    pub(crate) fn start_writing(
        &mut self,
        primary_len: u64,
        attributes_len: u64,
    ) -> Result<(), CheckpointError> {
        if self.manifest.stage == EptCheckpointStage::OutputReady {
            return Err(invalid("cannot restart output-ready checkpoint"));
        }
        check_file_len(&self.candidate_path(), primary_len)?;
        check_file_len(&self.candidate_attributes_path(), attributes_len)?;
        sync_file(&self.candidate_path())?;
        sync_file(&self.candidate_attributes_path())?;
        self.manifest.stage = EptCheckpointStage::Writing;
        self.manifest.primary_len = primary_len;
        self.manifest.attributes_len = attributes_len;
        self.persist()
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the journal record is consumed by the append"
    )]
    pub(crate) fn append_committed_block(
        &mut self,
        record: EptJournalRecord,
    ) -> Result<(), CheckpointError> {
        if self.manifest.stage != EptCheckpointStage::Writing {
            return Err(invalid("checkpoint is not writing"));
        }
        if record.ordinal != self.manifest.records {
            return Err(invalid("journal ordinal mismatch"));
        }
        if record.writer_record.len() as u64 > MAX_RECORD - 52 {
            return Err(invalid("journal record too large"));
        }
        if record.primary_len < self.manifest.primary_len
            || record.attributes_len < self.manifest.attributes_len
            || record.processed_points == 0
        {
            return Err(invalid("invalid committed lengths or point count"));
        }
        check_file_len(&self.candidate_path(), record.primary_len)?;
        check_file_len(&self.candidate_attributes_path(), record.attributes_len)?;
        sync_file(&self.candidate_path())?;
        sync_file(&self.candidate_attributes_path())?;
        let payload = encode_record(&record)?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.journal_path())?;
        f.write_all(&(payload.len() as u32).to_le_bytes())?;
        f.write_all(&payload)?;
        f.write_all(&crc32(&payload).to_le_bytes())?;
        f.sync_all()?;
        self.manifest.journal_len = self
            .manifest
            .journal_len
            .checked_add(8 + payload.len() as u64)
            .ok_or_else(|| invalid("journal length overflow"))?;
        self.manifest.records = self
            .manifest
            .records
            .checked_add(1)
            .ok_or_else(|| invalid("record count overflow"))?;
        self.manifest.points = self
            .manifest
            .points
            .checked_add(record.processed_points)
            .ok_or_else(|| invalid("point count overflow"))?;
        self.manifest.primary_len = record.primary_len;
        self.manifest.attributes_len = record.attributes_len;
        self.persist()
    }
    pub(crate) fn for_each_committed_record(
        &self,
        mut callback: impl FnMut(EptJournalRecord) -> Result<(), CheckpointError>,
    ) -> Result<(), CheckpointError> {
        if self.manifest.records == 0 {
            return if self.manifest.journal_len == 0 {
                Ok(())
            } else {
                Err(invalid("empty journal has a committed length"))
            };
        }
        let mut f = File::open(self.journal_path())?;
        let mut consumed = 0u64;
        let mut points = 0u64;
        let mut last_lengths = None;
        for ordinal in 0..self.manifest.records {
            let mut lb = [0; 4];
            f.read_exact(&mut lb)?;
            let len = u64::from(u32::from_le_bytes(lb));
            if len > MAX_RECORD {
                return Err(invalid("journal record too large"));
            }
            let next = consumed
                .checked_add(8 + len)
                .ok_or_else(|| invalid("journal length overflow"))?;
            if next > self.manifest.journal_len {
                return Err(invalid("committed journal prefix truncated"));
            }
            let mut payload = vec![0; len as usize];
            f.read_exact(&mut payload)?;
            let mut cb = [0; 4];
            f.read_exact(&mut cb)?;
            if u32::from_le_bytes(cb) != crc32(&payload) {
                return Err(invalid("journal CRC mismatch"));
            }
            let record = decode_record(&payload)?;
            if record.ordinal != ordinal {
                return Err(invalid("journal ordinal mismatch"));
            }
            if record.processed_points == 0
                || last_lengths.is_some_and(|(primary, attributes)| {
                    record.primary_len < primary || record.attributes_len < attributes
                })
            {
                return Err(invalid("invalid journal counters or candidate lengths"));
            }
            points = points
                .checked_add(record.processed_points)
                .ok_or_else(|| invalid("journal point count overflow"))?;
            if points > self.manifest.points {
                return Err(invalid("journal point count exceeds manifest"));
            }
            last_lengths = Some((record.primary_len, record.attributes_len));
            callback(record)?;
            consumed = next;
        }
        if consumed != self.manifest.journal_len
            || points != self.manifest.points
            || last_lengths.unwrap_or((0, 0))
                != (self.manifest.primary_len, self.manifest.attributes_len)
        {
            return Err(invalid("committed journal length mismatch"));
        }
        Ok(())
    }
    pub(crate) fn truncate_uncommitted_artifacts(&self) -> Result<(), CheckpointError> {
        if self.manifest.stage == EptCheckpointStage::OutputReady {
            return Err(invalid("cannot truncate output-ready candidates"));
        }
        truncate_checked(&self.candidate_path(), self.manifest.primary_len)?;
        truncate_checked(
            &self.candidate_attributes_path(),
            self.manifest.attributes_len,
        )?;
        truncate_checked(&self.journal_path(), self.manifest.journal_len)?;
        Ok(())
    }
    pub(crate) fn mark_output_ready(
        &mut self,
        point_count: u64,
        block_count: u64,
        output_validation: ConversionWorkerStats,
    ) -> Result<(), CheckpointError> {
        if point_count != self.manifest.points || block_count != self.manifest.records {
            return Err(invalid("output counters do not match committed journal"));
        }
        self.manifest.ready = Some(Ready {
            primary: fingerprint_file(&self.candidate_path())?.into(),
            attributes: fingerprint_file(&self.candidate_attributes_path())?.into(),
            point_count,
            block_count,
            output_validation: output_validation.into(),
        });
        self.manifest.stage = EptCheckpointStage::OutputReady;
        self.persist()
    }
    pub(crate) fn output_ready_details(&self) -> Option<(u64, u64, ConversionWorkerStats)> {
        self.manifest
            .ready
            .as_ref()
            .map(|r| (r.point_count, r.block_count, r.output_validation.into()))
    }
    pub(crate) fn verify_output_ready(&self) -> Result<(), CheckpointError> {
        let r = self
            .manifest
            .ready
            .as_ref()
            .ok_or_else(|| invalid("output is not ready"))?;
        if Fingerprint::from(fingerprint_file(&self.candidate_path())?) != r.primary
            || Fingerprint::from(fingerprint_file(&self.candidate_attributes_path())?)
                != r.attributes
        {
            return Err(invalid("completed EPT candidate is missing or corrupt"));
        }
        Ok(())
    }
    pub(crate) fn complete(mut self) -> Result<(), CheckpointError> {
        self.lock.take();
        fs::remove_dir_all(&self.root).map_err(|source| CheckpointError::Cleanup {
            path: self.root.clone(),
            source,
        })?;
        sync_parent(&self.root)?;
        Ok(())
    }
    fn persist(&mut self) -> Result<(), CheckpointError> {
        let path = self.root.join("manifest.json");
        let tmp = self.root.join("manifest.json.tmp");
        let b = serde_json::to_vec(&self.manifest).map_err(|e| invalid(e.to_string()))?;
        let mut f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)?;
        f.write_all(&b)?;
        f.sync_all()?;
        drop(f);
        fs::rename(tmp, path.clone())?;
        sync_parent(&path)?;
        Ok(())
    }
}
impl Drop for EptCheckpoint {
    fn drop(&mut self) {
        self.lock.take();
    }
}

fn read_manifest(path: &Path) -> Result<Manifest, CheckpointError> {
    let meta = fs::metadata(path).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            invalid("manifest is missing")
        } else {
            e.into()
        }
    })?;
    if meta.len() > MAX_MANIFEST {
        return Err(invalid("manifest too large"));
    }
    let b = fs::read(path)?;
    serde_json::from_slice(&b).map_err(|e| invalid(format!("invalid manifest: {e}")))
}
fn write_source_catalog(
    root: &Path,
    files: &[EptSourceFile],
) -> Result<SourceCatalog, CheckpointError> {
    let temporary = root.join("sources.bin.tmp");
    let path = root.join("sources.bin");
    let mut out = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    for source in files {
        let path_bytes = source.path.as_bytes();
        if path_bytes.len() > MAX_SOURCE_PATH {
            return Err(invalid("source path too long"));
        }
        let path_len =
            u32::try_from(path_bytes.len()).map_err(|_| invalid("source path length overflow"))?;
        let record_len = 24u64
            .checked_add(path_bytes.len() as u64)
            .ok_or_else(|| invalid("source record length overflow"))?;
        if record_len > MAX_RECORD {
            return Err(invalid("source record too large"));
        }
        let mut record = Vec::with_capacity(record_len as usize);
        record.extend_from_slice(&path_len.to_le_bytes());
        record.extend_from_slice(&source.bytes.to_le_bytes());
        record.extend_from_slice(&source.mtime_secs.to_le_bytes());
        record.extend_from_slice(&source.mtime_nanos.to_le_bytes());
        record.extend_from_slice(path_bytes);
        let len = u32::try_from(record.len()).map_err(|_| invalid("source record too large"))?;
        out.write_all(&len.to_le_bytes())?;
        out.write_all(&record)?;
        out.write_all(&crc32(&record).to_le_bytes())?;
    }
    out.sync_all()?;
    drop(out);
    fs::rename(&temporary, &path)?;
    sync_parent(&path)?;
    let fingerprint = fingerprint_file(&path)?;
    Ok(SourceCatalog {
        count: u64::try_from(files.len()).map_err(|_| invalid("source count overflow"))?,
        bytes: fingerprint.bytes,
        fingerprint: fingerprint.into(),
    })
}

fn compare_source_catalog(
    path: &Path,
    expected: &[EptSourceFile],
) -> Result<(SourceCatalog, bool), CheckpointError> {
    let fp = fingerprint_file(path).map_err(|e| match e {
        CheckpointError::Io(ref io) if io.kind() == io::ErrorKind::NotFound => {
            invalid("source catalog is missing")
        }
        other => other,
    })?;
    let catalog = SourceCatalog {
        count: u64::try_from(expected.len()).map_err(|_| invalid("source count overflow"))?,
        bytes: fp.bytes,
        fingerprint: fp.into(),
    };
    let mut input = File::open(path)?;
    let mut matches = true;
    let mut count = 0u64;
    let mut consumed = 0u64;
    loop {
        let mut len_bytes = [0u8; 4];
        match input.read_exact(&mut len_bytes) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                if consumed == fp.bytes {
                    break;
                }
                return Err(invalid("truncated source catalog length"));
            }
            Err(e) => return Err(e.into()),
        }
        let len = u64::from(u32::from_le_bytes(len_bytes));
        consumed = consumed
            .checked_add(4)
            .ok_or_else(|| invalid("source catalog length overflow"))?;
        if !(24..=MAX_RECORD).contains(&len) {
            return Err(invalid("invalid source catalog record length"));
        }
        let after = consumed
            .checked_add(len)
            .and_then(|n| n.checked_add(4))
            .ok_or_else(|| invalid("source catalog length overflow"))?;
        if after > fp.bytes {
            return Err(invalid("truncated source catalog record"));
        }
        let mut record = vec![0; len as usize];
        input.read_exact(&mut record)?;
        let mut crc = [0; 4];
        input.read_exact(&mut crc)?;
        consumed = consumed
            .checked_add(len + 4)
            .ok_or_else(|| invalid("source catalog length overflow"))?;
        if u32::from_le_bytes(crc) != crc32(&record) {
            return Err(invalid("source catalog CRC mismatch"));
        }
        let decoded = decode_source(&record)?;
        if expected.get(count as usize) != Some(&decoded) {
            matches = false;
        }
        count = count
            .checked_add(1)
            .ok_or_else(|| invalid("source count overflow"))?;
    }
    if count != expected.len() as u64 {
        matches = false;
    }
    Ok((catalog, matches))
}

fn decode_source(record: &[u8]) -> Result<EptSourceFile, CheckpointError> {
    if record.len() < 24 {
        return Err(invalid("truncated source catalog record"));
    }
    let path_len = u32::from_le_bytes(
        record[0..4]
            .try_into()
            .map_err(|_| invalid("invalid source path length"))?,
    ) as usize;
    if path_len > MAX_SOURCE_PATH
        || 24usize
            .checked_add(path_len)
            .ok_or_else(|| invalid("source path length overflow"))?
            != record.len()
    {
        return Err(invalid("invalid source path length"));
    }
    let bytes = u64::from_le_bytes(record[4..12].try_into().unwrap());
    let mtime_secs = i64::from_le_bytes(record[12..20].try_into().unwrap());
    let mtime_nanos = u32::from_le_bytes(record[20..24].try_into().unwrap());
    let path = std::str::from_utf8(&record[24..])
        .map_err(|_| invalid("source catalog path is not UTF-8"))?
        .to_owned();
    Ok(EptSourceFile {
        path,
        bytes,
        mtime_secs,
        mtime_nanos,
    })
}
fn checkpoint_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".checkpoint");
    PathBuf::from(s)
}
fn sync_file(p: &Path) -> Result<(), CheckpointError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(p)?
        .sync_all()?;
    Ok(())
}
fn truncate_checked(p: &Path, n: u64) -> Result<(), CheckpointError> {
    let f = OpenOptions::new().write(true).open(p)?;
    let len = f.metadata()?.len();
    if len < n {
        return Err(invalid("candidate shorter than committed length"));
    }
    f.set_len(n)?;
    f.sync_all()?;
    Ok(())
}
fn invalid(s: impl Into<String>) -> CheckpointError {
    CheckpointError::Invalid(s.into())
}
fn check_file_len(path: &Path, expected: u64) -> Result<(), CheckpointError> {
    if fs::metadata(path)?.len() != expected {
        return Err(invalid("candidate length does not match committed length"));
    }
    Ok(())
}
fn encode_record(r: &EptJournalRecord) -> Result<Vec<u8>, CheckpointError> {
    let writer_len =
        u32::try_from(r.writer_record.len()).map_err(|_| invalid("writer record too large"))?;
    let capacity = 52usize
        .checked_add(r.writer_record.len())
        .ok_or_else(|| invalid("journal record length overflow"))?;
    if capacity as u64 > MAX_RECORD {
        return Err(invalid("journal record too large"));
    }
    let mut b = Vec::with_capacity(capacity);
    for n in [
        r.ordinal,
        r.node_index,
        r.node_point_offset,
        r.processed_points,
        r.primary_len,
        r.attributes_len,
    ] {
        b.extend_from_slice(&n.to_le_bytes());
    }
    b.extend_from_slice(&writer_len.to_le_bytes());
    b.extend_from_slice(&r.writer_record);
    Ok(b)
}
fn decode_record(b: &[u8]) -> Result<EptJournalRecord, CheckpointError> {
    if b.len() < 52 || b.len() as u64 > MAX_RECORD {
        return Err(invalid("invalid journal record length"));
    }
    let read_u64 = |offset: usize| -> Result<u64, CheckpointError> {
        let end = offset
            .checked_add(8)
            .ok_or_else(|| invalid("journal field offset overflow"))?;
        let bytes: [u8; 8] = b
            .get(offset..end)
            .ok_or_else(|| invalid("truncated journal field"))?
            .try_into()
            .map_err(|_| invalid("truncated journal field"))?;
        Ok(u64::from_le_bytes(bytes))
    };
    let writer_len_bytes: [u8; 4] = b[48..52]
        .try_into()
        .map_err(|_| invalid("truncated writer record length"))?;
    let writer_len = u32::from_le_bytes(writer_len_bytes) as usize;
    let expected_len = 52usize
        .checked_add(writer_len)
        .ok_or_else(|| invalid("writer record length overflow"))?;
    if expected_len != b.len() {
        return Err(invalid("writer record length mismatch"));
    }
    Ok(EptJournalRecord {
        ordinal: read_u64(0)?,
        node_index: read_u64(8)?,
        node_point_offset: read_u64(16)?,
        processed_points: read_u64(24)?,
        primary_len: read_u64(32)?,
        attributes_len: read_u64(40)?,
        writer_record: b[52..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConvertOptions;
    use std::io::{Seek, SeekFrom};
    use viewer_format::FormatVersion;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
    fn options() -> ConvertOptions {
        ConvertOptions::default()
            .with_format_version(FormatVersion::V7_PRE1)
            .unwrap()
    }

    #[test]
    fn v72_checkpoint_resumes_only_with_its_exact_format_identity() {
        let tmp = dir();
        let p = tmp.path().join("out");
        let v72_options = ConvertOptions::default()
            .with_format_version(FormatVersion::V7_PRE2)
            .unwrap();
        let mut checkpoint = EptCheckpoint::open(&p, CheckpointMode::Create, v72_options).unwrap();
        checkpoint.bind_source_files(src()).unwrap();
        writing(&mut checkpoint);
        drop(checkpoint);

        let resumed = EptCheckpoint::open(&p, CheckpointMode::Resume, v72_options).unwrap();
        let mut records = 0;
        resumed
            .for_each_committed_record(|_| {
                records += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(records, 1);
        drop(resumed);
        assert!(matches!(
            EptCheckpoint::open(&p, CheckpointMode::Resume, options()),
            Err(CheckpointError::Mismatch)
        ));
    }
    fn src() -> Vec<EptSourceFile> {
        vec![EptSourceFile {
            path: "ept-data/0-0-0.laz".into(),
            bytes: 2,
            mtime_secs: 1,
            mtime_nanos: 0,
        }]
    }
    fn writing(c: &mut EptCheckpoint) {
        fs::write(c.candidate_path(), b"primary").unwrap();
        fs::write(c.candidate_attributes_path(), b"attrs").unwrap();
        c.start_writing(7, 5).unwrap();
        c.append_committed_block(EptJournalRecord {
            ordinal: 0,
            node_index: 0,
            node_point_offset: 0,
            processed_points: 2,
            primary_len: 7,
            attributes_len: 5,
            writer_record: vec![1, 2],
        })
        .unwrap();
    }
    #[test]
    fn resume_rejects_a_different_row_order() {
        let tmp = dir();
        let p = tmp.path().join("out");
        let v7 = |order| {
            ConvertOptions::default()
                .with_format_version(FormatVersion::V7_PRE2)
                .unwrap()
                .with_row_order(order)
                .unwrap()
        };
        let mut c = EptCheckpoint::open(&p, CheckpointMode::Create, v7(RowOrder::Source)).unwrap();
        c.bind_source_files(src()).unwrap();
        drop(c);
        assert!(matches!(
            EptCheckpoint::open(&p, CheckpointMode::Resume, v7(RowOrder::Hybrid)),
            Err(CheckpointError::Mismatch)
        ));
        EptCheckpoint::open(&p, CheckpointMode::Resume, v7(RowOrder::Source)).unwrap();
    }
    #[test]
    fn journals_without_row_order_resume_as_source_only() {
        let old: Options = serde_json::from_str(
            r#"{"version":"v7.1-pre","points_per_block":1,"quantization_bits":0,"memory":1,"workers":1,"workers_explicit":false,"divisions":1,"pipelined":false}"#,
        )
        .unwrap();
        assert_eq!(old.row_order, "source");
    }
    #[test]
    fn option_and_source_mismatch() {
        let tmp = dir();
        let p = tmp.path().join("out");
        let mut c = EptCheckpoint::open(&p, CheckpointMode::Create, options()).unwrap();
        c.bind_source_files(src()).unwrap();
        drop(c);
        assert!(matches!(
            EptCheckpoint::open(
                &p,
                CheckpointMode::Resume,
                ConvertOptions::default()
                    .with_format_version(FormatVersion::V7_PRE1)
                    .unwrap()
                    .with_spatial_divisions(4)
                    .unwrap()
            ),
            Err(CheckpointError::Mismatch)
        ));
        let mut c = EptCheckpoint::open(&p, CheckpointMode::Resume, options()).unwrap();
        assert!(matches!(
            c.bind_source_files(vec![]),
            Err(CheckpointError::Mismatch)
        ));
        c.bind_source_files(src()).unwrap();
    }
    #[test]
    fn source_catalog_detects_crc_corruption_and_truncation() {
        let tmp = dir();
        let p = tmp.path().join("out");
        let mut c = EptCheckpoint::open(&p, CheckpointMode::Create, options()).unwrap();
        c.bind_source_files(src()).unwrap();
        drop(c);
        let catalog = p.with_file_name("out.checkpoint").join("sources.bin");
        let original = fs::read(&catalog).unwrap();
        let mut corrupted = original.clone();
        corrupted[8] ^= 1;
        fs::write(&catalog, corrupted).unwrap();
        let mut c = EptCheckpoint::open(&p, CheckpointMode::Resume, options()).unwrap();
        assert!(c.bind_source_files(src()).is_err());
        drop(c);
        fs::write(&catalog, &original[..original.len() - 1]).unwrap();
        let mut c = EptCheckpoint::open(&p, CheckpointMode::Resume, options()).unwrap();
        assert!(c.bind_source_files(src()).is_err());
    }
    #[test]
    fn tails_are_ignored_until_truncate() {
        let tmp = dir();
        let p = tmp.path().join("out");
        let mut c = EptCheckpoint::open(&p, CheckpointMode::Create, options()).unwrap();
        c.bind_source_files(src()).unwrap();
        writing(&mut c);
        OpenOptions::new()
            .append(true)
            .open(c.journal_path())
            .unwrap()
            .write_all(&[1, 2, 3])
            .unwrap();
        OpenOptions::new()
            .append(true)
            .open(c.candidate_path())
            .unwrap()
            .write_all(b"tail")
            .unwrap();
        let mut n = 0;
        c.for_each_committed_record(|_| {
            n += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(n, 1);
        c.truncate_uncommitted_artifacts().unwrap();
        assert_eq!(fs::metadata(c.candidate_path()).unwrap().len(), 7);
        drop(c);
    }
    #[test]
    fn journal_tail_corruption_is_ignored_and_committed_corruption_is_error() {
        let tmp = dir();
        let p = tmp.path().join("out");
        let mut c = EptCheckpoint::open(&p, CheckpointMode::Create, options()).unwrap();
        c.bind_source_files(src()).unwrap();
        writing(&mut c);
        let journal = c.journal_path();
        let committed = fs::read(&journal).unwrap();
        let replay = |c: &EptCheckpoint| {
            let mut seen = Vec::new();
            c.for_each_committed_record(|r| {
                seen.push((r.ordinal, r.processed_points, r.writer_record));
                Ok(())
            })
            .map(|()| seen)
        };
        let baseline = replay(&c).unwrap();
        assert_eq!(baseline, vec![(0, 2, vec![1, 2])]);

        // The uncommitted tail is a well-formed record, as left by a crash between the
        // journal append and the manifest update; every prefix and every flipped byte of it
        // must be ignored.
        let tail = committed.clone();
        for length in 0..=tail.len() {
            let mut bytes = committed.clone();
            bytes.extend_from_slice(&tail[..length]);
            fs::write(&journal, &bytes).unwrap();
            assert_eq!(replay(&c).unwrap(), baseline, "tail prefix {length}");
        }
        for offset in 0..tail.len() {
            let mut bytes = committed.clone();
            bytes.extend_from_slice(&tail);
            bytes[committed.len() + offset] ^= 1 << (offset % 8);
            fs::write(&journal, &bytes).unwrap();
            assert_eq!(replay(&c).unwrap(), baseline, "tail flip {offset}");
            c.truncate_uncommitted_artifacts().unwrap();
            assert_eq!(fs::read(&journal).unwrap(), committed);
        }

        // Every byte of the committed record is covered by framing or CRC.
        for offset in 0..committed.len() {
            let mut bytes = committed.clone();
            bytes[offset] ^= 1 << (offset % 8);
            fs::write(&journal, &bytes).unwrap();
            assert!(replay(&c).is_err(), "committed flip {offset}");
        }
        // Dropping bytes from the committed prefix is also an error.
        for length in 0..committed.len() {
            fs::write(&journal, &committed[..length]).unwrap();
            assert!(replay(&c).is_err(), "committed truncation {length}");
        }
    }
    #[test]
    fn resumed_catalog_keeps_journal_replay_functional() {
        let tmp = dir();
        let p = tmp.path().join("out");
        let mut c = EptCheckpoint::open(&p, CheckpointMode::Create, options()).unwrap();
        c.bind_source_files(src()).unwrap();
        writing(&mut c);
        drop(c);
        let mut c = EptCheckpoint::open(&p, CheckpointMode::Resume, options()).unwrap();
        c.bind_source_files(src()).unwrap();
        let mut seen = Vec::new();
        c.for_each_committed_record(|r| {
            seen.push(r.ordinal);
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, vec![0]);
    }
    #[test]
    fn detects_corrupt_committed_record_and_short_candidate() {
        let tmp = dir();
        let p = tmp.path().join("out");
        let mut c = EptCheckpoint::open(&p, CheckpointMode::Create, options()).unwrap();
        c.bind_source_files(src()).unwrap();
        writing(&mut c);
        let mut f = OpenOptions::new()
            .write(true)
            .open(c.journal_path())
            .unwrap();
        f.seek(SeekFrom::Start(5)).unwrap();
        f.write_all(&[9]).unwrap();
        assert!(c.for_each_committed_record(|_| Ok(())).is_err());
        drop(c);
        let p2 = tmp.path().join("out2");
        let mut c = EptCheckpoint::open(&p2, CheckpointMode::Create, options()).unwrap();
        c.bind_source_files(src()).unwrap();
        writing(&mut c);
        OpenOptions::new()
            .write(true)
            .open(c.candidate_path())
            .unwrap()
            .set_len(1)
            .unwrap();
        assert!(c.truncate_uncommitted_artifacts().is_err());
    }
    #[test]
    fn output_ready_fingerprint_is_checked() {
        let tmp = dir();
        let p = tmp.path().join("out");
        let mut c = EptCheckpoint::open(&p, CheckpointMode::Create, options()).unwrap();
        c.bind_source_files(src()).unwrap();
        writing(&mut c);
        c.mark_output_ready(
            2,
            1,
            ConversionWorkerStats {
                worker_limit: 1,
                estimated_bytes_per_worker: 4096,
            },
        )
        .unwrap();
        fs::write(c.candidate_path(), b"changed").unwrap();
        assert!(c.verify_output_ready().is_err());
    }
}
