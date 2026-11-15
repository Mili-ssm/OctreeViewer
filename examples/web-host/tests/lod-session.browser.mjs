// Dedicated-worker regression for dataset ownership and atomic replacement.
// Run with fresh pkg bindings in a browser with WebGPU/OffscreenCanvas support.
import init, { WasmVoxelViewer } from "../../../pkg/viewer_wasm.js";

function assert(condition, message) {
    if (!condition) throw new Error(message);
}

function rejected(operation, message) {
    let failed = false;
    try { operation(); } catch { failed = true; }
    assert(failed, message);
}

self.onmessage = async ({ data = {} }) => {
    let viewer;
    try {
        await init();
        const coordinates = await fetch(new URL("../sample-v5.oct", import.meta.url)).then(r => r.blob());
        const attributes = await fetch(new URL("../sample-v5.octattrs", import.meta.url)).then(r => r.blob());
        const descriptor = {
            coordinateOrigin: [0, 0, 0], semantics: "replacement", cacheBytes: 1024 * 1024,
            sourcePointCount: "1", nodes: [{ id: "0", parent: null, level: 0,
                min: [0, 0, 0], max: [1, 1, 1], pointCount: "1", geometricError: 0, blocks: ["7"] }],
        };
        viewer = await WasmVoxelViewer.createOffscreen(new OffscreenCanvas(640, 480), 0.6);
        const insert = () => viewer.insertLodBlock("7", new Float32Array([0.5, 0.5, 0.5]), new Uint8Array([10, 20, 30]));
        const finish = async () => {
            for (let attempt = 0; attempt < 1000; attempt += 1) {
                const stats = viewer.updateLod(4);
                if (stats.coverageComplete && stats.pendingBlockIds.length === 0 && stats.lodStagePixels <= 4) {
                    assert(viewer.renderFrame() === "presented", "frame was not presented");
                    return stats;
                }
                await new Promise(resolve => setTimeout(resolve, 0));
            }
            throw new Error("session did not finish refinement");
        };
        assert(viewer.lodStats() === null, "unexpected initial LOD session");
        rejected(insert, "manual block accepted without a dataset");

        viewer.openLodDataset(descriptor);
        insert();
        viewer.fitScene();
        const external = await finish();
        assert(external.visiblePoints === "1", "external block was not retained");
        rejected(() => viewer.setColorMode("attribute:0:0"), "external session retained a reader");

        const opened = viewer.openFormatDataset(coordinates, 1024 * 1024, attributes);
        assert(opened.hasAttributes, "paired session lost companion");
        viewer.fitScene();
        const paired = await finish();
        viewer.setColorMode("attribute:0:0");
        const colored = await finish();
        assert(colored.renderedVoxels === paired.renderedVoxels, "attribute selection changed geometry");
        rejected(() => viewer.openFormatDataset(new Blob([new Uint8Array([0])]), 1024 * 1024),
            "malformed coordinates accepted");
        rejected(() => viewer.openFormatDataset(coordinates, 1024 * 1024, data.incompatibleAttributes ?? coordinates),
            "incompatible companion accepted");
        rejected(() => viewer.openLodDataset({ ...descriptor, semantics: "invalid" }),
            "invalid external descriptor accepted");
        // Force failures after candidate construction, while building each JS response.
        for (const [key, operation] of [
            ["hasAttributes", () => viewer.openFormatDataset(coordinates, 1024 * 1024, attributes)],
            ["status", () => viewer.openLodDataset(descriptor)],
        ]) {
            const originalSet = Reflect.set;
            Reflect.set = (target, property, value) => {
                if (property === key) throw new Error("injected response failure");
                return originalSet(target, property, value);
            };
            try { rejected(operation, "response failure was ignored"); }
            finally { Reflect.set = originalSet; }
            assert(viewer.lodStats() !== null, "response failure cleared the active session");
        }
        const retained = await finish();
        assert(retained.renderedVoxels === colored.renderedVoxels
            && retained.refinementWork.sceneBuilds === colored.refinementWork.sceneBuilds,
            "failed open replaced the active session");
        viewer.setColorMode("classification");
        await finish();

        viewer.openLodDataset(descriptor);
        insert();
        viewer.fitScene();
        assert((await finish()).visiblePoints === "1", "file-to-external replacement kept old blocks");
        rejected(() => viewer.setColorMode("attribute:0:0"), "file-to-external replacement kept the reader");

        const gray = viewer.openFormatDataset(coordinates, 1024 * 1024);
        assert(!gray.hasAttributes, "coordinate-only open reported attributes");
        viewer.fitScene();
        assert((await finish()).renderedVoxels === paired.renderedVoxels, "coordinate-only geometry changed");
        rejected(() => viewer.setColorMode("attribute:0:0"), "attribute accepted without companion");
        viewer.clear();
        assert(viewer.lodStats() === null, "clear retained LOD stats");
        rejected(insert, "clear retained a dataset");
        viewer.replacePoints(new Float32Array([0, 0, 0]), new Uint8Array([255, 0, 0]));
        assert(viewer.lodStats() === null, "manual points restored an old LOD session");
        assert(viewer.renderFrame() === "presented", "manual frame was not presented");

        viewer.openFormatDataset(coordinates, 1024 * 1024, attributes);
        viewer.fitScene();
        const reopened = await finish();
        assert(reopened.renderedVoxels === paired.renderedVoxels, "valid reopen after clear failed");
        self.postMessage({ type: "probeResult", passed: true,
            checks: ["none", "external", "paired", "failed replacement", "response failure", "external replacement", "gray", "clear", "manual", "reopen"],
            incompatibleSidecarChecked: Boolean(data.incompatibleAttributes) });
    } catch (error) {
        self.postMessage({ type: "probeResult", passed: false, error: String(error) });
    } finally {
        viewer?.free();
    }
};
