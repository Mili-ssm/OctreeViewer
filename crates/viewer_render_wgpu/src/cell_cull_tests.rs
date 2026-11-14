//! Task 7b2: two-phase per-cell Hi-Z occlusion against the compaction-only path (`k`).
//!
//! The scene is a closed shell of cells around a dense cloud of hidden ones, so a large share of
//! the pool really is occluded. Every frame is drawn twice, by a pool with occlusion (`kpc`) and by
//! an identical pool without it (`k`); with `depthCompare: less` the final depth is the minimum of
//! the fragments whatever the draw order, so the two depth images must be bit-identical. A pixel
//! nearer in `k` than in `kpc` is a hole: the culling was not conservative.

use super::*;
use crate::cell_cull::{
    HiZ, HiZBuilder, MARGIN_BASE_U, MARGIN_RHO_U, RECT_SLACK, RHO_MAX, depth_error_bound_u,
    depth_margin_u, hiz_shader_sources, level_size, mip_count,
};

const DEPTH_READ_WGSL: &str = "
@group(0) @binding(0) var src: texture_depth_2d;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(src);
    if id.x >= size.x || id.y >= size.y { return; }
    dst[id.y * size.x + id.x] = textureLoad(src, vec2<i32>(id.xy), 0);
}
";

struct Target {
    color: wgpu::Texture,
    depth: wgpu::Texture,
    size: [u32; 2],
}

struct Shot {
    depth: Vec<f32>,
    color: Vec<[u8; 4]>,
    /// Cells drawn by phase 1 and by phase 2 (plain compaction counts in the first).
    counts: [u32; 2],
}

fn target(h: &Harness, size: [u32; 2]) -> Target {
    let texture = |format| {
        h.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cull-test-target"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
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
    Target {
        color: texture(COLOR_FORMAT),
        depth: texture(wgpu::TextureFormat::Depth32Float),
        size,
    }
}

/// Encodes and submits one frame of `pool` with the given matrix; the pages are already active.
fn render_target(
    h: &Harness,
    pool: &mut CellRenderer,
    matrix: [[f32; 4]; 4],
    size: [u32; 2],
) -> Target {
    let target = target(h, size);
    let (color_view, depth_view) = (
        target.color.create_view(&Default::default()),
        target.depth.create_view(&Default::default()),
    );
    let mut encoder = h.device.create_command_encoder(&Default::default());
    pool.encode_cells(
        &mut encoder,
        &color_view,
        &depth_view,
        &CellFrame {
            view_projection: matrix,
            clear_color: clear_color(),
            depth_shading: false,
            viewport: size,
        },
    );
    h.queue.submit([encoder.finish()]);
    target
}

fn read_shot(h: &Harness, target: &Target, pool: &CellRenderer) -> Shot {
    let [width, height] = target.size;
    let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let staging = |size| {
        h.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cull-test-staging"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    };
    let color_out = staging(u64::from(row * height));
    let depth_out = staging(u64::from(width * height * 4));
    let count_out = staging(28);
    let depth_storage = h.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cull-test-depth"),
        size: u64::from(width * height * 4),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let module = h.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("cull-test-depth-read"),
        source: wgpu::ShaderSource::Wgsl(DEPTH_READ_WGSL.into()),
    });
    let pipeline = h
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cull-test-depth-read"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let depth_view = target.depth.create_view(&wgpu::TextureViewDescriptor {
        aspect: wgpu::TextureAspect::DepthOnly,
        ..Default::default()
    });
    let bind_group = h.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cull-test-depth-read"),
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
    let mut encoder = h.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target.color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &color_out,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
    }
    encoder.copy_buffer_to_buffer(
        &depth_storage,
        0,
        &depth_out,
        0,
        u64::from(width * height * 4),
    );
    encoder.copy_buffer_to_buffer(pool.draw_args_buffer(), 0, &count_out, 0, 28);
    h.queue.submit([encoder.finish()]);
    let color_bytes = read_buffer(h, &color_out);
    let color = (0..height as usize)
        .flat_map(|y| {
            color_bytes[y * row as usize..][..width as usize * 4]
                .as_chunks::<4>()
                .0
                .to_vec()
        })
        .collect();
    let words: Vec<u32> = bytemuck::pod_collect_to_vec(&read_buffer(h, &count_out));
    Shot {
        depth: bytemuck::pod_collect_to_vec(&read_buffer(h, &depth_out)),
        color,
        counts: [words[1], words[6]],
    }
}

fn shoot(h: &Harness, pool: &mut CellRenderer, matrix: [[f32; 4]; 4], size: [u32; 2]) -> Shot {
    let target = render_target(h, pool, matrix, size);
    read_shot(h, &target, pool)
}

// ---------------------------------------------------------------------------------------------
// Scene
// ---------------------------------------------------------------------------------------------

/// Appends a node with one level of `coords` (all drawn: octants set, mask clear).
fn add_plain_node(
    scene: &mut Scene,
    next_id: &mut u32,
    grid: CellGrid,
    depth: u8,
    mut coords: Vec<[u64; 3]>,
) {
    let node = scene.nodes.len();
    scene.nodes.push(TestNode {
        bounds: unit_bounds(&grid),
        grid,
        id: node as u64 + 1,
        node_depth: 0,
    });
    coords.sort_by_key(|coord| morton(*coord));
    let cells = coords
        .into_iter()
        .map(|coord| {
            *next_id += 1;
            cell(coord, *next_id)
        })
        .collect();
    scene.levels.push(TestLevel {
        node,
        level: NodeCellLevel {
            depth,
            cells,
            octant_offsets: None,
        },
        octants: 0xff,
        mask: [0; MASK_WORDS],
    });
}

