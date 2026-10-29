// Spike S9 (29-10-2026): throwaway OPFS "one file per child spool" measurement. No wasm.
// The host runs a FIFO ready-queue over a synthetic tree with N Blob workers. A job (worker) opens its
// input file and ALL its child files (+ one LOD file) with createSyncAccessHandle at the same time,
// reads the input, appends to the children in `block`-sized round-robin writes, flushes and closes
// every handle, then reports. The host deletes the job's input file as soon as the job ends; leaf
// and LOD files are retained until the end of the run. Data is xorshift noise (incompressible).
// The worker body is a function so it can start from a Blob URL (also works from file://).

function workerMain() {
  let dir = null;
  let wbuf = null;
  let rbuf = null;
  let state = 1;
  const xorshift = () => {
    state ^= state << 13; state >>>= 0;
    state ^= state >>> 17;
    state ^= state << 5; state >>>= 0;
    return state;
  };
  self.onmessage = async (event) => {
    const m = event.data;
    const out = { type: "done", jobId: m.jobId, createMs: [], getMs: [], flushMs: [], openMs: 0, closeMs: 0, writeMs: 0, readMs: 0, bytesWritten: 0, bytesRead: 0 };
    const handles = [];
    try {
      if (!wbuf) {
        state = ((m.workerId + 1) * 0x9e3779b9) >>> 0 || 1;
        wbuf = new Uint8Array(1048576);
        const v = new Uint32Array(wbuf.buffer);
        for (let i = 0; i < v.length; i++) v[i] = xorshift();
        rbuf = new Uint8Array(1048576);
      }
      const view = new DataView(wbuf.buffer);
      if (!dir) {
        const root = await navigator.storage.getDirectory();
        dir = await root.getDirectoryHandle("s9-" + m.runId, { create: true });
      }
      let input = null;
      const tOpen = performance.now();
      if (m.inputName) {
        let g = performance.now();
        const fh = await dir.getFileHandle(m.inputName);
        out.getMs.push(performance.now() - g);
        const t = performance.now();
        input = await fh.createSyncAccessHandle();
        out.createMs.push(performance.now() - t);
        handles.push(input);
      }
      const outs = [];
      for (const c of m.outputs) {
        const g = performance.now();
        const fh = await dir.getFileHandle(c.name, { create: true });
        out.getMs.push(performance.now() - g);
        const t = performance.now();
        const h = await fh.createSyncAccessHandle();
        out.createMs.push(performance.now() - t);
        handles.push(h);
        outs.push({ h, size: c.size, at: 0 });
      }
      out.openMs = performance.now() - tOpen;
      self.postMessage({ type: "opened", jobId: m.jobId, handles: handles.length, files: outs.length });

      if (input) {
        const size = input.getSize();
        if (size !== m.inputSize) throw new Error("input size " + size + " != " + m.inputSize);
        const t = performance.now();
        for (let at = 0; at < size; at += m.block) {
          const len = Math.min(m.block, size - at);
          const n = input.read(rbuf.subarray(0, len), { at });
          if (n !== len) throw new Error("short read " + n + "/" + len);
          out.bytesRead += n;
        }
        out.readMs = performance.now() - t;
      }
      let counter = 0;
      let pending = outs.length;
      const t = performance.now();
      while (pending > 0) {
        pending = 0;
        for (const o of outs) {
          const left = o.size - o.at;
          if (left <= 0) continue;
          const len = Math.min(m.block, left);
          view.setUint32(0, (++counter ^ m.jobId) >>> 0, true);
          const n = o.h.write(wbuf.subarray(0, len), { at: o.at });
          if (n !== len) throw new Error("short write " + n + "/" + len);
          o.at += len;
          out.bytesWritten += len;
          if (o.size - o.at > 0) pending++;
        }
      }
      out.writeMs = performance.now() - t;
      for (const o of outs) {
        const f = performance.now();
        o.h.flush();
        out.flushMs.push(performance.now() - f);
      }
    } catch (e) {
      out.error = String(e && e.name) + ": " + String(e && e.message);
    }
    out.closed = handles.length;
    const tc = performance.now();
    for (const h of handles) { try { h.close(); } catch (e) { out.closeError = String(e); } }
    out.closeMs = performance.now() - tc;
    self.postMessage(out);
  };
}

