//! State of a GPU-free LOD scene builder: the dataset, its stage machine and the colour
//! selection, driven by an explicit camera instead of a renderer.
//!
//! `WasmSceneBuilder` wraps this for JavaScript. It lives outside the bindings so native tests
//! can run the exact same code and compare its scenes with the viewer's.

use viewer_core::{BlockId, NodeId, PointBlock};
use viewer_render_wgpu::{
    ColorMode, ORBIT_CAMERA_F64_LEN, OrbitCamera, ViewProjection, VoxelBuildLimits,
};

use crate::{
    LodBuiltScene, LodDataset, LodViewStats, PageInsert,
    cells_glue::{BuilderCells, BuilderOutput},
    lod_driver::{ColorSelection, LodStep, LodStepResult, SceneSink, update_lod_view_paged},
    lod_session::LodSession,
};

// Same bounds as `WasmVoxelViewer::updateLod`.
use crate::cells_glue::TARGET_PIXELS_RANGE;

/// Keeps the rebuilt scene for the caller instead of uploading it.
#[derive(Default)]
struct CollectSink {
    scene: Option<LodBuiltScene>,
}

impl SceneSink for CollectSink {
    fn publish(&mut self, built: LodBuiltScene) -> Result<(), String> {
        self.scene = Some(built);
        Ok(())
    }

    fn base_cut_exceeded(&mut self, _dataset: &LodDataset) -> Result<(), String> {
        // Nothing is displayed here; the caller learns about it through the update error.
        Ok(())
    }
}

/// One finished update.
pub(crate) struct BuilderUpdate {
    /// Counts successful updates, so a consumer can discard results that arrive out of order.
    pub epoch: u64,
    /// `None` when the previous scene still applies.
    pub scene: Option<LodBuiltScene>,
    pub stats: LodViewStats,
    pub async_priority_order: Vec<BlockId>,
    /// More refinement stages (or newly arrived blocks) call for another update.
    pub needs_refinement: bool,
}

/// What one builder update produced: the cells path never carries a scene, so the render worker
/// keeps its pool; the CPU path never carries cells.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[allow(clippy::large_enum_variant)] // a transient return value, built once per update
pub(crate) enum BuilderStep {
    Cells(BuilderOutput),
    Scene(BuilderUpdate),
}

pub(crate) struct SceneBuilderCore<R> {
    session: LodSession<R>,
    /// Blocks are delivered by the host rather than by a reader owned by the session.
    host_delivers_blocks: bool,
    limits: VoxelBuildLimits,
    color_mode: ColorMode,
    selected_attribute: Option<(u32, u32)>,
    selected_attribute_range: Option<[f64; 2]>,
    target_pixels: Option<f64>,
    stage_pixels: Option<f64>,
    dirty: bool,
    last_camera: Option<OrbitCamera>,
    epoch: u64,
}

impl<R> SceneBuilderCore<R> {
    pub(crate) fn new(
        session: LodSession<R>,
        host_delivers_blocks: bool,
        limits: VoxelBuildLimits,
    ) -> Self {
        Self {
            session,
            host_delivers_blocks,
            limits,
            color_mode: ColorMode::Rgb,
            selected_attribute: None,
            selected_attribute_range: None,
            target_pixels: None,
            stage_pixels: None,
            dirty: false,
            last_camera: None,
            epoch: 0,
        }
    }

    pub(crate) fn session(&self) -> &LodSession<R> {
        &self.session
    }

    pub(crate) fn session_mut(&mut self) -> &mut LodSession<R> {
        &mut self.session
    }

    pub(crate) fn color_mode(&self) -> ColorMode {
        self.color_mode
    }

    pub(crate) fn selected_attribute(&self) -> (Option<(u32, u32)>, Option<[f64; 2]>) {
        (self.selected_attribute, self.selected_attribute_range)
    }

    /// Applies a parsed colour selection. A change drops every decoded block, because blocks are
    /// painted when they are loaded; returns whether anything changed.
    pub(crate) fn set_color_selection(&mut self, selection: ColorSelection) -> bool {
        if self.color_mode == selection.mode && self.selected_attribute == selection.attribute {
            return false;
        }
        self.selected_attribute = selection.attribute;
        self.selected_attribute_range = selection.attribute_range;
        self.color_mode = selection.mode;
        self.session.dataset_mut().clear_loaded_blocks();
        self.stage_pixels = Some(16.0);
        self.dirty = true;
        true
    }

