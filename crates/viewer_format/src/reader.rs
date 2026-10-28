use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Seek, SeekFrom},
    sync::{Arc, Mutex},
};

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeValues, BlockId, ColorRgb, DatasetMetadata,
    DatasetSource, Point, PointBatch, PointBlock, SpatialBounds,
};

use crate::geometry::{GeometryDecoder, GeometryEncoding};
use crate::layout::{
    FOOTER_SIZE, HEADER_SIZE, PayloadDirectoryEntry, StorageLayout, attribute_payload_size,
    block_payload_size,
};
use crate::version::FileRole;
use crate::{BlockCodec, FormatError, FormatMetadata, crc32, decompress_block};

use crate::index_codec::{
    BlockDirectoryEntry, parse_index, parse_split_footer, validate_index_range,
};
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

/// Seekable reader that loads metadata at open and verifies only requested block payloads.
pub(crate) struct IndexedReader<R> {
    reader: R,
    attributes: Option<R>,
    attributes_attached: bool,
    external_payloads_only: bool,
    split: bool,
    layout: StorageLayout,
    index_identity: (u64, u64, u32),
    attribute_file_length: u64,
    metadata: Arc<FormatMetadata>,
    blocks: Arc<BTreeMap<BlockId, BlockDirectoryEntry>>,
    block_bounds: Arc<BTreeMap<BlockId, Vec<SpatialBounds>>>,
    plan_owner: Arc<()>,
    attribute_range_cache: Arc<Mutex<VecDeque<CachedAttributeRange>>>,
    limits: FormatReaderLimits,
}

impl<R: Read + Seek> IndexedReader<R> {
    pub(crate) fn from_open_parts(
        reader: R,
        file_length: u64,
        header: &[u8],
        footer: &[u8],
        index: &[u8],
        limits: FormatReaderLimits,
    ) -> Result<Self, FormatError> {
        let header: &[u8; HEADER_SIZE as usize] =
            header.try_into().map_err(|_| FormatError::InvalidHeader)?;
        let identity = crate::version::parse_identity(header)?;
        let layout = validate_supported_primary(identity)?;
        if identity.role != FileRole::SplitPrimary {
            return Err(FormatError::InvalidHeader);
        }
        let footer: &[u8; FOOTER_SIZE as usize] =
            footer.try_into().map_err(|_| FormatError::InvalidFooter)?;
        let (index_offset, index_length, declared_length, checksum) =
            parse_split_footer(footer, layout.footer_magic())?;
        validate_index_range(
            file_length,
            index_offset,
            index_length,
            declared_length,
            limits,
        )?;
        if usize::try_from(index_length).ok() != Some(index.len()) {
            return Err(FormatError::InvalidFooter);
        }
        if crc32(index) != checksum {
            return Err(FormatError::InvalidChecksum("index"));
        }
        let (metadata, blocks, block_bounds) =
            parse_index(index, &limits, index_offset, true, layout)?;
        let attribute_file_length = blocks
            .values()
            .flat_map(|b| b.payloads.iter().skip(1))
            .try_fold(HEADER_SIZE, |end, p| {
                p.offset
                    .checked_add(p.stored_length)
                    .map(|n| end.max(n))
                    .ok_or(FormatError::FileSizeOverflow)
            })?
            .checked_add(FOOTER_SIZE)
            .ok_or(FormatError::FileSizeOverflow)?;
        Ok(Self {
            reader,
            attributes: None,
            split: true,
            layout,
            index_identity: (index_offset, index_length, checksum),
            attribute_file_length,
            metadata: Arc::new(metadata),
            blocks: Arc::new(blocks),
            block_bounds: Arc::new(block_bounds),
            attributes_attached: false,
            external_payloads_only: true,
            plan_owner: Arc::new(()),
            attribute_range_cache: Arc::new(Mutex::new(VecDeque::new())),
            limits,
        })
    }

