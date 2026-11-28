//! R3 Task 4 native coverage spike: what the per-node cell path would draw versus the CPU voxel
//! path, for real v7.5 datasets and without a GPU. Empty pixels need the Task 7 renderer and are
//! out of scope. Throwaway: not part of `scripts/check.sh`.
//!
//! Usage: `coverage_spike` INPUT.oct [`MAX_CUT_NODES=0`] [--export DIR [--check-cell-pages]]
//!        [--export-pool DIR] [--cell-lod]
//! (0 = no limit; larger cuts skip the case).
//!
//! `--cell-lod` (plan `nivel-pedido-y-lod-por-celda`, phase 0a) adds the `cell_lod` measurement
//! next to `octant_tight`: every cell of a cut node draws the depth its own distance asks for
//! (rule D3), at any depth, with the levels the stored chain lacks derived from the node's
//! points. It also runs a synthetic 10-pose zoom at 4 px. See the `cell_lod` section at the end.
//!
//! `--export DIR` writes, for the `octant_tight` variant only and for the poses/targets in
//! `EXPORT_POSES`/`EXPORT_TARGETS` plus the 4 px `DIRECTION_POSES`, the cube-pool files of
//! `examples/web-host/probes/spike-cube-pool-format.md`, and skips every other case and the CPU baseline.
//! `--check-cell-pages` (with `--export`) packs every exported W16 level with the library's
//! `pack_level` and compares it with the spike's own packing (pages, slots, words 0, 2, 3 and z),
//! and measures the eye-frame pixel error of the decoded cells; it exits non-zero on a mismatch.
//!
//! `--export-pool DIR` (R3 Task 7c, read by `cell_pool_bench`) selects the same cases as `--export`
//! and writes, per case, `DIR/<dataset>-<pose>-<target>px/{pool.json,levels.bin}`: everything
//! `CellPool::frame` needs with a fake loader. Every `f64` of `pool.json` is stored as its IEEE
//! bit pattern (`u64`), because `serde_json` without `float_roundtrip` can lose a ulp.
//! `pool.json`: `format` ("r3-pool-v1"), `dataset`, `pose`, `target_px_bits`, `viewport` [w, h],
//! `visual_scale_bits`, `coordinate_origin_bits` (zero: the grid is already local), `grid`
//! {`root_min_bits`[3], `root_side_bits`}, `camera_bits` (`OrbitCamera::to_f64_array`), `cut`
//! (node ids in the order handed to `CellPool::frame`) and `nodes`, one per cut node in that
//! order: {`id`, `bounds_min_bits`[3], `bounds_max_bits`[3], `points`, `levels`: [{`depth`,
//! `pages`, `offset`}]}; the levels of a node are all its stored levels, ascending.
//! `levels.bin`, little endian, at `offset` of each level: `pages * 512` instances of 4 `u32`
//! (`CellInstance`), then `pages` page records of 48 bytes: origin 3 x `u64`, `first_cell` `u64`,
//! `cells` `u32`, `flags` `u32`, `box_max` 3 x `u16`, 2 bytes of padding. Both come from
//! `pack_level`, so the bench offers the very pages the viewer would upload.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    error::Error,
    fs::{self, File},
    io::{BufWriter, Write},
    ops::Range,
    path::{Path, PathBuf},
    time::Instant,
};

use viewer_core::{
    BlockId, DEFAULT_LOD_PRIORITY_BYTES, DatasetSource, LodSelectionConfig, NodeId, PointBlock,
    ResidentBlockSource, SpatialBounds, SpatialFrustum,
};
use viewer_format::{FormatReader, PagedReader};
use viewer_render_wgpu::{
    CellCutEntry, CellCutNode, CellGrid, InstanceRaw, MAX_CELL_DEPTH, NodeCell, NodeCellLevel,
    NodeCellSource, OrbitCamera, PAGE_SLOTS, PackedLevel, PageEntry, PageFrame,
    PointNodeCellSource, RendererConfig, SceneBounds, ViewProjection, VoxelBuildLimits,
    build_voxel_scene_with_limits, cell_cut, cell_lod, decode_cell, octant_ranges, pack_level,
    representative_depth, srgb_to_linear_table,
};

const VIEWPORT: [u32; 2] = [1280, 720];
const TARGETS: [f64; 4] = [1.0, 2.0, 4.0, 8.0];
/// Budget `LodDataset::resolve_target` passes to `LodSelectionConfig::new`.
const VIEWER_BUDGET: u64 = u64::MAX;
const VIEWER_BUDGET_SOURCE: &str = "crates/viewer_wasm/src/lod_dataset.rs:517";
const POOL_BYTES: u64 = 256 * 1024 * 1024;
/// Decoded-point estimate above which the CPU baseline of a case is skipped (reported as NA).
const MAX_DECODED_BYTES: u64 = 6 * 1024 * 1024 * 1024;
const POINT_BYTES: u64 = 16;
const MAX_MASK_LEVELS: u8 = 3;
const EXPORT_POSES: [&str; 3] = ["orbit-0", "grazing", "inside-leaf"];
const EXPORT_TARGETS: [f64; 2] = [1.0, 4.0];
/// Extra inside-leaf directions (same eye as `inside-leaf`), built and exported only with `--export`, at
/// `DIRECTION_POSE_TARGET` only: pose name, unit direction. For +-z, `set_pose` clamps the pitch to
/// +-89 degrees and recomputes the eye from the orbit, so those two eyes move by ~1.7 % of the
/// target distance and the view is tilted 1 degree.
const DIRECTION_POSES: [(&str, [f64; 3]); 5] = [
    ("inside-leaf-nx", [-1.0, 0.0, 0.0]),
    ("inside-leaf-py", [0.0, 1.0, 0.0]),
    ("inside-leaf-ny", [0.0, -1.0, 0.0]),
    ("inside-leaf-pz", [0.0, 0.0, 1.0]),
    ("inside-leaf-nz", [0.0, 0.0, -1.0]),
];
const DIRECTION_POSE_TARGET: f64 = 4.0;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Per-cell attributes kept only for `--export`.
struct CellAttr {
    rgba: u32,
    /// Mean intensity as the CPU path stores it (`-1` without the attribute).
    intensity: f32,
    class: i16,
}

struct Level {
    depth: u8,
    /// First cell of the node at this depth (same rounding as the cell builder).
    lo: [u64; 3],
    /// Cell coordinate minus `lo`.
    rel: Vec<[u32; 3]>,
    /// Empty unless exporting.
    attrs: Vec<CellAttr>,
    /// Octant starts of the library level; `None` for a level spanning octants.
    octant_offsets: Option<[u32; 9]>,
    /// The library packing, only with `--check-cell-pages`.
    packed: Option<PackedLevel>,
}

struct NodeData {
    bounds: SpatialBounds,
    is_leaf: bool,
    rep: u8,
    rep_lo: [u64; 3],
    levels: Vec<Level>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "`Only256` is not in `RUN_VARIANTS` by default")]
enum Variant {
    Pyramid,
    Only256,
    /// Level test on the actual projected edge of a cube at the node's nearest point.
    Tight,
    /// Per child octant, conservative test (approximates Task 15).
    Octant,
    OctantTight,
    /// `OctantTight`, but the conservative rule when the eye is inside the tested box.
    OctantTightGuard,
    /// `OctantTightGuard` with the tight edge scaled by the off-axis factor of `projected_error_pixels`.
    OctantTightOffaxis,
    /// `OctantTightOffaxis`, with the factor only when the eye is closer to the box than its side.
    OctantTightNear,
}

/// Variants measured by default; `Only256` stays available.
const RUN_VARIANTS: [Variant; 7] = [
    Variant::Pyramid,
    Variant::Tight,
    Variant::Octant,
    Variant::OctantTight,
    Variant::OctantTightGuard,
    Variant::OctantTightOffaxis,
    Variant::OctantTightNear,
];
/// `DescendantTooFine` depends on the cut structure, not on the level choice.
const TRANSITION_VARIANTS: [Variant; 1] = [Variant::Pyramid];

impl Variant {
    fn name(self) -> &'static str {
        match self {
            Self::Pyramid => "pyramid",
            Self::Only256 => "only256",
            Self::Tight => "tight",
            Self::Octant => "octant",
            Self::OctantTight => "octant_tight",
            Self::OctantTightGuard => "octant_tight_guard",
            Self::OctantTightOffaxis => "octant_tight_offaxis",
            Self::OctantTightNear => "octant_tight_near",
        }
    }
    fn tight(self) -> bool {
        matches!(
            self,
            Self::Tight
                | Self::OctantTight
                | Self::OctantTightGuard
                | Self::OctantTightOffaxis
                | Self::OctantTightNear
        )
    }
    fn octants(self) -> bool {
        matches!(
            self,
            Self::Octant
                | Self::OctantTight
                | Self::OctantTightGuard
                | Self::OctantTightOffaxis
                | Self::OctantTightNear
        )
    }
    /// Indices into `levels` kept as stored.
    fn kept(self, node: &NodeData) -> Vec<usize> {
        match self {
            Self::Only256 if !node.is_leaf => vec![node.levels.len() - 1],
            _ => (0..node.levels.len()).collect(),
        }
    }
}

/// First stored depth passing `ok`, else the finest.
fn pick_depth(stored: &[u8], ok: impl Fn(u8) -> bool) -> u8 {
    stored
        .iter()
        .copied()
        .find(|d| ok(*d))
        .unwrap_or_else(|| *stored.last().expect("stored levels are not empty"))
}

/// Does a level of `depth` meet `target` over `bounds` (local, unscaled frame)?
#[expect(clippy::too_many_arguments, reason = "throwaway spike")]
#[expect(
    clippy::fn_params_excessive_bools,
    reason = "throwaway spike; the flags are independent switches of one experiment"
)]
fn level_ok(
    view: &ViewProjection,
    root_side: f64,
    bounds: SpatialBounds,
    depth: u8,
    target: f64,
    tight: bool,
    guard: bool,
    offaxis: bool,
    near_only: bool,
) -> bool {
    let side = root_side / (1_u64 << depth) as f64;
    let eye = view.camera().position().map(|v| v / view.visual_scale());
    let eye_inside = (0..3).all(|a| (bounds.min()[a]..=bounds.max()[a]).contains(&eye[a]));
    if !tight || (guard && eye_inside) {
        return view.projected_error_pixels(bounds, side, [0.0; 3]) <= target;
    }
    let min: [f64; 3] = std::array::from_fn(|a| {
        let lo = bounds.min()[a];
        let hi = (bounds.max()[a] - side).max(lo);
        eye[a].clamp(bounds.min()[a], bounds.max()[a]).clamp(lo, hi)
    });
    // Same factor as `OrbitCamera::projected_error_pixels` (camera.rs, VERTICAL_FOV = 45 deg).
    let box_side = (0..3)
        .map(|a| bounds.max()[a] - bounds.min()[a])
        .fold(0.0, f64::max);
    let distance = (0..3)
        .map(|a| {
            let d = (bounds.min()[a] - eye[a])
                .max(eye[a] - bounds.max()[a])
                .max(0.0);
            d * d
        })
        .sum::<f64>()
        .sqrt();
    let factor = if offaxis && (!near_only || distance < box_side) {
        let viewport = view.viewport();
        let tan_v = (45.0_f64.to_radians() * 0.5).tan();
        let tan_h = tan_v * f64::from(viewport[0]) / f64::from(viewport[1]);
        (1.0 + tan_v * tan_v + tan_h * tan_h).sqrt()
    } else {
        1.0
    };
    SpatialBounds::new(min, min.map(|v| v + side))
        .is_ok_and(|cube| view.projected_voxel_edge_pixels(cube, [0.0; 3]) * factor <= target)
}

fn octant_bounds(bounds: SpatialBounds, octant: usize) -> Option<SpatialBounds> {
    let mid: [f64; 3] = std::array::from_fn(|a| f64::midpoint(bounds.min()[a], bounds.max()[a]));
    let min = std::array::from_fn(|a| {
        if octant >> a & 1 == 0 {
            bounds.min()[a]
        } else {
            mid[a]
        }
    });
    let max = std::array::from_fn(|a| {
        if octant >> a & 1 == 0 {
            mid[a]
        } else {
            bounds.max()[a]
        }
    });
    SpatialBounds::new(min, max).ok()
}

struct Resident(HashMap<BlockId, PointBlock>);

impl ResidentBlockSource for Resident {
    fn peek(&self, id: BlockId) -> Option<&PointBlock> {
        self.0.get(&id)
    }
}

/// Same rounding as the crate-private `node_range`.
fn node_lo(grid: &CellGrid, bounds: SpatialBounds, depth: u8) -> [u64; 3] {
    let count = 1_u64 << depth;
    let cell_side = grid.root_side / count as f64;
    std::array::from_fn(|axis| {
        let edge = ((bounds.min()[axis] - grid.root_min[axis]) / cell_side).round();
        if edge <= 0.0 {
            0
        } else if edge >= (count - 1) as f64 {
            count - 1
        } else {
            edge as u64
        }
    })
}

struct Pose {
    name: &'static str,
    camera: OrbitCamera,
}

struct Case {
    pose: usize,
    target: f64,
    budget: u64,
    cut: Vec<NodeId>,
}

/// Cell ranges of one level for the octants in `octants`, through the library's `octant_ranges`.
fn cell_ranges(level: &Level, octants: u8) -> Vec<Range<usize>> {
    match level.octant_offsets {
        // A spanning level is drawn once, whole.
        None => {
            if octants != 0 {
                #[allow(
                    clippy::single_range_in_vec_init,
                    reason = "one range, not an expanded list"
                )]
                let whole = vec![0..level.rel.len()];
                whole
            } else {
                Vec::new()
            }
        }
        Some(offsets) => octant_ranges(
            &NodeCellLevel {
                depth: level.depth,
                cells: Vec::new(),
                octant_offsets: Some(offsets),
            },
            octants,
        ),
    }
}

fn level_draw_ranges(level: &Level, octants: u8) -> usize {
    cell_ranges(level, octants).len()
}

fn fmt3(v: [f64; 3]) -> String {
    format!("({:.4},{:.4},{:.4})", v[0], v[1], v[2])
}

