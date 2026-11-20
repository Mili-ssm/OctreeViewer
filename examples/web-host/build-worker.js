import init, { WasmAsyncFormatReader, WasmAsyncReadBudget, WasmBlockRequestScheduler, WasmSceneBuilder } from "../../pkg/viewer_wasm.js";
import { isAbortError, readConcurrency, resolveSource, sumRangeStats } from "./http-range-source.mjs";

// Build worker (render plan R2): owns the dataset hierarchy, the block reader and scheduler, the
// decoded block cache, the LOD planner and the voxel builder (`WasmSceneBuilder`, no GPU). The
// render worker (viewer-worker.js) sends the camera and draws the instance bytes this worker
// returns. Self-contained on purpose: package-local.py embeds it without module imports.
//
// render -> build: init{wasmBytes?}, open{gen,file,attributes,cacheBytes,limits,cells?}, colorMode{gen,mode},
//                  view{gen,epoch,viewEpoch?,camera,viewport,visualScale,targetPx,minSide?}, metrics{reset?}
// build -> render: opened{gen,opened}, colorMode{gen,mode,attributeRange} (the mode was accepted;
//                  a rejected one answers with error{operation:"colorMode"} and changes nothing),
//                  scene{gen,epoch,viewEpoch,buf,count,blockIds,lod,digest},
//                  lod{gen,epoch,viewEpoch,lod} (scene reused), error{gen,operation,message},
//                  metrics{metrics}
// `epoch` counts this worker's builds (strictly increasing within a gen); `viewEpoch` echoes the
// render worker's view epoch that the build used.
//
// Cells path (R3, requested with `open.cells`, which the render worker sets by default; every colour mode, R3-Q32/Q33): the builder emits no `scene`.
// build -> render: nodeCells{gen,generation,buffers} (one per node batch, transferred, in order, never
//                  coalesced), nodeLevels{gen,generation,buffers} (levels built on demand; sent right after the
//                  nodeCells of the same batch, transferred, in order, never coalesced),
//                  levelsRefused{gen,nodes,depths} (BigUint64Array ids and a parallel Uint8Array of depths the
//                  builder cannot serve, e.g. a leaf already at the deepest cell depth; the driver stops wanting
//                  them, so the view can settle; sent after nodeLevels, before cellCut, through the same
//                  messageQueue as the node messages), cellCut{gen,cutEpoch,viewEpoch,cut,support,idle,targetPixels} (latest wins),
//                  cellLod{gen,cutEpoch,viewEpoch,lod} (stats for the LOD panel).
// render -> build: poolAck{gen,cutEpoch,drawn,missing} (latest wins), evict{gen,nodes} (ordered, never
//                  coalesced), poolReset{gen}, driverReplaced{gen},
//                  levelRequests{gen,nodes,depths} (BigUint64Array ids and a parallel Uint8Array of
//                  depths: the whole set of levels the driver wants built; it replaces the previous set, so the
//                  latest wins; messages are handled in order, so one sent behind `open` finds the builder
//                  ready). They wait in `cellInbox` and are applied
//                  before the next update in the order the builder requires: driverReplaced, poolReset,
//                  evict, poolAck, levelRequests: the order of `ViewerCells::take_messages` and of the in-process path. The
//                  ack of a batch comes from the driver's last frame, after every eviction in the same
//                  batch, so its `drawn` holds no evicted node: evictions go first (they only clear `built`)
//                  and the ack then sets `drawn` last, so an evicted node is never marked drawn.
//                  `view.viewEpoch` is the viewer's epoch of that camera (`setViewEpoch`).
//                  A throw in one apply does not skip the rest of the batch: every item is applied, then
//                  one `error{operation:"build:cellInbox"}` goes to the render side.

