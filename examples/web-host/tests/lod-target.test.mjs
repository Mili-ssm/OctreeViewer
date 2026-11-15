import assert from "node:assert/strict";
import { test } from "node:test";

import { lodStillLoading, lodTargetReached } from "../benchmark-metrics.mjs";

const lod = (extra = {}) => ({
    coverageComplete: true, pendingBlockIds: [], lodStagePixels: 4, targetVoxelPixels: 4, lodError: null, ...extra,
});

test("the target is reached only without pending blocks or pages", () => {
    assert.equal(lodTargetReached(lod()), true);
    assert.equal(lodTargetReached(lod({ pendingPageAnchors: [] })), true);
    assert.equal(lodTargetReached(lod({ pendingBlockIds: ["1"] })), false);
    assert.equal(lodTargetReached(lod({ pendingPageAnchors: ["5"] })), false);
    assert.equal(lodTargetReached(lod({ lodStagePixels: 8 })), false);
    assert.equal(lodTargetReached(lod({ coverageComplete: false })), false);
});

test("a page the budget cannot hold is neither reached nor still loading", () => {
    const blocked = lod({ pendingPageAnchors: ["5"], pageBudgetExhausted: true });
    assert.equal(lodTargetReached(blocked), false);
    assert.equal(lodStillLoading(blocked), false);
    assert.equal(lodStillLoading(lod({ pendingPageAnchors: ["5"] })), true);
    assert.equal(lodStillLoading(lod()), false);
});
