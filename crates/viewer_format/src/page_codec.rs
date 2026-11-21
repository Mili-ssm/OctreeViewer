//! Page codec of the paged layouts (`v7.5-pre` and `v7`) ([spec](../../../docs/reference/format/FORMAT_V7_PRE5.md)): node
//! identity as `(page number, index in the page)`, and encode/decode/validation of one page.

use viewer_core::{AttributeDescriptor, BlockId, NodeId};

use crate::global_frame::{BLOCK_SIDE_EXPONENT, CellBox, GlobalFrame, power_of_two_exponent};
use crate::index_codec::{IndexWriter, validate_column_codec};
use crate::layout::{ColumnRules, HEADER_SIZE, PayloadDirectoryEntry, StorageLayout, StoredColumn};
use crate::reader::read_attribute_values;
use crate::reader_limits::FormatReaderLimits;
use crate::slice_cursor::SliceCursor;
use crate::{BlockCodec, Crc32, FormatError, GeometryEncoding, NodeAttributeRange};

/// Deepest page: a page spans 1..=5 octree levels below its anchor.
pub const MAX_PAGE_LEVELS: u8 = 5;
/// The writer picks the largest `k` whose page fits this size.
pub const TARGET_PAGE_BYTES: usize = 512 * 1024;
const MAX_LEVEL: u32 = 64;
const SPLIT_BELOW_LEVEL: u32 = MAX_LEVEL - BLOCK_SIDE_EXPONENT;
const MIN_SPACING_EXPONENT: i32 = -15;
const SIDE_EXPONENT_BASE: i32 = 49;
const INTERNAL_SPACING_EXPONENT_BASE: i32 = 41;
/// Anchor level 1, prefix 24, `k` 1, node count 4, first block id 8.
const PAGE_HEADER_LEN: usize = 38;
const CRC_LEN: usize = 4;
/// Depth 1, path 2, flags 1, point count 8, geometric error 8, block count 4, range count 2.
const MIN_NODE_RECORD_LEN: usize = 26;
/// Geometry offset 8, attribute offset 8, point count 8, geometry tag 1, error 8, plus the geometry
/// payload entry.
const MIN_BLOCK_RECORD_LEN: usize = 33;
/// Smallest valid page: header, one node, CRC.
pub(crate) const MIN_PAGE_LEN: usize = PAGE_HEADER_LEN + MIN_NODE_RECORD_LEN + CRC_LEN;
/// A page holds at most `8 + ... + 8^5` nodes below its anchor (plus the root node in the root page).
const MAX_NODES_PER_PAGE: u32 = 37_448;

/// Absolute level and per-axis prefix of the node a page hangs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageAnchor {
    pub level: u32,
    pub prefix: [u64; 3],
}

/// Where a child page lives: absolute file offset, byte length and page number. The page number
/// is stored; `anchor_node` is not: the decoder fills in the id of the node that holds the
/// pointer, which the child page needs to resolve the parent of its top nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagePointer {
    pub offset: u64,
    pub length: u32,
    pub page: u32,
    pub anchor_node: NodeId,
}

/// `NodeId` of the node at `index` (position in page order) of page number `page`.
pub const fn page_node_id(page: u32, index: u32) -> NodeId {
    NodeId::new(((page as u64) << 32) | index as u64)
}

/// `(page number, index in the page)` of a `v7.5-pre` node id.
pub const fn split_node_id(id: NodeId) -> (u32, u32) {
    let value = id.get();
    ((value >> 32) as u32, value as u32)
}

/// One block record, inline in its node.
#[derive(Debug, Clone)]
pub struct PageBlock {
    pub(crate) id: BlockId,
    pub(crate) offset: u64,
    /// Geometry length in the `.oct`: the stored length of the geometry column.
    pub(crate) payload_length: u64,
    /// Start of the block's colour and attribute columns in the `.octattrs`.
    pub(crate) attribute_offset: u64,
    pub(crate) point_count: u64,
    pub(crate) geometry_encoding: GeometryEncoding,
    pub(crate) quantization_error: f64,
    pub(crate) origin_quanta: [i64; 3],
    pub(crate) payloads: Vec<PayloadDirectoryEntry>,
}

