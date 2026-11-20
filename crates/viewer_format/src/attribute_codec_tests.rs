use super::*;
use crate::compress_block;
use std::{collections::BTreeMap, fs::File, io::BufReader, time::Instant};

fn encode_without_wide_dictionary(raw: &[u8], width: usize) -> Result<EncodedBlock, FormatError> {
    if width <= 1 {
        return encode_pre4(raw, width, EntropyCoder::Lz4);
    }
    if width == 0 || !raw.len().is_multiple_of(width) {
        return Err(malformed());
    }
    let mut best = compress_block(raw);
    let rows = raw.len() / width;
    if rows < 2 || rows > u32::MAX as usize {
        return Ok(best);
    }
    if raw.chunks_exact(width).all(|row| row == &raw[..width]) && width < best.bytes().len() {
        return Ok(EncodedBlock {
            codec: BlockCodec::Constant,
            bytes: raw[..width].to_vec(),
        });
    }
    let mut planes = allocate(raw.len())?;
    planes.resize(raw.len(), 0);
    for byte in 0..width {
        for row in 0..rows {
            planes[byte * rows + row] = raw[row * width + byte];
        }
    }
    let encoded = compress_block(&planes);
    if encoded.codec() == BlockCodec::Lz4 && encoded.bytes().len() < best.bytes().len() {
        best = EncodedBlock {
            codec: BlockCodec::BytePlanes,
            bytes: encoded.into_bytes(),
        };
    }
    Ok(best)
}

fn values_as_le_bytes(values: &viewer_core::AttributeValues) -> Vec<u8> {
    use viewer_core::AttributeValues::*;
    let mut bytes = Vec::new();
    macro_rules! append {
        ($values:expr) => {
            for value in $values {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        };
    }
    match values {
        U8(values) => bytes.extend_from_slice(values),
        I8(values) => bytes.extend(values.iter().map(|value| *value as u8)),
        U16(values) => append!(values),
        I16(values) => append!(values),
        U32(values) => append!(values),
        I32(values) => append!(values),
        U64(values) => append!(values),
        I64(values) => append!(values),
        F32(values) => {
            for value in values {
                bytes.extend_from_slice(&value.to_bits().to_le_bytes());
            }
        }
        F64(values) => {
            for value in values {
                bytes.extend_from_slice(&value.to_bits().to_le_bytes());
            }
        }
    }
    bytes
}

#[test]
fn no_wide_dictionary_candidate_preserves_bytes_and_never_beats_exhaustive_encoder() {
    for width in [1, 2, 3, 6, 8, 12] {
        for unique in [0, 1, 3, 256] {
            let rows = if unique == 0 { 0 } else { 513 };
            let raw: Vec<u8> = (0..rows)
                .flat_map(|row| {
                    (0..width).map(move |byte| {
                        if unique == 1 {
                            0x5a
                        } else {
                            ((row % unique) * 29 + byte * 17) as u8
                        }
                    })
                })
                .collect();
            let baseline = encode_pre4(&raw, width, EntropyCoder::Lz4).unwrap();
            let candidate = encode_without_wide_dictionary(&raw, width).unwrap();
            assert!(
                candidate.bytes().len() >= baseline.bytes().len(),
                "width={width} unique={unique}"
            );
            assert_eq!(
                decode(
                    baseline.codec(),
                    baseline.bytes(),
                    raw.len(),
                    width,
                    raw.len()
                )
                .unwrap(),
                raw
            );
            assert_eq!(
                decode(
                    candidate.codec(),
                    candidate.bytes(),
                    raw.len(),
                    width,
                    raw.len()
                )
                .unwrap(),
                raw
            );
        }
    }
}

#[derive(Default)]
struct AttributeEncoderProfile {
    blocks: u64,
    rows: u64,
    bytes: [u64; 2],
    times: [[u128; 3]; 2],
    codecs: [BTreeMap<String, u64>; 2],
}

impl AttributeEncoderProfile {
    fn observe(&mut self, raw: &[u8], width: usize, stored: crate::PayloadReadRange) {
        let reference = [
            encode_pre4(raw, width, EntropyCoder::Lz4).unwrap(),
            encode_without_wide_dictionary(raw, width).unwrap(),
        ];
        assert_eq!(reference[0].codec(), stored.codec());
        assert_eq!(reference[0].bytes().len() as u64, stored.stored_bytes());
        assert_eq!(crate::crc32(reference[0].bytes()), stored.checksum());
        assert!(reference[1].bytes().len() >= reference[0].bytes().len());
        self.blocks += 1;
        self.rows += (raw.len() / width) as u64;
        for (variant, encoded) in reference.iter().enumerate() {
            self.bytes[variant] += encoded.bytes().len() as u64;
            *self.codecs[variant]
                .entry(format!("{:?}", encoded.codec()))
                .or_default() += 1;
            assert_eq!(
                decode(
                    encoded.codec(),
                    encoded.bytes(),
                    raw.len(),
                    width,
                    raw.len()
                )
                .unwrap(),
                raw
            );
        }
        // Index spans two arrays (`times[variant][repetition]`) and drives the run order; iterators would obscure that.
        #[expect(
            clippy::needless_range_loop,
            reason = "index spans two arrays and drives the run order"
        )]
        for repetition in 0..3 {
            let order = if repetition % 2 == 0 { [0, 1] } else { [1, 0] };
            for variant in order {
                let started = Instant::now();
                let encoded = if variant == 0 {
                    encode_pre4(raw, width, EntropyCoder::Lz4)
                } else {
                    encode_without_wide_dictionary(raw, width)
                }
                .unwrap();
                self.times[variant][repetition] += started.elapsed().as_nanos();
                assert_eq!(encoded.codec(), reference[variant].codec());
                assert_eq!(encoded.bytes(), reference[variant].bytes());
            }
        }
    }
}

