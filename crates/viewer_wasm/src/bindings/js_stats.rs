use std::sync::Mutex;

use serde_wasm_bindgen::to_value;
use wasm_bindgen::prelude::*;

use super::WasmVoxelViewer;
use crate::{LodDataset, LodViewStats, browser_io::BrowserReadStats, lod_session::LodSession};
use viewer_render_wgpu::SurfaceRenderer;

#[wasm_bindgen]
impl WasmVoxelViewer {
    #[wasm_bindgen(js_name = lodStats)]
    pub fn lod_stats(&self) -> Result<JsValue, JsValue> {
        match &self.last_lod_stats {
            Some((stats, range_read_stats)) => {
                let mut current_stats = stats.clone();
                if let Some(dataset) = self.lod_session.as_ref().map(LodSession::dataset) {
                    current_stats.refinement_work = dataset.refinement_work().clone();
                }
                let value = lod_view_stats_to_js(&current_stats, *range_read_stats)?;
                add_runtime_gpu_stats(&value, self.gpu_renderer.as_ref())?;
                add_cache_activity(&value, self.lod_session.as_ref().map(LodSession::dataset))?;
                js_sys::Reflect::set(
                    &value,
                    &"lodError".into(),
                    &self
                        .last_lod_error
                        .as_ref()
                        .map_or(JsValue::NULL, |message| JsValue::from_str(message)),
                )?;
                js_sys::Reflect::set(
                    &value,
                    &"sceneDigest".into(),
                    &self.last_scene_digest.map_or(JsValue::NULL, |digest| {
                        JsValue::from_str(&format!("{digest:016x}"))
                    }),
                )?;
                js_sys::Reflect::set(
                    &value,
                    &"attributeRange".into(),
                    &super::selected_range_to_js(
                        self.selected_attribute.and(self.selected_attribute_range),
                    ),
                )?;
                if let Some(cells) = self.cells_stats() {
                    super::cells::add_cells_stats(&value, self.cell_path, &cells)?;
                }
                Ok(value)
            }
            None => match self.cells_stats() {
                // In-process cells have no `LodViewStats`: the host reads the plan's fields (pending
                // ids, uncovered nodes) beside the cells'. A remote render worker has only the
                // cells fields; its plan arrives from the build worker.
                Some(cells) => {
                    let value = super::cells::cells_stats_object(self.cell_path, &cells)?;
                    if let Some((plan, target_voxel_pixels)) = &self.last_cells_plan {
                        super::cells::add_plan_stats(&value, plan)?;
                        js_sys::Reflect::set(
                            &value,
                            &"targetVoxelPixels".into(),
                            &JsValue::from_f64(*target_voxel_pixels),
                        )?;
                        js_sys::Reflect::set(
                            &value,
                            &"lodError".into(),
                            &self
                                .last_lod_error
                                .as_ref()
                                .map_or(JsValue::NULL, |message| JsValue::from_str(message)),
                        )?;
                    }
                    Ok(value)
                }
                None => Ok(JsValue::NULL),
            },
        }
    }