/// Distinct (ancestor, descendant) pairs more than `MAX_MASK_LEVELS` apart, found the way
/// `cell_cut` looks for ancestors.
fn too_fine_pairs(cut: &[NodeId], cache: &HashMap<NodeId, NodeData>) -> usize {
    let mut by_cell = HashMap::with_capacity(cut.len());
    for id in cut {
        let n = &cache[id];
        by_cell.entry((n.rep, n.rep_lo)).or_insert(*id);
    }
    let mut pairs = HashSet::new();
    for id in cut {
        let d = &cache[id];
        for k in (MAX_MASK_LEVELS + 1)..=d.rep.saturating_sub(3) {
            let ancestor_lo = d.rep_lo.map(|c| (c >> k) & !7);
            if let Some(parent) = by_cell.get(&(d.rep - k, ancestor_lo)) {
                pairs.insert((*parent, *id));
            }
        }
    }
    pairs.len()
}

fn main() -> Result<()> {
    let mut args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut export_dir: Option<PathBuf> = None;
    if let Some(at) = args.iter().position(|a| a == "--export") {
        if at + 1 >= args.len() {
            return Err("--export needs a directory".into());
        }
        export_dir = Some(PathBuf::from(args.remove(at + 1)));
        args.remove(at);
    }
    let mut export_pool_dir: Option<PathBuf> = None;
    if let Some(at) = args.iter().position(|a| a == "--export-pool") {
        if at + 1 >= args.len() {
            return Err("--export-pool needs a directory".into());
        }
        export_pool_dir = Some(PathBuf::from(args.remove(at + 1)));
        args.remove(at);
    }
    let cell_lod_on = args
        .iter()
        .position(|a| a == "--cell-lod")
        .map(|at| args.remove(at))
        .is_some();
    // Residency of the primary `cell_lod` columns: r1 (range, default), r2 (drawn depths plus the
    // next one below the separation depth), r3 (r2 plus the next finer and coarser of every drawn
    // depth); r2 and r3 bytes are always printed. `--centre-distance` restores the centre distance
    // of the first run (default: nearest point of the cell).
    let mut lod_opts = LodOpts {
        residency: 1,
        nearest: true,
        metric: Metric::Distance,
        px_only: false,
    };
    // `--metric bound|edge|distance` picks the refine rule (default: the phase 0a one);
    // `--px-only` skips the spacing mode; `--poses a,b` limits the poses (no synthetic zoom).
    if let Some(at) = args.iter().position(|a| a == "--metric") {
        if at + 1 >= args.len() {
            return Err("--metric needs bound, edge or distance".into());
        }
        lod_opts.metric = match args.remove(at + 1).to_string_lossy().as_ref() {
            "bound" => Metric::Bound,
            "edge" => Metric::Edge,
            "distance" => Metric::Distance,
            other => return Err(format!("unknown metric {other}").into()),
        };
        args.remove(at);
    }
    if let Some(at) = args.iter().position(|a| a == "--px-only") {
        args.remove(at);
        lod_opts.px_only = true;
    }
    let mut pose_filter: Option<Vec<String>> = None;
    if let Some(at) = args.iter().position(|a| a == "--poses") {
        if at + 1 >= args.len() {
            return Err("--poses needs a comma separated list".into());
        }
        pose_filter = Some(
            args.remove(at + 1)
                .to_string_lossy()
                .split(',')
                .map(str::to_string)
                .collect(),
        );
        args.remove(at);
    }
    if let Some(at) = args.iter().position(|a| a == "--residency") {
        if at + 1 >= args.len() {
            return Err("--residency needs r1, r2 or r3".into());
        }
        lod_opts.residency = match args.remove(at + 1).to_string_lossy().as_ref() {
            "r1" => 1,
            "r2" => 2,
            "r3" => 3,
            "r3c" => 4,
            other => return Err(format!("unknown residency {other}").into()),
        };
        args.remove(at);
    }
    if let Some(at) = args.iter().position(|a| a == "--centre-distance") {
        args.remove(at);
        lod_opts.nearest = false;
    }
    let exporting = export_dir.is_some() || export_pool_dir.is_some();
    let check_cell_pages = args
        .iter()
        .position(|a| a == "--check-cell-pages")
        .map(|at| args.remove(at))
        .is_some();
    if check_cell_pages && export_dir.is_none() {
        return Err("--check-cell-pages needs --export".into());
    }
    if args.is_empty() || args.len() > 2 {
        return Err(
            "usage: coverage_spike INPUT.oct [MAX_CUT_NODES=0] [--export DIR [--check-cell-pages]] [--export-pool DIR] [--cell-lod [--residency r1|r2|r3|r3c] [--centre-distance] [--metric bound|edge|distance] [--px-only] [--poses a,b]]"
                .into(),
        );
    }
    let dataset_name = Path::new(&args[0])
        .file_stem()
        .map_or_default(|stem| stem.to_string_lossy().into_owned());
    let dataset_name = dataset_name
        .split('-')
        .next()
        .unwrap_or_default()
        .to_string();
    let max_cut_nodes: usize = args
        .get(1)
        .map(|v| v.to_string_lossy().parse())
        .transpose()?
        .unwrap_or(0);
    let mut reader = FormatReader::new(File::open(&args[0])?)?;
    let mut attrs_path = args[0].clone();
    attrs_path.push("attrs");
    if !reader.has_attributes() && Path::new(&attrs_path).exists() {
        reader.attach_attributes(File::open(attrs_path)?)?;
    }
    let mut leaf_flags = HashMap::new();
    {
        let mut paged = PagedReader::open(File::open(&args[0])?)?;
        let mut pages = vec![paged.root_page().clone()];
        let mut cursor = 0;
        while cursor < pages.len() {
            let page = pages[cursor].clone();
            for node in page.nodes() {
                leaf_flags.insert(node.id(), !node.has_children());
            }
            for (anchor, pointer) in page.child_pages() {
                pages.push(paged.read_child_page(anchor, pointer)?);
            }
            cursor += 1;
        }
    }
    let viewer_dataset = reader.format_metadata().viewer_dataset()?;
    let metadata = &*viewer_dataset;
    let hierarchy = metadata.hierarchy().clone();
    let origin = metadata.coordinate_frame().origin();
    let root = hierarchy.stats().root_bounds;
    let root_side = (0..3)
        .map(|a| root.max()[a] - root.min()[a])
        .fold(0.0, f64::max);
    let grid = CellGrid {
        root_min: std::array::from_fn(|a| root.min()[a] - origin[a]),
        root_side,
    };
    let is_leaf_of = |id: NodeId| -> Result<bool> {
        if leaf_flags.is_empty() {
            Ok(hierarchy.is_leaf(id))
        } else {
            leaf_flags
                .get(&id)
                .copied()
                .ok_or_else(|| "node missing authoritative has_children flag".into())
        }
    };
    let to_local = |b: SpatialBounds| -> Result<SpatialBounds> {
        Ok(SpatialBounds::new(
            std::array::from_fn(|a| b.min()[a] - origin[a]),
            std::array::from_fn(|a| b.max()[a] - origin[a]),
        )?)
    };

    // Poses.
    // The viewer builds its view with the dataset's visual scale (bindings/lod.rs:240) and its
    // renderer's effective voxel limits (`SurfaceRenderer::voxel_build_limits`, gpu.rs:721).
    let visual_scale = reader.format_metadata().visual_scale();
    let defaults = RendererConfig::default();
    let build_limits = VoxelBuildLimits {
        max_voxels: defaults
            .voxel_build_limits
            .max_voxels
            .min(defaults.gpu_budget.max_visible_instances),
        ..defaults.voxel_build_limits
    };
    let root_local = SceneBounds {
        min: grid.root_min.map(|v| v * visual_scale),
        max: std::array::from_fn(|a| (root.max()[a] - origin[a]) * visual_scale),
    };
    let centre: [f64; 3] =
        std::array::from_fn(|a| f64::midpoint(root_local.min[a], root_local.max[a]));
    let mut fitted = OrbitCamera::default();
    if !fitted.fit(root_local, VIEWPORT) {
        return Err("cannot fit camera to dataset".into());
    }
    let mut orbit90 = fitted;
    // `orbit` takes normalized deltas (1.0 = full turn): 0.25 is a quarter turn.
    orbit90.orbit(0.25, 0.0);
    let fit_pos = fitted.position();
    let fit_target = fitted.target();
    let offset: [f64; 3] = std::array::from_fn(|a| fit_pos[a] - fit_target[a]);
    let distance = offset.iter().map(|v| v * v).sum::<f64>().sqrt();
    let horizontal = offset[0].hypot(offset[1]).max(1e-12);
    let elevation = 5.0_f64.to_radians();
    let mut grazing = fitted;
    let graze_eye = [
        centre[0] + distance * elevation.cos() * offset[0] / horizontal,
        centre[1] + distance * elevation.cos() * offset[1] / horizontal,
        centre[2] + distance * elevation.sin(),
    ];
    if !grazing.set_pose(graze_eye, centre) {
        return Err("grazing pose rejected".into());
    }
    let all_nodes: Vec<_> = hierarchy.query(root).collect();
    let (leaf_node, leaf_points) = all_nodes
        .iter()
        .filter(|n| is_leaf_of(n.id()).unwrap_or(false))
        .map(|n| (*n, n.point_count()))
        .max_by_key(|(_, points)| *points)
        .ok_or("dataset has no leaf")?;
    let leaf_local = to_local(leaf_node.bounds())?;
    let leaf_eye: [f64; 3] = std::array::from_fn(|a| {
        f64::midpoint(leaf_local.min()[a], leaf_local.max()[a]) * visual_scale
    });
    let leaf_target = [
        leaf_eye[0] + root_side * visual_scale / 16.0,
        leaf_eye[1],
        leaf_eye[2],
    ];
    let mut inside = fitted;
    if !inside.set_pose(leaf_eye, leaf_target) {
        return Err("inside-leaf pose rejected".into());
    }
    drop(all_nodes);
    let mut poses = vec![
        Pose {
            name: "orbit-0",
            camera: fitted,
        },
        Pose {
            name: "orbit-90",
            camera: orbit90,
        },
        Pose {
            name: "grazing",
            camera: grazing,
        },
        Pose {
            name: "inside-leaf",
            camera: inside,
        },
    ];
    // Transitions and their expected count only cover these; the direction poses are export-only.
    let base_pose_count = poses.len();
    if exporting {
        let d = root_side * visual_scale / 16.0;
        for (name, dir) in DIRECTION_POSES {
            let target: [f64; 3] = std::array::from_fn(|a| leaf_eye[a] + d * dir[a]);
            let mut camera = fitted;
            if !camera.set_pose(leaf_eye, target) {
                return Err(format!("{name} pose rejected").into());
            }
            poses.push(Pose { name, camera });
        }
    }
    // Synthetic zoom (`--cell-lod`): the eye covers half of the remaining way to the inside-leaf eye
    // each step, so the 10th pose is the inside-leaf pose itself; the target moves with the same
    // fraction. Zoom poses only run at 4 px, px-only, for `octant_tight` and `cell_lod`.
    let zoom_start = poses.len();
    if cell_lod_on && pose_filter.is_none() {
        for step in 0..ZOOM_STEPS {
            let t = if step + 1 == ZOOM_STEPS {
                1.0
            } else {
                1.0 - 0.5_f64.powi(step as i32)
            };
            let camera = if step == 0 {
                fitted
            } else if step + 1 == ZOOM_STEPS {
                inside
            } else {
                let eye: [f64; 3] =
                    std::array::from_fn(|a| fit_pos[a] + t * (leaf_eye[a] - fit_pos[a]));
                let target: [f64; 3] =
                    std::array::from_fn(|a| fit_target[a] + t * (leaf_target[a] - fit_target[a]));
                let mut camera = fitted;
                if !camera.set_pose(eye, target) {
                    return Err(format!("zoom pose {step} rejected").into());
                }
                camera
            };
            let name: &'static str = Box::leak(format!("zoom-{step}").into_boxed_str());
            poses.push(Pose { name, camera });
        }
    }
    let reference_spacing = hierarchy.reference_spacing();
    if cell_lod_on {
        println!(
            "# cell_lod: reference spacing {reference_spacing:?} (viewer_core::reference_spacing over the leaves, dataset units); focal {:.4} px; near/eye per pose in the case table; metric {:?}",
            lod_focal(),
            lod_opts.metric
        );
    }
    let mut lod_rows: Vec<String> = Vec::new();
    let mut lod_records: Vec<LodRecord> = Vec::new();
    let mut zoom_rows: Vec<String> = Vec::new();
    let mut zoom_prev: Option<HashSet<(u64, u8)>> = None;
    let mut fine: HashMap<NodeId, FineNode> = HashMap::new();
    let mut budgets = vec![VIEWER_BUDGET];
    if VIEWER_BUDGET != u64::MAX {
        budgets.push(u64::MAX);
    }

    println!("# coverage_spike {}", args[0].to_string_lossy());
    println!(
        "# viewport {}x{}; root_side {root_side}; origin {}",
        VIEWPORT[0],
        VIEWPORT[1],
        fmt3(origin)
    );
    println!(
        "# point budget (viewer) {VIEWER_BUDGET} from {VIEWER_BUDGET_SOURCE}; budgets run: {budgets:?}"
    );
    println!(
        "# selector: select_lod_stable_projector as lod_dataset.rs:517-522; visual_scale {visual_scale} (dataset metadata, bindings/lod.rs:240); voxel limits max_voxels {} (min of voxel_grid.rs:64 and budget.rs:19-22, via gpu.rs:721), max_estimated_bytes {} (voxel_grid.rs:65); RendererConfig defaults, the web host may override",
        build_limits.max_voxels, build_limits.max_estimated_bytes
    );
    for pose in &poses {
        println!(
            "# pose {}: position {} target {} (local coords x visual_scale)",
            pose.name,
            fmt3(pose.camera.position()),
            fmt3(pose.camera.target())
        );
    }
    println!(
        "# inside-leaf: node {} with {leaf_points} points, bounds {:?}",
        leaf_node.id().get(),
        leaf_local
    );
    println!(
        "# grazing: eye {elevation:.4} rad above the horizontal plane through the root centre, looking down, fitted distance {distance:.4}"
    );
    println!("# orbit-90: OrbitCamera::orbit(0.25, 0.0) (quarter turn in normalized units)");

    let mut cache: HashMap<NodeId, NodeData> = HashMap::new();
    let mut check_mismatches = 0_u64;
    let mut check_groups = 0_u64;
    let mut cache_bytes = 0_usize;
    let mut coarse_first_level_nodes = 0_usize;
    let mut coarse_nodes: Vec<String> = Vec::new();
    let mut cases: Vec<Case> = Vec::new();
    let mut rows = Vec::new();
    let mut stop_resident: Vec<Vec<String>> = vec![Vec::new(); RUN_VARIANTS.len()];
    let mut stop_bytes: Vec<Vec<String>> = vec![Vec::new(); RUN_VARIANTS.len()];
    let mut stop_grazing: Vec<Vec<String>> = vec![Vec::new(); RUN_VARIANTS.len()];
    let mut na_grazing: Vec<Vec<String>> = vec![Vec::new(); RUN_VARIANTS.len()];
    let mut skipped: Vec<String> = Vec::new();
    // Library vs the spike's own `level_ok` choice, per (node, octant), for `octant_tight`.
    let (mut octant_compared, mut octant_masked, mut octant_mismatches) = (0_u64, 0_u64, 0_u64);
    let mut octant_examples: Vec<String> = Vec::new();
    // (draw_ranges, cut_nodes, case) of `octant_tight` at 4 px, per case.
    let mut ranges_4px: Vec<(usize, usize, String)> = Vec::new();
    let source = PointNodeCellSource::default();
    let started_all = Instant::now();

    for (pose_index, pose) in poses.iter().enumerate() {
        if pose_filter
            .as_ref()
            .is_some_and(|f| !f.iter().any(|n| n == pose.name))
        {
            continue;
        }
        let is_zoom = pose_index >= zoom_start;
        let view = ViewProjection::new(pose.camera, VIEWPORT, visual_scale, build_limits);
        // Dataset-unit frustum for the selector; unscaled-local one for cells and bounds.
        let wf = view.spatial_frustum(origin).ok_or("no frustum")?;
        let local_frustum = view.spatial_frustum([0.0; 3]).ok_or("no frustum")?;
        for &target in &TARGETS {
            if pose_index >= base_pose_count && target != DIRECTION_POSE_TARGET {
                continue;
            }
            for &budget in &budgets {
                let selection = hierarchy
                    .select_lod_stable_projector(
                        &wf,
                        LodSelectionConfig::new(target, budget)?,
                        DEFAULT_LOD_PRIORITY_BYTES,
                        |node| {
                            view.projected_error_pixels(
                                node.bounds(),
                                node.representative_spacing(),
                                origin,
                            )
                        },
                    )
                    .map(|(selection, _stats)| selection)?;
                let cut: Vec<NodeId> = selection.node_ids().to_vec();
                eprintln!(
                    "[{:>6.1}s] {} {target}px budget {budget}: {} nodes, {} points",
                    started_all.elapsed().as_secs_f64(),
                    pose.name,
                    cut.len(),
                    selection.point_count()
                );
                if exporting
                    && !(EXPORT_POSES.contains(&pose.name) && EXPORT_TARGETS.contains(&target))
                    && pose_index < base_pose_count
                {
                    continue;
                }
                if max_cut_nodes != 0 && cut.len() > max_cut_nodes {
                    eprintln!("  skipped: cut larger than MAX_CUT_NODES={max_cut_nodes}");
                    skipped.push(format!("{} {target}px budget {budget}", pose.name));
                    continue;
                }
                // Cells, cached by node.
                for id in &cut {
                    if cache.contains_key(id) {
                        continue;
                    }
                    let node = hierarchy.node(*id).ok_or("cut node missing")?;
                    let local = to_local(node.bounds())?;
                    let is_leaf = is_leaf_of(*id)?;
                    let blocks: Vec<PointBlock> = node
                        .blocks()
                        .iter()
                        .map(|b| reader.read_block(*b))
                        .collect::<std::result::Result<_, _>>()?;
                    let cells =
                        source.node_cells(&grid, *id, local, node.spacing(), is_leaf, &blocks)?;
                    drop(blocks);
                    let mut levels = Vec::new();
                    for level in &cells.levels {
                        let lo = node_lo(&grid, local, level.depth);
                        let rel = level
                            .cells
                            .iter()
                            .map(|cell| {
                                let mut out = [0_u32; 3];
                                for a in 0..3 {
                                    out[a] =
                                        u32::try_from(cell.coord[a] - lo[a]).map_err(|_| {
                                            format!(
                                                "node {} relative cell coordinate exceeds u32",
                                                id.get()
                                            )
                                        })?;
                                }
                                Ok(out)
                            })
                            .collect::<Result<Vec<_>>>()?;
                        cache_bytes += rel.capacity() * 12 + std::mem::size_of::<Level>();
                        let attrs = if export_dir.is_some() {
                            level.cells.iter().map(cell_attr).collect()
                        } else {
                            Vec::new()
                        };
                        levels.push(Level {
                            depth: level.depth,
                            lo,
                            rel,
                            attrs,
                            octant_offsets: level.octant_offsets,
                            packed: (check_cell_pages || export_pool_dir.is_some())
                                .then(|| pack_level(&grid, local, *id, level)),
                        });
                    }
                    if levels.is_empty() || levels.windows(2).any(|w| w[0].depth >= w[1].depth) {
                        return Err(format!("node {} has no or unsorted levels", id.get()).into());
                    }
                    let rep = representative_depth(&grid, local);
                    // Leaves may start coarser than their 8^3 grid (spacing depth); that only matters
                    // when a mask applies, which needs cut descendants (checked where it is used).
                    if levels[0].depth < rep {
                        coarse_first_level_nodes += 1;
                        coarse_nodes.push(format!(
                            "node {} node_depth {} first_stored_depth {} leaf {is_leaf}",
                            id.get(),
                            rep.saturating_sub(3),
                            levels[0].depth
                        ));
                    }
                    cache.insert(
                        *id,
                        NodeData {
                            bounds: local,
                            is_leaf,
                            rep,
                            rep_lo: node_lo(&grid, local, rep),
                            levels,
                        },
                    );
                }

                if cell_lod_on {
                    for id in &cut {
                        if fine.contains_key(id) {
                            continue;
                        }
                        let n = &cache[id];
                        let blocks: Vec<PointBlock> = hierarchy
                            .node(*id)
                            .ok_or("cut node missing")?
                            .blocks()
                            .iter()
                            .map(|b| reader.read_block(*b))
                            .collect::<std::result::Result<_, _>>()?;
                        let finest = n.levels.last().map_or(0, |l| l.depth);
                        match build_fine(&grid, *id, n.bounds, finest, &blocks) {
                            Ok(node) => {
                                fine.insert(*id, node);
                            }
                            Err(e) => eprintln!("  cell_lod: node {} not buildable: {e}", id.get()),
                        }
                    }
                }

                // CPU baseline.
                let decoded_estimate = selection.point_count() * POINT_BYTES;
                let cpu = if exporting {
                    Err("NA(export)".to_string())
                } else if is_zoom {
                    Err("NA(zoom)".to_string())
                } else if decoded_estimate > MAX_DECODED_BYTES {
                    Err(format!("NA(decoded>{}GiB)", MAX_DECODED_BYTES >> 30))
                } else {
                    let mut blocks = HashMap::new();
                    for id in &cut {
                        for b in hierarchy.node(*id).ok_or("cut node missing")?.blocks() {
                            if !blocks.contains_key(b) {
                                blocks.insert(
                                    *b,
                                    PointBlock::new(*b, reader.read_geometry(*b)?, Vec::new()),
                                );
                            }
                        }
                    }
                    let resident = Resident(blocks);
                    let project = |bounds: SpatialBounds| {
                        if local_frustum.intersects(&bounds) {
                            view.projected_voxel_edge_pixels(bounds, [0.0; 3])
                        } else {
                            0.0
                        }
                    };
                    build_voxel_scene_with_limits(
                        &hierarchy,
                        &cut,
                        &resident,
                        origin,
                        target,
                        &project,
                        view.voxel_build_limits(),
                    )
                    .map(|scene| (scene.scene.instance_count(), scene.oversized_voxels))
                    .map_err(|e| format!("NA({e:?})"))
                };

                for (vi, variant) in RUN_VARIANTS.iter().copied().enumerate() {
                    if (exporting || is_zoom) && variant != Variant::OctantTight {
                        continue;
                    }
                    let t0 = Instant::now();
                    let node_info = |id: NodeId| {
                        cache.get(&id).map(|n| {
                            let stored: Vec<u8> = if variant == Variant::Tight {
                                let all: Vec<u8> = n.levels.iter().map(|l| l.depth).collect();
                                vec![pick_depth(&all, |d| {
                                    level_ok(
                                        &view,
                                        grid.root_side,
                                        n.bounds,
                                        d,
                                        target,
                                        true,
                                        false,
                                        false,
                                        false,
                                    )
                                })]
                            } else {
                                variant.kept(n).iter().map(|&i| n.levels[i].depth).collect()
                            };
                            CellCutNode {
                                bounds: n.bounds,
                                buildable: stored
                                    .first()
                                    .zip(stored.last())
                                    .map_or((0, 0), |(a, b)| (*a, *b)),
                                resident: stored.clone(),
                                stored,
                            }
                        })
                    };
                    let entries = cell_cut(&cut, &node_info, &view, &grid, [0.0; 3], target)
                        .map_err(|e| {
                            format!(
                                "desired cut rejected ({} {target}px {}): {e}",
                                pose.name,
                                variant.name()
                            )
                        })?;
                    if is_zoom {
                        let lv = lod_view(&view, &local_frustum, &grid, target, 0.0, lod_opts)?;
                        let res = cell_lod_case(&lv, &entries, &cache, &mut fine);
                        let now: HashSet<(u64, u8)> =
                            res.resident.iter().map(|r| (r.node, r.depth)).collect();
                        let (mut new_levels, mut new_on_demand, mut new_bytes) = (0, 0, 0);
                        for r in &res.resident {
                            if zoom_prev
                                .as_ref()
                                .is_none_or(|p| !p.contains(&(r.node, r.depth)))
                            {
                                new_levels += 1;
                                new_on_demand += usize::from(r.on_demand);
                                new_bytes += r.bytes;
                            }
                        }
                        zoom_rows.push(format!(
                            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{new_levels}\t{new_on_demand}\t{new_bytes}\t{:.2}\t{:.4}\t{:.4}",
                            args[0].to_string_lossy(),
                            pose.name,
                            cut.len(),
                            res.drawn,
                            res.resident_bytes,
                            res.on_demand_bytes,
                            res.deepest,
                            res.resident.len(),
                            dist3(lv.eye, leaf_eye.map(|v| v / visual_scale)),
                            lv.near,
                            lv.min_side,
                        ));
                        zoom_prev = Some(now);
                        continue;
                    }
                    if let Some(dir) = &export_dir {
                        let (mismatches, groups) = export_case(&ExportCase {
                            dir: dir.join(format!("{dataset_name}-{}-{target}px", pose.name)),
                            dataset: &dataset_name,
                            pose: pose.name,
                            target,
                            view: &view,
                            grid: &grid,
                            cache: &cache,
                            entries: &entries,
                            check: check_cell_pages,
                        })?;
                        check_mismatches += mismatches;
                        check_groups += groups;
                    }
                    if let Some(dir) = &export_pool_dir {
                        export_pool_case(&PoolCase {
                            dir: dir.join(format!("{dataset_name}-{}-{target}px", pose.name)),
                            dataset: &dataset_name,
                            pose: pose.name,
                            target,
                            view: &view,
                            grid: &grid,
                            cache: &cache,
                            cut: &cut,
                            points: &|id| {
                                hierarchy
                                    .node(id)
                                    .map(viewer_core::SpatialNode::point_count)
                            },
                        })?;
                    }
                    let (mut total, mut on_screen, mut stages) = (0_u64, 0_u64, 0_usize);
                    let mut draw_ranges = 0_usize;
                    let (mut over, mut over_limited) = (0_u64, 0_u64);
                    let (mut resident_cells, mut resident_levels) = (0_u64, 0_u64);
                    for entry in &entries {
                        stages += entry.stage.iter().filter(|stage| **stage).count();
                        let n = &cache[&entry.node];
                        for (depth, octants) in entry.draws() {
                            let level = n
                                .levels
                                .iter()
                                .find(|l| l.depth == depth)
                                .ok_or("drawn level not stored")?;
                            draw_ranges += level_draw_ranges(level, octants);
                        }
                        let depths: [u8; 8] = if variant.octants() {
                            let all: Vec<u8> = n.levels.iter().map(|l| l.depth).collect();
                            std::array::from_fn(|o| {
                                let b = octant_bounds(n.bounds, o).unwrap_or(n.bounds);
                                pick_depth(&all, |d| {
                                    level_ok(
                                        &view,
                                        grid.root_side,
                                        b,
                                        d,
                                        target,
                                        variant.tight(),
                                        matches!(
                                            variant,
                                            Variant::OctantTightGuard
                                                | Variant::OctantTightOffaxis
                                                | Variant::OctantTightNear
                                        ),
                                        matches!(
                                            variant,
                                            Variant::OctantTightOffaxis | Variant::OctantTightNear
                                        ),
                                        variant == Variant::OctantTightNear,
                                    )
                                })
                            })
                        } else {
                            let Some(depth) = entry.drawn[0] else {
                                continue;
                            };
                            [depth; 8]
                        };
                        if variant == Variant::OctantTight {
                            let node_depth = n.rep.saturating_sub(3);
                            let finest = n.levels.last().map_or(0, |l| l.depth);
                            for (spike_o, spike_depth) in depths.iter().enumerate() {
                                // The spike indexes octants with x as bit 0; the library with x
                                // as bit 2 (o = 4x + 2y + z).
                                let o = (spike_o & 1) << 2 | (spike_o & 2) | spike_o >> 2;
                                let Some(library) = entry.wanted[o] else {
                                    octant_masked += 1;
                                    continue;
                                };
                                // Levels spanning octants move to the next stored finer one, or
                                // the finest when none exists.
                                let expected = if *spike_depth > node_depth {
                                    *spike_depth
                                } else {
                                    n.levels
                                        .iter()
                                        .map(|l| l.depth)
                                        .find(|d| *d > node_depth)
                                        .unwrap_or(finest)
                                };
                                octant_compared += 1;
                                if library != expected {
                                    octant_mismatches += 1;
                                    if octant_examples.len() < 20 {
                                        octant_examples.push(format!(
                                            "{} {target}px budget {budget} node {} octant {o}: library {library}, spike {expected}",
                                            pose.name,
                                            entry.node.get()
                                        ));
                                    }
                                }
                            }
                        }
                        let mut distinct = depths.to_vec();
                        distinct.sort_unstable();
                        distinct.dedup();
                        let mid: [f64; 3] = std::array::from_fn(|a| {
                            f64::midpoint(n.bounds.min()[a], n.bounds.max()[a])
                        });
                        let finest = n.levels.last().map_or(0, |l| l.depth);
                        for &depth in &distinct {
                            let level = n
                                .levels
                                .iter()
                                .find(|l| l.depth == depth)
                                .ok_or("drawn level not stored")?;
                            resident_cells += level.rel.len() as u64;
                            resident_levels += 1;
                            let any_mask = entry.mask.iter().any(|w| *w != 0);
                            if any_mask && depth < n.rep {
                                return Err(format!(
                                    "node {} draws depth {depth} coarser than its masked 8^3 grid",
                                    entry.node.get()
                                )
                                .into());
                            }
                            let shift = depth.saturating_sub(n.rep);
                            let cell_side = grid.root_side / (1_u64 << depth) as f64;
                            for rel in &level.rel {
                                let min: [f64; 3] = std::array::from_fn(|a| {
                                    grid.root_min[a]
                                        + (level.lo[a] + u64::from(rel[a])) as f64 * cell_side
                                });
                                if distinct.len() > 1 {
                                    let octant = (0..3).fold(0_usize, |acc, a| {
                                        acc | usize::from(min[a] + cell_side / 2.0 >= mid[a]) << a
                                    });
                                    if depths[octant] != depth {
                                        continue;
                                    }
                                }
                                if any_mask {
                                    let m: [u64; 3] = std::array::from_fn(|a| {
                                        ((level.lo[a] + u64::from(rel[a])) >> shift)
                                            .wrapping_sub(n.rep_lo[a])
                                    });
                                    if m.iter().all(|v| *v < 8)
                                        && entry.mask[m[2] as usize] >> (m[0] + 8 * m[1]) & 1 == 1
                                    {
                                        continue;
                                    }
                                }
                                total += 1;
                                let max = min.map(|v| v + cell_side);
                                if let Ok(b) = SpatialBounds::new(min, max)
                                    && local_frustum.intersects(&b)
                                {
                                    on_screen += 1;
                                    if view.projected_voxel_edge_pixels(b, [0.0; 3])
                                        > target * (1.0 + 1e-6)
                                    {
                                        over += 1;
                                        over_limited += u64::from(depth == finest);
                                    }
                                }
                            }
                        }
                    }
                    let build_ms = t0.elapsed().as_secs_f64() * 1000.0;
                    let bytes12 = total * 12;
                    let (cpu_text, ratio_text, cpu_over) = match &cpu {
                        Ok((count, oversized)) => (
                            count.to_string(),
                            format!("{:.3}", on_screen as f64 / (*count).max(1) as f64),
                            oversized.to_string(),
                        ),
                        Err(text) => (text.clone(), "NA".to_string(), "NA".to_string()),
                    };
                    let resident_bytes12 = resident_cells * 12;
                    if resident_bytes12 > POOL_BYTES {
                        stop_resident[vi].push(format!(
                            "{} {target}px budget {budget}: {resident_bytes12} B",
                            pose.name
                        ));
                    }
                    if bytes12 > POOL_BYTES {
                        stop_bytes[vi].push(format!(
                            "{} {target}px budget {budget}: {bytes12} B",
                            pose.name
                        ));
                    }
                    if pose.name == "grazing" {
                        // Owner limits for the adopted variants: 2.5x at 1 px, 2x otherwise.
                        let limit = if target == 1.0
                            && matches!(
                                variant,
                                Variant::OctantTight
                                    | Variant::OctantTightGuard
                                    | Variant::OctantTightOffaxis
                            ) {
                            2.5
                        } else {
                            2.0
                        };
                        match &cpu {
                            Ok((count, _))
                                if on_screen as f64 > limit * (*count as f64) => stop_grazing[vi]
                                .push(format!(
                                    "{target}px budget {budget}: on-screen {on_screen} vs CPU {count}"
                                )),
                            Ok(_) => {}
                            Err(_) => na_grazing[vi].push(format!("{target}px budget {budget}")),
                        }
                    }
                    if variant == Variant::OctantTight
                        && target == 4.0
                        && pose_index < base_pose_count
                    {
                        ranges_4px.push((
                            draw_ranges,
                            cut.len(),
                            format!("{} budget {budget}", pose.name),
                        ));
                    }
                    rows.push(format!(
                        "{}\t{}\t{target}\t{budget}\t{}\t{}\t{}\t{total}\t{on_screen}\t{bytes12}\t{}\t{cpu_text}\t{ratio_text}\t{stages}\t{build_ms:.1}\t{over}\t{:.4}\t{over_limited}\t{cpu_over}\t{resident_bytes12}\t{resident_levels}\t{draw_ranges}",
                        args[0].to_string_lossy(),
                        pose.name,
                        variant.name(),
                        cut.len(),
                        selection.point_count(),
                        total * 16,
                        over as f64 / on_screen.max(1) as f64,
                    ));
                    if cell_lod_on && variant == Variant::OctantTight {
                        for (mode, spacing_mode) in LOD_MODES {
                            if lod_opts.px_only && spacing_mode {
                                continue;
                            }
                            let min_side = if spacing_mode {
                                reference_spacing.ok_or("no reference spacing for spacing mode")?
                            } else {
                                0.0
                            };
                            let lv =
                                lod_view(&view, &local_frustum, &grid, target, min_side, lod_opts)?;
                            let t_lod = Instant::now();
                            let res = cell_lod_case(&lv, &entries, &cache, &mut fine);
                            let lod_ms = t_lod.elapsed().as_secs_f64() * 1000.0;
                            let vs_ot = res.drawn as f64 / on_screen.max(1) as f64;
                            let vs_cpu = match &cpu {
                                Ok((count, _)) => Some(res.drawn as f64 / (*count).max(1) as f64),
                                Err(_) => None,
                            };
                            lod_rows.push(format!(
                                "{}\t{}\t{target}\t{mode}\t{}\t{}\t{on_screen}\t{vs_ot:.3}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{lod_ms:.0}\t{}\t{}",
                                args[0].to_string_lossy(),
                                pose.name,
                                cut.len(),
                                res.drawn,
                                cpu_text,
                                vs_cpu.map_or("NA".to_string(), |v| format!("{v:.3}")),
                                res.resident_bytes,
                                res.on_demand_bytes,
                                res.baseline_bytes,
                                res.drawn_only_bytes,
                                res.r2_bytes,
                                res.r3_bytes,
                                res.r4_bytes,
                                res.deepest,
                                res.resident.len(),
                                res.over,
                                res.beyond_chain,
                                res.below_s,
                                res.unbuilt,
                                res.over_1p5,
                                res.non_monotone,
                            ));
                            lod_records.push(LodRecord {
                                dataset: args[0].to_string_lossy().into_owned(),
                                pose: pose.name,
                                target,
                                mode,
                                vs_cpu,
                                resident_bytes: res.resident_bytes,
                            });
                        }
                    }
                }
                cases.push(Case {
                    pose: pose_index,
                    target,
                    budget,
                    cut,
                });
            }
        }
    }

    println!("# block case");
    println!(
        "dataset_file\tpose\ttarget_px\tbudget\tvariant\tcut_nodes\tcut_points\tcells_total\tcells_on_screen\tbytes12\tbytes16\tcpu_instances\tratio_on_screen_vs_cpu\tstage_octants\tbuild_ms\tover_target_cells\tover_target_frac\tover_target_source_limited\tcpu_oversized\tresident_bytes12\tresident_levels\tdraw_ranges"
    );
    for row in &rows {
        println!("{row}");
    }

    if cell_lod_on {
        println!("# block cell_lod");
        println!(
            "dataset_file\tpose\ttarget_px\tmode\tcut_nodes\tdrawn_instances\toctant_tight_instances\tratio_vs_octant_tight\tcpu_instances\tratio_vs_cpu\tresident_bytes\ton_demand_bytes\toctant_tight_resident_bytes\tdrawn_depths_only_bytes\tr2_bytes\tr3_bytes\tr3_coarser_only_bytes\tdeepest_depth\tresident_levels\tover_target_cells\tbeyond_chain_cells\tbelow_separation_cells\tunbuilt_nodes\tlod_ms\tover_1p5_target_cells\tnon_monotone_pairs"
        );
        for row in &lod_rows {
            println!("{row}");
        }
        println!("# block cell_lod_zoom");
        println!(
            "dataset_file\tpose\tcut_nodes\tdrawn_instances\tresident_bytes\ton_demand_bytes\tdeepest_depth\tresident_levels\tnew_levels\tnew_on_demand_levels\tnew_level_bytes\teye_to_inside_leaf\tnear\tmin_side"
        );
        for row in &zoom_rows {
            println!("{row}");
        }
        println!("# block cell_lod_stop_criteria");
        for line in lod_verdicts(&lod_records) {
            println!("{line}");
        }
    }

    println!("# block octant_mismatches");
    println!(
        "octant_mismatches\t{octant_mismatches}\t(compared {octant_compared} (node, octant) pairs of octant_tight; {octant_masked} fully masked octants skipped)"
    );
    for line in &octant_examples {
        println!("#   mismatch: {line}");
    }

    // Transitions: mixed cut = previous union next, all resident, next view and target.
    let mut index: BTreeMap<(u64, usize, u64), usize> = BTreeMap::new();
    for (i, c) in cases.iter().enumerate() {
        index.insert((c.budget, c.pose, c.target.to_bits()), i);
    }
    let mut transitions: Vec<(String, usize, usize)> = Vec::new();
    for &budget in &budgets {
        for pose in 0..base_pose_count {
            for pair in TARGETS.windows(2).rev() {
                // 8 -> 4 -> 2 -> 1
                if let (Some(&a), Some(&b)) = (
                    index.get(&(budget, pose, pair[1].to_bits())),
                    index.get(&(budget, pose, pair[0].to_bits())),
                ) {
                    transitions.push((
                        format!("within {} {}->{}px", poses[pose].name, pair[1], pair[0]),
                        a,
                        b,
                    ));
                }
            }
            if pose > 0 {
                for &t in &TARGETS {
                    if let (Some(&a), Some(&b)) = (
                        index.get(&(budget, pose - 1, t.to_bits())),
                        index.get(&(budget, pose, t.to_bits())),
                    ) {
                        transitions.push((
                            format!(
                                "between {}->{} {t}px",
                                poses[pose - 1].name,
                                poses[pose].name
                            ),
                            a,
                            b,
                        ));
                    }
                }
            }
        }
    }
    println!("# block transitions");
    println!(
        "transition\tbudget\tvariant\tprev_nodes\tnext_nodes\tunion_nodes\tresult\toffending_pairs"
    );
    let mut counts = [(0_usize, 0_usize); TRANSITION_VARIANTS.len()];
    let mut failing: [Vec<String>; TRANSITION_VARIANTS.len()] =
        [const { Vec::new() }; TRANSITION_VARIANTS.len()];
    for (name, a, b) in &transitions {
        let (prev, next) = (&cases[*a], &cases[*b]);
        let mut seen = HashSet::new();
        let union: Vec<NodeId> = prev
            .cut
            .iter()
            .chain(&next.cut)
            .copied()
            .filter(|id| seen.insert(*id))
            .collect();
        let view = ViewProjection::new(
            poses[next.pose].camera,
            VIEWPORT,
            visual_scale,
            build_limits,
        );
        for (v, variant) in TRANSITION_VARIANTS.into_iter().enumerate() {
            let node_info = |id: NodeId| {
                cache.get(&id).map(|n| {
                    let stored: Vec<u8> =
                        variant.kept(n).iter().map(|&i| n.levels[i].depth).collect();
                    CellCutNode {
                        bounds: n.bounds,
                        buildable: stored
                            .first()
                            .zip(stored.last())
                            .map_or((0, 0), |(a, b)| (*a, *b)),
                        resident: stored.clone(),
                        stored,
                    }
                })
            };
            let result = cell_cut(&union, &node_info, &view, &grid, [0.0; 3], next.target);
            counts[v].1 += 1;
            let (text, pairs) = match result {
                Ok(_) => ("ok".to_string(), 0),
                Err(viewer_render_wgpu::CellCutError::DescendantTooFine { .. }) => {
                    counts[v].0 += 1;
                    let pairs = too_fine_pairs(&union, &cache);
                    failing[v].push(format!("{name} budget {}: {pairs} pairs", next.budget));
                    ("DescendantTooFine".to_string(), pairs)
                }
                Err(e) => return Err(format!("transition {name}: {e}").into()),
            };
            println!(
                "{name}\t{}\t{}\t{}\t{}\t{}\t{text}\t{pairs}",
                next.budget,
                variant.name(),
                prev.cut.len(),
                next.cut.len(),
                union.len()
            );
        }
    }
    let expected_transitions = budgets.len()
        * (base_pose_count * (TARGETS.len() - 1) + (base_pose_count - 1) * TARGETS.len());
    println!(
        "# transitions evaluated per variant: {} of {expected_transitions} expected (the rest involve skipped cases)",
        transitions.len()
    );
    for (v, name) in TRANSITION_VARIANTS
        .map(Variant::name)
        .into_iter()
        .enumerate()
    {
        println!(
            "# {name}: DescendantTooFine in {}/{} transitions (frequency {:.4})",
            counts[v].0,
            counts[v].1,
            counts[v].0 as f64 / counts[v].1.max(1) as f64
        );
        for line in &failing[v] {
            println!("#   fails: {line}");
        }
    }
    let worst = counts
        .iter()
        .map(|(f, t)| *f as f64 / (*t).max(1) as f64)
        .fold(0.0, f64::max);
    let incomplete = transitions.len() < expected_transitions;

    println!("# block stop_criteria");
    {
        let failing: Vec<String> = ranges_4px
            .iter()
            .filter(|(ranges, nodes, _)| *ranges > 3 * *nodes)
            .map(|(ranges, nodes, case)| format!("{case}: {ranges} ranges vs {nodes} nodes"))
            .collect();
        let worst = ranges_4px.iter().max_by(|a, b| {
            (a.0 as f64 / a.1.max(1) as f64).total_cmp(&(b.0 as f64 / b.1.max(1) as f64))
        });
        let verdict = if ranges_4px.is_empty() {
            "UNKNOWN"
        } else if failing.is_empty() {
            "PASS"
        } else {
            "FAIL"
        };
        let detail = worst.map_or(String::new(), |(r, n, c)| {
            format!(
                " (worst {c}: draw_ranges {r}, cut_nodes {n}, limit {})",
                3 * n
            )
        });
        println!(
            "{verdict}\t[octant_tight] draw_ranges <= 3 x cut_nodes at 4 px in every case{detail}{}",
            if failing.is_empty() {
                String::new()
            } else {
                format!(": {}", failing.join("; "))
            }
        );
    }
    let skipped_text = if skipped.is_empty() {
        String::new()
    } else {
        format!(" [skipped cases: {}]", skipped.join("; "))
    };
    let skipped_grazing: Vec<_> = skipped
        .iter()
        .filter(|c| c.starts_with("grazing"))
        .cloned()
        .collect();
    for (vi, variant) in RUN_VARIANTS.iter().enumerate() {
        let name = variant.name();
        let (verdict, detail) = if !stop_bytes[vi].is_empty() {
            ("FAIL", format!(": {}", stop_bytes[vi].join("; ")))
        } else if !skipped.is_empty() {
            ("UNKNOWN", String::new())
        } else {
            ("PASS", String::new())
        };
        println!(
            "{verdict}\t[{name}] cut bytes (12 B) <= 256 MiB in every case{detail}{skipped_text}"
        );
        let (verdict, detail) = if !stop_resident[vi].is_empty() {
            ("FAIL", format!(": {}", stop_resident[vi].join("; ")))
        } else if !skipped.is_empty() {
            ("UNKNOWN", String::new())
        } else {
            ("PASS", String::new())
        };
        println!(
            "{verdict}\t[{name}] resident_bytes12 (whole drawn levels) <= 256 MiB in every case{detail}{skipped_text}"
        );
        let limit_text = if matches!(
            variant,
            Variant::OctantTight
                | Variant::OctantTightGuard
                | Variant::OctantTightOffaxis
                | Variant::OctantTightNear
        ) {
            "2.5x (1 px) / 2x (2, 4, 8 px)"
        } else {
            "2x"
        };
        let (verdict, detail) = if !stop_grazing[vi].is_empty() {
            ("FAIL", format!(": {}", stop_grazing[vi].join("; ")))
        } else if !na_grazing[vi].is_empty() || !skipped_grazing.is_empty() {
            (
                "UNKNOWN",
                format!(
                    ": no CPU count for rows [{}], skipped [{}]",
                    na_grazing[vi].join("; "),
                    skipped_grazing.join("; ")
                ),
            )
        } else {
            ("PASS", String::new())
        };
        println!(
            "{verdict}\t[{name}] on-screen instances <= {limit_text} CPU instances in the grazing pose{detail}"
        );
    }
    let verdict = if worst > 0.05 {
        "FAIL"
    } else if incomplete {
        "UNKNOWN"
    } else {
        "PASS"
    };
    println!("{verdict}\tDescendantTooFine in <= 5 % of cut updates (worst variant {worst:.4})");
    println!(
        "# cell cache: {} nodes, {cache_bytes} bytes (nothing evicted, so peak = final)",
        cache.len()
    );
    println!(
        "# nodes whose first stored level is coarser than their 8^3 grid: {coarse_first_level_nodes}"
    );
    println!("# nodes with a first stored level coarser than their 8^3 grid:");
    for line in &coarse_nodes {
        println!("#   {line}");
    }
    eprintln!("total wall {:.1}s", started_all.elapsed().as_secs_f64());
    if octant_mismatches > 0 {
        return Err(format!(
            "octant_mismatches: {octant_mismatches} (node, octant) pairs differ from level_ok"
        )
        .into());
    }
    if check_cell_pages && check_groups == 0 {
        return Err("check-cell-pages compared no (node, level) group".into());
    }
    if check_mismatches > 0 {
        return Err(format!("check-cell-pages: {check_mismatches} mismatches").into());
    }
    Ok(())
}

