import assert from "node:assert/strict";
import test from "node:test";
import zlib from "node:zlib";

import { BACKGROUND_RGB, convexHull, countEmptyPixels, decodePng, projectBounds } from "../pixel-metrics.mjs";

function chunk(type, body) {
    const head = Buffer.alloc(8);
    head.writeUInt32BE(body.length, 0);
    head.write(type, 4, "latin1");
    return Buffer.concat([head, body, Buffer.alloc(4)]); // CRC is not checked by the decoder.
}

// Rows use filter 0 (none), 1 (sub) or 2 (up) to exercise the predictors.
function encodePng(width, height, rgb, filters = []) {
    const rows = [];
    for (let y = 0; y < height; y++) {
        const filter = filters[y] ?? 0;
        const row = Buffer.alloc(1 + width * 3);
        row[0] = filter;
        for (let x = 0; x < width * 3; x++) {
            const value = rgb[y * width * 3 + x];
            const left = x >= 3 ? rgb[y * width * 3 + x - 3] : 0;
            const up = y > 0 ? rgb[(y - 1) * width * 3 + x] : 0;
            row[1 + x] = (value - (filter === 1 ? left : filter === 2 ? up : 0)) & 0xff;
        }
        rows.push(row);
    }
    const ihdr = Buffer.alloc(13);
    ihdr.writeUInt32BE(width, 0);
    ihdr.writeUInt32BE(height, 4);
    ihdr[8] = 8;
    ihdr[9] = 2;
    return Buffer.concat([
        Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
        chunk("IHDR", ihdr), chunk("IDAT", zlib.deflateSync(Buffer.concat(rows))), chunk("IEND", Buffer.alloc(0)),
    ]);
}

test("decodes RGB PNG rows that use none, sub and up filters", () => {
    const rgb = Uint8Array.from({ length: 4 * 3 * 3 }, (_, index) => (index * 37) & 0xff);
    const image = decodePng(encodePng(4, 3, rgb, [1, 2, 0]));
    assert.equal(image.width, 4);
    assert.equal(image.height, 3);
    assert.deepEqual([...image.rgba.subarray(0, 8)], [rgb[0], rgb[1], rgb[2], 255, rgb[3], rgb[4], rgb[5], 255]);
    assert.equal(image.rgba[(2 * 4 + 3) * 4 + 1], rgb[(2 * 4 + 3) * 3 + 1]);
    assert.throws(() => decodePng(Buffer.from("nope nope")), /not a PNG/);
});

test("counts background pixels inside the whole image or a convex region only", () => {
    const width = 10;
    const height = 10;
    const rgba = new Uint8Array(width * height * 4);
    for (let i = 0; i < width * height; i++) rgba.set([...BACKGROUND_RGB, 255], i * 4);
    for (let y = 0; y < 5; y++) for (let x = 0; x < 5; x++) rgba.set([200, 200, 200, 255], (y * width + x) * 4);
    const image = { width, height, rgba };

    assert.deepEqual(countEmptyPixels(image), { regionPixels: 100, emptyPixels: 75, emptyFraction: 0.75 });
    const square = [[0, 0], [5, 0], [5, 5], [0, 5]];
    assert.equal(countEmptyPixels(image, square).emptyPixels, 0);
    const lower = [[0, 5], [10, 5], [10, 10], [0, 10]];
    assert.deepEqual(countEmptyPixels(image, lower), { regionPixels: 50, emptyPixels: 50, emptyFraction: 1 });
    assert.equal(countEmptyPixels(image, [[20, 20], [30, 20], [30, 30]]).emptyFraction, null);
});

test("projects bounds in front of the camera and reports null when a corner is behind it", () => {
    const corners = projectBounds([0, -10, 0], [0, 0, 0], [-1, -1, -1], [1, 1, 1], 200, 100);
    assert.equal(corners.length, 8);
    const xs = corners.map(point => point[0]);
    const ys = corners.map(point => point[1]);
    assert.ok(Math.min(...xs) > 60 && Math.max(...xs) < 140, `x range ${xs}`);
    assert.ok(Math.abs((Math.min(...ys) + Math.max(...ys)) / 2 - 50) < 1e-9);
    assert.equal(convexHull(corners).length >= 4, true);
    assert.equal(projectBounds([0, 0, 0], [0, 1, 0], [-1, -1, -1], [1, 1, 1], 200, 100), null);
});