// ---- synthetic tree (deterministic) ----
function mulberry32(a) {
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// Child-spool sizes are heavy-tailed (lognormal, sigma 1.3): the Task 1 JSON has totals but no
// per-event child sizes, so the shape is a stated assumption. Sizes are relative until scaled so
// that everything written (root seed + every child + LOD) sums to totalBytes.
// d2: fan-out 8 everywhere. d8: root with 81 children; the 8 largest become "wide" jobs with 81
// leaf children each (8 x 81 simultaneous handles), the rest have fan-out 8.
function buildTree(scenario, totalBytes) {
  const rnd = mulberry32(scenario === "d8" ? 8 : 2);
  const gauss = () => Math.sqrt(-2 * Math.log(1 - rnd())) * Math.cos(2 * Math.PI * rnd());
  const weights = (k) => {
    const w = Array.from({ length: k }, () => Math.exp(1.3 * gauss()));
    const s = w.reduce((a, b) => a + b, 0);
    return w.map((x) => x / s).sort((a, b) => b - a);
  };
  const minFrac = scenario === "d8" ? 0.012 : 0.006;
  const maxDepth = 6;
  const jobs = [];
  const mk = (rel, depth, fan, leavesOnly) => {
    const job = { id: jobs.length + 1, inputRel: rel, outputs: [], lodRel: rel * 0.02 };
    jobs.push(job);
    weights(fan).forEach((x, i) => {
      const crel = rel * x;
      const o = { rel: crel, child: null };
      job.outputs.push(o);
      if (!leavesOnly && depth + 1 < maxDepth && crel > minFrac) {
        const wide = scenario === "d8" && depth === 0 && i < 8;
        o.child = mk(crel, depth + 1, wide ? 81 : 8, wide);
      }
    });
    return job;
  };
  const root = mk(1, 0, scenario === "d8" ? 81 : 8, false);
  let rel = 1;
  for (const j of jobs) rel += j.outputs.reduce((a, o) => a + o.rel, 0) + j.lodRel;
  const unit = totalBytes / rel;
  const size = (r) => Math.max(4096, Math.round((r * unit) / 4096) * 4096);
  let n = 0;
  const fname = () => "f" + ++n;
  const rootName = fname();
  const rootSize = size(1);
  const inputOf = new Map([[root, { name: rootName, size: rootSize }]]);
  const specs = [];
  for (const j of jobs) {
    const spec = { jobId: j.id, inputName: inputOf.get(j).name, inputSize: inputOf.get(j).size, outputs: [], children: [] };
    for (const o of j.outputs) {
      const f = { name: fname(), size: size(o.rel) };
      spec.outputs.push(f);
      if (o.child) { inputOf.set(o.child, f); spec.children.push(o.child.id); }
    }
    spec.outputs.push({ name: fname(), size: size(j.lodRel) }); // LOD: retained
    specs.push(spec);
  }
  return { rootName, rootSize, jobs: specs };
}

const pct = (arr, p) => {
  if (!arr.length) return null;
  const s = [...arr].sort((a, b) => a - b);
  return +s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))].toFixed(2);
};
const lat = (arr) => ({ n: arr.length, p50: pct(arr, 50), p95: pct(arr, 95), max: arr.length ? +Math.max(...arr).toFixed(2) : null });

