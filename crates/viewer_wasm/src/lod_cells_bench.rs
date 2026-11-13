//! R3 Tasks 9b2b and 9c2b: native measurement of the cells path of `LodDataset` on real data. It is an
//! `#[ignore]` test because `lod_driver` is not reachable from an example. Run one case with
//!
//! `BENCH_DATASET=majadas BENCH_POSE=py BENCH_PX=4 cargo test --profile perf -p viewer_wasm
//! --offline --lib cells_bench -- --ignored --nocapture`
//!
//! `BENCH_POSE` is `px | py | pz | orbit` (the poses of `coverage_spike`: the eye in the centre
//! of the leaf with most points, looking along the axis; `orbit` is the fitted camera),
//! `BENCH_TIMEOUT_S` (default 1500) bounds the run and a partial result is reported, and the JSON
//! goes to `target/r3-t9c2b-<dataset>-<pose>-<px>px.json`.

use super::*;
use viewer_format::PagedReader;

pub(super) const VIEWPORT: [u32; 2] = [1280, 720];
const CELL_CPU_STOP_BYTES: u64 = 512 << 20;

pub(super) fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

/// Peak resident set of the process (`VmHWM`), Linux only.
fn peak_rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VmHWM:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        })
        .unwrap_or(0)
}

/// The leaf with most points, as `coverage_spike` finds it, and the camera frame of the dataset.
pub(super) struct Frame {
    pub(super) leaf_eye: [f64; 3],
    pub(super) fitted: OrbitCamera,
    pub(super) step: f64,
}

pub(super) fn spike_frame(path: &std::path::Path) -> Frame {
    let mut reader = FormatReader::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut attrs = path.as_os_str().to_owned();
    attrs.push("attrs");
    if !reader.has_attributes() && std::path::Path::new(&attrs).exists() {
        reader
            .attach_attributes(std::fs::File::open(attrs).unwrap())
            .unwrap();
    }
    let mut leaf_flags = HashMap::new();
    let mut paged = PagedReader::open(std::fs::File::open(path).unwrap()).unwrap();
    let mut pages = vec![paged.root_page().clone()];
    let mut cursor = 0;
    while cursor < pages.len() {
        let page = pages[cursor].clone();
        for node in page.nodes() {
            leaf_flags.insert(node.id(), !node.has_children());
        }
        for (anchor, pointer) in page.child_pages() {
            pages.push(paged.read_child_page(anchor, pointer).unwrap());
        }
        cursor += 1;
    }
    let metadata = reader.format_metadata().viewer_dataset().unwrap();
    let scale = reader.format_metadata().visual_scale();
    let hierarchy = metadata.hierarchy().clone();
    let origin = metadata.coordinate_frame().origin();
    let root = hierarchy.stats().root_bounds;
    let root_side = (0..3)
        .map(|a| root.max()[a] - root.min()[a])
        .fold(0.0, f64::max);
    let root_local = SceneBounds {
        min: std::array::from_fn(|a| (root.min()[a] - origin[a]) * scale),
        max: std::array::from_fn(|a| (root.max()[a] - origin[a]) * scale),
    };
    let mut fitted = OrbitCamera::default();
    assert!(fitted.fit(root_local, VIEWPORT));
    let leaf = hierarchy
        .query(root)
        .filter(|n| leaf_flags[&n.id()])
        .max_by_key(|n| n.point_count())
        .expect("a leaf");
    let b = leaf.bounds();
    let leaf_eye = std::array::from_fn(|a| ((b.min()[a] + b.max()[a]) / 2.0 - origin[a]) * scale);
    Frame {
        leaf_eye,
        fitted,
        step: root_side * scale / 16.0,
    }
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1]
}

fn stats_of(values: &[f64]) -> [f64; 3] {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    [
        percentile(&v, 0.5),
        percentile(&v, 0.95),
        percentile(&v, 1.0),
    ]
}

