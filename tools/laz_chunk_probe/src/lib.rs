//! S2 probe: decode a range of LAZ chunks with a fresh sequential decoder, without `seek`.
//!
//! `laz` 0.13 `LasZipDecompressor::seek` computes the in-chunk delta as
//! `point_idx % chunk.point_count`, which is wrong for variable-size chunks. Here a range is
//! decoded by building, in memory, a tiny LAZ stream for just that range (offset word + the
//! range's compressed bytes + a sub-table with the range's entries) and decoding it from its
//! start with `LasZipDecompressor::new`. No decoder state is moved to the middle of a stream.

mod fixture;
#[cfg(target_arch = "wasm32")]
mod wasm;

pub use fixture::{Fixture, FixtureError};

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::ops::Range;

use laz::laszip::{ChunkTable, ChunkTableEntry, LasZipDecompressor, LazVlr};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("LAZ error: {0}")]
    Laz(#[from] laz::LasZipError),
    #[error("LAS error: {0}")]
    Las(#[from] las::Error),
    #[error("invalid input: {0}")]
    Invalid(&'static str),
}

/// What is needed to start a decoder at any chunk: the VLR and the decoded chunk table, with
/// the byte offset of chunk 0 inside the source.
pub struct ChunkIndex {
    pub vlr: LazVlr,
    pub table: ChunkTable,
    /// Offset of the first chunk byte, i.e. `offset_to_point_data + 8`.
    pub data_start: u64,
}

impl ChunkIndex {
    /// Reads the chunk table of a LAZ stream whose point data starts at `point_data_offset`
    /// (the 8-byte table offset sits there). For fixed-size chunks the entries get
    /// `chunk_size` points, except the last one, which gets the remainder of `header_points`.
    pub fn read<R: Read + Seek>(
        src: &mut R,
        vlr: LazVlr,
        point_data_offset: u64,
        header_points: u64,
    ) -> Result<Self, Error> {
        src.seek(SeekFrom::Start(point_data_offset))?;
        let mut word = [0u8; 8];
        src.read_exact(&mut word)?;
        let table_offset = u64::try_from(i64::from_le_bytes(word))
            .ok()
            .filter(|offset| *offset > point_data_offset)
            .ok_or(Error::Invalid("chunk table offset not set"))?;
        src.seek(SeekFrom::Start(table_offset))?;
        let variable = vlr.uses_variable_size_chunks();
        let read = ChunkTable::read(src, variable)?;
        let table = if variable {
            read
        } else {
            let size = u64::from(vlr.chunk_size());
            let mut table = ChunkTable::with_capacity(read.len());
            let mut left = header_points;
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
        Ok(Self {
            vlr,
            table,
            data_start: point_data_offset + 8,
        })
    }

    /// Same as [`ChunkIndex::read`] taking the VLR and point count from a parsed header.
    pub fn read_for_header<R: Read + Seek>(
        src: &mut R,
        header: &las::Header,
        point_data_offset: u64,
    ) -> Result<Self, Error> {
        Self::read(
            src,
            header.laz_vlr()?,
            point_data_offset,
            header.number_of_points(),
        )
    }

    /// Bytes per decoded point record (the laszip item sizes).
    pub fn record_size(&self) -> usize {
        self.vlr.items_size() as usize
    }

    fn entries(&self, chunks: &Range<usize>) -> Result<&[ChunkTableEntry], Error> {
        self.table
            .as_ref()
            .get(chunks.clone())
            .ok_or(Error::Invalid("chunk range outside the chunk table"))
    }
}

/// A decoder positioned at the first point of a chunk range, plus the points it must yield.
pub struct RangeDecoder {
    decompressor: LasZipDecompressor<'static, Cursor<Vec<u8>>>,
    points: u64,
    record_size: usize,
}

impl RangeDecoder {
    /// Builds the per-range stream and the decoder. Reads the range's compressed bytes from
    /// `src`, never decodes anything yet.
    pub fn new<R: Read + Seek>(
        src: &mut R,
        index: &ChunkIndex,
        chunks: Range<usize>,
    ) -> Result<Self, Error> {
        let entries = index.entries(&chunks)?;
        let first_offset: u64 = index.table.as_ref()[..chunks.start]
            .iter()
            .map(|entry| entry.byte_count)
            .sum();
        let bytes: u64 = entries.iter().map(|entry| entry.byte_count).sum();
        let points: u64 = entries.iter().map(|entry| entry.point_count).sum();
        let mut sub_table = ChunkTable::with_capacity(entries.len());
        for entry in entries {
            sub_table.push(*entry);
        }
        let mut stream = Vec::with_capacity(8 + bytes as usize + 16 + 8 * entries.len());
        // The table goes right after the chunks; `laz` reads this offset first.
        stream.extend_from_slice(&((8 + bytes) as i64).to_le_bytes());
        src.seek(SeekFrom::Start(index.data_start + first_offset))?;
        let read = src.take(bytes).read_to_end(&mut stream)?;
        if read as u64 != bytes {
            return Err(Error::Invalid("source shorter than the chunk bytes"));
        }
        sub_table.write_to(&mut stream, &index.vlr)?;
        let decompressor = LasZipDecompressor::new(Cursor::new(stream), index.vlr.clone())?;
        Ok(Self {
            decompressor,
            points,
            record_size: index.record_size(),
        })
    }

    pub fn points(&self) -> u64 {
        self.points
    }

    /// Decodes every point of the range, appending raw LAS point records to `out`.
    pub fn decode_into(mut self, out: &mut Vec<u8>) -> Result<u64, Error> {
        let start = out.len();
        out.resize(start + self.points as usize * self.record_size, 0);
        self.decompressor.decompress_many(&mut out[start..])?;
        Ok(self.points)
    }
}

/// Decodes `chunks` into `out` (raw point records, `index.record_size()` bytes each) with a
/// decoder started at the first chunk of the range. Returns the number of points.
pub fn decode_chunk_range<R: Read + Seek>(
    src: &mut R,
    index: &ChunkIndex,
    chunks: Range<usize>,
    out: &mut Vec<u8>,
) -> Result<u64, Error> {
    RangeDecoder::new(src, index, chunks)?.decode_into(out)
}

/// Reference: one sequential decoder from the start of the point data, first `points` points.
pub fn decode_sequential<R: Read + Seek + Send + Sync + 'static>(
    mut src: R,
    vlr: &LazVlr,
    point_data_offset: u64,
    points: u64,
    out: &mut Vec<u8>,
) -> Result<(), Error> {
    src.seek(SeekFrom::Start(point_data_offset))?;
    let mut decompressor = LasZipDecompressor::new(src, vlr.clone())?;
    let record_size = vlr.items_size() as usize;
    let start = out.len();
    out.resize(start + points as usize * record_size, 0);
    decompressor.decompress_many(&mut out[start..])?;
    Ok(())
}

/// Splits `0..chunks` into `parts` contiguous ranges of near-equal chunk count.
pub fn balanced_split(chunks: usize, parts: usize) -> Vec<Range<usize>> {
    (0..parts)
        .map(|part| (chunks * part / parts)..(chunks * (part + 1) / parts))
        .collect()
}

/// FNV-1a over `bytes`; identical on every target, used to compare native and wasm32 output.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A LAZ file on disk with its chunk index.
pub struct OpenedLaz {
    pub index: ChunkIndex,
    pub point_data_offset: u64,
    pub header_points: u64,
}

impl OpenedLaz {
    pub fn open(path: &std::path::Path) -> Result<Self, Error> {
        let header = las::raw::Header::read_from(std::fs::File::open(path)?)?;
        let point_data_offset = u64::from(header.offset_to_point_data);
        let mut file = std::fs::File::open(path)?;
        let parsed = las::Reader::new(std::io::BufReader::new(std::fs::File::open(path)?))?
            .header()
            .clone();
        let index = ChunkIndex::read_for_header(&mut file, &parsed, point_data_offset)?;
        Ok(Self {
            index,
            point_data_offset,
            header_points: parsed.number_of_points(),
        })
    }

    /// Points in the first `chunks` chunks.
    pub fn points_in(&self, chunks: usize) -> u64 {
        self.index.table.as_ref()[..chunks]
            .iter()
            .map(|entry| entry.point_count)
            .sum()
    }
}

/// Chunk sizes of [`synthetic_variable_chunk_laz`].
pub const SYNTHETIC_CHUNK_SIZES: [usize; 6] = [7, 13, 5, 11, 50_000, 3];

/// A complete LAS 1.2 format-0 LAZ file (viewer_convert input) with variable-size chunks
/// [`SYNTHETIC_CHUNK_SIZES`] and pseudo-random points spread over a 1000 x 1000 x 100 box.
/// Deterministic: fixed seed and a fixed header creation date.
pub fn synthetic_variable_chunk_laz() -> Result<Vec<u8>, Error> {
    use las::{Builder, Point, Writer, point::Format};
    use laz::laszip::{LasZipCompressor, LazVlrBuilder};

    let total: usize = SYNTHETIC_CHUNK_SIZES.iter().sum();
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(0)?;
    builder.point_format.is_compressed = true;
    builder.transforms = las::Vector {
        x: las::Transform {
            scale: 0.01,
            offset: 0.0,
        },
        y: las::Transform {
            scale: 0.01,
            offset: 0.0,
        },
        z: las::Transform {
            scale: 0.01,
            offset: 0.0,
        },
    };
    let mut writer = Writer::new(Cursor::new(Vec::new()), builder.into_header()?)?;
    let mut state = 0x5eed_u64;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        state >> 33
    };
    for _ in 0..total {
        writer.write_point(Point {
            x: (next() % 100_000) as f64 / 100.0,
            y: (next() % 100_000) as f64 / 100.0,
            z: (next() % 10_000) as f64 / 100.0,
            intensity: (next() & 0xffff) as u16,
            return_number: 1,
            number_of_returns: 1,
            ..Default::default()
        })?;
    }
    let template = writer.into_inner()?.into_inner();

    let data_offset = u32::from_le_bytes(template[96..100].try_into().unwrap()) as usize;
    let vlr_data = 227 + 54;
    let vlr = LazVlr::from_buffer(&template[vlr_data..data_offset])?;
    let mut raw = Vec::new();
    decode_sequential(
        Cursor::new(template.clone()),
        &vlr,
        data_offset as u64,
        total as u64,
        &mut raw,
    )?;

    let variable = LazVlrBuilder::default()
        .with_laz_items(vlr.items().clone())
        .with_variable_chunk_size()
        .build();
    let mut header = template[..data_offset].to_vec();
    // chunk_size field of the laszip VLR (u32::MAX = variable-size chunks).
    header[vlr_data + 12..vlr_data + 16].copy_from_slice(&u32::MAX.to_le_bytes());
    // File creation day-of-year and year: fixed so the fixture is reproducible.
    header[90..92].copy_from_slice(&1u16.to_le_bytes());
    header[92..94].copy_from_slice(&2026u16.to_le_bytes());
    let mut cursor = Cursor::new(header);
    cursor.seek(SeekFrom::End(0))?;
    let record = vlr.items_size() as usize;
    let mut compressor = LasZipCompressor::new(&mut cursor, variable)?;
    let mut at = 0;
    for (number, size) in SYNTHETIC_CHUNK_SIZES.iter().enumerate() {
        compressor.compress_many(&raw[at * record..(at + size) * record])?;
        at += size;
        // `done` closes the last chunk itself; closing it here too adds an empty entry.
        if number + 1 < SYNTHETIC_CHUNK_SIZES.len() {
            compressor.finish_current_chunk()?;
        }
    }
    compressor.done()?;
    drop(compressor);
    Ok(cursor.into_inner())
}
