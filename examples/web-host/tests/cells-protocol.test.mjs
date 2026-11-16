import { readConcurrency } from "../http-range-source.mjs";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import { test } from "node:test";

import { lodStillLoading } from "../benchmark-metrics.mjs";

// R3 Task 10c1: the cells protocol between the render worker and the build worker, with fake wasm
// classes. Both workers run in `vm` exactly as their files are shipped (no module imports).
const stripImports = source => source.replace(/^import .*\n/gm, "");
const viewerSource = stripImports(await readFile(new URL("../viewer-worker.js", import.meta.url), "utf8"));
const buildSource = stripImports(await readFile(new URL("../build-worker.js", import.meta.url), "utf8"));
const settle = async () => {
    await new Promise(setImmediate);
    await new Promise(setImmediate);
};
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

const lod = (extra = {}) => ({
    visiblePoints: "10", coverageComplete: true, pendingBlockIds: [], pendingPageAnchors: [],
    lodStagePixels: 4, targetVoxelPixels: 4, visibleBlocks: 1, lodError: null, ...extra,
});
const cellsInfo = () => ({
    generation: 3, origin: new Float64Array(3), rootMin: new Float64Array([0, 0, 0]), rootSide: 8, referenceSpacing: 0.25,
});
const opened = (cellPath = "enabled") => ({
    status: "metadata_ready", visualScale: 1, nodeCount: 1, maxLevel: 0,
    coordinateOrigin: new Float64Array(3), boundsMin: new Float64Array(3), boundsMax: new Float64Array([1, 1, 1]),
    semantics: "replacement", attributeSchema: [], hasAttributes: false, sourcePointCount: 1,
    cellPath, cellsInfo: cellPath === "enabled" ? cellsInfo() : null,
});

// The key set of `add_cells_stats` (crates/viewer_wasm/src/bindings/cells.rs), with its value types.
const REAL_CELL_STATS_KEYS = [
    "cellPath", "cellsBusy", "cellsNeedFrame", "cutDigestAtRest", "viewEpoch", "cutEpochSent", "cutEpochApplied",
    "queuedNodeCells", "queuedNodeBytes", "cellCpuBytes", "poolCapacityPages", "poolStages", "poolNodesWithoutLevel", "poolCoarsened",
    "poolOverflow", "poolResidentPages", "poolUploadedBytes", "wasmLinearMemoryBytes", "poolDescendantTooFine",
];
const realCellStats = (overrides = {}) => ({
    cellPath: "enabled", cellsBusy: false, cellsNeedFrame: false, cutDigestAtRest: null, viewEpoch: 0,
    cutEpochSent: 0, cutEpochApplied: 0, queuedNodeCells: 0, queuedNodeBytes: 0, cellCpuBytes: 0, poolCapacityPages: 65536, poolStages: 0,
    poolNodesWithoutLevel: 0, poolCoarsened: 0, poolOverflow: 0, poolResidentPages: 0, poolUploadedBytes: 0,
    wasmLinearMemoryBytes: 65536, poolDescendantTooFine: false, ...overrides,
});
const NOT_ENABLED = "the cells path is not enabled";

// ---- Build worker --------------------------------------------------------------------------

async function runBuildWorker({ cells = true, nodeBuffers = () => [new ArrayBuffer(4), new ArrayBuffer(6)],
    levelBuffers = () => [], refine = 0, cellPoolPages = 0, beforeOpened = null } = {}) {
    const posted = [];
    const transfers = new Map();
    const calls = [];
    const updates = [];
    let receive;
    class WasmSceneBuilder {
        enableCells(on) { calls.push(["enableCells", on]); this.cellsOn = on; }
        setCellPoolPages(pages) { calls.push(["setCellPoolPages", pages]); }
        openLodDataset() {
            calls.push(["open"]);
            return { status: "metadata_ready", visualScale: 2, ...(this.cellsOn ? { cellPath: "enabled", cellsInfo: cellsInfo() } : { cellPath: "off" }) };
        }
        pendingAsyncLodBlocks() { return []; }
        // Like the real builder: an attribute mode starts a new colour generation (the nodes are
        // built again), a shader mode builds nothing.
        setColorMode(mode) {
            calls.push(["setColorMode", mode]);
            this.mode = mode;
            this.recolor = mode.startsWith("attribute:");
        }
        attributeRange() { return null; }
        // Like the real builder: every cells method throws while the cells path is off.
        need() { if (!this.cellsOn) throw new Error(NOT_ENABLED); }
        setViewEpoch(epoch) { this.need(); calls.push(["setViewEpoch", epoch]); this.viewEpoch = epoch; }
        driverReplaced() { this.need(); calls.push(["driverReplaced"]); }
        poolReset() { this.need(); calls.push(["poolReset"]); }
        applyEvict(nodes) {
            this.need();
            calls.push(["applyEvict", [...nodes]]);
            if ([...nodes].includes(99n)) throw new Error("bad node 99");
        }
        applyPoolAck(ack) { this.need(); calls.push(["applyPoolAck", ack.cutEpoch]); }
        applyLevelRequests(nodes, depths) { this.need(); calls.push(["applyLevelRequests", [...nodes], [...depths]]); this.levelsDirty = true; }
        // `cellsBusy` is the builder's dirty flag: true while more updates are worth running.
        cellStats() { return this.cellsOn ? realCellStats({ cellsBusy: updates.length <= refine || Boolean(this.recolor) || Boolean(this.levelsDirty) }) : null; }
        update() {
            calls.push(["update"]);
            updates.push(updates.length + 1);
            this.levelsDirty = false;
            const n = updates.length;
            if (this.cellsOn) {
                const rebuilt = this.recolor || this.mode === undefined || this.mode === "rgb";
                this.recolor = false;
                return {
                    epoch: n, generation: 3, cutEpoch: n, viewEpoch: this.viewEpoch, cut: new BigUint64Array([5n]),
                    support: new BigUint64Array(0), idle: false, stagePixels: 8, targetPixels: 4,
                    nodeCells: rebuilt ? nodeBuffers(n) : [], nodeLevels: levelBuffers(n), needsRefinement: n <= refine, sceneBuffer: null, instanceCount: 0,
                    blockIds: [], digest: null, lod: lod({ cellsBusy: true }),
                };
            }
            return {
                epoch: n, sceneBuffer: new ArrayBuffer(8), instanceCount: 1, blockIds: ["7"], digest: "00",
                needsRefinement: false, lod: lod(),
            };
        }
        free() {}
    }
    class Reader {
        static async open() { return new Reader(); }
        lodDatasetDescriptor() {
            return {
                coordinateOrigin: [0, 0, 0], semantics: "replacement", hasAttributes: false, attributeSchema: [],
                dataMin: [0, 0, 0], dataMax: [1, 1, 1], nodeCount: 1, maxLevel: 0, paged: false,
            };
        }
        free() {}
    }
    class Scheduler { updateBlocks() { return []; } pendingBlocks() { return []; } cancelAll() { return []; } }
    vm.runInNewContext(buildSource, {
        self: {
            addEventListener(_name, callback) { receive = callback; },
            postMessage(message, transfer) { posted.push(message); if (transfer) transfers.set(message, transfer); },
        },
        performance: { now: () => Date.now() },
        resolveSource: async spec => spec,
        sumRangeStats: () => null,
        readConcurrency,
        isAbortError: (error) => error?.name === "AbortError",
        init: async () => ({ memory: new WebAssembly.Memory({ initial: 1 }) }),
        WasmSceneBuilder, WasmAsyncFormatReader: Reader, WasmAsyncReadBudget: class { clone() { return this; } },
        WasmBlockRequestScheduler: Scheduler, Float64Array, setTimeout, clearTimeout,
    });
    const send = (type, fields = {}) => receive({ data: { type, ...fields } });
    send("init", {});
    send("open", { gen: 1, file: {}, cacheBytes: 1024, limits: [100, 200], cells, cellPoolPages });
    beforeOpened?.(send);
    await settle();
    const view = (fields = {}) => send("view", {
        gen: 1, epoch: 1, viewEpoch: 7, camera: [1], viewport: [64, 64], visualScale: 2, targetPx: 4, ...fields,
    });
    const sync = async () => { await settle(); await sleep(30); };
    return { posted, transfers, calls, send, view, sync, settle };
}

