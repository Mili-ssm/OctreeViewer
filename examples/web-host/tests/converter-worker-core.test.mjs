import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import { test } from "node:test";

// The worker is evaluated the way package-local.py ships it: spool-tiers inlined, imports removed.
const spoolTiers = (await readFile(new URL("../spool-tiers.mjs", import.meta.url), "utf8"))
    .replace(/^export /gm, "");
const source = spoolTiers + "\n" + (await readFile(new URL("../converter-worker.js", import.meta.url), "utf8"))
    .replace(/^import .*\n/gm, "");

async function run({ fail = null, zones = 2, quota = null } = {}) {
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
            return { zones, spilledPeakBytes: 3e9, outputBytes: 1e9 };
        }
        constructor(file, primary, attrs, options, onProgress, spoolHost) {
            assert.notEqual(primary, attrs);
            Object.assign(seen, { options, spoolHost, zoneCount: spoolHost.zoneCount });
            this.onProgress = onProgress;
        }
        convert() {
            this.onProgress({ stage: "writing", processedPoints: 1, totalPoints: 2 });
            if (fail) throw fail;
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
            storage: {
                async getDirectory() { return { async getDirectoryHandle() { return directory; } }; },
                async estimate() { return quota; },
            },
        },
        crypto: { randomUUID: () => "core-pair" }, performance, setTimeout, Date, DOMException,
        init: async () => {}, WasmLasOctConverter: class {}, WasmCoreConverter: CoreConverter,
    });
    receive({ data: { type: "convert", engine: "core", options: { pointsPerBlock: 4096 }, file: { name: "source.las" } } });
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
    assert.equal(seen.options.pointsPerBlock, 4096);
    assert.equal(seen.estimateOptions.pointsPerBlock, 4096);
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
