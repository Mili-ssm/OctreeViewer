//! Global integer frame of `v7.4-pre` ([spec](../../../docs/reference/format/FORMAT_V7_PRE4.md)).
//!
//! Coordinates are `i64` quanta of 2^-15 CRS units and the octree is implicit in the bits of
//! `u = q ^ 2^63`. Node geometry is computed in integers from level and prefix; every `f64`
//! slot that mirrors it is derived and informative. No `log2` is used (ADR 0016): exponents
//! come from `f64` bits.
use std::collections::{BTreeMap, BTreeSet};

use viewer_core::{BlockId, NodeId, SpatialBounds, SpatialError, SpatialNode};

use crate::MetadataError;

/// Quanta per CRS unit is 2^15; the quantum is fixed by the format version.
pub(crate) const UNITS_PER_QUANTUM: f64 = 1.0 / 32_768.0;
const QUANTA_PER_UNIT: f64 = 32_768.0;
/// Writers reject `|c| >= 2^48` CRS units so that `|q| < 2^63`.
const MAX_ABS_COORDINATE: f64 = 281_474_976_710_656.0;
const SIGN_BIT: u64 = 0x8000_0000_0000_0000;
const MAX_LEVEL: u32 = 64;
const MIN_SPACING_EXPONENT: i32 = -15;
/// Internal nodes sample a 256-cell grid: `side / 256 = 2^(41 - a)` CRS units.
const INTERNAL_SPACING_EXPONENT_BASE: i32 = 41;
const SIDE_EXPONENT_BASE: i32 = 49;
/// A block holds offsets `0..2^32` from its origin.
pub(crate) const BLOCK_SIDE_EXPONENT: u32 = 32;

/// `round(c * 2^15)` (ties away from zero) or `None` for non-finite or `|c| >= 2^48`.
pub fn quantize_coordinate(coordinate: f64) -> Option<i64> {
    if !coordinate.is_finite() || coordinate.abs() >= MAX_ABS_COORDINATE {
        return None;
    }
    Some(round_to_quanta(coordinate * QUANTA_PER_UNIT))
}

/// `value.round() as i64` (ties away from zero, saturating) without the libm call that `round`
/// is on baseline x86-64: adding the largest `f64` below 0.5 with the sign of `value` and
/// truncating gives the same integer for every finite `f64`, and `0` for NaN.
#[inline]
pub fn round_to_quanta(value: f64) -> i64 {
    (value + 0.499_999_999_999_999_94_f64.copysign(value)) as i64
}

/// Quanta to CRS units, rounded to the nearest `f64`.
pub fn quanta_to_units(quanta: i64) -> f64 {
    quanta as f64 * UNITS_PER_QUANTUM
}

fn shift_left(prefix: u64, shift: u32) -> u64 {
    prefix.checked_shl(shift).unwrap_or(0)
}

fn prefix_fits(level: u32, prefix: u64) -> bool {
    prefix.checked_shr(level).unwrap_or(0) == 0
}

/// Origin of the node `(level, prefix)` on one axis, in quanta.
pub fn cell_origin_quanta(level: u32, prefix: u64) -> i64 {
    (shift_left(prefix, MAX_LEVEL - level) ^ SIGN_BIT) as i64
}

/// Lattice base of a global cloud: `u = q - base` is the unsigned value whose bits form the prefix.
pub const GLOBAL_LATTICE_BASE: [i64; 3] = [i64::MIN; 3];

/// Origin of the node `(level, prefix)` on one axis for a lattice anchored at `base`, in quanta;
/// `None` if it does not fit `i64` (only a corrupt file).
pub fn cell_origin_from_base(base: i64, level: u32, prefix: u64) -> Option<i64> {
    i64::try_from(i128::from(base) + i128::from(shift_left(prefix, MAX_LEVEL - level))).ok()
}

/// Integer box of one node: origin in quanta and side `2^side_exponent` quanta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CellBox {
    pub(crate) origin: [i64; 3],
    pub(crate) side_exponent: u32,
}

impl CellBox {
    #[cfg(test)]
    pub(crate) fn new(level: u32, prefix: [u64; 3]) -> Self {
        Self {
            origin: prefix.map(|p| cell_origin_quanta(level, p)),
            side_exponent: MAX_LEVEL - level,
        }
    }

