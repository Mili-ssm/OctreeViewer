import init, { WasmVoxelViewer } from "../../pkg/viewer_wasm.js";

let viewer = null;
let targetVoxelPixels = 4;
let renderedFrames = 0;
let datasetOpenedAt = 0;
let targetLodStartedAt = 0;
let firstCoarseMs = null;
let targetLodMs = null;
let lastLodProgressKey = "";
let refinementContext = { id: 0, trigger: "dataset", colorMode: "rgb" };
let currentMetadata = null;
let sceneGeneration = 0;
const FRAME_SAMPLE_CAPACITY = 4096;
let frameRenderSamples = new Float64Array(FRAME_SAMPLE_CAPACITY);
let frameRenderSampleCount = 0;
let nextFrameRenderSample = 0;
const coalescedCommands = new Set(["resize", "refine", "depthShading", "fit", "birdView"]);

function samplePointCloud() {
    const positions = [];
    const colors = [];

    for (let x = -8; x <= 8; x += 1) {
        for (let z = -8; z <= 8; z += 1) {
            const height = Math.sin(x * 0.45) * Math.cos(z * 0.35) * 3.0;

            for (let layer = 0; layer < 4; layer += 1) {
                positions.push(x, z, height + layer * 0.35);
                colors.push(
                    Math.round(120 + (x + 8) * 7),
                    Math.round(90 + (z + 8) * 7),
                    Math.round(180 + layer * 18),
                );
            }
        }
    }

    return {
        positions: new Float32Array(positions),
        colors: new Uint8Array(colors),
    };
}

function resetFrameRenderSamples() {
    frameRenderSamples = new Float64Array(FRAME_SAMPLE_CAPACITY);
    frameRenderSampleCount = 0;
    nextFrameRenderSample = 0;
}

function beginRefinement(trigger, id, colorMode, startedAt = performance.now()) {
    targetLodStartedAt = startedAt;
    firstCoarseMs = null;
    targetLodMs = null;
    lastLodProgressKey = "";
    resetFrameRenderSamples();
    refinementContext = { id, trigger, colorMode };
}

function recordFrameRenderSample(durationMs) {
    frameRenderSamples[nextFrameRenderSample] = durationMs;
    nextFrameRenderSample = (nextFrameRenderSample + 1) % FRAME_SAMPLE_CAPACITY;
    frameRenderSampleCount = Math.min(frameRenderSampleCount + 1, FRAME_SAMPLE_CAPACITY);
}

function frameRenderPercentiles() {
    if (frameRenderSampleCount === 0) {
        return { sampleCount: 0, p50Ms: null, p95Ms: null, p99Ms: null };
    }
    const ordered = Array.from({ length: frameRenderSampleCount }, (_, index) => {
        const oldest = (nextFrameRenderSample - frameRenderSampleCount + FRAME_SAMPLE_CAPACITY)
            % FRAME_SAMPLE_CAPACITY;
        return frameRenderSamples[(oldest + index) % FRAME_SAMPLE_CAPACITY];
    }).sort((left, right) => left - right);
    const percentile = (fraction) => ordered[Math.ceil(ordered.length * fraction) - 1];
    return {
        sampleCount: frameRenderSampleCount,
        p50Ms: percentile(0.50),
        p95Ms: percentile(0.95),
        p99Ms: percentile(0.99),
    };
}

function postStats(frameStatus = null) {
    self.postMessage({
        type: "stats",
        frameStatus,
        frameRenderMs: frameRenderPercentiles(),
        gpu: viewer.gpuStats(),
        lod: viewer.lodStats(),
        viewer: viewer.stats(),
    });
}

function pickFramebufferCoordinates(cssX, cssY, cssWidth, cssHeight) {
    if (![cssX, cssY, cssWidth, cssHeight].every(Number.isFinite)
        || cssWidth <= 0 || cssHeight <= 0) return null;
    const [width, height] = viewer.viewportSize();
    return [cssX * width / cssWidth, cssY * height / cssHeight];
}

