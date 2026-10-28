//! Experimental streams, NOT a supported .octattrs format. See docs/records/benchmarks/ATTRIBUTE_DICTIONARY_BENCH.md.
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    time::Instant,
};
use viewer_core::AttributeValues;
use viewer_format::{BlockCodec, BlockPayloadKind, FormatReader, compress_block, decompress_block};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const HEADER: usize = 32;

fn values_bytes(values: &AttributeValues) -> Vec<u8> {
    let mut out = Vec::new();
    macro_rules! encode {
        ($values:expr) => {
            for value in $values {
                out.extend_from_slice(&value.to_le_bytes());
            }
        };
    }
    match values {
        AttributeValues::U8(v) => out.extend_from_slice(v),
        AttributeValues::I8(v) => encode!(v),
        AttributeValues::U16(v) => encode!(v),
        AttributeValues::I16(v) => encode!(v),
        AttributeValues::U32(v) => encode!(v),
        AttributeValues::I32(v) => encode!(v),
        AttributeValues::U64(v) => encode!(v),
        AttributeValues::I64(v) => encode!(v),
        AttributeValues::F32(v) => encode!(v),
        AttributeValues::F64(v) => encode!(v),
    }
    out
}

// Header: mode/codecs/bits (4 bytes), then seven LE u32s:
// rows, tuple width, cardinality, raw table/data lengths, stored table/data lengths.
fn encode(raw: &[u8], width: usize, dictionary: bool) -> Vec<u8> {
    assert!(width > 0 && raw.len().is_multiple_of(width));
    let rows = raw.len() / width;
    let (table, indices, cardinality, bits) = if dictionary {
        let mut lookup = HashMap::<&[u8], u32>::new();
        let mut table = Vec::new();
        let ids: Vec<u32> = raw
            .chunks_exact(width)
            .map(|row| {
                let next = lookup.len() as u32;
                *lookup.entry(row).or_insert_with(|| {
                    table.extend_from_slice(row);
                    next
                })
            })
            .collect();
        let cardinality = lookup.len();
        let bits = 32 - (cardinality as u32).saturating_sub(1).leading_zeros();
        let mut indices = vec![0; (rows * bits as usize).div_ceil(8)];
        for (row, id) in ids.into_iter().enumerate() {
            for bit in 0..bits as usize {
                let offset = row * bits as usize + bit;
                indices[offset / 8] |= (((id >> bit) & 1) as u8) << (offset % 8);
            }
        }
        (table, indices, cardinality, bits)
    } else {
        (raw.to_vec(), Vec::new(), 0, 0)
    };
    let first = compress_block(&table);
    let second = compress_block(&indices);
    let mut output = vec![
        u8::from(dictionary),
        first.codec() as u8,
        second.codec() as u8,
        bits as u8,
    ];
    for value in [
        rows,
        width,
        cardinality,
        table.len(),
        indices.len(),
        first.bytes().len(),
        second.bytes().len(),
    ] {
        output.extend_from_slice(
            &u32::try_from(value)
                .expect("experimental stream exceeds u32")
                .to_le_bytes(),
        );
    }
    output.extend_from_slice(first.bytes());
    output.extend_from_slice(second.bytes());
    output
}

fn field(bytes: &[u8], offset: usize) -> usize {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize
}

fn decode(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() < HEADER {
        return Err("short header".into());
    }
    let codec = |byte| match byte {
        0 => Ok(BlockCodec::None),
        1 => Ok(BlockCodec::Lz4),
        _ => Err("unknown codec"),
    };
    let rows = field(bytes, 4);
    let width = field(bytes, 8);
    let unique = field(bytes, 12);
    let raw_first = field(bytes, 16);
    let raw_second = field(bytes, 20);
    let first_len = field(bytes, 24);
    let second_len = field(bytes, 28);
    if bytes.len() != HEADER + first_len + second_len || width == 0 {
        return Err("invalid stream length".into());
    }
    let first = decompress_block(
        codec(bytes[1])?,
        &bytes[HEADER..HEADER + first_len],
        raw_first,
        raw_first,
    )?;
    let second = decompress_block(
        codec(bytes[2])?,
        &bytes[HEADER + first_len..],
        raw_second,
        raw_second,
    )?;
    if bytes[0] == 0 {
        if first.len() != rows * width || !second.is_empty() {
            return Err("invalid raw stream".into());
        }
        return Ok(first);
    }
    let bits = usize::from(bytes[3]);
    if bytes[0] != 1
        || bits > 32
        || first.len() != unique * width
        || second.len() != (rows * bits).div_ceil(8)
    {
        return Err("invalid dictionary stream".into());
    }
    let mut output = Vec::with_capacity(rows * width);
    for row in 0..rows {
        let mut id = 0_usize;
        for bit in 0..bits {
            let offset = row * bits + bit;
            id |= usize::from((second[offset / 8] >> (offset % 8)) & 1) << bit;
        }
        if id >= unique {
            return Err("invalid dictionary id".into());
        }
        output.extend_from_slice(&first[id * width..(id + 1) * width]);
    }
    Ok(output)
}

