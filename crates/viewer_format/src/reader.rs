use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Seek, SeekFrom},
    sync::{Arc, Mutex},
};

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeValues, BlockId, ColorRgb, DatasetMetadata,
    DatasetSource, Point, PointBatch, PointBlock,
};

use crate::geometry::{GeometryDecoder, GeometryEncoding};
use crate::layout::{
    FOOTER_SIZE, HEADER_SIZE, PayloadDirectoryEntry, StorageLayout, StoredColumn,
    attribute_payload_size, block_payload_size,
};
use crate::version::FileRole;
use crate::{BlockCodec, FormatError, FormatMetadata, crc32::checksum_matches, decompress_block};

use crate::index_codec::{BlockDirectoryEntry, parse_split_footer};
pub use crate::reader_limits::FormatReaderLimits;
use crate::slice_cursor::SliceCursor;

const ATTRIBUTE_RANGE_CACHE_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
struct CachedAttributeRange {
    schema_index: u32,
    component: u32,
    range: Option<[f64; 2]>,
}

/// Aggregate sizes for an entire block, excluding the file index.
/// `logical_bytes` is the canonical geometry and attribute size; `raw_bytes` is the
/// sum of the independently decoded payloads and can differ for encoded geometry.
/// Use `block_payload_storage_stats`
/// to inspect their independent codecs and sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockStorageStats {
    pub id: viewer_core::BlockId,
    pub point_count: u64,
    pub stored_bytes: u64,
    pub raw_bytes: u64,
    pub logical_bytes: u64,
    pub geometry_encoding: GeometryEncoding,
}

/// Identity of an independently encoded payload in a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockPayloadKind {
    Geometry,
    DisplayColor,
    Attribute { schema_index: u32, name: String },
}

/// Stored and decoded bytes of one payload from the validated file index.
/// These sizes exclude header, index and footer bytes and require no payload I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockPayloadStorageStats {
    pub id: BlockId,
    pub kind: BlockPayloadKind,
    pub codec: BlockCodec,
    pub stored_bytes: u64,
    pub raw_bytes: u64,
}

/// An index read and validated by [`IndexedReader::load_identity`], not yet bound to its reader.
pub(crate) struct LoadedPrimary {
    layout: StorageLayout,
    index_identity: (u64, u64, u32),
    attribute_file_length: u64,
    metadata: FormatMetadata,
    blocks: BTreeMap<BlockId, BlockDirectoryEntry>,
    reference_spacing: f64,
}

/// Seekable reader that loads metadata at open and verifies only requested block payloads.
pub(crate) struct IndexedReader<R> {
    reader: R,
    attributes: Option<R>,
    attributes_attached: bool,
    external_payloads_only: bool,
    layout: StorageLayout,
    index_identity: (u64, u64, u32),
    attribute_file_length: u64,
    metadata: Arc<FormatMetadata>,
    blocks: Arc<BTreeMap<BlockId, BlockDirectoryEntry>>,
    /// Lazily read `v7.5-pre`: the block directory of the resident pages (`blocks` is empty).
    paged: Option<Arc<crate::lazy_pages::PageSource>>,
    /// Reference spacing stored in the metadata block of an eagerly read `v7.5-pre` file.
    stored_reference_spacing: Option<f64>,
    plan_owner: Arc<()>,
    attribute_range_cache: Arc<Mutex<VecDeque<CachedAttributeRange>>>,
    limits: FormatReaderLimits,
}

/// A block directory entry that does not borrow the reader.
pub(crate) enum EntryHandle {
    Table(Arc<BTreeMap<BlockId, BlockDirectoryEntry>>, BlockId),
    Shared(Arc<BlockDirectoryEntry>),
}

impl std::ops::Deref for EntryHandle {
    type Target = BlockDirectoryEntry;

    fn deref(&self) -> &BlockDirectoryEntry {
        match self {
            Self::Table(table, id) => &table[id],
            Self::Shared(entry) => entry,
        }
    }
}

/// The payload of `column` (0 geometry, 1 display colour, then the schema) if the block stores it.
fn stored_payload(
    block: &BlockDirectoryEntry,
    id: BlockId,
    column: usize,
) -> Result<&PayloadDirectoryEntry, FormatError> {
    match block.payloads.get(column) {
        Some(payload) if payload.stored.is_present() => Ok(payload),
        Some(_) | None if column >= 2 => Err(FormatError::AttributeAbsent {
            block: id,
            attribute: (column - 2) as u32,
        }),
        _ => Err(FormatError::MalformedIndex(
            "geometry or display colour column is not stored",
        )),
    }
}

impl<R> IndexedReader<R> {
    fn entry(&self, id: BlockId) -> Result<EntryHandle, FormatError> {
        if let Some(paged) = &self.paged {
            return paged
                .block(id)
                .map(EntryHandle::Shared)
                .ok_or(FormatError::UnknownBlock(id));
        }
        if self.blocks.contains_key(&id) {
            Ok(EntryHandle::Table(Arc::clone(&self.blocks), id))
        } else {
            Err(FormatError::UnknownBlock(id))
        }
    }
}

impl<R: Read + Seek> IndexedReader<R> {
    pub(crate) fn attach_attribute_identity(
        &mut self,
        file_length: u64,
        header: &[u8],
        footer: &[u8],
    ) -> Result<(), FormatError> {
        if !self.external_payloads_only {
            return Err(FormatError::InvalidMetadata(
                "attribute identity requires a metadata-only reader",
            ));
        }
        // A lazy reader has not seen every block, so the companion length is checked by `verify`.
        if self.paged.is_none() && file_length != self.attribute_file_length {
            return Err(FormatError::InvalidFooter);
        }
        let header: &[u8; HEADER_SIZE as usize] =
            header.try_into().map_err(|_| FormatError::InvalidHeader)?;
        let identity = crate::version::parse_identity(header)?;
        if identity.version != self.layout.version() || identity.role != FileRole::Attributes {
            return Err(FormatError::InvalidHeader);
        }
        let footer: &[u8; FOOTER_SIZE as usize] =
            footer.try_into().map_err(|_| FormatError::InvalidFooter)?;
        let (offset, len, declared, checksum) =
            parse_split_footer(footer, self.layout.footer_magic())?;
        if declared != file_length || (offset, len, checksum) != self.index_identity {
            return Err(FormatError::InvalidMetadata(
                "attribute companion does not match primary index",
            ));
        }
        self.attributes_attached = true;
        Ok(())
    }

