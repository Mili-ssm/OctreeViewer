import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import { test } from "node:test";

import { WINDOW_MS, createPxController } from "../px-controller.mjs";

const source = (await readFile(new URL("../viewer-worker.js", import.meta.url), "utf8"))
    .replace(/^import .*\n/gm, "");

async function runWorker(wasmMemory = new WebAssembly.Memory({ initial: 1 }), AsyncFormatReader = null, { skipInit = false } = {}) {
    const messages = [];
    let receive;
    let now = 0;
    let colorMode = "rgb";
    let colorFrames = 0;
    let targetVoxelPixels = 4;
    let pendingBlockIds = [];
    const insertedBlocks = [];
    let pendingPageAnchors = [];
    const graftedPages = [];
    const budgets = [];
    const timing = [];
    const updateLodCalls = [];
    let gpuFrameMs = null;
    let holdStage = null; // while set, updateLod leaves the ladder on this coarse rung
    let lod = makeLod(targetVoxelPixels, targetVoxelPixels);
    const performance = { now: () => now };
    const viewer = {
        resize() {},
        setColorMode(mode) {
            if (mode === "broken") {
                now += 10;
                throw new Error("color mode rejected");
            }
            if (mode === colorMode) return;
            now += 20;
            colorMode = mode;
            colorFrames = 0;
            lod = makeLod(8, targetVoxelPixels);
        },
        setDepthShading() {},
        replacePoints() {},
        fitScene() {},
        openLodDataset() {
            colorMode = "rgb";
            return { status: "metadata_ready" };
        },
        nextAsyncLodBlock: () => pendingBlockIds[0] ?? null,
        pendingAsyncLodBlocks: () => [...pendingBlockIds],
        pendingPageAnchors: () => [...pendingPageAnchors],
        graftPage(page) {
            if (page.failGraft) throw new Error(`graft of ${page.anchor} failed`);
            graftedPages.push(page.anchor);
            pendingPageAnchors = pendingPageAnchors.filter(anchor => anchor !== page.anchor);
        },
        insertAsyncLodBlock(blockId) {
            insertedBlocks.push(blockId);
            pendingBlockIds = pendingBlockIds.filter(id => id !== String(blockId));
        },
        orbit() {},
        pan() {},
        zoom() {},
        moveCamera() {},
        setGpuFrameTiming(enabled) { timing.push(enabled); },
        resetGpuFrameStats() { timing.push("reset"); },
        updateLod(pixels) {
            updateLodCalls.push(pixels);
            targetVoxelPixels = pixels;
            if (colorMode !== "intensity") lod = makeLod(holdStage ?? pixels, pixels);
            return lod;
        },
        lodStats: () => lod,
        renderFrame() {
            if (colorMode === "intensity") {
                colorFrames += 1;
                if (colorFrames > 1) lod = makeLod(targetVoxelPixels, targetVoxelPixels);
            }
            return "rendered";
        },
        gpuStats: () => ({
            visibleInstances: Number(lod.visiblePoints),
            requestedResources: { totalBytes: String(lod.lodStagePixels) },
            gpuFrameMs,
        }),
        stats: () => ({ point_count: Number(lod.visiblePoints) }),
        viewportSize: () => [1, 1],
    };
    class WasmVoxelViewer {
        static async createOffscreen() { return viewer; }
    }
    class DefaultAsyncFormatReader {
        static async open() { return new DefaultAsyncFormatReader(); }
        lodDatasetDescriptor() {
            return {
                coordinateOrigin: [0, 0, 0],
                semantics: "replacement",
                dataMin: [0, 0, 0], dataMax: [1, 1, 1],
                nodeCount: 1,
                maxLevel: 0,
                nodes: [{ parent: null, level: 0, min: [0, 0, 0], max: [1, 1, 1] }],
                hasAttributes: false,
                attributeSchema: [],
            };
        }
        free() {}
    }
    class DefaultAsyncReadBudget {
        constructor(limit) { this.limit = limit; budgets.push(this); }
        clone() {
            const copy = new DefaultAsyncReadBudget(this.limit);
            copy.cloned = true;
            return copy;
        }
        limitBytes() { return String(this.limit); }
        free() {}
    }
    class DefaultBlockRequestScheduler {
        constructor(limit) {
            this.limit = limit;
            this.nextTicket = 1;
            this.selected = [];
            this.active = new Map();
            this.completed = new Set();
            this.failed = new Set();
        }
        updateBlocks(ids) {
            this.selected = ids.map(String);
            const cancelled = [];
            for (const request of this.active.values()) {
                if (request.current && !this.selected.includes(request.blockId)) {
                    request.current = false;
                    cancelled.push(request.ticket);
                }
            }
            for (const id of [...this.completed]) if (!this.selected.includes(id)) this.completed.delete(id);
            for (const id of [...this.failed]) if (!this.selected.includes(id)) this.failed.delete(id);
            return cancelled;
        }
        pendingBlocks() {
            const active = new Set([...this.active.values()].map(item => item.blockId));
            return this.selected.filter(id => !active.has(id)
                && !this.completed.has(id) && !this.failed.has(id));
        }
        takeNextAdmitted(ids) {
            if (this.active.size >= this.limit) return null;
            const allowed = ids.map(String);
            const blockId = this.pendingBlocks().find(id => allowed.includes(id));
            if (blockId === undefined) return null;
            const ticket = String(this.nextTicket++);
            this.active.set(ticket, { ticket, blockId, current: true });
            return { ticket, blockId };
        }
        complete(ticket, actualBlockId, succeeded) {
            const request = this.active.get(String(ticket));
            if (!request) throw new Error("unknown ticket");
            this.active.delete(String(ticket));
            if (!request.current || !this.selected.includes(request.blockId)) return "discarded";
            if (!succeeded) {
                this.failed.add(request.blockId);
                return "failed";
            }
            if (request.blockId !== String(actualBlockId)) {
                this.failed.add(request.blockId);
                return "unexpectedBlock";
            }
            this.completed.add(request.blockId);
            return "delivered";
        }
        isCurrent(ticket) {
            const request = this.active.get(String(ticket));
            return Boolean(request?.current && this.selected.includes(request.blockId));
        }
        retry(blockId) {
            const id = String(blockId);
            if (!this.selected.includes(id) || !this.failed.delete(id)) return false;
            return true;
        }
        fail(blockId) {
            const id = String(blockId);
            if (!this.selected.includes(id)) return false;
            this.failed.add(id);
            return true;
        }
        cancelAll() {
            const cancelled = [];
            for (const request of this.active.values()) {
                if (request.current) {
                    request.current = false;
                    cancelled.push(request.ticket);
                }
            }
            this.selected = [];
            this.completed.clear();
            this.failed.clear();
            return cancelled;
        }
        inFlightCount() { return this.active.size; }
    }
    vm.runInNewContext(source, {
        self: {
            addEventListener(_name, callback) { receive = callback; },
            postMessage(message) { messages.push(message); },
        },
        performance,
        resolveSource: async (spec) => spec,
        init: async () => ({ memory: wasmMemory }),
        WasmVoxelViewer,
        WasmAsyncReadBudget: DefaultAsyncReadBudget,
        WasmBlockRequestScheduler: DefaultBlockRequestScheduler,
        WasmAsyncFormatReader: AsyncFormatReader ?? DefaultAsyncFormatReader,
        Float64Array,
        setTimeout,
        createPxController,
        PX_WINDOW_MS: WINDOW_MS,
    });

    async function send(type, fields = {}) {
        receive({ data: { type, ...fields } });
        await new Promise(setImmediate);
        await new Promise(setImmediate);
    }
    if (!skipInit) {
        await send("init", {
            canvas: {}, width: 1, height: 1, colorMode: "rgb", depthShading: false,
        });
    }
    return {
        messages,
        send,
        setNow(value) { now = value; },
        holdStage(stage) { holdStage = stage; lod = makeLod(stage ?? targetVoxelPixels, targetVoxelPixels); },
        setGpuFrameMs(p95, samples = 60) { gpuFrameMs = p95 === null ? null : { p95, samples }; },
        timing,
        updateLodCalls,
        setPendingBlock(value) { pendingBlockIds = value === null ? [] : [String(value)]; },
        setPendingBlocks(values) { pendingBlockIds = values.map(String); },
        setPendingPageAnchors(values) { pendingPageAnchors = [...values]; },
        insertedBlocks,
        graftedPages,
        budgetLimits: () => budgets.filter(budget => !budget.cloned).map(budget => budget.limit),
    };
}