// Per dataset: remote sources may take more reads in flight than local ones (`readConcurrency`).
let maxAsyncLodReads = 4;
let wasmExports = null;
let generation = 0;
let builder = null;
let reader = null;
let scheduler = null;
let readBudget = null;
let asyncHasAttributes = false;
let asyncAttributeIndices = new Map();
let currentColorMode = "rgb";
let cellsEnabled = false;
const cellInbox = { driverReplaced: false, poolReset: false, evict: [], poolAck: null, levelRequests: null };
let latestView = null;
// The view the last cells update planned. A render report (ack, evict) on that same view that
// leaves the builder not dirty needs no new cut: replanning would send a cut per ack, the render
// side would ack each one, and the exchange would never settle.
let lastCellsView = null;
let viewsSinceBuild = 0;
let retryFailedNext = false;
// View epoch for which a fetched page did not fit the page budget: refetching it before the view
// changes would fail the same way, so the pending anchor is left to the exhausted-budget state.
let pageBudgetBlockedEpoch = null;
let buildTimer = 0;
const asyncReadTasks = new Map();
const failedAsyncBlocks = new Set();
const asyncReadCancelTokens = new Set();

const workKinds = new Map();
// `waves`: page reads started while none was in flight, i.e. sequential page round trips.
const pageTotals = { reads: 0, waves: 0 };
const asyncBlockTotals = { blocks: 0, readMs: 0, decodeMs: 0, maxDecodeMs: 0, insertMs: 0, maxInsertMs: 0 };
const buildTotals = { builds: 0, scenes: 0, reused: 0, buildMs: 0, maxBuildMs: 0, viewsReceived: 0, viewsCoalesced: 0 };
// Reported (under `build`) only while the cells path is on, so the shape stays the same with it off.
const cellTotals = { cellUpdates: 0, nodeCellMessages: 0, nodeCellBuffers: 0, nodeCellBytes: 0 };

function recordWork(kind, startedAt, endedAt) {
    let entry = workKinds.get(kind);
    if (!entry) {
        entry = { count: 0, busyMs: 0, maxBusyMs: 0 };
        workKinds.set(kind, entry);
    }
    const busy = endedAt - startedAt;
    entry.count += 1;
    entry.busyMs += busy;
    entry.maxBusyMs = Math.max(entry.maxBusyMs, busy);
}

// Range sources of the open dataset (empty for local Blobs): their stats are the `remote` metrics.
let remoteSources = [];

// Real cancellation of a ticket: besides the logical flag, aborting the controller cancels the HTTP
// request(s) that only this ticket needs (`HttpRangeSource.withSignal`). Local Blobs keep the flag only.
function bindTicketAbort(request) {
    const token = request.cancelToken();
    const [file, attributes] = remoteSources;
    if (typeof file?.withSignal !== "function" || typeof request.setSource !== "function") return token;
    const controller = new AbortController();
    request.setSource(file.withSignal(controller.signal), attributes?.withSignal?.(controller.signal));
    return { cancel() { token.cancel(); controller.abort(); } };
}


function metricsSnapshot() {
    return {
        timeOrigin: performance.timeOrigin ?? null,
        busyByMessage: Object.fromEntries(workKinds),
        asyncBlocks: { ...asyncBlockTotals },
        pages: { ...pageTotals },
        remote: sumRangeStats(remoteSources),
        build: { ...buildTotals, ...(cellsEnabled ? cellTotals : {}) },
        ...(cellsEnabled ? { cellStats: builder?.cellStats?.() ?? null } : {}),
        wasmLinearMemoryBytes: wasmExports?.memory?.buffer.byteLength ?? null,
    };
}

function resetMetrics() {
    workKinds.clear();
    for (const source of remoteSources) source?.resetStats?.();
    Object.assign(asyncBlockTotals, {
        blocks: 0, readMs: 0, decodeMs: 0, maxDecodeMs: 0, insertMs: 0, maxInsertMs: 0,
    });
    Object.assign(buildTotals, {
        builds: 0, scenes: 0, reused: 0, buildMs: 0, maxBuildMs: 0, viewsReceived: 0, viewsCoalesced: 0,
    });
    Object.assign(cellTotals, { cellUpdates: 0, nodeCellMessages: 0, nodeCellBuffers: 0, nodeCellBytes: 0 });
}

function clearCellInbox() {
    Object.assign(cellInbox, { driverReplaced: false, poolReset: false, evict: [], poolAck: null, levelRequests: null });
}