    /// Lazy `v7.5-pre` view: the metadata block and root page are validated, child pages arrive
    /// through [`Self::read_page`]. `external` readers never touch the stream for payloads.
    pub(crate) fn from_paged_core(
        reader: R,
        core: crate::paged_index::PagedCore,
        external: bool,
    ) -> Result<Self, FormatError> {
        let header = core.header().clone();
        let root = core.root_page().clone();
        let index_identity = core.index_identity();
        let limits = *core.limits();
        let layout = core.layout();
        let (metadata, _) =
            crate::paged_index::assemble_with(&header, std::slice::from_ref(&root), true)?;
        let (source, _) = crate::lazy_pages::PageSource::new(core)?;
        Ok(Self {
            reader,
            attributes: None,
            attributes_attached: false,
            external_payloads_only: external,
            layout,
            index_identity,
            attribute_file_length: 0,
            metadata: Arc::new(metadata),
            blocks: Arc::new(BTreeMap::new()),
            paged: Some(source),
            stored_reference_spacing: None,
            plan_owner: Arc::new(()),
            attribute_range_cache: Arc::new(Mutex::new(VecDeque::new())),
            limits,
        })
    }

    pub(crate) fn page_source(&self) -> Option<&Arc<crate::lazy_pages::PageSource>> {
        self.paged.as_ref()
    }

    /// Reads, validates and registers the page hanging from `anchor` from the primary stream.
    pub(crate) fn read_page(
        &mut self,
        anchor: viewer_core::NodeId,
    ) -> Result<crate::lazy_pages::PageGraft, FormatError> {
        if self.external_payloads_only {
            return Err(FormatError::ExternalPayloadBuffersRequired);
        }
        let source = self
            .paged
            .clone()
            .ok_or(FormatError::InvalidMetadata("the index is not paged"))?;
        let range = source.page_range(anchor)?;
        let mut bytes = vec![0; range.length as usize];
        self.reader.seek(SeekFrom::Start(range.offset))?;
        self.reader.read_exact(&mut bytes)?;
        source.accept_page(anchor, &bytes)
    }

    /// Reads and validates the whole index of a monolithic-view primary file. Borrows the reader so
    /// the caller still owns it on failure (see [`Self::from_loaded`]).
    pub(crate) fn load_identity(
        reader: &mut R,
        limits: &FormatReaderLimits,
        identity: crate::FormatIdentity,
    ) -> Result<LoadedPrimary, FormatError> {
        let layout = validate_supported_primary(identity)?;
        let file_length = reader.seek(SeekFrom::End(0))?;
        if file_length < HEADER_SIZE + FOOTER_SIZE {
            return Err(FormatError::FileTooShort);
        }
        if identity.role == FileRole::Attributes {
            return Err(FormatError::InvalidHeader);
        }
        if identity.role != FileRole::SplitPrimary {
            return Err(FormatError::InvalidHeader);
        }
        // Eager view: every page is read and the global checks run. Lazy readers use
        // `PagedReader` instead and never read the header, so only this path can compare
        // the header identity with the footer one.
        reader.seek(SeekFrom::Start(file_length - FOOTER_SIZE))?;
        let mut footer = [0; FOOTER_SIZE as usize];
        reader.read_exact(&mut footer)?;
        if crate::paged_index::paged_layout_of_footer(&footer) != Some(layout) {
            return Err(FormatError::InvalidFooter);
        }
        let ((metadata, blocks), index_identity, reference_spacing) =
            crate::paged_index::load_all(reader, limits)?;
        let attribute_file_length = split_attribute_file_length(&blocks)?;
        Ok(LoadedPrimary {
            layout,
            index_identity,
            attribute_file_length,
            metadata,
            blocks,
            reference_spacing,
        })
    }

