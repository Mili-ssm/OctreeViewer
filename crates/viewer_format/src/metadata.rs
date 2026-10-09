use std::collections::BTreeMap;
use std::sync::Arc;

use viewer_core::{AttributeValues, DatasetMetadata, NodeId};

/// Extra file-level metadata not interpreted by spatial queries.
#[derive(Debug, Clone, PartialEq)]
pub struct FormatMetadata {
    dataset: Arc<DatasetMetadata>,
    crs: Option<String>,
    sampling_policy: Option<String>,
    node_attribute_ranges: BTreeMap<NodeId, Vec<NodeAttributeRange>>,
}

impl FormatMetadata {
    pub fn new(
        dataset: DatasetMetadata,
        crs: Option<String>,
        sampling_policy: Option<String>,
        node_attribute_ranges: Vec<(NodeId, Vec<NodeAttributeRange>)>,
    ) -> Result<Self, MetadataError> {
        let mut ranges = BTreeMap::new();
        for (node_id, node_ranges) in node_attribute_ranges {
            if ranges.insert(node_id, node_ranges).is_some() {
                return Err(MetadataError::DuplicateNodeRanges(node_id));
            }
        }
        Ok(Self {
            dataset: Arc::new(dataset),
            crs,
            sampling_policy,
            node_attribute_ranges: ranges,
        })
    }

    pub fn dataset(&self) -> &DatasetMetadata {
        &self.dataset
    }

    pub fn shared_dataset(&self) -> Arc<DatasetMetadata> {
        Arc::clone(&self.dataset)
    }

    pub fn crs(&self) -> Option<&str> {
        self.crs.as_deref()
    }

    pub fn sampling_policy(&self) -> Option<&str> {
        self.sampling_policy.as_deref()
    }

    pub fn node_attribute_ranges(&self, node_id: NodeId) -> &[NodeAttributeRange] {
        self.node_attribute_ranges
            .get(&node_id)
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn node_range_entries(
        &self,
    ) -> impl Iterator<Item = (NodeId, &[NodeAttributeRange])> {
        self.node_attribute_ranges
            .iter()
            .map(|(id, ranges)| (*id, ranges.as_slice()))
    }

    pub(crate) fn with_dataset(&self, dataset: DatasetMetadata) -> Self {
        Self {
            dataset: Arc::new(dataset),
            crs: self.crs.clone(),
            sampling_policy: self.sampling_policy.clone(),
            node_attribute_ranges: self.node_attribute_ranges.clone(),
        }
    }
}

/// Per-component min/max values for one attribute in a node's own payload.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeAttributeRange {
    schema_index: u32,
    min: AttributeValues,
    max: AttributeValues,
}

impl NodeAttributeRange {
    pub fn new(schema_index: u32, min: AttributeValues, max: AttributeValues) -> Self {
        Self {
            schema_index,
            min,
            max,
        }
    }

    pub const fn schema_index(&self) -> u32 {
        self.schema_index
    }

    pub fn min(&self) -> &AttributeValues {
        &self.min
    }

    pub fn max(&self) -> &AttributeValues {
        &self.max
    }
}

/// One source block supplied to the format writer before coordinate quantization.
#[derive(Debug, PartialEq)]
pub struct SourcePointBlock {
    id: viewer_core::BlockId,
    positions_local: Vec<[f64; 3]>,
    colors: Vec<viewer_core::ColorRgb>,
    attributes: Vec<viewer_core::AttributeColumn>,
}

impl SourcePointBlock {
    pub fn new(
        id: viewer_core::BlockId,
        positions_local: Vec<[f64; 3]>,
        colors: Vec<viewer_core::ColorRgb>,
        attributes: Vec<viewer_core::AttributeColumn>,
    ) -> Self {
        Self {
            id,
            positions_local,
            colors,
            attributes,
        }
    }

    pub const fn id(&self) -> viewer_core::BlockId {
        self.id
    }

    pub fn positions_local(&self) -> &[[f64; 3]] {
        &self.positions_local
    }

    pub fn colors(&self) -> &[viewer_core::ColorRgb] {
        &self.colors
    }

    pub fn attributes(&self) -> &[viewer_core::AttributeColumn] {
        &self.attributes
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MetadataError {
    #[error("attribute ranges are repeated for node {0:?}")]
    DuplicateNodeRanges(NodeId),
}