const names = calls => calls.map(call => call[0]);

test("build worker enables cells before opening only when asked and reports the path", async () => {
    const on = await runBuildWorker();
    assert.deepEqual(names(on.calls).slice(0, 2), ["enableCells", "open"]);
    assert.equal(on.posted.find(item => item.type === "opened").opened.cellPath, "enabled");
    assert.equal(names(on.calls).includes("setCellPoolPages"), false, "no pool size, no call");
    const sized = await runBuildWorker({ cellPoolPages: 49152 });
    assert.deepEqual(sized.calls.slice(0, 3), [["enableCells", true], ["setCellPoolPages", 49152], ["open"]]);
    const off = await runBuildWorker({ cells: false });
    assert.equal(names(off.calls).includes("enableCells"), false);
    off.view();
    await off.sync();
    assert.equal(off.posted.filter(item => item.type === "scene").length, 1);
    assert.equal(off.posted.some(item => item.type === "nodeCells"), false);
});

test("nodeCells are posted in order, never coalesced, with their buffers as the transfer list", async () => {
    const batches = [];
    const worker = await runBuildWorker({
        refine: 2, nodeBuffers: (n) => { const list = [new ArrayBuffer(n), new ArrayBuffer(n + 10)]; batches.push(list); return list; },
    });
    worker.view();
    await worker.sync();
    await worker.sync();
    const nodeCells = worker.posted.filter(item => item.type === "nodeCells");
    assert.equal(nodeCells.length, 3, "one message per update, none merged");
    nodeCells.forEach((message, index) => {
        assert.equal(message.gen, 1);
        assert.equal(message.generation, 3);
        assert.deepEqual(message.buffers, batches[index]);
        const transfer = worker.transfers.get(message);
        assert.equal(transfer.length, 2);
        message.buffers.forEach((buffer, i) => assert.equal(transfer[i], buffer, "transferred, not copied"));
    });
    assert.deepEqual(worker.posted.map(item => item.type).filter(type => type !== "opened"),
        ["nodeCells", "cellCut", "cellLod", "nodeCells", "cellCut", "cellLod", "nodeCells", "cellCut", "cellLod"]);
    assert.equal(worker.posted.some(item => item.type === "scene" || item.type === "lod"), false, "no scene in cells mode");
});

test("an update without new nodes still sends the cut but no empty nodeCells", async () => {
    const worker = await runBuildWorker({ nodeBuffers: () => [] });
    worker.view();
    await worker.sync();
    assert.equal(worker.posted.some(item => item.type === "nodeCells"), false);
    assert.equal(worker.posted.filter(item => item.type === "cellCut").length, 1);
});

test("viewEpoch goes to the builder before update and comes back in cellCut and cellLod", async () => {
    const worker = await runBuildWorker();
    worker.view({ viewEpoch: 41 });
    await worker.sync();
    assert.deepEqual(worker.calls.slice(-2), [["setViewEpoch", 41], ["update"]]);
    const cut = worker.posted.find(item => item.type === "cellCut");
    assert.deepEqual([cut.cutEpoch, cut.viewEpoch, cut.idle, cut.targetPixels], [1, 41, false, 4]);
    assert.deepEqual([...cut.cut], [5n]);
    assert.equal(worker.posted.find(item => item.type === "cellLod").viewEpoch, 41);
});

test("render reports are applied before the next update in the builder's required order", async () => {
    const worker = await runBuildWorker();
    worker.send("poolAck", { gen: 1, cutEpoch: 1, drawn: new BigUint64Array(0), missing: new BigUint64Array(0) });
    worker.send("evict", { gen: 1, nodes: new BigUint64Array([1n, 2n]) });
    worker.send("poolAck", { gen: 1, cutEpoch: 2, drawn: new BigUint64Array(0), missing: new BigUint64Array(0) });
    worker.send("poolReset", { gen: 1 });
    worker.send("evict", { gen: 1, nodes: new BigUint64Array([3n]) });
    worker.send("driverReplaced", { gen: 1 });
    await worker.settle();
    assert.equal(names(worker.calls).includes("update"), false, "nothing builds without a view");
    worker.view();
    await worker.sync();
    const applied = worker.calls.slice(worker.calls.findIndex(call => call[0] === "driverReplaced"));
    assert.deepEqual(applied, [
        ["driverReplaced"], ["poolReset"], ["applyEvict", [1n, 2n]], ["applyEvict", [3n]],
        ["applyPoolAck", 2], ["setViewEpoch", 7], ["update"],
    ], "evictions stay separate and in order, only the last ack is applied");
    // The inbox is empty afterwards.
    worker.view({ epoch: 2 });
    await worker.sync();
    assert.deepEqual(worker.calls.slice(-2), [["setViewEpoch", 7], ["update"]]);
});

test("a render report on a settled view does not replan: the ack/cut exchange ends", async () => {
    const worker = await runBuildWorker();
    worker.view();
    await worker.sync();
    const cuts = () => worker.posted.filter(item => item.type === "cellCut").length;
    assert.equal(cuts(), 1);
    for (const cutEpoch of [1, 2, 3]) {
        worker.send("poolAck", { gen: 1, cutEpoch, drawn: new BigUint64Array(0), missing: new BigUint64Array(0) });
        await worker.sync();
    }
    assert.equal(cuts(), 1, "acks alone send no cut");
    assert.equal(names(worker.calls).filter(name => name === "update").length, 1);
    worker.view({ epoch: 2 });
    await worker.sync();
    assert.equal(cuts(), 2, "a new view still plans");
});

test("render reports for another gen, or without a cells dataset, are ignored", async () => {
    const worker = await runBuildWorker();
    for (const type of ["poolReset", "driverReplaced"]) worker.send(type, { gen: 9 });
    worker.send("evict", { gen: 9, nodes: new BigUint64Array([1n]) });
    worker.send("poolAck", { gen: 9, cutEpoch: 1 });
    worker.view();
    await worker.sync();
    assert.deepEqual(names(worker.calls).filter(name => name.startsWith("apply") || name.endsWith("Reset") || name === "driverReplaced"), []);
    const plain = await runBuildWorker({ cells: false });
    plain.send("poolReset", { gen: 1 });
    plain.send("driverReplaced", { gen: 1 });
    plain.view();
    await plain.sync();
    assert.deepEqual(names(plain.calls).filter(name => name === "poolReset" || name === "driverReplaced"), []);
});

test("every colour mode stays on the cells path; only an attribute mode builds nodes again", async () => {
    const worker = await runBuildWorker();
    worker.view();
    await worker.sync();
    const cuts = () => worker.posted.filter(item => item.type === "cellCut").length;
    const nodeMessages = () => worker.posted.filter(item => item.type === "nodeCells").length;
    assert.equal(cuts(), 1);
    const base = nodeMessages();
    // Shader mode: the cut is sent again (the view is replanned), no node is built.
    worker.send("colorMode", { gen: 1, mode: "intensity" });
    await worker.sync();
    assert.equal(nodeMessages(), base, "a shader mode builds no node");
    assert.equal(cuts(), 2);
    // Attribute mode: new colour generation, the nodes are built again.
    worker.send("colorMode", { gen: 1, mode: "attribute:0:1" });
    await worker.sync();
    assert.equal(nodeMessages(), base + 1, "an attribute mode builds the nodes anew");
    worker.send("colorMode", { gen: 1, mode: "rgb" });
    await worker.sync();
    assert.equal(worker.posted.filter(item => item.type === "scene").length, 0, "no CPU scene in any mode");
    const order = worker.posted.map(item => item.type).filter(type => type === "scene" || type === "colorMode");
    assert.deepEqual(order, ["colorMode", "colorMode", "colorMode"]);
});

// ---- Render worker -------------------------------------------------------------------------

