use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
};

use viewer_core::BlockId;

use crate::{
    CellColorRanges, ColorMode, GpuBudget, OrbitCamera, SceneBounds, VoxelBuildLimits,
    cell_render::CellRenderer,
};

mod blocks;
mod cells;
mod error;
mod init;
mod occlusion;
mod pick;
mod pipeline;
mod render;
mod stats;
mod view;

pub use blocks::BlockUploadStats;
pub use error::GpuRendererError;
pub use pick::PickHit;
pub use stats::GpuResourceStats;

#[cfg(test)]
use crate::{CellPoolError, InstanceRaw};
use blocks::GpuInstanceBuffer;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use blocks::scene_upload_metadata;
#[cfg(test)]
use cells::{check_pool_creation_budget, upload_creating_pool};
#[cfg(test)]
use init::{MAX_FRAMEBUFFER_DIMENSION, MAX_FRAMEBUFFER_PIXELS, bounded_framebuffer_size};
#[cfg(test)]
use pick::{PICK_CHUNK_INSTANCES, check_pick_candidate_budget, ray_box_entry};
pub(crate) use pick::{PICK_MAX_CANDIDATE_INSTANCES, PickChunk, PickRay, ray_box_interval};
#[cfg(test)]
pub(crate) use pick::{PickCopy, build_pick_chunks, resolve_pick_instances};
#[cfg(any(test, not(target_arch = "wasm32")))]
pub(crate) use pipeline::create_pipeline_resources;
pub(crate) use pipeline::{CUBE_INDICES, ViewUniform};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameStatus {
    Presented,
    SkippedTimeout,
    Reconfigured,
    Suspended,
}

/// How the device was lost: `destroyed` (an explicit `destroy()`, never recovered, R3-Q26) or
/// `unknown` (driver or GPU-process loss), and the message the driver gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLoss {
    pub reason: &'static str,
    pub message: String,
}

fn device_loss(reason: wgpu::DeviceLostReason, message: String) -> DeviceLoss {
    let reason = match reason {
        wgpu::DeviceLostReason::Destroyed => "destroyed",
        wgpu::DeviceLostReason::Unknown => "unknown",
    };
    DeviceLoss { reason, message }
}

/// GPU renderer for a surface owned by the calling application.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the flags are independent latches of surface state, not one state machine"
)]
pub struct SurfaceRenderer<'window> {
    /// Filled by the device-lost callback; the host reads it through `device_loss()`.
    device_loss: Arc<Mutex<Option<DeviceLoss>>>,
    adapter_info: wgpu::AdapterInfo,
    surface: wgpu::Surface<'window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    depth_texture: wgpu::Texture,
    hzb: Option<crate::hzb::HzbResources>,
    hzb_depth_supported: bool,
    hzb_reference_camera: Option<([[f32; 4]; 4], [f32; 4])>,
    hzb_scene_opaque: bool,
    hzb_count_readback: Option<wgpu::Buffer>,
    hzb_count_receiver: Option<mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
    frame_timer: Option<crate::frame_timer::FrameTimer>,
    /// Limits of the CPU path: `downlevel_defaults` with the adapter's resolution. Every CPU size
    /// and HZB decision uses these, not the (possibly raised) limits of the device.
    cpu_limits: wgpu::Limits,
    /// Pages of the pool this device runs (ADR 0040); `None`: the CPU path only.
    cell_pool_pages: Option<u32>,
    cell_pool_budget_bytes: u64,
    cell_pool: Option<CellRenderer>,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    scene_buffer: Option<GpuInstanceBuffer>,
    block_buffers: HashMap<BlockId, GpuInstanceBuffer>,
    visible_block_ids: Vec<BlockId>,
    visible_instance_count: usize,
    pick_chunks: Vec<PickChunk>,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    clear_color: wgpu::Color,
    uploaded_bytes: u64,
    suspended: bool,
    camera: OrbitCamera,
    scene_bounds: Option<SceneBounds>,
    gpu_budget: GpuBudget,
    voxel_build_limits: VoxelBuildLimits,
    intensity_range: Option<[f32; 2]>,
    elevation_range: Option<[f32; 2]>,
    color_mode: ColorMode,
    /// Ranges of the cut the cells draw, for the shader colour modes (`CellTarget::set_color_ranges`).
    cell_color_ranges: CellColorRanges,
    depth_shading: bool,
    visual_scale: f64,
    coordinate_origin: [f64; 3],
}