    #[wasm_bindgen(js_name = stats)]
    pub fn stats(&self) -> Result<JsValue, JsValue> {
        to_value(&self.inner.stats()).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    #[wasm_bindgen(js_name = gpuStats)]
    pub fn gpu_stats(&self) -> Result<JsValue, JsValue> {
        let renderer = self
            .gpu_renderer
            .as_ref()
            .ok_or_else(|| JsValue::from_str("GPU renderer has not been initialized"))?;
        let stats = js_sys::Object::new();
        let adapter_info = renderer.adapter_info();
        let adapter = js_sys::Object::new();
        js_sys::Reflect::set(
            &adapter,
            &"name".into(),
            &JsValue::from_str(&adapter_info.name),
        )?;
        js_sys::Reflect::set(
            &adapter,
            &"backend".into(),
            &JsValue::from_str(&format!("{:?}", adapter_info.backend)),
        )?;
        js_sys::Reflect::set(
            &adapter,
            &"deviceType".into(),
            &JsValue::from_str(&format!("{:?}", adapter_info.device_type)),
        )?;
        js_sys::Reflect::set(
            &adapter,
            &"driver".into(),
            &JsValue::from_str(&adapter_info.driver),
        )?;
        js_sys::Reflect::set(
            &adapter,
            &"driverInfo".into(),
            &JsValue::from_str(&adapter_info.driver_info),
        )?;
        js_sys::Reflect::set(&stats, &"adapter".into(), &adapter)?;
        js_sys::Reflect::set(
            &stats,
            &"uploadedBytes".into(),
            &JsValue::from_str(&renderer.uploaded_bytes().to_string()),
        )?;
        js_sys::Reflect::set(
            &stats,
            &"visibleInstances".into(),
            &JsValue::from_f64(renderer.visible_instance_count() as f64),
        )?;
        js_sys::Reflect::set(
            &stats,
            &"visibleBlocks".into(),
            &JsValue::from_f64(renderer.visible_block_count() as f64),
        )?;
        js_sys::Reflect::set(
            &stats,
            &"hzbActive".into(),
            &JsValue::from_bool(renderer.hzb_active()),
        )?;
        let resources = renderer.resource_stats();
        let memory = js_sys::Object::new();
        for (name, bytes) in [
            ("instanceBufferBytes", resources.instance_buffer_bytes),
            ("fixedBufferBytes", resources.fixed_buffer_bytes),
            ("hzbBufferBytes", resources.hzb_buffer_bytes),
            ("depthTextureBytes", resources.depth_texture_bytes),
            ("hzbTextureBytes", resources.hzb_texture_bytes),
            ("totalBytes", resources.total_bytes()),
        ] {
            js_sys::Reflect::set(
                &memory,
                &name.into(),
                &JsValue::from_str(&bytes.to_string()),
            )?;
        }
        js_sys::Reflect::set(&stats, &"requestedResources".into(), &memory)?;
        // GPU time per frame from timestamp queries (R3-Q31); null without the feature or samples.
        let frame_ms = match renderer.gpu_frame_stats() {
            Some(frame) => {
                let object = js_sys::Object::new();
                for (name, value) in [
                    ("p50", frame.p50_ms),
                    ("p95", frame.p95_ms),
                    ("max", frame.max_ms),
                    ("samples", frame.samples as f64),
                ] {
                    js_sys::Reflect::set(&object, &name.into(), &JsValue::from_f64(value))?;
                }
                object.into()
            }
            None => JsValue::NULL,
        };
        js_sys::Reflect::set(&stats, &"gpuFrameMs".into(), &frame_ms)?;
        Ok(stats.into())
    }

    /// Starts a new window for the `gpuFrameMs` percentiles.
    #[wasm_bindgen(js_name = resetGpuFrameStats)]
    pub fn reset_gpu_frame_stats(&mut self) {
        if let Some(renderer) = self.gpu_renderer.as_mut() {
            renderer.reset_gpu_frame_stats();
        }
    }
}

pub(super) fn accumulate_browser_read_delta(
    total: &mut BrowserReadStats,
    before: BrowserReadStats,
    after: BrowserReadStats,
) {
    total.calls = total
        .calls
        .saturating_add(after.calls.saturating_sub(before.calls));
    total.bytes = total
        .bytes
        .saturating_add(after.bytes.saturating_sub(before.bytes));
    total.elapsed_ms += (after.elapsed_ms - before.elapsed_ms).max(0.0);
}

pub(super) fn snapshot_browser_read_stats(stats: &Mutex<BrowserReadStats>) -> BrowserReadStats {
    *stats
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) fn reset_browser_read_stats(stats: &Mutex<BrowserReadStats>) {
    *stats
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = BrowserReadStats::default();
}

// Cache activity is live even when the last accepted scene predates a failed insertion.
pub(super) fn add_cache_activity(
    value: &JsValue,
    dataset: Option<&LodDataset>,
) -> Result<(), JsValue> {
    let Some(dataset) = dataset else {
        return Ok(());
    };
    let stats = dataset.cache_stats();
    let activity = js_sys::Object::new();
    for (name, count) in [
        ("insertions", stats.insertions),
        ("evictions", stats.evictions),
        ("rejectedOversize", stats.rejected_oversize),
        ("rejectedProtected", stats.rejected_protected),
    ] {
        js_sys::Reflect::set(
            &activity,
            &name.into(),
            &JsValue::from_str(&count.to_string()),
        )?;
    }
    js_sys::Reflect::set(value, &"cacheActivity".into(), &activity)?;
    let (pages, page_bytes, page_evictions) = dataset.page_stats();
    let page_activity = js_sys::Object::new();
    for (name, count) in [
        ("residentPages", pages as f64),
        ("pageBytes", page_bytes as f64),
        ("pageEvictions", page_evictions as f64),
        ("residentNodes", dataset.resident_node_count() as f64),
    ] {
        js_sys::Reflect::set(&page_activity, &name.into(), &JsValue::from_f64(count))?;
    }
    js_sys::Reflect::set(value, &"pageActivity".into(), &page_activity)?;
    js_sys::Reflect::set(
        value,
        &"pendingTargetEvictions".into(),
        &JsValue::from_f64(dataset.pending_target_evictions() as f64),
    )?;
    js_sys::Reflect::set(
        value,
        &"cacheCycleDetected".into(),
        &JsValue::from_bool(dataset.cache_cycle_detected()),
    )?;
    Ok(())
}

pub(super) fn lod_view_stats_to_js(
    stats: &LodViewStats,
    range_read_stats: BrowserReadStats,
) -> Result<JsValue, JsValue> {
    let value = js_sys::Object::new();
    for (name, number) in [
        ("minVoxelEdgePixels", stats.min_voxel_edge_pixels),
        ("maxVoxelEdgePixels", stats.max_voxel_edge_pixels),
        ("oversizedVoxels", stats.oversized_voxels as f64),
        (
            "projectionCachePeakEntries",
            stats.projection_cache_peak_entries as f64,
        ),
    ] {
        js_sys::Reflect::set(&value, &name.into(), &JsValue::from_f64(number))?;
    }
    for (name, bytes) in [
        ("voxelBuildBudgetBytes", stats.voxel_build_budget_bytes),
        (
            "voxelBuildEstimatedPeakBytes",
            stats.voxel_build_estimated_peak_bytes,
        ),
        (
            "voxelInstanceCapacityBytes",
            stats.voxel_instance_capacity_bytes,
        ),
    ] {
        js_sys::Reflect::set(&value, &name.into(), &JsValue::from_str(&bytes.to_string()))?;
    }
    js_sys::Reflect::set(
        &value,
        &"targetVoxelPixels".into(),
        &JsValue::from_f64(stats.target_voxel_pixels),
    )?;
    js_sys::Reflect::set(
        &value,
        &"visibleBlocks".into(),
        &JsValue::from_f64(stats.visible_blocks as f64),
    )?;
    js_sys::Reflect::set(
        &value,
        &"visiblePoints".into(),
        &JsValue::from_str(&stats.visible_points.to_string()),
    )?;
    js_sys::Reflect::set(
        &value,
        &"renderedVoxels".into(),
        &JsValue::from_f64(stats.rendered_voxels as f64),
    )?;
    js_sys::Reflect::set(
        &value,
        &"voxelBudget".into(),
        &JsValue::from_f64(stats.voxel_budget as f64),
    )?;
    js_sys::Reflect::set(
        &value,
        &"lodStagePixels".into(),
        &JsValue::from_f64(stats.lod_stage_pixels),
    )?;
    js_sys::Reflect::set(
        &value,
        &"maxVisibleLevel".into(),
        &stats
            .max_visible_level
            .map_or(JsValue::NULL, |level| JsValue::from_f64(f64::from(level))),
    )?;
    js_sys::Reflect::set(
        &value,
        &"cacheBytes".into(),
        &JsValue::from_str(&stats.cache_bytes.to_string()),
    )?;
    js_sys::Reflect::set(
        &value,
        &"cacheBudgetBytes".into(),
        &JsValue::from_str(&stats.cache_budget_bytes.to_string()),
    )?;
    let pending_block_ids = js_sys::Array::new();
    for block_id in &stats.pending_block_ids {
        pending_block_ids.push(&JsValue::from_str(&block_id.get().to_string()));
    }
    js_sys::Reflect::set(&value, &"pendingBlockIds".into(), &pending_block_ids)?;
    // Hierarchy pages the target needs, highest projected error first (paged formats only).
    let pending_page_anchors = js_sys::Array::new();
    for anchor in &stats.pending_page_anchors {
        pending_page_anchors.push(&JsValue::from_str(&anchor.get().to_string()));
    }
    js_sys::Reflect::set(&value, &"pendingPageAnchors".into(), &pending_page_anchors)?;
    js_sys::Reflect::set(
        &value,
        &"pageBudgetExhausted".into(),
        &JsValue::from_bool(stats.page_budget_exhausted),
    )?;
    js_sys::Reflect::set(
        &value,
        &"uncoveredNodes".into(),
        &JsValue::from_f64(stats.uncovered_nodes as f64),
    )?;
    js_sys::Reflect::set(
        &value,
        &"coverageComplete".into(),
        &JsValue::from_bool(stats.uncovered_nodes == 0),
    )?;
    js_sys::Reflect::set(
        &value,
        &"budgetOmittedNodes".into(),
        &JsValue::from_f64(stats.budget_omitted_nodes as f64),
    )?;
    let work = js_sys::Object::new();
    for (name, number) in [
        ("updateCalls", stats.refinement_work.update_calls as f64),
        (
            "blockLoadCalls",
            stats.refinement_work.block_load_calls as f64,
        ),
        ("loadedBlocks", stats.refinement_work.loaded_blocks as f64),
        (
            "evictedPendingBlocks",
            stats.refinement_work.evicted_pending_blocks as f64,
        ),
        ("sceneBuilds", stats.refinement_work.scene_builds as f64),
        (
            "prefixRebuilds",
            stats.refinement_work.prefix_rebuilds as f64,
        ),
        (
            "prefixVoxelBuildMs",
            stats.refinement_work.prefix_voxel_build_ms,
        ),
        (
            "reusedSceneUpdates",
            stats.refinement_work.reused_scene_updates as f64,
        ),
        ("selections", stats.refinement_work.selections as f64),
        ("prepares", stats.refinement_work.prepares as f64),
        // Cumulative selections/prepares over stage evaluations; 1.0 means no redundant work.
        (
            "selectionsPerUpdate",
            per_update(
                stats.refinement_work.selections,
                stats.refinement_work.update_calls,
            ),
        ),
        (
            "preparesPerUpdate",
            per_update(
                stats.refinement_work.prepares,
                stats.refinement_work.update_calls,
            ),
        ),
        ("selectionMs", stats.refinement_work.selection_ms),
        ("planningMs", stats.refinement_work.planning_ms),
        ("blockLoadMs", stats.refinement_work.block_load_ms),
        ("rangeReadMs", range_read_stats.elapsed_ms),
        // Residual synchronous block-callback time; it is not pure decompression CPU time.
        (
            "decodeAndOtherCpuMs",
            (stats.refinement_work.block_load_ms - range_read_stats.elapsed_ms).max(0.0),
        ),
        ("voxelBuildMs", stats.refinement_work.voxel_build_ms),
        ("uploadMs", stats.refinement_work.upload_ms),
        (
            "unchangedCutBuilds",
            stats.refinement_work.unchanged_cut_builds as f64,
        ),
        (
            "reusableSceneBuilds",
            stats.refinement_work.reusable_scene_builds as f64,
        ),
        (
            "reusableVoxelBuildMs",
            stats.refinement_work.reusable_voxel_build_ms,
        ),
    ] {
        js_sys::Reflect::set(&work, &name.into(), &JsValue::from_f64(number))?;
    }
    js_sys::Reflect::set(
        &work,
        &"rangeReadCalls".into(),
        &JsValue::from_f64(range_read_stats.calls as f64),
    )?;
    js_sys::Reflect::set(
        &work,
        &"rangeReadBytes".into(),
        &JsValue::from_str(&range_read_stats.bytes.to_string()),
    )?;
    js_sys::Reflect::set(
        &work,
        &"voxelSourcePoints".into(),
        &JsValue::from_str(&stats.refinement_work.voxel_source_points.to_string()),
    )?;
    js_sys::Reflect::set(
        &work,
        &"reusableVoxelSourcePoints".into(),
        &JsValue::from_str(
            &stats
                .refinement_work
                .reusable_voxel_source_points
                .to_string(),
        ),
    )?;
    js_sys::Reflect::set(
        &work,
        &"reusedSourcePoints".into(),
        &JsValue::from_str(&stats.refinement_work.reused_source_points.to_string()),
    )?;
    for (name, points) in [
        (
            "prefixRetainedSourcePoints",
            stats.refinement_work.prefix_retained_source_points,
        ),
        (
            "prefixAddedSourcePoints",
            stats.refinement_work.prefix_added_source_points,
        ),
    ] {
        js_sys::Reflect::set(&work, &name.into(), &JsValue::from_str(&points.to_string()))?;
    }
    js_sys::Reflect::set(&value, &"refinementWork".into(), &work)?;
    Ok(value.into())
}

pub(super) fn add_runtime_gpu_stats(
    value: &JsValue,
    renderer: Option<&SurfaceRenderer<'_>>,
) -> Result<(), JsValue> {
    let Some(renderer) = renderer else {
        return Ok(());
    };
    js_sys::Reflect::set(
        value,
        &"renderedVoxels".into(),
        &JsValue::from_f64(renderer.visible_instance_count() as f64),
    )?;
    js_sys::Reflect::set(
        value,
        &"hzbActive".into(),
        &JsValue::from_bool(renderer.hzb_active()),
    )?;
    Ok(())
}

fn per_update(count: u64, update_calls: u64) -> f64 {
    if update_calls == 0 {
        0.0
    } else {
        count as f64 / update_calls as f64
    }
}
