//! Picking: GPU readback of the voxel under a pixel and its CPU resolution.

use super::SurfaceRenderer;

use crate::{
    InstanceRaw, SceneBounds,
    cell_pick::{PoolPick, prepare_pool_pick},
};

pub(super) const PICK_CHUNK_INSTANCES: usize = 256;
pub(crate) const PICK_MAX_CANDIDATE_INSTANCES: usize = 65_536;

#[derive(Debug, Clone, Copy)]
pub(crate) struct PickChunk {
    pub(crate) first_instance: usize,
    pub(crate) instance_count: usize,
    pub(crate) bounds: SceneBounds,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PickCopy {
    pub(crate) staging_offset: u64,
    pub(crate) first_instance: usize,
    pub(crate) instance_count: usize,
}

/// Staging copy of a CPU-path pick.
pub struct CpuPick {
    buffer: wgpu::Buffer,
    copies: Vec<PickCopy>,
    ray: PickRay,
    /// The units of the scene at `prepare_pick`: the host awaits the mapping before `resolve_pick`.
    visual_scale: f64,
    coordinate_origin: [f64; 3],
}

/// What `prepare_pick` copied; `resolve_pick` decides by the variant, not by the renderer's current
/// mode, because the host awaits the mapping between the two calls.
pub enum PickReadback {
    Cpu(CpuPick),
    Pool(PoolPick),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PickRay {
    pub(crate) origin: glam::DVec3,
    pub(crate) direction: glam::DVec3,
    pub(crate) near_distance: f64,
    pub(crate) far_distance: f64,
}

impl PickReadback {
    pub fn buffer(&self) -> &wgpu::Buffer {
        match self {
            Self::Cpu(pick) => &pick.buffer,
            Self::Pool(pick) => pick.buffer(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickHit {
    pub instance_index: usize,
    pub position: [f64; 3],
    pub voxel_center: [f64; 3],
    pub voxel_size: f64,
    pub distance: f64,
}

pub(crate) fn build_pick_chunks(instances: &[InstanceRaw], visual_scale: f64) -> Vec<PickChunk> {
    instances
        .chunks(PICK_CHUNK_INSTANCES)
        .enumerate()
        .filter_map(|(chunk_index, chunk)| {
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for instance in chunk {
                let half = f64::from(instance.size) * visual_scale * 0.5;
                for axis in 0..3 {
                    let center = f64::from(instance.center[axis]) * visual_scale;
                    min[axis] = min[axis].min(((center - half) as f32).next_down());
                    max[axis] = max[axis].max(((center + half) as f32).next_up());
                }
            }
            if min.iter().chain(&max).any(|value| !value.is_finite()) {
                return None;
            }
            Some(PickChunk {
                first_instance: chunk_index * PICK_CHUNK_INSTANCES,
                instance_count: chunk.len(),
                bounds: SceneBounds {
                    min: min.map(f64::from),
                    max: max.map(f64::from),
                },
            })
        })
        .collect()
}

fn ray_bounds_entry(
    origin: glam::DVec3,
    direction: glam::DVec3,
    bounds: SceneBounds,
) -> Option<f64> {
    ray_box_entry(origin, direction, bounds.min, bounds.max)
}

pub(super) fn check_pick_candidate_budget(candidate_count: usize) -> Result<(), String> {
    if candidate_count > PICK_MAX_CANDIDATE_INSTANCES {
        Err(format!(
            "pick ray intersects {candidate_count} candidate voxels; limit is {PICK_MAX_CANDIDATE_INSTANCES}"
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn resolve_pick_instances(
    ray: PickRay,
    copies: &[PickCopy],
    mapped: &[u8],
    visual_scale: f64,
    coordinate_origin: [f64; 3],
) -> Result<Option<PickHit>, String> {
    let instance_bytes = std::mem::size_of::<InstanceRaw>();
    let mut nearest: Option<(f64, usize, InstanceRaw)> = None;
    for copy in copies {
        let start = usize::try_from(copy.staging_offset)
            .map_err(|_| "pick staging offset is not addressable".to_owned())?;
        for relative_index in 0..copy.instance_count {
            let byte_start = start + relative_index * instance_bytes;
            let byte_end = byte_start + instance_bytes;
            let raw = mapped
                .get(byte_start..byte_end)
                .ok_or_else(|| "pick staging result is truncated".to_owned())?;
            let instance = bytemuck::pod_read_unaligned::<InstanceRaw>(raw);
            let half = f64::from(instance.size) * visual_scale * 0.5;
            let center_render = instance.center.map(|value| f64::from(value) * visual_scale);
            let min = center_render.map(|value| value - half);
            let max = center_render.map(|value| value + half);
            let Some((entry, exit)) = ray_box_interval(ray.origin, ray.direction, min, max) else {
                continue;
            };
            let distance = entry.max(ray.near_distance);
            if distance > exit || distance > ray.far_distance {
                continue;
            }
            if nearest.is_none_or(|(best, _, _)| distance < best) {
                nearest = Some((distance, copy.first_instance + relative_index, instance));
            }
        }
    }
    let Some((distance, instance_index, instance)) = nearest else {
        return Ok(None);
    };
    let center = instance.center.map(f64::from);
    let position = std::array::from_fn(|axis| {
        (ray.origin[axis] + ray.direction[axis] * distance) / visual_scale + coordinate_origin[axis]
    });
    let voxel_center = std::array::from_fn(|axis| center[axis] + coordinate_origin[axis]);
    Ok(Some(PickHit {
        instance_index,
        position,
        voxel_center,
        voxel_size: f64::from(instance.size),
        distance: distance / visual_scale,
    }))
}

pub(super) fn ray_box_entry(
    origin: glam::DVec3,
    direction: glam::DVec3,
    min: [f64; 3],
    max: [f64; 3],
) -> Option<f64> {
    ray_box_interval(origin, direction, min, max).map(|(near, _)| near)
}

pub(crate) fn ray_box_interval(
    origin: glam::DVec3,
    direction: glam::DVec3,
    min: [f64; 3],
    max: [f64; 3],
) -> Option<(f64, f64)> {
    let mut near = 0.0_f64;
    let mut far = f64::INFINITY;
    for axis in 0..3 {
        if direction[axis].abs() < 1.0e-15 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
            continue;
        }
        let inverse = 1.0 / direction[axis];
        let mut first = (min[axis] - origin[axis]) * inverse;
        let mut second = (max[axis] - origin[axis]) * inverse;
        if first > second {
            std::mem::swap(&mut first, &mut second);
        }
        near = near.max(first);
        far = far.min(second);
        if near > far {
            return None;
        }
    }
    far.is_finite().then_some((near, far))
}

impl<'window> SurfaceRenderer<'window> {
    /// Copies only ray-intersecting instance ranges into a bounded staging buffer.
    pub fn prepare_pick(&self, pixel: [f64; 2]) -> Result<Option<PickReadback>, String> {
        if let Some(pool) = &self.cell_pool {
            let Some((origin, direction, near_distance, far_distance)) =
                self.camera.ray(pixel, self.viewport_size())
            else {
                return Ok(None);
            };
            let headroom = self.gpu_budget.max_bytes.saturating_sub(
                self.allocated_gpu_bytes()
                    .map_err(|error| error.to_string())?,
            );
            return Ok(prepare_pool_pick(
                &self.device,
                &self.queue,
                pool,
                PickRay {
                    origin,
                    direction,
                    near_distance,
                    far_distance,
                },
                self.visual_scale,
                self.coordinate_origin,
                headroom,
            )?
            .map(PickReadback::Pool));
        }
        let Some(source) = self.scene_buffer.as_ref() else {
            return Ok(None);
        };
        let Some((ray_origin, ray_direction, near_distance, far_distance)) =
            self.camera.ray(pixel, self.viewport_size())
        else {
            return Ok(None);
        };
        let mut candidates = self
            .pick_chunks
            .iter()
            .filter_map(|chunk| {
                ray_bounds_entry(ray_origin, ray_direction, chunk.bounds)
                    .map(|entry| (entry, *chunk))
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.0.total_cmp(&right.0));
        let candidate_count = candidates
            .iter()
            .try_fold(0_usize, |total, (_, chunk)| {
                total.checked_add(chunk.instance_count)
            })
            .ok_or_else(|| "pick candidate count overflow".to_owned())?;
        check_pick_candidate_budget(candidate_count)?;
        if candidate_count == 0 {
            return Ok(None);
        }

        let instance_bytes = std::mem::size_of::<InstanceRaw>() as u64;
        let byte_len = u64::try_from(candidate_count)
            .ok()
            .and_then(|count| count.checked_mul(instance_bytes))
            .ok_or_else(|| "pick staging buffer size overflow".to_owned())?;
        let projected_gpu_bytes = self
            .allocated_cpu_bytes()
            .map_err(|error| error.to_string())?
            .saturating_add(byte_len);
        if projected_gpu_bytes > self.gpu_budget.max_bytes {
            return Err(format!(
                "pick staging buffer would exceed the configured GPU memory budget ({projected_gpu_bytes} > {})",
                self.gpu_budget.max_bytes
            ));
        }
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("octree-viewer-pick-staging"),
            size: byte_len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("octree-viewer-pick-copy"),
            });
        let mut copies = Vec::with_capacity(candidates.len());
        let mut staging_offset = 0_u64;
        for (_, chunk) in candidates {
            let size = u64::try_from(chunk.instance_count)
                .ok()
                .and_then(|count| count.checked_mul(instance_bytes))
                .ok_or_else(|| "pick chunk size overflow".to_owned())?;
            let source_offset = u64::try_from(chunk.first_instance)
                .ok()
                .and_then(|index| index.checked_mul(instance_bytes))
                .ok_or_else(|| "pick source offset overflow".to_owned())?;
            encoder.copy_buffer_to_buffer(
                &source.buffer,
                source_offset,
                &staging,
                staging_offset,
                size,
            );
            copies.push(PickCopy {
                staging_offset,
                first_instance: chunk.first_instance,
                instance_count: chunk.instance_count,
            });
            staging_offset += size;
        }
        self.queue.submit([encoder.finish()]);
        Ok(Some(PickReadback::Cpu(CpuPick {
            buffer: staging,
            copies,
            ray: PickRay {
                origin: ray_origin,
                direction: ray_direction,
                near_distance,
                far_distance,
            },
            visual_scale: self.visual_scale,
            coordinate_origin: self.coordinate_origin,
        })))
    }

    pub fn resolve_pick(
        &self,
        readback: &PickReadback,
        mapped: &[u8],
    ) -> Result<Option<PickHit>, String> {
        match readback {
            PickReadback::Cpu(pick) => resolve_pick_instances(
                pick.ray,
                &pick.copies,
                mapped,
                pick.visual_scale,
                pick.coordinate_origin,
            ),
            PickReadback::Pool(pick) => pick.resolve(mapped),
        }
    }
}
