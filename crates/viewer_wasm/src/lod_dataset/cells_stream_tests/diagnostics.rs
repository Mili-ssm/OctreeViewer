//! ADR 0055 plan, tasks 2 and 7a: what the twin wants per node against the node range, what the eye
//! node holds and how late detail arrives when zooming in (diagnostics, mostly `#[ignore]`).

use super::*;

const DEMO_VIEWPORT: [u32; 2] = [870, 750];
/// Frames of the wheel and drag deltas of the web demo: a wheel step is -0.3 of zoom, a pointer
/// drag is normalised by the canvas height.
const WHEEL_STEP: f64 = -0.3;

/// The camera of a pose of the web demo, from the initial fit: P1 = right drag 150 px upward then
/// 3 wheel steps; P2 = P1 + 6 wheel steps + right drag 80; P3 = P2 + 4 wheel steps; P4 = 8 wheel
/// steps + right drag 120; P5 = P2 + 6 half wheel steps. The drag direction (upward, as in P4) and
/// the order inside a pose are assumptions: zoom and orbit commute except at the pitch clamp.
fn demo_pose_camera(fit: OrbitCamera, pose: &str) -> OrbitCamera {
    let mut camera = fit;
    let drag = |camera: &mut OrbitCamera, px: f64| assert!(camera.orbit(0.0, -px / 750.0));
    let wheel = |camera: &mut OrbitCamera, steps: usize, k: f64| {
        (0..steps).for_each(|_| assert!(camera.zoom(WHEEL_STEP * k)));
    };
    match pose {
        "P1" => {
            drag(&mut camera, 150.0);
            wheel(&mut camera, 3, 1.0);
        }
        "P2" | "P3" | "P5" => {
            drag(&mut camera, 150.0);
            wheel(&mut camera, 9, 1.0);
            drag(&mut camera, 80.0);
            match pose {
                "P3" => wheel(&mut camera, 4, 1.0),
                "P5" => wheel(&mut camera, 6, 0.5),
                _ => {}
            }
        }
        "P4" => {
            wheel(&mut camera, 8, 1.0);
            drag(&mut camera, 120.0);
        }
        other => panic!("unknown pose {other}"),
    }
    camera
}

/// The sample settled at `pose` (from the initial fit, like `p4_border_levels`).
fn settled_sample_pose(pose: &str) -> (Harness, ViewProjection) {
    let (mut h, origin, scale) = sample_harness();
    let local = |p: [f64; 3]| -> [f64; 3] { std::array::from_fn(|a| (p[a] - origin[a]) * scale) };
    let data = h.dataset.metadata.hierarchy().stats().data_bounds;
    let mut fit = OrbitCamera::default();
    assert!(fit.fit(
        SceneBounds {
            min: local(data.min()),
            max: local(data.max())
        },
        DEMO_VIEWPORT
    ));
    let initial = ViewProjection::new(fit, DEMO_VIEWPORT, scale, VoxelBuildLimits::default());
    let view = ViewProjection::new(
        demo_pose_camera(fit, pose),
        DEMO_VIEWPORT,
        scale,
        VoxelBuildLimits::default(),
    );
    h.settle(&initial, 3, 5000);
    h.camera_moved();
    h.settle(&view, 3, 5000);
    (h, view)
}

/// `gpu_input_digest` of the settled cut at each demo pose, golden. It hashes what
/// the GPU receives per node (active-depth bits, mask, node depth), not the per-octant fields of
/// the entries, so it must not change while those fields are replaced by node-level ones. The
/// cut goldens of `cells/cut_tests.rs` are pure cuts without a pool, so they emit no active bits:
/// these poses stand in for them.
const GPU_INPUT_GOLDEN: &[(&str, u64)] = &[
    ("P1", 0x2f2f_4f1a_b372_490e),
    ("P2", 0xe0f8_8c8d_0cc1_68e4),
    ("P3", 0xdc13_ecd0_6a84_91cc),
    ("P4", 0xdfa4_469d_d7df_e145),
    ("P5", 0xe105_c88c_8ae1_50cc),
];

#[test]
fn the_gpu_input_at_the_sample_poses_is_pinned() {
    let mut actual = Vec::new();
    for pose in ["P1", "P2", "P3", "P4", "P5"] {
        let (h, _) = settled_sample_pose(pose);
        actual.push((pose.to_owned(), h.totals.gpu_digest_last));
    }
    let matches = actual.len() == GPU_INPUT_GOLDEN.len()
        && actual
            .iter()
            .zip(GPU_INPUT_GOLDEN)
            .all(|(a, g)| a.0 == g.0 && a.1 == g.1);
    if !matches {
        use std::fmt::Write as _;
        let table: String = actual
            .iter()
            .fold(String::new(), |mut table, (name, hash)| {
                let _ = writeln!(table, "    (\"{name}\", {hash:#018x}),");
                table
            });
        panic!("GPU input table changed; actual:\n{table}");
    }
}

// --- B/C/D: the twin-wanted depth range of every node of the cut -----------------------------

/// Screen lattice step in px of the samples (a drawn cell is at least ~4 px wide).
const LATTICE_PX: usize = 6;
/// Geometric step of the samples along each ray (a cell changes by one depth per doubling).
const DEPTH_RATIO: f64 = 1.1;
/// px tolerance for f32 (twin) against f64 (probes) in the containment property.
const PX_EPSILON: f32 = 0.05;

/// Slab of the ray `d * t` inside the box `[min, max]` (eye-relative): `(t_in, t_out)`.
fn ray_slab(d: [f64; 3], min: [f64; 3], max: [f64; 3]) -> Option<(f64, f64)> {
    let (mut lo, mut hi) = (1e-4_f64, f64::INFINITY);
    for a in 0..3 {
        if d[a].abs() < 1e-12 {
            if min[a] > 0.0 || max[a] < 0.0 {
                return None;
            }
            continue;
        }
        let (t1, t2) = (min[a] / d[a], max[a] / d[a]);
        lo = lo.max(t1.min(t2));
        hi = hi.min(t1.max(t2));
    }
    (lo <= hi).then_some((lo, hi))
}

