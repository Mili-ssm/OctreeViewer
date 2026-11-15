//! R3 Task 7c: native measurement of the production cell path (`CellPool` residency + `CellRenderer`
//! with Hi-Z occlusion) on a real GPU. Throwaway like `coverage_spike`; not part of `check.sh`.
//!
//! Usage: cell_pool_bench [--out FILE.json] CASE_DIR|ROOT_DIR...
//! A case directory is one written by `coverage_spike --export-pool` (format in that file's header);
//! a root directory is expanded to its case subdirectories. Run it with `--profile perf`. It needs a
//! real adapter with `timestamp-query` and refuses software ones (llvmpipe).
//!
//! Per case, offscreen 1280x720, pool of 32 768 pages, upload budget of the default config, a fake
//! loader: the driver holds every level of the cut nodes (from `levels.bin`) and offers each
//! requested level in the same frame:
//! * convergence: frames of a still camera until 30 consecutive frames have no upload, page free,
//!   request or pool event;
//! * static: `kpc` and `k`, 3 rounds x 30 frames each (interleaved, 3 warm-up frames after each
//!   switch); GPU time between two encoder timestamps around `encode_cells`;
//! * moving: 120 frames, yaw 0.5 degrees about the world z axis through the eye per frame (cumulative)
//!   and a translation of one median cell side per frame along the initial view direction, as the
//!   T16a spike, with the cut fixed and `CellPool::frame` run every frame (`kpc`);
//! * CPU per frame, still and moving, timed from the driver's phase marks (`FramePhase`) with the
//!   spans of the T7c/T7d records: `pool_frame_ms` (cut + fit), `page_table_ms` (page entries of
//!   the active pages), `set_active_pages_ms`, and their `cut_fit_plus_table_ms` sum (the "cpu"
//!   column). `driver_frame_ms` (the whole `CellDriver::frame`, loader and GPU writes included) is
//!   an extra key and the "drv" column.
use std::{
    collections::HashMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use serde_json::{Value, json};
use viewer_core::{NodeId, SpatialBounds};
use viewer_render_wgpu::{
    CellCutNode, CellDriver, CellGrid, CellInstance, CutApply, DriverConfig, FramePhase, FramePlan,
    OrbitCamera, PAGE_SLOTS, PackedLevel, PageInfo, PoolConfig, PoolNode, RendererConfig,
    ViewProjection, VoxelBuildLimits,
    cell_bench::{BenchRenderer, bench_cells_supported, bench_device_limits},
    cell_cut, representative_depth,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// The production pool, capped like `pool_pages_for_limits` at the usual 65,535 workgroups per
/// dispatch dimension (the slot pass dispatches one workgroup per page).
const POOL_PAGES: u32 = if viewer_render_wgpu::DEFAULT_POOL_PAGES > 65_535 {
    65_535
} else {
    viewer_render_wgpu::DEFAULT_POOL_PAGES
};
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const ROUNDS: usize = 3;
const FRAMES_PER_ROUND: usize = 30;
const WARMUP: usize = 3;
const MOVING_FRAMES: usize = 120;
const YAW_DEG_PER_FRAME: f64 = 0.5;
const QUIET_FRAMES: usize = 30;
const MAX_CONVERGE_FRAMES: usize = 600;
const PAGE_RECORD: usize = 48;

// ---- Case files ----

struct LevelFile {
    pages: usize,
    offset: usize,
}

struct CaseNode {
    pool: PoolNode,
    levels: HashMap<u8, LevelFile>,
}

struct Case {
    name: String,
    pose: String,
    viewport: [u32; 2],
    visual_scale: f64,
    target_px: f64,
    grid: CellGrid,
    camera: OrbitCamera,
    cut: Vec<NodeId>,
    nodes: HashMap<NodeId, CaseNode>,
    bin: Vec<u8>,
}

fn f64s<const N: usize>(value: &Value) -> Result<[f64; N]> {
    let array = value.as_array().ok_or("expected an array")?;
    if array.len() != N {
        return Err(format!("expected {N} values, found {}", array.len()).into());
    }
    let mut out = [0.0; N];
    for (slot, item) in out.iter_mut().zip(array) {
        *slot = f64::from_bits(item.as_u64().ok_or("expected f64 bits")?);
    }
    Ok(out)
}

fn load_case(dir: &Path) -> Result<Case> {
    let meta: Value = serde_json::from_slice(&fs::read(dir.join("pool.json"))?)?;
    if meta["format"] != "r3-pool-v1" {
        return Err(format!("{}: unknown pool format", dir.display()).into());
    }
    let bin = fs::read(dir.join("levels.bin"))?;
    let viewport = [
        meta["viewport"][0].as_u64().ok_or("viewport")? as u32,
        meta["viewport"][1].as_u64().ok_or("viewport")? as u32,
    ];
    let camera = OrbitCamera::from_f64_array(&f64s::<11>(&meta["camera_bits"])?)
        .ok_or("camera does not round-trip")?;
    let grid = CellGrid {
        root_min: f64s(&meta["grid"]["root_min_bits"])?,
        root_side: f64::from_bits(meta["grid"]["root_side_bits"].as_u64().ok_or("root_side")?),
    };
    let mut nodes = HashMap::new();
    for node in meta["nodes"].as_array().ok_or("nodes")? {
        let id = NodeId::new(node["id"].as_u64().ok_or("node id")?);
        let mut levels = HashMap::new();
        let mut stored = Vec::new();
        for level in node["levels"].as_array().ok_or("levels")? {
            let depth = level["depth"].as_u64().ok_or("depth")? as u8;
            stored.push(depth);
            levels.insert(
                depth,
                LevelFile {
                    pages: level["pages"].as_u64().ok_or("pages")? as usize,
                    offset: level["offset"].as_u64().ok_or("offset")? as usize,
                },
            );
        }
        let bounds = SpatialBounds::new(
            f64s(&node["bounds_min_bits"])?,
            f64s(&node["bounds_max_bits"])?,
        )?;
        nodes.insert(
            id,
            CaseNode {
                pool: PoolNode {
                    bounds,
                    stored,
                    points: node["points"].as_u64().ok_or("points")?,
                },
                levels,
            },
        );
    }
    Ok(Case {
        name: dir
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        pose: meta["pose"].as_str().unwrap_or("").to_string(),
        viewport,
        visual_scale: f64::from_bits(meta["visual_scale_bits"].as_u64().ok_or("visual_scale")?),
        target_px: f64::from_bits(meta["target_px_bits"].as_u64().ok_or("target_px")?),
        grid,
        camera,
        cut: meta["cut"]
            .as_array()
            .ok_or("cut")?
            .iter()
            .map(|v| v.as_u64().map(NodeId::new).ok_or("cut id"))
            .collect::<std::result::Result<_, _>>()?,
        nodes,
        bin,
    })
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

impl Case {
    /// The fake loader: parses one level from `levels.bin`.
    fn load(&self, node: NodeId, depth: u8) -> PackedLevel {
        let file = &self.nodes[&node].levels[&depth];
        let mut at = file.offset;
        let instances = (0..file.pages * PAGE_SLOTS)
            .map(|i| {
                let base = at + i * 16;
                CellInstance(std::array::from_fn(|w| u32_at(&self.bin, base + w * 4)))
            })
            .collect();
        at += file.pages * PAGE_SLOTS * 16;
        let pages = (0..file.pages)
            .map(|p| {
                let base = at + p * PAGE_RECORD;
                let b = &self.bin;
                PageInfo {
                    node,
                    depth,
                    origin: std::array::from_fn(|a| u64_at(b, base + a * 8)),
                    first_cell: u64_at(b, base + 24) as usize,
                    cells: u32_at(b, base + 32),
                    flags: u32_at(b, base + 36),
                    box_max: std::array::from_fn(|a| {
                        u16::from_le_bytes([b[base + 40 + a * 2], b[base + 41 + a * 2]])
                    }),
                }
            })
            .collect();
        PackedLevel {
            node,
            depth,
            instances,
            pages,
        }
    }

    /// The pool's own size estimate (`cell_pool::estimate_pages`), for the estimate-vs-exact check.
    fn estimate(&self, node: NodeId, depth: u8) -> usize {
        let data = &self.nodes[&node].pool;
        let node_depth = representative_depth(&self.grid, data.bounds).saturating_sub(3);
        let gap = u32::from(depth.saturating_sub(node_depth));
        let cells = if gap >= 21 {
            u64::MAX
        } else {
            1_u64 << (3 * gap)
        };
        (data.points.min(cells).div_ceil(PAGE_SLOTS as u64)).max(1) as usize
    }

    fn exact(&self, node: NodeId, depth: u8) -> usize {
        self.nodes[&node].levels[&depth].pages
    }
}

// ---- GPU ----

struct Gpu {
    adapter_name: String,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    period_ns: f32,
}

fn init_gpu() -> Result<Gpu> {
    let instance = wgpu::Instance::default();
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let mut listed = Vec::new();
    let mut chosen = None;
    for adapter in adapters {
        let info = adapter.get_info();
        listed.push(format!(
            "{} ({:?}, {:?})",
            info.name, info.backend, info.device_type
        ));
        if chosen.is_none()
            && !matches!(info.device_type, wgpu::DeviceType::Cpu)
            && info.backend != wgpu::Backend::Gl
        {
            chosen = Some(adapter);
        }
    }
    let adapter = chosen
        .ok_or_else(|| format!("no real (non-software, non-GL) adapter; found {listed:?}"))?;
    let info = adapter.get_info();
    let wanted = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    if !adapter.features().contains(wanted) {
        return Err(format!(
            "{} lacks timestamp-query (+ inside encoders); has {:?}",
            info.name,
            adapter.features()
        )
        .into());
    }
    let limits = bench_device_limits(&adapter);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("cell-pool-bench"),
        required_features: wanted,
        required_limits: limits,
        memory_hints: wgpu::MemoryHints::Performance,
        ..Default::default()
    }))?;
    if !bench_cells_supported(&adapter, &device.limits()) {
        return Err("the device does not run the production cell pool".into());
    }
    let period_ns = queue.get_timestamp_period();
    Ok(Gpu {
        adapter_name: format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type),
        adapter,
        device,
        queue,
        period_ns,
    })
}