    /// The node `(level, prefix)` of the lattice anchored at `base` (see [`GlobalFrame::lattice_base`]).
    pub(crate) fn in_lattice(base: [i64; 3], level: u32, prefix: [u64; 3]) -> Option<Self> {
        let mut origin = [0_i64; 3];
        for axis in 0..3 {
            origin[axis] = cell_origin_from_base(base[axis], level, prefix[axis])?;
        }
        Some(Self {
            origin,
            side_exponent: MAX_LEVEL - level,
        })
    }

    pub(crate) fn side(self) -> i128 {
        1_i128 << self.side_exponent
    }

    pub(crate) fn contains(self, quanta: [i128; 3]) -> bool {
        (0..3).all(|axis| {
            let low = i128::from(self.origin[axis]);
            quanta[axis] >= low && quanta[axis] < low + self.side()
        })
    }

    /// The `f64` box relative to `frame_origin` (quanta), rounded to nearest and scaled by 2^-15.
    pub(crate) fn bounds(self, frame_origin: [i64; 3]) -> Result<SpatialBounds, SpatialError> {
        let to_units = |value: i128| value as f64 * UNITS_PER_QUANTUM;
        let min = std::array::from_fn(|axis| {
            to_units(i128::from(self.origin[axis]) - i128::from(frame_origin[axis]))
        });
        let max = std::array::from_fn(|axis| {
            to_units(i128::from(self.origin[axis]) + self.side() - i128::from(frame_origin[axis]))
        });
        SpatialBounds::new(min, max)
    }
}

/// A node box whose bounds and distances to the frame origin all fit in `i64`, so the per-point
/// checks and the local position need no `i128`. `None` from [`CellBox::i64_fast`] sends the
/// caller to the `i128` path (a node side above 2^62 quanta, or a shallow root far from the frame).
#[derive(Debug, Clone, Copy)]
pub(crate) struct FastBox {
    low: [i64; 3],
    high: [i64; 3],
    frame_origin: [i64; 3],
}

impl CellBox {
    pub(crate) fn i64_fast(self, frame_origin: [i64; 3]) -> Option<FastBox> {
        if self.side_exponent > 62 {
            return None;
        }
        let side = 1_i64 << self.side_exponent;
        let mut high = [0_i64; 3];
        for axis in 0..3 {
            high[axis] = self.origin[axis].checked_add(side)?;
            // `q - frame` is monotone in `q`, so both ends bound every point of the box.
            self.origin[axis].checked_sub(frame_origin[axis])?;
            high[axis].checked_sub(frame_origin[axis])?;
        }
        Some(FastBox {
            low: self.origin,
            high,
            frame_origin,
        })
    }
}

impl FastBox {
    /// `contains` for an axis-aligned box: it holds for every point of a set iff it holds for the
    /// per-axis minimum and maximum of the set.
    pub(crate) fn contains_extremes(&self, min: [i64; 3], max: [i64; 3]) -> bool {
        (0..3).all(|axis| min[axis] >= self.low[axis] && max[axis] < self.high[axis])
    }

    pub(crate) fn contains(&self, quanta: [i64; 3]) -> bool {
        self.contains_extremes(quanta, quanta)
    }

    /// Same value as [`local_position_units`] for a point inside the box.
    pub(crate) fn local_position_units(&self, quanta: [i64; 3]) -> [f64; 3] {
        std::array::from_fn(|axis| {
            (quanta[axis] - self.frame_origin[axis]) as f64 * UNITS_PER_QUANTUM
        })
    }
}

/// Origin of the block that holds `quanta` inside `node`: the node origin, or the origin of the
/// 2^32-quanta cell that contains the point when the node is larger than one block.
pub(crate) fn block_cell_origin(node: CellBox, quanta: [i64; 3]) -> [i64; 3] {
    if node.side_exponent <= BLOCK_SIDE_EXPONENT {
        return node.origin;
    }
    std::array::from_fn(|axis| {
        let relative = i128::from(quanta[axis]) - i128::from(node.origin[axis]);
        let cell = (relative >> BLOCK_SIDE_EXPONENT) << BLOCK_SIDE_EXPONENT;
        i64::try_from(i128::from(node.origin[axis]) + cell).expect("cell lies inside the node")
    })
}

