// Dedicated-worker regression: a resident coarse branch does not cap a fine sibling globally.
import init, { WasmVoxelViewer } from "../../../pkg/viewer_wasm.js";

function assert(condition, message) {
    if (!condition) throw new Error(message);
}

self.onmessage = async () => {
    let viewer;
    try {
        await init();
        viewer = await WasmVoxelViewer.createOffscreen(new OffscreenCanvas(640, 480), 0.6);
        viewer.openLodDataset({
            coordinateOrigin: [0, 0, 0], semantics: "replacement", cacheBytes: 1024 * 1024,
            sourcePointCount: "4", nodes: [
                { id: "0", parent: null, level: 0, min: [0, 0, 0], max: [16, 16, 16], pointCount: "0", geometricError: 16, blocks: [] },
                { id: "1", parent: "0", level: 1, min: [0, 0, 0], max: [8, 8, 8], pointCount: "1", geometricError: 8, blocks: ["1"] },
                { id: "2", parent: "0", level: 1, min: [8, 0, 0], max: [16, 8, 8], pointCount: "2", geometricError: 0, blocks: ["2"] },
                { id: "3", parent: "1", level: 2, min: [0, 0, 0], max: [4, 4, 4], pointCount: "2", geometricError: 0, blocks: ["3"] },
            ],
        });
        const insert = (id, points) => viewer.insertLodBlock(id,
            new Float32Array(points.flat()), new Uint8Array(points.flatMap(() => [20, 30, 40])));
        insert("1", [[0.5, 0.5, 0.5]]);
        insert("2", [[8.125, 0.125, 0.125], [8.375, 0.125, 0.125]]);
        viewer.fitScene();
        // The first pending stage remains at 16 px. Put the sibling close enough to
        // require cells below 1 world unit even at that stage, with both branches visible.
        viewer.setCameraPose(new Float64Array([20, 0.25, 0.25]), new Float64Array([8, 0.25, 0.25]));
        const pending = viewer.updateLod(0.5);
        assert(pending.coverageComplete && pending.pendingBlockIds.includes("3"), "coarse fallback was not retained");
        assert(pending.renderedVoxels === 3 && pending.visiblePoints === "3", "fine sibling was capped by coarse branch");
        assert(viewer.renderFrame() === "presented", "mixed cut did not render");
        viewer.resize(641, 480);
        const camera = viewer.updateLod(0.5);
        assert(camera.renderedVoxels === 3 && camera.coverageComplete, "camera rebuild lost regional detail");
        insert("3", [[0.125, 0.125, 0.125], [0.375, 0.125, 0.125]]);
        // Reconsider the now fully resident target instead of retaining the pending stage.
        viewer.orbit(0.001, 0);
        const refined = viewer.updateLod(0.5);
        assert(refined.renderedVoxels === 4 && refined.visiblePoints === "4"
            && refined.pendingBlockIds.length === 0 && refined.coverageComplete,
            "resident child did not replace its parent");
        assert(viewer.renderFrame() === "presented", "refined cut did not render");
        self.postMessage({ type: "probeResult", passed: true,
            mixedVoxels: pending.renderedVoxels, refinedVoxels: refined.renderedVoxels,
            checks: ["resident parent fallback", "independent sibling detail", "resize rebuild", "child replacement"] });
    } catch (error) {
        self.postMessage({ type: "probeResult", passed: false, error: String(error) });
    } finally {
        viewer?.free();
    }
};