    /// Records that a block entered the cache, so the next update may refine further.
    pub(crate) fn block_inserted(&mut self) {
        self.dirty = self.target_pixels.is_some();
    }

    /// Selects the cut for `camera` and rebuilds the scene when it changed.
    ///
    /// `load_block` serves blocks the session's reader can supply; hosts that deliver blocks
    /// asynchronously return `Ok(None)` and read `async_priority_order` instead. `load_page`
    /// reads the hierarchy pages of a paged reader.
    pub(crate) fn update(
        &mut self,
        camera: OrbitCamera,
        viewport: [u32; 2],
        visual_scale: f64,
        target_pixels: f64,
        mut load_block: impl FnMut(&mut R, BlockId) -> Result<Option<PointBlock>, String>,
        mut load_page: impl FnMut(&mut R, NodeId) -> Result<PageInsert, String>,
    ) -> Result<BuilderUpdate, String> {
        if !TARGET_PIXELS_RANGE.contains(&target_pixels) {
            return Err("targetVoxelPixels must be finite and between 0.5 and 16".to_owned());
        }
        if viewport.contains(&0) {
            return Err("viewport must be non-empty".to_owned());
        }
        if visual_scale != self.session.dataset().visual_scale() {
            return Err("visualScale does not match the dataset".to_owned());
        }
        if self.target_pixels != Some(target_pixels) {
            self.stage_pixels = None;
        }
        if self.last_camera != Some(camera) {
            self.dirty = true;
        }
        self.last_camera = Some(camera);
        self.target_pixels = Some(target_pixels);

        let view = ViewProjection::new(camera, viewport, visual_scale, self.limits);
        let can_load_blocks = self.host_delivers_blocks || self.session.reader().is_some();
        let (dataset, reader) = self.session.parts_mut();
        let mut sink = CollectSink::default();
        let mut stage_pixels = self.stage_pixels;
        let mut dirty = self.dirty;
        let result = update_lod_view_paged(
            dataset,
            &LodStep {
                view: &view,
                target_pixels,
                track_async_order: self.host_delivers_blocks,
                can_load_blocks,
            },
            &mut stage_pixels,
            &mut dirty,
            &mut sink,
            reader,
            &mut load_block,
            &mut load_page,
        );
        self.stage_pixels = stage_pixels;
        self.dirty = dirty;
        let LodStepResult {
            stats,
            async_priority_order,
        } = result?;
        self.epoch += 1;
        Ok(BuilderUpdate {
            epoch: self.epoch,
            scene: sink.scene,
            stats,
            async_priority_order,
            needs_refinement: self.dirty,
        })
    }
}

impl<R> SceneBuilderCore<R> {
    /// [`update`](Self::update) on the CPU path, or step 1 of the cells path while `cells` is
    /// present. Same arguments otherwise.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn update_with_cells(
        &mut self,
        cells: Option<&mut BuilderCells>,
        camera: OrbitCamera,
        viewport: [u32; 2],
        visual_scale: f64,
        target_pixels: f64,
        mut load_block: impl FnMut(&mut R, BlockId) -> Result<Option<PointBlock>, String>,
        mut load_page: impl FnMut(&mut R, NodeId) -> Result<PageInsert, String>,
    ) -> Result<BuilderStep, String> {
        let Some(cells) = cells else {
            return self
                .update(
                    camera,
                    viewport,
                    visual_scale,
                    target_pixels,
                    load_block,
                    load_page,
                )
                .map(BuilderStep::Scene);
        };
        let view = cells.begin_update(
            self.session.dataset(),
            camera,
            viewport,
            visual_scale,
            target_pixels,
            self.limits,
        )?;
        let can_load_blocks = self.host_delivers_blocks || self.session.reader().is_some();
        let (dataset, reader) = self.session.parts_mut();
        cells
            .plan(
                dataset,
                &view,
                target_pixels,
                can_load_blocks,
                self.host_delivers_blocks,
                reader,
                &mut load_block,
                &mut load_page,
            )
            .map(BuilderStep::Cells)
    }
}

/// Decodes the camera array a host sends along with each update.
pub(crate) fn camera_from_transport(values: &[f64]) -> Result<OrbitCamera, String> {
    OrbitCamera::from_f64_array(values).ok_or_else(|| {
        format!("camera must be {ORBIT_CAMERA_F64_LEN} numbers with a 0 or 1 fitted flag")
    })
}