impl PageBlock {
    pub fn id(&self) -> BlockId {
        self.id
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn payload_length(&self) -> u64 {
        self.payload_length
    }

    pub fn attribute_offset(&self) -> u64 {
        self.attribute_offset
    }

    pub fn point_count(&self) -> u64 {
        self.point_count
    }

    pub fn quantization_error(&self) -> f64 {
        self.quantization_error
    }

    pub fn origin_quanta(&self) -> [i64; 3] {
        self.origin_quanta
    }

    /// Stored (compressed) byte length of each payload column, geometry first.
    /// End of the block's columns in the `.octattrs`.
    pub(crate) fn attribute_end(&self) -> u64 {
        self.attribute_offset
            + self
                .payloads
                .iter()
                .skip(1)
                .map(|payload| payload.stored_length)
                .sum::<u64>()
    }

    pub fn stored_lengths(&self) -> Vec<u64> {
        self.payloads.iter().map(|p| p.stored_length).collect()
    }
}

/// One node of a page. `depth` is relative to the anchor; ids are derived, never stored.
#[derive(Debug, Clone)]
pub struct PageNode {
    pub(crate) id: NodeId,
    pub(crate) parent: Option<NodeId>,
    pub(crate) depth: u8,
    pub(crate) path: u16,
    pub(crate) level: u32,
    pub(crate) prefix: [u64; 3],
    pub(crate) has_children: bool,
    pub(crate) point_count: u64,
    pub(crate) geometric_error: f64,
    pub(crate) spacing: f64,
    pub(crate) blocks: Vec<PageBlock>,
    pub(crate) ranges: Vec<NodeAttributeRange>,
    pub(crate) child_page: Option<PagePointer>,
}

impl PageNode {
    pub fn id(&self) -> NodeId {
        self.id
    }

    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    /// Depth below the page anchor (0 only for the root node in the root page).
    pub fn depth(&self) -> u8 {
        self.depth
    }

    pub fn path(&self) -> u16 {
        self.path
    }

    pub fn level(&self) -> u32 {
        self.level
    }

    pub fn prefix(&self) -> [u64; 3] {
        self.prefix
    }

    pub fn has_children(&self) -> bool {
        self.has_children
    }

    pub fn point_count(&self) -> u64 {
        self.point_count
    }

    pub fn geometric_error(&self) -> f64 {
        self.geometric_error
    }

    pub fn spacing(&self) -> f64 {
        self.spacing
    }

    pub fn blocks(&self) -> &[PageBlock] {
        &self.blocks
    }

    pub fn attribute_ranges(&self) -> &[NodeAttributeRange] {
        &self.ranges
    }

    /// Pointer to the page anchored at this node; only depth-`k` nodes with children have one.
    pub fn child_page(&self) -> Option<PagePointer> {
        self.child_page
    }

    pub fn anchor(&self) -> PageAnchor {
        PageAnchor {
            level: self.level,
            prefix: self.prefix,
        }
    }
}

/// A decoded, validated page.
#[derive(Debug, Clone)]
pub struct Page {
    pub(crate) number: u32,
    pub(crate) anchor: PageAnchor,
    pub(crate) k: u8,
    pub(crate) is_root: bool,
    pub(crate) first_block_id: u64,
    pub(crate) nodes: Vec<PageNode>,
    pub(crate) encoded_len: usize,
}

impl Page {
    /// Page number: 0 for the root page, then the writer's plan order (breadth-first).
    pub fn number(&self) -> u32 {
        self.number
    }

    pub fn anchor(&self) -> PageAnchor {
        self.anchor
    }

    pub fn levels(&self) -> u8 {
        self.k
    }

    pub fn is_root(&self) -> bool {
        self.is_root
    }

    pub fn nodes(&self) -> &[PageNode] {
        &self.nodes
    }

