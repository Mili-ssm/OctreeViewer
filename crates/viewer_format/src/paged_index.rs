//! `v7.5-pre` index: metadata block, root page and child pages
//! ([spec](../../../docs/reference/format/FORMAT_V7_PRE5.md)). Holds the writer path that plans and
//! emits pages, the page-granular [`PagedReader`], and the eager loader used by `FormatReader`.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};

use viewer_core::{
    AttributeDescriptor, BlockId, CoordinateFrame, DatasetMetadata, HierarchySemantics, NodeId,
    SpatialBounds, SpatialHierarchy, SpatialNode,
};

use crate::index_codec::{
    BlockDirectoryEntry, IndexPrefix, IndexWriter, ParsedIndex, footer_bytes, parse_index_prefix,
    parse_split_footer, validate_block_layout, validate_index_range, validate_node_error_bounds,
    validate_node_point_counts, write_index_prefix,
};
use crate::layout::{FOOTER_SIZE, HEADER_SIZE, StorageLayout};
use crate::metadata::validate_metadata;
use crate::page_codec::{
    MAX_PAGE_LEVELS, MIN_PAGE_LEN, Page, PageAnchor, PageBlock, PageContext, PageExpect, PageNode,
    PagePointer, decode_page, encode_page, page_node_id, prefix_path,
};
use crate::prepare::BlockRecord;
use crate::reader_limits::FormatReaderLimits;
use crate::slice_cursor::SliceCursor;
use crate::{
    Crc32, FormatError, FormatMetadata, GlobalFrame, NodeAttributeRange, NodeCell,
    crc32::checksum_matches,
};

const LAYOUT: StorageLayout = StorageLayout::V7Pre5;
/// How much of the file end a reader fetches first: footer, metadata block and root page.
pub const TAIL_READ_BYTES: u64 = 1024 * 1024;
const PAGE_HEADER_LEN: usize = 38;
const CRC_LEN: usize = 4;

/// The metadata block: everything of the index except the node table.
#[derive(Debug, Clone)]
pub struct PagedHeader {
    dataset_origin: [f64; 3],
    frame: GlobalFrame,
    crs_label: Option<String>,
    source_point_count: Option<u64>,
    sampling_policy: Option<String>,
    semantics: HierarchySemantics,
    visual_scale: f64,
    schema: Vec<AttributeDescriptor>,
    total_nodes: u64,
    total_blocks: u64,
    pages_start: u64,
    /// Range over every node of the file, per attribute and component; `None` when no node has a
    /// finite value. Stored here because a lazy reader never holds every page.
    attribute_ranges: Vec<Vec<Option<[f64; 2]>>>,
}

/// Range of each attribute component over `ranges`, the per-node ranges of the whole file.
fn global_attribute_ranges<'a>(
    schema: &[AttributeDescriptor],
    ranges: impl Iterator<Item = &'a NodeAttributeRange>,
) -> Result<Vec<Vec<Option<[f64; 2]>>>, FormatError> {
    let mut result: Vec<Vec<Option<[f64; 2]>>> = schema
        .iter()
        .map(|attribute| vec![None; attribute.component_count() as usize])
        .collect();
    for range in ranges {
        let row = result
            .get_mut(range.schema_index() as usize)
            .ok_or(FormatError::InvalidSchema)?;
        for (component, slot) in row.iter_mut().enumerate() {
            let low = range
                .min()
                .scalar_as_f64(component)
                .ok_or(FormatError::InvalidSchema)?;
            let high = range
                .max()
                .scalar_as_f64(component)
                .ok_or(FormatError::InvalidSchema)?;
            if low.is_finite() && high.is_finite() {
                *slot = Some(slot.map_or([low, high], |r| [r[0].min(low), r[1].max(high)]));
            }
        }
    }
    Ok(result)
}

impl PagedHeader {
    /// Global range of one attribute component, from the metadata block.
    pub fn attribute_range(
        &self,
        schema_index: u32,
        component: u32,
    ) -> Result<Option<[f64; 2]>, FormatError> {
        let row = self
            .attribute_ranges
            .get(schema_index as usize)
            .ok_or(FormatError::UnknownAttribute(schema_index))?;
        row.get(component as usize)
            .copied()
            .ok_or(FormatError::InvalidSchema)
    }

    /// Frame, CRS and root identity; its node-cell table is empty (nodes live in pages).
    pub fn frame(&self) -> &GlobalFrame {
        &self.frame
    }

    pub fn schema(&self) -> &[AttributeDescriptor] {
        &self.schema
    }

    pub fn source_point_count(&self) -> Option<u64> {
        self.source_point_count
    }

    pub fn sampling_policy(&self) -> Option<&str> {
        self.sampling_policy.as_deref()
    }

    pub fn visual_scale(&self) -> f64 {
        self.visual_scale
    }

    /// Declared node total: informative until every page has been read (`verify`).
    pub fn total_nodes(&self) -> u64 {
        self.total_nodes
    }

    /// Declared block total: informative until every page has been read (`verify`).
    pub fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    /// File offset where the block payloads end and the child pages begin.
    pub fn pages_start(&self) -> u64 {
        self.pages_start
    }
}

