//! LAS flags and Extra Bytes (RF-24) as attribute columns: the descriptor parser (VLR record 4 of
//! `LASF_Spec`), the dynamic schema builder and the per-point decoding. Independent of any LAS
//! parsing crate: callers hand over raw record bytes.
//!
//! Column design (documented in `FORMAT_V7_PRE5.md`): `las_flags` is one `u8` column; each Extra
//! Byte descriptor is **one column** typed as its stored element type, with 1 to 3 components for
//! the (deprecated but still found) array types 11..=30. Values are stored raw; scale, offset and
//! the no-data sentinel live in the descriptor, so nothing is rounded.

use crate::{AttributeDataType, AttributeDescriptor, AttributeValues, SpatialError};

/// Attribute columns a LAS-sourced schema may hold (base columns, flags and Extra Bytes).
pub const MAX_LAS_ATTRIBUTES: usize = 256;
/// Largest total Extra Bytes payload of one point that the converter carries.
pub const MAX_EXTRA_BYTES_PER_POINT: usize = 64;
/// Size of one Extra Bytes descriptor record in the LAS 1.4 specification.
pub const EXTRA_BYTE_DESCRIPTOR_LEN: usize = 192;
/// Name of the flags column.
pub const LAS_FLAGS_NAME: &str = "las_flags";
/// Bits of the `las_flags` column.
pub const LAS_FLAG_SYNTHETIC: u8 = 1;
pub const LAS_FLAG_KEY_POINT: u8 = 2;
pub const LAS_FLAG_WITHHELD: u8 = 4;
pub const LAS_FLAG_OVERLAP: u8 = 8;

/// `options` bits of a descriptor that are kept.
pub const EXTRA_OPTION_NO_DATA: u8 = 1;
pub const EXTRA_OPTION_SCALE: u8 = 1 << 3;
pub const EXTRA_OPTION_OFFSET: u8 = 1 << 4;
const KEPT_OPTIONS: u8 = EXTRA_OPTION_NO_DATA | EXTRA_OPTION_SCALE | EXTRA_OPTION_OFFSET;
const NAME_LEN: usize = 32;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum LasExtraBytesError {
    #[error("Extra Bytes descriptors are {0} B, not a multiple of {EXTRA_BYTE_DESCRIPTOR_LEN} B")]
    DescriptorLength(usize),
    #[error(
        "Extra Bytes descriptor {index} has data type {data_type}; only types 1..=30 are supported (0 is undocumented, 31..=255 are deprecated)"
    )]
    UnsupportedType { index: usize, data_type: u8 },
    #[error("Extra Bytes descriptor {0} has a name that is not UTF-8")]
    InvalidName(usize),
    #[error("Extra Bytes descriptor {0} has a non-finite scale or offset")]
    InvalidTransform(usize),
    #[error("point format carries {record} Extra Bytes but the descriptors describe {described} B")]
    SizeMismatch { record: usize, described: usize },
    #[error("{bytes} Extra Bytes per point exceed the supported {MAX_EXTRA_BYTES_PER_POINT} B")]
    TooManyBytes { bytes: usize },
    #[error("the schema would hold {count} attributes, more than the {MAX_LAS_ATTRIBUTES} allowed")]
    TooManyAttributes { count: usize },
    #[error(transparent)]
    Schema(#[from] SpatialError),
}

