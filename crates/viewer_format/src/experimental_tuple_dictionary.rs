//! Experimental columnar full-tuple dictionary, NOT a supported `.octattrs` revision.
//! One shared packed ID per row, segment counts (e.g. internal depths), independent tables.
use crate::{BlockCodec, CodecError, compress_block, crc32, decompress_block};
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
};
use thiserror::Error;

const MAGIC: &[u8; 8] = b"OCTATEXP";
const INDEPENDENT_MAGIC: &[u8; 8] = b"OCTARAWX";
const HEADER: usize = 32;
const DIRECTORY_ENTRY: usize = 29;

#[derive(Debug, Error)]
pub enum TupleDictionaryError {
    #[error("invalid experimental dictionary: {0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Codec(#[from] CodecError),
}
type Result<T> = std::result::Result<T, TupleDictionaryError>;
fn invalid(message: &'static str) -> TupleDictionaryError {
    TupleDictionaryError::Invalid(message)
}
fn u32_size(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|_| invalid("u32 length overflow"))
}
fn product(a: usize, b: usize) -> Result<usize> {
    a.checked_mul(b).ok_or_else(|| invalid("size overflow"))
}
fn bits_for(k: usize) -> usize {
    (32 - (k as u32).saturating_sub(1).leading_zeros()) as usize
}

/// Encode one block-local group of equal-semantics columns. Counts delimit depth arrays.
/// Source rows and node summaries need separate groups: their schemas differ.
/// Deduplication uses complete byte keys with collision-resolving equality, first occurrence IDs.
/// Counts, directory, widths, CRCs, tables and IDs are all included in the returned bytes.
/// This builder is experimental and uses normal allocation policy, not a peak-memory limit.
pub fn encode(level_counts: &[u32], columns: &[(usize, Vec<u8>)]) -> Result<Vec<u8>> {
    encode_impl(level_counts, columns, true)
}

/// Equivalent independent raw/LZ4 columns, for controlled experimental comparisons.
/// Same directory envelope, with an empty ID payload; no tuple deduplication.
pub fn encode_independent(level_counts: &[u32], columns: &[(usize, Vec<u8>)]) -> Result<Vec<u8>> {
    encode_impl(level_counts, columns, false)
}

fn encode_impl(
    level_counts: &[u32],
    columns: &[(usize, Vec<u8>)],
    dictionary: bool,
) -> Result<Vec<u8>> {
    if columns.is_empty() {
        return Err(invalid("no columns"));
    }
    let rows = level_counts.iter().try_fold(0_usize, |n, &v| {
        n.checked_add(v as usize)
            .ok_or_else(|| invalid("row overflow"))
    })?;
    u32_size(rows)?;
    let width = columns.iter().try_fold(0_usize, |n, (w, raw)| {
        if *w == 0 || raw.len() != product(rows, *w)? {
            return Err(invalid("column alignment"));
        }
        u32_size(*w)?;
        n.checked_add(*w)
            .ok_or_else(|| invalid("tuple width overflow"))
    })?;
    let mut lookup = HashMap::<Vec<u8>, u32>::new();
    let mut tables = vec![Vec::new(); columns.len()];
    let mut ids = Vec::with_capacity(rows);
    let mut key = Vec::with_capacity(width);
    for row in 0..if dictionary { rows } else { 0 } {
        key.clear();
        for (w, raw) in columns {
            key.extend_from_slice(&raw[row * w..(row + 1) * w]);
        }
        let next = u32_size(lookup.len())?;
        let id = *lookup.entry(key.clone()).or_insert_with(|| {
            for ((w, raw), table) in columns.iter().zip(&mut tables) {
                table.extend_from_slice(&raw[row * w..(row + 1) * w]);
            }
            next
        });
        ids.push(id);
    }
    let unique = if dictionary { lookup.len() } else { rows };
    let bits = if dictionary { bits_for(unique) } else { 0 };
    if !dictionary {
        tables = columns.iter().map(|(_, raw)| raw.clone()).collect();
    }
    let mut packed = vec![0_u8; product(rows, bits)?.div_ceil(8)];
    for (row, id) in ids.into_iter().enumerate() {
        for bit in 0..bits {
            let offset = row * bits + bit;
            packed[offset / 8] |= (((id >> bit) & 1) as u8) << (offset % 8);
        }
    }
    let raw_payloads: Vec<_> = std::iter::once(packed).chain(tables).collect();
    let payloads: Vec<_> = raw_payloads.iter().map(|raw| compress_block(raw)).collect();
    let metadata = HEADER
        .checked_add(product(level_counts.len(), 4)?)
        .and_then(|v| v.checked_add(product(columns.len(), 4).ok()?))
        .and_then(|v| v.checked_add(product(payloads.len(), DIRECTORY_ENTRY).ok()?))
        .and_then(|v| v.checked_add(4))
        .ok_or_else(|| invalid("metadata overflow"))?;
    let mut out = if dictionary { MAGIC } else { INDEPENDENT_MAGIC }.to_vec();
    for n in [rows, unique, level_counts.len(), columns.len()] {
        out.extend_from_slice(&u32_size(n)?.to_le_bytes());
    }
    out.push(bits as u8);
    out.extend_from_slice(&[0; 7]);
    for &count in level_counts {
        out.extend_from_slice(&count.to_le_bytes());
    }
    for (width, _) in columns {
        out.extend_from_slice(&u32_size(*width)?.to_le_bytes());
    }
    let mut offset = metadata as u64;
    for (raw, stored) in raw_payloads.iter().zip(&payloads) {
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&(stored.bytes().len() as u64).to_le_bytes());
        out.extend_from_slice(&(raw.len() as u64).to_le_bytes());
        out.push(stored.codec() as u8);
        out.extend_from_slice(&crc32(stored.bytes()).to_le_bytes());
        offset = offset
            .checked_add(stored.bytes().len() as u64)
            .ok_or_else(|| invalid("offset overflow"))?;
    }
    out.extend_from_slice(&crc32(&out).to_le_bytes());
    debug_assert_eq!(out.len(), metadata);
    for stored in payloads {
        out.extend_from_slice(stored.bytes());
    }
    Ok(out)
}

