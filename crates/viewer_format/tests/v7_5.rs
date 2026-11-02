//! `v7.5-pre`: paged hierarchy (root page next to the footer, chained child pages).

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Seek, SeekFrom};

use common::{repair_index_and_footer, root_origin};
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    CrsRecord, FormatError, FormatMetadata, FormatReader, FormatReaderLimits, FormatVersion,
    FormatWriter, FormatWriterOptions, GlobalFrame, NodeCell, PageAnchor, PagedReader,
    SourcePointBlock, page_node_id, split_node_id,
};

const V74: FormatVersion = FormatVersion::V7_PRE4;
const V75: FormatVersion = FormatVersion::V7_PRE5;
const ROOT_LEVEL: u32 = 40;
const ROOT_PREFIX: [u64; 3] = [(1 << 39) + 3, (1 << 39) + 4, (1 << 39) + 5];

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

struct TreeNode {
    id: u64,
    parent: Option<u64>,
    depth: u32,
    prefix: [u64; 3],
    has_children: bool,
}

/// Breadth-first random tree: octant 0 always continues down to `max_depth` (so the tree crosses
/// several page levels), every other octant appears with probability `percent`.
fn random_tree(seed: u64, max_depth: u32, percent: u64) -> Vec<TreeNode> {
    let mut random = Lcg(seed);
    let mut nodes = vec![TreeNode {
        id: 0,
        parent: None,
        depth: 0,
        prefix: ROOT_PREFIX,
        has_children: false,
    }];
    let mut cursor = 0;
    while cursor < nodes.len() {
        let (id, depth, prefix) = (nodes[cursor].id, nodes[cursor].depth, nodes[cursor].prefix);
        if depth < max_depth {
            for octant in 0..8_u64 {
                if octant == 0 || random.next() % 100 < percent {
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

fn schema() -> Vec<AttributeDescriptor> {
    vec![AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap()]
}

fn wkt_crs() -> CrsRecord {
    CrsRecord::new(
        "EPSG".to_owned(),
        32632,
        viewer_format::CrsRaw::Wkt(
            "PROJCS[\"WGS 84 / UTM zone 32N\",AUTHORITY[\"EPSG\",\"32632\"]]".into(),
        ),
    )
    .unwrap()
}

/// Metadata plus one two-point block per node (block id = node id + 1), points at 1/4 and 3/4 of
/// the node cell.
fn dataset(tree: &[TreeNode]) -> (FormatMetadata, Vec<SourcePointBlock>) {
    dataset_at(tree, ROOT_LEVEL, ROOT_PREFIX)
}

fn dataset_at(
    tree: &[TreeNode],
    root_level: u32,
    root_prefix: [u64; 3],
) -> (FormatMetadata, Vec<SourcePointBlock>) {
    let cells: Vec<_> = tree
        .iter()
        .map(|n| {
            let spacing = if n.has_children {
                2.0_f64.powi(41 - (root_level + n.depth) as i32)
            } else {
                2.0_f64.powi((49 - (root_level + n.depth) as i32).min(-5))
            };
            (NodeId::new(n.id), NodeCell::new(n.prefix, spacing))
        })
        .collect();
    // A level-0 root has its corner 2^48 units away; the frame origin sits at the cloud instead.
    let origin = if root_level == 0 {
        [0; 3]
    } else {
        root_origin(root_level, root_prefix)
    };
    let frame = GlobalFrame::new(
        root_level,
        root_prefix,
        origin,
        origin.map(|q| q + 100 * 32_768),
        wkt_crs(),
        None,
        cells,
    )
    .unwrap();
    let mut nodes = Vec::new();
    let mut blocks = Vec::new();
    // A block must fit one cell of its node. Near the root a node is huge, so a chain puts every
    // node's points in its deepest node (the chain's last entry), which every ancestor contains.
    let deepest = tree.last().unwrap();
    let deepest_bounds = frame.node_bounds(deepest.depth, deepest.prefix).unwrap();
    for n in tree {
        let bounds = frame.node_bounds(n.depth, n.prefix).unwrap();
        let point_bounds = if root_level == 0 {
            deepest_bounds
        } else {
            bounds
        };
        nodes.push(
            SpatialNode::new(
                NodeId::new(n.id),
                n.parent.map(NodeId::new),
                n.depth,
                bounds,
                2,
                0.0,
                vec![BlockId::new(n.id + 1)],
            )
            .unwrap(),
        );
        // Chain: stay within 2^32 quanta (the side of one block cell) of the origin, on the grid.
        let at = |fraction: f64| {
            std::array::from_fn(|axis| {
                let side = point_bounds.max()[axis] - point_bounds.min()[axis];
                let side = if root_level == 0 {
                    side.min(1024.0)
                } else {
                    side
                };
                point_bounds.min()[axis] + side * fraction
            })
        };
        let intensity = AttributeColumn::new(
            schema().remove(0),
            AttributeValues::U16(vec![n.id as u16, 7 + n.depth as u16]),
            2,
        )
        .unwrap();
        blocks.push(SourcePointBlock::new(
            BlockId::new(n.id + 1),
            vec![at(0.25), at(0.75)],
            vec![ColorRgb::new(1, 2, 3); 2],
            vec![intensity],
        ));
    }
    let hierarchy = SpatialHierarchy::new(nodes, schema(), Some(2 * tree.len() as u64)).unwrap();
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
    (metadata, blocks)
}

/// The `.oct` / `.octattrs` pair.
fn write_pair(
    version: FormatVersion,
    tree: &[TreeNode],
    page_target: Option<usize>,
) -> (Vec<u8>, Vec<u8>) {
    let (metadata, blocks) = dataset(tree);
    let mut options = FormatWriterOptions::new(0.01, 1 << 24).unwrap();
    if let Some(target) = page_target {
        options = options.with_page_target_bytes(target);
    }
    let mut writer = FormatWriter::new_split_for_version(
        version,
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

/// The `.oct` alone.
/// A level-0 root (prefix 0), as a cloud straddling a high lattice bit produces.
fn write_chain(tree: &[TreeNode]) -> (Vec<u8>, Vec<u8>) {
    let (metadata, blocks) = dataset_at(tree, 0, [0; 3]);
    let options = FormatWriterOptions::new(0.01, 1 << 24).unwrap();
    let mut writer = FormatWriter::new_split_for_version(
        V75,
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

fn write(version: FormatVersion, tree: &[TreeNode], page_target: Option<usize>) -> Vec<u8> {
    write_pair(version, tree, page_target).0
}

fn open_pair(primary: Vec<u8>, attributes: Vec<u8>) -> FormatReader<Cursor<Vec<u8>>> {
    let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
    reader.attach_attributes(Cursor::new(attributes)).unwrap();
    reader
}

/// Every page of a file in breadth-first order, with its pointer (`None` for the root).
fn all_pages(
    reader: &mut PagedReader<Cursor<Vec<u8>>>,
) -> Vec<(viewer_format::Page, Option<viewer_format::PagePointer>)> {
    let mut pages = vec![(reader.root_page().clone(), None)];
    let mut next = 0;
    while next < pages.len() {
        let children: Vec<_> = pages[next].0.child_pages().collect();
        next += 1;
        for (anchor, pointer) in children {
            pages.push((
                reader.read_child_page(anchor, pointer).unwrap(),
                Some(pointer),
            ));
        }
    }
    pages
}

#[test]
fn v7_5_is_writable_and_v5_stays_the_default() {
    assert!(FormatVersion::WRITABLE.contains(&V75));
    assert_eq!(FormatVersion::DEFAULT_OUTPUT, FormatVersion::V5);
    assert_eq!(V75.to_string(), "v7.5-pre");
    let bytes = write(V75, &random_tree(1, 3, 10), None);
    assert_eq!(&bytes[..8], b"OCTVFRM1");
    assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 5);
    assert_eq!(&bytes[bytes.len() - 40..bytes.len() - 32], b"OCTVE75P");
}

#[test]
fn pages_chain_and_round_trip_for_several_k() {
    let tree = random_tree(42, 8, 12);
    for target in [None, Some(3_000), Some(1_200), Some(1)] {
        let bytes = write(V75, &tree, target);
        let mut reader = PagedReader::open(Cursor::new(bytes)).unwrap();
        let pages = all_pages(&mut reader);
        let levels: BTreeSet<u8> = pages.iter().map(|(p, _)| p.levels()).collect();
        let nodes: usize = pages.iter().map(|(p, _)| p.nodes().len()).sum();
        assert_eq!(nodes, tree.len(), "{target:?}");
        assert_eq!(reader.header().total_nodes(), tree.len() as u64);
        assert!(
            pages.len() > 1,
            "{target:?}: the tree must need several pages"
        );
        match target {
            None => assert!(levels.contains(&5), "{levels:?}"),
            Some(1) => assert_eq!(levels, BTreeSet::from([1]), "{levels:?}"),
            Some(_) => assert!(levels.len() > 1, "{target:?}: {levels:?}"),
        }
        // A page above the target is only allowed at k = 1, and the child anchor matches its node.
        for (page, pointer) in &pages {
            if let Some(target) = target
                && page.levels() > 1
            {
                assert!(
                    page.encoded_len() <= target,
                    "{target}: {}",
                    page.encoded_len()
                );
            }
            if let Some(pointer) = pointer {
                assert_eq!(pointer.length as usize, page.encoded_len());
            }
        }
        let mut seen = BTreeSet::new();
        for (page, _) in &pages {
            for node in page.nodes() {
                assert!(seen.insert((node.level(), node.prefix())));
            }
        }
        assert_eq!(seen.len(), tree.len());
    }
}

#[test]
fn node_ids_are_page_number_and_index_in_page_order() {
    assert_eq!(page_node_id(0, 0), NodeId::new(0));
    assert_eq!(page_node_id(3, 7), NodeId::new((3 << 32) | 7));
    assert_eq!(split_node_id(page_node_id(u32::MAX, 5)), (u32::MAX, 5));
    let tree = random_tree(5, 7, 15);
    let bytes = write(V75, &tree, Some(2_000));
    assert_eq!(bytes, write(V75, &tree, Some(2_000)), "deterministic");
    let mut reader = PagedReader::open(Cursor::new(bytes)).unwrap();
    let pages = all_pages(&mut reader);
    assert!(pages.len() > 3);
    let mut seen = BTreeSet::new();
    for (position, (page, pointer)) in pages.iter().enumerate() {
        // Pages are numbered in plan (breadth-first) order and the pointer says so.
        assert_eq!(page.number() as usize, position);
        assert_eq!(
            pointer.map(|p| p.page as usize),
            (position > 0).then_some(position)
        );
        let keys: Vec<_> = page.nodes().iter().map(|n| (n.depth(), n.path())).collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "(depth, path) order");
        for (index, node) in page.nodes().iter().enumerate() {
            assert_eq!(node.id(), page_node_id(page.number(), index as u32));
            assert!(seen.insert(node.id()));
        }
        // The parent of a top node is the anchor record in the parent page.
        if let Some(pointer) = pointer {
            assert!(pointer.anchor_node.get() < (position as u64) << 32);
            for node in page.nodes().iter().filter(|n| n.depth() == 1) {
                assert_eq!(node.parent(), Some(pointer.anchor_node));
            }
        }
    }
    assert_eq!(seen.len(), tree.len());
    let eager = FormatReader::new(Cursor::new(write(V75, &tree, Some(2_000)))).unwrap();
    let hierarchy = eager.format_metadata().dataset().hierarchy();
    assert_eq!(hierarchy.root_id(), page_node_id(0, 0));
    for (page, _) in &pages {
        for node in page.nodes() {
            assert_eq!(hierarchy.node(node.id()).unwrap().parent(), node.parent());
        }
    }
}

/// A chain far deeper than the old 21-level limit (a cloud straddling a high lattice bit has a
/// level-0 root and a long single-child chain).
#[test]
fn chains_deeper_than_21_levels_write_and_read() {
    for depth in [22_u32, 40, 56] {
        let tree: Vec<TreeNode> = (0..=depth)
            .map(|d| TreeNode {
                id: d.into(),
                parent: d.checked_sub(1).map(u64::from),
                depth: d,
                // Level 1 prefix 1 is the positive half-lattice; then octant 0 all the way down,
                // so the chain hugs the lattice origin.
                prefix: [d.checked_sub(1).map_or(0, |shift| 1_u64 << shift); 3],
                has_children: d < depth,
            })
            .collect();
        let (primary, attributes) = write_chain(&tree);
        let mut paged = PagedReader::open(Cursor::new(primary.clone())).unwrap();
        let pages = all_pages(&mut paged);
        let nodes: usize = pages.iter().map(|(page, _)| page.nodes().len()).sum();
        assert_eq!(nodes, tree.len(), "depth {depth}");
        assert!(pages.len() >= depth as usize / 5, "depth {depth}");
        let stats = viewer_format::verify_paged(
            &mut Cursor::new(primary.clone()),
            &FormatReaderLimits::default(),
        )
        .unwrap();
        assert_eq!(stats.nodes, tree.len() as u64);
        let mut reader = open_pair(primary, attributes);
        let hierarchy = reader.format_metadata().dataset().hierarchy();
        assert_eq!(hierarchy.stats().max_level, depth);
        let leaf = hierarchy
            .query(hierarchy.stats().root_bounds)
            .last()
            .unwrap();
        let block = leaf.blocks()[0];
        assert_eq!(reader.read_positions(block).unwrap().len(), 2);
        assert_eq!(reader.read_display_colors(block).unwrap().len(), 2);
    }
}

#[test]
fn open_costs_one_tail_read_and_no_header_read() {
    struct Counting {
        inner: Cursor<Vec<u8>>,
        reads: usize,
        seeks: Vec<SeekFrom>,
    }
    impl Read for Counting {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads += 1;
            self.inner.read(buf)
        }
    }
    impl Seek for Counting {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.seeks.push(pos);
            self.inner.seek(pos)
        }
    }
    let bytes = write(V75, &random_tree(9, 8, 12), None);
    let length = bytes.len() as u64;
    let counting = Counting {
        inner: Cursor::new(bytes),
        reads: 0,
        seeks: Vec::new(),
    };
    let reader = PagedReader::open(counting).unwrap();
    let counting = reader.into_inner();
    assert_eq!(
        counting.reads, 1,
        "footer, metadata and root page come in one read"
    );
    assert_eq!(counting.seeks, [SeekFrom::End(0), SeekFrom::Start(0)]);
    assert!(length < viewer_format::TAIL_READ_BYTES);
}

/// Same metadata and blocks written as v7.4-pre and v7.5-pre read back identically when every
/// page is read; nodes are compared by (level, prefix), not by `NodeId`.
#[test]
fn all_pages_read_equals_v7_4_by_level_and_prefix() {
    let tree = random_tree(77, 8, 14);
    let (old, old_attributes) = write_pair(V74, &tree, None);
    let (new, new_attributes) = write_pair(V75, &tree, Some(4_000));
    let mut old_reader = open_pair(old.clone(), old_attributes);
    let mut new_reader = open_pair(new.clone(), new_attributes);

    type Summary = (
        u64,
        u64,
        [f64; 3],
        [f64; 3],
        f64,
        Vec<viewer_format::NodeAttributeRange>,
    );
    let summarize = |reader: &FormatReader<Cursor<Vec<u8>>>| {
        let metadata = reader.format_metadata();
        let frame = metadata.global_frame().unwrap();
        let hierarchy = metadata.dataset().hierarchy();
        let root = hierarchy.node(hierarchy.root_id()).unwrap();
        let mut map = BTreeMap::new();
        for node in hierarchy.query(root.bounds()) {
            let cell = frame.node_cell(node.id()).unwrap();
            let level = frame.root_level() + node.level();
            let summary: Summary = (
                node.point_count(),
                node.blocks().len() as u64,
                node.bounds().min(),
                node.bounds().max(),
                node.geometric_error(),
                metadata.node_attribute_ranges(node.id()).to_vec(),
            );
            map.insert((level, cell.prefix()), (summary, cell.spacing(), node.id()));
        }
        map
    };
    let old_nodes = summarize(&old_reader);
    let new_nodes = summarize(&new_reader);
    assert_eq!(old_nodes.len(), tree.len());
    assert_eq!(
        old_nodes.keys().collect::<Vec<_>>(),
        new_nodes.keys().collect::<Vec<_>>()
    );
    for (key, (summary, spacing, _)) in &old_nodes {
        let (new_summary, new_spacing, new_id) = &new_nodes[key];
        assert_eq!(summary, new_summary, "{key:?}");
        assert_eq!(spacing, new_spacing, "{key:?}");
        assert!((split_node_id(*new_id).0 as usize) < tree.len());
    }
    // Same blocks, in the same order per node: positions, colors and attributes decode equal.
    let block_ids = |reader: &FormatReader<Cursor<Vec<u8>>>, id: NodeId| {
        reader
            .format_metadata()
            .dataset()
            .hierarchy()
            .node(id)
            .unwrap()
            .blocks()
            .to_vec()
    };
    for (key, (_, _, old_id)) in &old_nodes {
        let new_id = new_nodes[key].2;
        for (old_block, new_block) in block_ids(&old_reader, *old_id)
            .into_iter()
            .zip(block_ids(&new_reader, new_id))
        {
            assert_eq!(
                old_reader.read_positions(old_block).unwrap(),
                new_reader.read_positions(new_block).unwrap()
            );
            assert_eq!(
                old_reader.read_display_colors(old_block).unwrap(),
                new_reader.read_display_colors(new_block).unwrap()
            );
            assert_eq!(
                old_reader.read_attribute(old_block, 0).unwrap(),
                new_reader.read_attribute(new_block, 0).unwrap()
            );
            assert_eq!(
                old_reader.read_block_quanta(old_block).unwrap(),
                new_reader.read_block_quanta(new_block).unwrap()
            );
        }
    }
    // The paged reader sees the same nodes and stored block bytes.
    let mut paged = PagedReader::open(Cursor::new(new)).unwrap();
    let mut old_cursor = Cursor::new(old);
    let mut checked = 0;
    for (page, _) in all_pages(&mut paged) {
        for node in page.nodes() {
            let (summary, ..) = &old_nodes[&(node.level(), node.prefix())];
            assert_eq!(node.point_count(), summary.0);
            for block in node.blocks() {
                let stored = paged.read_block_bytes(block).unwrap();
                let mut expected = vec![0; block.payload_length() as usize];
                old_cursor.seek(SeekFrom::Start(block.offset())).unwrap();
                old_cursor.read_exact(&mut expected).unwrap();
                assert_eq!(stored, expected);
                checked += 1;
            }
        }
    }
    assert_eq!(checked, tree.len());
}

#[test]
fn paged_index_is_at_least_30_percent_smaller_than_v7_4() {
    let tree = random_tree(3, 8, 14);
    let old = FormatReader::new(Cursor::new(write(V74, &tree, None))).unwrap();
    let new_bytes = write(V75, &tree, None);
    let payloads: u64 = {
        let mut r = PagedReader::open(Cursor::new(new_bytes.clone())).unwrap();
        all_pages(&mut r)
            .iter()
            .flat_map(|(p, _)| {
                p.nodes()
                    .iter()
                    .flat_map(|n| n.blocks().iter().map(|b| b.payload_length()))
            })
            .sum()
    };
    let new_index = new_bytes.len() as u64 - 24 - 40 - payloads;
    let old_index = old.index_size_bytes();
    assert!(
        new_index * 10 <= old_index * 7,
        "v7.5 index {new_index} B vs v7.4 {old_index} B"
    );
}

// ---- Rejections ------------------------------------------------------------------------------

fn fix_page_crc(bytes: &mut [u8], start: usize, length: usize) {
    let crc = viewer_format::crc32(&bytes[start..start + length - 4]);
    bytes[start + length - 4..start + length].copy_from_slice(&crc.to_le_bytes());
}

/// A two-page file and the pointer, anchor and bytes of its only child page.
fn two_pages() -> (Vec<u8>, PageAnchor, viewer_format::PagePointer) {
    let tree = random_tree(21, 6, 0);
    let bytes = write(V75, &tree, None);
    let reader = PagedReader::open(Cursor::new(bytes.clone())).unwrap();
    let (anchor, pointer) = reader.root_page().child_pages().next().unwrap();
    (bytes, anchor, pointer)
}

fn read_child(
    bytes: Vec<u8>,
    anchor: PageAnchor,
    pointer: viewer_format::PagePointer,
) -> Result<(), FormatError> {
    let mut reader = PagedReader::open(Cursor::new(bytes))?;
    reader.read_child_page(anchor, pointer).map(|_| ())
}

#[test]
fn valid_chain_reads() {
    let (bytes, anchor, pointer) = two_pages();
    read_child(bytes, anchor, pointer).unwrap();
}

#[test]
fn page_with_a_bad_crc_is_rejected() {
    let (mut bytes, anchor, pointer) = two_pages();
    bytes[pointer.offset as usize + 45] ^= 1;
    assert!(matches!(
        read_child(bytes, anchor, pointer),
        Err(FormatError::InvalidChecksum("page"))
    ));
}

#[test]
fn child_page_must_be_anchored_at_the_pointing_node() {
    let (bytes, anchor, pointer) = two_pages();
    // The caller asks for a different anchor than the one the page was written for.
    let wrong = PageAnchor {
        level: anchor.level,
        prefix: anchor.prefix.map(|p| p ^ 1),
    };
    assert!(matches!(
        read_child(bytes.clone(), wrong, pointer),
        Err(FormatError::MalformedIndex(message)) if message.contains("anchor")
    ));
    // The page itself claims another anchor (CRC repaired).
    let mut tampered = bytes;
    tampered[pointer.offset as usize + 1] ^= 1;
    fix_page_crc(
        &mut tampered,
        pointer.offset as usize,
        pointer.length as usize,
    );
    assert!(matches!(
        read_child(tampered, anchor, pointer),
        Err(FormatError::MalformedIndex(message)) if message.contains("anchor")
    ));
}

#[test]
fn block_ids_may_not_repeat_across_pages() {
    let (mut bytes, anchor, pointer) = two_pages();
    // `first_block_id` sits at byte 30 of the page; 0 is the root page's first id.
    let start = pointer.offset as usize;
    bytes[start + 30..start + 38].copy_from_slice(&0_u64.to_le_bytes());
    fix_page_crc(&mut bytes, start, pointer.length as usize);
    assert!(matches!(
        read_child(bytes.clone(), anchor, pointer),
        Err(FormatError::MalformedIndex(message)) if message.contains("repeats")
    ));
    // The eager whole-file view refuses it too.
    assert!(FormatReader::new(Cursor::new(bytes)).is_err());
}

#[test]
fn block_payload_ranges_must_lie_in_the_data_section() {
    let (bytes, anchor, pointer) = two_pages();
    let reader = PagedReader::open(Cursor::new(bytes.clone())).unwrap();
    let mut reader = reader;
    let page = reader.read_child_page(anchor, pointer).unwrap();
    let offset = page.nodes()[0].blocks()[0].offset();
    let start = pointer.offset as usize;
    let mut tampered = bytes;
    let page_bytes = &mut tampered[start..start + pointer.length as usize];
    let at = page_bytes
        .windows(8)
        .position(|w| w == offset.to_le_bytes())
        .unwrap();
    // Points into the page section.
    page_bytes[at..at + 8].copy_from_slice(&pointer.offset.to_le_bytes());
    fix_page_crc(&mut tampered, start, pointer.length as usize);
    assert!(matches!(
        read_child(tampered, anchor, pointer),
        Err(FormatError::MalformedIndex(message)) if message.contains("outside the data section")
    ));
}

#[test]
fn child_pointer_outside_the_page_section_is_rejected() {
    let (mut bytes, ..) = two_pages();
    let reader = PagedReader::open(Cursor::new(bytes.clone())).unwrap();
    let root_length = reader.root_page().encoded_len();
    let pointer = reader.root_page().child_pages().next().unwrap().1;
    let root_start = bytes.len() - 40 - root_length;
    let at = bytes[root_start..]
        .windows(8)
        .position(|w| w == pointer.offset.to_le_bytes())
        .unwrap();
    bytes[root_start + at..root_start + at + 8].copy_from_slice(&5_u64.to_le_bytes());
    fix_page_crc(&mut bytes, root_start, root_length);
    repair_index_and_footer(&mut bytes);
    assert!(matches!(
        PagedReader::open(Cursor::new(bytes)),
        Err(FormatError::MalformedIndex(message)) if message.contains("pointer")
    ));
}

#[test]
fn page_level_count_must_be_one_to_five() {
    let (bytes, anchor, pointer) = two_pages();
    for k in [0_u8, 6] {
        let mut tampered = bytes.clone();
        let start = pointer.offset as usize;
        tampered[start + 25] = k;
        fix_page_crc(&mut tampered, start, pointer.length as usize);
        assert!(read_child(tampered, anchor, pointer).is_err(), "k = {k}");
    }
}

#[test]
fn page_size_and_resident_limits_apply() {
    let bytes = write(V75, &random_tree(21, 6, 0), None);
    let tiny = FormatReaderLimits::default()
        .with_page_limits(64, 4)
        .unwrap();
    assert!(matches!(
        PagedReader::with_limits(Cursor::new(bytes.clone()), tiny),
        Err(FormatError::PageExceedsLimit { .. })
    ));
    assert!(
        FormatReaderLimits::default()
            .with_page_limits(0, 4)
            .is_err()
    );
    let limits = FormatReaderLimits::default()
        .with_page_limits(1 << 20, 2)
        .unwrap();
    let reader = PagedReader::with_limits(Cursor::new(bytes), limits).unwrap();
    assert!(reader.check_resident_pages(2).is_ok());
    assert!(matches!(
        reader.check_resident_pages(3),
        Err(FormatError::CountLimit {
            kind: "resident pages",
            ..
        })
    ));
}

#[test]
fn released_pages_can_be_read_again_but_resident_ones_keep_their_ids() {
    let (bytes, anchor, pointer) = two_pages();
    let mut reader = PagedReader::open(Cursor::new(bytes)).unwrap();
    reader.read_child_page(anchor, pointer).unwrap();
    reader.read_child_page(anchor, pointer).unwrap();
    reader.release_page(pointer);
    reader.read_child_page(anchor, pointer).unwrap();
}

#[test]
fn v7_5_rejects_monolithic_output() {
    let tree = random_tree(2, 2, 10);
    let (metadata, _) = dataset(&tree);
    let options = FormatWriterOptions::new(0.01, 1 << 24).unwrap();
    assert!(
        FormatWriter::new_for_version(V75, Cursor::new(Vec::new()), metadata, options).is_err()
    );
}

#[test]
fn oct_opens_alone_and_companion_must_match_the_index() {
    let tree = random_tree(8, 5, 20);
    let (primary, attributes) = write_pair(V75, &tree, Some(3_000));
    // Header roles: split primary and companion.
    assert_eq!(&attributes[..8], b"OCTVFRM1");
    assert_eq!(
        &attributes[attributes.len() - 40..attributes.len() - 32],
        b"OCTVE75P"
    );
    let mut alone = FormatReader::new(Cursor::new(primary.clone())).unwrap();
    assert!(!alone.has_attributes());
    let block = alone
        .format_metadata()
        .dataset()
        .hierarchy()
        .node(alone.format_metadata().dataset().hierarchy().root_id())
        .unwrap()
        .blocks()[0];
    assert_eq!(alone.read_positions(block).unwrap().len(), 2);
    assert!(alone.read_display_colors(block).is_err());
    let mut attached = open_pair(primary.clone(), attributes.clone());
    assert_eq!(
        attached.read_display_colors(block).unwrap(),
        vec![ColorRgb::new(1, 2, 3); 2]
    );
    // A companion of another file, or a damaged one, is not accepted.
    let (_, other) = write_pair(V75, &random_tree(9, 5, 20), Some(3_000));
    let mut reader = FormatReader::new(Cursor::new(primary.clone())).unwrap();
    assert!(reader.attach_attributes(Cursor::new(other)).is_err());
    let mut short = attributes.clone();
    short.pop();
    assert!(reader.attach_attributes(Cursor::new(short)).is_err());
}

#[test]
fn attribute_columns_must_tile_the_companion() {
    let (bytes, anchor, pointer) = two_pages();
    // The attribute offset is the second field of the page's first block record.
    let start = pointer.offset as usize;
    let mut reader = PagedReader::open(Cursor::new(bytes.clone())).unwrap();
    let page = reader.read_child_page(anchor, pointer).unwrap();
    let original = page.nodes()[0].blocks()[0].attribute_offset();
    let mut tampered = bytes;
    let page_bytes = &mut tampered[start..start + pointer.length as usize];
    let at = page_bytes
        .windows(8)
        .position(|w| w == original.to_le_bytes())
        .unwrap();
    page_bytes[at..at + 8].copy_from_slice(&(original + 1).to_le_bytes());
    fix_page_crc(&mut tampered, start, pointer.length as usize);
    // One page alone still decodes; the whole-file view finds the gap.
    read_child(tampered.clone(), anchor, pointer).unwrap();
    assert!(matches!(
        FormatReader::new(Cursor::new(tampered)),
        Err(FormatError::MalformedIndex(message)) if message.contains("attribute blocks")
    ));
}

// ---- Corruption sweeps -----------------------------------------------------------------------

/// Opens through both readers and walks every page; errors are fine, panics are not.
fn exercise(bytes: &[u8]) -> bool {
    let eager = FormatReader::new(Cursor::new(bytes.to_vec())).is_ok();
    if let Ok(mut reader) = PagedReader::open(Cursor::new(bytes.to_vec())) {
        let mut pending: Vec<_> = reader.root_page().child_pages().collect();
        while let Some((anchor, pointer)) = pending.pop() {
            if let Ok(page) = reader.read_child_page(anchor, pointer) {
                pending.extend(page.child_pages());
            }
        }
    }
    eager
}

#[test]
fn truncation_at_every_prefix_is_an_error() {
    let bytes = write(V75, &random_tree(21, 6, 0), None);
    for length in 0..bytes.len() {
        assert!(
            !exercise(&bytes[..length]),
            "prefix of {length} bytes opened"
        );
        assert!(PagedReader::open(Cursor::new(bytes[..length].to_vec())).is_err());
    }
}

#[test]
fn bit_flips_with_repaired_checksums_never_panic() {
    let pristine = write(V75, &random_tree(21, 6, 0), None);
    assert!(exercise(&pristine));
    let reader = PagedReader::open(Cursor::new(pristine.clone())).unwrap();
    let pages_start = reader.header().pages_start() as usize;
    let root_length = reader.root_page().encoded_len();
    let child = reader.root_page().child_pages().next().unwrap().1;
    let (child_start, child_length) = (child.offset as usize, child.length as usize);
    let footer = pristine.len() - 40;
    let root_start = footer - root_length;
    let mut rejected = 0;
    for position in pages_start..footer {
        for bit in 0..8 {
            let mut bytes = pristine.clone();
            bytes[position] ^= 1 << bit;
            if position >= root_start
                || position < child_start + child_length && position >= child_start
            {
                let (start, length) = if position >= root_start {
                    (root_start, root_length)
                } else {
                    (child_start, child_length)
                };
                fix_page_crc(&mut bytes, start, length);
            }
            if position >= child_start + child_length {
                repair_index_and_footer(&mut bytes);
            }
            if !exercise(&bytes) {
                rejected += 1;
            }
        }
    }
    assert!(
        rejected > 100,
        "too few mutations were rejected: {rejected}"
    );
}

/// Lazy reader: grafting every page onto the root-page hierarchy gives the eager hierarchy, and
/// blocks of a page are readable only while that page is resident.
#[test]
fn lazy_reader_grows_to_the_eager_hierarchy_and_serves_resident_blocks() {
    let tree = random_tree(9, 7, 20);
    let (primary, attributes) = write_pair(V75, &tree, Some(2_000));
    let mut eager = open_pair(primary.clone(), attributes.clone());
    let eager_dataset = eager.format_metadata().viewer_dataset().unwrap();

    let mut lazy =
        FormatReader::open_lazy(Cursor::new(primary), FormatReaderLimits::default()).unwrap();
    lazy.attach_attributes(Cursor::new(attributes)).unwrap();
    let root_only = lazy.format_metadata().viewer_dataset().unwrap();
    let mut hierarchy = root_only.hierarchy().clone();
    let source = std::sync::Arc::clone(lazy.page_source().unwrap());
    let mut pending: Vec<NodeId> = source.root_unloaded().to_vec();
    for id in &pending {
        hierarchy.mark_unloaded(*id).unwrap();
    }
    assert!(hierarchy.stats().node_count < eager_dataset.hierarchy().stats().node_count);
    // A block of a page that is not loaded is unknown.
    let unknown = eager_dataset
        .hierarchy()
        .block_ids()
        .iter()
        .copied()
        .find(|id| !hierarchy.references_block(*id))
        .unwrap();
    assert!(matches!(
        lazy.read_geometry(unknown),
        Err(FormatError::UnknownBlock(_))
    ));
    let mut loaded = Vec::new();
    while let Some(anchor) = pending.pop() {
        let graft = lazy.read_page(anchor).unwrap();
        assert_eq!(graft.anchor, Some(anchor));
        hierarchy
            .graft_page(anchor, graft.nodes.clone(), &graft.unloaded)
            .unwrap();
        pending.extend(graft.unloaded.iter().copied());
        loaded.push(anchor);
    }
    let eager_hierarchy = eager_dataset.hierarchy();
    assert_eq!(
        hierarchy.stats().node_count,
        eager_hierarchy.stats().node_count
    );
    for id in eager_hierarchy
        .query(
            eager_hierarchy
                .node(eager_hierarchy.root_id())
                .unwrap()
                .bounds(),
        )
        .map(|n| n.id())
        .collect::<Vec<_>>()
    {
        let (a, b) = (
            hierarchy.node(id).unwrap(),
            eager_hierarchy.node(id).unwrap(),
        );
        assert_eq!(
            (a.bounds(), a.level(), a.parent()),
            (b.bounds(), b.level(), b.parent())
        );
        assert_eq!(a.blocks(), b.blocks());
        assert_eq!(a.spacing(), b.spacing());
        for block in a.blocks() {
            let lazy_block = lazy.read_geometry(*block).unwrap();
            let eager_block = eager.read_geometry(*block).unwrap();
            assert_eq!(lazy_block, eager_block);
        }
    }
    // Releasing a page frees its blocks and lets the page be read again.
    let anchor = *loaded.last().unwrap();
    let block = hierarchy
        .node(hierarchy.child_ids(anchor)[0])
        .unwrap()
        .blocks()[0];
    source.release_page(anchor);
    assert!(matches!(
        lazy.read_geometry(block),
        Err(FormatError::UnknownBlock(_))
    ));
    lazy.read_page(anchor).unwrap();
    lazy.read_geometry(block).unwrap();
}

/// The global attribute range is in the metadata block: a lazy reader that holds only the root
/// page reports the range of the whole file, not of its own nodes.
#[test]
fn lazy_attribute_range_covers_pages_that_are_not_loaded() {
    let tree = random_tree(9, 7, 20);
    let (primary, attributes) = write_pair(V75, &tree, Some(2_000));
    let eager = open_pair(primary.clone(), attributes);
    let lazy =
        FormatReader::open_lazy(Cursor::new(primary), FormatReaderLimits::default()).unwrap();
    assert!(!lazy.page_source().unwrap().root_unloaded().is_empty());
    let expected = eager.attribute_range(0, 0).unwrap();
    assert!(expected.is_some());
    assert_eq!(lazy.attribute_range(0, 0).unwrap(), expected);
    assert!(matches!(
        lazy.attribute_range(0, 1),
        Err(FormatError::InvalidSchema)
    ));
}

/// RF-05: the reference spacing is the point-weighted median leaf spacing, stored in the metadata
/// block, so a lazy reader reports it before any page is loaded and it never changes as pages
/// are grafted.
#[test]
fn reference_spacing_is_stored_and_stable_while_pages_are_grafted() {
    let tree = random_tree(9, 7, 20);
    let (primary, attributes) = write_pair(V75, &tree, Some(2_000));
    let eager = open_pair(primary.clone(), attributes);
    let mut paged = PagedReader::open(Cursor::new(primary.clone())).unwrap();
    let leaves: Vec<(u64, f64)> = all_pages(&mut paged)
        .iter()
        .flat_map(|(page, _)| page.nodes().to_vec())
        .filter(|node| !node.has_children())
        .map(|node| (node.point_count(), node.spacing()))
        .collect();
    let expected = viewer_core::reference_spacing(leaves).unwrap();
    assert_eq!(eager.reference_spacing(), Some(expected));

    let mut lazy =
        FormatReader::open_lazy(Cursor::new(primary), FormatReaderLimits::default()).unwrap();
    let source = std::sync::Arc::clone(lazy.page_source().unwrap());
    assert_eq!(source.reference_spacing(), expected);
    let mut pending: Vec<NodeId> = source.root_unloaded().to_vec();
    assert!(!pending.is_empty());
    while let Some(anchor) = pending.pop() {
        pending.extend(lazy.read_page(anchor).unwrap().unloaded);
        assert_eq!(source.reference_spacing(), expected);
        assert_eq!(lazy.reference_spacing(), Some(expected));
    }
}

fn patch_reference_spacing(bytes: &mut [u8], value: f64) {
    let footer = bytes.len() - 40;
    let index = u64::from_le_bytes(bytes[footer + 8..footer + 16].try_into().unwrap()) as usize;
    let length = u64::from_le_bytes(bytes[footer + 16..footer + 24].try_into().unwrap()) as usize;
    let reader = PagedReader::open(Cursor::new(bytes.to_vec())).unwrap();
    let stored = reader.header().reference_spacing().to_le_bytes();
    let at = bytes[index..index + length]
        .windows(8)
        .position(|window| window == stored)
        .unwrap()
        + index;
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    repair_index_and_footer(bytes);
}

#[test]
fn reference_spacing_is_validated_and_checked_against_the_leaves() {
    let tree = random_tree(9, 7, 20);
    let primary = write_pair(V75, &tree, Some(2_000)).0;
    let stored = PagedReader::open(Cursor::new(primary.clone()))
        .unwrap()
        .header()
        .reference_spacing();
    assert!(stored > 0.0 && stored.is_finite());
    for bad in [f64::NAN, f64::INFINITY, 0.0, -stored] {
        let mut bytes = primary.clone();
        patch_reference_spacing(&mut bytes, bad);
        assert!(PagedReader::open(Cursor::new(bytes)).is_err(), "{bad}");
    }
    // Well formed but not the median of the leaves: only readers that hold every page see it.
    let mut bytes = primary;
    patch_reference_spacing(&mut bytes, stored * 2.0);
    assert!(PagedReader::open(Cursor::new(bytes.clone())).is_ok());
    assert!(matches!(
        FormatReader::new(Cursor::new(bytes)),
        Err(FormatError::MalformedIndex(_))
    ));
}
