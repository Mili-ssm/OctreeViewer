import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { createConverter } from "../converter-api.mjs";

const originalWorker = globalThis.Worker;
const workers = [];

class FakeWorker {
    constructor() {
        this.listeners = new Map();
        this.messages = [];
        this.terminated = false;
        workers.push(this);
    }
    addEventListener(type, callback) {
        const callbacks = this.listeners.get(type) ?? new Set();
        callbacks.add(callback);
        this.listeners.set(type, callbacks);
    }
    postMessage(message) { this.messages.push(message); }
    terminate() { this.terminated = true; }
    dispatch(type, data) {
        for (const callback of this.listeners.get(type) ?? []) callback({ data, message: data?.message });
    }
}

beforeEach(() => { globalThis.Worker = FakeWorker; });
afterEach(() => {
    globalThis.Worker = originalWorker;
    workers.length = 0;
});

function lasFile(name = "points.las") {
    const file = new Blob(["local test data"]);
    Object.defineProperty(file, "name", { value: name });
    return file;
}

function fakeStorage(initialNames = []) {
    const entries = new Set(initialNames);
    const directory = {
        async *keys() { yield* [...entries]; },
        async removeEntry(name) {
            if (!entries.delete(name)) throw Object.assign(new Error("missing"), { name: "NotFoundError" });
        },
    };
    return {
        entries,
        async getDirectory() { return { async getDirectoryHandle() { return directory; } }; },
    };
}

const complete = { type: "complete", file: new Blob(["oct"]), outputName: "p.oct", points: "1" };

test("the legacy engine stays the default and sends no options", { concurrency: false }, () => {
    const converter = createConverter({ converterOptions: { pointsPerBlock: 4096 } });
    void converter.convert(lasFile()).catch(() => {});
    assert.deepEqual(Object.keys(workers[0].messages[0]), ["type", "file"]);
    converter.dispose();
});

test("engine core passes options to the worker and relays progress, completion and errors", { concurrency: false }, async () => {
    const converter = createConverter({ engine: "core", converterOptions: { spatialDivisions: 2 } });
    const worker = workers[0];
    const stages = [];
    converter.on("progress", (value) => stages.push(value.stage));
    const first = converter.convert(lasFile(), { options: { pointsPerBlock: 4096, spatialDivisions: 8 } });
    assert.equal(worker.messages[0].engine, "core");
    assert.deepEqual(worker.messages[0].options, { pointsPerBlock: 4096, spatialDivisions: 8 });
    worker.dispatch("message", { type: "progress", stage: "writing" });
    worker.dispatch("message", complete);
    assert.equal((await first).name, "p.oct");
    assert.deepEqual(stages, ["writing"]);

    const second = converter.convert(lasFile());
    assert.deepEqual(worker.messages[1].options, { spatialDivisions: 2 });
    worker.dispatch("message", { type: "error", message: "bad input" });
    await assert.rejects(second, /bad input/);
    await assert.rejects(converter.convert(lasFile(), { engine: "turbo" }), /unknown engine/);
    converter.dispose();
});

test("cancelling the core engine terminates the worker and deletes its partial files", { concurrency: false }, async () => {
    const names = ["conversion-1.oct.partial", "conversion-1.octattrs.partial"];
    const storage = fakeStorage([...names, "keep.oct"]);
    const converter = createConverter({ engine: "core", storage });
    const cancelled = [];
    converter.on("cancelled", (error) => cancelled.push(error.name));
    const active = converter.convert(lasFile());
    workers[0].dispatch("message", { type: "partial-files", directory: "octree-viewer-conversions", names });
    assert.equal(await converter.cancel(), true);
    await assert.rejects(active, { name: "AbortError" });
    assert.equal(workers[0].terminated, true);
    assert.deepEqual(cancelled, ["AbortError"]);
    assert.deepEqual([...storage.entries], ["keep.oct"]);

    const next = converter.convert(lasFile());
    assert.equal(workers.length, 2);
    workers[1].dispatch("message", complete);
    await next;
    converter.dispose();
});

test("orphan partial files of earlier sessions are deleted when a converter starts", { concurrency: false }, async () => {
    const storage = fakeStorage(["conversion-a.oct.partial", "conversion-a.octattrs.partial", "result.oct"]);
    const converter = createConverter({ storage });
    for (let i = 0; i < 10; i++) await new Promise(setImmediate);
    assert.deepEqual([...storage.entries], ["result.oct"]);
    converter.dispose();
});