/// One Extra Bytes descriptor: what the converter keeps of the 192-byte LAS record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LasExtraByteFields {
    /// LAS `data_type`, 1..=30.
    pub las_type: u8,
    /// Kept `options` bits (no data, scale, offset).
    pub options: u8,
    /// The `no_data` field of each element, raw 8 bytes little endian (typed by `las_type`).
    pub no_data: [[u8; 8]; 3],
    pub scale: [f64; 3],
    pub offset: [f64; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct LasExtraByte {
    name: String,
    fields: LasExtraByteFields,
}

impl LasExtraByte {
    /// Validates the type, the name (≤ 32 bytes, no NUL) and the transform; options outside
    /// the kept bits are dropped, and an absent scale or offset is normalised to 1 and 0.
    pub fn new(
        name: impl Into<String>,
        mut fields: LasExtraByteFields,
    ) -> Result<Self, LasExtraBytesError> {
        let name = name.into();
        if !(1..=30).contains(&fields.las_type) {
            return Err(LasExtraBytesError::UnsupportedType {
                index: 0,
                data_type: fields.las_type,
            });
        }
        if name.len() > NAME_LEN || name.contains('\0') {
            return Err(LasExtraBytesError::InvalidName(0));
        }
        fields.options &= KEPT_OPTIONS;
        for element in 0..3 {
            if fields.options & EXTRA_OPTION_SCALE == 0 {
                fields.scale[element] = 1.0;
            }
            if fields.options & EXTRA_OPTION_OFFSET == 0 {
                fields.offset[element] = 0.0;
            }
            if fields.options & EXTRA_OPTION_NO_DATA == 0 {
                fields.no_data[element] = [0; 8];
            }
            if !fields.scale[element].is_finite() || !fields.offset[element].is_finite() {
                return Err(LasExtraBytesError::InvalidTransform(0));
            }
        }
        Ok(Self { name, fields })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn fields(&self) -> &LasExtraByteFields {
        &self.fields
    }

    pub fn has_no_data(&self) -> bool {
        self.fields.options & EXTRA_OPTION_NO_DATA != 0
    }

    pub fn is_scaled(&self) -> bool {
        self.fields.options & (EXTRA_OPTION_SCALE | EXTRA_OPTION_OFFSET) != 0
    }

    /// Stored element type.
    pub fn data_type(&self) -> AttributeDataType {
        match (self.fields.las_type - 1) % 10 {
            0 => AttributeDataType::U8,
            1 => AttributeDataType::I8,
            2 => AttributeDataType::U16,
            3 => AttributeDataType::I16,
            4 => AttributeDataType::U32,
            5 => AttributeDataType::I32,
            6 => AttributeDataType::U64,
            7 => AttributeDataType::I64,
            8 => AttributeDataType::F32,
            _ => AttributeDataType::F64,
        }
    }

    /// Elements per point: 1 for types 1..=10, 2 for 11..=20, 3 for 21..=30.
    pub fn components(&self) -> u32 {
        u32::from((self.fields.las_type - 1) / 10) + 1
    }

    fn element_len(&self) -> usize {
        match self.data_type() {
            AttributeDataType::U8 | AttributeDataType::I8 => 1,
            AttributeDataType::U16 | AttributeDataType::I16 => 2,
            AttributeDataType::U32 | AttributeDataType::I32 | AttributeDataType::F32 => 4,
            AttributeDataType::U64 | AttributeDataType::I64 | AttributeDataType::F64 => 8,
        }
    }

    /// Bytes this attribute takes in a point record.
    pub fn byte_len(&self) -> usize {
        self.element_len() * self.components() as usize
    }

    /// Empty column values of the stored type, with room for `points` rows.
    pub fn new_values(&self, points: usize) -> AttributeValues {
        let capacity = points.saturating_mul(self.components() as usize);
        match self.data_type() {
            AttributeDataType::U8 => AttributeValues::U8(Vec::with_capacity(capacity)),
            AttributeDataType::I8 => AttributeValues::I8(Vec::with_capacity(capacity)),
            AttributeDataType::U16 => AttributeValues::U16(Vec::with_capacity(capacity)),
            AttributeDataType::I16 => AttributeValues::I16(Vec::with_capacity(capacity)),
            AttributeDataType::U32 => AttributeValues::U32(Vec::with_capacity(capacity)),
            AttributeDataType::I32 => AttributeValues::I32(Vec::with_capacity(capacity)),
            AttributeDataType::U64 => AttributeValues::U64(Vec::with_capacity(capacity)),
            AttributeDataType::I64 => AttributeValues::I64(Vec::with_capacity(capacity)),
            AttributeDataType::F32 => AttributeValues::F32(Vec::with_capacity(capacity)),
            AttributeDataType::F64 => AttributeValues::F64(Vec::with_capacity(capacity)),
        }
    }

    /// Appends the elements of one point. `raw` is exactly [`Self::byte_len`] bytes, and `values`
    /// the variant of [`Self::new_values`].
    pub fn push_values(&self, raw: &[u8], values: &mut AttributeValues) {
        macro_rules! push {
            ($vec:expr, $type:ty) => {{
                const WIDTH: usize = std::mem::size_of::<$type>();
                $vec.extend(
                    raw.chunks_exact(WIDTH)
                        .map(|bytes| <$type>::from_le_bytes(bytes.try_into().expect("exact"))),
                );
            }};
        }
        match values {
            AttributeValues::U8(v) => push!(v, u8),
            AttributeValues::I8(v) => push!(v, i8),
            AttributeValues::U16(v) => push!(v, u16),
            AttributeValues::I16(v) => push!(v, i16),
            AttributeValues::U32(v) => push!(v, u32),
            AttributeValues::I32(v) => push!(v, i32),
            AttributeValues::U64(v) => push!(v, u64),
            AttributeValues::I64(v) => push!(v, i64),
            AttributeValues::F32(v) => push!(v, f32),
            AttributeValues::F64(v) => push!(v, f64),
        }
    }
}

/// Parses the payload of the Extra Bytes VLR (`LASF_Spec`, record 4), or of several of them
/// concatenated. Deprecated or undocumented types are an explicit error.
pub fn parse_extra_byte_descriptors(data: &[u8]) -> Result<Vec<LasExtraByte>, LasExtraBytesError> {
    if !data.len().is_multiple_of(EXTRA_BYTE_DESCRIPTOR_LEN) {
        return Err(LasExtraBytesError::DescriptorLength(data.len()));
    }
    data.as_chunks::<EXTRA_BYTE_DESCRIPTOR_LEN>()
        .0
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let at = |start: usize| -> [u8; 8] {
                record[start..start + 8].try_into().expect("fixed width")
            };
            let float = |start: usize| f64::from_le_bytes(at(start));
            let data_type = record[2];
            if !(1..=30).contains(&data_type) {
                return Err(LasExtraBytesError::UnsupportedType { index, data_type });
            }
            let name_bytes = &record[4..4 + NAME_LEN];
            let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
            let name = std::str::from_utf8(&name_bytes[..name_end])
                .map_err(|_| LasExtraBytesError::InvalidName(index))?;
            let fields = LasExtraByteFields {
                las_type: data_type,
                options: record[3],
                no_data: std::array::from_fn(|element| at(40 + 8 * element)),
                scale: std::array::from_fn(|element| float(112 + 8 * element)),
                offset: std::array::from_fn(|element| float(136 + 8 * element)),
            };
            LasExtraByte::at_index(name, fields, index)
        })
        .collect()
}

