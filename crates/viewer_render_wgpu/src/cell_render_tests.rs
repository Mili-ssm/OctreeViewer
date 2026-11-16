//! Task 7b1: the static GPU pool against the CPU path, on every adapter with an offscreen harness.
//!
//! A synthetic multi-node scene gives each cell a unique colour. The cells path and the CPU path
//! (the same cells as `InstanceRaw`, lighting off) must show the same colour at every pixel except
//! next to an identity boundary of the reference, with depth inside a tolerance derived from the
//! projection and the f32 resolution of the scene.

use std::collections::HashSet;

use viewer_core::{ColorRgb, NodeId, SpatialBounds};
use wgpu::util::DeviceExt;

use crate::cell_pages::{CellInstance, PackedLevel, PageEntry, PageFrame, srgb_to_linear_table};
use crate::cell_render::{
    CellFrame, CellLimitMode, CellRenderer, DEFAULT_CELL_POOL_BUDGET_BYTES, DeviceLimits,
    MASK_WORDS, MIN_POOL_PAGES, PAGE_BYTES, POOL_BYTES, cells_supported, cells_supported_for,
    plan_device_limits, pool_pages_for_limits, pool_pages_within_budget, pool_resource_bytes,
    shader_sources,
};
use crate::gpu::{CUBE_INDICES, ViewProjection, create_pipeline_resources};
use crate::{
    CellGrid, InstanceRaw, NodeCell, NodeCellLevel, OrbitCamera, PAGE_SLOTS, RendererConfig,
    SceneBounds, decode_cell, pack_level,
};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 192;
const POOL_PAGES_FOR_TESTS: u32 = 64;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Task 7b2: two-phase occlusion against the compaction-only path.
#[path = "cell_cull_tests.rs"]
mod cull;

/// Task 12: the shader colour modes of the cell shader against the CPU path's.
#[path = "cell_color_tests.rs"]
mod color;

/// Task 8: picking on the pool against the CPU-path picking.
#[path = "cell_pick_tests.rs"]
mod pick;

/// Task 9a2: the cell driver on a real pool, at a far coordinate origin.
#[path = "cell_driver_gpu_tests.rs"]
mod driver;

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

struct Harness {
    label: String,
    real: bool,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    limits: DeviceLimits,
    /// The production pool fits (`cells_supported`).
    supported: bool,
    /// A pool of the size the tests use fits, which is also true of adapters whose own storage
    /// limit is below the production 256 MiB.
    runs_test_pool: bool,
}

fn harness(adapter: &wgpu::Adapter, mode: CellLimitMode) -> Result<Harness, String> {
    let info = adapter.get_info();
    if info.backend == wgpu::Backend::Gl {
        // The oracle reads depth back, which the GLSL backend cannot do (neither a copy nor
        // `textureLoad` on a depth texture). The viewer's cell pool is WebGPU-only anyway.
        return Err("Gl backend: no depth readback".into());
    }
    let limits = plan_device_limits(adapter, mode);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("cell-render-test"),
        required_features: wgpu::Features::empty(),
        required_limits: limits.required_limits.clone(),
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        ..Default::default()
    }))
    .map_err(|error| format!("device: {error}"))?;
    let supported = cells_supported(adapter, &device.limits());
    let runs_test_pool = cells_supported_for(
        adapter,
        &device.limits(),
        u64::from(POOL_PAGES_FOR_TESTS) * PAGE_BYTES,
    );
    Ok(Harness {
        label: format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type),
        real: info.device_type != wgpu::DeviceType::Cpu,
        adapter: adapter.clone(),
        device,
        queue,
        limits,
        supported,
        runs_test_pool,
    })
}

fn all_adapters() -> Vec<wgpu::Adapter> {
    let instance = wgpu::Instance::default();
    let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    if let Ok(fallback) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
            ..Default::default()
        }))
        && !adapters.iter().any(|a| a.get_info() == fallback.get_info())
    {
        adapters.push(fallback);
    }
    adapters
}

/// Runs `body` on every adapter (real and software). `body` returns false when the adapter cannot
/// run the pool. Fails when nothing ran; `CELL_POOL_REQUIRE_GPU=1` also demands a real adapter.
fn for_each_adapter(mode: CellLimitMode, mut body: impl FnMut(&Harness) -> bool) {
    let (mut ran, mut real_ran) = (0, 0);
    for adapter in &all_adapters() {
        match harness(adapter, mode) {
            Ok(h) if body(&h) => {
                ran += 1;
                real_ran += usize::from(h.real);
            }
            Ok(h) => println!("# adapter {}: skipped (no cell pool support)", h.label),
            Err(error) => println!("# adapter {:?}: skipped ({error})", adapter.get_info().name),
        }
    }
    assert!(ran > 0, "no adapter ran the test");
    if real_ran == 0 {
        println!("# no real adapter ran the test");
        assert!(
            std::env::var("CELL_POOL_REQUIRE_GPU").as_deref() != Ok("1"),
            "CELL_POOL_REQUIRE_GPU=1 but no real adapter ran"
        );
    }
}

struct Image {
    color: Vec<[u8; 4]>,
    depth: Vec<f32>,
}

fn target_textures(h: &Harness) -> (wgpu::Texture, wgpu::Texture) {
    let size = wgpu::Extent3d {
        width: WIDTH,
        height: HEIGHT,
        depth_or_array_layers: 1,
    };
    let texture = |format| {
        h.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cell-test-target"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    };
    (
        texture(COLOR_FORMAT),
        texture(wgpu::TextureFormat::Depth32Float),
    )
}

/// Depth texels into a buffer with a compute shader: not every backend (GL) copies depth
/// textures to buffers.
const DEPTH_COPY_WGSL: &str = "
@group(0) @binding(0) var src: texture_depth_2d;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= 256u || id.y >= 192u { return; }
    dst[id.y * 256u + id.x] = textureLoad(src, vec2<i32>(id.xy), 0);
}
";

fn read_buffer(h: &Harness, buffer: &wgpu::Buffer) -> Vec<u8> {
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = sender.send(r);
    });
    h.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    receiver.recv().unwrap().expect("map");
    let data = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    data
}