#[test]
#[ignore = "measurement; needs target/render-data/*-v75.oct"]
fn cells_bench() {
    let dataset = env_or("BENCH_DATASET", "majadas");
    let pose = env_or("BENCH_POSE", "py");
    let px: f64 = env_or("BENCH_PX", "4").parse().unwrap();
    let timeout_s: u64 = env_or("BENCH_TIMEOUT_S", "1500").parse().unwrap();
    let file = format!("{dataset}-v75.oct");
    let path = real_path(&file).unwrap_or_else(|| panic!("{file} is missing"));
    let frame = spike_frame(&path);
    let mut real = open_real(&file, px, 256 << 20).unwrap();
    real.viewport = VIEWPORT;
    let mut camera = frame.fitted;
    if pose != "orbit" {
        let dir: [f64; 3] = match pose.as_str() {
            "px" => [1.0, 0.0, 0.0],
            "py" => [0.0, 1.0, 0.0],
            "pz" => [0.0, 0.0, 1.0],
            other => panic!("unknown pose {other}"),
        };
        let target = std::array::from_fn(|a| frame.leaf_eye[a] + frame.step * dir[a]);
        assert!(camera.set_pose(frame.leaf_eye, target));
    }
    let view = ViewProjection::new(camera, VIEWPORT, real.scale, VoxelBuildLimits::default());
    let h = &mut real.harness;
    h.count_octant_coarsenings = true;
    h.soft_checks = true;
    // Peak RSS counts only the run, not the leaf search above.
    let _ = std::fs::write("/proc/self/clear_refs", "5");

    let started = Instant::now();
    let (mut build_ms, mut build_ms_nonzero, mut frame_ms) = (vec![], vec![], vec![]);
    let (mut nodes_built, mut peak_cpu, mut peak_cell_cpu) = (0usize, 0u64, 0u64);
    let mut last = None::<FrameInfo>;
    let (mut calm, mut settled_at) = (0, None);
    let mut stop = "settled".to_owned();
    let mut cpu_bytes = 0;
    loop {
        if started.elapsed().as_secs() >= timeout_s {
            stop = format!("timeout after {timeout_s} s");
            break;
        }
        let t = Instant::now();
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h.frame(&view)));
        let info = match out {
            Ok(info) => info,
            Err(payload) => {
                stop = format!(
                    "panic: {}",
                    payload
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                        .unwrap_or_default()
                );
                break;
            }
        };
        frame_ms.push(t.elapsed().as_secs_f64() * 1e3);
        build_ms.push(info.build_ms);
        if info.built > 0 {
            build_ms_nonzero.push(info.build_ms);
        }
        nodes_built += info.built;
        cpu_bytes = h.driver.cpu_bytes();
        peak_cpu = peak_cpu.max(cpu_bytes);
        peak_cell_cpu = peak_cell_cpu.max(info.cpu_cell_bytes);
        last = Some(info);
        let settled = !h.dirty
            && h.stage == Some(px)
            && info.apply == CutApply::Applied
            && info.uploaded_pages == 0
            && info.pending_blocks == 0
            && info.built == 0
            && info.deferred == 0
            && !h.driver.needs_frame_on(&h.target);
        calm = if settled { calm + 1 } else { 0 };
        if calm == 1 {
            settled_at = Some(h.totals.frames);
        }
        if calm >= 3 {
            break;
        }
        if info.max_gap > 1 || h.totals.longest_retained > MAX_RETAINED_STREAK || info.dtf {
            stop = format!(
                "stop rule: gap {} streak {} dtf {}",
                info.max_gap, h.totals.longest_retained, info.dtf
            );
            break;
        }
        if let Some(failure) = &h.first_failure {
            stop = format!("stop rule: {failure}");
            break;
        }
        if px == 4.0 && dataset == "majadas" && peak_cell_cpu > CELL_CPU_STOP_BYTES {
            stop = format!("stop rule: cell CPU {peak_cell_cpu} B > 512 MiB");
            break;
        }
    }
    let wall = started.elapsed().as_secs_f64();
    let t = &h.totals;
    let (all, nz) = (stats_of(&build_ms), stats_of(&build_ms_nonzero));
    let fm = stats_of(&frame_ms);
    let last = last.expect("at least one frame");
    let rss = peak_rss_kib();
    let resident_end = last.resident_pages;
    let first_failure = h.first_failure.clone().unwrap_or_default();
    let pages_used = h.dataset.pages.as_ref().map_or(0, |p| p.pages());
    let json = format!(
        "{{\"dataset\":\"{dataset}\",\"pose\":\"{pose}\",\"px\":{px},\"viewport\":[1280,720],\
\"stop\":{stop:?},\"updates\":{},\"updates_to_settle\":{},\"nodes_built\":{nodes_built},\
\"build_ms_all\":{{\"median\":{:.3},\"p95\":{:.3},\"max\":{:.3}}},\
\"build_ms_with_builds\":{{\"n\":{},\"median\":{:.3},\"p95\":{:.3},\"max\":{:.3}}},\
\"frame_ms\":{{\"median\":{:.3},\"p95\":{:.3},\"max\":{:.3}}},\
\"driver_cpu_bytes_end\":{cpu_bytes},\"driver_cpu_bytes_peak\":{peak_cpu},\
\"cpu_cell_bytes_end\":{},\"cpu_cell_bytes_peak\":{peak_cell_cpu},\
\"pool_resident_pages_end\":{resident_end},\"dataset_pages_resident\":{pages_used},\
\"longest_retained_streak\":{},\"max_gap\":{},\"gaps_over_one\":{},\"dtf\":{},\
\"pool_resident_pages_peak\":{},\"fallback_pages_peak\":{},\"held_nodes_peak\":{},\
\"excluded_nodes_peak\":{},\"hold_age_max\":{},\"hold_unsettled_total\":{},\"wait_frames\":{},\
\"hole_frames\":{},\"lost_samples\":{},\"first_failure\":{first_failure:?},\
\"cross_node_coarsenings\":{},\"octant_coarsenings\":{},\"octant_lowered\":{},\"wall_s\":{wall:.1},\
\"peak_rss_kib\":{rss},\"final_cut_nodes\":{},\"stages\":{:?}}}\n",
        t.frames,
        settled_at.map_or("null".to_owned(), |n| n.to_string()),
        all[0],
        all[1],
        all[2],
        build_ms_nonzero.len(),
        nz[0],
        nz[1],
        nz[2],
        fm[0],
        fm[1],
        fm[2],
        last.cpu_cell_bytes,
        t.longest_retained,
        t.max_gap,
        t.gaps_over_one,
        t.dtf,
        t.resident_peak,
        t.fallback_peak,
        t.held_peak,
        t.excluded_peak,
        t.hold_age_max,
        t.hold_unsettled,
        t.wait_frames,
        h.hole_frames,
        h.lost_samples,
        h.zone_coarsenings,
        h.octant_coarsenings,
        h.octant_lowered,
        last.cut_nodes,
        t.stages,
    );
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/r3-t9c2b-{dataset}-{pose}-{px}px.json"
    ));
    std::fs::write(&out, &json).unwrap();
    println!("BENCH {json}");
    println!("BENCH json: {}", out.display());
    h.report(&format!("{dataset} {pose} {px}px"));
}

