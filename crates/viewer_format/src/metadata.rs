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
    point_distance_quantiles: Option<PointDistanceQuantiles>,
    lod_attribute_columns: Option<Vec<u32>>,
    display_colour_source: Option<DisplayColourSource>,
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
            point_distance_quantiles: None,
            lod_attribute_columns: None,
            display_colour_source: None,
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
    #[must_use]
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

    /// Attaches the nearest-neighbour distance quantiles (`v8.N-pre` only; other versions do not
    /// store them, and the writer rejects them there).
    #[must_use]
    pub const fn with_point_distance_quantiles(
        mut self,
        quantiles: PointDistanceQuantiles,
    ) -> Self {
        self.point_distance_quantiles = Some(quantiles);
        self
    }

    /// Distance quantiles to store, when set; see [`PointDistanceQuantiles`].
    pub const fn point_distance_quantiles(&self) -> Option<PointDistanceQuantiles> {
        self.point_distance_quantiles
    }

    /// Sets the schema columns blocks of nodes with children keep (`v8.N-pre` only; other versions
    /// store every column everywhere, and the writer rejects a list there). Geometry and the display
    /// colour are always kept. The list must be strictly increasing and inside the schema; an empty
    /// list is valid. The writer and the reader reject any other list.
    #[must_use]
    pub fn with_lod_attribute_columns(mut self, columns: Vec<u32>) -> Self {
        self.lod_attribute_columns = Some(columns);
        self
    }

    /// The LOD column set, when set; see [`Self::with_lod_attribute_columns`].
    pub fn lod_attribute_columns(&self) -> Option<&[u32]> {
        self.lod_attribute_columns.as_deref()
    }

    /// Sets where leaf blocks derive their display colour from (`v8.N-pre` only; other versions
    /// store it in every block, and the writer rejects a source there). The writer checks every
    /// leaf against the derivation and errors on a difference.
    #[must_use]
    pub fn with_display_colour_source(mut self, source: DisplayColourSource) -> Self {
        self.display_colour_source = Some(source);
        self
    }

    /// The display colour source, when set; see [`Self::with_display_colour_source`].
    pub fn display_colour_source(&self) -> Option<DisplayColourSource> {
        self.display_colour_source
    }

    /// Rules for [`crate::layout::StorageLayout::stored_column`]; empty without a LOD set.
    pub(crate) fn column_rules(&self) -> crate::layout::ColumnRules<'_> {
        crate::layout::ColumnRules {
            lod_columns: self.lod_attribute_columns.as_deref().unwrap_or(&[]),
            display: self.display_colour_source.unwrap_or_default(),
        }
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
            point_distance_quantiles: self.point_distance_quantiles,
            lod_attribute_columns: self.lod_attribute_columns.clone(),
            display_colour_source: self.display_colour_source,
        }
    }
}

/// Quantiles of the distance from a point to its nearest neighbour, in dataset units, with exact
/// duplicates (distance 0) left out.
///
/// All zeros is the defined "not measured" value: a cloud where no sampled point found a
/// neighbour at non-zero distance (only duplicates, a single point) stores zeros. Readers must
/// treat `p5 == 0` as "no recommendation".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointDistanceQuantiles {
    pub p1: f64,
    pub p5: f64,
    pub p50: f64,
}

impl PointDistanceQuantiles {
    /// Rejects non-finite or negative values and quantiles out of order.
    pub fn new(p1: f64, p5: f64, p50: f64) -> Result<Self, FormatError> {
        let valid =
            [p1, p5, p50].iter().all(|v| v.is_finite() && *v >= 0.0) && p1 <= p5 && p5 <= p50;
        if valid {
            Ok(Self { p1, p5, p50 })
        } else {
            Err(FormatError::InvalidMetadata("invalid distance quantiles"))
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

    #[must_use]
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

/// Where a leaf block's display colour comes from when it is not stored (`v8.N-pre`). The schema
/// index names the 16-bit source column; the reader derives each channel as `value >> 8`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DisplayColourSource {
    /// Not derivable: leaf blocks store the display colour.
    #[default]
    None,
    /// A `u16` x 3 schema column; the display colour is its high byte per channel.
    Rgb(u32),
    /// A `u16` schema column; the display colour is its high byte as r = g = b.
    Intensity(u32),
}

impl DisplayColourSource {
    /// The source must name a column of the right shape inside the schema.
    pub(crate) fn validate(
        self,
        schema: &[viewer_core::AttributeDescriptor],
    ) -> Result<(), &'static str> {
        let (index, components) = match self {
            Self::None => return Ok(()),
            Self::Rgb(index) => (index, 3),
            Self::Intensity(index) => (index, 1),
        };
        let column = schema
            .get(index as usize)
            .ok_or("display colour source is outside the schema")?;
        if column.data_type() != AttributeDataType::U16 || column.component_count() != components {
            return Err("display colour source is not a 16-bit column of the right kind");
        }
        Ok(())
    }
}

/// A LOD column list names schema columns in strictly increasing order: no index outside the
/// schema, no repeat. One order means one byte form per set.
pub(crate) fn validate_lod_columns(columns: &[u32], schema_len: usize) -> Result<(), &'static str> {
    if columns.len() > schema_len {
        return Err("LOD column list is longer than the schema");
    }
    if columns.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("LOD column list is not strictly increasing");
    }
    if columns
        .last()
        .is_some_and(|&last| last as usize >= schema_len)
    {
        return Err("LOD column index is outside the schema");
    }
    Ok(())
}

pub(crate) fn validate_metadata(metadata: &FormatMetadata) -> Result<(), FormatError> {
    if let Some(source) = metadata.display_colour_source() {
        source
            .validate(metadata.dataset().hierarchy().attributes())
            .map_err(FormatError::InvalidMetadata)?;
    }
    if let Some(columns) = metadata.lod_attribute_columns() {
        validate_lod_columns(columns, metadata.dataset().hierarchy().attributes().len())
            .map_err(FormatError::InvalidMetadata)?;
    }
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