/// Every cell of the 16 x 16 x 16 grid that touches the faces `ring` cells inside the border.
fn shell_coords(ring: u64) -> Vec<[u64; 3]> {
    let (lo, hi) = (ring, 15 - ring);
    let mut out = Vec::new();
    for x in lo..=hi {
        for y in lo..=hi {
            for z in lo..=hi {
                if [x, y, z].iter().any(|&v| v == lo || v == hi) {
                    out.push([x, y, z]);
                }
            }
        }
    }
    out
}

/// A closed shell at depth 4 (level 0) around `cloud` random cells of depth 5 inside it (level 1),
/// in the unit cube. From outside the camera sees only the shell.
fn occluded_scene(seed: u64, cloud: usize) -> Scene {
    let mut rng = Rng(seed);
    let mut next_id = 0;
    let mut scene = Scene {
        nodes: Vec::new(),
        levels: Vec::new(),
    };
    let grid = CellGrid {
        root_min: [0.0; 3],
        root_side: 1.0,
    };
    add_plain_node(&mut scene, &mut next_id, grid, 4, shell_coords(0));
    let mut seen = HashSet::new();
    let mut coords = Vec::new();
    while coords.len() < cloud {
        // Depth-5 coordinates 6..26 lie inside the second shell as well (depth-4 cells 3..13).
        let coord: [u64; 3] = std::array::from_fn(|_| 6 + rng.below(20));
        if seen.insert(coord) {
            coords.push(coord);
        }
    }
    add_plain_node(&mut scene, &mut next_id, grid, 5, coords);
    scene
}

/// The same scene with the shell one cell further in: other cells in the shell's page slots.
fn replace_shell(scene: &mut Scene, ring: u64) {
    let mut next_id = 90_000;
    let mut other = Scene {
        nodes: Vec::new(),
        levels: Vec::new(),
    };
    let grid = scene.nodes[0].grid;
    add_plain_node(&mut other, &mut next_id, grid, 4, shell_coords(ring));
    scene.levels[0] = other.levels.remove(0);
}

fn orbit_camera() -> OrbitCamera {
    test_camera()
}

fn matrix_of(camera: &OrbitCamera, size: [u32; 2]) -> [[f32; 4]; 4] {
    camera.eye_frame(size).unwrap().view_projection
}

/// A pool holding the scene's levels in consecutive slots from 0.
fn loaded(h: &Harness, scene: &Scene, occlusion: bool) -> (CellRenderer, Vec<Placed>) {
    let mut pool = new_occlusion_pool(h);
    pool.set_occlusion(occlusion);
    let placed = place(&mut pool, scene, 0..);
    (pool, placed)
}

fn show(
    h: &Harness,
    pool: &mut CellRenderer,
    scene: &Scene,
    placed: &[Placed],
    camera: &OrbitCamera,
    size: [u32; 2],
) -> Shot {
    pool.set_active_pages(&entries(scene, placed, camera.position()))
        .unwrap();
    shoot(h, pool, matrix_of(camera, size), size)
}

/// Asserts bit-identical depth and colour (no two drawn faces are coplanar and overlapping, so
/// draw order cannot change a colour).
fn assert_same_depth(label: &str, frame: usize, occluded: &Shot, plain: &Shot) {
    assert_same_depth_within(label, frame, occluded, plain, 0);
}

/// [`assert_same_depth`] allowing `colour_ties` pixels of different colour. Callers pass a
/// non-zero number only on the software adapter (`!h.real`): llvmpipe's threaded rasterizer
/// resolves exact depth ties between faces of neighbouring cells differently from run to run. Every
/// pixel has bit-equal depth in both images when this is reached, so a differing colour is such a
/// tie; real GPUs must give identical colour.
fn assert_same_depth_within(
    label: &str,
    frame: usize,
    occluded: &Shot,
    plain: &Shot,
    colour_ties: usize,
) {
    let holes = occluded
        .depth
        .iter()
        .zip(&plain.depth)
        .filter(|(a, b)| a > b)
        .count();
    let different = occluded
        .depth
        .iter()
        .zip(&plain.depth)
        .filter(|(a, b)| a.to_bits() != b.to_bits())
        .count();
    assert_eq!(
        (holes, different),
        (0, 0),
        "{label}: frame {frame}: {holes} holes, {different} pixels differ in depth"
    );
    assert!(
        plain.depth.iter().filter(|d| **d < 1.0).count() > 3000,
        "{label}: frame {frame}: the scene is nearly off screen"
    );
    let colors = occluded
        .color
        .iter()
        .zip(&plain.color)
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        colors <= colour_ties,
        "{label}: frame {frame}: {colors} pixels differ in colour"
    );
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[test]
fn occlusion_shaders_validate_with_naga() {
    let validate = |source: &str| {
        let module = naga::front::wgsl::parse_str(source).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    };
    for source in hiz_shader_sources() {
        validate(source);
    }
    assert_eq!(mip_count([1, 1]), 1);
    assert_eq!(mip_count([256, 192]), 9);
    assert_eq!(mip_count([255, 1]), 8);
    assert_eq!(mip_count([301, 177]), 9);
}

#[test]
fn slot_pass_stays_within_eight_storage_buffers() {
    // Per pipeline: the compute group binds seven storage buffers (instances, pages, masks, page
    // list, visible indices, draw arguments, visibility bits) and finalize adds the dispatch
    // arguments, so finalize reaches the WebGPU minimum of 8 exactly.
    const { assert!(7 < crate::cell_render::MIN_STORAGE_BUFFERS) };
    for (source_index, source) in crate::cell_render::shader_sources().iter().enumerate() {
        let module = naga::front::wgsl::parse_str(source).unwrap();
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
        for (index, entry) in module.entry_points.iter().enumerate() {
            let function_info = info.get_entry_point(index);
            let storage = module
                .global_variables
                .iter()
                .filter(|(handle, variable)| {
                    matches!(variable.space, naga::AddressSpace::Storage { .. })
                        && !function_info[*handle].is_empty()
                })
                .count();
            println!(
                "# source {source_index} {}: {storage} storage buffers",
                entry.name
            );
            assert!(storage <= 7, "{}: {storage} storage buffers", entry.name);
            if entry.name == "slot_main" {
                assert_eq!(storage, 7);
            }
            if entry.name == "finalize_main" {
                // Its three, plus the four it leaves to the other entry points of the group.
                assert!(storage + 4 < 9);
            }
        }
    }
}