    pub(crate) fn from_loaded(
        reader: R,
        limits: FormatReaderLimits,
        loaded: LoadedPrimary,
    ) -> Self {
        Self {
            reader,
            attributes: None,
            attributes_attached: false,
            external_payloads_only: false,
            layout: loaded.layout,
            index_identity: loaded.index_identity,
            attribute_file_length: loaded.attribute_file_length,
            metadata: Arc::new(loaded.metadata),
            blocks: Arc::new(loaded.blocks),
            paged: None,
            stored_reference_spacing: Some(loaded.reference_spacing),
            plan_owner: Arc::new(()),
            attribute_range_cache: Arc::new(Mutex::new(VecDeque::new())),
            limits,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_identity(
        mut reader: R,
        limits: FormatReaderLimits,
        identity: crate::FormatIdentity,
    ) -> Result<Self, FormatError> {
        let loaded = Self::load_identity(&mut reader, &limits, identity)?;
        Ok(Self::from_loaded(reader, limits, loaded))
    }

    /// Attach a committed companion belonging to this primary index. Payload CRCs are
    /// checked lazily on access; binding protects accidental mismatch, not forgery.
    pub fn attach_attributes(&mut self, mut attributes: R) -> Result<(), FormatError> {
        let length = attributes.seek(SeekFrom::End(0))?;
        if self.paged.is_none() && length != self.attribute_file_length {
            return Err(FormatError::InvalidFooter);
        }
        attributes.seek(SeekFrom::Start(0))?;
        let mut header = [0; HEADER_SIZE as usize];
        attributes.read_exact(&mut header)?;
        let attribute_identity = crate::version::parse_identity(&header)?;
        if attribute_identity.version != self.layout.version()
            || attribute_identity.role != FileRole::Attributes
        {
            return Err(FormatError::InvalidHeader);
        }
        if length < HEADER_SIZE + FOOTER_SIZE {
            return Err(FormatError::FileTooShort);
        }
        attributes.seek(SeekFrom::Start(length - FOOTER_SIZE))?;
        let mut footer = [0; FOOTER_SIZE as usize];
        attributes.read_exact(&mut footer)?;
        let (offset, index_length, declared_length, checksum) =
            parse_split_footer(&footer, self.layout.footer_magic())?;
        if declared_length != length || (offset, index_length, checksum) != self.index_identity {
            return Err(FormatError::InvalidMetadata(
                "attribute companion does not match primary index",
            ));
        }
        self.attributes = Some(attributes);
        self.attributes_attached = true;
        Ok(())
    }

    pub fn has_attributes(&self) -> bool {
        self.attributes_attached
    }

    /// Whether `id` stores schema column `index`: `Ok(false)` for an index beyond the schema and
    /// for a column the format omits from that block. An unknown block or a failing index lookup
    /// is an error, never `false`, so callers cannot skip a column on a real failure.
    pub fn attribute_present(&self, id: BlockId, index: u32) -> Result<bool, FormatError> {
        let entry = self.entry(id)?;
        Ok((index as usize)
            .checked_add(2)
            .and_then(|slot| entry.payloads.get(slot))
            .is_some_and(|payload| payload.stored.is_present()))
    }

    /// Nearest-neighbour distance quantiles stored in the header (`v8.N-pre`); `None` for
    /// formats that do not store them. Eager and lazy readers report the same values.
    pub fn point_distance_quantiles(&self) -> Option<crate::PointDistanceQuantiles> {
        match &self.paged {
            Some(source) => source.point_distance_quantiles(),
            None => self.metadata.point_distance_quantiles(),
        }
    }

    /// Per-cloud reference spacing (RF-05): stored in `v7.5-pre`, otherwise computed from every
    /// leaf of the resident hierarchy (always complete for the other formats).
    pub fn reference_spacing(&self) -> Option<f64> {
        match (&self.paged, self.stored_reference_spacing) {
            (Some(source), _) => Some(source.reference_spacing()),
            (None, Some(stored)) => Some(stored),
            (None, None) => self.metadata.dataset().hierarchy().reference_spacing(),
        }
    }

    /// Finite range over all node payloads, used consistently across LOD blocks.
    /// The last 64 component queries (including absent ranges) are cached across forked readers.
    /// The first query scans resident metadata; neither hits nor misses read payloads.
    pub fn attribute_range(
        &self,
        schema_index: u32,
        component: u32,
    ) -> Result<Option<[f64; 2]>, FormatError> {
        let descriptor = self
            .metadata
            .dataset()
            .hierarchy()
            .attributes()
            .get(schema_index as usize)
            .ok_or(FormatError::UnknownAttribute(schema_index))?;
        if component >= descriptor.component_count() {
            return Err(FormatError::InvalidSchema);
        }
        if let Some(source) = &self.paged {
            return source.attribute_range(schema_index, component);
        }
        let mut cache = self
            .attribute_range_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(position) = cache
            .iter()
            .position(|entry| entry.schema_index == schema_index && entry.component == component)
        {
            let entry = cache.remove(position).expect("located range cache entry");
            let range = entry.range;
            cache.push_back(entry);
            return Ok(range);
        }

        let mut result: Option<[f64; 2]> = None;
        for (_, ranges) in self.metadata.node_range_entries() {
            if let Some(range) = ranges
                .iter()
                .find(|range| range.schema_index() == schema_index)
            {
                let low = range
                    .min()
                    .scalar_as_f64(component as usize)
                    .ok_or(FormatError::InvalidSchema)?;
                let high = range
                    .max()
                    .scalar_as_f64(component as usize)
                    .ok_or(FormatError::InvalidSchema)?;
                if low.is_finite() && high.is_finite() {
                    result = Some(result.map_or([low, high], |r| [r[0].min(low), r[1].max(high)]));
                }
            }
        }
        if cache.len() == ATTRIBUTE_RANGE_CACHE_CAPACITY {
            cache.pop_front();
        }
        cache.push_back(CachedAttributeRange {
            schema_index,
            component,
            range: result,
        });
        Ok(result)
    }

    pub fn format_metadata(&self) -> &FormatMetadata {
        &self.metadata
    }

    /// Bytes of the serialized common index, stored only in the primary file.
    /// This is not an estimate of resident or peak memory used by the decoded index.
    pub fn index_size_bytes(&self) -> u64 {
        self.index_identity.1
    }

    /// Returns aggregate block sizes from the validated directory without payload I/O.
    /// Inspect `block_payload_storage_stats` for per-payload codecs and sizes.
    pub fn block_storage_stats(&self) -> Result<Vec<BlockStorageStats>, FormatError> {
        self.blocks
            .iter()
            .map(|(id, entry)| {
                Ok(BlockStorageStats {
                    id: *id,
                    point_count: entry.point_count,
                    stored_bytes: entry
                        .payloads
                        .iter()
                        .map(|payload| payload.stored_length)
                        .sum(),
                    raw_bytes: entry.payloads.iter().try_fold(0_u64, |sum, payload| {
                        sum.checked_add(payload.raw_length)
                            .ok_or(FormatError::BlockSizeOverflow)
                    })?,
                    logical_bytes: block_payload_size(
                        usize::try_from(entry.point_count)
                            .map_err(|_| FormatError::PointCountOutOfRange)?,
                        self.metadata.dataset().hierarchy().attributes(),
                        |column| {
                            entry
                                .payloads
                                .get(column)
                                .is_some_and(|payload| payload.stored.is_present())
                        },
                    )?,
                    geometry_encoding: entry.geometry_encoding,
                })
            })
            .collect()
    }

    /// Returns geometry and attribute payload sizes in block/schema order without I/O.
    pub fn block_payload_storage_stats(
        &self,
    ) -> Result<Vec<BlockPayloadStorageStats>, FormatError> {
        let schema = self.metadata.dataset().hierarchy().attributes();
        let mut stats = Vec::new();
        for (id, entry) in self.blocks.iter() {
            stats.try_reserve(entry.payloads.len()).map_err(|_| {
                FormatError::AllocationFailed(
                    entry.payloads.len() * std::mem::size_of::<BlockPayloadStorageStats>(),
                )
            })?;
            for (index, payload) in entry.payloads.iter().enumerate() {
                if !payload.stored.is_present() {
                    continue;
                }
                let kind = if index == 0 {
                    BlockPayloadKind::Geometry
                } else if index == 1 {
                    BlockPayloadKind::DisplayColor
                } else {
                    BlockPayloadKind::Attribute {
                        schema_index: (index - 2) as u32,
                        name: schema[index - 2].name().to_owned(),
                    }
                };
                stats.push(BlockPayloadStorageStats {
                    id: *id,
                    kind,
                    codec: payload.codec,
                    stored_bytes: payload.stored_length,
                    raw_bytes: payload.raw_length,
                });
            }
        }
        Ok(stats)
    }

    pub fn into_inner(self) -> R {
        self.reader
    }

    pub fn format_version(&self) -> crate::FormatVersion {
        self.layout.version()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn shares_index_with<S>(&self, other: &IndexedReader<S>) -> bool {
        Arc::ptr_eq(&self.blocks, &other.blocks) && Arc::ptr_eq(&self.metadata, &other.metadata)
    }

    /// Plans only the requested payloads from the validated index, without payload I/O.
    /// Estimates cover logical payload buffers, excluding the index, transport caches,
    /// allocator overhead, schema strings and small decoder control structures.
    pub fn plan_read(
        &self,
        id: BlockId,
        selection: crate::ReadSelection,
    ) -> Result<crate::ReadPlan, FormatError> {
        use crate::{ReadPlan, ReadSelection};
        let block = self.entry(id)?;
        let points_bytes = || {
            block
                .point_count
                .checked_mul(std::mem::size_of::<Point>() as u64)
                .ok_or(FormatError::BlockSizeOverflow)
        };
        let colors_bytes = || {
            block
                .point_count
                .checked_mul(std::mem::size_of::<ColorRgb>() as u64)
                .ok_or(FormatError::BlockSizeOverflow)
        };
        let mut ranges = Vec::new();
        ranges.try_reserve_exact(2).map_err(|_| {
            FormatError::AllocationFailed(2 * std::mem::size_of::<crate::PayloadReadRange>())
        })?;
        let memory = match selection {
            ReadSelection::Positions | ReadSelection::Geometry => {
                let range = self.payload_range(&block, stored_payload(&block, id, 0)?, false)?;
                let positions = crate::read_plan::estimate_payload(
                    &range,
                    points_bytes()?,
                    Some((block.geometry_encoding, block.point_count)),
                )?;
                ranges.push(range);
                if selection == ReadSelection::Geometry && self.has_attributes() {
                    let (payload, _) = self.display_colour_payload(&block, id)?;
                    let range = self.payload_range(&block, payload, true)?;
                    let colors = crate::read_plan::estimate_payload(&range, colors_bytes()?, None)?;
                    ranges.push(range);
                    crate::read_plan::combine_geometry(positions, colors)?
                } else {
                    positions
                }
            }
            ReadSelection::DisplayColors => {
                // A derived colour reads its source column, so the estimate counts that payload.
                let (payload, _) = self.display_colour_payload(&block, id)?;
                let range = self.payload_range(&block, payload, true)?;
                let memory = crate::read_plan::estimate_payload(&range, colors_bytes()?, None)?;
                ranges.push(range);
                memory
            }
            ReadSelection::Attribute { schema_index } => {
                let descriptor = self
                    .metadata
                    .dataset()
                    .hierarchy()
                    .attributes()
                    .get(schema_index as usize)
                    .ok_or(FormatError::UnknownAttribute(schema_index))?;
                let output_bytes = attribute_payload_size(block.point_count, descriptor)?;
                let range = self.payload_range(
                    &block,
                    stored_payload(&block, id, schema_index as usize + 2)?,
                    true,
                )?;
                let memory = crate::read_plan::estimate_payload(&range, output_bytes, None)?;
                ranges.push(range);
                memory
            }
        };
        Ok(ReadPlan::new(
            Arc::clone(&self.plan_owner),
            id,
            selection,
            ranges,
            memory,
        ))
    }

    /// Materializes a plan from externally fetched stored payload buffers. No file I/O occurs.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "consuming the plan and buffers keeps a plan from being materialized twice"
    )]
    pub fn materialize_selected_from_buffers(
        &self,
        plan: crate::ReadPlan,
        buffers: Vec<Vec<u8>>,
    ) -> Result<crate::SelectedRead, FormatError> {
        use crate::{ReadSelection as S, SelectedRead as O};
        if !Arc::ptr_eq(&plan.owner, &self.plan_owner) {
            return Err(FormatError::PlanReaderMismatch);
        }
        if buffers.len() != plan.ranges().len() {
            return Err(FormatError::PayloadBufferCount {
                expected: plan.ranges().len(),
                actual: buffers.len(),
            });
        }
        let id = plan.block_id();
        let selection = plan.selection();
        let entry = self.entry(id)?;
        let mut decoded = Vec::new();
        decoded
            .try_reserve_exact(buffers.len())
            .map_err(|_| FormatError::AllocationFailed(buffers.len()))?;
        for (range_index, (range, stored)) in plan.ranges().iter().zip(buffers).enumerate() {
            let expected = usize::try_from(range.stored_bytes())
                .map_err(|_| FormatError::SizeOutOfRange(range.stored_bytes()))?;
            if stored.len() != expected {
                return Err(FormatError::PayloadBufferLength {
                    expected,
                    actual: stored.len(),
                });
            }
            let attribute_width = match selection {
                crate::ReadSelection::Attribute { schema_index } => {
                    let descriptor = self
                        .metadata
                        .dataset()
                        .hierarchy()
                        .attributes()
                        .get(schema_index as usize)
                        .ok_or(FormatError::UnknownAttribute(schema_index))?;
                    Some(crate::layout::attribute_tuple_width(descriptor)?)
                }
                crate::ReadSelection::DisplayColors => {
                    Some(self.display_colour_payload(&entry, id)?.1.width)
                }
                crate::ReadSelection::Geometry if range_index == 1 => {
                    Some(self.display_colour_payload(&entry, id)?.1.width)
                }
                _ => None,
            };
            let raw = self.decode_stored_payload(id, range, stored, attribute_width)?;
            decoded.push(raw);
        }
        let count =
            usize::try_from(entry.point_count).map_err(|_| FormatError::PointCountOutOfRange)?;
        Ok(match selection {
            S::Positions => {
                let mut c = SliceCursor::new(&decoded[0]);
                O::Positions(self.decode_geometry(&mut c, &entry, count)?)
            }
            S::Geometry if decoded.len() == 1 => {
                let mut c = SliceCursor::new(&decoded[0]);
                O::Geometry(self.decode_geometry(&mut c, &entry, count)?)
            }
            S::Geometry => {
                let mut c = SliceCursor::new(&decoded[0]);
                let mut points = self.decode_geometry(&mut c, &entry, count)?.into_points();
                let colors = self.decode_display_colors(&decoded[1], &entry, id)?;
                for (p, color) in points.iter_mut().zip(colors) {
                    p.color = color;
                }
                O::Geometry(PointBatch::new(points))
            }
            S::DisplayColors => {
                O::DisplayColors(self.decode_display_colors(&decoded[0], &entry, id)?)
            }
            S::Attribute { schema_index } => {
                let descriptor =
                    self.metadata.dataset().hierarchy().attributes()[schema_index as usize].clone();
                let values = read_attribute_values(
                    &mut SliceCursor::new(&decoded[0]),
                    descriptor.data_type(),
                    count
                        .checked_mul(descriptor.component_count() as usize)
                        .ok_or_else(|| {
                            FormatError::AttributeLengthOverflow(descriptor.name().to_owned())
                        })?,
                )?;
                O::Attribute(AttributeColumn::new(descriptor, values, count)?)
            }
        })
    }

    fn payload_range(
        &self,
        block: &BlockDirectoryEntry,
        payload: &PayloadDirectoryEntry,
        is_attribute: bool,
    ) -> Result<crate::PayloadReadRange, FormatError> {
        let (source, offset) = if is_attribute {
            if !self.has_attributes() {
                return Err(FormatError::AttributesUnavailable);
            }
            (crate::ReadSource::Attributes, payload.offset)
        } else {
            (
                crate::ReadSource::Primary,
                block
                    .offset
                    .checked_add(payload.offset)
                    .ok_or(FormatError::FileSizeOverflow)?,
            )
        };
        for requested in [payload.stored_length, payload.raw_length] {
            if requested > self.limits.max_block_bytes {
                return Err(FormatError::BlockExceedsLimit {
                    requested,
                    limit: self.limits.max_block_bytes,
                });
            }
        }
        Ok(crate::PayloadReadRange::new(
            source,
            offset,
            payload.stored_length,
            payload.raw_length,
            payload.codec,
            payload.checksum,
        ))
    }

    /// Rejects requests exceeding a per-read payload-buffer estimate before payload I/O.
    /// This is not a shared concurrency quota or a process RSS limit. Replanning here
    /// avoids executing a caller-provided plan against a different dataset or companion.
    pub fn read_selected(
        &mut self,
        id: BlockId,
        selection: crate::ReadSelection,
        max_peak_bytes: u64,
    ) -> Result<crate::SelectedRead, FormatError> {
        let requested = self.plan_read(id, selection)?.memory().peak_bytes();
        if requested > max_peak_bytes {
            return Err(FormatError::ReadBudgetExceeded {
                requested,
                limit: max_peak_bytes,
            });
        }
        self.materialize_selected(id, selection)
    }

    pub fn try_read_selected(
        &mut self,
        id: BlockId,
        selection: crate::ReadSelection,
        budget: &crate::ReadBudget,
    ) -> Result<Option<crate::BudgetedRead>, FormatError> {
        let plan = self.plan_read(id, selection)?;
        let Some(reservation) = budget.try_reserve(plan.memory().peak_bytes())? else {
            return Ok(None);
        };
        self.complete_reserved_read(plan, reservation).map(Some)
    }

    pub fn read_reserved(
        &mut self,
        id: BlockId,
        selection: crate::ReadSelection,
        reservation: crate::ReadReservation,
    ) -> Result<crate::BudgetedRead, FormatError> {
        let plan = self.plan_read(id, selection)?;
        self.complete_reserved_read(plan, reservation)
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "consuming the plan and reservation keeps both single-use"
    )]
    fn complete_reserved_read(
        &mut self,
        plan: crate::ReadPlan,
        reservation: crate::ReadReservation,
    ) -> Result<crate::BudgetedRead, FormatError> {
        let requested = plan.memory().peak_bytes();
        if requested > reservation.bytes() {
            return Err(FormatError::ReadBudgetExceeded {
                requested,
                limit: reservation.bytes(),
            });
        }
        let result = self.materialize_selected(plan.block_id(), plan.selection())?;
        crate::BudgetedRead::new(result, reservation, plan.memory().output_bytes())
    }

