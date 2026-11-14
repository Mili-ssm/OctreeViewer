import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import { test } from "node:test";

const stripImports = source => source.replace(/^import .*\n/gm, "");
const viewerSource = stripImports(await readFile(new URL("../viewer-worker.js", import.meta.url), "utf8"));
const buildSource = stripImports(await readFile(new URL("../build-worker.js", import.meta.url), "utf8"));
const settle = async () => {
    await new Promise(setImmediate);
    await new Promise(setImmediate);
};

const lod = (pending = 0, stage = 4) => ({
    visiblePoints: "10", coverageComplete: true, pendingBlockIds: new Array(pending).fill("1"),
    lodStagePixels: stage, targetVoxelPixels: 4, visibleBlocks: 1, lodError: null,
});
const opened = () => ({
    status: "metadata_ready", visualScale: 1, nodeCount: 1, maxLevel: 0,
    coordinateOrigin: new Float64Array(3), boundsMin: new Float64Array(3), boundsMax: new Float64Array([1, 1, 1]),
    semantics: "replacement", attributeSchema: [], hasAttributes: false, sourcePointCount: 1,
});

// ---- Build worker in a vm with a fake WASM binding ------------------------------------------

async function runBuildWorker({
    updates = [], graftFails = false, pageLoadFails = false, asyncBlocks = null,
} = {}) {
    const posted = [];
    const grafted = [];
    const pageLoads = [];
    const updateCalls = [];
    const builders = [];
    const budgets = [];
    const planCalls = [];
    const inserts = [];
    let receive;
    class WasmSceneBuilder {
        constructor(maxVoxels, maxBytes) { this.limits = [maxVoxels, maxBytes]; builders.push(this); }
        openLodDataset() {
            return {
                status: "metadata_ready", visualScale: 2, cellPath: asyncBlocks?.cells ? "enabled" : "disabled",
            };
        }
        enableCells() {}
        setCellPoolPages() {}
        pendingAsyncLodBlocks() { return asyncBlocks && !inserts.length ? ["7"] : []; }
        insertAsyncLodBlock(...args) { inserts.push(args); }
        setColorMode(mode) {
            if (mode === "attribute:9:0") throw new Error("invalid attribute index");
            this.mode = mode;
            this.range = mode.startsWith("attribute:") ? [0, 2.5] : null;
        }
        attributeRange() { return this.range ?? null; }
        update(camera, width, height, visualScale, targetPx) {
            updateCalls.push({ camera, width, height, visualScale, targetPx });
            const planned = updates[updateCalls.length - 1] ?? {};
            return {
                epoch: updateCalls.length, sceneBuffer: planned.scene === false ? null : new ArrayBuffer(8),
                instanceCount: 1, blockIds: ["7"], digest: "00", needsRefinement: planned.needsRefinement ?? false,
                lod: planned.lod ?? lod(),
            };
        }
        graftPage(page) {
            grafted.push(page.anchor);
            if (graftFails) throw new Error("page budget exhausted: a 100-byte page does not fit");
        }
        free() {}
    }
    class Reader {
        static async open() { return new Reader(); }
        async loadPage(anchor) {
            pageLoads.push(anchor);
            if (pageLoadFails) throw new Error("network error");
            return { anchor, free() {} };
        }
        planLodRead(...args) {
            planCalls.push(args);
            return {
                cancelToken: () => ({ cancel() {} }),
                read: async () => ({
                    value: { positions: new Float32Array(3), colors: new Uint8Array(3), attributes: ["column"] },
                    release() {}, free() {},
                }),
                free() {},
            };
        }
        lodDatasetDescriptor() {
            return {
                coordinateOrigin: [0, 0, 0], semantics: "replacement",
                hasAttributes: asyncBlocks ? asyncBlocks.hasAttributes !== false : false,
                attributeSchema: asyncBlocks?.schema ?? [],
                dataMin: [0, 0, 0], dataMax: [0.5, 0.5, 0.5],
                nodeCount: 1,
                maxLevel: 0,
                nodes: [{ parent: null, level: 0, min: [0, 0, 0], max: [1, 1, 1] }],
            };
        }
        free() {}
    }
    class Budget {
        constructor(limit) { this.limit = limit; budgets.push(this); }
        clone() { return this; }
        limitBytes() { return String(this.limit); }
        free() {}
    }
    class Scheduler {
        updateBlocks() { return []; }
        pendingBlocks() { return asyncBlocks && !inserts.length ? ["7"] : []; }
        inFlightCount() { return 0; }
        takeNextAdmitted() { return { ticket: 1 }; }
        isCurrent() { return true; }
        complete() { return "completed"; }
        cancelAll() { return []; }
    }
    vm.runInNewContext(buildSource, {
        self: {
            addEventListener(_name, callback) { receive = callback; },
            postMessage(message) { posted.push(message); },
        },
        performance: { now: () => Date.now() },
        resolveSource: async (spec) => spec,
        sumRangeStats: () => null,
        init: async () => ({ memory: new WebAssembly.Memory({ initial: 2 }) }),
        WasmSceneBuilder, WasmAsyncFormatReader: Reader, WasmAsyncReadBudget: Budget,
        WasmBlockRequestScheduler: Scheduler,
        Float64Array, setTimeout, clearTimeout,
    });
    const send = (type, fields = {}) => receive({ data: { type, ...fields } });
    send("init", {});
    send("open", { gen: 1, file: {}, cacheBytes: 1024, limits: [100, 200] });
    await settle();
    return { posted, updateCalls, builders, grafted, budgets, pageLoads, planCalls, inserts, send, settle };
}

