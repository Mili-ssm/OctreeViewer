//! Pre3 attribute-only transforms. Width/row count come from the existing schema/directory.
use std::collections::HashMap;

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

pub(crate) fn encode(raw: &[u8], width: usize) -> Result<EncodedBlock, FormatError> {
    if width == 0 || !raw.len().is_multiple_of(width) {
        return Err(malformed());
    }
    let mut best = compress_block(raw);
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
    let mut lookup = HashMap::<&[u8], u32>::new();
    lookup
        .try_reserve(rows)
        .map_err(|_| FormatError::AllocationFailed(rows))?;
    let mut table = allocate(raw.len())?;
    let mut ids = Vec::new();
    ids.try_reserve_exact(rows)
        .map_err(|_| FormatError::AllocationFailed(rows))?;
    for row in raw.chunks_exact(width) {
        let next = lookup.len() as u32;
        ids.push(*lookup.entry(row).or_insert_with(|| {
            table.extend_from_slice(row);
            next
        }));
    }
    let unique = lookup.len();
    let bits = id_bits(unique);
    let size = rows
        .checked_mul(bits)
        .ok_or(FormatError::BlockSizeOverflow)?
        .div_ceil(8);
    let mut packed = allocate(size)?;
    packed.resize(size, 0);
    for (row, id) in ids.into_iter().enumerate() {
        for bit in 0..bits {
            let offset = row * bits + bit;
            packed[offset / 8] |= (((id >> bit) & 1) as u8) << (offset % 8);
        }
    }
    let table = compress_block(&table);
    let packed = compress_block(&packed);
    let length = HEADER
        .checked_add(table.bytes().len())
        .and_then(|n| n.checked_add(packed.bytes().len()))
        .ok_or(FormatError::BlockSizeOverflow)?;
    if length < best.bytes().len() && u32::try_from(table.bytes().len()).is_ok() {
        let mut bytes = allocate(length)?;
        bytes.extend_from_slice(&(unique as u32).to_le_bytes());
        bytes.push(table.codec() as u8);
        bytes.push(packed.codec() as u8);
        bytes.extend_from_slice(&(table.bytes().len() as u32).to_le_bytes());
        bytes.extend_from_slice(table.bytes());
        bytes.extend_from_slice(packed.bytes());
        best = EncodedBlock {
            codec: BlockCodec::Dictionary,
            bytes,
        };
    }
    Ok(best)
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
    for row in 0..rows {
        let mut id = 0_usize;
        for bit in 0..bits {
            let offset = row * bits + bit;
            id |= usize::from((packed[offset / 8] >> (offset % 8)) & 1) << bit;
        }
        if id >= unique {
            return Err(malformed());
        }
        output.extend_from_slice(&table[id * width..(id + 1) * width]);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_bits_components_and_fallback() {
        for width in [1, 3, 8, 16] {
            for rows in [0, 1, 7, 8, 9, 257, 4096] {
                for unique in [1, 3, 17, 257] {
                    let raw: Vec<u8> = (0..rows)
                        .flat_map(|r| {
                            (0..width).map(move |c| ((r % unique * 37 + c * 11) % 256) as u8)
                        })
                        .collect();
                    let encoded = encode(&raw, width).unwrap();
                    assert!(encoded.bytes().len() <= compress_block(&raw).bytes().len());
                    assert_eq!(
                        decode(
                            encoded.codec(),
                            encoded.bytes(),
                            raw.len(),
                            width,
                            raw.len()
                        )
                        .unwrap(),
                        raw
                    );
                }
            }
        }
        let constant = encode(&[255; 4096], 1).unwrap();
        assert_eq!(constant.codec(), BlockCodec::Constant);
        let raw: Vec<u8> = (0..4096)
            .flat_map(|r| ((r * 37 % 17) as u64).to_le_bytes())
            .collect();
        assert_eq!(encode(&raw, 8).unwrap().codec(), BlockCodec::Dictionary);
    }
    #[test]
    fn rejects_invalid_shapes_nested_codecs_ids_padding_and_limits() {
        let bad = |bytes: &[u8], rows: usize| {
            decode(BlockCodec::Dictionary, bytes, rows * 8, 8, rows * 8).is_err()
        };
        // Three exact eight-byte entries, 9 two-bit IDs: last six padding bits must be zero.
        let mut stream = vec![3, 0, 0, 0, 0, 0, 24, 0, 0, 0];
        stream.extend_from_slice(&[0; 24]);
        stream.extend_from_slice(&[0, 0, 0]);
        assert!(!bad(&stream, 9));
        for size in 0..stream.len() {
            assert!(bad(&stream[..size], 9));
        }
        for (offset, value) in [(0, 0), (0, 10), (4, 2), (5, 3), (6, 255), (34, 3), (36, 4)] {
            let mut corrupt = stream.clone();
            corrupt[offset] = value;
            assert!(bad(&corrupt, 9));
        }
        assert!(decode(BlockCodec::Constant, &[0], 100, 1, 99).is_err());
        assert!(decode(BlockCodec::Constant, &[0, 1], 100, 1, 100).is_err());
    }
}