/// Copies both targets (and optionally the cell counts of the draw arguments: word 1 of phase 1
/// or of the plain pass, word 6 of phase 2) out after the passes.
fn finish(
    h: &Harness,
    mut encoder: wgpu::CommandEncoder,
    color: &wgpu::Texture,
    depth: &wgpu::Texture,
    draw_args: Option<&wgpu::Buffer>,
) -> (Image, Option<[u32; 2]>) {
    let row = WIDTH * 4;
    assert_eq!(row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT, 0);
    let staging = |label, size| {
        h.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    };
    let image_bytes = u64::from(row * HEIGHT);
    let (color_out, depth_out) = (
        staging("color-out", image_bytes),
        staging("depth-out", image_bytes),
    );
    let count_out = staging("count-out", 28);
    let extent = wgpu::Extent3d {
        width: WIDTH,
        height: HEIGHT,
        depth_or_array_layers: 1,
    };
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &color_out,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(HEIGHT),
            },
        },
        extent,
    );
    let depth_storage = h.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("depth-storage"),
        size: image_bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let module = h.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("depth-copy"),
        source: wgpu::ShaderSource::Wgsl(DEPTH_COPY_WGSL.into()),
    });
    let pipeline = h
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("depth-copy"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor {
        aspect: wgpu::TextureAspect::DepthOnly,
        ..Default::default()
    });
    let bind_group = h.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("depth-copy"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: depth_storage.as_entire_binding(),
            },
        ],
    });
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(WIDTH.div_ceil(8), HEIGHT.div_ceil(8), 1);
    }
    encoder.copy_buffer_to_buffer(&depth_storage, 0, &depth_out, 0, image_bytes);
    if let Some(args) = draw_args {
        encoder.copy_buffer_to_buffer(args, 0, &count_out, 0, 28);
    }
    h.queue.submit([encoder.finish()]);
    let color = read_buffer(h, &color_out);
    let depth = read_buffer(h, &depth_out);
    let count = draw_args.map(|_| {
        let words: Vec<u32> = bytemuck::pod_collect_to_vec(&read_buffer(h, &count_out));
        [words[1], words[6]]
    });
    (
        Image {
            color: color.as_chunks::<4>().0.to_vec(),
            depth: bytemuck::pod_collect_to_vec(&depth),
        },
        count,
    )
}

fn clear_color() -> wgpu::Color {
    RendererConfig::default().clear_color
}

/// The CPU path as `render_offscreen` builds it, with a caller-chosen scene and lighting.
fn render_cpu(h: &Harness, instances: &[InstanceRaw], camera: &OrbitCamera, lit: bool) -> Image {
    render_cpu_styled(h, instances, camera, lit, [0.0, 1.0, 0.0, 1.0], [0.0, 1.0])
}

/// [`render_cpu`] with the shader colour mode (`style` x, and `z` and `w` for the height range)
/// and the intensity range of the CPU path.
fn render_cpu_styled(
    h: &Harness,
    instances: &[InstanceRaw],
    camera: &OrbitCamera,
    lit: bool,
    style: [f32; 4],
    intensity_range: [f32; 2],
) -> Image {
    let frame = camera.frame([WIDTH, HEIGHT]).unwrap();
    let resources = pollster::block_on(create_pipeline_resources(&h.device, COLOR_FORMAT)).unwrap();
    let mut origin = frame.render_origin;
    origin[3] = 1.0;
    h.queue.write_buffer(
        &resources.uniform_buffer,
        0,
        bytemuck::bytes_of(&ViewProjection {
            matrix: frame.view_projection,
            render_origin: origin,
            style,
            intensity_range: [
                intensity_range[0],
                intensity_range[1],
                f32::from(u8::from(lit)),
                0.0,
            ],
        }),
    );
    let buffer = h
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cpu-instances"),
            contents: bytemuck::cast_slice(instances),
            usage: wgpu::BufferUsages::VERTEX,
        });
    let (color, depth) = target_textures(h);
    let (color_view, depth_view) = (
        color.create_view(&Default::default()),
        depth.create_view(&Default::default()),
    );
    let mut encoder = h.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cpu-reference"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear_color()),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&resources.pipeline);
        pass.set_bind_group(0, &resources.uniform_bind_group, &[]);
        pass.set_vertex_buffer(0, resources.vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, buffer.slice(..));
        pass.set_index_buffer(resources.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..CUBE_INDICES.len() as u32, 0, 0..instances.len() as u32);
    }
    finish(h, encoder, &color, &depth, None).0
}

fn render_cells(
    h: &Harness,
    pool: &mut CellRenderer,
    camera: &OrbitCamera,
    lit: bool,
) -> (Image, u32) {
    let (image, counts) = render_cells_phases(h, pool, camera, lit);
    (image, counts[0])
}

/// Like [`render_cells`], with the cells drawn by phase 1 and by phase 2 (phase 2 is 0 when the
/// pool draws without occlusion).
fn render_cells_phases(
    h: &Harness,
    pool: &mut CellRenderer,
    camera: &OrbitCamera,
    lit: bool,
) -> (Image, [u32; 2]) {
    let eye_frame = camera.eye_frame([WIDTH, HEIGHT]).unwrap();
    let (color, depth) = target_textures(h);
    let (color_view, depth_view) = (
        color.create_view(&Default::default()),
        depth.create_view(&Default::default()),
    );
    let mut encoder = h.device.create_command_encoder(&Default::default());
    pool.encode_cells(
        &mut encoder,
        &color_view,
        &depth_view,
        &CellFrame {
            view_projection: eye_frame.view_projection,
            clear_color: clear_color(),
            depth_shading: lit,
            viewport: [WIDTH, HEIGHT],
        },
    );
    let (image, counts) = finish(h, encoder, &color, &depth, Some(pool.draw_args_buffer()));
    (image, counts.unwrap())
}

// ---------------------------------------------------------------------------------------------
// Synthetic scene
// ---------------------------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

fn morton(coord: [u64; 3]) -> u64 {
    (0..21).fold(0, |key, bit| {
        (0..3).fold(key, |key, axis| {
            key | ((coord[axis] >> bit) & 1) << (3 * bit + axis as u64)
        })
    })
}

#[derive(Clone)]
struct TestNode {
    grid: CellGrid,
    bounds: SpatialBounds,
    id: u64,
    node_depth: u8,
}

#[derive(Clone)]
struct TestLevel {
    node: usize,
    level: NodeCellLevel,
    octants: u32,
    /// Mask of this level (bit set = hidden); each level has its own mask slot.
    mask: [u32; MASK_WORDS],
}

#[derive(Clone)]
struct Scene {
    nodes: Vec<TestNode>,
    levels: Vec<TestLevel>,
}

