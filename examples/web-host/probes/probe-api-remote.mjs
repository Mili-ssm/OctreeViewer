// Browser-only, bounded probe of `load({ url })` in the embeddable API (D4, issue #11). Run it from
// probe-api-remote.html; the procedure is in PROBES.md ("Remote source through the embeddable API").
import { lodTargetReached } from "../benchmark-metrics.mjs";
import { sidecarUrl } from "../http-range-source.mjs";
import { createViewer } from "../viewer-api.mjs";

const TIMEOUT_MS = 60_000;
const withTimeout = (promise, label) => Promise.race([
    promise,
    new Promise((_, reject) => setTimeout(() => reject(new Error(`${label} timed out after ${TIMEOUT_MS / 1000} s`)), TIMEOUT_MS)),
]);

/**
 * Opens `datasetUrl` as a remote `{ url }` source and waits for the LOD target (`lodTargetReached`).
 * `buildWorker`: use the build worker (`createViewer` option). `attributes`: "derived" opens the
 * `sidecarUrl(datasetUrl)` sidecar explicitly, "none" opens without one.
 * Resolves with `{ name, bytes, hasAttributes, metadata, reachedTarget }`, or `{ error }` when the load fails.
 */
export async function probeApiRemote(datasetUrl, { buildWorker = false, attributes = "none" } = {}) {
    const canvas = document.createElement("canvas");
    canvas.style.width = "800px";
    canvas.style.height = "600px";
    document.body.append(canvas);
    let viewer;
    try {
        viewer = createViewer(canvas, { buildWorker });
        await withTimeout(viewer.ready, "viewer.ready");
        const sidecar = attributes === "derived" ? { url: sidecarUrl(datasetUrl) } : undefined;
        try {
            await withTimeout(viewer.load({ url: datasetUrl }, { attributes: sidecar }), "load");
        } catch (error) {
            return { error: String(error?.message ?? error) };
        }
        const deadline = performance.now() + TIMEOUT_MS;
        let reachedTarget = false;
        while (performance.now() < deadline) {
            const stats = await withTimeout(viewer.getStats(), "getStats");
            if (stats.lod?.lodError) return { error: stats.lod.lodError };
            // The cells path (the default) reports no `coverageComplete`: reuse the benchmark definition.
            if (lodTargetReached(stats.lod)) {
                reachedTarget = true;
                break;
            }
            await new Promise((resolve) => setTimeout(resolve, 20));
        }
        const metadata = await withTimeout(viewer.getMetadata(), "getMetadata");
        const { name, bytes, hasAttributes, nodeCount, maxLevel, semantics, sourcePointCount } = metadata;
        return {
            name, bytes, hasAttributes, reachedTarget,
            metadata: { nodeCount, maxLevel, semantics, sourcePointCount: String(sourcePointCount) },
        };
    } finally {
        viewer?.dispose();
        canvas.remove();
    }
}