fn parse_header_block(
    cursor: &mut SliceCursor<'_>,
    limits: &FormatReaderLimits,
    index_offset: u64,
) -> Result<PagedHeader, FormatError> {
    if cursor.take(8)? != LAYOUT.index_magic() {
        return Err(FormatError::MalformedIndex("invalid index magic"));
    }
    let IndexPrefix {
        dataset_origin,
        root_level,
        root_prefix,
        frame_origin_quanta,
        data_max_quanta,
        source_point_count,
        crs,
        crs_record,
        source_scale_offset,
        sampling_policy,
        semantics,
        visual_scale,
        schema,
    } = parse_index_prefix(cursor, limits, true)?;
    let total_nodes = cursor.u64()?;
    let total_blocks = cursor.u64()?;
    let pages_start = cursor.u64()?;
    let mut attribute_ranges = Vec::with_capacity(schema.len());
    for attribute in &schema {
        let mut row = Vec::with_capacity(attribute.component_count() as usize);
        for _ in 0..attribute.component_count() {
            row.push(match cursor.u8()? {
                0 => None,
                1 => {
                    let (low, high) = (cursor.f64()?, cursor.f64()?);
                    if !low.is_finite() || !high.is_finite() || low > high {
                        return Err(FormatError::MalformedIndex(
                            "invalid global attribute range",
                        ));
                    }
                    Some([low, high])
                }
                _ => {
                    return Err(FormatError::MalformedIndex(
                        "invalid global attribute range flag",
                    ));
                }
            });
        }
        attribute_ranges.push(row);
    }
    // Reserved for the LAS flag / Extra Byte descriptors of plan task P2a.
    if cursor.u32()? != 0 {
        return Err(FormatError::MalformedIndex(
            "nonzero reserved index extension",
        ));
    }
    // The declared totals are informative for a lazy reader, so `max_nodes` and `max_blocks` bound
    // what is actually read (each page, and the eager loader's running totals), not these fields.
    if total_nodes == 0 {
        return Err(FormatError::MalformedIndex("index declares no nodes"));
    }
    if pages_start < HEADER_SIZE || pages_start > index_offset {
        return Err(FormatError::MalformedIndex(
            "page section start is out of range",
        ));
    }
    if visual_scale.is_nan() {
        return Err(FormatError::MalformedIndex("invalid visual scale"));
    }
    let frame = GlobalFrame::new(
        root_level,
        root_prefix,
        frame_origin_quanta,
        data_max_quanta,
        crs_record.ok_or(FormatError::MalformedIndex("missing CRS record"))?,
        source_scale_offset,
        Vec::new(),
    )
    .map_err(|_| {
        FormatError::MalformedIndex("invalid root level, prefix, frame origin or data maximum")
    })?;
    if dataset_origin != frame.frame_origin() {
        return Err(FormatError::MalformedIndex(
            "frame origin does not match the root prefix",
        ));
    }
    Ok(PagedHeader {
        dataset_origin,
        frame,
        crs_label: crs,
        source_point_count,
        sampling_policy,
        semantics,
        visual_scale,
        schema,
        total_nodes,
        total_blocks,
        pages_start,
        attribute_ranges,
    })
}

// ---------------------------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------------------------

/// Where the index region sits in the file, from the footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexLocation {
    pub offset: u64,
    pub length: u64,
    pub crc: u32,
}

/// I/O-free state of a lazily read `v7.5-pre` file: the metadata block, the root page and the
/// ids of the pages currently held. Callers fetch the bytes (sync or async) and feed them in.
pub struct PagedCore {
    limits: FormatReaderLimits,
    header: PagedHeader,
    root: Page,
    index_offset: u64,
    index_length: u64,
    index_crc: u32,
    /// Block id range of every page currently resident, by page offset, to reject repeated ids.
    resident: BTreeMap<u64, (u32, (u64, u64))>,
    by_first_id: BTreeMap<u64, (u64, u64)>,
    /// Page offset of every resident page number, to reject a number used by two pages.
    by_number: BTreeMap<u32, u64>,
}

impl PagedCore {
    /// Whether `footer` (the last 40 bytes of a file) carries the `v7.5-pre` footer magic.
    pub fn is_paged_footer(footer: &[u8]) -> bool {
        footer.len() == FOOTER_SIZE as usize && footer[..8] == *LAYOUT.footer_magic()
    }

    /// Parses the 40-byte footer (the last bytes of the file) and checks the index range.
    pub fn locate_index(
        file_length: u64,
        footer: &[u8],
        limits: &FormatReaderLimits,
    ) -> Result<IndexLocation, FormatError> {
        if file_length < HEADER_SIZE + FOOTER_SIZE {
            return Err(FormatError::FileTooShort);
        }
        let footer: &[u8; FOOTER_SIZE as usize] =
            footer.try_into().map_err(|_| FormatError::InvalidFooter)?;
        let (offset, length, declared_length, crc) =
            parse_split_footer(footer, LAYOUT.footer_magic())?;
        validate_index_range(file_length, offset, length, declared_length, *limits)?;
        Ok(IndexLocation {
            offset,
            length,
            crc,
        })
    }

