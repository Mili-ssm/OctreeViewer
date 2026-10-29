use std::{
    borrow::BorrowMut,
    io::{Cursor, Read, Seek},
    sync::Arc,
};

use las::Reader;

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
    #[cfg_attr(not(test), allow(dead_code, reason = "used by M2 range planning"))]
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
    #[cfg_attr(not(test), allow(dead_code, reason = "used by M2 range reads"))]
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
#[cfg_attr(not(test), allow(dead_code, reason = "used by the serial host loop"))]
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
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "split is used by M2 range planning")
)]
pub trait SourceFactory {
    type Source: PointSource;
    /// Contiguous ranges in order covering `0..point_count`, at most `parts`.
    #[allow(dead_code, reason = "used by M2 range planning")]
    fn split(&self, parts: usize, batch: u64) -> Result<Vec<PointRange>, ConvertError>;
    /// A fresh source at `range.start`, preflight included.
    fn open(&self, range: PointRange) -> Result<Self::Source, ConvertError>;
}

/// LAS/LAZ sources over any [`ByteSource`]; compressed input only opens as a whole.
pub struct LasFactory<B: ByteSource> {
    bytes: B,
}

impl<B: ByteSource> LasFactory<B> {
    pub fn new(bytes: B) -> Self {
        Self { bytes }
    }

    fn open_reader(&self) -> Result<Reader, ConvertError> {
        open_las_reader_from(self.bytes.open()?, self.bytes.len())
    }
}

impl<B: ByteSource> SourceFactory for LasFactory<B> {
    type Source = LasPointSource<Reader>;

    /// One range for now: there is no LAZ chunk index to cut compressed input yet.
    #[cfg_attr(not(test), allow(dead_code, reason = "used by M2 range planning"))]
    fn split(&self, _parts: usize, _batch: u64) -> Result<Vec<PointRange>, ConvertError> {
        let end = self.open_reader()?.header().number_of_points();
        Ok(vec![PointRange { start: 0, end }])
    }

    fn open(&self, range: PointRange) -> Result<Self::Source, ConvertError> {
        let reader = self.open_reader()?;
        if range.start == 0 && range.end == reader.header().number_of_points() {
            Ok(LasPointSource::full(reader))
        } else {
            LasPointSource::records(reader, range)
        }
    }
}