// ---- `--export`: cube-pool files for the WebGPU spike (examples/web-host/probes/spike-cube-pool-format.md) ----

const SENTINEL: u32 = u32::MAX;
const MISSING_INTENSITY: u16 = u16::MAX;
/// Sampled drawn cells for `f32_max_error_px`: spread over all cells, plus the deepest depth.
const ERROR_SAMPLES: u64 = 700;
const ERROR_SAMPLES_DEEPEST: u64 = 300;

fn cell_attr(cell: &NodeCell) -> CellAttr {
    let count = u64::from(cell.count);
    let [r, g, b] = cell
        .rgb_sum
        .map(|sum| ((sum + count / 2) / count).min(255) as u32);
    CellAttr {
        rgba: r | g << 8 | b << 16 | 255 << 24,
        intensity: if cell.intensity_count == 0 {
            -1.0
        } else {
            (cell.intensity_sum / f64::from(cell.intensity_count)) as f32
        },
        class: cell.classification,
    }
}

/// Mean intensity rounded into `u16` (the CPU path keeps an `f32`); `0xFFFF` marks "missing", so a
/// real 65535 is clamped to 65534.
fn intensity_u16(attr: &CellAttr) -> u16 {
    if attr.intensity < 0.0 {
        MISSING_INTENSITY
    } else {
        attr.intensity.round().min(f32::from(MISSING_INTENSITY - 1)) as u16
    }
}