    fn materialize_selected(
        &mut self,
        id: BlockId,
        selection: crate::ReadSelection,
    ) -> Result<crate::SelectedRead, FormatError> {
        use crate::{ReadSelection, SelectedRead};
        match selection {
            ReadSelection::Positions => self.read_positions(id).map(SelectedRead::Positions),
            ReadSelection::Geometry => self.read_geometry(id).map(SelectedRead::Geometry),
            ReadSelection::DisplayColors => self
                .read_display_colors(id)
                .map(SelectedRead::DisplayColors),
            ReadSelection::Attribute { schema_index } => self
                .read_attribute(id, schema_index)
                .map(SelectedRead::Attribute),
        }
    }

    /// Creates a reader over an independent cursor while sharing the validated immutable index.
    /// The supplied stream must refer to the same unchanged file snapshot as this reader.
    pub fn fork_reader<S: Read + Seek>(&self, reader: S) -> IndexedReader<S> {
        IndexedReader {
            reader,
            attributes: None,
            attributes_attached: false,
            external_payloads_only: false,
            layout: self.layout,
            index_identity: self.index_identity,
            attribute_file_length: self.attribute_file_length,
            metadata: Arc::clone(&self.metadata),
            blocks: Arc::clone(&self.blocks),
            paged: self.paged.clone(),
            stored_reference_spacing: self.stored_reference_spacing,
            plan_owner: Arc::clone(&self.plan_owner),
            attribute_range_cache: Arc::clone(&self.attribute_range_cache),
            limits: self.limits,
        }
    }

