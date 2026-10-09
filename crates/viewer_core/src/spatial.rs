use std::collections::HashMap;

use thiserror::Error;

/// Identifier unique within a dataset; its value has no spatial meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u64);

impl NodeId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Identifier unique within a dataset; its value is not a file offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(u64);

impl BlockId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Finite dataset-coordinate bounds with inclusive faces and edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialBounds {
    min: [f64; 3],
    max: [f64; 3],
}

impl SpatialBounds {
    pub fn new(min: [f64; 3], max: [f64; 3]) -> Result<Self, SpatialError> {
        let valid = (0..3)
            .all(|axis| min[axis].is_finite() && max[axis].is_finite() && min[axis] <= max[axis]);
        if !valid {
            return Err(SpatialError::InvalidBounds { min, max });
        }

        Ok(Self { min, max })
    }

    pub const fn min(&self) -> [f64; 3] {
        self.min
    }

    pub const fn max(&self) -> [f64; 3] {
        self.max
    }

    pub fn contains(&self, other: &Self) -> bool {
        (0..3).all(|axis| self.min[axis] <= other.min[axis] && self.max[axis] >= other.max[axis])
    }

    pub fn intersects(&self, other: &Self) -> bool {
        (0..3).all(|axis| self.min[axis] <= other.max[axis] && other.min[axis] <= self.max[axis])
    }
}

/// A plane whose inside half-space satisfies `normal · point + offset >= 0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrustumPlane {
    normal: [f64; 3],
    offset: f64,
}

impl FrustumPlane {
    pub fn new(normal: [f64; 3], offset: f64) -> Result<Self, SpatialError> {
        if normal.iter().any(|value| !value.is_finite())
            || normal.iter().all(|value| *value == 0.0)
            || !offset.is_finite()
        {
            return Err(SpatialError::InvalidFrustumPlane { normal, offset });
        }

        Ok(Self { normal, offset })
    }

    pub const fn normal(&self) -> [f64; 3] {
        self.normal
    }

    pub const fn offset(&self) -> f64 {
        self.offset
    }

    fn intersects(&self, bounds: &SpatialBounds) -> bool {
        let support_point: [f64; 3] = std::array::from_fn(|axis| {
            if self.normal[axis] >= 0.0 {
                bounds.max[axis]
            } else {
                bounds.min[axis]
            }
        });
        let coefficient_scale = self
            .normal
            .iter()
            .map(|value| value.abs())
            .fold(self.offset.abs(), f64::max);
        let coordinate_scale = support_point
            .iter()
            .map(|value| value.abs())
            .fold(1.0, f64::max);
        let maximum_distance = (0..3)
            .map(|axis| {
                (self.normal[axis] / coefficient_scale) * (support_point[axis] / coordinate_scale)
            })
            .sum::<f64>()
            + (self.offset / coefficient_scale) / coordinate_scale;

        maximum_distance >= 0.0
    }
}

/// Convex query volume represented by inward-facing planes.
#[derive(Debug, Clone, PartialEq)]
pub struct SpatialFrustum {
    planes: Vec<FrustumPlane>,
}

impl SpatialFrustum {
    pub fn new(planes: Vec<FrustumPlane>) -> Result<Self, SpatialError> {
        if planes.is_empty() {
            return Err(SpatialError::EmptyFrustum);
        }
        Ok(Self { planes })
    }

    pub fn planes(&self) -> &[FrustumPlane] {
        &self.planes
    }