#[test]
#[ignore = "profiles attribute encoders on an existing split .oct corpus"]
fn profile_attribute_encoders_on_split_corpus() {
    let path = std::path::PathBuf::from(
        std::env::var("VIEWER_ATTRIBUTE_PROFILE_PATH")
            .expect("set VIEWER_ATTRIBUTE_PROFILE_PATH to a split primary .oct file"),
    );
    let limit = std::env::var("VIEWER_ATTRIBUTE_PROFILE_BLOCKS")
        .ok()
        .map_or(128, |value| {
            value.parse::<usize>().expect("valid block limit")
        });
    assert!(limit > 0, "block limit must be positive");
    let mut reader = crate::FormatReader::new(BufReader::new(File::open(&path).unwrap())).unwrap();
    reader
        .attach_attributes(BufReader::new(
            File::open(path.with_extension("octattrs")).expect("split companion .octattrs"),
        ))
        .unwrap();
    let ids = reader.format_metadata().dataset().hierarchy().block_ids();
    let count = limit.min(ids.len());
    assert!(count > 0, "corpus contains no blocks");
    let selected: Vec<_> = (0..count)
        .map(|index| ids[index * (ids.len() - 1) / (count - 1).max(1)])
        .collect();
    println!(
        "attribute_profile_input total_blocks={} sampled_blocks={count}",
        ids.len()
    );
    let schema = reader
        .format_metadata()
        .dataset()
        .hierarchy()
        .attributes()
        .to_vec();
    let mut summaries: Vec<_> = (0..=schema.len())
        .map(|_| AttributeEncoderProfile::default())
        .collect();
    // Read and serialize one payload at a time; neither operation is timed.
    for id in selected {
        let colors = reader.read_display_colors(id).unwrap();
        let raw: Vec<_> = colors
            .iter()
            .flat_map(|color| [color.r, color.g, color.b])
            .collect();
        let plan = reader
            .plan_read(id, crate::ReadSelection::DisplayColors)
            .unwrap();
        summaries[0].observe(&raw, 3, plan.ranges()[0]);
        drop(raw);
        drop(colors);
        for (index, descriptor) in schema.iter().enumerate() {
            let column = reader.read_attribute(id, index as u32).unwrap();
            let raw = values_as_le_bytes(column.values());
            let width = crate::layout::attribute_tuple_width(descriptor).unwrap();
            let plan = reader
                .plan_read(
                    id,
                    crate::ReadSelection::Attribute {
                        schema_index: index as u32,
                    },
                )
                .unwrap();
            summaries[index + 1].observe(&raw, width, plan.ranges()[0]);
        }
    }
    for (index, summary) in summaries.iter().enumerate() {
        let (name, width) = if index == 0 {
            ("display_rgb", 3)
        } else {
            (
                schema[index - 1].name(),
                crate::layout::attribute_tuple_width(&schema[index - 1]).unwrap(),
            )
        };
        println!("attribute_profile_name column={index} name={name:?}");
        if index == 0 {
            println!("attribute_profile_schema column=0 primitive=U8 components=3");
        } else {
            println!(
                "attribute_profile_schema column={index} primitive={:?} components={}",
                schema[index - 1].data_type(),
                schema[index - 1].component_count()
            );
        }
        let mut baseline = summary.times[0];
        let mut candidate = summary.times[1];
        baseline.sort_unstable();
        candidate.sort_unstable();
        println!(
            "attribute_profile column={index} width={width} blocks={} rows={} baseline_bytes={} candidate_bytes={} baseline_time_ns={} candidate_time_ns={}",
            summary.blocks,
            summary.rows,
            summary.bytes[0],
            summary.bytes[1],
            baseline[1],
            candidate[1]
        );
        for (variant, times) in summary.times.iter().enumerate() {
            for (repetition, elapsed) in times.iter().enumerate() {
                println!(
                    "attribute_profile_time column={index} variant={variant} repetition={repetition} elapsed_ns={elapsed}"
                );
            }
        }
        for (variant, codecs) in summary.codecs.iter().enumerate() {
            for (codec, blocks) in codecs {
                println!(
                    "attribute_profile_codec column={index} variant={variant} codec={codec} blocks={blocks}"
                );
            }
        }
    }
}

