//! Byte-plane LZ4 frames for spool records.
//!
//! Records are fixed-size; storing byte `k` of every record contiguously (planes) lets LZ4 find the
//! long runs in coordinates and repeated attributes, which a record-interleaved layout hides.
//! A frame is `logical_len: u32 LE`, `stored_len: u32 LE` (bit 31 = payload stored raw), payload.

use std::cell::RefCell;
use std::io;

pub(crate) const FRAME_HEADER_BYTES: usize = 8;
pub(crate) const MAX_FRAME_RECORDS: usize = 8192;
const RAW_FLAG: u32 = 1 << 31;

thread_local! {
    static PLANES: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameHeader {
    pub logical_len: u32,
    pub stored_len: u32,
    pub raw: bool,
}

impl FrameHeader {
    pub(crate) fn parse(bytes: [u8; FRAME_HEADER_BYTES]) -> Self {
        let logical_len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let stored = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        Self {
            logical_len,
            stored_len: stored & !RAW_FLAG,
            raw: stored & RAW_FLAG != 0,
        }
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

/// Appends header and payload to `out`. `records.len()` must be a multiple of `record_bytes` and at
/// most `MAX_FRAME_RECORDS` records. Allocates only by growing `out` (plus a per-thread scratch).
pub(crate) fn encode_frame(records: &[u8], record_bytes: usize, out: &mut Vec<u8>) {
    assert!(record_bytes > 0 && records.len().is_multiple_of(record_bytes));
    assert!(records.len() / record_bytes <= MAX_FRAME_RECORDS);
    let logical_len = u32::try_from(records.len()).expect("frame fits u32 by the record limit");
    let start = out.len();
    let max = lz4_flex::block::get_maximum_output_size(records.len());
    out.resize(start + FRAME_HEADER_BYTES + max, 0);
    let compressed = PLANES.with_borrow_mut(|planes| {
        planes.clear();
        planes.resize(records.len(), 0);
        let count = records.len() / record_bytes;
        for (record, chunk) in records.chunks_exact(record_bytes).enumerate() {
            for (plane, &byte) in chunk.iter().enumerate() {
                planes[plane * count + record] = byte;
            }
        }
        lz4_flex::block::compress_into(planes, &mut out[start + FRAME_HEADER_BYTES..])
            .expect("output sized with get_maximum_output_size")
    });
    let (stored, flag) = if compressed < records.len() {
        out.truncate(start + FRAME_HEADER_BYTES + compressed);
        (compressed, 0)
    } else {
        out.truncate(start + FRAME_HEADER_BYTES);
        out.extend_from_slice(records);
        (records.len(), RAW_FLAG)
    };
    let stored = u32::try_from(stored).expect("stored length fits u32") | flag;
    out[start..start + 4].copy_from_slice(&logical_len.to_le_bytes());
    out[start + 4..start + FRAME_HEADER_BYTES].copy_from_slice(&stored.to_le_bytes());
}

/// Replaces `out` with the original records. The header is untrusted: every length is validated.
pub(crate) fn decode_frame(
    header: FrameHeader,
    payload: &[u8],
    record_bytes: usize,
    out: &mut Vec<u8>,
) -> io::Result<()> {
    let logical = header.logical_len as usize;
    if record_bytes == 0
        || !logical.is_multiple_of(record_bytes)
        || logical / record_bytes > MAX_FRAME_RECORDS
    {
        return Err(invalid("spool frame length is not a valid record multiple"));
    }
    if header.stored_len as usize != payload.len() {
        return Err(invalid("spool frame payload length mismatch"));
    }
    out.clear();
    if header.raw {
        if payload.len() != logical {
            return Err(invalid("raw spool frame length mismatch"));
        }
        out.extend_from_slice(payload);
        return Ok(());
    }
    let count = logical / record_bytes;
    out.resize(logical, 0);
    PLANES.with_borrow_mut(|planes| {
        planes.clear();
        planes.resize(logical, 0);
        let written = lz4_flex::block::decompress_into(payload, planes)
            .map_err(|_| invalid("corrupt compressed spool frame"))?;
        if written != logical {
            return Err(invalid("spool frame decompressed size mismatch"));
        }
        for (record, chunk) in out.chunks_exact_mut(record_bytes).enumerate() {
            for (plane, byte) in chunk.iter_mut().enumerate() {
                *byte = planes[plane * count + record];
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
    }

    fn structured(count: usize, record_bytes: usize) -> Vec<u8> {
        let mut rng = Lcg(7);
        let mut data = Vec::with_capacity(count * record_bytes);
        for index in 0..count {
            let mut record = vec![0xAB_u8; record_bytes];
            let coordinate = (index as u64 * 3 + rng.next() % 3).to_le_bytes();
            record[..8].copy_from_slice(&coordinate);
            record[8] = (rng.next() % 4) as u8;
            data.extend_from_slice(&record);
        }
        data
    }

    fn decode(frame: &[u8], record_bytes: usize) -> io::Result<Vec<u8>> {
        let header = FrameHeader::parse(frame[..FRAME_HEADER_BYTES].try_into().unwrap());
        let mut out = vec![1, 2, 3];
        decode_frame(header, &frame[FRAME_HEADER_BYTES..], record_bytes, &mut out)?;
        Ok(out)
    }

    #[test]
    fn frames_round_trip_for_spool_and_lod_records() {
        for record_bytes in [42, 54] {
            for count in [1, 780, MAX_FRAME_RECORDS] {
                let records = structured(count, record_bytes);
                let mut frame = vec![9];
                encode_frame(&records, record_bytes, &mut frame);
                let frame = &frame[1..];
                assert_eq!(decode(frame, record_bytes).unwrap(), records);
                if count > 1 {
                    assert!(frame.len() < records.len(), "{record_bytes} x {count}");
                }
                if count == 780 {
                    println!(
                        "ratio {record_bytes} B x {count}: {:.2}",
                        records.len() as f64 / frame.len() as f64
                    );
                }
            }
        }
    }

    #[test]
    fn incompressible_frames_are_stored_raw() {
        let mut rng = Lcg(42);
        let records: Vec<u8> = (0..42 * 100).map(|_| rng.next() as u8).collect();
        let mut out = Vec::new();
        encode_frame(&records, 42, &mut out);
        assert!(FrameHeader::parse(out[..8].try_into().unwrap()).raw);
        assert_eq!(out.len(), FRAME_HEADER_BYTES + records.len());
        assert_eq!(decode(&out, 42).unwrap(), records);
    }

    #[test]
    fn corrupt_payload_is_invalid_data() {
        let records = structured(780, 42);
        let mut frame = Vec::new();
        encode_frame(&records, 42, &mut frame);
        let header = FrameHeader::parse(frame[..8].try_into().unwrap());
        assert!(!header.raw);
        let payload = &frame[FRAME_HEADER_BYTES..frame.len() - 5];
        let mut out = Vec::new();
        let truncated = FrameHeader {
            stored_len: u32::try_from(payload.len()).unwrap(),
            ..header
        };
        for bad in [header, truncated] {
            let error = decode_frame(bad, payload, 42, &mut out).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
        let misaligned = FrameHeader {
            logical_len: header.logical_len - 1,
            ..header
        };
        let error = decode_frame(misaligned, &frame[8..], 42, &mut out).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
