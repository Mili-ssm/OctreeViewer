//! Picking on the cell pool.
//!
//! The pool lives on the GPU, so `prepare` copies the pages the ray can touch into a staging
//! buffer and `resolve` walks their cells on the CPU with the filters of the slot pass (empty
//! slot, octant bit, node mask; no frustum and no Hi-Z). In cell mode the drawn test is the
//! per-cell decision of [`crate::cell_lod`] over the node records the renderer mirrors. Everything runs in the frame of the page
//! entries, i.e. relative to the frame (eye, visual scale, coordinate origin) stored with the page
//! table, so the f32 entries stay exact whatever the current camera does. The camera is read at
//! `prepare`; the pool is not. The renderer's scale and origin can change after the pages were
//! written (`set_visual_scale`, `fit_spatial_bounds`); `prepare` then refuses to pick until they
//! are rewritten.
//!
//! The candidates are the pages whose tight box meets the ray, nearest first, and at most
//! [`PICK_MAX_PAGES`] of them are copied (the same 65 536-slot limit as the CPU path). When some
//! remain, a hit is only safe if it lies before the first page left out: `cutoff`.

use glam::DVec3;

use std::collections::HashMap;

use crate::cell_lod::{LodView, NodeLod, draws};
use crate::cell_pages::{CellInstance, PAGE_SLOTS, PageEntry, decode_cell, srgb_to_linear_table};
use crate::cell_render::{CellRenderer, MASK_WORDS, NODE_WORDS, PAGE_BYTES};
use crate::gpu::{PICK_MAX_CANDIDATE_INSTANCES, PickHit, PickRay, ray_box_interval};

/// Pages copied per pick.
pub(crate) const PICK_MAX_PAGES: usize = PICK_MAX_CANDIDATE_INSTANCES / PAGE_SLOTS;

/// A page copied for picking, with the state the resolution needs at the time of the copy.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PoolCopy {
    pub slot: u32,
    pub page: PageEntry,
    pub mask: [u32; MASK_WORDS],
}

/// Everything `resolve` needs besides the mapped bytes. The copy `i` is at `i * PAGE_BYTES`.
#[derive(Debug, Clone)]
pub(crate) struct PoolPickPlan {
    /// The ray in the frame of the page entries (camera ray minus `eye`).
    pub ray: PickRay,
    pub eye: [f64; 3],
    pub visual_scale: f64,
    pub coordinate_origin: [f64; 3],
    pub copies: Vec<PoolCopy>,
    /// Entry distance of the first candidate that was not copied.
    pub cutoff: Option<f64>,
    /// `Some` in cell mode.
    pub lod: Option<LodPick>,
}

/// What the per-cell decision needs at the time of the copy: the view the last frame drew with
/// and the record of each node slot the copied pages name.
#[derive(Debug, Clone)]
pub(crate) struct LodPick {
    pub view: LodView,
    pub nodes: HashMap<u32, NodeLod>,
}

/// The node of a record in the layout of [`crate::cell_lod::node_record`].
pub(crate) fn node_of_record(record: &[u32; NODE_WORDS]) -> NodeLod {
    NodeLod {
        anchor: std::array::from_fn(|axis| f32::from_bits(record[16 + axis])),
        node_depth: record[19] as u8,
        active: u64::from(record[20]) | u64::from(record[21]) << 32,
        mask: std::array::from_fn(|i| {
            u64::from(record[2 * i]) | u64::from(record[2 * i + 1]) << 32
        }),
    }
}

pub struct PoolPick {
    buffer: wgpu::Buffer,
    plan: PoolPickPlan,
}

impl PoolPick {
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    pub(crate) fn resolve(&self, mapped: &[u8]) -> Result<Option<PickHit>, String> {
        resolve_pool_pick(&self.plan, mapped)
    }
}

#[derive(Clone, Copy)]
struct Best {
    distance: f64,
    global: usize,
    lo: [f32; 3],
    hi: [f32; 3],
    side: f32,
}

pub(crate) struct Candidate {
    pub distance: f64,
    pub slot: u32,
    pub page: PageEntry,
}

