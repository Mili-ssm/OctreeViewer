//! Hook for `examples/cell_pool_bench.rs` (R3 Task 7c): the crate-private `CellRenderer` behind a
//! thin `#[doc(hidden)]` wrapper, so a native example can time the production cell path. It adds no
//! behaviour; every method forwards. Not a stable API.

use crate::cell_pages::{PackedLevel, PageEntry};
use crate::cell_render::{
    CellFrame, CellLimitMode, CellPoolError, CellRenderer, MASK_WORDS, cells_supported,
    plan_device_limits,
};

/// The device limits the production pool needs on `adapter` (what `SurfaceRenderer` requests).
pub fn bench_device_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
    plan_device_limits(adapter, CellLimitMode::Auto).required_limits
}

/// Whether `device_limits` run the production pool on `adapter`.
pub fn bench_cells_supported(adapter: &wgpu::Adapter, device_limits: &wgpu::Limits) -> bool {
    cells_supported(adapter, device_limits)
}

pub struct BenchRenderer(CellRenderer);

impl BenchRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat,
        capacity: u32,
    ) -> Self {
        Self(CellRenderer::new(device, queue, color_format, capacity))
    }

    pub fn set_occlusion(&mut self, enabled: bool) {
        self.0.set_occlusion(enabled);
    }

    pub fn upload_level(
        &mut self,
        level: &PackedLevel,
        page_slots: &[u32],
    ) -> Result<(), CellPoolError> {
        self.0.upload_level(level, page_slots)
    }

    pub fn free_pages(&mut self, page_slots: &[u32]) -> Result<(), CellPoolError> {
        self.0.free_pages(page_slots)
    }

    pub fn alloc_mask_slot(&mut self) -> Option<u32> {
        self.0.alloc_mask_slot()
    }

    pub fn set_node_mask(
        &mut self,
        slot: u32,
        mask: &[u32; MASK_WORDS],
    ) -> Result<(), CellPoolError> {
        self.0.set_node_mask(slot, mask)
    }

    pub fn set_active_pages(&mut self, pages: &[(u32, PageEntry)]) -> Result<(), CellPoolError> {
        self.0.set_active_pages(pages)
    }

    /// `view_projection` is `EyeFrame::view_projection`; depth shading on, as the viewer.
    pub fn encode_cells(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        view_projection: [[f32; 4]; 4],
        viewport: [u32; 2],
    ) {
        self.0.encode_cells(
            encoder,
            color_view,
            depth_view,
            &CellFrame {
                view_projection,
                clear_color: wgpu::Color::BLACK,
                depth_shading: true,
                viewport,
            },
        );
    }

    /// Indirect draw arguments (20 bytes per phase); word 1 of phase 1 (or of the plain pass) and
    /// word 6 of phase 2 are the cells drawn.
    pub fn draw_args_buffer(&self) -> &wgpu::Buffer {
        self.0.draw_args_buffer()
    }
}
