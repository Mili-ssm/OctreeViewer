use std::io::{Cursor, Read, Seek};

use viewer_core::{
    AttributeColumn, AttributeValues, BlockId, ColorRgb, DatasetMetadata, DatasetSource,
    PointBatch, PointBlock,
};

use crate::reader::{
    BlockPayloadStorageStats, BlockStorageStats, FormatReaderLimits, IndexedReader,
};
use crate::{
    FormatError, FormatIdentity, FormatMetadata, FormatVersion, OctreeReader, probe_format,
};

/// Format-dispatching reader over an immutable seekable storage stream.
pub struct FormatReader<R> {
    backend: IndexedReader<R>,
}

impl<R: Read + Seek> FormatReader<R> {
    pub fn new(reader: R) -> Result<Self, FormatError> {
        Self::with_limits(reader, FormatReaderLimits::default())
    }

    pub fn with_limits(mut reader: R, limits: FormatReaderLimits) -> Result<Self, FormatError> {
        let identity = probe_format(&mut reader)
            .map_err(|error| crate::explain_open_failure(&mut reader, error))?;
        // The reader stays borrowed while the index loads, so a failure can still read the envelope
        // to name a retired family; a successful open never reads it again.
        let loaded = validate_identity(identity)
            .and_then(|()| IndexedReader::load_identity(&mut reader, &limits, identity));
        match loaded {
            Ok(loaded) => Ok(Self {
                backend: IndexedReader::from_loaded(reader, limits, loaded),
            }),
            Err(error) => Err(crate::explain_open_failure(&mut reader, error)),
        }
    }

    fn backend(&self) -> &IndexedReader<R> {
        &self.backend
    }