struct ExportCase<'a> {
    check: bool,
    dir: PathBuf,
    dataset: &'a str,
    pose: &'a str,
    target: f64,
    view: &'a ViewProjection,
    grid: &'a CellGrid,
    cache: &'a HashMap<NodeId, NodeData>,
    entries: &'a [CellCutEntry],
}

struct PageSpec {
    start: usize,
    end: usize,
    /// Node-relative cell origin of the page (per-axis minimum).
    origin: [u32; 3],
}

/// Greedy split in stored (Morton) order: a new page when the extent exceeds `2^bits` on any axis
/// or the page holds `per_page` cells.
fn split_pages(rel: &[[u32; 3]], bits: u32, per_page: usize) -> Vec<PageSpec> {
    let limit = 1_u64 << bits;
    let mut pages = Vec::new();
    let Some(first) = rel.first() else {
        return pages;
    };
    let (mut start, mut min, mut max) = (0, *first, *first);
    for (i, c) in rel.iter().enumerate().skip(1) {
        let new_min: [u32; 3] = std::array::from_fn(|a| min[a].min(c[a]));
        let new_max: [u32; 3] = std::array::from_fn(|a| max[a].max(c[a]));
        let too_wide = (0..3).any(|a| u64::from(new_max[a]) - u64::from(new_min[a]) + 1 > limit);
        if too_wide || i - start == per_page {
            pages.push(PageSpec {
                start,
                end: i,
                origin: min,
            });
            (start, min, max) = (i, *c, *c);
        } else {
            (min, max) = (new_min, new_max);
        }
    }
    pages.push(PageSpec {
        start,
        end: rel.len(),
        origin: min,
    });
    pages
}