function postLodProgress(lod, afterRenderFrame = false) {
    if (!lod) return;
    const now = performance.now();
    const hasVisiblePoints = Number(lod.visiblePoints) > 0;
    if (afterRenderFrame && firstCoarseMs === null && hasVisiblePoints) {
        firstCoarseMs = now - targetLodStartedAt;
    }
    const targetReached = lod.coverageComplete
        && lod.pendingBlockIds.length === 0
        && lod.lodStagePixels <= lod.targetVoxelPixels;
    if (afterRenderFrame && targetLodMs === null && targetReached) {
        targetLodMs = now - targetLodStartedAt;
    }
    const key = [
        lod.lodStagePixels,
        lod.pendingBlockIds.length,
        lod.visibleBlocks,
        lod.coverageComplete,
        targetReached,
        lod.lodError,
        firstCoarseMs,
        targetLodMs,
    ].join(":");
    if (key === lastLodProgressKey) return;
    lastLodProgressKey = key;
    self.postMessage({
        type: "lodProgress",
        lod,
        gpu: viewer.gpuStats(),
        viewer: viewer.stats(),
        frameRenderMs: frameRenderPercentiles(),
        firstCoarseMs,
        targetLodMs,
        elapsedMs: now - datasetOpenedAt,
        refinement: {
            ...refinementContext,
            firstFrameMs: firstCoarseMs,
            targetLodMs,
        },
    });
}

