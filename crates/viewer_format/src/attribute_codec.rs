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
    debug_assert!(table.is_empty());
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
        bytes.push(compressed_table.codec() as u8);
        bytes.push(compressed_packed.codec() as u8);
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

/// Pre4 keeps every pre3 candidate, adding a reversible independent byte-plane transform.
pub(crate) fn encode_pre4(raw: &[u8], width: usize) -> Result<EncodedBlock, FormatError> {
    let best = encode(raw, width)?;
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
mod tests {
    use super::*;

    fn old_slice_encoder(raw: &[u8], width: usize) -> Result<EncodedBlock, FormatError> {
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
        let mut table = allocate(raw.len())?;
        let mut lookup = HashMap::<&[u8], u32>::new();
        lookup
            .try_reserve(rows)
            .map_err(|_| FormatError::AllocationFailed(rows))?;
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
        let unique = table.len() / width;
        let bits = id_bits(unique);
        drop(lookup);
        let packed = pack_ids(ids.iter().copied(), bits)?;
        drop(ids);
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
            bytes.push(compressed_table.codec() as u8);
            bytes.push(compressed_packed.codec() as u8);
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

    #[test]
    fn integer_dictionary_matches_old_slice_encoder_byte_for_byte() {
        let mut cases: Vec<(usize, Vec<u8>)> = Vec::new();
        for width in [2, 3, 4, 5, 6, 7, 8, 9, 16] {
            for rows in [0, 1, 2, 19, 257] {
                cases.push((
                    width,
                    (0..rows * width)
                        .map(|i| (i * 73 + width * 11) as u8)
                        .collect(),
                ));
            }
            cases.push((width, vec![0x5a; width * 257]));
            cases.push((
                width,
                (0..257)
                    .flat_map(|i| (0..width).map(move |b| ((i % 3) * 31 + b) as u8))
                    .collect(),
            ));
            cases.push((
                width,
                (0..257 * width)
                    .map(|i| ((i * 131 + i / width * 17) >> 2) as u8)
                    .collect(),
            ));
        }
        // Distinct IEEE encodings, including signed zeros and NaN payloads.
        let patterns = [
            0x0000000000000000_u64,
            0x8000000000000000,
            0x7ff8000000000001,
            0x7ff8000000000042,
        ];
        cases.push((
            8,
            (0..257)
                .flat_map(|i| patterns[i % patterns.len()].to_le_bytes())
                .collect(),
        ));
        let mut state = 0x1234_5678_u32;
        for width in [2, 3, 4, 6, 8, 12] {
            let data = (0..4096 * width)
                .map(|_| {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    (state >> 24) as u8
                })
                .collect();
            cases.push((width, data));
        }
        for (width, raw) in cases {
            let expected = old_slice_encoder(&raw, width).unwrap();
            let actual = encode(&raw, width).unwrap();
            assert_eq!(
                actual.codec(),
                expected.codec(),
                "width={width}, rows={}",
                raw.len() / width
            );
            assert_eq!(
                actual.bytes(),
                expected.bytes(),
                "width={width}, rows={}",
                raw.len() / width
            );
        }
    }

    #[test]
    #[ignore = "focused full attribute encoder A/B benchmark"]
    fn integer_dictionary_full_encoder_benchmark() {
        use std::{hint::black_box, time::Instant};
        let rows = 262_144;
        for width in [2, 3, 6, 8] {
            for unique in [4, 256, if width == 2 { 65_536 } else { rows }] {
                let raw: Vec<u8> = (0..rows)
                    .flat_map(|row| {
                        let value = ((row % unique) as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                        value.to_le_bytes()[..width].to_vec()
                    })
                    .collect();
                for repetition in 0..3 {
                    for old in if repetition % 2 == 0 {
                        [true, false]
                    } else {
                        [false, true]
                    } {
                        let input = black_box(&raw);
                        let started = Instant::now();
                        let encoded = if old {
                            old_slice_encoder(input, width)
                        } else {
                            encode(input, width)
                        }
                        .unwrap();
                        black_box((encoded.codec(), encoded.bytes()));
                        println!(
                            "width={width} unique={unique} rep={} old={old} elapsed_ms={:.3}",
                            repetition + 1,
                            started.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                }
            }
        }
    }

    fn previous_byte_dictionary(raw: &[u8]) -> EncodedBlock {
        let mut best = compress_block(raw);
        if !raw.is_empty() && raw.iter().all(|value| *value == raw[0]) && 1 < best.bytes().len() {
            return EncodedBlock {
                codec: BlockCodec::Constant,
                bytes: vec![raw[0]],
            };
        }
        let mut lookup = HashMap::<u8, u32>::new();
        let mut table = Vec::new();
        let mut ids = Vec::with_capacity(raw.len());
        for &value in raw {
            let next = lookup.len() as u32;
            ids.push(*lookup.entry(value).or_insert_with(|| {
                table.push(value);
                next
            }));
        }
        let unique = table.len();
        let table = compress_block(&table);
        let packed = compress_block(&pack_ids(ids.iter().copied(), id_bits(unique)).unwrap());
        let length = HEADER + table.bytes().len() + packed.bytes().len();
        if length < best.bytes().len() && u32::try_from(table.bytes().len()).is_ok() {
            let mut bytes = Vec::with_capacity(length);
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
        best
    }

    #[test]
    fn byte_dictionary_encoding_matches_previous_payload_for_all_cardinalities() {
        for unique in 1..=256 {
            for rows in [0, 1, 7, 257, 4096] {
                let raw: Vec<_> = (0..rows)
                    .map(|row| ((unique - 1 - row % unique) * 73 % 256) as u8)
                    .collect();
                let expected = previous_byte_dictionary(&raw);
                let actual = encode(&raw, 1).unwrap();
                assert_eq!(
                    actual.codec(),
                    expected.codec(),
                    "unique={unique} rows={rows}"
                );
                assert_eq!(
                    actual.bytes(),
                    expected.bytes(),
                    "unique={unique} rows={rows}"
                );
                assert_eq!(
                    decode(actual.codec(), actual.bytes(), raw.len(), 1, raw.len()).unwrap(),
                    raw
                );
            }
        }
    }
    #[test]
    #[ignore = "isolated dictionary construction benchmark; run with --profile perf"]
    fn byte_dictionary_lookup_benchmark() {
        use std::{hint::black_box, time::Instant};
        let rows = 262_144;
        for unique in [2, 3, 256] {
            let raw: Vec<_> = (0..rows)
                .map(|row| (unique - 1 - row % unique) as u8)
                .collect();
            for repetition in 0..9 {
                for direct in if repetition % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let input = black_box(&raw);
                    let started = Instant::now();
                    let mut lookup = HashMap::<&[u8], u32>::new();
                    if !direct {
                        lookup.try_reserve(rows).unwrap();
                    }
                    let mut table = allocate(if direct { 256 } else { rows }).unwrap();
                    let mut ids = Vec::with_capacity(rows);
                    if direct {
                        let lookup = byte_dictionary(input, &mut table);
                        ids.extend(
                            input
                                .iter()
                                .map(|value| u32::from(lookup[usize::from(*value)])),
                        );
                    } else {
                        for row in input.chunks_exact(black_box(input.len() / rows)) {
                            let next = lookup.len() as u32;
                            ids.push(*lookup.entry(row).or_insert_with(|| {
                                table.extend_from_slice(row);
                                next
                            }));
                        }
                    }
                    black_box((&table, &ids));
                    let elapsed = started.elapsed().as_secs_f64();
                    assert_eq!(table.len(), unique);
                    for (row, &id) in ids.iter().enumerate() {
                        assert_eq!(table[id as usize], raw[row]);
                    }
                    println!(
                        "unique={unique} rows={rows} rep={} direct={direct} elapsed_ms={:.6}",
                        repetition + 1,
                        elapsed * 1000.0
                    );
                }
            }
        }
    }

    #[test]
    fn byte_dictionary_matches_hashmap_palette_order_for_all_cardinalities() {
        for unique in 1..=256 {
            for rows in [0, 1, 7, 257, 4096] {
                // Reverse first occurrences so value order cannot substitute for ID order.
                let raw: Vec<_> = (0..rows)
                    .map(|row| (unique - 1 - row % unique) as u8)
                    .collect();
                let mut lookup = HashMap::new();
                let mut expected_table = Vec::new();
                let expected_ids: Vec<_> = raw
                    .iter()
                    .map(|&value| {
                        let next = lookup.len() as u32;
                        *lookup.entry(value).or_insert_with(|| {
                            expected_table.push(value);
                            next
                        })
                    })
                    .collect();
                let mut table = allocate(raw.len().min(256)).unwrap();
                let lookup = byte_dictionary(&raw, &mut table);
                let ids: Vec<_> = raw
                    .iter()
                    .map(|&value| u32::from(lookup[usize::from(value)]))
                    .collect();
                assert_eq!(table, expected_table);
                assert_eq!(ids, expected_ids);
                assert!(table.capacity() <= 256);
                let encoded = encode_pre4(&raw, 1).unwrap();
                assert_eq!(
                    decode(encoded.codec(), encoded.bytes(), raw.len(), 1, raw.len()).unwrap(),
                    raw
                );
            }
        }
    }

    #[test]
    fn packed_ids_match_independent_bit_oracle_at_every_width() {
        for bits in 0_usize..=32 {
            for rows in [0, 1, 7, 8, 9, 257] {
                let mask = ((1_u64 << bits) - 1) as u32;
                let ids: Vec<_> = (0..rows)
                    .map(|row| (row as u32).wrapping_mul(0x9e3779b9) & mask)
                    .collect();
                let mut oracle = vec![0; (rows * bits).div_ceil(8)];
                for (row, &id) in ids.iter().enumerate() {
                    for bit in 0..bits {
                        let offset = row * bits + bit;
                        oracle[offset / 8] |= (((id >> bit) & 1) as u8) << (offset % 8);
                    }
                }
                assert_eq!(pack_ids(ids.iter().copied(), bits).unwrap(), oracle);
                let mut reader = PackedIds {
                    bytes: oracle.iter(),
                    bits,
                    buffer: 0,
                    buffered: 0,
                };
                for id in ids {
                    assert_eq!(reader.read().unwrap(), id as usize);
                }
                if !oracle.is_empty() {
                    let mut truncated = PackedIds {
                        bytes: oracle[..oracle.len() - 1].iter(),
                        bits,
                        buffer: 0,
                        buffered: 0,
                    };
                    assert!((0..rows).any(|_| truncated.read().is_err()));
                }
            }
        }
    }

    #[test]
    fn pre4_byte_planes_preserve_bits_and_never_exceed_pre3() {
        for width in [1, 3, 8, 16] {
            for rows in [0, 1, 9, 257, 4096] {
                let raw: Vec<_> = (0..rows)
                    .flat_map(|r| {
                        (0..width).map(move |c| {
                            if c % 2 == 0 {
                                (r * 37 + c * 11) as u8
                            } else {
                                c as u8
                            }
                        })
                    })
                    .collect();
                let old = encode(&raw, width).unwrap();
                let new = encode_pre4(&raw, width).unwrap();
                assert!(new.bytes().len() <= old.bytes().len());
                assert_eq!(
                    decode(new.codec(), new.bytes(), raw.len(), width, raw.len()).unwrap(),
                    raw
                );
            }
        }
        let mut state = 1234_u32;
        let raw: Vec<_> = (0..4096)
            .flat_map(|_| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                (0x7ff8000000000000_u64 | u64::from(state)).to_le_bytes()
            })
            .collect();
        let new = encode_pre4(&raw, 8).unwrap();
        assert_eq!(new.codec(), BlockCodec::BytePlanes);
        assert_eq!(
            decode(new.codec(), new.bytes(), raw.len(), 8, raw.len()).unwrap(),
            raw
        );
        assert!(decode(new.codec(), new.bytes(), raw.len(), 8, raw.len() - 1).is_err());
        assert!(decode(new.codec(), new.bytes(), raw.len(), 1, raw.len()).is_err());
        assert!(
            decode(
                new.codec(),
                &new.bytes()[..new.bytes().len() - 1],
                raw.len(),
                8,
                raw.len()
            )
            .is_err()
        );
    }
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
