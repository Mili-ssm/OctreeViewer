import init, { WasmAsyncFormatReader, WasmAsyncReadBudget, WasmBlockRequestScheduler, WasmVoxelViewer } from "../../pkg/viewer_wasm.js";
import { resolveSource } from "./http-range-source.mjs";

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
const coalescedCommands = new Set(["resize", "refine", "depthShading", "continuity", "fit", "birdView"]);

// Benchmark instrumentation (render plan R0, E2/E3/E5). Aggregates only; read with the "metrics"
// message. Self-contained on purpose: package-local.py embeds this worker without module imports.
const QUEUE_WAIT_SAMPLE_CAPACITY = 8192;
const LOD_LOG_CAPACITY = 4096;
const workKinds = new Map();
const frameQueueWaits = [];
const lodLog = [];
let lodLogDropped = 0;
const asyncBlockTotals = { blocks: 0, readMs: 0, decodeMs: 0, maxDecodeMs: 0, insertMs: 0, maxInsertMs: 0 };

// Build-worker mode (render plan R2). Off unless `init` carries `buildWorker: { url, type }`: the
// LOD hierarchy, block reads and scene building then live in build-worker.js and this worker only
// keeps the camera, the renderer and the last LOD stats. See build-worker.js for the protocol.
const BUILD_SAMPLE_CAPACITY = 8192;
const BUILD_METRICS_TIMEOUT_MS = 5000;
let buildOptions = null;
let buildWorker = null;
let buildWorkerUsed = false;
let buildOpened = false;
let pendingOpen = null;
let appliedEpoch = 0;
let viewEpoch = 0;
// Cells path (R3, opt-in with `init.cells`). `cellsRemote`: the build worker's builder produces the
// cells and the viewer's driver lives here. See build-worker.js for the messages.
let cellsRequested = false;
let cellsRemote = false;
// R3-Q24: cells run only with the build worker. Set when they were requested without one; the CPU
// path then runs and the LOD stats report this as `cellPath`.
let cellGate = null;
let pendingCellCut = null;
let appliedCutEpoch = 0;
let lastOpenRequest = null;
// Frames in a row with cells busy and no progress after which the driver counts as stuck: the busy
// flag is then reported false (the host's frame loop ends) and `cellsStuck` is set.
const CELLS_STUCK_FRAME_LIMIT = 600;
let cellsStuckKey = null;
let cellsStuckFrames = 0;
const cellCounters = { nodeCellMessages: 0, nodeCells: 0, nodeCellBytes: 0, cellCuts: 0, cellCutsSuperseded: 0, strayScenes: 0, messagesSent: 0 };
let remoteVisualScale = 1;
let lastSentView = null;
let currentLod = null;
// What the build worker last confirmed (`colorMode` reply), so a rejected mode can be undone here.
let confirmedColorMode = "rgb";
let buildAttributeRange = null;
// Digest of the scene displayed from a build worker; the in-worker path reads it from the viewer.
let appliedSceneDigest = null;
let pendingBuildMetrics = [];
const viewSentAt = new Map();
const buildSamples = { sceneAgeMs: [], epochLag: [], sceneApplyMs: [] };
const buildCounters = { viewsSent: 0, scenesApplied: 0, scenesDropped: 0, spawns: 0 };
// Epoch milliseconds, comparable with the main thread's timestamps.
const epochNow = () => (performance.timeOrigin ?? 0) + performance.now();

function recordWork(kind, queuedAt, startedAt, endedAt) {
    let entry = workKinds.get(kind);
    if (!entry) {
        entry = { count: 0, busyMs: 0, maxBusyMs: 0, queueWaitMs: 0, maxQueueWaitMs: 0 };
        workKinds.set(kind, entry);
    }
    const busy = endedAt - startedAt;
    const wait = Math.max(0, startedAt - queuedAt);
    entry.count += 1;
    entry.busyMs += busy;
    entry.maxBusyMs = Math.max(entry.maxBusyMs, busy);
    entry.queueWaitMs += wait;
    entry.maxQueueWaitMs = Math.max(entry.maxQueueWaitMs, wait);
    if (kind === "frame" && frameQueueWaits.length < QUEUE_WAIT_SAMPLE_CAPACITY) frameQueueWaits.push(wait);
}

// Wraps a task chained on messageQueue so its queue wait and busy time are recorded by kind.
function timed(kind, task) {
    const queuedAt = performance.now();
    return () => {
        const startedAt = performance.now();
        try {
            return task();
        } finally {
            recordWork(kind, queuedAt, startedAt, performance.now());
        }
    };
}