impl SurfaceRenderer<'_> {
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_config.format
    }

    /// The device loss reported by the driver, if any (the first one is kept).
    pub fn device_loss(&self) -> Option<DeviceLoss> {
        self.device_loss.lock().ok().and_then(|slot| slot.clone())
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    #[test]
    fn device_loss_reason_separates_destroy_from_driver_loss() {
        let destroyed = device_loss(wgpu::DeviceLostReason::Destroyed, "bye".into());
        assert_eq!(
            (destroyed.reason, destroyed.message.as_str()),
            ("destroyed", "bye")
        );
        let lost = device_loss(wgpu::DeviceLostReason::Unknown, "gpu crash".into());
        assert_eq!(lost.reason, "unknown");
    }

    #[test]
    fn failed_first_upload_drops_the_new_pool_and_keeps_an_existing_one() {
        let mut slot: Option<u8> = None;
        assert_eq!(
            upload_creating_pool(&mut slot, || 1, |_| Err::<(), _>("bad")),
            Err("bad")
        );
        assert_eq!(slot, None);
        assert_eq!(
            upload_creating_pool(&mut slot, || 1, |_| Ok::<(), ()>(())),
            Ok(true)
        );
        assert_eq!(slot, Some(1));
        assert_eq!(
            upload_creating_pool(&mut slot, || 2, |_| Err::<(), _>("bad")),
            Err("bad")
        );
        assert_eq!(slot, Some(1));
        assert_eq!(
            upload_creating_pool(&mut slot, || 2, |_| Ok::<(), ()>(())),
            Ok(false)
        );
    }

    #[test]
    fn pool_creation_budget_counts_only_the_pool() {
        assert!(check_pool_creation_budget(100, 100).is_ok());
        assert!(matches!(
            check_pool_creation_budget(101, 100),
            Err(GpuRendererError::CellPool(CellPoolError::BudgetExceeded {
                required_bytes: 101,
                max_bytes: 100
            }))
        ));
    }

    #[test]
    fn framebuffer_limits_apply_to_direct_renderer_callers() {
        let [width, height] = bounded_framebuffer_size(12_000, 8_000, 16_384);
        assert!(u64::from(width) * u64::from(height) <= MAX_FRAMEBUFFER_PIXELS);
        assert!(width <= MAX_FRAMEBUFFER_DIMENSION);
        assert!(height <= MAX_FRAMEBUFFER_DIMENSION);
        assert_eq!(bounded_framebuffer_size(u32::MAX, 1, 8_192), [4_096, 1]);
        assert_eq!(bounded_framebuffer_size(0, 800, 8_192), [0, 0]);
        assert_eq!(bounded_framebuffer_size(800, 600, 1_024), [800, 600]);
    }

    #[test]
    fn pick_candidate_budget_is_strict_and_bounded() {
        assert!(check_pick_candidate_budget(PICK_MAX_CANDIDATE_INSTANCES).is_ok());
        assert!(check_pick_candidate_budget(PICK_MAX_CANDIDATE_INSTANCES + 1).is_err());
    }

    #[test]
    fn ray_box_entry_orders_front_hits_and_rejects_misses() {
        let origin = glam::DVec3::ZERO;
        let direction = glam::DVec3::NEG_Z;
        assert_eq!(
            ray_box_entry(origin, direction, [-1.0; 3], [1.0; 3]),
            Some(0.0)
        );
        assert_eq!(
            ray_box_entry(origin, direction, [-1.0, -1.0, -4.0], [1.0, 1.0, -3.0]),
            Some(3.0)
        );
        assert_eq!(
            ray_box_entry(origin, direction, [2.0, -1.0, -4.0], [3.0, 1.0, -3.0]),
            None
        );
        assert_eq!(
            ray_box_entry(origin, glam::DVec3::Z, [-1.0; 3], [1.0, 1.0, -3.0]),
            None
        );
    }

    #[test]
    fn pick_chunk_bounds_cover_cube_faces_and_keep_instance_ranges() {
        let instances = (0..=PICK_CHUNK_INSTANCES)
            .map(|index| InstanceRaw {
                center: [index as f32, 0.0, 2.0],
                size: 2.0,
                color: [1.0; 4],
                intensity: -1.0,
                classification: -1.0,
            })
            .collect::<Vec<_>>();
        let chunks = build_pick_chunks(&instances, 1.0);
        assert_eq!(chunks.len(), 2);
        assert_eq!(
            (chunks[0].first_instance, chunks[0].instance_count),
            (0, 256)
        );
        assert_eq!(
            (chunks[1].first_instance, chunks[1].instance_count),
            (256, 1)
        );
        assert!(chunks[0].bounds.min[0] < -1.0 && chunks[0].bounds.min[0] > -1.000_001);
        assert!(chunks[0].bounds.min[1] < -1.0 && chunks[0].bounds.min[1] > -1.000_001);
        assert!(chunks[0].bounds.min[2] < 1.0 && chunks[0].bounds.min[2] > 0.999_999);
        assert!(chunks[0].bounds.max[0] > 256.0 && chunks[0].bounds.max[0] < 256.001);
    }

    #[test]
    fn pick_readback_selects_frontmost_voxel_and_restores_dataset_coordinates() {
        let instances = [
            InstanceRaw {
                center: [0.0, 0.0, -3.0],
                size: 1.0,
                color: [1.0; 4],
                intensity: -1.0,
                classification: -1.0,
            },
            InstanceRaw {
                center: [0.0, 0.0, -5.0],
                size: 1.0,
                color: [1.0; 4],
                intensity: -1.0,
                classification: -1.0,
            },
        ];
        let mapped = bytemuck::cast_slice(&instances);
        let copies = [PickCopy {
            staging_offset: 0,
            first_instance: 10,
            instance_count: 2,
        }];
        let hit = resolve_pick_instances(
            PickRay {
                origin: glam::DVec3::ZERO,
                direction: glam::DVec3::NEG_Z,
                near_distance: 6.0,
                far_distance: 10.0,
            },
            &copies,
            mapped,
            2.0,
            [1_000.0, 2_000.0, 3_000.0],
        )
        .unwrap()
        .unwrap();
        assert_eq!(hit.instance_index, 10);
        assert_eq!(hit.voxel_center, [1_000.0, 2_000.0, 2_997.0]);
        assert_eq!(hit.voxel_size, 1.0);
        assert_eq!(hit.position, [1_000.0, 2_000.0, 2_997.0]);
        assert_eq!(hit.distance, 3.0);
        assert!(
            resolve_pick_instances(
                PickRay {
                    origin: glam::DVec3::ZERO,
                    direction: glam::DVec3::X,
                    near_distance: 0.1,
                    far_distance: 10.0,
                },
                &copies,
                mapped,
                1.0,
                [0.0; 3],
            )
            .unwrap()
            .is_none()
        );
        assert!(
            resolve_pick_instances(
                PickRay {
                    origin: glam::DVec3::ZERO,
                    direction: glam::DVec3::NEG_Z,
                    near_distance: 0.1,
                    far_distance: 10.0,
                },
                &copies,
                &[],
                1.0,
                [0.0; 3],
            )
            .is_err()
        );
    }
}
