//! Pre3 attribute-only transforms. Width/row count come from the existing schema/directory.
use std::{collections::HashMap, hash::Hash};

use crate::{BlockCodec, EncodedBlock, FormatError, compress_block, decompress_block};

const HEADER: usize = 10;

fn malformed() -> FormatError {
    FormatError::MalformedBlock("invalid attribute dictionary")
}

fn allocate(size: usize) -> Result<Vec<u8>, FormatError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| FormatError::AllocationFailed(size))?;
    Ok(bytes)
}

fn id_bits(unique: usize) -> usize {
    (32 - (unique as u32).saturating_sub(1).leading_zeros()) as usize
}

// IDs are LSB-first, at most 32 bits. A byte-fed u64 holds at most 39 bits.
fn pack_ids(ids: impl ExactSizeIterator<Item = u32>, bits: usize) -> Result<Vec<u8>, FormatError> {
    let size = ids
        .len()
        .checked_mul(bits)
        .ok_or(FormatError::BlockSizeOverflow)?
        .div_ceil(8);
    let mut packed = allocate(size)?;
    let mut buffer = 0_u64;
    let mut buffered = 0;
    for id in ids {
        buffer |= u64::from(id) << buffered;
        buffered += bits;
        while buffered >= 8 {
            packed.push(buffer as u8);
            buffer >>= 8;
            buffered -= 8;
        }
    }
    if buffered != 0 {
        packed.push(buffer as u8);
    }
    Ok(packed)
}

struct PackedIds<'a> {
    bytes: std::slice::Iter<'a, u8>,
    bits: usize,
    buffer: u64,
    buffered: usize,
}

impl PackedIds<'_> {
    fn read(&mut self) -> Result<usize, FormatError> {
        while self.buffered < self.bits {
            self.buffer |= u64::from(*self.bytes.next().ok_or_else(malformed)?) << self.buffered;
            self.buffered += 8;
        }
        let id = self.buffer & ((1_u64 << self.bits) - 1);
        self.buffer >>= self.bits;
        self.buffered -= self.bits;
        Ok(id as usize)
    }
}

/// Fill a byte palette in first-occurrence order and return byte-to-ID lookup.
fn byte_dictionary(raw: &[u8], table: &mut Vec<u8>) -> [u16; 256] {
    debug_assert!(table.is_empty(), "{table:?}");
    let mut lookup = [u16::MAX; 256];
    for &value in raw {
        let id = &mut lookup[usize::from(value)];
        if *id == u16::MAX {
            *id = table.len() as u16; // At most 256 distinct byte values.
            table.push(value);
        }
    }
    lookup
}

fn encode_raw_or_constant(raw: &[u8], width: usize) -> Result<EncodedBlock, FormatError> {
    if width == 0 || !raw.len().is_multiple_of(width) {
        return Err(malformed());
    }
    let best = compress_block(raw);
    let rows = raw.len() / width;
    if rows < 2 || rows > u32::MAX as usize {
        return Ok(best);
    }
    if raw.chunks_exact(width).all(|row| row == &raw[..width]) && width < best.bytes().len() {
        return Ok(EncodedBlock {
            codec: BlockCodec::Constant,
            bytes: raw[..width].to_vec(),
        });
    }
    Ok(best)
}

pub(crate) fn encode(raw: &[u8], width: usize) -> Result<EncodedBlock, FormatError> {
    let mut best = encode_raw_or_constant(raw, width)?;
    let rows = raw.len() / width;
    if rows < 2 || rows > u32::MAX as usize || best.codec() == BlockCodec::Constant {
        return Ok(best);
    }
    let mut table = allocate(if width == 1 { rows.min(256) } else { raw.len() })?;
    let (unique, packed) = if width == 1 {
        let lookup = byte_dictionary(raw, &mut table);
        let unique = table.len();
        let bits = id_bits(unique);
        let packed = pack_ids(
            raw.iter()
                .map(|value| u32::from(lookup[usize::from(*value)])),
            bits,
        )?;
        (unique, packed)
    } else if width <= 8 {
        dictionary_ids(raw, width, &mut table, row_key)?
    } else {
        dictionary_ids(raw, width, &mut table, |row| row)?
    };
    let compressed_table = compress_block(&table);
    drop(table);
    let compressed_packed = compress_block(&packed);
    drop(packed);
    let length = HEADER
        .checked_add(compressed_table.bytes().len())
        .and_then(|n| n.checked_add(compressed_packed.bytes().len()))
        .ok_or(FormatError::BlockSizeOverflow)?;
    if length < best.bytes().len() && u32::try_from(compressed_table.bytes().len()).is_ok() {
        let mut bytes = allocate(length)?;
        bytes.extend_from_slice(&(unique as u32).to_le_bytes());
        bytes.push(inner_tag(compressed_table.codec()));
        bytes.push(inner_tag(compressed_packed.codec()));
        bytes.extend_from_slice(&(compressed_table.bytes().len() as u32).to_le_bytes());
        bytes.extend_from_slice(compressed_table.bytes());
        bytes.extend_from_slice(compressed_packed.bytes());
        best = EncodedBlock {
            codec: BlockCodec::Dictionary,
            bytes,
        };
    }
    Ok(best)
}