test("build worker opens the dataset and reports its metadata", async () => {
    const worker = await runBuildWorker();
    assert.deepEqual(worker.builders[0].limits, [100, 200]);
    const message = worker.posted.find(item => item.type === "opened");
    assert.equal(message.gen, 1);
    assert.equal(message.opened.visualScale, 2);
    assert.equal(message.opened.nodeCount, 1);
    assert.deepEqual([...message.opened.boundsMax], [0.5, 0.5, 0.5]); // tight data extent, not the root node
});

test("build worker recreates the read budget when a later open brings another cacheBytes", async () => {
    const worker = await runBuildWorker();
    worker.send("open", { gen: 2, file: {}, cacheBytes: 1024, limits: [100, 200] });
    await worker.settle();
    assert.deepEqual(worker.budgets.map(budget => budget.limit), [1024]);
    worker.send("open", { gen: 3, file: {}, cacheBytes: 4096, limits: [100, 200] });
    await worker.settle();
    assert.deepEqual(worker.budgets.map(budget => budget.limit), [1024, 4096]);
});

test("build worker coalesces views queued during a build: latest wins", async () => {
    const worker = await runBuildWorker();
    for (const epoch of [1, 2, 3]) {
        worker.send("view", {
            gen: 1, epoch, camera: [epoch], viewport: [640, 480], visualScale: 2, targetPx: 4,
        });
    }
    await worker.settle();
    await new Promise(resolve => setTimeout(resolve, 20));
    assert.equal(worker.updateCalls.length, 1);
    assert.deepEqual(worker.updateCalls[0].camera, [3]);
    assert.equal(worker.updateCalls[0].width, 640);
    const scenes = worker.posted.filter(item => item.type === "scene");
    assert.equal(scenes.length, 1);
    assert.equal(scenes[0].viewEpoch, 3);
    assert.equal(scenes[0].epoch, 1);
    assert.deepEqual(scenes[0].blockIds, ["7"]);
    worker.send("metrics");
    await worker.settle();
    const metrics = worker.posted.find(item => item.type === "metrics").metrics;
    assert.equal(metrics.build.viewsReceived, 3);
    assert.equal(metrics.build.viewsCoalesced, 2);
    assert.equal(metrics.build.builds, 1);
    assert.equal(metrics.busyByMessage.build.count, 1);
    assert.equal(metrics.wasmLinearMemoryBytes, 2 * 65536);
});

test("build worker repeats the build while the planner asks for refinement and reuses scenes", async () => {
    const worker = await runBuildWorker({ updates: [{ needsRefinement: true }, { scene: false }] });
    worker.send("view", { gen: 1, epoch: 1, camera: [1], viewport: [4, 4], visualScale: 2, targetPx: 4 });
    await worker.settle();
    await new Promise(resolve => setTimeout(resolve, 30));
    assert.equal(worker.updateCalls.length, 2);
    assert.deepEqual(worker.posted.filter(item => item.type === "scene" || item.type === "lod")
        .map(item => [item.type, item.epoch, item.viewEpoch]), [["scene", 1, 1], ["lod", 2, 1]]);
});

