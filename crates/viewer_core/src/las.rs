//! Shared LAS-to-attribute contract; independent of any LAS parsing crate.

use std::io::{Read, Seek, SeekFrom};

use crate::{AttributeDataType, AttributeDescriptor, SpatialBounds};

/// Longest bounds axis is scaled to this many render units.
pub const TARGET_RENDER_EXTENT: f64 = 100.0;

/// Attribute schema stored for LAS sources, in on-disk column order.
///
/// Order is a contract: the seven base descriptors come first, then `gps_time`, `rgb` and
/// `nir` when enabled. Callers pair descriptors with per-point values positionally (see the
/// `debug_assert`s in `viewer_convert`'s `las_source.rs`), so the flags must match the values
/// they push.
pub fn las_attribute_schema(
    has_gps_time: bool,
    has_color: bool,
    has_nir: bool,
) -> Vec<AttributeDescriptor> {
    let descriptor = |name, data_type, components| {
        AttributeDescriptor::new(name, data_type, components)
            .expect("static attribute schema is valid")
    };
    let mut schema = vec![
        descriptor("intensity", AttributeDataType::U16, 1),
        descriptor("classification", AttributeDataType::U8, 1),
        descriptor("return_number", AttributeDataType::U8, 1),
        descriptor("number_of_returns", AttributeDataType::U8, 1),
        descriptor("scan_angle_degrees", AttributeDataType::F32, 1),
        descriptor("user_data", AttributeDataType::U8, 1),
        descriptor("point_source_id", AttributeDataType::U16, 1),
    ];
    if has_gps_time {
        schema.push(descriptor("gps_time", AttributeDataType::F64, 1));
    }
    if has_color {
        schema.push(descriptor("rgb", AttributeDataType::U16, 3));
    }
    if has_nir {
        schema.push(descriptor("nir", AttributeDataType::U16, 1));
    }
    schema
}

/// Visual scale for `bounds`: [`TARGET_RENDER_EXTENT`] divided by the longest axis extent.
///
/// Zero extent (a single point or empty bounds) yields `Some(1.0)`. Returns `None` when the
/// scale is not representable as a finite non-zero `f32`; use [`checked_visual_scale`] to also
/// get the rejected raw value.
pub fn visual_scale_for_bounds(bounds: SpatialBounds) -> Option<f64> {
    checked_visual_scale(bounds).ok()
}

/// Like [`visual_scale_for_bounds`], but `Err` carries the raw, unrepresentable scale.
pub fn checked_visual_scale(bounds: SpatialBounds) -> Result<f64, f64> {
    let min = bounds.min();
    let max = bounds.max();
    let extent = (0..3)
        .map(|axis| max[axis] - min[axis])
        .fold(0.0_f64, f64::max);
    if extent == 0.0 {
        return Ok(1.0);
    }
    let scale = TARGET_RENDER_EXTENT / extent;
    if !scale.is_finite() || scale <= 0.0 || !(scale as f32).is_finite() || (scale as f32) == 0.0 {
        return Err(scale);
    }
    Ok(scale)
}

/// Rejection reasons from [`preflight_las`]; each names the offending header field.
#[derive(Debug, thiserror::Error)]
pub enum LasPreflightError {
    #[error("LAS/LAZ input is truncated: {0}")]
    Truncated(&'static str),
    #[error("{0} extends past the end of the file")]
    VlrOutOfBounds(&'static str),
    #[error("EVLR record_length/start_of_first_evlr extends past the end of the file")]
    EvlrOutOfBounds,
    #[error(
        "LAZ chunk of {points} points x {point_size} B exceeds the {limit} B decompressed-chunk cap"
    )]
    ChunkTooLarge {
        points: u64,
        point_size: u64,
        limit: u64,
    },
    #[error("header point count x point_data_record_length ({points} x {record_len} B) overflows u64")]
    PointDataOverflow { points: u64, record_len: u64 },
    #[error(
        "header point count x point_data_record_length ({points} x {record_len} B) exceeds the {available} B after offset_to_point_data"
    )]
    PointDataExceedsFile {
        points: u64,
        record_len: u64,
        available: u64,
    },
    #[error("LAZ chunk table offset {0} lies outside the file")]
    ChunkTableOutOfBounds(u64),
    #[error("LAZ chunk table declares {chunks} chunks, more than the {limit} the file can hold")]
    ChunkCountExceedsPoints { chunks: u64, limit: u64 },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Cap on decompressed bytes per LAZ chunk: `laz` allocates this much from table
