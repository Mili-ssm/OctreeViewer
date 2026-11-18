//! The cell pool of the renderer and its `CellTarget` implementation.

use super::{SurfaceRenderer, error::GpuRendererError};

use crate::{
    CellColorRanges, CellPoolError,
    budget::check_peak_bytes,
    cell_driver::CellTarget,
    cell_lod::LodView,
    cell_pages::{PackedLevel, PageEntry, PageFrame},
    cell_render::{CellRenderer, NODE_WORDS, PageFrameStamp, pool_resource_bytes},
};

/// The pool is allocated alone: the CPU scene is released once the first level is uploaded, so
/// only the pool counts against the budget.
pub(super) fn check_pool_creation_budget(
    pool_bytes: u64,
    max_bytes: u64,
) -> Result<(), GpuRendererError> {
    check_peak_bytes(0, pool_bytes, max_bytes).map_err(|_| {
        GpuRendererError::CellPool(CellPoolError::BudgetExceeded {
            required_bytes: pool_bytes,
            max_bytes,
        })
    })
}

/// Runs `upload` on the pool, creating it first when absent. If that call created the pool and
/// fails, the pool is dropped. Returns whether this call created the pool.
pub(super) fn upload_creating_pool<P, E>(
    slot: &mut Option<P>,
    create: impl FnOnce() -> P,
    upload: impl FnOnce(&mut P) -> Result<(), E>,
) -> Result<bool, E> {
    let created = slot.is_none();
    let pool = slot.get_or_insert_with(create);
    if let Err(error) = upload(pool) {
        if created {
            *slot = None;
        }
        return Err(error);
    }
    Ok(created)
}

/// The cell driver (`CellDriver`) writes through the renderer's public cell methods.
impl CellTarget for SurfaceRenderer<'_> {
    type Error = GpuRendererError;

    fn has_pool(&self) -> bool {
        self.cells_active()
    }

    fn drop_pool(&mut self) {
        self.clear_cells();
    }

    fn eye(&self) -> [f64; 3] {
        self.cell_eye()
    }

    fn upload_level(
        &mut self,
        level: &PackedLevel,
        page_slots: &[u32],
    ) -> Result<(), GpuRendererError> {
        self.upload_cell_level(level, page_slots)
    }

    fn free_pages(&mut self, page_slots: &[u32]) -> Result<(), GpuRendererError> {
        self.free_cell_pages(page_slots)
    }

    fn alloc_mask_slot(&mut self) -> Result<Option<u32>, GpuRendererError> {
        self.alloc_cell_mask_slot()
    }

    fn set_node_record(
        &mut self,
        slot: u32,
        record: &[u32; NODE_WORDS],
    ) -> Result<(), GpuRendererError> {
        self.set_cell_node_record(slot, record)
    }

    fn set_color_ranges(&mut self, ranges: CellColorRanges) {
        self.cell_color_ranges = ranges;
    }

    fn set_lod_view(&mut self, view: Option<LodView>) {
        if let Some(pool) = &mut self.cell_pool {
            pool.set_lod_view(view);
        }
    }

    fn set_active_pages(
        &mut self,
        pages: &[(u32, PageEntry)],
        frame: &PageFrame,
    ) -> Result<(), GpuRendererError> {
        self.set_active_cell_pages(pages, frame)
    }
}

impl<'window> SurfaceRenderer<'window> {
    pub(super) fn cell_pool_bytes(&self) -> u64 {
        self.cell_pool
            .as_ref()
            .map_or(0, CellRenderer::resource_bytes)
    }

    /// Whether the device meets the limits and features of the cell pool (R3-Q5). When false, only
    /// the CPU path is available and the cell methods return `CellPoolError::Unsupported`.
    pub fn cells_supported(&self) -> bool {
        self.cell_pool_pages.is_some()
    }

    /// Pages of the cell pool this device runs: 65,536 (512 MiB) or what its limits admit, at
    /// least 32,768 (ADR 0040). `None` when only the CPU path is available. The driver's
    /// `PoolConfig::capacity_pages` must be this value.
    pub fn cell_pool_pages(&self) -> Option<u32> {
        self.cell_pool_pages
    }

    /// Whether the cell pool exists, which is when `render` draws cells instead of the CPU scene.
    pub fn cells_active(&self) -> bool {
        self.cell_pool.is_some()
    }

