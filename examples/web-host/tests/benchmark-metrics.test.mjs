import assert from "node:assert/strict";
import test from "node:test";

import { FRAME_RATE_WARMUP_MS, measureFrameRate } from "../benchmark-metrics.mjs";

test("measures frame reply cadence after the target LOD warmup", () => {
    const replies = Array.from({ length: 601 }, (_, index) => ({
        time: index * 25,
        renderCallMs: index % 10,
    }));
    const result = measureFrameRate(replies, 0, 15_000);

    assert.equal(FRAME_RATE_WARMUP_MS, 5_000);
    assert.equal(result.sampleCount, 400);
    assert.equal(result.durationMs, 10_000);
    assert.equal(result.meanFps, 40);
    assert.equal(result.intervalP50Ms, 25);
    assert.equal(result.intervalP95Ms, 25);
    assert.equal(result.renderCallP50Ms, 4);
    assert.equal(result.renderCallP95Ms, 9);
    assert.equal(result.renderCallP99Ms, 9);
});

test("does not report FPS before the target is stable or with fewer than two replies", () => {
    assert.equal(measureFrameRate([{ time: 5_000, renderCallMs: 1 }], 0, 5_000).meanFps, null);
    assert.equal(measureFrameRate([{ time: 1_000, renderCallMs: 1 }, { time: 2_000, renderCallMs: 1 }], null, 2_000).sampleCount, 0);
});