    pub fn encoded_len(&self) -> usize {
        self.encoded_len
    }

    /// Ids `first..first + block_count` belong to this page, in node order.
    pub fn block_id_range(&self) -> (u64, u64) {
        let count = self
            .nodes
            .iter()
            .map(|n| n.blocks.len() as u64)
            .sum::<u64>();
        (
            self.first_block_id,
            self.first_block_id.saturating_add(count),
        )
    }

    /// Anchors and pointers of the pages that hang from this page.
    pub fn child_pages(&self) -> impl Iterator<Item = (PageAnchor, PagePointer)> + '_ {
        self.nodes
            .iter()
            .filter_map(|node| node.child_page.map(|pointer| (node.anchor(), pointer)))
    }
}

/// Which page the caller expects: the root, or the child anchored where a pointer said.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PageExpect {
    Root,
    Child {
        anchor: PageAnchor,
        page: u32,
        anchor_node: NodeId,
    },
}

/// Everything page validation needs from the metadata block.
pub(crate) struct PageContext<'a> {
    pub(crate) frame: &'a GlobalFrame,
    pub(crate) schema: &'a [AttributeDescriptor],
    pub(crate) limits: &'a FormatReaderLimits,
    pub(crate) pages_start: u64,
    pub(crate) index_offset: u64,
    pub(crate) layout: StorageLayout,
    /// The header's LOD column set and display colour source.
    pub(crate) rules: ColumnRules<'a>,
}

fn prefix_fits(level: u32, prefix: u64) -> bool {
    prefix.checked_shr(level).unwrap_or(0) == 0
}

/// Whether `(level, prefix)` lies at or below the root node.
fn under_root(root_level: u32, root_prefix: [u64; 3], level: u32, prefix: [u64; 3]) -> bool {
    let Some(depth) = level.checked_sub(root_level) else {
        return false;
    };
    level <= MAX_LEVEL
        && (0..3).all(|axis| {
            prefix_fits(level, prefix[axis])
                && prefix[axis].checked_shr(depth).unwrap_or(0) == root_prefix[axis]
        })
}

/// Prefix of the node at `depth`/`path` below `anchor`.
pub(crate) fn path_prefix(anchor: PageAnchor, depth: u8, path: u16) -> [u64; 3] {
    let mut prefix = anchor.prefix;
    for step in (0..depth).rev() {
        let octant = u64::from(path >> (3 * step)) & 7;
        for (axis, value) in prefix.iter_mut().enumerate() {
            *value = (*value << 1) | ((octant >> axis) & 1);
        }
    }
    prefix
}

/// Path of a node under `anchor` from its prefix (inverse of [`path_prefix`]).
pub(crate) fn prefix_path(prefix: [u64; 3], depth: u8) -> u16 {
    let mut path = 0_u16;
    for step in (0..depth).rev() {
        let octant = (0..3).fold(0_u16, |bits, axis| {
            bits | ((((prefix[axis] >> step) & 1) as u16) << axis)
        });
        path = (path << 3) | octant;
    }
    path
}

fn spacing_from_exponent(exponent: i32) -> f64 {
    f64::from_bits(((exponent + 1023) as u64) << 52)
}

fn internal_spacing(level: u32) -> Option<f64> {
    let exponent = INTERNAL_SPACING_EXPONENT_BASE - level as i32;
    (exponent >= MIN_SPACING_EXPONENT).then(|| spacing_from_exponent(exponent))
}

fn leaf_spacing(level: u32, exponent: i8) -> Result<f64, FormatError> {
    let exponent = i32::from(exponent);
    if exponent < MIN_SPACING_EXPONENT || exponent > SIDE_EXPONENT_BASE - level as i32 {
        return Err(FormatError::MalformedIndex(
            "page spacing is outside 2^-15 .. node side",
        ));
    }
    Ok(spacing_from_exponent(exponent))
}

fn node_origin_quanta(frame: &GlobalFrame, level: u32, prefix: [u64; 3]) -> Option<CellBox> {
    CellBox::in_lattice(frame.lattice_base(), level, prefix)
}