/// Dictionary inner tag: 0 raw, 1 LZ4.
fn inner_tag(codec: BlockCodec) -> u8 {
    match codec {
        BlockCodec::None => 0,
        BlockCodec::Lz4 => 1,
        _ => unreachable!("dictionary parts are stored raw or LZ4"),
    }
}

fn dictionary_ids<'a, K: Eq + Hash>(
    raw: &'a [u8],
    width: usize,
    table: &mut Vec<u8>,
    key: impl Fn(&'a [u8]) -> K,
) -> Result<(usize, Vec<u8>), FormatError> {
    let rows = raw.len() / width;
    let mut lookup = HashMap::<K, u32>::new();
    lookup
        .try_reserve(rows)
        .map_err(|_| FormatError::AllocationFailed(rows))?;
    let mut ids = Vec::new();
    ids.try_reserve_exact(rows)
        .map_err(|_| FormatError::AllocationFailed(rows))?;
    // IDs and the serialized palette retain the original first-occurrence order.
    for row in raw.chunks_exact(width) {
        let next = lookup.len() as u32;
        ids.push(*lookup.entry(key(row)).or_insert_with(|| {
            table.extend_from_slice(row);
            next
        }));
    }
    let unique = table.len() / width;
    drop(lookup);
    let packed = pack_ids(ids.iter().copied(), id_bits(unique))?;
    Ok((unique, packed))
}

#[inline]
// Fixed-width loads avoid a variable-size copy on every dictionary lookup.
// Zero padding is injective because each dictionary has one fixed tuple width.
fn row_key(row: &[u8]) -> u64 {
    match row.len() {
        2 => u64::from(u16::from_le_bytes([row[0], row[1]])),
        3 => u64::from(u32::from_le_bytes([row[0], row[1], row[2], 0])),
        4 => u64::from(u32::from_le_bytes([row[0], row[1], row[2], row[3]])),
        5 => u64::from_le_bytes([row[0], row[1], row[2], row[3], row[4], 0, 0, 0]),
        6 => u64::from_le_bytes([row[0], row[1], row[2], row[3], row[4], row[5], 0, 0]),
        7 => u64::from_le_bytes([row[0], row[1], row[2], row[3], row[4], row[5], row[6], 0]),
        8 => u64::from_le_bytes(row.try_into().expect("validated tuple width")),
        _ => unreachable!("integer dictionary needs a width from two to eight"),
    }
}

/// Pre4 keeps every pre3 candidate, adding a reversible independent byte-plane transform. Retired
/// with the writers that used it; the tests keep it as the reference the V8 encoder is compared to.
#[cfg(test)]
pub(crate) fn encode_pre4(raw: &[u8], width: usize) -> Result<EncodedBlock, FormatError> {
    encode_byte_planes(raw, width, encode(raw, width)?)
}

/// V7.3 keeps dictionary candidates only for narrow 1/2/4-byte tuples.
/// The alternative candidates and their tie order stay identical to pre4.
pub(crate) fn encode_v7_pre3(raw: &[u8], width: usize) -> Result<EncodedBlock, FormatError> {
    let best = if matches!(width, 1 | 2 | 4) {
        encode(raw, width)?
    } else {
        encode_raw_or_constant(raw, width)?
    };
    encode_byte_planes(raw, width, best)
}

