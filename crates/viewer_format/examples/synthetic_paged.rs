//! Deterministic synthetic `v7.5-pre` file with a very large node count and two-point blocks,
//! for open-time and lazy-page tests that need a hierarchy far bigger than any real dataset
//! fits in the repository.
//!
//! `cargo run --release -p viewer_format --example synthetic_paged -- OUT.oct [NODES]`
//! (default 6 000 000 nodes). The same arguments always produce the same bytes. The file is not
//! committed; regenerate it where needed and check it with `viewer_convert verify`.

use std::{fs::File, io::BufWriter, time::Instant};

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    CrsRaw, CrsRecord, FormatMetadata, FormatVersion, FormatWriter, FormatWriterOptions,
    GlobalFrame, NodeCell, SourcePointBlock, cell_origin_quanta,
};

const ROOT_LEVEL: u32 = 40;
const ROOT_PREFIX: [u64; 3] = [(1 << 39) + 3, (1 << 39) + 4, (1 << 39) + 5];
const MAX_DEPTH: u32 = 21;
/// Chance (percent) that an octant other than 0 has a node; octant 0 always continues down.
const BRANCH_PERCENT: u64 = 35;
const DEFAULT_NODES: usize = 6_000_000;

struct TreeNode {
    parent: Option<u64>,
    depth: u32,
    prefix: [u64; 3],
    has_children: bool,
}

/// Mixes the parent's index and the octant into a pseudo-random percentage (SplitMix64).
fn roll(parent: u64, octant: u64) -> u64 {
    let mut z = (parent << 3 | octant).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) % 100
}

/// Breadth-first tree; once `target` nodes exist the rest of the frontier stays as leaves. Node
/// `i` has id `i` and a parent with a smaller id.
fn tree(target: usize) -> Vec<TreeNode> {
    let mut nodes = vec![TreeNode {
        parent: None,
        depth: 0,
        prefix: ROOT_PREFIX,
        has_children: false,
    }];
    let mut cursor = 0;
    while cursor < nodes.len() && nodes.len() < target {
        let (depth, prefix) = (nodes[cursor].depth, nodes[cursor].prefix);
        if depth < MAX_DEPTH {
            for octant in 0..8_u64 {
                if nodes.len() < target
                    && (octant == 0 || roll(cursor as u64, octant) < BRANCH_PERCENT)
                {
                    nodes.push(TreeNode {
                        parent: Some(cursor as u64),
                        depth: depth + 1,
                        prefix: std::array::from_fn(|axis| {
                            (prefix[axis] << 1) | ((octant >> axis) & 1)
                        }),
                        has_children: false,
                    });
                    nodes[cursor].has_children = true;
                }
            }
        }
        cursor += 1;
    }
    nodes
}

fn schema() -> Vec<AttributeDescriptor> {
    vec![AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).expect("valid schema")]
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let Some(output) = arguments.next() else {
        eprintln!("usage: synthetic_paged OUT.oct [NODES]");
        std::process::exit(2);
    };
    let target = arguments
        .next()
        .map_or(DEFAULT_NODES, |nodes| nodes.parse().expect("node count"));
    let started = Instant::now();
    let tree = tree(target);

    let cells: Vec<_> = tree
        .iter()
        .enumerate()
        .map(|(id, n)| {
            let spacing = if n.has_children {
                2.0_f64.powi(1 - n.depth as i32)
            } else {
                2.0_f64.powi(-5)
            };
            (NodeId::new(id as u64), NodeCell::new(n.prefix, spacing))
        })
        .collect();
    let origin = ROOT_PREFIX.map(|p| cell_origin_quanta(ROOT_LEVEL, p));
    let crs = CrsRecord::new(
        "EPSG".to_owned(),
        32632,
        CrsRaw::Wkt("PROJCS[\"WGS 84 / UTM zone 32N\",AUTHORITY[\"EPSG\",\"32632\"]]".into()),
    )
    .expect("valid CRS");
    let frame = GlobalFrame::new(
        ROOT_LEVEL,
        ROOT_PREFIX,
        origin,
        origin.map(|q| q + 100 * 32_768),
        crs,
        None,
        cells,
    )
    .expect("valid frame");
    let bounds: Vec<_> = tree
        .iter()
        .map(|n| frame.node_bounds(n.depth, n.prefix).expect("node bounds"))
        .collect();
    let nodes: Vec<_> = tree
        .iter()
        .enumerate()
        .map(|(id, n)| {
            SpatialNode::new(
                NodeId::new(id as u64),
                n.parent.map(NodeId::new),
                n.depth,
                bounds[id],
                2,
                0.0,
                vec![BlockId::new(id as u64 + 1)],
            )
            .expect("node")
        })
        .collect();
    let hierarchy =
        SpatialHierarchy::new(nodes, schema(), Some(2 * tree.len() as u64)).expect("hierarchy");
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(
            CoordinateFrame::new(frame.frame_origin()).expect("frame origin"),
            hierarchy,
        ),
        None,
        Some("sample-grid:256;leaf:64000".to_owned()),
        Vec::new(),
    )
    .expect("metadata")
    .with_global_frame(frame)
    .expect("global frame");

    let file = BufWriter::new(File::create(&output).expect("create output"));
    let options = FormatWriterOptions::new(0.01, 1 << 24).expect("writer options");
    let mut writer = FormatWriter::new_for_version(FormatVersion::V7_PRE5, file, metadata, options)
        .expect("writer");
    // The writer needs `Seek` and `BufWriter<File>` has it; blocks are made one at a time.
    for (id, node) in tree.iter().enumerate() {
        let cell = bounds[id];
        let at = |fraction: f64| {
            std::array::from_fn(|axis| {
                cell.min()[axis] + (cell.max()[axis] - cell.min()[axis]) * fraction
            })
        };
        let intensity = AttributeColumn::new(
            schema().remove(0),
            AttributeValues::U16(vec![id as u16, 7 + node.depth as u16]),
            2,
        )
        .expect("attribute column");
        writer
            .write_block(SourcePointBlock::new(
                BlockId::new(id as u64 + 1),
                vec![at(0.25), at(0.75)],
                vec![ColorRgb::new(1, 2, 3); 2],
                vec![intensity],
            ))
            .expect("block");
    }
    writer.finish().expect("finish");
    let bytes = std::fs::metadata(&output).expect("output metadata").len();
    println!(
        "{} nodes, {bytes} bytes, {:.1} s -> {output}",
        tree.len(),
        started.elapsed().as_secs_f64()
    );
}
