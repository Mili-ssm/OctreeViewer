use std::io::{Read, Seek};

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
        let identity = probe_format(&mut reader)?;
        validate_identity(identity)?;
        let backend = IndexedReader::with_identity(reader, limits, identity)?;
        Ok(Self { backend })
    }

    fn backend(&self) -> &IndexedReader<R> {
        &self.backend
    }

    fn backend_mut(&mut self) -> &mut IndexedReader<R> {
        &mut self.backend
    }

    pub fn format_metadata(&self) -> &FormatMetadata {
        self.backend().format_metadata()
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

    pub fn attach_attributes(&mut self, attributes: R) -> Result<(), FormatError> {
        self.backend_mut().attach_attributes(attributes)
    }

    pub fn has_attributes(&self) -> bool {
        self.backend().has_attributes()
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use viewer_core::{
        BlockId, ColorRgb, CoordinateFrame, DatasetMetadata, NodeId, SpatialBounds,
        SpatialHierarchy, SpatialNode,
    };

    use super::*;
    use crate::{FormatWriter, FormatWriterOptions, SourcePointBlock};

    fn metadata() -> FormatMetadata {
        let id = BlockId::new(1);
        let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
        let node = SpatialNode::new(NodeId::new(0), None, 0, bounds, 1, 0.0, vec![id]).unwrap();
        let hierarchy = SpatialHierarchy::new(vec![node], Vec::new(), Some(1)).unwrap();
        FormatMetadata::new(
            DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
            None,
            None,
            Vec::new(),
        )
        .unwrap()
    }

    fn block(color: ColorRgb) -> SourcePointBlock {
        SourcePointBlock::new(
            BlockId::new(1),
            vec![[0.25, 0.5, 0.75]],
            vec![color],
            Vec::new(),
        )
    }

    fn options() -> FormatWriterOptions {
        FormatWriterOptions::new(0.001, 1024).unwrap()
    }

    fn v5_bytes() -> Vec<u8> {
        let mut writer = FormatWriter::new(Cursor::new(Vec::new()), metadata(), options()).unwrap();
        writer
            .write_block(block(ColorRgb::new(10, 20, 30)))
            .unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn facade_autodetects_v5_and_forks_the_validated_backend() {
        let bytes = v5_bytes();
        let mut reader = FormatReader::new(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(reader.format_version(), FormatVersion::V5);
        assert_eq!(
            reader
                .format_metadata()
                .dataset()
                .hierarchy()
                .stats()
                .node_count,
            1
        );
        assert_eq!(reader.read_positions(BlockId::new(1)).unwrap().len(), 1);

        let mut fork = reader.fork_reader(Cursor::new(bytes));
        assert_eq!(fork.read_positions(BlockId::new(1)).unwrap().len(), 1);
    }

    #[test]
    fn rejects_v6_before_looking_for_a_payload_or_index() {
        let header = FormatIdentity {
            version: FormatVersion::V6,
            role: crate::FileRole::Monolithic,
        }
        .to_header_bytes()
        .unwrap();
        assert!(matches!(
            FormatReader::new(Cursor::new(header)),
            Err(FormatError::UnsupportedFormatVersion(FormatVersion::V6))
        ));
    }

    #[test]
    fn fork_can_attach_the_matching_split_sidecar_independently() {
        let mut writer = FormatWriter::new_split(
            Cursor::new(Vec::new()),
            Cursor::new(Vec::new()),
            metadata(),
            options(),
        )
        .unwrap();
        writer
            .write_block(block(ColorRgb::new(10, 20, 30)))
            .unwrap();
        let (primary, attributes) = writer.finish_pair().unwrap();
        let primary = primary.into_inner();
        let attributes = attributes.unwrap().into_inner();

        let reader = FormatReader::new(Cursor::new(primary.clone())).unwrap();
        let mut fork = reader.fork_reader(Cursor::new(primary));
        assert!(!reader.has_attributes());
        fork.attach_attributes(Cursor::new(attributes)).unwrap();
        assert!(fork.has_attributes());
        assert!(!reader.has_attributes());
        assert_eq!(
            fork.read_display_colors(BlockId::new(1)).unwrap(),
            vec![ColorRgb::new(10, 20, 30)]
        );
    }
}