    /// Internal async transport supplies cursors over the same already validated open files.
    #[cfg(all(feature = "async-reader", not(target_arch = "wasm32")))]
    pub(crate) fn fork_with_cursors<S: Read + Seek>(
        &self,
        reader: S,
        attributes: Option<S>,
    ) -> IndexedReader<S> {
        let mut fork = self.fork_reader(reader);
        fork.attributes = attributes;
        fork.attributes_attached = fork.attributes.is_some();
        fork
    }

    /// Decodes a `v7.4-pre` block: exact `i64` quanta checked against the integer box of the
    /// node, plus the `f32` position local to the frame origin.
    fn decode_global_geometry(
        &self,
        cursor: &mut SliceCursor<'_>,
        entry: &BlockDirectoryEntry,
        cell: crate::global_frame::CellBox,
        point_count: usize,
        mut visit: impl FnMut([i64; 3], [f32; 3]),
    ) -> Result<(), FormatError> {
        let frame_origin = self
            .metadata
            .global_frame()
            .ok_or(FormatError::MalformedIndex("missing global frame"))?
            .frame_origin_quanta();
        let mut decoder = GeometryDecoder::with_count(
            entry.geometry_encoding,
            cursor.remaining_slice(),
            point_count,
        )?;
        for _ in 0..point_count {
            let offset = decoder.next_coordinate()?;
            let quanta: [i128; 3] = std::array::from_fn(|axis| {
                i128::from(entry.origin_quanta[axis]) + i128::from(offset[axis])
            });
            if !cell.contains(quanta) {
                return Err(FormatError::MalformedBlock(
                    "decoded position is outside its node",
                ));
            }
            let local = crate::global_frame::local_position_units(quanta, frame_origin);
            let local = local.map(|value| value as f32);
            if local.iter().any(|value| !value.is_finite()) {
                return Err(FormatError::MalformedBlock(
                    "decoded position is out of range",
                ));
            }
            let exact = quanta.map(|value| i64::try_from(value).expect("inside the node box"));
            visit(exact, local);
        }
        decoder.finish()
    }