#[test]
fn v73_dictionary_policy_matches_profiled_candidates_and_preserves_bits() {
    let mut state = 0x1234_5678_u32;
    for width in 1..=16 {
        for rows in [0, 1, 257, 4096] {
            for constant in [false, true] {
                let raw: Vec<_> = (0..rows * width)
                    .map(|_| {
                        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        if constant { 0x5a } else { (state >> 24) as u8 }
                    })
                    .collect();
                let expected = if matches!(width, 1 | 2 | 4) {
                    encode_pre4(&raw, width, EntropyCoder::Lz4)
                } else {
                    encode_without_wide_dictionary(&raw, width)
                }
                .unwrap();
                let actual = encode_v7_pre3(&raw, width, EntropyCoder::Lz4).unwrap();
                assert_eq!(
                    actual.codec(),
                    expected.codec(),
                    "width={width} rows={rows}"
                );
                assert_eq!(
                    actual.bytes(),
                    expected.bytes(),
                    "width={width} rows={rows}"
                );
                if !matches!(width, 1 | 2 | 4) {
                    assert_ne!(actual.codec(), BlockCodec::Dictionary);
                }
                assert_eq!(
                    decode(actual.codec(), actual.bytes(), raw.len(), width, raw.len()).unwrap(),
                    raw
                );
            }
        }
    }
    let bits = [0_u64, 1 << 63, 0x7ff8_0000_0000_0001, 0x7ff8_0000_0000_0042];
    let raw: Vec<_> = (0..4096)
        .flat_map(|i| bits[i % bits.len()].to_le_bytes())
        .collect();
    let actual = encode_v7_pre3(&raw, 8, EntropyCoder::Lz4).unwrap();
    assert_ne!(actual.codec(), BlockCodec::Dictionary);
    assert_eq!(
        decode(actual.codec(), actual.bytes(), raw.len(), 8, raw.len()).unwrap(),
        raw
    );
}

fn old_slice_encoder(raw: &[u8], width: usize) -> Result<EncodedBlock, FormatError> {
    if width == 0 || !raw.len().is_multiple_of(width) {
        return Err(malformed());
    }
    let mut best = compress_block(raw);
    let rows = raw.len() / width;
    if rows < 2 || rows > u32::MAX as usize {
        return Ok(best);
    }
    if raw.chunks_exact(width).all(|row| row == &raw[..width]) && width < best.bytes().len() {
        return Ok(EncodedBlock {
            codec: BlockCodec::Constant,
            bytes: raw[..width].to_vec(),
        });
    }
    let mut table = allocate(raw.len())?;
    let mut lookup = HashMap::<&[u8], u32>::new();
    lookup
        .try_reserve(rows)
        .map_err(|_| FormatError::AllocationFailed(rows))?;
    let mut ids = Vec::new();
    ids.try_reserve_exact(rows)
        .map_err(|_| FormatError::AllocationFailed(rows))?;
    for row in raw.chunks_exact(width) {
        let next = lookup.len() as u32;
        ids.push(*lookup.entry(row).or_insert_with(|| {
            table.extend_from_slice(row);
            next
        }));
    }
    let unique = table.len() / width;
    let bits = id_bits(unique);
    drop(lookup);
    let packed = pack_ids(ids.iter().copied(), bits)?;
    drop(ids);
    let compressed_table = compress_block(&table);
    drop(table);
    let compressed_packed = compress_block(&packed);
    drop(packed);
    let length = HEADER
        .checked_add(compressed_table.bytes().len())
        .and_then(|n| n.checked_add(compressed_packed.bytes().len()))
        .ok_or(FormatError::BlockSizeOverflow)?;
    if length < best.bytes().len() && u32::try_from(compressed_table.bytes().len()).is_ok() {
        let mut bytes = allocate(length)?;
        bytes.extend_from_slice(&(unique as u32).to_le_bytes());
        bytes.push(compressed_table.codec() as u8);
        bytes.push(compressed_packed.codec() as u8);
        bytes.extend_from_slice(&(compressed_table.bytes().len() as u32).to_le_bytes());
        bytes.extend_from_slice(compressed_table.bytes());
        bytes.extend_from_slice(compressed_packed.bytes());
        best = EncodedBlock {
            codec: BlockCodec::Dictionary,
            bytes,
        };
    }
    Ok(best)
}

