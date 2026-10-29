//! LAZ input read by ranges of whole chunks (S2 recipe, `LAZ_CHUNK_RANGES_2026-10-29`).
//!
//! `laz` 0.13 `LasZipDecompressor::seek` takes the in-chunk delta as `point % chunk_size`, which
//! is wrong for variable-size chunks, so a range never seeks. A fresh decoder starts at the first
//! chunk of the range over a virtual stream `[8 + bytes][chunks a..b][sub-table]` that looks like
//! a tiny LAZ point-data section; only the prefix and the sub-table (about 16 bytes per chunk)
//! live in memory, the chunk bytes are read from the input on demand.

use std::{
    io::{self, BufReader, Read, Seek, SeekFrom},
    ops::Range,
};

use las::PointDataBuilder;
use laz::laszip::{ChunkTable, ChunkTableEntry, LasZipDecompressor, LazVlr};

use super::point_source::{PointRange, PointSource};
use crate::{
    ConvertError,
    source::{contain_decoder_panic, contain_decoder_panic_at},
};

/// Read buffer of the chunk bytes of one range.
const CHUNK_READ_BUFFER_BYTES: usize = 64 * 1024;

fn laz_error(error: laz::LasZipError) -> ConvertError {
    ConvertError::Las(las::Error::LasZipError(error))
}

fn invalid(message: &'static str) -> ConvertError {
    ConvertError::Io(io::Error::new(io::ErrorKind::InvalidData, message))
}

/// What is needed to start a decoder at any chunk: the VLR and the decoded chunk table, with the
/// byte offset of chunk 0 inside the input.
pub struct ChunkIndex {
    pub vlr: LazVlr,
    pub table: ChunkTable,
    /// Offset of the first chunk byte, i.e. `offset_to_point_data + 8`.
    pub data_start: u64,
}

impl ChunkIndex {
    /// Reads the chunk table of a LAZ input whose point data starts at `point_data_offset` (the
    /// 8-byte table offset sits there). For fixed-size chunks the entries get `chunk_size`
    /// points, except the last one, which gets the remainder of the header's point count.
    pub fn read<R: Read + Seek>(
        src: &mut R,
        header: &las::Header,
        point_data_offset: u64,
    ) -> Result<Self, ConvertError> {
        let vlr = header.laz_vlr()?;
        let total = header.number_of_points();
        src.seek(SeekFrom::Start(point_data_offset))?;
        let mut word = [0u8; 8];
        src.read_exact(&mut word)?;
        let table_offset = u64::try_from(i64::from_le_bytes(word))
            .ok()
            .filter(|offset| *offset > point_data_offset)
            .ok_or_else(|| invalid("LAZ chunk table offset is not set"))?;
        src.seek(SeekFrom::Start(table_offset))?;
        let variable = vlr.uses_variable_size_chunks();
        // `wasm32` cannot unwind: corrupt tables are contained like any decoder call.
        let read = contain_decoder_panic(|| ChunkTable::read(src, variable))??;
        let table = if variable {
            read
        } else {
            let size = u64::from(vlr.chunk_size());
            let mut table = ChunkTable::with_capacity(read.len());
            let mut left = total;
            for entry in read.as_ref() {
                let point_count = left.min(size);
                left -= point_count;
                table.push(ChunkTableEntry {
                    point_count,
                    byte_count: entry.byte_count,
                });
            }
            table
        };
        let counted = table
            .as_ref()
            .iter()
            .try_fold(0_u64, |sum, entry| sum.checked_add(entry.point_count))
            .ok_or(ConvertError::SizeOverflow)?;
        if counted != total {
            return Err(ConvertError::PointCountMismatch {
                expected: total,
                actual: counted,
            });
        }
        Ok(Self {
            vlr,
            table,
            data_start: point_data_offset
                .checked_add(8)
                .ok_or(ConvertError::SizeOverflow)?,
        })
    }

    pub fn chunk_count(&self) -> usize {
        self.table.len()
    }

