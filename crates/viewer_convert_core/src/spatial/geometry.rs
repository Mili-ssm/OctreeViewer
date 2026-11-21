//! Cell and sample geometry in the global frame.

use viewer_core::SpatialBounds;

#[inline]
pub fn cube_side(bounds: SpatialBounds) -> f64 {
    bounds.max_side()
}

#[cfg(test)]
pub(super) fn child_index(position: [f64; 3], bounds: SpatialBounds, divisions: u8) -> usize {
    viewer_core::SpatialSubdivision::new(bounds, divisions)
        .expect("converter subdivisions are validated")
        .cell_index(position)
}

#[cfg(test)]
pub(super) fn child_bounds(bounds: SpatialBounds, child: usize, divisions: u8) -> SpatialBounds {
    viewer_core::SpatialSubdivision::new(bounds, divisions)
        .expect("converter subdivisions are validated")
        .cell_bounds(child)
        .expect("child index is in range")
}

/// Exponent of a positive power-of-two side read from the `f64` bits (no `log2`).
fn power_of_two_exponent(value: f64) -> i32 {
    ((value.to_bits() >> 52) & 0x7ff) as i32 - 1023
}

/// Absolute level `a` of a `v7.4-pre` node from its cube (side `2^(49 - a)` CRS units).
pub fn global_level(bounds: SpatialBounds) -> u32 {
    (49 - power_of_two_exponent(cube_side(bounds))).clamp(0, 64) as u32
}

/// Side of a `v7.4-pre` node in quanta as a power-of-two exponent (`64 - a`).
fn global_side_exponent(bounds: SpatialBounds) -> u32 {
    64 - global_level(bounds)
}

pub(super) fn quanta(position: [f64; 3]) -> [i64; 3] {
    // Positions of a global conversion are on the grid and inside the coordinate limit.
    // `as` saturates: a level-0 corner -2^48 units is exactly -2^63 quanta.
    position.map(|value| viewer_format::round_to_quanta(value * 32_768.0))
}

/// Per-node constants of [`sample_cell_global`]: computed once instead of per point.
#[derive(Clone, Copy)]
pub struct GlobalCellFrame {
    origin: [i64; 3],
    shift: u32,
    grid: usize,
    /// Every cell center is an `i64`: the cube lies inside the coordinate domain, so the
    /// centers need no `i128` arithmetic or conversion.
    centers_fit: bool,
}

impl GlobalCellFrame {
    pub fn new(bounds: SpatialBounds, grid: usize) -> Self {
        let origin = quanta(bounds.min());
        let side_exponent = global_side_exponent(bounds);
        let centers_fit = origin
            .iter()
            .all(|&origin| (i128::from(origin) + (1_i128 << side_exponent)) <= 1_i128 << 63);
        Self {
            origin,
            shift: side_exponent - grid.trailing_zeros(),
            grid,
            centers_fit,
        }
    }

    #[inline]
    fn center(&self, axis: usize, cell: usize) -> f64 {
        if self.centers_fit {
            let half = if self.shift == 0 {
                0
            } else {
                1_u64 << (self.shift - 1)
            };
            // The true sum is an `i64`, so wrapping arithmetic yields it exactly.
            let quanta = self.origin[axis]
                .wrapping_add(((cell as u64) << self.shift) as i64)
                .wrapping_add(half as i64);
            quanta as f64 * viewer_format::quanta_to_units(1)
        } else {
            global_cell_center(self.origin[axis], cell, self.shift)
        }
    }

    /// Cell index and center of `position`; see [`sample_cell_global`].
    #[inline]
    pub fn cell(&self, position: [f64; 3]) -> (usize, [f64; 3]) {
        let q = quanta(position);
        let grid = self.grid;
        let mut cell = [0_usize; 3];
        let mut center = [0.0_f64; 3];
        for axis in 0..3 {
            // The difference fits `i64` for any point inside a node below the root; wider
            // ones take the `i128` path (same value, only slower).
            cell[axis] = match q[axis].checked_sub(self.origin[axis]) {
                Some(relative) => (relative >> self.shift).clamp(0, grid as i64 - 1) as usize,
                None => ((i128::from(q[axis]) - i128::from(self.origin[axis])) >> self.shift)
                    .clamp(0, grid as i128 - 1) as usize,
            };
            center[axis] = self.center(axis, cell[axis]);
        }
        ((cell[0] * grid + cell[1]) * grid + cell[2], center)
    }
}

/// The cell of `position` is
/// `(q - origin) >> (side_exponent - log2(grid))` and the center is an integer number of quanta.
#[inline]
pub fn sample_cell_global(
    position: [f64; 3],
    bounds: SpatialBounds,
    grid: usize,
) -> (usize, [f64; 3]) {
    GlobalCellFrame::new(bounds, grid).cell(position)
}

#[inline]
fn global_cell_center(origin: i64, cell: usize, shift: u32) -> f64 {
    let half = if shift == 0 { 0 } else { 1_i128 << (shift - 1) };
    let center = i128::from(origin) + ((cell as i128) << shift) + half;
    center as f64 * viewer_format::quanta_to_units(1)
}

#[inline]
pub fn voxel_center_global(index: usize, bounds: SpatialBounds, grid: usize) -> [f64; 3] {
    let shift = global_side_exponent(bounds) - grid.trailing_zeros();
    let origin = quanta(bounds.min());
    let cell = [index / (grid * grid), (index / grid) % grid, index % grid];
    std::array::from_fn(|axis| global_cell_center(origin[axis], cell[axis], shift))
}

/// Half the cell diagonal, or the whole diagonal where samples sit at their nearest point
/// (`v7.4-pre` nodes with `a < 32`).
pub fn lod_sample_error(bounds: SpatialBounds, grid: usize) -> f64 {
    let half_diagonal = (3.0_f64).sqrt() * (cube_side(bounds) / grid as f64) / 2.0;
    if global_level(bounds) < 32 {
        2.0 * half_diagonal
    } else {
        half_diagonal
    }
}

pub fn center_rounding_error(bounds: SpatialBounds, frame: viewer_core::CoordinateFrame) -> f64 {
    (0..3)
        .map(|axis| {
            let min = bounds.min()[axis] - frame.origin()[axis];
            let max = bounds.max()[axis] - frame.origin()[axis];
            let coordinate = if min.abs() > max.abs() { min } else { max };
            let rounded = coordinate as f32;
            let magnitude_bits = rounded.abs().to_bits();
            let magnitude = f64::from(rounded.abs());
            let lower = if magnitude_bits == 0 {
                0.0
            } else {
                magnitude - f64::from(f32::from_bits(magnitude_bits - 1))
            };
            let upper = magnitude_bits
                .checked_add(1)
                .map(f32::from_bits)
                .filter(|next| next.is_finite())
                .map_or(lower, |next| f64::from(next) - magnitude);
            lower.max(upper) / 2.0
        })
        .map(|error| error * error)
        .sum::<f64>()
        .sqrt()
}