struct VariantOut {
    name: &'static str,
    bits: u32,
    per_page: usize,
    instances: BufWriter<File>,
    pages: BufWriter<File>,
    ranges: BufWriter<File>,
    slots: u64,
    page_count: u64,
    empty: u64,
    range_count: u64,
    range_slots: u64,
    /// W16 instance words of the current group, only with `--check-cell-pages`.
    captured: Option<Vec<[u32; 4]>>,
}

impl VariantOut {
    fn create(dir: &Path, name: &'static str, bits: u32, per_page: usize) -> Result<Self> {
        let open = |file: &str| -> Result<BufWriter<File>> {
            Ok(BufWriter::with_capacity(
                1 << 20,
                File::create(dir.join(format!("{name}_{file}.bin")))?,
            ))
        };
        Ok(Self {
            name,
            bits,
            per_page,
            instances: open("instances")?,
            pages: open("pages")?,
            ranges: open("ranges")?,
            slots: 0,
            page_count: 0,
            empty: 0,
            range_count: 0,
            range_slots: 0,
            captured: None,
        })
    }

    fn write_words(writer: &mut BufWriter<File>, words: &[u32]) -> Result<()> {
        for word in words {
            writer.write_all(&word.to_le_bytes())?;
        }
        Ok(())
    }
}

/// Everything one (node, level) group needs, for either variant.
struct GroupCtx<'a> {
    level: &'a Level,
    node_index: u32,
    octant_bits: u32,
    mask_shift: u32,
    grid: &'a CellGrid,
    /// Eye in the dataset-local frame (unscaled).
    eye: [f64; 3],
    scale: f64,
    cell_ranges: &'a [Range<usize>],
}

fn cell_side(grid: &CellGrid, depth: u8) -> f64 {
    grid.root_side / (1_u64 << depth) as f64
}

/// Page min corner relative to the eye, in dataset units x `visual_scale` (f64, then cast).
fn page_origin_f32(ctx: &GroupCtx, origin: [u32; 3]) -> [f32; 3] {
    let side = cell_side(ctx.grid, ctx.level.depth);
    std::array::from_fn(|a| {
        let corner = ctx.grid.root_min[a] + (ctx.level.lo[a] + u64::from(origin[a])) as f64 * side;
        ((corner - ctx.eye[a]) * ctx.scale) as f32
    })
}

/// Writes pages, slots and draw ranges of one group; returns its pages.
fn write_group(vo: &mut VariantOut, ctx: &GroupCtx) -> Result<Vec<PageSpec>> {
    let level = ctx.level;
    let pages = split_pages(&level.rel, vo.bits, vo.per_page);
    let side32 = (cell_side(ctx.grid, level.depth) * ctx.scale) as f32;
    let mut slot_of = vec![0_u32; level.rel.len()];
    for page in &pages {
        let first_slot = vo.slots;
        let origin = page_origin_f32(ctx, page.origin);
        let mut record = [0_u32; 12];
        for a in 0..3 {
            record[a] = origin[a].to_bits();
        }
        record[3] = side32.to_bits();
        record[4] = ctx.node_index;
        record[5] = u32::from(level.depth);
        record[6..9].copy_from_slice(&page.origin);
        record[9] = ctx.octant_bits;
        record[10] = ctx.mask_shift;
        record[11] = 0;
        VariantOut::write_words(&mut vo.pages, &record)?;
        vo.page_count += 1;
        for k in 0..vo.per_page {
            let i = page.start + k;
            if i >= page.end {
                if vo.bits == 10 {
                    VariantOut::write_words(&mut vo.instances, &[SENTINEL, 0, 0])?;
                } else {
                    VariantOut::write_words(&mut vo.instances, &[SENTINEL, SENTINEL, 0, 0])?;
                    if let Some(captured) = &mut vo.captured {
                        captured.push([SENTINEL, SENTINEL, 0, 0]);
                    }
                }
                vo.empty += 1;
            } else {
                slot_of[i] = (first_slot + k as u64) as u32;
                let [x, y, z] =
                    std::array::from_fn::<u32, 3, _>(|a| level.rel[i][a] - page.origin[a]);
                let attr = &level.attrs[i];
                let extra = u32::from(intensity_u16(attr)) | u32::from(attr.class as u16) << 16;
                if vo.bits == 10 {
                    VariantOut::write_words(
                        &mut vo.instances,
                        &[x | y << 10 | z << 20, attr.rgba, extra],
                    )?;
                } else {
                    let words = [x | y << 16, z, attr.rgba, extra];
                    VariantOut::write_words(&mut vo.instances, &words)?;
                    if let Some(captured) = &mut vo.captured {
                        captured.push(words);
                    }
                }
            }
            vo.slots += 1;
        }
    }
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for range in ctx.cell_ranges.iter().filter(|r| !r.is_empty()) {
        let (start, end) = (slot_of[range.start], slot_of[range.end - 1] + 1);
        match merged.last_mut() {
            Some(last) if last.1 >= start => last.1 = end,
            _ => merged.push((start, end)),
        }
    }
    for (start, end) in merged {
        VariantOut::write_words(&mut vo.ranges, &[start, end - start])?;
        vo.range_count += 1;
        vo.range_slots += u64::from(end - start);
    }
    Ok(pages)
}

/// Clip-space NDC of `p` (eye-relative, scaled) for the column-major matrix `m`; `None` behind the eye.
fn ndc(m: &[f64; 16], p: [f64; 3]) -> Option<[f64; 2]> {
    let row = |r: usize| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r];
    let w = row(3);
    (w > 0.0).then(|| [row(0) / w, row(1) / w])
}

/// Compares the spike's W16 group (`pages`, `words` in slot order) with the library packing and
/// returns the number of differences: page count, page cell ranges and origins, words 0, 2, 3 and
/// z, and the flags (word 1, high half) against the spike's own formula of
/// `spike-cube-pool-format.md` ("Vertex-stage contract"): mask cell `(page origin + rel) >>
/// mask_shift` (or `<<` when negative) in u32 math, bit 9 = the level has no octant offsets. The
/// mask bits are compared where that math is valid (no u32 overflow, every axis below 8).
struct GroupCheck {
    mismatches: u64,
    flag_mismatches: u64,
    flag_skipped: u64,
}

