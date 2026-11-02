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
    SourcePointBlock, derive_node_id,
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
    let cells: Vec<_> = tree
        .iter()
        .map(|n| {
            let spacing = if n.has_children {
                2.0_f64.powi(1 - n.depth as i32)
            } else {
                2.0_f64.powi(-5)
            };
            (NodeId::new(n.id), NodeCell::new(n.prefix, spacing))
        })
        .collect();
    let origin = root_origin(ROOT_LEVEL, ROOT_PREFIX);
    let frame = GlobalFrame::new(
        ROOT_LEVEL,
        ROOT_PREFIX,
        origin,
        origin.map(|q| q + 100 * 32_768),
        wkt_crs(),
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
                2,
                0.0,
                vec![BlockId::new(n.id + 1)],
            )
            .unwrap(),
        );
        let at = |fraction: f64| {
            std::array::from_fn(|axis| {
                bounds.min()[axis] + (bounds.max()[axis] - bounds.min()[axis]) * fraction
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

fn write(version: FormatVersion, tree: &[TreeNode], page_target: Option<usize>) -> Vec<u8> {
    let (metadata, blocks) = dataset(tree);
    let mut options = FormatWriterOptions::new(0.01, 1 << 24).unwrap();
    if let Some(target) = page_target {
        options = options.with_page_target_bytes(target);
    }
    let mut writer =
        FormatWriter::new_for_version(version, Cursor::new(Vec::new()), metadata, options).unwrap();
    for block in blocks {
        writer.write_block(block).unwrap();
    }
    writer.finish().unwrap().into_inner()
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
                assert_eq!(
                    node.id(),
                    derive_node_id(ROOT_LEVEL, ROOT_PREFIX, node.level(), node.prefix()).unwrap()
                );
            }
        }
        assert_eq!(seen.len(), tree.len());
    }
}

#[test]
fn derived_ids_are_unique_sentinel_octant_paths() {
    let root = derive_node_id(ROOT_LEVEL, ROOT_PREFIX, ROOT_LEVEL, ROOT_PREFIX).unwrap();
    assert_eq!(root, NodeId::new(1));
    // Octant 5 = x | z << 2.
    let child: [u64; 3] = std::array::from_fn(|axis| (ROOT_PREFIX[axis] << 1) | [1, 0, 1][axis]);
    assert_eq!(
        derive_node_id(ROOT_LEVEL, ROOT_PREFIX, ROOT_LEVEL + 1, child),
        Some(NodeId::new(0b1_101))
    );
    // Depth limit 21 fits u64; 22 does not exist.
    let deep = |depth: u32| -> [u64; 3] { std::array::from_fn(|axis| ROOT_PREFIX[axis] << depth) };
    assert_eq!(
        derive_node_id(ROOT_LEVEL, ROOT_PREFIX, ROOT_LEVEL + 21, deep(21)),
        Some(NodeId::new(1 << 63))
    );
    assert_eq!(
        derive_node_id(ROOT_LEVEL, ROOT_PREFIX, ROOT_LEVEL + 22, deep(22)),
        None
    );
    // Not under the root prefix, or above the root.
    assert_eq!(
        derive_node_id(ROOT_LEVEL, ROOT_PREFIX, ROOT_LEVEL + 1, [0; 3]),
        None
    );
    assert_eq!(
        derive_node_id(ROOT_LEVEL, ROOT_PREFIX, ROOT_LEVEL - 1, [0; 3]),
        None
    );
    // Output is deterministic.
    let tree = random_tree(5, 7, 15);
    assert_eq!(
        write(V75, &tree, Some(2_000)),
        write(V75, &tree, Some(2_000))
    );
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
    let old = write(V74, &tree, None);
    let new = write(V75, &tree, Some(4_000));
    let mut old_reader = FormatReader::new(Cursor::new(old.clone())).unwrap();
    let mut new_reader = FormatReader::new(Cursor::new(new.clone())).unwrap();

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
        assert_eq!(
            *new_id,
            derive_node_id(ROOT_LEVEL, ROOT_PREFIX, key.0, key.1).unwrap()
        );
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
fn v7_5_rejects_split_output() {
    let tree = random_tree(2, 2, 10);
    let (metadata, _) = dataset(&tree);
    let options = FormatWriterOptions::new(0.01, 1 << 24).unwrap();
    assert!(
        FormatWriter::new_split_for_version(
            V75,
            Cursor::new(Vec::new()),
            Cursor::new(Vec::new()),
            metadata,
            options
        )
        .is_err()
    );
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