    /// Global index of the first point of every chunk, plus the total as the last element.
    fn point_starts(&self) -> Vec<u64> {
        let mut starts = Vec::with_capacity(self.table.len() + 1);
        let mut at = 0_u64;
        starts.push(at);
        for entry in self.table.as_ref() {
            at += entry.point_count;
            starts.push(at);
        }
        starts
    }

    /// Contiguous groups of whole chunks (at most `parts`), cut at the chunk boundaries nearest
    /// to `k · total / parts`. Every range holds points; together they cover `0..total`.
    pub fn split(&self, parts: usize) -> Vec<PointRange> {
        let starts = self.point_starts();
        let chunks = starts.len() - 1;
        let total = starts[chunks];
        let mut cuts = vec![0_usize];
        for part in 1..parts.max(1) {
            let target = (u128::from(total) * part as u128 / parts as u128) as u64;
            let above = starts.partition_point(|start| *start < target);
            let below = above.saturating_sub(1);
            let nearest = if above <= chunks
                && starts[above].abs_diff(target) < starts[below].abs_diff(target)
            {
                above
            } else {
                below
            };
            if nearest > *cuts.last().expect("cuts start with 0") && nearest < chunks {
                cuts.push(nearest);
            }
        }
        cuts.push(chunks);
        cuts.windows(2)
            .map(|pair| PointRange {
                start: starts[pair[0]],
                end: starts[pair[1]],
            })
            .filter(|range| range.end > range.start)
            .collect()
    }

    /// The chunks that exactly cover `range`; `None` when its ends are not chunk boundaries.
    pub fn chunks_for(&self, range: PointRange) -> Option<Range<usize>> {
        let starts = self.point_starts();
        let first = starts.binary_search(&range.start).ok()?;
        let last = starts.binary_search(&range.end).ok()?;
        (first <= last).then_some(first..last)
    }
}

/// A virtual `Read + Seek` over `[i64 LE = 8 + bytes][chunks a..b of the input][sub-table]`.
pub struct ChunkRangeStream<R> {
    input: BufReader<R>,
    prefix: [u8; 8],
    table: Vec<u8>,
    /// Offset in `input` of the first byte of chunk `a`.
    input_start: u64,
    chunk_bytes: u64,
    pos: u64,
    /// Where `input` is positioned, `None` when unknown (after a failed seek).
    input_at: Option<u64>,
}

impl<R: Read + Seek> ChunkRangeStream<R> {
    pub fn new(input: R, index: &ChunkIndex, chunks: Range<usize>) -> Result<Self, ConvertError> {
        let entries = index
            .table
            .as_ref()
            .get(chunks.clone())
            .ok_or_else(|| invalid("chunk range is outside the chunk table"))?;
        let sum = |entries: &[ChunkTableEntry]| {
            entries
                .iter()
                .try_fold(0_u64, |sum, entry| sum.checked_add(entry.byte_count))
        };
        let skipped =
            sum(&index.table.as_ref()[..chunks.start]).ok_or(ConvertError::SizeOverflow)?;
        let chunk_bytes = sum(entries).ok_or(ConvertError::SizeOverflow)?;
        let prefix = i64::try_from(chunk_bytes)
            .ok()
            .and_then(|bytes| bytes.checked_add(8))
            .ok_or(ConvertError::SizeOverflow)?
            .to_le_bytes();
        let mut sub_table = ChunkTable::with_capacity(entries.len());
        for entry in entries {
            sub_table.push(*entry);
        }
        let mut table = Vec::new();
        sub_table.write_to(&mut table, &index.vlr)?;
        Ok(Self {
            input: BufReader::with_capacity(CHUNK_READ_BUFFER_BYTES, input),
            prefix,
            table,
            input_start: index
                .data_start
                .checked_add(skipped)
                .ok_or(ConvertError::SizeOverflow)?,
            chunk_bytes,
            pos: 0,
            input_at: None,
        })
    }

    fn chunks_end(&self) -> u64 {
        8 + self.chunk_bytes
    }

    fn len(&self) -> u64 {
        self.chunks_end() + self.table.len() as u64
    }
}