/// or VLR values. Real files use 50 000 points x <=67 B (~3.4 MB); COPC <=1 M x 67 B (~67 MB).
pub const MAX_LAZ_CHUNK_BYTES: u64 = 256 << 20;
const LAS_MIN_HEADER: u64 = 227;
const VLR_HEADER_LEN: u64 = 54;
const EVLR_HEADER_LEN: u64 = 60;
const LASZIP_RECORD_ID: u16 = 22204;
const LASZIP_VARIABLE_CHUNK: u32 = u32::MAX;

/// Rejects chunks whose decompressed size exceeds [`MAX_LAZ_CHUNK_BYTES`].
pub fn check_chunk_bytes(points: u64, point_size: u64) -> Result<(), LasPreflightError> {
    match points.checked_mul(point_size) {
        Some(bytes) if bytes <= MAX_LAZ_CHUNK_BYTES => Ok(()),
        _ => Err(LasPreflightError::ChunkTooLarge {
            points,
            point_size,
            limit: MAX_LAZ_CHUNK_BYTES,
        }),
    }
}

fn min_point_record_len(format_id: u8) -> u64 {
    match format_id {
        0 => 20,
        1 => 28,
        2 => 26,
        3 => 34,
        4 => 57,
        5 => 63,
        6 => 30,
        7 => 36,
        8 => 38,
        9 => 59,
        10 => 67,
        _ => 20,
    }
}

fn read_array<const N: usize, R: Read + Seek>(
    reader: &mut R,
    at: u64,
) -> Result<[u8; N], LasPreflightError> {
    reader.seek(SeekFrom::Start(at))?;
    let mut buf = [0u8; N];
    reader.read_exact(&mut buf).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            LasPreflightError::Truncated("header or record shorter than declared")
        } else {
            error.into()
        }
    })?;
    Ok(buf)
}

fn read_u16_at<R: Read + Seek>(reader: &mut R, at: u64) -> Result<u16, LasPreflightError> {
    Ok(u16::from_le_bytes(read_array(reader, at)?))
}

fn read_u32_at<R: Read + Seek>(reader: &mut R, at: u64) -> Result<u32, LasPreflightError> {
    Ok(u32::from_le_bytes(read_array(reader, at)?))
}

fn read_u64_at<R: Read + Seek>(reader: &mut R, at: u64) -> Result<u64, LasPreflightError> {
    Ok(u64::from_le_bytes(read_array(reader, at)?))
}

/// Where the LAZ chunk table lives and what the header says about its contents.
/// Raw header facts only; decoding the table needs the `laz` crate (see
/// `viewer_convert`), which bounds the decoded entries with these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LazChunkTableLayout {
    pub point_data_offset: u64,
    pub table_offset: u64,
    /// `None` for variable-size chunks.
    pub fixed_chunk_size: Option<u32>,
    /// Decompressed bytes per point as `laz` sizes buffers: the larger of the header
    /// record length and the laszip item sizes.
    pub point_size: u64,
    pub header_points: u64,
}

/// Bounds header-declared sizes by the real file length before the `las`/`laz`
/// crates allocate from them. Parses raw bytes; the reader is rewound to 0.
/// No `BufReader`: every access seeks, so a buffer would be discarded each time.
pub fn preflight_las<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
) -> Result<(), LasPreflightError> {
    preflight_las_layout(reader, file_len).map(|_| ())
}

/// Same checks as [`preflight_las`]; also returns the chunk table location for
/// LAZ inputs that carry one.
pub fn preflight_las_layout<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
) -> Result<Option<LazChunkTableLayout>, LasPreflightError> {
    let result = preflight_inner(reader, file_len);
    reader.seek(SeekFrom::Start(0))?;
    result
}

