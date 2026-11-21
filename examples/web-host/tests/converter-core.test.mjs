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

// `lockedMs` makes removal fail like a terminated worker's sync handle that is not released yet.
function fakeStorage(initialNames = [], { lockedMs = 0 } = {}) {
    const entries = new Set(initialNames);
    const lockedUntil = Date.now() + lockedMs;
    const directory = {
        async *keys() { yield* [...entries]; },
        async removeEntry(name) {
            if (Date.now() < lockedUntil) {
                throw Object.assign(new Error("locked"), { name: "NoModificationAllowedError" });
            }
            if (!entries.delete(name)) throw Object.assign(new Error("missing"), { name: "NotFoundError" });
        },
    };
    return {
        entries,
        async getDirectory() { return { async getDirectoryHandle() { return directory; } }; },
    };
}

const complete = { type: "complete", file: new Blob(["oct"]), outputName: "p.oct", points: "1" };

test("the core engine is the default and receives the converter options", { concurrency: false }, () => {
    const converter = createConverter({ converterOptions: { maxError: 0.5 } });
    void converter.convert(lasFile()).catch(() => {});
    assert.equal(workers[0].messages[0].engine, "core");
    assert.deepEqual(workers[0].messages[0].options, { maxError: 0.5 });
    converter.dispose();
});

test("engine core is accepted as a no-op and legacy is rejected with a clear message", { concurrency: false }, async () => {
    const converter = createConverter({ engine: "core" });
    void converter.convert(lasFile()).catch(() => {});
    assert.equal(workers[0].messages[0].engine, "core");
    workers[0].dispatch("message", { type: "error", message: "stop" });
    await assert.rejects(
        converter.convert(lasFile(), { engine: "legacy" }),
        { message: 'the legacy converter was removed; omit `engine` or use "core"' },
    );
    await assert.rejects(createConverter({ engine: "legacy" }).convert(lasFile()), /legacy converter was removed/);
    converter.dispose();
});