#[test]
fn integer_dictionary_matches_old_slice_encoder_byte_for_byte() {
    let mut cases: Vec<(usize, Vec<u8>)> = Vec::new();
    for width in [2, 3, 4, 5, 6, 7, 8, 9, 16] {
        for rows in [0, 1, 2, 19, 257] {
            cases.push((
                width,
                (0..rows * width)
                    .map(|i| (i * 73 + width * 11) as u8)
                    .collect(),
            ));
        }
        cases.push((width, vec![0x5a; width * 257]));
        cases.push((
            width,
            (0..257)
                .flat_map(|i| (0..width).map(move |b| ((i % 3) * 31 + b) as u8))
                .collect(),
        ));
        cases.push((
            width,
            (0..257 * width)
                .map(|i| ((i * 131 + i / width * 17) >> 2) as u8)
                .collect(),
        ));
    }
    // Distinct IEEE encodings, including signed zeros and NaN payloads.
    let patterns = [
        0x0000_0000_0000_0000_u64,
        0x8000_0000_0000_0000,
        0x7ff8_0000_0000_0001,
        0x7ff8_0000_0000_0042,
    ];
    cases.push((
        8,
        (0..257)
            .flat_map(|i| patterns[i % patterns.len()].to_le_bytes())
            .collect(),
    ));
    let mut state = 0x1234_5678_u32;
    for width in [2, 3, 4, 6, 8, 12] {
        let data = (0..4096 * width)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        cases.push((width, data));
    }
    for (width, raw) in cases {
        let expected = old_slice_encoder(&raw, width).unwrap();
        let actual = encode(&raw, width, EntropyCoder::Lz4).unwrap();
        assert_eq!(
            actual.codec(),
            expected.codec(),
            "width={width}, rows={}",
            raw.len() / width
        );
        assert_eq!(
            actual.bytes(),
            expected.bytes(),
            "width={width}, rows={}",
            raw.len() / width
        );
    }
}