    /// Validates the index region (metadata block and root page) read at `location`.
    pub fn from_index(
        index: &[u8],
        location: IndexLocation,
        limits: FormatReaderLimits,
    ) -> Result<Self, FormatError> {
        if usize::try_from(location.length).ok() != Some(index.len()) {
            return Err(FormatError::InvalidFooter);
        }
        if !checksum_matches(index, location.crc) {
            return Err(FormatError::InvalidChecksum("index"));
        }
        let mut cursor = SliceCursor::new(index);
        let header = parse_header_block(&mut cursor, &limits, location.offset)?;
        let root = decode_page(
            cursor.remaining_slice(),
            &PageContext {
                frame: &header.frame,
                schema: &header.schema,
                limits: &limits,
                pages_start: header.pages_start,
                index_offset: location.offset,
            },
            PageExpect::Root,
        )?;
        let mut core = Self {
            limits,
            header,
            root,
            index_offset: location.offset,
            index_length: location.length,
            index_crc: location.crc,
            resident: BTreeMap::new(),
            by_first_id: BTreeMap::new(),
            by_number: BTreeMap::new(),
        };
        let root_range = core.root.block_id_range();
        core.register_page(location.offset, 0, root_range)?;
        Ok(core)
    }

    pub fn header(&self) -> &PagedHeader {
        &self.header
    }

    pub fn root_page(&self) -> &Page {
        &self.root
    }

    pub fn limits(&self) -> &FormatReaderLimits {
        &self.limits
    }

    /// Footer fields of the index: `(offset, length, crc32)`.
    pub(crate) fn index_identity(&self) -> (u64, u64, u32) {
        (self.index_offset, self.index_length, self.index_crc)
    }

    /// Checks that `pointer` lies in the page section and within the page size limit.
    pub fn check_pointer(&self, pointer: PagePointer) -> Result<(), FormatError> {
        if pointer.offset < self.header.pages_start
            || pointer
                .offset
                .checked_add(u64::from(pointer.length))
                .is_none_or(|end| end > self.index_offset)
            || (pointer.length as usize) < MIN_PAGE_LEN
            || u64::from(pointer.length) > self.limits.max_page_bytes
        {
            return Err(FormatError::MalformedIndex(
                "child page pointer is outside the page section",
            ));
        }
        Ok(())
    }

    /// Validates the bytes of the page `pointer` names; `anchor` must be its node's identity.
    /// Errors if the page's block ids repeat ids of another resident page.
    pub fn decode_child(
        &mut self,
        bytes: &[u8],
        anchor: PageAnchor,
        pointer: PagePointer,
    ) -> Result<Page, FormatError> {
        self.check_pointer(pointer)?;
        let page = decode_page(
            bytes,
            &PageContext {
                frame: &self.header.frame,
                schema: &self.header.schema,
                limits: &self.limits,
                pages_start: self.header.pages_start,
                index_offset: self.index_offset,
            },
            PageExpect::Child {
                anchor,
                page: pointer.page,
                anchor_node: pointer.anchor_node,
            },
        )?;
        self.register_page(pointer.offset, page.number(), page.block_id_range())?;
        Ok(page)
    }

    /// Forgets a page the caller evicted, so its block ids may be seen again.
    pub fn release_page(&mut self, pointer: PagePointer) {
        if let Some((number, (first, _))) = self.resident.remove(&pointer.offset) {
            self.by_first_id.remove(&first);
            self.by_number.remove(&number);
        }
    }

    /// Errors if keeping `resident` pages would exceed `max_resident_pages`.
    pub fn check_resident_pages(&self, resident: usize) -> Result<(), FormatError> {
        let limit = u64::from(self.limits.max_resident_pages);
        if resident as u64 > limit {
            return Err(FormatError::CountLimit {
                kind: "resident pages",
                requested: resident as u64,
                limit,
            });
        }
        Ok(())
    }

    fn register_page(
        &mut self,
        offset: u64,
        number: u32,
        range: (u64, u64),
    ) -> Result<(), FormatError> {
        if let Some(known) = self.resident.get(&offset) {
            return if *known == (number, range) {
                Ok(())
            } else {
                Err(FormatError::MalformedIndex(
                    "a page changed since it was read",
                ))
            };
        }
        if self.by_number.contains_key(&number) {
            return Err(FormatError::MalformedIndex(
                "a page number repeats across pages",
            ));
        }
        let (first, end) = range;
        if end > first {
            let clashes = self
                .by_first_id
                .range(..end)
                .next_back()
                .is_some_and(|(_, (other_end, _))| *other_end > first);
            if clashes {
                return Err(FormatError::MalformedIndex(
                    "a block id repeats across pages",
                ));
            }
            self.by_first_id.insert(first, (end, offset));
        }
        self.resident.insert(offset, (number, range));
        self.by_number.insert(number, offset);
        Ok(())
    }
}

/// Page-granular reader over a local seekable stream: opens with one tail read (footer, metadata
/// block, root page) and fetches child pages by pointer. It keeps no page cache; the caller owns
/// residency and reports evictions with [`Self::release_page`].
pub struct PagedReader<R> {
    reader: R,
    core: PagedCore,
}

impl<R: Read + Seek> PagedReader<R> {
    pub fn open(reader: R) -> Result<Self, FormatError> {
        Self::with_limits(reader, FormatReaderLimits::default())
    }

