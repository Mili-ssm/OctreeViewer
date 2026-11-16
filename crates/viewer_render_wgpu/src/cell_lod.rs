//! Pure-Rust twin of the per-cell LOD decision the GPU makes for each point of a node.
//!
//! Every length is in scaled render units with the eye at the origin. The WGSL port mirrors
//! these functions operation by operation (plain `*` then `+`, no `mul_add`), so a cell and its
//! children evaluate the same floats on both sides.
//!
//! Integer cell coordinates are relative to the node corner: at depth `d` a cell's `rel` ranges
//! over `[0, 2^(d - node_depth))`, and bit `i` of [`NodeLod::active`] is depth `node_depth + i`.

// Nothing outside the tests calls it yet: the cut, picking and the GPU port arrive in later tasks.
#![allow(dead_code)]

use crate::OrbitCamera;
use crate::camera::VERTICAL_FOV;

/// Deepest span below the node depth a cell can have: `rel` stays a `u32` and no shift reaches 32.
pub const MAX_NODE_SPAN: u8 = 31;

/// Depth offset below the node depth where the 8x8x8 mask lives (one bit per cell).
const MASK_DEPTH_OFFSET: u8 = 3;

/// The camera as the per-cell decision sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LodView {
    pub forward: [f32; 3],
    pub focal: f32,
    /// `OrbitCamera::lod_min_depth` in scaled units: the bound clamps the nearest depth to it.
    pub near: f32,
    pub target_px: f32,
    /// A cell with `side <= min_side` never refines.
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
        let forward = camera.forward();
        LodView {
            forward: [forward[0] as f32, forward[1] as f32, forward[2] as f32],
            focal: (f64::from(viewport[1]) / (2.0 * (VERTICAL_FOV * 0.5).tan())) as f32,
            near: camera.lod_min_depth() as f32,
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

/// Upper bound of the projected edge of the cube `[lo, lo + side]`, in pixels:
/// `side * focal * dmax / max(zmin, near)^2`, with `dmax` the distance from the eye to the
/// farthest corner. Clamping `zmin` to `near` keeps it finite for a cell around the eye.
pub fn edge_bound(view: &LodView, lo: [f32; 3], side: f32) -> f32 {
    let half = side * 0.5;
    let centre = [lo[0] + half, lo[1] + half, lo[2] + half];
    let forward = view.forward;
    let depth = centre[0] * forward[0] + centre[1] * forward[1] + centre[2] * forward[2];
    let spread = forward[0].abs() + forward[1].abs() + forward[2].abs();
    let zmin = (depth - half * spread).max(view.near);
    let fx = centre[0].abs() + half;
    let fy = centre[1].abs() + half;
    let fz = centre[2].abs() + half;
    let dmax = (fx * fx + fy * fy + fz * fz).sqrt();
    side * view.focal * dmax / (zmin * zmin)
}

/// E1: the cell is worth replacing by finer ones.
pub fn refines(view: &LodView, lo: [f32; 3], side: f32) -> bool {
    side > view.min_side && edge_bound(view, lo, side) > view.target_px
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

/// E4: does the cell `rel` of `depth` (side `side`) draw its points?
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
    let coarser = node.active & ((1u64 << level) - 1);
    if coarser == 0 {
        return true;
    }
    let prev_level = 63 - coarser.leading_zeros();
    let up = level - prev_level;
    let prev_rel = rel.map(|r| r >> up);
    let prev_side = side * pow2(up);
    let prev_depth = node.node_depth + prev_level as u8;
    refines(view, cell_lo(node, prev_rel, prev_side), prev_side)
        || masked_any(node, prev_depth, prev_rel)
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
#[path = "cell_lod_tests.rs"]
mod tests;
