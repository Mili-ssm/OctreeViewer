// Playwright browser regression for the host's progress-only updates. No GPU dependency.
// The deterministic Worker exercises real host DOM/export code; renderer behavior is separate.
async (page) => {
    const results = [];
    for (const viewport of [{ width: 1280, height: 900 }, { width: 390, height: 844 }]) {
        const probe = await page.context().newPage();
        try {
            const errors = [];
            probe.on("pageerror", error => errors.push(String(error)));
            probe.on("console", message => { if (message.type() === "error") errors.push(message.text()); });
            // Routing disables the context's HTTP cache, including imported modules.
            await probe.route("**/*", route => route.continue());
            await probe.setViewportSize(viewport);
            await probe.addInitScript(() => {
                window.Worker = class extends EventTarget {
                    constructor() { super(); window.__hostWorker = this; }
                    postMessage(message) {
                        if (message.type === "init") queueMicrotask(() => {
                            this.dispatchEvent(new MessageEvent("message", { data: { type: "ready" } }));
                        });
                        if (message.type === "openFile") window.__openMessage = message;
                    }
                    terminate() {}
                };
                const original = URL.createObjectURL.bind(URL);
                URL.createObjectURL = blob => {
                    if (blob.type === "application/json") window.__exportText = blob.text();
                    return original(blob);
                };
                HTMLAnchorElement.prototype.click = function () {};
            });
            await probe.goto("http://127.0.0.1:8767/examples/web-host/index.html");
            await probe.locator("#dataset-file").setInputFiles({
                name: "fixture.oct", mimeType: "application/octet-stream", buffer: Buffer.from([0]),
            });
            await probe.waitForFunction(() => window.__openMessage);
            const result = await probe.evaluate(async () => {
                const lod = {
                    visiblePoints: "100", coverageComplete: true, uncoveredNodes: 0,
                    pendingBlockIds: [], lodStagePixels: 4, targetVoxelPixels: 4,
                    visibleBlocks: 1, lodError: null, minVoxelEdgePixels: 2, maxVoxelEdgePixels: 4,
                    renderedVoxels: 10, voxelBudget: 100, maxVisibleLevel: 2,
                    cacheBytes: "1000", cacheBudgetBytes: "2000",
                    cacheActivity: { evictions: "0", rejectedOversize: "0", rejectedProtected: "0" },
                };
                const gpu = {
                    visibleInstances: 10, visibleBlocks: 1, uploadedBytes: "400",
                    requestedResources: { totalBytes: "900" },
                };
                const emit = data => window.__hostWorker.dispatchEvent(new MessageEvent("message", { data }));
                emit({
                    type: "stats", lod: { ...lod, pendingBlockIds: [1, 2, 3] },
                    gpu: { ...gpu, uploadedBytes: "200", requestedResources: { totalBytes: "500" } },
                    viewer: { point_count: 100 }, frameRenderMs: null,
                });
                // No periodic stats or frame reply follows this terminal progress message.
                emit({
                    type: "lodProgress", lod, gpu, viewer: { point_count: 100 }, frameRenderMs: null,
                    firstCoarseMs: 1, targetLodMs: 2,
                    refinement: {
                        id: window.__openMessage.refinementId, trigger: "dataset", colorMode: "rgb",
                        firstFrameMs: 1, targetLodMs: 2,
                    },
                });
                document.querySelector("#download-benchmark").dispatchEvent(new Event("click"));
                const exported = JSON.parse(await window.__exportText);
                const stats = document.querySelector("#stats").textContent;
                if (!stats.includes("pending blocks: 0") || !stats.includes("uploaded instance bytes: 400")
                    || exported.environment.gpuStats.requestedResources.totalBytes !== "900"
                    || exported.scene.pendingBlocks !== 0) throw new Error("Progress statistics are stale");
                if (exported.steadyState.frameReplies !== 0) throw new Error("Progress was counted as FPS");
                return {
                    pending: exported.scene.pendingBlocks,
                    requestedResourceBytes: exported.environment.gpuStats.requestedResources.totalBytes,
                    frameReplies: exported.steadyState.frameReplies,
                    stats,
                };
            });
            if (errors.length) throw new Error(errors.join("\n"));
            results.push({ viewport, result, errors });
        } finally {
            await probe.close();
        }
    }
    return { coverage: "real host DOM/export with deterministic Worker; no GPU rendering", results };
}
