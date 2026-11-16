use thiserror::Error;

use crate::global_frame::{
    CellBox, UNITS_PER_QUANTUM, block_cell_origin, local_position_units, quantize_coordinate,
};

const MAX_QUANTIZED_COORDINATE: u32 = u32::MAX;

/// Per-axis affine transform from local `f64` coordinates to `u32` values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuantizationTransform {
    origin: [f64; 3],
    scale: [f64; 3],
}

impl QuantizationTransform {
    pub const fn origin(self) -> [f64; 3] {
        self.origin
    }

    pub const fn scale(self) -> [f64; 3] {
        self.scale
    }
}

/// Quantized local coordinates and an error bound for the reader's final `f32` values.
#[derive(Debug, Clone, PartialEq)]
pub struct QuantizedCoordinates {
    transform: QuantizationTransform,
    coordinates: Vec<[u32; 3]>,
    max_error: f64,
}

impl QuantizedCoordinates {
    /// Internal reconstruction preserves validated transform/error metadata unchanged.
    pub(crate) fn from_parts(
        transform: QuantizationTransform,
        coordinates: Vec<[u32; 3]>,
        max_error: f64,
    ) -> Self {
        Self {
            transform,
            coordinates,
            max_error,
        }
    }

    pub const fn transform(&self) -> QuantizationTransform {
        self.transform
    }

    pub fn coordinates(&self) -> &[[u32; 3]] {
        &self.coordinates
    }

    pub const fn max_error(&self) -> f64 {
        self.max_error
    }

    /// Retained inline data and coordinate capacity; excludes allocator overhead.
    pub fn estimated_memory_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(
            self.coordinates
                .capacity()
                .saturating_mul(std::mem::size_of::<[u32; 3]>()),
        )
    }

    /// Reconstructs only this block into the local `f32` render coordinate frame.
    pub fn decode_local(&self) -> Result<Vec<[f32; 3]>, QuantizationError> {
        self.decoded_local_positions().collect()
    }

    /// Visits reconstructed positions without allocating another point-sized buffer.
    pub(crate) fn decoded_local_positions(
        &self,
    ) -> impl Iterator<Item = Result<[f32; 3], QuantizationError>> + '_ {
        self.coordinates.iter().map(|coordinate| {
            let decoded = std::array::from_fn(|axis| {
                self.transform.origin[axis]
                    + f64::from(coordinate[axis]) * self.transform.scale[axis]
            });
            let local = decoded.map(|value| value as f32);
            if decoded.iter().any(|value| !value.is_finite())
                || local.iter().any(|value| !value.is_finite())
            {
                return Err(QuantizationError::DecodedPositionOutOfRange(decoded));
            }
            Ok(local)
        })
    }
}

/// Quantizes one block of dataset-local `f64` positions and enforces a maximum error.
///
/// The reported bound is measured against the exact `f32` positions returned by
/// [`QuantizedCoordinates::decode_local`], with a floating-point arithmetic margin.
/// The input positions must already be relative to the dataset origin; subtracting a
/// large world origin belongs before this function.
pub fn quantize_local_positions(
    positions: &[[f64; 3]],
    max_allowed_error: f64,
) -> Result<QuantizedCoordinates, QuantizationError> {
    if !max_allowed_error.is_finite() || max_allowed_error < 0.0 {
        return Err(QuantizationError::InvalidErrorLimit(max_allowed_error));
    }
    if positions.iter().flatten().any(|value| !value.is_finite()) {
        return Err(QuantizationError::InvalidPosition);
    }

    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for position in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    if positions.is_empty() {
        min = [0.0; 3];
        max = [0.0; 3];
    }

    let mut scale = [0.0; 3];
    for axis in 0..3 {
        let span = max[axis] - min[axis];
        if !span.is_finite() {
            return Err(QuantizationError::CoordinateSpanOutOfRange { axis });
        }
        if span > 0.0 {
            scale[axis] = span / f64::from(MAX_QUANTIZED_COORDINATE);
            if !scale[axis].is_finite() || scale[axis] <= 0.0 {
                return Err(QuantizationError::CoordinateSpanOutOfRange { axis });
            }
        }
    }

    let transform = QuantizationTransform { origin: min, scale };
    let mut coordinates = Vec::with_capacity(positions.len());
    let mut max_error: f64 = 0.0;
    for position in positions {
        let mut quantized = [0; 3];
        let mut decoded_position = [0.0; 3];
        let mut decoded_f32_position = [0.0; 3];
        for axis in 0..3 {
            let q = if scale[axis] == 0.0 || position[axis] == min[axis] {
                0.0
            } else if position[axis] == max[axis] {
                f64::from(MAX_QUANTIZED_COORDINATE)
            } else {
                ((position[axis] - min[axis]) / scale[axis]).round()
            };
            if !q.is_finite() || q < 0.0 || q > f64::from(MAX_QUANTIZED_COORDINATE) {
                return Err(QuantizationError::CoordinateOutOfRange { axis, value: q });
            }
            quantized[axis] = q as u32;

            let decoded = min[axis] + f64::from(quantized[axis]) * scale[axis];
            let decoded_f32 = decoded as f32;
            if !decoded.is_finite() || !decoded_f32.is_finite() {
                return Err(QuantizationError::DecodedPositionOutOfRange([
                    decoded, decoded, decoded,
                ]));
            }
            decoded_position[axis] = decoded;
            decoded_f32_position[axis] = f64::from(decoded_f32);
        }
        let error_bound = error_bound(*position, decoded_position, decoded_f32_position)?;
        max_error = max_error.max(error_bound);
        coordinates.push(quantized);
    }

    if max_error > max_allowed_error {
        return Err(QuantizationError::ErrorLimitExceeded {
            required: max_error,
            allowed: max_allowed_error,
        });
    }

    Ok(QuantizedCoordinates {
        transform,
        coordinates,
        max_error,
    })
}

