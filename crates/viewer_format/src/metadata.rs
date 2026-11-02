use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use viewer_core::{AttributeDataType, AttributeValues, DatasetMetadata, NodeId, las::LasExtraByte};

use crate::{FormatError, GlobalFrame};

/// Extra file-level metadata not interpreted by spatial queries.
#[derive(Debug, Clone, PartialEq)]
pub struct FormatMetadata {
    dataset: Arc<DatasetMetadata>,
    crs: Option<String>,
    sampling_policy: Option<String>,
    visual_scale: f64,
    node_attribute_ranges: BTreeMap<NodeId, Vec<NodeAttributeRange>>,
    global_frame: Option<GlobalFrame>,
    las_extra_bytes: Vec<LasExtraByte>,
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
            visual_scale: 1.0,
            node_attribute_ranges: ranges,
            global_frame: None,
            las_extra_bytes: Vec::new(),
        })
    }

    /// Attaches the `v7.4-pre` global frame: root, CRS record, source scale/offset and per-node
    /// prefix and spacing. The dataset's frame origin and node bounds must be the derived ones
    /// (checked when the file is written or read). `crs()` then reports the CRS identity.
    pub fn with_global_frame(mut self, mut frame: GlobalFrame) -> Result<Self, MetadataError> {
        let hierarchy = self.dataset.hierarchy();
        let root = hierarchy
            .node(hierarchy.root_id())
            .ok_or(MetadataError::InvalidGlobalFrame("root node is missing"))?;
        let nodes: Vec<_> = hierarchy.query(root.bounds()).collect();
        frame.bind_blocks(&nodes)?;
        self.crs = frame.crs().identity_label();
        self.global_frame = Some(frame);
        Ok(self)
    }

    /// Attaches the LAS Extra Bytes descriptors (`v7.5-pre` only; other versions do not store
    /// them). The last `descriptors.len()` schema columns are their columns, in order.
    pub fn with_las_extra_bytes(mut self, descriptors: Vec<LasExtraByte>) -> Self {
        self.las_extra_bytes = descriptors;
        self
    }

    pub fn las_extra_bytes(&self) -> &[LasExtraByte] {
        &self.las_extra_bytes
    }

    /// Present exactly in `v7.4-pre` files.
    pub fn global_frame(&self) -> Option<&GlobalFrame> {
        self.global_frame.as_ref()
    }

    pub fn dataset(&self) -> &DatasetMetadata {
        &self.dataset
    }

    pub fn shared_dataset(&self) -> Arc<DatasetMetadata> {
        Arc::clone(&self.dataset)
    }

    /// The dataset as the viewer consumes it: node bounds in the same space as the coordinate
    /// origin (world), plus each node's stored representative spacing.
    ///
    /// `v7.4-pre` stores bounds relative to the frame origin and a spacing per node, so they are
    /// shifted back by the origin (exact: the origin and bounds are multiples of 2^-15 below 2^35),
    /// the spacing is attached and `stats().data_bounds` is the tight data extent of the frame. Other versions return `shared_dataset()` unchanged.
    pub fn viewer_dataset(&self) -> Result<Arc<DatasetMetadata>, MetadataError> {
        let Some(frame) = &self.global_frame else {
            return Ok(self.shared_dataset());
        };
        let origin = self.dataset.coordinate_frame().origin();
        let hierarchy = self.dataset.hierarchy();
        let invalid = |_| MetadataError::InvalidGlobalFrame("viewer dataset");
        let mut nodes = Vec::new();
        let mut pending = vec![hierarchy.root_id()];
        while let Some(id) = pending.pop() {
            let node = hierarchy
                .node(id)
                .ok_or(MetadataError::InvalidGlobalFrame("missing node"))?;
            let bounds = viewer_core::SpatialBounds::new(
                std::array::from_fn(|axis| node.bounds().min()[axis] + origin[axis]),
                std::array::from_fn(|axis| node.bounds().max()[axis] + origin[axis]),
            )
            .map_err(invalid)?;
            let mut shifted = viewer_core::SpatialNode::new(
                id,
                node.parent(),
                node.level(),
                bounds,
                node.point_count(),
                node.geometric_error(),
                node.blocks().to_vec(),
            )
            .map_err(invalid)?;
            if let Some(cell) = frame.node_cell(id) {
                shifted = shifted.with_spacing(cell.spacing()).map_err(invalid)?;
            }
            nodes.push(shifted);
            pending.extend(hierarchy.child_ids(id).iter().rev().copied());
        }
        let rebuilt = viewer_core::SpatialHierarchy::new_with_semantics(
            nodes,
            hierarchy.attributes().to_vec(),
            hierarchy.stats().source_point_count,
            hierarchy.semantics(),
        )
        .map_err(invalid)?
        .with_data_bounds(frame.data_bounds().map_err(invalid)?)
        .map_err(invalid)?;
        Ok(Arc::new(DatasetMetadata::new(
            self.dataset.coordinate_frame(),
            rebuilt,
        )))
    }

    pub fn crs(&self) -> Option<&str> {
        self.crs.as_deref()
    }

    pub fn sampling_policy(&self) -> Option<&str> {
        self.sampling_policy.as_deref()
    }

    /// Uniform scale applied only by the renderer to local coordinates and point glyphs.
    pub const fn visual_scale(&self) -> f64 {
        self.visual_scale
    }

    pub fn with_visual_scale(mut self, visual_scale: f64) -> Result<Self, MetadataError> {
        if !visual_scale.is_finite() || visual_scale <= 0.0 {
            return Err(MetadataError::InvalidVisualScale);
        }
        self.visual_scale = visual_scale;
        Ok(self)
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

    pub(crate) fn with_dataset(
        &self,
        dataset: DatasetMetadata,
        ranges: BTreeMap<NodeId, Vec<NodeAttributeRange>>,
    ) -> Self {
        Self {
            dataset: Arc::new(dataset),
            crs: self.crs.clone(),
            sampling_policy: self.sampling_policy.clone(),
            visual_scale: self.visual_scale,
            node_attribute_ranges: ranges,
            global_frame: self.global_frame.clone(),
            las_extra_bytes: self.las_extra_bytes.clone(),
        }
    }
}