struct Targets {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    args: wgpu::Buffer,
}

fn make_targets(gpu: &Gpu, viewport: [u32; 2]) -> Targets {
    let size = wgpu::Extent3d {
        width: viewport[0],
        height: viewport[1],
        depth_or_array_layers: 1,
    };
    let texture = |format| {
        gpu.device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("cell-bench-target"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    };
    let buffer = |size, usage| {
        gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cell-bench-buffer"),
            size,
            usage,
            mapped_at_creation: false,
        })
    };
    Targets {
        color: texture(COLOR_FORMAT),
        depth: texture(wgpu::TextureFormat::Depth32Float),
        queries: gpu.device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("cell-bench-ts"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        }),
        resolve: buffer(
            16,
            wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        ),
        readback: buffer(
            16,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        ),
        args: buffer(
            40,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        ),
    }
}

fn read_back(gpu: &Gpu, buffer: &wgpu::Buffer) -> Vec<u8> {
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = sender.send(r);
    });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    receiver.recv().unwrap().expect("map");
    let data = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    data
}

// ---- Driver + renderer ----

struct Rig<'a> {
    case: &'a Case,
    gpu: &'a Gpu,
    targets: Targets,
    renderer: BenchRenderer,
    driver: CellDriver,
    frame_period: f64,
}

