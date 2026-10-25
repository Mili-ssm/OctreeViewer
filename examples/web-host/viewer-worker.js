import init, { WasmAsyncFormatReader, WasmAsyncReadBudget, WasmBlockRequestScheduler, WasmVoxelViewer } from "../../pkg/viewer_wasm.js";

let viewer = null;
let wasmExports = null;
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
let datasetGeneration = 0;
let asyncFormatReader = null;
const MAX_ASYNC_LOD_READS = 4;
const asyncReadTasks = new Map();
const failedAsyncBlocks = new Set();
let asyncLodScheduler = null;
let currentColorMode = "rgb";
let asyncHasAttributes = false;
let asyncAttributeIndices = new Map();
let asyncReadBudget = null;
const asyncReadCancelTokens = new Set();
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

function advanceSceneGeneration() {
    sceneGeneration += 1;
}

function cancelAllAsyncReads() {
    asyncLodScheduler?.cancelAll();
    for (const token of asyncReadCancelTokens) token.cancel();
    asyncReadCancelTokens.clear();
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

// Capacity of this worker's WASM instance, not live allocations or process RSS.
// Always re-read buffer: memory.grow can replace/detach its previous buffer.
function memoryStats() {
    return {
        wasmLinearMemoryBytes: wasmExports?.memory?.buffer.byteLength ?? null,
        workerJsHeapUsedBytes: null,
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
        memory: memoryStats(),
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
        memory: memoryStats(),
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

function scheduleAsyncLodRead(retryFailed = false) {
    if (!asyncFormatReader || !viewer || !asyncLodScheduler) return;
    const pending = typeof viewer.pendingAsyncLodBlocks === "function"
        ? viewer.pendingAsyncLodBlocks()
        : [viewer.nextAsyncLodBlock()].filter(blockId => blockId !== null);
    const cancelledTickets = asyncLodScheduler.updateBlocks(pending);
    for (const ticket of cancelledTickets) {
        for (const task of asyncReadTasks.values()) {
            if (task.ticket === String(ticket)) task.cancelToken.cancel();
        }
    }
    if (retryFailed) {
        for (const blockId of failedAsyncBlocks) {
            asyncLodScheduler.retry(blockId);
        }
        failedAsyncBlocks.clear();
    }

    const reader = asyncFormatReader;
    const generation = datasetGeneration;
    const ticketScheduler = asyncLodScheduler;
    for (const blockId of ticketScheduler.pendingBlocks()) {
        if (ticketScheduler.inFlightCount() >= MAX_ASYNC_LOD_READS) break;
        if (asyncReadTasks.has(String(blockId))) continue;

        const colorMode = currentColorMode;
        const selectedAttribute = colorMode.startsWith("attribute:")
            ? Number(colorMode.split(":")[1])
            : null;
        const requestedAttribute = selectedAttribute !== null
            ? selectedAttribute
            : colorMode === "intensity" ? asyncAttributeIndices.get("intensity")
                : colorMode === "classification" ? asyncAttributeIndices.get("classification") : null;
        const geometrySelection = colorMode === "rgb" && selectedAttribute === null
            ? "geometry" : "positions";
        const selection = requestedAttribute !== null && asyncHasAttributes
            ? "positionsAttribute" : geometrySelection;
        let request;
        try {
            request = reader.planLodRead(BigInt(blockId), selection, requestedAttribute);
        } catch (error) {
            ticketScheduler.fail(String(blockId));
            failedAsyncBlocks.add(String(blockId));
            self.postMessage({ type: "error", operation: "asyncLodRead", message: String(error) });
            continue;
        }
        // A null result means live reservations currently occupy the shared budget. Leave the
        // block queued without taking a scheduler slot; lower-priority admitted blocks may proceed.
        if (!request) continue;
        const scheduled = ticketScheduler.takeNextAdmitted([String(blockId)]);
        if (!scheduled) {
            request.free();
            break;
        }
        const ticket = String(scheduled.ticket);
        const cancelToken = request.cancelToken();
        asyncReadCancelTokens.add(cancelToken);
        asyncReadTasks.set(String(blockId), { generation, reader, request, cancelToken, ticket });

        void (async () => {
            let result = null;
            let payload = null;
            let ticketCompleted = false;
            try {
                result = await request.read();
                payload = result.value;
                await (messageQueue = messageQueue.then(() => {
                    if (generation !== datasetGeneration || reader !== asyncFormatReader
                        || ticketScheduler !== asyncLodScheduler) {
                        ticketScheduler.complete(ticket, String(blockId), false);
                        ticketCompleted = true;
                        return;
                    }
                    if (!ticketScheduler.isCurrent(ticket)) {
                        ticketScheduler.complete(ticket, String(blockId), false);
                        ticketCompleted = true;
                        return;
                    }
                    viewer.insertAsyncLodBlock(
                        blockId, payload.positions, payload.colors ?? null, payload.attribute ?? null,
                    );
                    ticketScheduler.complete(ticket, String(blockId), true);
                    ticketCompleted = true;
                    const lod = viewer.updateLod(targetVoxelPixels);
                    self.postMessage({ type: "lod", lod });
                    postLodProgress(lod);
                }).catch((error) => {
                    if (!ticketCompleted) {
                        const outcome = ticketScheduler.complete(ticket, String(blockId), false);
                        ticketCompleted = true;
                        if (outcome === "failed") failedAsyncBlocks.add(String(blockId));
                    }
                    if (generation === datasetGeneration) {
                        self.postMessage({ type: "error", operation: "asyncLodRead", message: String(error) });
                    }
                }));
            } catch (error) {
                await (messageQueue = messageQueue.then(() => {
                    if (!ticketCompleted) {
                        const outcome = ticketScheduler.complete(ticket, String(blockId), false);
                        ticketCompleted = true;
                        if (outcome === "failed") failedAsyncBlocks.add(String(blockId));
                    }
                    if (generation === datasetGeneration && outcome !== "discarded") {
                        self.postMessage({ type: "error", operation: "asyncLodRead", message: String(error) });
                    }
                }));
            } finally {
                payload = null;
                result?.release();
                result?.free();
                request.free();
                asyncReadCancelTokens.delete(cancelToken);
                asyncReadTasks.delete(String(blockId));
                scheduleAsyncLodRead();
            }
        })();
    }
}

function startAsyncDataset(data, requestId = null) {
    advanceSceneGeneration();
    cancelAllAsyncReads();
    datasetGeneration += 1;
    const generation = datasetGeneration;
    asyncLodScheduler = new WasmBlockRequestScheduler(MAX_ASYNC_LOD_READS);
    targetVoxelPixels = data.targetVoxelPixels;
    datasetOpenedAt = performance.now();
    currentColorMode = "rgb";
    asyncFormatReader = null;
    asyncHasAttributes = false;
    asyncAttributeIndices = new Map();
    failedAsyncBlocks.clear();
    beginRefinement("dataset", data.refinementId ?? 0, "rgb", datasetOpenedAt);
    const name = data.name ?? data.file.name;
    self.postMessage({ type: "status", message: `Opening ${name}…` });
    asyncReadBudget ??= new WasmAsyncReadBudget(data.cacheBytes);
    void WasmAsyncFormatReader.open(data.file, data.attributes, asyncReadBudget.clone()).then((reader) => {
        messageQueue = messageQueue.then(() => {
            if (generation !== datasetGeneration) {
                reader.free();
                return;
            }
            const descriptor = reader.lodDatasetDescriptor(data.cacheBytes);
            const root = descriptor.nodes.find(node => node.parent === null);
            const opened = {
                ...viewer.openLodDataset(descriptor),
                nodeCount: descriptor.nodes.length,
                maxLevel: descriptor.nodes.reduce((max, node) => Math.max(max, node.level), 0),
                coordinateOrigin: new Float64Array(descriptor.coordinateOrigin),
                boundsMin: new Float64Array(root.min),
                boundsMax: new Float64Array(root.max),
                semantics: descriptor.semantics,
                attributeSchema: descriptor.attributeSchema,
                hasAttributes: descriptor.hasAttributes,
                sourcePointCount: descriptor.sourcePointCount ?? null,
            };
            asyncFormatReader = reader;
            asyncHasAttributes = descriptor.hasAttributes;
            asyncAttributeIndices = new Map(descriptor.attributeSchema.map(
                (attribute, index) => [attribute.name, index],
            ));
            viewer.fitScene();
            const lod = viewer.updateLod(targetVoxelPixels);
            currentMetadata = { ...opened, name, bytes: data.file.size, hasAttributes: descriptor.hasAttributes };
            if (requestId !== null) {
                self.postMessage({ type: "apiResult", requestId, value: { opened, lod, name } });
            } else {
                self.postMessage({ type: "datasetOpened", opened, name });
                self.postMessage({ type: "dataset", opened, lod, name });
            }
            postLodProgress(lod);
            scheduleAsyncLodRead();
        }).catch((error) => {
            if (generation === datasetGeneration) {
                self.postMessage({ type: "error", operation: "openFile", requestId, message: String(error) });
            }
        });
    }).catch((error) => {
        if (generation === datasetGeneration) {
            self.postMessage({ type: "error", operation: "openFile", requestId, message: String(error) });
        }
    });
}

async function handleMessage({ data }) {
    try {
        switch (data.type) {
            case "init": {
                wasmExports = await init();
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
                let deferredResult = false;
                switch (data.operation) {
                    case "load": {
                        startAsyncDataset(data, data.requestId);
                        deferredResult = true;
                        break;
                    }
                    case "fit":
                        advanceSceneGeneration();
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
                        advanceSceneGeneration();
                        value = viewer.setCameraPose(
                            new Float64Array(data.position), new Float64Array(data.target),
                        );
                        break;
                    case "setOrbitTarget":
                        advanceSceneGeneration();
                        value = viewer.setOrbitTarget(new Float64Array(data.target));
                        break;
                    case "getMetadata":
                        value = currentMetadata;
                        break;
                    case "getStats":
                        value = { lod: viewer.lodStats(), gpu: viewer.gpuStats(), viewer: viewer.stats(), memory: memoryStats() };
                        break;
                    case "moveCamera":
                        advanceSceneGeneration();
                        value = viewer.moveCamera(data.x, data.y, data.z);
                        break;
                    case "birdView":
                        advanceSceneGeneration();
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
                            advanceSceneGeneration();
                            viewer.renderFrame();
                        }
                        value = { hit, sceneGeneration: pickedGeneration };
                        break;
                    }
                    default:
                        throw new Error(`unknown viewer API operation: ${data.operation}`);
                }
                if (!deferredResult) self.postMessage({ type: "apiResult", requestId: data.requestId, value });
                break;
            }
            case "resize":
                advanceSceneGeneration();
                viewer.resize(data.width, data.height);
                break;
            case "refine": {
                const startedAt = performance.now();
                const colorModeChanged = data.colorMode !== currentColorMode;
                viewer.setColorMode(data.colorMode);
                advanceSceneGeneration();
                if (colorModeChanged) cancelAllAsyncReads();
                currentColorMode = data.colorMode;
                targetVoxelPixels = data.targetVoxelPixels;
                beginRefinement(data.trigger, data.refinementId ?? 0, data.colorMode, startedAt);
                if (viewer.lodStats()) {
                    const lod = viewer.updateLod(targetVoxelPixels);
                    self.postMessage({ type: "lod", lod });
                    postLodProgress(lod);
                    scheduleAsyncLodRead(true);
                }
                break;
            }
            case "depthShading":
                viewer.setDepthShading(data.value);
                break;
            case "openFile": {
                startAsyncDataset(data);
                break;
            }
            case "fit":
                advanceSceneGeneration();
                viewer.fitScene();
                break;
            case "birdView":
                advanceSceneGeneration();
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
                    advanceSceneGeneration();
                }
                self.postMessage({ type: "pick", hit, sceneGeneration: pickedGeneration });
                break;
            }
            case "frame": {
                const camera = data.camera;
                if (Object.values(camera).some((value) => value !== 0)) advanceSceneGeneration();
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
                scheduleAsyncLodRead();
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