fn preflight_inner<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
) -> Result<Option<LazChunkTableLayout>, LasPreflightError> {
    if file_len < LAS_MIN_HEADER {
        return Err(LasPreflightError::Truncated(
            "file shorter than a LAS header",
        ));
    }
    if &read_array::<4, _>(reader, 0)? != b"LASF" {
        // Not LAS: the parser rejects it without header-driven allocations.
        return Ok(None);
    }
    let [_, minor] = read_array::<2, _>(reader, 24)?;
    let declared_header_size = u64::from(read_u16_at(reader, 94)?);
    if declared_header_size < LAS_MIN_HEADER || declared_header_size > file_len {
        return Err(LasPreflightError::Truncated("header_size"));
    }
    // `las` always reads the fixed header of the version; VLRs start after the larger one.
    let header_size = declared_header_size.max(match minor {
        0..=2 => 227,
        3 => 235,
        _ => 375,
    });
    if header_size > file_len {
        return Err(LasPreflightError::Truncated("version header"));
    }
    let point_data_offset = u64::from(read_u32_at(reader, 96)?);
    let vlr_count = read_u32_at(reader, 100)?;
    let format_id = read_array::<1, _>(reader, 104)?[0] & 0x3f;
    let record_len = u64::from(read_u16_at(reader, 105)?);
    let mut points = u64::from(read_u32_at(reader, 107)?);
    if minor >= 4 && points == 0 {
        // Same precedence as `las`: legacy count when non-zero.
        points = read_u64_at(reader, 247)?;
    }

    // `las` multiplies these unchecked (panics in debug builds).
    let point_bytes = points
        .checked_mul(record_len)
        .ok_or(LasPreflightError::PointDataOverflow { points, record_len })?;

    let mut laz_chunk_size = None;
    let mut laz_items_size = 0u64;
    let mut pos = header_size;
    for _ in 0..vlr_count {
        let end = pos + VLR_HEADER_LEN;
        if end > file_len {
            return Err(LasPreflightError::VlrOutOfBounds("VLR header"));
        }
        let user_id = read_array::<16, _>(reader, pos + 2)?;
        let record_id = read_u16_at(reader, pos + 18)?;
        let vlr_len = u64::from(read_u16_at(reader, pos + 20)?);
        if end + vlr_len > file_len {
            return Err(LasPreflightError::VlrOutOfBounds("VLR record_length"));
        }
        if laz_chunk_size.is_none()
            && record_id == LASZIP_RECORD_ID
            && user_id.starts_with(b"laszip encoded\0")
        {
            // laszip record: compressor u16, coder u16, version 4 B, options u32 (chunk_size at +12).
            if vlr_len < 16 {
                return Err(LasPreflightError::VlrOutOfBounds("laszip record"));
            }
            laz_chunk_size = Some(match read_u32_at(reader, end + 12)? {
                0 => LASZIP_VARIABLE_CHUNK,
                size => size,
            });
            // Items follow the 32-byte fixed part: count u16, then (type, size, version) u16 each.
            // A shorter record is rejected by `laz` itself.
            if vlr_len >= 34 {
                let count = u64::from(read_u16_at(reader, end + 32)?).min((vlr_len - 34) / 6);
                for item in 0..count {
                    laz_items_size += u64::from(read_u16_at(reader, end + 34 + item * 6 + 2)?);
                }
            }
        }
        pos = end + vlr_len;
    }

    if minor >= 4 {
        let first_evlr = read_u64_at(reader, 235)?;
        let evlr_count = read_u32_at(reader, 243)?;
        let mut pos = first_evlr;
        // `las` ignores EVLRs unless both fields are non-zero.
        let evlr_count = if first_evlr == 0 { 0 } else { evlr_count };
        for _ in 0..evlr_count {
            let end = pos
                .checked_add(EVLR_HEADER_LEN)
                .filter(|end| *end <= file_len)
                .ok_or(LasPreflightError::EvlrOutOfBounds)?;
            let len = read_u64_at(reader, pos + 20)?;
            pos = end
                .checked_add(len)
                .filter(|next| *next <= file_len)
                .ok_or(LasPreflightError::EvlrOutOfBounds)?;
        }
    }

    if let Some(chunk_size) = laz_chunk_size {
        let point_size = record_len.max(laz_items_size);
        if chunk_size != LASZIP_VARIABLE_CHUNK {
            check_chunk_bytes(u64::from(chunk_size), point_size)?;
        }
        let mut table = i64::from_le_bytes(read_array(reader, point_data_offset)?);
        if table <= point_data_offset as i64 {
            // Writer could not patch the offset; laz reads it from the file tail.
            table = i64::from_le_bytes(read_array(reader, file_len - 8)?);
            if table <= point_data_offset as i64 {
                return Ok(None);
            }
        }
        let table = table as u64;
        if table.checked_add(8).is_none_or(|end| end > file_len) {
            return Err(LasPreflightError::ChunkTableOutOfBounds(table));
        }
        let chunks = u64::from(read_u32_at(reader, table + 4)?);
        let by_points = if chunk_size == LASZIP_VARIABLE_CHUNK {
            points.min(file_len) + 1
        } else {
            points.div_ceil(u64::from(chunk_size)) + 1
        };
        // Every chunk starts with at least one uncompressed point record.
        let min_record = record_len.max(min_point_record_len(format_id)).max(1);
        let by_bytes = table
            .checked_sub(point_data_offset + 8)
            .map_or(0, |bytes| bytes / min_record);
        // A valid empty file may still carry one empty chunk.
        let by_bytes = if points == 0 {
            by_bytes.max(1)
        } else {
            by_bytes
        };
        let limit = by_points.min(by_bytes);
        if chunks > limit {
            return Err(LasPreflightError::ChunkCountExceedsPoints { chunks, limit });
        }
        return Ok(Some(LazChunkTableLayout {
            point_data_offset,
            table_offset: table,
            fixed_chunk_size: (chunk_size != LASZIP_VARIABLE_CHUNK).then_some(chunk_size),
            point_size,
            header_points: points,
        }));
    }
    // Uncompressed: records are stored verbatim. EVLRs are not subtracted, so a file
    // `las` can read is never rejected.
    let available = file_len.saturating_sub(point_data_offset);
    if point_bytes > available {
        return Err(LasPreflightError::PointDataExceedsFile {
            points,
            record_len,
            available,
        });
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_schema_has_seven_entries() {
        assert_eq!(las_attribute_schema(false, false, false).len(), 7);
    }

    #[test]
    fn full_schema_keeps_contract_order() {
        let schema = las_attribute_schema(true, true, true);
        let expected = [
            ("intensity", AttributeDataType::U16, 1),
            ("classification", AttributeDataType::U8, 1),
            ("return_number", AttributeDataType::U8, 1),
            ("number_of_returns", AttributeDataType::U8, 1),
            ("scan_angle_degrees", AttributeDataType::F32, 1),
            ("user_data", AttributeDataType::U8, 1),
            ("point_source_id", AttributeDataType::U16, 1),
            ("gps_time", AttributeDataType::F64, 1),
            ("rgb", AttributeDataType::U16, 3),
            ("nir", AttributeDataType::U16, 1),
        ];
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(name, data_type, components)| {
                AttributeDescriptor::new(name, data_type, components).unwrap()
            })
            .collect();
        assert_eq!(schema.len(), 10);
        assert_eq!(schema, expected);
    }

    #[test]
    fn visual_scale_matches_contract() {
        let bounds = SpatialBounds::new([10.0, -4.0, 0.0], [20.0, 16.0, 2.0]).unwrap();
        assert_eq!(visual_scale_for_bounds(bounds), Some(5.0));
        let zero = SpatialBounds::new([0.0; 3], [0.0; 3]).unwrap();
        assert_eq!(visual_scale_for_bounds(zero), Some(1.0));
        let tiny = SpatialBounds::new([0.0; 3], [f64::MIN_POSITIVE, 0.0, 0.0]).unwrap();
        assert_eq!(visual_scale_for_bounds(tiny), None);
    }

    // --- preflight_las ---

    use std::io::Cursor;

    fn las_header(minor: u8, header_size: u16) -> Vec<u8> {
        let mut h = vec![0u8; header_size as usize];
        h[..4].copy_from_slice(b"LASF");
        h[24] = 1;
        h[25] = minor;
        h[94..96].copy_from_slice(&header_size.to_le_bytes());
        h[96..100].copy_from_slice(&u32::from(header_size).to_le_bytes());
        h[104] = 0;
        h[105..107].copy_from_slice(&20u16.to_le_bytes());
        h
    }

    fn check(bytes: &[u8]) -> Result<(), LasPreflightError> {
        preflight_las(&mut Cursor::new(bytes), bytes.len() as u64)
    }

    #[test]
    fn sample_las_files_pass() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/samples");
        let mut seen = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "las" || e == "laz") {
                let mut file = std::fs::File::open(&path).unwrap();
                let len = file.metadata().unwrap().len();
                preflight_las(&mut file, len).unwrap();
                seen += 1;
            }
        }
        assert!(seen >= 2);
    }

    #[test]
    fn las_1_2_header_without_evlr_fields_passes() {
        let mut bytes = las_header(2, 227);
        bytes[107..111].copy_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 60]);
        check(&bytes).unwrap();
    }

    #[test]
    fn point_data_product_overflow_is_rejected() {
        let mut bytes = las_header(4, 375);
        bytes[105..107].copy_from_slice(&u16::MAX.to_le_bytes());
        bytes[247..255].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(
            check(&bytes),
            Err(LasPreflightError::PointDataOverflow { .. })
        ));
    }

    #[test]
    fn uncompressed_las_declaring_more_points_than_bytes_is_rejected() {
        let mut bytes = las_header(2, 227);
        bytes[107..111].copy_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 60]); // 3 records of 20 B
        assert!(matches!(
            check(&bytes),
            Err(LasPreflightError::PointDataExceedsFile { .. })
        ));
        bytes.extend_from_slice(&[0u8; 20]);
        check(&bytes).unwrap();
    }

    #[test]
    fn preflight_restores_reader_position() {
        let bytes = las_header(2, 227);
        let mut cursor = Cursor::new(bytes.as_slice());
        cursor.set_position(5);
        preflight_las(&mut cursor, bytes.len() as u64).unwrap();
        assert_eq!(cursor.position(), 0);
    }

    #[test]
    fn short_header_is_truncated() {
        assert!(matches!(
            check(&las_header(2, 227)[..100]),
            Err(LasPreflightError::Truncated(_))
        ));
    }

    #[test]
    fn huge_evlr_length_is_rejected() {
        let mut bytes = las_header(4, 375);
        let evlr_start = bytes.len() as u64;
        bytes[235..243].copy_from_slice(&evlr_start.to_le_bytes());
        bytes[243..247].copy_from_slice(&1u32.to_le_bytes());
        let mut evlr = vec![0u8; 60];
        evlr[20..28].copy_from_slice(&u64::MAX.to_le_bytes());
        bytes.extend_from_slice(&evlr);
        assert!(matches!(
            check(&bytes),
            Err(LasPreflightError::EvlrOutOfBounds)
        ));
    }

    fn laz_file(number_of_chunks: u32, chunk_size: u32, points: u32) -> Vec<u8> {
        let mut bytes = las_header(4, 375);
        bytes[100..104].copy_from_slice(&1u32.to_le_bytes());
        bytes[107..111].copy_from_slice(&points.to_le_bytes());
        let mut vlr = vec![0u8; 54];
        vlr[2..16].copy_from_slice(b"laszip encoded");
        vlr[18..20].copy_from_slice(&22204u16.to_le_bytes());
        let mut record = vec![0u8; 34];
        record[0..2].copy_from_slice(&2u16.to_le_bytes());
        record[12..16].copy_from_slice(&chunk_size.to_le_bytes());
        record[16..24].copy_from_slice(&(-1i64).to_le_bytes());
        vlr[20..22].copy_from_slice(&(record.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&vlr);
        bytes.extend_from_slice(&record);
        let point_data = bytes.len() as u32;
        bytes[96..100].copy_from_slice(&point_data.to_le_bytes());
        // 2000 bytes of chunk data (100 records of 20 B) precede the table.
        let table_offset = u64::from(point_data) + 8 + 2000;
        bytes.extend_from_slice(&table_offset.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 2000]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&number_of_chunks.to_le_bytes());
        bytes
    }

    #[test]
    fn evlr_length_is_read_at_offset_20() {
        let mut bytes = las_header(4, 375);
        bytes[235..243].copy_from_slice(&375u64.to_le_bytes());
        bytes[243..247].copy_from_slice(&1u32.to_le_bytes());
        // Neighbouring fields are 0xFF: reading the length at @18/@22 would reject.
        let mut evlr = vec![0xFFu8; 60];
        evlr[20..28].copy_from_slice(&8u64.to_le_bytes());
        bytes.extend_from_slice(&evlr);
        bytes.extend_from_slice(&[0u8; 8]);
        check(&bytes).unwrap();
        bytes.pop();
        assert!(matches!(
            check(&bytes),
            Err(LasPreflightError::EvlrOutOfBounds)
        ));
    }

    /// Crafted LAZ with a laszip VLR (point format 3, 34 B records) and a tiny chunk table.
    fn poc_laz(minor: u8, header_total: usize, chunk_size: u32, legacy: u32, wide: u64) -> Vec<u8> {
        let mut b = vec![0u8; header_total];
        b[..4].copy_from_slice(b"LASF");
        b[24] = 1;
        b[25] = minor;
        b[94..96].copy_from_slice(&(header_total.min(375) as u16).to_le_bytes());
        b[100..104].copy_from_slice(&1u32.to_le_bytes());
        b[104] = 0x80 | 3;
        b[105..107].copy_from_slice(&34u16.to_le_bytes());
        b[107..111].copy_from_slice(&legacy.to_le_bytes());
        if header_total >= 255 {
            b[247..255].copy_from_slice(&wide.to_le_bytes());
        }
        let mut record = vec![0u8; 46];
        record[0..2].copy_from_slice(&2u16.to_le_bytes());
        record[12..16].copy_from_slice(&chunk_size.to_le_bytes());
        let mut vlr = vec![0u8; 54];
        vlr[2..16].copy_from_slice(b"laszip encoded");
        vlr[18..20].copy_from_slice(&22204u16.to_le_bytes());
        vlr[20..22].copy_from_slice(&(record.len() as u16).to_le_bytes());
        b.extend_from_slice(&vlr);
        b.extend_from_slice(&record);
        let point_data = b.len() as u32;
        b[96..100].copy_from_slice(&point_data.to_le_bytes());
        b.extend_from_slice(&(u64::from(point_data) + 8 + 16).to_le_bytes());
        b.extend_from_slice(&[0u8; 16]);
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        b.extend_from_slice(&[0u8; 64]);
        b
    }

    #[test]
    fn poc_a_las12_tiny_chunks_with_huge_count_is_rejected() {
        let bytes = poc_laz(2, 227, 1, u32::MAX, 0);
        assert!(matches!(
            check(&bytes),
            Err(LasPreflightError::ChunkCountExceedsPoints { .. })
        ));
    }

    #[test]
    fn poc_b_las14_huge_wide_point_count_is_rejected() {
        let bytes = poc_laz(4, 375, 50_000, 0, 1 << 58);
        assert!(matches!(
            check(&bytes),
            Err(LasPreflightError::ChunkCountExceedsPoints { .. })
        ));
    }

    #[test]
    fn poc_c_las13_with_short_header_size_still_finds_laszip_vlr() {
        let mut bytes = poc_laz(3, 235, 50_000, 100, 0);
        bytes[94..96].copy_from_slice(&227u16.to_le_bytes());
        assert!(matches!(
            check(&bytes),
            Err(LasPreflightError::ChunkCountExceedsPoints { .. })
        ));
    }

    #[test]
    fn laz_chunk_count_beyond_points_is_rejected() {
        assert!(matches!(
            check(&laz_file(u32::MAX, 50_000, 100)),
            Err(LasPreflightError::ChunkCountExceedsPoints { .. })
        ));
        assert!(matches!(
            check(&laz_file(u32::MAX, u32::MAX, 100)),
            Err(LasPreflightError::ChunkCountExceedsPoints { .. })
        ));
    }

    #[test]
    fn laz_with_plausible_chunk_count_passes() {
        check(&laz_file(2, 50_000, 100_000)).unwrap();
    }

    #[test]
    fn laz_chunk_table_offset_past_end_is_rejected() {
        let mut bytes = laz_file(1, 50_000, 100);
        let at = u32::from_le_bytes(bytes[96..100].try_into().unwrap()) as usize;
        bytes[at..at + 8].copy_from_slice(&i64::MAX.to_le_bytes());
        assert!(matches!(
            check(&bytes),
            Err(LasPreflightError::ChunkTableOutOfBounds(_))
        ));
    }

    /// Zero-filled virtual file with a few real byte ranges.
    struct SparseFile {
        parts: Vec<(u64, Vec<u8>)>,
        pos: u64,
    }

    impl std::io::Read for SparseFile {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            buf.fill(0);
            for (start, data) in &self.parts {
                for (i, byte) in buf.iter_mut().enumerate() {
                    let at = self.pos + i as u64;
                    if at >= *start && at < start + data.len() as u64 {
                        *byte = data[(at - start) as usize];
                    }
                }
            }
            self.pos += buf.len() as u64;
            Ok(buf.len())
        }
    }

    impl std::io::Seek for SparseFile {
        fn seek(&mut self, from: std::io::SeekFrom) -> std::io::Result<u64> {
            if let std::io::SeekFrom::Start(at) = from {
                self.pos = at;
            }
            Ok(self.pos)
        }
    }

    #[test]
    fn laz_1_2_legacy_point_count_with_table_beyond_u32_passes() {
        let mut head = laz_file(10_177, 50_000, 508_809_556);
        head[24..26].copy_from_slice(&[1, 2]);
        head[94..96].copy_from_slice(&227u16.to_le_bytes());
        // 1.2 header is 227 B: move VLR + record down, leaving bytes 227.. as VLR data.
        let vlr_and_record = head[375..375 + 54 + 34].to_vec();
        let mut bytes = head[..227].to_vec();
        bytes.extend_from_slice(&vlr_and_record);
        let point_data = bytes.len() as u64;
        bytes[96..100].copy_from_slice(&(point_data as u32).to_le_bytes());
        let table = 4_305_464_060u64;
        let mut table_bytes = 0u32.to_le_bytes().to_vec();
        table_bytes.extend_from_slice(&10_177u32.to_le_bytes());
        let len = table + 1_000;
        let mut file = SparseFile {
            parts: vec![
                (0, bytes),
                (point_data, table.to_le_bytes().to_vec()),
                (table, table_bytes),
            ],
            pos: 0,
        };
        preflight_las(&mut file, len).unwrap();
    }

    #[test]
    fn evlr_walk_is_skipped_when_start_of_first_evlr_is_zero() {
        // Bytes 24..28 of the header read as a huge EVLR length if the walk ran from offset 0.
        let mut bytes = las_header(4, 375);
        bytes[243..247].copy_from_slice(&1u32.to_le_bytes());
        check(&bytes).unwrap();
        // Same count with a real start offset past the end is still rejected.
        let past_end = bytes.len() as u64 + 1;
        bytes[235..243].copy_from_slice(&past_end.to_le_bytes());
        assert!(matches!(
            check(&bytes),
            Err(LasPreflightError::EvlrOutOfBounds)
        ));
    }

    /// `poc_laz` with an empty chunk table, so only the header point count is observable.
    fn header_points(minor: u8, legacy: u32, wide: u64) -> u64 {
        let mut bytes = poc_laz(
            minor,
            if minor >= 4 { 375 } else { 227 },
            50_000,
            legacy,
            wide,
        );
        let layout = preflight_las_layout(&mut Cursor::new(&bytes), bytes.len() as u64);
        // The crafted table claims `u32::MAX` chunks; zero them to reach the layout.
        assert!(layout.is_err());
        let point_data = u32::from_le_bytes(bytes[96..100].try_into().unwrap()) as usize;
        let chunks_at = point_data + 8 + 16 + 4;
        bytes[chunks_at..chunks_at + 4].copy_from_slice(&0u32.to_le_bytes());
        preflight_las_layout(&mut Cursor::new(&bytes), bytes.len() as u64)
            .unwrap()
            .unwrap()
            .header_points
    }

    #[test]
    fn las_1_4_legacy_point_count_wins_over_wide_count_when_non_zero() {
        assert_eq!(header_points(4, 100, 1 << 58), 100);
        assert_eq!(header_points(4, 0, 1 << 40), 1 << 40);
        // Before 1.4 the wide field does not exist and is ignored.
        assert_eq!(header_points(2, 100, 0), 100);
    }
}