#[derive(Default, Clone)]
struct FrameStats {
    /// `CellPool::frame` (cut + fit), as the T7c/T7d records timed it.
    pool_ms: f64,
    /// Page entries of the active pages.
    table_ms: f64,
    set_active_ms: f64,
    /// Whole `CellDriver::frame`: the three above plus loader and GPU writes.
    frame_ms: f64,
    uploads_pages: usize,
    evictions: usize,
    cancels: usize,
    frees: usize,
    requests: usize,
    active_pages: usize,
    counters: viewer_render_wgpu::PoolCounters,
    apply: Option<bool>,
}

impl FrameStats {
    fn quiet(&self) -> bool {
        self.uploads_pages == 0
            && self.frees == 0
            && self.requests == 0
            && self.evictions == 0
            && self.cancels == 0
    }
}

impl<'a> Rig<'a> {
    fn new(case: &'a Case, gpu: &'a Gpu) -> Result<Self> {
        // The fake loader gives the driver every level of the cut nodes up front (the viewer gives
        // the cells of a node once); the driver answers the pool's requests from them.
        let mut driver = CellDriver::new(DriverConfig {
            pool: PoolConfig {
                capacity_pages: POOL_PAGES,
                ..PoolConfig::default()
            },
            grid: case.grid,
            coordinate_origin: [0.0; 3],
            cpu_budget_bytes: u64::MAX,
        });
        for id in &case.cut {
            let node = &case.nodes[id];
            let levels = node
                .pool
                .stored
                .iter()
                .map(|depth| case.load(*id, *depth))
                .collect();
            driver.add_packed(*id, node.pool.bounds, node.pool.points, levels)?;
        }
        Ok(Self {
            case,
            gpu,
            targets: make_targets(gpu, case.viewport),
            renderer: BenchRenderer::new(&gpu.device, &gpu.queue, COLOR_FORMAT, POOL_PAGES),
            driver,
            frame_period: 0.0,
        })
    }