    /// Exact `i64` quanta (2^-15 CRS units) of a `v7.4-pre` block, in row order.
    pub fn read_block_quanta(&mut self, id: BlockId) -> Result<Vec<[i64; 3]>, FormatError> {
        let entry = self.entry(id)?;
        let cell = entry.node_cell.ok_or(FormatError::InvalidMetadata(
            "exact quanta exist only in v7.4-pre",
        ))?;
        let raw = self.read_payload(id, &entry, stored_payload(&entry, id, 0)?, None)?;
        let count =
            usize::try_from(entry.point_count).map_err(|_| FormatError::PointCountOutOfRange)?;
        let mut quanta = Vec::new();
        quanta
            .try_reserve_exact(count)
            .map_err(|_| FormatError::AllocationFailed(count))?;
        let mut cursor = SliceCursor::new(&raw);
        self.decode_global_geometry(&mut cursor, &entry, cell, count, |exact, _| {
            quanta.push(exact);
        })?;
        Ok(quanta)
    }

    fn decode_geometry(
        &self,
        cursor: &mut SliceCursor<'_>,
        entry: &BlockDirectoryEntry,
        point_count: usize,
    ) -> Result<PointBatch, FormatError> {
        let cell = entry
            .node_cell
            .ok_or(FormatError::MalformedIndex("block has no node cell"))?;
        let point_bytes = allocation_capacity(point_count, std::mem::size_of::<Point>())?;
        let mut points = Vec::new();
        points
            .try_reserve_exact(point_count)
            .map_err(|_| FormatError::AllocationFailed(point_bytes))?;
        self.decode_global_geometry(cursor, entry, cell, point_count, |_, local| {
            points.push(Point::new(local, ColorRgb::new(160, 160, 160)));
        })?;
        Ok(PointBatch::new(points))
    }

    /// Loads XYZ and display RGB without reading attribute payloads.
    /// Row indices in the returned batch are stable within this immutable block.
    pub fn read_geometry(&mut self, id: BlockId) -> Result<PointBatch, FormatError> {
        let geometry = self.read_positions(id)?;
        if !self.has_attributes() {
            return Ok(geometry);
        }
        let colors = self.read_display_colors(id)?;
        let mut points = geometry.into_points();
        for (point, color) in points.iter_mut().zip(colors) {
            point.color = color;
        }
        Ok(PointBatch::new(points))
    }

    pub fn read_display_colors(&mut self, id: BlockId) -> Result<Vec<ColorRgb>, FormatError> {
        let entry = self.entry(id)?;
        let (payload, source) = self.display_colour_payload(&entry, id)?;
        let raw = self.read_payload(id, &entry, payload, Some(source.width))?;
        self.decode_display_colors(&raw, &entry, id)
    }