    pub(crate) fn attach_attribute_identity(
        &mut self,
        file_length: u64,
        header: &[u8],
        footer: &[u8],
    ) -> Result<(), FormatError> {
        if !self.split {
            return Err(FormatError::InvalidMetadata(
                "monolithic file cannot attach attributes",
            ));
        }
        if !self.external_payloads_only {
            return Err(FormatError::InvalidMetadata(
                "attribute identity requires a metadata-only reader",
            ));
        }
        if file_length != self.attribute_file_length {
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

    #[cfg(test)]
    pub fn new(reader: R) -> Result<Self, FormatError> {
        Self::with_limits(reader, FormatReaderLimits::default())
    }

    #[cfg(test)]
    pub fn with_limits(mut reader: R, limits: FormatReaderLimits) -> Result<Self, FormatError> {
        reader.seek(SeekFrom::Start(0))?;
        let mut header = [0; HEADER_SIZE as usize];
        reader.read_exact(&mut header)?;
        let identity = crate::version::parse_identity(&header)?;
        Self::with_identity(reader, limits, identity)
    }

    pub(crate) fn with_identity(
        mut reader: R,
        limits: FormatReaderLimits,
        identity: crate::FormatIdentity,
    ) -> Result<Self, FormatError> {
        let layout = validate_supported_primary(identity)?;
        let file_length = reader.seek(SeekFrom::End(0))?;
        if file_length < HEADER_SIZE + FOOTER_SIZE {
            return Err(FormatError::FileTooShort);
        }
        if identity.role == FileRole::Attributes {
            return Err(FormatError::InvalidHeader);
        }
        let split = identity.role == FileRole::SplitPrimary;

        let footer_offset = file_length - FOOTER_SIZE;
        reader.seek(SeekFrom::Start(footer_offset))?;
        let mut footer = [0; FOOTER_SIZE as usize];
        reader.read_exact(&mut footer)?;
        let (index_offset, index_length, declared_file_length, index_checksum) =
            parse_split_footer(&footer, layout.footer_magic())?;
        if index_length > limits.max_index_bytes {
            return Err(FormatError::IndexExceedsLimit {
                requested: index_length,
                limit: limits.max_index_bytes,
            });
        }
        validate_index_range(
            file_length,
            index_offset,
            index_length,
            declared_file_length,
            limits,
        )?;
        let index_size =
            usize::try_from(index_length).map_err(|_| FormatError::SizeOutOfRange(index_length))?;
        let mut index = Vec::new();
        index
            .try_reserve_exact(index_size)
            .map_err(|_| FormatError::AllocationFailed(index_size))?;
        index.resize(index_size, 0);
        reader.seek(SeekFrom::Start(index_offset))?;
        reader.read_exact(&mut index)?;
        if crc32(&index) != index_checksum {
            return Err(FormatError::InvalidChecksum("index"));
        }

        let (metadata, blocks, block_bounds) =
            parse_index(&index, &limits, index_offset, split, layout)?;
        let attribute_file_length = if split {
            blocks
                .values()
                .flat_map(|block| block.payloads.iter().skip(1))
                .try_fold(HEADER_SIZE, |end, p| {
                    p.offset
                        .checked_add(p.stored_length)
                        .map(|next| end.max(next))
                        .ok_or(FormatError::FileSizeOverflow)
                })?
                .checked_add(FOOTER_SIZE)
                .ok_or(FormatError::FileSizeOverflow)?
        } else {
            0
        };
        Ok(Self {
            reader,
            attributes: None,
            attributes_attached: false,
            external_payloads_only: false,
            split,
            layout,
            index_identity: (index_offset, index_length, index_checksum),
            attribute_file_length,
            metadata: Arc::new(metadata),
            blocks: Arc::new(blocks),
            block_bounds: Arc::new(block_bounds),
            plan_owner: Arc::new(()),
            attribute_range_cache: Arc::new(Mutex::new(VecDeque::new())),
            limits,
        })
    }

    /// Attach a committed companion belonging to this primary index. Payload CRCs are
    /// checked lazily on access; binding protects accidental mismatch, not forgery.
    pub fn attach_attributes(&mut self, mut attributes: R) -> Result<(), FormatError> {
        if !self.split {
            return Err(FormatError::InvalidMetadata(
                "monolithic file cannot attach attributes",
            ));
        }
        let length = attributes.seek(SeekFrom::End(0))?;
        if length != self.attribute_file_length {
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
        !self.split || self.attributes_attached
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
        let block = self.blocks.get(&id).ok_or(FormatError::UnknownBlock(id))?;
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
                let range = self.payload_range(block, &block.payloads[0], false)?;
                let positions = crate::read_plan::estimate_payload(
                    &range,
                    points_bytes()?,
                    Some((block.geometry_encoding, block.point_count)),
                )?;
                ranges.push(range);
                if selection == ReadSelection::Geometry && self.has_attributes() {
                    let range = self.payload_range(block, &block.payloads[1], true)?;
                    let colors = crate::read_plan::estimate_payload(&range, colors_bytes()?, None)?;
                    ranges.push(range);
                    crate::read_plan::combine_geometry(positions, colors)?
                } else {
                    positions
                }
            }
            ReadSelection::DisplayColors => {
                let range = self.payload_range(block, &block.payloads[1], true)?;
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
                let range =
                    self.payload_range(block, &block.payloads[schema_index as usize + 2], true)?;
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
    pub fn materialize_selected_from_buffers(
        &self,
        plan: crate::ReadPlan,
        buffers: Vec<Vec<u8>>,
    ) -> Result<crate::SelectedRead, FormatError> {
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
        let entry = self.blocks.get(&id).ok_or(FormatError::UnknownBlock(id))?;
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
                crate::ReadSelection::DisplayColors => Some(3),
                crate::ReadSelection::Geometry if range_index == 1 => Some(3),
                _ => None,
            };
            let raw = self.decode_stored_payload(id, range, stored, attribute_width)?;
            decoded.push(raw);
        }
        let count =
            usize::try_from(entry.point_count).map_err(|_| FormatError::PointCountOutOfRange)?;
        use crate::{ReadSelection as S, SelectedRead as O};
        Ok(match selection {
            S::Positions => {
                let mut c = SliceCursor::new(&decoded[0]);
                O::Positions(self.decode_geometry(&mut c, id, entry, count)?)
            }
            S::Geometry if decoded.len() == 1 => {
                let mut c = SliceCursor::new(&decoded[0]);
                O::Geometry(self.decode_geometry(&mut c, id, entry, count)?)
            }
            S::Geometry => {
                let mut c = SliceCursor::new(&decoded[0]);
                let mut points = self
                    .decode_geometry(&mut c, id, entry, count)?
                    .into_points();
                for (p, rgb) in points.iter_mut().zip(decoded[1].as_chunks::<3>().0) {
                    p.color = ColorRgb::new(rgb[0], rgb[1], rgb[2]);
                }
                O::Geometry(PointBatch::new(points))
            }
            S::DisplayColors => O::DisplayColors(
                decoded[0]
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|v| ColorRgb::new(v[0], v[1], v[2]))
                    .collect(),
            ),
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
        let (source, offset) = if self.split && is_attribute {
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
            split: self.split,
            layout: self.layout,
            index_identity: self.index_identity,
            attribute_file_length: self.attribute_file_length,
            metadata: Arc::clone(&self.metadata),
            blocks: Arc::clone(&self.blocks),
            block_bounds: Arc::clone(&self.block_bounds),
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

    fn decode_geometry(
        &self,
        cursor: &mut SliceCursor<'_>,
        id: BlockId,
        entry: &BlockDirectoryEntry,
        point_count: usize,
    ) -> Result<PointBatch, FormatError> {
        let mut decoder = GeometryDecoder::with_count(
            entry.geometry_encoding,
            cursor.remaining_slice(),
            point_count,
        )?;
        let point_bytes = allocation_capacity(point_count, std::mem::size_of::<Point>())?;
        let mut points = Vec::new();
        points
            .try_reserve_exact(point_count)
            .map_err(|_| FormatError::AllocationFailed(point_bytes))?;
        for _ in 0..point_count {
            let quantized = decoder.next_coordinate()?;
            let position = std::array::from_fn(|axis| {
                (entry.origin[axis] + f64::from(quantized[axis]) * entry.scale[axis]) as f32
            });
            if position.iter().any(|value| !value.is_finite()) {
                return Err(FormatError::MalformedBlock(
                    "decoded position is out of range",
                ));
            }
            let origin = self.metadata.dataset().coordinate_frame().origin();
            let world = std::array::from_fn(|axis| origin[axis] + f64::from(position[axis]));
            let bounds = self
                .block_bounds
                .get(&id)
                .ok_or(FormatError::MalformedIndex("block has no node bounds"))?;
            if world.iter().any(|coordinate| !coordinate.is_finite())
                || bounds.iter().any(|bounds| !contains_point(*bounds, world))
            {
                return Err(FormatError::MalformedBlock(
                    "decoded position is outside its node bounds",
                ));
            }
            points.push(Point::new(position, ColorRgb::new(160, 160, 160)));
        }
        decoder.finish()?;

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
        let blocks = Arc::clone(&self.blocks);
        let entry = blocks.get(&id).ok_or(FormatError::UnknownBlock(id))?;
        let raw = self.read_payload(id, entry, &entry.payloads[1], Some(3))?;
        Ok(raw
            .as_chunks::<3>()
            .0
            .iter()
            .map(|rgb| ColorRgb::new(rgb[0], rgb[1], rgb[2]))
            .collect())
    }

    /// Reads only XYZ; supplies neutral gray without accessing color or attribute payloads.
    pub fn read_positions(&mut self, id: BlockId) -> Result<PointBatch, FormatError> {
        let blocks = Arc::clone(&self.blocks);
        let entry = blocks.get(&id).ok_or(FormatError::UnknownBlock(id))?;
        let raw = self.read_payload(id, entry, &entry.payloads[0], None)?;
        let mut cursor = SliceCursor::new(&raw);
        let count =
            usize::try_from(entry.point_count).map_err(|_| FormatError::PointCountOutOfRange)?;
        let points = self.decode_geometry(&mut cursor, id, entry, count)?;
        Ok(points)
    }

    /// Loads one complete attribute column by its index in the shared dataset schema.
    /// This reads no geometry or other attribute payloads.
    pub fn read_attribute(
        &mut self,
        id: BlockId,
        schema_index: u32,
    ) -> Result<AttributeColumn, FormatError> {
        let blocks = Arc::clone(&self.blocks);
        let entry = blocks.get(&id).ok_or(FormatError::UnknownBlock(id))?;
        let descriptor = self
            .metadata
            .dataset()
            .hierarchy()
            .attributes()
            .get(schema_index as usize)
            .ok_or(FormatError::UnknownAttribute(schema_index))?
            .clone();
        let width = crate::layout::attribute_tuple_width(&descriptor)?;
        let raw = self.read_payload(
            id,
            entry,
            &entry.payloads[schema_index as usize + 2],
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

    /// Resolves (block_id, row_id) to the requested attribute components.
    /// Compression granularity is one column per block, so this decodes that whole column.
    pub fn read_attribute_row(
        &mut self,
        id: BlockId,
        row_id: u64,
        schema_index: u32,
    ) -> Result<AttributeValues, FormatError> {
        let entry = self.blocks.get(&id).ok_or(FormatError::UnknownBlock(id))?;
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
        if crc32(&stored) != range.checksum() {
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
            attributes.push(self.read_attribute(id, schema_index as u32)?);
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

fn validate_supported_primary(
    identity: crate::FormatIdentity,
) -> Result<StorageLayout, FormatError> {
    if !matches!(identity.version.major(), 5..=7) {
        return Err(FormatError::UnsupportedVersion(identity.version.major()));
    }
    let layout = StorageLayout::from_version(identity.version)?;
    if identity.role == FileRole::Attributes {
        return Err(FormatError::InvalidHeader);
    }
    Ok(layout)
}

pub(crate) fn contains_point(bounds: SpatialBounds, point: [f64; 3]) -> bool {
    let min = bounds.min();
    let max = bounds.max();
    (0..3).all(|axis| point[axis] >= min[axis] && point[axis] <= max[axis])
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