    /// The camera eye in scaled local units, in `f64`: `PageFrame::eye_scaled` for the page
    /// entries of this frame.
    pub fn cell_eye(&self) -> [f64; 3] {
        self.camera.position()
    }

    /// Writes a packed level into the given page slots. The first call creates the pool and, once
    /// the upload succeeds, releases the CPU scene: the CPU path and the cells path are exclusive.
    /// If that first call fails, the new pool is dropped and the renderer stays as it was. The pool
    /// is freed by `upload_scene`, `upload_voxel_cut`, `sync_visible_blocks`, `clear_dataset` and
    /// `clear_cells`.
    pub fn upload_cell_level(
        &mut self,
        level: &PackedLevel,
        page_slots: &[u32],
    ) -> Result<(), GpuRendererError> {
        let Some(pool_pages) = self.cell_pool_pages else {
            return Err(GpuRendererError::CellPool(CellPoolError::Unsupported));
        };
        if self.cell_pool.is_none() {
            // The pages were chosen to fit this budget; the check guards the invariant.
            check_pool_creation_budget(
                pool_resource_bytes(pool_pages),
                self.cell_pool_budget_bytes,
            )?;
        }
        let device = &self.device;
        let queue = &self.queue;
        let format = self.surface_config.format;
        let created = upload_creating_pool(
            &mut self.cell_pool,
            || CellRenderer::new(device, queue, format, pool_pages),
            |pool| pool.upload_level(level, page_slots),
        )
        .map_err(GpuRendererError::CellPool)?;
        if created {
            self.release_cpu_scene();
        }
        Ok(())
    }

    fn existing_cell_pool(&mut self) -> Result<&mut CellRenderer, GpuRendererError> {
        self.cell_pool
            .as_mut()
            .ok_or(GpuRendererError::CellPool(CellPoolError::NoPool))
    }

    /// Makes the pages inert; do this before uploading other levels into the same slots.
    pub fn free_cell_pages(&mut self, page_slots: &[u32]) -> Result<(), GpuRendererError> {
        match &mut self.cell_pool {
            Some(pool) => pool
                .free_pages(page_slots)
                .map_err(GpuRendererError::CellPool),
            None => Err(GpuRendererError::CellPool(CellPoolError::NoPool)),
        }
    }

    pub fn alloc_cell_mask_slot(&mut self) -> Result<Option<u32>, GpuRendererError> {
        Ok(self.existing_cell_pool()?.alloc_mask_slot())
    }

    pub fn free_cell_mask_slot(&mut self, slot: u32) -> Result<(), GpuRendererError> {
        match &mut self.cell_pool {
            Some(pool) => pool
                .free_mask_slot(slot)
                .map_err(GpuRendererError::CellPool),
            None => Err(GpuRendererError::CellPool(CellPoolError::NoPool)),
        }
    }

    /// Writes a node record; in its mask bit `x + 8y + 64z` set means that cell of the node is
    /// replaced by finer ones and not drawn.
    pub fn set_cell_node_record(
        &mut self,
        slot: u32,
        record: &[u32; NODE_WORDS],
    ) -> Result<(), GpuRendererError> {
        self.existing_cell_pool()?
            .set_node_record(slot, record)
            .map_err(GpuRendererError::CellPool)
    }

    /// The pages to draw this frame, with eye-relative origins and octant bits (see
    /// `PageInfo::entry` and `cell_eye`). `frame` is the one the entries were built with: its
    /// coordinate origin and visual scale must be the renderer's, and its eye is kept for picking.
    pub fn set_active_cell_pages(
        &mut self,
        pages: &[(u32, PageEntry)],
        frame: &PageFrame,
    ) -> Result<(), GpuRendererError> {
        let (visual_scale, coordinate_origin) = (self.visual_scale, self.coordinate_origin);
        let pool = self.existing_cell_pool()?;
        let stamp = PageFrameStamp::of(frame);
        if !stamp.same_units(visual_scale, coordinate_origin) {
            return Err(GpuRendererError::PageFrameMismatch);
        }
        pool.set_active_pages(pages)
            .map_err(GpuRendererError::CellPool)?;
        pool.set_page_frame(stamp);
        Ok(())
    }

    /// Frees the pool; `render` draws the CPU scene again.
    pub fn clear_cells(&mut self) {
        self.cell_pool = None;
    }
}