// ---------------------------------------------------------------------------------------------
// Diagnosis of the forget/rebuild loop under the CPU budget (ADR 0037)
// ---------------------------------------------------------------------------------------------

/// What the planner will work on at `px` before a frame: the selection, the co-owners of a block
/// shared with a selected node that has cells, the nodes without cells those lead to (with their
/// ancestors) and which of the candidates' chains already have cells.
#[derive(Default)]
pub(super) struct Pre {
    pub(super) selected: HashSet<NodeId>,
    pub(super) waiters: HashSet<NodeId>,
    pub(super) candidates: Vec<NodeId>,
    pub(super) before: HashSet<NodeId>,
}

pub(super) fn analyse_pre(dataset: &mut LodDataset, view: &ViewProjection, px: f64) -> Pre {
    let mut pre = Pre::default();
    let sel = dataset.select_target(view, px).unwrap().1;
    let hierarchy = dataset.metadata.hierarchy();
    pre.selected = sel.node_ids().iter().copied().collect();
    for id in sel
        .node_ids()
        .iter()
        .filter(|id| dataset.cells.has_cells(**id))
    {
        for block in hierarchy.node(*id).into_iter().flat_map(|n| n.blocks()) {
            for owner in dataset.block_owners.get(block).into_iter().flatten() {
                if !dataset.cells.has_cells(*owner) {
                    pre.waiters.insert(*owner);
                }
            }
        }
    }
    let mut seen = HashSet::new();
    for id in pre.selected.iter().chain(&pre.waiters) {
        if dataset.cells.has_cells(*id) {
            continue;
        }
        let mut chain = vec![*id];
        let mut up = hierarchy.node(*id).and_then(|n| n.parent());
        while let Some(a) = up {
            let Some(node) = hierarchy.node(a) else { break };
            if !node.blocks().is_empty() && !dataset.cells.has_cells(a) {
                chain.push(a);
            }
            up = node.parent();
        }
        for c in chain {
            if seen.insert(c) {
                pre.candidates.push(c);
            }
        }
    }
    for c in &pre.candidates {
        let mut up = Some(*c);
        while let Some(a) = up {
            if dataset.cells.has_cells(a) {
                pre.before.insert(a);
            }
            up = hierarchy.node(a).and_then(|n| n.parent());
        }
    }
    pre
}