    /// Identifies the revision from the footer magic and never reads the file header, so opening
    /// costs one request for the tail (plus one more if the index is larger than the hint).
    pub fn with_limits(mut reader: R, limits: FormatReaderLimits) -> Result<Self, FormatError> {
        let core = open_core(&mut reader, limits)?;
        Ok(Self { reader, core })
    }

    pub fn header(&self) -> &PagedHeader {
        self.core.header()
    }

    pub fn root_page(&self) -> &Page {
        self.core.root_page()
    }

    pub(crate) fn index_identity(&self) -> (u64, u64, u32) {
        self.core.index_identity()
    }

    /// Reads and validates the page a node points to. `anchor` must be that node's identity.
    /// Errors if the page's block ids repeat ids of another resident page.
    pub fn read_child_page(
        &mut self,
        anchor: PageAnchor,
        pointer: PagePointer,
    ) -> Result<Page, FormatError> {
        self.core.check_pointer(pointer)?;
        let mut bytes = vec![0; pointer.length as usize];
        self.reader.seek(SeekFrom::Start(pointer.offset))?;
        self.reader.read_exact(&mut bytes)?;
        self.core.decode_child(&bytes, anchor, pointer)
    }

    /// Forgets a page the caller evicted, so its block ids may be seen again.
    pub fn release_page(&mut self, pointer: PagePointer) {
        self.core.release_page(pointer);
    }

    /// Errors if keeping `resident` pages would exceed `max_resident_pages`.
    pub fn check_resident_pages(&self, resident: usize) -> Result<(), FormatError> {
        self.core.check_resident_pages(resident)
    }

    /// Stored bytes of one block's geometry column in the `.oct`, without decoding. Colour and
    /// attribute columns live in the `.octattrs` at `block.attribute_offset()`.
    pub fn read_block_bytes(&mut self, block: &PageBlock) -> Result<Vec<u8>, FormatError> {
        let length = usize::try_from(block.payload_length)
            .map_err(|_| FormatError::SizeOutOfRange(block.payload_length))?;
        let mut bytes = vec![0; length];
        self.reader.seek(SeekFrom::Start(block.offset))?;
        self.reader.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    pub fn into_inner(self) -> R {
        self.reader
    }
}

/// One tail read: footer, metadata block and root page (a second read only if the index is larger
/// than [`TAIL_READ_BYTES`]).
pub(crate) fn open_core<R: Read + Seek>(
    reader: &mut R,
    limits: FormatReaderLimits,
) -> Result<PagedCore, FormatError> {
    let file_length = reader.seek(SeekFrom::End(0))?;
    if file_length < HEADER_SIZE + FOOTER_SIZE {
        return Err(FormatError::FileTooShort);
    }
    let tail_length = file_length.min(TAIL_READ_BYTES);
    reader.seek(SeekFrom::Start(file_length - tail_length))?;
    let mut tail = vec![0; tail_length as usize];
    reader.read_exact(&mut tail)?;
    let location = PagedCore::locate_index(
        file_length,
        &tail[tail.len() - FOOTER_SIZE as usize..],
        &limits,
    )?;
    let index_size = usize::try_from(location.length)
        .map_err(|_| FormatError::SizeOutOfRange(location.length))?;
    let index = if location.length + FOOTER_SIZE <= tail_length {
        let end = tail.len() - FOOTER_SIZE as usize;
        tail[end - index_size..end].to_vec()
    } else {
        let mut index = Vec::new();
        index
            .try_reserve_exact(index_size)
            .map_err(|_| FormatError::AllocationFailed(index_size))?;
        index.resize(index_size, 0);
        reader.seek(SeekFrom::Start(location.offset))?;
        reader.read_exact(&mut index)?;
        index
    };
    drop(tail);
    PagedCore::from_index(&index, location, limits)
}

/// Every page of a file in breadth-first order (root first) with the pointer each child page was
/// read through.
struct AllPages {
    pages: Vec<Page>,
    pointers: Vec<PagePointer>,
    header: PagedHeader,
    index_identity: (u64, u64, u32),
    index_offset: u64,
}

fn read_all_pages<R: Read + Seek>(
    reader: &mut R,
    limits: &FormatReaderLimits,
) -> Result<AllPages, FormatError> {
    let mut paged = PagedReader::with_limits(&mut *reader, *limits)?;
    let mut pages = vec![paged.core.root.clone()];
    let mut pointers = Vec::new();
    let mut next = 0;
    let (mut nodes_total, mut blocks_total) = (0_u64, 0_u64);
    while next < pages.len() {
        let page = &pages[next];
        next += 1;
        nodes_total += page.nodes.len() as u64;
        blocks_total += page
            .nodes
            .iter()
            .map(|n| n.blocks.len() as u64)
            .sum::<u64>();
        if nodes_total > limits.max_nodes {
            return Err(FormatError::CountLimit {
                kind: "nodes",
                requested: nodes_total,
                limit: limits.max_nodes,
            });
        }
        if blocks_total > limits.max_blocks {
            return Err(FormatError::CountLimit {
                kind: "blocks",
                requested: blocks_total,
                limit: limits.max_blocks,
            });
        }
        let children: Vec<_> = page.child_pages().collect();
        for (anchor, pointer) in children {
            // Pages are numbered in plan order, which is the order a breadth-first walk meets them.
            if pointer.page as usize != pages.len() {
                return Err(FormatError::MalformedIndex(
                    "page numbers do not follow the plan order",
                ));
            }
            pages.push(paged.read_child_page(anchor, pointer)?);
            pointers.push(pointer);
        }
    }
    if nodes_total != paged.core.header.total_nodes
        || blocks_total != paged.core.header.total_blocks
    {
        return Err(FormatError::MalformedIndex(
            "declared node or block totals do not match the pages",
        ));
    }
    let nodes = pages.iter().flat_map(|page| page.nodes.iter());
    if global_attribute_ranges(
        &paged.core.header.schema,
        nodes.flat_map(|node| node.ranges.iter()),
    )? != paged.core.header.attribute_ranges
    {
        return Err(FormatError::MalformedIndex(
            "global attribute ranges do not match the pages",
        ));
    }
    Ok(AllPages {
        index_identity: paged.index_identity(),
        index_offset: paged.core.index_offset,
        header: paged.core.header,
        pages,
        pointers,
    })
}

/// Reads every page and assembles the whole-index view that `FormatReader` serves, with the
/// global checks (contiguous blocks, totals) that single pages cannot make.
pub(crate) fn load_all<R: Read + Seek>(
    reader: &mut R,
    limits: &FormatReaderLimits,
) -> Result<(ParsedIndex, (u64, u64, u32)), FormatError> {
    let all = read_all_pages(reader, limits)?;
    let parsed = assemble(&all.header, &all.pages)?;
    Ok((parsed, all.index_identity))
}

/// Sizes of a fully verified `v7.5-pre` index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagedStats {
    /// Root page included.
    pub pages: u64,
    pub nodes: u64,
    pub blocks: u64,
    /// Metadata block plus root page.
    pub index_region_bytes: u64,
    /// Child pages plus the index region: everything the paged index occupies in the file.
    pub index_bytes: u64,
    /// Largest page, root included.
    pub max_page_bytes: u64,
    /// Number of pages with `k = 1 ..= 5` (index `k - 1`).
    pub pages_by_levels: [u64; 5],
}

