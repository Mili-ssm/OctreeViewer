//! R3 Task 4 native coverage spike: what the per-node cell path would draw versus the CPU voxel
//! path, for real v7.5 datasets and without a GPU. Empty pixels need the Task 7 renderer and are
//! out of scope. Throwaway: not part of `scripts/check.sh`.
//!
//! Usage: coverage_spike INPUT.oct [MAX_CUT_NODES=0]   (0 = no limit; larger cuts skip the case)
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    error::Error,
    fs::File,
    path::Path,
    time::Instant,
};

use viewer_core::{
    BlockId, DEFAULT_LOD_PRIORITY_BYTES, DatasetSource, LodSelectionConfig, NodeId, PointBlock,
    ResidentBlockSource, SpatialBounds,
};
use viewer_format::{FormatReader, OctreeReader, PagedReader};
use viewer_render_wgpu::{
    CellCutNode, CellGrid, NodeCellSource, OrbitCamera, PointNodeCellSource, RendererConfig,
    SceneBounds, ViewProjection, VoxelBuildLimits, build_voxel_scene_with_limits, cell_cut,
    representative_depth,
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

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Level {
    depth: u8,
    /// First cell of the node at this depth (same rounding as the cell builder).
    lo: [u64; 3],
    /// Cell coordinate minus `lo`.
    rel: Vec<[u32; 3]>,
}

struct NodeData {
    bounds: SpatialBounds,
    is_leaf: bool,
    rep: u8,
    rep_lo: [u64; 3],
    levels: Vec<Level>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    Pyramid,
    Only256,
}

impl Variant {
    fn name(self) -> &'static str {
        match self {
            Self::Pyramid => "pyramid",
            Self::Only256 => "only256",
        }
    }
    /// Indices into `levels` kept as stored.
    fn kept(self, node: &NodeData) -> Vec<usize> {
        match self {
            Self::Only256 if !node.is_leaf => vec![node.levels.len() - 1],
            _ => (0..node.levels.len()).collect(),
        }
    }
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
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() || args.len() > 2 {
        return Err("usage: coverage_spike INPUT.oct [MAX_CUT_NODES=0]".into());
    }
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
    if reader.format_version().uses_pages() {
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
    let centre: [f64; 3] = std::array::from_fn(|a| (root_local.min[a] + root_local.max[a]) / 2.0);
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
    let leaf_eye: [f64; 3] =
        std::array::from_fn(|a| (leaf_local.min()[a] + leaf_local.max()[a]) / 2.0 * visual_scale);
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
    let poses = [
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
    let mut cache_bytes = 0_usize;
    let mut coarse_first_level_nodes = 0_usize;
    let mut cases: Vec<Case> = Vec::new();
    let mut rows = Vec::new();
    let mut stop_bytes: Vec<String> = Vec::new();
    let mut stop_grazing: Vec<String> = Vec::new();
    let mut na_grazing: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let source = PointNodeCellSource::default();
    let started_all = Instant::now();

    for (pose_index, pose) in poses.iter().enumerate() {
        let view = ViewProjection::new(pose.camera, VIEWPORT, visual_scale, build_limits);
        // Dataset-unit frustum for the selector; unscaled-local one for cells and bounds.
        let wf = view.spatial_frustum(origin).ok_or("no frustum")?;
        let local_frustum = view.spatial_frustum([0.0; 3]).ok_or("no frustum")?;
        for &target in &TARGETS {
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
                        levels.push(Level {
                            depth: level.depth,
                            lo,
                            rel,
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

                // CPU baseline.
                let decoded_estimate = selection.point_count() * POINT_BYTES;
                let cpu = if decoded_estimate > MAX_DECODED_BYTES {
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
                    .map(|scene| scene.scene.instance_count())
                    .map_err(|e| format!("NA({e:?})"))
                };

                for variant in [Variant::Pyramid, Variant::Only256] {
                    let t0 = Instant::now();
                    let node_info = |id: NodeId| {
                        cache.get(&id).map(|n| {
                            let stored: Vec<u8> =
                                variant.kept(n).iter().map(|&i| n.levels[i].depth).collect();
                            CellCutNode {
                                bounds: n.bounds,
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
                    let (mut total, mut on_screen, mut stages) = (0_u64, 0_u64, 0_usize);
                    for entry in &entries {
                        stages += usize::from(entry.stage);
                        let Some(depth) = entry.drawn else { continue };
                        let n = &cache[&entry.node];
                        let level = n
                            .levels
                            .iter()
                            .find(|l| l.depth == depth)
                            .ok_or("drawn level not stored")?;
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
                            let min: [f64; 3] = std::array::from_fn(|a| {
                                grid.root_min[a]
                                    + (level.lo[a] + u64::from(rel[a])) as f64 * cell_side
                            });
                            let max = min.map(|v| v + cell_side);
                            if let Ok(b) = SpatialBounds::new(min, max)
                                && local_frustum.intersects(&b)
                            {
                                on_screen += 1;
                            }
                        }
                    }
                    let build_ms = t0.elapsed().as_secs_f64() * 1000.0;
                    let bytes12 = total * 12;
                    let (cpu_text, ratio_text) = match &cpu {
                        Ok(count) => (
                            count.to_string(),
                            format!("{:.3}", on_screen as f64 / (*count).max(1) as f64),
                        ),
                        Err(text) => (text.clone(), "NA".to_string()),
                    };
                    if bytes12 > POOL_BYTES {
                        stop_bytes.push(format!(
                            "{} {target}px budget {budget} {}: {bytes12} B",
                            pose.name,
                            variant.name()
                        ));
                    }
                    if pose.name == "grazing" {
                        match &cpu {
                            Ok(count) if on_screen > 2 * (*count as u64) => stop_grazing.push(format!(
                                "{target}px budget {budget} {}: on-screen {on_screen} vs CPU {count}",
                                variant.name()
                            )),
                            Ok(_) => {}
                            Err(_) => na_grazing.push(format!(
                                "{target}px budget {budget} {}",
                                variant.name()
                            )),
                        }
                    }
                    rows.push(format!(
                        "{}\t{}\t{target}\t{budget}\t{}\t{}\t{}\t{total}\t{on_screen}\t{bytes12}\t{}\t{cpu_text}\t{ratio_text}\t{stages}\t{build_ms:.1}",
                        args[0].to_string_lossy(),
                        pose.name,
                        variant.name(),
                        cut.len(),
                        selection.point_count(),
                        total * 16,
                    ));
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
        "dataset_file\tpose\ttarget_px\tbudget\tvariant\tcut_nodes\tcut_points\tcells_total\tcells_on_screen\tbytes12\tbytes16\tcpu_instances\tratio_on_screen_vs_cpu\tstage_entries\tbuild_ms"
    );
    for row in &rows {
        println!("{row}");
    }

    // Transitions: mixed cut = previous union next, all resident, next view and target.
    let mut index: BTreeMap<(u64, usize, u64), usize> = BTreeMap::new();
    for (i, c) in cases.iter().enumerate() {
        index.insert((c.budget, c.pose, c.target.to_bits()), i);
    }
    let mut transitions: Vec<(String, usize, usize)> = Vec::new();
    for &budget in &budgets {
        for pose in 0..poses.len() {
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
    let mut counts = [(0_usize, 0_usize); 2];
    let mut failing: [Vec<String>; 2] = [Vec::new(), Vec::new()];
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
        for (v, variant) in [Variant::Pyramid, Variant::Only256].into_iter().enumerate() {
            let node_info = |id: NodeId| {
                cache.get(&id).map(|n| {
                    let stored: Vec<u8> =
                        variant.kept(n).iter().map(|&i| n.levels[i].depth).collect();
                    CellCutNode {
                        bounds: n.bounds,
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
    let expected_transitions =
        budgets.len() * (poses.len() * (TARGETS.len() - 1) + (poses.len() - 1) * TARGETS.len());
    println!(
        "# transitions evaluated per variant: {} of {expected_transitions} expected (the rest involve skipped cases)",
        transitions.len()
    );
    for (v, name) in ["pyramid", "only256"].into_iter().enumerate() {
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
    let skipped_text = if skipped.is_empty() {
        String::new()
    } else {
        format!(" [skipped cases: {}]", skipped.join("; "))
    };
    let (verdict, detail) = if !stop_bytes.is_empty() {
        ("FAIL", format!(": {}", stop_bytes.join("; ")))
    } else if !skipped.is_empty() {
        ("UNKNOWN", String::new())
    } else {
        ("PASS", String::new())
    };
    println!("{verdict}\tcut bytes (12 B) <= 256 MiB in every case{detail}{skipped_text}");
    let skipped_grazing: Vec<_> = skipped
        .iter()
        .filter(|c| c.starts_with("grazing"))
        .cloned()
        .collect();
    let (verdict, detail) = if !stop_grazing.is_empty() {
        ("FAIL", format!(": {}", stop_grazing.join("; ")))
    } else if !na_grazing.is_empty() || !skipped_grazing.is_empty() {
        (
            "UNKNOWN",
            format!(
                ": no CPU count for rows [{}], skipped [{}]",
                na_grazing.join("; "),
                skipped_grazing.join("; ")
            ),
        )
    } else {
        ("PASS", String::new())
    };
    println!("{verdict}\ton-screen instances <= 2x CPU instances in the grazing pose{detail}");
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
    eprintln!("total wall {:.1}s", started_all.elapsed().as_secs_f64());
    Ok(())
}