    fn view(&self, camera: OrbitCamera) -> ViewProjection {
        let defaults = RendererConfig::default();
        let limits = VoxelBuildLimits {
            max_voxels: defaults
                .voxel_build_limits
                .max_voxels
                .min(defaults.gpu_budget.max_visible_instances),
            ..defaults.voxel_build_limits
        };
        ViewProjection::new(camera, self.case.viewport, self.case.visual_scale, limits)
    }

    /// One frame of the viewer loop without drawing.
    fn step(&mut self, camera: OrbitCamera) -> Result<(FramePlan, FrameStats)> {
        let view = self.view(camera);
        self.renderer.set_eye(camera.position());
        let t0 = Instant::now();
        let mut ends = [t0; 5];
        let report = self.driver.frame_marked(
            &mut self.renderer,
            &self.case.cut,
            &view,
            self.case.target_px,
            &mut |phase| ends[phase as usize] = Instant::now(),
        )?;
        let end = Instant::now();
        let span = |from: Instant, to: Instant| to.duration_since(from).as_secs_f64() * 1e3;
        let at = |phase: FramePhase| ends[phase as usize];
        let stats = FrameStats {
            pool_ms: span(t0, at(FramePhase::Planned)),
            table_ms: span(at(FramePhase::Written), at(FramePhase::TableBuilt)),
            set_active_ms: span(at(FramePhase::TableBuilt), at(FramePhase::ActivePagesSet)),
            frame_ms: span(t0, end),
            uploads_pages: report.uploaded_pages,
            evictions: report.evicted,
            cancels: report.cancelled,
            frees: report.freed_pages,
            requests: report.requested,
            active_pages: report.active_pages,
            counters: report.plan.counters,
            apply: Some(report.plan.apply == CutApply::Applied),
        };
        Ok((report.plan, stats))
    }

    /// Draws one frame and returns the GPU milliseconds between the two timestamps, and the
    /// draw arguments (phase 1 / plain count, phase 2 count) when `counts` is set.
    fn draw(&mut self, camera: OrbitCamera, counts: bool) -> Result<(f64, Option<[u32; 2]>)> {
        let eye = camera.eye_frame(self.case.viewport).ok_or("no eye frame")?;
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.write_timestamp(&self.targets.queries, 0);
        self.renderer.encode_cells(
            &mut encoder,
            &self.targets.color,
            &self.targets.depth,
            eye.view_projection,
            self.case.viewport,
        );
        encoder.write_timestamp(&self.targets.queries, 1);
        encoder.resolve_query_set(&self.targets.queries, 0..2, &self.targets.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.targets.resolve, 0, &self.targets.readback, 0, 16);
        if counts {
            encoder.copy_buffer_to_buffer(
                self.renderer.draw_args_buffer(),
                0,
                &self.targets.args,
                0,
                40,
            );
        }
        let wall = Instant::now();
        self.gpu.queue.submit([encoder.finish()]);
        let raw = read_back(self.gpu, &self.targets.readback);
        self.frame_period = wall.elapsed().as_secs_f64() * 1e3;
        let ticks = [u64_at(&raw, 0), u64_at(&raw, 8)];
        let ms = ticks[1].saturating_sub(ticks[0]) as f64 * f64::from(self.gpu.period_ns) / 1e6;
        let counts = counts.then(|| {
            let args = read_back(self.gpu, &self.targets.args);
            [u32_at(&args, 4), u32_at(&args, 24)]
        });
        Ok((ms, counts))
    }
}