#[derive(Default)]
struct LoopRecord {
    builds: usize,
    forgets: usize,
    /// Role at the last build: selected | waiter | ancestor.
    build_roles: Vec<&'static str>,
    /// Where it stood at each forget: cut | support | selected | waiter | ancestor-of-candidate | none.
    forget_places: Vec<&'static str>,
}

/// Runs the cells path with a CPU budget (driver and build side) and records the role of every
/// node that is built and forgotten. Roles use the selection at the target px before each frame.
///
/// `BENCH_DATASET=majadas BENCH_POSE=py BENCH_PX=1 BENCH_BUDGET_MB=512 BENCH_FRAMES=3000 cargo test
/// --profile perf -p viewer_wasm --offline --lib forget_loop_diagnosis -- --ignored --nocapture`
#[test]
#[ignore = "diagnosis; needs target/render-data/*-v75.oct"]
fn forget_loop_diagnosis() {
    let dataset = env_or("BENCH_DATASET", "majadas");
    let pose = env_or("BENCH_POSE", "py");
    let px: f64 = env_or("BENCH_PX", "1").parse().unwrap();
    let frames: usize = env_or("BENCH_FRAMES", "3000").parse().unwrap();
    let budget: u64 = env_or("BENCH_BUDGET_MB", "512").parse::<u64>().unwrap() << 20;
    let file = format!("{dataset}-v75.oct");
    let path = real_path(&file).unwrap_or_else(|| panic!("{file} is missing"));
    let frame = spike_frame(&path);
    println!("DIAG leaf_eye {:?} step {}", frame.leaf_eye, frame.step);
    let cache_mb: usize = env_or("BENCH_CACHE_MB", "2048").parse().unwrap();
    let mut real = open_real_with_cache(&file, px, 256 << 20, cache_mb << 20).unwrap();
    real.viewport = VIEWPORT;
    let mut camera = frame.fitted;
    let dir: [f64; 3] = match pose.as_str() {
        "px" => [1.0, 0.0, 0.0],
        "py" => [0.0, 1.0, 0.0],
        "pz" => [0.0, 0.0, 1.0],
        other => panic!("unknown pose {other}"),
    };
    let target = std::array::from_fn(|a| frame.leaf_eye[a] + frame.step * dir[a]);
    assert!(camera.set_pose(frame.leaf_eye, target));
    let view = ViewProjection::new(camera, VIEWPORT, real.scale, VoxelBuildLimits::default());
    let h = &mut real.harness;
    h.soft_checks = true;
    h.count_octant_coarsenings = true;
    h.driver = driver_for(&h.dataset, budget);
    h.config.cpu_budget_bytes = Some(budget);

    let mut records: HashMap<NodeId, LoopRecord> = HashMap::new();
    let (mut calm, mut frames_run) = (0, 0);
    let mut forget_total = 0usize;
    for n in 0..frames {
        // Pre-frame view of the work at the target px, as the planner will see it.
        let analyse = h.stage == Some(px);
        let Pre {
            selected,
            waiters,
            candidates,
            before,
        } = if analyse {
            analyse_pre(&mut h.dataset, &view, px)
        } else {
            Pre::default()
        };
        let info = h.frame(&view);
        frames_run += 1;
        let hierarchy = h.dataset.metadata.hierarchy();
        let new_built = h.totals.last_built.clone();
        let forgotten = h.totals.last_forgotten.clone();
        forget_total += forgotten.len();
        let mut have = before.clone();
        have.extend(new_built.iter().copied());
        let role = |id: &NodeId| -> &'static str {
            if selected.contains(id) {
                "selected"
            } else if waiters.contains(id) {
                "waiter"
            } else {
                "ancestor"
            }
        };
        for id in &new_built {
            let r = records.entry(*id).or_default();
            r.builds += 1;
            if analyse {
                r.build_roles.push(role(id));
            }
        }
        // The cut and support of this frame, as the planner sent them.
        let plan_ids: HashSet<NodeId> = h
            .dataset
            .visible_plan
            .as_ref()
            .map(|p| p.node_ids().iter().copied().collect())
            .unwrap_or_default();
        let mut support: HashSet<NodeId> = HashSet::new();
        for c in candidates.iter().filter(|c| !have.contains(c)) {
            let mut up = hierarchy.node(*c).and_then(|n| n.parent());
            while let Some(a) = up {
                if have.contains(&a) {
                    support.insert(a);
                }
                up = hierarchy.node(a).and_then(|n| n.parent());
            }
        }
        let cand_ancestors: HashSet<NodeId> = candidates.iter().copied().collect();
        for id in &forgotten {
            let place = if plan_ids.contains(id) {
                "cut"
            } else if support.contains(id) {
                "support"
            } else if selected.contains(id) {
                "selected"
            } else if waiters.contains(id) {
                "waiter"
            } else if cand_ancestors.contains(id) {
                "ancestor-of-candidate"
            } else {
                "none"
            };
            let r = records.entry(*id).or_default();
            r.forgets += 1;
            r.forget_places.push(place);
        }
        if n % 100 == 0 || n + 1 == frames {
            let cut: Vec<NodeId> = plan_ids
                .iter()
                .copied()
                .filter(|id| h.dataset.cells.has_cells(*id))
                .collect();
            let bytes = |ids: &mut dyn Iterator<Item = NodeId>| -> u64 {
                ids.filter_map(|id| h.dataset.cells.built_bytes(id)).sum()
            };
            let cut_b = bytes(&mut cut.iter().copied());
            let sup_b = bytes(&mut support.iter().copied());
            let needed_b = bytes(
                &mut cut
                    .iter()
                    .copied()
                    .chain(support.iter().copied())
                    .collect::<HashSet<_>>()
                    .into_iter(),
            );
            let waiters_built: Vec<NodeId> = waiters
                .iter()
                .copied()
                .filter(|w| have.contains(w))
                .collect();
            let waiters_b = bytes(&mut waiters_built.iter().copied());
            println!(
                "DIAG frame {n} stage {} built {} forgotten_total {forget_total} cut {} ({cut_b} B) support {} ({sup_b} B) needed {needed_b} B driver {} B waiters_built {} ({waiters_b} B) cand {} cpu_coarsened {} pending_blocks {} deferred {} rebuilt {} block_evictions {}",
                info.stage,
                info.built,
                cut.len(),
                support.len(),
                h.driver.cpu_bytes(),
                waiters_built.len(),
                candidates.len(),
                info.cpu_coarsened,
                info.pending_blocks,
                info.deferred,
                h.dataset.cells.diag.rebuilt_total,
                h.dataset.cells.diag.block_evictions,
            );
        }
        let settled = !h.dirty
            && h.stage == Some(px)
            && info.apply == CutApply::Applied
            && info.uploaded_pages == 0
            && info.pending_blocks == 0
            && info.built == 0
            && info.deferred == 0
            && !h.driver.needs_frame();
        calm = if settled { calm + 1 } else { 0 };
        if calm >= 3 {
            break;
        }
    }
    let mut roles: std::collections::BTreeMap<String, usize> = Default::default();
    let mut rebuilt = 0;
    for r in records.values().filter(|r| r.forgets > 0) {
        let build = r.build_roles.last().copied().unwrap_or("pre-target-stage");
        let place = r
            .forget_places
            .last()
            .copied()
            .unwrap_or("pre-target-stage");
        *roles
            .entry(format!("built-as {build:<16} forgotten-while {place}"))
            .or_default() += 1;
        rebuilt += usize::from(r.builds > 1);
    }
    println!(
        "DIAG summary frames {frames_run} settled {} forget_events {forget_total} distinct_forgotten {} rebuilt {rebuilt}",
        calm >= 3,
        records.values().filter(|r| r.forgets > 0).count()
    );
    for (k, v) in &roles {
        println!("DIAG role {k}: {v} nodes");
    }
    let mut events: std::collections::BTreeMap<&str, usize> = Default::default();
    for r in records.values() {
        for p in &r.forget_places {
            *events.entry(*p).or_default() += 1;
        }
    }
    println!("DIAG forget events by place: {events:?}");
    let mut worst: Vec<_> = records.iter().filter(|(_, r)| r.forgets > 0).collect();
    worst.sort_by_key(|(id, r)| (std::cmp::Reverse(r.forgets), **id));
    for (id, r) in worst.iter().take(10) {
        println!(
            "DIAG node {id:?}: builds {} forgets {} roles {:?} places {:?}",
            r.builds, r.forgets, r.build_roles, r.forget_places
        );
    }
    h.report(&format!("{dataset} {pose} {px}px budget {budget}"));
}

