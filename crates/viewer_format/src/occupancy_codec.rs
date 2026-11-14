//! v6.2-pre physical geometry stream: six-byte nodes in contiguous depth arrays.
use crate::{FormatError, QuantizedOccupancy};

const NODE_BYTES: usize = 6;
const OWN_OCTANT: u8 = 0x40;

/// OQ-09 experiment: `OQ09_MORTON=1` writes every block sorted by Morton code with no row index;
/// `OQ09_MORTON=best` tries both orders per block and keeps the smaller geometry.
pub(crate) fn morton_best() -> bool {
    static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("OQ09_MORTON").is_ok_and(|v| v == "best"))
}

thread_local! {
    /// Per-thread order override used by `best` mode while a block is prepared.
    pub(crate) static MORTON_OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

pub(crate) fn morton() -> bool {
    if let Some(v) = MORTON_OVERRIDE.with(|c| c.get()) {
        return v;
    }
    static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("OQ09_MORTON").is_ok_and(|v| v != "best"))
}
pub(crate) const MORTON_FLAG: u8 = 0x80;

fn malformed() -> FormatError {
    FormatError::MalformedBlock("invalid occupied geometry")
}

fn var_len(value: u32) -> usize {
    ((32 - value.leading_zeros()).max(1) as usize).div_ceil(7)
}

fn write_var(mut value: u32, bytes: &mut Vec<u8>) {
    while value >= 128 {
        bytes.push(value as u8 | 128);
        value >>= 7;
    }
    bytes.push(value as u8);
}