/// Per-component min/max values for one attribute in a node's own payload.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeAttributeRange {
    schema_index: u32,
    pub(crate) min: AttributeValues,
    pub(crate) max: AttributeValues,
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

    pub fn with_id(mut self, id: viewer_core::BlockId) -> Self {
        self.id = id;
        self
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
    #[error("visual scale must be finite and greater than zero")]
    InvalidVisualScale,
    #[error("invalid CRS record: {0}")]
    InvalidCrs(&'static str),
    #[error("invalid global frame: {0}")]
    InvalidGlobalFrame(&'static str),
}

pub(crate) fn validate_metadata(metadata: &FormatMetadata) -> Result<(), FormatError> {
    if !metadata.visual_scale().is_finite() || metadata.visual_scale() <= 0.0 {
        return Err(FormatError::InvalidVisualScale);
    }
    for value in [metadata.crs(), metadata.sampling_policy()]
        .into_iter()
        .flatten()
    {
        if value.len() > u32::MAX as usize {
            return Err(FormatError::StringTooLong(value.len()));
        }
    }
    let hierarchy = metadata.dataset().hierarchy();
    let extras = metadata.las_extra_bytes();
    let tail = hierarchy
        .attributes()
        .len()
        .checked_sub(extras.len())
        .ok_or(FormatError::InvalidMetadata(
            "more Extra Bytes descriptors than schema attributes",
        ))?;
    for (extra, column) in extras.iter().zip(&hierarchy.attributes()[tail..]) {
        if extra.data_type() != column.data_type() || extra.components() != column.component_count()
        {
            return Err(FormatError::InvalidMetadata(
                "Extra Bytes descriptor does not match its schema column",
            ));
        }
    }
    if hierarchy.attributes().len() > u32::MAX as usize {
        return Err(FormatError::SchemaTooLarge);
    }
    for descriptor in hierarchy.attributes() {
        if descriptor.name().len() > u32::MAX as usize {
            return Err(FormatError::StringTooLong(descriptor.name().len()));
        }
    }
    let root = hierarchy
        .node(hierarchy.root_id())
        .ok_or(FormatError::InvalidMetadata("root node is missing"))?;
    let all_nodes: Vec<_> = hierarchy.query(root.bounds()).collect();
    let node_ids: BTreeSet<_> = all_nodes.iter().map(|node| node.id()).collect();
    if let Some(frame) = metadata.global_frame() {
        crate::global_frame::validate_nodes(
            frame,
            metadata.dataset().coordinate_frame().origin(),
            &all_nodes,
        )
        .map_err(FormatError::InvalidMetadata)?;
    }
    let schema = hierarchy.attributes();
    for (node_id, ranges) in metadata.node_range_entries() {
        if !node_ids.contains(&node_id) {
            return Err(FormatError::UnknownNode(node_id));
        }
        if ranges.len() > u32::MAX as usize {
            return Err(FormatError::TooManyAttributeRanges(node_id));
        }
        let mut indices = BTreeSet::new();
        for range in ranges {
            let index = usize::try_from(range.schema_index())
                .map_err(|_| FormatError::InvalidAttributeRange(node_id))?;
            let Some(descriptor) = schema.get(index) else {
                return Err(FormatError::InvalidAttributeRange(node_id));
            };
            if !indices.insert(index)
                || !valid_range_values(range, descriptor.data_type(), descriptor.component_count())
            {
                return Err(FormatError::InvalidAttributeRange(node_id));
            }
        }
    }
    Ok(())
}

fn valid_range_values(
    range: &NodeAttributeRange,
    data_type: AttributeDataType,
    count: u32,
) -> bool {
    if range.min().data_type() != data_type || range.max().data_type() != data_type {
        return false;
    }
    let expected = count as usize;
    match (range.min(), range.max()) {
        (AttributeValues::U8(min), AttributeValues::U8(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::I8(min), AttributeValues::I8(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::U16(min), AttributeValues::U16(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::I16(min), AttributeValues::I16(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::U32(min), AttributeValues::U32(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::I32(min), AttributeValues::I32(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::U64(min), AttributeValues::U64(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::I64(min), AttributeValues::I64(max)) => {
            min.len() == expected
                && max.len() == expected
                && min.iter().zip(max).all(|(a, b)| a <= b)
        }
        (AttributeValues::F32(min), AttributeValues::F32(max)) => {
            min.len() == expected
                && max.len() == expected
                && min
                    .iter()
                    .zip(max)
                    .all(|(a, b)| a.is_finite() && b.is_finite() && a <= b)
        }
        (AttributeValues::F64(min), AttributeValues::F64(max)) => {
            min.len() == expected
                && max.len() == expected
                && min
                    .iter()
                    .zip(max)
                    .all(|(a, b)| a.is_finite() && b.is_finite() && a <= b)
        }
        _ => false,
    }
}