fn check_group(
    level: &Level,
    pages: &[PageSpec],
    words: &[[u32; 4]],
    mask_shift: i32,
) -> GroupCheck {
    let mut out = GroupCheck {
        mismatches: 0,
        flag_mismatches: 0,
        flag_skipped: 0,
    };
    let Some(packed) = &level.packed else {
        out.mismatches = 1;
        return out;
    };
    if pages.len() != packed.pages.len() || words.len() != packed.instances.len() {
        out.mismatches = 1 + u64::from(pages.len() != packed.pages.len());
        return out;
    }
    let mut mismatches = 0;
    for (spike, page) in pages.iter().zip(&packed.pages) {
        let origin = std::array::from_fn(|a| level.lo[a] + u64::from(spike.origin[a]));
        mismatches += u64::from(
            spike.start != page.first_cell
                || spike.end - spike.start != page.cells as usize
                || origin != page.origin,
        );
    }
    for (slot, (spike, library)) in words.iter().zip(&packed.instances).enumerate() {
        let library = library.0;
        mismatches += u64::from(
            [0, 2, 3].iter().any(|w| spike[*w] != library[*w])
                || spike[1] & 0xFFFF != library[1] & 0xFFFF,
        );
        if spike[0] == SENTINEL {
            continue;
        }
        let origin = pages[slot / PAGE_SLOTS].origin;
        let rel = [spike[0] & 0xFFFF, spike[0] >> 16, spike[1] & 0xFFFF];
        let mask: [Option<u32>; 3] = std::array::from_fn(|a| {
            let cell = origin[a].checked_add(rel[a])?;
            let shifted = if mask_shift >= 0 {
                u64::from(cell)
                    .checked_shr(mask_shift as u32)
                    .filter(|_| mask_shift < 32)?
            } else {
                u64::from(cell).checked_shl(mask_shift.unsigned_abs())?
            };
            u32::try_from(shifted).ok().filter(|v| *v < 8)
        });
        let flags = library[1] >> 16;
        let spans = u32::from(level.octant_offsets.is_none());
        let mut wrong = flags >> 9 & 1 != spans || flags >> 10 != 0;
        if let [Some(x), Some(y), Some(z)] = mask {
            wrong |= flags & 0x1FF != x + 8 * y + 64 * z;
        } else {
            out.flag_skipped += 1;
        }
        out.flag_mismatches += u64::from(wrong);
    }
    out.mismatches = mismatches + out.flag_mismatches;
    out
}

/// Returns the `--check-cell-pages` mismatches and compared (node, level) groups (0 without the
/// flag).
fn export_case(case: &ExportCase) -> Result<(u64, u64)> {
    let dir = &case.dir;
    fs::create_dir_all(dir)?;
    let scale = case.view.visual_scale();
    let camera = case.view.camera();
    let eye_scaled = camera.position();
    let eye: [f64; 3] = eye_scaled.map(|v| v / scale);
    let viewport = case.view.viewport();
    let frame = camera.frame(viewport).ok_or("camera frame unavailable")?;
    // The renderer draws `M * (center * scale - target)`; the exchange format feeds `p = center *
    // scale - eye`, so the translation `eye - target` folds into the last column.
    let target = camera.target();
    let shift: [f64; 3] = std::array::from_fn(|a| eye_scaled[a] - target[a]);
    let cols = frame.view_projection;
    let mut m = [0.0_f64; 16];
    for c in 0..3 {
        for r in 0..4 {
            m[c * 4 + r] = f64::from(cols[c][r]);
        }
    }
    for r in 0..4 {
        m[12 + r] = f64::from(cols[3][r])
            + (0..3)
                .map(|c| f64::from(cols[c][r]) * shift[c])
                .sum::<f64>();
    }
    let m32: [f32; 16] = m.map(|v| v as f32);
    let m: [f64; 16] = m32.map(f64::from);

    let mut v10 = VariantOut::create(dir, "w10", 10, 682)?;
    let mut v16 = VariantOut::create(dir, "w16", 16, 512)?;
    let frame = PageFrame {
        grid: *case.grid,
        coordinate_origin: [0.0; 3],
        visual_scale: scale,
        eye_scaled,
    };
    let table = srgb_to_linear_table();
    let (mut mismatches, mut library_pages, mut library_empty) = (0_u64, 0_u64, 0_u64);
    let (mut groups, mut flag_mismatches, mut flag_skipped, mut eye_px_skipped) =
        (0_u64, 0_u64, 0_u64, 0_u64);
    let (mut eye_px, mut eye_px_at, mut eye_px_corners) = (0.0_f64, 0.0_f64, 0_u64);
    let mut nodes = BufWriter::new(File::create(dir.join("nodes.bin"))?);
    let mut i40 = BufWriter::with_capacity(1 << 20, File::create(dir.join("i40_instances.bin"))?);

    let deepest = case
        .entries
        .iter()
        .flat_map(viewer_render_wgpu::CellCutEntry::required_levels)
        .max()
        .unwrap_or(0);
    let mut resident_cells = 0_u64;
    let mut deepest_cells = 0_u64;
    for entry in case.entries {
        let node = &case.cache[&entry.node];
        for depth in entry.required_levels() {
            let len = node
                .levels
                .iter()
                .find(|l| l.depth == depth)
                .map_or(0, |l| l.rel.len() as u64);
            resident_cells += len;
            if depth == deepest {
                deepest_cells += len;
            }
        }
    }
    let stride_all = (resident_cells / ERROR_SAMPLES).max(1);
    let stride_deep = (deepest_cells / ERROR_SAMPLES_DEEPEST).max(1);
    let (mut seen_all, mut seen_deep) = (0_u64, 0_u64);

    let (mut drawn_cells, mut levels, mut lo_mismatches) = (0_u64, 0_u64, 0_u64);
    let mut max_error_px = 0.0_f64;
    let mut samples = 0_u64;
    let half = [f64::from(viewport[0]) / 2.0, f64::from(viewport[1]) / 2.0];
    for (node_index, entry) in case.entries.iter().enumerate() {
        let node = &case.cache[&entry.node];
        let words: Vec<u32> = entry
            .mask
            .iter()
            .flat_map(|w| [*w as u32, (*w >> 32) as u32])
            .collect();
        for word in &words {
            nodes.write_all(&word.to_le_bytes())?;
        }
        let any_mask = entry.mask.iter().any(|w| *w != 0);
        for (depth, octants) in entry.draws() {
            let level = node
                .levels
                .iter()
                .find(|l| l.depth == depth)
                .ok_or("drawn level not stored")?;
            if any_mask && depth < node.rep {
                return Err(format!(
                    "node {} draws depth {depth} coarser than its masked 8^3 grid",
                    entry.node.get()
                )
                .into());
            }
            let node_depth = node.rep.saturating_sub(3);
            let signed_shift = i32::from(depth) - (i32::from(node_depth) + 3);
            // Signed (two's complement): negative below node depth + 3, the page shifts left.
            let mask_shift = signed_shift as u32;
            if signed_shift >= 0 && (0..3).any(|a| level.lo[a] != node.rep_lo[a] << signed_shift) {
                lo_mismatches += 1;
            }
            let ranges = cell_ranges(level, octants);
            let ctx = GroupCtx {
                level,
                node_index: node_index as u32,
                octant_bits: u32::from(octants),
                mask_shift,
                grid: case.grid,
                eye,
                scale,
                cell_ranges: &ranges,
            };
            let pages10 = write_group(&mut v10, &ctx)?;
            v16.captured = case.check.then(Vec::new);
            let pages16 = write_group(&mut v16, &ctx)?;
            if let Some(words) = v16.captured.take() {
                let found = check_group(level, &pages16, &words, signed_shift);
                if found.mismatches != 0 {
                    eprintln!(
                        "  MISMATCH node {} depth {depth}: {} differences ({} in flags)",
                        entry.node.get(),
                        found.mismatches,
                        found.flag_mismatches
                    );
                }
                mismatches += found.mismatches;
                flag_mismatches += found.flag_mismatches;
                flag_skipped += found.flag_skipped;
                groups += 1;
                if let Some(packed) = &level.packed {
                    library_pages += packed.pages.len() as u64;
                    library_empty += packed.empty_slots() as u64;
                }
            }
            levels += 1;
            let check = level.packed.as_ref().filter(|_| case.check).map(|packed| {
                let entries: Vec<PageEntry> =
                    packed.pages.iter().map(|p| p.entry(&frame, 0, 0)).collect();
                (packed, entries)
            });

            let side = cell_side(case.grid, depth);
            let side32 = (side * scale) as f32;
            let rep_shift = depth.saturating_sub(node.rep);
            for range in &ranges {
                for i in range.clone() {
                    let rel = level.rel[i];
                    if any_mask {
                        let mc: [u64; 3] = std::array::from_fn(|a| {
                            ((level.lo[a] + u64::from(rel[a])) >> rep_shift)
                                .wrapping_sub(node.rep_lo[a])
                        });
                        if mc.iter().all(|v| *v < 8)
                            && entry.mask[mc[2] as usize] >> (mc[0] + 8 * mc[1]) & 1 == 1
                        {
                            continue;
                        }
                    }
                    drawn_cells += 1;
                    if let Some((packed, entries)) = &check {
                        let page = packed.pages.partition_point(|p| p.first_cell <= i) - 1;
                        let slot = page * PAGE_SLOTS + (i - packed.pages[page].first_cell);
                        let decoded = decode_cell(&entries[page], &packed.instances[slot], &table);
                        let coord: [u64; 3] =
                            std::array::from_fn(|a| level.lo[a] + u64::from(rel[a]));
                        for (got, offset) in [(decoded.lo, 0.0), (decoded.hi, 1.0)] {
                            // Both in the eye-relative frame that `m` expects.
                            let exact: [f64; 3] = std::array::from_fn(|a| {
                                (case.grid.root_min[a] + (coord[a] as f64 + offset) * side) * scale
                                    - eye_scaled[a]
                            });
                            let seen: [f64; 3] = got.map(f64::from);
                            match (ndc(&m, exact), ndc(&m, seen)) {
                                (Some(a), Some(b)) if a[0].abs() <= 1.5 && a[1].abs() <= 1.5 => {
                                    let px =
                                        ((a[0] - b[0]) * half[0]).hypot((a[1] - b[1]) * half[1]);
                                    eye_px_corners += 1;
                                    if px > eye_px {
                                        let metres =
                                            exact.iter().map(|v| v * v).sum::<f64>().sqrt();
                                        (eye_px, eye_px_at) = (px, metres / scale);
                                    }
                                }
                                // Behind the eye or outside 1.5 NDC.
                                _ => eye_px_skipped += 1,
                            }
                        }
                    }
                    let reference: [f64; 3] = std::array::from_fn(|a| {
                        (case.grid.root_min[a]
                            + (level.lo[a] + u64::from(rel[a])) as f64 * side
                            + side / 2.0
                            - eye[a])
                            * scale
                    });
                    let attr = &level.attrs[i];
                    let rgb = [0, 8, 16].map(|s| (attr.rgba >> s & 0xFF) as u8);
                    let instance = InstanceRaw {
                        center: reference.map(|v| v as f32),
                        size: side32,
                        color: viewer_core::ColorRgb::new(rgb[0], rgb[1], rgb[2]).to_linear_rgba(),
                        intensity: attr.intensity,
                        classification: f32::from(attr.class),
                    };
                    i40.write_all(bytemuck::bytes_of(&instance))?;

                    seen_all += 1;
                    let take = (seen_all - 1) % stride_all == 0
                        || depth == deepest && {
                            seen_deep += 1;
                            (seen_deep - 1) % stride_deep == 0
                        };
                    if take {
                        let page_index = pages10.partition_point(|p| p.start <= i) - 1;
                        let page = &pages10[page_index];
                        let origin = page_origin_f32(&ctx, page.origin);
                        let rebuilt: [f64; 3] = std::array::from_fn(|a| {
                            let r = (rel[a] - page.origin[a]) as f32;
                            f64::from(origin[a] + (r + 0.5) * side32)
                        });
                        if let (Some(a), Some(b)) = (ndc(&m, reference), ndc(&m, rebuilt))
                            && a[0].abs() <= 1.5
                            && a[1].abs() <= 1.5
                        {
                            let px = ((a[0] - b[0]) * half[0]).hypot((a[1] - b[1]) * half[1]);
                            max_error_px = max_error_px.max(px);
                            samples += 1;
                        }
                    }
                }
            }
        }
    }
    for writer in [&mut v10.instances, &mut v10.pages, &mut v10.ranges] {
        writer.flush()?;
    }
    for writer in [&mut v16.instances, &mut v16.pages, &mut v16.ranges] {
        writer.flush()?;
    }
    nodes.flush()?;
    i40.flush()?;

    let variant_json = |v: &VariantOut| {
        format!(
            "\"{}\": {{ \"slots_per_page\": {}, \"pages\": {}, \"slots_used\": {}, \"empty_slots\": {} }}",
            v.name, v.per_page, v.page_count, v.slots, v.empty
        )
    };
    let json = format!(
        "{{\n  \"dataset\": \"{}\", \"pose\": \"{}\", \"target_px\": {},\n  \"viewport\": [{}, {}],\n  \"view_proj\": [{}],\n  \"eye\": [{}, {}, {}],\n  \"visual_scale\": {},\n  \"nodes\": {}, \"levels\": {levels},\n  \"variants\": {{\n    {},\n    {}\n  }},\n  \"drawn_cells\": {drawn_cells}, \"resident_cells\": {resident_cells},\n  \"ranges_w10\": {}, \"ranges_w16\": {},\n  \"f32_max_error_px\": {max_error_px}\n}}\n",
        case.dataset,
        case.pose,
        case.target,
        viewport[0],
        viewport[1],
        m32.map(|v| v.to_string()).join(", "),
        eye[0],
        eye[1],
        eye[2],
        scale,
        case.entries.len(),
        variant_json(&v10),
        variant_json(&v16),
        v10.range_count,
        v16.range_count,
    );
    fs::write(dir.join("case.json"), json)?;
    validate_export(dir, case.entries.len(), &v10, &v16)?;
    println!(
        "# export {}: drawn {drawn_cells} resident {resident_cells} levels {levels} | w10 pages {} slots {} empty {:.4} ranges {} range_slots {} | w16 pages {} slots {} empty {:.4} ranges {} range_slots {} | f32_max_error_px {max_error_px:.5} ({samples} samples) | lo mismatches {lo_mismatches}",
        dir.display(),
        v10.page_count,
        v10.slots,
        v10.empty as f64 / v10.slots.max(1) as f64,
        v10.range_count,
        v10.range_slots,
        v16.page_count,
        v16.slots,
        v16.empty as f64 / v16.slots.max(1) as f64,
        v16.range_count,
        v16.range_slots,
    );
    if case.check {
        if groups == 0 {
            eprintln!(
                "  MISMATCH {}: no (node, level) group was compared",
                dir.display()
            );
            mismatches += 1;
        }
        println!(
            "# check-cell-pages {}: groups {groups} pages {} (spike {}) slots {} empty {:.4} mismatches {mismatches} (flags {flag_mismatches}, flag cells skipped {flag_skipped}) | eye-frame max error {eye_px:.5} px at {eye_px_at:.2} m ({eye_px_corners} corners, {eye_px_skipped} skipped)",
            dir.display(),
            library_pages,
            v16.page_count,
            v16.slots,
            library_empty as f64 / v16.slots.max(1) as f64,
        );
    }
    Ok((mismatches, groups))
}