// Applies what the render worker reported since the last update, in the builder's required order.
function applyCellInbox() {
    const { driverReplaced, poolReset, evict, poolAck, levelRequests } = cellInbox;
    clearCellInbox();
    const failures = [];
    const apply = (call) => {
        try {
            call();
        } catch (error) {
            failures.push(String(error));
        }
    };
    if (driverReplaced) apply(() => builder.driverReplaced());
    if (poolReset) apply(() => builder.poolReset());
    for (const nodes of evict) apply(() => builder.applyEvict(nodes));
    if (poolAck) apply(() => builder.applyPoolAck(poolAck));
    if (levelRequests) apply(() => builder.applyLevelRequests(levelRequests.nodes, levelRequests.depths));
    if (failures.length > 0) {
        self.postMessage({ type: "error", operation: "build:cellInbox", message: failures.join("; ") });
    }
}

function postCellUpdate(result, view, buildMs) {
    cellTotals.cellUpdates += 1;
    const buffers = result.nodeCells;
    if (buffers.length > 0) {
        cellTotals.nodeCellMessages += 1;
        cellTotals.nodeCellBuffers += buffers.length;
        for (const buffer of buffers) cellTotals.nodeCellBytes += buffer.byteLength;
        // The transfer list is the buffers themselves: nothing is copied.
        self.postMessage({ type: "nodeCells", gen: generation, generation: result.generation, buffers }, buffers);
    }
    const levels = result.nodeLevels ?? [];
    if (levels.length > 0) {
        self.postMessage({ type: "nodeLevels", gen: generation, generation: result.generation, buffers: levels }, levels);
    }
    if (result.levelsRefused) {
        const { nodes, depths } = result.levelsRefused;
        self.postMessage({ type: "levelsRefused", gen: generation, nodes, depths }, [nodes.buffer, depths.buffer]);
    }
    self.postMessage({
        type: "cellCut",
        gen: generation,
        cutEpoch: result.cutEpoch,
        viewEpoch: result.viewEpoch,
        cut: result.cut,
        support: result.support,
        idle: result.idle,
        targetPixels: result.targetPixels,
    });
    self.postMessage({
        type: "cellLod", gen: generation, cutEpoch: result.cutEpoch, viewEpoch: result.viewEpoch, lod: result.lod, buildMs,
    });
}

function postError(operation, error, requestGeneration = generation) {
    self.postMessage({ type: "error", gen: requestGeneration, operation, message: String(error) });
}

function cancelAllAsyncReads() {
    scheduler?.cancelAll();
    for (const token of asyncReadCancelTokens) token.cancel();
    asyncReadCancelTokens.clear();
}

// A single timer per burst: messages queued meanwhile (views, block arrivals) are handled first,
// so one build serves all of them with the latest view.
function requestBuild() {
    if (buildTimer || !builder || !latestView) return;
    buildTimer = setTimeout(runBuild, 0);
}

function runBuild() {
    buildTimer = 0;
    if (!builder || !latestView) return;
    const view = latestView;
    buildTotals.viewsCoalesced += Math.max(0, viewsSinceBuild - 1);
    viewsSinceBuild = 0;
    const startedAt = performance.now();
    let result;
    try {
        if (cellsEnabled) {
            applyCellInbox();
            if (view === lastCellsView && !builder.cellStats?.()?.cellsBusy) return;
            if (typeof view.viewEpoch === "number") builder.setViewEpoch(view.viewEpoch);
            // Dataset units, `null` for none; the cut does not refine into cells below it (RQ-09).
            builder.setMinSide(view.minSide ?? undefined);
        }
        result = builder.update(view.camera, view.viewport[0], view.viewport[1], view.visualScale, view.targetPx);
    } catch (error) {
        recordWork("build", startedAt, performance.now());
        postError("build", error);
        return;
    }
    const buildMs = performance.now() - startedAt;
    recordWork("build", startedAt, startedAt + buildMs);
    buildTotals.builds += 1;
    buildTotals.buildMs += buildMs;
    buildTotals.maxBuildMs = Math.max(buildTotals.maxBuildMs, buildMs);
    lastCellsView = result.nodeCells ? view : null;
    if (result.nodeCells) {
        postCellUpdate(result, view, buildMs);
    } else if (result.sceneBuffer) {
        buildTotals.scenes += 1;
        self.postMessage({
            type: "scene",
            gen: generation,
            epoch: result.epoch,
            viewEpoch: view.epoch,
            buf: result.sceneBuffer,
            count: result.instanceCount,
            blockIds: result.blockIds,
            lod: result.lod,
            digest: result.digest,
            buildMs,
        }, [result.sceneBuffer]);
    } else {
        buildTotals.reused += 1;
        self.postMessage({
            type: "lod", gen: generation, epoch: result.epoch, viewEpoch: view.epoch, lod: result.lod,
        });
    }
    if (retryFailedNext) failedPageAnchors.clear();
    if (pageBudgetBlockedEpoch !== view.epoch) schedulePageReads(result.lod.pendingPageAnchors);
    refreshAsyncReads(retryFailedNext);
    retryFailedNext = false;
    if (result.needsRefinement) requestBuild();
}

