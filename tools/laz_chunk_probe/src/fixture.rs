//! Self-contained fixture so native and wasm32 run the identical code on identical bytes:
//! `u32 vlr_len | vlr | u32 chunks | chunks * (u64 points, u64 bytes) | chunk bytes`.

use std::io::{Cursor, Read, Seek, SeekFrom};

use laz::laszip::{ChunkTable, ChunkTableEntry, LazVlr};

use crate::{ChunkIndex, Error};

#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("fixture truncated")]
    Truncated,
    #[error(transparent)]
    Probe(#[from] Error),
}

pub struct Fixture {
    pub index: ChunkIndex,
    /// The concatenated chunk bytes; `index.data_start` is 0 inside it.
    pub data: Vec<u8>,
}

impl Fixture {
    /// Serializes the first `chunks` chunks of a source described by `index`.
    pub fn encode<R: Read + Seek>(
        src: &mut R,
        index: &ChunkIndex,
        chunks: usize,
    ) -> Result<Vec<u8>, Error> {
        let entries = index
            .table
            .as_ref()
            .get(..chunks)
            .ok_or(Error::Invalid("fewer chunks than requested"))?;
        let mut vlr = Vec::new();
        index.vlr.write_to(&mut vlr)?;
        let mut out = Vec::new();
        out.extend_from_slice(&(vlr.len() as u32).to_le_bytes());
        out.extend_from_slice(&vlr);
        out.extend_from_slice(&(chunks as u32).to_le_bytes());
        for entry in entries {
            out.extend_from_slice(&entry.point_count.to_le_bytes());
            out.extend_from_slice(&entry.byte_count.to_le_bytes());
        }
        let bytes: u64 = entries.iter().map(|entry| entry.byte_count).sum();
        src.seek(SeekFrom::Start(index.data_start))?;
        let read = src.take(bytes).read_to_end(&mut out)?;
        if read as u64 != bytes {
            return Err(Error::Invalid("source shorter than the chunk bytes"));
        }
        Ok(out)
    }

    pub fn parse(mut bytes: Vec<u8>) -> Result<Self, FixtureError> {
        let mut at = 0usize;
        let mut take = |len: usize, bytes: &[u8]| -> Result<Range, FixtureError> {
            let end = at.checked_add(len).filter(|end| *end <= bytes.len());
            let end = end.ok_or(FixtureError::Truncated)?;
            let range = at..end;
            at = end;
            Ok(range)
        };
        let word =
            |range: Range, bytes: &[u8]| u32::from_le_bytes(bytes[range].try_into().unwrap());
        let vlr_len = word(take(4, &bytes)?, &bytes) as usize;
        let vlr_range = take(vlr_len, &bytes)?;
        let vlr = LazVlr::read_from(Cursor::new(&bytes[vlr_range])).map_err(Error::from)?;
        let count = word(take(4, &bytes)?, &bytes) as usize;
        let mut table = ChunkTable::with_capacity(count);
        for _ in 0..count {
            let range = take(16, &bytes)?;
            let entry = &bytes[range];
            table.push(ChunkTableEntry {
                point_count: u64::from_le_bytes(entry[..8].try_into().unwrap()),
                byte_count: u64::from_le_bytes(entry[8..].try_into().unwrap()),
            });
        }
        let data = bytes.split_off(at);
        let expected: u64 = table.as_ref().iter().map(|entry| entry.byte_count).sum();
        if data.len() as u64 != expected {
            return Err(FixtureError::Truncated);
        }
        Ok(Self {
            index: ChunkIndex {
                vlr,
                table,
                data_start: 0,
            },
            data,
        })
    }
}

type Range = std::ops::Range<usize>;
