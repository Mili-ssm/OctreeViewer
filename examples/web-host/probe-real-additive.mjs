// Browser-only, bounded real-data diagnostic. See docs/records/benchmarks/VOXEL_BUILD_BENCH.md.
import { createViewer } from "./viewer-api.mjs";

export async function probeRealAdditive(inputUrl = "/target/additive-real-v5.oct") {
    const canvas = document.createElement("canvas");
    canvas.style.width = "800px";
    canvas.style.height = "600px";
    document.body.append(canvas);
    let viewer;
    async function complete() {
        const deadline = performance.now() + 30_000;
        while (performance.now() < deadline) {
            const stats = await viewer.getStats();
            if (stats.lod?.lodError) throw new Error(stats.lod.lodError);
            if (stats.lod?.coverageComplete && stats.lod.pendingBlockIds.length === 0
                && stats.lod.lodStagePixels <= 4) return stats;
            await new Promise(resolve => setTimeout(resolve, 10));
        }
        throw new Error("Real Additive sample did not reach 4 px");
    }
    try {
        viewer = createViewer(canvas, { cells: false });
        await viewer.ready;
        const response = await fetch(inputUrl);
        if (!response.ok) throw new Error(`Fixture HTTP status: ${response.status}`);
        const metadata = await viewer.load(await response.blob());
        if (metadata.semantics !== "additive" || metadata.nodeCount !== 3
            || metadata.sourcePointCount !== "8884") {
            throw new Error("Expected the partial three-node USGS Additive fixture");
        }
        const initial = await complete();
        const beforeCamera = await viewer.getCamera();
        const translate = values => values.map((value, axis) => value + (axis === 0 ? 1000 : 0));
        await viewer.setCamera(translate(beforeCamera.position), translate(beforeCamera.target));
        const moved = await complete();
        const afterCamera = await viewer.getCamera();
        if (Math.abs(afterCamera.target[0] - beforeCamera.target[0] - 1000) > 1e-6) {
            throw new Error("Camera translation was not applied");
        }
        return { metadata, initial, moved, beforeCamera, afterCamera };
    } finally {
        viewer?.dispose();
        canvas.remove();
    }
}
