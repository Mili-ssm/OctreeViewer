use crate::FormatError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GeometryEncoding {
    RawU32 = 0,
    DeltaVarint = 1,
}

impl TryFrom<u8> for GeometryEncoding {
    type Error = FormatError;

    fn try_from(tag: u8) -> Result<Self, Self::Error> {
        match tag {
            0 => Ok(Self::RawU32),
            1 => Ok(Self::DeltaVarint),
            tag => Err(FormatError::InvalidTag {
                field: "geometry encoding",
                tag,
            }),
        }
    }
}

impl GeometryEncoding {
    pub fn validate_raw_length(self, point_count: u64, raw_length: u64) -> Result<(), FormatError> {
        let min = point_count
            .checked_mul(3)
            .ok_or(FormatError::BlockSizeOverflow)?;
        let max = point_count
            .checked_mul(15)
            .ok_or(FormatError::BlockSizeOverflow)?;
        let (lower, upper) = match self {
            Self::RawU32 => {
                let exact = point_count
                    .checked_mul(12)
                    .ok_or(FormatError::BlockSizeOverflow)?;
                (exact, exact)
            }
            Self::DeltaVarint => (min, max),
        };
        if raw_length < lower || raw_length > upper {
            return Err(FormatError::MalformedBlock("invalid geometry raw length"));
        }
        Ok(())
    }
}

pub(crate) fn encode_delta(coords: &[[u32; 3]]) -> Result<Vec<u8>, FormatError> {
    let point_count = u64::try_from(coords.len()).map_err(|_| FormatError::PointCountOutOfRange)?;
    let max_len = point_count
        .checked_mul(15)
        .ok_or(FormatError::BlockSizeOverflow)?;
    let capacity = usize::try_from(max_len).map_err(|_| FormatError::SizeOutOfRange(max_len))?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|_| FormatError::AllocationFailed(capacity))?;

    let mut previous = [0u32; 3];
    for coordinate in coords {
        for axis in 0..3 {
            let delta = i64::from(coordinate[axis]) - i64::from(previous[axis]);
            previous[axis] = coordinate[axis];
            let zigzag = ((delta << 1) ^ (delta >> 63)) as u64;
            let mut value = zigzag;
            while value >= 0x80 {
                output.push((value as u8) | 0x80);
                value >>= 7;
            }
            output.push(value as u8);
        }
    }
    Ok(output)
}

pub(crate) struct GeometryDecoder<'a> {
    encoding: GeometryEncoding,
    bytes: &'a [u8],
    offset: usize,
    previous: [u32; 3],
}

impl<'a> GeometryDecoder<'a> {
    pub(crate) fn new(encoding: GeometryEncoding, bytes: &'a [u8]) -> Self {
        Self {
            encoding,
            bytes,
            offset: 0,
            previous: [0; 3],
        }
    }

    pub(crate) fn next_coordinate(&mut self) -> Result<[u32; 3], FormatError> {
        let mut coordinate = [0u32; 3];
        for (axis, component) in coordinate.iter_mut().enumerate() {
            *component = match self.encoding {
                GeometryEncoding::RawU32 => {
                    let end = self
                        .offset
                        .checked_add(4)
                        .ok_or(FormatError::BlockSizeOverflow)?;
                    let bytes = self
                        .bytes
                        .get(self.offset..end)
                        .ok_or(FormatError::MalformedBlock("truncated raw geometry"))?;
                    self.offset = end;
                    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                }
                GeometryEncoding::DeltaVarint => {
                    let zigzag = self.read_varint()?;
                    let delta = ((zigzag >> 1) as i64) ^ (-((zigzag & 1) as i64));
                    let value = i64::from(self.previous[axis]) + delta;
                    if !(0..=i64::from(u32::MAX)).contains(&value) {
                        return Err(FormatError::MalformedBlock(
                            "delta geometry coordinate out of range",
                        ));
                    }
                    value as u32
                }
            };
            self.previous[axis] = *component;
        }
        Ok(coordinate)
    }

    pub(crate) fn finish(self) -> Result<(), FormatError> {
        if self.offset != self.bytes.len() {
            return Err(FormatError::MalformedBlock("trailing geometry bytes"));
        }
        Ok(())
    }