/// The pyramid texel is the maximum of the pixels it covers, on odd sizes too.
#[test]
fn hiz_texels_are_the_maximum_of_their_pixels_on_every_adapter() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = occluded_scene(5, 3000);
        let camera = orbit_camera();
        let builder = HiZBuilder::new(&h.device);
        for size in [[256, 192], [301, 177], [13, 7], [1, 40]] {
            let (mut pool, placed) = loaded(h, &scene, false);
            pool.set_active_pages(&entries(&scene, &placed, camera.position()))
                .unwrap();
            let target = render_target(h, &mut pool, matrix_of(&camera, size), size);
            let shot = read_shot(h, &target, &pool);
            let hiz = HiZ::new(&h.device, &builder, size);
            assert_eq!(hiz.mip_count(), mip_count(size));
            let mut encoder = h.device.create_command_encoder(&Default::default());
            hiz.encode_build(
                &h.device,
                &builder,
                &mut encoder,
                &target.depth.create_view(&Default::default()),
            );
            h.queue.submit([encoder.finish()]);
            for level in 0..hiz.mip_count() {
                let (w, h_px) = (
                    level_size(size[0], level) as usize,
                    level_size(size[1], level) as usize,
                );
                let texels = read_hiz_level(h, &hiz, level, [w as u32, h_px as u32]);
                for y in 0..h_px {
                    for x in 0..w {
                        let mut expected = f32::MIN;
                        // The last texel of an axis extends to the edge of the viewport.
                        let end = |i: usize, extent: usize, limit: u32| {
                            if i + 1 == extent {
                                limit as usize
                            } else {
                                (i + 1) << level
                            }
                        };
                        for py in (y << level)..end(y, h_px, size[1]) {
                            for px in (x << level)..end(x, w, size[0]) {
                                expected = expected.max(shot.depth[py * size[0] as usize + px]);
                            }
                        }
                        assert_eq!(
                            texels[y * w + x].to_bits(),
                            expected.to_bits(),
                            "{}: size {size:?} level {level} texel ({x}, {y})",
                            h.label
                        );
                    }
                }
            }
        }
        true
    });
}

fn read_hiz_level(h: &Harness, hiz: &HiZ, level: u32, size: [u32; 2]) -> Vec<f32> {
    let row = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = h.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hiz-readback"),
        size: u64::from(row * size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = h.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: hiz.texture(),
            mip_level: level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(size[1]),
            },
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
    h.queue.submit([encoder.finish()]);
    let bytes = read_buffer(h, &buffer);
    (0..size[1] as usize)
        .flat_map(|y| {
            bytemuck::pod_collect_to_vec::<u8, f32>(
                &bytes[y * row as usize..][..size[0] as usize * 4],
            )
        })
        .collect()
}

#[test]
fn still_frame_depth_is_identical_with_occlusion_on_every_adapter() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = occluded_scene(21, 6000);
        let camera = orbit_camera();
        let size = [WIDTH, HEIGHT];
        let (mut occluded, placed) = loaded(h, &scene, true);
        let (mut plain, plain_placed) = loaded(h, &scene, false);
        let reference = show(h, &mut plain, &scene, &plain_placed, &camera, size);
        let total = reference.counts[0];
        for frame in 0..5 {
            let shot = show(h, &mut occluded, &scene, &placed, &camera, size);
            assert_same_depth(&h.label, frame, &shot, &reference);
            let [phase1, phase2] = shot.counts;
            println!(
                "# {} still frame {frame}: phase 1 {phase1}, phase 2 {phase2}, k {total}",
                h.label
            );
            if frame == 0 {
                // No bits yet: everything goes through phase 2.
                assert_eq!((phase1, phase2), (0, total));
            } else {
                assert!(
                    phase1 > 0,
                    "{}: frame {frame}: phase 1 drew nothing",
                    h.label
                );
            }
            if frame >= 2 {
                assert!(
                    phase1 + phase2 < total,
                    "{}: frame {frame}: occlusion culled nothing ({phase1} + {phase2} of {total})",
                    h.label
                );
                assert!(
                    (phase1 + phase2) * 2 < total,
                    "{}: frame {frame}: drew {} of {total}, not under half",
                    h.label,
                    phase1 + phase2
                );
                assert!(
                    phase2 < phase1,
                    "{}: frame {frame}: phase 2 still draws most ({phase2} vs {phase1})",
                    h.label
                );
            }
        }
        true
    });
}

#[test]
fn moving_camera_depth_is_identical_from_a_cold_start_on_every_adapter() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = occluded_scene(33, 6000);
        let mut camera = orbit_camera();
        let size = [WIDTH, HEIGHT];
        let (mut occluded, placed) = loaded(h, &scene, true);
        let (mut plain, plain_placed) = loaded(h, &scene, false);
        let mut culled_frames = 0;
        for frame in 0..10 {
            let reference = show(h, &mut plain, &scene, &plain_placed, &camera, size);
            let shot = show(h, &mut occluded, &scene, &placed, &camera, size);
            assert_same_depth(&h.label, frame, &shot, &reference);
            let [phase1, phase2] = shot.counts;
            println!(
                "# {} moving frame {frame}: phase 1 {phase1}, phase 2 {phase2}, k {}",
                h.label, reference.counts[0]
            );
            if frame > 0 {
                assert!(
                    phase1 > 0,
                    "{}: frame {frame}: phase 1 drew nothing",
                    h.label
                );
            }
            culled_frames += usize::from(phase1 + phase2 < reference.counts[0]);
            assert!(camera.orbit(0.004, 0.0015));
        }
        assert!(culled_frames >= 6, "{}: culled in {culled_frames}", h.label);
        // A camera cut starts from empty bits and is still exact.
        occluded.reset_visibility();
        assert!(camera.orbit(0.2, 0.05));
        let reference = show(h, &mut plain, &scene, &plain_placed, &camera, size);
        let shot = show(h, &mut occluded, &scene, &placed, &camera, size);
        assert_same_depth(&h.label, 10, &shot, &reference);
        assert_eq!(
            shot.counts,
            [0, reference.counts[0]],
            "{}: a reset frame draws everything in phase 2",
            h.label
        );
        true
    });
}

