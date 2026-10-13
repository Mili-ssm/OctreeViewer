// Real WebGPU regression: resident fine construction fails, coarse fallback remains visible.
import init, { WasmVoxelViewer } from "../../../pkg/viewer_wasm.js";

function assert(condition, message) {
    if (!condition) throw new Error(message);
}

self.onmessage = async () => {
    let bounded;
    let reference;
    try {
        await init();
        for (const [voxels, bytes] of [[0, 64 * 1024 * 1024], [1, 0]]) {
            let rejected = false;
            try {
                const viewer = await WasmVoxelViewer.createOffscreenWithVoxelLimits(
                    new OffscreenCanvas(320, 240), 0.6, voxels, bytes,
                );
                viewer.free();
            } catch (error) {
                rejected = String(error).includes("limits must be positive");
            }
            assert(rejected, "zero construction limit was accepted");
        }
        const descriptor = {
            coordinateOrigin: [0, 0, 0], semantics: "replacement", cacheBytes: 1024 * 1024,
            sourcePointCount: "8", nodes: [{ id: "0", parent: null, level: 0,
                min: [0, 0, 0], max: [1, 1, 1], pointCount: "8", geometricError: 0, blocks: ["7"] }],
        };
        const positions = new Float32Array(Array.from({ length: 8 }, (_, i) => [0.51 + i * 0.004, 0.51, 0.51]).flat());
        const colors = new Uint8Array(Array.from({ length: 8 }, () => [10, 20, 30]).flat());
        const open = viewer => {
            viewer.openLodDataset(descriptor);
            viewer.insertLodBlock("7", positions, colors);
            viewer.fitScene();
        };
        reference = await WasmVoxelViewer.createOffscreen(new OffscreenCanvas(320, 240), 0.6);
        open(reference);
        const fine = reference.updateLod(0.5);
        assert(fine.lodStagePixels === 0.5 && fine.renderedVoxels > 1,
            "fixture does not exceed the constrained fine voxel budget");
        assert(reference.renderFrame() === "presented", "reference fine frame was not presented");

        bounded = await WasmVoxelViewer.createOffscreenWithVoxelLimits(
            new OffscreenCanvas(320, 240), 0.6, 1, 64 * 1024 * 1024,
        );
        open(bounded);
        const coarse = bounded.updateLod(16);
        assert(coarse.renderedVoxels === 1 && coarse.lodStagePixels === 16,
            "coarse fixture does not fit the configured budget");
        assert(bounded.renderFrame() === "presented", "coarse frame was not presented");
        // Invalidate the coarse build context so fallback must build, rather than reuse it.
        bounded.resize(321, 240);
        const fallback = bounded.updateLod(0.5);
        assert(fallback.lodStagePixels === 16 && fallback.targetVoxelPixels === 0.5,
            "resident fine failure did not retry the coarse stage");
        assert(fallback.refinementWork.sceneBuilds === coarse.refinementWork.sceneBuilds + 2,
            "expected a failed fine build followed by a successful coarse build");
        assert(fallback.coverageComplete && fallback.pendingBlockIds.length === 0
            && fallback.visiblePoints === "8" && fallback.renderedVoxels === 1,
            "fallback lost the resident source or visible scene");
        assert(fallback.voxelBudget === 1 && fallback.voxelBuildBudgetBytes === String(64 * 1024 * 1024),
            "construction limits were not retained");
        let error;
        for (let attempt = 0; attempt < 10; attempt += 1) {
            try { bounded.updateLod(0.5); }
            catch (failure) { error = String(failure); break; }
        }
        assert(error?.includes("1-voxel instance limit"), "progressive refinement did not reach the real construction error");
        assert(bounded.renderFrame() === "presented", "failed refinement prevented rendering the previous cut");
        const retained = bounded.lodStats();
        assert(retained.renderedVoxels === 1 && retained.coverageComplete
            && retained.visiblePoints === "8", "failed refinement changed the previous visible cut");
        const recovered = bounded.updateLod(16);
        assert(recovered.renderedVoxels === 1 && recovered.lodError === null,
            "coarse detail did not recover after the failure");
        assert(recovered.refinementWork.loadedBlocks === 0 && recovered.cacheActivity.insertions === "1",
            "resident fallback unexpectedly loaded or reinserted payloads");
        self.postMessage({ type: "probeResult", passed: true, fineVoxels: fine.renderedVoxels,
            coarseVoxels: coarse.renderedVoxels, fallbackBuilds: fallback.refinementWork.sceneBuilds - coarse.refinementWork.sceneBuilds,
            constructionError: error, checks: ["zero limits", "fine reference", "real fine failure", "coarse fallback", "retained scene", "recovery", "no payload loads"] });
    } catch (error) {
        self.postMessage({ type: "probeResult", passed: false, error: String(error) });
    } finally {
        bounded?.free();
        reference?.free();
    }
};