/// Conservative bound on the distance from `position` to the decoded `f32` position.
///
/// `norm3` is within a few ulp of the exact norm, far below `arithmetic_margin`
/// (8 * EPSILON * (|p| + |d| + 1) per axis), so the bound holds; `f64::hypot` is avoided
/// because libm differs by an ulp between platforms and the result is stored in the file.
fn error_bound(
    position: [f64; 3],
    decoded_position: [f64; 3],
    decoded_f32_position: [f64; 3],
) -> Result<f64, QuantizationError> {
    let measured_error =
        norm3([0, 1, 2].map(|axis| (position[axis] - decoded_f32_position[axis]).abs()));
    let arithmetic_margin = if measured_error == 0.0 {
        0.0
    } else {
        norm3([0, 1, 2].map(|axis| {
            f64::EPSILON * (position[axis].abs() + decoded_position[axis].abs() + 1.0) * 8.0
        }))
    };
    let error_bound = measured_error + arithmetic_margin;
    if error_bound.is_finite() {
        Ok(error_bound)
    } else {
        Err(QuantizationError::ErrorBoundOutOfRange)
    }
}

/// Quantizes one block of frame-local positions onto the global `v7.4-pre` grid.
///
/// Each source coordinate is `frame_origin + local` in `f64`, rounded to `i64` quanta of 2^-15
/// and stored as a `u32` offset from the block origin (the node origin, or the 2^32-quanta cell
/// of a split node). The error bound is measured against the exact `f32` the reader returns.
/// The returned origin is in quanta; the transform inside the result is informative only.
pub(crate) fn quantize_global_positions(
    positions: &[[f64; 3]],
    frame_origin: [f64; 3],
    frame_origin_quanta: [i64; 3],
    node: CellBox,
    max_allowed_error: f64,
) -> Result<(QuantizedCoordinates, [i64; 3]), QuantizationError> {
    if !max_allowed_error.is_finite() || max_allowed_error < 0.0 {
        return Err(QuantizationError::InvalidErrorLimit(max_allowed_error));
    }
    if positions.iter().flatten().any(|value| !value.is_finite()) {
        return Err(QuantizationError::InvalidPosition);
    }
    let mut block_origin = node.origin;
    let mut coordinates = Vec::with_capacity(positions.len());
    let mut max_error: f64 = 0.0;
    for (index, position) in positions.iter().enumerate() {
        let mut quanta = [0_i64; 3];
        for axis in 0..3 {
            let world = frame_origin[axis] + position[axis];
            quanta[axis] = quantize_coordinate(world)
                .ok_or(QuantizationError::CoordinateOutOfRange { axis, value: world })?;
        }
        if !node.contains(quanta.map(i128::from)) {
            return Err(QuantizationError::PointOutsideNode);
        }
        let cell_origin = block_cell_origin(node, quanta);
        if index == 0 {
            block_origin = cell_origin;
        } else if cell_origin != block_origin {
            return Err(QuantizationError::BlockSpansCells);
        }
        let offset = std::array::from_fn(|axis| {
            u32::try_from(i128::from(quanta[axis]) - i128::from(block_origin[axis]))
                .expect("the point lies inside the 2^32-quanta cell of its block origin")
        });
        let decoded = local_position_units(quanta.map(i128::from), frame_origin_quanta);
        let decoded_f32 = decoded.map(|value| value as f32);
        if decoded.iter().any(|value| !value.is_finite())
            || decoded_f32.iter().any(|value| !value.is_finite())
        {
            return Err(QuantizationError::DecodedPositionOutOfRange(decoded));
        }
        let bound = error_bound(*position, decoded, decoded_f32.map(f64::from))?;
        max_error = max_error.max(bound);
        coordinates.push(offset);
    }
    if max_error > max_allowed_error {
        return Err(QuantizationError::ErrorLimitExceeded {
            required: max_error,
            allowed: max_allowed_error,
        });
    }
    let transform = QuantizationTransform {
        origin: [0.0; 3],
        scale: [UNITS_PER_QUANTUM; 3],
    };
    Ok((
        QuantizedCoordinates::from_parts(transform, coordinates, max_error),
        block_origin,
    ))
}

