use std::{
    borrow::BorrowMut,
    io::{Cursor, Read, Seek},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use las::Reader;

use super::laz_chunks::{ChunkIndex, LazChunkPointSource};
use crate::{
    ConvertError,
    source::{contain_decoder_panic_at, open_las_reader_from, seek_points_at},
};

/// Half-open range of global point indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointRange {
    pub start: u64,
    pub end: u64,
}

impl PointRange {
    /// Splits `total` points into at most `parts` contiguous ranges whose starts are multiples
    /// of `batch`, so each range issues the same `fill` calls as the serial read.
    pub fn split(total: u64, batch: u64, parts: usize) -> Vec<Self> {
        let batches = total.div_ceil(batch.max(1));
        let per_part = batches.div_ceil(parts.max(1) as u64).max(1);
        (0..batches)
            .step_by(per_part as usize)
            .map(|first| Self {
                start: first * batch,
                end: ((first + per_part) * batch).min(total),
            })
            .collect()
    }
}

pub trait PointSource {
    fn header(&self) -> &las::Header;
    fn range(&self) -> PointRange;
    /// Global index of the next point.
    fn next_index(&self) -> u64;
    /// Up to `count` points, clamped to the range end; fewer only if the file ends early.
    /// Decoder panics are contained and reported with the global index of the first point.
    fn fill(&mut self, count: u64, data: &mut las::PointData) -> Result<u64, ConvertError>;
}

pub struct LasPointSource<R: BorrowMut<Reader>> {
    reader: R,
    range: PointRange,
    next: u64,
}

impl<R: BorrowMut<Reader>> LasPointSource<R> {
    /// The whole file, read from the reader's current position (start of the point data).
    pub fn full(reader: R) -> Self {
        let end = reader.borrow().header().number_of_points();
        Self {
            reader,
            range: PointRange { start: 0, end },
            next: 0,
        }
    }

    /// Seeks to `range.start` on fixed-size records; compressed input is rejected because
    /// LAZ chunks have variable size (A9).
    pub fn records(mut reader: R, range: PointRange) -> Result<Self, ConvertError> {
        if reader.borrow().header().point_format().is_compressed {
            return Err(ConvertError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "record ranges need fixed-size (uncompressed) LAS records",
            )));
        }
        if range.start > 0 {
            seek_points_at(reader.borrow_mut(), range.start, range.start)?;
        }
        Ok(Self {
            reader,
            range,
            next: range.start,
        })
    }
}

impl<R: BorrowMut<Reader>> PointSource for LasPointSource<R> {
    fn header(&self) -> &las::Header {
        self.reader.borrow().header()
    }

    fn range(&self) -> PointRange {
        self.range
    }

    fn next_index(&self) -> u64 {
        self.next
    }

    fn fill(&mut self, count: u64, data: &mut las::PointData) -> Result<u64, ConvertError> {
        let count = count.min(self.range.end - self.next);
        let reader = self.reader.borrow_mut();
        let read = contain_decoder_panic_at(Some(self.next), || reader.fill_points(count, data))??;
        self.next += read;
        Ok(read)
    }
}

/// Random-access input bytes that can be opened any number of times.
#[allow(
    clippy::len_without_is_empty,
    reason = "the length bounds allocations; emptiness is never asked"
)]
pub trait ByteSource {
    type Reader: Read + Seek + Send + Sync + 'static;
    fn open(&self) -> Result<Self::Reader, ConvertError>;
    fn len(&self) -> u64;
}

/// In-memory input.
pub struct MemBytes(pub Arc<[u8]>);

impl ByteSource for MemBytes {
    type Reader = Cursor<Arc<[u8]>>;

    fn open(&self) -> Result<Self::Reader, ConvertError> {
        Ok(Cursor::new(Arc::clone(&self.0)))
    }

    fn len(&self) -> u64 {
        self.0.len() as u64
    }
}

/// Opens the sources the planner needs beyond the first one.
pub trait SourceFactory {
    type Source: PointSource;
    /// Contiguous ranges in order covering `0..point_count`, at most `parts`.
    fn split(&self, parts: usize, batch: u64) -> Result<Vec<PointRange>, ConvertError>;
    /// A fresh source at `range.start`, preflight included.
    fn open(&self, range: PointRange) -> Result<Self::Source, ConvertError>;
}

