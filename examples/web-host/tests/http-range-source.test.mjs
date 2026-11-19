import assert from "node:assert/strict";
import { mkdtempSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";

import { HttpRangeError, HttpRangeSource, readConcurrency, resolveSource, sidecarUrl } from "../http-range-source.mjs";
import { parseRange, startServer } from "../red-ref-server.mjs";

const SIZE = 3000;
const content = Uint8Array.from({ length: SIZE }, (_, i) => (i * 7) % 251);
let dir;
let server;
let ignoring;
let url;
let ignoringUrl;

before(async () => {
  dir = mkdtempSync(join(tmpdir(), "red-ref-"));
  writeFileSync(join(dir, "data.bin"), content);
  server = await startServer({ root: dir });
  ignoring = await startServer({ root: dir, ignoreRange: true });
  url = `http://127.0.0.1:${server.port}/data.bin`;
  ignoringUrl = `http://127.0.0.1:${ignoring.port}/data.bin`;
});
after(async () => {
  await server.close();
  await ignoring.close();
  rmSync(dir, { recursive: true, force: true });
});

const bytesOf = async (source, start, end) => new Uint8Array(await source.slice(start, end).arrayBuffer());

test("parseRange understands single ranges and refuses the rest", () => {
  assert.deepEqual(parseRange("bytes=10-19", 100), { start: 10, end: 19 });
  assert.deepEqual(parseRange("bytes=90-", 100), { start: 90, end: 99 });
  assert.deepEqual(parseRange("bytes=-5", 100), { start: 95, end: 99 });
  assert.deepEqual(parseRange("bytes=0-999", 100), { start: 0, end: 99 });
  assert.equal(parseRange("bytes=0-1,5-6", 100), "multi");
  assert.equal(parseRange("bytes=100-", 100), "unsatisfiable");
  assert.equal(parseRange("items=1-2", 100), null);
});

test("server answers 206 with CORS, ETag and honours If-Range", async () => {
  const first = await fetch(url, { headers: { Range: "bytes=5-9" } });
  assert.equal(first.status, 206);
  assert.equal(first.headers.get("Content-Range"), `bytes 5-9/${SIZE}`);
  assert.equal(first.headers.get("Access-Control-Allow-Origin"), "*");
  assert.match(first.headers.get("Access-Control-Expose-Headers"), /Content-Range/);
  const etag = first.headers.get("ETag");
  const same = await fetch(url, { headers: { Range: "bytes=5-9", "If-Range": etag } });
  assert.equal(same.status, 206);
  const stale = await fetch(url, { headers: { Range: "bytes=5-9", "If-Range": '"old"' } });
  assert.equal(stale.status, 200);
  assert.equal((await stale.arrayBuffer()).byteLength, SIZE);
  const multi = await fetch(url, { headers: { Range: "bytes=0-1,5-6" } });
  assert.equal(multi.status, 416);
  assert.ok(server.log.some((entry) => entry.multiRange));
  const preflight = await fetch(url, { method: "OPTIONS" });
  assert.equal(preflight.status, 204);
  assert.match(preflight.headers.get("Access-Control-Allow-Headers"), /If-Range/);
});

test("open is one request and the tail serves later reads from memory", async () => {
  server.reset();
  const source = await HttpRangeSource.open(url, { tailBytes: 1000 });
  assert.equal(source.size, SIZE);
  assert.ok(source.etag);
  assert.deepEqual(await bytesOf(source, SIZE - 40, SIZE), content.slice(SIZE - 40));
  assert.deepEqual(await bytesOf(source, SIZE - 1000, SIZE - 500), content.slice(SIZE - 1000, SIZE - 500));
  assert.equal(server.log.length, 1);
  assert.equal(server.log[0].range, "bytes=-1000");
  assert.equal(source.stats.requests, 1);
});

test("reads of the same tick that touch are one single-range request pinned by If-Range", async () => {
  const source = await HttpRangeSource.open(url, { tailBytes: 100 });
  server.reset();
  const [a, b, c, far] = await Promise.all([
    bytesOf(source, 100, 200),
    bytesOf(source, 200, 350),
    bytesOf(source, 150, 220),
    bytesOf(source, 1000, 1100),
  ]);
  assert.deepEqual(a, content.slice(100, 200));
  assert.deepEqual(b, content.slice(200, 350));
  assert.deepEqual(c, content.slice(150, 220));
  assert.deepEqual(far, content.slice(1000, 1100));
  assert.deepEqual(server.log.map((e) => e.range), ["bytes=100-349", "bytes=1000-1099"]);
  assert.ok(server.log.every((e) => e.ifRange === source.etag && !e.multiRange));
});

test("headBytes prefetches the start of the file", async () => {
  const source = await HttpRangeSource.open(url, { tailBytes: 40, headBytes: 24 });
  server.reset();
  assert.deepEqual(await bytesOf(source, 0, 24), content.slice(0, 24));
  assert.deepEqual(await bytesOf(source, SIZE - 40, SIZE), content.slice(SIZE - 40));
  assert.equal(server.log.length, 0);
});

test("a server that ignores Range is detected and aborted", async () => {
  await assert.rejects(HttpRangeSource.open(ignoringUrl), (error) => {
    assert.ok(error instanceof HttpRangeError);
    assert.match(error.message, /ignores Range/);
    return true;
  });
});

test("a file that changes after open is an error, not mixed bytes", async () => {
  const path = join(dir, "moving.bin");
  writeFileSync(path, content);
  const source = await HttpRangeSource.open(`http://127.0.0.1:${server.port}/moving.bin`, { tailBytes: 100 });
  writeFileSync(path, content.slice().reverse());
  utimesSync(path, new Date(), new Date(Date.now() + 5000));
  await assert.rejects(bytesOf(source, 0, 50), (error) => error instanceof HttpRangeError);
});

test("HTTP errors surface with their status", async () => {
  await assert.rejects(HttpRangeSource.open(`http://127.0.0.1:${server.port}/missing.bin`), /HTTP 404/);
});

test("resolveSource passes Blob-likes through and opens URL descriptors", async () => {
  const blob = new Blob([content]);
  assert.equal(await resolveSource(blob), blob);
  assert.equal(await resolveSource(undefined), undefined);
  const source = await resolveSource({ url, tailBytes: 64 });
  assert.equal(source.size, SIZE);
});

test("maxGap merges reads across a gap and counts the over-read as gapBytes", async () => {
  const source = await HttpRangeSource.open(url, { tailBytes: 100, maxGap: 300 });
  server.reset();
  const [a, b, far] = await Promise.all([
    bytesOf(source, 100, 200),
    bytesOf(source, 400, 500), // 200 B gap after `a`: merged
    bytesOf(source, 1000, 1100), // 500 B gap: separate
  ]);
  assert.deepEqual(a, content.slice(100, 200));
  assert.deepEqual(b, content.slice(400, 500));
  assert.deepEqual(far, content.slice(1000, 1100));
  assert.deepEqual(server.log.map((e) => e.range), ["bytes=100-499", "bytes=1000-1099"]);
  assert.equal(source.stats.gapBytes, 200);
  source.resetStats();
  assert.equal(source.stats.gapBytes, 0);
});

test("overlapping reads add no gapBytes and the default maxGap never over-reads", async () => {
  const merged = await HttpRangeSource.open(url, { tailBytes: 100, maxGap: 50 });
  await Promise.all([bytesOf(merged, 100, 200), bytesOf(merged, 150, 300), bytesOf(merged, 320, 330)]);
  assert.equal(merged.stats.gapBytes, 20);
  const plain = await HttpRangeSource.open(url, { tailBytes: 100 });
  await Promise.all([bytesOf(plain, 100, 200), bytesOf(plain, 201, 210)]);
  assert.equal(plain.stats.requests, 3); // open + two separate reads
  assert.equal(plain.stats.gapBytes, 0);
});

test("readConcurrency: local sources keep 4, remote ones default to 8 or take remoteReads", () => {
  assert.equal(readConcurrency(new Blob([content])), 4);
  assert.equal(readConcurrency({ url: "http://x/a.oct" }), 8);
  assert.equal(readConcurrency({ url: "http://x/a.oct", remoteReads: 16 }), 16);
  assert.equal(readConcurrency({ url: "http://x/a.oct", remoteReads: 0 }), 8);
  assert.equal(readConcurrency({ url: "http://x/a.oct", remoteReads: "abc" }), 8);
  assert.equal(readConcurrency({ url: "http://x/a.oct", remoteReads: 9999 }), 64);
  assert.equal(readConcurrency({ url: "http://x/a.oct", remoteReads: 8, slice() {} }), 4);
});

test("resolveSource forwards maxGap from the descriptor", async () => {
  const source = await resolveSource({ url, tailBytes: 64, maxGap: 300 });
  server.reset();
  await Promise.all([bytesOf(source, 100, 200), bytesOf(source, 400, 500)]);
  assert.equal(server.log.length, 1);
  assert.equal(source.stats.gapBytes, 200);
});

test("sidecarUrl appends attrs to the path, keeps the query and drops the fragment", () => {
  assert.equal(sidecarUrl("https://h.example/d/a.oct"), "https://h.example/d/a.octattrs");
  assert.equal(sidecarUrl(new URL("https://h.example/d/a.oct?v=1&sig=x#top")), "https://h.example/d/a.octattrs?v=1&sig=x");
  assert.equal(sidecarUrl("https://h.example/a%20b.oct#f"), "https://h.example/a%20b.octattrs");
});

test("sidecarUrl rejects a trailing slash and a path that already ends in .octattrs", () => {
  assert.throws(() => sidecarUrl("https://h.example/d/"), TypeError);
  assert.throws(() => sidecarUrl("https://h.example/d/?x=a.oct"), TypeError);
  assert.throws(() => sidecarUrl("https://h.example/a.octattrs?v=1"), TypeError);
  assert.throws(() => sidecarUrl("not absolute"), TypeError);
});