/// Measures every node of the settled cell cut at `view` (see the section header for the rules).
fn twin_rows(h: &Harness, view: &ViewProjection, lattice_px: usize) -> Vec<NodeRow> {
    let ctx = h.cell_ctx.as_ref().expect("cell mode context");
    let hierarchy = h.dataset.metadata.hierarchy();
    let camera = view.camera();
    let scale = ctx.scale;
    let near = lod_near_depth_scaled(&camera);
    let data = hierarchy.stats().data_bounds;
    let eye_ds: [f64; 3] = std::array::from_fn(|a| ctx.eye[a] / scale + ctx.origin[a]);
    let [w, hgt] = ctx.viewport;
    let focal = f64::from(ctx.lod.focal);
    let f64v = |v: [f32; 3]| v.map(f64::from);
    let (forward, right, up) = (f64v(ctx.lod.forward), f64v(ctx.lod.right), f64v(ctx.lod.up));
    let targets = [
        ctx.lod.target_px,
        ctx.lod.target_px + PX_EPSILON,
        ctx.lod.target_px - PX_EPSILON,
    ];
    let mut rows = Vec::new();
    for (j, node) in h.cell_nodes.iter().enumerate() {
        let span = buildable_span(h, node.id).expect("blocks of the cut are cached");
        let mut row = NodeRow {
            id: node.id.get(),
            node_depth: node.lod.node_depth,
            span,
            points: hierarchy.node(node.id).unwrap().point_count(),
            eye_inside: (0..3).all(|a| eye_ds[a] >= node.min[a] && eye_ds[a] <= node.max[a]),
            range: h.last_levels[j].2,
            capped: h.frame_capped.contains(&node.id),
            wanted_max: h.last_entries[j].1.iter().flatten().copied().max(),
            ..NodeRow::default()
        };
        // node ∩ data, eye-relative scaled.
        let lo_ds: [f64; 3] = std::array::from_fn(|a| node.min[a].max(data.min()[a]));
        let hi_ds: [f64; 3] = std::array::from_fn(|a| node.max[a].min(data.max()[a]));
        if (0..3).any(|a| lo_ds[a] >= hi_ds[a]) {
            rows.push(row);
            continue;
        }
        let rmin: [f64; 3] =
            std::array::from_fn(|a| (lo_ds[a] - ctx.origin[a]) * scale - ctx.eye[a]);
        let rmax: [f64; 3] =
            std::array::from_fn(|a| (hi_ds[a] - ctx.origin[a]) * scale - ctx.eye[a]);
        let (mut tmin, mut tmax) = (u8::MAX, 0_u8);
        let (mut emin, mut emax) = (u8::MAX, 0_u8);
        let mut visit = |t: f64, d: [f64; 3], at: [usize; 2], row: &mut NodeRow| {
            if t < near {
                row.excluded_near += 1;
                return;
            }
            let rel: [f64; 3] = std::array::from_fn(|a| d[a] * t);
            if !ctx.on_screen(rel) {
                return;
            }
            let (found, crosses) = twin_depths(&ctx.lod, node, rel, span, targets);
            let depth = found[0];
            if crosses {
                row.excluded_cross += 1;
                return;
            }
            row.samples += 1;
            if depth < tmin {
                row.at_min = Some((t, at));
            }
            if depth > tmax || row.at_max.is_none() {
                row.at_max = Some((t, at));
            }
            tmin = tmin.min(depth);
            tmax = tmax.max(depth);
            emin = emin.min(found[1]);
            emax = emax.max(found[2]);
        };
        let columns: Vec<usize> = (0..w as usize)
            .step_by(lattice_px)
            .chain([w as usize - 1])
            .collect();
        let rows_px: Vec<usize> = (0..hgt as usize)
            .step_by(lattice_px)
            .chain([hgt as usize - 1])
            .collect();
        for &py in &rows_px {
            for &px in &columns {
                let (u, v) = (
                    (px as f64 - w / 2.0) / focal,
                    (hgt / 2.0 - py as f64) / focal,
                );
                let d: [f64; 3] = std::array::from_fn(|a| forward[a] + right[a] * u + up[a] * v);
                let Some((t_in, t_out)) = ray_slab(d, rmin, rmax) else {
                    continue;
                };
                let t_out = t_out.min(ctx.clip.1);
                let mut t = t_in;
                while t <= t_out {
                    visit(t, d, [px, py], &mut row);
                    t *= DEPTH_RATIO;
                }
                visit(t_out, d, [px, py], &mut row);
            }
        }
        if row.samples > 0 {
            row.twin = Some((tmin, tmax));
            row.twin_eps = Some((emin, emax));
        }
        // The shipped candidate range (`node_view_range`, ADR 0055 plan task 3) over node ∩ data.
        row.candidate = node_view_range(
            view,
            &h.dataset.cell_grid(),
            h.dataset.coordinate_origin(),
            SpatialBounds::new(lo_ds, hi_ds).ok(),
            span,
            f64::from(ctx.lod.target_px),
            h.driver.pool().min_side(),
        );
        rows.push(row);
    }
    rows
}

fn histogram(values: impl Iterator<Item = i32>) -> std::collections::BTreeMap<i32, usize> {
    let mut map = std::collections::BTreeMap::new();
    for v in values {
        *map.entry(v).or_insert(0) += 1;
    }
    map
}

fn print_twin_rows(label: &str, rows: &[NodeRow]) {
    println!("== {label}");
    println!(
        "node  nd buildable eye samples excl_near excl_cross | twin [min,max] | range [lo,hi] | \
         lo-twin_min twin_max-hi | candidate [lo_c,hi_c] twin_min-lo_c hi_c-twin_max"
    );
    for r in rows {
        let slack = |a: Option<(u8, u8)>, b: Option<(u8, u8)>| -> String {
            match (a, b) {
                (Some(t), Some(x)) => format!(
                    "{:>3} {:>3}",
                    i32::from(x.0) - i32::from(t.0),
                    i32::from(t.1) - i32::from(x.1)
                ),
                _ => "  -   -".into(),
            }
        };
        let cand = match (r.twin, r.candidate) {
            (Some(t), Some(c)) => format!(
                "{:>3} {:>3}",
                i32::from(t.0) - i32::from(c.0),
                i32::from(c.1) - i32::from(t.1)
            ),
            _ => "  -   -".into(),
        };
        println!(
            "{:>4} {:>3} {:>6} {:>3} {:>7} {:>9} {:>10} | {:?} | {:?} wmax {:?}{} | {} | {:?} {}",
            r.id,
            r.node_depth,
            format!("{}..{}", r.span.0, r.span.1),
            if r.eye_inside { "EYE" } else { "" },
            r.samples,
            r.excluded_near,
            r.excluded_cross,
            r.twin,
            r.range,
            r.wanted_max,
            if r.capped { " CAPPED" } else { "" },
            slack(r.twin, r.range),
            r.candidate,
            cand
        );
        if let (Some(a), Some(b)) = (r.at_min, r.at_max) {
            println!(
                "      twin min first reached at view depth {:.3} screen px {:?}; twin max at view depth {:.3} screen px {:?}",
                a.0, a.1, b.0, b.1
            );
        }
    }
}

