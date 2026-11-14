import assert from "node:assert/strict";
import { test } from "node:test";

import { HttpRangeError, HttpRangeSource } from "../http-range-source.mjs";

const SIZE = 1000;
const content = Uint8Array.from({ length: SIZE }, (_, i) => i % 251);

// A fake `fetch`: `script` entries are consumed in order and may be a status, an Error, "hang" or a
// function. The default answer serves the requested range with a stable ETag.
const response = (status, headers = {}, body = new Uint8Array(0)) => ({
  status,
  headers: { get: (name) => headers[name] ?? null },
  arrayBuffer: async () => body.buffer.slice(body.byteOffset, body.byteOffset + body.length),
});

const rangeResponse = (range) => {
  const suffix = /^bytes=-(\d+)$/.exec(range);
  const [start, end] = suffix
    ? [Math.max(0, SIZE - Number(suffix[1])), SIZE]
    : range.slice(6).split("-").map(Number).map((n, i) => (i ? n + 1 : n));
  return response(206, { ETag: '"v1"', "Content-Range": `bytes ${start}-${end - 1}/${SIZE}` }, content.slice(start, end));
};

const fakeFetch = (script = []) => {
  const calls = [];
  const signals = [];
  const fn = async (url, { headers, signal }) => {
    calls.push(headers.Range);
    signals.push(signal);
    const step = script[calls.length - 1];
    if (step instanceof Error) throw step;
    if (step === "hang") {
      return new Promise((_, reject) => {
        signal.addEventListener("abort", () => reject(signal.reason ?? new DOMException("aborted", "AbortError")));
      });
    }
    if (typeof step === "number") return response(step);
    if (typeof step === "function") return step(headers.Range);
    return rangeResponse(headers.Range);
  };
  fn.calls = calls;
  fn.signals = signals;
  return fn;
};

const open = (fetchImpl, options = {}) =>
  HttpRangeSource.open("http://x/data.bin", { tailBytes: 100, fetchImpl, random: () => 0.5, ...options });

test("503, 429 and network errors are retried with backoff and then succeed", async () => {
  const fetchImpl = fakeFetch([undefined, 503, 429, new TypeError("network"), undefined]);
  const source = await open(fetchImpl, { retryDelays: [1, 1, 1] });
  const bytes = new Uint8Array(await source.slice(0, 10).arrayBuffer());
  assert.deepEqual(bytes, content.slice(0, 10));
  assert.equal(source.stats.retries, 3);
  assert.equal(fetchImpl.calls.length, 5);
});

test("backoff delays grow and Retry-After is honoured up to its bound", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const slow = (range) => response(503, { "Retry-After": "3600" });
  const fetchImpl = fakeFetch([undefined, 503, 503, slow, 503]);
  const source = await open(fetchImpl, { retryDelays: [200, 800, 2400], timeoutMs: 0 });
  const outcome = assert.rejects(source.slice(0, 10).arrayBuffer(), /HTTP 503/);
  const settle = () => new Promise((resolve) => setImmediate(resolve));
  await settle();
  assert.equal(fetchImpl.calls.length, 2);
  t.mock.timers.tick(149); // 200 ms with jitter 0.5 -> 200
  await settle();
  assert.equal(fetchImpl.calls.length, 2);
  t.mock.timers.tick(51);
  await settle();
  assert.equal(fetchImpl.calls.length, 3);
  t.mock.timers.tick(800);
  await settle();
  assert.equal(fetchImpl.calls.length, 4);
  // third retry: Retry-After 3600 s is bounded to 10 s, not 2.4 s
  t.mock.timers.tick(9_999);
  await settle();
  assert.equal(fetchImpl.calls.length, 4);
  t.mock.timers.tick(1);
  await settle();
  assert.equal(fetchImpl.calls.length, 5);
  await outcome;
  assert.equal(source.stats.retries, 3);
});

test("404 and other non-429 4xx are never retried", async () => {
  const fetchImpl = fakeFetch([undefined, 404]);
  const source = await open(fetchImpl, { retryDelays: [1, 1, 1] });
  await assert.rejects(source.slice(0, 10).arrayBuffer(), /HTTP 404/);
  assert.equal(fetchImpl.calls.length, 2);
  assert.equal(source.stats.retries, 0);
});

test("an ETag change is an error without retries", async () => {
  const changed = (range) => {
    const ok = rangeResponse(range);
    return response(206, { ETag: '"v2"', "Content-Range": ok.headers.get("Content-Range") }, content.slice(0, 10));
  };
  const fetchImpl = fakeFetch([undefined, changed]);
  const source = await open(fetchImpl, { retryDelays: [1, 1, 1] });
  await assert.rejects(source.slice(0, 10).arrayBuffer(), /the file changed/);
  assert.equal(fetchImpl.calls.length, 2);
  assert.equal(source.stats.retries, 0);
});

test("a 200 answer (If-Range mismatch) is not retried", async () => {
  const fetchImpl = fakeFetch([undefined, 200]);
  const source = await open(fetchImpl, { retryDelays: [1, 1, 1] });
  await assert.rejects(source.slice(0, 10).arrayBuffer(), HttpRangeError);
  assert.equal(source.stats.retries, 0);
});

