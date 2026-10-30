// Spike S5 residual (30-10-2026): throwaway measurement of what OPFS zones need.
//   test=big : one `gib`-GiB file written by a worker through a sync access handle in 1 MiB pages
//              of per-page-seeded xorshift noise; sequential read with verification of every page,
//              random reads (>= 96 pages, >= 32 above 2^32, first and last included), then
//              truncate(1 GiB) and truncate(0) with flush. The page parks after every phase
//              (window.zState / window.zGo) so that the driver can take df/du and estimate().
//   test=small: write({at}) throughput at 32 KiB, 64 KiB, 256 KiB and 1 MiB (noise pre-generated).
//   test=churn: one synchronous loop allocating `gib` GiB of segments with <= 2 GiB live (pool=0 new
//              Uint8Array per segment, pool=1 fixed pool); parks at baseline / churn-done / released.
//   test=truncq: usage/disk after truncate(0) with the handle open, after close(), after removeEntry().
//   test=term: a worker holds sync handle(s) mid-write, the page terminate()s it and retries
//              removeEntry (rm1, rm8) or createSyncAccessHandle from a fresh worker (open1).
//   test=ram : a worker retains `gib` GiB of `seg`-byte ArrayBuffers, parked at 1..gib GiB.
// Workers start from Blob URLs so that file:// works. Every worker body is a function.

const MiB = 1048576;
const GiB = 1073741824;

function bigWorker() {
  const PAGE = 1048576, GiB = 1073741824;
  const fill = (words, page) => {
    let s = (Math.imul(page + 1, 0x9e3779b1) ^ 0x85ebca6b) >>> 0 || 1;
    for (let i = 0; i < words.length; i++) {
      s ^= s << 13; s >>>= 0; s ^= s >>> 17; s ^= s << 5; s >>>= 0;
      words[i] = s;
    }
  };
  let go = null;
  const park = (msg) => new Promise((resolve) => { go = resolve; self.postMessage(msg); });
  self.onmessage = (e) => {
    if (e.data.type === "go" && go) { const g = go; go = null; g(); }
    else if (e.data.type === "start") run(e.data).catch((err) => self.postMessage({ type: "error", name: err && err.name, error: String(err) }));
  };
  async function run(m) {
    const pages = m.gib * 1024;
    const r = { pages, gib: m.gib };
    const root = await navigator.storage.getDirectory();
    const fh = await root.getFileHandle("zone-big", { create: true });
    const t0 = performance.now();
    const h = await fh.createSyncAccessHandle();
    r.openMs = performance.now() - t0;
    const buf = new Uint8Array(PAGE), words = new Uint32Array(buf.buffer);
    const exp = new Uint8Array(PAGE), expWords = new Uint32Array(exp.buffer);
    try {
      await park({ type: "state", label: "start" });
      // Write
      let writeMs = 0, fillMs = 0, shortWrite = null;
      const tw = performance.now();
      for (let p = 0; p < pages; p++) {
        const a = performance.now(); fill(words, p); const b = performance.now();
        const n = h.write(buf, { at: p * PAGE });
        const c = performance.now();
        fillMs += b - a; writeMs += c - b;
        if (n !== PAGE) { shortWrite = { page: p, n }; break; }
      }
      r.writeLoopMs = performance.now() - tw; r.writeMs = writeMs; r.fillMs = fillMs; r.shortWrite = shortWrite;
      const tf = performance.now(); h.flush(); r.flushMs = performance.now() - tf;
      r.writeMBps = (pages * PAGE) / 1e6 / (writeMs / 1e3);
      r.getSizeAfterWrite = h.getSize();
      await park({ type: "state", label: "written", size: r.getSizeAfterWrite });
      // Sequential read with verification of every page (verification time excluded from MB/s)
      let readMs = 0, bad = 0, firstBad = null;
      for (let p = 0; p < pages; p++) {
        const a = performance.now();
        const n = h.read(buf, { at: p * PAGE });
        readMs += performance.now() - a;
        fill(expWords, p);
        let ok = n === PAGE;
        for (let i = 0; ok && i < words.length; i++) if (words[i] !== expWords[i]) ok = false;
        if (!ok) { bad++; if (firstBad === null) firstBad = p; }
      }
      r.seqReadMBps = (pages * PAGE) / 1e6 / (readMs / 1e3); r.seqReadMs = readMs;
      r.seqBadPages = bad; r.seqFirstBad = firstBad;
      // Random reads: pages spread over the file; offsets above 2^32 counted
      const picks = new Set([0, pages - 1, 4095, 4096, 4097]);
      let x = 12345;
      while (picks.size < 128) { x = (Math.imul(x, 1103515245) + 12345) >>> 0; picks.add(x % pages); }
      let rrMs = 0, rrBad = 0, above = 0;
      for (const p of picks) {
        const off = p * PAGE;
        if (off >= 2 ** 32) above++;
        const a = performance.now();
        const n = h.read(buf, { at: off });
        rrMs += performance.now() - a;
        fill(expWords, p);
        let ok = n === PAGE;
        for (let i = 0; ok && i < words.length; i++) if (words[i] !== expWords[i]) ok = false;
        if (!ok) rrBad++;
      }
      r.randomPages = picks.size; r.randomAbove4GiB = above; r.randomBad = rrBad;
      r.randomReadMBps = (picks.size * PAGE) / 1e6 / (rrMs / 1e3);
      // A read that crosses the 2^32 boundary and one past the end
      const edge = new Uint8Array(65536);
      r.edgeCrossRead = h.read(edge, { at: 2 ** 32 - 32768 });
      r.edgePastEnd = h.read(edge, { at: pages * PAGE });
      await park({ type: "state", label: "read" });
      // Partial truncate, then full truncate
      let t = performance.now(); h.truncate(1 * GiB); r.truncate1GiBMs = performance.now() - t;
      t = performance.now(); h.flush(); r.flush1GiBMs = performance.now() - t; r.sizeAfter1GiB = h.getSize();
      await park({ type: "state", label: "trunc1GiB", size: r.sizeAfter1GiB });
      t = performance.now(); h.truncate(0); r.truncate0Ms = performance.now() - t;
      t = performance.now(); h.flush(); r.flush0Ms = performance.now() - t; r.sizeAfter0 = h.getSize();
      await park({ type: "state", label: "trunc0", size: r.sizeAfter0 });
      // Page reuse after truncate(0): write 256 MiB again
      const t2 = performance.now();
      for (let p = 0; p < 256; p++) { fill(words, p); h.write(buf, { at: p * PAGE }); }
      h.flush(); r.rewrite256MiBMs = performance.now() - t2;
      h.truncate(0); h.flush();
    } finally {
      try { h.close(); } catch (e) { r.closeError = String(e); }
      const tr = performance.now();
      try { await root.removeEntry("zone-big"); r.removeMs = performance.now() - tr; } catch (e) { r.removeError = String(e); }
    }
    await park({ type: "state", label: "removed" });
    self.postMessage({ type: "done", result: r });
  }
}

