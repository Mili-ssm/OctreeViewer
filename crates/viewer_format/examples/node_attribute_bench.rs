//! Standalone experimental artifacts; never emits a supported .oct/.octattrs revision.
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Instant,
};
use viewer_core::{
    AttributeColumn, AttributeDescriptor, AttributeValues, BlockId, CategoricalAttribute, ColorRgb,
    DatasetSource, NodeAttributePolicy as Policy, OccupiedCellIndex, OccupiedOctreeRoot, Point,
    PointBlock, VoxelCoord, summarize_attribute, summarize_colors,
};
use viewer_format::{
    BlockPayloadKind, FormatReader,
    experimental_tuple_dictionary::{encode, encode_independent, read_column},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type Columns = Vec<(usize, Vec<u8>)>;

fn values_bytes(values: &AttributeValues) -> Vec<u8> {
    let mut out = Vec::new();
    macro_rules! bytes {
        ($v:expr) => {
            for value in $v {
                out.extend_from_slice(&value.to_le_bytes());
            }
        };
    }
    match values {
        AttributeValues::U8(v) => out.extend_from_slice(v),
        AttributeValues::I8(v) => bytes!(v),
        AttributeValues::U16(v) => bytes!(v),
        AttributeValues::I16(v) => bytes!(v),
        AttributeValues::U32(v) => bytes!(v),
        AttributeValues::I32(v) => bytes!(v),
        AttributeValues::U64(v) => bytes!(v),
        AttributeValues::I64(v) => bytes!(v),
        AttributeValues::F32(v) => bytes!(v),
        AttributeValues::F64(v) => bytes!(v),
    }
    out
}
fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}
struct Counted {
    file: File,
    bytes: u64,
}
impl Read for Counted {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.file.read(out)?;
        self.bytes += n as u64;
        Ok(n)
    }
}
impl Seek for Counted {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

#[derive(Default)]
struct Totals {
    bytes: u64,
    encode_ms: f64,
    decode_ms: f64,
    expand_ms: f64,
    selected_bytes: u64,
    unique: u64,
    rows: u64,
}
fn measure(
    output: &Path,
    block: usize,
    group: &str,
    counts: &[u32],
    columns: &Columns,
    repetitions: usize,
    totals: &mut [Totals; 2],
) -> Result<()> {
    let modes = ["independent", "shared_tuple_columns"];
    let mut enc_times = [Vec::new(), Vec::new()];
    let mut dec_times = [Vec::new(), Vec::new()];
    let mut expand_times = [Vec::new(), Vec::new()];
    let mut sizes = [0; 2];
    let mut uniques = [0; 2];
    let mut read_bytes = [vec![], vec![]];
    let rows: u64 = counts.iter().map(|&v| u64::from(v)).sum();
    for rep in 0..repetitions {
        for mode in if rep % 2 == 0 { [0, 1] } else { [1, 0] } {
            let start = Instant::now();
            let bytes = if mode == 0 {
                encode_independent(counts, columns)?
            } else {
                encode(counts, columns)?
            };
            enc_times[mode].push(start.elapsed().as_secs_f64() * 1000.);
            let path = output.join(format!("{block:05}-{group}-{}.bin", modes[mode]));
            if rep == 0 {
                fs::write(&path, &bytes)?;
            } else {
                assert_eq!(fs::read(&path)?, bytes, "nondeterministic encoding");
            }
            sizes[mode] = bytes.len();
            let mut dec = 0.;
            let mut expand = 0.;
            let mut selected = Vec::new();
            for (c, (_, raw)) in columns.iter().enumerate() {
                let mut reader = Counted {
                    file: File::open(&path)?,
                    bytes: 0,
                };
                let start = Instant::now();
                let projection = read_column(&mut reader, c, 256 * 1024 * 1024)?;
                dec += start.elapsed().as_secs_f64() * 1000.;
                uniques[mode] = projection.unique_count();
                selected.push(reader.bytes);
                let start = Instant::now();
                let restored = projection.expand();
                expand += start.elapsed().as_secs_f64() * 1000.;
                assert_eq!(restored, *raw, "roundtrip {group} column {c}");
                assert_eq!(
                    projection.level_counts().collect::<Vec<_>>(),
                    counts.iter().map(|&v| v as usize).collect::<Vec<_>>()
                );
            }
            dec_times[mode].push(dec);
            expand_times[mode].push(expand);
            read_bytes[mode] = selected;
        }
    }
    for mode in 0..2 {
        let enc = median(&mut enc_times[mode]);
        let dec = median(&mut dec_times[mode]);
        let expand = median(&mut expand_times[mode]);
        totals[mode].bytes += sizes[mode] as u64;
        totals[mode].encode_ms += enc;
        totals[mode].decode_ms += dec;
        totals[mode].expand_ms += expand;
        totals[mode].selected_bytes += read_bytes[mode][columns.len() - 1];
        totals[mode].unique += uniques[mode] as u64;
        totals[mode].rows += rows;
        println!(
            "{{\"block\":{block},\"group\":\"{group}\",\"mode\":\"{}\",\"rows\":{rows},\"unique\":{},\"bytes\":{},\"encode_ms\":{enc:.6},\"decode_ms\":{dec:.6},\"expand_ms\":{expand:.6},\"column_read_bytes\":{:?}}}",
            modes[mode], uniques[mode], sizes[mode], read_bytes[mode]
        );
    }
    Ok(())
}

fn rule_specs(text: &str) -> Result<Vec<(usize, Policy)>> {
    if text.is_empty() {
        return Ok(vec![]);
    }
    let mut rules = Vec::new();
    for spec in text.split(',') {
        let fields: Vec<_> = spec.split(':').collect();
        let column = fields.get(1).ok_or("missing column index")?.parse()?;
        let policy = match fields.as_slice() {
            ["mean", _] => Policy::Mean,
            ["range", _] => Policy::Range,
            ["category", _] => Policy::Categories { category_count: 0 },
            ["category", _, count] => Policy::Categories {
                category_count: count.parse()?,
            },
            _ => return Err("rules: mean:INDEX,range:INDEX,category:INDEX[:DENSE_DOMAIN]".into()),
        };
        if rules.iter().any(|&(i, _)| i == column) {
            return Err("duplicate rule column".into());
        }
        rules.push((column, policy));
    }
    Ok(rules)
}

fn synthetic(high_entropy: bool) -> PointBlock {
    let n = 4096;
    let points = (0..n)
        .map(|row| {
            let color = if high_entropy {
                (row * 73) as u8
            } else {
                ((row / 128) % 4 * 60) as u8
            };
            Point::new(
                [
                    (row % 16) as f32,
                    ((row / 16) % 16) as f32,
                    ((row / 256) % 16) as f32,
                ],
                ColorRgb::new(color, color, color),
            )
        })
        .collect::<Vec<_>>();
    let make = |name: &str, values: AttributeValues| {
        AttributeColumn::new(
            AttributeDescriptor::new(name, values.data_type(), 1).unwrap(),
            values,
            n,
        )
        .unwrap()
    };
    let intensity = make(
        "intensity",
        AttributeValues::U16(
            (0..n)
                .map(|r| {
                    if high_entropy {
                        r as u16
                    } else {
                        (r / 128 % 4) as u16
                    }
                })
                .collect(),
        ),
    );
    let category = make(
        "category",
        AttributeValues::U16((0..n).map(|r| (r / 128 % 9) as u16).collect()),
    );
    let time = make(
        "timestamp",
        AttributeValues::U64(
            (0..n)
                .map(|r| {
                    (1 << 53)
                        + if high_entropy {
                            r as u64
                        } else {
                            (r / 128) as u64
                        }
                })
                .collect(),
        ),
    );
    PointBlock::new(
        BlockId::new(0),
        points.into(),
        vec![intensity, category, time],
    )
}

fn experiment(
    output: &Path,
    block: usize,
    source: &PointBlock,
    rules: &[(usize, Policy)],
    repetitions: usize,
    totals: &mut [[Totals; 2]; 2],
) -> Result<f64> {
    let rows = source.points().len();
    if rows == 0 {
        return Ok(0.);
    }
    let colors: Vec<_> = source.points().points().iter().map(|p| p.color).collect();
    let mut originals = vec![(3, colors.iter().flat_map(|c| [c.r, c.g, c.b]).collect())];
    for column in source.attributes() {
        let bytes = values_bytes(column.values());
        originals.push((bytes.len() / rows, bytes));
    }
    measure(
        output,
        block,
        "source",
        &[u32::try_from(rows)?],
        &originals,
        repetitions,
        &mut totals[0],
    )?;
    // Explicit experimental depth-4 normalized block grid, not the persisted pre2 topology.
    let positions: Vec<_> = source
        .points()
        .points()
        .iter()
        .map(|p| p.position)
        .collect();
    let low: [f32; 3] =
        std::array::from_fn(|a| positions.iter().map(|p| p[a]).fold(f32::INFINITY, f32::min));
    let high: [f32; 3] = std::array::from_fn(|a| {
        positions
            .iter()
            .map(|p| p[a])
            .fold(f32::NEG_INFINITY, f32::max)
    });
    let index = OccupiedCellIndex::new(
        OccupiedOctreeRoot::new([0; 3], 4)?,
        positions.iter().map(|p| {
            let c: [i32; 3] = std::array::from_fn(|a| {
                if low[a] == high[a] {
                    0
                } else {
                    (((f64::from(p[a]) - f64::from(low[a]))
                        / (f64::from(high[a]) - f64::from(low[a]))
                        * 16.)
                        .floor() as i32)
                        .clamp(0, 15)
                }
            });
            VoxelCoord::new(c[0], c[1], c[2])
        }),
    )?;
    let categorize_start = Instant::now();
    let mut prepared = Vec::new();
    let mut category_domain_bytes = 0;
    for &(column, rule) in rules {
        let source_column = source
            .attributes()
            .get(column)
            .ok_or("rule column outside schema")?;
        let categorized = if rule == (Policy::Categories { category_count: 0 }) {
            Some(CategoricalAttribute::new(source_column)?)
        } else {
            None
        };
        prepared.push((column, rule, categorized));
    }
    let categorize_ms = categorize_start.elapsed().as_secs_f64() * 1000.;
    for (column, _, categorized) in &prepared {
        if let Some(c) = categorized {
            let mut bytes = c.category_count().to_le_bytes().to_vec();
            bytes.extend_from_slice(&c.dictionary().descriptor().component_count().to_le_bytes());
            bytes.extend_from_slice(&values_bytes(c.dictionary().values()));
            category_domain_bytes += bytes.len();
            fs::write(
                output.join(format!("{block:05}-category-{column}.bin")),
                bytes,
            )?;
        }
    }
    let mut summary_times = Vec::new();
    let mut summaries = Vec::new();
    for _ in 0..repetitions {
        let start = Instant::now();
        summaries = vec![summarize_colors(&index, &colors)?];
        for (column, rule, categorized) in &prepared {
            let source_column = source
                .attributes()
                .get(*column)
                .ok_or("rule column outside schema")?;
            if let Some(categorized) = categorized {
                summaries.push(summarize_attribute(
                    &index,
                    categorized.codes(),
                    Policy::Categories {
                        category_count: categorized.category_count(),
                    },
                )?);
            } else {
                summaries.push(summarize_attribute(&index, source_column, *rule)?);
            }
        }
        summary_times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    let summary_ms = median(&mut summary_times);
    let counts = index
        .topology()
        .levels()
        .iter()
        .take(4)
        .map(|nodes| u32::try_from(nodes.len()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let columns = summaries
        .iter()
        .map(|c| (c.record_width(), c.levels().concat()))
        .collect();
    measure(
        output,
        block,
        "internal",
        &counts,
        &columns,
        repetitions,
        &mut totals[1],
    )?;
    println!(
        "{{\"block\":{block},\"summary_build_ms\":{summary_ms:.6},\"categorize_ms\":{categorize_ms:.6},\"category_domain_bytes\":{category_domain_bytes},\"leaf_cells\":{},\"internal_counts\":{counts:?}}}",
        index.leaf_count()
    );
    Ok(summary_ms)
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 2 || args.len() > 5 {
        return Err("usage: node_attribute_bench INPUT.oct|--synthetic NEW_OUTPUT_DIR [MAX_BLOCKS [REPETITIONS [RULES]]] ; RULES=mean:INDEX,category:INDEX[:DENSE_DOMAIN],range:INDEX".into());
    }
    let output = PathBuf::from(&args[1]);
    let max_blocks: usize = args
        .get(2)
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(usize::MAX);
    let repetitions: usize = args.get(3).map(|v| v.parse()).transpose()?.unwrap_or(3);
    if max_blocks == 0 || repetitions == 0 {
        return Err("positive limits required".into());
    }
    let mut rules = rule_specs(args.get(4).map(String::as_str).unwrap_or(""))?;
    fs::create_dir(&output)?;
    let mut totals: [[Totals; 2]; 2] =
        std::array::from_fn(|_| std::array::from_fn(|_| Totals::default()));
    let mut summary_ms = 0.;
    let blocks;
    if args[0] == "--synthetic" {
        rules = vec![
            (0, Policy::Mean),
            (1, Policy::Categories { category_count: 9 }),
            (2, Policy::Range),
        ];
        blocks = 2.min(max_blocks);
        for b in 0..blocks {
            summary_ms += experiment(
                &output,
                b,
                &synthetic(b == 1),
                &rules,
                repetitions,
                &mut totals,
            )?;
        }
    } else {
        let input = PathBuf::from(&args[0]);
        let mut reader = FormatReader::new(File::open(&input)?)?;
        reader.attach_attributes(File::open(input.with_extension("octattrs"))?)?;
        for (i, descriptor) in reader
            .format_metadata()
            .dataset()
            .hierarchy()
            .attributes()
            .iter()
            .enumerate()
        {
            eprintln!("schema {i}: {descriptor:?}");
        }
        eprintln!(
            "explicit rules: {rules:?}; RGB uses linear mean; other attributes remain exact only"
        );
        let metadata = reader.block_storage_stats()?;
        let payloads = reader.block_payload_storage_stats()?;
        blocks = metadata.len().min(max_blocks);
        for (b, block) in metadata.iter().take(max_blocks).enumerate() {
            let source_payload_bytes: u64 = payloads
                .iter()
                .filter(|p| p.id == block.id && p.kind != BlockPayloadKind::Geometry)
                .map(|p| p.stored_bytes)
                .sum();
            println!("{{\"block\":{b},\"input_source_payload_bytes\":{source_payload_bytes}}}");
            summary_ms += experiment(
                &output,
                b,
                &reader.read_block(block.id)?,
                &rules,
                repetitions,
                &mut totals,
            )?;
        }
    }
    for (group, group_totals) in ["source", "internal"].iter().zip(&totals) {
        for (mode, t) in ["independent", "shared_tuple_columns"]
            .iter()
            .zip(group_totals)
        {
            println!(
                "{{\"summary\":true,\"group\":\"{group}\",\"mode\":\"{mode}\",\"blocks\":{blocks},\"rows\":{},\"unique_sum\":{},\"bytes\":{},\"encode_ms\":{:.6},\"decode_ms\":{:.6},\"expand_ms\":{:.6},\"selected_last_column_bytes\":{},\"summary_build_ms\":{summary_ms:.6},\"repetitions\":{repetitions}}}",
                t.rows, t.unique, t.bytes, t.encode_ms, t.decode_ms, t.expand_ms, t.selected_bytes
            );
        }
    }
    Ok(())
}
