import assert from "node:assert/strict";
import test from "node:test";

import {
    MAX_FRAMEBUFFER_DIMENSION,
    MAX_FRAMEBUFFER_PIXELS,
    framebufferSize,
} from "../rendering-budget.mjs";

test("caps high-DPI framebuffers by pixel count and dimension", () => {
    const [width, height] = framebufferSize(3840, 2160, 2);

    assert.ok(width * height <= MAX_FRAMEBUFFER_PIXELS);
    assert.ok(width <= MAX_FRAMEBUFFER_DIMENSION);
    assert.ok(height <= MAX_FRAMEBUFFER_DIMENSION);
});

test("preserves small viewports while limiting the effective pixel ratio", () => {
    assert.deepEqual(framebufferSize(1280, 720, 1), [1280, 720]);
    assert.deepEqual(framebufferSize(1280, 720, 2), [2560, 1440]);
});

test("suspends zero-sized viewports", () => {
    assert.deepEqual(framebufferSize(0, 800, 2), [0, 0]);
});
