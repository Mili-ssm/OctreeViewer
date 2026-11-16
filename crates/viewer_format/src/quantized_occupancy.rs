//! Exact block-local integer occupancy candidate. Not a serialized storage revision.
use thiserror::Error;
use viewer_core::{OccupiedCellIndex, OccupiedOctreeError, OccupiedOctreeRoot, VoxelCoord};

use crate::{QuantizationTransform, QuantizedCoordinates};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QuantizedOccupancyError {
    #[error("residual bit count must be between 0 and 32, got {0}")]
    InvalidResidualBits(u8),
    #[error("packed residual size overflows the platform length")]
    ResidualSizeOverflow,
    #[error(transparent)]
    Topology(#[from] OccupiedOctreeError),
}

/// Splits the original quantized u32 values into occupied cells and packed low bits.
///
/// Cells use the biased code space `i32::MIN + (q >> residual_bits)`, rooted at `i32::MIN`
/// with depth `32 - residual_bits`. This is not a physical cube, CRS or common dataset grid:
/// the original per-axis affine scales may differ or be zero. Input row identity is retained.
/// The original coordinate vector and attributes are not retained. At 32 residual bits,
/// the residual byte stream contains all XYZ information.
#[derive(Debug, Clone, PartialEq)]
pub struct QuantizedOccupancy {
    transform: QuantizationTransform,
    max_error: f64,
    residual_bits: u8,
    index: OccupiedCellIndex,
    residual_bytes: Vec<u8>,
}

impl QuantizedOccupancy {
    /// Builds an experimental candidate before any f32 projection or row reordering.
    /// Allocation failures follow the normal Rust policy; this is not a bounded disk parser.
    pub fn new(
        source: &QuantizedCoordinates,
        residual_bits: u8,
    ) -> Result<Self, QuantizedOccupancyError> {
        if residual_bits > 32 {
            return Err(QuantizedOccupancyError::InvalidResidualBits(residual_bits));
        }
        let bit_count = source
            .coordinates()
            .len()
            .checked_mul(3)
            .and_then(|n| n.checked_mul(usize::from(residual_bits)))
            .ok_or(QuantizedOccupancyError::ResidualSizeOverflow)?;
        let mut residual_bytes = Vec::with_capacity(bit_count.div_ceil(8));
        let mask = (1_u64 << residual_bits) - 1;
        let mut pending = 0_u8;
        let mut buffer = 0_u64;
        for coordinate in source.coordinates() {
            for &q in coordinate {
                buffer |= (u64::from(q) & mask) << pending;
                pending += residual_bits;
                while pending >= 8 {
                    residual_bytes.push(buffer as u8);
                    buffer >>= 8;
                    pending -= 8;
                }
            }
        }
        if pending != 0 {
            residual_bytes.push(buffer as u8);
        }
        let root = OccupiedOctreeRoot::new([i32::MIN; 3], 32 - residual_bits)?;
        let cells = source.coordinates().iter().map(|coordinate| {
            let cell = coordinate
                .map(|q| (i64::from(i32::MIN) + (u64::from(q) >> residual_bits) as i64) as i32);
            VoxelCoord::new(cell[0], cell[1], cell[2])
        });
        let index = OccupiedCellIndex::new(root, cells)?;
        Ok(Self {
            transform: source.transform(),
            max_error: source.max_error(),
            residual_bits,
            index,
            residual_bytes,
        })
    }

    pub fn transform(&self) -> QuantizationTransform {
        self.transform
    }

    pub fn max_error(&self) -> f64 {
        self.max_error
    }

    pub fn residual_bits(&self) -> u8 {
        self.residual_bits
    }

    pub fn index(&self) -> &OccupiedCellIndex {
        &self.index
    }

    /// Low bits in the input row/XYZ order, least significant bit first; final padding is zero.
    /// Only residuals: this byte slice cannot reconstruct a block without its index/metadata.
    pub fn residual_bytes(&self) -> &[u8] {
        &self.residual_bytes
    }

    /// Reconstructs all u32 coordinates in the input row order, with unchanged transform/error.
    /// The writer may have reordered rows before this (ADR 0041); the codec keeps whatever order it gets.
    /// Allocates the output; source and candidate need not remain alive together.
    pub fn reconstruct(&self) -> QuantizedCoordinates {
        let mut coordinates = vec![[0; 3]; self.index.row_count()];
        for (leaf, coord) in self.index.topology().cells().enumerate() {
            let coarse =
                [coord.x, coord.y, coord.z].map(|v| (i64::from(v) - i64::from(i32::MIN)) as u64);
            for &row in self.index.rows_for_leaf(leaf).expect("valid occupied leaf") {
                coordinates[row] = std::array::from_fn(|axis| {
                    ((coarse[axis] << self.residual_bits) | self.residual(row, axis)) as u32
                });
            }
        }
        QuantizedCoordinates::from_parts(self.transform, coordinates, self.max_error)
    }

    /// Inline data and retained vector capacities; excludes allocator overhead, source and peaks.
    pub fn estimated_memory_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(
                self.index.estimated_memory_bytes() - std::mem::size_of::<OccupiedCellIndex>(),
            )
            .saturating_add(self.residual_bytes.capacity())
    }

    fn residual(&self, row: usize, axis: usize) -> u64 {
        if self.residual_bits == 0 {
            return 0;
        }
        let bit_offset = (row * 3 + axis) * usize::from(self.residual_bits);
        let byte_offset = bit_offset / 8;
        let shift = bit_offset % 8;
        let length = (shift + usize::from(self.residual_bits)).div_ceil(8);
        let mut value = 0_u64;
        for (i, &byte) in self.residual_bytes[byte_offset..byte_offset + length]
            .iter()
            .enumerate()
        {
            value |= u64::from(byte) << (i * 8);
        }
        (value >> shift) & ((1_u64 << self.residual_bits) - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quantize_local_positions;

    fn fixture(coordinates: Vec<[u32; 3]>) -> QuantizedCoordinates {
        let affine =
            quantize_local_positions(&[[-13.0, 42.0, 7.0], [11.0, 42.0, 30.0]], 0.01).unwrap();
        QuantizedCoordinates::from_parts(affine.transform(), coordinates, affine.max_error())
    }

    #[test]
    fn every_bit_partition_reconstructs_exact_integers_and_original_projection() {
        let mut coordinates = vec![
            [0; 3],
            [u32::MAX; 3],
            [1, u32::MAX - 1, 1 << 31],
            [1, 2, 3],
            [1, 2, 3],
        ];
        let mut state = 0x1234_5678_u32;
        for _ in 0..128 {
            coordinates.push(std::array::from_fn(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                state
            }));
        }
        let source = fixture(coordinates);
        let expected_projection = source
            .decode_local()
            .unwrap()
            .into_iter()
            .map(|p| p.map(f32::to_bits))
            .collect::<Vec<_>>();
        for bits in 0..=32 {
            let candidate = QuantizedOccupancy::new(&source, bits).unwrap();
            let reconstructed = candidate.reconstruct();
            assert_eq!(reconstructed.coordinates(), source.coordinates());
            assert_eq!(candidate.transform(), source.transform());
            assert_eq!(
                candidate.max_error().to_bits(),
                source.max_error().to_bits()
            );
            assert_eq!(reconstructed.transform(), source.transform());
            assert_eq!(
                reconstructed.max_error().to_bits(),
                source.max_error().to_bits()
            );
            assert_eq!(
                reconstructed
                    .decode_local()
                    .unwrap()
                    .into_iter()
                    .map(|p| p.map(f32::to_bits))
                    .collect::<Vec<_>>(),
                expected_projection
            );
            assert_eq!(candidate.index().row_count(), source.coordinates().len());
            assert_eq!(candidate.index().topology().root().depth(), 32 - bits);
            let mut rows = Vec::new();
            for leaf in 0..candidate.index().leaf_count() {
                rows.extend_from_slice(candidate.index().rows_for_leaf(leaf).unwrap());
            }
            rows.sort_unstable();
            assert_eq!(rows, (0..source.coordinates().len()).collect::<Vec<_>>());
        }
    }

    #[test]
    fn residual_packing_matches_independent_bit_oracle_and_zero_padding() {
        let source = fixture(vec![[3, 6, 7], [4, 5, 6], [u32::MAX, 0x1234_5678, 1]]);
        for bits in 0..=32 {
            let candidate = QuantizedOccupancy::new(&source, bits).unwrap();
            let mut expected =
                vec![0_u8; (source.coordinates().len() * 3 * usize::from(bits)).div_ceil(8)];
            let mut offset = 0;
            for q in source.coordinates().iter().flatten() {
                for bit in 0..bits {
                    expected[offset / 8] |= (((q >> bit) & 1) as u8) << (offset % 8);
                    offset += 1;
                }
            }
            assert_eq!(candidate.residual_bytes(), expected);
        }
        let source = fixture(vec![[0, 7, 3]]);
        assert_eq!(
            QuantizedOccupancy::new(&source, 3)
                .unwrap()
                .residual_bytes(),
            &[248, 0]
        );
    }

    #[test]
    fn duplicates_empty_and_invalid_bit_counts() {
        for bits in [0, 1, 16, 31, 32] {
            let empty = quantize_local_positions(&[], 0.0).unwrap();
            let candidate = QuantizedOccupancy::new(&empty, bits).unwrap();
            assert_eq!(candidate.index().leaf_count(), 0);
            assert!(
                candidate.residual_bytes().is_empty(),
                "{:?}",
                candidate.residual_bytes()
            );
            assert_eq!(candidate.reconstruct(), empty);
            let identical = fixture(vec![[u32::MAX; 3]; 5]);
            let candidate = QuantizedOccupancy::new(&identical, bits).unwrap();
            assert_eq!(candidate.index().leaf_count(), 1);
            assert_eq!(
                candidate.index().rows_for_leaf(0),
                Some([0, 1, 2, 3, 4].as_slice())
            );
            assert_eq!(
                candidate.reconstruct().coordinates(),
                identical.coordinates()
            );
        }
        for bits in [33, 255] {
            assert_eq!(
                QuantizedOccupancy::new(&fixture(vec![]), bits).unwrap_err(),
                QuantizedOccupancyError::InvalidResidualBits(bits)
            );
        }
    }

    #[test]
    fn points_in_one_cell_keep_distinct_residuals_and_input_order() {
        let expected = vec![[7, 1, 2], [3, 6, 5], [7, 1, 2]];
        let candidate = QuantizedOccupancy::new(&fixture(expected.clone()), 3).unwrap();
        assert_eq!(candidate.index().leaf_count(), 1);
        assert_eq!(
            candidate.index().rows_for_leaf(0),
            Some([0, 1, 2].as_slice())
        );
        assert_eq!(candidate.reconstruct().coordinates(), expected);
    }
}
