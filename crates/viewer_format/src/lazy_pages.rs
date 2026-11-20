//! Lazily read `v7.5-pre` index: the shared state behind a reader that holds only the pages the
//! viewer asked for. The hierarchy itself lives in the viewer; this module hands it validated
//! [`PageGraft`]s and keeps the block directory of the resident pages, which payload reads need.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};

use viewer_core::{BlockId, NodeId, SpatialBounds, SpatialNode};

use crate::index_codec::BlockDirectoryEntry;
use crate::page_codec::{Page, PageAnchor, PagePointer};
use crate::paged_index::{PagedCore, block_entry};
use crate::{FormatError, GlobalFrame};

/// A validated page ready to be grafted into the viewer's hierarchy.
#[derive(Debug, Clone)]
pub struct PageGraft {
    /// Page number (0 for the root page).
    pub number: u32,
    /// Node the page hangs from; `None` for the root page.
    pub anchor: Option<NodeId>,
    /// Viewer-space nodes (world bounds, stored spacing), parents before children.
    pub nodes: Vec<SpatialNode>,
    /// Nodes of this page whose own child page is not loaded.
    pub unloaded: Vec<NodeId>,
    /// Encoded size, the unit of the viewer's page budget.
    pub encoded_len: usize,
}

/// Where to fetch a page, as the file offset and length of its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRange {
    pub offset: u64,
    pub length: u64,
}

struct State {
    core: PagedCore,
    frame: GlobalFrame,
    origin: [f64; 3],
    /// Pointer of the page hanging from each node of a resident page.
    pointers: HashMap<NodeId, (PageAnchor, PagePointer)>,
    blocks: BTreeMap<BlockId, Arc<BlockDirectoryEntry>>,
    /// Blocks and child-page holders of each resident page, by page number.
    page_contents: HashMap<u32, (Vec<BlockId>, Vec<NodeId>)>,
}

/// Shared by the reader (payload lookups) and the viewer (page fetch and release).
pub struct PageSource {
    state: Mutex<State>,
    root_unloaded: Vec<NodeId>,
    declared_nodes: u64,
}

impl PageSource {
    /// Takes the validated index; the root page is registered like any other.
    pub(crate) fn new(core: PagedCore) -> Result<(Arc<Self>, PageGraft), FormatError> {
        let header = core.header();
        let frame = header.frame().clone();
        let origin = frame.frame_origin();
        let root = core.root_page().clone();
        let mut source = Self {
            root_unloaded: Vec::new(),
            declared_nodes: core.header().total_nodes(),
            state: Mutex::new(State {
                core,
                frame,
                origin,
                pointers: HashMap::new(),
                blocks: BTreeMap::new(),
                page_contents: HashMap::new(),
            }),
        };
        let graft = source.lock().add_page(&root, None)?;
        source.root_unloaded.clone_from(&graft.unloaded);
        Ok((Arc::new(source), graft))
    }

    /// Node total the file declares (informative until every page has been read).
    pub fn declared_nodes(&self) -> u64 {
        self.declared_nodes
    }

    /// Global range of one attribute component, over every page of the file.
    pub fn attribute_range(
        &self,
        schema_index: u32,
        component: u32,
    ) -> Result<Option<[f64; 2]>, FormatError> {
        self.lock()
            .core
            .header()
            .attribute_range(schema_index, component)
    }

    /// Reference spacing of the cloud (RF-05), from the metadata block: it does not change as
    /// pages are loaded or released.
    pub fn reference_spacing(&self) -> f64 {
        self.lock().core.header().reference_spacing()
    }

    /// Nearest-neighbour distance quantiles, from the index header (`v8.N-pre` only).
    pub fn point_distance_quantiles(&self) -> Option<crate::PointDistanceQuantiles> {
        self.lock().core.header().distance_quantiles()
    }