async function runRenderWorker({ buildWorker = { url: "build-worker.js", type: "module" }, cells = true,
    cellPath = "enabled", cellNodeBytesPerFrame, failSpawn = false, poolPages = 65536, failCreates = 0, canvasReplacement = false } = {}) {
    const messages = [];
    const workers = [];
    const log = [];
    const errors = [];
    const warnings = [];
    const uploads = [];
    let receive;
    let viewEpoch = 10;
    let camera = [0, 0, 0, 10, 0, 0, 1, 0, 0, 0, 1];
    let renderFrames = 0;
    let pendingMessages = null;
    let cellStats = realCellStats({ viewEpoch: 10 });
    let cellsOn = false;
    let loss = null;
    let lossPolls = 0;
    let samples = 0;
    let creates = 0;
    const canvasesUsed = [];
    let failuresLeft = failCreates;
    const pose = { position: [1, 2, 3], target: [4, 5, 6] };
    const cpuLodStats = buildWorker ? null : { cellPath: "off", sceneDigest: "cpu" };
    const viewer = {
        resize() {}, setColorMode() {},
        setPresentationColorMode(mode) { log.push(["setPresentationColorMode", mode]); },
        setDepthShading() {},
        fitScene() {}, pan() {}, zoom() {}, moveCamera() {},
        orbit() { viewEpoch += 1; camera = camera.map((value, index) => (index === 4 ? value + 0.1 : value)); },
        voxelBuildLimits: () => [500, 600],
        cameraTransport: () => new Float64Array(camera),
        viewEpoch: () => viewEpoch,
        viewportSize: () => [100, 50],
        beginRemoteDataset() { return true; },
        uploadSceneInstances(bytes) { uploads.push(bytes); return bytes.length; },
        lodStats: () => cpuLodStats,
        renderFrame() { renderFrames += 1; return "rendered"; },
        gpuStats: () => ({}), stats: () => ({}),
        enableCells(on) { log.push(["enableCells", on]); },
        setCellNodeBytesPerFrame(bytes) { log.push(["setCellNodeBytesPerFrame", bytes]); },
        cellPoolPages() { return poolPages; },
        setContinuity(enabled, factor) { log.push(["setContinuity", enabled, factor]); },
        beginRemoteCells(origin, rootMin, rootSide, referenceSpacing) {
            log.push(["beginRemoteCells", rootSide]);
            log.push(["referenceSpacing", referenceSpacing]);
            cellsOn = cellPath === "enabled";
            return cellPath;
        },
        addCellNode(generation, bytes) {
            if (!cellsOn) throw new Error(NOT_ENABLED);
            if (bytes.length === 0) throw new Error("malformed nodeCells entry: empty node");
            log.push(["addCellNode", generation, bytes.length]);
        },
        setCellCut(message) {
            if (!cellsOn) throw new Error(NOT_ENABLED);
            if (!(message.targetPixels >= 0.5 && message.targetPixels <= 16)) {
                throw new Error("targetPixels must be finite and between 0.5 and 16");
            }
            log.push(["setCellCut", message.cutEpoch, message.viewEpoch]);
        },
        addCellLevels(generation, bytes) {
            if (!cellsOn) throw new Error(NOT_ENABLED);
            if (bytes.length === 0) throw new Error("malformed nodeLevels entry: empty node");
            log.push(["addCellLevels", generation, bytes.length]);
        },
        takeCellMessages() {
            if (!cellsOn) throw new Error(NOT_ENABLED);
            const taken = pendingMessages ?? { poolAck: null, evict: new BigUint64Array(0), poolReset: false, driverReplaced: false };
            pendingMessages = null;
            return taken;
        },
        cellStats: () => cellStats,
        deviceLost: () => { lossPolls += 1; return loss; },
        replacePoints() { samples += 1; },
        cameraPose: () => pose,
        setCameraPose(position, target) { log.push(["setCameraPose", [...position], [...target]]); },
        setGpuFrameTiming() {},
        free() { log.push(["free"]); },
    };
    class FakeWorker {
        constructor(url) {
            if (failSpawn) throw new Error("nested workers refused");
            this.url = url; this.posted = []; this.transfers = new Map(); this.terminated = false;
            this.listeners = []; this.errorListeners = [];
            workers.push(this);
        }
        addEventListener(type, callback) {
            if (type === "message") this.listeners.push(callback);
            if (type === "error") this.errorListeners.push(callback);
        }
        postMessage(message, transfer) {
            this.posted.push(message);
            if (transfer) this.transfers.set(message, transfer);
        }
        terminate() { this.terminated = true; }
        emit(data) { for (const callback of this.listeners) callback({ data }); }
    }
    vm.runInNewContext(viewerSource, {
        self: {
            addEventListener(_name, callback) { receive = callback; },
            postMessage(message) { messages.push(message); },
        },
        performance: { now: () => 0 },
        resolveSource: async spec => spec,
        sumRangeStats: () => null,
        readConcurrency,
        isAbortError: (error) => error?.name === "AbortError",
        init: async () => ({ memory: new WebAssembly.Memory({ initial: 1 }) }),
        WasmVoxelViewer: {
            createOffscreen: async (canvas) => {
                creates += 1;
                canvasesUsed.push(canvas.id);
                if (creates > 1 && failuresLeft > 0) {
                    failuresLeft -= 1;
                    throw new Error("no adapter");
                }
                loss = null;
                cellsOn = false;
                // A new driver reports itself to the builder on its first take.
                if (creates > 1) pendingMessages = { poolAck: null, evict: new BigUint64Array(0), poolReset: false, driverReplaced: true };
                return viewer;
            },
        },
        WasmAsyncReadBudget: class {}, WasmBlockRequestScheduler: class {}, WasmAsyncFormatReader: class {},
        Worker: FakeWorker, Float64Array, Uint8Array, BigUint64Array, setTimeout, clearTimeout,
        console: { warn: message => warnings.push(message), error: message => errors.push(message) },
    });
    const send = async (type, fields = {}) => {
        receive({ data: { type, ...fields } });
        await settle();
    };
    await send("init", { canvas: { id: "old" }, width: 100, height: 50, colorMode: "rgb", depthShading: false, buildWorker, cells, cellNodeBytesPerFrame, canvasReplacement, deviceRecoveryBackoffMs: [1, 1, 1] });
    const frame = (fields = {}) => send("frame", {
        camera: { orbitX: 0, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0, ...fields },
    });
    const open = () => send("openFile", { file: { name: "a.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4 });
    // Opens the dataset and answers as a build worker that enabled cells.
    const openCells = async () => {
        await open();
        const worker = workers.at(-1);
        worker.emit({ type: "opened", gen: 1, opened: opened("enabled") });
        await settle();
        return worker;
    };
    // The render worker waits for the build worker's metrics before it answers.
    const metrics = async () => {
        await send("metrics");
        workers.at(-1).emit({ type: "metrics", metrics: { asyncBlocks: {}, build: {}, wasmLinearMemoryBytes: 1 } });
        await settle();
        return messages.filter(item => item.type === "metrics").at(-1).workerMetrics;
    };
    return {
        messages, workers, log, errors, warnings, uploads, send, open, openCells, frame, metrics,
        setPending(value) { pendingMessages = value; },
        loseDevice(reason = "unknown", message = "gpu process crashed") { loss = { reason, message }; },
        creates: () => creates,
        lossPolls: () => lossPolls,
        samples: () => samples,
        canvasesUsed,
        setCellStats(value) { cellStats = { ...cellStats, ...value }; },
        renders: () => renderFrames,
        calls: name => log.filter(entry => entry[0] === name),
        sent: (worker, ...types) => worker.posted.filter(item => types.includes(item.type)),
    };
}

const nodeCellsMessage = (fields = {}) => ({
    type: "nodeCells", gen: 1, generation: 3, buffers: [new Uint8Array([1, 2, 3]).buffer], ...fields,
});
const cellCutMessage = (cutEpoch, fields = {}) => ({
    type: "cellCut", gen: 1, cutEpoch, viewEpoch: 10, cut: new BigUint64Array([1n]), support: new BigUint64Array(0),
    idle: false, targetPixels: 4, ...fields,
});

test("cells=1 reaches both workers and the render worker starts its driver from the builder's info", async () => {
    const render = await runRenderWorker();
    assert.deepEqual(render.log[0], ["enableCells", true]);
    const [worker] = render.workers;
    assert.equal(worker.posted[0].cells, true, "init of the build worker");
    const builder = await render.openCells();
    assert.equal(builder.posted.find(item => item.type === "open").cells, true);
    assert.deepEqual(render.calls("beginRemoteCells"), [["beginRemoteCells", 8]]);
    const off = await runRenderWorker({ cells: false });
    assert.equal(off.calls("enableCells").length, 0);
    assert.equal(off.workers[0].posted[0].cells, false);
});

test("the continuity command reaches the viewer, and the driver starts with the cloud's reference spacing", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    assert.deepEqual(render.calls("referenceSpacing"), [["referenceSpacing", 0.25]]);
    await render.send("continuity", { enabled: true });
    await render.send("continuity", { enabled: false, factor: 2 });
    assert.deepEqual(render.calls("setContinuity"), [["setContinuity", true, undefined], ["setContinuity", false, 2]]);
    assert.equal(render.sent(worker, "nodeCells", "open").length, render.sent(worker, "open").length, "no dataset work");
    assert.deepEqual(render.errors, []);
});

test("every open sends the build worker the render side's pool pages, also on a respawned worker", async () => {
    const render = await runRenderWorker({ poolPages: 49152 });
    const first = await render.openCells();
    assert.equal(first.posted.find(item => item.type === "open").cellPoolPages, 49152);
    // A second open replaces the used worker with a fresh one that is told again.
    const second = await render.openCells();
    assert.notEqual(second, first);
    assert.equal(second.posted.find(item => item.type === "open").cellPoolPages, 49152);
    // Cells off: nothing to size.
    const off = await runRenderWorker({ cells: false });
    await off.open();
    assert.equal(off.workers.at(-1).posted.find(item => item.type === "open").cellPoolPages, 0);
});

test("cells without a build worker stay off: CPU path, `cellPath` reports needsBuildWorker (R3-Q24)", async () => {
    const render = await runRenderWorker({ buildWorker: null });
    assert.equal(render.calls("enableCells").length, 0, "no in-process cells");
    assert.equal(render.workers.length, 0);
    await render.frame();
    assert.equal(render.renders(), 1);
    // This fake's frame fails after the draw; the worker first waits for a device-loss report.
    await new Promise(resolve => setTimeout(resolve, 450));
    await render.send("metrics");
    const workerMetrics = render.messages.filter(item => item.type === "metrics").at(-1).workerMetrics;
    assert.equal(workerMetrics.cellPath, "needsBuildWorker");
    assert.equal(workerMetrics.sceneDigest, "cpu");
    assert.equal(workerMetrics.cellStats, undefined, "no cells snapshot");
});

test("a build worker that cannot be created turns the cells off with the same report", async () => {
    const render = await runRenderWorker({ cells: true, failSpawn: true });
    assert.deepEqual(render.calls("enableCells"), [["enableCells", true], ["enableCells", false]]);
    await render.send("metrics");
    assert.equal(render.messages.filter(item => item.type === "metrics").at(-1).workerMetrics.cellPath, "needsBuildWorker");
});

test("nodeCells are all applied in order, outside the epoch filter, and never coalesced", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    worker.emit(cellLodMessage(50));
    // Several arrive before the queue runs; they carry no epoch and none may be dropped.
    for (const size of [3, 4, 5]) worker.emit(nodeCellsMessage({ buffers: [new Uint8Array(size).buffer] }));
    worker.emit(nodeCellsMessage({ buffers: [new Uint8Array(6).buffer, new Uint8Array(7).buffer], generation: 4 }));
    await settle();
    assert.deepEqual(render.calls("addCellNode"), [
        ["addCellNode", 3, 3], ["addCellNode", 3, 4], ["addCellNode", 3, 5],
        ["addCellNode", 4, 6], ["addCellNode", 4, 7],
    ]);
});

const cellLodMessage = (cutEpoch, fields = {}) => ({
    type: "cellLod", gen: 1, cutEpoch, viewEpoch: 10, lod: lod({ cellsBusy: true }), ...fields,
});

test("cellCut: the last of a run wins and the cut keeps its place after the nodes", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    worker.emit(cellCutMessage(1));
    worker.emit(nodeCellsMessage());
    worker.emit(cellCutMessage(2));
    worker.emit(cellCutMessage(3, { viewEpoch: 12 }));
    await settle();
    assert.deepEqual(render.log.filter(entry => entry[0] === "setCellCut" || entry[0] === "addCellNode"),
        [["addCellNode", 3, 3], ["setCellCut", 3, 12]], "superseded cuts are not applied; the viewEpoch is the cut's own");
    worker.emit(cellCutMessage(4));
    await settle();
    assert.equal(render.calls("setCellCut").length, 2, "a later cut is applied again");
});