function recordLodMessage(source, lod) {
    if (!lod) return;
    if (lodLog.length >= LOD_LOG_CAPACITY) {
        lodLogDropped += 1;
        return;
    }
    lodLog.push({
        time: epochNow(),
        source,
        stagePx: lod.lodStagePixels,
        targetPx: lod.targetVoxelPixels,
        oversizedVoxels: lod.oversizedVoxels,
        uncoveredNodes: lod.uncoveredNodes,
        pendingBlocks: lod.pendingBlockIds?.length ?? 0,
        visibleBlocks: lod.visibleBlocks,
        pendingTargetEvictions: lod.pendingTargetEvictions ?? null,
        cacheCycleDetected: lod.cacheCycleDetected ?? null,
    });
}

function summarizeSamples(samples) {
    const ordered = [...samples].sort((a, b) => a - b);
    const at = fraction => ordered.length
        ? ordered[Math.min(ordered.length - 1, Math.ceil(ordered.length * fraction) - 1)] : null;
    return { count: ordered.length, p50: at(0.50), p95: at(0.95), max: at(1) };
}

function recordBuildSample(name, value) {
    const samples = buildSamples[name];
    if (samples.length < BUILD_SAMPLE_CAPACITY) samples.push(value);
}

function workerMetricsSnapshot(buildMetrics = null) {
    const snapshot = {
        timeOrigin: performance.timeOrigin ?? null,
        busyByMessage: Object.fromEntries(workKinds),
        frameQueueWaitMs: summarizeSamples(frameQueueWaits),
        asyncBlocks: { ...asyncBlockTotals },
        lodLog: [...lodLog],
        lodLogDropped,
        buildWorkerEnabled: buildOptions !== null,
        sceneDigest: currentSceneDigest(),
    };
    // Absent with cells off, so the shape consumers read does not change.
    if (cellsRequested) {
        snapshot.cutDigestAtRest = (buildOptions ? currentLodStats() : viewer?.lodStats?.())?.cutDigestAtRest ?? null;
        snapshot.cellStats = viewer?.cellStats?.() ?? null;
        snapshot.cellCounters = { ...cellCounters, cellsStuckFrames };
    }
    if (cellGate) snapshot.cellPath = cellGate;
    if (buildOptions === null) return snapshot;
    // The build worker owns reads and decoding, so its totals replace this worker's empty ones.
    if (buildMetrics) snapshot.asyncBlocks = { ...buildMetrics.asyncBlocks };
    return {
        ...snapshot,
        sceneAgeMs: summarizeSamples(buildSamples.sceneAgeMs),
        epochLag: summarizeSamples(buildSamples.epochLag),
        sceneApplyMs: {
            ...summarizeSamples(buildSamples.sceneApplyMs),
            totalMs: buildSamples.sceneApplyMs.reduce((sum, value) => sum + value, 0),
        },
        viewsSent: buildCounters.viewsSent,
        viewsCoalesced: buildMetrics?.build.viewsCoalesced ?? null,
        scenesApplied: buildCounters.scenesApplied,
        scenesDropped: buildCounters.scenesDropped,
        buildWorkerSpawns: buildCounters.spawns,
        wasmLinearMemoryBytes: {
            render: wasmExports?.memory?.buffer.byteLength ?? null,
            build: buildMetrics?.wasmLinearMemoryBytes ?? null,
        },
        buildWorker: buildMetrics,
    };
}

function resetWorkerMetrics() {
    workKinds.clear();
    frameQueueWaits.length = 0;
    lodLog.length = 0;
    lodLogDropped = 0;
    Object.assign(asyncBlockTotals, {
        blocks: 0, readMs: 0, decodeMs: 0, maxDecodeMs: 0, insertMs: 0, maxInsertMs: 0,
    });
    for (const samples of Object.values(buildSamples)) samples.length = 0;
    Object.assign(buildCounters, { viewsSent: 0, scenesApplied: 0, scenesDropped: 0, spawns: 0 });
    for (const key of Object.keys(cellCounters)) cellCounters[key] = 0;
    buildWorker?.postMessage({ type: "metrics", reset: true });
}

function requestBuildMetrics() {
    if (!buildWorker) return Promise.resolve(null);
    return new Promise((resolve) => {
        const waiter = (metrics) => {
            clearTimeout(timer);
            resolve(metrics);
        };
        const timer = setTimeout(() => {
            pendingBuildMetrics = pendingBuildMetrics.filter(entry => entry !== waiter);
            resolve(null);
        }, BUILD_METRICS_TIMEOUT_MS);
        pendingBuildMetrics.push(waiter);
        buildWorker.postMessage({ type: "metrics" });
    });
}

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

function withCellGate(lod) {
    return lod && cellGate ? { ...lod, cellPath: cellGate } : lod;
}