fn encode_byte_planes(
    raw: &[u8],
    width: usize,
    best: EncodedBlock,
) -> Result<EncodedBlock, FormatError> {
    if width <= 1 || raw.len() / width < 2 || best.codec() == BlockCodec::Constant {
        return Ok(best);
    }
    let rows = raw.len() / width;
    let mut planes = allocate(raw.len())?;
    planes.resize(raw.len(), 0);
    for byte in 0..width {
        for row in 0..rows {
            planes[byte * rows + row] = raw[row * width + byte];
        }
    }
    let encoded = compress_block(&planes);
    if encoded.codec() == BlockCodec::Lz4 && encoded.bytes().len() < best.bytes().len() {
        Ok(EncodedBlock {
            codec: BlockCodec::BytePlanes,
            bytes: encoded.into_bytes(),
        })
    } else {
        Ok(best)
    }
}

pub(crate) fn decode(
    codec: BlockCodec,
    stored: &[u8],
    raw_size: usize,
    width: usize,
    limit: usize,
) -> Result<Vec<u8>, FormatError> {
    if raw_size > limit {
        return Err(crate::CodecError::BlockExceedsLimit {
            requested: raw_size,
            limit,
        }
        .into());
    }
    if width == 0 || !raw_size.is_multiple_of(width) {
        return Err(malformed());
    }
    if matches!(codec, BlockCodec::None | BlockCodec::Lz4) {
        return Ok(decompress_block(codec, stored, raw_size, limit)?);
    }
    let rows = raw_size / width;
    if rows < 2 || stored.len() >= raw_size {
        return Err(malformed());
    }
    if codec == BlockCodec::Constant {
        if stored.len() != width {
            return Err(malformed());
        }
        let mut output = allocate(raw_size)?;
        for _ in 0..rows {
            output.extend_from_slice(stored);
        }
        return Ok(output);
    }
    if codec == BlockCodec::BytePlanes {
        if width <= 1 {
            return Err(malformed());
        }
        let planes = decompress_block(BlockCodec::Lz4, stored, raw_size, limit)?;
        let mut output = allocate(raw_size)?;
        output.resize(raw_size, 0);
        for byte in 0..width {
            for row in 0..rows {
                output[row * width + byte] = planes[byte * rows + row];
            }
        }
        return Ok(output);
    }
    if codec != BlockCodec::Dictionary {
        return Err(malformed());
    }
    if stored.len() < HEADER {
        return Err(malformed());
    }
    let unique = u32::from_le_bytes(stored[..4].try_into().unwrap()) as usize;
    if unique == 0 || unique > rows {
        return Err(malformed());
    }
    let table_size = unique.checked_mul(width).ok_or_else(malformed)?;
    let bits = id_bits(unique);
    let packed_size = rows.checked_mul(bits).ok_or_else(malformed)?.div_ceil(8);
    // Both intermediate raw sizes are bounded individually before any decoding allocation.
    if table_size > raw_size || packed_size > raw_size || packed_size > limit {
        return Err(malformed());
    }
    let table_stored = u32::from_le_bytes(stored[6..10].try_into().unwrap()) as usize;
    let split = HEADER.checked_add(table_stored).ok_or_else(malformed)?;
    if split > stored.len() {
        return Err(malformed());
    }
    let inner = |tag| match tag {
        0 => Ok(BlockCodec::None),
        1 => Ok(BlockCodec::Lz4),
        _ => Err(malformed()),
    };
    let table = decompress_block(inner(stored[4])?, &stored[HEADER..split], table_size, limit)?;
    let packed = decompress_block(inner(stored[5])?, &stored[split..], packed_size, limit)?;
    let used = (rows * bits) % 8;
    if used != 0 && packed.last().is_some_and(|byte| byte >> used != 0) {
        return Err(malformed());
    }
    let mut output = allocate(raw_size)?;
    let mut ids = PackedIds {
        bytes: packed.iter(),
        bits,
        buffer: 0,
        buffered: 0,
    };
    for _ in 0..rows {
        let id = ids.read()?;
        if id >= unique {
            return Err(malformed());
        }
        output.extend_from_slice(&table[id * width..(id + 1) * width]);
    }
    Ok(output)
}

#[cfg(test)]
#[path = "attribute_codec_tests.rs"]
mod tests;