test("a timeout is a retryable failure, then an error once attempts run out", async () => {
  const fetchImpl = fakeFetch([undefined, "hang", "hang", undefined]);
  const source = await open(fetchImpl, { retryDelays: [1, 1, 1], timeoutMs: 20 });
  const bytes = new Uint8Array(await source.slice(0, 10).arrayBuffer());
  assert.deepEqual(bytes, content.slice(0, 10));
  assert.equal(source.stats.timeouts, 2);
  assert.equal(source.stats.retries, 2);

  const hung = fakeFetch([undefined, "hang", "hang", "hang", "hang"]);
  const failing = await open(hung, { retryDelays: [1, 1, 1], timeoutMs: 20 });
  await assert.rejects(failing.slice(0, 10).arrayBuffer(), /timeout after 20 ms/);
  assert.equal(failing.stats.timeouts, 4);
});

test("an abort through the signal is an AbortError, counted, never retried", async () => {
  const controller = new AbortController();
  const fetchImpl = fakeFetch([undefined, "hang"]);
  const source = await open(fetchImpl, { retryDelays: [1, 1, 1], signal: controller.signal });
  const read = source.slice(100, 400).arrayBuffer();
  await new Promise((resolve) => setImmediate(resolve));
  controller.abort();
  await assert.rejects(read, (error) => error.name === "AbortError");
  assert.equal(fetchImpl.calls.length, 2);
  assert.equal(source.stats.retries, 0);
  assert.equal(source.stats.aborted, 1);
  assert.equal(source.stats.abortedBytes, 300);
});

const tick = () => new Promise((resolve) => setImmediate(resolve));

// A fetch that answers the Nth call only when `release()` is called, so reads stay in flight.
const gatedFetch = () => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const inner = fakeFetch();
  const fn = async (url, init) => {
    if (inner.calls.length === 0) return inner(url, init); // the opening request
    inner.calls.push(init.headers.Range);
    fn.signals.push(init.signal);
    await Promise.race([
      gate,
      new Promise((_, reject) => init.signal.addEventListener("abort", () => reject(init.signal.reason))),
    ]);
    return rangeResponse(init.headers.Range);
  };
  fn.calls = inner.calls;
  fn.signals = [];
  fn.release = release;
  return fn;
};

test("a merged request is aborted only when all the reads it serves are aborted", async () => {
  const fetchImpl = gatedFetch();
  const source = await open(fetchImpl, { maxGap: 0 });
  const a = new AbortController();
  const b = new AbortController();
  const readA = assert.rejects(source.withSignal(a.signal).slice(100, 200).arrayBuffer(), (e) => e.name === "AbortError");
  const readB = assert.rejects(source.withSignal(b.signal).slice(200, 300).arrayBuffer(), (e) => e.name === "AbortError");
  await tick();
  assert.deepEqual(fetchImpl.calls.slice(1), ["bytes=100-299"]);
  a.abort();
  await readA;
  assert.equal(fetchImpl.signals[0].aborted, false);
  b.abort();
  await readB;
  assert.equal(fetchImpl.signals[0].aborted, true);
  assert.equal(source.stats.retries, 0);
  assert.equal(source.stats.aborted, 1);
  assert.equal(source.stats.abortedBytes, 200);
});

test("a partial abort keeps the request and the surviving read gets its bytes", async () => {
  const fetchImpl = gatedFetch();
  const source = await open(fetchImpl);
  const a = new AbortController();
  const b = new AbortController();
  const readA = assert.rejects(source.withSignal(a.signal).slice(100, 200).arrayBuffer(), (e) => e.name === "AbortError");
  const readB = source.withSignal(b.signal).slice(200, 300).arrayBuffer();
  await tick();
  a.abort();
  await readA;
  fetchImpl.release();
  assert.deepEqual(new Uint8Array(await readB), content.slice(200, 300));
  assert.equal(fetchImpl.signals[0].aborted, false);
  assert.equal(source.stats.aborted, 0);
});

test("a read without an owner keeps a merged request alive; an already aborted signal never fetches", async () => {
  const fetchImpl = gatedFetch();
  const source = await open(fetchImpl);
  const a = new AbortController();
  const readA = assert.rejects(source.withSignal(a.signal).slice(100, 200).arrayBuffer(), (e) => e.name === "AbortError");
  const plain = source.slice(200, 300).arrayBuffer();
  await tick();
  a.abort();
  await readA;
  fetchImpl.release();
  assert.deepEqual(new Uint8Array(await plain), content.slice(200, 300));
  const done = new AbortController();
  done.abort();
  await assert.rejects(source.withSignal(done.signal).slice(500, 600).arrayBuffer(), (e) => e.name === "AbortError");
  assert.equal(fetchImpl.calls.length, 2);
});

test("a signal view reads from the cache like the source and reports its size", async () => {
  const fetchImpl = fakeFetch();
  const source = await open(fetchImpl);
  const view = source.withSignal(new AbortController().signal);
  assert.equal(view.size, SIZE);
  assert.deepEqual(new Uint8Array(await view.slice(SIZE - 10, SIZE).arrayBuffer()), content.slice(SIZE - 10));
  assert.equal(fetchImpl.calls.length, 1);
});
