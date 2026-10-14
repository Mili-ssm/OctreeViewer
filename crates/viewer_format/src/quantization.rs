use thiserror::Error;

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
        let measured_error = (0..3)
            .map(|axis| (position[axis] - decoded_f32_position[axis]).abs())
            .fold(0.0_f64, f64::hypot);
        let arithmetic_margin = if measured_error == 0.0 {
            0.0
        } else {
            (0..3)
                .map(|axis| {
                    f64::EPSILON * (position[axis].abs() + decoded_position[axis].abs() + 1.0) * 8.0
                })
                .fold(0.0_f64, f64::hypot)
        };
        let error_bound = measured_error + arithmetic_margin;
        if !error_bound.is_finite() {
            return Err(QuantizationError::ErrorBoundOutOfRange);
        }
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
    #[error("quantization needs error bound {required}, exceeding allowed {allowed}")]
    ErrorLimitExceeded { required: f64, allowed: f64 },
}

#[cfg(test)]
mod tests {
    use super::{QuantizationError, quantize_local_positions};

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
        assert!(encoded.coordinates().is_empty());
        assert!(encoded.decode_local().unwrap().is_empty());
        assert_eq!(encoded.max_error(), 0.0);
    }

    #[test]
    fn exact_constant_positions_allow_zero_error() {
        let encoded = quantize_local_positions(&[[0.0, 1.0, -2.0]], 0.0).unwrap();

        assert_eq!(encoded.max_error(), 0.0);
        assert_eq!(encoded.decode_local().unwrap(), [[0.0, 1.0, -2.0]]);
    }
}
