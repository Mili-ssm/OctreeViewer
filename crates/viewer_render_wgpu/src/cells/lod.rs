//! Pure-Rust twin of the per-cell LOD decision the GPU makes for each point of a node.
//!
//! Every length is in scaled render units with the eye at the origin. The WGSL port mirrors
//! these functions operation by operation (plain `*` then `+`, no `mul_add`), so a cell and its
//! children evaluate the same floats on both sides. `refines` is not monotone in general, so
//! the draw rule checks every coarser active depth instead of relying on it.
//!
//! Integer cell coordinates are relative to the node corner: at depth `d` a cell's `rel` ranges
//! over `[0, 2^(d - node_depth))`, and bit `i` of [`NodeLod::active`] is depth `node_depth + i`.

use crate::OrbitCamera;
use crate::camera::VERTICAL_FOV;
use crate::cells::pages::NODE_WORDS;

/// Deepest span below the node depth a cell can have: `rel` stays a `u32` and no shift reaches 32.
pub const MAX_NODE_SPAN: u8 = 31;

/// Depth offset below the node depth where the 8x8x8 mask lives (one bit per cell).
const MASK_DEPTH_OFFSET: u8 = 3;

/// The camera as the per-cell decision sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LodView {
    pub forward: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub focal: f32,
    pub target_px: f32,
    /// A cell refines only if its children are at least this long (`side / 2 >= min_side`).
    pub min_side: f32,
}

impl LodView {
    /// `camera` positions are already in scaled units; the eye is the origin of the cells.
    pub fn from_camera(
        camera: &OrbitCamera,
        viewport: [f32; 2],
        target_px: f32,
        min_side_scaled: f32,
    ) -> LodView {
        let (right, up) = camera.right_up();
        LodView {
            forward: camera.forward().map(|c| c as f32),
            right: right.map(|c| c as f32),
            up: up.map(|c| c as f32),
            focal: (f64::from(viewport[1]) / (2.0 * (VERTICAL_FOV * 0.5).tan())) as f32,
            target_px,
            min_side: min_side_scaled,
        }
    }
}

/// What the decision needs of a node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeLod {
    /// Node corner, eye-relative.
    pub anchor: [f32; 3],
    pub node_depth: u8,
    /// Bit `d - node_depth` set: depth `d` is active.
    pub active: u64,
    /// 8x8x8 mask: bit `x + 8y` of word `z`, as `CellCutEntry::mask`.
    pub mask: [u64; 8],
}

impl NodeLod {
    /// `(4^3 mip, 2^3 mip, 1^3 mip)`: a bit is set iff any of its 2^3 children is set one
    /// level finer (4^3 bit `x + 4y + 16z`, 2^3 bit `x + 2y + 4z`).
    pub fn mips(&self) -> (u64, u8, bool) {
        let bit8 = |x: usize, y: usize, z: usize| self.mask[z] >> (x + 8 * y) & 1 != 0;
        let mut m4 = 0u64;
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..4 {
                    let any = (0..8)
                        .any(|c| bit8(2 * x + (c & 1), 2 * y + (c >> 1 & 1), 2 * z + (c >> 2)));
                    m4 |= u64::from(any) << (x + 4 * y + 16 * z);
                }
            }
        }
        let bit4 = |x: usize, y: usize, z: usize| m4 >> (x + 4 * y + 16 * z) & 1 != 0;
        let mut m2 = 0u8;
        for z in 0..2 {
            for y in 0..2 {
                for x in 0..2 {
                    let any = (0..8)
                        .any(|c| bit4(2 * x + (c & 1), 2 * y + (c >> 1 & 1), 2 * z + (c >> 2)));
                    m2 |= u8::from(any) << (x + 2 * y + 4 * z);
                }
            }
        }
        (m4, m2, m2 != 0)
    }
}