/// Reads the files back: sizes, ranges within slots, non-empty slots decode inside `2^W`.
fn validate_export(dir: &Path, nodes: usize, v10: &VariantOut, v16: &VariantOut) -> Result<()> {
    let read = |name: &str| fs::read(dir.join(name));
    let size = |name: &str| -> Result<u64> { Ok(fs::metadata(dir.join(name))?.len()) };
    if size("nodes.bin")? != 64 * nodes as u64 {
        return Err("nodes.bin size mismatch".into());
    }
    for (v, words) in [(v10, 3_usize), (v16, 4)] {
        let stride = 4 * words;
        let name = v.name;
        let bytes = read(&format!("{name}_instances.bin"))?;
        if bytes.len() != stride * v.slots as usize {
            return Err(format!("{name}_instances.bin size mismatch").into());
        }
        if v.slots != v.page_count * v.per_page as u64 {
            return Err(format!("{name}: slots != pages x slots_per_page").into());
        }
        if size(&format!("{name}_pages.bin"))? != 48 * v.page_count {
            return Err(format!("{name}_pages.bin size mismatch").into());
        }
        let word = |slot: usize, w: usize| {
            let at = slot * stride + 4 * w;
            u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
        };
        let mut empty = 0_u64;
        for slot in 0..v.slots as usize {
            if word(slot, 0) == SENTINEL {
                empty += 1;
            } else if v.bits == 10 {
                if word(slot, 0) >> 30 != 0 {
                    return Err(format!("{name} slot {slot}: bits above 29 set").into());
                }
            } else if word(slot, 1) >> 16 != 0 {
                return Err(format!("{name} slot {slot}: flags not zero").into());
            }
        }
        if empty != v.empty {
            return Err(format!("{name}: {empty} empty slots read, {} counted", v.empty).into());
        }
        let ranges = read(&format!("{name}_ranges.bin"))?;
        if ranges.len() != 8 * v.range_count as usize {
            return Err(format!("{name}_ranges.bin size mismatch").into());
        }
        for pair in ranges.as_chunks::<8>().0 {
            let first = u64::from(u32::from_le_bytes(pair[..4].try_into().expect("4 bytes")));
            let count = u64::from(u32::from_le_bytes(pair[4..].try_into().expect("4 bytes")));
            if count == 0 || first + count > v.slots {
                return Err(
                    format!("{name}: range {first}+{count} outside {} slots", v.slots).into(),
                );
            }
        }
    }
    Ok(())
}

// ---- `--export-pool`: packed levels for `cell_pool_bench` (format in the file header) ----

struct PoolCase<'a> {
    dir: PathBuf,
    dataset: &'a str,
    pose: &'a str,
    target: f64,
    view: &'a ViewProjection,
    grid: &'a CellGrid,
    cache: &'a HashMap<NodeId, NodeData>,
    cut: &'a [NodeId],
    points: &'a dyn Fn(NodeId) -> Option<u64>,
}

fn export_pool_case(case: &PoolCase) -> Result<()> {
    fs::create_dir_all(&case.dir)?;
    let bits3 = |v: [f64; 3]| v.map(f64::to_bits);
    let mut bin = BufWriter::with_capacity(1 << 20, File::create(case.dir.join("levels.bin"))?);
    let mut offset = 0_u64;
    let mut nodes = Vec::new();
    for id in case.cut {
        let node = &case.cache[id];
        let mut levels = Vec::new();
        for level in &node.levels {
            let packed = level
                .packed
                .as_ref()
                .ok_or("pool export needs the packed levels")?;
            levels.push(serde_json::json!({
                "depth": level.depth, "pages": packed.pages.len(), "offset": offset,
            }));
            for instance in &packed.instances {
                for word in instance.0 {
                    bin.write_all(&word.to_le_bytes())?;
                }
            }
            for page in &packed.pages {
                for axis in page.origin {
                    bin.write_all(&axis.to_le_bytes())?;
                }
                bin.write_all(&(page.first_cell as u64).to_le_bytes())?;
                bin.write_all(&page.cells.to_le_bytes())?;
                bin.write_all(&page.flags.to_le_bytes())?;
                for axis in page.box_max {
                    bin.write_all(&axis.to_le_bytes())?;
                }
                bin.write_all(&[0, 0])?;
            }
            offset += (packed.instances.len() * 16 + packed.pages.len() * 48) as u64;
        }
        nodes.push(serde_json::json!({
            "id": id.get(),
            "bounds_min_bits": bits3(node.bounds.min()),
            "bounds_max_bits": bits3(node.bounds.max()),
            "points": (case.points)(*id).ok_or("cut node missing")?,
            "levels": levels,
        }));
    }
    bin.flush()?;
    let viewport = case.view.viewport();
    let meta = serde_json::json!({
        "format": "r3-pool-v1",
        "dataset": case.dataset,
        "pose": case.pose,
        "target_px_bits": case.target.to_bits(),
        "viewport": viewport,
        "visual_scale_bits": case.view.visual_scale().to_bits(),
        "coordinate_origin_bits": bits3([0.0; 3]),
        "grid": {
            "root_min_bits": bits3(case.grid.root_min),
            "root_side_bits": case.grid.root_side.to_bits(),
        },
        "camera_bits": case.view.camera().to_f64_array().map(f64::to_bits),
        "cut": case.cut.iter().map(|id| id.get()).collect::<Vec<_>>(),
        "nodes": nodes,
    });
    fs::write(case.dir.join("pool.json"), serde_json::to_vec(&meta)?)?;
    eprintln!(
        "# pool export {}: {} nodes, {} MiB",
        case.dir.display(),
        case.cut.len(),
        offset >> 20
    );
    Ok(())
}

// ---- `--cell-lod`: per-cell level of detail on the CPU (plan nivel-pedido-y-lod-por-celda, 0a) ----
//
// Model, all in the unscaled local frame of the grid:
// - `refines(c) := side(c) > max(min_side, target * max(dist(centre(c), eye), near) / focal)` (D3).
// - Per cut node, the descent starts at the coarsest stored depth and splits a cell while it
//   refines, down to `MAX_CELL_DEPTH`. Children come from the node's points: the cells of the
//   deepest depth `S` where its points separate (built once with the library, `min_growth` 1.0),
//   sorted by an absolute Morton order, so the cell of any depth `d <= S` is a contiguous range
//   and its coordinate is `coord_S >> (S - d)`. Below `S` a cell holds one point, so its only
//   child is the sub-cell of that point; the spike keeps the centre of the `S` cell for it (the
//   point position is not kept), which only matters for cells within a few `S` cells of the eye.
// - A cell is dropped when it is outside the frustum or under a resident descendant node (the
//   mask of `cell_cut`, as in `octant_tight`); a cell coarser than the node's 8^3 grid under a
//   non-empty mask is forced to refine (D5). Only cells in the frustum are counted or required.
// - Residency (D4): per node the depths from the coarsest to the finest drawn one, except that
//   below `S` only the drawn depths and the next one count. Bytes are `pack_level` pages times
//   `PAGE_BYTES`; levels missing from the stored chain are "on demand".

const ZOOM_STEPS: usize = 10;
const LOD_MODES: [(&str, bool); 2] = [("px", false), ("spacing", true)];
/// Page of 512 slots of 16 B plus its 48 B record (`--export-pool` format).
const PAGE_BYTES: u64 = PAGE_SLOTS as u64 * 16 + 48;
const MIB: f64 = 1024.0 * 1024.0;

/// Vertical focal length in pixels (`projected_voxel_edge_pixels`, `VERTICAL_FOV` = 45 degrees).
fn lod_focal() -> f64 {
    f64::from(VIEWPORT[1]) / (2.0 * (45.0_f64.to_radians() * 0.5).tan())
}

fn dist3(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

struct LodView<'a> {
    view: &'a ViewProjection,
    frustum: &'a SpatialFrustum,
    grid: &'a CellGrid,
    eye: [f64; 3],
    focal: f64,
    near: f64,
    target: f64,
    min_side: f64,
    opts: LodOpts,
    /// The eye-relative scaled view `cell_lod::refines` takes (`Metric::Bound`).
    bound: cell_lod::LodView,
    eye_scaled: [f64; 3],
    scale: f64,
}

/// The phase 2a E1 bound the metric gate measured (it failed; `cell_lod` now uses the actual edge):
/// `side * focal * dmax / max(zmin, near)^2`, kept here so the record stays reproducible. `near` is
/// the frustum near plane's distance in scaled units, the nearest equivalent the spike has of the
/// `lod_min_depth` the library function used.
fn bound_refines(view: &cell_lod::LodView, near: f32, lo: [f32; 3], side: f32) -> bool {
    let half = side * 0.5;
    let centre = [lo[0] + half, lo[1] + half, lo[2] + half];
    let forward = view.forward;
    let depth = centre[0] * forward[0] + centre[1] * forward[1] + centre[2] * forward[2];
    let spread = forward[0].abs() + forward[1].abs() + forward[2].abs();
    let zmin = (depth - half * spread).max(near);
    let fx = centre[0].abs() + half;
    let fy = centre[1].abs() + half;
    let fz = centre[2].abs() + half;
    let dmax = (fx * fx + fy * fy + fz * fz).sqrt();
    side > view.min_side && side * view.focal * dmax / (zmin * zmin) > view.target_px
}

/// Refine rule of the descent: `Bound` is the failed phase 2a E1 bound (`bound_refines`), `Edge` refines while
/// the actual projected edge is above the target, `Distance` is the phase 0a rule.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Metric {
    Bound,
    Edge,
    Distance,
}

#[derive(Clone, Copy)]
struct LodOpts {
    residency: u8,
    nearest: bool,
    metric: Metric,
    px_only: bool,
}

fn lod_view<'a>(
    view: &'a ViewProjection,
    frustum: &'a SpatialFrustum,
    grid: &'a CellGrid,
    target: f64,
    min_side: f64,
    opts: LodOpts,
) -> Result<LodView<'a>> {
    let scale = view.visual_scale();
    let eye = view.camera().position().map(|v| v / scale);
    // Plane 4 of the frustum is the near plane (`spatial_frustum_with_near`); its normal is scaled by
    // `scale` and the plane equation then gives the signed distance in scaled units.
    let near_plane = frustum
        .planes()
        .get(4)
        .ok_or("frustum without near plane")?;
    let n = near_plane.normal();
    let near = -((0..3).map(|i| n[i] * eye[i]).sum::<f64>() + near_plane.offset()) / scale;
    Ok(LodView {
        view,
        frustum,
        grid,
        eye,
        focal: lod_focal(),
        near,
        target,
        min_side,
        opts,
        bound: cell_lod::LodView::from_camera(
            &view.camera(),
            [VIEWPORT[0] as f32, VIEWPORT[1] as f32],
            target as f32,
            (min_side * scale) as f32,
        ),
        eye_scaled: view.camera().position(),
        scale,
    })
}

impl LodView<'_> {
    fn refines(&self, side: f64, min: [f64; 3]) -> bool {
        match self.opts.metric {
            Metric::Bound => {
                // Eye-relative scaled f32 coordinates, as the cut computes them.
                let lo: [f32; 3] =
                    std::array::from_fn(|i| (min[i] * self.scale - self.eye_scaled[i]) as f32);
                bound_refines(
                    &self.bound,
                    (self.near * self.scale) as f32,
                    lo,
                    (side * self.scale) as f32,
                )
            }
            Metric::Edge => {
                let Ok(bounds) = SpatialBounds::new(min, min.map(|v| v + side)) else {
                    return false;
                };
                side > self.min_side
                    && self.view.projected_voxel_edge_pixels(bounds, [0.0; 3]) > self.target
            }
            Metric::Distance => self.refines_distance(side, min),
        }
    }

    fn refines_distance(&self, side: f64, min: [f64; 3]) -> bool {
        let dist = if self.opts.nearest {
            // Nearest point of the cell to the eye (ADR 0034): zero when the eye is inside.
            (0..3)
                .map(|i| (self.eye[i].clamp(min[i], min[i] + side) - self.eye[i]).powi(2))
                .sum::<f64>()
                .sqrt()
        } else {
            dist3(min.map(|v| v + side / 2.0), self.eye)
        };
        let threshold = (self.target * dist.max(self.near) / self.focal).max(self.min_side);
        side > threshold
    }
}

/// Cells of the deepest depth `S` of one node where its points separate.
struct FineNode {
    s: u8,
    coords: Vec<[u64; 3]>,
    counts: Vec<u32>,
    /// Packed bytes per depth, filled on demand.
    bytes: HashMap<u8, u64>,
}

/// Morton order on absolute coordinates without interleaving: the axis of the most significant
/// differing bit decides.
fn morton_cmp(a: &[u64; 3], b: &[u64; 3]) -> std::cmp::Ordering {
    let x: [u64; 3] = std::array::from_fn(|i| a[i] ^ b[i]);
    let mut best = 0;
    for i in 1..3 {
        if x[best] < x[i] && x[best] < (x[best] ^ x[i]) {
            best = i;
        }
    }
    a[best].cmp(&b[best])
}