/// Reads every page and runs all the global checks that a lazy reader cannot make: each page
/// validates against the node that points to it, the child pages tile the page section without
/// gaps or overlaps, block payloads fill `[24, pages_start)` without gaps, every block belongs to
/// one node, and the declared totals match.
pub fn verify_paged<R: Read + Seek>(
    reader: &mut R,
    limits: &FormatReaderLimits,
) -> Result<PagedStats, FormatError> {
    let all = read_all_pages(reader, limits)?;
    let mut sorted = all.pointers.clone();
    sorted.sort_unstable_by_key(|pointer| pointer.offset);
    let mut expected = all.header.pages_start;
    for pointer in &sorted {
        if pointer.offset != expected {
            return Err(FormatError::MalformedIndex(
                "child pages leave a gap or overlap in the page section",
            ));
        }
        expected += u64::from(pointer.length);
    }
    if expected != all.index_offset {
        return Err(FormatError::MalformedIndex(
            "child pages do not end at the index region",
        ));
    }
    assemble(&all.header, &all.pages)?;
    let mut pages_by_levels = [0_u64; 5];
    for page in &all.pages {
        pages_by_levels[usize::from(page.k - 1)] += 1;
    }
    let index_region_bytes = all.index_identity.1;
    Ok(PagedStats {
        pages: all.pages.len() as u64,
        nodes: all.header.total_nodes,
        blocks: all.header.total_blocks,
        index_region_bytes,
        index_bytes: index_region_bytes + (all.index_offset - all.header.pages_start),
        max_page_bytes: all
            .pages
            .iter()
            .map(|page| page.encoded_len() as u64)
            .max()
            .unwrap_or(0),
        pages_by_levels,
    })
}

/// The colour and attribute columns of all blocks tile the `.octattrs` between its header and
/// footer without gaps or overlaps (the split-file rule of V7.4-pre).
fn validate_attribute_layout(nodes: &[&PageNode]) -> Result<(), FormatError> {
    let mut ranges = nodes
        .iter()
        .flat_map(|node| node.blocks.iter())
        .map(|block| (block.attribute_offset, block.attribute_end()))
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    let mut end = HEADER_SIZE;
    for (start, next) in ranges {
        if start != end {
            return Err(FormatError::MalformedIndex(
                "attribute blocks contain gaps or overlaps",
            ));
        }
        end = next;
    }
    Ok(())
}

fn assemble(header: &PagedHeader, pages: &[Page]) -> Result<ParsedIndex, FormatError> {
    assemble_with(header, pages, false)
}