fn cell(coord: [u64; 3], id: u32) -> NodeCell {
    let rgb = [
        16 + u64::from(id % 200),
        16 + u64::from(id / 200 % 200),
        200,
    ];
    NodeCell {
        coord,
        rgb_sum: rgb,
        intensity_sum: 0.0,
        intensity_count: 0,
        count: 1,
        classification: -1,
        z_min: 0.0,
        z_max: 0.0,
    }
}

fn unit_bounds(grid: &CellGrid) -> SpatialBounds {
    SpatialBounds::new(grid.root_min, grid.root_min.map(|v| v + grid.root_side)).unwrap()
}

impl Scene {
    /// A node whose levels share one cut: each of the 8x8x8 mask cells is drawn by one level only
    /// (its mask bit is clear in that level and set in the others), so drawn cells never nest and
    /// no two faces are coplanar. `levels` are `(depth, cell count, octant bits)`.
    fn add_cut_node(
        &mut self,
        rng: &mut Rng,
        next_id: &mut u32,
        grid: CellGrid,
        levels: &[(u8, usize, u32)],
    ) {
        let node = self.nodes.len();
        self.nodes.push(TestNode {
            bounds: unit_bounds(&grid),
            grid,
            id: node as u64 + 1,
            node_depth: 0,
        });
        // The level that draws each mask cell; one extra choice draws nothing.
        let choice: Vec<usize> = (0..512)
            .map(|_| rng.below(levels.len() as u64 + 1) as usize)
            .collect();
        for (k, &(depth, count, octants)) in levels.iter().enumerate() {
            let chosen: Vec<usize> = (0..512).filter(|m| choice[*m] == k).collect();
            let span = 1_u64 << (depth - 3);
            let mut seen = HashSet::new();
            let mut coords = Vec::new();
            while coords.len() < count {
                // 70 % inside the regions this level draws, the rest are masked decoys.
                let m = if !chosen.is_empty() && rng.below(10) < 7 {
                    chosen[rng.below(chosen.len() as u64) as usize]
                } else {
                    rng.below(512) as usize
                };
                let mask_cell = [m % 8, m / 8 % 8, m / 64].map(|v| v as u64);
                let coord: [u64; 3] =
                    std::array::from_fn(|axis| mask_cell[axis] * span + rng.below(span));
                if seen.insert(coord) {
                    coords.push(coord);
                }
            }
            coords.sort_by_key(|coord| morton(*coord));
            let cells = coords
                .into_iter()
                .map(|coord| {
                    *next_id += 1;
                    cell(coord, *next_id)
                })
                .collect();
            let mut mask = [0_u32; MASK_WORDS];
            for (m, chosen_level) in choice.iter().enumerate() {
                if *chosen_level != k {
                    mask[m / 32] |= 1 << (m % 32);
                }
            }
            self.levels.push(TestLevel {
                node,
                level: NodeCellLevel {
                    depth,
                    cells,
                    octant_offsets: None,
                },
                octants,
                mask,
            });
        }
    }

    /// A node whose single level spans octants (depth <= node depth): its all-ones mask must be
    /// ignored by the shader.
    fn add_spanning_node(&mut self, next_id: &mut u32) {
        let grid = CellGrid {
            root_min: [0.55, 0.2, 0.4],
            root_side: 1.0,
        };
        let min = [1.05, 0.2, 0.4];
        let bounds = SpatialBounds::new(min, min.map(|v| v + 0.25)).unwrap();
        let node_depth = crate::representative_depth(&grid, bounds) - 3;
        assert_eq!(node_depth, 2);
        let node = self.nodes.len();
        self.nodes.push(TestNode {
            grid,
            bounds,
            id: node as u64 + 1,
            node_depth,
        });
        *next_id += 1;
        self.levels.push(TestLevel {
            node,
            level: NodeCellLevel {
                depth: 2,
                cells: vec![cell([2, 0, 0], *next_id)],
                octant_offsets: None,
            },
            octants: 0x01,
            mask: [u32::MAX; MASK_WORDS],
        });
    }

    /// Whether the shader must keep the cell before the frustum test: octant bit set and mask bit
    /// clear (levels that span octants skip both), computed from the absolute coordinates.
    fn drawn(&self, level: &TestLevel, cell: &NodeCell) -> bool {
        let node = &self.nodes[level.node];
        if level.level.depth <= node.node_depth {
            return level.octants != 0;
        }
        let m = cell.coord.map(|c| (c >> (level.level.depth - 3)) as usize);
        let octant = 4 * (m[0] >> 2) + 2 * (m[1] >> 2) + (m[2] >> 2);
        let bit = m[0] + 8 * m[1] + 64 * m[2];
        (level.octants >> octant) & 1 == 1 && (level.mask[bit / 32] >> (bit % 32)) & 1 == 0
    }

    fn cpu_instances(&self) -> Vec<InstanceRaw> {
        self.cpu_instances_skipping(&[])
    }

    fn cpu_instances_skipping(&self, skip: &[usize]) -> Vec<InstanceRaw> {
        self.drawn_cells()
            .into_iter()
            .filter(|(level, ..)| !skip.contains(level))
            .map(|(.., instance)| instance)
            .collect()
    }

    /// The cells the shader draws as `(level, index in the level's cells, instance)`, in the order
    /// of `cpu_instances`.
    fn drawn_cells(&self) -> Vec<(usize, usize, InstanceRaw)> {
        let mut out = Vec::new();
        for (index, level) in self.levels.iter().enumerate() {
            let grid = self.nodes[level.node].grid;
            let side = grid.root_side / (1_u64 << level.level.depth) as f64;
            for (cell_index, cell) in level.level.cells.iter().enumerate() {
                if !self.drawn(level, cell) {
                    continue;
                }
                let rgb = cell.rgb_sum.map(|c| c as u8);
                out.push((
                    index,
                    cell_index,
                    InstanceRaw {
                        center: std::array::from_fn(|axis| {
                            (grid.root_min[axis] + (cell.coord[axis] as f64 + 0.5) * side) as f32
                        }),
                        size: side as f32,
                        color: ColorRgb::new(rgb[0], rgb[1], rgb[2]).to_linear_rgba(),
                        intensity: -1.0,
                        classification: -1.0,
                    },
                ));
            }
        }
        out
    }
}

