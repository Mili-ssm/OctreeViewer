//! Block instance buffers: scene upload and visible-block synchronisation.

use super::{SurfaceRenderer, error::GpuRendererError, pick::build_pick_chunks};
use std::{collections::HashMap, mem::size_of};

use viewer_core::{BlockId, ResidentBlockSource};

use crate::{
    CellColorRanges, InstanceRaw, RenderScene, SceneBounds,
    block_sync::ResidentBlockInfo,
    block_sync::plan_block_sync,
    budget::{buffer_capacity, check_peak_bytes, check_visible_instances},
};

pub(super) struct GpuInstanceBuffer {
    pub(super) buffer: wgpu::Buffer,
    pub(super) capacity: u64,
    pub(super) instance_count: u32,
    bounds: Option<SceneBounds>,
    intensity_range: Option<[f32; 2]>,
    elevation_range: Option<[f32; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockUploadStats {
    pub visible_block_count: usize,
    pub visible_instance_count: usize,
    pub uploaded_block_count: usize,
    pub removed_block_count: usize,
    pub uploaded_bytes: u64,
}

pub(crate) fn scene_upload_metadata(
    scene: &RenderScene,
    max_buffer_size: u64,
    max_visible_instances: usize,
) -> Result<(u64, u32, Option<SceneBounds>), GpuRendererError> {
    let bytes = scene
        .instance_count()
        .checked_mul(size_of::<InstanceRaw>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or(GpuRendererError::InvalidSceneSize)?;
    if bytes > max_buffer_size {
        return Err(GpuRendererError::GpuBufferLimitExceeded {
            requested_bytes: bytes,
            max_bytes: max_buffer_size,
        });
    }
    let instance_count =
        u32::try_from(scene.instance_count()).map_err(|_| GpuRendererError::InvalidSceneSize)?;
    check_visible_instances(scene.instance_count(), max_visible_instances)?;
    let bounds = scene.bounds();
    if instance_count > 0 && bounds.is_none() {
        return Err(GpuRendererError::InvalidSceneData);
    }
    Ok((bytes, instance_count, bounds))
}

fn create_instance_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &RenderScene,
    bytes: u64,
    instance_count: u32,
    bounds: Option<SceneBounds>,
    max_buffer_size: u64,
) -> Result<GpuInstanceBuffer, GpuRendererError> {
    let capacity = buffer_capacity(bytes, size_of::<InstanceRaw>() as u64, max_buffer_size)?;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("octree-viewer-instances"),
        size: capacity,
        usage: wgpu::BufferUsages::VERTEX
            | wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if bytes > 0 {
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(scene.instances()));
    }
    Ok(GpuInstanceBuffer {
        buffer,
        capacity,
        instance_count,
        bounds,
        intensity_range: scene.intensity_range(),
        elevation_range: scene.elevation_range(),
    })
}

impl SurfaceRenderer<'_> {
    /// Uploads one scene and returns the bytes written. Existing GPU storage is reused when it fits.
    pub fn upload_scene(&mut self, scene: &RenderScene) -> Result<u64, GpuRendererError> {
        let (bytes, count, bounds) = scene_upload_metadata(
            scene,
            self.cpu_limits.max_buffer_size,
            self.gpu_budget.max_visible_instances,
        )?;
        let current_capacity = self
            .scene_buffer
            .as_ref()
            .map_or(0, |buffer| buffer.capacity);
        let target_capacity = buffer_capacity(
            bytes,
            size_of::<InstanceRaw>() as u64,
            self.cpu_limits.max_buffer_size,
        )?;
        let added_capacity = if target_capacity > current_capacity {
            target_capacity
        } else {
            0
        };
        // The pool is freed when the CPU path takes over, so it does not count here.
        check_peak_bytes(
            self.allocated_cpu_bytes()?,
            added_capacity,
            self.gpu_budget.max_bytes,
        )?;
        if bytes > current_capacity {
            self.scene_buffer = Some(create_instance_buffer(
                &self.device,
                &self.queue,
                scene,
                bytes,
                count,
                bounds,
                self.cpu_limits.max_buffer_size,
            )?);
        } else if let Some(buffer) = &mut self.scene_buffer {
            if bytes > 0 {
                self.queue
                    .write_buffer(&buffer.buffer, 0, bytemuck::cast_slice(scene.instances()));
            }
            buffer.instance_count = count;
            buffer.bounds = bounds;
            buffer.intensity_range = scene.intensity_range();
            buffer.elevation_range = scene.elevation_range();
        }
        if self.scene_buffer.is_none() {
            self.scene_buffer = Some(create_instance_buffer(
                &self.device,
                &self.queue,
                scene,
                bytes,
                count,
                bounds,
                self.cpu_limits.max_buffer_size,
            )?);
        }
        self.block_buffers.clear();
        self.visible_block_ids.clear();
        self.cell_pool = None;
        self.hzb_reference_camera = None;
        self.visible_instance_count = scene.instance_count();
        self.pick_chunks = build_pick_chunks(scene.instances(), self.visual_scale);
        self.uploaded_bytes = self.uploaded_bytes.saturating_add(bytes);
        self.scene_bounds = bounds;
        self.intensity_range = scene.intensity_range();
        self.elevation_range = scene.elevation_range();
        self.hzb_scene_opaque = scene
            .instances()
            .iter()
            .all(|instance| instance.color[3] >= 1.0);
        self.prepare_hzb_scene(bytes);
        Ok(bytes)
    }