    pub fn intersects(&self, bounds: &SpatialBounds) -> bool {
        self.planes.iter().all(|plane| plane.intersects(bounds))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeDataType {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeDescriptor {
    name: String,
    data_type: AttributeDataType,
    component_count: u32,
}

impl AttributeDescriptor {
    pub fn new(
        name: impl Into<String>,
        data_type: AttributeDataType,
        component_count: u32,
    ) -> Result<Self, SpatialError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(SpatialError::EmptyAttributeName);
        }
        if component_count == 0 {
            return Err(SpatialError::ZeroAttributeComponents { name });
        }

        Ok(Self {
            name,
            data_type,
            component_count,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn data_type(&self) -> AttributeDataType {
        self.data_type
    }

    pub const fn component_count(&self) -> u32 {
        self.component_count
    }
}

/// Metadata for one hierarchy node. `point_count` counts this node's payload only.
#[derive(Debug, Clone, PartialEq)]
pub struct SpatialNode {
    id: NodeId,
    parent: Option<NodeId>,
    level: u32,
    bounds: SpatialBounds,
    point_count: u64,
    geometric_error: f64,
    blocks: Vec<BlockId>,
}

impl SpatialNode {
    pub fn new(
        id: NodeId,
        parent: Option<NodeId>,
        level: u32,
        bounds: SpatialBounds,
        point_count: u64,
        geometric_error: f64,
        blocks: Vec<BlockId>,
    ) -> Result<Self, SpatialError> {
        if !geometric_error.is_finite() || geometric_error < 0.0 {
            return Err(SpatialError::InvalidGeometricError {
                node: id,
                geometric_error,
            });
        }

        Ok(Self {
            id,
            parent,
            level,
            bounds,
            point_count,
            geometric_error,
            blocks,
        })
    }

    pub const fn id(&self) -> NodeId {
        self.id
    }

    pub const fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    pub const fn level(&self) -> u32 {
        self.level
    }

    pub const fn bounds(&self) -> SpatialBounds {
        self.bounds
    }

    pub const fn point_count(&self) -> u64 {
        self.point_count
    }

    pub const fn geometric_error(&self) -> f64 {
        self.geometric_error
    }

    pub fn blocks(&self) -> &[BlockId] {
        &self.blocks
    }
}

/// Statistics that do not assume that samples in different LOD nodes are disjoint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialStats {
    pub root_bounds: SpatialBounds,
    pub node_count: usize,
    pub max_level: u32,
    pub source_point_count: Option<u64>,
}

#[derive(Debug, Error, PartialEq)]
pub enum SpatialError {
    #[error("spatial bounds must be finite and ordered, got min {min:?} and max {max:?}")]
    InvalidBounds { min: [f64; 3], max: [f64; 3] },
    #[error("frustum plane must have a finite, non-zero normal and finite offset")]
    InvalidFrustumPlane { normal: [f64; 3], offset: f64 },
    #[error("frustum must contain at least one plane")]
    EmptyFrustum,
    #[error("attribute name must not be empty")]
    EmptyAttributeName,
    #[error("attribute {name:?} must have at least one component")]
    ZeroAttributeComponents { name: String },
    #[error("attribute name {0:?} is repeated")]
    DuplicateAttributeName(String),
    #[error("node {node:?} has invalid geometric error {geometric_error}")]
    InvalidGeometricError { node: NodeId, geometric_error: f64 },
    #[error("spatial hierarchy must contain at least one node")]
    EmptyHierarchy,
    #[error("node identifier {0:?} is repeated")]
    DuplicateNodeId(NodeId),
    #[error("node {node:?} refers to missing parent {parent:?}")]
    UnknownParent { node: NodeId, parent: NodeId },
    #[error("spatial hierarchy must have exactly one root, found {0}")]
    RootCount(usize),
    #[error("root node {node:?} must have level zero, got {level}")]
    InvalidRootLevel { node: NodeId, level: u32 },
    #[error("node {node:?} must be one level below parent {parent:?}")]
    InvalidChildLevel { node: NodeId, parent: NodeId },
    #[error("node {node:?} bounds must be contained by parent {parent:?}")]
    ChildOutsideParent { node: NodeId, parent: NodeId },
}

#[derive(Debug, Clone, PartialEq)]
struct IndexedNode {
    metadata: SpatialNode,
    children: Vec<NodeId>,
}

/// Validated resident metadata index; point and block payloads are stored elsewhere.
#[derive(Debug, Clone, PartialEq)]
pub struct SpatialHierarchy {
    root: NodeId,
    nodes: HashMap<NodeId, IndexedNode>,
    attributes: Vec<AttributeDescriptor>,
    stats: SpatialStats,
}

impl SpatialHierarchy {
    pub fn new(
        nodes: Vec<SpatialNode>,
        attributes: Vec<AttributeDescriptor>,
        source_point_count: Option<u64>,
    ) -> Result<Self, SpatialError> {
        if nodes.is_empty() {
            return Err(SpatialError::EmptyHierarchy);
        }

        let mut node_map = HashMap::with_capacity(nodes.len());
        let mut ordered_ids = Vec::with_capacity(nodes.len());
        let mut roots = Vec::new();
        for node in nodes {
            if node_map.contains_key(&node.id) {
                return Err(SpatialError::DuplicateNodeId(node.id));
            }
            if node.parent.is_none() {
                roots.push(node.id);
            }
            ordered_ids.push(node.id);
            node_map.insert(
                node.id,
                IndexedNode {
                    metadata: node,
                    children: Vec::new(),
                },
            );
        }

        if roots.len() != 1 {
            return Err(SpatialError::RootCount(roots.len()));
        }
        let root = roots[0];
        let root_node = &node_map[&root].metadata;
        if root_node.level != 0 {
            return Err(SpatialError::InvalidRootLevel {
                node: root,
                level: root_node.level,
            });
        }
        let root_bounds = root_node.bounds;

        for node_id in ordered_ids {
            let parent_id = node_map[&node_id].metadata.parent;
            let Some(parent_id) = parent_id else {
                continue;
            };
            let Some(parent) = node_map.get(&parent_id) else {
                return Err(SpatialError::UnknownParent {
                    node: node_id,
                    parent: parent_id,
                });
            };
            let node = &node_map[&node_id].metadata;
            if parent.metadata.level.checked_add(1) != Some(node.level) {
                return Err(SpatialError::InvalidChildLevel {
                    node: node_id,
                    parent: parent_id,
                });
            }
            if !parent.metadata.bounds.contains(&node.bounds) {
                return Err(SpatialError::ChildOutsideParent {
                    node: node_id,
                    parent: parent_id,
                });
            }
            node_map
                .get_mut(&parent_id)
                .expect("parent was validated above")
                .children
                .push(node_id);
        }

        let mut attribute_names = HashMap::with_capacity(attributes.len());
        for attribute in &attributes {
            if attribute_names.insert(attribute.name(), ()).is_some() {
                return Err(SpatialError::DuplicateAttributeName(
                    attribute.name().to_owned(),
                ));
            }
        }

        let max_level = node_map
            .values()
            .map(|indexed| indexed.metadata.level)
            .max()
            .unwrap_or(0);
        let stats = SpatialStats {
            root_bounds,
            node_count: node_map.len(),
            max_level,
            source_point_count,
        };

        Ok(Self {
            root,
            nodes: node_map,
            attributes,
            stats,
        })
    }

    pub const fn root_id(&self) -> NodeId {
        self.root
    }

    pub fn node(&self, id: NodeId) -> Option<&SpatialNode> {
        self.nodes.get(&id).map(|indexed| &indexed.metadata)
    }

    pub fn attributes(&self) -> &[AttributeDescriptor] {
        &self.attributes
    }

    pub const fn stats(&self) -> SpatialStats {
        self.stats
    }

    /// Returns intersecting nodes in deterministic depth-first order, including ancestors.
    pub fn query(&self, bounds: SpatialBounds) -> SpatialQuery<'_> {
        SpatialQuery {
            hierarchy: self,
            region: SpatialQueryRegion::Bounds(bounds),
            pending: vec![self.root],
        }
    }

    /// Returns conservative frustum candidates in deterministic depth-first order.
    pub fn query_frustum(&self, frustum: SpatialFrustum) -> SpatialQuery<'_> {
        SpatialQuery {
            hierarchy: self,
            region: SpatialQueryRegion::Frustum(frustum),
            pending: vec![self.root],
        }
    }
}

enum SpatialQueryRegion {
    Bounds(SpatialBounds),
    Frustum(SpatialFrustum),
}

impl SpatialQueryRegion {
    fn intersects(&self, bounds: &SpatialBounds) -> bool {
        match self {
            Self::Bounds(query_bounds) => query_bounds.intersects(bounds),
            Self::Frustum(frustum) => frustum.intersects(bounds),
        }
    }
}

/// Lazy depth-first query over hierarchy metadata; disjoint subtrees are pruned.
pub struct SpatialQuery<'a> {
    hierarchy: &'a SpatialHierarchy,
    region: SpatialQueryRegion,
    pending: Vec<NodeId>,
}

impl<'a> Iterator for SpatialQuery<'a> {
    type Item = &'a SpatialNode;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(id) = self.pending.pop() {
            let indexed = &self.hierarchy.nodes[&id];
            if !self.region.intersects(&indexed.metadata.bounds) {
                continue;
            }
            self.pending.extend(indexed.children.iter().rev().copied());
            return Some(&indexed.metadata);
        }
        None
    }
}