/// Seeded scene: two overlapping nodes on offset grids (several levels, masks, octants), one far
/// off-frustum node, one large node that straddles the frustum, one spanning-level node.
fn build_scene(seed: u64) -> Scene {
    let mut rng = Rng(seed);
    let mut next_id = 0;
    let mut scene = Scene {
        nodes: Vec::new(),
        levels: Vec::new(),
    };
    let grid = |root_min, root_side| CellGrid {
        root_min,
        root_side,
    };
    scene.add_cut_node(
        &mut rng,
        &mut next_id,
        grid([0.0; 3], 1.0),
        &[(3, 90, 0b1011_0111), (4, 320, 0b0110_1101), (5, 900, 0xff)],
    );
    scene.add_cut_node(
        &mut rng,
        &mut next_id,
        grid([0.137, -0.093, 0.211], 0.9),
        &[(3, 60, 0xff), (4, 220, 0b1100_1100)],
    );
    scene.add_cut_node(
        &mut rng,
        &mut next_id,
        grid([0.6, 0.4, -0.4], 3.0),
        &[(4, 150, 0xff), (5, 400, 0b1111_0011)],
    );
    scene.add_cut_node(
        &mut rng,
        &mut next_id,
        grid([6.0, 0.0, 0.0], 1.0),
        &[(3, 40, 0xff)],
    );
    scene.add_spanning_node(&mut next_id);
    scene
}

/// Large cells on screen (at least 24 px with `big_camera`): a coarse node around the camera
/// target and a second one on an offset grid, so most covered pixels are interior.
fn build_big_scene(seed: u64) -> Scene {
    let mut rng = Rng(seed);
    let mut next_id = 0;
    let mut scene = Scene {
        nodes: Vec::new(),
        levels: Vec::new(),
    };
    scene.add_cut_node(
        &mut rng,
        &mut next_id,
        CellGrid {
            root_min: [-1.5; 3],
            root_side: 4.0,
        },
        &[(3, 200, 0xff), (4, 260, 0xff)],
    );
    scene.add_cut_node(
        &mut rng,
        &mut next_id,
        CellGrid {
            root_min: [0.113, -0.287, 0.231],
            root_side: 3.17,
        },
        &[(3, 80, 0xff)],
    );
    scene
}

fn big_camera() -> OrbitCamera {
    let mut camera = test_camera();
    assert!(camera.zoom(-0.5));
    camera
}

fn test_camera() -> OrbitCamera {
    let mut camera = OrbitCamera::default();
    assert!(camera.fit(
        SceneBounds {
            min: [0.0; 3],
            max: [1.0; 3],
        },
        [WIDTH, HEIGHT]
    ));
    assert!(camera.orbit(0.9, 0.25));
    camera
}

/// A level placed in a pool: page slots and mask slot.
struct Placed {
    packed: PackedLevel,
    slots: Vec<u32>,
    mask_slot: u32,
}

fn place(
    pool: &mut CellRenderer,
    scene: &Scene,
    mut next_slots: impl Iterator<Item = u32>,
) -> Vec<Placed> {
    scene
        .levels
        .iter()
        .map(|level| {
            let node = &scene.nodes[level.node];
            let packed = pack_level(&node.grid, node.bounds, NodeId::new(node.id), &level.level);
            let slots: Vec<u32> = next_slots.by_ref().take(packed.pages.len()).collect();
            assert_eq!(slots.len(), packed.pages.len(), "test pool is large enough");
            pool.upload_level(&packed, &slots).unwrap();
            let mask_slot = pool.alloc_mask_slot().expect("mask slot");
            pool.set_node_mask(mask_slot, &level.mask).unwrap();
            Placed {
                packed,
                slots,
                mask_slot,
            }
        })
        .collect()
}

fn entries(scene: &Scene, placed: &[Placed], eye: [f64; 3]) -> Vec<(u32, PageEntry)> {
    let mut out = Vec::new();
    for (level, placed) in scene.levels.iter().zip(placed) {
        let frame = PageFrame {
            grid: scene.nodes[level.node].grid,
            coordinate_origin: [0.0; 3],
            visual_scale: 1.0,
            eye_scaled: eye,
        };
        for (page, slot) in placed.packed.pages.iter().zip(&placed.slots) {
            out.push((*slot, page.entry(&frame, placed.mask_slot, level.octants)));
        }
    }
    out
}

/// A pool that compacts without occlusion (`k`): its draw count is the count of the shader tests.
fn new_pool(h: &Harness) -> CellRenderer {
    let mut pool = new_occlusion_pool(h);
    pool.set_occlusion(false);
    pool
}

/// A pool with the default two-phase occlusion.
fn new_occlusion_pool(h: &Harness) -> CellRenderer {
    CellRenderer::new(&h.device, &h.queue, COLOR_FORMAT, POOL_PAGES_FOR_TESTS)
}

// ---------------------------------------------------------------------------------------------
// Oracle
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default)]
struct OracleStats {
    covered_cells: usize,
    covered_reference: usize,
    interior_covered: usize,
    compared: usize,
    boundary: usize,
    max_depth_diff: f32,
    max_depth_tolerance: f32,
    /// Largest `diff / tolerance` over interior pixels: how close the tolerance is to the data.
    max_tolerance_use: f32,
    distinct_colors: usize,
}

/// Depth tolerance at reference depth `z`. Positions in both paths are f32 numbers of magnitude up
/// to `|eye|` (eye-relative origins for cells, target-relative centres for the CPU path); each
/// accumulates a few half-ulp roundings, bounded here by 12 epsilon of `w + |eye|`. That position
/// error is turned into depth with d(depth)/d(w) of the projection, and 4 epsilon cover the
/// rounding of the depth value itself.
fn depth_tolerance(z: f32, near: f64, far: f64, eye_length: f64) -> f32 {
    let (z, n, f) = (f64::from(z), near, far);
    let w = f * n / (f - z * (f - n));
    let slope = f * n / ((f - n) * w * w);
    let epsilon = f64::from(f32::EPSILON);
    (slope * 12.0 * epsilon * (w + eye_length) + 4.0 * epsilon) as f32
}

/// Largest depth step from pixel `(x, y)` to a neighbour of the same identity (colour within
/// `jump`): the gradient of the face the pixel lies on.
fn neighbours_depth_step(reference: &Image, x: i32, y: i32, jump: u8) -> f32 {
    let index = |x: i32, y: i32| (y as usize) * WIDTH as usize + x as usize;
    (-1..=1)
        .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
        .filter(|(dx, dy)| {
            (0..WIDTH as i32).contains(&(x + dx))
                && (0..HEIGHT as i32).contains(&(y + dy))
                && channel_distance(
                    reference.color[index(x + dx, y + dy)],
                    reference.color[index(x, y)],
                ) <= jump
        })
        .map(|(dx, dy)| {
            (reference.depth[index(x + dx, y + dy)] - reference.depth[index(x, y)]).abs()
        })
        .fold(0.0, f32::max)
}

