import assert from "node:assert/strict";
import test from "node:test";

import {
    FRAME_RATE_WARMUP_MS, lodCounters, lodLogEntry, measureFrameRate, summarizeFrameReplies,
} from "../benchmark-metrics.mjs";

test("summarizes frame reply intervals, round trips and render calls over a window", () => {
    const replies = [0, 16, 32, 100, 116].map((time, index) => ({
        time, renderCallMs: index, roundTripMs: 10 + index * 10,
    }));
    const all = summarizeFrameReplies(replies);
    assert.equal(all.replies, 5);
    assert.deepEqual(all.intervalMs, { count: 4, p50: 16, p95: 68, max: 68 });
    assert.deepEqual(all.roundTripMs, { count: 5, p50: 30, p95: 50, max: 50 });
    assert.equal(all.renderCallMs.max, 4);

    const tail = summarizeFrameReplies(replies, 90);
    assert.equal(tail.replies, 2);
    assert.deepEqual(tail.intervalMs, { count: 1, p50: 16, p95: 16, max: 16 });
    assert.deepEqual(summarizeFrameReplies([]).intervalMs, { count: 0, p50: null, p95: null, max: null });
    const shifted = replies.map(reply => ({ ...reply, time: reply.time + 5_000 }));
    assert.equal(measureFrameRate(shifted, 0, 5_116).intervalMaxMs, 68);
});

test("builds deterministic counters and LOD log entries tolerant of missing fields", () => {
    const lod = {
        lodStagePixels: 8, targetVoxelPixels: 4, oversizedVoxels: 3, uncoveredNodes: 2,
        pendingBlockIds: ["1", "2"], visibleBlocks: 7,
        refinementWork: { sceneBuilds: 5, voxelSourcePoints: "123", loadedBlocks: 9, evictedPendingBlocks: 1 },
    };
    assert.deepEqual(lodLogEntry(42, "asyncInsert", lod), {
        time: 42, source: "asyncInsert", stagePx: 8, targetPx: 4, oversizedVoxels: 3,
        uncoveredNodes: 2, pendingBlocks: 2, visibleBlocks: 7,
    });
    assert.deepEqual(lodCounters(lod), {
        sceneBuilds: 5, voxelSourcePoints: "123", loadedBlocks: 9, evictedPendingBlocks: 1,
        uncoveredNodes: 2, oversizedVoxels: 3,
    });
    assert.equal(lodCounters(null), null);
    assert.equal(lodCounters({}).sceneBuilds, null);
});

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

test("keeps a complete observation window when replies do not land on its boundaries", () => {
    const replies = Array.from({ length: 625 }, (_, index) => ({
        time: 5_008 + index * 16,
        renderCallMs: 1,
    }));
    const result = measureFrameRate(replies, 0, 15_000);

    assert.equal(result.windowDurationMs, 10_000);
    assert.equal(result.durationMs, 9_984);
    assert.equal(result.meanFps, 62.5);
    assert.equal(measureFrameRate(replies, 0, 14_000).windowDurationMs, 9_000);
    assert.equal(measureFrameRate([], 0, 4_000).windowDurationMs, 0);
    assert.equal(measureFrameRate([], null, 15_000).windowDurationMs, 0);
    assert.equal(measureFrameRate([], 0, 15_000).meanFps, null);
});