test("build worker does not request a page that failed to load again until a view asks to retry", async () => {
    const pending = { ...lod(), pendingPageAnchors: ["5"] };
    const worker = await runBuildWorker({ pageLoadFails: true, updates: new Array(6).fill({ lod: pending }) });
    const view = (epoch, retryFailed = false) => worker.send("view", {
        gen: 1, epoch, camera: [epoch], viewport: [4, 4], visualScale: 2, targetPx: 4, retryFailed,
    });
    view(1);
    await worker.settle();
    await new Promise(resolve => setTimeout(resolve, 30));
    view(2);
    await worker.settle();
    await new Promise(resolve => setTimeout(resolve, 30));
    assert.deepEqual(worker.pageLoads, ["5"], "a new view alone does not refetch the failed page");
    view(3, true);
    await worker.settle();
    await new Promise(resolve => setTimeout(resolve, 30));
    assert.deepEqual(worker.pageLoads, ["5", "5"]);
});

test("build worker stops refetching a page the budget cannot hold until the view changes", async () => {
    const pending = { ...lod(), pendingPageAnchors: ["5"] };
    const exhausted = { ...pending, pageBudgetExhausted: true };
    const worker = await runBuildWorker({
        graftFails: true,
        updates: [{ lod: pending }, { lod: exhausted }, { lod: exhausted }, { lod: exhausted }],
    });
    worker.send("view", { gen: 1, epoch: 1, camera: [1], viewport: [4, 4], visualScale: 2, targetPx: 4 });
    await worker.settle();
    await new Promise(resolve => setTimeout(resolve, 30));
    assert.deepEqual(worker.grafted, ["5"], "no refetch loop while the view is unchanged");
    assert.equal(worker.updateCalls.length, 2, "the failed graft refreshes the stats once");
    assert.ok(worker.posted.filter(item => item.type === "scene" || item.type === "lod")
        .some(item => item.lod.pageBudgetExhausted), "the host receives the exhausted state");
    assert.ok(worker.posted.some(item => item.type === "error" && item.operation === "asyncPageRead"));
    worker.send("view", { gen: 1, epoch: 2, camera: [2], viewport: [4, 4], visualScale: 2, targetPx: 4 });
    await worker.settle();
    await new Promise(resolve => setTimeout(resolve, 30));
    assert.deepEqual(worker.grafted, ["5", "5"], "a new view retries the page");
});

test("build worker ignores views and color changes for another gen", async () => {
    const worker = await runBuildWorker();
    worker.send("view", { gen: 9, epoch: 1, camera: [1], viewport: [4, 4], visualScale: 2, targetPx: 4 });
    worker.send("colorMode", { gen: 9, mode: "intensity" });
    await worker.settle();
    await new Promise(resolve => setTimeout(resolve, 20));
    assert.equal(worker.updateCalls.length, 0);
    assert.equal(worker.builders[0].mode, undefined);
    worker.send("colorMode", { gen: 1, mode: "intensity" });
    await worker.settle();
    assert.equal(worker.builders[0].mode, "intensity");
});

test("build worker confirms an accepted colour mode with the attribute range it normalises by", async () => {
    const worker = await runBuildWorker();
    worker.send("colorMode", { gen: 1, mode: "attribute:0:1" });
    await worker.settle();
    const [reply] = worker.posted.filter(item => item.type === "colorMode");
    assert.deepEqual({ ...reply, attributeRange: [...reply.attributeRange] },
        { type: "colorMode", gen: 1, mode: "attribute:0:1", attributeRange: [0, 2.5] });
    worker.send("colorMode", { gen: 1, mode: "rgb" });
    await worker.settle();
    assert.equal(worker.posted.filter(item => item.type === "colorMode").at(-1).attributeRange, null);
});

test("build worker reports a rejected colour mode as an error and keeps the previous mode", async () => {
    const worker = await runBuildWorker();
    worker.send("colorMode", { gen: 1, mode: "intensity" });
    worker.send("colorMode", { gen: 1, mode: "attribute:9:0" });
    await worker.settle();
    assert.equal(worker.builders[0].mode, "intensity");
    const error = worker.posted.find(item => item.type === "error");
    assert.equal(error.operation, "colorMode");
    assert.equal(error.gen, 1);
    assert.match(error.message, /invalid attribute index/);
    assert.deepEqual(worker.posted.filter(item => item.type === "colorMode").map(item => item.mode), ["intensity"]);
});

