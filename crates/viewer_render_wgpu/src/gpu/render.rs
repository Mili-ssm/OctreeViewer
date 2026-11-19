//! The frame.

use super::{
    FrameStatus, SurfaceRenderer,
    error::GpuRendererError,
    init::create_depth_texture,
    pipeline::{CUBE_INDICES, ViewUniform},
};
use std::{mem::size_of, sync::mpsc};

use crate::{
    CameraFrame,
    cell_render::{CellColor, CellFrame},
};

impl SurfaceRenderer<'_> {
    /// Presents a frame with the current camera.
    pub fn render(&mut self) -> Result<FrameStatus, GpuRendererError> {
        self.update_hzb_visible_count();
        if let Some(timer) = &mut self.frame_timer {
            timer.collect(&self.device);
        }
        if self.suspended {
            return Ok(FrameStatus::Suspended);
        }
        let viewport = [self.surface_config.width, self.surface_config.height];
        // With a cell pool the cells are drawn eye-relative and the CPU scene and its HZB are idle.
        let use_cells = self.cell_pool.is_some();
        let (view_projection, mut render_origin) = if use_cells {
            let Some(frame) = self.camera.eye_frame(viewport) else {
                return Ok(FrameStatus::Suspended);
            };
            (frame.view_projection, [0.0; 4])
        } else {
            let Some(CameraFrame {
                view_projection,
                render_origin,
            }) = self.camera.frame(viewport)
            else {
                return Ok(FrameStatus::Suspended);
            };
            (view_projection, render_origin)
        };
        render_origin[3] = self.visual_scale as f32;
        let camera_key = (view_projection, render_origin);
        let use_hzb = !use_cells
            && self.hzb_reference_camera == Some(camera_key)
            && self
                .hzb
                .as_ref()
                .is_some_and(|hzb| hzb.indexed_bind_group().is_some());
        if !use_hzb && !use_cells {
            self.visible_instance_count = self
                .scene_buffer
                .as_ref()
                .map_or(0, |buffer| buffer.instance_count as usize);
        }
        let (frame, suboptimal) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => (frame, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            wgpu::CurrentSurfaceTexture::Timeout => return Ok(FrameStatus::SkippedTimeout),
            wgpu::CurrentSurfaceTexture::Occluded => return Ok(FrameStatus::Suspended),
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                self.depth_texture = create_depth_texture(
                    &self.device,
                    &self.surface_config,
                    self.hzb_depth_supported,
                );
                self.refresh_hzb_targets();
                if let Some(pool) = &mut self.cell_pool {
                    pool.invalidate_viewport();
                }
                return Ok(FrameStatus::Reconfigured);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err(GpuRendererError::SurfaceAcquisition(
                    "surface lost; recreate the renderer and surface".into(),
                ));
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(GpuRendererError::SurfaceAcquisition(
                    "surface validation failed".into(),
                ));
            }
        };
        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&ViewUniform {
                matrix: view_projection,
                render_origin,
                style: [
                    self.color_mode.shader_value(),
                    1.0,
                    self.elevation_range.map_or(0.0, |range| range[0]),
                    self.elevation_range.map_or(1.0, |range| range[1]),
                ],
                intensity_range: self
                    .intensity_range
                    .map_or([0.0, 1.0, f32::from(self.depth_shading), 0.0], |range| {
                        [range[0], range[1], f32::from(self.depth_shading), 0.0]
                    }),
            }),
        );
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = self
            .depth_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("octree-viewer-frame"),
            });
        let timer_slot = self
            .frame_timer
            .as_mut()
            .and_then(|timer| timer.begin(&mut encoder));
        if use_cells && let Some(pool) = &mut self.cell_pool {
            let eye_z = pool
                .page_frame()
                .map_or_else(|| self.camera.position()[2], |stamp| stamp.eye_scaled[2]);
            pool.set_color(CellColor::for_frame(
                self.color_mode,
                self.cell_color_ranges,
                eye_z,
                self.visual_scale,
                self.coordinate_origin[2],
            ));
            pool.encode_cells(
                &mut encoder,
                &view,
                &depth_view,
                &CellFrame {
                    view_projection,
                    clear_color: self.clear_color,
                    depth_shading: self.depth_shading,
                    viewport,
                },
            );
        }
        if use_hzb && let Some(hzb) = &self.hzb {
            hzb.cull(&mut encoder, &self.queue);
        }
        if !use_cells {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("octree-viewer-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear_color),
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
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            if use_hzb {
                if let Some(hzb) = &self.hzb
                    && let (Some(bind_group), Some(indirect_args)) =
                        (hzb.indexed_bind_group(), hzb.indirect_args())
                {
                    pass.set_pipeline(hzb.indexed_pipeline());
                    pass.set_bind_group(0, hzb.camera_bind_group(), &[]);
                    pass.set_bind_group(1, bind_group, &[]);
                    pass.draw_indexed_indirect(indirect_args, 0);
                }
            } else {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            }
            if !use_hzb && let Some(buffer) = &self.scene_buffer {
                if buffer.instance_count > 0 {
                    pass.set_vertex_buffer(1, buffer.buffer.slice(..));
                    pass.draw_indexed(0..CUBE_INDICES.len() as u32, 0, 0..buffer.instance_count);
                }
            } else if !use_hzb {
                for block_id in &self.visible_block_ids {
                    if let Some(buffer) = self.block_buffers.get(block_id)
                        && buffer.instance_count > 0
                    {
                        pass.set_vertex_buffer(1, buffer.buffer.slice(..));
                        pass.draw_indexed(
                            0..CUBE_INDICES.len() as u32,
                            0,
                            0..buffer.instance_count,
                        );
                    }
                }
            }
        }
        if !use_hzb
            && !use_cells
            && let Some(hzb) = &self.hzb
            && hzb.indexed_bind_group().is_some()
        {
            hzb.build_depth_pyramid(&mut encoder);
            hzb.cull(&mut encoder, &self.queue);
        }
        if use_hzb
            && self.hzb_count_receiver.is_none()
            && let (Some(hzb), Some(readback)) = (&self.hzb, &self.hzb_count_readback)
            && let Some(indirect_args) = hzb.indirect_args()
        {
            encoder.copy_buffer_to_buffer(
                indirect_args,
                size_of::<u32>() as u64,
                readback,
                0,
                size_of::<u32>() as u64,
            );
        }
        if let (Some(timer), Some(slot)) = (&self.frame_timer, timer_slot) {
            timer.end(&mut encoder, slot);
        }
        self.queue.submit([encoder.finish()]);
        if let (Some(timer), Some(slot)) = (&mut self.frame_timer, timer_slot) {
            timer.submitted(slot);
        }
        if use_hzb
            && self.hzb_count_receiver.is_none()
            && let Some(readback) = &self.hzb_count_readback
        {
            let (sender, receiver) = mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
            self.hzb_count_receiver = Some(receiver);
        }
        self.hzb_reference_camera = if !use_cells
            && self
                .hzb
                .as_ref()
                .is_some_and(|hzb| hzb.indexed_bind_group().is_some())
        {
            Some(camera_key)
        } else {
            None
        };
        self.queue.present(frame);
        if suboptimal {
            self.surface.configure(&self.device, &self.surface_config);
            self.depth_texture =
                create_depth_texture(&self.device, &self.surface_config, self.hzb_depth_supported);
            self.refresh_hzb_targets();
            if let Some(pool) = &mut self.cell_pool {
                pool.invalidate_viewport();
            }
            return Ok(FrameStatus::Reconfigured);
        }
        Ok(FrameStatus::Presented)
    }
}