/// Position relative to the frame origin in CRS units: `((q - frame) as f64) * 2^-15`, with the
/// difference in `i128` (it can exceed `i64` for a shallow root) rounded to nearest.
pub(crate) fn local_position_units(quanta: [i128; 3], frame_origin: [i64; 3]) -> [f64; 3] {
    std::array::from_fn(|axis| {
        (quanta[axis] - i128::from(frame_origin[axis])) as f64 * UNITS_PER_QUANTUM
    })
}

/// CRS identity plus the raw bytes of the source description (RF-02, RF-24).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrsRecord {
    authority: String,
    code: u32,
    raw: CrsRaw,
}

/// Raw CRS description kept whole and uninterpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrsRaw {
    None,
    /// Payload of the WKT VLR (`LASF_Projection`, record 2112), UTF-8.
    Wkt(String),
    /// Payloads of GeoTIFF records 34735 (key directory), 34736 (doubles) and 34737 (ASCII).
    GeoTiff {
        key_directory: Vec<u8>,
        doubles: Vec<u8>,
        ascii: Vec<u8>,
    },
}

pub(crate) const MAX_AUTHORITY_BYTES: usize = 16;

impl CrsRecord {
    /// `authority` is empty iff `code` is zero; otherwise upper-case ASCII of at most 16 bytes.
    pub fn new(authority: String, code: u32, raw: CrsRaw) -> Result<Self, MetadataError> {
        let authority_ok = authority.len() <= MAX_AUTHORITY_BYTES
            && authority
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && !byte.is_ascii_lowercase());
        if !authority_ok {
            return Err(MetadataError::InvalidCrs("authority"));
        }
        if authority.is_empty() != (code == 0) {
            return Err(MetadataError::InvalidCrs(
                "authority is empty if and only if the code is zero",
            ));
        }
        if matches!(&raw, CrsRaw::Wkt(text) if text.is_empty()) {
            return Err(MetadataError::InvalidCrs("empty WKT"));
        }
        Ok(Self {
            authority,
            code,
            raw,
        })
    }

    /// No code and no raw bytes: the cloud is **local** and its octree is anchored at the frame origin.
    pub fn is_local(&self) -> bool {
        self.code == 0 && matches!(self.raw, CrsRaw::None)
    }

    /// A cloud with neither code nor raw bytes: never merged with another cloud.
    pub fn local() -> Self {
        Self {
            authority: String::new(),
            code: 0,
            raw: CrsRaw::None,
        }
    }

    pub fn authority(&self) -> &str {
        &self.authority
    }

    pub const fn code(&self) -> u32 {
        self.code
    }

    pub fn raw(&self) -> &CrsRaw {
        &self.raw
    }

    /// `(authority, code)` when the source declared one.
    pub fn identity(&self) -> Option<(&str, u32)> {
        (self.code != 0).then_some((self.authority.as_str(), self.code))
    }

    pub(crate) fn identity_label(&self) -> Option<String> {
        self.identity()
            .map(|(authority, code)| format!("{authority}:{code}"))
    }

    pub(crate) fn raw_encoding(&self) -> u8 {
        match self.raw {
            CrsRaw::None => 0,
            CrsRaw::Wkt(_) => 1,
            CrsRaw::GeoTiff { .. } => 2,
        }
    }

    /// Serialized raw payload: the WKT bytes, or three `u32`-length-prefixed byte strings.
    pub(crate) fn raw_bytes(&self) -> Vec<u8> {
        match &self.raw {
            CrsRaw::None => Vec::new(),
            CrsRaw::Wkt(text) => text.as_bytes().to_vec(),
            CrsRaw::GeoTiff {
                key_directory,
                doubles,
                ascii,
            } => {
                let mut bytes = Vec::new();
                for part in [key_directory, doubles, ascii] {
                    bytes.extend_from_slice(&(part.len() as u32).to_le_bytes());
                    bytes.extend_from_slice(part);
                }
                bytes
            }
        }
    }

    pub(crate) fn from_parts(
        authority: String,
        code: u32,
        encoding: u8,
        raw: &[u8],
    ) -> Result<Self, &'static str> {
        let raw = match encoding {
            0 if raw.is_empty() => CrsRaw::None,
            0 => return Err("CRS raw bytes without an encoding"),
            1 => CrsRaw::Wkt(
                std::str::from_utf8(raw)
                    .map_err(|_| "CRS WKT is not UTF-8")?
                    .to_owned(),
            ),
            2 => {
                let mut rest = raw;
                let mut parts = [Vec::new(), Vec::new(), Vec::new()];
                for part in &mut parts {
                    let (length, tail) = rest
                        .split_first_chunk::<4>()
                        .ok_or("CRS GeoTIFF records are truncated")?;
                    let length = u32::from_le_bytes(*length) as usize;
                    if tail.len() < length {
                        return Err("CRS GeoTIFF records are truncated");
                    }
                    let (bytes, tail) = tail.split_at(length);
                    *part = bytes.to_vec();
                    rest = tail;
                }
                if !rest.is_empty() {
                    return Err("CRS GeoTIFF records have trailing bytes");
                }
                let [key_directory, doubles, ascii] = parts;
                CrsRaw::GeoTiff {
                    key_directory,
                    doubles,
                    ascii,
                }
            }
            _ => return Err("unknown CRS raw encoding"),
        };
        Self::new(authority, code, raw).map_err(|_| "invalid CRS record")
    }
}