/// The GPU node record of `node` (`NODE_WORDS` layout, E8): mask, anchor bits, node depth,
/// active depths, 4^3 mip, 2^3 mip with the 1^3 mip at bit 8, then zeros.
pub fn node_record(node: &NodeLod) -> [u32; NODE_WORDS] {
    let mut record = [0u32; NODE_WORDS];
    for (i, word) in record[..16].iter_mut().enumerate() {
        *word = (node.mask[i / 2] >> (32 * (i % 2))) as u32;
    }
    for axis in 0..3 {
        record[16 + axis] = node.anchor[axis].to_bits();
    }
    record[19] = u32::from(node.node_depth);
    record[20] = node.active as u32;
    record[21] = (node.active >> 32) as u32;
    let (m4, m2, m1) = node.mips();
    record[22] = m4 as u32;
    record[23] = (m4 >> 32) as u32;
    record[24] = u32::from(m2) | u32::from(m1) << 8;
    record
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Longest projected edge of the cube `[lo, lo + side]`, in pixels (the twin of
/// `OrbitCamera::projected_voxel_edge_pixels`); `+inf` when any corner is at or behind the eye
/// plane, so a cell around the eye always refines.
///
/// Operation order, copied literally by the WGSL port: corner coordinate `lo[a]` or `lo[a] + side`
/// (corner bit `a` of the index selects the far side); `d = x*fx + y*fy + z*fz`; `+inf` if
/// `d <= 0`; pixels `focal * (dot(o, right) / d)` and `focal * (dot(o, up) / d)`; the result is
/// the maximum over the 12 edges of `sqrt(dx*dx + dy*dy)`.
pub fn edge_px(view: &LodView, lo: [f32; 3], side: f32) -> f32 {
    let mut projected = [[0.0f32; 2]; 8];
    for (index, pixel) in projected.iter_mut().enumerate() {
        let corner: [f32; 3] = std::array::from_fn(|a| {
            if index >> a & 1 == 0 {
                lo[a]
            } else {
                lo[a] + side
            }
        });
        let depth = dot(corner, view.forward);
        if depth <= 0.0 {
            return f32::INFINITY;
        }
        *pixel = [
            view.focal * (dot(corner, view.right) / depth),
            view.focal * (dot(corner, view.up) / depth),
        ];
    }
    let mut maximum = 0.0f32;
    for index in 0..8usize {
        for axis in 0..3 {
            if index >> axis & 1 != 0 {
                continue;
            }
            let other = index | 1 << axis;
            let dx = projected[index][0] - projected[other][0];
            let dy = projected[index][1] - projected[other][1];
            maximum = maximum.max((dx * dx + dy * dy).sqrt());
        }
    }
    maximum
}

/// Smallest projected edge among the eight children of the cell `[lo, lo + side]`, in pixels; a
/// child's edge is the longest of its 12 projected edges, as in `edge_px`. A child with a corner
/// at or behind the eye plane measures `+inf` (it never limits refinement), so the result is
/// `+inf` when every child has one.
///
/// Operation order, copied literally by the WGSL port: `half = side * 0.5`; the 3x3x3 grid of
/// child corners has coordinates `lo[a]`, `lo[a] + half`, `(lo[a] + half) + half` at index 0, 1, 2
/// (point `i + 3j + 9k`); each point is projected once as in `edge_px` and a point with
/// `d <= 0` is marked behind; child `(cx, cy, cz)` is `+inf` if any of its 8 points is behind,
/// else the maximum over its 12 edges of `sqrt(dx*dx + dy*dy)`, edges visited as in `edge_px`
/// (corner index bit `a` selects the far point on axis `a`); the result is the minimum child.
pub fn min_child_edge_px(view: &LodView, lo: [f32; 3], side: f32) -> f32 {
    let half = side * 0.5;
    let mut projected = [[0.0f32; 2]; 27];
    let mut behind = 0u32;
    for (index, pixel) in projected.iter_mut().enumerate() {
        let steps = [index % 3, index / 3 % 3, index / 9];
        let point: [f32; 3] = std::array::from_fn(|a| match steps[a] {
            0 => lo[a],
            1 => lo[a] + half,
            _ => (lo[a] + half) + half,
        });
        let depth = dot(point, view.forward);
        if depth <= 0.0 {
            behind |= 1 << index;
            continue;
        }
        *pixel = [
            view.focal * (dot(point, view.right) / depth),
            view.focal * (dot(point, view.up) / depth),
        ];
    }
    let mut smallest = f32::INFINITY;
    for child in 0..8usize {
        let base = (child & 1) + (child >> 1 & 1) * 3 + (child >> 2 & 1) * 9;
        let point = |corner: usize| base + (corner & 1) + (corner >> 1 & 1) * 3 + (corner >> 2) * 9;
        if (0..8).any(|corner| behind >> point(corner) & 1 != 0) {
            continue;
        }
        let mut maximum = 0.0f32;
        for corner in 0..8usize {
            for axis in 0..3 {
                if corner >> axis & 1 != 0 {
                    continue;
                }
                let first = projected[point(corner)];
                let second = projected[point(corner | 1 << axis)];
                let dx = first[0] - second[0];
                let dy = first[1] - second[1];
                maximum = maximum.max((dx * dx + dy * dy).sqrt());
            }
        }
        smallest = smallest.min(maximum);
    }
    smallest
}

/// E1 (ADR 0054): the cell is replaced by finer ones iff its children would still be at least
/// `min_side` long and every one of them at least `target_px` on screen (`min_child_edge_px`; a
/// child behind the eye plane never blocks, so a cell around the eye always refines). The target
/// is a minimum: the smallest child is not always the one farthest along `forward` off axis.
pub fn refines(view: &LodView, lo: [f32; 3], side: f32) -> bool {
    side * 0.5 >= view.min_side && min_child_edge_px(view, lo, side) >= view.target_px
}

/// `2^exponent` as an `f32`, exact for `0 <= exponent <= MAX_NODE_SPAN`.
fn pow2(exponent: u32) -> f32 {
    f32::from_bits((127 + exponent) << 23)
}

/// E3: lower corner of the cell `rel` of side `side`, eye-relative.
pub fn cell_lo(node: &NodeLod, rel: [u32; 3], side: f32) -> [f32; 3] {
    std::array::from_fn(|axis| node.anchor[axis] + rel[axis] as f32 * side)
}

/// E5: does any set mask bit lie inside the cell `rel` of `depth`?
pub fn masked_any(node: &NodeLod, depth: u8, rel: [u32; 3]) -> bool {
    debug_assert!(depth >= node.node_depth && depth - node.node_depth <= MAX_NODE_SPAN);
    let level = depth - node.node_depth;
    debug_assert!(rel.iter().all(|&r| u64::from(r) < 1u64 << level));
    if level >= MASK_DEPTH_OFFSET {
        let shift = level - MASK_DEPTH_OFFSET;
        let [x, y, z] = rel.map(|r| (r >> shift) as usize);
        return node.mask[z] >> (x + 8 * y) & 1 != 0;
    }
    let (m4, m2, m1) = node.mips();
    let [x, y, z] = rel.map(|r| r as usize);
    match level {
        0 => m1,
        1 => m2 >> (x + 2 * y + 4 * z) & 1 != 0,
        _ => m4 >> (x + 4 * y + 16 * z) & 1 != 0,
    }
}

/// E4 (all ancestors): does the cell `rel` of `depth` (side `side`) draw its points?
/// Exactly one active depth draws each unmasked point whose depths are all evaluated.
pub fn draws(view: &LodView, node: &NodeLod, depth: u8, rel: [u32; 3], side: f32) -> bool {
    debug_assert!(depth >= node.node_depth && depth - node.node_depth <= MAX_NODE_SPAN);
    let level = u32::from(depth - node.node_depth);
    if node.active >> level & 1 == 0 || masked_any(node, depth, rel) {
        return false;
    }
    let finer = node.active >> level >> 1 != 0;
    if finer && refines(view, cell_lo(node, rel, side), side) {
        return false;
    }
    // Every coarser active depth must refine (or be masked over this cell), coarsest first.
    let mut coarser = node.active & ((1u64 << level) - 1);
    while coarser != 0 {
        let ancestor_level = coarser.trailing_zeros();
        coarser &= coarser - 1;
        let up = level - ancestor_level;
        let ancestor_rel = rel.map(|r| r >> up);
        let ancestor_side = side * pow2(up);
        let ancestor_depth = node.node_depth + ancestor_level as u8;
        if !(refines(
            view,
            cell_lo(node, ancestor_rel, ancestor_side),
            ancestor_side,
        ) || masked_any(node, ancestor_depth, ancestor_rel))
        {
            return false;
        }
    }
    true
}

/// Oracle for simulations: the active depth whose cell containing `point` draws it, if any.
/// `point` and `node_lo` share a frame (any f64 one); `side_at_node_depth` is the node side.
pub fn drawn_depth_at(
    view: &LodView,
    node: &NodeLod,
    point: [f64; 3],
    node_lo: [f64; 3],
    side_at_node_depth: f64,
) -> Option<u8> {
    (0..=u32::from(MAX_NODE_SPAN)).find_map(|level| {
        if node.active >> level & 1 == 0 {
            return None;
        }
        let side = side_at_node_depth / f64::from(pow2(level));
        let cells = (1u64 << level) as f64;
        let rel = std::array::from_fn(|axis| {
            ((point[axis] - node_lo[axis]) / side)
                .floor()
                .clamp(0.0, cells - 1.0) as u32
        });
        let depth = node.node_depth + level as u8;
        draws(view, node, depth, rel, side as f32).then_some(depth)
    })
}

#[cfg(test)]
#[path = "lod_tests.rs"]
mod tests;
