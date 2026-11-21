//! V1 gate: a v8.1-pre file holds the same drawable data as the v7 file converted from the same input.
//!
//! Usage: `v1_equivalence V7.oct V8PRE1.oct` (each with its `.octattrs` beside it).
//!
//! Precondition: same schema, node ids, hierarchy, block ids, point counts and quantised geometry.
//! Leaves: every column compared byte for byte, plus the display colour the reader returns. LOD
//! nodes: geometry, display colour, intensity and classification; v8.1-pre must store exactly the
//! columns the header's LOD set names (D2) and nothing else. When the row order inside a block
//! differs, the sorted multiset of whole rows is compared instead. Exits 0 only on zero
//! differences. `V1_EQUIVALENCE_MUTATE=1` flips one byte of one compared leaf value in memory, to
//! show the comparison can fail.
use std::{collections::BTreeSet, fs::File, path::Path};

use viewer_core::{AttributeDescriptor, AttributeValues, BlockId, NodeId, SpatialNode};
use viewer_format::{FormatReader, OctreeReader};

type Reader = FormatReader<File>;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MAX_REPORTED: usize = 20;

fn open(oct: &Path) -> Result<Reader> {
    let mut reader = FormatReader::new(File::open(oct)?)?;
    reader.attach_attributes(File::open(oct.with_extension("octattrs"))?)?;
    Ok(reader)
}

fn le_bytes(values: &AttributeValues) -> Vec<u8> {
    macro_rules! flat {
        ($v:expr) => {
            $v.iter().flat_map(|x| x.to_le_bytes()).collect()
        };
    }
    match values {
        AttributeValues::U8(v) => v.clone(),
        AttributeValues::I8(v) => flat!(v),
        AttributeValues::U16(v) => flat!(v),
        AttributeValues::I16(v) => flat!(v),
        AttributeValues::U32(v) => flat!(v),
        AttributeValues::I32(v) => flat!(v),
        AttributeValues::U64(v) => flat!(v),
        AttributeValues::I64(v) => flat!(v),
        AttributeValues::F32(v) => flat!(v),
        AttributeValues::F64(v) => flat!(v),
    }
}

/// One block of one format: every column the gate compares, as raw bytes.
struct Block {
    quanta: Vec<[i64; 3]>,
    display: Vec<[u8; 3]>,
    /// (schema index, little-endian bytes, bytes per row), only for stored columns.
    columns: Vec<(u32, Vec<u8>, usize)>,
}

fn read_block(reader: &mut Reader, id: BlockId, schema_len: u32, wanted: &[u32]) -> Result<Block> {
    let quanta = reader.read_block_quanta(id)?;
    let display = reader
        .read_display_colors(id)?
        .iter()
        .map(|c| [c.r, c.g, c.b])
        .collect();
    let mut columns = Vec::new();
    for index in 0..schema_len {
        if !wanted.contains(&index) || !reader.attribute_present(id, index)? {
            continue;
        }
        let column = reader.read_attribute(id, index)?;
        let bytes = le_bytes(column.values());
        let width = bytes.len().checked_div(quanta.len()).unwrap_or(0);
        columns.push((index, bytes, width));
    }
    Ok(Block {
        quanta,
        display,
        columns,
    })
}

#[derive(Default)]
struct Totals {
    nodes: u64,
    blocks: u64,
    points: u64,
    leaf_blocks: u64,
    lod_blocks: u64,
    columns_compared: u64,
    resorted_blocks: u64,
    differences: u64,
    reported: usize,
}

impl Totals {
    fn differ(&mut self, message: &str) {
        self.differences += 1;
        if self.reported < MAX_REPORTED {
            self.reported += 1;
            println!("DIFFERENCE: {message}");
        }
    }
}

fn rows(block: &Block, columns: &[u32]) -> Vec<Vec<u8>> {
    (0..block.quanta.len())
        .map(|row| {
            let mut key = Vec::new();
            for axis in block.quanta[row] {
                key.extend_from_slice(&axis.to_le_bytes());
            }
            key.extend_from_slice(&block.display[row]);
            for (index, bytes, width) in &block.columns {
                if columns.contains(index) {
                    key.extend_from_slice(&bytes[row * width..(row + 1) * width]);
                }
            }
            key
        })
        .collect()
}

