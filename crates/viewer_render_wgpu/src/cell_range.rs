//! Nearest occupied view depth of a node (ADR 0055 plan, task 5): the smallest view depth, inside
//! the view frustum, of the page boxes of one of its levels. A node's page boxes bound where its
//! cells are, so no cell of that level is nearer than this, and the fine end of the node's depth
//! range (`node_view_range_at`) can be evaluated there instead of at the near clip.
//!
//! Frames are those of `node_view_range`: dataset coordinates minus `coordinate_origin`, camera
//! position divided by the visual scale; depths are along the view direction, in those units.

use crate::{
    PageInfo, ViewProjection, camera::VERTICAL_FOV, cell_cut::view_eye, node_cells::CellGrid,
};

/// Half-space `n . p <= c` of the eye-relative frame, `n` a unit vector.
type Plane = ([f64; 3], f64);

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(u: [f64; 3], v: [f64; 3]) -> [f64; 3] {
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}

/// Feasibility tolerance (relative): it only admits points marginally outside a plane, so the
/// depth can only come out lower, the conservative side.
fn tol(p: [f64; 3]) -> f64 {
    1e-9 * (1.0 + p.iter().fold(0.0_f64, |m, v| m.max(v.abs())))
}

/// The view frustum as six half-spaces through the eye (the four sides, the near plane at
/// `near` and the far plane), and the data to place boxes in its frame.
pub(crate) struct ViewFrustum {
    eye: [f64; 3],
    forward: [f64; 3],
    planes: [Plane; 6],
    near: f64,
}

impl ViewFrustum {
    /// `None` for an empty viewport. The near plane is the farther of the render near clip and
    /// the LOD near depth, like `node_view_range`'s nearest depth.
    pub(crate) fn new(view: &ViewProjection) -> Option<Self> {
        let [width, height] = view.viewport();
        if width == 0 || height == 0 {
            return None;
        }
        let camera = view.camera();
        let scale = view.visual_scale();
        let forward = camera.forward();
        let (right, up) = camera.right_up();
        let tan_v = (VERTICAL_FOV * 0.5).tan();
        let tan_h = tan_v * f64::from(width) / f64::from(height);
        let near = camera.lod_near_depth().max(camera.clip_distances().0) / scale;
        let far = camera.clip_distances().1 / scale;
        let mut planes = [([0.0; 3], 0.0); 6];
        planes[0] = (forward.map(|v| -v), -near);
        planes[1] = (forward, far);
        for (slot, (side, tan)) in [(right, tan_h), (up, tan_v)].into_iter().enumerate() {
            for (k, sign) in [1.0, -1.0].into_iter().enumerate() {
                // sign * side . p <= depth * tan
                let n: [f64; 3] = std::array::from_fn(|a| sign * side[a] - tan * forward[a]);
                let length = dot3(n, n).sqrt();
                planes[2 + 2 * slot + k] = (n.map(|v| v / length), 0.0);
            }
        }
        Some(Self {
            eye: view_eye(view),
            forward,
            planes,
            near,
        })
    }