/// The sources a [`LasFactory`] opens: the file as a whole (or LAS records) through `las`, or a
/// group of whole LAZ chunks.
#[allow(
    clippy::large_enum_variant,
    reason = "one source per range, never in a collection"
)]
pub enum RangeSource<R: Read + Seek + Send + Sync + 'static> {
    Las(LasPointSource<Reader>),
    Laz(LazChunkPointSource<R>),
}

impl<R: Read + Seek + Send + Sync + 'static> PointSource for RangeSource<R> {
    fn header(&self) -> &las::Header {
        match self {
            Self::Las(source) => source.header(),
            Self::Laz(source) => source.header(),
        }
    }

    fn range(&self) -> PointRange {
        match self {
            Self::Las(source) => source.range(),
            Self::Laz(source) => source.range(),
        }
    }

    fn next_index(&self) -> u64 {
        match self {
            Self::Las(source) => source.next_index(),
            Self::Laz(source) => source.next_index(),
        }
    }

    fn fill(&mut self, count: u64, data: &mut las::PointData) -> Result<u64, ConvertError> {
        match self {
            Self::Las(source) => source.fill(count, data),
            Self::Laz(source) => source.fill(count, data),
        }
    }
}

/// LAS/LAZ sources over any [`ByteSource`]. LAS cuts into record ranges; LAZ into groups of whole
/// chunks, read from a chunk index loaded on the first partial open.
pub struct LasFactory<B: ByteSource> {
    bytes: B,
    chunks: OnceLock<Arc<ChunkIndex>>,
    /// `fill` size of the last `split`, for the panic index of chunk sources.
    batch: AtomicU64,
}

impl<B: ByteSource> LasFactory<B> {
    pub fn new(bytes: B) -> Self {
        Self {
            bytes,
            chunks: OnceLock::new(),
            batch: AtomicU64::new(1),
        }
    }

    fn open_reader(&self) -> Result<Reader, ConvertError> {
        open_las_reader_from(self.bytes.open()?, self.bytes.len())
    }

    fn chunk_index(&self, header: &las::Header) -> Result<Arc<ChunkIndex>, ConvertError> {
        if let Some(index) = self.chunks.get() {
            return Ok(Arc::clone(index));
        }
        let mut input = self.bytes.open()?;
        let raw = las::raw::Header::read_from(&mut input)?;
        let index = Arc::new(ChunkIndex::read(
            &mut input,
            header,
            u64::from(raw.offset_to_point_data),
        )?);
        Ok(Arc::clone(self.chunks.get_or_init(|| index)))
    }
}

impl<B: ByteSource> SourceFactory for LasFactory<B> {
    type Source = RangeSource<B::Reader>;

    fn split(&self, parts: usize, batch: u64) -> Result<Vec<PointRange>, ConvertError> {
        self.batch.store(batch.max(1), Ordering::Relaxed);
        let reader = self.open_reader()?;
        let header = reader.header();
        let total = header.number_of_points();
        if parts <= 1 || total == 0 {
            return Ok(vec![PointRange {
                start: 0,
                end: total,
            }]);
        }
        if header.point_format().is_compressed {
            Ok(self.chunk_index(header)?.split(parts))
        } else {
            Ok(PointRange::split(total, batch, parts))
        }
    }

    fn open(&self, range: PointRange) -> Result<Self::Source, ConvertError> {
        let reader = self.open_reader()?;
        let header = reader.header();
        if range.start == 0 && range.end == header.number_of_points() {
            return Ok(RangeSource::Las(LasPointSource::full(reader)));
        }
        if !header.point_format().is_compressed {
            return Ok(RangeSource::Las(LasPointSource::records(reader, range)?));
        }
        let index = self.chunk_index(header)?;
        let chunks = index.chunks_for(range).ok_or_else(|| {
            ConvertError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "LAZ ranges must start and end at chunk boundaries",
            ))
        })?;
        Ok(RangeSource::Laz(LazChunkPointSource::new(
            self.bytes.open()?,
            header.clone(),
            &index,
            chunks,
            self.batch.load(Ordering::Relaxed),
        )?))
    }
}
