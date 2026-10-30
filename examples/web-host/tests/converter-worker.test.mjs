import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import { test } from "node:test";

const source = (await readFile(new URL("../converter-worker.js", import.meta.url), "utf8"))
    .replace(/^import .*\n/gm, "");

async function send(data) {
    const messages = [], converted = [];
    let receive;
    class Converter {
        static estimate() { throw new Error("stop after dispatch"); }
    }
    vm.runInNewContext(source, {
        self: { addEventListener(_name, callback) { receive = callback; }, postMessage(message) { messages.push(message); } },
        navigator: {},
        init: async () => { converted.push(data.engine ?? "default"); },
        WasmCoreConverter: Converter, performance, DOMException,
    });
    receive({ data });
    for (let i = 0; i < 10; i++) await new Promise(setImmediate);
    return { messages, converted };
}

test("a convert message without engine runs the core converter", async () => {
    const { messages, converted } = await send({ type: "convert", file: { name: "source.las" } });
    assert.deepEqual(converted, ["default"]);
    assert.match(messages[0].message, /stop after dispatch/);
});

test("engine core is accepted", async () => {
    const { converted } = await send({ type: "convert", engine: "core", file: { name: "source.las" } });
    assert.deepEqual(converted, ["core"]);
});

test("engine legacy fails with a clear message before touching the converter", async () => {
    const { messages, converted } = await send({ type: "convert", engine: "legacy", file: { name: "source.las" } });
    assert.deepEqual(converted, []);
    assert.equal(messages.length, 1);
    assert.equal(messages[0].type, "error");
    assert.equal(messages[0].message, 'the legacy converter was removed; omit `engine` or use "core"');
});