impl LasExtraByte {
    fn at_index(
        name: &str,
        fields: LasExtraByteFields,
        index: usize,
    ) -> Result<Self, LasExtraBytesError> {
        Self::new(name, fields).map_err(|error| match error {
            LasExtraBytesError::InvalidTransform(_) => LasExtraBytesError::InvalidTransform(index),
            other => other,
        })
    }
}

/// Which LAS-derived columns a schema holds, in on-disk order: the seven base columns, then
/// `gps_time`, `rgb`, `nir`, `las_flags` and the Extra Bytes.
#[derive(Debug, Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per optional LAS column group; part of the public API"
)]
pub struct LasSchemaSpec<'a> {
    pub has_gps_time: bool,
    pub has_color: bool,
    pub has_nir: bool,
    pub las_flags: bool,
    pub extra_bytes: &'a [LasExtraByte],
}

/// Checks that the descriptors cover the point record exactly. `record_extra_bytes`
/// is the Extra Bytes size of the point format.
pub fn validate_extra_bytes(
    extra_bytes: &[LasExtraByte],
    record_extra_bytes: usize,
) -> Result<(), LasExtraBytesError> {
    let described: usize = extra_bytes.iter().map(LasExtraByte::byte_len).sum();
    if described != record_extra_bytes {
        return Err(LasExtraBytesError::SizeMismatch {
            record: record_extra_bytes,
            described,
        });
    }
    Ok(())
}

