import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import { test } from "node:test";

const source = (await readFile(new URL("../viewer-worker.js", import.meta.url), "utf8"))
    .replace(/^import .*\n/, "");

async function runWorker(wasmMemory = new WebAssembly.Memory({ initial: 1 })) {
    const messages = [];
    let receive;
    let now = 0;
    let colorMode = "rgb";
    let colorFrames = 0;
    let targetVoxelPixels = 4;
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
        openFormatDataset() {
            colorMode = "rgb";
            return { status: "format_ready" };
        },
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
    vm.runInNewContext(source, {
        self: {
            addEventListener(_name, callback) { receive = callback; },
            postMessage(message) { messages.push(message); },
        },
        performance,
        init: async () => ({ memory: wasmMemory }),
        WasmVoxelViewer,
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