/// Decodes and validates one page; see the spec's "Page validation".
pub(crate) fn decode_page(
    bytes: &[u8],
    ctx: &PageContext<'_>,
    expect: PageExpect,
) -> Result<Page, FormatError> {
    if bytes.len() < MIN_PAGE_LEN {
        return Err(FormatError::TruncatedIndex);
    }
    if bytes.len() as u64 > ctx.limits.max_page_bytes {
        return Err(FormatError::PageExceedsLimit {
            requested: bytes.len() as u64,
            limit: ctx.limits.max_page_bytes,
        });
    }
    let (body, crc) = bytes.split_at(bytes.len() - CRC_LEN);
    if !crate::crc32::checksum_matches(body, u32::from_le_bytes(crc.try_into().expect("4 bytes"))) {
        return Err(FormatError::InvalidChecksum("page"));
    }
    let mut cursor = SliceCursor::new(body);
    let anchor = PageAnchor {
        level: u32::from(cursor.u8()?),
        prefix: [cursor.u64()?, cursor.u64()?, cursor.u64()?],
    };
    let k = cursor.u8()?;
    let node_count = cursor.u32()?;
    let first_block_id = cursor.u64()?;

    let (is_root, expected, number, anchor_node) = match expect {
        PageExpect::Root => (
            true,
            PageAnchor {
                level: ctx.frame.root_level(),
                prefix: ctx.frame.root_prefix(),
            },
            0,
            None,
        ),
        PageExpect::Child {
            anchor,
            page,
            anchor_node,
        } => (false, anchor, page, Some(anchor_node)),
    };
    if anchor != expected {
        return Err(FormatError::MalformedIndex(
            "page anchor does not match the node that points to it",
        ));
    }
    if k == 0 || k > MAX_PAGE_LEVELS || anchor.level + u32::from(k) > MAX_LEVEL {
        return Err(FormatError::MalformedIndex("page level count is invalid"));
    }
    let root_level = ctx.frame.root_level();
    if !under_root(
        root_level,
        ctx.frame.root_prefix(),
        anchor.level,
        anchor.prefix,
    ) {
        return Err(FormatError::MalformedIndex(
            "page anchor is outside the root prefix",
        ));
    }
    if node_count == 0
        || node_count > MAX_NODES_PER_PAGE + u32::from(is_root)
        || u64::from(node_count) > ctx.limits.max_nodes
        || (node_count as usize).saturating_mul(MIN_NODE_RECORD_LEN) > cursor.remaining()
    {
        return Err(FormatError::MalformedIndex("page node count is invalid"));
    }

    let schema = ctx.schema;
    let attribute_count = schema.len();
    let mut nodes: Vec<PageNode> = Vec::new();
    nodes
        .try_reserve_exact(node_count as usize)
        .map_err(|_| FormatError::AllocationFailed(node_count as usize))?;
    let mut previous_key: Option<(u8, u16)> = None;
    let mut child_counts: Vec<u32> = Vec::new();
    child_counts
        .try_reserve_exact(node_count as usize)
        .map_err(|_| FormatError::AllocationFailed(node_count as usize))?;
    let mut index_of = std::collections::BTreeMap::<(u8, u16), usize>::new();
    let mut next_block_id = first_block_id;
    let mut block_ranges: Vec<(u64, u64)> = Vec::new();
    let mut attribute_ranges: Vec<(u64, u64)> = Vec::new();
    let mut total_ranges = 0_u64;
    let first_depth = u8::from(!is_root);

    for position in 0..node_count as usize {
        let depth = cursor.u8()?;
        let path = cursor.u16()?;
        if depth < first_depth
            || depth > k
            || (is_root && position == 0) != (depth == 0)
            || u32::from(path) >= 1_u32 << (3 * u32::from(depth))
        {
            return Err(FormatError::MalformedIndex("page node key is invalid"));
        }
        if previous_key.is_some_and(|previous| previous >= (depth, path)) {
            return Err(FormatError::MalformedIndex(
                "page nodes are not uniquely sorted",
            ));
        }
        previous_key = Some((depth, path));
        let flags = cursor.u8()?;
        if flags & !1 != 0 {
            return Err(FormatError::MalformedIndex("page node reserved flags"));
        }
        let has_children = flags & 1 == 1;
        let level = anchor.level + u32::from(depth);
        let prefix = path_prefix(anchor, depth, path);
        let id = page_node_id(number, position as u32);
        let point_count = cursor.u64()?;
        let geometric_error = cursor.f64()?;
        if !geometric_error.is_finite() || geometric_error < 0.0 {
            return Err(FormatError::MalformedIndex("page node geometric error"));
        }
        let spacing = if has_children {
            internal_spacing(level).ok_or(FormatError::MalformedIndex(
                "internal node too deep for its spacing",
            ))?
        } else {
            leaf_spacing(level, cursor.i8()?)?
        };

        // Parent: a node of the page with children, or the anchor itself at depth 1 of a child page.
        let parent = if depth == 0 {
            None
        } else if depth > 1 || is_root {
            let parent_index =
                *index_of
                    .get(&(depth - 1, path >> 3))
                    .ok_or(FormatError::MalformedIndex(
                        "page node has no parent in the page",
                    ))?;
            if !nodes[parent_index].has_children {
                return Err(FormatError::MalformedIndex(
                    "page node under a node without children",
                ));
            }
            child_counts[parent_index] += 1;
            Some(page_node_id(number, parent_index as u32))
        } else {
            anchor_node
        };

        let block_count = cursor.u32()?;
        if u64::from(block_count) > ctx.limits.max_blocks
            || (block_count as usize).saturating_mul(MIN_BLOCK_RECORD_LEN) > cursor.remaining()
        {
            return Err(FormatError::CountLimit {
                kind: "node block references",
                requested: u64::from(block_count),
                limit: ctx.limits.max_blocks,
            });
        }
        let node_box = node_origin_quanta(ctx.frame, level, prefix).ok_or(
            FormatError::MalformedIndex("node origin exceeds the quanta range"),
        )?;
        let mut blocks = Vec::new();
        blocks
            .try_reserve_exact(block_count as usize)
            .map_err(|_| FormatError::AllocationFailed(block_count as usize))?;
        let mut block_points = 0_u64;
        let mut block_error = 0.0_f64;
        for _ in 0..block_count {
            let block = decode_block(
                &mut cursor,
                ctx,
                node_box,
                level,
                next_block_id,
                has_children,
            )?;
            next_block_id = next_block_id
                .checked_add(1)
                .ok_or(FormatError::MalformedIndex("page block ids overflow"))?;
            block_points = block_points
                .checked_add(block.point_count)
                .ok_or(FormatError::PointCountOverflow(id))?;
            block_error = block_error.max(block.quantization_error);
            block_ranges.push((block.offset, block.offset + block.payload_length));
            attribute_ranges.push((block.attribute_offset, block.attribute_end()));
            blocks.push(block);
        }
        if block_points != point_count {
            return Err(FormatError::NodePointCountMismatch {
                node: id,
                declared: point_count,
                blocks: block_points,
            });
        }
        if geometric_error < block_error {
            return Err(FormatError::MalformedIndex(
                "node geometric error excludes block quantization error",
            ));
        }

        let range_count = usize::from(cursor.u16()?);
        if range_count > attribute_count || (point_count == 0 && range_count != 0) {
            return Err(FormatError::MalformedIndex(
                "page node attribute ranges are invalid",
            ));
        }
        total_ranges = total_ranges.saturating_add(range_count as u64);
        if total_ranges > ctx.limits.max_attribute_ranges {
            return Err(FormatError::CountLimit {
                kind: "node attribute ranges",
                requested: total_ranges,
                limit: ctx.limits.max_attribute_ranges,
            });
        }
        let mut ranges = Vec::new();
        ranges
            .try_reserve_exact(range_count)
            .map_err(|_| FormatError::AllocationFailed(range_count))?;
        let mut previous_schema = None;
        for _ in 0..range_count {
            let schema_index = u32::from(cursor.u16()?);
            if schema_index as usize >= attribute_count
                || previous_schema.is_some_and(|previous| previous >= schema_index)
            {
                return Err(FormatError::MalformedIndex(
                    "node attribute ranges are not uniquely sorted",
                ));
            }
            previous_schema = Some(schema_index);
            let descriptor = &schema[schema_index as usize];
            let value_count = descriptor.component_count() as usize;
            let min = read_attribute_values(&mut cursor, descriptor.data_type(), value_count)?;
            let max = read_attribute_values(&mut cursor, descriptor.data_type(), value_count)?;
            ranges.push(NodeAttributeRange::new(schema_index, min, max));
        }

        let child_page = if depth == k && has_children {
            let pointer = PagePointer {
                offset: cursor.u64()?,
                length: cursor.u32()?,
                page: cursor.u32()?,
                anchor_node: id,
            };
            if pointer.page == 0
                || pointer.offset < ctx.pages_start
                || pointer
                    .offset
                    .checked_add(u64::from(pointer.length))
                    .is_none_or(|end| end > ctx.index_offset)
                || (pointer.length as usize) < MIN_PAGE_LEN
                || u64::from(pointer.length) > ctx.limits.max_page_bytes
            {
                return Err(FormatError::MalformedIndex(
                    "child page pointer is outside the page section",
                ));
            }
            Some(pointer)
        } else {
            None
        };

        index_of.insert((depth, path), nodes.len());
        child_counts.push(0);
        nodes.push(PageNode {
            id,
            parent,
            depth,
            path,
            level,
            prefix,
            has_children,
            point_count,
            geometric_error,
            spacing,
            blocks,
            ranges,
            child_page,
        });
    }
    if cursor.remaining() != 0 {
        return Err(FormatError::MalformedIndex("trailing page bytes"));
    }
    for (node, children) in nodes.iter().zip(&child_counts) {
        if node.has_children && node.depth < k && *children == 0 {
            return Err(FormatError::MalformedIndex(
                "page node claims children that are not in the page",
            ));
        }
    }
    for ranges in [&mut block_ranges, &mut attribute_ranges] {
        ranges.sort_unstable();
        if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(FormatError::MalformedIndex("page block payloads overlap"));
        }
    }
    Ok(Page {
        number,
        anchor,
        k,
        is_root,
        first_block_id,
        nodes,
        encoded_len: bytes.len(),
    })
}

