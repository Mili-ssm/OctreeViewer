//! Tokio-facing selective reader over shared positioned file handles.
//!
//! File I/O and decoding run on Tokio's blocking pool. Independent cursors use positional reads
//! on Unix; other platforms serialize only seek/read on the shared handle. The shared payload
//! budget covers planned logical buffers and retained `BudgetedRead` outputs, not metadata,
//! allocator/runtime overhead, caches, caller clones, or process RSS.

#[cfg(not(unix))]
use std::sync::Mutex;
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use viewer_core::BlockId;

use crate::{
    BudgetedRead, FormatError, FormatReader, FormatVersion, OctreeReader, ReadBudget, ReadPlan,
    ReadReservation, ReadSelection,
};

#[derive(Debug, Error)]
pub enum AsyncReadError {
    #[error(transparent)]
    Format(#[from] FormatError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("blocking read task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("max in-flight reads must be between 1 and the semaphore limit, got {0}")]
    InvalidMaxInFlight(usize),
    #[error("async reader semaphore is closed")]
    Closed,
}

struct Inner {
    base: FormatReader<PositionedCursor>,
    primary: Arc<SharedFile>,
    attributes: Option<Arc<SharedFile>>,
    budget: ReadBudget,
    slots: Arc<Semaphore>,
}

/// Cloneable async facade. Clones share the validated index, files, budget and in-flight limit.
#[derive(Clone)]
pub struct AsyncFormatReader {
    inner: Arc<Inner>,
}

impl AsyncFormatReader {
    /// Opens and validates the immutable file pair on Tokio's blocking pool.
    pub async fn open(
        primary: impl AsRef<Path>,
        attributes: Option<PathBuf>,
        max_in_flight: usize,
        max_payload_bytes: u64,
    ) -> Result<Self, AsyncReadError> {
        if max_in_flight == 0 || max_in_flight > Semaphore::MAX_PERMITS {
            return Err(AsyncReadError::InvalidMaxInFlight(max_in_flight));
        }
        let primary_path = primary.as_ref().to_path_buf();
        let (primary, attributes, base) = tokio::task::spawn_blocking(move || {
            let primary = Arc::new(SharedFile::open(&primary_path)?);
            let attributes = attributes
                .as_deref()
                .map(SharedFile::open)
                .transpose()?
                .map(Arc::new);
            let mut base = FormatReader::new(PositionedCursor::new(Arc::clone(&primary)))?;
            if let Some(attributes) = &attributes {
                base.attach_attributes(PositionedCursor::new(Arc::clone(attributes)))?;
            }
            Ok::<_, AsyncReadError>((primary, attributes, base))
        })
        .await??;
        Ok(Self {
            inner: Arc::new(Inner {
                base,
                primary,
                attributes,
                budget: ReadBudget::new(max_payload_bytes),
                slots: Arc::new(Semaphore::new(max_in_flight)),
            }),
        })
    }

    pub fn format_version(&self) -> FormatVersion {
        self.inner.base.format_version()
    }

    /// Builds a read plan from the shared validated index without payload I/O.
    pub fn plan_read(
        &self,
        id: BlockId,
        selection: ReadSelection,
    ) -> Result<ReadPlan, FormatError> {
        self.inner.base.plan_read(id, selection)
    }

    pub fn reserved_bytes(&self) -> u64 {
        self.inner.budget.reserved_bytes()
    }

    pub fn peak_reserved_bytes(&self) -> u64 {
        self.inner.budget.peak_reserved_bytes()
    }

    pub fn limit_bytes(&self) -> u64 {
        self.inner.budget.limit_bytes()
    }

    /// Waits for both a concurrency slot and enough payload quota, then reads asynchronously.
    /// The returned owner retains its output quota until the last `Arc` is dropped.
    pub async fn read(
        &self,
        id: BlockId,
        selection: ReadSelection,
    ) -> Result<Arc<BudgetedRead>, AsyncReadError> {
        self.read_with(id, selection, move |reader, reservation| {
            reader.read_reserved(id, selection, reservation)
        })
        .await
    }

    async fn read_with<F>(
        &self,
        id: BlockId,
        selection: ReadSelection,
        decode: F,
    ) -> Result<Arc<BudgetedRead>, AsyncReadError>
    where
        F: FnOnce(
                &mut FormatReader<PositionedCursor>,
                ReadReservation,
            ) -> Result<BudgetedRead, FormatError>
            + Send
            + 'static,
    {
        let plan = self.plan_read(id, selection)?;
        let requested = plan.memory().peak_bytes();
        if requested > self.inner.budget.limit_bytes() {
            return Err(FormatError::ReadBudgetExceeded {
                requested,
                limit: self.inner.budget.limit_bytes(),
            }
            .into());
        }

        let mut decode = Some(decode);
        loop {
            let slot = Arc::clone(&self.inner.slots)
                .acquire_owned()
                .await
                .map_err(|_| AsyncReadError::Closed)?;
            let notified = self.inner.budget.notifier().notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let Some(reservation) = self.inner.budget.try_reserve(requested)? else {
                drop(slot);
                notified.await;
                continue;
            };

            let primary_cursor = PositionedCursor::new(Arc::clone(&self.inner.primary));
            let attributes_cursor = self
                .inner
                .attributes
                .as_ref()
                .map(|file| PositionedCursor::new(Arc::clone(file)));
            let mut reader = self
                .inner
                .base
                .fork_with_cursors(primary_cursor, attributes_cursor);
            let decode = decode.take().expect("read closure is consumed once");
            let task = tokio::task::spawn_blocking(move || {
                let _slot: OwnedSemaphorePermit = slot;
                decode(&mut reader, reservation)
            });
            return task.await?.map(Arc::new).map_err(AsyncReadError::from);
        }
    }
}

struct SharedFile {
    file: PlatformFile,
    len: u64,
}

#[cfg(unix)]
struct PlatformFile(File);

#[cfg(not(unix))]
struct PlatformFile(Mutex<File>);

impl SharedFile {
    fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        #[cfg(unix)]
        let file = PlatformFile(file);
        #[cfg(not(unix))]
        let file = PlatformFile(Mutex::new(file));
        Ok(Self { file, len })
    }

    fn read_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
        if offset >= self.len || buffer.is_empty() {
            return Ok(0);
        }
        let available = usize::try_from(self.len - offset).unwrap_or(usize::MAX);
        let target_len = buffer.len().min(available);
        let target = &mut buffer[..target_len];
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.file.0.read_at(target, offset)
        }
        #[cfg(not(unix))]
        {
            let mut file = self
                .file
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            file.seek(SeekFrom::Start(offset))?;
            file.read(target)
        }
    }
}

struct PositionedCursor {
    file: Arc<SharedFile>,
    position: u64,
}

impl PositionedCursor {
    fn new(file: Arc<SharedFile>) -> Self {
        Self { file, position: 0 }
    }
}

impl Read for PositionedCursor {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.file.read_at(buffer, self.position)?;
        self.position = self
            .position
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cursor offset overflow"))?;
        Ok(read)
    }
}

impl Seek for PositionedCursor {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.file.len) + i128::from(delta),
        };
        if !(0..=i128::from(u64::MAX)).contains(&next) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid seek outside the cursor offset range",
            ));
        }
        self.position = next as u64;
        Ok(self.position)
    }
}