/// Requested dictionary column and shared IDs only; other attribute tables are never read.
/// Access by depth/node remains indirect but constant-time once these buffers are resident.
#[derive(Debug)]
pub struct ColumnProjection {
    level_offsets: Vec<usize>,
    width: usize,
    unique: usize,
    bits: usize,
    table: Vec<u8>,
    ids: Vec<u8>,
    independent: bool,
}
impl ColumnProjection {
    pub fn record_width(&self) -> usize {
        self.width
    }
    pub fn unique_count(&self) -> usize {
        self.unique
    }
    pub fn level_counts(&self) -> impl Iterator<Item = usize> + '_ {
        self.level_offsets.windows(2).map(|w| w[1] - w[0])
    }
    pub fn row_count(&self) -> usize {
        *self.level_offsets.last().expect("offset sentinel")
    }
    fn id(&self, row: usize) -> usize {
        if self.independent {
            return row;
        }
        let mut id = 0_usize;
        for bit in 0..self.bits {
            let offset = row * self.bits + bit;
            id |= usize::from((self.ids[offset / 8] >> (offset % 8)) & 1) << bit;
        }
        id
    }
    pub fn record(&self, depth: usize, index: usize) -> Option<&[u8]> {
        let start = *self.level_offsets.get(depth)?;
        let end = *self.level_offsets.get(depth.checked_add(1)?)?;
        if index >= end - start {
            return None;
        }
        let id = self.id(start + index);
        Some(&self.table[id * self.width..(id + 1) * self.width])
    }
    /// Expands only the selected column. This allocation is separate from reader limits.
    pub fn expand(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.row_count() * self.width);
        for row in 0..self.row_count() {
            let id = self.id(row);
            out.extend_from_slice(&self.table[id * self.width..(id + 1) * self.width]);
        }
        out
    }
}

#[derive(Clone, Copy)]
struct Payload {
    offset: u64,
    stored: usize,
    raw: usize,
    codec: BlockCodec,
    crc: u32,
}
fn read_u32(reader: &mut impl Read) -> Result<usize> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes) as usize)
}
fn read_payload(
    reader: &mut (impl Read + Seek),
    payload: Payload,
    limit: usize,
) -> Result<Vec<u8>> {
    if payload.raw > limit || payload.stored > limit {
        return Err(invalid("payload limit"));
    }
    reader.seek(SeekFrom::Start(payload.offset))?;
    let mut stored = vec![0; payload.stored];
    reader.read_exact(&mut stored)?;
    if crc32(&stored) != payload.crc {
        return Err(invalid("payload CRC"));
    }
    Ok(decompress_block(
        payload.codec,
        &stored,
        payload.raw,
        limit,
    )?)
}