fn channel_distance(a: [u8; 4], b: [u8; 4]) -> u8 {
    (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap()
}

/// Compares the cells image with the reference. Unlit images carry one identity per colour: equal
/// everywhere except next to an identity boundary of the reference, where a neighbouring identity
/// may show up. Lit images shade each face, so a boundary there is a colour jump of more than 8
/// LSB, and interior pixels may differ by 1 LSB.
fn compare_images(
    cells: &Image,
    reference: &Image,
    camera: &OrbitCamera,
    lit: bool,
) -> OracleStats {
    let (near, far) = camera.clip_distances();
    let eye = camera.position();
    let eye_length = eye.iter().map(|v| v * v).sum::<f64>().sqrt();
    let (jump, lsb) = if lit { (8, 1) } else { (0, 0) };
    let background = reference.color[0];
    let index_of = |x: i32, y: i32| (y as usize) * WIDTH as usize + x as usize;
    let mut stats = OracleStats {
        covered_cells: cells.color.iter().filter(|c| **c != background).count(),
        covered_reference: reference.color.iter().filter(|c| **c != background).count(),
        distinct_colors: reference
            .color
            .iter()
            .filter(|c| **c != background)
            .collect::<HashSet<_>>()
            .len(),
        ..Default::default()
    };
    let covered_difference = stats.covered_cells.abs_diff(stats.covered_reference);
    assert!(
        covered_difference <= stats.covered_reference / 1000,
        "covered pixels differ: cells {} vs reference {}",
        stats.covered_cells,
        stats.covered_reference
    );
    for y in 0..HEIGHT as i32 {
        for x in 0..WIDTH as i32 {
            let index = index_of(x, y);
            let own = reference.color[index];
            let neighbours: Vec<[u8; 4]> = (-1..=1)
                .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
                .filter_map(|(dx, dy)| {
                    let (nx, ny) = (x + dx, y + dy);
                    ((0..WIDTH as i32).contains(&nx) && (0..HEIGHT as i32).contains(&ny))
                        .then(|| reference.color[index_of(nx, ny)])
                })
                .collect();
            let (zc, zr) = (cells.depth[index], reference.depth[index]);
            let steepest = neighbours_depth_step(reference, x, y, jump);
            // A sample can move by a few 1/256 px steps of the rasteriser along the local gradient.
            let tolerance = depth_tolerance(zr, near, far, eye_length) + steepest / 128.0;
            if neighbours.iter().any(|c| channel_distance(*c, own) > jump) {
                stats.boundary += 1;
                if !lit {
                    // Next to an identity boundary only a neighbouring identity may show up, and
                    // when it is this pixel's own identity it lies at the reference depth.
                    let shown = cells.color[index];
                    assert!(
                        neighbours.contains(&shown),
                        "pixel ({x}, {y}): cells {shown:?} is not an identity near {own:?}"
                    );
                    if shown != background {
                        // The same identity at a neighbouring pixel of the reference bounds
                        // the depth: a silhouette sample may sit a fraction of a pixel away.
                        let same: Vec<f32> = (-1..=1)
                            .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
                            .filter(|(dx, dy)| {
                                let (nx, ny) = (x + dx, y + dy);
                                (0..WIDTH as i32).contains(&nx)
                                    && (0..HEIGHT as i32).contains(&ny)
                                    && reference.color[index_of(nx, ny)] == shown
                            })
                            .map(|(dx, dy)| reference.depth[index_of(x + dx, y + dy)])
                            .collect();
                        let (low, high) = same
                            .iter()
                            .fold((f32::MAX, f32::MIN), |(l, h), z| (l.min(*z), h.max(*z)));
                        // Rasterisers snap vertices to 1/256 px, so an edge sample can move by a few
                        // such steps and take a depth along the face gradient: 1/32 of the
                        // neighbourhood's depth range (8 steps over its 2 px) bounds that.
                        let slack = tolerance + (high - low) / 32.0;
                        assert!(
                            zc >= low - slack && zc <= high + slack,
                            "boundary pixel ({x}, {y}): depth {zc} outside {low}..{high}, tolerance {tolerance}"
                        );
                    }
                }
                continue;
            }
            stats.compared += 1;
            assert!(
                channel_distance(cells.color[index], own) <= lsb,
                "pixel ({x}, {y}): cells {:?} vs reference {own:?}",
                cells.color[index]
            );
            if own != background {
                stats.interior_covered += 1;
                let diff = (zc - zr).abs();
                assert!(
                    diff <= tolerance,
                    "pixel ({x}, {y}): depth {zc} vs reference {zr}, tolerance {tolerance}"
                );
                stats.max_tolerance_use = stats.max_tolerance_use.max(diff / tolerance);
                stats.max_depth_diff = stats.max_depth_diff.max(diff);
                stats.max_depth_tolerance = stats.max_depth_tolerance.max(tolerance);
            }
        }
    }
    stats
}

fn run_oracle(
    h: &Harness,
    pool: &mut CellRenderer,
    scene: &Scene,
    camera: &OrbitCamera,
    lit: bool,
) -> OracleStats {
    run_oracle_against(h, pool, &scene.cpu_instances(), camera, lit)
}

fn run_oracle_against(
    h: &Harness,
    pool: &mut CellRenderer,
    instances: &[InstanceRaw],
    camera: &OrbitCamera,
    lit: bool,
) -> OracleStats {
    let (cells, _) = render_cells(h, pool, camera, lit);
    let reference = render_cpu(h, instances, camera, lit);
    let stats = compare_images(&cells, &reference, camera, lit);
    assert!(
        stats.covered_cells > 1000,
        "cells image is nearly empty: {stats:?}"
    );
    assert!(
        stats.covered_reference > 1000,
        "reference image is nearly empty: {stats:?}"
    );
    assert!(
        stats.distinct_colors >= 20 || lit,
        "too few identities: {stats:?}"
    );
    // The tolerance is derived, not tuned, but it must be tight: some pixel uses at least 7 % of
    // it (within about 14x), so an error of that size cannot hide.
    assert!(
        stats.max_tolerance_use >= 0.07,
        "depth tolerance is too loose to catch errors: {stats:?}"
    );
    stats
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[test]
fn cell_shaders_validate_with_naga() {
    for source in shader_sources() {
        let module = naga::front::wgsl::parse_str(&source).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
}

/// Byte offset of every member of the WGSL struct `name`, and its size, from naga's layout.
fn wgsl_struct_layout(name: &str) -> (Vec<(String, u32)>, u32) {
    for source in shader_sources() {
        let module = naga::front::wgsl::parse_str(&source).unwrap();
        for (_, ty) in module.types.iter() {
            if let (Some(n), naga::TypeInner::Struct { members, span }) = (&ty.name, &ty.inner)
                && n == name
            {
                let members = members
                    .iter()
                    .map(|m| (m.name.clone().unwrap(), m.offset))
                    .collect();
                return (members, *span);
            }
        }
    }
    panic!("no WGSL struct {name}");
}

#[test]
fn frame_uniform_is_176_bytes_and_matches_the_wgsl_layout() {
    assert_eq!(size_of::<crate::cell_render::FrameUniform>(), 176);
    let (members, span) = wgsl_struct_layout("CellFrame");
    assert_eq!(span, 176);
    let expected = [
        ("view_proj", 0),
        ("num_pages", 64),
        ("shading", 68),
        ("prev", 72),
        ("capacity", 76),
        ("viewport", 80),
        ("mip_count", 88),
        ("pad0", 92),
        ("color_mode", 96),
        ("intensity_min", 100),
        ("intensity_max", 104),
        ("height_offset", 108),
        ("height_span", 112),
        ("lod_mode", 116),
        ("counters", 120),
        ("target_px", 124),
        ("forward", 128),
        ("focal", 140),
        ("right", 144),
        ("min_side", 156),
        ("up", 160),
        ("pad1", 172),
    ];
    let found: Vec<(&str, u32)> = members.iter().map(|(n, o)| (n.as_str(), *o)).collect();
    assert_eq!(found, expected);
}

#[test]
fn cell_page_is_48_bytes_in_wgsl() {
    assert_eq!(size_of::<PageEntry>(), 48);
    let (members, span) = wgsl_struct_layout("CellPage");
    assert_eq!(span, 48);
    let rel = members.iter().find(|(n, _)| n == "rel").unwrap();
    let depth = members.iter().find(|(n, _)| n == "depth").unwrap();
    assert_eq!((rel.1, depth.1), (32, 44));
}

#[test]
fn pool_constants_follow_the_adr() {
    assert_eq!(PAGE_BYTES, 8192);
    assert_eq!(POOL_BYTES, 512 << 20);
    assert_eq!(u64::from(MIN_POOL_PAGES) * PAGE_BYTES, 256 << 20);
    assert_eq!(
        u64::from(crate::DEFAULT_POOL_PAGES),
        POOL_BYTES / PAGE_BYTES
    );
}

fn limits_with(binding: u64, buffer: u64, workgroups: u32) -> wgpu::Limits {
    wgpu::Limits {
        max_storage_buffer_binding_size: binding,
        max_buffer_size: buffer,
        max_compute_workgroups_per_dimension: workgroups,
        ..wgpu::Limits::downlevel_defaults()
    }
}

#[test]
fn pool_size_follows_the_pool_budget_as_well_as_the_limits() {
    let mib = 1u64 << 20;
    let roomy = limits_with(2048 * mib, 2048 * mib, 65_535);
    // The default budget holds the largest pool the limits allow.
    assert!(pool_resource_bytes(65_535) <= DEFAULT_CELL_POOL_BUDGET_BYTES);
    assert_eq!(
        pool_pages_within_budget(&roomy, DEFAULT_CELL_POOL_BUDGET_BYTES),
        Some(65_535)
    );
    // A smaller budget gives the largest pool that fits it, not more.
    let budget = 400 * mib;
    let pages = pool_pages_within_budget(&roomy, budget).unwrap();
    assert!(pool_resource_bytes(pages) <= budget && pool_resource_bytes(pages + 1) > budget);
    // The floor's resources are the least: below them the CPU path draws.
    let floor = pool_resource_bytes(MIN_POOL_PAGES);
    assert_eq!(
        pool_pages_within_budget(&roomy, floor),
        Some(MIN_POOL_PAGES)
    );
    assert_eq!(pool_pages_within_budget(&roomy, floor - 1), None);
}

#[test]
fn pool_size_is_the_largest_the_limits_admit_with_a_floor() {
    let mib = 1u64 << 20;
    // 512 MiB admitted (and more): the full pool.
    assert_eq!(
        pool_pages_for_limits(&limits_with(2048 * mib, 2048 * mib, 1 << 20)),
        Some(65_536)
    );
    assert_eq!(
        pool_pages_for_limits(&limits_with(512 * mib, 512 * mib, 65_536)),
        Some(65_536)
    );
    // Only 300 MiB of binding, or of buffer: the pages that fit.
    assert_eq!(
        pool_pages_for_limits(&limits_with(300 * mib, 2048 * mib, 65_535)),
        Some((300 * mib / PAGE_BYTES) as u32)
    );
    assert_eq!(
        pool_pages_for_limits(&limits_with(2048 * mib, 300 * mib, 65_535)),
        Some((300 * mib / PAGE_BYTES) as u32)
    );
    // One workgroup per page: the usual dispatch limit of 65,535 costs one page.
    assert_eq!(
        pool_pages_for_limits(&limits_with(2048 * mib, 2048 * mib, 65_535)),
        Some(65_535)
    );
    // The floor is 256 MiB; below it (or a dispatch limit that cannot reach it): CPU path.
    assert_eq!(
        pool_pages_for_limits(&limits_with(256 * mib, 256 * mib, 65_535)),
        Some(MIN_POOL_PAGES)
    );
    assert_eq!(
        pool_pages_for_limits(&limits_with(256 * mib - PAGE_BYTES, 2048 * mib, 65_535)),
        None
    );
    assert_eq!(
        pool_pages_for_limits(&limits_with(128 * mib, 2048 * mib, 65_535)),
        None
    );
    assert_eq!(
        pool_pages_for_limits(&limits_with(2048 * mib, 2048 * mib, MIN_POOL_PAGES - 1)),
        None
    );
}

#[test]
fn cells_match_the_cpu_path_per_pixel_on_every_adapter() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = build_scene(7);
        let camera = test_camera();
        let mut pool = new_pool(h);
        let placed = place(&mut pool, &scene, 0..);
        pool.set_active_pages(&entries(&scene, &placed, camera.position()))
            .unwrap();
        let stats = run_oracle(h, &mut pool, &scene, &camera, false);
        println!("# adapter {}: oracle {stats:?}", h.label);
        // Lighting on: the colour also depends on interpolated normals and on depth.
        let lit = run_oracle(h, &mut pool, &scene, &camera, true);
        println!("# adapter {}: lit oracle {lit:?}", h.label);
        true
    });
}