    /// Uploads a voxelized LOD cut while retaining its source block IDs for diagnostics.
    pub fn upload_voxel_cut(
        &mut self,
        scene: &RenderScene,
        block_ids: &[BlockId],
    ) -> Result<u64, GpuRendererError> {
        let bytes = self.upload_scene(scene)?;
        self.visible_block_ids = block_ids.to_vec();
        Ok(bytes)
    }

    /// Synchronizes GPU residency to a deduplicated renderable block cut.
    /// IDs are dataset-local; call `clear_dataset` before using IDs from another dataset.
    /// The previous cut remains active if validation fails.
    pub fn sync_visible_blocks(
        &mut self,
        block_ids: &[BlockId],
        cache: &dyn ResidentBlockSource,
    ) -> Result<BlockUploadStats, GpuRendererError> {
        let resident: Vec<_> = self
            .block_buffers
            .iter()
            .map(|(id, buffer)| ResidentBlockInfo {
                id: *id,
                instance_count: buffer.instance_count as usize,
                capacity: buffer.capacity,
                bounds: buffer.bounds,
                intensity_range: buffer.intensity_range,
                elevation_range: buffer.elevation_range,
            })
            .collect();
        let plan = plan_block_sync(
            &resident,
            block_ids,
            cache,
            self.cpu_limits.max_buffer_size,
            self.gpu_budget,
            self.scene_buffer
                .as_ref()
                .map_or(0, |buffer| buffer.capacity),
        )?;
        let mut additions = HashMap::with_capacity(plan.added.len());
        for (id, scene) in &plan.added {
            let (bytes, count, bounds) = scene_upload_metadata(
                scene,
                self.cpu_limits.max_buffer_size,
                self.gpu_budget.max_visible_instances,
            )?;
            additions.insert(
                *id,
                create_instance_buffer(
                    &self.device,
                    &self.queue,
                    scene,
                    bytes,
                    count,
                    bounds,
                    self.cpu_limits.max_buffer_size,
                )?,
            );
        }

        self.block_buffers
            .retain(|id, _| plan.selected.binary_search(id).is_ok());
        self.block_buffers.extend(additions);
        self.visible_block_ids = plan.selected;
        self.visible_instance_count = plan.visible_instance_count;
        self.scene_bounds = plan.visible_bounds;
        self.intensity_range = plan.visible_intensity_range;
        self.elevation_range = plan.visible_elevation_range;
        self.scene_buffer = None;
        self.cell_pool = None;
        self.pick_chunks.clear();
        self.hzb_reference_camera = None;
        self.hzb_scene_opaque = false;
        if let Some(hzb) = &mut self.hzb {
            hzb.reset_scene();
        }
        self.uploaded_bytes = self.uploaded_bytes.saturating_add(plan.uploaded_bytes);

        Ok(BlockUploadStats {
            visible_block_count: self.visible_block_ids.len(),
            visible_instance_count: self.visible_instance_count,
            uploaded_block_count: plan.added.len(),
            removed_block_count: plan.removed.len(),
            uploaded_bytes: plan.uploaded_bytes,
        })
    }

    /// Frees what only the CPU path draws from, so that in cells mode only the pool (plus the
    /// shared depth texture and uniforms) is resident. Bounds, ranges and the camera are kept.
    pub(super) fn release_cpu_scene(&mut self) {
        self.scene_buffer = None;
        self.block_buffers.clear();
        self.visible_block_ids.clear();
        self.visible_instance_count = 0;
        self.pick_chunks.clear();
        self.hzb_reference_camera = None;
        self.hzb_scene_opaque = false;
        self.hzb_count_receiver = None;
        if let Some(hzb) = &mut self.hzb {
            hzb.reset_scene();
        }
    }

    /// Drops scene and block GPU data before opening another dataset.
    pub fn clear_dataset(&mut self) {
        self.scene_buffer = None;
        self.cell_pool = None;
        self.cell_color_ranges = CellColorRanges::default();
        self.hzb_reference_camera = None;
        self.hzb_scene_opaque = false;
        self.block_buffers.clear();
        self.visible_block_ids.clear();
        self.visible_instance_count = 0;
        self.pick_chunks.clear();
        self.scene_bounds = None;
        self.intensity_range = None;
        self.elevation_range = None;
        self.uploaded_bytes = 0;
        self.visual_scale = 1.0;
        if let Some(hzb) = &mut self.hzb {
            hzb.reset_scene();
        }
    }

    pub fn visible_block_count(&self) -> usize {
        self.visible_block_ids.len()
    }

    pub fn visible_instance_count(&self) -> usize {
        self.visible_instance_count
    }
}
