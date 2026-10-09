const CRC32_TABLE: [u32; 256] = make_table();

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
        for byte in bytes {
            let index = (self.0 as u8 ^ byte) as usize;
            self.0 = CRC32_TABLE[index] ^ (self.0 >> 8);
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