fn build_fine(
    grid: &CellGrid,
    id: NodeId,
    bounds: SpatialBounds,
    finest_stored: u8,
    blocks: &[PointBlock],
) -> Result<FineNode> {
    // Asking for a leaf chain that starts at the finest stored depth makes the library build only
    // the levels from there to the separation depth; `is_leaf` is true for internal nodes too,
    // which only changes where the chain may end.
    let spacing = grid.root_side / (1_u64 << finest_stored) as f64;
    let cells = PointNodeCellSource { min_growth: 1.0 }.node_cells(
        grid,
        id,
        bounds,
        Some(spacing),
        true,
        blocks,
    )?;
    let last = cells
        .levels
        .into_iter()
        .last()
        .ok_or("node without levels")?;
    let mut sorted: Vec<([u64; 3], u32)> = last.cells.iter().map(|c| (c.coord, c.count)).collect();
    sorted.sort_unstable_by(|a, b| morton_cmp(&a.0, &b.0));
    Ok(FineNode {
        s: last.depth,
        coords: sorted.iter().map(|c| c.0).collect(),
        counts: sorted.iter().map(|c| c.1).collect(),
        bytes: HashMap::new(),
    })
}

#[derive(Default)]
struct NodeDraw {
    drawn: u64,
    over: u64,
    /// Drawn cells above 1.5 times the target.
    over_1p5: u64,
    /// Bit `d` set when a cell of depth `d` is drawn.
    depths: u64,
    /// Drawn cells deeper than the finest stored level of the node.
    beyond_chain: u64,
    /// Drawn cells deeper than the separation depth `S`.
    below_s: u64,
    /// Drawn cells with a child (in the frustum, holding points) that refines: the child refines
    /// while its parent does not.
    non_monotone: u64,
}

fn draw_cell(
    lv: &LodView,
    dn: &DescentNode,
    bounds: &SpatialBounds,
    depth: u8,
    out: &mut NodeDraw,
) {
    out.drawn += 1;
    out.beyond_chain += u64::from(dn.node.levels.last().is_some_and(|l| depth > l.depth));
    out.below_s += u64::from(depth > dn.fine.s);
    out.depths |= 1 << depth;
    let edge = lv.view.projected_voxel_edge_pixels(*bounds, [0.0; 3]);
    if edge > lv.target * (1.0 + 1e-6) {
        out.over += 1;
    }
    out.over_1p5 += u64::from(edge > lv.target * 1.5);
}

struct DescentNode<'a> {
    node: &'a NodeData,
    fine: &'a FineNode,
    mask: Option<&'a [u64; 8]>,
}

fn descend(lv: &LodView, dn: &DescentNode, d: u8, a: usize, b: usize, out: &mut NodeDraw) {
    let (n, f) = (dn.node, dn.fine);
    let coord: [u64; 3] = f.coords[a].map(|c| c >> (f.s - d));
    let side = lv.grid.root_side / (1_u64 << d) as f64;
    let min: [f64; 3] = std::array::from_fn(|i| lv.grid.root_min[i] + coord[i] as f64 * side);
    let Ok(bounds) = SpatialBounds::new(min, min.map(|v| v + side)) else {
        return;
    };
    if !lv.frustum.intersects(&bounds) {
        return;
    }
    let mut forced = false;
    if let Some(mask) = dn.mask {
        if d < n.rep {
            forced = true;
        } else {
            let m: [u64; 3] =
                std::array::from_fn(|i| (coord[i] >> (d - n.rep)).wrapping_sub(n.rep_lo[i]));
            if m.iter().all(|v| *v < 8) && mask[m[2] as usize] >> (m[0] + 8 * m[1]) & 1 == 1 {
                return;
            }
        }
    }
    let centre: [f64; 3] = min.map(|v| v + side / 2.0);
    if d < f.s {
        if forced || lv.refines(side, min) {
            let shift = f.s - d - 1;
            let mut i = a;
            while i < b {
                let key = f.coords[i].map(|c| c >> shift);
                let mut j = i + 1;
                while j < b && f.coords[j].map(|c| c >> shift) == key {
                    j += 1;
                }
                descend(lv, dn, d + 1, i, j, out);
                i = j;
            }
        } else {
            draw_cell(lv, dn, &bounds, d, out);
            let shift = f.s - d - 1;
            let mut i = a;
            while i < b {
                let key = f.coords[i].map(|c| c >> shift);
                let mut j = i + 1;
                while j < b && f.coords[j].map(|c| c >> shift) == key {
                    j += 1;
                }
                let child_side = side / 2.0;
                let child_min: [f64; 3] =
                    std::array::from_fn(|k| lv.grid.root_min[k] + key[k] as f64 * child_side);
                let in_frustum = SpatialBounds::new(child_min, child_min.map(|v| v + child_side))
                    .is_ok_and(|c| lv.frustum.intersects(&c));
                out.non_monotone += u64::from(in_frustum && lv.refines(child_side, child_min));
                i = j;
            }
        }
        return;
    }
    // d == S: one cell. Several points on one position cannot separate: it stays.
    let mut depth = d;
    if f.counts[a] == 1 {
        while depth < MAX_CELL_DEPTH
            && lv.refines(
                lv.grid.root_side / (1_u64 << depth) as f64,
                centre.map(|v| v - lv.grid.root_side / (1_u64 << depth) as f64 / 2.0),
            )
        {
            depth += 1;
        }
    }
    if depth == d {
        draw_cell(lv, dn, &bounds, d, out);
        return;
    }
    let tail_side = lv.grid.root_side / (1_u64 << depth) as f64;
    let tail = SpatialBounds::new(
        centre.map(|v| v - tail_side / 2.0),
        centre.map(|v| v + tail_side / 2.0),
    );
    if let Ok(tail) = tail
        && lv.frustum.intersects(&tail)
    {
        draw_cell(lv, dn, &tail, depth, out);
    }
}

/// Depths that must be resident for a node that draws `depths` (bit set). r1: the D4 range, plus
/// the next one when the finest is at or below `S`. r2: the drawn depths plus the next finer one of
/// every drawn depth below `S`. r3: the drawn depths plus the next finer and the next coarser one
/// of every drawn depth. r4: r2 plus the next coarser one of every drawn depth.
fn resident_depths(depths: u64, s: u8, residency: u8) -> Vec<u8> {
    if depths == 0 {
        return Vec::new();
    }
    let lo = depths.trailing_zeros() as u8;
    let hi = 63 - depths.leading_zeros() as u8;
    let mut set = depths;
    let below_s = depths & !((1_u64 << s) | ((1_u64 << s) - 1));
    match residency {
        1 => {
            for d in lo..=hi {
                if d <= s {
                    set |= 1 << d;
                }
            }
            if hi >= s {
                set |= 1 << (hi + 1).min(MAX_CELL_DEPTH);
            }
        }
        2 => set |= below_s << 1,
        3 => set |= (depths << 1) | (depths >> 1),
        _ => set |= (below_s << 1) | (depths >> 1),
    }
    (0..=MAX_CELL_DEPTH).filter(|d| set >> d & 1 == 1).collect()
}

/// Packed bytes of one (node, depth): cells from the `S` level (shifted right above it, left below).
fn level_bytes(f: &mut FineNode, grid: &CellGrid, n: &NodeData, id: NodeId, depth: u8) -> u64 {
    if let Some(bytes) = f.bytes.get(&depth) {
        return *bytes;
    }
    let mut coords: Vec<[u64; 3]> = if depth <= f.s {
        f.coords
            .iter()
            .map(|c| c.map(|v| v >> (f.s - depth)))
            .collect()
    } else {
        f.coords
            .iter()
            .map(|c| c.map(|v| v << (depth - f.s)))
            .collect()
    };
    coords.dedup();
    let level = NodeCellLevel {
        depth,
        cells: coords
            .iter()
            .map(|coord| NodeCell {
                coord: *coord,
                rgb_sum: [0; 3],
                intensity_sum: 0.0,
                intensity_count: 0,
                count: 1,
                classification: -1,
                z_min: 0.0,
                z_max: 0.0,
            })
            .collect(),
        octant_offsets: None,
    };
    let bytes = pack_level(grid, n.bounds, id, &level).pages.len() as u64 * PAGE_BYTES;
    f.bytes.insert(depth, bytes);
    bytes
}

struct ResidentLevel {
    node: u64,
    depth: u8,
    bytes: u64,
    on_demand: bool,
}

#[derive(Default)]
struct LodCase {
    drawn: u64,
    over: u64,
    deepest: u8,
    over_1p5: u64,
    beyond_chain: u64,
    below_s: u64,
    resident: Vec<ResidentLevel>,
    resident_bytes: u64,
    on_demand_bytes: u64,
    /// Same accounting for the whole levels `octant_tight` keeps resident (`required_levels`).
    baseline_bytes: u64,
    /// Bytes if only the drawn depths and the next one below `S` were resident (no depths in
    /// between): a lower bound that shows how much of D4 is the contiguous range.
    drawn_only_bytes: u64,
    r2_bytes: u64,
    r3_bytes: u64,
    r4_bytes: u64,
    /// Cut nodes whose points could not be separated by the library: not measured.
    unbuilt: usize,
    non_monotone: u64,
}

fn cell_lod_case(
    lv: &LodView,
    entries: &[CellCutEntry],
    cache: &HashMap<NodeId, NodeData>,
    fine: &mut HashMap<NodeId, FineNode>,
) -> LodCase {
    let mut case = LodCase::default();
    for entry in entries {
        let n = &cache[&entry.node];
        let Some(f) = fine.get(&entry.node) else {
            case.unbuilt += 1;
            continue;
        };
        let mask = entry.mask.iter().any(|w| *w != 0).then_some(&entry.mask);
        let dn = DescentNode {
            node: n,
            fine: f,
            mask,
        };
        let first = n.levels[0].depth;
        let mut out = NodeDraw::default();
        let shift = f.s - first;
        let mut i = 0;
        while i < f.coords.len() {
            let key = f.coords[i].map(|c| c >> shift);
            let mut j = i + 1;
            while j < f.coords.len() && f.coords[j].map(|c| c >> shift) == key {
                j += 1;
            }
            descend(lv, &dn, first, i, j, &mut out);
            i = j;
        }
        case.drawn += out.drawn;
        case.over += out.over;
        case.over_1p5 += out.over_1p5;
        case.beyond_chain += out.beyond_chain;
        case.below_s += out.below_s;
        case.non_monotone += out.non_monotone;
        if out.depths != 0 {
            case.deepest = case.deepest.max(63 - out.depths.leading_zeros() as u8);
        }
        let s = f.s;
        let f = fine.get_mut(&entry.node).expect("checked above");
        for depth in (0..=MAX_CELL_DEPTH).filter(|d| out.depths >> d & 1 == 1) {
            case.drawn_only_bytes += level_bytes(f, lv.grid, n, entry.node, depth);
        }
        for depth in entry.required_levels() {
            case.baseline_bytes += level_bytes(f, lv.grid, n, entry.node, depth);
        }
        for (r, total) in [
            (2, &mut case.r2_bytes),
            (3, &mut case.r3_bytes),
            (4, &mut case.r4_bytes),
        ] {
            for depth in resident_depths(out.depths, s, r) {
                *total += level_bytes(f, lv.grid, n, entry.node, depth);
            }
        }
        for depth in resident_depths(out.depths, s, lv.opts.residency) {
            let bytes = level_bytes(f, lv.grid, n, entry.node, depth);
            let on_demand = !n.levels.iter().any(|l| l.depth == depth);
            case.resident_bytes += bytes;
            case.on_demand_bytes += if on_demand { bytes } else { 0 };
            case.resident.push(ResidentLevel {
                node: entry.node.get(),
                depth,
                bytes,
                on_demand,
            });
        }
    }
    case
}

struct LodRecord {
    dataset: String,
    pose: &'static str,
    target: f64,
    mode: &'static str,
    vs_cpu: Option<f64>,
    resident_bytes: u64,
}

/// Kill criteria of phase 0a: resident bytes above 460 MiB at >= 2 px or 512 MiB at 1 px in any
/// pose; instances above 2.5x the CPU path at 1 px or 2x at 2-8 px in the grazing pose.
fn lod_verdicts(records: &[LodRecord]) -> Vec<String> {
    let mut lines = Vec::new();
    for (mode, _) in LOD_MODES {
        let rows: Vec<&LodRecord> = records.iter().filter(|r| r.mode == mode).collect();
        if rows.is_empty() {
            continue;
        }
        let over: Vec<String> = rows
            .iter()
            .filter(|r| {
                let limit = if r.target == 1.0 { 512.0 } else { 460.0 } * MIB;
                r.resident_bytes as f64 > limit
            })
            .map(|r| {
                format!(
                    "{} {} {}px: {:.1} MiB",
                    r.dataset,
                    r.pose,
                    r.target,
                    r.resident_bytes as f64 / MIB
                )
            })
            .collect();
        lines.push(format!(
            "{}\t[cell_lod {mode}] resident bytes <= 460 MiB (>= 2 px) / 512 MiB (1 px) in every pose{}",
            if over.is_empty() { "PASS" } else { "FAIL" },
            if over.is_empty() { String::new() } else { format!(": {}", over.join("; ")) }
        ));
        let graze: Vec<&&LodRecord> = rows.iter().filter(|r| r.pose == "grazing").collect();
        let failing: Vec<String> = graze
            .iter()
            .filter_map(|r| {
                let limit = if r.target == 1.0 { 2.5 } else { 2.0 };
                r.vs_cpu.filter(|ratio| *ratio > limit).map(|ratio| {
                    format!("{} {}px: {ratio:.3}x (limit {limit})", r.dataset, r.target)
                })
            })
            .collect();
        let unknown = graze.iter().any(|r| r.vs_cpu.is_none());
        let verdict = if !failing.is_empty() {
            "FAIL"
        } else if unknown || graze.is_empty() {
            "UNKNOWN"
        } else {
            "PASS"
        };
        lines.push(format!(
            "{verdict}\t[cell_lod {mode}] drawn instances <= 2.5x (1 px) / 2x (2-8 px) the CPU path in the grazing pose{}",
            if failing.is_empty() { String::new() } else { format!(": {}", failing.join("; ")) }
        ));
    }
    lines
}