test("messages of another dataset's gen are dropped", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    worker.emit(nodeCellsMessage({ gen: 9 }));
    worker.emit(cellCutMessage(1, { gen: 9 }));
    worker.emit(cellLodMessage(1, { gen: 9 }));
    await settle();
    assert.equal(render.calls("addCellNode").length, 0);
    assert.equal(render.calls("setCellCut").length, 0);
    assert.equal(render.messages.some(item => item.type === "lod" || item.type === "dataset"), false);
    // Positive control: the same messages with the current gen are applied by this same worker.
    worker.emit(nodeCellsMessage());
    worker.emit(cellCutMessage(1));
    worker.emit(cellLodMessage(1));
    await settle();
    assert.equal(render.calls("addCellNode").length, 1);
    assert.equal(render.calls("setCellCut").length, 1);
    assert.equal(render.messages.some(item => item.type === "lod" || item.type === "dataset"), true);
});

test("after a frame the render worker forwards the driver's reports in the required order, uncoalesced", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    // The first take after beginRemoteCells carries driverReplaced in the real viewer; here a frame does.
    const evictA = new BigUint64Array([1n, 2n]);
    render.setPending({
        poolAck: { cutEpoch: 4, drawn: new BigUint64Array([1n]), missing: new BigUint64Array(0) },
        evict: evictA, poolReset: true, driverReplaced: true,
    });
    await render.frame();
    const evictB = new BigUint64Array([9n]);
    render.setPending({ poolAck: null, evict: evictB, poolReset: false, driverReplaced: false });
    await render.frame();
    const reports = render.sent(worker, "driverReplaced", "poolReset", "evict", "poolAck");
    assert.deepEqual(reports.map(item => item.type), ["driverReplaced", "poolReset", "evict", "poolAck", "evict"]);
    assert.ok(reports.every(item => item.gen === 1));
    assert.deepEqual([...reports[2].nodes], [1n, 2n]);
    assert.deepEqual([...reports[4].nodes], [9n]);
    assert.equal(worker.transfers.get(reports[2])[0], reports[2].nodes.buffer, "evict ids are transferred");
    assert.equal(reports[3].cutEpoch, 4);
    assert.deepEqual([...reports[3].drawn], [1n]);
    // Nothing owed, nothing sent.
    await render.frame();
    assert.equal(render.sent(worker, "driverReplaced", "poolReset", "evict", "poolAck").length, 5);
});

test("the driver's first report (driverReplaced) is forwarded as soon as the cells begin", async () => {
    const render = await runRenderWorker();
    render.setPending({ poolAck: null, evict: new BigUint64Array(0), poolReset: false, driverReplaced: true });
    const worker = await render.openCells();
    assert.deepEqual(render.sent(worker, "driverReplaced").map(item => item.gen), [1]);
});

