use std::collections::{HashMap, HashSet};

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

    pub(crate) fn allocated_name_bytes(&self) -> usize {
        self.name.capacity()
    }
}

/// Metadata for one hierarchy node. `point_count` counts only points stored in this
/// node's payload. The interpretation of parent and child payloads is selected by
/// [`HierarchySemantics`].
///
/// `geometric_error` is a conservative spatial error bound in dataset units. Its
/// interpretation follows the hierarchy semantics and must be provided by the producer.
#[derive(Debug, Clone, PartialEq)]
pub struct SpatialNode {
    id: NodeId,
    parent: Option<NodeId>,
    level: u32,
    bounds: SpatialBounds,
    point_count: u64,
    geometric_error: f64,
    blocks: Vec<BlockId>,
    /// Spacing of the node's representatives, stored by `v7.4-pre`; older formats derive it.
    spacing: Option<f64>,
}

/// Describes how payloads in parent and child nodes combine during refinement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HierarchySemantics {
    /// A parent payload represents its subtree; visible children replace it.
    Replacement,
    /// Each payload adds distinct points; visible children are added to ancestors.
    Additive,
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
            spacing: None,
        })
    }

    /// Sets the stored representative spacing (a finite positive value in dataset units).
    pub fn with_spacing(mut self, spacing: f64) -> Result<Self, SpatialError> {
        if !spacing.is_finite() || spacing <= 0.0 {
            return Err(SpatialError::InvalidSpacing {
                node: self.id,
                spacing,
            });
        }
        self.spacing = Some(spacing);
        Ok(self)
    }

    /// Spacing stored in the index, when the format has one.
    pub const fn spacing(&self) -> Option<f64> {
        self.spacing
    }

    /// Distance between the node's representatives: the stored spacing when the format has one,
    /// otherwise one eighth of the node side (the 8 representatives per axis of V5 and V7.1-3-pre).
    pub fn representative_spacing(&self) -> f64 {
        self.spacing.unwrap_or_else(|| {
            let side = (0..3)
                .map(|axis| self.bounds.max()[axis] - self.bounds.min()[axis])
                .fold(0.0, f64::max);
            side / 8.0
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
    /// Extent of the data for framing the dataset; equals `root_bounds` unless the root is a
    /// lattice cube larger than the points (`v7.4-pre`, see [`SpatialHierarchy::with_data_bounds`]).
    pub data_bounds: SpatialBounds,
    pub node_count: usize,
    pub max_level: u32,
    pub source_point_count: Option<u64>,
}

/// Inputs to metadata-only LOD selection. The projected error callback receives the
/// conservative world-space error of each candidate node and returns its screen-space
/// error in pixels for the current view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LodSelectionConfig {
    pub max_error_pixels: f64,
    pub point_budget: u64,
}

impl LodSelectionConfig {
    pub fn new(max_error_pixels: f64, point_budget: u64) -> Result<Self, LodSelectionError> {
        if !max_error_pixels.is_finite() || max_error_pixels < 0.0 {
            return Err(LodSelectionError::InvalidPixelTolerance(max_error_pixels));
        }
        Ok(Self {
            max_error_pixels,
            point_budget,
        })
    }
}

/// Visible hierarchy payloads selected within the configured point budget.
#[derive(Debug, Clone, PartialEq)]
pub struct LodSelection {
    node_ids: Vec<NodeId>,
    point_count: u64,
    point_budget: u64,
    max_error_pixels: f64,
    budget_limited: Vec<NodeId>,
    resolution_limited: Vec<NodeId>,
    unloaded_anchors: Vec<NodeId>,
}

impl LodSelection {
    pub fn node_ids(&self) -> &[NodeId] {
        &self.node_ids
    }

    pub const fn point_count(&self) -> u64 {
        self.point_count
    }

    pub const fn point_budget(&self) -> u64 {
        self.point_budget
    }

    pub const fn max_error_pixels(&self) -> f64 {
        self.max_error_pixels
    }

    /// Frontier nodes that could not be refined because adding or replacing children exceeded budget.
    pub fn budget_limited(&self) -> &[NodeId] {
        &self.budget_limited
    }

    /// Selected leaves or branches without visible child payloads that exceed tolerance.
    pub fn resolution_limited(&self) -> &[NodeId] {
        &self.resolution_limited
    }

    /// Selected anchors whose children are not loaded yet and that exceed tolerance. The caller
    /// requests their pages, highest projected error first; they are not resolution-limited.
    pub fn unloaded_anchors(&self) -> &[NodeId] {
        &self.unloaded_anchors
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum LodSelectionError {
    #[error("maximum projected error must be finite and non-negative, got {0}")]
    InvalidPixelTolerance(f64),
    #[error("projected error for node {node:?} must be non-negative and not NaN, got {error}")]
    InvalidProjectedError { node: NodeId, error: f64 },
    #[error("visible base representation requires {required} points, exceeding budget {budget}")]
    BaseCutExceedsBudget { required: u64, budget: u64 },
    #[error("point count overflow while evaluating node {0:?}")]
    PointCountOverflow(NodeId),
}

#[derive(Debug, Error, PartialEq)]
pub enum SpatialError {
    #[error("node {0:?} is not in the hierarchy")]
    UnknownNode(NodeId),
    #[error("node {0:?} already has known children and cannot become an unloaded anchor")]
    AnchorHasChildren(NodeId),
    #[error("node {0:?} is not an unloaded anchor")]
    NotUnloadedAnchor(NodeId),
    #[error("node {0:?} is not the anchor of a grafted page")]
    NotGraftedPage(NodeId),
    #[error("page node {node:?} does not descend from anchor {anchor:?}")]
    PageNodeOutsideAnchor { node: NodeId, anchor: NodeId },
    #[error("page of anchor {anchor:?} has a resident descendant page at {descendant:?}")]
    PageHasResidentDescendant { anchor: NodeId, descendant: NodeId },
    #[error("page of anchor {anchor:?} contains retained node {node:?}")]
    PageNodeRetained { anchor: NodeId, node: NodeId },
    #[error("spatial bounds must be finite and ordered, got min {min:?} and max {max:?}")]
    InvalidBounds { min: [f64; 3], max: [f64; 3] },
    #[error("spatial subdivision divisions must be 2, 4, or 8, got {0}")]
    InvalidSubdivisionDivisions(u8),
    #[error("spatial subdivision cell index {index} exceeds the child count {child_count}")]
    InvalidSubdivisionCell { index: usize, child_count: usize },
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
    #[error("node {node:?} has invalid spacing {spacing}")]
    InvalidSpacing { node: NodeId, spacing: f64 },
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
    #[error("additive hierarchy point count overflow")]
    AdditivePointCountOverflow,
    #[error("additive hierarchy contains {actual} points, source metadata declares {expected}")]
    AdditiveSourcePointCountMismatch { expected: u64, actual: u64 },
}

/// Deterministic uniform subdivision shared by spatial index builders.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialSubdivision {
    bounds: SpatialBounds,
    axis_bits: u32,
}

impl SpatialSubdivision {
    pub fn new(bounds: SpatialBounds, divisions: u8) -> Result<Self, SpatialError> {
        if !matches!(divisions, 2 | 4 | 8) {
            return Err(SpatialError::InvalidSubdivisionDivisions(divisions));
        }
        Ok(Self {
            bounds,
            axis_bits: divisions.trailing_zeros(),
        })
    }

    pub fn cell_count(self) -> usize {
        1 << (3 * self.axis_bits)
    }

    /// Maps a point to a cell; points on internal planes map to the upper cell.
    pub fn cell_index(self, position: [f64; 3]) -> usize {
        let divisions = 1_usize << self.axis_bits;
        let cell = if divisions == 2 {
            let min = self.bounds.min();
            let max = self.bounds.max();
            let middle: [f64; 3] =
                std::array::from_fn(|axis| min[axis] + (max[axis] - min[axis]) / 2.0);
            [
                usize::from(position[0] >= middle[0]),
                usize::from(position[1] >= middle[1]),
                usize::from(position[2] >= middle[2]),
            ]
        } else {
            std::array::from_fn(|axis| {
                let min = self.bounds.min()[axis];
                let max = self.bounds.max()[axis];
                let cell_side = (max - min) / divisions as f64;
                let scaled = ((position[axis] - min) / cell_side).floor();
                (scaled as usize).min(divisions - 1)
            })
        };
        // The validated power-of-two grid packs X, Y, Z into disjoint bit fields.
        (cell[0] << (2 * self.axis_bits)) | (cell[1] << self.axis_bits) | cell[2]
    }

    pub fn cell_bounds(self, index: usize) -> Result<SpatialBounds, SpatialError> {
        let divisions = 1_usize << self.axis_bits;
        let child_count = self.cell_count();
        if index >= child_count {
            return Err(SpatialError::InvalidSubdivisionCell { index, child_count });
        }
        let cell = [
            index >> (2 * self.axis_bits),
            (index >> self.axis_bits) & (divisions - 1),
            index & (divisions - 1),
        ];
        let min = std::array::from_fn(|axis| {
            let span = self.bounds.max()[axis] - self.bounds.min()[axis];
            self.bounds.min()[axis] + cell[axis] as f64 * span / divisions as f64
        });
        let max = std::array::from_fn(|axis| {
            if cell[axis] + 1 == divisions {
                self.bounds.max()[axis]
            } else {
                let span = self.bounds.max()[axis] - self.bounds.min()[axis];
                self.bounds.min()[axis] + (cell[axis] + 1) as f64 * span / divisions as f64
            }
        });
        SpatialBounds::new(min, max)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct IndexedNode {
    metadata: SpatialNode,
    children: Vec<NodeId>,
}

/// Reference spacing of a cloud (RF-05): the point-weighted median of its leaves' spacing, given
/// as `(point count, spacing)` pairs. Spacings are sorted ascending and the first whose cumulative
/// weight reaches half of the total wins, so an exact half picks the smaller value. A cloud
/// without points weighs every leaf equally. `None` when there is no finite positive spacing.
///
/// `v7.5-pre` stores this value; the other formats compute it at open from all leaves.
pub fn reference_spacing(leaves: impl IntoIterator<Item = (u64, f64)>) -> Option<f64> {
    let mut leaves: Vec<(f64, u64)> = leaves
        .into_iter()
        .filter(|(_, spacing)| spacing.is_finite() && *spacing > 0.0)
        .map(|(points, spacing)| (spacing, points))
        .collect();
    leaves.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let weigh = |points: u64| u128::from(points);
    let weighted_total: u128 = leaves.iter().map(|leaf| weigh(leaf.1)).sum();
    let unit = weighted_total == 0;
    let total = if unit {
        leaves.len() as u128
    } else {
        weighted_total
    };
    let mut cumulative = 0_u128;
    for (spacing, points) in &leaves {
        cumulative += if unit { 1 } else { weigh(*points) };
        if cumulative * 2 >= total {
            return Some(*spacing);
        }
    }
    None
}

/// Validated resident metadata index; point and block payloads are stored elsewhere.
#[derive(Debug, Clone, PartialEq)]
pub struct SpatialHierarchy {
    root: NodeId,
    nodes: HashMap<NodeId, IndexedNode>,
    block_ids: Box<[BlockId]>,
    attributes: Vec<AttributeDescriptor>,
    stats: SpatialStats,
    semantics: HierarchySemantics,
    /// Nodes whose children are not known yet (page not resident). Always childless.
    unloaded: HashSet<NodeId>,
    /// Anchors whose page was grafted after construction; only these can be evicted.
    grafted: HashSet<NodeId>,
}

impl SpatialHierarchy {
    pub fn new(
        nodes: Vec<SpatialNode>,
        attributes: Vec<AttributeDescriptor>,
        source_point_count: Option<u64>,
    ) -> Result<Self, SpatialError> {
        Self::new_with_semantics(
            nodes,
            attributes,
            source_point_count,
            HierarchySemantics::Replacement,
        )
    }

    pub fn new_with_semantics(
        nodes: Vec<SpatialNode>,
        attributes: Vec<AttributeDescriptor>,
        source_point_count: Option<u64>,
        semantics: HierarchySemantics,
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
            data_bounds: root_bounds,
            node_count: node_map.len(),
            max_level,
            source_point_count,
        };

        if semantics == HierarchySemantics::Additive
            && let Some(expected) = source_point_count
        {
            let actual = node_map.values().try_fold(0_u64, |sum, node| {
                sum.checked_add(node.metadata.point_count)
                    .ok_or(SpatialError::AdditivePointCountOverflow)
            })?;
            if actual != expected {
                return Err(SpatialError::AdditiveSourcePointCountMismatch { expected, actual });
            }
        }

        let mut block_ids = node_map
            .values()
            .flat_map(|indexed| indexed.metadata.blocks.iter().copied())
            .collect::<Vec<_>>();
        block_ids.sort_unstable();
        block_ids.dedup();

        Ok(Self {
            root,
            nodes: node_map,
            block_ids: block_ids.into_boxed_slice(),
            attributes,
            stats,
            semantics,
            unloaded: HashSet::new(),
            grafted: HashSet::new(),
        })
    }

    pub const fn root_id(&self) -> NodeId {
        self.root
    }

    pub fn node(&self, id: NodeId) -> Option<&SpatialNode> {
        self.nodes.get(&id).map(|indexed| &indexed.metadata)
    }

    /// Returns all referenced dataset-local block IDs in ascending order, without duplicates.
    ///
    /// The catalog stores one additional `u64` (8 bytes) per unique block ID.
    pub fn block_ids(&self) -> &[BlockId] {
        &self.block_ids
    }

    /// Checks whether any node references the dataset-local block identifier.
    pub fn references_block(&self, block_id: BlockId) -> bool {
        self.block_ids.binary_search(&block_id).is_ok()
    }

    /// Per-cloud reference spacing (RF-05) over every leaf; see [`reference_spacing`]. `None` while
    /// any anchor is unloaded, because the true leaves are then not resident.
    pub fn reference_spacing(&self) -> Option<f64> {
        if !self.unloaded.is_empty() {
            return None;
        }
        reference_spacing(
            self.nodes
                .values()
                .filter(|indexed| indexed.children.is_empty())
                .map(|indexed| {
                    (
                        indexed.metadata.point_count(),
                        indexed.metadata.representative_spacing(),
                    )
                }),
        )
    }

    /// Whether an existing node has no structural children.
    pub fn is_leaf(&self, id: NodeId) -> bool {
        self.nodes
            .get(&id)
            .is_some_and(|node| node.children.is_empty())
    }

    /// Structural children in stable NodeId order; empty for leaves or unknown IDs.
    /// Traversal is independent of storage block ordering and hierarchy semantics.
    pub fn child_ids(&self, id: NodeId) -> &[NodeId] {
        self.nodes
            .get(&id)
            .map_or(&[], |indexed| indexed.children.as_slice())
    }

    /// Whether the node's children are not loaded yet (an anchor of a missing page).
    pub fn is_unloaded(&self, id: NodeId) -> bool {
        self.unloaded.contains(&id)
    }

    /// Marks a childless node as an unloaded anchor: its subtree exists but is not resident.
    pub fn mark_unloaded(&mut self, id: NodeId) -> Result<(), SpatialError> {
        let node = self.nodes.get(&id).ok_or(SpatialError::UnknownNode(id))?;
        if !node.children.is_empty() {
            return Err(SpatialError::AnchorHasChildren(id));
        }
        self.unloaded.insert(id);
        Ok(())
    }

    /// Grows the hierarchy with the page of an unloaded `anchor`. `nodes` must be in
    /// parent-before-child order, each descending from the anchor; `anchors` are page nodes
    /// whose own pages stay unloaded. Validation is atomic: on error nothing changes.
    pub fn graft_page(
        &mut self,
        anchor: NodeId,
        nodes: Vec<SpatialNode>,
        anchors: &[NodeId],
    ) -> Result<(), SpatialError> {
        if !self.nodes.contains_key(&anchor) {
            return Err(SpatialError::UnknownNode(anchor));
        }
        if !self.unloaded.contains(&anchor) {
            return Err(SpatialError::NotUnloadedAnchor(anchor));
        }
        let mut incoming: HashMap<NodeId, &SpatialNode> = HashMap::with_capacity(nodes.len());
        for node in &nodes {
            if self.nodes.contains_key(&node.id) || incoming.insert(node.id, node).is_some() {
                return Err(SpatialError::DuplicateNodeId(node.id));
            }
        }
        for node in &nodes {
            let parent_id = node.parent.ok_or(SpatialError::PageNodeOutsideAnchor {
                node: node.id,
                anchor,
            })?;
            let parent = if parent_id == anchor {
                &self.nodes[&anchor].metadata
            } else if let Some(parent) = incoming.get(&parent_id) {
                parent
            } else {
                return Err(SpatialError::UnknownParent {
                    node: node.id,
                    parent: parent_id,
                });
            };
            if parent.level.checked_add(1) != Some(node.level) {
                return Err(SpatialError::InvalidChildLevel {
                    node: node.id,
                    parent: parent_id,
                });
            }
            if !parent.bounds.contains(&node.bounds) {
                return Err(SpatialError::ChildOutsideParent {
                    node: node.id,
                    parent: parent_id,
                });
            }
        }
        // Parents must precede children so the anchor reachability check below is one pass.
        let mut reachable = HashSet::from([anchor]);
        for node in &nodes {
            if !reachable.contains(&node.parent.expect("validated above")) {
                return Err(SpatialError::UnknownParent {
                    node: node.id,
                    parent: node.parent.expect("validated above"),
                });
            }
            reachable.insert(node.id);
        }
        for id in anchors {
            if !incoming.contains_key(id) {
                return Err(SpatialError::UnknownNode(*id));
            }
        }

        self.unloaded.remove(&anchor);
        self.grafted.insert(anchor);
        for node in nodes {
            let id = node.id;
            let parent = node.parent.expect("validated above");
            self.nodes
                .get_mut(&parent)
                .expect("parent was validated above")
                .children
                .push(id);
            self.nodes.insert(
                id,
                IndexedNode {
                    metadata: node,
                    children: Vec::new(),
                },
            );
        }
        self.unloaded.extend(anchors.iter().copied());
        self.refresh_derived();
        Ok(())
    }

    /// Shrinks the hierarchy by dropping the page below `anchor`, which becomes unloaded again.
    /// Refused while a descendant page is resident or a `retained` (planner cut) node lies in
    /// the page, so no retained node ever loses its ancestors. Returns the removed node ids.
    pub fn evict_page(
        &mut self,
        anchor: NodeId,
        retained: &[NodeId],
    ) -> Result<Vec<NodeId>, SpatialError> {
        if !self.nodes.contains_key(&anchor) {
            return Err(SpatialError::UnknownNode(anchor));
        }
        if !self.grafted.contains(&anchor) {
            return Err(SpatialError::NotGraftedPage(anchor));
        }
        let mut descendants = Vec::new();
        let mut pending = self.nodes[&anchor].children.clone();
        while let Some(id) = pending.pop() {
            descendants.push(id);
            pending.extend(self.nodes[&id].children.iter().copied());
        }
        if let Some(descendant) = descendants.iter().find(|id| self.grafted.contains(id)) {
            return Err(SpatialError::PageHasResidentDescendant {
                anchor,
                descendant: *descendant,
            });
        }
        let page: HashSet<NodeId> = descendants.iter().copied().collect();
        if let Some(node) = retained.iter().find(|id| page.contains(id)) {
            return Err(SpatialError::PageNodeRetained {
                anchor,
                node: *node,
            });
        }
        for id in &descendants {
            self.nodes.remove(id);
            self.unloaded.remove(id);
        }
        self.nodes
            .get_mut(&anchor)
            .expect("anchor checked above")
            .children
            .clear();
        self.grafted.remove(&anchor);
        self.unloaded.insert(anchor);
        self.refresh_derived();
        Ok(descendants)
    }

    /// Recomputes the values derived from the resident node set after a graft or eviction.
    fn refresh_derived(&mut self) {
        let mut block_ids = self
            .nodes
            .values()
            .flat_map(|indexed| indexed.metadata.blocks.iter().copied())
            .collect::<Vec<_>>();
        block_ids.sort_unstable();
        block_ids.dedup();
        self.block_ids = block_ids.into_boxed_slice();
        self.stats.node_count = self.nodes.len();
        self.stats.max_level = self
            .nodes
            .values()
            .map(|indexed| indexed.metadata.level)
            .max()
            .unwrap_or(0);
    }

    pub fn attributes(&self) -> &[AttributeDescriptor] {
        &self.attributes
    }

    pub const fn stats(&self) -> SpatialStats {
        self.stats
    }

    /// Sets the tight data extent reported in [`SpatialStats::data_bounds`]. Node bounds are
    /// untouched; the extent must lie inside the root bounds.
    pub fn with_data_bounds(mut self, data_bounds: SpatialBounds) -> Result<Self, SpatialError> {
        if !self.stats.root_bounds.contains(&data_bounds) {
            return Err(SpatialError::InvalidBounds {
                min: data_bounds.min(),
                max: data_bounds.max(),
            });
        }
        self.stats.data_bounds = data_bounds;
        Ok(self)
    }

    pub const fn semantics(&self) -> HierarchySemantics {
        self.semantics
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

    /// Selects visible payloads within a hard point budget.
    ///
    /// Replacement hierarchies substitute child payloads for parents; additive
    /// hierarchies retain parent payloads and add child payloads. Nodes with zero
    /// representatives are structural and are always traversed.
    /// The callback projects each node's world-space geometric error into pixels for
    /// the current camera. A visible base selection that already exceeds budget is rejected
    /// instead of returning partial coverage.
    pub fn select_lod(
        &self,
        frustum: &SpatialFrustum,
        config: LodSelectionConfig,
        projected_error_pixels: impl Fn(&SpatialNode) -> f64,
    ) -> Result<LodSelection, LodSelectionError> {
        let mut pending = Vec::new();
        let mut selected = Vec::new();
        self.visible_frontier_into([self.root], frustum, &mut pending, &mut selected);
        let mut frontier = selected.clone();
        let mut point_count = self.count_points(&selected)?;
        if point_count > config.point_budget {
            return Err(LodSelectionError::BaseCutExceedsBudget {
                required: point_count,
                budget: config.point_budget,
            });
        }

        // Retain scratch capacity within this selection, without caching projections or cuts.
        let mut children = Vec::new();
        let mut best_children = Vec::new();
        let mut budget_limited = Vec::new();
        let mut resolution_limited = Vec::new();
        let mut unloaded_anchors = Vec::new();
        loop {
            let mut best: Option<(usize, NodeId, f64, u64)> = None;
            budget_limited.clear();
            resolution_limited.clear();
            unloaded_anchors.clear();

            for (index, id) in frontier.iter().copied().enumerate() {
                let node = &self.nodes[&id].metadata;
                let error = projected_error_pixels(node);
                if error.is_nan() || error < 0.0 {
                    return Err(LodSelectionError::InvalidProjectedError { node: id, error });
                }
                if error <= config.max_error_pixels {
                    continue;
                }

                self.visible_frontier_into(
                    self.nodes[&id].children.iter().copied(),
                    frustum,
                    &mut pending,
                    &mut children,
                );
                if children.is_empty() {
                    if self.unloaded.contains(&id) {
                        unloaded_anchors.push(id);
                    } else {
                        resolution_limited.push(id);
                    }
                    continue;
                }
                let children_count = self.count_points(&children)?;
                let candidate_count = match self.semantics {
                    HierarchySemantics::Replacement => point_count
                        .checked_sub(node.point_count)
                        .and_then(|count| count.checked_add(children_count))
                        .ok_or(LodSelectionError::PointCountOverflow(id))?,
                    HierarchySemantics::Additive => point_count
                        .checked_add(children_count)
                        .ok_or(LodSelectionError::PointCountOverflow(id))?,
                };
                if candidate_count > config.point_budget {
                    budget_limited.push(id);
                    continue;
                }

                let should_replace = best.as_ref().is_none_or(|(_, best_id, best_error, _)| {
                    error > *best_error || (error == *best_error && id < *best_id)
                });
                if should_replace {
                    std::mem::swap(&mut children, &mut best_children);
                    best = Some((index, id, error, candidate_count));
                }
            }

            let Some((index, _, _, new_point_count)) = best else {
                let mut max_error_pixels: f64 = 0.0;
                for id in &frontier {
                    let error = projected_error_pixels(&self.nodes[id].metadata);
                    if error.is_nan() || error < 0.0 {
                        return Err(LodSelectionError::InvalidProjectedError { node: *id, error });
                    }
                    max_error_pixels = max_error_pixels.max(error);
                }
                return Ok(LodSelection {
                    node_ids: selected,
                    point_count,
                    point_budget: config.point_budget,
                    max_error_pixels,
                    budget_limited,
                    resolution_limited,
                    unloaded_anchors,
                });
            };

            match self.semantics {
                HierarchySemantics::Replacement => {
                    selected.splice(index..=index, best_children.iter().copied());
                }
                HierarchySemantics::Additive => {
                    let selected_index = selected
                        .iter()
                        .position(|selected_id| *selected_id == frontier[index])
                        .expect("frontier payload is selected");
                    selected.splice(
                        selected_index + 1..selected_index + 1,
                        best_children.iter().copied(),
                    );
                }
            }
            frontier.splice(index..=index, best_children.drain(..));
            point_count = new_point_count;
        }
    }

    fn visible_frontier_into(
        &self,
        starts: impl IntoIterator<Item = NodeId>,
        frustum: &SpatialFrustum,
        pending: &mut Vec<NodeId>,
        frontier: &mut Vec<NodeId>,
    ) {
        pending.clear();
        pending.extend(starts);
        pending.reverse();
        frontier.clear();
        while let Some(id) = pending.pop() {
            let indexed = &self.nodes[&id];
            if !frustum.intersects(&indexed.metadata.bounds) {
                continue;
            }
            // An unloaded anchor stays in the frontier even when empty: its page may hold points.
            if indexed.metadata.point_count > 0 || self.unloaded.contains(&id) {
                frontier.push(id);
            } else {
                pending.extend(indexed.children.iter().rev().copied());
            }
        }
    }

    fn count_points(&self, ids: &[NodeId]) -> Result<u64, LodSelectionError> {
        ids.iter().try_fold(0_u64, |count, id| {
            count
                .checked_add(self.nodes[id].metadata.point_count)
                .ok_or(LodSelectionError::PointCountOverflow(*id))
        })
    }
}

#[cfg(test)]
#[path = "spatial_lod_tests.rs"]
mod lod_tests;

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

#[path = "spatial_lod_priority.rs"]
mod lod_priority;
pub use lod_priority::{DEFAULT_LOD_PRIORITY_BYTES, LodPriorityFallback, LodPriorityStats};
