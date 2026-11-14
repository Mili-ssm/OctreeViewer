import assert from "node:assert/strict";
import { test } from "node:test";

import { DWELL_MS, PX_LADDER, WINDOW_MS, createPxController } from "../px-controller.mjs";

// Feeds one window per second of simulated time; returns [timeMs, newPx] for every change.
function run(controller, signals, { source = "gpu", startMs = 0 } = {}) {
    const changes = [];
    signals.forEach((value, i) => {
        const now = startMs + (i + 1) * WINDOW_MS;
        const signal = source === "gpu" ? { gpuP95Ms: value } : { intervalP95Ms: value };
        const next = controller.update(now, value === null ? {} : signal);
        if (next !== null) changes.push([now, next]);
    });
    return changes;
}

test("steady load between the thresholds keeps the target", () => {
    const controller = createPxController({ initialPx: 4 });
    assert.deepEqual(run(controller, Array(30).fill(13)), []);
    assert.equal(controller.targetPx, 4);
});

test("overload coarsens one step per dwell and clamps at the coarsest rung", () => {
    const controller = createPxController({ initialPx: 2 });
    const changes = run(controller, Array(10).fill(25));
    assert.deepEqual(changes.map(([, px]) => px), [4, 8]);
    assert.ok(changes[1][0] - changes[0][0] >= DWELL_MS);
    assert.equal(controller.targetPx, PX_LADDER.at(-1));
});

test("headroom refines after two windows and clamps at the finest rung", () => {
    const controller = createPxController({ initialPx: 4 });
    const changes = run(controller, Array(12).fill(6));
    assert.deepEqual(changes.map(([, px]) => px), [2, 1]);
    assert.equal(changes[0][0], 2 * WINDOW_MS, "two consecutive windows, no dwell at start");
    assert.equal(controller.targetPx, 1);
});

test("one window with headroom is not enough, and a bad window resets the streak", () => {
    const controller = createPxController({ initialPx: 4 });
    assert.deepEqual(run(controller, [6, 13, 6, 13, 6, 13]), []);
});

test("alternating loads around the thresholds never change faster than the dwell", () => {
    const controller = createPxController({ initialPx: 4 });
    const signals = Array.from({ length: 60 }, (_, i) => (i % 3 === 0 ? 20 : 6));
    const changes = run(controller, signals);
    assert.ok(changes.length > 0, "the overload windows do coarsen");
    for (let i = 1; i < changes.length; i++) assert.ok(changes[i][0] - changes[i - 1][0] >= DWELL_MS);
});

test("a heavy change then light load waits out the dwell before refining", () => {
    const controller = createPxController({ initialPx: 4 });
    const changes = run(controller, [25, 6, 6, 6, 6]);
    assert.deepEqual(changes, [[1000, 8], [3000, 4], [5000, 2]]);
});

test("without GPU timestamps the interval signal coarsens at 20 ms and refines only after 3 windows under 17.5 ms", () => {
    const controller = createPxController({ initialPx: 4 });
    assert.deepEqual(run(controller, [19, 19, 19], { source: "interval" }), [], "19 ms is not overload nor headroom");
    const refine = createPxController({ initialPx: 4 });
    assert.deepEqual(run(refine, [17, 17], { source: "interval" }), []);
    assert.deepEqual(run(createPxController({ initialPx: 4 }), [17, 17, 17], { source: "interval" }), [[3000, 2]]);
    assert.deepEqual(run(createPxController({ initialPx: 4 }), [21], { source: "interval" }), [[1000, 8]]);
    assert.deepEqual(controller.last, { signalMs: 19, source: "interval" });
});

test("empty windows hold the target and reset the headroom streak", () => {
    const controller = createPxController({ initialPx: 4 });
    assert.deepEqual(run(controller, [6, null, 6, null]), []);
    assert.deepEqual(controller.last, { signalMs: null, source: null });
});

test("the initial px snaps to the ladder", () => {
    assert.equal(createPxController({ initialPx: 3 }).targetPx, 4);
    assert.equal(createPxController({ initialPx: 0.5 }).targetPx, 1);
    assert.equal(createPxController({ initialPx: 16 }).targetPx, 8);
});
