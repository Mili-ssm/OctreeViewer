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
    assert.equal(workers[0].messages.at(-1).type, "cancel");
    workers[0].dispatch("message", { type: "cancelled", message: "Conversion cancelled." });
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