#[test]
#[ignore = "focused full attribute encoder A/B benchmark"]
fn integer_dictionary_full_encoder_benchmark() {
    use std::{hint::black_box, time::Instant};
    let rows = 262_144;
    for width in [2, 3, 6, 8] {
        for unique in [4, 256, if width == 2 { 65_536 } else { rows }] {
            let raw: Vec<u8> = (0..rows)
                .flat_map(|row| {
                    let value = ((row % unique) as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                    value.to_le_bytes()[..width].to_vec()
                })
                .collect();
            for repetition in 0..3 {
                for old in if repetition % 2 == 0 {
                    [true, false]
                } else {
                    [false, true]
                } {
                    let input = black_box(&raw);
                    let started = Instant::now();
                    let encoded = if old {
                        old_slice_encoder(input, width)
                    } else {
                        encode(input, width, EntropyCoder::Lz4)
                    }
                    .unwrap();
                    black_box((encoded.codec(), encoded.bytes()));
                    println!(
                        "width={width} unique={unique} rep={} old={old} elapsed_ms={:.3}",
                        repetition + 1,
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                }
            }
        }
    }
}

fn previous_byte_dictionary(raw: &[u8]) -> EncodedBlock {
    let mut best = compress_block(raw);
    if !raw.is_empty() && raw.iter().all(|value| *value == raw[0]) && 1 < best.bytes().len() {
        return EncodedBlock {
            codec: BlockCodec::Constant,
            bytes: vec![raw[0]],
        };
    }
    let mut lookup = HashMap::<u8, u32>::new();
    let mut table = Vec::new();
    let mut ids = Vec::with_capacity(raw.len());
    for &value in raw {
        let next = lookup.len() as u32;
        ids.push(*lookup.entry(value).or_insert_with(|| {
            table.push(value);
            next
        }));
    }
    let unique = table.len();
    let table = compress_block(&table);
    let packed = compress_block(&pack_ids(ids.iter().copied(), id_bits(unique)).unwrap());
    let length = HEADER + table.bytes().len() + packed.bytes().len();
    if length < best.bytes().len() && u32::try_from(table.bytes().len()).is_ok() {
        let mut bytes = Vec::with_capacity(length);
        bytes.extend_from_slice(&(unique as u32).to_le_bytes());
        bytes.push(table.codec() as u8);
        bytes.push(packed.codec() as u8);
        bytes.extend_from_slice(&(table.bytes().len() as u32).to_le_bytes());
        bytes.extend_from_slice(table.bytes());
        bytes.extend_from_slice(packed.bytes());
        best = EncodedBlock {
            codec: BlockCodec::Dictionary,
            bytes,
        };
    }
    best
}

#[test]
fn byte_dictionary_encoding_matches_previous_payload_for_all_cardinalities() {
    for unique in 1..=256 {
        for rows in [0, 1, 7, 257, 4096] {
            let raw: Vec<_> = (0..rows)
                .map(|row| ((unique - 1 - row % unique) * 73 % 256) as u8)
                .collect();
            let expected = previous_byte_dictionary(&raw);
            let actual = encode(&raw, 1, EntropyCoder::Lz4).unwrap();
            assert_eq!(
                actual.codec(),
                expected.codec(),
                "unique={unique} rows={rows}"
            );
            assert_eq!(
                actual.bytes(),
                expected.bytes(),
                "unique={unique} rows={rows}"
            );
            assert_eq!(
                decode(actual.codec(), actual.bytes(), raw.len(), 1, raw.len()).unwrap(),
                raw
            );
        }
    }
}
#[test]
#[ignore = "isolated dictionary construction benchmark; run with --profile perf"]
fn byte_dictionary_lookup_benchmark() {
    use std::{hint::black_box, time::Instant};
    let rows = 262_144;
    for unique in [2, 3, 256] {
        let raw: Vec<_> = (0..rows)
            .map(|row| (unique - 1 - row % unique) as u8)
            .collect();
        for repetition in 0..9 {
            for direct in if repetition % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let input = black_box(&raw);
                let started = Instant::now();
                let mut lookup = HashMap::<&[u8], u32>::new();
                if !direct {
                    lookup.try_reserve(rows).unwrap();
                }
                let mut table = allocate(if direct { 256 } else { rows }).unwrap();
                let mut ids = Vec::with_capacity(rows);
                if direct {
                    let lookup = byte_dictionary(input, &mut table);
                    ids.extend(
                        input
                            .iter()
                            .map(|value| u32::from(lookup[usize::from(*value)])),
                    );
                } else {
                    for row in input.chunks_exact(black_box(input.len() / rows)) {
                        let next = lookup.len() as u32;
                        ids.push(*lookup.entry(row).or_insert_with(|| {
                            table.extend_from_slice(row);
                            next
                        }));
                    }
                }
                black_box((&table, &ids));
                let elapsed = started.elapsed().as_secs_f64();
                assert_eq!(table.len(), unique);
                for (row, &id) in ids.iter().enumerate() {
                    assert_eq!(table[id as usize], raw[row]);
                }
                println!(
                    "unique={unique} rows={rows} rep={} direct={direct} elapsed_ms={:.6}",
                    repetition + 1,
                    elapsed * 1000.0
                );
            }
        }
    }
}

#[test]
fn byte_dictionary_matches_hashmap_palette_order_for_all_cardinalities() {
    for unique in 1..=256 {
        for rows in [0, 1, 7, 257, 4096] {
            // Reverse first occurrences so value order cannot substitute for ID order.
            let raw: Vec<_> = (0..rows)
                .map(|row| (unique - 1 - row % unique) as u8)
                .collect();
            let mut lookup = HashMap::new();
            let mut expected_table = Vec::new();
            let expected_ids: Vec<_> = raw
                .iter()
                .map(|&value| {
                    let next = lookup.len() as u32;
                    *lookup.entry(value).or_insert_with(|| {
                        expected_table.push(value);
                        next
                    })
                })
                .collect();
            let mut table = allocate(raw.len().min(256)).unwrap();
            let lookup = byte_dictionary(&raw, &mut table);
            let ids: Vec<_> = raw
                .iter()
                .map(|&value| u32::from(lookup[usize::from(value)]))
                .collect();
            assert_eq!(table, expected_table);
            assert_eq!(ids, expected_ids);
            assert!(table.capacity() <= 256);
            let encoded = encode_pre4(&raw, 1, EntropyCoder::Lz4).unwrap();
            assert_eq!(
                decode(encoded.codec(), encoded.bytes(), raw.len(), 1, raw.len()).unwrap(),
                raw
            );
        }
    }
}

#[test]
fn packed_ids_match_independent_bit_oracle_at_every_width() {
    for bits in 0_usize..=32 {
        for rows in [0, 1, 7, 8, 9, 257] {
            let mask = ((1_u64 << bits) - 1) as u32;
            let ids: Vec<_> = (0..rows)
                .map(|row| (row as u32).wrapping_mul(0x9e37_79b9) & mask)
                .collect();
            let mut oracle = vec![0; (rows * bits).div_ceil(8)];
            for (row, &id) in ids.iter().enumerate() {
                for bit in 0..bits {
                    let offset = row * bits + bit;
                    oracle[offset / 8] |= (((id >> bit) & 1) as u8) << (offset % 8);
                }
            }
            assert_eq!(pack_ids(ids.iter().copied(), bits).unwrap(), oracle);
            let mut reader = PackedIds {
                bytes: oracle.iter(),
                bits,
                buffer: 0,
                buffered: 0,
            };
            for id in ids {
                assert_eq!(reader.read().unwrap(), id as usize);
            }
            if !oracle.is_empty() {
                let mut truncated = PackedIds {
                    bytes: oracle[..oracle.len() - 1].iter(),
                    bits,
                    buffer: 0,
                    buffered: 0,
                };
                assert!((0..rows).any(|_| truncated.read().is_err()));
            }
        }
    }
}

#[test]
fn pre4_byte_planes_preserve_bits_and_never_exceed_pre3() {
    for width in [1, 3, 8, 16] {
        for rows in [0, 1, 9, 257, 4096] {
            let raw: Vec<_> = (0..rows)
                .flat_map(|r| {
                    (0..width).map(move |c| {
                        if c % 2 == 0 {
                            (r * 37 + c * 11) as u8
                        } else {
                            c as u8
                        }
                    })
                })
                .collect();
            let old = encode(&raw, width, EntropyCoder::Lz4).unwrap();
            let new = encode_pre4(&raw, width, EntropyCoder::Lz4).unwrap();
            assert!(new.bytes().len() <= old.bytes().len());
            assert_eq!(
                decode(new.codec(), new.bytes(), raw.len(), width, raw.len()).unwrap(),
                raw
            );
        }
    }
    let mut state = 1234_u32;
    let raw: Vec<_> = (0..4096)
        .flat_map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (0x7ff8_0000_0000_0000_u64 | u64::from(state)).to_le_bytes()
        })
        .collect();
    let new = encode_pre4(&raw, 8, EntropyCoder::Lz4).unwrap();
    assert_eq!(new.codec(), BlockCodec::BytePlanes);
    assert_eq!(
        decode(new.codec(), new.bytes(), raw.len(), 8, raw.len()).unwrap(),
        raw
    );
    assert!(decode(new.codec(), new.bytes(), raw.len(), 8, raw.len() - 1).is_err());
    assert!(decode(new.codec(), new.bytes(), raw.len(), 1, raw.len()).is_err());
    assert!(
        decode(
            new.codec(),
            &new.bytes()[..new.bytes().len() - 1],
            raw.len(),
            8,
            raw.len()
        )
        .is_err()
    );
}
#[test]
fn roundtrip_bits_components_and_fallback() {
    for width in [1, 3, 8, 16] {
        for rows in [0, 1, 7, 8, 9, 257, 4096] {
            for unique in [1, 3, 17, 257] {
                let raw: Vec<u8> = (0..rows)
                    .flat_map(|r| (0..width).map(move |c| ((r % unique * 37 + c * 11) % 256) as u8))
                    .collect();
                let encoded = encode(&raw, width, EntropyCoder::Lz4).unwrap();
                assert!(encoded.bytes().len() <= compress_block(&raw).bytes().len());
                assert_eq!(
                    decode(
                        encoded.codec(),
                        encoded.bytes(),
                        raw.len(),
                        width,
                        raw.len()
                    )
                    .unwrap(),
                    raw
                );
            }
        }
    }
    let constant = encode(&[255; 4096], 1, EntropyCoder::Lz4).unwrap();
    assert_eq!(constant.codec(), BlockCodec::Constant);
    let raw: Vec<u8> = (0..4096)
        .flat_map(|r| ((r * 37 % 17) as u64).to_le_bytes())
        .collect();
    assert_eq!(
        encode(&raw, 8, EntropyCoder::Lz4).unwrap().codec(),
        BlockCodec::Dictionary
    );
}
#[test]
fn rejects_invalid_shapes_nested_codecs_ids_padding_and_limits() {
    let bad = |bytes: &[u8], rows: usize| {
        decode(BlockCodec::Dictionary, bytes, rows * 8, 8, rows * 8).is_err()
    };
    // Three exact eight-byte entries, 9 two-bit IDs: last six padding bits must be zero.
    let mut stream = vec![3, 0, 0, 0, 0, 0, 24, 0, 0, 0];
    stream.extend_from_slice(&[0; 24]);
    stream.extend_from_slice(&[0, 0, 0]);
    assert!(!bad(&stream, 9));
    for size in 0..stream.len() {
        assert!(bad(&stream[..size], 9));
    }
    for (offset, value) in [(0, 0), (0, 10), (4, 2), (5, 3), (6, 255), (34, 3), (36, 4)] {
        let mut corrupt = stream.clone();
        corrupt[offset] = value;
        assert!(bad(&corrupt, 9));
    }
    assert!(decode(BlockCodec::Constant, &[0], 100, 1, 99).is_err());
    assert!(decode(BlockCodec::Constant, &[0, 1], 100, 1, 100).is_err());
}

