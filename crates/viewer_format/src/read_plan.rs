//! Validated, selective payload read plans and conservative logical memory estimates.
//!
//! Estimates cover payload buffers and materialized outputs only. They exclude index and
//! control structures, DFS metadata, strings, allocator overhead, runtime state and caches;
//! they are not RSS estimates or hard process-memory limits.

use viewer_core::{AttributeColumn, BlockId, ColorRgb, PointBatch};

use crate::{BlockCodec, FormatError, GeometryEncoding};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Which logical result a selective block read should materialize.
pub enum ReadSelection {
    Positions,
    Geometry,
    DisplayColors,
    Attribute { schema_index: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// File stream containing a payload range.
pub enum ReadSource {
    Primary,
    Attributes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// One stored payload range and its on-disk decoding metadata.
pub struct PayloadReadRange {
    source: ReadSource,
    offset: u64,
    stored_bytes: u64,
    decoded_bytes: u64,
    codec: BlockCodec,
    checksum: u32,
}

impl PayloadReadRange {
    pub(crate) fn new(
        source: ReadSource,
        offset: u64,
        stored_bytes: u64,
        decoded_bytes: u64,
        codec: BlockCodec,
        checksum: u32,
    ) -> Self {
        Self {
            source,
            offset,
            stored_bytes,
            decoded_bytes,
            codec,
            checksum,
        }
    }

    /// Source stream containing this payload.
    pub const fn source(&self) -> ReadSource {
        self.source
    }
    /// Byte offset in the source stream.
    pub const fn offset(&self) -> u64 {
        self.offset
    }
    /// Stored payload length, also counted as input traffic by an estimate.
    pub const fn stored_bytes(&self) -> u64 {
        self.stored_bytes
    }
    /// Raw payload length after codec decoding; excludes materialization scratch.
    pub const fn decoded_bytes(&self) -> u64 {
        self.decoded_bytes
    }
    /// Codec used to transform the stored payload.
    pub const fn codec(&self) -> BlockCodec {
        self.codec
    }
    /// Expected checksum for the stored payload.
    pub const fn checksum(&self) -> u32 {
        self.checksum
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Conservative logical buffers for one selected read, not an RSS or allocator estimate.
pub struct ReadMemoryEstimate {
    /// Total stored payload bytes read across the selection.
    stored_bytes: u64,
    /// Total raw payload bytes produced by codecs; excludes geometry scratch.
    decoded_bytes: u64,
    /// Materialized output retained for the caller.
    output_bytes: u64,
    /// Estimated maximum simultaneously live logical payload/output buffers. This is not the
    /// sum of the other fields and excludes metadata, allocator overhead, runtime state and caches.
    peak_bytes: u64,
}

impl ReadMemoryEstimate {
    /// Total stored payload traffic.
    pub const fn stored_bytes(&self) -> u64 {
        self.stored_bytes
    }
    /// Total raw payload traffic after codec decoding.
    pub const fn decoded_bytes(&self) -> u64 {
        self.decoded_bytes
    }
    /// Output retained for the caller after the read completes.
    pub const fn output_bytes(&self) -> u64 {
        self.output_bytes
    }
    /// Estimated simultaneous logical buffer peak; not RSS or a hard process limit.
    pub const fn peak_bytes(&self) -> u64 {
        self.peak_bytes
    }
}

/// Read decision and its ranges, with conservative logical memory accounting.
#[derive(Debug)]
pub struct ReadPlan {
    block_id: BlockId,
    selection: ReadSelection,
    ranges: Vec<PayloadReadRange>,
    memory: ReadMemoryEstimate,
}

impl ReadPlan {
    pub(crate) fn new(
        block_id: BlockId,
        selection: ReadSelection,
        ranges: Vec<PayloadReadRange>,
        memory: ReadMemoryEstimate,
    ) -> Self {
        Self {
            block_id,
            selection,
            ranges,
            memory,
        }
    }

    pub const fn block_id(&self) -> BlockId {
        self.block_id
    }
    pub const fn selection(&self) -> ReadSelection {
        self.selection
    }
    pub fn ranges(&self) -> &[PayloadReadRange] {
        &self.ranges
    }
    pub const fn memory(&self) -> ReadMemoryEstimate {
        self.memory
    }
}

/// Materialized result of a planned selective read.
#[derive(Debug)]
pub enum SelectedRead {
    Positions(PointBatch),
    Geometry(PointBatch),
    DisplayColors(Vec<ColorRgb>),
    Attribute(AttributeColumn),
}

fn checked_usize(value: u64) -> Result<(), FormatError> {
    usize::try_from(value)
        .map(|_| ())
        .map_err(|_| FormatError::SizeOutOfRange(value))
}

fn add(left: u64, right: u64) -> Result<u64, FormatError> {
    left.checked_add(right)
        .ok_or(FormatError::BlockSizeOverflow)
}

/// Estimate payload-buffer peaks. For Occupied/CompactOccupied geometry, materialization
/// conservatively adds 12 bytes per point and a presence bitset to raw payload and output.
pub(crate) fn estimate_payload(
    range: &PayloadReadRange,
    output_bytes: u64,
    geometry: Option<(GeometryEncoding, u64)>,
) -> Result<ReadMemoryEstimate, FormatError> {
    range
        .offset
        .checked_add(range.stored_bytes)
        .ok_or(FormatError::BlockSizeOverflow)?;
    checked_usize(range.stored_bytes)?;
    checked_usize(range.decoded_bytes)?;
    checked_usize(output_bytes)?;
    let mut materialization_scratch = 0;
    if let Some((encoding, point_count)) = geometry {
        let raw_bytes = point_count
            .checked_mul(12)
            .ok_or(FormatError::BlockSizeOverflow)?;
        checked_usize(raw_bytes)?;
        if matches!(
            encoding,
            GeometryEncoding::Occupied | GeometryEncoding::CompactOccupied
        ) {
            let bitset_bytes = point_count.div_ceil(8);
            checked_usize(bitset_bytes)?;
            materialization_scratch = add(raw_bytes, bitset_bytes)?;
        }
    }
    let decoded_bytes = range.decoded_bytes;
    let codec_peak = match range.codec {
        BlockCodec::None => range.stored_bytes,
        BlockCodec::Lz4 | BlockCodec::Constant => add(range.stored_bytes, decoded_bytes)?,
        BlockCodec::BytePlanes => add(
            range.stored_bytes,
            decoded_bytes
                .checked_mul(2)
                .ok_or(FormatError::BlockSizeOverflow)?,
        )?,
        BlockCodec::Dictionary => add(
            range.stored_bytes,
            decoded_bytes
                .checked_mul(3)
                .ok_or(FormatError::BlockSizeOverflow)?,
        )?,
    };
    let materialize_peak = add(add(decoded_bytes, output_bytes)?, materialization_scratch)?;
    Ok(ReadMemoryEstimate {
        stored_bytes: range.stored_bytes,
        decoded_bytes,
        output_bytes,
        peak_bytes: codec_peak.max(materialize_peak),
    })
}

/// Combine sequential geometry and display-color reads while accounting for retained positions.
pub(crate) fn combine_geometry(
    position: ReadMemoryEstimate,
    color: ReadMemoryEstimate,
) -> Result<ReadMemoryEstimate, FormatError> {
    Ok(ReadMemoryEstimate {
        stored_bytes: add(position.stored_bytes, color.stored_bytes)?,
        decoded_bytes: add(position.decoded_bytes, color.decoded_bytes)?,
        output_bytes: position.output_bytes,
        peak_bytes: position
            .peak_bytes
            .max(add(position.output_bytes, color.peak_bytes)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(codec: BlockCodec, stored: u64, decoded: u64) -> PayloadReadRange {
        PayloadReadRange::new(ReadSource::Primary, 10, stored, decoded, codec, 123)
    }

    #[test]
    fn estimates_codec_peaks_and_materialization() {
        for (codec, multiplier) in [
            (BlockCodec::None, 1),
            (BlockCodec::Lz4, 2),
            (BlockCodec::Constant, 2),
            (BlockCodec::BytePlanes, 3),
            (BlockCodec::Dictionary, 4),
        ] {
            let estimate = estimate_payload(&range(codec, 10, 20), 5, None).unwrap();
            assert_eq!(estimate.stored_bytes(), 10);
            assert_eq!(estimate.decoded_bytes(), 20);
            assert_eq!(estimate.output_bytes(), 5);
            assert_eq!(estimate.peak_bytes(), (10 + 20 * (multiplier - 1)).max(25));
        }
    }

    #[test]
    fn occupied_geometry_estimate_keeps_raw_length_and_counts_materialization_scratch() {
        let estimate = estimate_payload(
            &range(BlockCodec::None, 3, 4),
            24,
            Some((GeometryEncoding::Occupied, 9)),
        )
        .unwrap();
        assert_eq!(estimate.decoded_bytes(), 4);
        assert_eq!(estimate.peak_bytes(), 4 + 9 * 12 + 2 + 24);
    }

    #[test]
    fn combined_geometry_counts_sequential_reads_and_retained_positions() {
        let position = estimate_payload(&range(BlockCodec::Lz4, 10, 20), 30, None).unwrap();
        let color = estimate_payload(&range(BlockCodec::Lz4, 4, 8), 12, None).unwrap();
        let combined = combine_geometry(position, color).unwrap();
        assert_eq!(combined.stored_bytes(), 14);
        assert_eq!(combined.decoded_bytes(), 28);
        assert_eq!(combined.output_bytes(), 30);
        assert_eq!(combined.peak_bytes(), 30 + color.peak_bytes());
    }

    #[test]
    fn rejects_range_and_estimate_overflow() {
        assert!(
            estimate_payload(
                &PayloadReadRange::new(ReadSource::Primary, u64::MAX, 1, 1, BlockCodec::None, 0),
                0,
                None
            )
            .is_err()
        );
        assert!(
            estimate_payload(
                &range(BlockCodec::None, 1, u64::MAX),
                1,
                Some((GeometryEncoding::Occupied, 1))
            )
            .is_err()
        );
        let huge = ReadMemoryEstimate {
            stored_bytes: u64::MAX,
            decoded_bytes: 0,
            output_bytes: 0,
            peak_bytes: 0,
        };
        assert!(combine_geometry(huge, huge).is_err());
    }
}
