//! Multi-node `v8` fixtures: a tree of nodes under a level-40 root, one block of four points per
//! node (block id = node id + 1), so a small page target chains child pages. Include with
//! `#[path = "common/v8_tree.rs"] mod v8_tree;` next to `mod common;` and `mod v8_fixture;`.
#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

use std::io::Cursor;

use viewer_core::{
    AttributeColumn, AttributeDescriptor, BlockId, ColorRgb, CoordinateFrame, DatasetMetadata,
    NodeId, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    FormatMetadata, FormatVersion, FormatWriter, FormatWriterOptions, GlobalFrame, NodeCell,
    SourcePointBlock,
};

use crate::common::root_origin;
use crate::v8_fixture::{v8_crs, with_v8_extras};

pub const TREE_ROOT_LEVEL: u32 = 40;
pub const TREE_ROOT_PREFIX: [u64; 3] = [(1 << 39) + 3, (1 << 39) + 4, (1 << 39) + 5];
pub const TREE_POINTS: usize = 4;

pub struct TreeNode {
    pub id: u64,
    pub parent: Option<u64>,
    pub depth: u32,
    pub prefix: [u64; 3],
    pub has_children: bool,
}

/// Breadth-first tree in which octant 0 always continues down to `max_depth` and every other
/// octant appears when `keep(parent id, octant)` says so.
pub fn tree(max_depth: u32, keep: impl Fn(u64, u64) -> bool) -> Vec<TreeNode> {
    let mut nodes = vec![TreeNode {
        id: 0,
        parent: None,
        depth: 0,
        prefix: TREE_ROOT_PREFIX,
        has_children: false,
    }];
    let mut cursor = 0;
    while cursor < nodes.len() {
        let (id, depth, prefix) = (nodes[cursor].id, nodes[cursor].depth, nodes[cursor].prefix);
        if depth < max_depth {
            for octant in 0..8_u64 {
                if octant == 0 || keep(id, octant) {
                    let child = nodes.len() as u64;
                    nodes.push(TreeNode {
                        id: child,
                        parent: Some(id),
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

/// Metadata and one block per node. `columns(node, row count)` gives the node's attribute columns.
pub fn tree_dataset(
    tree: &[TreeNode],
    schema: &[AttributeDescriptor],
    columns: impl Fn(&TreeNode, usize) -> Vec<AttributeColumn>,
) -> (FormatMetadata, Vec<SourcePointBlock>) {
    let cells: Vec<_> = tree
        .iter()
        .map(|n| {
            let spacing = if n.has_children {
                2.0_f64.powi(41 - (TREE_ROOT_LEVEL + n.depth) as i32)
            } else {
                2.0_f64.powi((49 - (TREE_ROOT_LEVEL + n.depth) as i32).min(-5))
            };
            (NodeId::new(n.id), NodeCell::new(n.prefix, spacing))
        })
        .collect();
    let origin = root_origin(TREE_ROOT_LEVEL, TREE_ROOT_PREFIX);
    let frame = GlobalFrame::new(
        TREE_ROOT_LEVEL,
        TREE_ROOT_PREFIX,
        origin,
        origin.map(|q| q + 100 * 32_768),
        v8_crs(),
        None,
        cells,
    )
    .unwrap();
    let mut nodes = Vec::new();
    let mut blocks = Vec::new();
    for n in tree {
        let bounds = frame.node_bounds(n.depth, n.prefix).unwrap();
        nodes.push(
            SpatialNode::new(
                NodeId::new(n.id),
                n.parent.map(NodeId::new),
                n.depth,
                bounds,
                TREE_POINTS as u64,
                0.0,
                vec![BlockId::new(n.id + 1)],
            )
            .unwrap(),
        );
        let points = (1..=TREE_POINTS)
            .map(|k| {
                let fraction = k as f64 / (TREE_POINTS + 1) as f64;
                std::array::from_fn(|axis| {
                    bounds.min()[axis] + (bounds.max()[axis] - bounds.min()[axis]) * fraction
                })
            })
            .collect();
        blocks.push(SourcePointBlock::new(
            BlockId::new(n.id + 1),
            points,
            vec![ColorRgb::new(1, 2, 3); TREE_POINTS],
            columns(n, TREE_POINTS),
        ));
    }
    let hierarchy = SpatialHierarchy::new(
        nodes,
        schema.to_vec(),
        Some((TREE_POINTS * tree.len()) as u64),
    )
    .unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(
            CoordinateFrame::new(frame.frame_origin()).unwrap(),
            hierarchy,
        ),
        None,
        Some("sample-grid:256;leaf:64000".to_owned()),
        Vec::new(),
    )
    .unwrap()
    .with_global_frame(frame)
    .unwrap();
    (with_v8_extras(metadata), blocks)
}

/// Writes the `.oct` / `.octattrs` pair; a small `page_target` chains child pages.
pub fn write_tree_pair(
    metadata: FormatMetadata,
    blocks: Vec<SourcePointBlock>,
    page_target: Option<usize>,
) -> (Vec<u8>, Vec<u8>) {
    let mut options = FormatWriterOptions::new(0.01, 1 << 24).unwrap();
    if let Some(target) = page_target {
        options = options.with_page_target_bytes(target);
    }
    let mut writer = FormatWriter::new_split_for_version(
        FormatVersion::V8,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata,
        options,
    )
    .unwrap();
    for block in blocks {
        writer.write_block(block).unwrap();
    }
    let (primary, attributes) = writer.finish_pair().unwrap();
    (primary.into_inner(), attributes.unwrap().into_inner())
}
