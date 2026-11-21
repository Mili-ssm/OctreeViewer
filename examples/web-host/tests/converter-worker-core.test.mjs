import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import { test } from "node:test";

// The worker is evaluated the way package-local.py ships it: spool-tiers inlined, imports removed.
const spoolTiers = (await readFile(new URL("../spool-tiers.mjs", import.meta.url), "utf8"))
    .replace(/^export /gm, "");
const source = spoolTiers + "\n" + (await readFile(new URL("../converter-worker.js", import.meta.url), "utf8"))
    .replace(/^import .*\n/gm, "");

async function run({
    fail = null, zones = 2, quota = null, opfs = true, userAgent = "Chrome", persist = undefined,
    estimate = {}, options = { maxError: 0.5 },
} = {}) {
    const messages = [], handles = [], removed = [], events = [], seen = {};
    let receive;
    const directory = {
        async getFileHandle(name) {
            events.push(`create:${name}`);
            const handle = {
                name, closed: false, flush() {}, truncate() {},
                close() { this.closed = true; events.push(`close:${name}`); },
            };
            handles.push(handle);
            return { async createSyncAccessHandle() { return handle; }, async getFile() { return { name }; } };
        },
        async removeEntry(name) {
            removed.push(name);
            events.push(`remove:${name}`);
        },
    };
    class CoreConverter {
        static estimate(file, options) {
            seen.estimateOptions = options;
            events.push("estimate");
            return { zones, spilledPeakBytes: 3e9, outputBytes: 1e9, ramOnlyBytes: 1e9, ...estimate };
        }
        constructor(file, primary, attrs, options, onProgress, spoolHost) {
            assert.notEqual(primary, attrs);
            seen.constructed = true;
            seen.handles = { primary, attrs };
            Object.assign(seen, { options, spoolHost, zoneCount: spoolHost.zoneCount });
            this.onProgress = onProgress;
        }
        convert() {
            this.onProgress({ stage: "writing", processedPoints: 1, totalPoints: 2 });
            if (fail) throw fail;
            if (!opfs) {
                this.write = (handle, text) => handle.write(new TextEncoder().encode(text), { at: 0 });
                this.write(seen.handles.primary, "geometry-bytes");
                this.write(seen.handles.attrs, "attrs");
                // Validation reads the output back through the same handles.
                const view = new Uint8Array(64);
                seen.readBack = seen.handles.primary.read(view, { at: 0 });
            }
            return { pointCount: "2", blockCount: 3, spool: { spilledBytes: 7 } };
        }
        free() {}
    }
    const post = (message) => {
        messages.push(message);
        events.push(`post:${message.type}`);
    };
    vm.runInNewContext(source, {
        self: { addEventListener(_name, callback) { receive = callback; }, postMessage: post },
        navigator: {
            userAgent,
            storage: {
                async getDirectory() {
                    if (!opfs) throw new DOMException("denied", "SecurityError");
                    return { async getDirectoryHandle() { return directory; } };
                },
                async estimate() { return quota; },
                ...(persist === undefined ? {} : { async persist() { events.push("persist"); return persist; } }),
            },
        },
        TextEncoder, Uint8Array, Blob,
        crypto: { randomUUID: () => "core-pair" }, performance, setTimeout, Date, DOMException,
        init: async () => {}, WasmCoreConverter: CoreConverter,
    });
    receive({ data: { type: "convert", engine: "core", options, file: { name: "source.las" } } });
    for (let i = 0; i < 20; i++) await new Promise(setImmediate);
    return { messages, handles, removed, events, seen };
}

const output = ["conversion-core-pair.oct.partial", "conversion-core-pair.octattrs.partial"];
const zoneNames = (count) => Array.from({ length: count }, (_, i) => `conversion-core-pair.zone-${i}.partial`);

test("core engine sizes its zones from the estimate and announces every file before creating any", async () => {
    const { messages, events, seen } = await run({ zones: 3 });
    const names = [...output, ...zoneNames(3)];
    assert.deepEqual([...messages.find((message) => message.type === "partial-files").names], names);
    assert.equal(seen.zoneCount, 3);
    assert.equal(seen.options.maxError, 0.5);
    assert.equal(seen.estimateOptions.maxError, 0.5);
    const announced = events.indexOf("post:partial-files");
    assert.ok(announced >= 0);
    assert.deepEqual(events.filter((event) => event.startsWith("create:")), names.map((name) => `create:${name}`));
    assert.ok(events.findIndex((event) => event.startsWith("create:")) > announced);
    assert.ok(announced < events.indexOf("post:progress"));
});

test("core engine closes everything, drops the zone files and keeps the outputs on success", async () => {
    const { messages, handles, removed } = await run({ zones: 2 });
    const done = messages.find((message) => message.type === "complete");
    assert.equal(done.points, "2");
    assert.equal(done.outputAttributesName, "source.octattrs");
    assert.ok(handles.every((handle) => handle.closed));
    assert.deepEqual([...removed], zoneNames(2));
});

