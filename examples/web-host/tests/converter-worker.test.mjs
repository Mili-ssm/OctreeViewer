import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import { test } from "node:test";

const source = (await readFile(new URL("../converter-worker.js", import.meta.url), "utf8"))
    .replace(/^import .*\n/, "");

async function run(failure = false, cancel = false) {
    const messages = [], removed = [], handles = [];
    let receive;
    const directory = {
        async getFileHandle(name) {
            const handle = { closed: false, flush() {}, truncate() {}, close() { this.closed = true; } };
            handles.push(handle);
            return { async createSyncAccessHandle() { return handle; }, async getFile() { return { name }; } };
        },
        async removeEntry(name) { removed.push(name); },
    };
    class Converter {
        constructor(_input, primary, attrs) { assert.notEqual(primary, attrs); }
        convertNext() {
            if (cancel) receive({ data: { type: "cancel" } });
            return { done: true, stage: "writing", totalPoints: "2" };
        }
        finish() { if (failure) throw new Error("disk full"); }
        free() {}
    }
    vm.runInNewContext(source, {
        self: { addEventListener(_name, callback) { receive = callback; }, postMessage(message) { messages.push(message); } },
        navigator: { storage: { async getDirectory() { return { async getDirectoryHandle() { return directory; } }; } } },
        crypto: { randomUUID: () => "test-pair" }, performance, setTimeout, DOMException,
        init: async () => {}, WasmLasOctConverter: Converter,
    });
    receive({ data: { type: "convert", file: { name: "source.las" } } });
    for (let i = 0; i < 10; i++) await new Promise(setImmediate);
    return { messages, removed, handles };
}

test("worker completes both output files and closes both handles", async () => {
    const { messages, removed, handles } = await run();
    const result = messages.find((message) => message.type === "complete");
    assert.ok(result.file);
    assert.ok(result.attributesFile);
    assert.equal(result.outputAttributesName, "source.octattrs");
    assert.equal(handles.length, 2);
    assert.ok(handles.every((handle) => handle.closed));
    assert.equal(removed.length, 0);
});
for (const scenario of ["failure", "cancel"]) {
    test(`worker cleans both files after ${scenario}`, async () => {
        const { messages, removed, handles } = await run(scenario === "failure", scenario === "cancel");
        assert.ok(messages.some((message) => message.type === (scenario === "failure" ? "error" : "cancelled")));
        assert.equal(handles.length, 2);
        assert.ok(handles.every((handle) => handle.closed));
        assert.deepEqual(removed.sort(), ["conversion-test-pair.oct.partial", "conversion-test-pair.octattrs.partial"]);
        assert.ok(!messages.some((message) => message.type === "complete"));
    });
}