function smallWorker() {
  // write({at}) throughput by granularity; noise is pre-generated (16 MiB) so that fill time is excluded.
  self.onmessage = async (e) => {
    const { mib } = e.data;
    const noise = new Uint8Array(16 * 1048576), words = new Uint32Array(noise.buffer);
    let s = 0x9e3779b9;
    for (let i = 0; i < words.length; i++) { s ^= s << 13; s >>>= 0; s ^= s >>> 17; s ^= s << 5; s >>>= 0; words[i] = s; }
    const root = await navigator.storage.getDirectory();
    const h = await (await root.getFileHandle("zone-small", { create: true })).createSyncAccessHandle();
    const rows = [];
    try {
      for (const g of [32768, 65536, 262144, 1048576]) {
        h.truncate(0); h.flush();
        const total = mib * 1048576, n = total / g;
        const lat = new Float64Array(n);
        const t0 = performance.now();
        for (let i = 0; i < n; i++) {
          const a = performance.now();
          const w = h.write(noise.subarray(((i * g) % (16 * 1048576 - g + 1)) & ~7, (((i * g) % (16 * 1048576 - g + 1)) & ~7) + g), { at: i * g });
          lat[i] = performance.now() - a;
          if (w !== g) throw new Error("short write " + w);
        }
        const wMs = performance.now() - t0;
        const tf = performance.now(); h.flush(); const fMs = performance.now() - tf;
        const sorted = Array.from(lat).sort((x, y) => x - y);
        rows.push({ granularity: g, writes: n, writeMBps: total / 1e6 / (wMs / 1e3), flushMs: fMs,
          p50us: sorted[n >> 1] * 1000, p99us: sorted[Math.floor(n * 0.99)] * 1000, maxMs: sorted[n - 1] });
      }
      h.truncate(0); h.flush();
    } catch (err) {
      rows.push({ error: String(err), name: err && err.name });
    }
    h.close();
    await root.removeEntry("zone-small");
    self.postMessage({ type: "done", result: { mib, rows } });
  };
}