mod zstd {
    use super::*;
    use crate::layout::{PayloadDirectoryEntry, StorageLayout, StoredColumn};
    use crate::{FormatVersion, index_codec::validate_column_codec};

    /// Column shapes that make each candidate win for some width.
    fn corpus() -> Vec<(usize, Vec<u8>)> {
        let mut state = 0x9E37_79B9_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let mut corpus = Vec::new();
        for width in [1_usize, 2, 3, 4, 8, 16] {
            let rows = 4096;
            // Few distinct tuples in random order: dictionary.
            let palette: Vec<Vec<u8>> = (0..13)
                .map(|_| (0..width).map(|_| next() as u8).collect())
                .collect();
            let mut dictionary = Vec::new();
            for _ in 0..rows {
                dictionary.extend_from_slice(&palette[next() as usize % 13]);
            }
            corpus.push((width, dictionary));
            // A slowly rising counter in the first bytes, noise-free high bytes: byte planes.
            let mut planes = Vec::new();
            for row in 0..rows {
                let value = (row as u64 * 3 + u64::from(next() % 3)).to_le_bytes();
                planes.extend((0..width).map(|byte| value.get(byte).copied().unwrap_or(0)));
            }
            corpus.push((width, planes));
            // Skewed noise with many distinct tuples: plain entropy coding.
            corpus.push((
                width,
                (0..rows * width).map(|_| (next() % 9) as u8).collect(),
            ));
            corpus.push((width, vec![7; rows * width]));
        }
        corpus
    }

