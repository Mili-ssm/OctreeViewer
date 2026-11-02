use crate::lod_dataset::LodDataset;
use serde::Deserialize;
use std::sync::Arc;
use viewer_core::{
    AttributeDataType, AttributeDescriptor, BlockId, CoordinateFrame, DatasetMetadata,
    HierarchySemantics, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LodDatasetDescriptor {
    pub(crate) coordinate_origin: [f64; 3],
    pub(crate) semantics: String,
    pub(crate) cache_bytes: usize,
    #[serde(default = "default_visual_scale")]
    pub(crate) visual_scale: f64,
    #[serde(default)]
    pub(crate) has_attributes: bool,
    #[serde(default)]
    pub(crate) attribute_schema: Vec<LodAttributeDescriptor>,
    #[serde(default)]
    pub(crate) attribute_ranges: Option<Vec<Vec<Option<[f64; 2]>>>>,
    #[serde(default)]
    pub(crate) source_point_count: Option<String>,
    #[serde(default)]
    pub(crate) crs: Option<String>,
    /// Tight data extent (world space); absent means the root bounds.
    #[serde(default)]
    pub(crate) data_min: Option<[f64; 3]>,
    #[serde(default)]
    pub(crate) data_max: Option<[f64; 3]>,
    pub(crate) nodes: Vec<LodNodeDescriptor>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LodAttributeDescriptor {
    pub(crate) name: String,
    pub(crate) data_type: String,
    pub(crate) component_count: u32,
}

fn default_visual_scale() -> f64 {
    1.0
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LodNodeDescriptor {
    pub(crate) id: String,
    pub(crate) parent: Option<String>,
    pub(crate) level: u32,
    pub(crate) min: [f64; 3],
    pub(crate) max: [f64; 3],
    pub(crate) point_count: String,
    pub(crate) geometric_error: f64,
    pub(crate) blocks: Vec<String>,
    /// Stored representative spacing (`v7.4-pre`); absent in older formats.
    #[serde(default)]
    pub(crate) spacing: Option<f64>,
}

impl LodDatasetDescriptor {
    pub fn build(self) -> Result<LodDataset, String> {
        if !self.visual_scale.is_finite() || self.visual_scale <= 0.0 {
            return Err("visualScale must be finite and greater than zero".into());
        }
        let schema = self
            .attribute_schema
            .into_iter()
            .map(|attribute| {
                let data_type = match attribute.data_type.as_str() {
                    "u8" => AttributeDataType::U8,
                    "i8" => AttributeDataType::I8,
                    "u16" => AttributeDataType::U16,
                    "i16" => AttributeDataType::I16,
                    "u32" => AttributeDataType::U32,
                    "i32" => AttributeDataType::I32,
                    "u64" => AttributeDataType::U64,
                    "i64" => AttributeDataType::I64,
                    "f32" => AttributeDataType::F32,
                    "f64" => AttributeDataType::F64,
                    other => return Err(format!("unknown attribute data type {other:?}")),
                };
                AttributeDescriptor::new(attribute.name, data_type, attribute.component_count)
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(ranges) = &self.attribute_ranges {
            if ranges.len() != schema.len()
                || ranges
                    .iter()
                    .zip(&schema)
                    .any(|(row, descriptor)| row.len() != descriptor.component_count() as usize)
            {
                return Err("attributeRanges dimensions must match attributeSchema".into());
            }
            if ranges
                .iter()
                .flatten()
                .flatten()
                .any(|range| !range[0].is_finite() || !range[1].is_finite() || range[0] > range[1])
            {
                return Err("attributeRanges must contain finite ordered min/max values".into());
            }
        }
        let semantics = match self.semantics.as_str() {
            "replacement" => HierarchySemantics::Replacement,
            "additive" => HierarchySemantics::Additive,
            other => return Err(format!("unknown hierarchy semantics {other:?}")),
        };
        let source_point_count = self
            .source_point_count
            .map(|value| parse_u64(&value, "sourcePointCount"))
            .transpose()?;
        let nodes = self
            .nodes
            .into_iter()
            .map(|node| {
                let id = NodeId::new(parse_u64(&node.id, "node id")?);
                let parent = node
                    .parent
                    .as_deref()
                    .map(|value| parse_u64(value, "parent id").map(NodeId::new))
                    .transpose()?;
                let blocks = node
                    .blocks
                    .iter()
                    .map(|value| parse_u64(value, "block id").map(BlockId::new))
                    .collect::<Result<Vec<_>, _>>()?;
                let spatial = SpatialNode::new(
                    id,
                    parent,
                    node.level,
                    SpatialBounds::new(node.min, node.max).map_err(|error| error.to_string())?,
                    parse_u64(&node.point_count, "pointCount")?,
                    node.geometric_error,
                    blocks,
                )
                .map_err(|error| error.to_string())?;
                match node.spacing {
                    Some(spacing) => spatial
                        .with_spacing(spacing)
                        .map_err(|error| error.to_string()),
                    None => Ok(spatial),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let hierarchy =
            SpatialHierarchy::new_with_semantics(nodes, schema, source_point_count, semantics)
                .map_err(|error| error.to_string())?;
        let hierarchy = match (self.data_min, self.data_max) {
            (Some(min), Some(max)) => hierarchy
                .with_data_bounds(SpatialBounds::new(min, max).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?,
            _ => hierarchy,
        };
        let frame =
            CoordinateFrame::new(self.coordinate_origin).map_err(|error| error.to_string())?;
        Ok(LodDataset::from_shared_metadata_and_visual_scale(
            Arc::new(DatasetMetadata::new(frame, hierarchy)),
            self.visual_scale,
            self.cache_bytes,
        )
        .with_crs(self.crs))
    }
}

fn parse_u64(value: &str, field: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("{field} must be an unsigned 64-bit integer string"))
}