/// Active pages with octant bits whose tight box meets the ray within [near, far], sorted by
/// entry distance and then by slot.
pub(crate) fn candidates(
    pages: impl Iterator<Item = (u32, PageEntry)>,
    ray: &PickRay,
) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = pages
        .filter(|(_, page)| page.0[5] != 0)
        .filter_map(|(slot, page)| {
            let (min, max) = page_box(&page);
            let (entry, exit) = ray_box_interval(ray.origin, ray.direction, min, max)?;
            let distance = entry.max(ray.near_distance);
            (distance <= exit && distance <= ray.far_distance).then_some(Candidate {
                distance,
                slot,
                page,
            })
        })
        .collect();
    found.sort_by(|a, b| a.distance.total_cmp(&b.distance).then(a.slot.cmp(&b.slot)));
    found
}

/// The candidates to copy (the nearest [`PICK_MAX_PAGES`]) and the entry distance of the first one
/// left out.
pub(crate) fn select_pages(found: &[Candidate]) -> (&[Candidate], Option<f64>) {
    let cutoff = found
        .get(PICK_MAX_PAGES)
        .map(|candidate| candidate.distance);
    (&found[..found.len().min(PICK_MAX_PAGES)], cutoff)
}

/// The tight box of a page, computed like the page pass (`fma`) and widened one ulp.
fn page_box(page: &PageEntry) -> ([f64; 3], [f64; 3]) {
    let (origin, side) = (page.origin(), page.side());
    let box_max = [page.0[6] & 0xffff, page.0[6] >> 16, page.0[7] & 0xffff];
    (
        origin.map(|value| f64::from(value.next_down())),
        std::array::from_fn(|axis| {
            f64::from(
                (box_max[axis] as f32 + 1.0)
                    .mul_add(side, origin[axis])
                    .next_up(),
            )
        }),
    )
}

/// Copies the candidate pages of `camera_ray` (in the frame of the camera, scaled units) into a
/// staging buffer. `visual_scale` and `coordinate_origin` are the renderer's current ones; the pick
/// fails if the pages were built for others. `headroom_bytes` is what the GPU budget leaves after
/// the resources in use.
pub(crate) fn prepare_pool_pick(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pool: &CellRenderer,
    camera_ray: PickRay,
    visual_scale: f64,
    coordinate_origin: [f64; 3],
    headroom_bytes: u64,
) -> Result<Option<PoolPick>, String> {
    let Some(frame) = pool.page_frame() else {
        return Ok(None);
    };
    if !frame.same_units(visual_scale, coordinate_origin) {
        return Err(
            "the cell pages were built for another visual scale or coordinate origin; rewrite them \
             with set_active_cell_pages before picking"
                .to_owned(),
        );
    }
    let eye = frame.eye_scaled;
    let ray = PickRay {
        origin: camera_ray.origin - DVec3::from_array(eye),
        ..camera_ray
    };
    let found = candidates(pool.active_entries(), &ray);
    if found.is_empty() {
        return Ok(None);
    }
    let (copied, cutoff) = select_pages(&found);
    let byte_len = copied.len() as u64 * PAGE_BYTES;
    if byte_len > headroom_bytes {
        return Err(format!(
            "pick staging buffer would exceed the configured GPU memory budget ({byte_len} bytes needed, {headroom_bytes} left)"
        ));
    }
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("octree-viewer-pick-staging"),
        size: byte_len,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("octree-viewer-pick-copy"),
    });
    let mut copies = Vec::with_capacity(copied.len());
    for (index, candidate) in copied.iter().enumerate() {
        encoder.copy_buffer_to_buffer(
            pool.instances_buffer(),
            u64::from(candidate.slot) * PAGE_BYTES,
            &staging,
            index as u64 * PAGE_BYTES,
            PAGE_BYTES,
        );
        copies.push(PoolCopy {
            slot: candidate.slot,
            page: candidate.page,
            mask: *pool.node_mask(candidate.page.0[4]),
        });
    }
    queue.submit([encoder.finish()]);
    let lod = pool.lod_view().map(|view| LodPick {
        view,
        nodes: copies
            .iter()
            .map(|copy| copy.page.0[4])
            .map(|slot| (slot, node_of_record(pool.node_record(slot))))
            .collect(),
    });
    Ok(Some(PoolPick {
        buffer: staging,
        plan: PoolPickPlan {
            ray,
            eye,
            visual_scale: frame.visual_scale,
            coordinate_origin: frame.coordinate_origin,
            copies,
            cutoff,
            lod,
        },
    }))
}