function makeLod(stage, target) {
    return {
        visiblePoints: "10",
        coverageComplete: true,
        pendingBlockIds: [],
        lodStagePixels: stage,
        targetVoxelPixels: target,
        visibleBlocks: 1,
        lodError: null,
    };
}

function latestProgress(messages) {
    return messages.filter((message) => message.type === "lodProgress").at(-1);
}

test("worker starts scoped refinement timers after successful color changes", async () => {
    const worker = await runWorker();
    worker.setNow(100);
    await worker.send("openFile", {
        file: { name: "sample.oct", size: 8 },
        cacheBytes: 1024,
        targetVoxelPixels: 4,
    });
    const datasetOpened = worker.messages.find(message => message.type === "datasetOpened");
    assert.equal(datasetOpened.opened.nodeCount, 1);
    assert.equal(datasetOpened.opened.hasAttributes, false);
    assert.deepEqual([...datasetOpened.opened.boundsMax], [1, 1, 1]);
    assert.deepEqual({ ...latestProgress(worker.messages).refinement }, {
        id: 0,
        trigger: "dataset",
        colorMode: "rgb",
        firstFrameMs: null,
        targetLodMs: null,
    });

    worker.setNow(125);
    await worker.send("frame", { camera: { orbitX: 0, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0 } });
    assert.equal(latestProgress(worker.messages).refinement.firstFrameMs, 25);
    assert.equal(latestProgress(worker.messages).targetLodMs, 25);

    worker.setNow(200);
    await worker.send("refine", { trigger: "color", colorMode: "intensity", targetVoxelPixels: 4, refinementId: 7 });
    worker.setNow(230);
    await worker.send("frame", { camera: { orbitX: 0, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0 } });
    let progress = latestProgress(worker.messages);
    assert.equal(progress.firstCoarseMs, 30);
    assert.equal(progress.targetLodMs, null);
    assert.deepEqual({ ...progress.refinement }, {
        id: 7,
        trigger: "color",
        colorMode: "intensity",
        firstFrameMs: 30,
        targetLodMs: null,
    });

    worker.setNow(240);
    await worker.send("refine", { trigger: "color", colorMode: "broken", targetVoxelPixels: 4, refinementId: 8 });
    worker.setNow(250);
    await worker.send("frame", { camera: { orbitX: 0, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0 } });
    progress = latestProgress(worker.messages);
    assert.equal(progress.refinement.id, 7);
    assert.equal(progress.refinement.trigger, "color");
    assert.equal(progress.targetLodMs, 50);

    worker.setNow(300);
    await worker.send("refine", { trigger: "detail", colorMode: "intensity", targetVoxelPixels: 2, refinementId: 9 });
    worker.setNow(310);
    await worker.send("frame", { camera: { orbitX: 0, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0 } });
    progress = latestProgress(worker.messages);
    assert.deepEqual({ ...progress.refinement }, {
        id: 9,
        trigger: "detail",
        colorMode: "intensity",
        firstFrameMs: 10,
        targetLodMs: 10,
    });
});


