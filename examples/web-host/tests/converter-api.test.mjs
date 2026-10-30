import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { createConverter } from "../converter-api.mjs";

const originalWorker = globalThis.Worker;
const workers = [];

class FakeWorker {
    constructor(url, options) {
        this.url = url;
        this.options = options;
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

    postMessage(message) {
        if (this.postMessageError) throw this.postMessageError;
        this.messages.push(message);
    }
    terminate() { this.terminated = true; this.terminateCount = (this.terminateCount ?? 0) + 1; }
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

test("converts a Blob in the worker and relays progress events", { concurrency: false }, async () => {
    const converter = createConverter();
    const worker = workers[0];
    const progress = [];
    converter.on("progress", (value) => progress.push(value));
    const resultPromise = converter.convert(lasFile());
    assert.equal(worker.messages[0].type, "convert");

    worker.dispatch("message", { type: "progress", processedPoints: "12", totalPoints: "20" });
    const output = new Blob(["oct"]);
    const attributesFile = new Blob(["octattrs"]);
    const timings = { planningMs: 100, writingMs: 200, totalMs: 350 };
    worker.dispatch("message", {
        type: "complete", file: output, outputName: "points.oct",
        attributesFile, outputAttributesName: "points.octattrs", points: "20", timings,
    });

    assert.deepEqual(progress.map(({ processedPoints }) => processedPoints), ["12"]);
    assert.deepEqual(await resultPromise, {
        file: output, name: "points.oct", attributesFile,
        outputAttributesName: "points.octattrs", points: "20", timings,
    });
    converter.dispose();
    assert.equal(worker.terminated, true);
});

test("validates input and prevents overlapping conversions", { concurrency: false }, async () => {
    const converter = createConverter();
    await assert.rejects(converter.convert(lasFile("points.txt")), /\.las or \.laz/);
    const active = converter.convert(lasFile());
    await assert.rejects(converter.convert(lasFile()), /already in progress/);
    converter.cancel();
    assert.equal(workers[0].terminated, true);
    await assert.rejects(active, { name: "AbortError" });
    converter.dispose();
});

test("dispose rejects an active conversion and releases its worker", { concurrency: false }, async () => {
    const converter = createConverter();
    const pending = converter.convert(lasFile());
    const worker = workers[0];
    converter.dispose();
    await assert.rejects(pending, /disposed/);
    assert.equal(worker.terminated, true);
    await assert.rejects(converter.convert(lasFile()), /disposed/);
});

test("a synchronous postMessage failure rejects and releases the conversion slot", { concurrency: false }, async () => {
    const converter = createConverter();
    const worker = workers[0];
    worker.postMessageError = new Error("worker send failed");

    const failed = converter.convert(lasFile());
    await assert.rejects(failed, /worker send failed/);

    worker.postMessageError = null;
    const retry = converter.convert(lasFile());
    assert.equal(worker.messages.at(-1).type, "convert");
    worker.dispatch("message", {
        type: "complete", file: new Blob(["oct"]), outputName: "points.oct", points: "1",
    });
    await retry;
    converter.dispose();
});

test("messageerror emits an error and rejects the active conversion", { concurrency: false }, async () => {
    const converter = createConverter();
    const worker = workers[0];
    const errors = [];
    converter.on("error", (error) => errors.push(error.message));
    const active = converter.convert(lasFile());

    worker.dispatch("messageerror", { message: "message decode failed" });

    await assert.rejects(active, /message decode failed/);
    assert.deepEqual(errors, ["message decode failed"]);
    converter.dispose();
});

test("a worker error message recycles the worker on the next conversion", { concurrency: false }, async () => {
    const converter = createConverter();
    const first = workers[0];
    const failed = converter.convert(lasFile());
    first.dispatch("message", { type: "error", message: "RuntimeError: unreachable" });
    await assert.rejects(failed, /unreachable/);
    assert.equal(first.terminated, true);
    assert.equal(workers.length, 1);

    const retry = converter.convert(lasFile());
    assert.equal(workers.length, 2);
    assert.equal(first.terminateCount, 1);
    assert.equal(workers[1].messages.at(-1).type, "convert");
    workers[1].dispatch("message", { type: "complete", file: new Blob(["oct"]), outputName: "p.oct", points: "1" });
    await retry;
    converter.dispose();
    assert.equal(first.terminateCount, 1);
    assert.equal(workers[1].terminated, true);
});

test("a cancellation respawns the worker for the next conversion", { concurrency: false }, async () => {
    const converter = createConverter();
    const active = converter.convert(lasFile());
    converter.cancel();
    await assert.rejects(active, { name: "AbortError" });
    const next = converter.convert(lasFile());
    assert.equal(workers.length, 2);
    assert.equal(workers[0].terminated, true);
    workers[1].dispatch("message", { type: "complete", file: new Blob(["oct"]), outputName: "p.oct", points: "1" });
    await next;
    converter.dispose();
});

test("a worker error event recycles the worker on the next conversion", { concurrency: false }, async () => {
    const converter = createConverter();
    const first = workers[0];
    const failed = converter.convert(lasFile());
    first.dispatch("error", { message: "worker crashed" });
    await assert.rejects(failed, /worker crashed/);
    assert.equal(first.terminated, true);
    const retry = converter.convert(lasFile());
    assert.equal(workers.length, 2);
    assert.equal(first.terminateCount, 1);
    workers[1].dispatch("message", { type: "complete", file: new Blob(["oct"]), outputName: "p.oct", points: "1" });
    await retry;
    converter.dispose();
});