    fn zstd_payloads() -> Vec<(usize, Vec<u8>, EncodedBlock)> {
        corpus()
            .into_iter()
            .map(|(width, raw)| {
                let encoded = encode_pre4(&raw, width, EntropyCoder::Zstd).unwrap();
                (width, raw, encoded)
            })
            .collect()
    }

    #[test]
    fn zstd_payload_round_trips() {
        let mut seen = std::collections::BTreeSet::new();
        for (width, raw, encoded) in zstd_payloads() {
            assert_ne!(encoded.codec(), BlockCodec::Lz4);
            assert_ne!(encoded.codec(), BlockCodec::BytePlanes);
            if encoded.codec() == BlockCodec::Dictionary {
                let tags = (encoded.bytes()[4], encoded.bytes()[5]);
                assert!(tags.0 != 1 && tags.1 != 1, "LZ4 tag in a zstd dictionary");
                if tags.0 == 2 || tags.1 == 2 {
                    seen.insert("dictionary with a zstd part".to_string());
                }
            } else {
                seen.insert(format!("{:?}", encoded.codec()));
            }
            assert_eq!(
                decode_with(
                    encoded.codec(),
                    encoded.bytes(),
                    raw.len(),
                    width,
                    raw.len(),
                    true
                )
                .unwrap(),
                raw,
                "width {width}"
            );
            // Deterministic.
            assert_eq!(
                encode_pre4(&raw, width, EntropyCoder::Zstd).unwrap(),
                encoded
            );
        }
        for expected in [
            "Zstd",
            "BytePlanesZstd",
            "Constant",
            "dictionary with a zstd part",
        ] {
            assert!(seen.contains(expected), "{expected} missing from {seen:?}");
        }
    }

    #[test]
    fn zstd_payload_round_trips_through_every_layout_that_allows_it() {
        for (width, raw, _) in zstd_payloads() {
            let mut bytes = Vec::new();
            let mut payloads = Vec::new();
            crate::prepare::append_payload(
                &raw,
                &mut bytes,
                &mut payloads,
                StorageLayout::V8Pre2,
                width,
                EntropyCoder::Zstd,
            )
            .unwrap();
            let column = &payloads[0];
            assert_eq!(column.raw_length, raw.len() as u64);
            assert_eq!(column.stored_length, bytes.len() as u64);
            assert_eq!(
                decode_with(column.codec, &bytes, raw.len(), width, raw.len(), true).unwrap(),
                raw
            );
            for older in [
                StorageLayout::V8Pre1,
                StorageLayout::V7,
                StorageLayout::V6Pre3,
            ] {
                assert!(
                    crate::prepare::append_payload(
                        &raw,
                        &mut Vec::new(),
                        &mut Vec::new(),
                        older,
                        width,
                        EntropyCoder::Zstd
                    )
                    .is_err()
                );
            }
        }
    }

    fn column(codec: BlockCodec) -> PayloadDirectoryEntry {
        PayloadDirectoryEntry {
            offset: 0,
            stored_length: 10,
            raw_length: 100,
            codec,
            checksum: 0,
            stored: StoredColumn::Present,
        }
    }