test("core engine failure reports the error, closes handles and removes every file", async () => {
    const { messages, handles, removed, events } = await run({ fail: new Error("conversion failed: bad input") });
    assert.match(messages.find((message) => message.type === "error").message, /bad input/);
    assert.ok(handles.every((handle) => handle.closed));
    assert.deepEqual([...removed].sort(), [...output, ...zoneNames(2)].sort());
    assert.ok(!messages.some((message) => message.type === "complete"));
    assert.ok(events.lastIndexOf("close:" + zoneNames(2)[1]) < events.indexOf("remove:" + output[0]));
});

test("a full browser store becomes a clear message and still cleans up", async () => {
    const fail = Object.assign(new Error("conversion failed: no space"), { name: "QuotaExceededError" });
    const { messages, handles, removed } = await run({ fail, quota: { quota: 10e9, usage: 7.5e9 } });
    const error = messages.find((message) => message.type === "error");
    assert.match(error.message, /not enough browser storage: need about 4\.0 GB, available 2\.5 GB/i);
    assert.ok(handles.every((handle) => handle.closed));
    assert.equal(removed.length, 4);
});

const GiB = 2 ** 30;

test("without OPFS the conversion runs RAM-only, warns, and delivers the output as Blobs", async () => {
    const { messages, seen, events } = await run({ opfs: false });
    assert.equal(seen.zoneCount, 0);
    assert.equal(seen.readBack, "geometry-bytes".length);
    assert.ok(!messages.some((message) => message.type === "partial-files"));
    const warning = messages.find((message) => message.type === "warning");
    assert.match(warning.message, /without browser storage.*limited to RAM.*http:\/\/localhost/);
    assert.ok(events.indexOf("post:warning") < events.indexOf("post:progress"));
    const done = messages.find((message) => message.type === "complete");
    assert.equal(done.outputName, "source.oct");
    assert.equal(done.outputAttributesName, "source.octattrs");
    assert.equal(await done.file.text(), "geometry-bytes");
    assert.equal(await done.attributesFile.text(), "attrs");
});

test("without OPFS an estimate above ramBudget is rejected before the converter exists", async () => {
    const { messages, seen } = await run({ opfs: false, estimate: { ramOnlyBytes: 5e9 } });
    assert.equal(seen.constructed, undefined);
    const error = messages.find((message) => message.type === "error");
    assert.match(error.message, /about 5\.0 GB of RAM but the limit is 4\.0 GB.*http:\/\/localhost/s);
    assert.ok(!messages.some((message) => message.type === "warning" || message.type === "complete"));
});

test("ramBudget comes from the options and is capped at 8 GB", async () => {
    const fits = await run({ opfs: false, estimate: { ramOnlyBytes: 6e9 }, options: { ramBudget: 7e9 } });
    assert.ok(fits.messages.some((message) => message.type === "complete"));
    const capped = await run({ opfs: false, estimate: { ramOnlyBytes: 9e9 }, options: { ramBudget: 64e9 } });
    assert.match(capped.messages.find((message) => message.type === "error").message, /limit is 8\.0 GB/);
});

// Quota rule (R4, C11): only Firefox without persist() is rejected up front (hard stop at 10 GiB - 1 MiB
// while estimate() may report more); Chrome enforces far more than it reports, so it is never rejected.
const need15 = { spilledPeakBytes: 12 * GiB, outputBytes: 3 * GiB };

test("Firefox without persistent storage is rejected when the need exceeds the 10 GiB limit", async () => {
    const { messages, seen, events } = await run({
        userAgent: "Mozilla/5.0 Gecko/20100101 Firefox/130.0", persist: false,
        quota: { quota: 100 * GiB, usage: 1 * GiB }, estimate: need15,
    });
    assert.equal(seen.constructed, undefined);
    assert.ok(!events.some((event) => event.startsWith("create:")));
    const error = messages.find((message) => message.type === "error");
    assert.match(error.message, /need about 16\.1 GB, available 9\.7 GB.*persistent storage/s);
});

test("Firefox with persistent storage granted uses the reported free space", async () => {
    const firefox = "Mozilla/5.0 Gecko/20100101 Firefox/130.0";
    const ok = await run({ userAgent: firefox, persist: true, quota: { quota: 476 * GiB, usage: 0 }, estimate: need15 });
    assert.ok(ok.messages.some((message) => message.type === "complete"));
    assert.ok(ok.events.indexOf("persist") < ok.events.findIndex((event) => event.startsWith("create:")));
});

test("Chrome reporting 10 GiB is not rejected for a 15 GiB need", async () => {
    const { messages, events } = await run({
        userAgent: "Mozilla/5.0 Chrome/130.0 Safari/537.36", persist: false,
        quota: { quota: 10 * GiB, usage: 0 }, estimate: need15,
    });
    assert.ok(events.includes("persist"));
    assert.ok(messages.some((message) => message.type === "complete"));
    assert.ok(!messages.some((message) => message.type === "error"));
});