#[test]
fn reused_page_slots_start_with_cleared_bits_on_every_adapter() {
    swap_shell(true);
}

/// `upload_level` does not require the slots to be freed first, so it clears their bits too.
#[test]
fn upload_over_active_slots_starts_with_cleared_bits_on_every_adapter() {
    swap_shell(false);
}

fn swap_shell(free_first: bool) {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let mut scene = occluded_scene(44, 6000);
        let mut camera = orbit_camera();
        let size = [WIDTH, HEIGHT];
        let (mut occluded, mut placed) = loaded(h, &scene, true);
        let (mut plain, mut plain_placed) = loaded(h, &scene, false);
        for frame in 0..4 {
            let reference = show(h, &mut plain, &scene, &plain_placed, &camera, size);
            let shot = show(h, &mut occluded, &scene, &placed, &camera, size);
            assert_same_depth(&h.label, frame, &shot, &reference);
        }
        // The shell is the only thing the camera sees: its bits are set. Free it and put another
        // shell into the same slots, in both pools.
        let shell_slots = placed[0].slots.clone();
        for (pool, placed) in [
            (&mut occluded, &mut placed),
            (&mut plain, &mut plain_placed),
        ] {
            if free_first {
                pool.free_pages(&shell_slots).unwrap();
            }
            pool.free_mask_slot(placed[0].mask_slot).unwrap();
        }
        replace_shell(&mut scene, 1);
        for (pool, placed) in [
            (&mut occluded, &mut placed),
            (&mut plain, &mut plain_placed),
        ] {
            let node = &scene.nodes[0];
            let packed = pack_level(
                &node.grid,
                node.bounds,
                NodeId::new(node.id),
                &scene.levels[0].level,
            );
            let slots: Vec<u32> = (0..64)
                .filter(|slot| {
                    shell_slots.contains(slot)
                        || !placed.iter().any(|other| other.slots.contains(slot))
                })
                .take(packed.pages.len())
                .collect();
            let common = slots.len().min(shell_slots.len());
            assert_eq!(
                slots[..common],
                shell_slots[..common],
                "the new level must start in the freed slots"
            );
            pool.upload_level(&packed, &slots).unwrap();
            let mask_slot = pool.alloc_mask_slot().unwrap();
            pool.set_node_mask(mask_slot, &scene.levels[0].mask)
                .unwrap();
            placed[0] = Placed {
                packed,
                slots,
                mask_slot,
            };
        }
        for frame in 4..9 {
            let reference = show(h, &mut plain, &scene, &plain_placed, &camera, size);
            let shot = show(h, &mut occluded, &scene, &placed, &camera, size);
            assert_same_depth(&h.label, frame, &shot, &reference);
            let [phase1, phase2] = shot.counts;
            println!(
                "# {} after the swap, frame {frame}: phase 1 {phase1}, phase 2 {phase2}",
                h.label
            );
            if frame == 4 {
                // Nothing in the pool was visible before the swap except the old shell, whose
                // bits must not leak into the cells that now sit in its slots.
                assert_eq!(phase1, 0, "{}: stale bits drew {phase1} cells", h.label);
                assert!(phase2 > 0);
            } else {
                assert!(
                    phase1 > 0,
                    "{}: frame {frame}: phase 1 drew nothing",
                    h.label
                );
            }
            assert!(camera.orbit(0.003, 0.0));
        }
        true
    });
}

#[test]
fn resize_rebuilds_the_hiz_and_depth_stays_identical_on_every_adapter() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = occluded_scene(55, 6000);
        let camera = orbit_camera();
        let (mut occluded, placed) = loaded(h, &scene, true);
        let (mut plain, plain_placed) = loaded(h, &scene, false);
        let mut frame = 0;
        for size in [[WIDTH, HEIGHT], [301, 177], [WIDTH, HEIGHT], [97, 61]] {
            for _ in 0..4 {
                let reference = show(h, &mut plain, &scene, &plain_placed, &camera, size);
                let shot = show(h, &mut occluded, &scene, &placed, &camera, size);
                assert_eq!(occluded.hiz().size(), size);
                assert_eq!(occluded.hiz().mip_count(), mip_count(size));
                assert_same_depth(&h.label, frame, &shot, &reference);
                frame += 1;
            }
            // The size has not changed on this size's last frame, so phase 1 has drawn: bits
            // survive frames, and they restart only when the size changes.
        }
        // The explicit invalidation (surface `Outdated`) rebuilds as well.
        occluded.invalidate_viewport();
        let reference = show(
            h,
            &mut plain,
            &scene,
            &plain_placed,
            &camera,
            [WIDTH, HEIGHT],
        );
        let shot = show(h, &mut occluded, &scene, &placed, &camera, [WIDTH, HEIGHT]);
        assert_same_depth(&h.label, frame, &shot, &reference);
        assert_eq!(
            shot.counts[0], 0,
            "{}: invalidation restarts the bits",
            h.label
        );
        true
    });
}

