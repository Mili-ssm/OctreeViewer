//! Hi-Z occlusion targets and scene preparation.

use super::SurfaceRenderer;
use std::{mem::size_of, sync::mpsc};

use crate::{InstanceRaw, budget::check_peak_bytes};

impl SurfaceRenderer<'_> {
    pub(super) fn prepare_hzb_scene(&mut self, source_bytes: u64) {
        self.hzb_reference_camera = None;
        let allocated_without_hzb = self
            .allocated_cpu_bytes()
            .unwrap_or(self.gpu_budget.max_bytes)
            .saturating_sub(
                self.hzb
                    .as_ref()
                    .map_or(0, super::super::hzb::HzbResources::resource_bytes),
            );
        let available = self
            .gpu_budget
            .max_bytes
            .saturating_sub(allocated_without_hzb);
        let Some(hzb) = &mut self.hzb else {
            return;
        };
        let opaque = self.hzb_scene_opaque;
        let Some(buffer) = &self.scene_buffer else {
            hzb.reset_scene();
            return;
        };
        if !opaque
            || buffer.instance_count < crate::hzb::MIN_OCCLUSION_CANDIDATES as u32
            || !hzb.can_prepare_scene(
                &self.cpu_limits,
                source_bytes,
                buffer.instance_count as usize,
                available,
            )
        {
            hzb.reset_scene();
            return;
        }
        hzb.prepare_scene(
            &self.device,
            &buffer.buffer,
            source_bytes,
            buffer.instance_count as usize,
        );
    }

    pub fn hzb_active(&self) -> bool {
        let Some(frame) = self.camera.frame(self.viewport_size()) else {
            return false;
        };
        let mut render_origin = frame.render_origin;
        render_origin[3] = self.visual_scale as f32;
        self.hzb_reference_camera == Some((frame.view_projection, render_origin))
            && self
                .hzb
                .as_ref()
                .is_some_and(|hzb| hzb.indexed_bind_group().is_some())
    }

    pub(super) fn update_hzb_visible_count(&mut self) {
        let Some(receiver) = &self.hzb_count_receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(())) => {
                self.hzb_count_receiver = None;
                if let Some(buffer) = &self.hzb_count_readback {
                    if let Ok(view) = buffer.slice(..).get_mapped_range() {
                        self.visible_instance_count = *bytemuck::from_bytes::<u32>(&view) as usize;
                    }
                    buffer.unmap();
                }
            }
            Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                self.hzb_count_receiver = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    pub(super) fn refresh_hzb_targets(&mut self) {
        self.hzb_reference_camera = None;
        let Some(hzb) = &mut self.hzb else {
            return;
        };
        // Drop per-scene index buffers before resizing. The old pyramid remains alive until
        // resize_targets replaces it, so account for both pyramids during that transition.
        hzb.reset_scene();
        let required_bytes = crate::hzb::estimated_pyramid_bytes([
            self.surface_config.width,
            self.surface_config.height,
        ]);
        // The pool counts here: it stays resident while the HZB targets are rebuilt.
        let resize_fits_budget = self
            .allocated_gpu_bytes()
            .and_then(|current_bytes| {
                check_peak_bytes(current_bytes, required_bytes, self.gpu_budget.max_bytes)
            })
            .is_ok();
        if required_bytes > self.gpu_budget.max_bytes / 2 || !resize_fits_budget {
            self.hzb = None;
            self.hzb_count_readback = None;
            self.hzb_count_receiver = None;
            return;
        }
        let resized = self.hzb.as_mut().is_some_and(|hzb| {
            hzb.resize_targets(
                &self.device,
                &self.depth_texture,
                [self.surface_config.width, self.surface_config.height],
                self.cpu_limits.max_texture_dimension_2d,
            )
        });
        if !resized {
            self.hzb = None;
            self.hzb_count_readback = None;
            self.hzb_count_receiver = None;
            return;
        }
        self.reprepare_hzb_scene();
    }

    fn reprepare_hzb_scene(&mut self) {
        self.hzb_reference_camera = None;
        let allocated_without_hzb = self
            .allocated_cpu_bytes()
            .unwrap_or(self.gpu_budget.max_bytes)
            .saturating_sub(
                self.hzb
                    .as_ref()
                    .map_or(0, super::super::hzb::HzbResources::resource_bytes),
            );
        let available = self
            .gpu_budget
            .max_bytes
            .saturating_sub(allocated_without_hzb);
        let Some(hzb) = &mut self.hzb else {
            return;
        };
        if !self.hzb_scene_opaque {
            hzb.reset_scene();
            return;
        }
        let Some(buffer) = &self.scene_buffer else {
            hzb.reset_scene();
            return;
        };
        if buffer.instance_count < crate::hzb::MIN_OCCLUSION_CANDIDATES as u32
            || !hzb.can_prepare_scene(
                &self.cpu_limits,
                u64::from(buffer.instance_count) * size_of::<InstanceRaw>() as u64,
                buffer.instance_count as usize,
                available,
            )
        {
            hzb.reset_scene();
            return;
        }
        hzb.prepare_scene(
            &self.device,
            &buffer.buffer,
            u64::from(buffer.instance_count) * size_of::<InstanceRaw>() as u64,
            buffer.instance_count as usize,
        );
    }
}