fn decode_block(
    cursor: &mut SliceCursor<'_>,
    ctx: &PageContext<'_>,
    node_box: CellBox,
    level: u32,
    id: u64,
    block_is_lod: bool,
) -> Result<PageBlock, FormatError> {
    let layout = ctx.layout;
    let column_rule = |column: usize| layout.stored_column(block_is_lod, column, ctx.rules);
    let offset = cursor.u64()?;
    let attribute_offset = cursor.u64()?;
    let point_count = cursor.u64()?;
    let geometry_encoding = GeometryEncoding::try_from(cursor.u8()?)?;
    let quantization_error = cursor.f64()?;
    if !quantization_error.is_finite() || quantization_error < 0.0 {
        return Err(FormatError::MalformedIndex(
            "invalid block quantization error",
        ));
    }
    let origin_quanta = if level < SPLIT_BELOW_LEVEL {
        let mut origin = node_box.origin;
        for value in &mut origin {
            // A cell of a level-0 node can be 2^31 or more, so the shift cannot happen in `i64`.
            let cell = i128::from(cursor.u32()?) << BLOCK_SIDE_EXPONENT;
            *value = i64::try_from(i128::from(*value) + cell)
                .map_err(|_| FormatError::MalformedIndex("block cell exceeds the quanta range"))?;
        }
        if !crate::global_frame::block_origin_is_valid(node_box, origin) {
            return Err(FormatError::MalformedIndex(
                "block origin does not follow the node origin or split rule",
            ));
        }
        origin
    } else {
        node_box.origin
    };
    let logical = crate::layout::block_payload_size(
        usize::try_from(point_count).map_err(|_| FormatError::PointCountOutOfRange)?,
        ctx.schema,
        |column| column_rule(column).is_present(),
    )?;
    if logical > ctx.limits.max_block_bytes {
        return Err(FormatError::BlockExceedsLimit {
            requested: logical,
            limit: ctx.limits.max_block_bytes,
        });
    }
    if attribute_offset < HEADER_SIZE {
        return Err(FormatError::MalformedIndex(
            "attribute offset before header",
        ));
    }

    let mut payloads = Vec::new();
    payloads
        .try_reserve_exact(ctx.schema.len() + 2)
        .map_err(|_| FormatError::AllocationFailed(ctx.schema.len() + 2))?;
    // The geometry column starts the block in the `.oct`; the other columns follow each other in the
    // `.octattrs` from `attribute_offset`, so `expected_offset` is the running sum of their lengths.
    let mut expected_offset = 0_u64;
    for payload_index in 0..ctx.schema.len() + 2 {
        let stored = column_rule(payload_index);
        if stored != StoredColumn::Present {
            // No directory record: the slot keeps the offset the next stored column would have.
            let offset = if payload_index == 0 {
                0
            } else {
                attribute_offset
                    .checked_add(expected_offset)
                    .ok_or(FormatError::FileSizeOverflow)?
            };
            payloads.push(PayloadDirectoryEntry::not_stored(stored, offset));
            continue;
        }
        let codec = BlockCodec::try_from(cursor.u8()?)?;
        let checksum = cursor.u32()?;
        let stored_length = cursor.u64()?;
        let raw_length = match payload_index {
            0 => cursor.u64()?,
            1 => point_count
                .checked_mul(3)
                .ok_or(FormatError::BlockSizeOverflow)?,
            _ => {
                crate::layout::attribute_payload_size(point_count, &ctx.schema[payload_index - 2])?
            }
        };
        let column = PayloadDirectoryEntry {
            offset: if payload_index == 0 {
                0
            } else {
                attribute_offset
                    .checked_add(expected_offset)
                    .ok_or(FormatError::FileSizeOverflow)?
            },
            stored_length,
            raw_length,
            codec,
            checksum,
            stored,
        };
        validate_column_codec(ctx.schema, payload_index, point_count, &column)?;
        if (payload_index == 0
            && geometry_encoding
                .validate_raw_length(point_count, raw_length)
                .is_err())
            || (codec == BlockCodec::None && stored_length != raw_length)
            || (codec == BlockCodec::Lz4 && stored_length >= raw_length)
        {
            return Err(FormatError::MalformedIndex(
                "invalid payload range or length",
            ));
        }
        if payload_index > 0 {
            expected_offset = expected_offset
                .checked_add(stored_length)
                .ok_or(FormatError::FileSizeOverflow)?;
        }
        payloads.push(column);
    }
    if attribute_offset.checked_add(expected_offset).is_none() {
        return Err(FormatError::FileSizeOverflow);
    }
    let payload_length = payloads[0].stored_length;
    if payload_length > ctx.limits.max_block_bytes {
        return Err(FormatError::BlockExceedsLimit {
            requested: payload_length,
            limit: ctx.limits.max_block_bytes,
        });
    }
    if offset < HEADER_SIZE
        || offset
            .checked_add(payload_length)
            .is_none_or(|end| end > ctx.pages_start)
    {
        return Err(FormatError::MalformedIndex(
            "block payload range is outside the data section",
        ));
    }
    let total_raw = payloads.iter().try_fold(0_u64, |sum, payload| {
        sum.checked_add(payload.raw_length)
            .ok_or(FormatError::BlockSizeOverflow)
    })?;
    if total_raw > ctx.limits.max_block_bytes {
        return Err(FormatError::BlockExceedsLimit {
            requested: total_raw,
            limit: ctx.limits.max_block_bytes,
        });
    }
    Ok(PageBlock {
        id: BlockId::new(id),
        offset,
        payload_length,
        attribute_offset,
        point_count,
        geometry_encoding,
        quantization_error,
        origin_quanta,
        payloads,
    })
}