// ---- Statistics ----

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn summary(values: &[f64]) -> Value {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mean = sorted.iter().sum::<f64>() / sorted.len().max(1) as f64;
    json!({
        "n": sorted.len(),
        "median": percentile(&sorted, 0.5),
        "p95": percentile(&sorted, 0.95),
        "max": sorted.last().copied().unwrap_or(f64::NAN),
        "mean": mean,
    })
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    percentile(&sorted, 0.5)
}

fn num(value: &Value, key: &str) -> f64 {
    value[key].as_f64().unwrap_or(f64::NAN)
}

// ---- One case ----

fn run_case(case: &Case, gpu: &Gpu) -> Result<Value> {
    let mut rig = Rig::new(case, gpu)?;
    let camera = case.camera;

    // d. Convergence with a still camera.
    let mut quiet_run = 0;
    let mut last_noisy = 0;
    let mut frames = 0;
    let (mut total_pages, mut total_evictions, mut total_cancels) = (0, 0, 0);
    let mut cpu: Vec<FrameStats> = Vec::new();
    let mut last = FrameStats::default();
    let mut last_plan = None;
    while quiet_run < QUIET_FRAMES && frames < MAX_CONVERGE_FRAMES {
        let (plan, stats) = rig.step(camera)?;
        frames += 1;
        total_pages += stats.uploads_pages;
        total_evictions += stats.evictions;
        total_cancels += stats.cancels;
        if stats.quiet() {
            quiet_run += 1;
            cpu.push(stats.clone());
        } else {
            quiet_run = 0;
            last_noisy = frames;
            cpu.clear();
        }
        last = stats;
        last_plan = Some(plan);
    }
    let converged = quiet_run >= QUIET_FRAMES;
    let plan = last_plan.ok_or("no frame ran")?;
    let counters = last.counters;
    let still_evals = cpu
        .iter()
        .map(|s| s.counters.cut_evaluations)
        .max()
        .unwrap_or(0);
    let converge = json!({
        "converged": converged,
        "frames": last_noisy,
        "frames_run": frames,
        "uploaded_pages": total_pages,
        "evictions": total_evictions,
        "cancels": total_cancels,
        "pool_coarsened": counters.pool_coarsened,
        "still_cut_evaluations_max": still_evals,
        "pool_overflow": counters.pool_overflow,
        "descendant_too_fine": counters.descendant_too_fine,
        "nodes_without_level": counters.nodes_without_level,
        "stages": counters.stages,
        "resident_pages": counters.resident_pages,
        "apply_applied": last.apply,
    });

    // Cut and size diagnostics at the still pose: uncapped wanted cut, estimated vs exact pages.
    let diagnostics = size_diagnostics(case, &rig, &plan, camera);

    // c (still). CPU per frame over the quiet frames.
    let cpu_still = cpu_summary(&cpu);

    // a. Static GPU time, kpc and k interleaved by round.
    let mut gpu_ms: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    let mut round_medians: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    let mut drawn = [[0_u32; 2]; 2];
    for _round in 0..ROUNDS {
        for (index, occlusion) in [true, false].into_iter().enumerate() {
            rig.renderer.set_occlusion(occlusion);
            for _ in 0..WARMUP {
                rig.draw(camera, false)?;
            }
            let mut round = Vec::new();
            for _ in 0..FRAMES_PER_ROUND {
                round.push(rig.draw(camera, false)?.0);
            }
            round_medians[index].push(median(&round));
            gpu_ms[index].extend(round);
            drawn[index] = rig.draw(camera, true)?.1.ok_or("no counts")?;
        }
    }
    let static_json = json!({
        "kpc": summary(&gpu_ms[0]),
        "k": summary(&gpu_ms[1]),
        "kpc_round_medians": round_medians[0],
        "k_round_medians": round_medians[1],
        "kpc_cells_phase1": drawn[0][0],
        "kpc_cells_phase2": drawn[0][1],
        "k_cells": drawn[1][0],
    });

    // b. Moving camera, kpc, cut fixed, the pool runs every frame.
    rig.renderer.set_occlusion(true);
    for _ in 0..WARMUP {
        rig.draw(camera, false)?;
    }
    let sides: Vec<f64> = plan
        .active
        .iter()
        .filter_map(|a| rig.driver.pool().level(a.key))
        .filter_map(|l| l.pages.first())
        .map(|p| p.cell_side(&case.grid) * case.visual_scale)
        .collect();
    let median_side = median(&sides);
    let eye0 = camera.position();
    let target0 = camera.target();
    let dir0: [f64; 3] = std::array::from_fn(|a| target0[a] - eye0[a]);
    let norm = dir0.iter().map(|v| v * v).sum::<f64>().sqrt();
    let forward: [f64; 3] = dir0.map(|v| v / norm);
    let (mut gpu_moving, mut wall_moving) = (Vec::new(), Vec::new());
    let mut series: Vec<FrameStats> = Vec::new();
    let mut moving_phase_cells = Vec::new();
    for f in 1..=MOVING_FRAMES {
        let theta = (f as f64 * YAW_DEG_PER_FRAME).to_radians();
        let (s, c) = theta.sin_cos();
        let rotated = [
            c * dir0[0] - s * dir0[1],
            s * dir0[0] + c * dir0[1],
            dir0[2],
        ];
        let step = f as f64 * median_side;
        let eye: [f64; 3] = std::array::from_fn(|a| eye0[a] + forward[a] * step);
        let target: [f64; 3] = std::array::from_fn(|a| eye[a] + rotated[a]);
        let mut moved = camera;
        if !moved.set_pose(eye, target) {
            return Err("moving pose rejected".into());
        }
        let (_, stats) = rig.step(moved)?;
        let (ms, counts) = rig.draw(moved, f % 10 == 0)?;
        if let Some(counts) = counts {
            moving_phase_cells.push(counts);
        }
        gpu_moving.push(ms);
        wall_moving.push(rig.frame_period);
        series.push(stats);
    }
    let per_frame =
        |get: &dyn Fn(&FrameStats) -> f64| -> Vec<f64> { series.iter().map(get).collect() };
    let moving_json = json!({
        "median_cell_side_scaled": median_side,
        "gpu_ms": summary(&gpu_moving),
        "submit_to_done_ms": summary(&wall_moving),
        "uploaded_pages_per_frame": summary(&per_frame(&|s| s.uploads_pages as f64)),
        "uploaded_pages_total": series.iter().map(|s| s.uploads_pages).sum::<usize>(),
        "evictions_per_frame": summary(&per_frame(&|s| s.evictions as f64)),
        "evictions_total": series.iter().map(|s| s.evictions).sum::<usize>(),
        "cancels_total": series.iter().map(|s| s.cancels).sum::<usize>(),
        "frames_with_uploads": series.iter().filter(|s| s.uploads_pages > 0).count(),
        "active_pages": summary(&per_frame(&|s| s.active_pages as f64)),
        "pool_coarsened_max": series.iter().map(|s| s.counters.pool_coarsened).max(),
        "pool_overflow_last": series.last().map(|s| s.counters.pool_overflow),
        "nodes_without_level_max": series.iter().map(|s| s.counters.nodes_without_level).max(),
        "descendant_too_fine_frames": series.iter().filter(|s| s.counters.descendant_too_fine).count(),
        "retained_frames": series.iter().filter(|s| s.apply == Some(false)).count(),
        "cut_evaluations": summary(&per_frame(&|s| f64::from(s.counters.cut_evaluations))),
        "depth_refreshes": summary(&per_frame(&|s| f64::from(s.counters.depth_refreshes))),
        "wanted_reused_frames": series.iter().filter(|s| s.counters.wanted_reused).count(),
        "real_reused_frames": series.iter().filter(|s| s.counters.real_reused).count(),
        "cpu": cpu_summary(&series),
        "kpc_cells_phase1_phase2_sampled": moving_phase_cells,
    });

    Ok(json!({
        "case": case.name,
        "pose": case.pose,
        "target_px": case.target_px,
        "cut_nodes": case.cut.len(),
        "active_pages": last.active_pages,
        "active_levels": plan.active.len(),
        "static": static_json,
        "moving": moving_json,
        "cpu_still": cpu_still,
        "convergence": converge,
        "diagnostics": diagnostics,
    }))
}

