import assert from "node:assert/strict";
import test from "node:test";

import { CoalescedCommandQueue } from "../coalesced-command-queue.mjs";

test("keeps one command in flight and sends only the latest pending value", () => {
    const sent = [];
    const queue = new CoalescedCommandQueue((type, values) => sent.push({ type, values }));

    queue.enqueue("resize", { width: 800, height: 600 });
    queue.enqueue("resize", { width: 1024, height: 768 });
    queue.enqueue("resize", { width: 1280, height: 720 });

    assert.deepEqual(sent, [{ type: "resize", values: { width: 800, height: 600 } }]);
    assert.equal(queue.acknowledge("resize"), true);
    assert.deepEqual(sent[1], { type: "resize", values: { width: 1280, height: 720 } });
    assert.equal(queue.acknowledge("resize"), true);
    assert.equal(queue.acknowledge("resize"), false);
});

test("allows distinct command kinds while bounding each kind independently", () => {
    const sent = [];
    const queue = new CoalescedCommandQueue((type, values) => sent.push({ type, values }));

    queue.enqueue("color", { value: "rgb" });
    queue.enqueue("voxelPixels", { value: 1 });
    queue.enqueue("voxelPixels", { value: 4 });
    queue.acknowledge("color");
    assert.equal(sent.length, 2);
    queue.acknowledge("voxelPixels");

    assert.deepEqual(sent, [
        { type: "color", values: { value: "rgb" } },
        { type: "voxelPixels", values: { value: 1 } },
        { type: "voxelPixels", values: { value: 4 } },
    ]);
});

test("reset discards pending work and invalidates acknowledgements", () => {
    const sent = [];
    const queue = new CoalescedCommandQueue((type, values) => sent.push({ type, values }));

    queue.enqueue("fit");
    queue.enqueue("fit");
    queue.reset();

    assert.equal(queue.acknowledge("fit"), false);
    assert.deepEqual(sent, [{ type: "fit", values: {} }]);
});