/// The per-cell decision for `cell` of a page of the copy (cell mode only draws what the twin of
/// the slot pass draws). Pages the cell decision never activates (no relative origin, depth
/// outside the node's span) draw nothing.
fn drawn_in_cell_mode(lod: &LodPick, copy: &PoolCopy, cell: &CellInstance) -> bool {
    let entry = &copy.page.0;
    let (depth, origin) = (entry[11], [entry[8], entry[9], entry[10]]);
    let Some(node) = lod.nodes.get(&entry[4]) else {
        return false;
    };
    let depth = u8::try_from(depth).unwrap_or(u8::MAX);
    if origin == [u32::MAX; 3]
        || depth < node.node_depth
        || depth - node.node_depth > crate::cell_lod::MAX_NODE_SPAN
    {
        return false;
    }
    let rel: [u32; 3] =
        std::array::from_fn(|axis| origin[axis].wrapping_add(u32::from(cell.rel()[axis])));
    draws(&lod.view, node, depth, rel, copy.page.side())
}

/// The nearest drawn cell of the copied pages; ties go to the lower global slot
/// (`page slot * 512 + local`).
pub(crate) fn resolve_pool_pick(
    plan: &PoolPickPlan,
    mapped: &[u8],
) -> Result<Option<PickHit>, String> {
    let table = srgb_to_linear_table();
    let ray = &plan.ray;
    let mut best: Option<Best> = None;
    for (index, copy) in plan.copies.iter().enumerate() {
        let start = index * PAGE_BYTES as usize;
        let bytes = mapped
            .get(start..start + PAGE_BYTES as usize)
            .ok_or_else(|| "pick staging result is truncated".to_owned())?;
        for local in 0..PAGE_SLOTS {
            let cell = bytemuck::pod_read_unaligned::<CellInstance>(&bytes[local * 16..][..16]);
            if cell.is_empty() {
                continue;
            }
            // Same filters as `slot_state` in the slot pass.
            if let Some(lod) = &plan.lod {
                if !drawn_in_cell_mode(lod, copy, &cell) {
                    continue;
                }
            } else if !cell.spans_octants() {
                if (copy.page.0[5] >> cell.octant()) & 1 == 0 {
                    continue;
                }
                let bit = cell.mask_index() as usize;
                if (copy.mask[bit / 32] >> (bit % 32)) & 1 == 1 {
                    continue;
                }
            }
            let decoded = decode_cell(&copy.page, &cell, &table);
            let Some((entry, exit)) = ray_box_interval(
                ray.origin,
                ray.direction,
                decoded.lo.map(f64::from),
                decoded.hi.map(f64::from),
            ) else {
                continue;
            };
            let distance = entry.max(ray.near_distance);
            if distance > exit || distance > ray.far_distance {
                continue;
            }
            let global = copy.slot as usize * PAGE_SLOTS + local;
            if best.is_none_or(|b| (distance, global) < (b.distance, b.global)) {
                best = Some(Best {
                    distance,
                    global,
                    lo: decoded.lo,
                    hi: decoded.hi,
                    side: copy.page.side(),
                });
            }
        }
    }
    if let Some(cutoff) = plan.cutoff
        && best.is_none_or(|b| b.distance >= cutoff)
    {
        return Err(format!(
            "no safe hit within the {PICK_MAX_PAGES} nearest pages along the ray"
        ));
    }
    let Some(Best {
        distance,
        global: instance_index,
        lo,
        hi,
        side,
    }) = best
    else {
        return Ok(None);
    };
    let scale = plan.visual_scale;
    let position = std::array::from_fn(|axis| {
        (ray.origin[axis] + plan.eye[axis] + ray.direction[axis] * distance) / scale
            + plan.coordinate_origin[axis]
    });
    let voxel_center = std::array::from_fn(|axis| {
        f64::midpoint(f64::from(lo[axis]), f64::from(hi[axis])) / scale
            + plan.eye[axis] / scale
            + plan.coordinate_origin[axis]
    });
    Ok(Some(PickHit {
        instance_index,
        position,
        voxel_center,
        voxel_size: f64::from(side) / scale,
        distance: distance / scale,
    }))
}