fn precondition(old: &Reader, new: &Reader) -> std::result::Result<Vec<(NodeId, bool)>, String> {
    let (old_data, new_data) = (
        old.format_metadata().dataset(),
        new.format_metadata().dataset(),
    );
    if old_data.hierarchy().attributes() != new_data.hierarchy().attributes() {
        return Err("schemas differ".into());
    }
    let (old_h, new_h) = (old_data.hierarchy(), new_data.hierarchy());
    if old_h.root_id() != new_h.root_id() || old_h.block_ids() != new_h.block_ids() {
        return Err("root id or block ids differ".into());
    }
    let mut order = Vec::new();
    let mut stack = vec![old_h.root_id()];
    while let Some(id) = stack.pop() {
        let (Some(a), Some(b)) = (old_h.node(id), new_h.node(id)) else {
            return Err(format!("node {} missing in one file", id.get()));
        };
        let same = |a: &SpatialNode, b: &SpatialNode| {
            a.id() == b.id()
                && a.parent() == b.parent()
                && a.level() == b.level()
                && a.bounds() == b.bounds()
                && a.point_count() == b.point_count()
                && a.blocks() == b.blocks()
        };
        if !same(a, b) {
            return Err(format!(
                "node {} differs (parent, level, bounds, point count or blocks)",
                id.get()
            ));
        }
        if old_h.child_ids(id) != new_h.child_ids(id) {
            return Err(format!("children of node {} differ", id.get()));
        }
        order.push((id, !old_h.child_ids(id).is_empty()));
        stack.extend(old_h.child_ids(id).iter().rev());
    }
    if order.len() != old_h.stats().node_count || order.len() != new_h.stats().node_count {
        return Err("node counts differ from the traversal".into());
    }
    Ok(order)
}