/// Outcome of [`leaf_run`].
#[derive(Debug)]
struct LeafRun {
    /// Frames until the cut was at rest (`None`: it never was within the frames given).
    settled_after: Option<usize>,
    stage: Option<f64>,
    forgotten: usize,
    rebuilt: u64,
    cpu_coarsened: usize,
    pending_blocks: usize,
    block_evictions: u64,
}

/// The camera inside the densest leaf of `dataset` looking along +y at `px`, with the browser's
/// 256 MiB decoded-block cache by default, a CPU budget of `budget_mb` on both sides and at most
/// `frames` frames. Stops once settled (3 quiet frames) and then checks 100 more frames of
/// nothing built or forgotten.
fn leaf_run(dataset: &str, px: f64, cache_mb: usize, budget_mb: u64, frames: usize) -> LeafRun {
    let file = format!("{dataset}-v75.oct");
    let path = real_path(&file).unwrap_or_else(|| panic!("{file} is missing"));
    let frame = spike_frame(&path);
    let mut real = open_real_with_cache(&file, px, 256 << 20, cache_mb << 20).unwrap();
    real.viewport = VIEWPORT;
    let mut camera = frame.fitted;
    let target = [
        frame.leaf_eye[0],
        frame.leaf_eye[1] + frame.step,
        frame.leaf_eye[2],
    ];
    assert!(camera.set_pose(frame.leaf_eye, target));
    let view = ViewProjection::new(camera, VIEWPORT, real.scale, VoxelBuildLimits::default());
    let h = &mut real.harness;
    h.soft_checks = true;
    h.count_octant_coarsenings = true;
    let budget = budget_mb << 20;
    h.driver = driver_for(&h.dataset, budget);
    h.config.cpu_budget_bytes = Some(budget);
    let mut calm = 0;
    let mut settled_after = None;
    let mut last = None;
    for n in 0..frames {
        let info = h.frame(&view);
        last = Some(info);
        let at_rest = !h.dirty
            && h.stage == Some(px)
            && info.apply == CutApply::Applied
            && info.uploaded_pages == 0
            && info.pending_blocks == 0
            && info.built == 0
            && info.deferred == 0
            && !h.driver.needs_frame();
        calm = if at_rest { calm + 1 } else { 0 };
        if calm >= 3 {
            settled_after = Some(n + 1);
            break;
        }
    }
    if settled_after.is_some() {
        let forgotten = h.totals.forgotten;
        for _ in 0..100 {
            h.dirty = true;
            let info = h.frame(&view);
            assert_eq!(info.built, 0, "a node was built after settling");
            last = Some(info);
        }
        assert_eq!(h.totals.forgotten, forgotten, "forgetting went on at rest");
    }
    let last = last.expect("at least one frame");
    let diag = h.dataset.cells.diag;
    LeafRun {
        settled_after,
        stage: h.stage,
        forgotten: h.totals.forgotten,
        rebuilt: diag.rebuilt_total,
        cpu_coarsened: last.cpu_coarsened,
        pending_blocks: last.pending_blocks,
        block_evictions: diag.block_evictions,
    }
}

/// B1c.4. The browser's block cache (256 MiB) cannot hold the blocks of the nodes the plan
/// waits for: built children of incomplete sibling groups lose their blocks again and the
/// ladder never leaves its rung. Majadas at 1 px inside a leaf.
#[test]
#[ignore = "real data: needs target/render-data/majadas-v75.oct"]
fn majadas_at_1px_settles_with_the_browser_block_cache() {
    let run = leaf_run("majadas", 1.0, 256, 512, 6_000);
    println!("{run:?}");
    assert!(
        run.settled_after.is_some(),
        "never settled: stage {:?}, {} pending blocks, {} block evictions, {} forgotten, {} rebuilt",
        run.stage,
        run.pending_blocks,
        run.block_evictions,
        run.forgotten,
        run.rebuilt
    );
}
