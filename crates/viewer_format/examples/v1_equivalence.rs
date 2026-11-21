//! V1 gate: a v8.1-pre file holds the same drawable data as the v7 file converted from the same input.
//!
//! Usage: `v1_equivalence V7.oct V8PRE1.oct` (each with its `.octattrs` beside it).
//!
//! Precondition: same schema, node ids, hierarchy, block ids, point counts and quantised geometry.
//! Leaves: every column compared byte for byte, plus the display colour the reader returns. LOD
//! nodes: geometry, display colour, intensity and classification; v8.1-pre must store exactly the
//! columns the header's LOD set names (D2) and nothing else. When the row order inside a block
//! differs, the sorted multiset of whole rows is compared instead. Exits 0 only on zero
//! differences. The global frame (root cell, CRS, source scale/offset) must also match.
//!
//! `V1_EQUIVALENCE_MUTATE=<mode>` alters the v8.1-pre side in memory (the files stay untouched) to
//! show each comparison path can fail: `leaf-column` (flip a byte of a leaf column), `display`
//! (flip a display colour byte), `lod-column` (flip a byte of a LOD column), `stored-set` (drop a
//! stored leaf column), `reorder` (swap two rows of a leaf block: the multiset path must still
//! report 0 differences) and `reorder-corrupt` (swap two rows and flip a byte: the multiset path
//! must report a difference).
use std::{collections::BTreeSet, fs::File, path::Path};

use viewer_core::{AttributeDescriptor, AttributeValues, BlockId, NodeId, SpatialNode};
use viewer_format::{FormatReader, OctreeReader};

type Reader = FormatReader<File>;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MAX_REPORTED: usize = 20;

#[derive(Clone, Copy, PartialEq)]
enum Mutation {
    LeafColumn,
    Display,
    LodColumn,
    StoredSet,
    Reorder,
    ReorderCorrupt,
}

impl Mutation {
    fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "leaf-column" => Self::LeafColumn,
            "display" => Self::Display,
            "lod-column" => Self::LodColumn,
            "stored-set" => Self::StoredSet,
            "reorder" => Self::Reorder,
            "reorder-corrupt" => Self::ReorderCorrupt,
            other => return Err(format!("unknown V1_EQUIVALENCE_MUTATE mode {other:?}").into()),
        })
    }

    fn wants_lod(self) -> bool {
        self == Self::LodColumn
    }

    /// Applies the mutation to a v8.1-pre block when it is eligible; returns what was done.
    fn apply(self, block: &mut Block) -> Option<&'static str> {
        let flip = |block: &mut Block| {
            let (_, bytes, _) = block.columns.iter_mut().find(|c| !c.1.is_empty())?;
            bytes[0] ^= 1;
            Some("flipped one bit of the first byte of a column")
        };
        let swap = |block: &mut Block| {
            if block.quanta.len() < 2 || block.quanta[0] == block.quanta[1] {
                return None;
            }
            block.quanta.swap(0, 1);
            block.display.swap(0, 1);
            for (_, bytes, width) in &mut block.columns {
                for k in 0..*width {
                    bytes.swap(k, *width + k);
                }
            }
            Some("swapped rows 0 and 1")
        };
        match self {
            Self::LeafColumn | Self::LodColumn => flip(block),
            Self::Display => {
                block.display.first_mut()?[0] ^= 1;
                Some("flipped one bit of a display colour byte")
            }
            Self::StoredSet => {
                block.columns.pop()?;
                Some("dropped the last stored column")
            }
            Self::Reorder => swap(block),
            Self::ReorderCorrupt => {
                swap(block)?;
                flip(block)?;
                Some("swapped rows 0 and 1 and flipped one bit of a column byte")
            }
        }
    }
}

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
    leaf_points: u64,
    lod_points: u64,
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
    let mutation = std::env::var("V1_EQUIVALENCE_MUTATE")
        .ok()
        .map(|mode| Mutation::parse(&mode))
        .transpose()?;
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
    if old.format_metadata().global_frame() != new.format_metadata().global_frame() {
        println!(
            "PRECONDITION FAILED (global frame): root cell, CRS or source scale/offset differ"
        );
        return Ok(false);
    }
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
            let mut v7_has = Vec::new();
            for index in 0..schema_len {
                if old.attribute_present(id, index)? {
                    v7_has.push(index);
                }
            }
            let label = format!(
                "node {} block {} ({})",
                node_id.get(),
                id.get(),
                if is_lod { "LOD" } else { "leaf" }
            );
            totals.points += a.quanta.len() as u64;
            if is_lod {
                totals.lod_blocks += 1;
                totals.lod_points += a.quanta.len() as u64;
            } else {
                totals.leaf_blocks += 1;
                totals.leaf_points += a.quanta.len() as u64;
            }

            if let Some(mode) = mutation
                && !mutated
                && mode.wants_lod() == is_lod
                && (is_lod || !a.quanta.is_empty())
                && let Some(done) = mode.apply(&mut b)
            {
                mutated = true;
                println!("MUTATION: {done} in memory ({label})");
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
            // Reverse check: nothing stored in v8.1-pre that v7 does not have.
            for (index, _, _) in &b.columns {
                if !v7_has.contains(index) {
                    totals.differ(&format!(
                        "{label}: column {index} stored in v8.1-pre but absent in v7"
                    ));
                }
            }
            // Keep only the columns v7 was asked for (LOD: the LOD set).
            b.columns.retain(|c| wanted.contains(&c.0));

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
    if mutation.is_some() && !mutated {
        println!("MUTATION requested but no eligible block was found");
        return Ok(false);
    }
    println!(
        "summary: nodes={} blocks={} (leaf {}, LOD {}) points={} (leaf {}, LOD {}) columns_compared={} \
         row_order={} resorted_blocks={} differences={}",
        totals.nodes,
        totals.blocks,
        totals.leaf_blocks,
        totals.lod_blocks,
        totals.points,
        totals.leaf_points,
        totals.lod_points,
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