    /// Smallest view depth of the points of the box `[min, max]` (frame of `node_view_range`)
    /// inside the frustum, or `None` when the intersection is empty. A box fully inside costs its
    /// eight corners; one fully outside one plane is dropped; otherwise the minimum of the depth
    /// (linear) over the convex polytope `box ∩ frustum` sits at a vertex, so the planes that
    /// cut the box and the six faces of the box are intersected by triples and the feasible
    /// vertices kept (at most 12 planes, 220 triples; usually 1 or 2 planes cut a box).
    pub(crate) fn min_depth(&self, min: [f64; 3], max: [f64; 3]) -> Option<f64> {
        let (min, max): ([f64; 3], [f64; 3]) = (
            std::array::from_fn(|a| min[a] - self.eye[a]),
            std::array::from_fn(|a| max[a] - self.eye[a]),
        );
        let corners: [[f64; 3]; 8] = std::array::from_fn(|k| {
            std::array::from_fn(|a| if k >> a & 1 == 1 { max[a] } else { min[a] })
        });
        let mut cutting = [([0.0; 3], 0.0); 12];
        let mut count = 0;
        for (n, c) in self.planes {
            let outside = corners
                .iter()
                .filter(|p| dot3(n, **p) > c + tol(**p))
                .count();
            if outside == corners.len() {
                return None;
            }
            if outside > 0 {
                cutting[count] = (n, c);
                count += 1;
            }
        }
        if count == 0 {
            return corners
                .iter()
                .map(|p| dot3(*p, self.forward))
                .reduce(f64::min);
        }
        for a in 0..3 {
            let unit: [f64; 3] = std::array::from_fn(|i| f64::from(u8::from(i == a)));
            cutting[count] = (unit.map(|v| -v), -min[a]);
            cutting[count + 1] = (unit, max[a]);
            count += 2;
        }
        let set = &cutting[..count];
        let feasible = |p: [f64; 3]| set.iter().all(|(n, c)| dot3(*n, p) <= c + tol(p));
        let mut best: Option<f64> = None;
        for i in 0..count {
            for j in i + 1..count {
                for k in j + 1..count {
                    let ((n1, c1), (n2, c2), (n3, c3)) = (set[i], set[j], set[k]);
                    let det = dot3(n1, cross3(n2, n3));
                    if det.abs() < 1e-9 {
                        continue;
                    }
                    let (x, y, z) = (cross3(n2, n3), cross3(n3, n1), cross3(n1, n2));
                    let p: [f64; 3] =
                        std::array::from_fn(|a| (c1 * x[a] + c2 * y[a] + c3 * z[a]) / det);
                    if feasible(p) {
                        let depth = dot3(p, self.forward);
                        best = Some(best.map_or(depth, |b| b.min(depth)));
                    }
                }
            }
        }
        best
    }

    /// Lower bound of `min_depth` of any box: nothing inside the frustum is nearer than the near
    /// plane, and a box is not nearer than its nearest corner.
    fn lower_bound(&self, min: [f64; 3], max: [f64; 3]) -> f64 {
        let nearest: f64 = (0..3)
            .map(|a| {
                let corner = if self.forward[a] > 0.0 {
                    min[a]
                } else {
                    max[a]
                };
                (corner - self.eye[a]) * self.forward[a]
            })
            .sum();
        nearest.max(self.near)
    }
}

/// The smallest view depth, inside the view frustum, of the page boxes `pages` of the level at
/// absolute `depth` (the tight box of a page spans `box_max + 1` cells from its origin), or `None`
/// when no page box meets the frustum. See `nearest_occupied_page`.
pub fn nearest_occupied_depth(
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    depth: u8,
    pages: &[PageInfo],
) -> Option<f64> {
    nearest_occupied_page(view, grid, coordinate_origin, depth, pages).map(|(depth, _)| depth)
}

/// `nearest_occupied_depth` and the index of the first page that gives it. One pass, no
/// allocation; a page whose lower bound is not below the best so far is skipped, so the exact
/// clip runs on few of them.
pub fn nearest_occupied_page(
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    depth: u8,
    pages: &[PageInfo],
) -> Option<(f64, usize)> {
    let frustum = ViewFrustum::new(view)?;
    let side = grid.root_side / 2.0_f64.powi(i32::from(depth));
    let base: [f64; 3] = std::array::from_fn(|a| grid.root_min[a] - coordinate_origin[a]);
    let mut best: Option<(f64, usize)> = None;
    for (index, page) in pages.iter().enumerate() {
        // A page of another depth has other cell sides: its box is not placed by this `side`.
        if page.depth != depth {
            continue;
        }
        let min: [f64; 3] = std::array::from_fn(|a| base[a] + page.origin[a] as f64 * side);
        let max: [f64; 3] =
            std::array::from_fn(|a| min[a] + (f64::from(page.box_max[a]) + 1.0) * side);
        if best.is_some_and(|(b, _)| frustum.lower_bound(min, max) >= b) {
            continue;
        }
        if let Some(depth) = frustum.min_depth(min, max)
            && best.is_none_or(|(b, _)| depth < b)
        {
            best = Some((depth, index));
        }
    }
    best
}