test("the view carries the viewer's epoch read with the camera", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    const views = () => render.sent(worker, "view");
    assert.equal(views()[0].viewEpoch, 10);
    await render.frame({ orbitX: 1 });
    assert.equal(views().length, 2);
    assert.equal(views()[1].viewEpoch, 11);
    assert.ok(views()[1].epoch > views()[0].epoch, "the worker's own epoch is separate");
    const plain = await runRenderWorker({ cells: false });
    const other = await plain.open().then(() => plain.workers[0]);
    other.emit({ type: "opened", gen: 1, opened: opened("off") });
    await settle();
    assert.equal(plain.sent(other, "view")[0].viewEpoch, undefined);
});

test("a scene in cells mode is reported and not applied, in any colour mode", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    const scene = (epoch) => ({
        type: "scene", gen: 1, epoch, viewEpoch: 1, buf: new Uint8Array(4).buffer, count: 1, blockIds: ["1"], lod: lod(), digest: "aa",
    });
    worker.emit(scene(1));
    await settle();
    assert.equal(render.uploads.length, 0);
    assert.equal(render.errors.length, 1);
    assert.equal((await render.metrics()).cellCounters.strayScenes, 1);
    await render.send("refine", { trigger: "color", colorMode: "intensity", targetVoxelPixels: 4, refinementId: 1 });
    worker.emit({ type: "colorMode", gen: 1, mode: "intensity", attributeRange: null });
    worker.emit(scene(2));
    await settle();
    assert.equal(render.uploads.length, 0);
    assert.equal(render.errors.length, 2);
    assert.equal((await render.metrics()).cellCounters.strayScenes, 2);
});

test("cellsBusy keeps frames going: the final draw waits for the driver and the stats carry the flag", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    render.setCellStats({ cellsBusy: true });
    worker.emit(cellLodMessage(1, { lod: lod({ cellsBusy: false }) }));
    await settle();
    const dataset = render.messages.find(item => item.type === "dataset");
    assert.equal(dataset.lod.cellsBusy, true, "the driver's flag is merged into the builder's stats");
    assert.equal(render.renders(), 0, "no final draw while the driver still has work");
    render.setCellStats({ cellsBusy: false });
    worker.emit(cellLodMessage(2, { lod: lod({ cellsBusy: false }) }));
    await settle();
    assert.equal(render.renders(), 1, "at rest the last state is drawn");
    // The stats message that ends the host's frame loop is sent when the flag changes.
    const progress = render.messages.filter(item => item.type === "lodProgress");
    assert.equal(progress.at(0).lod.cellsBusy, true);
    assert.equal(progress.at(-1).lod.cellsBusy, false);
    // An older cellLod is ignored.
    worker.emit(cellLodMessage(1));
    await settle();
    assert.equal(render.messages.filter(item => item.type === "lod").length, 1);
});

test("the host's frame loop treats cellsBusy as still loading", () => {
    const rest = lod();
    assert.equal(lodStillLoading(rest), false);
    assert.equal(lodStillLoading({ ...rest, cellsBusy: true }), true);
    assert.equal(lodStillLoading({ ...rest, cellsBusy: true, lodError: "x", pageBudgetExhausted: true }), true);
    assert.equal(lodStillLoading({ ...rest, cellsBusy: false }), false);
});

test("cutDigestAtRest and the cell stats come from the driver in LOD stats and worker metrics", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    worker.emit(cellLodMessage(1, { lod: lod({ cellsBusy: false, cutDigestAtRest: "builder-side" }) }));
    await settle();
    render.setCellStats({ cutDigestAtRest: "00000000000000ab" });
    await render.send("api", { operation: "getStats", requestId: 5 });
    const stats = render.messages.find(item => item.type === "apiResult" && item.requestId === 5).value.lod;
    assert.equal(stats.cutDigestAtRest, "00000000000000ab");
    assert.equal(stats.wasmLinearMemoryBytes, 65536);
    const metrics = await render.metrics();
    assert.equal(metrics.cutDigestAtRest, "00000000000000ab");
    assert.equal(metrics.cellStats.cellPath, "enabled");
});

test("a render side that cannot draw cells reopens the dataset on the CPU path", async () => {
    const render = await runRenderWorker({ cellPath: "CellPathUnsupported" });
    const first = await render.openCells();
    assert.match(render.warnings.at(-1), /cells path unavailable/);
    assert.equal(first.terminated, true);
    const second = render.workers.at(-1);
    assert.notEqual(second, first);
    assert.equal(second.posted[0].cells, false);
    assert.equal(second.posted.find(item => item.type === "open").cells, false);
    assert.deepEqual(render.calls("enableCells").at(-1), ["enableCells", false]);
});

// ---- Review fixes ---------------------------------------------------------------------------

const errorsOf = messages => messages.filter(item => item.type === "error");
const lastProgress = render => render.messages.filter(item => item.type === "lodProgress").at(-1);
const statsOf = async (render, requestId) => {
    await render.send("api", { operation: "getStats", requestId });
    return render.messages.find(item => item.type === "apiResult" && item.requestId === requestId).value.lod;
};

test("a colour change keeps the cells path in the render worker: no scene, the driver's busy flag still counts", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    render.setCellStats({ cellsBusy: true });
    worker.emit(cellLodMessage(1));
    await settle();
    assert.equal(lastProgress(render).lod.cellsBusy, true);
    await render.send("refine", { trigger: "color", colorMode: "intensity", targetVoxelPixels: 4, refinementId: 1 });
    assert.deepEqual(render.calls("setPresentationColorMode").at(-1), ["setPresentationColorMode", "intensity"]);
    assert.equal((await statsOf(render, 1)).cellsBusy, true, "the cells are still drawn: their busy flag counts");
    // The cut of the new mode is applied, not stale.
    const lodMessages = render.messages.filter(item => item.type === "lod").length;
    worker.emit({ type: "colorMode", gen: 1, mode: "intensity", attributeRange: null });
    worker.emit(cellLodMessage(2));
    await settle();
    assert.ok(render.messages.filter(item => item.type === "lod").length > lodMessages);
    // A scene is stray in every mode.
    worker.emit({
        type: "scene", gen: 1, epoch: 1, viewEpoch: 1, buf: new Uint8Array(4).buffer, count: 1, blockIds: ["1"], lod: lod(), digest: "aa",
    });
    await settle();
    assert.equal(render.uploads.length, 0);
    assert.equal(render.errors.length, 1);
    // An attribute mode is the same: the builder paints it, the render side draws rgb.
    await render.send("refine", { trigger: "color", colorMode: "attribute:0:0", targetVoxelPixels: 4, refinementId: 2 });
    worker.emit({ type: "colorMode", gen: 1, mode: "attribute:0:0", attributeRange: [0, 1] });
    worker.emit(cellLodMessage(3));
    await settle();
    assert.equal((await statsOf(render, 2)).cellsBusy, true);
    assert.equal(errorsOf(render.messages).length + render.errors.length, 1);
});

test("a rejected colour mode on the cells path is undone in the render worker", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    await render.send("refine", { trigger: "color", colorMode: "intensity", targetVoxelPixels: 4, refinementId: 1 });
    worker.emit({ type: "colorMode", gen: 1, mode: "intensity", attributeRange: null });
    await render.send("refine", { trigger: "color", colorMode: "attribute:9:0", targetVoxelPixels: 4, refinementId: 2 });
    worker.emit({ type: "error", gen: 1, operation: "colorMode", message: "invalid attribute index" });
    await settle();
    assert.deepEqual(render.calls("setPresentationColorMode").map(call => call[1]), ["intensity", "attribute:9:0", "intensity"]);
});

test("the worker's own lodStillLoading counts cellsBusy (the copy that runs in the worker)", () => {
    const source = viewerSource.match(/function lodStillLoading\(lod\) \{[\s\S]*?\n\}\n/)?.[0];
    assert.ok(source, "the worker defines lodStillLoading");
    const workerCopy = vm.runInNewContext(`${source}; lodStillLoading`);
    const rest = lod();
    for (const [input, expected] of [
        [rest, false],
        [{ ...rest, cellsBusy: true }, true],
        [{ ...rest, cellsBusy: true, lodError: "x", pageBudgetExhausted: true }, true],
        [{ ...rest, cellsBusy: false }, false],
        [{ ...rest, pendingBlockIds: ["1"] }, true],
        [{ ...rest, pageBudgetExhausted: true, pendingBlockIds: ["1"] }, false],
        [{ ...rest, lodStagePixels: 9 }, true],
    ]) {
        assert.equal(workerCopy(input), expected, JSON.stringify(input));
        assert.equal(lodStillLoading(input), expected, `host copy: ${JSON.stringify(input)}`);
    }
});

