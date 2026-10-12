// Dedicated-worker integration probe; requires rebuilt pkg assets and browser WebGPU.
import init, { WasmVoxelViewer } from "../../../pkg/viewer_wasm.js";

function assert(condition, message) {
    if (!condition) throw new Error(message);
}

self.onmessage = async () => {
    let viewer;
    try {
        await init();
        viewer = await WasmVoxelViewer.createOffscreen(new OffscreenCanvas(640, 480), 0.6);
        const coordinates = await fetch(new URL("../sample.oct", import.meta.url)).then(r => r.blob());
        const attributes = await fetch(new URL("../sample.octattrs", import.meta.url)).then(r => r.blob());
        const finish = (pixels = 4) => {
            for (let attempt = 0; attempt < 20; attempt += 1) {
                const stats = viewer.updateLod(pixels);
                if (stats.coverageComplete && stats.pendingBlockIds.length === 0
                    && stats.lodStagePixels <= pixels) return stats;
            }
            throw new Error("sample did not complete refinement");
        };
        viewer.openFormatDataset(coordinates, 256 * 1024 * 1024, attributes);
        viewer.fitScene();
        const loaded = finish();
        // Both tiny fixtures fit in their metadata-open cache fills, excluded from LOD accounting.
        assert(loaded.refinementWork.rangeReadCalls === 0
            && loaded.refinementWork.rangeReadBytes === "0", "metadata reads leaked into block accounting");
        const repeated = finish();
        const reads = repeated.refinementWork;
        assert(Number.isFinite(reads.rangeReadMs) && reads.rangeReadMs >= 0
            && Number.isFinite(reads.decodeAndOtherCpuMs) && reads.decodeAndOtherCpuMs >= 0,
            "invalid read/decode accounting");
        assert(reads.rangeReadCalls === loaded.refinementWork.rangeReadCalls
            && reads.rangeReadBytes === loaded.refinementWork.rangeReadBytes,
            "identical scene performed additional file reads");
        assert(typeof reads.rangeReadBytes === "string", "read byte count lost integer representation");
        assert(repeated.refinementWork.sceneBuilds === loaded.refinementWork.sceneBuilds,
            "identical scene was rebuilt");
        assert(repeated.refinementWork.reusedSceneUpdates === loaded.refinementWork.reusedSceneUpdates + 1,
            "scene reuse was not counted");
        assert(repeated.renderedVoxels === loaded.renderedVoxels, "scene changed on reuse");

        viewer.zoom(0.1);
        const moved = finish();
        assert(moved.refinementWork.sceneBuilds > repeated.refinementWork.sceneBuilds,
            "camera change did not rebuild");
        viewer.resize(800, 600);
        const resized = finish();
        assert(resized.refinementWork.sceneBuilds > moved.refinementWork.sceneBuilds,
            "viewport change did not rebuild");
        const finer = finish(2);
        assert(finer.refinementWork.sceneBuilds > resized.refinementWork.sceneBuilds,
            "voxel stage change did not rebuild");

        // Visible cache entries are protected: a failed replacement must retain the scene.
        let rejected = false;
        try {
            viewer.insertLodBlock("1", new Float32Array([0, 0, 0]), new Uint8Array([255, 0, 0]));
        } catch (error) {
            rejected = String(error).includes("ProtectedEntries");
        }
        assert(rejected, "protected visible payload was replaced");
        const retained = finish(2);
        assert(retained.refinementWork.sceneBuilds === finer.refinementWork.sceneBuilds
            && retained.renderedVoxels === finer.renderedVoxels, "failed replacement changed the scene");
        // Attribute changes release the old cut. The same block ID can now hold new data.
        viewer.setColorMode("intensity");
        viewer.insertLodBlock("1", new Float32Array([0, 0, 0]), new Uint8Array([255, 0, 0]));
        const replaced = finish(2);
        assert(replaced.renderedVoxels === 1, "stale scene survived cache reset and new payload");
        viewer.setColorMode("rgb");
        const recolored = finish(2);
        assert(recolored.refinementWork.loadedBlocks === 1 && recolored.renderedVoxels > 1,
            "attribute cache reset did not reload the original payload");
        viewer.openFormatDataset(coordinates, 256 * 1024 * 1024);
        viewer.fitScene();
        const reopened = finish();
        assert(reopened.refinementWork.loadedBlocks === 1 && reopened.refinementWork.sceneBuilds === 3,
            "dataset reopen retained old accounting or scene");
        assert(reopened.refinementWork.rangeReadCalls === 0
            && reopened.refinementWork.rangeReadBytes === "0", "reopen retained range accounting");
        self.postMessage({ type: "probeResult", passed: true,
            checks: ["identical view", "camera", "viewport", "stage", "payload", "attributes", "reopen"] });
    } catch (error) {
        self.postMessage({ type: "probeResult", passed: false, error: String(error) });
    } finally {
        viewer?.free();
    }
};