#[test]
fn toggling_occlusion_off_and_on_restarts_from_empty_bits_on_every_adapter() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = occluded_scene(66, 4000);
        let camera = orbit_camera();
        let size = [WIDTH, HEIGHT];
        let (mut pool, placed) = loaded(h, &scene, true);
        let (mut plain, plain_placed) = loaded(h, &scene, false);
        let reference = show(h, &mut plain, &scene, &plain_placed, &camera, size);
        for _ in 0..3 {
            show(h, &mut pool, &scene, &placed, &camera, size);
        }
        // A reference frame (occlusion off) must not flip or reuse the halves.
        pool.set_occlusion(false);
        let off = show(h, &mut pool, &scene, &placed, &camera, size);
        assert_same_depth(&h.label, 0, &off, &reference);
        assert_eq!(off.counts, [reference.counts[0], 0]);
        pool.set_occlusion(true);
        let on = show(h, &mut pool, &scene, &placed, &camera, size);
        assert_same_depth(&h.label, 1, &on, &reference);
        assert_eq!(on.counts[0], 0);
        let next = show(h, &mut pool, &scene, &placed, &camera, size);
        assert_same_depth(&h.label, 2, &next, &reference);
        assert!(next.counts[0] > 0);
        true
    });
}

// ---------------------------------------------------------------------------------------------
// Depth margin
// ---------------------------------------------------------------------------------------------

/// Minimum over the eight corners of `z / w`, computed with the shader's own `cell_clip` and
/// `cell_corner` (f32, as compiled for the GPU). `2.0` marks a box with a corner behind the near
/// plane, which the test would not use.
const NEAREST_CORNER_WGSL: &str = "
@group(1) @binding(0) var<storage, read> boxes: array<vec4<f32>>;
@group(1) @binding(1) var<storage, read_write> nearest: array<f32>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= arrayLength(&boxes) / 2u { return; }
    let lo = boxes[2u * i].xyz;
    let hi = boxes[2u * i + 1u].xyz;
    var best = 3.0e38;
    var valid = true;
    for (var c = 0u; c < 8u; c++) {
        let q = cell_clip(cell_corner(lo, hi, c));
        if !(q.w > 1.0e-30) || !(q.z >= 0.0) { valid = false; }
        best = min(best, q.z / q.w);
    }
    nearest[i] = select(2.0, best, valid);
}
";

/// A cell whose colour identifies it: `r` and `g` step by 4 so that a rounding of one level in the
/// framebuffer cannot confuse two cells (up to 2500 cells).
fn spaced_cell(coord: [u64; 3], key: u32) -> NodeCell {
    NodeCell {
        rgb_sum: [
            u64::from(16 + key % 50 * 4),
            u64::from(16 + key / 50 * 4),
            200,
        ],
        ..cell(coord, 0)
    }
}

fn spaced_key(rgb: [u64; 3]) -> u32 {
    ((rgb[1] as u32 - 16) / 4) * 50 + (rgb[0] as u32 - 16) / 4
}

/// Nodes of 700 random cells at depths 4, 5 and 6 on offset grids of different sizes, so cells span
/// two orders of magnitude of size; unique identifying colours.
fn spaced_scene(seed: u64) -> Scene {
    let mut rng = Rng(seed);
    let mut scene = Scene {
        nodes: Vec::new(),
        levels: Vec::new(),
    };
    let mut key = 0;
    for (depth, side, min) in [
        (4_u8, 1.0, [0.0; 3]),
        (5, 0.8, [0.1, 0.05, 0.12]),
        (6, 0.3, [0.4, 0.3, 0.35]),
    ] {
        let grid = CellGrid {
            root_min: min,
            root_side: side,
        };
        let node = scene.nodes.len();
        scene.nodes.push(TestNode {
            bounds: unit_bounds(&grid),
            grid,
            id: node as u64 + 1,
            node_depth: 0,
        });
        let mut seen = HashSet::new();
        let mut coords = Vec::new();
        while coords.len() < 700 {
            let coord: [u64; 3] = std::array::from_fn(|_| rng.below(1 << depth));
            if seen.insert(coord) {
                coords.push(coord);
            }
        }
        coords.sort_by_key(|coord| morton(*coord));
        let cells = coords
            .into_iter()
            .map(|coord| {
                key += 1;
                spaced_cell(coord, key)
            })
            .collect();
        scene.levels.push(TestLevel {
            node,
            level: NodeCellLevel {
                depth,
                cells,
                octant_offsets: None,
            },
            octants: 0xff,
            mask: [0; MASK_WORDS],
        });
    }
    scene
}

/// `(key, lo, hi)` of every drawn cell, as the shader decodes it.
fn spaced_boxes(scene: &Scene, placed: &[Placed], eye: [f64; 3]) -> Vec<(u32, [f32; 3], [f32; 3])> {
    let table = srgb_to_linear_table();
    let page_entries = entries(scene, placed, eye);
    let mut entry_of = page_entries.iter();
    let mut out = Vec::new();
    for (level, placed) in scene.levels.iter().zip(placed) {
        for (page_index, page) in placed.packed.pages.iter().enumerate() {
            let (_, entry) = entry_of.next().unwrap();
            for local in 0..page.cells as usize {
                let instance = &placed.packed.instances[page_index * PAGE_SLOTS + local];
                let cell = &level.level.cells[page.first_cell + local];
                let decoded = decode_cell(entry, instance, &table);
                out.push((spaced_key(cell.rgb_sum), decoded.lo, decoded.hi));
            }
        }
    }
    out
}