test("build inbox: an evict and the ack of one batch are applied evict first", async () => {
    const worker = await runBuildWorker();
    worker.send("poolAck", { gen: 1, cutEpoch: 3, drawn: new BigUint64Array([5n]), missing: new BigUint64Array(0) });
    worker.send("evict", { gen: 1, nodes: new BigUint64Array([7n]) });
    worker.view();
    await worker.sync();
    const applied = names(worker.calls).filter(name => name === "applyEvict" || name === "applyPoolAck");
    assert.deepEqual(applied, ["applyEvict", "applyPoolAck"], "arrival order does not matter: the driver's ack is after its evictions");
});

test("build inbox: a throw in one apply does not drop the rest of the batch and is reported", async () => {
    const worker = await runBuildWorker();
    worker.send("driverReplaced", { gen: 1 });
    worker.send("evict", { gen: 1, nodes: new BigUint64Array([99n]) });
    worker.send("evict", { gen: 1, nodes: new BigUint64Array([5n]) });
    worker.send("poolAck", { gen: 1, cutEpoch: 4, drawn: new BigUint64Array(0), missing: new BigUint64Array(0) });
    worker.view();
    await worker.sync();
    assert.deepEqual(worker.calls.filter(call => call[0].startsWith("apply") || call[0] === "driverReplaced"), [
        ["driverReplaced"], ["applyEvict", [99n]], ["applyEvict", [5n]], ["applyPoolAck", 4],
    ], "everything that can be applied is applied");
    assert.ok(names(worker.calls).includes("update"), "the update still runs");
    const [error, ...others] = errorsOf(worker.posted);
    assert.equal(others.length, 0);
    assert.equal(error.operation, "build:cellInbox");
    assert.match(error.message, /bad node 99/);
    // Recovers: the inbox was emptied, the next update has nothing left to apply.
    worker.view({ epoch: 2 });
    await worker.sync();
    assert.equal(names(worker.calls).filter(name => name === "applyEvict").length, 2);
    assert.equal(errorsOf(worker.posted).length, 1);
});

test("build worker: reports for a builder with cells off never reach its cells methods", async () => {
    const plain = await runBuildWorker({ cells: false });
    for (const [type, fields] of [["poolReset", {}], ["driverReplaced", {}], ["evict", { nodes: new BigUint64Array([1n]) }],
        ["poolAck", { cutEpoch: 1 }]]) plain.send(type, { gen: 1, ...fields });
    plain.view();
    await plain.sync();
    assert.equal(errorsOf(plain.posted).length, 0, "the fake throws if any cells method were called");
    assert.equal(names(plain.calls).filter(name => name !== "enableCells" && name !== "open" && name !== "update").length, 0);
});

test("with cells off the build metrics and the render metrics keep their old shape", async () => {
    const plain = await runBuildWorker({ cells: false });
    plain.view();
    await plain.sync();
    plain.send("metrics");
    await plain.settle();
    const off = plain.posted.find(item => item.type === "metrics").metrics;
    assert.equal("cellStats" in off, false);
    assert.deepEqual(Object.keys(off.build).sort(),
        ["builds", "buildMs", "maxBuildMs", "reused", "scenes", "viewsCoalesced", "viewsReceived"].sort());
    const on = await runBuildWorker();
    on.view();
    await on.sync();
    on.send("metrics");
    await on.settle();
    const metrics = on.posted.find(item => item.type === "metrics").metrics;
    assert.deepEqual(Object.keys(metrics.cellStats).sort(), [...REAL_CELL_STATS_KEYS].sort());
    assert.equal(metrics.build.cellUpdates, 1);
    const render = await runRenderWorker({ cells: false });
    await render.open();
    const snapshot = await render.metrics();
    for (const key of ["cutDigestAtRest", "cellStats", "cellCounters"]) assert.equal(key in snapshot, false, key);
    const enabled = await runRenderWorker();
    await enabled.openCells();
    for (const key of ["cutDigestAtRest", "cellStats", "cellCounters"]) assert.equal(key in (await enabled.metrics()), true, key);
});

test("build-side and render-side cell stats merge: the driver's keys win, the planner's stay", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    const builderSide = realCellStats({
        cellsBusy: false, cutEpochSent: 5, cutEpochApplied: 1, cutDigestAtRest: "builder", poolStages: 1, viewEpoch: 3,
    });
    render.setCellStats({
        cellsBusy: false, cutEpochSent: 5, cutEpochApplied: 5, cutDigestAtRest: "00000000000000cd", poolStages: 7,
        poolUploadedBytes: 99, viewEpoch: 10,
    });
    worker.emit(cellLodMessage(1, { lod: lod({ ...builderSide, cellsBusy: true, pendingBlockIds: ["4"], builtNodes: 2 }) }));
    await settle();
    const stats = await statsOf(render, 3);
    assert.equal(stats.cutEpochApplied, 5);
    assert.equal(stats.cutDigestAtRest, "00000000000000cd");
    assert.equal(stats.poolStages, 7);
    assert.equal(stats.poolUploadedBytes, 99);
    assert.equal(stats.viewEpoch, 10);
    assert.deepEqual(stats.pendingBlockIds, ["4"], "planner stats are kept");
    assert.equal(stats.builtNodes, 2);
    assert.equal(stats.cellsBusy, true, "the builder still waits for a block");
    for (const key of REAL_CELL_STATS_KEYS) assert.ok(key in stats, key);
});

test("faults of the real API are reported by the render worker, which keeps working", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    const failures = () => errorsOf(render.messages);
    // A malformed node (empty here) is refused; the other nodes of the message are still stored.
    worker.emit(nodeCellsMessage({ buffers: [new ArrayBuffer(0), new Uint8Array(3).buffer, new Uint8Array(4).buffer] }));
    await settle();
    assert.equal(failures().length, 1);
    assert.equal(failures()[0].operation, "build:nodeCells");
    assert.match(failures()[0].message, /malformed nodeCells entry/);
    assert.deepEqual(render.calls("addCellNode"), [["addCellNode", 3, 3], ["addCellNode", 3, 4]]);
    // A cut with targetPixels out of range is refused; the next valid one is applied.
    worker.emit(cellCutMessage(1, { targetPixels: 20 }));
    await settle();
    assert.equal(failures().length, 2);
    assert.equal(failures()[1].operation, "build:cellCut");
    assert.match(failures()[1].message, /targetPixels/);
    worker.emit(cellCutMessage(2));
    worker.emit(nodeCellsMessage());
    await settle();
    assert.equal(failures().length, 2);
    assert.deepEqual(render.calls("setCellCut"), [["setCellCut", 2, 10]]);
    assert.equal(render.calls("addCellNode").length, 3);
});

test("the per-frame node byte cap is passed to the viewer only when the host gave one", async () => {
    const withCap = await runRenderWorker({ cellNodeBytesPerFrame: 4096 });
    assert.deepEqual(withCap.calls("setCellNodeBytesPerFrame"), [["setCellNodeBytesPerFrame", 4096]]);
    const without = await runRenderWorker();
    assert.equal(without.calls("setCellNodeBytesPerFrame").length, 0, "the viewer's default applies");
    const off = await runRenderWorker({ cells: false, cellNodeBytesPerFrame: 4096 });
    assert.equal(off.calls("setCellNodeBytesPerFrame").length, 0);
});

