import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import { test } from "node:test";

const source = (await readFile(new URL("../converter-worker.js", import.meta.url), "utf8"))
    .replace(/^import .*\n/, "");

async function run({ fail = false } = {}) {
    const messages = [], handles = [], removed = [], seen = {};
    let receive;
    const directory = {
        async getFileHandle(name) {
            const handle = { closed: false, flush() {}, truncate() {}, close() { this.closed = true; } };
            handles.push(handle);
            return { async createSyncAccessHandle() { return handle; }, async getFile() { return { name }; } };
        },
        async removeEntry(name) { removed.push(name); },
    };
    class CoreConverter {
        constructor(file, primary, attrs, options, onProgress) {
            assert.notEqual(primary, attrs);
            Object.assign(seen, { options });
            this.onProgress = onProgress;
        }
        convert() {
            this.onProgress({ stage: "writing", processedPoints: 1, totalPoints: 2 });
            if (fail) throw new Error("conversion failed: bad input");
            return { pointCount: "2", blockCount: 3 };
        }
        free() {}
    }
    vm.runInNewContext(source, {
        self: { addEventListener(_name, callback) { receive = callback; }, postMessage(message) { messages.push(message); } },
        navigator: { storage: { async getDirectory() { return { async getDirectoryHandle() { return directory; } }; } } },
        crypto: { randomUUID: () => "core-pair" }, performance, setTimeout, DOMException,
        init: async () => {}, WasmLasOctConverter: class {}, WasmCoreConverter: CoreConverter,
    });
    receive({ data: { type: "convert", engine: "core", options: { pointsPerBlock: 4096 }, file: { name: "source.las" } } });
    for (let i = 0; i < 10; i++) await new Promise(setImmediate);
    return { messages, handles, removed, seen };
}

const names = ["conversion-core-pair.oct.partial", "conversion-core-pair.octattrs.partial"];

test("core engine announces its partial files, passes options and closes both handles", async () => {
    const { messages, handles, seen } = await run();
    assert.equal(seen.options.pointsPerBlock, 4096);
    const order = messages.map((message) => message.type);
    assert.ok(order.indexOf("partial-files") < order.indexOf("progress"));
    assert.deepEqual([...messages.find((message) => message.type === "partial-files").names], names);
    const done = messages.find((message) => message.type === "complete");
    assert.equal(done.points, "2");
    assert.equal(done.outputAttributesName, "source.octattrs");
    assert.ok(handles.every((handle) => handle.closed));
});

test("core engine failure reports the error, closes handles and removes both files", async () => {
    const { messages, handles, removed } = await run({ fail: true });
    assert.match(messages.find((message) => message.type === "error").message, /bad input/);
    assert.ok(handles.every((handle) => handle.closed));
    assert.deepEqual([...removed].sort(), names);
    assert.ok(!messages.some((message) => message.type === "complete"));
});