/// Dynamic form of [`super::las_attribute_schema`]: adds `las_flags` and one column per Extra
/// Byte (`extra:<name>`; empty or repeated names get `#<index>` so column names stay unique).
pub fn las_attribute_schema_for(
    spec: &LasSchemaSpec<'_>,
) -> Result<Vec<AttributeDescriptor>, LasExtraBytesError> {
    let mut schema = super::las_attribute_schema(spec.has_gps_time, spec.has_color, spec.has_nir);
    if spec.las_flags {
        schema.push(AttributeDescriptor::new(
            LAS_FLAGS_NAME,
            AttributeDataType::U8,
            1,
        )?);
    }
    let count = schema.len() + spec.extra_bytes.len();
    if count > MAX_LAS_ATTRIBUTES {
        return Err(LasExtraBytesError::TooManyAttributes { count });
    }
    let bytes: usize = spec.extra_bytes.iter().map(LasExtraByte::byte_len).sum();
    if bytes > MAX_EXTRA_BYTES_PER_POINT {
        return Err(LasExtraBytesError::TooManyBytes { bytes });
    }
    for (index, extra) in spec.extra_bytes.iter().enumerate() {
        let mut name = format!("extra:{}", extra.name());
        if extra.name().is_empty() || schema.iter().any(|d| d.name() == name) {
            name = format!("extra:{}#{index}", extra.name());
        }
        schema.push(AttributeDescriptor::new(
            name,
            extra.data_type(),
            extra.components(),
        )?);
    }
    Ok(schema)
}