test("a driver that never settles stops holding the frame loop after the stuck limit and says so", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    render.setCellStats({ cellsBusy: true });
    worker.emit(cellLodMessage(1));
    await settle();
    for (let index = 0; index < 599; index += 1) await render.frame();
    assert.equal(lastProgress(render).lod.cellsBusy, true, "still within the limit");
    for (let index = 0; index < 3; index += 1) await render.frame();
    const stuck = lastProgress(render).lod;
    assert.equal(stuck.cellsBusy, false);
    assert.equal(stuck.cellsStuck, true);
    assert.ok(stuck.cellsStuckFrames >= 600);
    assert.equal(lodStillLoading(stuck), false, "the host's loop ends");
    assert.ok((await render.metrics()).cellCounters.cellsStuckFrames >= 600);
    // Progress (the cut applied moves) starts the count again.
    render.setCellStats({ cutEpochApplied: 9 });
    await render.frame();
    const resumed = lastProgress(render).lod;
    assert.equal(resumed.cellsBusy, true);
    assert.equal(resumed.cellsStuck, false);
});

// ---- Device loss (R3-Q26) ------------------------------------------------------------------

test("device loss on the cells path: the viewer is recreated, the camera kept and the builder told to resend", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    const before = render.sent(worker, "driverReplaced").length;
    render.loseDevice();
    await render.frame();
    await new Promise(resolve => setTimeout(resolve, 20));
    await settle();
    assert.equal(render.creates(), 2, "a new viewer on the same canvas");
    assert.deepEqual(render.messages.filter(m => m.type === "deviceLost").map(m => [m.reason, m.message]),
        [["unknown", "gpu process crashed"]]);
    assert.deepEqual(render.messages.filter(m => m.type === "deviceRecovery").map(m => m.state), ["started", "recovered"]);
    assert.deepEqual(render.calls("setCameraPose"), [["setCameraPose", [1, 2, 3], [4, 5, 6]]]);
    assert.equal(render.calls("beginRemoteCells").length, 2, "the driver starts again on the new device");
    assert.equal(worker.terminated, false, "the build worker and its dataset stay");
    assert.equal(render.workers.length, 1, "no second open of the file");
    assert.equal(render.messages.some(m => m.type === "error"), false, "the failed frame is not an error for the host");
    // The recovery finishes with a frame, and later frames work.
    const rendersAfter = render.renders();
    await render.frame();
    assert.ok(render.renders() > rendersAfter);
    assert.equal(render.sent(worker, "driverReplaced").length, before + 1, "the builder is told the driver was replaced");
});

test("a destroyed device is reported and not recovered", async () => {
    const render = await runRenderWorker();
    await render.openCells();
    render.loseDevice("destroyed", "destroy() was called");
    await render.frame();
    await new Promise(resolve => setTimeout(resolve, 20));
    assert.equal(render.creates(), 1);
    assert.deepEqual(render.messages.filter(m => m.type === "deviceLost").map(m => m.reason), ["destroyed"]);
    assert.equal(render.messages.some(m => m.type === "deviceRecovery"), false);
});

test("recovery retries with backoff and then reports one clear error", async () => {
    const render = await runRenderWorker({ failCreates: 3 });
    await render.openCells();
    render.loseDevice();
    await render.frame();
    await new Promise(resolve => setTimeout(resolve, 60));
    assert.equal(render.creates(), 4, "the first create plus three attempts");
    const errors = render.messages.filter(m => m.type === "error");
    assert.equal(errors.length, 1);
    assert.equal(errors[0].operation, "deviceLost");
    assert.match(errors[0].message, /after 3 attempts: Error: no adapter/);
    await render.frame();
    assert.equal(render.creates(), 4, "no further attempts once it gave up");
});

test("recovery succeeds on a later attempt", async () => {
    const render = await runRenderWorker({ failCreates: 2 });
    await render.openCells();
    render.loseDevice();
    await render.frame();
    await new Promise(resolve => setTimeout(resolve, 60));
    const states = render.messages.filter(m => m.type === "deviceRecovery").map(m => `${m.state}${m.attempt}`);
    assert.deepEqual(states, ["started1", "started2", "started3", "recovered3"]);
    assert.equal(render.messages.some(m => m.type === "error"), false);
});

test("with canvas replacement the worker waits for replaceCanvas, handled outside the queue, and uses the new canvas", async () => {
    const render = await runRenderWorker({ canvasReplacement: true });
    await render.openCells();
    render.loseDevice();
    await render.frame();
    assert.equal(render.creates(), 1, "recovery waits for the canvas while the queue is blocked");
    assert.equal(render.messages.some(m => m.type === "deviceLost"), true, "the host is told first, so it can replace the canvas");
    await render.send("replaceCanvas", { canvas: { id: "new" } });
    await new Promise(resolve => setTimeout(resolve, 20));
    assert.deepEqual(render.canvasesUsed, ["old", "new"]);
    assert.equal(render.messages.some(m => m.type === "deviceRecovery" && m.state === "recovered"), true);
});

test("a host that could not replace its canvas answers null and recovery uses the old one", async () => {
    const render = await runRenderWorker({ canvasReplacement: true });
    await render.openCells();
    render.loseDevice();
    await render.frame();
    await render.send("replaceCanvas", { canvas: null });
    await new Promise(resolve => setTimeout(resolve, 20));
    assert.deepEqual(render.canvasesUsed, ["old", "old"]);
});

test("a canvas that never arrives only delays recovery: the old canvas is used after the timeout", async () => {
    const render = await runRenderWorker({ canvasReplacement: true });
    await render.openCells();
    render.loseDevice();
    await render.frame();
    await new Promise(resolve => setTimeout(resolve, 2200));
    assert.deepEqual(render.canvasesUsed, ["old", "old"]);
    assert.equal(render.messages.some(m => m.type === "error"), false);
});

test("without canvas replacement offered the worker does not wait", async () => {
    const render = await runRenderWorker();
    await render.openCells();
    render.loseDevice();
    await render.frame();
    await new Promise(resolve => setTimeout(resolve, 20));
    assert.deepEqual(render.canvasesUsed, ["old", "old"]);
});

test("a loss while the dataset is still opening recovers into the opening dataset, never the demo sample", async () => {
    const render = await runRenderWorker();
    await render.open(); // the build worker has not answered `opened` yet
    const samplesBefore = render.samples();
    render.loseDevice();
    await render.frame();
    await new Promise(resolve => setTimeout(resolve, 20));
    assert.equal(render.creates(), 2);
    assert.equal(render.samples(), samplesBefore, "no sample was loaded");
    // The build worker's answer lands on the new viewer and starts its driver.
    render.workers.at(-1).emit({ type: "opened", gen: 1, opened: opened("enabled") });
    await settle();
    assert.equal(render.calls("beginRemoteCells").length, 1);
});

test("a canvas that arrives after the timeout triggers one more recreation on it", async () => {
    const render = await runRenderWorker({ canvasReplacement: true });
    await render.openCells();
    render.loseDevice();
    await render.frame();
    await new Promise(resolve => setTimeout(resolve, 2200));
    assert.deepEqual(render.canvasesUsed, ["old", "old"]);
    await render.send("replaceCanvas", { canvas: { id: "late" } });
    await new Promise(resolve => setTimeout(resolve, 30));
    assert.deepEqual(render.canvasesUsed, ["old", "old", "late"]);
    await render.send("replaceCanvas", { canvas: { id: "later" } });
    await new Promise(resolve => setTimeout(resolve, 30));
    assert.equal(render.canvasesUsed.length, 3, "only a late arrival after a timeout recreates");
});

test("after a destroyed loss nothing polls the device any more", async () => {
    const render = await runRenderWorker();
    await render.openCells();
    render.loseDevice("destroyed", "destroy() was called");
    await render.frame();
    const polls = render.lossPolls();
    await render.frame();
    await render.frame();
    await new Promise(resolve => setTimeout(resolve, 600));
    assert.equal(render.lossPolls(), polls);
});