/// Scale and offset of the source LAS file, kept for exact export.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceScaleOffset {
    scale: [f64; 3],
    offset: [f64; 3],
}

impl SourceScaleOffset {
    pub fn new(scale: [f64; 3], offset: [f64; 3]) -> Result<Self, MetadataError> {
        if scale
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
            || offset.iter().any(|value| !value.is_finite())
        {
            return Err(MetadataError::InvalidGlobalFrame("source scale or offset"));
        }
        Ok(Self { scale, offset })
    }

    pub const fn scale(&self) -> [f64; 3] {
        self.scale
    }

    pub const fn offset(&self) -> [f64; 3] {
        self.offset
    }

    /// `X = round((q * 2^-15 - offset) / scale)`. Exact for `scale >= 1e-4`: the grid error is at
    /// most 2^-16, below half the scale. `None` outside the `i64` range.
    pub fn las_integer(&self, axis: usize, quanta: i64) -> Option<i64> {
        let value = ((quanta_to_units(quanta) - self.offset[axis]) / self.scale[axis]).round();
        (value.is_finite() && value.abs() < 9.0e18).then_some(value as i64)
    }
}

/// Prefix and spacing of one node; its absolute level is the root level plus its depth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeCell {
    prefix: [u64; 3],
    spacing: f64,
}

impl NodeCell {
    pub const fn new(prefix: [u64; 3], spacing: f64) -> Self {
        Self { prefix, spacing }
    }

    pub const fn prefix(&self) -> [u64; 3] {
        self.prefix
    }

    pub const fn spacing(&self) -> f64 {
        self.spacing
    }
}

/// Frame, CRS and per-node identity of a `v7.4-pre` file.
#[derive(Debug, Clone, PartialEq)]
pub struct GlobalFrame {
    root_level: u32,
    root_prefix: [u64; 3],
    frame_origin_quanta: [i64; 3],
    data_max_quanta: [i64; 3],
    lattice_base: [i64; 3],
    crs: CrsRecord,
    source_scale_offset: Option<SourceScaleOffset>,
    nodes: BTreeMap<NodeId, NodeCell>,
    block_owner: BTreeMap<BlockId, (NodeId, u32)>,
}