    fn backend_mut(&mut self) -> &mut IndexedReader<R> {
        &mut self.backend
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn shares_index_with<S: Read + Seek>(&self, other: &FormatReader<S>) -> bool {
        self.backend.shares_index_with(&other.backend)
    }

    pub fn format_metadata(&self) -> &FormatMetadata {
        self.backend().format_metadata()
    }

    /// Opens a V8 primary file lazily: one tail read brings the footer, metadata block and
    /// root page; child pages come from [`Self::read_page`]. `format_metadata()` then holds only
    /// the root page's nodes.
    pub fn open_lazy(mut reader: R, limits: FormatReaderLimits) -> Result<Self, FormatError> {
        let core = crate::paged_index::open_core(&mut reader, limits)?;
        Ok(Self {
            backend: IndexedReader::from_paged_core(reader, core, false)?,
        })
    }

    /// The page registry of a lazily opened reader; `None` for eager formats.
    pub fn page_source(&self) -> Option<&std::sync::Arc<crate::PageSource>> {
        self.backend().page_source()
    }

    /// Reads and validates the page hanging from `anchor`, registering its blocks.
    pub fn read_page(
        &mut self,
        anchor: viewer_core::NodeId,
    ) -> Result<crate::PageGraft, FormatError> {
        self.backend_mut().read_page(anchor)
    }

    pub fn index_size_bytes(&self) -> u64 {
        self.backend().index_size_bytes()
    }

    pub fn block_storage_stats(&self) -> Result<Vec<BlockStorageStats>, FormatError> {
        self.backend().block_storage_stats()
    }

    pub fn block_payload_storage_stats(
        &self,
    ) -> Result<Vec<BlockPayloadStorageStats>, FormatError> {
        self.backend().block_payload_storage_stats()
    }

    pub fn into_inner(self) -> R {
        self.backend.into_inner()
    }

    /// Creates a reader over another cursor and shares the validated index.
    pub fn fork_reader<S: Read + Seek>(&self, reader: S) -> FormatReader<S> {
        FormatReader {
            backend: self.backend().fork_reader(reader),
        }
    }

    #[cfg(all(feature = "async-reader", not(target_arch = "wasm32")))]
    pub(crate) fn fork_with_cursors<S: Read + Seek>(
        &self,
        reader: S,
        attributes: Option<S>,
    ) -> FormatReader<S> {
        FormatReader {
            backend: self.backend.fork_with_cursors(reader, attributes),
        }
    }

    pub fn attach_attributes(&mut self, attributes: R) -> Result<(), FormatError> {
        self.backend_mut().attach_attributes(attributes)
    }

    pub fn has_attributes(&self) -> bool {
        self.backend().has_attributes()
    }

    /// Whether `id` stores schema column `index`; see `IndexedReader::attribute_present`.
    pub fn attribute_present(&self, id: BlockId, index: u32) -> Result<bool, FormatError> {
        self.backend().attribute_present(id, index)
    }

    /// Builds a selective range and payload-buffer plan without payload I/O.
    /// Metadata, transport caches and allocator/control overhead are outside its estimate.
    pub fn plan_read(
        &self,
        id: BlockId,
        selection: crate::ReadSelection,
    ) -> Result<crate::ReadPlan, FormatError> {
        self.backend().plan_read(id, selection)
    }

    /// Decodes externally fetched buffers for a plan created by this reader without file I/O.
    pub fn materialize_selected_from_buffers(
        &self,
        plan: crate::ReadPlan,
        buffers: Vec<Vec<u8>>,
    ) -> Result<crate::SelectedRead, FormatError> {
        self.backend()
            .materialize_selected_from_buffers(plan, buffers)
    }

    /// Executes a selective read after checking its estimated peak payload bytes.
    /// The budget is per invocation; callers must account for concurrent/retained results.
    pub fn read_selected(
        &mut self,
        id: BlockId,
        selection: crate::ReadSelection,
        max_peak_bytes: u64,
    ) -> Result<crate::SelectedRead, FormatError> {
        self.backend_mut()
            .read_selected(id, selection, max_peak_bytes)
    }

    /// Reserves a shared quota before payload I/O. `None` means the quota is currently busy;
    /// an oversized request errors. Completed output keeps its reservation until dropped.
    pub fn try_read_selected(
        &mut self,
        id: BlockId,
        selection: crate::ReadSelection,
        budget: &crate::ReadBudget,
    ) -> Result<Option<crate::BudgetedRead>, FormatError> {
        self.backend_mut().try_read_selected(id, selection, budget)
    }

    /// Consumes a reservation acquired before dispatching work to a reader thread.
    /// The plan is validated against this reader; insufficient reservations fail before I/O.
    /// Errors release the reservation; success retains only the estimated output bytes.
    pub fn read_reserved(
        &mut self,
        id: BlockId,
        selection: crate::ReadSelection,
        reservation: crate::ReadReservation,
    ) -> Result<crate::BudgetedRead, FormatError> {
        self.backend_mut().read_reserved(id, selection, reservation)
    }

    /// Nearest-neighbour distance quantiles (`v8.N-pre`); `None` for older formats.
    pub fn point_distance_quantiles(&self) -> Option<crate::PointDistanceQuantiles> {
        self.backend().point_distance_quantiles()
    }

    /// Per-cloud reference spacing (RF-05): stored in `v7.5-pre`, computed from all leaves in the
    /// other formats.
    pub fn reference_spacing(&self) -> Option<f64> {
        self.backend().reference_spacing()
    }

    pub fn attribute_range(
        &self,
        schema_index: u32,
        component: u32,
    ) -> Result<Option<[f64; 2]>, FormatError> {
        self.backend().attribute_range(schema_index, component)
    }

    pub fn read_geometry(&mut self, id: BlockId) -> Result<PointBatch, FormatError> {
        self.backend_mut().read_geometry(id)
    }

    pub fn read_display_colors(&mut self, id: BlockId) -> Result<Vec<ColorRgb>, FormatError> {
        self.backend_mut().read_display_colors(id)
    }

    pub fn read_positions(&mut self, id: BlockId) -> Result<PointBatch, FormatError> {
        self.backend_mut().read_positions(id)
    }

    /// Exact `i64` quanta (2^-15 CRS units) of a `v7.4-pre` block, for lossless export.
    pub fn read_block_quanta(&mut self, id: BlockId) -> Result<Vec<[i64; 3]>, FormatError> {
        self.backend_mut().read_block_quanta(id)
    }

    pub fn read_attribute(
        &mut self,
        id: BlockId,
        schema_index: u32,
    ) -> Result<AttributeColumn, FormatError> {
        self.backend_mut().read_attribute(id, schema_index)
    }

    pub fn read_attribute_row(
        &mut self,
        id: BlockId,
        row_id: u64,
        schema_index: u32,
    ) -> Result<AttributeValues, FormatError> {
        self.backend_mut()
            .read_attribute_row(id, row_id, schema_index)
    }
}

impl FormatReader<Cursor<Vec<u8>>> {
    /// Bind a previously fetched companion's identity without retaining an attribute cursor.
    pub fn attach_attribute_identity(
        &mut self,
        file_length: u64,
        header: &[u8],
        footer: &[u8],
    ) -> Result<(), FormatError> {
        self.backend
            .attach_attribute_identity(file_length, header, footer)
    }
}

fn validate_identity(identity: FormatIdentity) -> Result<(), FormatError> {
    crate::layout::StorageLayout::from_version(identity.version)?;
    if identity.role == crate::FileRole::Attributes {
        return Err(FormatError::InvalidHeader);
    }
    Ok(())
}

impl<R: Read + Seek> DatasetSource for FormatReader<R> {
    type Error = FormatError;

