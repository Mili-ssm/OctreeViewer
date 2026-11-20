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
    /// Magicless zstd frame (v8.2-pre only; never geometry).
    Zstd = 5,
    /// Zstd over independent byte planes of one attribute tuple (v8.2-pre only).
    BytePlanesZstd = 6,
}

/// Entropy stage of an attribute payload: every candidate swaps LZ4 for zstd as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntropyCoder {
    Lz4,
    /// Valid only when the layout supports zstd (v8.2-pre).
    Zstd,
}

impl EntropyCoder {
    pub(crate) const fn codec(self) -> BlockCodec {
        match self {
            Self::Lz4 => BlockCodec::Lz4,
            Self::Zstd => BlockCodec::Zstd,
        }
    }

    pub(crate) const fn byte_planes_codec(self) -> BlockCodec {
        match self {
            Self::Lz4 => BlockCodec::BytePlanes,
            Self::Zstd => BlockCodec::BytePlanesZstd,
        }
    }
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
            5 => Ok(Self::Zstd),
            6 => Ok(Self::BytePlanesZstd),
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

/// Like [`compress_block`] with the chosen entropy stage; zstd uses level 3 and no frame magic,
/// checksum or content size (the directory carries the raw length and the payload a CRC).
pub fn compress_block_with(
    uncompressed: &[u8],
    coder: EntropyCoder,
) -> Result<EncodedBlock, CodecError> {
    if coder == EntropyCoder::Lz4 {
        return Ok(compress_block(uncompressed));
    }
    let compressed = zstd_compress(uncompressed)?;
    Ok(if compressed.len() < uncompressed.len() {
        EncodedBlock {
            codec: BlockCodec::Zstd,
            bytes: compressed,
        }
    } else {
        EncodedBlock {
            codec: BlockCodec::None,
            bytes: uncompressed.to_vec(),
        }
    })
}

fn zstd_compress(raw: &[u8]) -> Result<Vec<u8>, CodecError> {
    use zstd::zstd_safe::{CParameter, FrameFormat};
    let fail = |error: std::io::Error| CodecError::ZstdFailure(error.to_string());
    let mut compressor = zstd::bulk::Compressor::new(3).map_err(fail)?;
    compressor
        .set_parameter(CParameter::Format(FrameFormat::Magicless))
        .map_err(fail)?;
    compressor.include_checksum(false).map_err(fail)?;
    compressor.include_contentsize(false).map_err(fail)?;
    compressor.compress(raw).map_err(fail)
}

/// Decodes into a buffer of exactly `size` bytes: a longer frame does not fit, a shorter one fails.
fn zstd_decompress(stored: &[u8], size: usize) -> Result<Vec<u8>, CodecError> {
    use zstd::zstd_safe::{DParameter, FrameFormat};
    let fail = |error: std::io::Error| CodecError::ZstdFailure(error.to_string());
    let mut output = Vec::new();
    output
        .try_reserve_exact(size)
        .map_err(|_| CodecError::AllocationFailed(size))?;
    output.resize(size, 0);
    let mut decompressor = zstd::bulk::Decompressor::new().map_err(fail)?;
    decompressor
        .set_parameter(DParameter::Format(FrameFormat::Magicless))
        .map_err(fail)?;
    let written = decompressor
        .decompress_to_buffer(stored, output.as_mut_slice())
        .map_err(fail)?;
    if written != size {
        return Err(CodecError::UnexpectedDecodedSize {
            expected: size,
            actual: written,
        });
    }
    Ok(output)
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

    if codec == BlockCodec::Zstd {
        return zstd_decompress(stored, uncompressed_size);
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
    #[error("zstd failure: {0}")]
    ZstdFailure(String),
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