/// Encodes a page. Block ids are implied by `first_block_id` and node order, so `page` must list
/// them consecutively; the writer guarantees that.
pub(crate) fn encode_page(page: &Page, frame: &GlobalFrame) -> Result<Vec<u8>, FormatError> {
    let mut bytes = Vec::new();
    let mut out = IndexWriter {
        writer: &mut bytes,
        checksum: Crc32::new(),
        length: 0,
    };
    out.u8(u8::try_from(page.anchor.level).map_err(|_| FormatError::InvalidMetadata("level"))?)?;
    for prefix in page.anchor.prefix {
        out.u64(prefix)?;
    }
    out.u8(page.k)?;
    out.u32(u32::try_from(page.nodes.len()).map_err(|_| FormatError::TooManyNodes)?)?;
    out.u64(page.first_block_id)?;
    for node in &page.nodes {
        out.u8(node.depth)?;
        out.write(&node.path.to_le_bytes())?;
        out.u8(u8::from(node.has_children))?;
        out.u64(node.point_count)?;
        out.f64(node.geometric_error)?;
        if !node.has_children {
            let exponent = power_of_two_exponent(node.spacing).ok_or(
                FormatError::InvalidMetadata("spacing is not a power of two"),
            )?;
            out.u8(
                i8::try_from(exponent).map_err(|_| FormatError::InvalidMetadata("spacing"))? as u8,
            )?;
        }
        out.u32(
            u32::try_from(node.blocks.len())
                .map_err(|_| FormatError::TooManyNodeBlocks(node.id))?,
        )?;
        let node_box = node_origin_quanta(frame, node.level, node.prefix)
            .ok_or(FormatError::InvalidMetadata("node origin"))?;
        for block in &node.blocks {
            out.u64(block.offset)?;
            out.u64(block.attribute_offset)?;
            out.u64(block.point_count)?;
            out.u8(block.geometry_encoding as u8)?;
            out.f64(block.quantization_error)?;
            if node.level < SPLIT_BELOW_LEVEL {
                for axis in 0..3 {
                    let cell = (i128::from(block.origin_quanta[axis])
                        - i128::from(node_box.origin[axis]))
                        >> BLOCK_SIDE_EXPONENT;
                    out.u32(
                        u32::try_from(cell)
                            .map_err(|_| FormatError::InvalidMetadata("block cell"))?,
                    )?;
                }
            }
            for (index, payload) in block.payloads.iter().enumerate() {
                if !payload.stored.is_present() {
                    continue;
                }
                out.u8(payload.codec as u8)?;
                out.u32(payload.checksum)?;
                out.u64(payload.stored_length)?;
                if index == 0 {
                    out.u64(payload.raw_length)?;
                }
            }
        }
        out.write(
            &u16::try_from(node.ranges.len())
                .map_err(|_| FormatError::SchemaTooLarge)?
                .to_le_bytes(),
        )?;
        for range in &node.ranges {
            out.write(
                &u16::try_from(range.schema_index())
                    .map_err(|_| FormatError::SchemaTooLarge)?
                    .to_le_bytes(),
            )?;
            crate::index_codec::encode_index_values(&mut out, range.min())?;
            crate::index_codec::encode_index_values(&mut out, range.max())?;
        }
        if node.depth == page.k && node.has_children {
            let pointer = node.child_page.ok_or(FormatError::InvalidMetadata(
                "depth-k node without a child page",
            ))?;
            out.u64(pointer.offset)?;
            out.u32(pointer.length)?;
            out.u32(pointer.page)?;
        }
    }
    let crc = out.checksum.finish();
    bytes.extend_from_slice(&crc.to_le_bytes());
    Ok(bytes)
}