impl GlobalFrame {
    pub fn new(
        root_level: u32,
        root_prefix: [u64; 3],
        frame_origin_quanta: [i64; 3],
        data_max_quanta: [i64; 3],
        crs: CrsRecord,
        source_scale_offset: Option<SourceScaleOffset>,
        nodes: Vec<(NodeId, NodeCell)>,
    ) -> Result<Self, MetadataError> {
        if root_level > MAX_LEVEL || root_prefix.iter().any(|p| !prefix_fits(root_level, *p)) {
            return Err(MetadataError::InvalidGlobalFrame("root level or prefix"));
        }
        let lattice_base = if crs.is_local() {
            frame_origin_quanta
        } else {
            GLOBAL_LATTICE_BASE
        };
        let root = CellBox::in_lattice(lattice_base, root_level, root_prefix)
            .ok_or(MetadataError::InvalidGlobalFrame("root level or prefix"))?;
        if !root.contains(frame_origin_quanta.map(i128::from)) {
            return Err(MetadataError::InvalidGlobalFrame(
                "frame origin outside the root node",
            ));
        }
        if (0..3).any(|axis| data_max_quanta[axis] < frame_origin_quanta[axis]) {
            return Err(MetadataError::InvalidGlobalFrame(
                "data maximum below the frame origin",
            ));
        }
        if !root.contains(data_max_quanta.map(i128::from)) {
            return Err(MetadataError::InvalidGlobalFrame(
                "data maximum outside the root node",
            ));
        }
        let mut cells = BTreeMap::new();
        for (id, cell) in nodes {
            if cells.insert(id, cell).is_some() {
                return Err(MetadataError::InvalidGlobalFrame("repeated node cell"));
            }
        }
        Ok(Self {
            root_level,
            root_prefix,
            frame_origin_quanta,
            data_max_quanta,
            lattice_base,
            crs,
            source_scale_offset,
            nodes: cells,
            block_owner: BTreeMap::new(),
        })
    }

    /// Absolute level `a0` of the stored root.
    pub const fn root_level(&self) -> u32 {
        self.root_level
    }

    pub const fn root_prefix(&self) -> [u64; 3] {
        self.root_prefix
    }

    pub fn crs(&self) -> &CrsRecord {
        &self.crs
    }

    pub fn source_scale_offset(&self) -> Option<&SourceScaleOffset> {
        self.source_scale_offset.as_ref()
    }

    pub fn node_cell(&self, id: NodeId) -> Option<&NodeCell> {
        self.nodes.get(&id)
    }

    /// Frame origin in quanta: a point inside the root node (the converter uses the cloud's minimum
    /// corner) that keeps `f32` local positions small for a shallow root.
    pub const fn frame_origin_quanta(&self) -> [i64; 3] {
        self.frame_origin_quanta
    }

    /// Maximum quantized corner of the data: with the frame origin (the minimum corner) it is the
    /// tight extent of the points, unlike the lattice-cube node bounds.
    pub const fn data_max_quanta(&self) -> [i64; 3] {
        self.data_max_quanta
    }

    /// Tight data extent in CRS units (world space, not frame-local): what the viewer frames.
    pub fn data_bounds(&self) -> Result<SpatialBounds, SpatialError> {
        SpatialBounds::new(
            self.frame_origin_quanta.map(quanta_to_units),
            self.data_max_quanta.map(quanta_to_units),
        )
    }

    /// Frame origin in CRS units, rounded to nearest: what the index and `CoordinateFrame` carry.
    pub fn frame_origin(&self) -> [f64; 3] {
        self.frame_origin_quanta().map(quanta_to_units)
    }

    /// Derived `f64` bounds (relative to the frame origin) of a node at `depth` below the root.
    pub fn node_bounds(&self, depth: u32, prefix: [u64; 3]) -> Result<SpatialBounds, SpatialError> {
        self.node_box(depth, prefix)
            .ok_or(SpatialError::InvalidBounds {
                min: [f64::NAN; 3],
                max: [f64::NAN; 3],
            })?
            .bounds(self.frame_origin_quanta())
    }

    /// Per-axis quanta whose offset from a node origin is the node prefix: `i64::MIN` (the sign
    /// bit flip) for a cloud with a CRS identity, the frame origin for a local cloud.
    pub const fn lattice_base(&self) -> [i64; 3] {
        self.lattice_base
    }

    pub(crate) fn node_box(&self, depth: u32, prefix: [u64; 3]) -> Option<CellBox> {
        let level = self.root_level.checked_add(depth)?;
        if level > MAX_LEVEL {
            return None;
        }
        CellBox::in_lattice(self.lattice_base, level, prefix)
    }