test("LOD progress carries current GPU and viewer statistics without a periodic stats frame", async () => {
    const worker = await runWorker();
    await worker.send("openFile", {
        file: { name: "sample.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4,
    });
    let progress = latestProgress(worker.messages);
    assert.equal(progress.gpu?.requestedResources.totalBytes, "4");
    assert.equal(progress.viewer?.point_count, Number(progress.lod.visiblePoints));
    await worker.send("refine", {
        trigger: "color", colorMode: "intensity", targetVoxelPixels: 4, refinementId: 1,
    });
    progress = latestProgress(worker.messages);
    assert.equal(progress.lod.lodStagePixels, 8);
    assert.equal(progress.gpu?.requestedResources.totalBytes, "8");
    assert.equal(worker.messages.filter(message => message.type === "stats").length, 0);
});


test("worker memory snapshots follow WASM growth in API, progress and periodic stats", async () => {
    const memory = new WebAssembly.Memory({ initial: 1 });
    const worker = await runWorker(memory);
    await worker.send("api", { operation: "getStats", requestId: 1 });
    assert.equal(worker.messages.at(-1).value.memory.wasmLinearMemoryBytes, 65536);
    assert.equal(worker.messages.at(-1).value.memory.workerJsHeapUsedBytes, null);
    memory.grow(2);
    await worker.send("openFile", { file: { name: "sample.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4 });
    assert.equal(latestProgress(worker.messages).memory.wasmLinearMemoryBytes, 3 * 65536);
    for (let index = 0; index < 30; index += 1) {
        await worker.send("frame", { camera: { orbitX: 0, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0 } });
    }
    const stats = worker.messages.filter(message => message.type === "stats").at(-1);
    assert.equal(stats.memory.wasmLinearMemoryBytes, 3 * 65536);
    assert.equal(stats.memory.workerJsHeapUsedBytes, null);
    memory.grow(1);
    await worker.send("api", { operation: "getStats", requestId: 2 });
    assert.equal(worker.messages.at(-1).value.memory.wasmLinearMemoryBytes, 4 * 65536);
});

test("async Blob reads leave the message queue responsive and discard stale generations", async () => {
    let resolveRead;
    let readStarted = false;
    class DelayedAsyncReader {
        static async open() { return new DelayedAsyncReader(); }
        lodDatasetDescriptor() {
            return {
                coordinateOrigin: [0, 0, 0], semantics: "replacement",
                dataMin: [0, 0, 0], dataMax: [1, 1, 1],
                nodeCount: 1,
                maxLevel: 0,
                nodes: [{ parent: null, level: 0, min: [0, 0, 0], max: [1, 1, 1] }],
                hasAttributes: false, attributeSchema: [],
            };
        }
        planLodRead() {
            return {
                cancelToken: () => ({ cancel() {} }),
                read: async () => {
                    readStarted = true;
                    const value = await new Promise(resolve => { resolveRead = resolve; });
                    return { value, release() {}, free() {} };
                },
                free() {},
            };
        }
        free() {}
    }
    const worker = await runWorker(new WebAssembly.Memory({ initial: 1 }), DelayedAsyncReader);
    worker.setPendingBlock("7");
    await worker.send("openFile", {
        file: { name: "sample.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4,
    });
    assert.equal(readStarted, true);

    worker.setPendingBlock(null);
    await worker.send("frame", {
        camera: { orbitX: 1, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0 },
    });
    assert.ok(worker.messages.some(message => message.type === "frame"));
    resolveRead({ positions: new Float32Array(3), colors: new Uint8Array(3), attribute: null });
    await new Promise(setImmediate);
    await new Promise(setImmediate);
    assert.deepEqual(worker.insertedBlocks, []);
});

test("async worker overlaps reads and retries budget-deferred blocks after a result frees quota", async () => {
    const resolvers = new Map();
    const started = [];
    let firstAdmission = true;
    class ConcurrentAsyncReader {
        static async open() { return new ConcurrentAsyncReader(); }
        lodDatasetDescriptor() {
            return {
                coordinateOrigin: [0, 0, 0], semantics: "replacement",
                dataMin: [0, 0, 0], dataMax: [1, 1, 1],
                nodeCount: 1,
                maxLevel: 0,
                nodes: [{ parent: null, level: 0, min: [0, 0, 0], max: [1, 1, 1] }],
                hasAttributes: false, attributeSchema: [],
            };
        }
        planLodRead(blockId) {
            const id = String(blockId);
            if (id === "9" && firstAdmission) {
                firstAdmission = false;
                return null;
            }
            return {
                cancelToken: () => ({ cancel() {} }),
                read: async () => {
                    started.push(id);
                    const value = await new Promise(resolve => { resolvers.set(id, resolve); });
                    return { value, release() {}, free() {} };
                },
                free() {},
            };
        }
        free() {}
    }
    const worker = await runWorker(new WebAssembly.Memory({ initial: 1 }), ConcurrentAsyncReader);
    worker.setPendingBlocks(["7", "8", "9"]);
    await worker.send("openFile", {
        file: { name: "sample.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4,
    });
    assert.deepEqual(started, ["7", "8"]);

    resolvers.get("7")({ positions: new Float32Array(3), colors: new Uint8Array(3), attribute: null });
    await new Promise(setImmediate);
    await new Promise(setImmediate);
    assert.deepEqual(started, ["7", "8", "9"]);

    resolvers.get("8")({ positions: new Float32Array(3), colors: new Uint8Array(3), attribute: null });
    resolvers.get("9")({ positions: new Float32Array(3), colors: new Uint8Array(3), attribute: null });
    await new Promise(setImmediate);
    await new Promise(setImmediate);
    assert.deepEqual(worker.insertedBlocks, ["7", "8", "9"]);
});

test("replacing a dataset cancels multiple active tickets before scheduling its blocks", async () => {
    const resolvers = new Map();
    const started = [];
    let openedDatasets = 0;
    class ReplaceableAsyncReader {
        static async open() { return new ReplaceableAsyncReader(++openedDatasets); }
        constructor(datasetId) { this.datasetId = datasetId; }
        lodDatasetDescriptor() {
            return {
                coordinateOrigin: [0, 0, 0], semantics: "replacement",
                dataMin: [0, 0, 0], dataMax: [1, 1, 1],
                nodeCount: 1,
                maxLevel: 0,
                nodes: [{ parent: null, level: 0, min: [0, 0, 0], max: [1, 1, 1] }],
                hasAttributes: false, attributeSchema: [],
            };
        }
        planLodRead(blockId) {
            const id = String(blockId);
            const readId = String(this.datasetId) + ":" + id;
            return {
                cancelToken: () => ({ cancel() {} }),
                read: async () => {
                    started.push(readId);
                    const value = await new Promise(resolve => { resolvers.set(readId, resolve); });
                    return { value, release() {}, free() {} };
                },
                free() {},
            };
        }
        free() {}
    }
    const worker = await runWorker(new WebAssembly.Memory({ initial: 1 }), ReplaceableAsyncReader);
    worker.setPendingBlocks(["7", "8"]);
    await worker.send("openFile", {
        file: { name: "first.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4,
    });
    assert.deepEqual(started, ["1:7", "1:8"]);

    await worker.send("openFile", {
        file: { name: "second.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4,
    });
    assert.deepEqual(started, ["1:7", "1:8"],
        "replacement dataset scheduled duplicate blocks before old reads released their slots");

    resolvers.get("1:7")({ positions: new Float32Array(3), colors: new Uint8Array(3), attribute: null });
    resolvers.get("1:8")({ positions: new Float32Array(3), colors: new Uint8Array(3), attribute: null });
    for (let index = 0; index < 4; index++) await new Promise(setImmediate);
    assert.deepEqual(started, ["1:7", "1:8", "2:7", "2:8"]);
    assert.deepEqual(worker.insertedBlocks, [],
        "stale dataset results must not reach the current viewer");

    resolvers.get("2:7")({ positions: new Float32Array(3), colors: new Uint8Array(3), attribute: null });
    resolvers.get("2:8")({ positions: new Float32Array(3), colors: new Uint8Array(3), attribute: null });
    for (let index = 0; index < 4; index++) await new Promise(setImmediate);
    assert.deepEqual(worker.insertedBlocks, ["7", "8"]);
});

test("async worker rejects a read plan larger than the configured logical read budget", async () => {
    let reads = 0;
    class OversizedAsyncReader {
        static async open() { return new OversizedAsyncReader(); }
        lodDatasetDescriptor() {
            return {
                coordinateOrigin: [0, 0, 0], semantics: "replacement",
                dataMin: [0, 0, 0], dataMax: [1, 1, 1],
                nodeCount: 1,
                maxLevel: 0,
                nodes: [{ parent: null, level: 0, min: [0, 0, 0], max: [1, 1, 1] }],
                hasAttributes: false, attributeSchema: [],
            };
        }
        planLodRead() {
            throw new Error("geometry read needs 1025 bytes; async read budget is 1024");
        }
        async read() { reads += 1; }
        free() {}
    }
    const worker = await runWorker(new WebAssembly.Memory({ initial: 1 }), OversizedAsyncReader);
    worker.setPendingBlock("7");
    await worker.send("openFile", {
        file: { name: "sample.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4,
    });
    const failure = worker.messages.find(message => message.operation === "asyncLodRead");
    assert.match(failure.message, /needs 1025 bytes/);
    assert.equal(reads, 0);
});

test("async worker reports a failed block read instead of throwing in its error path", async () => {
    class FailingReadAsyncReader {
        static async open() { return new FailingReadAsyncReader(); }
        lodDatasetDescriptor() {
            return {
                coordinateOrigin: [0, 0, 0], semantics: "replacement",
                dataMin: [0, 0, 0], dataMax: [1, 1, 1],
                nodeCount: 1,
                maxLevel: 0,
                nodes: [{ parent: null, level: 0, min: [0, 0, 0], max: [1, 1, 1] }],
                hasAttributes: false, attributeSchema: [],
            };
        }
        planLodRead() {
            return {
                cancelToken: () => ({ cancel() {} }),
                read: async () => { throw new Error("range read failed"); },
                free() {},
            };
        }
        free() {}
    }
    const worker = await runWorker(new WebAssembly.Memory({ initial: 1 }), FailingReadAsyncReader);
    worker.setPendingBlock("7");
    await worker.send("openFile", {
        file: { name: "sample.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4,
    });
    for (let index = 0; index < 4; index++) await new Promise(setImmediate);
    const failure = worker.messages.find(message => message.operation === "asyncLodRead");
    assert.match(failure?.message ?? "", /range read failed/);
});

test("metrics message reports busy time per message type, frame queue wait and the LOD log", async () => {
    const worker = await runWorker();
    await worker.send("openFile", { file: { name: "sample.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4 });
    await worker.send("refine", { trigger: "color", colorMode: "intensity", targetVoxelPixels: 4, refinementId: 1 });
    const camera = { orbitX: 0, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0 };
    await worker.send("frame", { camera });
    await worker.send("frame", { camera });
    await worker.send("metrics");
    const { workerMetrics } = worker.messages.filter(message => message.type === "metrics").at(-1);
    assert.equal(workerMetrics.busyByMessage.frame.count, 2);
    assert.equal(workerMetrics.busyByMessage.refine.count, 1);
    assert.equal(workerMetrics.busyByMessage.asyncOpen.count, 1);
    assert.equal(workerMetrics.busyByMessage.metrics, undefined);
    assert.equal(workerMetrics.frameQueueWaitMs.count, 2);
    assert.deepEqual(Array.from(workerMetrics.lodLog, entry => entry.source), ["dataset", "refine"]);
    assert.equal(workerMetrics.lodLog[1].stagePx, 8);
    assert.equal(workerMetrics.lodLogDropped, 0);

    await worker.send("metrics", { reset: true });
    await worker.send("metrics");
    const reset = worker.messages.filter(message => message.type === "metrics").at(-1).workerMetrics;
    assert.equal(Object.keys(reset.busyByMessage).length, 0);
    assert.equal(reset.lodLog.length, 0);
});

const pagedOpen = { file: { name: "paged.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4 };

// Minimal paged reader: one root node; subclasses define `loadPage`.
class PagedReader {
    static async open() { return new this(); }
    lodDatasetDescriptor() {
        return {
            coordinateOrigin: [0, 0, 0], semantics: "replacement",
            dataMin: [0, 0, 0], dataMax: [1, 1, 1], nodeCount: 1, maxLevel: 0,
            nodes: [{ parent: null, level: 0, min: [0, 0, 0], max: [1, 1, 1] }],
            hasAttributes: false, attributeSchema: [],
        };
    }
    free() {}
}

test("a page that fails to load is not requested again until an explicit refinement", async () => {
    const loads = [];
    class FailingPageReader extends PagedReader {
        async loadPage(anchor) {
            loads.push(anchor);
            await new Promise(setImmediate);
            throw new Error("network error");
        }
    }
    const worker = await runWorker(new WebAssembly.Memory({ initial: 1 }), FailingPageReader);
    worker.setPendingPageAnchors(["a"]);
    await worker.send("openFile", pagedOpen);
    for (let i = 0; i < 5; i += 1) await worker.send("frame", { camera: {} });
    assert.deepEqual(loads, ["a"]);
    await worker.send("refine", { trigger: "retry", colorMode: "rgb", targetVoxelPixels: 4 });
    for (let i = 0; i < 3; i += 1) await worker.send("frame", { camera: {} });
    assert.deepEqual(loads, ["a", "a"]);
});

test("a page whose graft throws does not stop the next queued page from grafting", async () => {
    class GraftReader extends PagedReader {
        async loadPage(anchor) {
            await new Promise(setImmediate);
            return { anchor, failGraft: anchor === "a", free() {} };
        }
    }
    const worker = await runWorker(new WebAssembly.Memory({ initial: 1 }), GraftReader);
    worker.setPendingPageAnchors(["a", "b"]);
    await worker.send("openFile", pagedOpen);
    for (let i = 0; i < 3; i += 1) await new Promise(setImmediate);
    assert.deepEqual(worker.graftedPages, ["b"]);
    assert.ok(worker.messages.some(message => message.type === "error" && message.operation === "asyncPageRead"));
});

test("a later dataset with another cacheBytes gets its own read budget", async () => {
    const worker = await runWorker();
    await worker.send("openFile", { ...pagedOpen, cacheBytes: 1024 });
    await worker.send("openFile", { ...pagedOpen, cacheBytes: 1024 });
    assert.deepEqual(worker.budgetLimits(), [1024]);
    await worker.send("openFile", { ...pagedOpen, cacheBytes: 4096 });
    assert.deepEqual(worker.budgetLimits(), [1024, 4096]);
});

test("a page that failed in one dataset is requested again after opening a new one", async () => {
    const loads = [];
    class FailingPageReader extends PagedReader {
        async loadPage(anchor) {
            loads.push(anchor);
            await new Promise(setImmediate);
            throw new Error("network error");
        }
    }
    const worker = await runWorker(new WebAssembly.Memory({ initial: 1 }), FailingPageReader);
    worker.setPendingPageAnchors(["a"]);
    await worker.send("openFile", pagedOpen);
    await worker.send("frame", { camera: {} });
    await worker.send("openFile", pagedOpen);
    await worker.send("frame", { camera: {} });
    assert.deepEqual(loads, ["a", "a"]);
});

const idleCamera = { orbitX: 0, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0 };

// Draws `count` frames `stepMs` apart, starting after the current time `from`.
async function drawFrames(worker, from, count, stepMs) {
    for (let i = 1; i <= count; i++) {
        worker.setNow(from + i * stepMs);
        await worker.send("frame", { camera: idleCamera });
    }
    return from + count * stepMs;
}

async function openedDynamicWorker() {
    const worker = await runWorker();
    await worker.send("openFile", { file: { name: "sample.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4 });
    return worker;
}

test("pxMode is fixed by default: no timer, no controller, the host's target rules", async () => {
    const worker = await openedDynamicWorker();
    await worker.send("refine", { trigger: "detail", colorMode: "rgb", targetVoxelPixels: 2 });
    await drawFrames(worker, 0, 100, 50);
    assert.deepEqual(worker.timing, []);
    assert.equal(worker.updateLodCalls.at(-1), 2);
    const px = worker.messages.filter(message => message.type === "stats").at(-1).px;
    assert.deepEqual({ ...px }, { mode: "fixed", targetPx: 2, signalMs: null, signalSource: null });
});

test("dynamic mode enables the GPU timer, coarsens on overload and refines on headroom", async () => {
    const worker = await openedDynamicWorker();
    await worker.send("api", { operation: "setPxMode", mode: "dynamic", requestId: 1 });
    assert.deepEqual(worker.timing, [true, "reset"]);
    assert.equal(worker.messages.find(message => message.requestId === 1).value.mode, "dynamic");
    // 4 px at the start; a 25 ms GPU p95 coarsens to 8 px within the first window.
    worker.setGpuFrameMs(25);
    let t = await drawFrames(worker, 0, 25, 50);
    assert.equal(worker.updateLodCalls.at(-1), 8);
    // Headroom for two windows refines back, after the dwell.
    worker.setGpuFrameMs(6);
    t = await drawFrames(worker, t, 70, 50);
    assert.equal(worker.updateLodCalls.at(-1), 4);
    const px = (await (async () => {
        await worker.send("api", { operation: "getStats", requestId: 2 });
        return worker.messages.find(message => message.requestId === 2).value.px;
    })());
    assert.deepEqual({ ...px }, { mode: "dynamic", targetPx: 4, signalMs: 6, signalSource: "gpu" });
    // The host's slider does not override the controller while dynamic.
    await worker.send("refine", { trigger: "detail", colorMode: "rgb", targetVoxelPixels: 1 });
    assert.equal(worker.updateLodCalls.at(-1), 4);
    // Going back to fixed restores the host's last target and turns the timer off.
    await worker.send("pxMode", { mode: "fixed" });
    assert.equal(worker.updateLodCalls.at(-1), 1);
    assert.equal(worker.timing.at(-1), false);
});

test("dynamic mode falls back to the frame interval when the GPU cannot time frames", async () => {
    const worker = await openedDynamicWorker();
    await worker.send("pxMode", { mode: "dynamic" });
    worker.setGpuFrameMs(null);
    // 30 ms between frames (p95 over 20 ms) coarsens; the signal source is reported.
    await drawFrames(worker, 0, 40, 30);
    assert.equal(worker.updateLodCalls.at(-1), 8);
    await worker.send("api", { operation: "getStats", requestId: 3 });
    const px = worker.messages.find(message => message.requestId === 3).value.px;
    assert.equal(px.signalSource, "interval");
});

test("an invalid pxMode is rejected", async () => {
    const worker = await openedDynamicWorker();
    await worker.send("pxMode", { mode: "auto" });
    assert.match(worker.messages.at(-1).message, /pxMode must be/);
});

test("frames on coarse rungs do not count toward a step until the ladder reaches the target", async () => {
    const worker = await openedDynamicWorker();
    await worker.send("pxMode", { mode: "dynamic" });
    worker.setGpuFrameMs(25);
    worker.holdStage(16);
    let t = await drawFrames(worker, 0, 100, 50);
    assert.equal(worker.updateLodCalls.at(-1), 4, "5 s of overload on unsettled frames: no step");
    worker.holdStage(null);
    t = await drawFrames(worker, t, 60, 50);
    assert.equal(worker.updateLodCalls.at(-1), 8, "settled, the overload counts");
});

test("a window that spans an idle gap is discarded", async () => {
    const worker = await openedDynamicWorker();
    await worker.send("pxMode", { mode: "dynamic" });
    worker.setGpuFrameMs(25);
    let t = await drawFrames(worker, 0, 10, 50);
    t = await drawFrames(worker, t + 5000, 5, 50); // idle, then resumes: the old window is dropped
    assert.equal(worker.updateLodCalls.at(-1), 4);
});

test("dynamic mode leaves a host-enabled GPU timer on when going back to fixed", async () => {
    const worker = await openedDynamicWorker();
    await worker.send("api", { operation: "setGpuFrameTiming", enabled: true, requestId: 5 });
    await worker.send("pxMode", { mode: "dynamic" });
    await worker.send("pxMode", { mode: "fixed" });
    assert.equal(worker.timing.filter(value => value === false).length, 0);
});

test("choosing dynamic before the viewer exists is rejected and the mode stays fixed", async () => {
    const worker = await runWorker(undefined, null, { skipInit: true });
    await worker.send("pxMode", { mode: "dynamic" });
    assert.match(worker.messages.at(-1).message, /not ready/);
});

test("a new dataset keeps the mode and restarts the window and dwell", async () => {
    const worker = await openedDynamicWorker();
    await worker.send("pxMode", { mode: "dynamic" });
    worker.setGpuFrameMs(25);
    let t = await drawFrames(worker, 0, 15, 50);
    await worker.send("openFile", { file: { name: "b.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4 });
    await worker.send("api", { operation: "getStats", requestId: 7 });
    const px = worker.messages.find(message => message.requestId === 7).value.px;
    assert.equal(px.mode, "dynamic");
    assert.equal(px.signalSource, null, "the last window's signal is gone");
});