fn run() -> Result<bool> {
    let mut args = std::env::args().skip(1);
    let (Some(old_path), Some(new_path), None) = (args.next(), args.next(), args.next()) else {
        return Err("usage: v1_equivalence V7.oct V8PRE1.oct".into());
    };
    let mutate = std::env::var_os("V1_EQUIVALENCE_MUTATE").is_some();
    let (mut old, mut new) = (open(Path::new(&old_path))?, open(Path::new(&new_path))?);
    println!(
        "v7: {:?}  v8.1-pre: {:?}",
        old.format_version(),
        new.format_version()
    );

    let order = match precondition(&old, &new) {
        Ok(order) => order,
        Err(why) => {
            println!("PRECONDITION FAILED (hierarchy): {why}");
            return Ok(false);
        }
    };
    let schema = old
        .format_metadata()
        .dataset()
        .hierarchy()
        .attributes()
        .to_vec();
    let schema_len = schema.len() as u32;
    let name_of = |name: &str| {
        schema
            .iter()
            .position(|d| d.name() == name)
            .map(|i| i as u32)
    };
    let lod_set: Vec<u32> = new
        .format_metadata()
        .lod_attribute_columns()
        .ok_or("v8.1-pre file has no LOD column set")?
        .to_vec();
    let expected_lod: BTreeSet<u32> = ["intensity", "classification"]
        .into_iter()
        .filter_map(name_of)
        .collect();
    if lod_set.iter().copied().collect::<BTreeSet<_>>() != expected_lod {
        println!("UNEXPECTED: the LOD column set is {lod_set:?}, expected {expected_lod:?} (D2)");
        return Ok(false);
    }
    let all: Vec<u32> = (0..schema_len).collect();
    println!(
        "schema ({} columns): {}",
        schema.len(),
        schema
            .iter()
            .map(AttributeDescriptor::name)
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "LOD set: {}",
        lod_set
            .iter()
            .map(|&i| schema[i as usize].name())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let mut totals = Totals::default();
    let mut mutated = false;
    for &(node_id, is_lod) in &order {
        totals.nodes += 1;
        let node = old
            .format_metadata()
            .dataset()
            .hierarchy()
            .node(node_id)
            .cloned();
        let node = node.ok_or("node vanished")?;
        let mut node_points = 0_u64;
        for &id in node.blocks() {
            totals.blocks += 1;
            let wanted: &[u32] = if is_lod { &lod_set } else { &all };
            let a = read_block(&mut old, id, schema_len, wanted)?;
            let mut b = read_block(&mut new, id, schema_len, &all)?;
            let label = format!(
                "node {} block {} ({})",
                node_id.get(),
                id.get(),
                if is_lod { "LOD" } else { "leaf" }
            );
            totals.points += a.quanta.len() as u64;
            if is_lod {
                totals.lod_blocks += 1;
            } else {
                totals.leaf_blocks += 1;
            }

            // Precondition: point counts and geometry bytes.
            if b.quanta.len() != a.quanta.len() {
                println!(
                    "PRECONDITION FAILED: {label}: point counts differ (v7 {}, v8.1-pre {})",
                    a.quanta.len(),
                    b.quanta.len()
                );
                return Ok(false);
            }
            node_points += a.quanta.len() as u64;
            let same_geometry = a.quanta == b.quanta;
            let mut sorted_geometry = (a.quanta.clone(), b.quanta.clone());
            sorted_geometry.0.sort_unstable();
            sorted_geometry.1.sort_unstable();
            if sorted_geometry.0 != sorted_geometry.1 {
                println!("PRECONDITION FAILED: {label}: quantised geometry differs");
                return Ok(false);
            }

            // D2: stored columns of the v8.1-pre block are exactly the expected set.
            let stored: Vec<u32> = b.columns.iter().map(|c| c.0).collect();
            let expected: &[u32] = if is_lod { &lod_set } else { &all };
            if stored.iter().collect::<BTreeSet<_>>() != expected.iter().collect::<BTreeSet<_>>() {
                totals.differ(&format!(
                    "{label}: v8.1-pre stores columns {stored:?}, expected {expected:?}"
                ));
            }
            // Keep only the columns v7 was asked for (LOD: the LOD set).
            b.columns.retain(|c| wanted.contains(&c.0));

            if mutate
                && !is_lod
                && !mutated
                && let Some((_, bytes, _)) = b.columns.iter_mut().find(|c| !c.1.is_empty())
            {
                bytes[0] ^= 1;
                mutated = true;
                println!("MUTATION: flipped one bit of a leaf column byte in memory ({label})");
            }

            if same_geometry {
                if a.display != b.display {
                    totals.differ(&format!("{label}: display colour differs"));
                }
                totals.columns_compared += 2; // geometry + display colour
                for (index, bytes, _) in &a.columns {
                    totals.columns_compared += 1;
                    match b.columns.iter().find(|c| c.0 == *index) {
                        Some(other) if other.1 == *bytes => {}
                        Some(_) => totals.differ(&format!(
                            "{label}: column {} ({}) differs",
                            index,
                            schema[*index as usize].name()
                        )),
                        None => {
                            totals.differ(&format!("{label}: column {index} missing in v8.1-pre"));
                        }
                    }
                }
            } else {
                totals.resorted_blocks += 1;
                let mut ra = rows(&a, wanted);
                let mut rb = rows(&b, wanted);
                ra.sort_unstable();
                rb.sort_unstable();
                totals.columns_compared += 2 + a.columns.len() as u64;
                if ra != rb {
                    totals.differ(&format!("{label}: sorted row multisets differ"));
                }
            }
        }
        if node_points != node.point_count() {
            println!(
                "PRECONDITION FAILED: node {}: its blocks hold {node_points} points, the index says {}",
                node_id.get(),
                node.point_count()
            );
            return Ok(false);
        }
    }
    if mutate && !mutated {
        println!("MUTATION requested but no leaf column was found to flip");
    }
    println!(
        "summary: nodes={} blocks={} (leaf {}, LOD {}) points={} columns_compared={} \
         row_order={} resorted_blocks={} differences={}",
        totals.nodes,
        totals.blocks,
        totals.leaf_blocks,
        totals.lod_blocks,
        totals.points,
        totals.columns_compared,
        if totals.resorted_blocks == 0 {
            "identical"
        } else {
            "multiset"
        },
        totals.resorted_blocks,
        totals.differences
    );
    Ok(totals.differences == 0)
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::from(2)
        }
    }
}