/// Euclidean norm from IEEE-754 correctly rounded operations only (`+ * /` and `sqrt`), so it is
/// bit-identical on every target. Scaling by the largest axis avoids overflow and underflow.
pub fn norm3(v: [f64; 3]) -> f64 {
    let m = v[0].abs().max(v[1].abs()).max(v[2].abs());
    if m == 0.0 {
        return 0.0;
    }
    let (x, y, z) = (v[0] / m, v[1] / m, v[2] / m);
    m * (x * x + y * y + z * z).sqrt()
}

#[derive(Debug, Error, PartialEq)]
pub enum QuantizationError {
    #[error("maximum quantization error must be finite and non-negative, got {0}")]
    InvalidErrorLimit(f64),
    #[error("source positions must be finite")]
    InvalidPosition,
    #[error("coordinate span on axis {axis} cannot be represented")]
    CoordinateSpanOutOfRange { axis: usize },
    #[error("quantized coordinate on axis {axis} is outside u32: {value}")]
    CoordinateOutOfRange { axis: usize, value: f64 },
    #[error("decoded local coordinate cannot be represented as finite f32: {0:?}")]
    DecodedPositionOutOfRange([f64; 3]),
    #[error("quantization error bound cannot be represented")]
    ErrorBoundOutOfRange,
    #[error("point lies outside the node that owns its block")]
    PointOutsideNode,
    #[error("points of one block fall in different 2^32-quanta cells of a split node")]
    BlockSpansCells,
    #[error("quantization needs error bound {required}, exceeding allowed {allowed}")]
    ErrorLimitExceeded { required: f64, allowed: f64 },
}

#[cfg(test)]
mod tests {
    use super::{QuantizationError, norm3, quantize_local_positions};

    #[test]
    fn quantizes_each_block_to_u32_and_decodes_local_f32() {
        let source = [[-12.0, 5.0, 1.25], [8.0, 5.0, 1.25], [0.0, 5.0, 1.25]];
        let encoded = quantize_local_positions(&source, 0.001).unwrap();

        assert_eq!(encoded.coordinates()[0][0], 0);
        assert_eq!(encoded.coordinates()[1][0], u32::MAX);
        assert_eq!(encoded.transform().scale()[1..], [0.0, 0.0]);
        assert_eq!(encoded.decode_local().unwrap()[0], [-12.0, 5.0, 1.25]);
        assert!(encoded.max_error() <= 0.001);
        assert_eq!(std::mem::size_of::<[u32; 3]>(), 12);
    }

