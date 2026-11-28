use std::io::Cursor;

use super::*;

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
    bytes[104] = 0x80;
    bytes[107..111].copy_from_slice(&points.to_le_bytes());
    let mut vlr = vec![0u8; 54];
    vlr[2..16].copy_from_slice(b"laszip encoded");
    vlr[18..20].copy_from_slice(&22204u16.to_le_bytes());
    let mut record = vec![0u8; 40];
    record[0..2].copy_from_slice(&2u16.to_le_bytes());
    // One item whose size matches the 20 B record length of `las_header`.
    record[32..34].copy_from_slice(&1u16.to_le_bytes());
    record[34..40].copy_from_slice(&[6, 0, 20, 0, 2, 0]);
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
    // One byte item of 34 B, matching the record length above.
    record[32..34].copy_from_slice(&1u16.to_le_bytes());
    record[34..40].copy_from_slice(&[0, 0, 34, 0, 2, 0]);
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
fn point_data_end_offset_overflow_is_rejected_for_laz() {
    // Product fits in u64 but `offset_to_point_data + product` does not (las header.rs).
    let mut bytes = poc_laz(4, 375, 50_000, 0, u64::MAX / 34);
    bytes[104] = 0x80 | 6;
    assert!(matches!(
        check(&bytes),
        Err(LasPreflightError::PointDataOverflow { .. })
    ));
}

#[test]
fn point_data_end_offset_overflow_is_rejected_for_las() {
    let mut bytes = las_header(4, 375);
    bytes[96..100].copy_from_slice(&375u32.to_le_bytes());
    bytes[105..107].copy_from_slice(&30u16.to_le_bytes());
    bytes[247..255].copy_from_slice(&(u64::MAX / 30).to_le_bytes());
    assert!(matches!(
        check(&bytes),
        Err(LasPreflightError::PointDataOverflow { .. })
    ));
}

#[test]
fn uncompressed_file_with_stale_laszip_vlr_gets_the_uncompressed_bound() {
    // `las` decides by the point-format compression bit, not by the VLR.
    let mut bytes = poc_laz(2, 227, 50_000, 0, 0);
    bytes[104] = 3;
    check(&bytes).unwrap();
    bytes[107..111].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        check(&bytes),
        Err(LasPreflightError::PointDataExceedsFile { .. })
    ));
}

#[test]
fn compressed_bit_without_laszip_vlr_is_left_to_las() {
    // `las` fails cleanly with LasZipVlrNotFound; no uncompressed byte bound applies.
    let mut bytes = poc_laz(2, 227, 50_000, u32::MAX, 0);
    bytes[100..104].copy_from_slice(&0u32.to_le_bytes());
    bytes.drain(227..227 + 54 + 46);
    check(&bytes).unwrap();
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
    let vlr_and_record = head[375..375 + 54 + 46].to_vec();
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