    #[test]
    fn zstd_is_rejected_before_v8_pre2() {
        use viewer_core::{AttributeDataType, AttributeDescriptor};
        let schema = [AttributeDescriptor::new("a", AttributeDataType::U32, 1).unwrap()];
        let layout_of = |version: FormatVersion| StorageLayout::from_version(version);
        let all = [
            StorageLayout::V5,
            StorageLayout::V6Pre1,
            StorageLayout::V6Pre2,
            StorageLayout::V6Pre3,
            StorageLayout::V6Pre4,
            StorageLayout::V7Pre1,
            StorageLayout::V7Pre2,
            StorageLayout::V7Pre3,
            StorageLayout::V7Pre4,
            StorageLayout::V7Pre5,
            StorageLayout::V7,
            StorageLayout::V8Pre1,
            StorageLayout::V8Pre2,
        ];
        assert_eq!(
            layout_of(FormatVersion::V8_PRE2).unwrap(),
            StorageLayout::V8Pre2
        );
        for layout in all {
            for codec in [BlockCodec::Zstd, BlockCodec::BytePlanesZstd] {
                let verdict = validate_column_codec(layout, &schema, 2, 25, &column(codec));
                assert_eq!(
                    verdict.is_ok(),
                    layout == StorageLayout::V8Pre2,
                    "{layout:?} {codec:?}"
                );
            }
            // Geometry is never zstd.
            assert!(
                validate_column_codec(layout, &schema, 0, 25, &column(BlockCodec::Zstd)).is_err()
            );
        }
        // The decoder of an older layout refuses the codecs and the dictionary tag.
        let mut refused = 0;
        for (width, raw, encoded) in zstd_payloads() {
            if encoded.codec() == BlockCodec::Constant
                || (encoded.codec() == BlockCodec::Dictionary
                    && encoded.bytes()[4] != 2
                    && encoded.bytes()[5] != 2)
            {
                continue;
            }
            refused += 1;
            assert!(
                decode_with(
                    encoded.codec(),
                    encoded.bytes(),
                    raw.len(),
                    width,
                    raw.len(),
                    false
                )
                .is_err(),
                "{:?}",
                encoded.codec()
            );
        }
        assert!(refused > 10);
    }

    #[test]
    fn corrupt_zstd_payload_is_an_error() {
        for (width, raw, encoded) in zstd_payloads() {
            let decode = |bytes: &[u8], size: usize| {
                decode_with(
                    encoded.codec(),
                    bytes,
                    size,
                    width,
                    size.max(raw.len()),
                    true,
                )
            };
            if matches!(encoded.codec(), BlockCodec::Constant) {
                continue;
            }
            let stored = encoded.bytes();
            // A raw length that is too small (the frame does not fit) or too large (it falls short).
            // Dictionary payloads size their own parts from the row count, so only the rest checks it.
            if matches!(
                encoded.codec(),
                BlockCodec::Zstd | BlockCodec::BytePlanesZstd
            ) {
                assert!(decode(stored, raw.len() - width).is_err());
                assert!(decode(stored, raw.len() + width).is_err());
            }
            assert!(decode(&stored[..stored.len() / 2], raw.len()).is_err());
            assert!(decode(&stored[..stored.len() - 1], raw.len()).is_err());
            let mut trailing = stored.to_vec();
            trailing.push(0);
            assert!(decode(&trailing, raw.len()).is_err());
            // Flipped bytes never panic and never produce a different length (the payload CRC
            // catches the rest before decoding).
            for position in (0..stored.len()).step_by((stored.len() / 16).max(1)) {
                let mut flipped = stored.to_vec();
                flipped[position] ^= 0xA5;
                if let Ok(output) = decode(&flipped, raw.len()) {
                    assert_eq!(output.len(), raw.len());
                }
            }
        }
    }

    #[test]
    fn lz4_coder_keeps_v8_pre1_bytes() {
        for (width, raw) in corpus() {
            let mut expected = (Vec::new(), Vec::new());
            let mut actual = (Vec::new(), Vec::new());
            crate::prepare::append_payload(
                &raw,
                &mut expected.0,
                &mut expected.1,
                StorageLayout::V8Pre1,
                width,
                EntropyCoder::Lz4,
            )
            .unwrap();
            crate::prepare::append_payload(
                &raw,
                &mut actual.0,
                &mut actual.1,
                StorageLayout::V8Pre2,
                width,
                EntropyCoder::Lz4,
            )
            .unwrap();
            assert_eq!(expected.0, actual.0, "width {width}");
            let key = |column: &PayloadDirectoryEntry| {
                (
                    column.codec as u8,
                    column.checksum,
                    column.stored_length,
                    column.raw_length,
                )
            };
            assert_eq!(key(&expected.1[0]), key(&actual.1[0]), "width {width}");
            assert!(matches!(
                actual.1[0].codec,
                BlockCodec::None
                    | BlockCodec::Lz4
                    | BlockCodec::Constant
                    | BlockCodec::Dictionary
                    | BlockCodec::BytePlanes
            ));
            let direct = encode_v7_pre3(&raw, width, EntropyCoder::Lz4).unwrap();
            assert_eq!(direct.bytes(), actual.0);
        }
    }
}