function churnWorker() {
  // One long synchronous loop, no awaits: allocate `totalGiB` of `seg`-byte segments keeping at most
  // `liveGiB` live (FIFO). pool=1 reuses a fixed set of segments instead of allocating.
  const MiB = 1048576, GiB = 1073741824;
  let go = null;
  const park = (msg) => new Promise((resolve) => { go = resolve; self.postMessage(msg); });
  self.onmessage = (e) => {
    if (e.data.type === "go" && go) { const g = go; go = null; g(); }
    else if (e.data.type === "start") run(e.data).catch((err) => self.postMessage({ type: "error", error: String(err) }));
  };
  async function run(m) {
    const noise = new Uint8Array(MiB), words = new Uint32Array(noise.buffer);
    let s = 0x9e3779b9;
    for (let i = 0; i < words.length; i++) { s ^= s << 13; s >>>= 0; s ^= s >>> 17; s ^= s << 5; s >>>= 0; words[i] = s; }
    const seg = m.seg, liveSegs = Math.round(m.liveGiB * GiB / seg), total = Math.round(m.totalGiB * GiB / seg);
    const fifo = new Array(liveSegs); let head = 0, count = 0;
    const free = [];
    if (m.pool) for (let i = 0; i < liveSegs; i++) free.push(new Uint8Array(seg));
    await park({ type: "state", label: "baseline" });
    let failure = null, allocated = 0, liveMax = 0;
    const t0 = performance.now();
    try {
      for (let i = 0; i < total; i++) {
        if (count === liveSegs) { const old = fifo[head]; fifo[head] = null; head = (head + 1) % liveSegs; count--; if (m.pool) free.push(old); }
        const seg8 = m.pool ? free.pop() : new Uint8Array(seg);
        seg8.set(noise.subarray(0, seg)); new DataView(seg8.buffer).setUint32(0, i >>> 0);
        fifo[(head + count) % liveSegs] = seg8; count++; allocated++;
        if (count > liveMax) liveMax = count;
      }
    } catch (err) { failure = { name: err && err.name, message: String(err), atSegment: allocated }; }
    const loopMs = performance.now() - t0;
    await park({ type: "state", label: "churn-done", live: count * seg, allocatedBytes: allocated * seg, loopMs, failure });
    for (let i = 0; i < liveSegs; i++) fifo[i] = null;
    free.length = 0;
    await park({ type: "state", label: "released" });
    self.postMessage({ type: "done", result: { seg, pool: m.pool, allocatedBytes: allocated * seg, liveMaxBytes: liveMax * seg, loopMs, failure } });
  }
}

function truncQWorker() {
  // Does estimate().usage drop after truncate(0)+flush while the handle is still open, after close(), after removeEntry()?
  let go = null;
  const park = (msg) => new Promise((resolve) => { go = resolve; self.postMessage(msg); });
  self.onmessage = (e) => {
    if (e.data.type === "go" && go) { const g = go; go = null; g(); }
    else if (e.data.type === "start") run(e.data).catch((err) => self.postMessage({ type: "error", error: String(err) }));
  };
  async function run(m) {
    const root = await navigator.storage.getDirectory();
    const h = await (await root.getFileHandle("zone-q", { create: true })).createSyncAccessHandle();
    const buf = new Uint8Array(1048576), words = new Uint32Array(buf.buffer);
    let s = 0x9e3779b9;
    for (let i = 0; i < words.length; i++) { s ^= s << 13; s >>>= 0; s ^= s >>> 17; s ^= s << 5; s >>>= 0; words[i] = s; }
    await park({ type: "state", label: "start" });
    for (let p = 0; p < m.gib * 1024; p++) { words[0] = p; h.write(buf, { at: p * 1048576 }); }
    h.flush();
    await park({ type: "state", label: "written" });
    h.truncate(0); h.flush();
    await park({ type: "state", label: "trunc0" });
    h.close();
    await park({ type: "state", label: "closed" });
    await root.removeEntry("zone-q");
    await park({ type: "state", label: "removed" });
    self.postMessage({ type: "done", result: { gib: m.gib } });
  }
}