test("a failed recovery sends the error and no deviceRecovered frame", async () => {
    const render = await runRenderWorker({ failCreates: 3 });
    await render.openCells();
    render.loseDevice();
    await render.frame();
    await new Promise(resolve => setTimeout(resolve, 60));
    assert.equal(render.messages.some(m => m.frameStatus === "deviceRecovered"), false);
    assert.equal(render.messages.filter(m => m.type === "error").length, 1);
    // The success path still acknowledges the frame.
    const ok = await runRenderWorker();
    await ok.openCells();
    ok.loseDevice();
    await ok.frame();
    await new Promise(resolve => setTimeout(resolve, 20));
    assert.equal(ok.messages.some(m => m.frameStatus === "deviceRecovered"), true);
});

// ---- On-demand levels (#35) ------------------------------------------------------------------

const levelRequestsOf = (nodes, depths) => ({
    nodes: new BigUint64Array(nodes), depths: new Uint8Array(depths),
});
const noMessages = { poolAck: null, evict: new BigUint64Array(0), poolReset: false, driverReplaced: false, levelRequests: null };

test("levelRequests are forwarded after evict and ack, with their buffers transferred", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    render.setPending({
        poolAck: { cutEpoch: 4, drawn: new BigUint64Array([1n]), missing: new BigUint64Array(0) },
        evict: new BigUint64Array([2n]), poolReset: false, driverReplaced: false,
        levelRequests: levelRequestsOf([5n, 6n], [3, 4]),
    });
    await render.frame();
    const reports = render.sent(worker, "driverReplaced", "poolReset", "evict", "poolAck", "levelRequests");
    assert.deepEqual(reports.map(item => item.type), ["evict", "poolAck", "levelRequests"]);
    const request = reports[2];
    assert.deepEqual([request.gen, [...request.nodes], [...request.depths]], [1, [5n, 6n], [3, 4]]);
    const transfer = worker.transfers.get(request);
    assert.equal(transfer.length, 2);
    assert.equal(transfer[0], request.nodes.buffer);
    assert.equal(transfer[1], request.depths.buffer);
    // An empty set is a message too: it replaces the previous one. No change, nothing sent.
    render.setPending({ ...noMessages, levelRequests: levelRequestsOf([], []) });
    await render.frame();
    await render.frame();
    const later = render.sent(worker, "levelRequests");
    assert.equal(later.length, 2);
    assert.equal(later[1].nodes.length, 0);
});

test("build inbox: levelRequests are applied after the ack and the latest set wins when coalesced", async () => {
    const worker = await runBuildWorker();
    worker.send("levelRequests", { gen: 1, nodes: new BigUint64Array([1n]), depths: new Uint8Array([2]) });
    worker.send("poolAck", { gen: 1, cutEpoch: 2, drawn: new BigUint64Array(0), missing: new BigUint64Array(0) });
    worker.send("levelRequests", { gen: 1, nodes: new BigUint64Array([7n, 8n]), depths: new Uint8Array([5, 6]) });
    worker.send("evict", { gen: 1, nodes: new BigUint64Array([3n]) });
    await worker.settle();
    assert.equal(names(worker.calls).includes("update"), false, "nothing builds without a view");
    worker.view();
    await worker.sync();
    const applied = worker.calls.slice(worker.calls.findIndex(call => call[0] === "applyEvict"));
    assert.deepEqual(applied, [
        ["applyEvict", [3n]], ["applyPoolAck", 2], ["applyLevelRequests", [7n, 8n], [5, 6]], ["setViewEpoch", 7], ["update"],
    ]);
    // The inbox is empty afterwards: the set is not applied twice.
    worker.view({ epoch: 2 });
    await worker.sync();
    assert.equal(worker.calls.filter(call => call[0] === "applyLevelRequests").length, 1);
});

test("a levelRequests message wakes the build worker that has a view and replans", async () => {
    const worker = await runBuildWorker();
    worker.view();
    await worker.sync();
    const before = worker.calls.filter(call => call[0] === "update").length;
    worker.send("levelRequests", { gen: 1, nodes: new BigUint64Array([1n]), depths: new Uint8Array([2]) });
    await worker.sync();
    assert.deepEqual(worker.calls.filter(call => call[0] === "applyLevelRequests"), [["applyLevelRequests", [1n], [2]]]);
    assert.ok(worker.calls.filter(call => call[0] === "update").length > before, "the request built something");
});

test("nodeLevels follow the nodeCells of their batch, transferred and in order", async () => {
    const levels = [];
    const worker = await runBuildWorker({
        refine: 1, levelBuffers: (n) => { const list = [new ArrayBuffer(n + 1), new ArrayBuffer(n + 2)]; levels.push(list); return list; },
    });
    worker.view();
    await worker.sync();
    const types = worker.posted.map(item => item.type).filter(type => type !== "opened");
    assert.deepEqual(types.slice(0, 4), ["nodeCells", "nodeLevels", "cellCut", "cellLod"]);
    const messages = worker.posted.filter(item => item.type === "nodeLevels");
    assert.equal(messages.length, 2, "one per update, none merged");
    messages.forEach((message, index) => {
        assert.deepEqual([message.gen, message.generation], [1, 3]);
        assert.deepEqual(message.buffers, levels[index]);
        message.buffers.forEach((buffer, i) => assert.equal(worker.transfers.get(message)[i], buffer, "transferred, not copied"));
    });
});

test("nodeLevels are applied in order after the nodeCells of the same batch, with no epoch of their own", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    worker.emit(cellLodMessage(50));
    worker.emit(nodeCellsMessage({ buffers: [new Uint8Array(3).buffer] }));
    worker.emit({ type: "nodeLevels", gen: 1, generation: 3, buffers: [new Uint8Array(5).buffer, new Uint8Array(6).buffer] });
    worker.emit(nodeCellsMessage({ buffers: [new Uint8Array(4).buffer] }));
    worker.emit({ type: "nodeLevels", gen: 1, generation: 4, buffers: [new Uint8Array(7).buffer] });
    await settle();
    assert.deepEqual(render.log.filter(entry => entry[0] === "addCellNode" || entry[0] === "addCellLevels"), [
        ["addCellNode", 3, 3], ["addCellLevels", 3, 5], ["addCellLevels", 3, 6], ["addCellNode", 3, 4], ["addCellLevels", 4, 7],
    ]);
});

test("levelRequests and nodeLevels of another gen are ignored", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    worker.emit({ type: "nodeLevels", gen: 9, generation: 3, buffers: [new Uint8Array(5).buffer] });
    await settle();
    assert.equal(render.calls("addCellLevels").length, 0);
    worker.emit({ type: "nodeLevels", gen: 1, generation: 3, buffers: [new Uint8Array(5).buffer] });
    await settle();
    assert.equal(render.calls("addCellLevels").length, 1, "positive control: the current gen is applied");
    const build = await runBuildWorker();
    build.send("levelRequests", { gen: 9, nodes: new BigUint64Array([1n]), depths: new Uint8Array([2]) });
    build.view();
    await build.sync();
    assert.equal(build.calls.some(call => call[0] === "applyLevelRequests"), false);
});

test("a level request set that arrives before the builder is ready is applied once it is", async () => {
    const worker = await runBuildWorker({
        beforeOpened: send => {
            send("levelRequests", { gen: 1, nodes: new BigUint64Array([1n]), depths: new Uint8Array([2]) });
            send("levelRequests", { gen: 1, nodes: new BigUint64Array([7n]), depths: new Uint8Array([5]) });
            send("levelRequests", { gen: 9, nodes: new BigUint64Array([3n]), depths: new Uint8Array([1]) });
        },
    });
    assert.equal(worker.calls.some(call => call[0] === "applyLevelRequests"), false, "nothing is applied without a view");
    worker.view();
    await worker.sync();
    assert.deepEqual(worker.calls.filter(call => call[0] === "applyLevelRequests"), [["applyLevelRequests", [7n], [5]]],
        "the latest set of this gen, once");
});

test("a refused nodeLevels entry does not stop the rest of the message", async () => {
    const render = await runRenderWorker();
    const worker = await render.openCells();
    worker.emit({ type: "nodeLevels", gen: 1, generation: 3, buffers: [new Uint8Array(0).buffer, new Uint8Array(5).buffer] });
    await settle();
    assert.deepEqual(render.calls("addCellLevels"), [["addCellLevels", 3, 5]]);
});