#[test]
fn large_cells_are_mostly_interior_pixels_on_every_adapter() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = build_big_scene(5);
        let camera = big_camera();
        let mut pool = new_pool(h);
        let placed = place(&mut pool, &scene, 0..);
        pool.set_active_pages(&entries(&scene, &placed, camera.position()))
            .unwrap();
        let stats = run_oracle(h, &mut pool, &scene, &camera, false);
        let ratio = stats.interior_covered as f64 / stats.covered_reference as f64;
        println!(
            "# adapter {}: large cells, interior {ratio:.3}, {stats:?}",
            h.label
        );
        assert!(ratio > 0.8, "interior ratio {ratio}: {stats:?}");
        true
    });
}

/// Rust twin of the frustum test of the shader, with the relative margin as a parameter.
fn outside(matrix: &[[f32; 4]; 4], lo: [f32; 3], hi: [f32; 3], margin: f32) -> bool {
    let mut all_out = 63_u32;
    for index in 0..8_u32 {
        let upper = [((index + 1) >> 1) & 1, (index >> 1) & 1, index >> 2];
        let p: [f32; 3] = std::array::from_fn(|a| if upper[a] == 1 { hi[a] } else { lo[a] });
        let q: [f32; 4] = std::array::from_fn(|row| {
            matrix[0][row] * p[0] + matrix[1][row] * p[1] + matrix[2][row] * p[2] + matrix[3][row]
        });
        let m = margin * q[3].abs() + 1.0e-30;
        let mut bits = 0;
        if q[0] < -q[3] - m {
            bits |= 1;
        }
        if q[0] > q[3] + m {
            bits |= 2;
        }
        if q[1] < -q[3] - m {
            bits |= 4;
        }
        if q[1] > q[3] + m {
            bits |= 8;
        }
        if q[2] < -m {
            bits |= 16;
        }
        if q[2] > q[3] + m {
            bits |= 32;
        }
        all_out &= bits;
    }
    all_out != 0
}