/// `None` means this candidate cannot fit the configured raw payload budget or u32 indices.
pub(crate) fn encode(
    candidate: &QuantizedOccupancy,
    own_octant: bool,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>, FormatError> {
    encode_impl(candidate, own_octant, max_bytes, false)
}

pub(crate) fn encode_compact(
    candidate: &QuantizedOccupancy,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>, FormatError> {
    encode_impl(candidate, false, max_bytes, true)
}

fn encode_impl(
    candidate: &QuantizedOccupancy,
    own_octant: bool,
    max_bytes: u64,
    compact: bool,
) -> Result<Option<Vec<u8>>, FormatError> {
    let index = candidate.index();
    if index.row_count() == 0 || u32::try_from(index.row_count()).is_err() {
        return Ok(None);
    }
    let levels = index.topology().levels();
    let mut length = 1_usize
        .checked_add(levels.len() * 4)
        .ok_or(FormatError::BlockSizeOverflow)?;
    for (depth, level) in levels.iter().enumerate() {
        if u32::try_from(level.len()).is_err() {
            return Ok(None);
        }
        length = length
            .checked_add(
                level
                    .len()
                    .checked_mul(if compact {
                        usize::from(depth + 1 < levels.len())
                    } else {
                        NODE_BYTES
                    })
                    .ok_or(FormatError::BlockSizeOverflow)?,
            )
            .ok_or(FormatError::BlockSizeOverflow)?;
        if level
            .iter()
            .any(|n| u32::try_from(n.first_child()).is_err())
        {
            return Ok(None);
        }
    }
    for leaf in 0..index.leaf_count() {
        let rows = index.rows_for_leaf(leaf).expect("occupied leaf");
        length = length
            .checked_add(var_len(rows.len() as u32))
            .ok_or(FormatError::BlockSizeOverflow)?;
        let mut previous = 0;
        if morton() {
            continue;
        }
        for &row in rows {
            length = length
                .checked_add(var_len(row as u32 - previous))
                .ok_or(FormatError::BlockSizeOverflow)?;
            previous = row as u32;
        }
    }
    length = length
        .checked_add(candidate.residual_bytes().len())
        .ok_or(FormatError::BlockSizeOverflow)?;
    if length as u64 > max_bytes {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| FormatError::AllocationFailed(length))?;
    bytes.push(
        candidate.residual_bits()
            | if own_octant { OWN_OCTANT } else { 0 }
            | if morton() { MORTON_FLAG } else { 0 },
    );
    for level in levels {
        bytes.extend_from_slice(&(level.len() as u32).to_le_bytes());
    }
    let mut offsets = Vec::with_capacity(levels.len());
    for (depth, level) in levels.iter().enumerate() {
        offsets.push(bytes.len());
        if compact && depth + 1 == levels.len() {
            continue;
        }
        for node in level {
            bytes.push(node.child_mask());
            if !compact {
                bytes.push(node.child_count() as u8);
                bytes.extend_from_slice(&(node.first_child() as u32).to_le_bytes());
            }
        }
    }
    if own_octant {
        for depth in 0..levels.len() - 1 {
            for node in &levels[depth] {
                for octant in 0..8 {
                    if let Some(child) = node.child_index(octant) {
                        bytes[offsets[depth + 1] + child * NODE_BYTES + 1] |= octant << 4;
                    }
                }
            }
        }
    }
    let experiment_index_start = bytes.len();
    for leaf in 0..index.leaf_count() {
        let rows = index.rows_for_leaf(leaf).expect("occupied leaf");
        write_var(rows.len() as u32, &mut bytes);
        let mut previous = 0;
        if morton() {
            assert!(
                rows.windows(2).all(|w| w[1] == w[0] + 1),
                "OQ09_MORTON needs contiguous rows per leaf"
            );
            continue;
        }
        for &row in rows {
            write_var(row as u32 - previous, &mut bytes);
            previous = row as u32;
        }
    }
    let experiment_index_end = bytes.len();
    bytes.extend_from_slice(candidate.residual_bytes());
    debug_assert_eq!(bytes.len(), length);
    if std::env::var_os("OQ09_STATS").is_some() {
        experiment_oq09(
            candidate,
            &bytes,
            experiment_index_start,
            experiment_index_end,
        );
    }
    Ok(Some(bytes))
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl Cursor<'_> {
    fn byte(&mut self) -> Result<u8, FormatError> {
        let value = *self.bytes.get(self.offset).ok_or_else(malformed)?;
        self.offset += 1;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32, FormatError> {
        let end = self.offset.checked_add(4).ok_or_else(malformed)?;
        let value = self.bytes.get(self.offset..end).ok_or_else(malformed)?;
        self.offset = end;
        Ok(u32::from_le_bytes(value.try_into().expect("four bytes")))
    }
    fn var(&mut self) -> Result<u32, FormatError> {
        let mut value = 0;
        for i in 0..5 {
            let byte = self.byte()?;
            if (i == 4 && byte > 15) || (i > 0 && byte == 0) {
                return Err(malformed());
            }
            value |= u32::from(byte & 127) << (i * 7);
            if byte & 128 == 0 {
                return Ok(value);
            }
        }
        Err(malformed())
    }
}

#[derive(Clone, Copy, Default)]
struct Level {
    start: usize,
    count: usize,
}

fn node(bytes: &[u8], level: Level, index: usize, compact: bool, leaf: bool) -> (u8, u8, usize) {
    if compact {
        let mask = if leaf { 0 } else { bytes[level.start + index] };
        return (mask, mask.count_ones() as u8, 0);
    }
    let p = level.start + index * NODE_BYTES;
    (
        bytes[p],
        bytes[p + 1],
        u32::from_le_bytes(bytes[p + 2..p + 6].try_into().expect("node index")) as usize,
    )
}

pub(crate) fn decode(bytes: &[u8], points: usize) -> Result<Vec<[u32; 3]>, FormatError> {
    decode_impl(bytes, points, false)
}

pub(crate) fn decode_compact(bytes: &[u8], points: usize) -> Result<Vec<[u32; 3]>, FormatError> {
    decode_impl(bytes, points, true)
}

fn decode_impl(bytes: &[u8], points: usize, compact: bool) -> Result<Vec<[u32; 3]>, FormatError> {
    if points == 0 || u32::try_from(points).is_err() {
        return Err(malformed());
    }
    let mut cursor = Cursor { bytes, offset: 0 };
    let config = cursor.byte()?;
    let bits = config & 63;
    let own_octant = config & OWN_OCTANT != 0;
    let morton_order = config & MORTON_FLAG != 0;
    if bits > 32 || (compact && own_octant) {
        return Err(malformed());
    }
    let depth = usize::from(32 - bits);
    let mut levels = [Level::default(); 33];
    for level in &mut levels[..=depth] {
        level.count = cursor.u32()? as usize;
        if level.count == 0 || level.count > points {
            return Err(malformed());
        }
    }
    if levels[0].count != 1 {
        return Err(malformed());
    }
    for (d, level) in levels[..=depth].iter_mut().enumerate() {
        level.start = cursor.offset;
        cursor.offset = cursor
            .offset
            .checked_add(
                level
                    .count
                    .checked_mul(if compact {
                        usize::from(d < depth)
                    } else {
                        NODE_BYTES
                    })
                    .ok_or_else(malformed)?,
            )
            .ok_or_else(malformed)?;
        if cursor.offset > bytes.len() {
            return Err(malformed());
        }
    }
    if node(bytes, levels[0], 0, compact, depth == 0).1 >> 4 != 0 {
        return Err(malformed());
    }
    for d in 0..=depth {
        let mut prefix = 0;
        for i in 0..levels[d].count {
            let (mask, meta, first) = node(bytes, levels[d], i, compact, d == depth);
            let count = usize::from(meta & 15);
            if meta & 128 != 0
                || (!own_octant && meta & 112 != 0)
                || count != mask.count_ones() as usize
            {
                return Err(malformed());
            }
            if d == depth {
                if mask != 0 || first != 0 {
                    return Err(malformed());
                }
            } else {
                if count == 0 || (!compact && first != prefix) {
                    return Err(malformed());
                }
                prefix = prefix.checked_add(count).ok_or_else(malformed)?;
                if prefix > levels[d + 1].count {
                    return Err(malformed());
                }
                if own_octant {
                    let mut child = first;
                    for octant in 0..8 {
                        if mask & (1 << octant) != 0 {
                            if node(bytes, levels[d + 1], child, false, d + 1 == depth).1 >> 4
                                != octant
                            {
                                return Err(malformed());
                            }
                            child += 1;
                        }
                    }
                }
            }
        }
        if d < depth && prefix != levels[d + 1].count {
            return Err(malformed());
        }
    }
    let residual_bits = points
        .checked_mul(3)
        .and_then(|n| n.checked_mul(usize::from(bits)))
        .ok_or_else(malformed)?;
    let residual_length = residual_bits.div_ceil(8);
    let residual_start = bytes
        .len()
        .checked_sub(residual_length)
        .ok_or_else(malformed)?;
    if residual_start < cursor.offset {
        return Err(malformed());
    }
    let residuals = &bytes[residual_start..];
    if residual_bits % 8 != 0
        && residuals
            .last()
            .is_some_and(|v| *v >> (residual_bits % 8) != 0)
    {
        return Err(malformed());
    }
    let rows_bytes = &bytes[cursor.offset..residual_start];
    let min_rows = levels[depth]
        .count
        .checked_add(if morton_order { 0 } else { points })
        .ok_or_else(malformed)?;
    if rows_bytes.len() < min_rows {
        return Err(malformed());
    }
    let mut rows = Cursor {
        bytes: rows_bytes,
        offset: 0,
    };
    let mut coordinates = Vec::new();
    coordinates
        .try_reserve_exact(points)
        .map_err(|_| FormatError::AllocationFailed(points.saturating_mul(12)))?;
    coordinates.resize(points, [0; 3]);
    let mut seen = Vec::new();
    seen.try_reserve_exact(points.div_ceil(8))
        .map_err(|_| FormatError::AllocationFailed(points.div_ceil(8)))?;
    seen.resize(points.div_ceil(8), 0_u8);
    let mut assigned = 0;
    // DFS in increasing octant order visits each depth's path-sorted nodes monotonically.
    // Running child prefixes reconstruct first_child with only O(depth) cursor storage.
    let mut next_child = [0_usize; 33];
    let mut stack = vec![(0_usize, 0_usize, [0_u32; 3])];
    while let Some((d, i, coarse)) = stack.pop() {
        let (mask, _, first) = node(bytes, levels[d], i, compact, d == depth);
        if d == depth {
            let count = rows.var()? as usize;
            if count == 0 || count > points - assigned {
                return Err(malformed());
            }
            let mut row = 0_u32;
            for j in 0..count {
                let row = if morton_order {
                    assigned + j
                } else {
                    let delta = rows.var()?;
                    if j != 0 && delta == 0 {
                        return Err(malformed());
                    }
                    row = row.checked_add(delta).ok_or_else(malformed)?;
                    row as usize
                };
                if row >= points || seen[row / 8] & (1 << (row % 8)) != 0 {
                    return Err(malformed());
                }
                seen[row / 8] |= 1 << (row % 8);
                for axis in 0..3 {
                    let offset = (row * 3 + axis) * usize::from(bits);
                    let shift = offset % 8;
                    let count = (shift + usize::from(bits)).div_ceil(8);
                    let mut value = 0_u64;
                    for k in 0..count {
                        value |= u64::from(residuals[offset / 8 + k]) << (k * 8);
                    }
                    let residual = (value >> shift) & ((1_u64 << bits) - 1);
                    coordinates[row][axis] = ((u64::from(coarse[axis]) << bits) | residual) as u32;
                }
            }
            assigned += count;
        } else {
            let first = if compact {
                let first = next_child[d];
                next_child[d] += mask.count_ones() as usize;
                first
            } else {
                first
            };
            let mut child = first + mask.count_ones() as usize;
            for inferred in (0..8).rev() {
                if mask & (1 << inferred) == 0 {
                    continue;
                }
                child -= 1;
                let octant = if own_octant {
                    node(bytes, levels[d + 1], child, false, d + 1 == depth).1 >> 4
                } else {
                    inferred
                };
                let next = std::array::from_fn(|axis| {
                    coarse[axis] * 2 + u32::from((octant >> (2 - axis)) & 1)
                });
                stack.push((d + 1, child, next));
            }
        }
    }
    if assigned != points || rows.offset != rows.bytes.len() {
        return Err(malformed());
    }
    if compact && (0..depth).any(|d| next_child[d] != levels[d + 1].count) {
        return Err(malformed());
    }
    Ok(coordinates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quantize_local_positions;

    fn source() -> crate::QuantizedCoordinates {
        let positions: Vec<_> = (0..129)
            .map(|i| [f64::from(i % 11), f64::from(i % 7), f64::from(i % 3)])
            .collect();
        quantize_local_positions(&positions, 0.01).unwrap()
    }

    #[test]
    fn physical_stream_roundtrips_every_partition_and_octant_variant() {
        let source = source();
        for bits in 0..=32 {
            let candidate = QuantizedOccupancy::new(&source, bits).unwrap();
            let compact = encode_compact(&candidate, 1 << 24).unwrap().unwrap();
            assert_eq!(
                decode_compact(&compact, source.coordinates().len()).unwrap(),
                source.coordinates()
            );
            assert!(
                encode_compact(&candidate, compact.len() as u64 - 1)
                    .unwrap()
                    .is_none()
            );
            for length in [0, 1, compact.len() / 2, compact.len() - 1] {
                assert!(decode_compact(&compact[..length], source.coordinates().len()).is_err());
            }
            for own_octant in [false, true] {
                let bytes = encode(&candidate, own_octant, 1 << 24).unwrap().unwrap();
                assert_eq!(
                    decode(&bytes, source.coordinates().len()).unwrap(),
                    source.coordinates()
                );
                assert!(
                    encode(&candidate, own_octant, bytes.len() as u64 - 1)
                        .unwrap()
                        .is_none()
                );
                for length in [0, 1, bytes.len() / 2, bytes.len() - 1] {
                    assert!(decode(&bytes[..length], source.coordinates().len()).is_err());
                }
                assert!(decode(&bytes, source.coordinates().len() + 1).is_err());
            }
        }
    }

    #[test]
    fn compact_rejects_bad_masks_counts_rows_flags_and_residual_padding() {
        let source = source();
        let candidate = QuantizedOccupancy::new(&source, 28).unwrap();
        let good = encode_compact(&candidate, 1 << 24).unwrap().unwrap();
        for (offset, value) in [(0, 64), (0, 128), (0, 33), (1, 2), (21, 0)] {
            let mut bad = good.clone();
            bad[offset] = value;
            assert!(decode_compact(&bad, 129).is_err(), "offset={offset}");
        }
        let mut bad = good.clone();
        bad[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_compact(&bad, 129).is_err());
        let unit = QuantizedOccupancy::new(&source, 32).unwrap();
        let good = encode_compact(&unit, 1 << 24).unwrap().unwrap();
        for offset in [5, 8] {
            let mut bad = good.clone();
            bad[offset] = 0;
            assert!(decode_compact(&bad, 129).is_err());
        }
        let mut bad = good;
        bad.push(0);
        assert!(decode_compact(&bad, 129).is_err());
        let source =
            quantize_local_positions(&[[0.; 3], [u32::MAX as f64; 3], [0.; 3]], 2.).unwrap();
        for bits in 0..=32 {
            let candidate = QuantizedOccupancy::new(&source, bits).unwrap();
            let bytes = encode_compact(&candidate, 1 << 24).unwrap().unwrap();
            assert_eq!(decode_compact(&bytes, 3).unwrap(), source.coordinates());
            if (9 * usize::from(bits)) % 8 != 0 {
                let mut bad = bytes;
                *bad.last_mut().unwrap() |= 128;
                assert!(decode_compact(&bad, 3).is_err());
            }
        }
    }

    #[test]
    fn rejects_invalid_counts_indices_masks_octants_rows_and_padding() {
        let source = source();
        let candidate = QuantizedOccupancy::new(&source, 28).unwrap();
        let good = encode(&candidate, false, 1 << 24).unwrap().unwrap();
        let root = 1 + 5 * 4;
        for (offset, value) in [
            (0, 128),
            (0, 33),
            (1, 2),
            (root, 0),
            (root + 1, 9),
            (root + 1, 16),
            (root + 2, 1),
        ] {
            let mut bad = good.clone();
            bad[offset] = value;
            assert!(decode(&bad, 129).is_err(), "offset={offset}");
        }
        let mut bad = good.clone();
        bad[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&bad, 129).is_err());
        let with_octants = encode(&candidate, true, 1 << 24).unwrap().unwrap();
        let mut bad = with_octants.clone();
        bad[root + 7] ^= 16;
        assert!(decode(&bad, 129).is_err());
        // A unit root has one leaf with all rows: config + level count + six-byte root.
        let unit = QuantizedOccupancy::new(&source, 32).unwrap();
        let good = encode(&unit, false, 1 << 24).unwrap().unwrap();
        let mut bad = good.clone();
        bad[11] = 0;
        assert!(decode(&bad, 129).is_err());
        // Count 129 uses two bytes, then first row 0 and first delta 1.
        let mut bad = good.clone();
        bad[14] = 0;
        assert!(decode(&bad, 129).is_err());
        let mut bad = good.clone();
        bad.push(0);
        assert!(decode(&bad, 129).is_err());
        let one = quantize_local_positions(&[[0.0; 3]], 0.0).unwrap();
        let one = QuantizedOccupancy::new(&one, 1).unwrap();
        let mut bad = encode(&one, false, 1 << 24).unwrap().unwrap();
        *bad.last_mut().unwrap() |= 128;
        assert!(decode(&bad, 1).is_err());
    }

    #[test]
    fn row_varints_are_canonical_and_empty_uses_fallback() {
        for bytes in [&[128, 0][..], &[255, 255, 255, 255, 16][..], &[128][..]] {
            assert!(Cursor { bytes, offset: 0 }.var().is_err());
        }
        let source = quantize_local_positions(&[], 0.0).unwrap();
        let empty = QuantizedOccupancy::new(&source, 28).unwrap();
        assert!(encode(&empty, false, 1024).unwrap().is_none());
        assert!(decode(&[], 0).is_err());
    }
}

/// OQ-09 experiment: Morton-order variant (no per-point row deltas, residuals in leaf order).
fn experiment_oq09(c: &QuantizedOccupancy, bytes: &[u8], start: usize, end: usize) {
    let index = c.index();
    let mut variant = bytes[..start].to_vec();
    let mut perm = Vec::with_capacity(index.row_count());
    let mut counts = 0;
    for leaf in 0..index.leaf_count() {
        let rows = index.rows_for_leaf(leaf).expect("leaf");
        let before = variant.len();
        write_var(rows.len() as u32, &mut variant);
        counts += variant.len() - before;
        let bits = usize::from(c.residual_bits());
        let mut keyed: Vec<(u128, u32)> = rows
            .iter()
            .map(|&r| {
                let mut key = 0_u128;
                for bit in (0..bits).rev() {
                    for axis in 0..3 {
                        key = (key << 1) | u128::from((c.residual(r, axis) >> bit) & 1);
                    }
                }
                (key, r as u32)
            })
            .collect();
        keyed.sort_unstable();
        perm.extend(keyed.iter().map(|&(_, r)| r));
    }
    let bits = usize::from(c.residual_bits());
    let (mut pending, mut buffer) = (0_usize, 0_u64);
    for &row in &perm {
        for axis in 0..3 {
            buffer |= c.residual(row as usize, axis) << pending;
            pending += bits;
            while pending >= 8 {
                variant.push(buffer as u8);
                buffer >>= 8;
                pending -= 8;
            }
        }
    }
    if pending != 0 {
        variant.push(buffer as u8);
    }
    let variant_comp = crate::compress_block(&variant).bytes().len();
    let nodes = start - 1 - index.topology().levels().len() * 4;
    let _ = nodes;
    EXPERIMENT.with(|e| {
        e.borrow_mut().push(Oq09Entry {
            raw_len: bytes.len(),
            index_bytes: end - start,
            counts_bytes: counts,
            residual_bytes: c.residual_bytes().len(),
            variant_raw: variant.len(),
            variant_comp,
            perm,
        })
    });
}

pub(crate) struct Oq09Entry {
    pub raw_len: usize,
    pub index_bytes: usize,
    pub counts_bytes: usize,
    pub residual_bytes: usize,
    pub variant_raw: usize,
    pub variant_comp: usize,
    pub perm: Vec<u32>,
}

thread_local! {
    pub(crate) static EXPERIMENT: std::cell::RefCell<Vec<Oq09Entry>> = const { std::cell::RefCell::new(Vec::new()) };
}