fn cpu_summary(frames: &[FrameStats]) -> Value {
    let get = |f: &dyn Fn(&FrameStats) -> f64| -> Vec<f64> { frames.iter().map(f).collect() };
    let total = get(&|s| s.pool_ms + s.table_ms);
    json!({
        "pool_frame_ms": summary(&get(&|s| s.pool_ms)),
        "page_table_ms": summary(&get(&|s| s.table_ms)),
        "cut_fit_plus_table_ms": summary(&total),
        "set_active_pages_ms": summary(&get(&|s| s.set_active_ms)),
        "driver_frame_ms": summary(&get(&|s| s.frame_ms)),
    })
}

/// Why the fit coarsens (or not): the uncapped wanted cut with exact and with estimated sizes
/// against the thresholds of `CellPool`, and how far the estimates are from the packed levels.
fn size_diagnostics(case: &Case, rig: &Rig, plan: &FramePlan, camera: OrbitCamera) -> Value {
    let usable = u64::from(POOL_PAGES - POOL_PAGES / 8);
    let (mut est_total, mut exact_total, mut under, mut over) = (0_u64, 0_u64, 0_u32, 0_u32);
    let mut worst_ratio = 0.0_f64;
    for (id, node) in &case.nodes {
        for depth in &node.pool.stored {
            let (e, x) = (case.estimate(*id, *depth), case.exact(*id, *depth));
            est_total += e as u64;
            exact_total += x as u64;
            under += u32::from(x > e);
            over += u32::from(x < e);
            worst_ratio = worst_ratio.max(x as f64 / e as f64);
        }
    }
    let view = rig.view(camera);
    let lookup = |id: NodeId| {
        let node = case.nodes.get(&id)?;
        Some(CellCutNode {
            bounds: node.pool.bounds,
            stored: node.pool.stored.clone(),
            resident: node.pool.stored.clone(),
        })
    };
    let wanted = cell_cut(
        &case.cut,
        &lookup,
        &view,
        &case.grid,
        [0.0; 3],
        case.target_px,
    );
    let uncapped = match wanted {
        Ok(entries) => {
            let mut keys = std::collections::HashSet::new();
            for entry in &entries {
                keys.insert((entry.node, case.nodes[&entry.node].pool.stored[0]));
                for depth in entry.wanted.iter().flatten() {
                    keys.insert((entry.node, *depth));
                }
            }
            let exact: u64 = keys.iter().map(|(n, d)| case.exact(*n, *d) as u64).sum();
            let estimated: u64 = keys.iter().map(|(n, d)| case.estimate(*n, *d) as u64).sum();
            json!({
                "levels": keys.len(),
                "exact_pages": exact,
                "estimated_pages": estimated,
            })
        }
        Err(error) => json!({ "error": error.to_string() }),
    };
    let drawn_exact: u64 = plan
        .active
        .iter()
        .map(|a| case.exact(a.key.node, a.key.depth) as u64)
        .sum();
    json!({
        "usable_pages": usable,
        "high_pages_90": usable * 90 / 100,
        "low_pages_80": usable * 80 / 100,
        "uncapped_wanted_cut": uncapped,
        "active_pages_exact": drawn_exact,
        "all_stored_levels_estimated_pages": est_total,
        "all_stored_levels_exact_pages": exact_total,
        "levels_exact_above_estimate": under,
        "levels_exact_below_estimate": over,
        "worst_exact_over_estimate": worst_ratio,
    })
}

