use thiserror::Error;

/// Independently decodable payload encoding stored in a block directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BlockCodec {
    None = 0,
    Lz4 = 1,
    /// One complete attribute tuple repeated for every row (pre3 only).
    Constant = 2,
    /// Local attribute palette and bitpacked row IDs (pre3 only).
    Dictionary = 3,
    /// LZ4 over independent byte planes of one attribute tuple (pre4+).
    BytePlanes = 4,
}

impl TryFrom<u8> for BlockCodec {
    type Error = CodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Lz4),
            2 => Ok(Self::Constant),
            3 => Ok(Self::Dictionary),
            4 => Ok(Self::BytePlanes),
            _ => Err(CodecError::UnsupportedCodec(value)),
        }
    }
}

/// One block after selecting the smaller of its raw and LZ4 representations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedBlock {
    pub(crate) codec: BlockCodec,
    pub(crate) bytes: Vec<u8>,
}

impl EncodedBlock {
    pub const fn codec(&self) -> BlockCodec {
        self.codec
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Encodes a single block and stores it raw when LZ4 would not save bytes.
pub fn compress_block(uncompressed: &[u8]) -> EncodedBlock {
    let compressed = lz4_flex::block::compress(uncompressed);
    if compressed.len() < uncompressed.len() {
        EncodedBlock {
            codec: BlockCodec::Lz4,
            bytes: compressed,
        }
    } else {
        EncodedBlock {
            codec: BlockCodec::None,
            bytes: uncompressed.to_vec(),
        }
    }
}

/// Decodes one independent block after enforcing the caller's allocation limit.
pub fn decompress_block(
    codec: BlockCodec,
    stored: &[u8],
    uncompressed_size: usize,
    max_uncompressed_size: usize,
) -> Result<Vec<u8>, CodecError> {
    if uncompressed_size > max_uncompressed_size {
        return Err(CodecError::BlockExceedsLimit {
            requested: uncompressed_size,
            limit: max_uncompressed_size,
        });
    }
    if codec == BlockCodec::None {
        if stored.len() != uncompressed_size {
            return Err(CodecError::UnexpectedRawBlockSize {
                expected: uncompressed_size,
                actual: stored.len(),
            });
        }
        let mut output = Vec::new();
        output
            .try_reserve_exact(uncompressed_size)
            .map_err(|_| CodecError::AllocationFailed(uncompressed_size))?;
        output.extend_from_slice(stored);
        return Ok(output);
    }

    if codec != BlockCodec::Lz4 {
        return Err(CodecError::UnsupportedCodec(codec as u8));
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(uncompressed_size)
        .map_err(|_| CodecError::AllocationFailed(uncompressed_size))?;
    output.resize(uncompressed_size, 0);
    let written = lz4_flex::block::decompress_into(stored, &mut output)
        .map_err(|error| CodecError::InvalidCompressedBlock(error.to_string()))?;
    if written != uncompressed_size {
        return Err(CodecError::UnexpectedDecodedSize {
            expected: uncompressed_size,
            actual: written,
        });
    }
    Ok(output)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CodecError {
    #[error("unsupported block codec {0}")]
    UnsupportedCodec(u8),
    #[error("block needs {requested} decoded bytes, exceeding limit {limit}")]
    BlockExceedsLimit { requested: usize, limit: usize },
    #[error("raw block has {actual} bytes, expected {expected}")]
    UnexpectedRawBlockSize { expected: usize, actual: usize },
    #[error("could not reserve {0} bytes for the decoded block")]
    AllocationFailed(usize),
    #[error("invalid LZ4 block: {0}")]
    InvalidCompressedBlock(String),
    #[error("decoded block has {actual} bytes, expected {expected}")]
    UnexpectedDecodedSize { expected: usize, actual: usize },
}

#[cfg(test)]
mod tests {
    use super::{BlockCodec, CodecError, compress_block, decompress_block};

    #[test]
    fn compresses_repetitive_blocks_and_round_trips_independently() {
        let source = vec![42; 32_768];
        let encoded = compress_block(&source);

        assert_eq!(encoded.codec(), BlockCodec::Lz4);
        assert!(encoded.bytes().len() < source.len());
        assert_eq!(
            decompress_block(encoded.codec(), encoded.bytes(), source.len(), source.len()).unwrap(),
            source
        );
    }

    #[test]
    fn retains_raw_bytes_when_compression_does_not_reduce_size() {
        let mut source = Vec::with_capacity(4096);
        let mut state = 0xA341_316C_u32;
        for _ in 0..4096 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            source.push(state as u8);
        }
        let encoded = compress_block(&source);

        assert_eq!(encoded.codec(), BlockCodec::None);
        assert_eq!(encoded.bytes(), source);
        assert_eq!(
            decompress_block(encoded.codec(), encoded.bytes(), source.len(), source.len()).unwrap(),
            source
        );
    }

    #[test]
    fn enforces_decoded_size_limits_before_allocation() {
        assert_eq!(
            decompress_block(BlockCodec::Lz4, &[], 1024, 512).unwrap_err(),
            CodecError::BlockExceedsLimit {
                requested: 1024,
                limit: 512,
            }
        );
    }
}