function holderWorker() {
  // Holds `files` sync handles and writes round-robin until terminated.
  self.onmessage = async (e) => {
    const { dir, files, tag } = e.data;
    const root = await navigator.storage.getDirectory();
    const d = await root.getDirectoryHandle(dir, { create: true });
    const hs = [];
    for (let i = 0; i < files; i++) hs.push(await (await d.getFileHandle(`${tag}-${i}`, { create: true })).createSyncAccessHandle());
    const buf = new Uint8Array(1048576).fill(7);
    self.postMessage({ type: "open" });
    for (let i = 0; ; i++) {
      hs[i % files].write(buf, { at: (i % 64) * 1048576 });
      if (i === 40) self.postMessage({ type: "writing" });
    }
  };
}

function probeWorker() {
  // Retries createSyncAccessHandle on a file until it succeeds or `capMs` elapses (epoch clock).
  self.onmessage = async (e) => {
    const { dir, name, t0, capMs } = e.data;
    const root = await navigator.storage.getDirectory();
    const d = await root.getDirectoryHandle(dir);
    const fh = await d.getFileHandle(name);
    const errors = {};
    let attempts = 0;
    for (;;) {
      attempts++;
      try {
        const h = await fh.createSyncAccessHandle();
        const ms = Date.now() - t0;
        h.close();
        self.postMessage({ type: "probe", ok: true, ms, attempts, errors });
        return;
      } catch (err) {
        errors[err && err.name] = (errors[err && err.name] || 0) + 1;
        if (Date.now() - t0 > capMs) { self.postMessage({ type: "probe", ok: false, ms: Date.now() - t0, attempts, errors }); return; }
        await new Promise((r) => setTimeout(r, 5));
      }
    }
  };
}

function ramWorker() {
  const MiB = 1048576, GiB = 1073741824;
  let go = null;
  const park = (msg) => new Promise((resolve) => { go = resolve; self.postMessage(msg); });
  self.onmessage = (e) => {
    if (e.data.type === "go" && go) { const g = go; go = null; g(); }
    else if (e.data.type === "start") run(e.data).catch((err) => self.postMessage({ type: "error", error: String(err) }));
  };
  async function run(m) {
    const noise = new Uint8Array(MiB), words = new Uint32Array(noise.buffer);
    let s = 0x9e3779b9;
    for (let i = 0; i < words.length; i++) { s ^= s << 13; s >>>= 0; s ^= s >>> 17; s ^= s << 5; s >>>= 0; words[i] = s; }
    const seg = m.seg, segs = [];
    let retained = 0, failure = null;
    await park({ type: "state", label: "baseline", retained: 0 });
    for (let g = 1; g <= m.gib && !failure; g++) {
      const target = g * GiB, t0 = performance.now();
      try {
        while (retained < target) {
          const b = new ArrayBuffer(seg);
          new Uint8Array(b).set(noise.subarray(0, seg)); // touch every page with incompressible data
          new DataView(b).setUint32(0, segs.length >>> 0);
          segs.push(b); retained += seg;
        }
      } catch (err) {
        failure = { name: err && err.name, message: String(err), atRetained: retained, segments: segs.length };
      }
      await park({ type: "state", label: failure ? "failed" : "retained", retained, allocMs: performance.now() - t0, failure });
    }
    self.postMessage({ type: "done", result: { seg, gib: m.gib, retained, segments: segs.length, failure } });
  }
}

const q = new URLSearchParams(location.search);
const test = q.get("test");
const out = document.getElementById("out");
const mkWorker = (fn) => new Worker(URL.createObjectURL(new Blob([`(${fn.toString()})();`], { type: "text/javascript" })));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let stateSeq = 0;
window.zState = { seq: 0 };
const publish = async (s) => {
  let est = null;
  try { const e = await navigator.storage.estimate(); est = { usage: e.usage, quota: e.quota }; } catch (err) { est = { error: String(err) }; }
  window.zState = { ...s, estimate: est, at: Date.now(), seq: ++stateSeq };
  out.textContent = JSON.stringify(window.zState);
};
const finish = (r) => { window.zResult = r; out.textContent = JSON.stringify(r); };