fn summarise_twin_rows(label: &str, rows: &[NodeRow]) {
    for (scope, keep) in [("all nodes", false), ("nodes holding the eye", true)] {
        let set: Vec<&NodeRow> = rows
            .iter()
            .filter(|r| r.twin.is_some() && r.range.is_some() && (!keep || r.eye_inside))
            .collect();
        let t = |r: &&NodeRow| r.twin.unwrap();
        let g = |r: &&NodeRow| r.range.unwrap();
        let lo_defect = set.iter().filter(|r| g(r).0 > t(r).0).count();
        let hi_defect = set.iter().filter(|r| g(r).1 < t(r).1).count();
        println!(
            "-- {label} / {scope}: {} nodes with samples and a range; lo > twin_min: {lo_defect}; \
             hi < twin_max: {hi_defect}",
            set.len()
        );
        println!(
            "   lo - twin_min {:?} | twin_max - hi {:?}",
            histogram(set.iter().map(|r| i32::from(g(r).0) - i32::from(t(r).0))),
            histogram(set.iter().map(|r| i32::from(t(r).1) - i32::from(g(r).1)))
        );
        let cs: Vec<&&NodeRow> = set.iter().filter(|r| r.candidate.is_some()).collect();
        let c = |r: &&&NodeRow| r.candidate.unwrap();
        let contained = cs
            .iter()
            .filter(|r| c(r).0 <= t(r).0 && c(r).1 >= t(r).1)
            .count();
        let hi_within_one = cs
            .iter()
            .filter(|r| (0..=1).contains(&(i32::from(c(r).1) - i32::from(t(r).1))))
            .count();
        println!(
            "   candidate: contains the twin range in {contained}/{}; hi_c - twin_max in 0..=1 in \
             {hi_within_one}/{}",
            cs.len(),
            cs.len()
        );
        println!(
            "   twin_min - lo_c {:?} | hi_c - twin_max {:?}",
            histogram(cs.iter().map(|r| i32::from(t(r).0) - i32::from(c(r).0))),
            histogram(cs.iter().map(|r| i32::from(c(r).1) - i32::from(t(r).1)))
        );
        // Pages: the depths a range holds beyond the twin's span, and the totals.
        let (mut twin_pages, mut today_pages, mut cand_pages, mut extra_pages) = (0, 0, 0, 0);
        for r in &cs {
            let (tl, th) = t(r);
            let (cl, ch) = c(r);
            twin_pages += pages_of(r, tl..=th);
            cand_pages += pages_of(r, cl.min(tl)..=ch.max(th));
            let (gl, gh) = g(r);
            today_pages += pages_of(r, gl.min(tl)..=gh.max(th));
            if cl < tl {
                extra_pages += pages_of(r, cl..=tl - 1);
            }
            if ch > th {
                extra_pages += pages_of(r, th + 1..=ch);
            }
        }
        println!(
            "   pages (upper-bound estimate, nodes with a candidate): twin span {twin_pages}; \
             today's range (union with the twin span) {today_pages}; candidate (union) \
             {cand_pages}; extra of the candidate slack {extra_pages}"
        );
    }
}

