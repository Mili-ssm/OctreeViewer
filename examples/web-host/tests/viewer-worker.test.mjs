import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import { test } from "node:test";

const source = (await readFile(new URL("../viewer-worker.js", import.meta.url), "utf8"))
    .replace(/^import .*\n/, "");

async function runWorker(wasmMemory = new WebAssembly.Memory({ initial: 1 }), AsyncFormatReader = null) {
    const messages = [];
    let receive;
    let now = 0;
    let colorMode = "rgb";
    let colorFrames = 0;
    let targetVoxelPixels = 4;
    let pendingBlockId = null;
    const insertedBlocks = [];
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
        nextAsyncLodBlock: () => pendingBlockId,
        insertAsyncLodBlock(blockId) { insertedBlocks.push(blockId); },
        orbit() {},
        pan() {},
        zoom() {},
        moveCamera() {},
        updateLod(pixels) {
            targetVoxelPixels = pixels;
            if (colorMode !== "intensity") lod = makeLod(pixels, pixels);
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
                nodes: [{ parent: null, level: 0, min: [0, 0, 0], max: [1, 1, 1] }],
                hasAttributes: false,
                attributeSchema: [],
            };
        }
        free() {}
    }
    vm.runInNewContext(source, {
        self: {
            addEventListener(_name, callback) { receive = callback; },
            postMessage(message) { messages.push(message); },
        },
        performance,
        init: async () => ({ memory: wasmMemory }),
        WasmVoxelViewer,
        WasmAsyncFormatReader: AsyncFormatReader ?? DefaultAsyncFormatReader,
        Float64Array,
        setTimeout,
    });

    async function send(type, fields = {}) {
        receive({ data: { type, ...fields } });
        await new Promise(setImmediate);
        await new Promise(setImmediate);
    }
    await send("init", {
        canvas: {}, width: 1, height: 1, colorMode: "rgb", depthShading: false,
    });
    return {
        messages,
        send,
        setNow(value) { now = value; },
        setPendingBlock(value) { pendingBlockId = value; },
        insertedBlocks,
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
                nodes: [{ parent: null, level: 0, min: [0, 0, 0], max: [1, 1, 1] }],
                hasAttributes: false, attributeSchema: [],
            };
        }
        planRead() { return { peak_bytes: "1", output_bytes: "1" }; }
        read() {
            readStarted = true;
            return new Promise(resolve => { resolveRead = resolve; });
        }
        free() {}
    }
    const worker = await runWorker(new WebAssembly.Memory({ initial: 1 }), DelayedAsyncReader);
    worker.setPendingBlock("7");
    await worker.send("openFile", {
        file: { name: "sample.oct", size: 8 }, cacheBytes: 1024, targetVoxelPixels: 4,
    });
    assert.equal(readStarted, true);

    await worker.send("frame", {
        camera: { orbitX: 1, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0 },
    });
    assert.ok(worker.messages.some(message => message.type === "frame"));
    worker.setPendingBlock(null);
    resolveRead({ kind: "geometry", values: new Float32Array(3), colors: new Uint8Array(3) });
    await new Promise(setImmediate);
    await new Promise(setImmediate);
    assert.deepEqual(worker.insertedBlocks, []);
});

test("async worker rejects a read plan larger than the configured logical read budget", async () => {
    let reads = 0;
    class OversizedAsyncReader {
        static async open() { return new OversizedAsyncReader(); }
        lodDatasetDescriptor() {
            return {
                coordinateOrigin: [0, 0, 0], semantics: "replacement",
                nodes: [{ parent: null, level: 0, min: [0, 0, 0], max: [1, 1, 1] }],
                hasAttributes: false, attributeSchema: [],
            };
        }
        planRead() { return { peak_bytes: "1025", output_bytes: "0" }; }
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