    /// Nodes of the root page whose child pages are not loaded.
    pub fn root_unloaded(&self) -> &[NodeId] {
        &self.root_unloaded
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// File range of the page hanging from `anchor`; the anchor's page must be resident.
    pub fn page_range(&self, anchor: NodeId) -> Result<PageRange, FormatError> {
        let state = self.lock();
        let (_, pointer) = state
            .pointers
            .get(&anchor)
            .ok_or(FormatError::MalformedIndex("node has no child page"))?;
        state.core.check_pointer(*pointer)?;
        Ok(PageRange {
            offset: pointer.offset,
            length: u64::from(pointer.length),
        })
    }

    /// Validates the bytes fetched for `anchor`'s page and registers its blocks.
    pub fn accept_page(&self, anchor: NodeId, bytes: &[u8]) -> Result<PageGraft, FormatError> {
        let mut state = self.lock();
        let (page_anchor, pointer) = *state
            .pointers
            .get(&anchor)
            .ok_or(FormatError::MalformedIndex("node has no child page"))?;
        let page = state.core.decode_child(bytes, page_anchor, pointer)?;
        match state.add_page(&page, Some(anchor)) {
            Ok(graft) => Ok(graft),
            Err(error) => {
                state.core.release_page(pointer);
                Err(error)
            }
        }
    }

    /// Forgets the page hanging from `anchor` after the viewer evicted it, freeing its block ids.
    pub fn release_page(&self, anchor: NodeId) {
        let mut state = self.lock();
        let Some((_, pointer)) = state.pointers.get(&anchor).copied() else {
            return;
        };
        state.core.release_page(pointer);
        if let Some((blocks, holders)) = state.page_contents.remove(&pointer.page) {
            for block in blocks {
                state.blocks.remove(&block);
            }
            for holder in holders {
                state.pointers.remove(&holder);
            }
        }
    }

    /// Number of pages whose blocks are currently registered (root included).
    pub fn resident_pages(&self) -> usize {
        self.lock().page_contents.len()
    }

    pub(crate) fn block(&self, id: BlockId) -> Option<Arc<BlockDirectoryEntry>> {
        self.lock().blocks.get(&id).cloned()
    }
}

impl State {
    fn add_page(&mut self, page: &Page, anchor: Option<NodeId>) -> Result<PageGraft, FormatError> {
        let root_level = self.frame.root_level();
        let mut nodes = Vec::with_capacity(page.nodes().len());
        let mut unloaded = Vec::new();
        let mut entries = Vec::new();
        let mut holders = Vec::new();
        let mut pointers = Vec::new();
        for node in page.nodes() {
            let depth = node.level() - root_level;
            let local = self
                .frame
                .node_bounds(depth, node.prefix())
                .map_err(|_| FormatError::MalformedIndex("derived node bounds are invalid"))?;
            let bounds = SpatialBounds::new(
                std::array::from_fn(|axis| local.min()[axis] + self.origin[axis]),
                std::array::from_fn(|axis| local.max()[axis] + self.origin[axis]),
            )
            .map_err(|_| FormatError::MalformedIndex("derived node bounds are invalid"))?;
            let spatial = SpatialNode::new(
                node.id(),
                node.parent(),
                depth,
                bounds,
                node.point_count(),
                node.geometric_error(),
                node.blocks()
                    .iter()
                    .map(super::page_codec::PageBlock::id)
                    .collect(),
            )?
            .with_spacing(node.spacing())
            .map_err(|_| FormatError::MalformedIndex("invalid node spacing"))?;
            nodes.push(spatial);
            let cell = self.frame.node_box(depth, node.prefix());
            for block in node.blocks() {
                entries.push((block.id(), Arc::new(block_entry(block, cell))));
            }
            if let Some(pointer) = node.child_page() {
                unloaded.push(node.id());
                holders.push(node.id());
                pointers.push((node.id(), (node.anchor(), pointer)));
            }
        }
        let block_ids: Vec<BlockId> = entries.iter().map(|(id, _)| *id).collect();
        if entries.iter().any(|(id, _)| self.blocks.contains_key(id)) {
            return Err(FormatError::MalformedIndex("duplicate block IDs"));
        }
        self.blocks.extend(entries);
        self.pointers.extend(pointers);
        self.page_contents
            .insert(page.number(), (block_ids, holders));
        Ok(PageGraft {
            number: page.number(),
            anchor,
            nodes,
            unloaded,
            encoded_len: page.encoded_len(),
        })
    }
}
