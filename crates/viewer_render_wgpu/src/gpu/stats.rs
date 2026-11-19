//! Resource and frame statistics.

use super::{SurfaceRenderer, error::GpuRendererError};

/// Requested sizes of currently owned GPU resources, not physical VRAM residency.
/// Textures use logical texel bytes; driver padding, swapchain, staging and pipelines are excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GpuResourceStats {
    pub instance_buffer_bytes: u64,
    pub fixed_buffer_bytes: u64,
    pub hzb_buffer_bytes: u64,
    pub depth_texture_bytes: u64,
    pub hzb_texture_bytes: u64,
    /// Buffers of the cell pool; 0 while the pool does not exist (it is created lazily).
    pub cell_pool_bytes: u64,
}

impl GpuResourceStats {
    pub fn total_bytes(self) -> u64 {
        self.instance_buffer_bytes
            + self.fixed_buffer_bytes
            + self.hzb_buffer_bytes
            + self.depth_texture_bytes
            + self.hzb_texture_bytes
            + self.cell_pool_bytes
    }
}

impl SurfaceRenderer<'_> {
    pub(super) fn allocated_gpu_bytes(&self) -> Result<u64, GpuRendererError> {
        let scene_capacity = self
            .scene_buffer
            .as_ref()
            .map_or(0, |buffer| buffer.capacity);
        let scene_and_blocks =
            self.block_buffers
                .values()
                .try_fold(scene_capacity, |sum, buffer| {
                    sum.checked_add(buffer.capacity)
                        .ok_or(GpuRendererError::InvalidSceneSize)
                })?;
        scene_and_blocks
            .checked_add(self.cell_pool_bytes())
            .and_then(|bytes| {
                bytes.checked_add(
                    self.hzb
                        .as_ref()
                        .map_or(0, super::super::hzb::HzbResources::resource_bytes),
                )
            })
            .and_then(|bytes| bytes.checked_add(self.hzb_count_readback.as_ref().map_or(0, |_| 4)))
            .ok_or(GpuRendererError::InvalidSceneSize)
    }

    /// Bytes the CPU path accounts for: everything but the cell pool, which is freed when the CPU
    /// path takes over and has its own creation check.
    pub(super) fn allocated_cpu_bytes(&self) -> Result<u64, GpuRendererError> {
        Ok(self.allocated_gpu_bytes()? - self.cell_pool_bytes())
    }

    /// Percentiles of the GPU time per frame over the last samples; `None` when the device has no
    /// `timestamp-query` or nothing was sampled yet.
    pub fn gpu_frame_stats(&self) -> Option<crate::GpuFrameStats> {
        self.frame_timer
            .as_ref()
            .and_then(super::super::frame_timer::FrameTimer::stats)
    }

    /// Turns the per-frame GPU timing on or off (off by default; no cost while off). A no-op
    /// without `timestamp-query`.
    pub fn set_gpu_frame_timing(&mut self, enabled: bool) {
        if let Some(timer) = &mut self.frame_timer {
            timer.set_enabled(enabled);
        }
    }

    /// Starts a new sampling window for `gpu_frame_stats`.
    pub fn reset_gpu_frame_stats(&mut self) {
        if let Some(timer) = &mut self.frame_timer {
            timer.reset();
        }
    }

    /// Snapshots resources owned now; replaced resources may still be retained by GPU work.
    pub fn resource_stats(&self) -> GpuResourceStats {
        let instance_buffer_bytes = self.scene_buffer.as_ref().map_or(0, |b| b.buffer.size())
            + self
                .block_buffers
                .values()
                .map(|b| b.buffer.size())
                .sum::<u64>();
        GpuResourceStats {
            instance_buffer_bytes,
            fixed_buffer_bytes: self.vertex_buffer.size()
                + self.index_buffer.size()
                + self.uniform_buffer.size(),
            hzb_buffer_bytes: self
                .hzb
                .as_ref()
                .map_or(0, super::super::hzb::HzbResources::requested_buffer_bytes)
                + self
                    .hzb_count_readback
                    .as_ref()
                    .map_or(0, wgpu::Buffer::size),
            depth_texture_bytes: u64::from(self.depth_texture.width())
                * u64::from(self.depth_texture.height())
                * 4,
            hzb_texture_bytes: self
                .hzb
                .as_ref()
                .map_or(0, super::super::hzb::HzbResources::logical_texture_bytes),
            cell_pool_bytes: self.cell_pool_bytes(),
        }
    }

    pub fn uploaded_bytes(&self) -> u64 {
        self.uploaded_bytes
    }
}