impl<R: Read + Seek> Read for ChunkRangeStream<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.pos >= self.len() {
            return Ok(0);
        }
        let chunks_end = self.chunks_end();
        if self.pos < 8 {
            let from = self.pos as usize;
            let count = buf.len().min(8 - from);
            buf[..count].copy_from_slice(&self.prefix[from..from + count]);
            self.pos += count as u64;
            return Ok(count);
        }
        if self.pos < chunks_end {
            let wanted = (buf.len() as u64).min(chunks_end - self.pos) as usize;
            let at = self.input_start + (self.pos - 8);
            if self.input_at != Some(at) {
                self.input_at = None;
                self.input.seek(SeekFrom::Start(at))?;
            }
            self.input_at = None;
            let read = self.input.read(&mut buf[..wanted])?;
            self.input_at = Some(at + read as u64);
            self.pos += read as u64;
            return Ok(read);
        }
        let from = (self.pos - chunks_end) as usize;
        let count = buf.len().min(self.table.len() - from);
        buf[..count].copy_from_slice(&self.table[from..from + count]);
        self.pos += count as u64;
        Ok(count)
    }
}

impl<R: Read + Seek> Seek for ChunkRangeStream<R> {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        let (base, delta) = match target {
            SeekFrom::Start(pos) => {
                self.pos = pos;
                return Ok(pos);
            }
            SeekFrom::End(delta) => (self.len(), delta),
            SeekFrom::Current(delta) => (self.pos, delta),
        };
        self.pos = base.checked_add_signed(delta).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid seek to a negative or overflowing position",
            )
        })?;
        Ok(self.pos)
    }
}

/// Points of a group of whole chunks, decoded by a fresh sequential decoder.
pub struct LazChunkPointSource<R: Read + Seek + Send + Sync + 'static> {
    decoder: LasZipDecompressor<'static, ChunkRangeStream<R>>,
    header: las::Header,
    range: PointRange,
    next: u64,
    batch: u64,
}

impl<R: Read + Seek + Send + Sync + 'static> LazChunkPointSource<R> {
    /// `batch` is the `fill` size of the serial read: a decoder panic reports the first point of
    /// the batch that contains the failing one, as the serial run does.
    pub fn new(
        input: R,
        header: las::Header,
        index: &ChunkIndex,
        chunks: Range<usize>,
        batch: u64,
    ) -> Result<Self, ConvertError> {
        if index.vlr.items_size() != u64::from(header.point_format().len()) {
            return Err(invalid("LAZ item sizes do not match the point format"));
        }
        let entries = index
            .table
            .as_ref()
            .get(chunks.clone())
            .ok_or_else(|| invalid("chunk range is outside the chunk table"))?;
        let points_before = index.table.as_ref()[..chunks.start]
            .iter()
            .map(|entry| entry.point_count)
            .sum::<u64>();
        let points = entries.iter().map(|entry| entry.point_count).sum::<u64>();
        let stream = ChunkRangeStream::new(input, index, chunks)?;
        let decoder = contain_decoder_panic(|| LasZipDecompressor::new(stream, index.vlr.clone()))?
            .map_err(laz_error)?;
        Ok(Self {
            decoder,
            header,
            range: PointRange {
                start: points_before,
                end: points_before + points,
            },
            next: points_before,
            batch: batch.max(1),
        })
    }
}