// Stats of the last LOD update: the renderer's own, or the build worker's in build-worker mode.
function currentLodStats() {
    return buildOptions ? withRenderCellStats(currentLod) : withStuckGuard(withCellGate(viewer.lodStats()));
}

// Counts frames that left the cells busy without progress (cut applied, queue, upload, cut sent).
function trackCellsStuck() {
    const cells = cellsRequested ? viewer.cellStats?.() : null;
    if (!cells?.cellsBusy) {
        cellsStuckKey = null;
        cellsStuckFrames = 0;
        return;
    }
    const key = [cells.cutEpochSent, cells.cutEpochApplied, cells.queuedNodeCells, cells.poolUploadedBytes].join(":");
    if (key === cellsStuckKey) {
        cellsStuckFrames += 1;
    } else {
        cellsStuckKey = key;
        cellsStuckFrames = 0;
    }
}

// A driver that stays busy without progress must not keep the host's frames running forever.
function withStuckGuard(lod) {
    if (!lod || !cellsRequested) return lod;
    if (lod.cellsBusy && cellsStuckFrames >= CELLS_STUCK_FRAME_LIMIT) {
        return { ...lod, cellsBusy: false, cellsStuck: true, cellsStuckFrames };
    }
    return { ...lod, cellsStuck: false, cellsStuckFrames };
}

// With a build worker the LOD stats come from the builder, but the driver is here: its busy flag and
// resting digest replace the builder's, so "still loading" covers the cells not yet drawn.
function withRenderCellStats(lod) {
    if (!lod || !cellsRemote) return lod;
    const cells = viewer.cellStats();
    if (!cells) return lod;
    return withStuckGuard({ ...lod, ...cells, cellsBusy: Boolean(lod.cellsBusy || cells.cellsBusy) });
}

// Draws a frame and sends the build worker what its driver owes it (acks, evictions, resets).
function renderViewerFrame() {
    const status = viewer.renderFrame();
    forwardCellMessages();
    return status;
}

// `takeCellMessages` is in the order the builder needs; the channel keeps it. `evict` and the resets
// are never merged. Only the ack is a state, and the build worker keeps the last.
function forwardCellMessages() {
    if (!cellsRemote || !buildWorker) return;
    const messages = viewer.takeCellMessages();
    const gen = datasetGeneration;
    if (messages.driverReplaced) buildWorker.postMessage({ type: "driverReplaced", gen });
    if (messages.poolReset) buildWorker.postMessage({ type: "poolReset", gen });
    if (messages.evict.length > 0) {
        buildWorker.postMessage({ type: "evict", gen, nodes: messages.evict }, [messages.evict.buffer]);
    }
    if (messages.poolAck) buildWorker.postMessage({ type: "poolAck", gen, ...messages.poolAck });
    cellCounters.messagesSent += Number(Boolean(messages.driverReplaced)) + Number(Boolean(messages.poolReset))
        + Number(messages.evict.length > 0) + Number(Boolean(messages.poolAck));
}

// Hex digest of the scene on screen (`null` while nothing is displayed), identical in both modes.
function currentSceneDigest() {
    return (buildOptions ? currentLod : viewer?.lodStats?.())?.sceneDigest ?? null;
}

// Debug dump of what the displayed scene was built from, to compare the in-worker and build-worker
// paths (`apiCall("sceneInputs")`). The camera is the renderer's current one, which is what the next
// view would carry; in build-worker mode `lastSentView` is what the build worker last received.
function sceneInputs() {
    const lod = currentLodStats();
    return {
        buildWorker: buildOptions !== null,
        camera: Array.from(viewer.cameraTransport()),
        viewport: Array.from(viewer.viewportSize()),
        voxelBuildLimits: Array.from(viewer.voxelBuildLimits()),
        targetPx: targetVoxelPixels,
        colorMode: currentColorMode,
        remoteVisualScale: buildOptions ? remoteVisualScale : null,
        viewEpoch: buildOptions ? viewEpoch : null,
        appliedEpoch: buildOptions ? appliedEpoch : null,
        lastSentView: buildOptions && lastSentView ? {
            camera: Array.from(lastSentView.camera), viewport: lastSentView.viewport, targetPx: lastSentView.targetPx,
        } : null,
        attributeRange: lod?.attributeRange ?? null,
        sceneDigest: currentSceneDigest(),
        lod,
    };
}