/// `las_flags` bits of a point record: from the classification byte (bits 5..=7, formats 0..=5,
/// no overlap bit there) or from the flags byte (low nibble, formats 6..=10).
#[inline]
pub fn las_flags_of_record(record: &[u8], is_extended: bool) -> u8 {
    let byte = record[15];
    if is_extended { byte & 0x0F } else { byte >> 5 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor_bytes(
        data_type: u8,
        options: u8,
        name: &str,
        scale: f64,
        offset: f64,
    ) -> Vec<u8> {
        let mut record = vec![0_u8; EXTRA_BYTE_DESCRIPTOR_LEN];
        record[2] = data_type;
        record[3] = options;
        record[4..4 + name.len()].copy_from_slice(name.as_bytes());
        record[40..48].copy_from_slice(&(-9_i64).to_le_bytes());
        for element in 0..3 {
            record[112 + 8 * element..120 + 8 * element].copy_from_slice(&scale.to_le_bytes());
            record[136 + 8 * element..144 + 8 * element].copy_from_slice(&offset.to_le_bytes());
        }
        record
    }

    #[test]
    fn parses_types_arrays_and_transforms() {
        let mut data = descriptor_bytes(4, 1 | 8 | 16, "amp", 0.5, 10.0);
        data.extend(descriptor_bytes(22, 0, "vec", 7.0, 7.0));
        let parsed = parse_extra_byte_descriptors(&data).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].data_type(), AttributeDataType::I16);
        assert_eq!((parsed[0].components(), parsed[0].byte_len()), (1, 2));
        assert!(parsed[0].has_no_data() && parsed[0].is_scaled());
        assert_eq!(parsed[0].fields().no_data[0], (-9_i64).to_le_bytes());
        assert_eq!(parsed[0].fields().scale[0], 0.5);
        assert_eq!(parsed[0].fields().offset[0], 10.0);
        // 22 = three elements of type 2 (i8); unset scale/offset/no-data are normalised.
        assert_eq!(parsed[1].data_type(), AttributeDataType::I8);
        assert_eq!((parsed[1].components(), parsed[1].byte_len()), (3, 3));
        assert_eq!(parsed[1].fields().scale, [1.0; 3]);
        assert_eq!(parsed[1].fields().no_data, [[0; 8]; 3]);
        assert!(!parsed[1].is_scaled());
    }

    #[test]
    fn rejects_undocumented_deprecated_and_malformed_descriptors() {
        for data_type in [0_u8, 31, 200, 255] {
            assert_eq!(
                parse_extra_byte_descriptors(&descriptor_bytes(data_type, 0, "x", 1.0, 0.0)),
                Err(LasExtraBytesError::UnsupportedType {
                    index: 0,
                    data_type
                })
            );
        }
        assert_eq!(
            parse_extra_byte_descriptors(&[0; 191]),
            Err(LasExtraBytesError::DescriptorLength(191))
        );
        let mut bad_name = descriptor_bytes(1, 0, "", 1.0, 0.0);
        bad_name[4] = 0xFF;
        assert_eq!(
            parse_extra_byte_descriptors(&bad_name),
            Err(LasExtraBytesError::InvalidName(0))
        );
        let nan_scale = descriptor_bytes(1, 8, "x", f64::NAN, 0.0);
        assert_eq!(
            parse_extra_byte_descriptors(&nan_scale),
            Err(LasExtraBytesError::InvalidTransform(0))
        );
    }

    #[test]
    fn schema_appends_flags_then_extra_bytes_with_unique_names() {
        let mut data = descriptor_bytes(1, 0, "a", 1.0, 0.0);
        data.extend(descriptor_bytes(1, 0, "a", 1.0, 0.0));
        data.extend(descriptor_bytes(20, 0, "", 1.0, 0.0));
        let extras = parse_extra_byte_descriptors(&data).unwrap();
        let schema = las_attribute_schema_for(&LasSchemaSpec {
            has_gps_time: true,
            has_color: false,
            has_nir: false,
            las_flags: true,
            extra_bytes: &extras,
        })
        .unwrap();
        let names: Vec<_> = schema
            .iter()
            .map(crate::spatial::AttributeDescriptor::name)
            .collect();
        assert_eq!(
            &names[7..],
            ["gps_time", "las_flags", "extra:a", "extra:a#1", "extra:#2"]
        );
        assert_eq!(schema[7].data_type(), AttributeDataType::F64);
        assert_eq!(schema[11].component_count(), 2);
        validate_extra_bytes(&extras, 1 + 1 + 16).unwrap();
        assert_eq!(
            validate_extra_bytes(&extras, 3),
            Err(LasExtraBytesError::SizeMismatch {
                record: 3,
                described: 18
            })
        );
    }

    #[test]
    fn schema_is_capped_at_256_attributes_and_64_extra_bytes() {
        let one = descriptor_bytes(1, 0, "b", 1.0, 0.0);
        let at_cap = parse_extra_byte_descriptors(&one.repeat(MAX_LAS_ATTRIBUTES - 8)).unwrap();
        let spec = |extra_bytes| LasSchemaSpec {
            has_gps_time: false,
            has_color: false,
            has_nir: false,
            las_flags: true,
            extra_bytes,
        };
        // 8 base columns + 248 extras is exactly 256: only the byte cap trips.
        assert!(matches!(
            las_attribute_schema_for(&spec(&at_cap)),
            Err(LasExtraBytesError::TooManyBytes { bytes: 248 })
        ));
        let over = parse_extra_byte_descriptors(&one.repeat(MAX_LAS_ATTRIBUTES - 7)).unwrap();
        assert_eq!(
            las_attribute_schema_for(&spec(&over)).unwrap_err(),
            LasExtraBytesError::TooManyAttributes { count: 257 }
        );
        let fits = parse_extra_byte_descriptors(&one.repeat(MAX_EXTRA_BYTES_PER_POINT)).unwrap();
        assert_eq!(
            las_attribute_schema_for(&spec(&fits)).unwrap().len(),
            8 + 64
        );
    }

    #[test]
    fn decodes_raw_elements_per_type() {
        let array = LasExtraByte::new(
            "v",
            LasExtraByteFields {
                las_type: 19, // two f32
                options: 0,
                no_data: [[0; 8]; 3],
                scale: [1.0; 3],
                offset: [0.0; 3],
            },
        )
        .unwrap();
        assert_eq!(array.byte_len(), 8);
        let mut values = array.new_values(2);
        let mut raw = 1.5_f32.to_le_bytes().to_vec();
        raw.extend((-2.0_f32).to_le_bytes());
        array.push_values(&raw, &mut values);
        assert_eq!(values, AttributeValues::F32(vec![1.5, -2.0]));
    }

    #[test]
    fn flags_come_from_the_classification_byte_or_the_flags_byte() {
        let mut record = [0_u8; 30];
        record[15] = 0b1110_0101; // withheld, key-point, synthetic, class 5
        assert_eq!(las_flags_of_record(&record, false), 0b111);
        record[15] = 0b1111_1010; // edge, direction, channel 3, overlap, withheld? low nibble 1010
        assert_eq!(
            las_flags_of_record(&record, true),
            LAS_FLAG_KEY_POINT | LAS_FLAG_OVERLAP
        );
    }
}
