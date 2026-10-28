use crate::{
    BlockEncoder, FormatError, FormatMetadata, FormatVersion, FormatWriterOptions,
    PreparedPointBlock, SourcePointBlock,
};

/// Selects immutable encoding separately from the output sinks and their thread safety.
/// Supported layouts share quantization and attributes, with versioned geometry encoding.
pub struct FormatBlockEncoder<'a> {
    version: FormatVersion,
    metadata: &'a FormatMetadata,
    options: FormatWriterOptions,
}

impl<'a> FormatBlockEncoder<'a> {
    pub fn new(
        version: FormatVersion,
        metadata: &'a FormatMetadata,
        options: FormatWriterOptions,
    ) -> Result<Self, FormatError> {
        version.ensure_writable()?;
        Ok(Self {
            version,
            metadata,
            options,
        })
    }
}

impl BlockEncoder for FormatBlockEncoder<'_> {
    type PreparedBlock = PreparedPointBlock;

    fn prepare(&self, block: SourcePointBlock) -> Result<Self::PreparedBlock, FormatError> {
        crate::prepare::prepare_point_block_for_version(
            self.version,
            block,
            self.metadata,
            self.options,
        )
    }

    fn prepare_with_diagnostics(
        &self,
        block: SourcePointBlock,
    ) -> Result<(Self::PreparedBlock, Option<crate::GeometryCodecDiagnostics>), FormatError> {
        crate::prepare::prepare_point_block_for_version_with_diagnostics(
            self.version,
            block,
            self.metadata,
            self.options,
        )
        .map(|(prepared, diagnostics)| (prepared, Some(diagnostics)))
    }

    fn encoded_payload_capacity_bytes(&self, block: &Self::PreparedBlock) -> usize {
        block.encoded_payload_capacity_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FormatWriter;
    use std::{io::Cursor, num::NonZeroU32};
    use viewer_core::{
        CoordinateFrame, DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
    };

    fn metadata() -> FormatMetadata {
        let node = SpatialNode::new(
            NodeId::new(0),
            None,
            0,
            SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap(),
            0,
            0.0,
            vec![],
        )
        .unwrap();
        FormatMetadata::new(
            DatasetMetadata::new(
                CoordinateFrame::new([0.0; 3]).unwrap(),
                SpatialHierarchy::new(vec![node], vec![], Some(0)).unwrap(),
            ),
            None,
            None,
            vec![],
        )
        .unwrap()
    }

    #[test]
    fn unsupported_output_versions_leave_both_sinks_untouched() {
        let metadata = metadata();
        let options = FormatWriterOptions::new(0.01, 1024).unwrap();
        for version in [
            FormatVersion::V6,
            FormatVersion::V6Pre(NonZeroU32::new(5).unwrap()),
            FormatVersion::V7,
            FormatVersion::V7Pre(NonZeroU32::new(4).unwrap()),
            FormatVersion::Unknown {
                major: 9,
                revision: 7,
            },
        ] {
            let mut primary = Cursor::new(vec![1, 2, 3]);
            let mut attributes = Cursor::new(vec![4, 5]);
            primary.set_position(2);
            attributes.set_position(1);
            assert!(
                matches!(FormatWriter::new_split_for_version(version, &mut primary, &mut attributes,
                metadata.clone(), options), Err(FormatError::UnsupportedFormatVersion(actual)) if actual == version)
            );
            assert_eq!(primary.get_ref(), &[1, 2, 3]);
            assert_eq!(attributes.get_ref(), &[4, 5]);
            assert_eq!(primary.position(), 2);
            assert_eq!(attributes.position(), 1);
            assert!(
                matches!(FormatBlockEncoder::new(version, &metadata, options),
                Err(FormatError::UnsupportedFormatVersion(actual)) if actual == version)
            );
        }
    }
}
