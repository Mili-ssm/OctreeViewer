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
    // (planes, compressed) scratch; lengths persist so steady state never re-zeroes.
    static SCRATCH: RefCell<(Vec<u8>, Vec<u8>)> = const { RefCell::new((Vec::new(), Vec::new())) };
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

fn grow(buffer: &mut Vec<u8>, len: usize) {
    if buffer.len() < len {
        buffer.resize(len, 0);
    }
}

/// Copies between record-interleaved and plane order; `src` and `dst` have the same length.
fn transpose(src: &[u8], dst: &mut [u8], record_bytes: usize, to_planes: bool) {
    let count = src.len() / record_bytes;
    for record in 0..count {
        for plane in 0..record_bytes {
            let interleaved = record * record_bytes + plane;
            let planar = plane * count + record;
            if to_planes {
                dst[planar] = src[interleaved];
            } else {
                dst[interleaved] = src[planar];
            }
        }
    }
}

/// Appends header and payload to `out`. `records.len()` must be a multiple of `record_bytes` and at
/// most `MAX_FRAME_RECORDS` records. Allocates only by growing `out` (plus a per-thread scratch).
pub(crate) fn encode_frame(records: &[u8], record_bytes: usize, out: &mut Vec<u8>) {
    assert!(record_bytes > 0 && records.len().is_multiple_of(record_bytes));
    assert!(records.len() / record_bytes <= MAX_FRAME_RECORDS);
    let logical_len = u32::try_from(records.len()).expect("frame fits u32 by the record limit");
    let max = lz4_flex::block::get_maximum_output_size(records.len());
    let header_at = out.len();
    out.extend_from_slice(&[0; FRAME_HEADER_BYTES]);
    let (stored, flag) = SCRATCH.with_borrow_mut(|(planes, compressed)| {
        grow(planes, records.len());
        grow(compressed, max);
        let planes = &mut planes[..records.len()];
        transpose(records, planes, record_bytes, true);
        let written = lz4_flex::block::compress_into(planes, compressed)
            .expect("output sized with get_maximum_output_size");
        if written < records.len() {
            out.extend_from_slice(&compressed[..written]);
            (written, 0)
        } else {
            out.extend_from_slice(records);
            (records.len(), RAW_FLAG)
        }
    });
    let stored = u32::try_from(stored).expect("stored length fits u32") | flag;
    out[header_at..header_at + 4].copy_from_slice(&logical_len.to_le_bytes());
    out[header_at + 4..header_at + FRAME_HEADER_BYTES].copy_from_slice(&stored.to_le_bytes());
}

/// Replaces `out` with the original records; on error `out` is left empty. The header is
/// untrusted: every length is validated.
pub(crate) fn decode_frame(
    header: FrameHeader,
    payload: &[u8],
    record_bytes: usize,
    out: &mut Vec<u8>,
) -> io::Result<()> {
    out.clear();
    let result = decode_into(header, payload, record_bytes, out);
    if result.is_err() {
        out.clear();
    }
    result
}

fn decode_into(
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
    if header.raw {
        if payload.len() != logical {
            return Err(invalid("raw spool frame length mismatch"));
        }
        out.extend_from_slice(payload);
        return Ok(());
    }
    SCRATCH.with_borrow_mut(|(planes, _)| {
        grow(planes, logical);
        let planes = &mut planes[..logical];
        let written = lz4_flex::block::decompress_into(payload, planes)
            .map_err(|_| invalid("corrupt compressed spool frame"))?;
        if written != logical {
            return Err(invalid("spool frame decompressed size mismatch"));
        }
        out.resize(logical, 0);
        transpose(planes, out, record_bytes, false);
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
                let header = FrameHeader::parse(frame[..8].try_into().unwrap());
                // Never larger than raw; raw frames are exactly header + records.
                assert!(frame.len() <= FRAME_HEADER_BYTES + records.len());
                if header.raw {
                    assert_eq!(frame.len(), FRAME_HEADER_BYTES + records.len());
                }
                if count > 1 {
                    assert!(
                        !header.raw && frame.len() < records.len(),
                        "{record_bytes} x {count}"
                    );
                }
            }
        }
    }

    #[test]
    fn compressed_payload_holds_byte_planes() {
        for record_bytes in [42, 54] {
            let records = structured(780, record_bytes);
            let mut frame = Vec::new();
            encode_frame(&records, record_bytes, &mut frame);
            let mut expected = Vec::new();
            for plane in 0..record_bytes {
                expected.extend(
                    records
                        .chunks_exact(record_bytes)
                        .map(|record| record[plane]),
                );
            }
            let mut planes = vec![0; records.len()];
            let written =
                lz4_flex::block::decompress_into(&frame[FRAME_HEADER_BYTES..], &mut planes)
                    .unwrap();
            assert_eq!(written, records.len());
            assert_eq!(planes, expected);
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
    fn corrupt_frames_are_invalid_data_and_leave_out_empty() {
        let records = structured(780, 42);
        let mut frame = Vec::new();
        encode_frame(&records, 42, &mut frame);
        let good = FrameHeader::parse(frame[..8].try_into().unwrap());
        assert!(!good.raw);
        let payload = &frame[FRAME_HEADER_BYTES..];
        let truncated = &payload[..payload.len() - 5];
        let len = |bytes: &[u8]| u32::try_from(bytes.len()).unwrap();
        let wrong_size_block = lz4_flex::block::compress(&records[..42 * 10]);
        let max_len = u32::try_from(MAX_FRAME_RECORDS * 42 + 42).unwrap();
        let cases: Vec<(&str, FrameHeader, &[u8], usize)> = vec![
            ("zero record_bytes", good, payload, 0),
            (
                "too many records",
                FrameHeader {
                    logical_len: max_len,
                    ..good
                },
                payload,
                42,
            ),
            (
                "not a multiple",
                FrameHeader {
                    logical_len: good.logical_len - 1,
                    ..good
                },
                payload,
                42,
            ),
            (
                "raw length differs",
                FrameHeader { raw: true, ..good },
                payload,
                42,
            ),
            (
                "stored_len differs",
                FrameHeader {
                    stored_len: good.stored_len + 1,
                    ..good
                },
                payload,
                42,
            ),
            (
                "truncated payload",
                FrameHeader {
                    stored_len: len(truncated),
                    ..good
                },
                truncated,
                42,
            ),
            (
                "valid block of wrong size",
                FrameHeader {
                    stored_len: len(&wrong_size_block),
                    ..good
                },
                &wrong_size_block,
                42,
            ),
        ];
        for (name, header, payload, record_bytes) in cases {
            let mut out = vec![1, 2, 3];
            let error = decode_frame(header, payload, record_bytes, &mut out).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{name}");
            assert!(out.is_empty(), "{name}");
        }
    }
}