impl<R: Read + Seek + Send + Sync + 'static> PointSource for LazChunkPointSource<R> {
    fn header(&self) -> &las::Header {
        &self.header
    }

    fn range(&self) -> PointRange {
        self.range
    }

    fn next_index(&self) -> u64 {
        self.next
    }

    fn fill(&mut self, count: u64, data: &mut las::PointData) -> Result<u64, ConvertError> {
        let count = count.min(self.range.end - self.next);
        if data.format() != self.header.point_format() {
            *data = PointDataBuilder::new().for_header(&self.header).build();
        }
        let bytes =
            data.resize_for(usize::try_from(count).map_err(|_| ConvertError::SizeOverflow)?);
        let batch_start = self.next - self.next % self.batch;
        let decoder = &mut self.decoder;
        contain_decoder_panic_at(Some(batch_start), || decoder.decompress_many(bytes))?
            .map_err(|error| ConvertError::Las(las::Error::Io(error)))?;
        self.next += count;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::pipeline::point_source::{LasFactory, MemBytes, SourceFactory};

    fn fixture() -> Vec<u8> {
        laz_chunk_probe::synthetic_variable_chunk_laz().unwrap()
    }

    fn header_and_offset(bytes: &[u8]) -> (las::Header, u64) {
        let header = las::Reader::new(Cursor::new(bytes.to_vec()))
            .unwrap()
            .header()
            .clone();
        let raw = las::raw::Header::read_from(Cursor::new(bytes)).unwrap();
        (header, u64::from(raw.offset_to_point_data))
    }

    #[test]
    fn chunk_sources_equal_the_probe_decoder_in_all_32_compositions() {
        let bytes = fixture();
        let (header, offset) = header_and_offset(&bytes);
        let index = ChunkIndex::read(&mut Cursor::new(&bytes), &header, offset).unwrap();
        let probe =
            laz_chunk_probe::ChunkIndex::read_for_header(&mut Cursor::new(&bytes), &header, offset)
                .unwrap();
        assert_eq!(index.chunk_count(), 6);
        for mask in 0..32_u32 {
            let mut cuts = vec![0_usize];
            cuts.extend((1..6).filter(|boundary| (mask >> (boundary - 1)) & 1 == 1));
            cuts.push(6);
            for pair in cuts.windows(2) {
                let chunks = pair[0]..pair[1];
                let mut expected = Vec::new();
                laz_chunk_probe::decode_chunk_range(
                    &mut Cursor::new(&bytes),
                    &probe,
                    chunks.clone(),
                    &mut expected,
                )
                .unwrap();
                // Fills of 5 points cross chunk boundaries at arbitrary offsets.
                let mut source = LazChunkPointSource::new(
                    Cursor::new(bytes.clone()),
                    header.clone(),
                    &index,
                    chunks.clone(),
                    5,
                )
                .unwrap();
                let range = source.range();
                let mut data = las::PointDataBuilder::new().for_header(&header).build();
                let mut got = Vec::new();
                while source.next_index() < range.end {
                    let read = source.fill(5, &mut data).unwrap();
                    assert!(read > 0);
                    got.extend_from_slice(data.raw_bytes());
                }
                assert_eq!(got, expected, "mask {mask:#07b} chunks {chunks:?}");
                assert_eq!(source.fill(5, &mut data).unwrap(), 0);
            }
        }
    }

    #[test]
    fn split_cuts_whole_chunks_nearest_to_the_even_share() {
        let bytes = fixture();
        let (header, offset) = header_and_offset(&bytes);
        let index = ChunkIndex::read(&mut Cursor::new(&bytes), &header, offset).unwrap();
        let total = 50_039;
        for parts in 1..=8 {
            let ranges = index.split(parts);
            assert!(ranges.len() <= parts);
            assert_eq!((ranges[0].start, ranges.last().unwrap().end), (0, total));
            for pair in ranges.windows(2) {
                assert_eq!(pair[0].end, pair[1].start);
            }
            for range in &ranges {
                assert!(index.chunks_for(*range).is_some(), "{range:?}");
            }
        }
        // Chunk starts: 0, 7, 20, 25, 36, 50036, 50039.
        assert_eq!(
            index.split(2),
            [
                PointRange { start: 0, end: 36 },
                PointRange {
                    start: 36,
                    end: total
                }
            ]
        );
        assert!(index.chunks_for(PointRange { start: 1, end: 7 }).is_none());
    }

    #[test]
    fn a_factory_opens_chunk_groups_and_rejects_unaligned_ranges() {
        let factory = LasFactory::new(MemBytes(fixture().into()));
        let ranges = factory.split(3, 4096).unwrap();
        assert!(ranges.len() >= 2);
        let mut seen = 0;
        for range in ranges {
            let source = factory.open(range).unwrap();
            assert_eq!(source.range(), range);
            seen += range.end - range.start;
        }
        assert_eq!(seen, 50_039);
        assert!(
            factory
                .open(PointRange {
                    start: 3,
                    end: 50_039
                })
                .is_err()
        );
        assert_eq!(factory.split(1, 1).unwrap().len(), 1);
    }
}