test("engine core passes options to the worker and relays progress, completion and errors", { concurrency: false }, async () => {
    const converter = createConverter({ engine: "core", converterOptions: { maxMemoryMiB: 256 } });
    const worker = workers[0];
    const stages = [];
    converter.on("progress", (value) => stages.push(value.stage));
    const first = converter.convert(lasFile(), { options: { maxError: 0.5, maxMemoryMiB: 128 } });
    assert.equal(worker.messages[0].engine, "core");
    assert.deepEqual(worker.messages[0].options, { maxError: 0.5, maxMemoryMiB: 128 });
    worker.dispatch("message", { type: "progress", stage: "writing" });
    const spool = { spilledBytes: 7, pagesPeak: 1 };
    worker.dispatch("message", { ...complete, spool, timings: { totalMs: 5 } });
    const result = await first;
    assert.equal(result.name, "p.oct");
    assert.deepEqual(result.spool, spool);
    assert.deepEqual(result.timings, { totalMs: 5 });
    assert.deepEqual(stages, ["writing"]);

    const second = converter.convert(lasFile());
    assert.deepEqual(worker.messages[1].options, { maxMemoryMiB: 256 });
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
    await converter.ready;
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

test("cancel keeps retrying removal while the terminated worker still holds its handles", { concurrency: false }, async () => {
    const names = ["conversion-2.oct.partial", "conversion-2.zone-0.partial"];
    const storage = fakeStorage(names, { lockedMs: 2000 });
    const converter = createConverter({ engine: "core", storage });
    await converter.ready;
    const rejected = assert.rejects(converter.convert(lasFile()), { name: "AbortError" });
    workers[0].dispatch("message", { type: "partial-files", directory: "octree-viewer-conversions", names });
    assert.equal(await converter.cancel(), true);
    await rejected;
    assert.equal(workers[0].terminated, true);
    assert.deepEqual([...storage.entries], []);
    converter.dispose();
});

test("orphan partial files of earlier sessions, zones included, are deleted when a converter starts", { concurrency: false }, async () => {
    const storage = fakeStorage([
        "conversion-a.oct.partial", "conversion-a.octattrs.partial", "conversion-a.zone-0.partial",
        "conversion-a.zone-12.partial", "result.oct", "conversion-a.zone-0.oct",
    ]);
    const converter = createConverter({ storage });
    for (let i = 0; i < 10; i++) await new Promise(setImmediate);
    assert.deepEqual([...storage.entries].sort(), ["conversion-a.zone-0.oct", "result.oct"]);
    converter.dispose();
});

test("convert waits for the orphan cleanup, so new zones never race it", { concurrency: false }, async () => {
    const storage = fakeStorage(["conversion-a.zone-0.partial"]);
    const order = [];
    const removeEntry = (await (await storage.getDirectory()).getDirectoryHandle()).removeEntry;
    // Slow removal: the cleanup is still running when convert() is called.
    const slow = {
        ...storage,
        async getDirectory() {
            const root = await storage.getDirectory();
            return {
                async getDirectoryHandle(...args) {
                    const directory = await root.getDirectoryHandle(...args);
                    return {
                        keys: () => directory.keys(),
                        async removeEntry(name) {
                            await new Promise((resolve) => setTimeout(resolve, 30));
                            await removeEntry(name);
                            order.push(`removed:${name}`);
                        },
                    };
                },
            };
        },
    };
    const converter = createConverter({ engine: "core", storage: slow });
    const result = converter.convert(lasFile());
    await new Promise((resolve) => setTimeout(resolve, 5));
    assert.equal(workers[0].messages.length, 0, "nothing is posted while the cleanup runs");
    await converter.ready;
    order.push("ready");
    await new Promise(setImmediate);
    assert.deepEqual(order, ["removed:conversion-a.zone-0.partial", "ready"]);
    assert.equal(workers[0].messages.length, 1);
    workers[0].dispatch("message", complete);
    await result;
    converter.dispose();
});

test("cancelling while the cleanup runs never reaches the worker", { concurrency: false }, async () => {
    const converter = createConverter({ engine: "core", storage: fakeStorage(["conversion-a.oct.partial"]) });
    const rejected = assert.rejects(converter.convert(lasFile()), { name: "AbortError" });
    assert.equal(converter.cancel(), true);
    await rejected;
    await converter.ready;
    await new Promise(setImmediate);
    assert.equal(workers[0].messages.length, 0);
    converter.dispose();
});

test("warnings from the worker are relayed to listeners", { concurrency: false }, async () => {
    const converter = createConverter({ engine: "core" });
    const warnings = [];
    converter.on("warning", (value) => warnings.push(value.message));
    const result = converter.convert(lasFile());
    workers[0].dispatch("message", { type: "warning", message: "RAM only" });
    workers[0].dispatch("message", complete);
    await result;
    assert.deepEqual(warnings, ["RAM only"]);
    converter.dispose();
});

test("options V8 fixes are refused before anything reaches the worker", { concurrency: false }, async () => {
    for (const name of ["formatVersion", "rowOrder", "pointsPerBlock", "spatialDivisions"]) {
        const created = createConverter({ converterOptions: { [name]: 1 } });
        await assert.rejects(created.convert(lasFile()), { message: `option ${name} was removed: V8 is the only format and fixes it` });
        const perCall = createConverter();
        await assert.rejects(perCall.convert(lasFile(), { options: { [name]: "x" } }), /was removed: V8 is the only format/);
        const both = createConverter({ converterOptions: { [name]: "v5" } });
        await assert.rejects(both.convert(lasFile(), { options: { maxError: 0.01 } }), { message: `option ${name} was removed: V8 is the only format and fixes it` });
        created.dispose();
        perCall.dispose();
        both.dispose();
    }
    assert.ok(workers.every((worker) => worker.messages.length === 0));
});