fn measure(
    raw: &[u8],
    width: usize,
    mode: usize,
    repetitions: usize,
    file: &mut File,
) -> Result<(usize, usize, f64, f64)> {
    let mut encode_ms = 0.;
    let mut bytes = Vec::new();
    for _ in 0..repetitions {
        let start = Instant::now();
        bytes = if mode == 0 {
            encode(raw, width, false)
        } else {
            let dictionary = encode(raw, width, true);
            if mode == 1 {
                let baseline = encode(raw, width, false);
                if dictionary.len() < baseline.len() {
                    dictionary
                } else {
                    baseline
                }
            } else {
                dictionary
            }
        };
        encode_ms += start.elapsed().as_secs_f64() * 1000.;
    }
    let position = file.stream_position()?;
    file.write_all(&bytes)?;
    file.flush()?;
    file.seek(SeekFrom::Start(position))?;
    let mut persisted = vec![0; bytes.len()];
    file.read_exact(&mut persisted)?;
    assert_eq!(persisted, bytes);
    let mut decode_ms = 0.;
    for _ in 0..repetitions {
        let start = Instant::now();
        let restored = decode(&persisted)?;
        decode_ms += start.elapsed().as_secs_f64() * 1000.;
        assert_eq!(restored, raw, "attribute bits or row order changed");
    }
    Ok((
        bytes.len(),
        field(&bytes, 12),
        encode_ms / repetitions as f64,
        decode_ms / repetitions as f64,
    ))
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let input = PathBuf::from(args.next().ok_or(
        "usage: attribute_dictionary_bench INPUT.oct NEW_OUTPUT_DIR [MAX_BLOCKS [REPETITIONS]]",
    )?);
    let output = PathBuf::from(args.next().ok_or("missing output directory")?);
    let max_blocks: usize = args
        .next()
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(usize::MAX);
    let repetitions: usize = args.next().map(|v| v.parse()).transpose()?.unwrap_or(3);
    if max_blocks == 0 || repetitions == 0 || args.next().is_some() {
        return Err("positive limits required; no extra arguments".into());
    }
    fs::create_dir(&output)?;
    let mut reader = FormatReader::new(File::open(&input)?)?;
    reader.attach_attributes(File::open(input.with_extension("octattrs"))?)?;
    let schema_count = reader
        .format_metadata()
        .dataset()
        .hierarchy()
        .attributes()
        .len();
    let blocks = reader.block_storage_stats()?;
    let payloads = reader.block_payload_storage_stats()?;
    let modes = ["baseline", "column_adaptive", "tuple_dictionary"];
    let mut files: Vec<File> = modes
        .iter()
        .map(|mode| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(output.join(format!("{mode}.bin")))
        })
        .collect::<std::io::Result<_>>()?;
    let mut totals = [0_usize; 3];
    let mut original = 0_u64;
    let mut total_rows = 0_usize;
    let mut encode_times = [0.; 3];
    let mut decode_times = [0.; 3];
    for (block_index, block) in blocks.iter().take(max_blocks).enumerate() {
        let rows = block.point_count as usize;
        if rows == 0 {
            return Err("empty block unsupported in experiment".into());
        }
        total_rows += rows;
        let colors = reader.read_display_colors(block.id)?;
        let mut columns = vec![
            colors
                .iter()
                .flat_map(|c| [c.r, c.g, c.b])
                .collect::<Vec<u8>>(),
        ];
        for index in 0..schema_count {
            columns.push(values_bytes(
                reader.read_attribute(block.id, index as u32)?.values(),
            ));
        }
        let source_bytes: u64 = payloads
            .iter()
            .filter(|p| p.id == block.id && p.kind != BlockPayloadKind::Geometry)
            .map(|p| p.stored_bytes)
            .sum();
        original += source_bytes;
        let widths: Vec<usize> = columns.iter().map(|c| c.len() / rows).collect();
        for (column, raw) in columns.iter().enumerate() {
            for mode in 0..2 {
                let (bytes, unique, enc, dec) =
                    measure(raw, widths[column], mode, repetitions, &mut files[mode])?;
                totals[mode] += bytes;
                encode_times[mode] += enc;
                decode_times[mode] += dec;
                println!(
                    "{{\"block\":{block_index},\"column\":{column},\"mode\":\"{}\",\"rows\":{rows},\"width\":{},\"bytes\":{bytes},\"unique\":{unique},\"encode_ms\":{enc:.6},\"decode_ms\":{dec:.6}}}",
                    modes[mode], widths[column]
                );
            }
        }
        assert_eq!(
            totals[0] as u64 - ((block_index + 1) * columns.len() * HEADER) as u64,
            original,
            "baseline must reproduce stored payload sizes"
        );
        let start = Instant::now();
        let width: usize = widths.iter().sum();
        let mut tuples = Vec::with_capacity(rows * width);
        for row in 0..rows {
            for (raw, width) in columns.iter().zip(&widths) {
                tuples.extend_from_slice(&raw[row * width..(row + 1) * width]);
            }
        }
        let transpose_ms = start.elapsed().as_secs_f64() * 1000.;
        let (bytes, unique, enc, dec) = measure(&tuples, width, 2, repetitions, &mut files[2])?;
        // Compare each original column independently to the reconstructed tuple stream, too.
        let restored = decode(&encode(&tuples, width, true))?;
        for row in 0..rows {
            let mut offset = row * width;
            for (raw, w) in columns.iter().zip(&widths) {
                assert_eq!(&restored[offset..offset + w], &raw[row * w..(row + 1) * w]);
                offset += w;
            }
        }
        totals[2] += bytes;
        encode_times[2] += enc;
        decode_times[2] += dec;
        println!(
            "{{\"block\":{block_index},\"mode\":\"tuple_dictionary\",\"rows\":{rows},\"width\":{width},\"bytes\":{bytes},\"unique\":{unique},\"transpose_ms\":{transpose_ms:.6},\"encode_ms\":{enc:.6},\"decode_ms\":{dec:.6}}}"
        );
    }
    for mode in 0..3 {
        assert_eq!(files[mode].metadata()?.len(), totals[mode] as u64);
        println!(
            "{{\"summary\":true,\"mode\":\"{}\",\"blocks\":{},\"rows\":{total_rows},\"repetitions\":{repetitions},\"original_payload_bytes\":{original},\"container_bytes\":{},\"encode_ms\":{:.6},\"decode_ms\":{:.6}}}",
            modes[mode],
            blocks.len().min(max_blocks),
            totals[mode],
            encode_times[mode],
            decode_times[mode]
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_roundtrip_cardinality_and_bit_boundaries() {
        for width in [1, 3, 8, 16] {
            for rows in [1, 7, 8, 9, 255, 256, 257] {
                for unique in [1, 2, 3, 17, rows] {
                    let raw: Vec<u8> = (0..rows)
                        .flat_map(|r| {
                            (0..width).map(move |b| ((r % unique * 37 + b * 11) % 256) as u8)
                        })
                        .collect();
                    for dictionary in [false, true] {
                        assert_eq!(decode(&encode(&raw, width, dictionary)).unwrap(), raw);
                    }
                }
            }
        }
        // Raw float bit patterns: distinct NaNs, signed zero, infinity, and >2^53 integer.
        let raw: Vec<u8> = [
            0x7ff8000000000001_u64,
            0x7ff8000000000002,
            0,
            1 << 63,
            0x7ff0000000000000,
            (1 << 53) + 1,
        ]
        .into_iter()
        .cycle()
        .take(19)
        .flat_map(u64::to_le_bytes)
        .collect();
        assert_eq!(decode(&encode(&raw, 8, true)).unwrap(), raw);
    }
    #[test]
    fn truncated_streams_and_bad_ids_are_rejected() {
        let bytes = encode(&[7; 9], 1, true);
        for size in 0..bytes.len() {
            assert!(decode(&bytes[..size]).is_err());
        }
        let mut invalid = bytes;
        invalid[12..16].copy_from_slice(&0_u32.to_le_bytes());
        assert!(decode(&invalid).is_err());
    }
}