/// Read a standalone experimental container from offset zero. Validates metadata, bounds,
/// selected CRCs, all shared IDs and final padding before exposing indexed access.
/// `limit` caps metadata, each selected buffer, and expanded column size separately, not RSS.
pub fn read_column(
    reader: &mut (impl Read + Seek),
    column: usize,
    limit: usize,
) -> Result<ColumnProjection> {
    reader.seek(SeekFrom::Start(0))?;
    let mut header = [0_u8; HEADER];
    reader.read_exact(&mut header)?;
    let independent = &header[..8] == INDEPENDENT_MAGIC;
    if (!independent && &header[..8] != MAGIC) || header[25..].iter().any(|&v| v != 0) {
        return Err(invalid("header"));
    }
    let field =
        |offset| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap()) as usize;
    let (rows, unique, levels, columns) = (field(8), field(12), field(16), field(20));
    let bits = usize::from(header[24]);
    let expected_bits = if independent { 0 } else { bits_for(unique) };
    if unique > rows
        || (rows != 0 && unique == 0)
        || (independent && unique != rows)
        || bits != expected_bits
        || column >= columns
    {
        return Err(invalid("counts/column/bits"));
    }
    let entries = columns
        .checked_add(1)
        .ok_or_else(|| invalid("directory overflow"))?;
    let metadata = HEADER
        .checked_add(product(levels, 4)?)
        .and_then(|v| v.checked_add(product(columns, 4).ok()?))
        .and_then(|v| v.checked_add(product(entries, DIRECTORY_ENTRY).ok()?))
        .and_then(|v| v.checked_add(4))
        .ok_or_else(|| invalid("metadata overflow"))?;
    if metadata > limit {
        return Err(invalid("metadata limit"));
    }
    let mut metadata_bytes = vec![0; metadata];
    metadata_bytes[..HEADER].copy_from_slice(&header);
    reader.read_exact(&mut metadata_bytes[HEADER..])?;
    let expected_crc = u32::from_le_bytes(metadata_bytes[metadata - 4..].try_into().unwrap());
    if crc32(&metadata_bytes[..metadata - 4]) != expected_crc {
        return Err(invalid("metadata CRC"));
    }
    let mut directory = std::io::Cursor::new(&metadata_bytes[HEADER..metadata - 4]);
    let mut offsets = vec![0_usize];
    for _ in 0..levels {
        let count = read_u32(&mut directory)?;
        offsets.push(
            offsets
                .last()
                .unwrap()
                .checked_add(count)
                .ok_or_else(|| invalid("row overflow"))?,
        );
    }
    if offsets.last() != Some(&rows) {
        return Err(invalid("level counts"));
    }
    let mut widths = Vec::with_capacity(columns);
    for _ in 0..columns {
        let width = read_u32(&mut directory)?;
        if width == 0 {
            return Err(invalid("zero width"));
        }
        widths.push(width);
    }
    if product(rows, widths[column])? > limit {
        return Err(invalid("expanded column limit"));
    }
    let mut payloads = Vec::with_capacity(entries);
    let mut expected_offset = metadata as u64;
    for entry in 0..entries {
        let mut bytes = [0; DIRECTORY_ENTRY];
        directory.read_exact(&mut bytes)?;
        let u64_field = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let offset = u64_field(0);
        let stored = usize::try_from(u64_field(8)).map_err(|_| invalid("stored overflow"))?;
        let raw = usize::try_from(u64_field(16)).map_err(|_| invalid("raw overflow"))?;
        let codec = match bytes[24] {
            0 => BlockCodec::None,
            1 => BlockCodec::Lz4,
            _ => return Err(invalid("codec")),
        };
        let expected_raw = if entry == 0 {
            product(rows, bits)?.div_ceil(8)
        } else {
            product(unique, widths[entry - 1])?
        };
        if offset != expected_offset
            || raw != expected_raw
            || stored > raw
            || (codec == BlockCodec::None && stored != raw)
        {
            return Err(invalid("payload directory"));
        }
        expected_offset = offset
            .checked_add(stored as u64)
            .ok_or_else(|| invalid("offset overflow"))?;
        payloads.push(Payload {
            offset,
            stored,
            raw,
            codec,
            crc: u32::from_le_bytes(bytes[25..29].try_into().unwrap()),
        });
    }
    if reader.seek(SeekFrom::End(0))? != expected_offset {
        return Err(invalid("file length"));
    }
    let ids = read_payload(reader, payloads[0], limit)?;
    let table = read_payload(reader, payloads[column + 1], limit)?;
    let projection = ColumnProjection {
        level_offsets: offsets,
        width: widths[column],
        unique,
        bits,
        table,
        ids,
        independent,
    };
    for row in 0..rows {
        if projection.id(row) >= unique {
            return Err(invalid("dictionary ID"));
        }
    }
    let used = product(rows, bits)? % 8;
    if used != 0 && projection.ids.last().is_some_and(|&byte| byte >> used != 0) {
        return Err(invalid("ID padding"));
    }
    Ok(projection)
}