/// `max |corner| / w` over the eight corners of a box, the `rho` of the margin derivation, from the
/// f32 matrix evaluated in f64.
fn box_rho(matrix: &[[f32; 4]; 4], lo: [f32; 3], hi: [f32; 3]) -> f64 {
    (0..8)
        .map(|corner| {
            let p: [f64; 3] = std::array::from_fn(|a| {
                f64::from(if corner >> a & 1 == 1 { hi[a] } else { lo[a] })
            });
            let w: f64 = (0..3).map(|a| f64::from(matrix[a][3]) * p[a]).sum();
            (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() / w
        })
        .fold(0.0, f64::max)
}

/// The shader's margin is a sound bound at every `rho` it accepts: the linear form is never below
/// the derived error bound, and the bound starts near 48 u.
#[test]
fn depth_margin_dominates_the_derived_bound() {
    let mut rho = 1.0;
    while rho <= RHO_MAX {
        let (bound, margin) = (depth_error_bound_u(rho), depth_margin_u(rho));
        assert!(
            bound <= margin,
            "rho {rho}: bound {bound} u > margin {margin} u"
        );
        assert!(
            margin - bound < 0.1 * margin,
            "rho {rho}: margin {margin} u is loose"
        );
        rho *= 1.07;
    }
    assert!((depth_error_bound_u(1.0) - 48.0).abs() < 1.0);
    assert_eq!((MARGIN_RHO_U, MARGIN_BASE_U), (25.0, 24.0));
}

/// How far a rasterized fragment falls below the nearest corner depth of its own cell, where the
/// corner depths come from the shader's `cell_clip` run on the GPU (the quantity the occlusion test
/// relies on: a pixel of the cell is never nearer than its nearest corner, up to this error).
/// Absolute NDC depth, in units of u = 2^-24, over 2100 cells seen from 30 real camera poses
/// (random orbit and zoom, so oblique faces, cells near the eye and near the far plane) and from
/// worst-case poses: far away (eye-relative coordinates up to 1e3 times the scene, depths against
/// the far plane), grazing (the pitch clamp) and next to the cells. Every pixel names its cell by
/// its unique colour. The error of each pixel must stay within `depth_error_bound_u` at the `rho`
/// of its cell, so no visible cell can be culled by the margin.
#[test]
fn depth_error_stays_within_the_derived_bound_at_worst_case_poses() {
    let ulp = 2.0_f64.powi(-24);
    let mut overall = f64::MIN;
    let mut adapters = Vec::new();
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let size = [WIDTH, HEIGHT];
        let scene = spaced_scene(2024);
        let (mut pool, placed) = loaded(h, &scene, false);
        let source = format!(
            "{}{}{NEAREST_CORNER_WGSL}",
            crate::cell_pages::CELL_PAGES_WGSL,
            crate::cell_render::COMMON_WGSL
        );
        let module = h.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nearest-corner"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = h
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("nearest-corner"),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
        let mut rng = Rng(0xfeed);
        let unit = |rng: &mut Rng| rng.below(1 << 24) as f64 / f64::from(1 << 24);
        let (mut worst, mut compared, mut near_one, mut deepest) =
            (f64::MIN, 0_usize, 0_usize, 0.0_f32);
        let mut camera = orbit_camera();
        let mut worst_slack = f64::MAX;
        let mut max_rho = 0.0_f64;
        // Worst-case poses after the random ones: (orbit x, orbit y, zoom).
        let extreme = [
            (0.1, 1.0, 6.0),
            (0.37, -1.0, 7.0),
            (0.6, 1.0, -3.0),
            (0.2, 0.05, 9.0),
            (0.8, -1.0, -3.0),
            (0.45, 0.02, -2.0),
        ];
        for pose in 0..30 + extreme.len() {
            if let Some((x, y, zoom)) = pose.checked_sub(30).map(|i| extreme[i]) {
                camera = orbit_camera();
                assert!(camera.orbit(x, y));
                assert!(camera.zoom(zoom));
            } else {
                assert!(camera.orbit(unit(&mut rng), (unit(&mut rng) - 0.5) * 0.3));
                assert!(camera.zoom((unit(&mut rng) - 0.5) * 3.0));
            }
            let matrix = matrix_of(&camera, size);
            let boxes = spaced_boxes(&scene, &placed, camera.position());
            let flat: Vec<[f32; 4]> = boxes
                .iter()
                .flat_map(|(_, lo, hi)| [[lo[0], lo[1], lo[2], 0.0], [hi[0], hi[1], hi[2], 0.0]])
                .collect();
            let mut frame_bytes = vec![0_u8; 96];
            frame_bytes[..64].copy_from_slice(bytemuck::bytes_of(&matrix));
            let make = |label, contents: &[u8], usage| {
                h.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(label),
                        contents,
                        usage,
                    })
            };
            let frame = make("frame", &frame_bytes, wgpu::BufferUsages::UNIFORM);
            let boxes_buffer = make(
                "boxes",
                bytemuck::cast_slice(&flat),
                wgpu::BufferUsages::STORAGE,
            );
            let nearest = h.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nearest"),
                size: boxes.len() as u64 * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let out = h.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nearest-out"),
                size: boxes.len() as u64 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let group = |index, entries: &[wgpu::BindGroupEntry]| {
                h.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &pipeline.get_bind_group_layout(index),
                    entries,
                })
            };
            let group0 = group(
                0,
                &[wgpu::BindGroupEntry {
                    binding: 1,
                    resource: frame.as_entire_binding(),
                }],
            );
            let group1 = group(
                1,
                &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: boxes_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: nearest.as_entire_binding(),
                    },
                ],
            );
            let mut encoder = h.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group0, &[]);
                pass.set_bind_group(1, &group1, &[]);
                pass.dispatch_workgroups((boxes.len() as u32).div_ceil(64), 1, 1);
            }
            encoder.copy_buffer_to_buffer(&nearest, 0, &out, 0, boxes.len() as u64 * 4);
            h.queue.submit([encoder.finish()]);
            let corner: Vec<f32> = bytemuck::pod_collect_to_vec(&read_buffer(h, &out));
            let mut by_key = vec![2.0_f32; 2500];
            let mut rho_of = vec![0.0_f64; 2500];
            for ((key, lo, hi), depth) in boxes.iter().zip(&corner) {
                by_key[*key as usize] = *depth;
                rho_of[*key as usize] = box_rho(&matrix, *lo, *hi);
            }
            pool.set_active_pages(&entries(&scene, &placed, camera.position()))
                .unwrap();
            let shot = shoot(h, &mut pool, matrix, size);
            for (pixel, depth) in shot.color.iter().zip(&shot.depth) {
                if *depth >= 1.0 {
                    continue;
                }
                let step = |value: u8, base: i32| (i32::from(value) - base + 2).div_euclid(4);
                let (x, y) = (step(pixel[0], 16), step(pixel[1], 16));
                if (i32::from(pixel[0]) - 16 - 4 * x).abs() > 1
                    || (i32::from(pixel[1]) - 16 - 4 * y).abs() > 1
                    || (i32::from(pixel[2]) - 200).abs() > 1
                    || !(0..50).contains(&x)
                    || !(0..50).contains(&y)
                {
                    continue;
                }
                let nearest_corner = by_key[(y * 50 + x) as usize];
                if nearest_corner > 1.5 {
                    continue;
                }
                compared += 1;
                near_one += usize::from(*depth > 0.999);
                deepest = deepest.max(*depth);
                let error = (f64::from(nearest_corner) - f64::from(*depth)) / ulp;
                worst = worst.max(error);
                let rho = rho_of[(y * 50 + x) as usize];
                max_rho = max_rho.max(rho);
                worst_slack = worst_slack.min(depth_error_bound_u(rho) - error);
            }
        }
        println!(
            "# {}: worst nearer-than-nearest-corner {worst:.2} u over {compared} pixels \
             ({near_one} deeper than 0.999, deepest {deepest}); smallest slack to the bound \
             {worst_slack:.2} u, largest rho {max_rho:.2}",
            h.label
        );
        assert!(
            worst_slack > 0.0,
            "{}: a fragment is nearer than the derived bound allows",
            h.label
        );
        assert!(
            max_rho > 8.0,
            "{}: no worst-case pose reached rho > 8",
            h.label
        );
        assert!(
            compared > 20_000,
            "{}: only {compared} pixels compared",
            h.label
        );
        assert!(
            near_one > 1000,
            "{}: only {near_one} pixels near depth 1",
            h.label
        );
        overall = overall.max(worst);
        adapters.push(h.label.clone());
        true
    });
    println!("# worst measured error {overall:.2} u on {adapters:?}");
}

