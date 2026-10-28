const CRC32_TABLE: [u32; 256] = make_table();
const CRC32_SLICES: [[u32; 256]; 8] = make_slices();

const fn make_slices() -> [[u32; 256]; 8] {
    let mut slices = [[0; 256]; 8];
    slices[0] = CRC32_TABLE;
    let mut slice = 1;
    while slice < 8 {
        let mut index = 0;
        while index < 256 {
            let previous = slices[slice - 1][index];
            slices[slice][index] = CRC32_TABLE[(previous & 255) as usize] ^ (previous >> 8);
            index += 1;
        }
        slice += 1;
    }
    slices
}

const fn make_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < table.len() {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                (value >> 1) ^ 0xEDB8_8320
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

/// Incremental IEEE CRC-32 checksum for a stream of bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crc32(u32);

impl Crc32 {
    pub const fn new() -> Self {
        Self(u32::MAX)
    }

    pub fn update(&mut self, bytes: &[u8]) {
        // Eight independent lookups replace the byte-to-byte dependency chain.
        // Explicit LE loads and slice access work on unaligned buffers and WASM.
        let (chunks, remainder) = bytes.as_chunks::<8>();
        for chunk in chunks {
            let first = u32::from_le_bytes(chunk[..4].try_into().unwrap()) ^ self.0;
            self.0 = CRC32_SLICES[7][(first & 255) as usize]
                ^ CRC32_SLICES[6][((first >> 8) & 255) as usize]
                ^ CRC32_SLICES[5][((first >> 16) & 255) as usize]
                ^ CRC32_SLICES[4][(first >> 24) as usize]
                ^ CRC32_SLICES[3][chunk[4] as usize]
                ^ CRC32_SLICES[2][chunk[5] as usize]
                ^ CRC32_SLICES[1][chunk[6] as usize]
                ^ CRC32_SLICES[0][chunk[7] as usize];
        }
        for byte in remainder {
            let index = (self.0 as u8 ^ byte) as usize;
            self.0 = CRC32_SLICES[0][index] ^ (self.0 >> 8);
        }
    }

    pub const fn finish(self) -> u32 {
        self.0 ^ u32::MAX
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// Computes the IEEE CRC-32 checksum of one byte slice.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut checksum = Crc32::new();
    checksum.update(bytes);
    checksum.finish()
}

#[cfg(test)]
mod tests {
    use super::{Crc32, crc32};

    // Independent polynomial oracle, without any lookup table.
    fn bitwise_crc(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xEDB8_8320 & 0_u32.wrapping_sub(crc & 1));
            }
        }
        crc ^ u32::MAX
    }

    #[test]
    fn sliced_crc_matches_bitwise_oracle_for_alignment_tails_and_splits() {
        let data: Vec<_> = (0..272_u32)
            .map(|i| i.wrapping_mul(37).wrapping_add(i / 7) as u8)
            .collect();
        for offset in 0..8 {
            for length in 0..=257 {
                let bytes = &data[offset..offset + length];
                let expected = bitwise_crc(bytes);
                assert_eq!(crc32(bytes), expected, "offset={offset} length={length}");
                for split in 0..=length {
                    let mut crc = Crc32::new();
                    crc.update(&bytes[..split]);
                    crc.update(&[]);
                    crc.update(&bytes[split..]);
                    assert_eq!(crc.finish(), expected);
                }
            }
        }
    }

    #[test]
    fn sliced_crc_matches_large_payload_and_irregular_incremental_chunks() {
        let data: Vec<_> = (0..65_537_u32).map(|i| (i ^ (i >> 8)) as u8).collect();
        let expected = bitwise_crc(&data);
        for width in [1, 7, 8, 9, 63, 1024, 4093] {
            let mut crc = Crc32::new();
            for chunk in data.chunks(width) {
                crc.update(chunk);
            }
            assert_eq!(crc.finish(), expected);
        }
    }

    #[test]
    fn matches_the_ieee_reference_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn incremental_updates_match_a_single_pass() {
        let mut checksum = Crc32::new();
        checksum.update(b"1234");
        checksum.update(b"56789");
        assert_eq!(checksum.finish(), crc32(b"123456789"));
    }
}
