//! Native offscreen rendering using the viewer's shader and pipeline.
use wgpu::util::DeviceExt;

use crate::gpu::{CUBE_INDICES, ViewProjection, create_pipeline_resources};
use crate::{GpuRendererError, OrbitCamera, RenderScene, RendererConfig};

#[derive(Debug)]
pub struct OffscreenImage {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8 sRGB rows, without GPU alignment padding.
    pub rgba: Vec<u8>,
    pub adapter: wgpu::AdapterInfo,
    pub non_background_pixels: usize,
}

/// Render once, wait for GPU completion, and read pixels back. No window or surface is created.
/// The camera and scene use local coordinates at visual scale 1, as in the default web viewer.
pub async fn render_offscreen(
    scene: &RenderScene,
    camera: &OrbitCamera,
    viewport: [u32; 2],
) -> Result<OffscreenImage, GpuRendererError> {
    let [width, height] = viewport;
    // Bound allocations, including row padding and the depth attachment.
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(GpuRendererError::Offscreen(
            "viewport must be between 1 and 8192 per axis".into(),
        ));
    }
    let frame = camera
        .frame(viewport)
        .ok_or_else(|| GpuRendererError::Offscreen("camera is not fitted".into()))?;
    if scene.instance_count() == 0 {
        return Err(GpuRendererError::Offscreen("scene is empty".into()));
    }
    let config = RendererConfig::default();
    super::gpu::scene_upload_metadata(scene, u64::MAX, config.gpu_budget.max_visible_instances)?;
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
        .ok_or(GpuRendererError::NoCompatibleAdapter)?;
    let adapter_info = adapter.get_info();
    let (device, queue) = adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                label: Some("octree-offscreen"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
            },
            None,
        )
        .await
        .map_err(|error| GpuRendererError::DeviceRequest(error.to_string()))?;
    super::gpu::scene_upload_metadata(
        scene,
        device.limits().max_buffer_size,
        config.gpu_budget.max_visible_instances,
    )?;
    if width > device.limits().max_texture_dimension_2d
        || height > device.limits().max_texture_dimension_2d
    {
        return Err(GpuRendererError::Offscreen(format!(
            "viewport exceeds adapter texture limit {}",
            device.limits().max_texture_dimension_2d
        )));
    }
    let pipeline = create_pipeline_resources(&device, wgpu::TextureFormat::Rgba8UnormSrgb).await?;
    let mut origin = frame.render_origin;
    origin[3] = 1.0;
    queue.write_buffer(
        &pipeline.uniform_buffer,
        0,
        bytemuck::bytes_of(&ViewProjection {
            matrix: frame.view_projection,
            render_origin: origin,
            style: [0.0, 1.0, 0.0, 1.0],
            intensity_range: [0.0, 1.0, 1.0, 0.0],
        }),
    );
    let instances = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("octree-offscreen-instances"),
        contents: bytemuck::cast_slice(scene.instances()),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let extent = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("octree-offscreen-color"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("octree-offscreen-depth"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let row_bytes = width * 4;
    let padded_row =
        row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("octree-offscreen-readback"),
        size: u64::from(padded_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let color_view = color.create_view(&Default::default());
    let depth_view = depth.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("octree-offscreen-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(config.clear_color),
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
        });
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &pipeline.uniform_bind_group, &[]);
        pass.set_vertex_buffer(0, pipeline.vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, instances.slice(..));
        pass.set_index_buffer(pipeline.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(
            0..CUBE_INDICES.len() as u32,
            0,
            0..scene.instance_count() as u32,
        );
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(height),
            },
        },
        extent,
    );
    queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    // A completed map is the readiness signal; there are no sleeps or presentation races.
    device.poll(wgpu::Maintain::Wait);
    receiver
        .recv()
        .map_err(|error| GpuRendererError::Offscreen(error.to_string()))?
        .map_err(|error| GpuRendererError::Offscreen(error.to_string()))?;
    let mapped = readback.slice(..).get_mapped_range();
    let rgba = unpack_rows(&mapped, row_bytes as usize, padded_row as usize);
    drop(mapped);
    readback.unmap();
    let background = [
        config.clear_color.r,
        config.clear_color.g,
        config.clear_color.b,
    ]
    .map(|linear| {
        let srgb = if linear <= 0.0031308 {
            12.92 * linear
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        };
        (srgb * 255.0).round() as u8
    });
    let non_background_pixels = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| (0..3).any(|axis| pixel[axis].abs_diff(background[axis]) > 1))
        .count();
    Ok(OffscreenImage {
        width,
        height,
        rgba,
        adapter: adapter_info,
        non_background_pixels,
    })
}

fn unpack_rows(mapped: &[u8], row_bytes: usize, padded_row: usize) -> Vec<u8> {
    mapped
        .chunks_exact(padded_row)
        .flat_map(|row| row[..row_bytes].iter().copied())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readback_removes_alignment_padding_without_losing_rows() {
        assert_eq!(
            unpack_rows(&[1, 2, 3, 4, 99, 99, 5, 6, 7, 8, 99, 99], 4, 6),
            vec![1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn invalid_viewports_are_rejected_before_requesting_a_gpu() {
        for viewport in [[0, 32], [32, 0], [8193, 32]] {
            assert!(
                pollster::block_on(render_offscreen(
                    &RenderScene::default(),
                    &OrbitCamera::default(),
                    viewport
                ))
                .is_err()
            );
        }
    }

    #[test]
    #[ignore = "requires a native GPU or a software Vulkan adapter"]
    fn gpu_readback_contains_a_rendered_cube_with_unaligned_row_width() {
        let scene = RenderScene::from_instances(vec![crate::InstanceRaw {
            center: [0.0; 3],
            size: 1.0,
            color: [1.0, 0.4, 0.2, 1.0],
            intensity: -1.0,
            classification: -1.0,
        }]);
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(scene.bounds().unwrap(), [257, 193]));
        let image = pollster::block_on(render_offscreen(&scene, &camera, [257, 193])).unwrap();
        assert_eq!(image.rgba.len(), 257 * 193 * 4);
        assert!(image.non_background_pixels > 100);
        assert!(image.non_background_pixels < 257 * 193);
    }
}