    /// Integer box of the node that owns `block`, if any.
    pub(crate) fn block_box(&self, block: BlockId) -> Option<CellBox> {
        let (node_id, depth) = *self.block_owner.get(&block)?;
        self.node_box(depth, self.nodes.get(&node_id)?.prefix)
    }

    /// Records which node owns each block; a block must have exactly one owner.
    pub(crate) fn bind_blocks(&mut self, nodes: &[&SpatialNode]) -> Result<(), MetadataError> {
        self.block_owner.clear();
        for node in nodes {
            for block in node.blocks() {
                if self
                    .block_owner
                    .insert(*block, (node.id(), node.level()))
                    .is_some()
                {
                    return Err(MetadataError::InvalidGlobalFrame(
                        "a block belongs to more than one node",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Exponent of a positive normal power of two read from the `f64` bits, no `log2`.
pub(crate) fn power_of_two_exponent(value: f64) -> Option<i32> {
    let bits = value.to_bits();
    let exponent_field = ((bits >> 52) & 0x7ff) as i32;
    let valid = bits >> 63 == 0
        && bits & ((1_u64 << 52) - 1) == 0
        && exponent_field != 0
        && exponent_field != 0x7ff;
    valid.then_some(exponent_field - 1023)
}

/// Checks every cross-field rule of the spec's "Validation (readers)" for the node table.
/// `frame_origin` is the dataset frame origin carried by the index.
pub(crate) fn validate_nodes(
    frame: &GlobalFrame,
    frame_origin: [f64; 3],
    nodes: &[&SpatialNode],
) -> Result<(), &'static str> {
    if frame_origin != frame.frame_origin() {
        return Err("frame origin does not match the root prefix");
    }
    if nodes.len() != frame.nodes.len() {
        return Err("node cells and hierarchy nodes disagree");
    }
    let by_id: BTreeMap<NodeId, &SpatialNode> = nodes.iter().map(|n| (n.id(), *n)).collect();
    let frame_origin_quanta = frame.frame_origin_quanta();
    let mut parents = BTreeSet::new();
    for node in nodes {
        if let Some(parent) = node.parent() {
            parents.insert(parent);
        }
    }
    let mut identities = BTreeSet::new();
    for node in nodes {
        let cell = frame
            .nodes
            .get(&node.id())
            .ok_or("node cells and hierarchy nodes disagree")?;
        let level = frame
            .root_level
            .checked_add(node.level())
            .filter(|level| *level <= MAX_LEVEL)
            .ok_or("node absolute level exceeds 64")?;
        match node.parent() {
            None => {
                if node.level() != 0 || cell.prefix != frame.root_prefix {
                    return Err("root prefix does not match the index root");
                }
            }
            Some(parent) => {
                let parent_cell = frame
                    .nodes
                    .get(&parent)
                    .zip(by_id.get(&parent))
                    .ok_or("node parent is missing")?;
                if parent_cell.1.level() + 1 != node.level()
                    || (0..3).any(|axis| cell.prefix[axis] >> 1 != parent_cell.0.prefix[axis])
                {
                    return Err("child prefix does not extend its parent prefix");
                }
            }
        }
        if cell.prefix.iter().any(|p| !prefix_fits(level, *p)) {
            return Err("node prefix exceeds its level");
        }
        if !identities.insert((level, cell.prefix)) {
            return Err("two nodes share level and prefix");
        }
        let derived = CellBox::in_lattice(frame.lattice_base, level, cell.prefix)
            .ok_or("node origin exceeds the quanta range")?
            .bounds(frame_origin_quanta)
            .map_err(|_| "derived node bounds are invalid")?;
        if node.bounds().min() != derived.min() || node.bounds().max() != derived.max() {
            return Err("node bounds are not derived from level and prefix");
        }
        let exponent =
            power_of_two_exponent(cell.spacing).ok_or("spacing is not a power of two")?;
        let level_exponent = i32::try_from(level).map_err(|_| "level")?;
        if exponent < MIN_SPACING_EXPONENT || exponent > SIDE_EXPONENT_BASE - level_exponent {
            return Err("spacing is outside 2^-15 .. node side");
        }
        if parents.contains(&node.id())
            && exponent != INTERNAL_SPACING_EXPONENT_BASE - level_exponent
        {
            return Err("internal node spacing is not side / 256");
        }
    }
    Ok(())
}

/// A block of an `a >= 32` node starts at the node origin; a larger node is split into cells of
/// side 2^32 quanta whose origin is the node origin plus a multiple of 2^32 per axis.
pub(crate) fn block_origin_is_valid(node: CellBox, origin: [i64; 3]) -> bool {
    (0..3).all(|axis| {
        let difference = i128::from(origin[axis]) - i128::from(node.origin[axis]);
        if node.side_exponent <= BLOCK_SIDE_EXPONENT {
            difference == 0
        } else {
            difference >= 0
                && difference < node.side()
                && difference % (1_i128 << BLOCK_SIDE_EXPONENT) == 0
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantizes_to_the_fixed_grid_and_rejects_out_of_range() {
        assert_eq!(quantize_coordinate(1.0), Some(32_768));
        assert_eq!(quantize_coordinate(-1.0), Some(-32_768));
        assert_eq!(quantize_coordinate(0.5 / 32_768.0), Some(1));
        assert_eq!(quantize_coordinate(-0.5 / 32_768.0), Some(-1));
        assert_eq!(quantize_coordinate(MAX_ABS_COORDINATE), None);
        assert!(quantize_coordinate(MAX_ABS_COORDINATE - 1.0).is_some());
        assert_eq!(quantize_coordinate(f64::NAN), None);
    }

    #[test]
    fn cell_origins_follow_the_flipped_sign_bit() {
        assert_eq!(cell_origin_quanta(0, 0), i64::MIN);
        assert_eq!(cell_origin_quanta(1, 0), i64::MIN);
        assert_eq!(cell_origin_quanta(1, 1), 0);
        assert_eq!(cell_origin_quanta(64, u64::MAX), i64::MAX);
        assert_eq!(cell_origin_quanta(64, 1_u64 << 63), 0);
        let whole = CellBox::new(0, [0; 3]);
        assert_eq!(whole.side(), 1_i128 << 64);
        assert!(whole.contains([i128::from(i64::MAX); 3]));
    }

    #[test]
    fn spacing_exponents_come_from_the_bits() {
        assert_eq!(power_of_two_exponent(1.0), Some(0));
        assert_eq!(power_of_two_exponent(0.25), Some(-2));
        assert_eq!(power_of_two_exponent(2.0_f64.powi(-15)), Some(-15));
        for bad in [0.0, -1.0, 3.0, 0.3, f64::NAN, f64::INFINITY, 1e-310] {
            assert_eq!(power_of_two_exponent(bad), None, "{bad}");
        }
    }

    #[test]
    fn crs_record_enforces_its_cross_field_rules() {
        assert!(CrsRecord::new("EPSG".into(), 32632, CrsRaw::None).is_ok());
        assert!(CrsRecord::new(String::new(), 0, CrsRaw::None).is_ok());
        assert!(CrsRecord::new("epsg".into(), 1, CrsRaw::None).is_err());
        assert!(CrsRecord::new(String::new(), 1, CrsRaw::None).is_err());
        assert!(CrsRecord::new("EPSG".into(), 0, CrsRaw::None).is_err());
        assert!(CrsRecord::new("A".repeat(17), 1, CrsRaw::None).is_err());
        assert!(CrsRecord::new(String::new(), 0, CrsRaw::Wkt(String::new())).is_err());
    }

    #[test]
    fn block_origins_follow_the_split_rule() {
        let small = CellBox::new(40, [5; 3]);
        assert!(block_origin_is_valid(small, small.origin));
        assert!(!block_origin_is_valid(small, small.origin.map(|o| o + 1)));
        let large = CellBox::new(30, [3; 3]);
        assert!(block_origin_is_valid(
            large,
            large.origin.map(|o| o + (1_i64 << 32))
        ));
        assert!(!block_origin_is_valid(large, large.origin.map(|o| o + 1)));
        assert!(!block_origin_is_valid(
            large,
            large.origin.map(|o| o - (1_i64 << 32))
        ));
    }
}