/// Diagnostic (prints, asserts nothing): per node of the applied cut at the poses P1-P5 of the
/// web demo on the built-in sample, the depth range the twin of the GPU rule wants over
/// node ∩ data bounds ∩ view frustum if every buildable depth were resident, against today's
/// `range`, and against the shipped candidate `node_view_range` (`lo_c`, `hi_c`: on-axis cells at
/// the farthest and nearest view depth of the region, the nearest with the px target divided by
/// the worst off-axis stretch of the frustum).
///
/// Sampling: a screen lattice of `LATTICE_PX` px (`TWIN_LATTICE_PX` overrides it) with a ray per
/// node; along each ray a point every `DEPTH_RATIO` in view depth plus both ends of the slab.
/// Excluded: points nearer than `lod_near_depth` (`excl_near`), and points whose twin cell at the
/// wanted depth crosses the eye plane (`excl_cross`, `edge_px == +inf`).
/// Run: `cargo test --offline --profile fast -p viewer_wasm --lib twin_wanted_range_vs_node_range -- --ignored --nocapture`.
#[test]
#[ignore = "diagnostic: prints the twin-wanted depth range per node against the node range"]
fn twin_wanted_range_vs_node_range() {
    let lattice = std::env::var("TWIN_LATTICE_PX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(LATTICE_PX);
    let poses = std::env::var("TWIN_POSES").unwrap_or_else(|_| "P1,P2,P3,P4,P5".into());
    for pose in poses.split(',') {
        let (h, view) = settled_sample_pose(pose);
        let camera = view.camera();
        println!(
            "######## {pose}: eye {:.3?} target {:.3?} lattice {lattice} px, depth ratio {DEPTH_RATIO}, \
             lod_near_depth {:.4} (scaled)",
            camera.position(),
            camera.target(),
            lod_near_depth_scaled(&camera)
        );
        let rows = twin_rows(&h, &view, lattice);
        print_twin_rows(pose, &rows);
        summarise_twin_rows(pose, &rows);
    }
}

/// Violations of the node range property (ADR 0055, D2) at the poses `poses`, over node ∩ data ∩
/// frustum, excluding points nearer than `lod_near_depth` and cells that cross the eye plane:
/// `lo` side (`lo <= twin min` with a `PX_EPSILON` tolerance on the px target for f32 against f64,
/// and at most one level above `lo`) when `lo_side`, `hi` side (`hi >= twin max`, at most one level
/// below it) when not.
fn range_violations(poses: &[&str], lo_side: bool) -> Vec<String> {
    let mut violations = Vec::new();
    for pose in poses {
        let (h, view) = settled_sample_pose(pose);
        for r in twin_rows(&h, &view, LATTICE_PX) {
            let (Some((emin, emax)), Some((lo, hi)), Some((tmin, tmax))) =
                (r.twin_eps, r.range, r.twin)
            else {
                continue;
            };
            if lo_side && lo > emin {
                violations.push(format!("{pose} node {}: lo {lo} > twin min {emin}", r.id));
            }
            if !lo_side && hi < emax {
                violations.push(format!("{pose} node {}: hi {hi} < twin max {emax}", r.id));
            }
            if lo_side && tmin > lo + 1 {
                violations.push(format!(
                    "{pose} node {}: lo {lo} is >1 below twin min {tmin}",
                    r.id
                ));
            }
            if !lo_side && hi > tmax + 1 {
                violations.push(format!(
                    "{pose} node {}: hi {hi} is >1 above twin max {tmax}",
                    r.id
                ));
            }
        }
    }
    violations
}

fn assert_no_violations(violations: &[String]) {
    assert!(
        violations.is_empty(),
        "{} violations:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// `lo` of the node range contains what the twin of the GPU rule wants, within one level (ADR 0055
/// plan, task 4: `lo` comes from `node_view_range`).
#[test]
#[ignore = "slow (about 3 minutes of twin sampling); P4 alone runs in the next test"]
fn the_node_lo_is_not_finer_than_what_the_twin_wants_within_one_level() {
    assert_no_violations(&range_violations(&["P1", "P2", "P3", "P4", "P5"], true));
}

/// The `lo` property at P4, where `node_far_depth` used to put node 14 three levels too fine.
#[test]
fn the_node_lo_at_p4_is_not_finer_than_what_the_twin_wants_within_one_level() {
    assert_no_violations(&range_violations(&["P4"], true));
}

/// `hi` of the node range contains what the twin wants, within one level. Paused: near the eye the
/// faithful `hi` is the finest level in empty air, a budget question (plan task 5).
#[test]
#[ignore = "paused: `hi` is not changed until plan task 5 decides what bounds it near the eye"]
fn the_node_hi_contains_what_the_twin_wants_within_one_level() {
    assert_no_violations(&range_violations(&["P1", "P2", "P3", "P4", "P5"], false));
}

// ---- Task 5a: cheaper sources of the nearest occupied view depth (no wiring) -----------------

/// Per-source tallies of the comparison with the reference and with `twin_data_max`.
#[derive(Default)]
struct SourceStats {
    violations: Vec<String>,
    /// `hi(source) - hi(ref)` and `hi(source) - twin_data_max`, over all nodes and the eye nodes.
    vs_ref: [Vec<i32>; 2],
    vs_tdm: [Vec<i32>; 2],
    at_hi_c: [usize; 2],
    nodes: [usize; 2],
}

impl SourceStats {
    #[expect(
        clippy::too_many_arguments,
        reason = "the parameters are independent inputs of one computation; a parameter struct would only rename them"
    )]
    fn add(
        &mut self,
        pose: &str,
        id: u64,
        eye: bool,
        hi: Option<u8>,
        reference: Option<u8>,
        tdm: u8,
        hi_c: u8,
    ) {
        let Some(hi) = hi else {
            self.violations
                .push(format!("{pose} node {id}: no hi (no geometry in view)"));
            return;
        };
        if hi < tdm {
            self.violations
                .push(format!("{pose} node {id}: hi {hi} < twin_data_max {tdm}"));
        }
        for group in std::iter::once(0).chain(eye.then_some(1)) {
            self.vs_tdm[group].push(i32::from(hi) - i32::from(tdm));
            if let Some(r) = reference {
                self.vs_ref[group].push(i32::from(hi) - i32::from(r));
            }
            self.at_hi_c[group] += usize::from(hi >= hi_c);
            self.nodes[group] += 1;
        }
    }

    fn print(&self, name: &str) {
        let hist = |v: &[i32]| format!("{:?}", histogram(v.iter().copied()));
        println!(
            "== SOURCE {name}: node-poses {} (eye {}); violations {}; at hi_c {} (eye {})\n   \
             hi-ref all {} | eye {}\n   hi-tdm all {} | eye {}",
            self.nodes[0],
            self.nodes[1],
            self.violations.len(),
            self.at_hi_c[0],
            self.at_hi_c[1],
            hist(&self.vs_ref[0]),
            hist(&self.vs_ref[1]),
            hist(&self.vs_tdm[0]),
            hist(&self.vs_tdm[1]),
        );
        for v in &self.violations {
            println!("   VIOLATION {v}");
        }
    }
}

/// Oscillation risk of the sources that depend on residency (a, b), for an eye node: the bound
/// with the levels resident today against the bound with the levels finer than the reference
/// `hi_d` filtered out, and the sequence `ceiling -> hi(source of the levels <= ceiling)` from
/// the reference `hi_d` (a cycle between two values means the bound and the residency chase each
/// other).
fn print_stability_probe(
    h: &Harness,
    view: &ViewProjection,
    j: usize,
    row: &NodeRow,
    d: &DataBound,
    pose: &str,
) {
    let show = |v: Option<u8>| v.map_or("-".to_string(), |x| x.to_string());
    let today = source_his(h, view, j, row, d, None);
    let Some(ref_hi) = d.hi_d else { return };
    let filtered = source_his(h, view, j, row, d, Some(ref_hi));
    println!(
        "PROBE {pose} node {} EYE finest resident {} ref hi_d {ref_hi}: today a.raw {} a.clip {} b.raw {} b.clip {} | levels <= ref hi_d only: a.raw {} a.clip {} b.raw {} b.clip {}",
        row.id,
        show(d.finest),
        show(today[1]),
        show(today[2]),
        show(today[3]),
        show(today[4]),
        show(filtered[1]),
        show(filtered[2]),
        show(filtered[3]),
        show(filtered[4]),
    );
    // Residency as if the bound were wired and only today's range were resident: levels up to
    // today's `hi` and up to today's `lo`.
    if let Some((lo, hi_now)) = row.range {
        for (label, cap) in [("<= today hi", hi_now), ("<= today lo", lo)] {
            let f = source_his(h, view, j, row, d, Some(cap));
            println!(
                "PROBE {pose} node {} levels {label} ({cap}): a.raw {} a.clip {} b.raw {} b.clip {}",
                row.id,
                show(f[1]),
                show(f[2]),
                show(f[3]),
                show(f[4])
            );
        }
    }
    for (slot, name) in [(1, "a.raw"), (2, "a.clip")] {
        let mut ceiling = ref_hi;
        let mut seq = vec![ceiling];
        for _ in 0..6 {
            let Some(next) = source_his(h, view, j, row, d, Some(ceiling))[slot] else {
                break;
            };
            seq.push(next);
            ceiling = next;
        }
        println!(
            "PROBE {pose} node {} {name} ceiling sequence {seq:?}",
            row.id
        );
    }
}

/// Diagnostic (prints, asserts nothing), OQ-16: bound the fine end of the node range by the data.
/// `hi_d` = the candidate's `hi` with the nearest view depth replaced by `max(lod_near_depth,
/// nearest face of an occupied cell of the node's finest resident level inside the frustum)`.
/// Compared per node with `twin_data_max` (the twin's maximum at the occupied cells: centres, and
/// the point of each cell nearest the eye), today's `hi`, `hi_c`, and in estimated pages
/// (`estimate_pages_like_the_pool`) of the run `lo..=X` for X in {today's hi, `twin_data_max`,
/// `hi_d`, `hi_c`}, with `lo` today's `lo`. Also prints the pages the pool holds for the node.
/// Run: `TWIN_POSES=P2,P4 cargo test --offline --profile fast -p viewer_wasm --lib data_bounded_hi -- --ignored --nocapture`.
#[test]
#[ignore = "diagnostic: prints a data-bounded fine end of the node range against the others"]
fn data_bounded_hi_vs_node_range() {
    let lattice = std::env::var("TWIN_LATTICE_PX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(LATTICE_PX);
    let poses = std::env::var("TWIN_POSES").unwrap_or_else(|_| "P1,P2,P3,P4,P5".into());
    let (mut slack_d, mut d_vs_c, mut violations) = (Vec::new(), Vec::new(), Vec::new());
    let mut stats: Vec<SourceStats> = (0..SOURCES.len()).map(|_| SourceStats::default()).collect();
    for pose in poses.split(',') {
        let (h, view) = settled_sample_pose(pose);
        let rows = twin_rows(&h, &view, lattice);
        let pool = h.driver.pool().resident_pages();
        println!("######## {pose}: pool resident pages {pool}");
        println!(
            "node EYE fin cells | hi_now wmax twin_max data_max cell_max hi_c hi_d (hi_c rst) | \
             hi_d-dmax hi_d-cmax hi_c-hi_d | near_occ | pages now/dmax/hi_d/hi_c | resident"
        );
        let mut tot = [0_usize; 5];
        let mut eye_tot = [0_usize; 5];
        let mut src_pages = [[0_usize; 2]; 9];
        for (j, row) in rows.iter().enumerate() {
            let d = data_bound(&h, &view, j, row, true);
            let show = |v: Option<u8>| v.map_or("-".to_string(), |x| x.to_string());
            let (Some((lo, hi)), Some((_, hi_c)), Some(hi_d), Some(dmax)) =
                (row.range, row.candidate, d.hi_d, d.data_max)
            else {
                println!(
                    "{:>4} {:>3} {:>3} {:>5} | no data in view or no range/candidate | resident {}",
                    row.id,
                    if row.eye_inside { "EYE" } else { "" },
                    show(d.finest),
                    d.in_view,
                    d.resident_pages
                );
                continue;
            };
            let cmax = d.cell_max.unwrap_or(dmax);
            let run = |x: u8| pages_of(row, lo.min(x)..=x);
            let cols = [run(hi), run(dmax), run(hi_d), run(hi_c), d.resident_pages];
            for (t, c) in tot.iter_mut().zip(cols) {
                *t += c;
            }
            if row.eye_inside {
                for (t, c) in eye_tot.iter_mut().zip(cols) {
                    *t += c;
                }
            }
            let (sd, sc) = (
                i32::from(hi_d) - i32::from(dmax),
                i32::from(hi_d) - i32::from(cmax),
            );
            slack_d.push(sd.min(sc));
            d_vs_c.push(i32::from(hi_c) - i32::from(hi_d));
            if sd < 0 || sc < 0 {
                violations.push(format!(
                    "{pose} node {}: hi_d {hi_d} < data_max {dmax} / cell_max {cmax}",
                    row.id
                ));
            }
            println!(
                "{:>4} {:>3} {:>3} {:>5} | {hi:>3} {:>4} {} {dmax:>8} {cmax:>8} {hi_c:>4} {hi_d:>4} ({}) | \
                 {sd:>3} {sc:>3} {:>3} | {:.4} | {}/{}/{}/{} | {}",
                row.id,
                if row.eye_inside { "EYE" } else { "" },
                show(d.finest),
                d.in_view,
                show(row.wanted_max),
                show(row.twin.map(|t| t.1)),
                show(d.hi_c_restated),
                i32::from(hi_c) - i32::from(hi_d),
                d.nearest_occupied.unwrap_or(f64::NAN),
                cols[0],
                cols[1],
                cols[2],
                cols[3],
                cols[4],
            );
            // Sources of the nearest occupied depth (Task 5a).
            let tdm = dmax.max(cmax);
            let his = source_his(&h, &view, j, row, &d, None);
            let mut line = format!(
                "SRC {pose} node {:>3}{} lo {lo} hi_now {hi} hi_c {hi_c} tdm {tdm} |",
                row.id,
                if row.eye_inside { " EYE" } else { "" }
            );
            for (s, hi_s) in his.iter().enumerate() {
                let pages = hi_s.map_or(0, |x| pages_of(row, lo.min(x)..=x));
                let _ = write!(line, " {} {} ({pages}p)", SOURCES[s], show(*hi_s));
                src_pages[s][0] += pages;
                // The nodes next to the eye of the sample: the two that hold or touch it.
                let near_eye = row.eye_inside || row.id == 14 || row.id == 17;
                if near_eye {
                    src_pages[s][1] += pages;
                }
                stats[s].add(pose, row.id, near_eye, *hi_s, his[0], tdm, hi_c);
            }
            println!("{line}");
            if row.eye_inside || row.id == 14 || row.id == 17 {
                let (_, nears, witness) = source_detail(&h, &view, j, row, &d, None);
                let depths: Vec<u8> = resident_levels(&h, h.cell_nodes[j].id)
                    .iter()
                    .map(|(depth, _)| *depth)
                    .collect();
                println!(
                    "NEAR {pose} node {} resident depths {depths:?} nearest view depth (scaled) {:?}; a.clip page (cells, box extent) {witness:?}",
                    row.id,
                    nears.map(|n| n.map(|v| (v * 1000.0).round() / 1000.0))
                );
                print_stability_probe(&h, &view, j, row, &d, pose);
            }
        }
        println!("-- {pose} SRC pages (all, nodes 14/17/EYE) per source: {src_pages:?}");
        println!(
            "-- {pose} totals (pages now/data_max/hi_d/hi_c/resident): all {tot:?}; nodes holding the eye {eye_tot:?}; pool resident {pool}"
        );
    }
    println!(
        "== hi_d - max(data_max, cell_max): {:?}",
        histogram(slack_d.into_iter())
    );
    println!("== hi_c - hi_d: {:?}", histogram(d_vs_c.into_iter()));
    for (s, st) in stats.iter().enumerate() {
        st.print(SOURCES[s]);
    }
    println!(
        "== violations ({}):\n{}",
        violations.len(),
        violations.join("\n")
    );
}

// ---------------------------------------------------------------------------------------------
// ADR 0055 plan, task 7a: what the eye node holds and how late detail arrives when zooming in
// (ignored diagnostics; the output formats are stable so a later run can be diffed against them)
// ---------------------------------------------------------------------------------------------

impl Harness {
    /// A frame followed by what the browser does with the driver's level requests: the set goes
    /// to the build side, which builds what it can on the next frame. `settle` does not relay
    /// them (the older diagnostics measure the levels the plan alone builds).
    fn frame_relayed(&mut self, view: &ViewProjection) -> FrameInfo {
        let info = self.frame(view);
        if let Some(requests) = self.driver.take_level_requests() {
            self.lag.requests = requests.len();
            self.dataset.apply_level_requests(&requests);
            self.dirty = true;
        }
        info
    }

    /// `settle` with the requests relayed; also waits for the requests the build side still holds.
    fn settle_relayed(&mut self, view: &ViewProjection, quiet: usize, max_frames: usize) {
        let mut calm = 0;
        for _ in 0..max_frames {
            let info = self.frame_relayed(view);
            let settled = !self.dirty
                && self.stage == Some(self.target_px)
                && info.apply == CutApply::Applied
                && info.uploaded_pages == 0
                && info.pending_blocks == 0
                && info.built == 0
                && info.deferred == 0
                && !self.driver.needs_frame()
                && !self.dataset.has_pending_level_requests(true);
            calm = if settled { calm + 1 } else { 0 };
            if calm >= quiet {
                return;
            }
        }
        panic!("did not settle in {max_frames} frames (requests relayed)");
    }
}

/// The sample settled at `pose` like `settled_sample_pose`, with the level requests relayed when
/// `relay` is set (what the browser does; the plain harness never answers them).
fn settled_sample_pose_with(pose: &str, relay: bool) -> (Harness, ViewProjection, ViewProjection) {
    let (mut h, origin, scale) = sample_harness();
    let local = |p: [f64; 3]| -> [f64; 3] { std::array::from_fn(|a| (p[a] - origin[a]) * scale) };
    let data = h.dataset.metadata.hierarchy().stats().data_bounds;
    let mut fit = OrbitCamera::default();
    assert!(fit.fit(
        SceneBounds {
            min: local(data.min()),
            max: local(data.max())
        },
        DEMO_VIEWPORT
    ));
    let initial = ViewProjection::new(fit, DEMO_VIEWPORT, scale, VoxelBuildLimits::default());
    let view = ViewProjection::new(
        demo_pose_camera(fit, pose),
        DEMO_VIEWPORT,
        scale,
        VoxelBuildLimits::default(),
    );
    if relay {
        h.settle_relayed(&initial, 3, 5000);
        h.camera_moved();
        h.settle_relayed(&view, 3, 5000);
    } else {
        h.settle(&initial, 3, 5000);
        h.camera_moved();
        h.settle(&view, 3, 5000);
    }
    (h, view, initial)
}

/// `hi_d` of node `j` of the settled cut, derived as `print_data_bound_totals` does (the pool's
/// own `CellPool::data_bound` is `cfg(test)` of the render crate, out of reach from here).
fn hi_d_of(h: &Harness, view: &ViewProjection, j: usize) -> Option<u8> {
    let ctx = h.cell_ctx.as_ref()?;
    let node = &h.cell_nodes[j];
    let data = h.dataset.metadata.hierarchy().stats().data_bounds;
    let region = SpatialBounds::new(
        std::array::from_fn(|a| node.min[a].max(data.min()[a])),
        std::array::from_fn(|a| node.max[a].min(data.max()[a])),
    )
    .ok();
    let span = buildable_span(h, node.id)?;
    let row = NodeRow {
        id: node.id.get(),
        node_depth: node.lod.node_depth,
        span,
        range: h.last_levels[j].2,
        candidate: node_view_range(
            view,
            &h.dataset.cell_grid(),
            h.dataset.coordinate_origin(),
            region,
            span,
            f64::from(ctx.lod.target_px),
            h.driver.pool().min_side(),
        ),
        ..NodeRow::default()
    };
    data_bound(h, view, j, &row, false).hi_d
}

/// One line per (pose, relay, node) of the eye node, its neighbour and the totals.
/// Run: `cargo test --profile fast -p viewer_wasm --offline --lib eye_node_residency_at_poses -- --ignored --nocapture`.
#[test]
#[ignore = "diagnostic: eye-node residency on the built-in sample at P1 to P5"]
fn eye_node_residency_at_poses() {
    for pose in ["P1", "P2", "P3", "P4", "P5"] {
        // Measured equal with and without relaying the requests (the plan itself builds the
        // levels it wishes), so only the browser-like variant runs.
        let (h, view, _) = settled_sample_pose_with(pose, true);
        print_eye_node_residency(&h, &view, pose, true);
    }
}

fn print_eye_node_residency(h: &Harness, view: &ViewProjection, pose: &str, relay: bool) {
    let tag = format!("EYE pose={pose} relay={}", u8::from(relay));
    let (mut over_levels, mut over_pages) = (0_usize, 0_usize);
    for (j, node) in h.cell_nodes.iter().enumerate() {
        let hi_d = hi_d_of(h, view, j);
        let by_depth = pages_by_depth(h, node.id);
        let resident: Vec<u8> = by_depth.iter().map(|(d, _)| *d).collect();
        let total: usize = by_depth.iter().map(|(_, p)| p).sum();
        let (levels_over, pages_over) = hi_d.map_or((0, 0), |bound| {
            by_depth
                .iter()
                .filter(|(d, _)| *d > bound)
                .fold((0, 0), |(n, p), (_, pages)| (n + 1, p + pages))
        });
        over_levels += levels_over;
        over_pages += pages_over;
        if ![14, 17].contains(&node.id.get()) {
            continue;
        }
        let (_, depths, range) = &h.last_levels[j];
        let mut active = depths.clone();
        active.sort_unstable();
        let wish = h.last_entries[j].1.iter().flatten().max().copied();
        println!(
            "{tag} node={} | resident={resident:?} active={active:?} range={range:?} \
             wish_max={wish:?} data_bound={hi_d:?} | pages_by_depth={by_depth:?} pages={total} | \
             levels_over_bound={levels_over} pages_over_bound={pages_over}",
            node.id.get()
        );
    }
    println!(
        "{tag} node=ALL | cut_nodes={} pool_pages={} | levels_over_bound={over_levels} \
         pages_over_bound={over_pages}",
        h.cell_nodes.len(),
        h.driver.pool().resident_pages()
    );
}

// ---- Zoom-in lag: drawn depth against what the same rule would draw with more levels ----------

/// Default samples per node of the lag measurement (about: see `select_samples`).
const LAG_SAMPLES_PER_NODE: usize = 64;

/// Occupied cells of `level` chosen by a position hash (so the choice does not move when a finer
/// level replaces it), about `LAG_SAMPLES_PER_NODE` of them; their centres in the scaled frame
/// minus the origin.
fn select_samples(
    grid: &CellGrid,
    ctx: &CellCtx,
    depth: u8,
    level: &PackedLevel,
    per_node: usize,
) -> Vec<[f64; 3]> {
    let slots: usize = level.pages.iter().map(|p| p.cells as usize).sum();
    let modulus = (slots / per_node.max(1)).max(1).next_power_of_two() as u64;
    let side = grid.root_side / 2.0_f64.powi(i32::from(depth)) * ctx.scale;
    let mut out = Vec::new();
    for (p, page) in level.pages.iter().enumerate() {
        for i in 0..page.cells as usize {
            let instance = level.instances[p * PAGE_SLOTS + i];
            if instance.is_empty() {
                continue;
            }
            let coordinate: [u64; 3] =
                std::array::from_fn(|a| page.origin[a] + u64::from(instance.rel()[a]));
            // Positions at a fixed resolution, whatever the depth.
            let mixed = coordinate.iter().fold(0_u64, |acc, c| {
                let fixed = if depth <= 24 {
                    c << (24 - depth)
                } else {
                    c >> (depth - 24)
                };
                (acc ^ fixed)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .rotate_left(29)
            });
            if (mixed >> 20) & (modulus - 1) != 0 {
                continue;
            }
            out.push(std::array::from_fn(|a| {
                (grid.root_min[a] - ctx.origin[a]) * ctx.scale + (coordinate[a] as f64 + 0.5) * side
            }));
        }
    }
    out
}

/// Whether an eye-relative point lies inside the side planes of the view frustum, ahead of the
/// eye, whatever the render clip distances (the sims' cameras clip the cloud to a thin slab; the
/// rule's decision does not depend on it).
fn in_frustum(ctx: &CellCtx, rel: [f64; 3]) -> bool {
    let dot = |v: [f32; 3]| (0..3).map(|a| rel[a] * f64::from(v[a])).sum::<f64>();
    let depth = dot(ctx.lod.forward);
    let focal = f64::from(ctx.lod.focal);
    depth > 0.0
        && dot(ctx.lod.right).abs() * focal <= depth * ctx.viewport[0] * 0.5
        && dot(ctx.lod.up).abs() * focal <= depth * ctx.viewport[1] * 0.5
}

/// `in_frustum` plus the render clip, as `ViewFrustum` (production, crate-private) tests a point:
/// view depth within `[max(lod_near_depth, clip near), clip far]` (restated, scaled units).
fn in_clip(ctx: &CellCtx, rel: [f64; 3]) -> bool {
    let depth: f64 = (0..3).map(|a| rel[a] * f64::from(ctx.lod.forward[a])).sum();
    in_frustum(ctx, rel) && depth >= ctx.near.max(ctx.clip.0) && depth <= ctx.clip.1
}

impl Harness {
    /// Appends the lag of the frame just drawn to `self.lag.frames`.
    pub(super) fn record_lag(&mut self, view: &ViewProjection) {
        let mut log = std::mem::take(&mut self.lag);
        let frame = self.measure_lag(&mut log, view);
        log.frames.push(frame);
        self.lag = log;
    }

    /// For every node of the applied cut, the occupied cells of its finest resident level (the
    /// hash-picked ones, on screen) are the samples. The depth the node
    /// draws at the cell centre (`drawn_depth_at`, the twin of the GPU rule over the real active
    /// depths) is compared with what the same twin draws if every depth of the node's buildable
    /// span (`b`) or up to its finest resident depth (`r`) were active. Neither reference knows
    /// whether a finer cell exists at the sample: they are proxies, `r` the honest floor.
    fn measure_lag(&self, log: &mut LagLog, view: &ViewProjection) -> LagFrame {
        let ctx = self.cell_ctx.as_ref().expect("cell mode context");
        let grid = self.dataset.cell_grid();
        let mut f = LagFrame {
            view_gen: log.view_gen,
            requests: log.requests as u32,
            pages: self.driver.pool().resident_pages(),
            ..LagFrame::default()
        };
        for (j, node) in self.cell_nodes.iter().enumerate() {
            if log
                .only
                .as_ref()
                .is_some_and(|ids| !ids.contains(&node.id.get()))
            {
                continue;
            }
            let levels = resident_levels(self, node.id);
            let Some(&(finest, level)) = levels.iter().max_by_key(|(d, _)| *d) else {
                continue;
            };
            let Some(span) = *log
                .spans
                .entry(node.id)
                .or_insert_with(|| buildable_span(self, node.id))
            else {
                continue;
            };
            let per_node = if log.per_node == 0 {
                LAG_SAMPLES_PER_NODE
            } else {
                log.per_node
            };
            let bound = if log.bound {
                hi_d_of(self, view, j)
            } else {
                None
            };
            let samples = log
                .samples
                .entry((node.id, finest))
                .or_insert_with(|| select_samples(&grid, ctx, finest, level, per_node));
            let drawn_lod = if log.drop_top == 0 {
                node.lod
            } else {
                let mut active = node.lod.active;
                for _ in 0..log.drop_top {
                    active &= !(1_u64 << active.ilog2());
                }
                NodeLod { active, ..node.lod }
            };
            let ideal = |rel: [f64; 3], cap: u8| -> Option<u8> {
                let (lo, hi) = (span.0 - node.lod.node_depth, cap - node.lod.node_depth);
                let active = ((1_u64 << (hi + 1)) - 1) & !((1_u64 << lo) - 1);
                let lod = NodeLod { active, ..node.lod };
                drawn_depth_at(&ctx.lod, &lod, rel, node.lo, node.side)
            };
            for position in samples.iter() {
                let rel: [f64; 3] = std::array::from_fn(|a| position[a] - ctx.eye[a]);
                if !in_frustum(ctx, rel) {
                    f.off_screen += 1;
                    continue;
                }
                let Some(drawn) = drawn_depth_at(&ctx.lod, &drawn_lod, rel, node.lo, node.side)
                else {
                    f.undrawn += 1;
                    continue;
                };
                let (Some(ideal_b), Some(ideal_r)) =
                    (ideal(rel, span.1.max(finest)), ideal(rel, finest))
                else {
                    f.undrawn += 1;
                    continue;
                };
                f.judged += 1;
                let (lag_b, lag_r) = (
                    i32::from(ideal_b) - i32::from(drawn),
                    i32::from(ideal_r) - i32::from(drawn),
                );
                let clipped = in_clip(ctx, rel);
                let part = if clipped {
                    &mut f.in_clip
                } else {
                    &mut f.out_clip
                };
                part.judged += 1;
                for k in 0..3 {
                    part.b[k] += u32::from(lag_b > k as i32);
                    part.r[k] += u32::from(lag_r > k as i32);
                }
                if clipped
                    && lag_b > 0
                    && log
                        .worst_in_clip
                        .as_ref()
                        .is_none_or(|(key, _)| (lag_r, lag_b) > *key)
                {
                    let depth: f64 = (0..3).map(|a| rel[a] * f64::from(ctx.lod.forward[a])).sum();
                    let half = grid.root_side / 2.0_f64.powi(i32::from(finest)) * ctx.scale / 2.0;
                    let reach: f64 = (0..3).map(|a| f64::from(ctx.lod.forward[a]).abs()).sum();
                    let active: Vec<u8> = (0..64_u8)
                        .filter(|b| node.lod.active >> b & 1 == 1)
                        .map(|b| b + node.lod.node_depth)
                        .collect();
                    let resident: Vec<u8> = levels.iter().map(|(d, _)| *d).collect();
                    let text = format!(
                        "node={} node_depth={} lo={:?} side={:?}\n\
                         sample finest_depth={finest} rel_eye={rel:?} view_depth={depth:.6}\n\
                         clip near_eff={:.6} (lod_near={:.6} clip_near={:.6}) far={:.6}\n\
                         cell cube depth range=[{:.6}, {:.6}] (centre +- {:.6}); centre in clip={clipped}; \
                         whole cube inside depth clip={}; cube touches clip={}\n\
                         resident={resident:?} active={active:?} span={span:?} hi_d={:?}\n\
                         drawn={drawn} ideal_r={ideal_r} ideal_b={ideal_b} lag_r={lag_r} lag_b={lag_b}",
                        node.id.get(),
                        node.lod.node_depth,
                        node.lo,
                        node.side,
                        ctx.near.max(ctx.clip.0),
                        ctx.near,
                        ctx.clip.0,
                        ctx.clip.1,
                        depth - half * reach,
                        depth + half * reach,
                        half * reach,
                        depth - half * reach >= ctx.near.max(ctx.clip.0)
                            && depth + half * reach <= ctx.clip.1,
                        depth + half * reach >= ctx.near.max(ctx.clip.0)
                            && depth - half * reach <= ctx.clip.1,
                        hi_d_of(self, view, j),
                    );
                    log.worst_in_clip =
                        Some(((lag_r, lag_b), format!("frame_gen={} {text}", log.view_gen)));
                }
                for k in 0..3 {
                    f.b[k] += u32::from(lag_b > k as i32);
                    f.r[k] += u32::from(lag_r > k as i32);
                }
                f.b_max = f.b_max.max(lag_b);
                f.r_max = f.r_max.max(lag_r);
                f.finer_b += u32::from(lag_b < 0);
                if let Some(bound) = bound {
                    let over = i32::from(drawn) - i32::from(bound);
                    f.over_bound += u32::from(over > 0);
                    f.over_bound_max = f.over_bound_max.max(over);
                }
            }
        }
        f
    }
}

/// Wheel steps and frames between steps of the synthetic zoom-in, and frames held after it.
const ZOOM_STEPS: usize = 4;
const ZOOM_GAP_FRAMES: usize = 2;
const ZOOM_HOLD_FRAMES: usize = 40;

/// The synthetic zoom-in: the sample settled at P2, then `ZOOM_STEPS` wheel steps (the four that
/// lead from P2 to P3) with `ZOOM_GAP_FRAMES` frames after each, then still. Per frame, the lag
/// at the occupied samples of nodes 14 (holds the eye) and 17 (its neighbour).
/// Run: `cargo test --profile fast -p viewer_wasm --offline --lib synthetic_zoom_in_lag -- --ignored --nocapture`.
#[test]
#[ignore = "diagnostic: lag of the drawn depth while zooming in on the built-in sample"]
fn synthetic_zoom_in_lag() {
    // The last run is a control: the six finest active depths of each node are dropped from what
    // is drawn, so the lag must show up (if it stayed 0 the metric would measure nothing).
    for drop_top in [0, 6] {
        let (mut h, p2, _) = settled_sample_pose_with("P2", true);
        h.lag = LagLog {
            drop_top,
            on: true,
            bound: true,
            per_node: 1 << 20,
            only: Some(vec![14, 17]),
            ..LagLog::default()
        };
        let mut camera = p2.camera();
        let (viewport, scale) = (p2.viewport(), p2.visual_scale());
        let mut view = p2;
        // The state before the first step, as a frame of its own.
        h.frame_relayed(&view);
        h.record_lag(&view);
        for step in 1..=ZOOM_STEPS {
            assert!(camera.zoom(WHEEL_STEP));
            view = ViewProjection::new(camera, viewport, scale, VoxelBuildLimits::default());
            h.camera_moved();
            h.lag.view_gen = step as u64;
            for _ in 0..ZOOM_GAP_FRAMES {
                h.frame_relayed(&view);
                h.record_lag(&view);
            }
        }
        for _ in 0..ZOOM_HOLD_FRAMES {
            h.frame_relayed(&view);
            h.record_lag(&view);
        }
        let control = if drop_top > 0 { " control" } else { "" };
        print_lag(&format!("zoom{control}"), &h.lag.frames);
    }
}