    /// The payload a block's display colour is read from: its own column 1, or for a derived colour
    /// (`v8.N-pre` leaves) the 16-bit source column named by the header.
    fn display_colour_payload<'a>(
        &self,
        block: &'a BlockDirectoryEntry,
        id: BlockId,
    ) -> Result<(&'a PayloadDirectoryEntry, DisplaySource), FormatError> {
        let StoredColumn::Derived { from } = block
            .payloads
            .get(1)
            .map_or(StoredColumn::Present, |payload| payload.stored)
        else {
            let source = DisplaySource {
                width: 3,
                derived_components: None,
            };
            return Ok((stored_payload(block, id, 1)?, source));
        };
        let descriptor = from
            .checked_sub(2)
            .and_then(|schema| self.metadata.dataset().hierarchy().attributes().get(schema))
            .filter(|descriptor| {
                descriptor.data_type() == AttributeDataType::U16
                    && matches!(descriptor.component_count(), 1 | 3)
            })
            .ok_or(FormatError::MalformedIndex(
                "display colour source is not a 16-bit column",
            ))?;
        let source = DisplaySource {
            width: crate::layout::attribute_tuple_width(descriptor)?,
            derived_components: Some(descriptor.component_count()),
        };
        Ok((stored_payload(block, id, from)?, source))
    }

    /// Display colours from the decoded bytes of [`Self::display_colour_payload`].
    fn decode_display_colors(
        &self,
        raw: &[u8],
        block: &BlockDirectoryEntry,
        id: BlockId,
    ) -> Result<Vec<ColorRgb>, FormatError> {
        let (_, source) = self.display_colour_payload(block, id)?;
        let Some(components) = source.derived_components else {
            return Ok(raw
                .as_chunks::<3>()
                .0
                .iter()
                .map(|rgb| ColorRgb::new(rgb[0], rgb[1], rgb[2]))
                .collect());
        };
        let count = usize::try_from(block.point_count)
            .map_err(|_| FormatError::PointCountOutOfRange)?
            .checked_mul(components as usize)
            .ok_or(FormatError::PointCountOutOfRange)?;
        let mut cursor = SliceCursor::new(raw);
        let AttributeValues::U16(values) =
            read_attribute_values(&mut cursor, AttributeDataType::U16, count)?
        else {
            unreachable!("read as u16");
        };
        if cursor.remaining() != 0 {
            return Err(FormatError::MalformedBlock("trailing display colour bytes"));
        }
        Ok(derive_display_colors(&values, components))
    }

    /// Reads only XYZ; supplies neutral gray without accessing color or attribute payloads.
    pub fn read_positions(&mut self, id: BlockId) -> Result<PointBatch, FormatError> {
        let entry = self.entry(id)?;
        let raw = self.read_payload(id, &entry, stored_payload(&entry, id, 0)?, None)?;
        let mut cursor = SliceCursor::new(&raw);
        let count =
            usize::try_from(entry.point_count).map_err(|_| FormatError::PointCountOutOfRange)?;
        let points = self.decode_geometry(&mut cursor, &entry, count)?;
        Ok(points)
    }

    /// Loads one complete attribute column by its index in the shared dataset schema.
    /// This reads no geometry or other attribute payloads.
    pub fn read_attribute(
        &mut self,
        id: BlockId,
        schema_index: u32,
    ) -> Result<AttributeColumn, FormatError> {
        let entry = self.entry(id)?;
        let descriptor = self
            .metadata
            .dataset()
            .hierarchy()
            .attributes()
            .get(schema_index as usize)
            .ok_or(FormatError::UnknownAttribute(schema_index))?
            .clone();
        let payload_index = (schema_index as usize)
            .checked_add(2)
            .ok_or(FormatError::UnknownAttribute(schema_index))?;
        let width = crate::layout::attribute_tuple_width(&descriptor)?;
        let raw = self.read_payload(
            id,
            &entry,
            stored_payload(&entry, id, payload_index)?,
            Some(width),
        )?;
        let count =
            usize::try_from(entry.point_count).map_err(|_| FormatError::PointCountOutOfRange)?;
        let value_count = count
            .checked_mul(descriptor.component_count() as usize)
            .ok_or_else(|| FormatError::AttributeLengthOverflow(descriptor.name().to_owned()))?;
        let mut cursor = SliceCursor::new(&raw);
        let values = read_attribute_values(&mut cursor, descriptor.data_type(), value_count)?;
        if cursor.remaining() != 0 {
            return Err(FormatError::MalformedBlock("trailing attribute bytes"));
        }
        Ok(AttributeColumn::new(descriptor, values, count)?)
    }

    /// Resolves (`block_id`, `row_id`) to the requested attribute components.
    /// Compression granularity is one column per block, so this decodes that whole column.
    pub fn read_attribute_row(
        &mut self,
        id: BlockId,
        row_id: u64,
        schema_index: u32,
    ) -> Result<AttributeValues, FormatError> {
        let entry = self.entry(id)?;
        if row_id >= entry.point_count {
            return Err(FormatError::UnknownRow { block: id, row_id });
        }
        let column = self.read_attribute(id, schema_index)?;
        let width = column.descriptor().component_count() as usize;
        let row = usize::try_from(row_id).map_err(|_| FormatError::PointCountOutOfRange)?;
        Ok(column.values().select_rows(&[row], width)?)
    }

    fn read_payload(
        &mut self,
        id: BlockId,
        block: &BlockDirectoryEntry,
        column: &PayloadDirectoryEntry,
        attribute_width: Option<usize>,
    ) -> Result<Vec<u8>, FormatError> {
        if self.external_payloads_only {
            return Err(FormatError::ExternalPayloadBuffersRequired);
        }
        let range = self.payload_range(block, column, attribute_width.is_some())?;
        let size = usize::try_from(column.stored_length)
            .map_err(|_| FormatError::SizeOutOfRange(column.stored_length))?;
        let mut stored = Vec::new();
        stored
            .try_reserve_exact(size)
            .map_err(|_| FormatError::AllocationFailed(size))?;
        stored.resize(size, 0);
        let reader = match range.source() {
            crate::ReadSource::Attributes => self
                .attributes
                .as_mut()
                .ok_or(FormatError::AttributesUnavailable)?,
            crate::ReadSource::Primary => &mut self.reader,
        };
        reader.seek(SeekFrom::Start(range.offset()))?;
        reader.read_exact(&mut stored)?;
        self.decode_stored_payload(id, &range, stored, attribute_width)
    }

    fn decode_stored_payload(
        &self,
        id: BlockId,
        range: &crate::PayloadReadRange,
        stored: Vec<u8>,
        attribute_width: Option<usize>,
    ) -> Result<Vec<u8>, FormatError> {
        let expected = usize::try_from(range.stored_bytes())
            .map_err(|_| FormatError::SizeOutOfRange(range.stored_bytes()))?;
        if stored.len() != expected {
            return Err(FormatError::PayloadBufferLength {
                expected,
                actual: stored.len(),
            });
        }
        if !checksum_matches(&stored, range.checksum()) {
            return Err(FormatError::InvalidBlockChecksum(id));
        }
        let raw_size = usize::try_from(range.decoded_bytes())
            .map_err(|_| FormatError::SizeOutOfRange(range.decoded_bytes()))?;
        if range.codec() == BlockCodec::None {
            Ok(stored)
        } else if let Some(width) = attribute_width {
            crate::attribute_codec::decode(
                range.codec(),
                &stored,
                raw_size,
                width,
                usize::try_from(self.limits.max_block_bytes).unwrap_or(usize::MAX),
            )
        } else {
            Ok(decompress_block(
                range.codec(),
                &stored,
                raw_size,
                usize::try_from(self.limits.max_block_bytes).unwrap_or(usize::MAX),
            )?)
        }
    }

    /// Geometry plus the attribute columns the block stores, in schema order. A format revision
    /// may omit some columns from a block, so identify a column by its descriptor, not by position.
    pub(crate) fn read_point_block(&mut self, id: BlockId) -> Result<PointBlock, FormatError> {
        let points = self.read_geometry(id)?;
        let schema_count = if self.has_attributes() {
            self.metadata.dataset().hierarchy().attributes().len()
        } else {
            0
        };
        let mut attributes = Vec::new();
        attributes
            .try_reserve_exact(schema_count)
            .map_err(|_| FormatError::AllocationFailed(schema_count))?;
        for schema_index in 0..schema_count {
            let schema_index = schema_index as u32;
            // A block may omit columns; the points are counted by the geometry, not by them.
            if self.attribute_present(id, schema_index)? {
                attributes.push(self.read_attribute(id, schema_index)?);
            }
        }
        Ok(PointBlock::new(id, points, attributes))
    }
}