/// `partial` skips the checks that need every page (tiling of the payload sections); the pages
/// given must still form a connected top of the tree.
pub(crate) fn assemble_with(
    header: &PagedHeader,
    pages: &[Page],
    partial: bool,
) -> Result<ParsedIndex, FormatError> {
    let page_nodes: Vec<&PageNode> = pages.iter().flat_map(|page| page.nodes.iter()).collect();
    let root_frame = &header.frame;
    let cells: Vec<(NodeId, NodeCell)> = page_nodes
        .iter()
        .map(|node| (node.id, NodeCell::new(node.prefix, node.spacing)))
        .collect();
    let mut frame = GlobalFrame::new(
        root_frame.root_level(),
        root_frame.root_prefix(),
        root_frame.frame_origin_quanta(),
        root_frame.data_max_quanta(),
        root_frame.crs().clone(),
        root_frame.source_scale_offset().cloned(),
        cells,
    )
    .map_err(|_| FormatError::MalformedIndex("repeated node identity"))?;

    let mut nodes = Vec::with_capacity(page_nodes.len());
    let mut node_ranges = Vec::with_capacity(page_nodes.len());
    for node in &page_nodes {
        let depth = node.level - root_frame.root_level();
        let bounds = frame
            .node_bounds(depth, node.prefix)
            .map_err(|_| FormatError::MalformedIndex("derived node bounds are invalid"))?;
        nodes.push(SpatialNode::new(
            node.id,
            node.parent,
            depth,
            bounds,
            node.point_count,
            node.geometric_error,
            node.blocks.iter().map(|block| block.id).collect(),
        )?);
        node_ranges.push((node.id, node.ranges.clone()));
    }
    nodes.sort_unstable_by_key(|node| node.id());
    let node_refs: Vec<&SpatialNode> = nodes.iter().collect();
    crate::global_frame::validate_nodes(&frame, header.dataset_origin, &node_refs)
        .map_err(FormatError::MalformedIndex)?;
    frame
        .bind_blocks(&node_refs)
        .map_err(|_| FormatError::MalformedIndex("a block belongs to more than one node"))?;

    let mut blocks = BTreeMap::new();
    for block in page_nodes.iter().flat_map(|node| node.blocks.iter()) {
        let entry = block_entry(block, frame.block_box(block.id));
        if blocks.insert(block.id, entry).is_some() {
            return Err(FormatError::MalformedIndex("duplicate block IDs"));
        }
    }
    if !partial {
        validate_attribute_layout(&page_nodes)?;
        validate_block_layout(&nodes, &blocks, header.pages_start)?;
    }
    validate_node_point_counts(&nodes, &blocks)?;
    validate_node_error_bounds(&nodes, &blocks)?;
    let mut block_bounds = BTreeMap::<BlockId, Vec<SpatialBounds>>::new();
    for node in &nodes {
        for block_id in node.blocks() {
            block_bounds
                .entry(*block_id)
                .or_default()
                .push(node.bounds());
        }
    }
    let hierarchy = SpatialHierarchy::new_with_semantics(
        nodes,
        header.schema.clone(),
        header.source_point_count,
        header.semantics,
    )?;
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new(header.dataset_origin)?, hierarchy),
        header.crs_label.clone(),
        header.sampling_policy.clone(),
        node_ranges,
    )?
    .with_visual_scale(header.visual_scale)?
    .with_global_frame(frame)?;
    validate_metadata(&metadata)?;
    Ok((metadata, blocks, block_bounds))
}

// ---------------------------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------------------------

struct PlanNode<'a> {
    node: &'a SpatialNode,
    prefix: [u64; 3],
    spacing: f64,
    /// Children in ascending octant order.
    children: Vec<usize>,
    record_len: usize,
}

struct PagePlan {
    anchor: usize,
    k: u8,
    /// `(depth, path, node)` in page order.
    entries: Vec<(u8, u16, usize)>,
    first_block_id: u64,
    size: usize,
    /// Page index of the page hanging from each entry that points to one.
    children: BTreeMap<usize, usize>,
}

pub(crate) fn block_entry(
    block: &PageBlock,
    node_cell: Option<crate::global_frame::CellBox>,
) -> BlockDirectoryEntry {
    BlockDirectoryEntry {
        offset: block.offset,
        payload_length: block.payload_length,
        point_count: block.point_count,
        quantization_error: block.quantization_error,
        origin: [0.0; 3],
        scale: [0.0; 3],
        origin_quanta: block.origin_quanta,
        node_cell,
        geometry_encoding: block.geometry_encoding,
        payloads: block.payloads.clone(),
    }
}

fn octant(prefix: [u64; 3]) -> u8 {
    (0..3).fold(0, |bits, axis| bits | (((prefix[axis] & 1) as u8) << axis))
}

fn value_bytes(values: &viewer_core::AttributeValues) -> usize {
    values.len() * crate::layout::primitive_width(values.data_type()) as usize
}

/// Bytes of a node record without its spacing byte and child pointer (those depend on the page).
fn base_record_len(
    node: &SpatialNode,
    level: u32,
    blocks: &BTreeMap<BlockId, BlockRecord>,
    ranges: &[NodeAttributeRange],
) -> Result<usize, FormatError> {
    // depth 1, path 2, flags 1, point count 8, geometric error 8, block count 4, range count 2
    let mut length = 26;
    for id in node.blocks() {
        let record = blocks.get(id).ok_or(FormatError::UnreferencedBlock(*id))?;
        // offset, length, points, geometry tag, error; split cell; per column codec, crc, length
        length += 33 + if level < 32 { 12 } else { 0 } + 13 * record.payloads.len() + 8;
    }
    for range in ranges {
        length += 2 + value_bytes(range.min()) + value_bytes(range.max());
    }
    Ok(length)
}

/// Where the paged index ended up, for the footer.
pub(crate) struct PagedIndexWritten {
    pub(crate) index_offset: u64,
    pub(crate) index_length: u64,
    pub(crate) index_crc: u32,
}

