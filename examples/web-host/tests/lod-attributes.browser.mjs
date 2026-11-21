// Dedicated-worker integration probe; timings describe this probe, not host loading or FPS.
import init, { WasmVoxelViewer } from "../../../pkg/viewer_wasm.js";

function assert(condition, message) {
    if (!condition) throw new Error(message);
}

self.onmessage = async ({ data = {} }) => {
    let viewer;
    try {
        await init();
        const coordinates = data.coordinates
            ?? await fetch(new URL("../sample-small.oct", import.meta.url)).then(r => r.blob());
        const attributes = data.attributes
            ?? await fetch(new URL("../sample-small.octattrs", import.meta.url)).then(r => r.blob());
        viewer = await WasmVoxelViewer.createOffscreen(new OffscreenCanvas(1280, 720), 0.6);
        const opened = viewer.openFormatDataset(coordinates, 256 * 1024 * 1024, attributes);
        assert(opened.hasAttributes && opened.attributeSchema.length > 0, "attributes were not attached");
        viewer.fitScene();
        const finish = async () => {
            for (let attempt = 0; attempt < 10_000; attempt += 1) {
                viewer.updateLod(4);
                viewer.renderFrame();
                const stats = viewer.lodStats();
                if (stats.coverageComplete && stats.pendingBlockIds.length === 0
                    && stats.lodStagePixels <= 4) return stats;
                // Allow progress delivery between synchronous update calls.
                await new Promise(resolve => setTimeout(resolve, 0));
            }
            throw new Error("attribute refinement did not complete");
        };
        const reference = await finish();
        const modes = data.modes ?? ["attribute:0:0", "classification"];
        const results = [];
        for (const mode of modes) {
            const started = performance.now();
            viewer.setColorMode(mode);
            const setupMs = performance.now() - started;
            const stats = await finish();
            assert(stats.renderedVoxels === reference.renderedVoxels
                && stats.visiblePoints === reference.visiblePoints
                && stats.visibleBlocks === reference.visibleBlocks,
                `${mode}: selected column changed final geometry`);
            assert(stats.refinementWork.loadedBlocks > 0, `${mode}: attribute cache was not reloaded`);
            results.push({ mode, setupMs, probeReloadMs: performance.now() - started, stats });
        }
        let rejected = false;
        try {
            viewer.setColorMode("attribute:4294967295:0");
        } catch {
            rejected = true;
        }
        assert(rejected, "invalid column was accepted");
        const retained = await finish();
        assert(retained.renderedVoxels === reference.renderedVoxels, "invalid selection lost the scene");
        if (data.incompatibleAttributes) {
            rejected = false;
            try {
                viewer.openFormatDataset(coordinates, 256 * 1024 * 1024, data.incompatibleAttributes);
            } catch {
                rejected = true;
            }
            assert(rejected, "incompatible sidecar was accepted");
            const afterFailure = await finish();
            assert(afterFailure.renderedVoxels === reference.renderedVoxels
                && afterFailure.refinementWork.sceneBuilds === retained.refinementWork.sceneBuilds,
                "failed sidecar attachment changed the existing scene");
        }
        self.postMessage({ type: "probeResult", passed: true, schema: opened.attributeSchema,
            incompatibleSidecarChecked: Boolean(data.incompatibleAttributes), reference, results });
    } catch (error) {
        self.postMessage({ type: "probeResult", passed: false, error: String(error) });
    } finally {
        viewer?.free();
    }
};