async function runSpike(scenario, n, block, totalMiB) {
  const result = { scenario, n, block, totalMiB, ua: navigator.userAgent, protocol: location.protocol, isSecureContext: self.isSecureContext };
  if (!(navigator.storage && navigator.storage.getDirectory)) { result.error = "navigator.storage.getDirectory unavailable"; return result; }
  try {
    const e = await navigator.storage.estimate();
    result.quotaMiB = Math.round(e.quota / 1048576);
    result.usageMiBBefore = Math.round(e.usage / 1048576);
  } catch (e) { result.estimateError = String(e); }
  const runId = Date.now().toString(36);
  const tree = buildTree(scenario, totalMiB * 1048576);
  const specs = new Map(tree.jobs.map((j) => [j.jobId, j]));
  result.jobsInTree = tree.jobs.length;
  result.filesInTree = 1 + tree.jobs.reduce((a, j) => a + j.outputs.length, 0);
  result.treeBytesPlanned = tree.rootSize + tree.jobs.reduce((a, j) => a + j.outputs.reduce((b, o) => b + o.size, 0), 0);
  result.maxFanout = Math.max(...tree.jobs.map((j) => j.outputs.length - 1));

  const root = await navigator.storage.getDirectory();
  const dir = await root.getDirectoryHandle("s9-" + runId, { create: true });
  const url = URL.createObjectURL(new Blob(["(" + workerMain.toString() + ")()"], { type: "text/javascript" }));
  const idle = [];
  const workers = Array.from({ length: n }, (_, id) => { idle.push(id); return new Worker(url); });

  const st = { liveFiles: 0, peakLiveFiles: 0, openHandles: 0, peakOpenHandles: 0, liveBytes: 0, peakLiveBytes: 0, createdFiles: 0, removed: 0,
    bytesWritten: 0, bytesRead: 0, writeMs: 0, readMs: 0, peakJobs: 0, running: 0 };
  const createMs = [], getMs = [], flushMs = [], removeMs = [];
  let openMs = 0, closeMs = 0;
  const errors = {};
  const samples = [];
  const note = (k) => { errors[k] = (errors[k] || 0) + 1; };
  const touch = () => {
    st.peakLiveFiles = Math.max(st.peakLiveFiles, st.liveFiles);
    st.peakOpenHandles = Math.max(st.peakOpenHandles, st.openHandles);
    st.peakLiveBytes = Math.max(st.peakLiveBytes, st.liveBytes);
    st.peakJobs = Math.max(st.peakJobs, st.running);
  };

  const ready = [];
  let completed = 0;
  let aborted = false;
  let finish;
  const finished = new Promise((r) => { finish = r; });

  const runJob = (workerId, spec, seed) => new Promise((resolve) => {
    const w = workers[workerId];
    w.onmessage = async (ev) => {
      const m = ev.data;
      if (m.type === "opened") {
        st.liveFiles += m.files; st.createdFiles += m.files; st.openHandles += m.handles;
        touch();
        return;
      }
      st.openHandles -= m.closed;
      if (!seed) {
        st.writeMs += m.writeMs; st.readMs += m.readMs; st.bytesWritten += m.bytesWritten; st.bytesRead += m.bytesRead;
      }
      st.liveBytes += m.bytesWritten;
      createMs.push(...m.createMs); getMs.push(...m.getMs); flushMs.push(...m.flushMs);
      if (!seed) { openMs += m.openMs; closeMs += m.closeMs; }
      if (m.error) { note("job " + m.error.split(":")[0]); samples.push(m.error.slice(0, 200)); }
      touch();
      if (!seed && !m.error) {
        const t = performance.now();
        try {
          await dir.removeEntry(spec.inputName);
          removeMs.push(performance.now() - t);
          st.removed++; st.liveFiles--; st.liveBytes -= spec.inputSize;
        } catch (e) { note("remove " + (e && e.name)); samples.push("remove: " + e); }
      }
      resolve(m);
    };
    w.onerror = (ev) => { note("worker error"); samples.push("worker error: " + ev.message); resolve({ error: "worker error", closed: 0, createMs: [], getMs: [], flushMs: [], openMs: 0, closeMs: 0, writeMs: 0, readMs: 0, bytesWritten: 0, bytesRead: 0 }); };
    w.postMessage({ runId, workerId, jobId: spec.jobId, block, inputName: seed ? null : spec.inputName, inputSize: spec.inputSize, outputs: spec.outputs });
  });

  const pump = () => {
    while (idle.length && ready.length && !aborted) {
      const workerId = idle.shift();
      const spec = ready.shift();
      st.running++;
      runJob(workerId, spec, false).then((m) => {
        st.running--;
        completed++;
        idle.push(workerId);
        if (m.error) aborted = true;
        else for (const c of spec.children) ready.push(specs.get(c));
        if (completed === tree.jobs.length || (aborted && st.running === 0)) finish();
        else pump();
      });
    }
  };

  let phaseStart = 0;
  try {
    // Seed: a worker writes the root input file (excluded from the throughput figures).
    const seedWorker = idle.shift();
    const sm = await runJob(seedWorker, { jobId: 0, inputSize: 0, outputs: [{ name: tree.rootName, size: tree.rootSize }] }, true);
    idle.push(seedWorker);
    if (sm.error) throw new Error("seed: " + sm.error);
    ready.push(specs.get(1));
    phaseStart = performance.now();
    pump();
    await finished;
    result.wallMs = +(performance.now() - phaseStart).toFixed(1);
  } catch (e) { result.runError = String(e); }
  result.completedJobs = completed;
  result.finalLiveFiles = st.liveFiles;
  result.finalLiveBytes = st.liveBytes;
  Object.assign(result, {
    peakLiveFiles: st.peakLiveFiles, peakOpenHandles: st.peakOpenHandles, peakConcurrentJobs: st.peakJobs,
    peakLiveBytes: st.peakLiveBytes, createdFiles: st.createdFiles, removedFiles: st.removed,
    bytesWritten: st.bytesWritten, bytesRead: st.bytesRead,
  });
  const MB = 1.048576 / 1048576;
  // Pure write rate = bytes / mean per-worker time inside write(); S1's aggWriteMBps divides by the
  // slowest worker's write time, so this one is the more generous of the two.
  result.aggWriteMBps = st.writeMs ? +((st.bytesWritten * MB) / (st.writeMs / n / 1000)).toFixed(1) : null;
  result.e2eMBps = result.wallMs ? +((st.bytesWritten * MB) / (result.wallMs / 1000)).toFixed(1) : null;
  result.sumWriteMs = +st.writeMs.toFixed(0);
  result.sumReadMs = +st.readMs.toFixed(0);
  result.sumOpenMs = +openMs.toFixed(0); // per-job time to open every handle, summed over jobs
  result.sumCloseMs = +closeMs.toFixed(0);
  result.sumRemoveMs = +removeMs.reduce((a, b) => a + b, 0).toFixed(0);
  result.latencyMs = { getFileHandle: lat(getMs), createSyncAccessHandle: lat(createMs), flush: lat(flushMs), removeEntry: lat(removeMs) };
  result.errors = errors;
  result.errorSamples = samples.slice(0, 5);
  for (const w of workers) w.terminate();
  try {
    const t = performance.now();
    await root.removeEntry("s9-" + runId, { recursive: true });
    result.cleanupMs = +(performance.now() - t).toFixed(1);
  } catch (e) { result.cleanupError = String(e); }
  try { result.usageMiBAfter = Math.round((await navigator.storage.estimate()).usage / 1048576); } catch (e) {}
  return result;
}

if (typeof window !== "undefined") {
  const q = new URLSearchParams(location.search);
  window.runSpike = runSpike;
  window.buildTree = buildTree;
  if (q.has("n")) {
    runSpike(q.get("scenario") || "d2", +q.get("n"), +(q.get("block") || 32768), +(q.get("totalMiB") || 7500)).then((r) => {
      window.s9Result = r;
      document.getElementById("out").textContent = JSON.stringify(r, null, 1);
    }, (e) => { window.s9Result = { fatal: String(e) }; });
  }
}