/// Writes child pages (breadth-first, after the block payloads), then the metadata block and the
/// root page. The stream position on entry is the end of the block payloads.
pub(crate) fn write_paged_index<W: Write + Seek>(
    writer: &mut W,
    metadata: &FormatMetadata,
    blocks: &BTreeMap<BlockId, BlockRecord>,
    page_target_bytes: usize,
) -> Result<PagedIndexWritten, FormatError> {
    let frame = metadata.global_frame().ok_or(FormatError::InvalidMetadata(
        "v7.5-pre needs a global frame in the metadata",
    ))?;
    let hierarchy = metadata.dataset().hierarchy();
    if hierarchy.semantics() != HierarchySemantics::Replacement {
        return Err(FormatError::InvalidMetadata(
            "v7.5-pre does not support additive hierarchies",
        ));
    }
    if hierarchy.attributes().len() > usize::from(u16::MAX) {
        return Err(FormatError::SchemaTooLarge);
    }
    let pages_start = writer.stream_position()?;
    let root = hierarchy
        .node(hierarchy.root_id())
        .ok_or(FormatError::InvalidMetadata("root node is missing"))?;
    let mut ordered = hierarchy.query(root.bounds()).collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|node| node.id());
    let index_of: BTreeMap<NodeId, usize> = ordered
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id(), i))
        .collect();

    let mut plan: Vec<PlanNode<'_>> = Vec::with_capacity(ordered.len());
    for node in &ordered {
        let cell = frame
            .node_cell(node.id())
            .ok_or(FormatError::UnknownNode(node.id()))?;
        let mut ranges = metadata.node_attribute_ranges(node.id()).to_vec();
        ranges.sort_unstable_by_key(|range| range.schema_index());
        plan.push(PlanNode {
            node,
            prefix: cell.prefix(),
            spacing: cell.spacing(),
            children: Vec::new(),
            record_len: base_record_len(node, frame.root_level() + node.level(), blocks, &ranges)?,
        });
    }
    for (index, node) in ordered.iter().enumerate() {
        if let Some(parent) = node.parent() {
            plan[index_of[&parent]].children.push(index);
        }
    }
    for index in 0..plan.len() {
        let mut children = std::mem::take(&mut plan[index].children);
        children.sort_unstable_by_key(|child| octant(plan[*child].prefix));
        if children
            .windows(2)
            .any(|pair| octant(plan[pair[0]].prefix) == octant(plan[pair[1]].prefix))
        {
            return Err(FormatError::InvalidMetadata("two siblings share an octant"));
        }
        plan[index].children = children;
    }
    let root_index = index_of[&root.id()];

    // Breadth-first page planning: a page's child pages are queued together, so siblings are
    // contiguous in the file.
    let mut pages: Vec<PagePlan> = Vec::new();
    let mut next_block_id = 0_u64;
    pages.push(PagePlan {
        anchor: root_index,
        k: 0,
        entries: Vec::new(),
        first_block_id: 0,
        size: 0,
        children: BTreeMap::new(),
    });
    let mut cursor = 0;
    while cursor < pages.len() {
        let anchor = pages[cursor].anchor;
        let is_root = cursor == 0;
        let anchor_level = frame.root_level() + plan[anchor].node.level();
        let max_k = u32::from(MAX_PAGE_LEVELS).min(64 - anchor_level) as u8;
        let mut chosen = None;
        for k in (1..=max_k).rev() {
            let entries = page_entries(&plan, anchor, k, is_root);
            let size = PAGE_HEADER_LEN
                + CRC_LEN
                + entries
                    .iter()
                    .map(|&(depth, _, node)| {
                        let p = &plan[node];
                        p.record_len
                            + usize::from(p.children.is_empty())
                            + if depth == k && !p.children.is_empty() {
                                16
                            } else {
                                0
                            }
                    })
                    .sum::<usize>();
            chosen = Some((k, entries, size));
            if size <= page_target_bytes {
                break;
            }
        }
        let (k, entries, size) = chosen.ok_or(FormatError::InvalidMetadata(
            "node below the deepest page level has children",
        ))?;
        let first_block_id = next_block_id;
        next_block_id += entries
            .iter()
            .map(|&(_, _, node)| plan[node].node.blocks().len() as u64)
            .sum::<u64>();
        let mut children = BTreeMap::new();
        for (position, &(depth, _, node)) in entries.iter().enumerate() {
            if depth == k && !plan[node].children.is_empty() {
                children.insert(position, pages.len());
                pages.push(PagePlan {
                    anchor: node,
                    k: 0,
                    entries: Vec::new(),
                    first_block_id: 0,
                    size: 0,
                    children: BTreeMap::new(),
                });
            }
        }
        let page = &mut pages[cursor];
        page.k = k;
        page.entries = entries;
        page.first_block_id = first_block_id;
        page.size = size;
        page.children = children;
        cursor += 1;
    }

    // Offsets: child pages in plan order from the end of the block payloads.
    let mut offsets = vec![0_u64; pages.len()];
    let mut next_offset = pages_start;
    for (index, page) in pages.iter().enumerate().skip(1) {
        offsets[index] = next_offset;
        next_offset = next_offset
            .checked_add(page.size as u64)
            .ok_or(FormatError::FileSizeOverflow)?;
    }

    let build = |index: usize| -> Result<Vec<u8>, FormatError> {
        let page = &pages[index];
        let anchor_node = &plan[page.anchor];
        let anchor = PageAnchor {
            level: frame.root_level() + anchor_node.node.level(),
            prefix: anchor_node.prefix,
        };
        let mut id = page.first_block_id;
        let mut nodes = Vec::with_capacity(page.entries.len());
        for (position, &(depth, path, node_index)) in page.entries.iter().enumerate() {
            let p = &plan[node_index];
            let node = p.node;
            let mut node_blocks = Vec::with_capacity(node.blocks().len());
            for block_id in node.blocks() {
                let record = &blocks[block_id];
                node_blocks.push(PageBlock {
                    id: BlockId::new(id),
                    offset: record.offset,
                    payload_length: record.payload_length,
                    attribute_offset: record.payloads[1].offset,
                    point_count: record.point_count,
                    geometry_encoding: record.geometry_encoding,
                    quantization_error: record.quantization_error,
                    origin_quanta: record.origin_quanta,
                    payloads: record.payloads.clone(),
                });
                id += 1;
            }
            let mut ranges = metadata.node_attribute_ranges(node.id()).to_vec();
            ranges.sort_unstable_by_key(|range| range.schema_index());
            nodes.push(PageNode {
                id: page_node_id(index as u32, position as u32),
                parent: None,
                depth,
                path,
                level: frame.root_level() + node.level(),
                prefix: p.prefix,
                has_children: !p.children.is_empty(),
                point_count: node.point_count(),
                geometric_error: node.geometric_error(),
                spacing: p.spacing,
                blocks: node_blocks,
                ranges,
                child_page: page.children.get(&position).map(|&child| PagePointer {
                    offset: offsets[child],
                    length: pages[child].size as u32,
                    page: child as u32,
                    anchor_node: page_node_id(index as u32, position as u32),
                }),
            });
        }
        let page_value = Page {
            number: index as u32,
            anchor,
            k: page.k,
            is_root: index == 0,
            first_block_id: page.first_block_id,
            nodes,
            encoded_len: page.size,
        };
        let bytes = encode_page(&page_value, frame)?;
        if bytes.len() != page.size {
            return Err(FormatError::InvalidMetadata("planned page size mismatch"));
        }
        Ok(bytes)
    };

    if pages
        .iter()
        .skip(1)
        .any(|page| u32::try_from(page.size).is_err())
    {
        return Err(FormatError::InvalidMetadata("page larger than 4 GiB"));
    }
    if u32::try_from(pages.len()).is_err() {
        return Err(FormatError::TooManyNodes);
    }
    for index in 1..pages.len() {
        writer.write_all(&build(index)?)?;
    }

    let index_offset = writer.stream_position()?;
    let mut header_bytes = Vec::new();
    let mut crc = Crc32::new();
    {
        let mut out = IndexWriter {
            writer: &mut header_bytes,
            checksum: Crc32::new(),
            length: 0,
        };
        out.write(LAYOUT.index_magic())?;
        write_index_prefix(&mut out, metadata, Some(frame))?;
        out.u64(plan.len() as u64)?;
        out.u64(blocks.len() as u64)?;
        out.u64(pages_start)?;
        for row in global_attribute_ranges(
            hierarchy.attributes(),
            plan.iter()
                .flat_map(|node| metadata.node_attribute_ranges(node.node.id()).iter()),
        )? {
            for range in row {
                match range {
                    None => out.u8(0)?,
                    Some([low, high]) => {
                        out.u8(1)?;
                        out.f64(low)?;
                        out.f64(high)?;
                    }
                }
            }
        }
        out.u32(0)?;
    }
    let root_bytes = build(0)?;
    crc.update(&header_bytes);
    crc.update(&root_bytes);
    writer.write_all(&header_bytes)?;
    writer.write_all(&root_bytes)?;
    Ok(PagedIndexWritten {
        index_offset,
        index_length: (header_bytes.len() + root_bytes.len()) as u64,
        index_crc: crc.finish(),
    })
}

/// Nodes of a page in canonical order: depth ascending, then path ascending. The root page also
/// holds the anchor (the root node) at depth 0.
fn page_entries(
    plan: &[PlanNode<'_>],
    anchor: usize,
    k: u8,
    include_anchor: bool,
) -> Vec<(u8, u16, usize)> {
    let mut entries = Vec::new();
    if include_anchor {
        entries.push((0, 0, anchor));
    }
    let mut frontier = vec![anchor];
    for depth in 1..=k {
        let mut next = Vec::new();
        for &parent in &frontier {
            next.extend(plan[parent].children.iter().copied());
        }
        for &child in &next {
            let path = prefix_path(plan[child].prefix, depth);
            entries.push((depth, path, child));
        }
        frontier = next;
    }
    entries
}

/// Footer bytes for a paged file.
pub(crate) fn paged_footer(written: &PagedIndexWritten, file_length: u64) -> [u8; 40] {
    footer_bytes(
        LAYOUT.footer_magic(),
        written.index_offset,
        written.index_length,
        file_length,
        written.index_crc,
    )
}