/// `(passing, eligible)`: cells that pass page frustum, empty, octant, mask and cell frustum, as
/// the shader computes them, and those that pass all but the frustum tests.
fn cpu_compaction_count(
    scene: &Scene,
    placed: &[Placed],
    entries: &[(u32, PageEntry)],
    matrix: &[[f32; 4]; 4],
    margin: f32,
    skip: &[usize],
) -> (usize, usize) {
    let table = srgb_to_linear_table();
    let (mut passing, mut eligible) = (0, 0);
    let mut entry_of = entries.iter();
    for (index, (level, placed)) in scene.levels.iter().zip(placed).enumerate() {
        for (page_index, page) in placed.packed.pages.iter().enumerate() {
            let (_, entry) = entry_of.next().unwrap();
            if skip.contains(&index) {
                continue;
            }
            let box_max = page.box_max.map(|v| f32::from(v) + 1.0);
            let origin = entry.origin();
            let page_hi: [f32; 3] =
                std::array::from_fn(|a| box_max[a].mul_add(entry.side(), origin[a]));
            let page_ok = entry.0[5] != 0 && !outside(matrix, origin, page_hi, margin);
            for local in 0..page.cells as usize {
                let instance: &CellInstance =
                    &placed.packed.instances[page_index * PAGE_SLOTS + local];
                let cell = &level.level.cells[page.first_cell + local];
                if !scene.drawn(level, cell) {
                    continue;
                }
                eligible += 1;
                let decoded = decode_cell(entry, instance, &table);
                if page_ok && !outside(matrix, decoded.lo, decoded.hi, margin) {
                    passing += 1;
                }
            }
        }
    }
    (passing, eligible)
}

#[test]
fn compaction_count_matches_a_cpu_count_on_every_adapter() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = build_scene(7);
        let camera = test_camera();
        let mut pool = new_pool(h);
        let placed = place(&mut pool, &scene, 0..);
        let page_entries = entries(&scene, &placed, camera.position());
        pool.set_active_pages(&page_entries).unwrap();
        let matrix = camera.eye_frame([WIDTH, HEIGHT]).unwrap().view_projection;
        // The decision must not depend on rounding noise: shifting the margin a little keeps it.
        let counts: Vec<_> = [0.9e-4, 1.0e-4, 1.1e-4]
            .map(|margin| {
                cpu_compaction_count(&scene, &placed, &page_entries, &matrix, margin, &[])
            })
            .to_vec();
        assert!(
            counts.windows(2).all(|pair| pair[0] == pair[1]),
            "{counts:?}"
        );
        let (expected, eligible) = counts[1];
        assert!(
            expected > 0 && expected < eligible,
            "{expected} of {eligible}"
        );
        let (_, gpu_count) = render_cells(h, &mut pool, &camera, false);
        assert_eq!(
            gpu_count as usize, expected,
            "{}: compacted {gpu_count}, CPU model {expected} of {eligible} eligible",
            h.label
        );
        println!(
            "# adapter {}: {gpu_count} cells compacted of {eligible} eligible",
            h.label
        );
        true
    });
}

#[test]
fn downlevel_limits_disable_cells_and_keep_the_cpu_path_identical() {
    let scene = build_scene(11);
    let instances = scene.cpu_instances();
    let camera = test_camera();
    let (mut ran, mut real_ran) = (0, 0);
    for adapter in &all_adapters() {
        let (Ok(default), Ok(downlevel)) = (
            harness(adapter, CellLimitMode::Auto),
            harness(adapter, CellLimitMode::DownlevelOnly),
        ) else {
            continue;
        };
        assert!(!downlevel.supported, "{}", downlevel.label);
        assert!(!cells_supported(
            &downlevel.adapter,
            &downlevel.device.limits()
        ));
        // The CPU path decides sizes with the same limits whatever the device was asked for.
        assert_eq!(default.limits.cpu_limits, downlevel.limits.cpu_limits);
        assert_eq!(
            downlevel.limits.required_limits,
            downlevel.limits.cpu_limits
        );
        let a = render_cpu(&default, &instances, &camera, true);
        let b = render_cpu(&downlevel, &instances, &camera, true);
        assert_eq!(a.color, b.color, "{}", default.label);
        assert_eq!(a.depth, b.depth, "{}", default.label);
        assert!(a.color.iter().filter(|c| **c != a.color[0]).count() > 1000);
        println!(
            "# adapter {}: downlevel device renders the CPU scene bit-identically (cells supported on the default device: {})",
            default.label, default.supported
        );
        ran += 1;
        real_ran += usize::from(default.real);
    }
    assert!(ran > 0, "no adapter ran the test");
    assert!(
        real_ran > 0 || std::env::var("CELL_POOL_REQUIRE_GPU").as_deref() != Ok("1"),
        "CELL_POOL_REQUIRE_GPU=1 but no real adapter ran"
    );
}