// Hierarchy pages of a paged format load like blocks: the LOD names the anchors it lacks.
const pageReads = new Set();
// A page that failed to load waits for a view with `retryFailed` or a new dataset, like failed blocks;
// a page the budget cannot hold waits for the next view instead (`pageBudgetBlockedEpoch`).
const failedPageAnchors = new Set();
function schedulePageReads(anchors) {
    if (!reader || !builder || !anchors?.length) return;
    const taskReader = reader;
    const taskGeneration = generation;
    for (const anchor of anchors) {
        if (pageReads.size >= maxAsyncLodReads) break;
        if (pageReads.has(anchor) || failedPageAnchors.has(anchor)) continue;
        if (pageReads.size === 0) pageTotals.waves += 1;
        pageTotals.reads += 1;
        pageReads.add(anchor);
        void taskReader.loadPage(anchor).then((page) => {
            if (taskGeneration !== generation || taskReader !== reader) {
                page.free();
                return;
            }
            builder.graftPage(page);
            requestBuild();
        }).catch((error) => {
            if (taskGeneration !== generation) return;
            postError("asyncPageRead", error, taskGeneration);
            if (String(error).includes("page budget exhausted")) {
                // Rebuild once so the stats carry `pageBudgetExhausted`.
                pageBudgetBlockedEpoch = latestView?.epoch ?? null;
                requestBuild();
            } else {
                failedPageAnchors.add(anchor);
            }
        }).finally(() => {
            pageReads.delete(anchor);
        });
    }
}

// Applies the planner's priority list to the scheduler, then starts reads for free slots.
function refreshAsyncReads(retryFailed = false) {
    if (!reader || !builder || !scheduler) return;
    const cancelledTickets = scheduler.updateBlocks(builder.pendingAsyncLodBlocks());
    for (const ticket of cancelledTickets) {
        for (const task of asyncReadTasks.values()) {
            if (task.ticket === String(ticket)) task.cancelToken.cancel();
        }
    }
    if (retryFailed) {
        for (const blockId of failedAsyncBlocks) scheduler.retry(blockId);
        failedAsyncBlocks.clear();
    }
    fillAsyncReads();
}