const COLUMNS = [{ name: "intensity" }, { name: "classification" }, { name: "vector" }];
const plan = call => JSON.parse(JSON.stringify(call, (_key, value) => (typeof value === "bigint" ? `${value}n` : value)));
const sendView = async (worker, epoch = 1) => {
    worker.send("view", { gen: 1, epoch, camera: [1], viewport: [4, 4], visualScale: 2, targetPx: 4 });
    await worker.settle();
    await new Promise(resolve => setTimeout(resolve, 20));
};

test("with cells the build worker reads intensity and class with the geometry in every colour mode", async () => {
    const worker = await runBuildWorker({ asyncBlocks: { cells: true, schema: COLUMNS } });
    await sendView(worker);
    assert.deepEqual(plan(worker.planCalls[0]), ["7n", "geometryAttributes", null, [0, 1]]);
    assert.deepEqual(plan(worker.inserts[0].slice(3)), [null, ["column"]]);
});

test("with cells an attribute colour mode adds its column to the geometry read", async () => {
    const worker = await runBuildWorker({ asyncBlocks: { cells: true, schema: COLUMNS } });
    worker.send("colorMode", { gen: 1, mode: "attribute:2:1" });
    await sendView(worker);
    assert.deepEqual(plan(worker.planCalls[0]), ["7n", "geometryAttributes", 2, [0, 1]]);
});

test("with cells an attribute mode on intensity reads it once and intensity mode reads no painted column", async () => {
    const worker = await runBuildWorker({ asyncBlocks: { cells: true, schema: COLUMNS } });
    worker.send("colorMode", { gen: 1, mode: "attribute:0:0" });
    await sendView(worker);
    assert.deepEqual(plan(worker.planCalls[0]), ["7n", "geometryAttributes", 0, [1]]);
    const other = await runBuildWorker({ asyncBlocks: { cells: true, schema: COLUMNS } });
    other.send("colorMode", { gen: 1, mode: "intensity" });
    await sendView(other);
    assert.deepEqual(plan(other.planCalls[0]), ["7n", "geometryAttributes", null, [0, 1]]);
});

test("with cells and no attribute file no column is read", async () => {
    const worker = await runBuildWorker({ asyncBlocks: { cells: true, schema: COLUMNS, hasAttributes: false } });
    worker.send("colorMode", { gen: 1, mode: "attribute:0:0" });
    await sendView(worker);
    assert.deepEqual(plan(worker.planCalls[0]), ["7n", "geometryAttributes", null, []]);
});

test("without cells the build worker keeps the per-mode reads", async () => {
    const worker = await runBuildWorker({ asyncBlocks: { cells: false, schema: COLUMNS } });
    await sendView(worker);
    assert.deepEqual(plan(worker.planCalls[0]), ["7n", "geometry", null, null]);
});

// ---- Render worker in build-worker mode ------------------------------------------------------