// ---------------------------------------------------------------------------------------------
// Disocclusion, near plane, screen edge, non-finite input
// ---------------------------------------------------------------------------------------------

/// What the first frame after a cold start exercises, from a CPU model of the shader. Phase 1 draws
/// nothing then (the tests assert it), so every cell that survives the frustum test reaches the
/// Hi-Z test in phase 2. Returns: cells with a corner behind the near plane (the "not occluded"
/// exit), cells whose rectangle leaves the viewport (clamp to the viewport), and cells whose
/// rectangle leaves the shader's `[-2, size + 2]` bounds (the first clamp).
fn path_coverage(
    scene: &Scene,
    placed: &[Placed],
    matrix: &[[f32; 4]; 4],
    eye: [f64; 3],
    size: [u32; 2],
) -> (usize, usize, usize) {
    let table = srgb_to_linear_table();
    let page_entries = entries(scene, placed, eye);
    let mut entry_of = page_entries.iter();
    let (mut near, mut clamped, mut hard_clamped) = (0, 0, 0);
    for (level, placed) in scene.levels.iter().zip(placed) {
        for (page_index, page) in placed.packed.pages.iter().enumerate() {
            let (_, entry) = entry_of.next().unwrap();
            for local in 0..page.cells as usize {
                let instance = &placed.packed.instances[page_index * PAGE_SLOTS + local];
                let cell = &level.level.cells[page.first_cell + local];
                if !scene.drawn(level, cell) {
                    continue;
                }
                let decoded = decode_cell(entry, instance, &table);
                if outside(matrix, decoded.lo, decoded.hi, 1.0e-4) {
                    continue;
                }
                let clip: Vec<[f64; 4]> = (0..8_u32)
                    .map(|index| {
                        let upper = [((index + 1) >> 1) & 1, (index >> 1) & 1, index >> 2];
                        let p: [f64; 3] = std::array::from_fn(|a| {
                            f64::from(if upper[a] == 1 {
                                decoded.hi[a]
                            } else {
                                decoded.lo[a]
                            })
                        });
                        std::array::from_fn(|row| {
                            (0..3)
                                .map(|c| f64::from(matrix[c][row]) * p[c])
                                .sum::<f64>()
                                + f64::from(matrix[3][row])
                        })
                    })
                    .collect();
                if clip.iter().any(|q| q[3] <= 1.0e-30 || q[2] < 0.0) {
                    near += 1;
                    continue;
                }
                let px = |axis: usize, extent: u32, flip: bool| {
                    let v: Vec<f64> = clip
                        .iter()
                        .map(|q| {
                            let ndc = q[axis] / q[3];
                            (if flip {
                                0.5 - ndc * 0.5
                            } else {
                                ndc * 0.5 + 0.5
                            }) * f64::from(extent)
                        })
                        .collect();
                    (
                        v.iter().copied().fold(f64::MAX, f64::min),
                        v.iter().copied().fold(f64::MIN, f64::max),
                    )
                };
                let (x0, x1) = px(0, size[0], false);
                let (y0, y1) = px(1, size[1], true);
                // The shader's rectangle: pixel centres and slack, then clamped to [-2, size + 2]
                // and again to the viewport.
                let (slack, extent) = (0.5 + f64::from(RECT_SLACK), size.map(f64::from));
                let (low, high) = (
                    [x0 - slack, y0 - slack],
                    [x1 - 1.0 + slack, y1 - 1.0 + slack],
                );
                if (0..2).any(|a| low[a] < -2.0 || high[a] > extent[a] + 2.0) {
                    hard_clamped += 1;
                }
                if (0..2).any(|a| low[a] < 0.0 || high[a] > extent[a] - 1.0) {
                    clamped += 1;
                }
            }
        }
    }
    (near, clamped, hard_clamped)
}