#[test]
fn reused_page_and_mask_slots_still_match_the_cpu_path() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let camera = test_camera();
        let first = build_scene(3);
        let mut pool = new_pool(h);
        let placed = place(&mut pool, &first, 0..);
        pool.set_active_pages(&entries(&first, &placed, camera.position()))
            .unwrap();
        run_oracle(h, &mut pool, &first, &camera, false);

        // Free everything, then fill the same slots (and mask slots) with another scene, in
        // descending order.
        let old_slots: Vec<u32> = placed.iter().flat_map(|p| p.slots.clone()).collect();
        let old_masks: Vec<u32> = placed.iter().map(|p| p.mask_slot).collect();
        pool.free_pages(&old_slots).unwrap();
        for slot in &old_masks {
            pool.free_mask_slot(*slot).unwrap();
        }
        let second = build_scene(4);
        let placed2 = place(&mut pool, &second, old_slots.iter().rev().copied());
        let new_masks: HashSet<u32> = placed2.iter().map(|p| p.mask_slot).collect();
        assert!(
            new_masks.iter().all(|slot| old_masks.contains(slot)),
            "mask slots come from the free list: {new_masks:?} vs {old_masks:?}"
        );
        let reused = placed2
            .iter()
            .flat_map(|p| &p.slots)
            .filter(|s| old_slots.contains(s))
            .count();
        assert!(reused > 5, "the second scene reuses slots of the first");
        pool.set_active_pages(&entries(&second, &placed2, camera.position()))
            .unwrap();
        let stats = run_oracle(h, &mut pool, &second, &camera, false);

        // A moved camera rewrites the eye-relative origins; the oracle holds there too.
        let mut moved = camera;
        assert!(moved.orbit(-0.4, 0.1));
        pool.set_active_pages(&entries(&second, &placed2, moved.position()))
            .unwrap();
        run_oracle(h, &mut pool, &second, &moved, false);
        println!("# adapter {}: reuse oracle {stats:?}", h.label);
        true
    });
}

#[test]
fn freed_pages_below_the_high_water_mark_stay_inert() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = build_scene(7);
        let camera = test_camera();
        let matrix = camera.eye_frame([WIDTH, HEIGHT]).unwrap().view_projection;
        let mut pool = new_pool(h);
        // Descending slots: the last level (the spanning node, whose cells skip the octant and
        // mask tests) gets the lowest slots, far below the high-water mark.
        let placed = place(&mut pool, &scene, (0..40).rev());
        let page_entries = entries(&scene, &placed, camera.position());
        pool.set_active_pages(&page_entries).unwrap();
        let model = |skip: &[usize]| {
            cpu_compaction_count(&scene, &placed, &page_entries, &matrix, 1.0e-4, skip).0
        };
        let (full, _) = (model(&[]), run_oracle(h, &mut pool, &scene, &camera, false));
        assert_eq!(render_cells(h, &mut pool, &camera, false).1 as usize, full);

        // An active page with octant word 0 draws nothing, even for a level that spans octants
        // (whose cells skip the octant test): that is the page pass guard, not the slot pass.
        let mut inert = page_entries.clone();
        for (slot, entry) in &mut inert {
            if placed[scene.levels.len() - 1].slots.contains(slot) {
                entry.0[5] = 0;
            }
        }
        pool.set_active_pages(&inert).unwrap();
        let spanning_cells = model(&[scene.levels.len() - 1]);
        assert!(spanning_cells < full);
        assert_eq!(
            render_cells(h, &mut pool, &camera, false).1 as usize,
            spanning_cells
        );
        pool.set_active_pages(&page_entries).unwrap();
        assert_eq!(render_cells(h, &mut pool, &camera, false).1 as usize, full);

        // set_active_pages -> free_pages -> encode, without a new active list.
        let spanning = scene.levels.len() - 1;
        let lowest_other = placed[..spanning]
            .iter()
            .flat_map(|p| p.slots.iter().copied())
            .min()
            .unwrap();
        assert!(placed[spanning].slots.iter().all(|s| *s < lowest_other));
        pool.free_pages(&placed[spanning].slots).unwrap();
        let without = model(&[spanning]);
        assert!(
            without < full,
            "the spanning cell must be on screen: {full} vs {without}"
        );
        assert_eq!(
            render_cells(h, &mut pool, &camera, false).1 as usize,
            without
        );
        run_oracle_against(
            h,
            &mut pool,
            &scene.cpu_instances_skipping(&[spanning]),
            &camera,
            false,
        );

        // Freeing the top slots lowers the high-water mark; the rest still draws.
        pool.free_pages(&placed[0].slots).unwrap();
        let without_both = model(&[spanning, 0]);
        assert!(without_both < without);
        assert_eq!(
            render_cells(h, &mut pool, &camera, false).1 as usize,
            without_both
        );
        run_oracle_against(
            h,
            &mut pool,
            &scene.cpu_instances_skipping(&[spanning, 0]),
            &camera,
            false,
        );
        true
    });
}

#[test]
fn pool_calls_reject_bad_input() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = build_scene(7);
        let camera = test_camera();
        let mut pool = new_pool(h);
        let placed = place(&mut pool, &scene, 0..);
        let page_entries = entries(&scene, &placed, camera.position());
        // Duplicate slots and unallocated mask slots are errors and leave the pool unchanged.
        let mut duplicate = page_entries.clone();
        duplicate[1].0 = duplicate[0].0;
        assert!(pool.set_active_pages(&duplicate).is_err());
        let mut bad_mask = page_entries.clone();
        bad_mask[0].1.0[4] = 60;
        assert!(pool.set_active_pages(&bad_mask).is_err());
        let slot = placed[0].mask_slot;
        pool.free_mask_slot(slot).unwrap();
        assert!(pool.free_mask_slot(slot).is_err(), "double free");
        assert!(pool.free_mask_slot(61).is_err(), "never allocated");
        assert!(pool.set_node_mask(slot, &[0; MASK_WORDS]).is_err());
        assert!(
            pool.set_active_pages(&page_entries).is_err(),
            "freed mask slot"
        );
        // A malformed level is an error, not a panic.
        let mut broken = placed[0].packed.clone();
        broken.instances.pop();
        assert!(pool.upload_level(&broken, &placed[0].slots).is_err());
        true
    });
}