async function runRenderWorker({ buildWorker = { url: "build-worker.js", type: "module" }, failSpawn = 0 } = {}) {
    const messages = [];
    const workers = [];
    const uploads = [];
    const beginCalls = [];
    let receive;
    let now = 0;
    let camera = [0, 0, 0, 10, 0, 0, 1, 0, 0, 0, 1];
    let viewport = [100, 50];
    let renderFrames = 0;
    const presentationModes = [];
    let viewerLodStats = null;
    const viewer = {
        resize(width, height) { viewport = [width, height]; },
        setColorMode() {}, setPresentationColorMode(mode) { presentationModes.push(mode); }, setDepthShading() {}, replacePoints() {},
        fitScene() {}, orbit() { camera = camera.map((value, index) => (index === 4 ? value + 0.1 : value)); },
        pan() {}, zoom() {}, moveCamera() {},
        voxelBuildLimits: () => [500, 600],
        cameraTransport: () => new Float64Array(camera),
        viewportSize: () => viewport,
        beginRemoteDataset(...args) { beginCalls.push(args); return true; },
        uploadSceneInstances(bytes, blockIds) { uploads.push({ bytes, blockIds }); return bytes.length; },
        lodStats: () => viewerLodStats,
        renderFrame() { renderFrames += 1; return "rendered"; },
        gpuStats: () => ({}), stats: () => ({}),
    };
    const warnings = [];
    let spawnFailures = failSpawn;
    class FakeWorker {
        constructor(url, options) {
            if (spawnFailures > 0) {
                spawnFailures -= 1;
                throw new Error("Script cannot be accessed from origin 'null'");
            }
            this.url = url; this.options = options; this.posted = []; this.terminated = false;
            this.listeners = [];
            this.errorListeners = [];
            workers.push(this);
        }
        addEventListener(type, callback) {
            if (type === "message") this.listeners.push(callback);
            if (type === "error") this.errorListeners.push(callback);
        }
        postMessage(message) {
            this.posted.push(message);
            if (message.type === "metrics" && !message.reset) {
                queueMicrotask(() => this.emit({
                    type: "metrics",
                    metrics: {
                        asyncBlocks: { blocks: 3, readMs: 12, decodeMs: 6, maxDecodeMs: 4, insertMs: 1, maxInsertMs: 1 },
                        build: { builds: 2, scenes: 1, reused: 1, viewsReceived: 5, viewsCoalesced: 3 },
                        busyByMessage: { build: { count: 2, busyMs: 9, maxBusyMs: 5 } },
                        wasmLinearMemoryBytes: 3 * 65536,
                    },
                }));
            }
        }
        terminate() { this.terminated = true; }
        emit(data) { for (const callback of this.listeners) callback({ data }); }
    }
    vm.runInNewContext(viewerSource, {
        self: {
            addEventListener(_name, callback) { receive = callback; },
            postMessage(message) { messages.push(message); },
        },
        performance: { now: () => now },
        resolveSource: async (spec) => spec,
        sumRangeStats: () => null,
        init: async () => ({ memory: new WebAssembly.Memory({ initial: 1 }) }),
        WasmVoxelViewer: { createOffscreen: async () => viewer },
        WasmAsyncReadBudget: class {}, WasmBlockRequestScheduler: class {}, WasmAsyncFormatReader: class {},
        Worker: FakeWorker, console: { warn: message => warnings.push(message) }, Float64Array, Uint8Array, setTimeout, clearTimeout,
    });
    const send = async (type, fields = {}) => {
        receive({ data: { type, ...fields } });
        await settle();
    };
    await send("init", { canvas: {}, width: 100, height: 50, colorMode: "rgb", depthShading: false, buildWorker, cells: false });
    const open = () => send("openFile", {
        file: { name: "a.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4,
    });
    const frame = (fields = {}) => send("frame", {
        camera: { orbitX: 0, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0, ...fields },
    });
    return {
        messages, workers, uploads, beginCalls, presentationModes, warnings, send, open, frame,
        setNow(value) { now = value; },
        setViewerLodStats(value) { viewerLodStats = value; },
        failNextSpawn() { spawnFailures = 1; },
        renders: () => renderFrames,
        setCamera(value) { camera = value; },
        views: worker => worker.posted.filter(item => item.type === "view"),
    };
}

const sceneMessage = (fields = {}) => ({
    type: "scene", gen: 1, epoch: 1, viewEpoch: 1, buf: new Uint8Array([1, 2, 3, 4]).buffer,
    count: 1, blockIds: ["5"], lod: lod(1), digest: "ab", ...fields,
});

test("render worker prewarms the build worker and forwards open without respawning the first time", async () => {
    const render = await runRenderWorker();
    assert.equal(render.workers.length, 1);
    assert.equal(render.workers[0].options.type, "module");
    assert.equal(render.workers[0].posted[0].type, "init");
    await render.open();
    assert.equal(render.workers.length, 1);
    const open = render.workers[0].posted.find(item => item.type === "open");
    assert.equal(open.gen, 1);
    assert.deepEqual([...open.limits], [500, 600]);
    assert.equal(open.cacheBytes, 1024);
});

test("render worker sends the first view once the dataset opens and applies scenes", async () => {
    const render = await runRenderWorker();
    await render.open();
    const [worker] = render.workers;
    render.setNow(100);
    worker.emit({ type: "opened", gen: 1, opened: opened() });
    await settle();
    assert.equal(render.beginCalls.length, 1);
    assert.equal(render.messages.filter(item => item.type === "datasetOpened").length, 1);
    const [view] = render.views(worker);
    assert.equal(view.epoch, 1);
    assert.deepEqual([...view.viewport], [100, 50]);
    assert.equal(view.targetPx, 4);
    assert.equal(view.visualScale, 1);
    assert.equal(view.camera.length, 11);

    render.setNow(130);
    worker.emit(sceneMessage());
    await settle();
    assert.deepEqual([...render.uploads[0].bytes], [1, 2, 3, 4]);
    assert.deepEqual(render.uploads[0].blockIds, ["5"]);
    const dataset = render.messages.find(item => item.type === "dataset");
    assert.equal(dataset.lod.renderedVoxels, 1);
    assert.equal(render.renders(), 0, "a still-loading LOD leaves drawing to the host frames");

    render.setNow(150);
    worker.emit(sceneMessage({ epoch: 2, viewEpoch: 1, lod: lod(0, 4) }));
    await settle();
    assert.equal(render.renders(), 1, "the final scene is drawn even if the host stopped asking for frames");
    const progress = render.messages.filter(item => item.type === "lodProgress").at(-1);
    assert.notEqual(progress.targetLodMs, null, "the target milestone is reported without a host frame");
    render.send("metrics");
    await settle();
    const metrics = render.messages.find(item => item.type === "metrics").workerMetrics;
    assert.equal(metrics.sceneAgeMs.count, 1, "age is measured against the view that is still known");
});

test("render worker drops scenes from an old epoch or another gen", async () => {
    const render = await runRenderWorker();
    await render.open();
    const [worker] = render.workers;
    worker.emit({ type: "opened", gen: 1, opened: opened() });
    await settle();
    worker.emit(sceneMessage({ epoch: 5 }));
    worker.emit(sceneMessage({ epoch: 5 }));
    worker.emit(sceneMessage({ epoch: 4 }));
    worker.emit(sceneMessage({ epoch: 9, gen: 7 }));
    worker.emit(sceneMessage({ epoch: 6 }));
    await settle();
    assert.equal(render.uploads.length, 2);
    render.send("metrics");
    await settle();
    const metrics = render.messages.find(item => item.type === "metrics").workerMetrics;
    assert.equal(metrics.scenesApplied, 2);
    assert.equal(metrics.scenesDropped, 3);
});

test("render worker terminates and respawns the build worker when a new dataset opens", async () => {
    const render = await runRenderWorker();
    await render.open();
    const [first] = render.workers;
    first.emit({ type: "opened", gen: 1, opened: opened() });
    await settle();
    await render.open();
    assert.equal(first.terminated, true);
    assert.equal(render.workers.length, 2);
    const second = render.workers[1];
    assert.equal(second.posted.find(item => item.type === "open").gen, 2);
    // Output the terminated worker had already queued must not reach the new dataset.
    first.emit(sceneMessage({ gen: 1, epoch: 50 }));
    first.emit({ type: "opened", gen: 1, opened: opened() });
    await settle();
    assert.equal(render.uploads.length, 0);
    second.emit({ type: "opened", gen: 2, opened: opened() });
    await settle();
    second.emit(sceneMessage({ gen: 2, epoch: 1, viewEpoch: 2 }));
    await settle();
    assert.equal(render.uploads.length, 1);
});

test("render worker sends a view only when the camera, viewport or pixel target changed", async () => {
    const render = await runRenderWorker();
    await render.open();
    const [worker] = render.workers;
    worker.emit({ type: "opened", gen: 1, opened: opened() });
    await settle();
    assert.equal(render.views(worker).length, 1);
    await render.frame();
    await render.frame();
    assert.equal(render.views(worker).length, 1, "static frames send nothing");
    await render.frame({ orbitX: 0.1 });
    assert.equal(render.views(worker).length, 2);
    assert.equal(render.views(worker)[1].epoch, 2);
    await render.send("resize", { width: 200, height: 80 });
    assert.deepEqual([...render.views(worker)[2].viewport], [200, 80]);
    await render.send("refine", { trigger: "detail", colorMode: "rgb", targetVoxelPixels: 2, refinementId: 1 });
    assert.equal(render.views(worker)[3].targetPx, 2);
    await render.send("refine", { trigger: "color", colorMode: "intensity", targetVoxelPixels: 2, refinementId: 2 });
    assert.deepEqual(worker.posted.filter(item => item.type === "colorMode")
        .map(item => [item.gen, item.mode]), [[1, "intensity"]]);
    assert.equal(render.views(worker).length, 5, "a refine always re-sends the view so the build worker answers");
});

test("render worker aggregates the build worker's metrics", async () => {
    const render = await runRenderWorker();
    await render.open();
    const [worker] = render.workers;
    render.setNow(10);
    worker.emit({ type: "opened", gen: 1, opened: opened() });
    await settle();
    render.setNow(20);
    await render.frame({ orbitX: 0.1 });
    render.setNow(50);
    worker.emit(sceneMessage({ epoch: 1, viewEpoch: 1 }));
    await settle();
    render.setNow(60);
    worker.emit(sceneMessage({ epoch: 2, viewEpoch: 2, lod: lod(1) }));
    await settle();
    await render.send("metrics");
    const metrics = render.messages.filter(item => item.type === "metrics").at(-1).workerMetrics;
    assert.equal(metrics.buildWorkerEnabled, true);
    assert.deepEqual({ ...metrics.sceneAgeMs }, { count: 2, p50: 40, p95: 40, max: 40 });
    assert.deepEqual({ ...metrics.epochLag }, { count: 2, p50: 0, p95: 1, max: 1 });
    assert.equal(metrics.sceneApplyMs.count, 2);
    assert.equal(metrics.viewsSent, 2);
    assert.equal(metrics.viewsCoalesced, 3);
    assert.equal(metrics.scenesApplied, 2);
    assert.equal(metrics.scenesDropped, 0);
    assert.equal(metrics.asyncBlocks.blocks, 3);
    assert.equal(metrics.asyncBlocks.readMs, 12);
    assert.equal(metrics.wasmLinearMemoryBytes.render, 65536);
    assert.equal(metrics.wasmLinearMemoryBytes.build, 3 * 65536);
    assert.equal(metrics.buildWorker.busyByMessage.build.count, 2);
    assert.ok(metrics.busyByMessage.buildScene.count >= 2);
    await render.send("metrics", { reset: true });
    await render.send("metrics");
    const reset = render.messages.filter(item => item.type === "metrics").at(-1).workerMetrics;
    assert.equal(reset.sceneAgeMs.count, 0);
    assert.equal(reset.scenesApplied, 0);
    assert.ok(worker.posted.some(item => item.type === "metrics" && item.reset));
});

test("with the option off no build worker exists and the open flow is the in-worker one", async () => {
    const render = await runRenderWorker({ buildWorker: null });
    assert.equal(render.workers.length, 0);
    await render.send("metrics");
    const metrics = render.messages.find(item => item.type === "metrics").workerMetrics;
    assert.equal(metrics.buildWorkerEnabled, false);
    assert.equal("sceneAgeMs" in metrics, false);
    assert.deepEqual(render.messages.map(item => item.type).filter(type => type !== "metrics"), ["ready"]);
});

// ---- Attribute colours and the scene digest in build mode ----------------------------------------

const openedWithAttributes = () => ({
    ...opened(), hasAttributes: true, attributeSchema: [{ name: "temperature", componentCount: 1 }],
});

test("render worker shows attribute modes as rgb and takes the range the build worker confirmed", async () => {
    const render = await runRenderWorker();
    await render.open();
    const [worker] = render.workers;
    worker.emit({ type: "opened", gen: 1, opened: openedWithAttributes() });
    await settle();
    await render.send("refine", { trigger: "color", colorMode: "attribute:0:0", targetVoxelPixels: 4, refinementId: 1 });
    assert.deepEqual(render.presentationModes, ["attribute:0:0"], "the viewer itself maps attribute:* to rgb");
    assert.deepEqual(worker.posted.filter(item => item.type === "colorMode")
        .map(item => [item.gen, item.mode]), [[1, "attribute:0:0"]]);
    worker.emit({ type: "colorMode", gen: 1, mode: "attribute:0:0", attributeRange: [-1, 3] });
    worker.emit(sceneMessage({ lod: lod(0, 4) }));
    await settle();
    await render.send("api", { operation: "getStats", requestId: 1 });
    const stats = render.messages.find(item => item.type === "apiResult" && item.requestId === 1).value;
    assert.deepEqual([...stats.lod.attributeRange], [-1, 3]);
});

test("render worker returns to the confirmed colour mode when the build worker rejects one", async () => {
    const render = await runRenderWorker();
    await render.open();
    const [worker] = render.workers;
    worker.emit({ type: "opened", gen: 1, opened: openedWithAttributes() });
    await settle();
    await render.send("refine", { trigger: "color", colorMode: "intensity", targetVoxelPixels: 4, refinementId: 1 });
    worker.emit({ type: "colorMode", gen: 1, mode: "intensity", attributeRange: null });
    await settle();
    await render.send("refine", { trigger: "color", colorMode: "attribute:9:0", targetVoxelPixels: 4, refinementId: 2 });
    worker.emit({ type: "error", gen: 1, operation: "colorMode", message: "invalid attribute index" });
    await settle();
    assert.deepEqual(render.presentationModes, ["intensity", "attribute:9:0", "intensity"]);
    const error = render.messages.find(item => item.type === "error");
    assert.equal(error.operation, "colorMode");
    // The mode the host asks for next is compared with the confirmed one, so a retry is forwarded.
    await render.send("refine", { trigger: "color", colorMode: "attribute:9:0", targetVoxelPixels: 4, refinementId: 3 });
    assert.equal(worker.posted.filter(item => item.type === "colorMode").length, 3);
});

test("render worker exposes the digest of the applied scene in LOD stats and metrics", async () => {
    const render = await runRenderWorker();
    await render.open();
    const [worker] = render.workers;
    worker.emit({ type: "opened", gen: 1, opened: opened() });
    await settle();
    worker.emit(sceneMessage({ epoch: 1, digest: "00000000000000aa", lod: lod(1) }));
    await settle();
    assert.equal(render.messages.find(item => item.type === "dataset").lod.sceneDigest, "00000000000000aa");
    // A reused scene keeps the digest of what is still on screen.
    worker.emit({ type: "lod", gen: 1, epoch: 2, viewEpoch: 1, lod: lod(0) });
    await settle();
    assert.equal(render.messages.filter(item => item.type === "lod").at(-1).lod.sceneDigest, "00000000000000aa");
    // A scene dropped for being old does not change what is displayed.
    worker.emit(sceneMessage({ epoch: 1, digest: "00000000000000bb" }));
    await settle();
    await render.send("metrics");
    assert.equal(render.messages.filter(item => item.type === "metrics").at(-1).workerMetrics.sceneDigest, "00000000000000aa");
    worker.emit(sceneMessage({ epoch: 3, digest: "00000000000000cc" }));
    await settle();
    await render.send("api", { operation: "getStats", requestId: 2 });
    assert.equal(render.messages.find(item => item.type === "apiResult" && item.requestId === 2).value.lod.sceneDigest,
        "00000000000000cc");
    // A new dataset starts without a digest.
    await render.open();
    await render.send("metrics");
    assert.equal(render.messages.filter(item => item.type === "metrics").at(-1).workerMetrics.sceneDigest, null);
});

test("in-worker mode reports the viewer's scene digest as sceneDigest", async () => {
    const render = await runRenderWorker({ buildWorker: null });
    await render.send("metrics");
    assert.equal(render.messages.find(item => item.type === "metrics").workerMetrics.sceneDigest, null);
    render.setViewerLodStats({ ...lod(0), sceneDigest: "00000000000000dd" });
    await render.send("metrics");
    assert.equal(render.messages.filter(item => item.type === "metrics").at(-1).workerMetrics.sceneDigest,
        "00000000000000dd");
});

// ---- Fallback when nested workers are refused (file:// in Chrome) ------------------------------

test("render worker falls back to the in-worker path when the build worker cannot be created", async () => {
    const render = await runRenderWorker({ failSpawn: 1 });
    assert.equal(render.workers.length, 0);
    assert.equal(render.messages.at(-1).type, "ready", "init still succeeds");
    assert.equal(render.messages.filter(item => item.type === "buildWorkerUnavailable").length, 1);
    assert.match(render.warnings[0], /build worker unavailable/);
    await render.send("metrics");
    assert.equal(render.messages.find(item => item.type === "metrics").workerMetrics.buildWorkerEnabled, false);
    await render.send("refine", { trigger: "color", colorMode: "intensity", targetVoxelPixels: 4, refinementId: 1 });
    assert.deepEqual(render.presentationModes, [], "the in-worker viewer colours, not the presentation mode");
});

test("render worker falls back when the prewarmed build worker fails to start", async () => {
    const render = await runRenderWorker();
    const [worker] = render.workers;
    for (const listener of worker.errorListeners) listener({ message: "Script load failed", preventDefault() {} });
    await settle();
    assert.equal(worker.terminated, true);
    assert.equal(render.messages.filter(item => item.type === "buildWorkerUnavailable").length, 1);
    await render.send("metrics");
    assert.equal(render.messages.find(item => item.type === "metrics").workerMetrics.buildWorkerEnabled, false);
});

test("render worker opens in-worker when respawning the build worker is refused", async () => {
    const render = await runRenderWorker();
    await render.open();
    const [first] = render.workers;
    first.emit({ type: "opened", gen: 1, opened: opened() });
    await settle();
    // The second dataset needs a fresh worker; this time the browser refuses it.
    render.failNextSpawn();
    await render.open();
    assert.equal(first.terminated, true);
    assert.equal(render.messages.filter(item => item.type === "buildWorkerUnavailable").length, 1);
    assert.equal(render.workers.length, 1, "no second build worker was created");
});