    fn read_varint(&mut self) -> Result<u64, FormatError> {
        let mut value = 0u64;
        for index in 0..5 {
            let byte = *self
                .bytes
                .get(self.offset)
                .ok_or(FormatError::MalformedBlock("truncated geometry varint"))?;
            self.offset += 1;
            if index == 4 && (byte & 0x80 != 0 || byte > 0x1f) {
                return Err(FormatError::MalformedBlock("geometry varint overflow"));
            }
            value |= u64::from(byte & 0x7f) << (index * 7);
            if byte & 0x80 == 0 {
                if index > 0 && byte == 0 {
                    return Err(FormatError::MalformedBlock("noncanonical geometry varint"));
                }
                return Ok(value);
            }
        }
        Err(FormatError::MalformedBlock("geometry varint overflow"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(coords: &[[u32; 3]]) {
        let encoded = encode_delta(coords).unwrap();
        let mut decoder = GeometryDecoder::new(GeometryEncoding::DeltaVarint, &encoded);
        for expected in coords {
            assert_eq!(decoder.next_coordinate().unwrap(), *expected);
        }
        decoder.finish().unwrap();
    }

    #[test]
    fn delta_round_trips_extremes_empty_and_repeated() {
        round_trip(&[]);
        round_trip(&[[0, 0, 0], [u32::MAX; 3], [0; 3], [0; 3]]);
    }

    #[test]
    fn delta_round_trips_deterministic_coordinates() {
        let mut state = 0x1234_5678u32;
        let coords: Vec<_> = (0..512)
            .map(|_| {
                std::array::from_fn(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    state
                })
            })
            .collect();
        round_trip(&coords);
    }

    #[test]
    fn rejects_malformed_varints_and_trailing_bytes() {
        for bytes in [
            &[0x80][..],
            &[0x80, 0][..],
            &[0xff, 0xff, 0xff, 0xff, 0x20][..],
        ] {
            let mut decoder = GeometryDecoder::new(GeometryEncoding::DeltaVarint, bytes);
            assert!(decoder.next_coordinate().is_err());
        }
        let mut decoder = GeometryDecoder::new(GeometryEncoding::DeltaVarint, &[0, 0, 0, 7]);
        assert_eq!(decoder.next_coordinate().unwrap(), [0; 3]);
        assert!(decoder.finish().is_err());
    }

    #[test]
    fn validates_lengths_and_tags() {
        assert!(GeometryEncoding::RawU32.validate_raw_length(0, 0).is_ok());
        assert!(GeometryEncoding::RawU32.validate_raw_length(2, 24).is_ok());
        assert!(
            GeometryEncoding::DeltaVarint
                .validate_raw_length(2, 6)
                .is_ok()
        );
        assert!(
            GeometryEncoding::DeltaVarint
                .validate_raw_length(2, 30)
                .is_ok()
        );
        assert!(GeometryEncoding::try_from(2).is_err());
        for (encoding, count, length) in [
            (GeometryEncoding::RawU32, 2, 23),
            (GeometryEncoding::DeltaVarint, 2, 5),
            (GeometryEncoding::DeltaVarint, 2, 31),
            (GeometryEncoding::DeltaVarint, 0, 1),
            (GeometryEncoding::DeltaVarint, u64::MAX, 0),
        ] {
            assert!(encoding.validate_raw_length(count, length).is_err());
        }
    }

    #[test]
    fn rejects_predictor_underflow_and_overflow() {
        let mut negative = GeometryDecoder::new(GeometryEncoding::DeltaVarint, &[1, 0, 0]);
        assert!(negative.next_coordinate().is_err());
        let mut encoded = encode_delta(&[[u32::MAX, 0, 0]]).unwrap();
        encoded.extend_from_slice(&[2, 0, 0]);
        let mut positive = GeometryDecoder::new(GeometryEncoding::DeltaVarint, &encoded);
        assert_eq!(positive.next_coordinate().unwrap(), [u32::MAX, 0, 0]);
        assert!(positive.next_coordinate().is_err());
    }
}