function fillAsyncReads() {
    if (!reader || !builder || !scheduler) return;
    const taskReader = reader;
    const taskGeneration = generation;
    const ticketScheduler = scheduler;
    for (const blockId of ticketScheduler.pendingBlocks()) {
        if (ticketScheduler.inFlightCount() >= maxAsyncLodReads) break;
        if (asyncReadTasks.has(String(blockId))) continue;

        const colorMode = currentColorMode;
        const selectedAttribute = colorMode.startsWith("attribute:")
            ? Number(colorMode.split(":")[1])
            : null;
        const requestedAttribute = selectedAttribute !== null
            ? selectedAttribute
            : colorMode === "intensity" ? asyncAttributeIndices.get("intensity")
                : colorMode === "classification" ? asyncAttributeIndices.get("classification") : null;
        let selection;
        let attributeColumn = requestedAttribute;
        let carriedColumns;
        if (cellsEnabled) {
            // The cells keep intensity and class in every colour mode (R3-Q32), so a shader-mode
            // change never rebuilds them. Only an `attribute:N:M` mode paints its column (R3-Q33).
            selection = "geometryAttributes";
            attributeColumn = asyncHasAttributes ? selectedAttribute : null;
            carriedColumns = asyncHasAttributes
                ? ["intensity", "classification"]
                    .map((name) => asyncAttributeIndices.get(name))
                    .filter((index) => index !== undefined && index !== attributeColumn)
                : [];
        } else {
            const geometrySelection = colorMode === "rgb" && selectedAttribute === null
                ? "geometry" : "positions";
            selection = requestedAttribute !== null && asyncHasAttributes
                ? "positionsAttribute" : geometrySelection;
        }
        let request;
        try {
            request = taskReader.planLodRead(
                BigInt(blockId), selection, attributeColumn, carriedColumns,
            );
        } catch (error) {
            ticketScheduler.fail(String(blockId));
            failedAsyncBlocks.add(String(blockId));
            postError("asyncLodRead", error);
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
        const cancelToken = bindTicketAbort(request);
        asyncReadCancelTokens.add(cancelToken);
        asyncReadTasks.set(String(blockId), { request, cancelToken, ticket });
        void readBlock(taskReader, taskGeneration, ticketScheduler, blockId, ticket, request, cancelToken);
    }
}

async function readBlock(taskReader, taskGeneration, ticketScheduler, blockId, ticket, request, cancelToken) {
    let result = null;
    let ticketCompleted = false;
    const stale = () => taskGeneration !== generation || taskReader !== reader
        || ticketScheduler !== scheduler;
    try {
        result = await request.read();
        const payload = result.value;
        const readMs = result.readMs ?? 0;
        const decodeMs = result.decodeMs ?? 0;
        const insertStartedAt = performance.now();
        if (stale() || !ticketScheduler.isCurrent(ticket)) {
            ticketScheduler.complete(ticket, String(blockId), false);
            ticketCompleted = true;
            return;
        }
        builder.insertAsyncLodBlock(
            blockId, payload.positions, payload.colors ?? null, payload.attribute ?? null,
            payload.attributes ?? null,
        );
        ticketScheduler.complete(ticket, String(blockId), true);
        ticketCompleted = true;
        const insertMs = performance.now() - insertStartedAt;
        recordWork("asyncInsert", insertStartedAt, insertStartedAt + insertMs);
        asyncBlockTotals.blocks += 1;
        asyncBlockTotals.readMs += readMs;
        asyncBlockTotals.decodeMs += decodeMs;
        asyncBlockTotals.maxDecodeMs = Math.max(asyncBlockTotals.maxDecodeMs, decodeMs);
        asyncBlockTotals.insertMs += insertMs;
        asyncBlockTotals.maxInsertMs = Math.max(asyncBlockTotals.maxInsertMs, insertMs);
        requestBuild();
    } catch (error) {
        let outcome = null;
        if (!ticketCompleted) {
            try {
                outcome = ticketScheduler.complete(ticket, String(blockId), false);
            } catch { /* ticket already settled */ }
            ticketCompleted = true;
            if (outcome === "failed" && !isAbortError(error)) failedAsyncBlocks.add(String(blockId));
        }
        // An abort is a cancellation (the ticket left the plan), never a failure to report or retry.
        if (!stale() && outcome !== "discarded" && !isAbortError(error)) postError("asyncLodRead", error, taskGeneration);
    } finally {
        result?.release?.();
        result?.free?.();
        request.free();
        asyncReadCancelTokens.delete(cancelToken);
        asyncReadTasks.delete(String(blockId));
        if (!stale()) fillAsyncReads();
    }
}

async function openDataset(data) {
    cancelAllAsyncReads();
    if (buildTimer) clearTimeout(buildTimer);
    buildTimer = 0;
    builder?.free?.();
    reader?.free?.();
    builder = null;
    reader = null;
    latestView = null;
    lastCellsView = null;
    viewsSinceBuild = 0;
    failedAsyncBlocks.clear();
    failedPageAnchors.clear();
    generation = data.gen;
    cellsEnabled = false;
    clearCellInbox();
    pageBudgetBlockedEpoch = null;
    currentColorMode = "rgb";
    asyncHasAttributes = false;
    asyncAttributeIndices = new Map();
    const requestGeneration = generation;
    const [maxVoxels, maxEstimatedBytes] = data.limits;
    maxAsyncLodReads = readConcurrency(data.file);
    const nextScheduler = new WasmBlockRequestScheduler(maxAsyncLodReads);
    scheduler = nextScheduler;
    // One budget per worker, shared by successive readers; a different cacheBytes replaces it.
    if (readBudget === null || readBudget.limitBytes() !== String(data.cacheBytes)) {
        const previous = readBudget;
        readBudget = new WasmAsyncReadBudget(data.cacheBytes);
        previous?.free();
    }
    // A remote file is `{ url }`: this worker opens its own range source (it cannot be cloned).
    const [file, attributes] = await Promise.all([
        resolveSource(data.file),
        resolveSource(data.attributes, { tailBytes: 40, headBytes: 24 }),
    ]);
    const nextReader = await WasmAsyncFormatReader.open(file, attributes, readBudget.clone());
    if (requestGeneration !== generation) {
        nextReader.free();
        return;
    }
    remoteSources = [file, attributes]; // only for the current open: a superseded one must not replace them
    const nextBuilder = new WasmSceneBuilder(maxVoxels, maxEstimatedBytes);
    if (data.cells) {
        nextBuilder.enableCells(true);
        if (data.cellPoolPages > 0) nextBuilder.setCellPoolPages(data.cellPoolPages);
    }
    const descriptor = nextReader.lodDatasetDescriptor(data.cacheBytes);
    const result = descriptor.paged
        ? nextBuilder.openPagedLodDataset(nextReader, data.cacheBytes)
        : nextBuilder.openLodDataset(descriptor);
    const opened = {
        ...result,
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
        crsUnit: descriptor.crsUnit ?? null,
        // The file's p5 in dataset units (ADR 0059); null without cells, without p5 or when unmeasured.
        recommendedMinSize: result.cellsInfo?.recommendedMinSize ?? null,
    };
    builder = nextBuilder;
    reader = nextReader;
    cellsEnabled = opened.cellPath === "enabled";
    asyncHasAttributes = descriptor.hasAttributes;
    asyncAttributeIndices = new Map(descriptor.attributeSchema.map(
        (attribute, index) => [attribute.name, index],
    ));
    // `fileBytes`: the size the main source reported on open (a remote source has no size until then).
    self.postMessage({ type: "opened", gen: requestGeneration, opened, fileBytes: file.size ?? null });
}

async function handleMessage({ data }) {
    const startedAt = performance.now();
    try {
        switch (data.type) {
            case "init":
                wasmExports = await init(data.wasmBytes ? { module_or_path: data.wasmBytes } : undefined);
                break;
            case "open":
                await openDataset(data);
                break;
            case "view":
                if (data.gen !== generation) break;
                latestView = data;
                viewsSinceBuild += 1;
                retryFailedNext ||= Boolean(data.retryFailed);
                buildTotals.viewsReceived += 1;
                requestBuild();
                break;
            case "poolAck":
            case "evict":
            case "levelRequests":
            case "poolReset":
            case "driverReplaced":
                if (data.gen !== generation || !builder || !cellsEnabled) break;
                if (data.type === "poolAck") {
                    cellInbox.poolAck = { cutEpoch: data.cutEpoch, drawn: data.drawn, missing: data.missing };
                } else if (data.type === "evict") {
                    cellInbox.evict.push(data.nodes);
                } else if (data.type === "levelRequests") {
                    cellInbox.levelRequests = { nodes: data.nodes, depths: data.depths };
                } else {
                    cellInbox[data.type] = true;
                }
                requestBuild();
                break;
            case "colorMode":
                if (data.gen !== generation || !builder) break;
                builder.setColorMode(data.mode);
                lastCellsView = null; // the next build always runs: an attribute mode builds the nodes anew
                if (data.mode !== currentColorMode) cancelAllAsyncReads();
                currentColorMode = data.mode;
                self.postMessage({
                    type: "colorMode", gen: generation, mode: data.mode,
                    attributeRange: builder.attributeRange?.() ?? null,
                });
                refreshAsyncReads(true);
                requestBuild();
                break;
            case "metrics":
                if (data.reset) resetMetrics();
                else self.postMessage({ type: "metrics", metrics: metricsSnapshot() });
                break;
            default:
                throw new Error(`unknown build worker message: ${data.type}`);
        }
    } catch (error) {
        postError(data.type === "open" ? "openFile" : data.type, error, data.gen ?? generation);
    } finally {
        if (data.type !== "metrics") recordWork(data.type, startedAt, performance.now());
    }
}

let messageQueue = Promise.resolve();
self.addEventListener("message", (event) => {
    messageQueue = messageQueue.then(() => handleMessage(event), () => handleMessage(event));
});