async function runParked(fn, startMsg) {
  const w = mkWorker(fn);
  const states = [];
  const result = await new Promise((resolve) => {
    w.onerror = (e) => resolve({ workerError: String(e.message) });
    w.onmessage = async (e) => {
      const m = e.data;
      if (m.type === "state") { states.push({ label: m.label, estimate: null }); await publish(m); }
      else if (m.type === "done") resolve(m.result);
      else if (m.type === "error") resolve({ error: m.error, errorName: m.name });
    };
    window.zGo = () => w.postMessage({ type: "go" });
    w.postMessage({ type: "start", ...startMsg });
  });
  w.terminate();
  return result;
}

async function termTest(trials) {
  const root = await navigator.storage.getDirectory();
  const res = { rm1: [], open1: [], rm8: [] };
  const CAP = 10000;
  const startHolder = async (dir, files, tag) => {
    const w = mkWorker(holderWorker);
    await new Promise((resolve) => { w.onmessage = (e) => { if (e.data.type === "writing") resolve(); }; w.postMessage({ dir, files, tag }); });
    return w;
  };
  const tryRemove = async (d, name, t0) => {
    const errors = {}; let attempts = 0;
    for (;;) {
      attempts++;
      try { await d.removeEntry(name); return { ok: true, ms: performance.now() - t0, attempts, errors }; }
      catch (err) {
        errors[err && err.name] = (errors[err && err.name] || 0) + 1;
        if (performance.now() - t0 > CAP) return { ok: false, ms: performance.now() - t0, attempts, errors };
        await sleep(5);
      }
    }
  };
  for (const kind of ["rm1", "open1", "rm8"]) {
    for (let i = 0; i < trials; i++) {
      const dir = `term-${kind}-${i}`;
      const files = kind === "rm8" ? 8 : 1;
      const w = await startHolder(dir, files, "z");
      const d = await root.getDirectoryHandle(dir);
      let probe = null;
      if (kind === "open1") probe = mkWorker(probeWorker);
      const t0 = performance.now(), e0 = Date.now();
      w.terminate();
      let r;
      if (kind === "open1") {
        r = await new Promise((resolve) => { probe.onmessage = (e) => resolve(e.data); probe.postMessage({ dir, name: "z-0", t0: e0, capMs: CAP }); });
        probe.terminate();
        const rr = await tryRemove(d, "z-0", performance.now()); r.removeAfterMs = rr.ms; r.removeOk = rr.ok;
      } else {
        const names = Array.from({ length: files }, (_, k) => `z-${k}`);
        const per = [];
        for (const n of names) per.push(await tryRemove(d, n, t0));
        r = { ok: per.every((x) => x.ok), ms: Math.max(...per.map((x) => x.ms)), firstMs: per[0].ms, attempts: per.reduce((a, x) => a + x.attempts, 0), errors: per.reduce((a, x) => { for (const k in x.errors) a[k] = (a[k] || 0) + x.errors[k]; return a; }, {}) };
      }
      res[kind].push(r);
      try { await root.removeEntry(dir, { recursive: true }); } catch {}
    }
  }
  return res;
}

async function main() {
  const result = { test, params: Object.fromEntries(q) };
  try {
    result.persisted = await navigator.storage.persisted();
    result.estimate0 = await navigator.storage.estimate();
    if (test === "big") result.big = await runParked(bigWorker, { gib: Number(q.get("gib") ?? 6) });
    else if (test === "ram") result.ram = await runParked(ramWorker, { seg: Number(q.get("seg") ?? 32768), gib: Number(q.get("gib") ?? 4) });
    else if (test === "truncq") result.truncq = await runParked(truncQWorker, { gib: Number(q.get("gib") ?? 2) });
    else if (test === "small") result.small = await runParked(smallWorker, { mib: Number(q.get("mib") ?? 1024) });
    else if (test === "churn") result.churn = await runParked(churnWorker, { seg: Number(q.get("seg") ?? 262144), pool: Number(q.get("pool") ?? 0), liveGiB: 2, totalGiB: Number(q.get("gib") ?? 8) });
    else if (test === "term") result.term = await termTest(Number(q.get("trials") ?? 10));
    else throw new Error("unknown test");
  } catch (e) {
    result.pageError = String(e);
  }
  finish(result);
}
main();