// ---- Main ----

fn case_dirs(args: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for arg in args {
        if arg.join("pool.json").is_file() {
            out.push(arg.clone());
        } else {
            let mut children: Vec<PathBuf> = fs::read_dir(arg)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.join("pool.json").is_file())
                .collect();
            children.sort();
            out.extend(children);
        }
    }
    if out.is_empty() {
        return Err("no case directory (with pool.json) found".into());
    }
    Ok(out)
}

fn main() -> Result<()> {
    let mut args: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let mut out_path = PathBuf::from("target/r3-t7c/bench.json");
    if let Some(at) = args.iter().position(|a| a == "--out") {
        if at + 1 >= args.len() {
            return Err("--out needs a path".into());
        }
        out_path = args.remove(at + 1);
        args.remove(at);
    }
    if args.is_empty() {
        return Err("usage: cell_pool_bench [--out FILE.json] CASE_DIR|ROOT_DIR...".into());
    }
    if cfg!(debug_assertions) {
        eprintln!("# warning: debug build; run with --profile perf for the numbers");
    }
    let dirs = case_dirs(&args)?;
    let gpu = init_gpu()?;
    println!("# adapter {}", gpu.adapter_name);
    println!(
        "# timestamp period {} ns; pool {POOL_PAGES} pages; viewport 1280x720; adapter backend limits ok ({:?})",
        gpu.period_ns,
        gpu.adapter.get_info().backend
    );
    println!(
        "{:<34} {:>6} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>6} {:>5} {:>8} {:>8} {:>8}",
        "case",
        "nodes",
        "kpc med",
        "kpc p95",
        "k med",
        "k p95",
        "mv med",
        "mv p95",
        "cpu med",
        "cpu p95",
        "drv med",
        "drv p95",
        "conv",
        "coars",
        "act pgs",
        "ph1 cel",
        "ph2 cel",
    );
    let mut results = Vec::new();
    for dir in &dirs {
        let case = load_case(dir)?;
        let result = run_case(&case, &gpu)?;
        let s = &result["static"];
        let m = &result["moving"];
        let cpu = &m["cpu"]["cut_fit_plus_table_ms"];
        let still = &result["cpu_still"]["cut_fit_plus_table_ms"];
        let drv = &m["cpu"]["driver_frame_ms"];
        let drv_still = &result["cpu_still"]["driver_frame_ms"];
        println!(
            "{:<34} {:>6} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>3.2}/{:<3.2} {:>3.2}/{:<3.2} {:>3.2}/{:<3.2} {:>3.2}/{:<3.2} {:>6} {:>5} {:>8} {:>8} {:>8}",
            case.name,
            case.cut.len(),
            num(&s["kpc"], "median"),
            num(&s["kpc"], "p95"),
            num(&s["k"], "median"),
            num(&s["k"], "p95"),
            num(&m["gpu_ms"], "median"),
            num(&m["gpu_ms"], "p95"),
            num(still, "median"),
            num(cpu, "median"),
            num(still, "p95"),
            num(cpu, "p95"),
            num(drv_still, "median"),
            num(drv, "median"),
            num(drv_still, "p95"),
            num(drv, "p95"),
            result["convergence"]["frames"],
            result["convergence"]["pool_coarsened"],
            result["active_pages"],
            s["kpc_cells_phase1"],
            s["kpc_cells_phase2"],
        );
        results.push(result);
    }
    println!("# cpu columns: median and p95 ms of cut+fit+page table, still/moving");
    println!("# drv columns: the same for the whole CellDriver::frame (loader and GPU writes too)");
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &out_path,
        serde_json::to_vec_pretty(&json!({
            "adapter": gpu.adapter_name,
            "timestamp_period_ns": gpu.period_ns,
            "pool_pages": POOL_PAGES,
            "cases": results,
        }))?,
    )?;
    println!("# wrote {}", out_path.display());
    Ok(())
}
