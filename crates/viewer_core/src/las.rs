//! Shared LAS-to-attribute contract; independent of any LAS parsing crate.

use std::io::{Read, Seek, SeekFrom};

use crate::{AttributeDataType, AttributeDescriptor, SpatialBounds};

/// Longest bounds axis is scaled to this many render units.
pub const TARGET_RENDER_EXTENT: f64 = 100.0;

/// Attribute schema stored for LAS sources, in on-disk column order.
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

/// Visual scale for `bounds`; `None` when it is not representable as a finite
/// non-zero `f32`.
pub fn visual_scale_for_bounds(bounds: SpatialBounds) -> Option<f64> {
    let min = bounds.min();
    let max = bounds.max();
    let extent = (0..3)
        .map(|axis| max[axis] - min[axis])
        .fold(0.0_f64, f64::max);
    if extent == 0.0 {
        return Some(1.0);
    }
    let scale = TARGET_RENDER_EXTENT / extent;
    if !scale.is_finite() || scale <= 0.0 || !(scale as f32).is_finite() || (scale as f32) == 0.0 {
        return None;
    }
    Some(scale)
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
    #[error("LAZ chunk table offset {0} lies outside the file")]
    ChunkTableOutOfBounds(u64),
    #[error("LAZ chunk table declares {chunks} chunks, more than the {limit} the file can hold")]
    ChunkCountExceedsPoints { chunks: u64, limit: u64 },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

const LAS_MIN_HEADER: u64 = 227;
const VLR_HEADER_LEN: u64 = 54;
const EVLR_HEADER_LEN: u64 = 60;
const LASZIP_RECORD_ID: u16 = 22204;
const LASZIP_VARIABLE_CHUNK: u32 = u32::MAX;

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

/// Bounds header-declared sizes by the real file length before the `las`/`laz`
/// crates allocate from them. Parses raw bytes; the reader is rewound to 0.
pub fn preflight_las<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
) -> Result<(), LasPreflightError> {
    let result = preflight_inner(reader, file_len);
    reader.seek(SeekFrom::Start(0))?;
    result
}

fn preflight_inner<R: Read + Seek>(reader: &mut R, file_len: u64) -> Result<(), LasPreflightError> {
    if file_len < LAS_MIN_HEADER {
        return Err(LasPreflightError::Truncated(
            "file shorter than a LAS header",
        ));
    }
    if &read_array::<4, _>(reader, 0)? != b"LASF" {
        // Not LAS: the parser rejects it without header-driven allocations.
        return Ok(());
    }
    let [_, minor] = read_array::<2, _>(reader, 24)?;
    let header_size = u64::from(read_u16_at(reader, 94)?);
    if header_size < LAS_MIN_HEADER || header_size > file_len {
        return Err(LasPreflightError::Truncated("header_size"));
    }
    let point_data_offset = u64::from(read_u32_at(reader, 96)?);
    let vlr_count = read_u32_at(reader, 100)?;
    let mut points = u64::from(read_u32_at(reader, 107)?);
    if minor >= 4 {
        if header_size < 375 {
            return Err(LasPreflightError::Truncated("LAS 1.4 header_size"));
        }
        let wide = read_u64_at(reader, 247)?;
        if wide != 0 {
            points = wide;
        }
    }

    let mut laz_chunk_size = None;
    let mut pos = header_size;
    for _ in 0..vlr_count {
        let end = pos + VLR_HEADER_LEN;
        if end > file_len {
            return Err(LasPreflightError::VlrOutOfBounds("VLR header"));
        }
        let user_id = read_array::<16, _>(reader, pos + 2)?;
        let record_id = read_u16_at(reader, pos + 18)?;
        let record_len = u64::from(read_u16_at(reader, pos + 20)?);
        if end + record_len > file_len {
            return Err(LasPreflightError::VlrOutOfBounds("VLR record_length"));
        }
        if record_id == LASZIP_RECORD_ID && user_id.starts_with(b"laszip encoded\0") {
            // laszip record: compressor u16, coder u16, version 4 B, options u32 (chunk_size at +12).
            if record_len < 16 {
                return Err(LasPreflightError::VlrOutOfBounds("laszip record"));
            }
            laz_chunk_size = Some(match read_u32_at(reader, end + 12)? {
                0 => LASZIP_VARIABLE_CHUNK,
                size => size,
            });
        }
        pos = end + record_len;
    }

    if minor >= 4 {
        let first_evlr = read_u64_at(reader, 235)?;
        let evlr_count = read_u32_at(reader, 243)?;
        let mut pos = first_evlr;
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
        let mut table = i64::from_le_bytes(read_array(reader, point_data_offset)?);
        if table <= point_data_offset as i64 {
            // Writer could not patch the offset; laz reads it from the file tail.
            table = i64::from_le_bytes(read_array(reader, file_len - 8)?);
            if table <= point_data_offset as i64 {
                return Ok(());
            }
        }
        let table = table as u64;
        if table.checked_add(8).is_none_or(|end| end > file_len) {
            return Err(LasPreflightError::ChunkTableOutOfBounds(table));
        }
        let chunks = u64::from(read_u32_at(reader, table + 4)?);
        let limit = if chunk_size == LASZIP_VARIABLE_CHUNK {
            points.min(file_len)
        } else {
            points.div_ceil(u64::from(chunk_size)) + 1
        };
        if chunks > limit {
            return Err(LasPreflightError::ChunkCountExceedsPoints { chunks, limit });
        }
    }
    Ok(())
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
        let table_offset = u64::from(point_data) + 8;
        bytes.extend_from_slice(&table_offset.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&number_of_chunks.to_le_bytes());
        bytes
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
        let at = bytes.len() - 16;
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
}