#[test]
fn disocclusion_after_a_large_orbit_is_drawn_in_phase_2_on_every_adapter() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = occluded_scene(77, 6000);
        let mut camera = orbit_camera();
        let size = [WIDTH, HEIGHT];
        let (mut occluded, placed) = loaded(h, &scene, true);
        let (mut plain, plain_placed) = loaded(h, &scene, false);
        for frame in 0..3 {
            let reference = show(h, &mut plain, &scene, &plain_placed, &camera, size);
            let shot = show(h, &mut occluded, &scene, &placed, &camera, size);
            assert_same_depth(&h.label, frame, &shot, &reference);
        }
        let (mut max_phase2, mut culling_frames) = (0, 0);
        for frame in 3..9 {
            // 0.2 turns (1.26 rad) per frame: faces hidden last frame come into view. A step of
            // 0.5 rad only reveals about 70 cells.
            assert!(camera.orbit(0.2, 0.0));
            let reference = show(h, &mut plain, &scene, &plain_placed, &camera, size);
            let shot = show(h, &mut occluded, &scene, &placed, &camera, size);
            assert_same_depth(&h.label, frame, &shot, &reference);
            let [phase1, phase2] = shot.counts;
            println!(
                "# {} big orbit frame {frame}: phase 1 {phase1}, phase 2 {phase2}, k {}",
                h.label, reference.counts[0]
            );
            max_phase2 = max_phase2.max(phase2);
            culling_frames += usize::from(phase1 + phase2 < reference.counts[0]);
        }
        assert!(
            max_phase2 > 100,
            "{}: phase 2 drew at most {max_phase2}",
            h.label
        );
        assert!(
            culling_frames >= 4,
            "{}: culled in {culling_frames}",
            h.label
        );
        true
    });
}

#[test]
fn camera_inside_the_shell_and_cells_off_screen_keep_depth_identical_on_every_adapter() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = occluded_scene(88, 6000);
        let size = [WIDTH, HEIGHT];
        let mut inside = orbit_camera();
        // The eye sits inside a wall cell, so cells straddle the near plane.
        assert!(inside.set_pose([0.03, 0.5, 0.5], [0.5, 0.41, 0.53]));
        let mut zoomed = orbit_camera();
        assert!(zoomed.zoom(-1.0));
        for (name, camera, expect_near) in [("inside", inside, true), ("zoomed", zoomed, false)] {
            let (mut occluded, placed) = loaded(h, &scene, true);
            let (mut plain, plain_placed) = loaded(h, &scene, false);
            let (near, clamped, hard_clamped) = path_coverage(
                &scene,
                &placed,
                &matrix_of(&camera, size),
                camera.position(),
                size,
            );
            println!(
                "# {} {name}: {near} cells behind the near plane, {clamped} clamped to the \
                 viewport, {hard_clamped} clamped to [-2, size + 2]",
                h.label
            );
            if expect_near {
                assert!(near > 0, "{name}: no cell straddles the near plane");
            }
            assert!(clamped > 0, "{name}: no cell leaves the viewport");
            assert!(
                hard_clamped > 0,
                "{name}: no rectangle passes the [-2, size + 2] clamp"
            );
            for frame in 0..4 {
                let reference = show(h, &mut plain, &scene, &plain_placed, &camera, size);
                let shot = show(h, &mut occluded, &scene, &placed, &camera, size);
                assert_same_depth_within(
                    &format!("{} {name}", h.label),
                    frame,
                    &shot,
                    &reference,
                    if h.real { 0 } else { 8 },
                );
                if frame == 0 {
                    assert_eq!(
                        shot.counts[0], 0,
                        "{name}: phase 1 must draw nothing on a cold start"
                    );
                }
            }
        }
        true
    });
}

/// NaN and infinite matrices must not occlude anything: every cell the frustum test keeps is drawn.
#[test]
fn non_finite_corners_are_never_occluded_on_every_adapter() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = occluded_scene(99, 2000);
        let size = [WIDTH, HEIGHT];
        let camera = orbit_camera();
        let mut kept = 0;
        for (name, poison) in [
            ("nan", f32::NAN),
            ("inf", f32::INFINITY),
            ("-inf", f32::NEG_INFINITY),
        ] {
            for (row, column) in [(0, 0), (1, 1), (2, 2), (3, 2), (2, 3)] {
                let mut matrix = matrix_of(&camera, size);
                matrix[row][column] = poison;
                let (mut occluded, placed) = loaded(h, &scene, true);
                let (mut plain, plain_placed) = loaded(h, &scene, false);
                let entries_now = entries(&scene, &placed, camera.position());
                let plain_entries = entries(&scene, &plain_placed, camera.position());
                plain.set_active_pages(&plain_entries).unwrap();
                let total = shoot(h, &mut plain, matrix, size).counts[0];
                // A NaN passes the frustum test (every comparison is false), so the cells reach the
                // occlusion test; an infinity usually fails the frustum test and keeps nothing.
                if name == "nan" {
                    assert!(
                        total > 0,
                        "{} nan at [{row}][{column}]: nothing kept",
                        h.label
                    );
                }
                kept += usize::from(total > 0);
                occluded.set_active_pages(&entries_now).unwrap();
                for frame in 0..3 {
                    let shot = shoot(h, &mut occluded, matrix, size);
                    assert_eq!(
                        shot.counts[0] + shot.counts[1],
                        total,
                        "{} {name} at [{row}][{column}], frame {frame}",
                        h.label
                    );
                }
            }
        }
        assert!(
            kept >= 5,
            "{}: only {kept} poisoned matrices kept cells",
            h.label
        );
        true
    });
}