function postStats(frameStatus = null) {
    self.postMessage({
        type: "stats",
        frameStatus,
        frameRenderMs: frameRenderPercentiles(),
        gpu: viewer.gpuStats(),
        lod: currentLodStats(),
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
        && (lod.pendingPageAnchors?.length ?? 0) === 0
        && lod.lodStagePixels <= lod.targetVoxelPixels
        && !lod.cellsBusy;
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
        lod.cellsBusy ?? "",
        lod.cellsStuck ?? "",
        lod.cutDigestAtRest ?? "",
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

// Hierarchy pages of a paged format load like blocks: the LOD names the anchors it lacks.
const pageReads = new Set();
// Like failed blocks, a failed page waits for an explicit refinement or a new dataset; otherwise the
// `finally` below would request it again at once, in a loop.
const failedPageAnchors = new Set();
function schedulePageReads(retryFailed = false) {
    if (retryFailed) failedPageAnchors.clear();
    if (!asyncFormatReader || !viewer || typeof viewer.pendingPageAnchors !== "function") return;
    const reader = asyncFormatReader;
    const generation = datasetGeneration;
    for (const anchor of viewer.pendingPageAnchors()) {
        if (pageReads.size >= MAX_ASYNC_LOD_READS) break;
        if (pageReads.has(anchor) || failedPageAnchors.has(anchor)) continue;
        pageReads.add(anchor);
        void reader.loadPage(anchor).then((page) => {
            const graft = messageQueue.then(() => {
                if (generation !== datasetGeneration || reader !== asyncFormatReader) {
                    page.free();
                    return;
                }
                viewer.graftPage(page);
                const lod = withCellGate(viewer.updateLod(targetVoxelPixels));
                self.postMessage({ type: "lod", lod });
                recordLodMessage("asyncPage", lod);
                postLodProgress(lod);
            });
            // A failed graft is reported below; it must not reject the queue and skip later tasks.
            messageQueue = graft.catch(() => {});
            return graft;
        }).catch((error) => {
            if (generation === datasetGeneration) {
                failedPageAnchors.add(anchor);
                self.postMessage({ type: "error", operation: "asyncPageRead", message: String(error) });
            }
        }).finally(() => {
            pageReads.delete(anchor);
            if (generation === datasetGeneration) scheduleAsyncLodRead();
        });
    }
}

function scheduleAsyncLodRead(retryFailed = false) {
    schedulePageReads(retryFailed);
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
                const readMs = result.readMs ?? 0;
                const decodeMs = result.decodeMs ?? 0;
                await (messageQueue = messageQueue.then(timed("asyncInsert", () => {
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
                    const insertStartedAt = performance.now();
                    viewer.insertAsyncLodBlock(
                        blockId, payload.positions, payload.colors ?? null, payload.attribute ?? null,
                    );
                    ticketScheduler.complete(ticket, String(blockId), true);
                    ticketCompleted = true;
                    const lod = withCellGate(viewer.updateLod(targetVoxelPixels));
                    const insertMs = performance.now() - insertStartedAt;
                    asyncBlockTotals.blocks += 1;
                    asyncBlockTotals.readMs += readMs;
                    asyncBlockTotals.decodeMs += decodeMs;
                    asyncBlockTotals.maxDecodeMs = Math.max(asyncBlockTotals.maxDecodeMs, decodeMs);
                    asyncBlockTotals.insertMs += insertMs;
                    asyncBlockTotals.maxInsertMs = Math.max(asyncBlockTotals.maxInsertMs, insertMs);
                    self.postMessage({ type: "lod", lod });
                    recordLodMessage("asyncInsert", lod);
                    postLodProgress(lod);
                })).catch((error) => {
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
                    let outcome = null;
                    if (!ticketCompleted) {
                        outcome = ticketScheduler.complete(ticket, String(blockId), false);
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

// Nested workers are not available everywhere (a Blob worker created from another Blob worker is
// refused on file:// in Chrome). The option then turns itself off and the in-worker path runs.
function gateCellsWithoutBuildWorker() {
    if (!cellsRequested) return;
    cellsRequested = false;
    cellGate = "needsBuildWorker";
    viewer.enableCells(false);
}

function disableBuildWorker(error) {
    const message = String(error?.message ?? error);
    console.warn(`build worker unavailable, building scenes in the render worker: ${message}`);
    buildWorker?.terminate();
    buildWorker = null;
    buildOptions = null;
    gateCellsWithoutBuildWorker();
    self.postMessage({ type: "buildWorkerUnavailable", message });
}

function spawnBuildWorker() {
    let worker;
    try {
        worker = new Worker(buildOptions.url, { type: buildOptions.type ?? "module" });
    } catch (error) {
        disableBuildWorker(error);
        return;
    }
    worker.addEventListener("message", event => onBuildMessage(worker, event.data));
    worker.addEventListener("error", (event) => {
        if (worker !== buildWorker) return;
        event.preventDefault?.();
        // A worker that fails before any dataset used it never started: fall back instead of failing.
        if (!buildWorkerUsed) {
            disableBuildWorker(event.message || "build worker failed to start");
            return;
        }
        self.postMessage({ type: "error", operation: "buildWorker", message: event.message || "build worker failed" });
    });
    // The packaged file:// worker has the WASM bytes inline; hand them over instead of embedding twice.
    worker.postMessage({
        type: "init",
        cells: cellsRequested,
        wasmBytes: typeof __VIEWER_WASM_BYTES__ !== "undefined" ? __VIEWER_WASM_BYTES__ : undefined,
    });
    buildWorker = worker;
    buildWorkerUsed = false;
    buildCounters.spawns += 1;
}

// Cancelling a dataset is terminate() + respawn: it also stops the reads and any build in flight.
function freshBuildWorker() {
    if (buildWorker && buildWorkerUsed) {
        buildWorker.terminate();
        buildWorker = null;
        pendingBuildMetrics = [];
    }
    if (!buildWorker) spawnBuildWorker();
    buildWorkerUsed = buildWorker !== null;
    return buildWorker;
}

function startBuiltDataset(data, requestId = null) {
    // Replacing a used worker can hit the same restriction as the first spawn.
    const worker = freshBuildWorker();
    if (!worker) {
        startAsyncDataset(data, requestId);
        return;
    }
    advanceSceneGeneration();
    datasetGeneration += 1;
    lastOpenRequest = { data, requestId };
    cellsRemote = false;
    pendingCellCut = null;
    appliedCutEpoch = 0;
    cellsStuckKey = null;
    cellsStuckFrames = 0;
    targetVoxelPixels = data.targetVoxelPixels;
    datasetOpenedAt = performance.now();
    currentColorMode = "rgb";
    confirmedColorMode = "rgb";
    buildAttributeRange = null;
    appliedSceneDigest = null;
    appliedEpoch = 0;
    buildOpened = false;
    lastSentView = null;
    currentLod = null;
    viewSentAt.clear();
    beginRefinement("dataset", data.refinementId ?? 0, "rgb", datasetOpenedAt);
    const name = data.name ?? data.file.name;
    pendingOpen = { requestId, name, bytes: data.file.size ?? null, opened: null };
    self.postMessage({ type: "status", message: `Opening ${name}…` });
    const [maxVoxels, maxEstimatedBytes] = viewer.voxelBuildLimits();
    worker.postMessage({
        type: "open",
        gen: datasetGeneration,
        file: data.file,
        attributes: data.attributes,
        cacheBytes: data.cacheBytes,
        limits: [Math.min(maxVoxels, 0xFFFFFFFF), Math.min(maxEstimatedBytes, 0xFFFFFFFF)],
        cells: cellsRequested,
        // The build side fits the cut to the pool this device runs (ADR 0040).
        cellPoolPages: cellsRequested ? viewer.cellPoolPages() : 0,
    });
}

function sameValues(left, right) {
    if (left.length !== right.length) return false;
    for (let index = 0; index < left.length; index += 1) {
        if (left[index] !== right[index]) return false;
    }
    return true;
}

// Sends the build worker the camera it should build for, at most once per message handled and only
// when the camera, viewport or pixel target differ from the last one sent (`force` skips the test).
function syncBuildView(force = false, retryFailed = false) {
    if (!buildWorker || !buildOpened) return;
    const camera = viewer.cameraTransport();
    // Read together with the camera: the viewer's epoch is the one the cells cut is judged against.
    const cellViewEpoch = cellsRemote ? viewer.viewEpoch() : undefined;
    const viewport = Array.from(viewer.viewportSize());
    if (!force && lastSentView && lastSentView.targetPx === targetVoxelPixels
        && sameValues(lastSentView.viewport, viewport) && sameValues(lastSentView.camera, camera)) return;
    lastSentView = { camera, viewport, targetPx: targetVoxelPixels };
    viewEpoch += 1;
    viewSentAt.set(viewEpoch, performance.now());
    if (viewSentAt.size > 256) viewSentAt.delete(viewSentAt.keys().next().value);
    buildCounters.viewsSent += 1;
    buildWorker.postMessage({
        type: "view",
        gen: datasetGeneration,
        epoch: viewEpoch,
        viewEpoch: cellViewEpoch,
        camera,
        viewport,
        visualScale: remoteVisualScale,
        targetPx: targetVoxelPixels,
        retryFailed,
    });
}

// Self-contained on purpose (workers import nothing); a test runs this copy.
function lodStillLoading(lod) {
    if (lod.cellsBusy) return true;
    return !lod.lodError && !lod.pageBudgetExhausted && (lod.pendingBlockIds.length > 0 || (lod.pendingPageAnchors?.length ?? 0) > 0 || lod.lodStagePixels > lod.targetVoxelPixels);
}

function applyBuiltLod(lod, source) {
    lod.sceneDigest = appliedSceneDigest;
    lod.attributeRange = buildAttributeRange;
    currentLod = lod;
    lod = withRenderCellStats(lod);
    if (pendingOpen) {
        const { requestId, name, opened } = pendingOpen;
        pendingOpen = null;
        if (requestId !== null) {
            self.postMessage({ type: "apiResult", requestId, value: { opened, lod, name } });
        } else {
            self.postMessage({ type: "dataset", opened, lod, name });
        }
        recordLodMessage("dataset", lod);
    } else {
        self.postMessage({ type: "lod", lod });
        recordLodMessage(source, lod);
    }
    postLodProgress(lod);
}

function onBuildMessage(worker, data) {
    if (worker !== buildWorker) return;
    if (data.type === "cellCut" && data.gen === datasetGeneration) pendingCellCut = data;
    if (data.type === "metrics") {
        const waiting = pendingBuildMetrics;
        pendingBuildMetrics = [];
        for (const resolve of waiting) resolve(data.metrics);
        return;
    }
    const kind = data.type === "scene" ? "buildScene" : `build:${data.type}`;
    const task = timed(kind, () => handleBuildMessage(data));
    void (messageQueue = messageQueue.then(task, task));
}

// The builder emits cells while the device cannot draw them (no support on the render side): reopen
// the dataset on the CPU path, which both sides then keep until the viewer is recreated.
function beginCells(opened) {
    const info = opened.cellsInfo;
    const path = info ? viewer.beginRemoteCells(info.origin, info.rootMin, info.rootSide, info.referenceSpacing) : "off";
    if (path === "enabled") {
        cellsRemote = true;
        forwardCellMessages();
        return true;
    }
    console.warn(`cells path unavailable in the render worker (${path}); reopening on the CPU path`);
    cellsRequested = false;
    viewer.enableCells(false);
    const { data, requestId } = lastOpenRequest;
    startBuiltDataset(data, requestId);
    return false;
}

function handleBuildMessage(data) {
    if (data.gen !== datasetGeneration) {
        if (data.type === "scene") buildCounters.scenesDropped += 1;
        return;
    }
    if (data.type === "cellCut" && pendingCellCut !== data) {
        // A later cut is queued behind this one: only the last of a run is applied.
        cellCounters.cellCutsSuperseded += 1;
        return;
    }
    try {
        if (data.type === "opened") {
            const { opened } = data;
            remoteVisualScale = opened.visualScale;
            viewer.beginRemoteDataset(
                opened.visualScale, opened.coordinateOrigin, opened.boundsMin, opened.boundsMax,
            );
            const { name, bytes } = pendingOpen;
            pendingOpen.opened = opened;
            currentMetadata = { ...opened, name, bytes, hasAttributes: opened.hasAttributes };
            if (opened.cellPath === "enabled" && !beginCells(opened)) return;
            buildOpened = true;
            if (pendingOpen.requestId === null) self.postMessage({ type: "datasetOpened", opened, name });
            syncBuildView(true);
        } else if (data.type === "nodeCells") {
            // Never dropped by epoch: a node lost here is a hole the builder believes it filled.
            // A refused node does not stop the rest of the message; the first error is reported.
            let failure = null;
            for (const buffer of data.buffers) {
                try {
                    viewer.addCellNode(data.generation, new Uint8Array(buffer));
                    cellCounters.nodeCells += 1;
                    cellCounters.nodeCellBytes += buffer.byteLength;
                } catch (error) {
                    failure ??= error;
                }
            }
            cellCounters.nodeCellMessages += 1;
            if (failure) throw failure;
        } else if (data.type === "cellCut") {
            pendingCellCut = null;
            cellCounters.cellCuts += 1;
            viewer.setCellCut(data);
        } else if (data.type === "cellLod") {
            if (data.cutEpoch <= appliedCutEpoch) return;
            appliedCutEpoch = data.cutEpoch;
            for (const key of viewSentAt.keys()) {
                if (key <= data.viewEpoch) viewSentAt.delete(key);
            }
            applyBuiltLod(data.lod, "cellLod");
            if (!lodStillLoading(currentLodStats())) {
                renderViewerFrame();
                postLodProgress(currentLodStats(), true);
            }
        } else if (data.type === "scene" || data.type === "lod") {
            if (data.type === "scene" && cellsRemote) {
                // The builder sends no scene in cells mode, and one would drop the pool.
                cellCounters.strayScenes += 1;
                console.error?.("build worker sent a scene while the cells path is active");
                return;
            }
            if (data.epoch <= appliedEpoch) {
                if (data.type === "scene") buildCounters.scenesDropped += 1;
                return;
            }
            appliedEpoch = data.epoch;
            if (data.type === "scene") {
                const startedAt = performance.now();
                viewer.uploadSceneInstances(new Uint8Array(data.buf), data.blockIds);
                appliedSceneDigest = data.digest ?? null;
                advanceSceneGeneration();
                const sentAt = viewSentAt.get(data.viewEpoch);
                recordBuildSample("sceneApplyMs", performance.now() - startedAt);
                if (sentAt !== undefined) recordBuildSample("sceneAgeMs", performance.now() - sentAt);
                recordBuildSample("epochLag", viewEpoch - data.viewEpoch);
                buildCounters.scenesApplied += 1;
                data.lod.renderedVoxels = data.count;
            }
            for (const key of viewSentAt.keys()) {
                if (key <= data.viewEpoch) viewSentAt.delete(key);
            }
            applyBuiltLod(data.lod, data.type === "scene" ? "scene" : "lod");
            // The host stops requesting frames once the LOD is complete: draw the final scene.
            if (data.type === "scene") {
                // The merged stats: the cells driver (if any) may still be busy.
                const merged = currentLodStats();
                if (!lodStillLoading(merged)) {
                    renderViewerFrame();
                    // No host frame follows, so this is where the first-frame and target milestones land.
                    postLodProgress(merged, true);
                }
            }
        } else if (data.type === "colorMode") {
            confirmedColorMode = data.mode;
            buildAttributeRange = data.attributeRange ?? null;
            if (currentLod) currentLod.attributeRange = buildAttributeRange;
        } else if (data.type === "error") {
            if (data.operation === "colorMode") {
                // The build worker kept its previous mode: show the same one, as the in-worker
                // path does when the viewer rejects a mode.
                currentColorMode = confirmedColorMode;
                viewer.setPresentationColorMode(confirmedColorMode);
            }
            const requestId = data.operation === "openFile" ? pendingOpen?.requestId : undefined;
            if (data.operation === "openFile") pendingOpen = null;
            self.postMessage({ type: "error", operation: data.operation, requestId, message: data.message });
        }
    } catch (error) {
        self.postMessage({ type: "error", operation: `build:${data.type}`, message: String(error) });
    }
}

function startAsyncDataset(data, requestId = null) {
    cellsRemote = false;
    cellsStuckKey = null;
    cellsStuckFrames = 0;
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
    failedPageAnchors.clear();
    beginRefinement("dataset", data.refinementId ?? 0, "rgb", datasetOpenedAt);
    const name = data.name ?? data.file.name;
    self.postMessage({ type: "status", message: `Opening ${name}…` });
    // One budget per worker, shared by successive readers; a different cacheBytes replaces it.
    if (asyncReadBudget === null || asyncReadBudget.limitBytes() !== String(data.cacheBytes)) {
        const previous = asyncReadBudget;
        asyncReadBudget = new WasmAsyncReadBudget(data.cacheBytes);
        previous?.free();
    }
    // A remote file is `{ url }`: each worker opens its own range source (it cannot be cloned).
    let fileBytes = data.file.size ?? null;
    void Promise.all([resolveSource(data.file), resolveSource(data.attributes, { tailBytes: 40, headBytes: 24 })]).then(([file, attributes]) => {
        fileBytes = file.size;
        return WasmAsyncFormatReader.open(file, attributes, asyncReadBudget.clone());
    }).then((reader) => {
        messageQueue = messageQueue.then(timed("asyncOpen", () => {
            if (generation !== datasetGeneration) {
                reader.free();
                return;
            }
            const descriptor = reader.lodDatasetDescriptor(data.cacheBytes);
            const opened = {
                ...(descriptor.paged
                    ? viewer.openPagedLodDataset(reader, data.cacheBytes)
                    : viewer.openLodDataset(descriptor)),
                nodeCount: descriptor.nodeCount,
                maxLevel: descriptor.maxLevel,
                paged: descriptor.paged,
                coordinateOrigin: new Float64Array(descriptor.coordinateOrigin),
                boundsMin: new Float64Array(descriptor.dataMin),
                boundsMax: new Float64Array(descriptor.dataMax),
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
            const lod = withCellGate(viewer.updateLod(targetVoxelPixels));
            currentMetadata = { ...opened, name, bytes: fileBytes, hasAttributes: descriptor.hasAttributes };
            if (requestId !== null) {
                self.postMessage({ type: "apiResult", requestId, value: { opened, lod, name } });
            } else {
                self.postMessage({ type: "datasetOpened", opened, name });
                self.postMessage({ type: "dataset", opened, lod, name });
            }
            recordLodMessage("dataset", lod);
            postLodProgress(lod);
            scheduleAsyncLodRead();
        })).catch((error) => {
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

async function handleMessage({ data }, queuedAt = performance.now()) {
    const startedAt = performance.now();
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
                cellsRequested = Boolean(data.cells);
                cellGate = null;
                buildOptions = data.buildWorker ?? null;
                if (cellsRequested && !buildOptions) {
                    cellsRequested = false;
                    cellGate = "needsBuildWorker";
                } else if (cellsRequested) {
                    viewer.enableCells(true);
                    if (data.cellNodeBytesPerFrame) viewer.setCellNodeBytesPerFrame(data.cellNodeBytesPerFrame);
                }
                // Compile the build worker's WASM now so the first dataset does not wait for it.
                if (buildOptions) spawnBuildWorker();
                self.postMessage({ type: "ready" });
                break;
            }
            case "api": {
                let value;
                let deferredResult = false;
                switch (data.operation) {
                    case "load": {
                        if (buildOptions) startBuiltDataset(data, data.requestId);
                        else startAsyncDataset(data, data.requestId);
                        deferredResult = true;
                        break;
                    }
                    case "fit":
                        advanceSceneGeneration();
                        value = viewer.fitScene();
                        break;
                    case "renderOnce":
                        value = renderViewerFrame();
                        postLodProgress(currentLodStats(), true);
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
                    case "sceneInputs":
                        value = sceneInputs();
                        break;
                    case "getStats":
                        value = { lod: currentLodStats(), gpu: viewer.gpuStats(), viewer: viewer.stats(), memory: memoryStats() };
                        break;
                    case "setGpuFrameTiming":
                        viewer.setGpuFrameTiming(Boolean(data.enabled));
                        value = true;
                        break;
                    case "resetGpuFrameStats":
                        viewer.resetGpuFrameStats();
                        value = true;
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
                            renderViewerFrame();
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
                if (buildOptions) viewer.setPresentationColorMode(data.colorMode);
                else viewer.setColorMode(data.colorMode);
                advanceSceneGeneration();
                if (colorModeChanged) {
                    if (buildOptions) {
                        buildWorker.postMessage({ type: "colorMode", gen: datasetGeneration, mode: data.colorMode });
                    } else {
                        cancelAllAsyncReads();
                    }
                }
                currentColorMode = data.colorMode;
                targetVoxelPixels = data.targetVoxelPixels;
                beginRefinement(data.trigger, data.refinementId ?? 0, data.colorMode, startedAt);
                if (buildOptions) {
                    syncBuildView(true, true);
                } else if (viewer.lodStats()) {
                    const lod = withCellGate(viewer.updateLod(targetVoxelPixels));
                    self.postMessage({ type: "lod", lod });
                    recordLodMessage("refine", lod);
                    postLodProgress(lod);
                    scheduleAsyncLodRead(true);
                }
                break;
            }
            case "depthShading":
                viewer.setDepthShading(data.value);
                break;
            case "continuity":
                // RF-05: only changes which cell level each octant draws; nothing is rebuilt.
                viewer.setContinuity(Boolean(data.enabled), data.factor ?? undefined);
                advanceSceneGeneration();
                break;
            case "metrics":
                if (data.reset) resetWorkerMetrics();
                else self.postMessage({ type: "metrics", workerMetrics: workerMetricsSnapshot(await requestBuildMetrics()) });
                break;
            case "openFile": {
                if (buildOptions) startBuiltDataset(data);
                else startAsyncDataset(data);
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
                syncBuildView();
                const startedAt = performance.now();
                const frameStatus = renderViewerFrame();
                const renderCallMs = performance.now() - startedAt;
                recordFrameRenderSample(renderCallMs);
                trackCellsStuck();
                postLodProgress(currentLodStats(), true);
                if (!buildOptions) scheduleAsyncLodRead();
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
        syncBuildView();
    } catch (error) {
        self.postMessage({ type: "error", operation: data.type, requestId: data.requestId, message: String(error) });
    } finally {
        if (coalescedCommands.has(data.type)) {
            self.postMessage({ type: "commandAck", command: data.type });
        }
        if (data.type !== "metrics") {
            recordWork(
                data.type === "api" ? `api:${data.operation}` : data.type,
                queuedAt, startedAt, performance.now(),
            );
        }
    }
}

let messageQueue = Promise.resolve();
self.addEventListener("message", (event) => {
    const queuedAt = performance.now();
    messageQueue = messageQueue.then(() => handleMessage(event, queuedAt), () => handleMessage(event, queuedAt));
});