    fn metadata(&self) -> &DatasetMetadata {
        self.format_metadata().dataset()
    }

    fn read_block(&mut self, id: BlockId) -> Result<PointBlock, Self::Error> {
        DatasetSource::read_block(self.backend_mut(), id)
    }
}

impl<R: Read + Seek> OctreeReader for FormatReader<R> {
    fn format_version(&self) -> FormatVersion {
        self.backend().format_version()
    }

    fn has_attributes(&self) -> bool {
        FormatReader::has_attributes(self)
    }

    fn attribute_present(&self, id: BlockId, index: u32) -> Result<bool, Self::Error> {
        FormatReader::attribute_present(self, id, index)
    }

    fn read_positions(&mut self, id: BlockId) -> Result<PointBatch, Self::Error> {
        FormatReader::read_positions(self, id)
    }

    fn read_geometry(&mut self, id: BlockId) -> Result<PointBatch, Self::Error> {
        FormatReader::read_geometry(self, id)
    }

    fn read_display_colors(&mut self, id: BlockId) -> Result<Vec<ColorRgb>, Self::Error> {
        FormatReader::read_display_colors(self, id)
    }

    fn read_attribute(&mut self, id: BlockId, index: u32) -> Result<AttributeColumn, Self::Error> {
        FormatReader::read_attribute(self, id, index)
    }

    fn read_attribute_row(
        &mut self,
        id: BlockId,
        row: u64,
        index: u32,
    ) -> Result<AttributeValues, Self::Error> {
        FormatReader::read_attribute_row(self, id, row, index)
    }

    fn attribute_range(&self, index: u32, component: u32) -> Result<Option<[f64; 2]>, Self::Error> {
        FormatReader::attribute_range(self, index, component)
    }
}

impl FormatReader<std::io::Cursor<Vec<u8>>> {
    /// Lazy paged-layout (`v7.5-pre` and `v7`) reader whose payloads and pages are fetched by the caller (browser Blobs).
    /// `footer` is the last 40 bytes of the file and `index` the index region it names.
    pub fn paged_external(
        file_length: u64,
        footer: &[u8],
        index: &[u8],
        limits: FormatReaderLimits,
    ) -> Result<Self, FormatError> {
        let location = crate::PagedCore::locate_index(file_length, footer, &limits)?;
        let core = crate::PagedCore::from_index(index, location, limits)?;
        Ok(Self {
            backend: IndexedReader::from_paged_core(std::io::Cursor::new(Vec::new()), core, true)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use viewer_core::BlockId;

    use super::*;
    use crate::test_support::{COLORS, V8_BLOCK, v8_pair};

    const ID: BlockId = V8_BLOCK;

    #[test]
    fn facade_opens_v8_and_forks_the_validated_backend() {
        let (primary, _) = v8_pair();
        let mut reader = FormatReader::new(Cursor::new(primary.clone())).unwrap();
        assert_eq!(reader.format_version(), FormatVersion::V8);
        assert_eq!(
            reader
                .format_metadata()
                .dataset()
                .hierarchy()
                .stats()
                .node_count,
            1
        );
        assert_eq!(reader.read_positions(ID).unwrap().len(), 4);

        let mut fork = reader.fork_reader(Cursor::new(primary));
        assert!(reader.shares_index_with(&fork));
        assert_eq!(fork.read_positions(ID).unwrap().len(), 4);
    }

    #[test]
    fn rejects_an_unknown_revision_before_looking_for_a_payload_or_index() {
        let (primary, _) = v8_pair();
        let mut header = primary[..24].to_vec();
        header[16..20].copy_from_slice(&99_u32.to_le_bytes());
        let crc = crate::crc32(&header[..20]);
        header[20..24].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(
            FormatReader::new(Cursor::new(header)),
            Err(FormatError::UnsupportedFormatVersion(version))
                if version.major() == 8 && version.revision() == 99
        ));
    }

    #[test]
    fn fork_can_attach_the_matching_split_sidecar_independently() {
        let (primary, attributes) = v8_pair();
        let reader = FormatReader::new(Cursor::new(primary.clone())).unwrap();
        let mut fork = reader.fork_reader(Cursor::new(primary));
        assert!(!reader.has_attributes());
        fork.attach_attributes(Cursor::new(attributes)).unwrap();
        assert!(fork.has_attributes());
        assert!(!reader.has_attributes());
        assert_eq!(fork.read_display_colors(ID).unwrap(), COLORS.to_vec());
    }
}