impl<R: Read + Seek> DatasetSource for IndexedReader<R> {
    type Error = FormatError;

    fn metadata(&self) -> &DatasetMetadata {
        self.metadata.dataset()
    }

    fn read_block(&mut self, id: BlockId) -> Result<PointBlock, Self::Error> {
        self.read_point_block(id)
    }
}

/// Length a split companion must have: header, the furthest attribute column and the footer.
fn split_attribute_file_length(
    blocks: &BTreeMap<BlockId, BlockDirectoryEntry>,
) -> Result<u64, FormatError> {
    blocks
        .values()
        .flat_map(|block| block.payloads.iter().skip(1))
        .filter(|payload| payload.stored.is_present())
        .try_fold(HEADER_SIZE, |end, payload| {
            payload
                .offset
                .checked_add(payload.stored_length)
                .map(|next| end.max(next))
                .ok_or(FormatError::FileSizeOverflow)
        })?
        .checked_add(FOOTER_SIZE)
        .ok_or(FormatError::FileSizeOverflow)
}

fn validate_supported_primary(
    identity: crate::FormatIdentity,
) -> Result<StorageLayout, FormatError> {
    if !matches!(identity.version.major(), 5..=8) {
        return Err(FormatError::UnsupportedVersion(identity.version.major()));
    }
    let layout = StorageLayout::from_version(identity.version)?;
    if identity.role == FileRole::Attributes {
        return Err(FormatError::InvalidHeader);
    }
    Ok(layout)
}

/// How a block's display colour is read.
struct DisplaySource {
    /// Byte-plane width of the column read.
    width: usize,
    /// Components of the 16-bit source column (1 intensity, 3 RGB) when the colour is derived.
    derived_components: Option<u32>,
}

/// The display colour of a leaf that does not store it: the high byte of each 16-bit channel, or of
/// the single value for r = g = b. Shared with the writer's check, so both sides agree by construction.
pub(crate) fn derived_display_colors(
    values: &[u16],
    components: u32,
) -> impl ExactSizeIterator<Item = ColorRgb> + '_ {
    let step = if components == 3 { 3 } else { 1 };
    values.chunks_exact(step).map(move |chunk| {
        if step == 3 {
            ColorRgb::new(
                (chunk[0] >> 8) as u8,
                (chunk[1] >> 8) as u8,
                (chunk[2] >> 8) as u8,
            )
        } else {
            let gray = (chunk[0] >> 8) as u8;
            ColorRgb::new(gray, gray, gray)
        }
    })
}

pub(crate) fn derive_display_colors(values: &[u16], components: u32) -> Vec<ColorRgb> {
    derived_display_colors(values, components).collect()
}

pub(crate) fn read_attribute_values(
    cursor: &mut SliceCursor<'_>,
    data_type: AttributeDataType,
    count: usize,
) -> Result<AttributeValues, FormatError> {
    macro_rules! read_values {
        ($ty:ty, $read:ident) => {{
            let mut values = Vec::new();
            values
                .try_reserve_exact(count)
                .map_err(|_| FormatError::AllocationFailed(count * std::mem::size_of::<$ty>()))?;
            for _ in 0..count {
                values.push(cursor.$read()?);
            }
            values
        }};
    }
    Ok(match data_type {
        AttributeDataType::U8 => AttributeValues::U8(read_values!(u8, u8)),
        AttributeDataType::I8 => AttributeValues::I8(read_values!(i8, i8)),
        AttributeDataType::U16 => AttributeValues::U16(read_values!(u16, u16)),
        AttributeDataType::I16 => AttributeValues::I16(read_values!(i16, i16)),
        AttributeDataType::U32 => AttributeValues::U32(read_values!(u32, u32)),
        AttributeDataType::I32 => AttributeValues::I32(read_values!(i32, i32)),
        AttributeDataType::U64 => AttributeValues::U64(read_values!(u64, u64)),
        AttributeDataType::I64 => AttributeValues::I64(read_values!(i64, i64)),
        AttributeDataType::F32 => AttributeValues::F32(read_values!(f32, f32)),
        AttributeDataType::F64 => AttributeValues::F64(read_values!(f64, f64)),
    })
}

fn allocation_capacity(count: usize, item_size: usize) -> Result<usize, FormatError> {
    count
        .checked_mul(item_size)
        .ok_or(FormatError::BlockSizeOverflow)?;
    Ok(count)
}

#[cfg(test)]
#[path = "reader_tests.rs"]
mod tests;
