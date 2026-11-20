use std::collections::BTreeSet;

use viewer_core::{AttributeDataType, AttributeValues, BlockId, SpatialBounds};

use crate::layout::{PayloadDirectoryEntry, StorageLayout, StoredColumn, attribute_payload_size};
use crate::prepare::{BlockRecord, encode_values};
use crate::{FormatError, crc32, crc32::checksum_matches};

pub(crate) const WRITER_RECORD_MAGIC: &[u8; 8] = b"OCTVWRC1";
pub(crate) const WRITER_RECORD_VERSION: u16 = 1;

pub(crate) fn validate_checkpoint_record(
    record: &BlockRecord,
    layout: StorageLayout,
    split: bool,
    schema: &[viewer_core::AttributeDescriptor],
) -> Result<(), FormatError> {
    if !record.quantization_error.is_finite()
        || record.quantization_error < 0.0
        || record
            .origin
            .iter()
            .chain(record.scale.iter())
            .any(|v| !v.is_finite())
        || record.scale.iter().any(|v| *v < 0.0)
    {
        return Err(FormatError::MalformedIndex(
            "writer checkpoint numeric metadata",
        ));
    }
    for (index, payload) in record.payloads.iter().enumerate() {
        if matches!(
            payload.codec,
            crate::BlockCodec::Zstd | crate::BlockCodec::BytePlanesZstd
        ) && !layout.supports_zstd()
        {
            return Err(FormatError::MalformedIndex(
                "writer checkpoint zstd codec not valid in this revision",
            ));
        }
        payload
            .offset
            .checked_add(payload.stored_length)
            .ok_or(FormatError::FileSizeOverflow)?;
        if payload.stored_length == 0 && payload.raw_length != 0 {
            return Err(FormatError::MalformedIndex(
                "writer checkpoint empty payload",
            ));
        }
        if index == 0 {
            record
                .geometry_encoding
                .validate_raw_length(record.point_count, payload.raw_length)?;
        } else if index == 1 {
            let expected = record
                .point_count
                .checked_mul(3)
                .ok_or(FormatError::BlockSizeOverflow)?;
            if payload.raw_length != expected {
                return Err(FormatError::MalformedIndex(
                    "writer checkpoint color raw length",
                ));
            }
        } else {
            let descriptor = schema.get(index - 2).ok_or(FormatError::MalformedIndex(
                "writer checkpoint attribute schema",
            ))?;
            if payload.raw_length != attribute_payload_size(record.point_count, descriptor)? {
                return Err(FormatError::MalformedIndex(
                    "writer checkpoint attribute raw length",
                ));
            }
        }
        if !split || index == 0 {
            let expected_offset = record.payloads[..index].iter().try_fold(0u64, |at, item| {
                at.checked_add(item.stored_length)
                    .ok_or(FormatError::FileSizeOverflow)
            })?;
            if payload.offset != expected_offset {
                return Err(FormatError::MalformedIndex(
                    "writer checkpoint payload offset",
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn encode_checkpoint_record(
    id: BlockId,
    record: &BlockRecord,
) -> Result<Vec<u8>, FormatError> {
    let mut out = Vec::new();
    out.extend_from_slice(WRITER_RECORD_MAGIC);
    out.extend_from_slice(&WRITER_RECORD_VERSION.to_le_bytes());
    out.extend_from_slice(&id.get().to_le_bytes());
    out.extend_from_slice(&record.offset.to_le_bytes());
    out.extend_from_slice(&record.payload_length.to_le_bytes());
    out.extend_from_slice(&record.point_count.to_le_bytes());
    out.push(record.geometry_encoding as u8);
    out.extend_from_slice(&record.quantization_error.to_bits().to_le_bytes());
    for value in record.origin.into_iter().chain(record.scale) {
        out.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    if let Some(bounds) = record.world_bounds {
        out.push(1);
        for value in bounds.min().into_iter().chain(bounds.max()) {
            out.extend_from_slice(&value.to_bits().to_le_bytes());
        }
    } else {
        out.push(0);
    }
    put_count(&mut out, record.payloads.len())?;
    for payload in &record.payloads {
        // Resumable layouts store every column; a record cannot say one is missing.
        if !payload.stored.is_present() {
            return Err(FormatError::InvalidMetadata(
                "checkpoint records store every column",
            ));
        }
        for value in [payload.offset, payload.stored_length, payload.raw_length] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.push(payload.codec as u8);
        out.extend_from_slice(&payload.checksum.to_le_bytes());
    }
    put_count(&mut out, record.ranges.len())?;
    for range in &record.ranges {
        out.extend_from_slice(&range.schema_index().to_le_bytes());
        encode_values(&range.min, &mut out);
        encode_values(&range.max, &mut out);
    }
    let checksum = crc32(&out);
    out.extend_from_slice(&checksum.to_le_bytes());
    Ok(out)
}

pub(crate) fn decode_checkpoint_record(
    bytes: &[u8],
    schema: &[viewer_core::AttributeDescriptor],
    layout: StorageLayout,
    split: bool,
) -> Result<(BlockId, BlockRecord), FormatError> {
    let mut maximum_length = 160usize
        .checked_add(
            schema
                .len()
                .checked_add(2)
                .ok_or(FormatError::SchemaTooLarge)?
                .checked_mul(29)
                .ok_or(FormatError::BlockSizeOverflow)?,
        )
        .ok_or(FormatError::BlockSizeOverflow)?;
    for descriptor in schema {
        let width = usize::try_from(crate::layout::primitive_width(descriptor.data_type()))
            .map_err(|_| FormatError::BlockSizeOverflow)?;
        maximum_length = maximum_length
            .checked_add(4)
            .and_then(|n| {
                n.checked_add(
                    (descriptor.component_count() as usize)
                        .checked_mul(width)?
                        .checked_mul(2)?,
                )
            })
            .ok_or(FormatError::BlockSizeOverflow)?;
    }
    if bytes.len() > maximum_length {
        return Err(FormatError::MalformedIndex(
            "writer checkpoint exceeds schema bounds",
        ));
    }
    if bytes.len() < 14 {
        return Err(FormatError::MalformedIndex("truncated writer checkpoint"));
    }
    let data_len = bytes
        .len()
        .checked_sub(4)
        .ok_or(FormatError::MalformedIndex("truncated writer checkpoint"))?;
    let expected_crc = u32::from_le_bytes(
        bytes[data_len..]
            .try_into()
            .map_err(|_| FormatError::MalformedIndex("truncated writer checkpoint"))?,
    );
    if !checksum_matches(&bytes[..data_len], expected_crc) {
        return Err(FormatError::InvalidChecksum("writer checkpoint"));
    }
    let mut input = CheckpointInput {
        bytes: &bytes[..data_len],
        at: 0,
    };
    if input.take(8)? != WRITER_RECORD_MAGIC || input.u16()? != WRITER_RECORD_VERSION {
        return Err(FormatError::MalformedIndex("writer checkpoint version"));
    }
    let id = BlockId::new(input.u64()?);
    let offset = input.u64()?;
    let payload_length = input.u64()?;
    let point_count = input.u64()?;
    let geometry_encoding = crate::GeometryEncoding::try_from(input.u8()?)?;
    let quantization_error = f64::from_bits(input.u64()?);
    let origin = input.triple()?;
    let scale = input.triple()?;
    let world_bounds = match input.u8()? {
        0 => None,
        1 => Some(SpatialBounds::new(input.triple()?, input.triple()?)?),
        tag => {
            return Err(FormatError::InvalidTag {
                field: "checkpoint bounds",
                tag,
            });
        }
    };
    let payload_count = input.count(
        schema
            .len()
            .checked_add(2)
            .ok_or(FormatError::SchemaTooLarge)?,
    )?;
    let mut payloads = Vec::new();
    payloads
        .try_reserve_exact(payload_count)
        .map_err(|_| FormatError::AllocationFailed(payload_count))?;
    for _ in 0..payload_count {
        payloads.push(PayloadDirectoryEntry {
            offset: input.u64()?,
            stored_length: input.u64()?,
            raw_length: input.u64()?,
            codec: crate::BlockCodec::try_from(input.u8()?)?,
            checksum: input.u32()?,
            stored: StoredColumn::Present,
        });
    }
    if payload_count != schema.len() + 2 {
        return Err(FormatError::MalformedIndex(
            "writer checkpoint payload count",
        ));
    }
    let range_count = input.count(schema.len())?;
    let mut ranges = Vec::new();
    ranges
        .try_reserve_exact(range_count)
        .map_err(|_| FormatError::AllocationFailed(range_count))?;
    let mut seen = BTreeSet::new();
    for _ in 0..range_count {
        let schema_index = input.u32()?;
        let descriptor = schema
            .get(schema_index as usize)
            .ok_or(FormatError::MalformedIndex(
                "writer checkpoint range schema",
            ))?;
        if !seen.insert(schema_index) {
            return Err(FormatError::MalformedIndex(
                "writer checkpoint duplicate range",
            ));
        }
        let count = descriptor.component_count() as usize;
        let min = input.values(descriptor.data_type(), count)?;
        let max = input.values(descriptor.data_type(), count)?;
        for component in 0..count {
            let low = min
                .scalar_as_f64(component)
                .ok_or(FormatError::MalformedIndex("writer checkpoint range"))?;
            let high = max
                .scalar_as_f64(component)
                .ok_or(FormatError::MalformedIndex("writer checkpoint range"))?;
            if !low.is_finite() || !high.is_finite() || low > high {
                return Err(FormatError::MalformedIndex(
                    "writer checkpoint range values",
                ));
            }
        }
        ranges.push(crate::NodeAttributeRange::new(schema_index, min, max));
    }
    if input.at != input.bytes.len() {
        return Err(FormatError::MalformedIndex(
            "writer checkpoint trailing bytes",
        ));
    }
    let record = BlockRecord {
        offset,
        payload_length,
        point_count,
        geometry_encoding,
        quantization_error,
        origin,
        scale,
        origin_quanta: [0; 3],
        world_bounds,
        payloads,
        ranges,
    };
    validate_checkpoint_record(&record, layout, split, schema)?;
    Ok((id, record))
}

pub(crate) fn put_count(out: &mut Vec<u8>, count: usize) -> Result<(), FormatError> {
    out.extend_from_slice(
        &u32::try_from(count)
            .map_err(|_| FormatError::TooManyBlocks)?
            .to_le_bytes(),
    );
    Ok(())
}

pub(crate) struct CheckpointInput<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) at: usize,
}
impl CheckpointInput<'_> {
    pub(crate) fn take(&mut self, len: usize) -> Result<&[u8], FormatError> {
        let end = self.at.checked_add(len).ok_or(FormatError::MalformedIndex(
            "writer checkpoint length overflow",
        ))?;
        let out = self
            .bytes
            .get(self.at..end)
            .ok_or(FormatError::MalformedIndex("truncated writer checkpoint"))?;
        self.at = end;
        Ok(out)
    }
    pub(crate) fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn u16(&mut self) -> Result<u16, FormatError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().map_err(
            |_| FormatError::MalformedIndex("truncated writer checkpoint"),
        )?))
    }
    pub(crate) fn u32(&mut self) -> Result<u32, FormatError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(
            |_| FormatError::MalformedIndex("truncated writer checkpoint"),
        )?))
    }
    pub(crate) fn u64(&mut self) -> Result<u64, FormatError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(
            |_| FormatError::MalformedIndex("truncated writer checkpoint"),
        )?))
    }
    pub(crate) fn triple(&mut self) -> Result<[f64; 3], FormatError> {
        Ok([
            f64::from_bits(self.u64()?),
            f64::from_bits(self.u64()?),
            f64::from_bits(self.u64()?),
        ])
    }
    pub(crate) fn count(&mut self, max: usize) -> Result<usize, FormatError> {
        let count = usize::try_from(self.u32()?)
            .map_err(|_| FormatError::MalformedIndex("writer checkpoint count"))?;
        if count > max {
            return Err(FormatError::MalformedIndex(
                "writer checkpoint count exceeds schema",
            ));
        }
        Ok(count)
    }
    pub(crate) fn values(
        &mut self,
        data_type: AttributeDataType,
        count: usize,
    ) -> Result<AttributeValues, FormatError> {
        let bytes = count
            .checked_mul(crate::layout::primitive_width(data_type) as usize)
            .ok_or(FormatError::MalformedIndex(
                "writer checkpoint range length overflow",
            ))?;
        let raw = self.take(bytes)?;
        macro_rules! parse {
            ($t:ty, $n:expr, $variant:ident) => {{
                let chunks = raw.chunks_exact($n);
                if !chunks.remainder().is_empty() {
                    return Err(FormatError::MalformedIndex("writer checkpoint range width"));
                }
                AttributeValues::$variant(
                    chunks
                        .map(|c| <$t>::from_le_bytes(c.try_into().expect("exact chunk")))
                        .collect(),
                )
            }};
        }
        Ok(match data_type {
            AttributeDataType::U8 => AttributeValues::U8(raw.to_vec()),
            AttributeDataType::I8 => AttributeValues::I8(raw.iter().map(|v| *v as i8).collect()),
            AttributeDataType::U16 => parse!(u16, 2, U16),
            AttributeDataType::I16 => parse!(i16, 2, I16),
            AttributeDataType::U32 => parse!(u32, 4, U32),
            AttributeDataType::I32 => parse!(i32, 4, I32),
            AttributeDataType::U64 => parse!(u64, 8, U64),
            AttributeDataType::I64 => parse!(i64, 8, I64),
            AttributeDataType::F32 => AttributeValues::F32(
                raw.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|chunk| f32::from_bits(u32::from_le_bytes(*chunk)))
                    .collect(),
            ),
            AttributeDataType::F64 => AttributeValues::F64(
                raw.as_chunks::<8>()
                    .0
                    .iter()
                    .map(|chunk| f64::from_bits(u64::from_le_bytes(*chunk)))
                    .collect(),
            ),
        })
    }
}