    #[test]
    fn error_bound_covers_the_reader_reconstruction() {
        let source = [[0.1, 0.2, 0.3], [1.7, 5.3, 8.9], [0.8, 2.4, 3.6]];
        let encoded = quantize_local_positions(&source, 0.01).unwrap();
        let decoded = encoded.decode_local().unwrap();
        for (source, decoded) in source.iter().zip(decoded) {
            let error = (0..3)
                .map(|axis| (source[axis] - f64::from(decoded[axis])).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!(error <= encoded.max_error());
        }
    }

    #[test]
    fn rejects_invalid_inputs_and_unachievable_tolerances() {
        assert_eq!(
            quantize_local_positions(&[[f64::NAN, 0.0, 0.0]], 1.0).unwrap_err(),
            QuantizationError::InvalidPosition
        );
        assert!(matches!(
            quantize_local_positions(&[[0.1, 0.2, 0.3], [1.0; 3]], 0.0),
            Err(QuantizationError::ErrorLimitExceeded { .. })
        ));
        assert!(matches!(
            quantize_local_positions(&[[f64::MIN, 0.0, 0.0], [f64::MAX, 0.0, 0.0]], 1.0),
            Err(QuantizationError::CoordinateSpanOutOfRange { axis: 0 })
        ));
    }

    #[test]
    fn empty_blocks_have_an_empty_transform_and_zero_error() {
        let encoded = quantize_local_positions(&[], 0.0).unwrap();
        assert!(
            encoded.coordinates().is_empty(),
            "{:?}",
            encoded.coordinates()
        );
        assert!(
            encoded.decode_local().unwrap().is_empty(),
            "{:?}",
            encoded.decode_local().unwrap()
        );
        assert_eq!(encoded.max_error(), 0.0);
    }

    #[test]
    fn exact_constant_positions_allow_zero_error() {
        let encoded = quantize_local_positions(&[[0.0, 1.0, -2.0]], 0.0).unwrap();

        assert_eq!(encoded.max_error(), 0.0);
        assert_eq!(encoded.decode_local().unwrap(), [[0.0, 1.0, -2.0]]);
    }

    #[test]
    fn norm3_is_exact_on_simple_inputs_and_survives_extreme_magnitudes() {
        assert_eq!(norm3([0.0; 3]), 0.0);
        for value in [0.0, 1e-300, 7.5, 1e300] {
            assert_eq!(norm3([value, 0.0, 0.0]), value);
            assert_eq!(norm3([0.0, value, 0.0]), value);
            assert_eq!(norm3([0.0, 0.0, value]), value);
        }
        assert_eq!(norm3([3.0, 4.0, 12.0]), 13.0);
        assert_eq!(norm3([12.0, 3.0, 4.0]), 13.0);
        // Squaring 1e300 overflows and squaring 1e-300 underflows; the scaling must avoid both.
        assert_eq!(norm3([1e-300, 2e-300, 2e-300]), 3e-300);
        assert_eq!(norm3([1e300, 2e300, 2e300]), 3e300);
    }

    #[test]
    fn error_bound_covers_the_reader_reconstruction_for_random_blocks() {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1_u64 << 53) as f64
        };
        let mut checked = 0;
        for block in 0..400 {
            // Offsets from 1e-6 to 1e7 and extents of up to 1e4 around them.
            let magnitude = 10.0_f64.powi(-6 + (block % 14));
            let extent = magnitude.min(1e4);
            let source: Vec<[f64; 3]> = (0..30)
                .map(|_| {
                    [0; 3].map(|_| {
                        let sign = if next() < 0.5 { -1.0 } else { 1.0 };
                        sign * magnitude + (next() - 0.5) * extent
                    })
                })
                .collect();
            let encoded = quantize_local_positions(&source, 1e9).unwrap();
            let decoded = encoded.decode_local().unwrap();
            for (source, decoded) in source.iter().zip(decoded) {
                let error = (0..3)
                    .map(|axis| (source[axis] - f64::from(decoded[axis])).powi(2))
                    .sum::<f64>()
                    .sqrt();
                assert!(error <= encoded.max_error(), "{source:?}: {error}");
                checked += 1;
            }
        }
        assert!(checked >= 10_000);
    }

    #[test]
    fn max_error_is_zero_when_every_point_is_exactly_representable() {
        let source = [[0.0, 1.0, -2.0], [4.0, 1.0, 6.0], [0.0, 1.0, 6.0]];
        let encoded = quantize_local_positions(&source, 0.0).unwrap();

        assert_eq!(encoded.max_error(), 0.0);
        let decoded = encoded.decode_local().unwrap();
        for (source, decoded) in source.iter().zip(decoded) {
            assert_eq!(*source, decoded.map(f64::from));
        }
    }
}
