use crate::lod_dataset::LodDataset;
use serde::Deserialize;
use viewer_core::{
    BlockId, HierarchySemantics, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LodDatasetDescriptor {
    pub coordinate_origin: [f64; 3],
    pub semantics: String,
    pub cache_bytes: usize,
    #[serde(default)]
    pub source_point_count: Option<String>,
    pub nodes: Vec<LodNodeDescriptor>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LodNodeDescriptor {
    pub id: String,
    pub parent: Option<String>,
    pub level: u32,
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub point_count: String,
    pub geometric_error: f64,
    pub blocks: Vec<String>,
}

impl LodDatasetDescriptor {
    pub fn build(self) -> Result<LodDataset, String> {
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
                SpatialNode::new(
                    id,
                    parent,
                    node.level,
                    SpatialBounds::new(node.min, node.max).map_err(|error| error.to_string())?,
                    parse_u64(&node.point_count, "pointCount")?,
                    node.geometric_error,
                    blocks,
                )
                .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let hierarchy =
            SpatialHierarchy::new_with_semantics(nodes, Vec::new(), source_point_count, semantics)
                .map_err(|error| error.to_string())?;
        LodDataset::new(self.coordinate_origin, hierarchy, self.cache_bytes)
            .map_err(|error| error.to_string())
    }
}

fn parse_u64(value: &str, field: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("{field} must be an unsigned 64-bit integer string"))
}
