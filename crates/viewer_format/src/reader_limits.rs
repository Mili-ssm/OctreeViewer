use crate::FormatError;

const MAX_ATTRIBUTES_DEFAULT: u32 = 256;
const MAX_COMPONENTS_DEFAULT: u32 = 65_535;
const MAX_STRING_BYTES_DEFAULT: u32 = 1_048_576;
const MAX_INDEX_BYTES_DEFAULT: u64 = 256 * 1024 * 1024;
const MAX_BLOCK_BYTES_DEFAULT: u64 = 256 * 1024 * 1024;
const MAX_NODES_DEFAULT: u64 = 2_000_000;
const MAX_BLOCKS_DEFAULT: u64 = 2_000_000;
// Measured worst case (`tests/index_amplification.rs`, `fast` profile): one `u8` range takes
// 6 B on disk but about 76 B of heap (two `Vec`s plus allocator overhead). 2^23 ranges cost
// about 640 MB, which with the index buffer (256 MiB) and node records keeps the worst case
// near 4x `MAX_INDEX_BYTES_DEFAULT` (1 GiB) instead of the unbounded ~13x.
const MAX_ATTRIBUTE_RANGES_DEFAULT: u64 = 1 << 23;

/// Resource limits checked before reader allocations.
///
/// The default accepts at most 256 attributes. The format and writer allow wider schemas;
/// callers opening those files must pass suitable limits to `FormatReader::with_limits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatReaderLimits {
    pub(crate) max_index_bytes: u64,
    pub(crate) max_block_bytes: u64,
    pub(crate) max_nodes: u64,
    pub(crate) max_blocks: u64,
    pub(crate) max_attributes: u32,
    pub(crate) max_attribute_components: u32,
    pub(crate) max_string_bytes: u32,
    pub(crate) max_attribute_ranges: u64,
}

impl FormatReaderLimits {
    pub fn new(
        max_index_bytes: u64,
        max_block_bytes: u64,
        max_nodes: u64,
        max_blocks: u64,
        max_attributes: u32,
        max_attribute_components: u32,
        max_string_bytes: u32,
    ) -> Result<Self, FormatError> {
        if [
            max_index_bytes,
            max_block_bytes,
            max_nodes,
            max_blocks,
            u64::from(max_attributes),
            u64::from(max_attribute_components),
            u64::from(max_string_bytes),
        ]
        .contains(&0)
        {
            return Err(FormatError::InvalidReaderLimits);
        }
        Ok(Self {
            max_index_bytes,
            max_block_bytes,
            max_nodes,
            max_blocks,
            max_attributes,
            max_attribute_components,
            max_string_bytes,
            max_attribute_ranges: MAX_ATTRIBUTE_RANGES_DEFAULT,
        })
    }

    /// Caps the total number of per-node attribute ranges in the index, checked before each
    /// node's ranges are allocated. Each range costs far more heap than its on-disk bytes.
    pub fn with_max_attribute_ranges(mut self, max: u64) -> Result<Self, FormatError> {
        if max == 0 {
            return Err(FormatError::InvalidReaderLimits);
        }
        self.max_attribute_ranges = max;
        Ok(self)
    }
}

impl Default for FormatReaderLimits {
    fn default() -> Self {
        Self {
            max_index_bytes: MAX_INDEX_BYTES_DEFAULT,
            max_block_bytes: MAX_BLOCK_BYTES_DEFAULT,
            max_nodes: MAX_NODES_DEFAULT,
            max_blocks: MAX_BLOCKS_DEFAULT,
            max_attributes: MAX_ATTRIBUTES_DEFAULT,
            max_attribute_components: MAX_COMPONENTS_DEFAULT,
            max_string_bytes: MAX_STRING_BYTES_DEFAULT,
            max_attribute_ranges: MAX_ATTRIBUTE_RANGES_DEFAULT,
        }
    }
}