async function handleMessage({ data }) {
    try {
        switch (data.type) {
            case "init": {
                await init();
                viewer = await WasmVoxelViewer.createOffscreen(data.canvas, 0.6);
                viewer.resize(data.width, data.height);
                viewer.setColorMode(data.colorMode);
                viewer.setDepthShading(data.depthShading);
                const sample = samplePointCloud();
                viewer.replacePoints(sample.positions, sample.colors);
                viewer.fitScene();
                self.postMessage({ type: "ready" });
                break;
            }
            case "api": {
                let value;
                switch (data.operation) {
                    case "load": {
                        sceneGeneration += 1;
                        targetVoxelPixels = data.targetVoxelPixels;
                        datasetOpenedAt = performance.now();
                        beginRefinement("dataset", data.refinementId ?? 0, "rgb", datasetOpenedAt);
                        const opened = viewer.openFormatDataset(data.file, data.cacheBytes, data.attributes);
                        viewer.fitScene();
                        const lod = viewer.updateLod(targetVoxelPixels);
                        currentMetadata = { ...opened, name: data.name ?? data.file.name, bytes: data.file.size };
                        value = currentMetadata;
                        self.postMessage({ type: "dataset", opened, lod, name: data.name ?? data.file.name });
                        postLodProgress(lod);
                        break;
                    }
                    case "fit":
                        sceneGeneration += 1;
                        value = viewer.fitScene();
                        break;
                    case "renderOnce":
                        value = viewer.renderFrame();
                        postLodProgress(viewer.lodStats(), true);
                        break;
                    case "getCamera":
                        value = viewer.cameraPose();
                        break;
                    case "setCamera":
                        sceneGeneration += 1;
                        value = viewer.setCameraPose(
                            new Float64Array(data.position), new Float64Array(data.target),
                        );
                        break;
                    case "setOrbitTarget":
                        sceneGeneration += 1;
                        value = viewer.setOrbitTarget(new Float64Array(data.target));
                        break;
                    case "getMetadata":
                        value = currentMetadata;
                        break;
                    case "getStats":
                        value = { lod: viewer.lodStats(), gpu: viewer.gpuStats(), viewer: viewer.stats() };
                        break;
                    case "moveCamera":
                        sceneGeneration += 1;
                        value = viewer.moveCamera(data.x, data.y, data.z);
                        break;
                    case "birdView":
                        sceneGeneration += 1;
                        viewer.setBirdView();
                        value = true;
                        break;
                    case "pick": {
                        const pixel = pickFramebufferCoordinates(
                            data.cssX, data.cssY, data.cssWidth, data.cssHeight,
                        );
                        const hit = pixel ? await viewer.pick(...pixel) : null;
                        const pickedGeneration = sceneGeneration;
                        if (hit && data.recenter) {
                            viewer.setOrbitTarget(hit.voxelCenter);
                            sceneGeneration += 1;
                            viewer.renderFrame();
                        }
                        value = { hit, sceneGeneration: pickedGeneration };
                        break;
                    }
                    default:
                        throw new Error(`unknown viewer API operation: ${data.operation}`);
                }
                self.postMessage({ type: "apiResult", requestId: data.requestId, value });
                break;
            }
            case "resize":
                sceneGeneration += 1;
                viewer.resize(data.width, data.height);
                break;
            case "refine": {
                const startedAt = performance.now();
                viewer.setColorMode(data.colorMode);
                sceneGeneration += 1;
                targetVoxelPixels = data.targetVoxelPixels;
                beginRefinement(data.trigger, data.refinementId ?? 0, data.colorMode, startedAt);
                if (viewer.lodStats()) {
                    const lod = viewer.updateLod(targetVoxelPixels);
                    self.postMessage({ type: "lod", lod });
                    postLodProgress(lod);
                }
                break;
            }
            case "depthShading":
                viewer.setDepthShading(data.value);
                break;
            case "openFile": {
                sceneGeneration += 1;
                targetVoxelPixels = data.targetVoxelPixels;
                datasetOpenedAt = performance.now();
                beginRefinement("dataset", data.refinementId ?? 0, "rgb", datasetOpenedAt);
                const name = data.name ?? data.file.name;
                self.postMessage({ type: "status", message: `Opening ${name}…` });
                const opened = viewer.openFormatDataset(data.file, data.cacheBytes, data.attributes);
                self.postMessage({ type: "datasetOpened", opened, name });
                viewer.fitScene();
                const lod = viewer.updateLod(targetVoxelPixels);
                self.postMessage({ type: "dataset", opened, lod, name });
                postLodProgress(lod);
                break;
            }
            case "fit":
                sceneGeneration += 1;
                viewer.fitScene();
                break;
            case "birdView":
                sceneGeneration += 1;
                viewer.setBirdView();
                break;
            case "pick": {
                const pixel = pickFramebufferCoordinates(
                    data.cssX, data.cssY, data.cssWidth, data.cssHeight,
                );
                const hit = pixel ? await viewer.pick(...pixel) : null;
                const pickedGeneration = sceneGeneration;
                if (hit && data.recenter) {
                    viewer.setOrbitTarget(hit.voxelCenter);
                    sceneGeneration += 1;
                }
                self.postMessage({ type: "pick", hit, sceneGeneration: pickedGeneration });
                break;
            }
            case "frame": {
                const camera = data.camera;
                if (Object.values(camera).some((value) => value !== 0)) sceneGeneration += 1;
                if (camera.orbitX !== 0 || camera.orbitY !== 0) {
                    viewer.orbit(camera.orbitX, camera.orbitY);
                }
                if (camera.panX !== 0 || camera.panY !== 0) {
                    viewer.pan(camera.panX, camera.panY);
                }
                if (camera.zoom !== 0) viewer.zoom(camera.zoom);
                if (camera.moveX !== 0 || camera.moveY !== 0 || camera.moveZ !== 0) {
                    viewer.moveCamera(camera.moveX, camera.moveY, camera.moveZ);
                }
                const startedAt = performance.now();
                const frameStatus = viewer.renderFrame();
                const renderCallMs = performance.now() - startedAt;
                recordFrameRenderSample(renderCallMs);
                postLodProgress(viewer.lodStats(), true);
                renderedFrames += 1;
                if (renderedFrames % 30 === 0) {
                    postStats({ status: frameStatus, renderCallMs });
                } else {
                    self.postMessage({ type: "frame", frameStatus, renderCallMs });
                }
                break;
            }
            default:
                throw new Error(`unknown worker message: ${data.type}`);
        }
    } catch (error) {
        self.postMessage({ type: "error", operation: data.type, requestId: data.requestId, message: String(error) });
    } finally {
        if (coalescedCommands.has(data.type)) {
            self.postMessage({ type: "commandAck", command: data.type });
        }
    }
}

let messageQueue = Promise.resolve();
self.addEventListener("message", (event) => {
    messageQueue = messageQueue.then(() => handleMessage(event), () => handleMessage(event));
});
