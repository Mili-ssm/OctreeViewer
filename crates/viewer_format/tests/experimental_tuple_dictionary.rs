use std::io::{Cursor, Read, Seek, SeekFrom};
use viewer_format::{
    crc32,
    experimental_tuple_dictionary::{encode, encode_independent, read_column},
};

#[test]
fn exact_columns_segments_and_bit_boundaries_roundtrip() {
    for rows in [0_usize, 1, 7, 8, 9, 255, 256, 257] {
        for unique in [1, 2, 3, 17, 257] {
            let a: Vec<u8> = (0..rows).map(|r| (r % unique) as u8).collect();
            let b: Vec<u8> = (0..rows)
                .flat_map(|r| ((r % unique) as u64).to_le_bytes())
                .collect();
            let columns = vec![(1, a), (8, b)];
            let counts = [(rows / 2) as u32, 0, (rows - rows / 2) as u32];
            for bytes in [
                encode(&counts, &columns).unwrap(),
                encode_independent(&counts, &columns).unwrap(),
            ] {
                for (c, (width, raw)) in columns.iter().enumerate() {
                    let projection = read_column(&mut Cursor::new(&bytes), c, 1024 * 1024).unwrap();
                    assert_eq!(projection.expand(), *raw);
                    assert_eq!(
                        projection.level_counts().collect::<Vec<_>>(),
                        counts.map(|n| n as usize)
                    );
                    assert_eq!(projection.record_width(), *width);
                    for (depth, &count) in counts.iter().enumerate() {
                        let start: usize = counts[..depth].iter().map(|&n| n as usize).sum();
                        for i in 0..count as usize {
                            assert_eq!(
                                projection.record(depth, i).unwrap(),
                                &raw[(start + i) * width..(start + i + 1) * width]
                            );
                        }
                        assert!(projection.record(depth, count as usize).is_none());
                    }
                    assert!(projection.record(usize::MAX, 0).is_none());
                }
            }
        }
    }
    let bits = [
        0x7ff8000000000001_u64,
        0x7ff8000000000002,
        0,
        1 << 63,
        u64::MAX,
        (1 << 53) + 1,
    ];
    let raw: Vec<_> = bits
        .into_iter()
        .cycle()
        .take(19)
        .flat_map(u64::to_le_bytes)
        .collect();
    let bytes = encode(&[19], &[(8, raw.clone())]).unwrap();
    assert_eq!(
        read_column(&mut Cursor::new(bytes), 0, 4096)
            .unwrap()
            .expand(),
        raw
    );
}

fn metadata_end(bytes: &[u8]) -> usize {
    u64::from_le_bytes(bytes[40..48].try_into().unwrap()) as usize
}
fn fix_metadata_crc(bytes: &mut [u8]) {
    let end = metadata_end(bytes);
    let crc = crc32(&bytes[..end - 4]);
    bytes[end - 4..end].copy_from_slice(&crc.to_le_bytes());
}
fn fix_ids_crc(bytes: &mut [u8]) {
    let start = metadata_end(bytes);
    let size = u64::from_le_bytes(bytes[48..56].try_into().unwrap()) as usize;
    let crc = crc32(&bytes[start..start + size]);
    bytes[65..69].copy_from_slice(&crc.to_le_bytes());
    fix_metadata_crc(bytes);
}

#[test]
fn truncated_corrupt_limits_and_repaired_crc_invalid_ids_are_rejected() {
    let bytes = encode(&[3], &[(1, vec![1, 2, 3])]).unwrap();
    for end in 0..bytes.len() {
        assert!(read_column(&mut Cursor::new(&bytes[..end]), 0, 4096).is_err());
    }
    for offset in 0..bytes.len() {
        let mut bad = bytes.clone();
        bad[offset] ^= 0x80;
        assert!(
            read_column(&mut Cursor::new(bad), 0, 4096).is_err(),
            "accepted mutation at {offset}"
        );
    }
    assert!(read_column(&mut Cursor::new(&bytes), 1, 4096).is_err());
    assert!(read_column(&mut Cursor::new(&bytes), 0, 32).is_err());
    let mut bad = bytes.clone();
    let start = metadata_end(&bad);
    bad[start] |= 3; // K=3, 2-bit IDs: 3 is invalid.
    fix_ids_crc(&mut bad);
    assert!(read_column(&mut Cursor::new(bad), 0, 4096).is_err());
    let mut bad = bytes.clone();
    bad[start] |= 0x80;
    fix_ids_crc(&mut bad);
    assert!(read_column(&mut Cursor::new(bad), 0, 4096).is_err());
    let mut bad = bytes.clone();
    bad[32..36].copy_from_slice(&4_u32.to_le_bytes());
    fix_metadata_crc(&mut bad);
    assert!(read_column(&mut Cursor::new(bad), 0, 4096).is_err());
    let mut bad = bytes.clone();
    bad[36..40].copy_from_slice(&u32::MAX.to_le_bytes());
    fix_metadata_crc(&mut bad);
    assert!(read_column(&mut Cursor::new(bad), 0, 4096).is_err());
    assert!(encode(&[2], &[(1, vec![1])]).is_err());
    assert!(encode(&[], &[]).is_err());
    assert!(encode(&[0], &[(0, vec![])]).is_err());
}

struct Tracked {
    inner: Cursor<Vec<u8>>,
    spans: Vec<(usize, usize)>,
}
impl Read for Tracked {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let start = self.inner.position() as usize;
        let n = self.inner.read(out)?;
        self.spans.push((start, start + n));
        Ok(n)
    }
}
impl Seek for Tracked {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[test]
fn selective_read_never_touches_other_dictionary_tables() {
    let columns = vec![
        (1, vec![3; 100]),
        (8, (0_u64..100).flat_map(u64::to_le_bytes).collect()),
    ];
    let mut bytes = encode(&[100], &columns).unwrap();
    // Header + level count + two widths; second table is directory entry two.
    let directory = 32 + 4 + 8;
    let other = directory + 2 * 29;
    let start = u64::from_le_bytes(bytes[other..other + 8].try_into().unwrap()) as usize;
    let length = u64::from_le_bytes(bytes[other + 8..other + 16].try_into().unwrap()) as usize;
    bytes[start] ^= 0xff; // Unrequested corruption must remain lazy.
    let mut reader = Tracked {
        inner: Cursor::new(bytes),
        spans: vec![],
    };
    let projection = read_column(&mut reader, 0, 10000).unwrap();
    assert_eq!(projection.expand(), columns[0].1);
    assert!(
        reader
            .spans
            .iter()
            .all(|&(a, b)| b <= start || a >= start + length)
    );
    assert!(read_column(&mut reader, 1, 10000).is_err());
}
