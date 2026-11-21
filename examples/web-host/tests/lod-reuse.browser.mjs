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
        const coordinates = await fetch(new URL("../sample-small.oct", import.meta.url)).then(r => r.blob());
        const attributes = await fetch(new URL("../sample-small.octattrs", import.meta.url)).then(r => r.blob());
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
        // Exercise HZB allocation and the first indirect draw on the 4,096-voxel fixture.
        // Chrome software WebGPU previously rejected mapped creation of the args buffer.
        assert(viewer.renderFrame() === "presented", "first sample frame was not presented");
        assert(viewer.renderFrame() === "presented", "sample indirect frame was not presented");
        // Both tiny fixtures fit in their metadata-open cache fills, excluded from LOD accounting.
        assert(loaded.refinementWork.rangeReadCalls === 0
            && loaded.refinementWork.rangeReadBytes === "0", "metadata reads leaked into block accounting");
        assert(loaded.cacheActivity.insertions === "1" && loaded.cacheActivity.evictions === "0",
            "invalid cache insertion accounting");
        assert(loaded.refinementWork.evictedPendingBlocks === 0, "unexpected target eviction");
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
            viewer.insertLodBlock("0", new Float32Array([0, 0, 0]), new Uint8Array([255, 0, 0]));
        } catch (error) {
            rejected = String(error).includes("ProtectedEntries");
        }
        assert(rejected, "protected visible payload was replaced");
        assert(viewer.lodStats().cacheActivity.rejectedProtected === "1",
            "failed insertion was hidden behind the last scene snapshot");
        const retained = finish(2);
        assert(retained.refinementWork.sceneBuilds === finer.refinementWork.sceneBuilds
            && retained.renderedVoxels === finer.renderedVoxels, "failed replacement changed the scene");
        // Attribute changes release the old cut. The same block ID can now hold new data.
        viewer.setColorMode("intensity");
        viewer.insertLodBlock("0", new Float32Array([0, 0, 0]), new Uint8Array([255, 0, 0]));
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
        assert(reopened.cacheActivity.insertions === "1"
            && reopened.cacheActivity.rejectedProtected === "0", "reopen retained cache activity");
        assert(reopened.refinementWork.rangeReadCalls === 0
            && reopened.refinementWork.rangeReadBytes === "0", "reopen retained range accounting");
        // Reach a fine stage while the second branch is outside the frustum, then reveal its
        // absent block. Each actual camera binding must preserve that published stage.
        viewer.openLodDataset({
            coordinateOrigin: [0, 0, 0], semantics: "replacement", cacheBytes: 1024 * 1024,
            sourcePointCount: "2", nodes: [
                { id: "0", parent: null, level: 0, min: [0, 0, 0], max: [100, 10, 10], pointCount: "0", geometricError: 100, blocks: [] },
                { id: "1", parent: "0", level: 1, min: [0, 0, 0], max: [10, 10, 10], pointCount: "1", geometricError: 0, blocks: ["1"] },
                { id: "2", parent: "0", level: 1, min: [90, 0, 0], max: [100, 10, 10], pointCount: "1", geometricError: 0, blocks: ["2"] },
            ],
        });
        viewer.insertLodBlock("1", new Float32Array([5, 5, 5]), new Uint8Array([20, 30, 40]));
        viewer.fitScene();
        viewer.setCameraPose(new Float64Array([5, -20, 5]), new Float64Array([5, 5, 5]));
        const initial = finish(4);
        assert(initial.lodStagePixels <= 8, "fixture did not reach a fine stage");
        viewer.setCameraPose(new Float64Array([50, -150, 5]), new Float64Array([50, 5, 5]));
        let stage = initial.lodStagePixels;
        for (const [name, move] of [
            ["pose", () => true],
            ["orbit", () => viewer.orbit(0.001, 0)],
            ["pan", () => viewer.pan(0.001, 0)],
            ["zoom", () => viewer.zoom(0.001)],
            ["resize", () => { viewer.resize(642, 480); return true; }],
        ]) {
            assert(move(), `${name} did not change the view`);
            const stats = viewer.updateLod(4);
            assert(stats.pendingBlockIds.includes("2"), `${name} lost the nonresident target`);
            assert(stats.lodStagePixels <= stage, `${name} raised stage ${stage} to ${stats.lodStagePixels}`);
            stage = stats.lodStagePixels;
        }
        const reset = viewer.updateLod(2);
        assert(reset.pendingBlockIds.includes("2") && reset.lodStagePixels === 16,
            "changing target did not restart the nonresident stage");
        self.postMessage({ type: "probeResult", passed: true,
            checks: ["identical view", "camera", "viewport", "stage", "payload", "attributes", "reopen", "pending camera stage"] });
    } catch (error) {
        self.postMessage({ type: "probeResult", passed: false, error: String(error) });
    } finally {
        viewer?.free();
    }
};
