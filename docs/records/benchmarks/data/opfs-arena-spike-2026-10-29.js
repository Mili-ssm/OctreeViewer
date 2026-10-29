// Spike S1 (29-10-2026): throwaway OPFS "job arena" measurement. No wasm.
// The page creates N workers; each one creates an append-only OPFS file, appends
// `mib` MiB in blocks of `block` bytes through one FileSystemSyncAccessHandle, reads
// it back and removes it. The worker body is a function so that it can be started from
// a Blob URL, which also works from file:// (where worker script files may be refused).

function workerMain() {
  self.onmessage = async (event) => {
    const { id, block, mib, runId } = event.data;
    const out = { id, block, mib };
    try {
      const total = mib * 1024 * 1024;
      const root = await navigator.storage.getDirectory();
      const dir = await root.getDirectoryHandle("arena-spike-" + runId, { create: true });
      const name = "arena-" + id;
      const file = await dir.getFileHandle(name, { create: true });
      let t = performance.now();
      const handle = await file.createSyncAccessHandle();
      out.createHandleMs = performance.now() - t;

      const buf = new Uint8Array(block);
      const view = new DataView(buf.buffer);
      const blocks = Math.ceil(total / block);
      const stamp = (i) => ((i * 2654435761) ^ (id + 1)) >>> 0;
      for (let o = 0; o + 4 <= block; o += 4096) view.setUint32(o, o, true);

      t = performance.now();
      let at = 0;
      for (let i = 0; i < blocks; i++) {
        view.setUint32(0, stamp(i), true);
        const len = Math.min(block, total - at);
        const n = handle.write(len === block ? buf : buf.subarray(0, len), { at });
        if (n !== len) throw new Error("short write " + n + "/" + len);
        at += len;
      }
      out.writeMs = performance.now() - t;
      t = performance.now();
      handle.flush();
      out.flushMs = performance.now() - t;
      out.size = handle.getSize();

      const rbuf = new Uint8Array(block);
      const rview = new DataView(rbuf.buffer);
      t = performance.now();
      at = 0;
      let bad = 0;
      for (let i = 0; i < blocks; i++) {
        const len = Math.min(block, total - at);
        const n = handle.read(len === block ? rbuf : rbuf.subarray(0, len), { at });
        if (n !== len) throw new Error("short read " + n + "/" + len);
        if (rview.getUint32(0, true) !== stamp(i)) bad++;
        at += len;
      }
      out.readMs = performance.now() - t;
      out.badBlocks = bad;

      handle.close();
      t = performance.now();
      await dir.removeEntry(name);
      out.removeMs = performance.now() - t;
    } catch (e) {
      out.error = String(e && e.name) + ": " + String(e && e.message);
    }
    self.postMessage(out);
  };
}

async function runSpike(n, block, mib) {
  const result = { n, block, mib, ua: navigator.userAgent, protocol: location.protocol };
  result.isSecureContext = self.isSecureContext;
  result.mainThreadOpfs = !!(navigator.storage && navigator.storage.getDirectory);
  try {
    if (navigator.storage && navigator.storage.estimate) {
      const e = await navigator.storage.estimate();
      result.quotaMiB = Math.round(e.quota / 1048576);
      result.usageMiB = Math.round(e.usage / 1048576);
    }
  } catch (e) {
    result.estimateError = String(e);
  }
  if (!result.mainThreadOpfs) {
    result.error = "navigator.storage.getDirectory unavailable";
    return result;
  }
  const runId = Date.now().toString(36);
  const url = URL.createObjectURL(new Blob(["(" + workerMain.toString() + ")()"], { type: "text/javascript" }));
  const t0 = performance.now();
  const results = await Promise.all(
    Array.from({ length: n }, (_, id) => new Promise((resolve) => {
      let w;
      try {
        w = new Worker(url);
      } catch (e) {
        resolve({ id, error: "Worker: " + e });
        return;
      }
      w.onmessage = (ev) => { w.terminate(); resolve(ev.data); };
      w.onerror = (ev) => { w.terminate(); resolve({ id, error: "worker error: " + ev.message }); };
      w.postMessage({ id, block, mib, runId });
    })),
  );
  result.wallMs = performance.now() - t0;
  result.workers = results;
  const ok = results.filter((r) => !r.error);
  const mbps = (ms) => (ms ? (mib * 1.048576) / (ms / 1000) : 0);
  if (ok.length) {
    result.perWorkerWriteMBps = ok.map((r) => +mbps(r.writeMs).toFixed(1));
    result.perWorkerReadMBps = ok.map((r) => +mbps(r.readMs).toFixed(1));
    // Aggregate = total bytes of all workers over the slowest worker's phase time.
    result.aggWriteMBps = +((ok.length * mib * 1.048576) / (Math.max(...ok.map((r) => r.writeMs)) / 1000)).toFixed(1);
    // Same, counting handle.flush() as part of the write (what a durable append costs).
    result.aggWriteFlushMBps = +((ok.length * mib * 1.048576) / (Math.max(...ok.map((r) => r.writeMs + r.flushMs)) / 1000)).toFixed(1);
    result.aggReadMBps = +((ok.length * mib * 1.048576) / (Math.max(...ok.map((r) => r.readMs)) / 1000)).toFixed(1);
    result.maxCreateHandleMs = +Math.max(...ok.map((r) => r.createHandleMs)).toFixed(1);
    result.maxFlushMs = +Math.max(...ok.map((r) => r.flushMs)).toFixed(1);
    result.maxRemoveMs = +Math.max(...ok.map((r) => r.removeMs)).toFixed(1);
    result.badBlocks = ok.reduce((a, r) => a + r.badBlocks, 0);
  }
  result.errors = results.filter((r) => r.error).map((r) => r.error);
  try {
    const root = await navigator.storage.getDirectory();
    await root.removeEntry("arena-spike-" + runId, { recursive: true });
  } catch (e) {
    result.cleanupError = String(e);
  }
  return result;
}

if (typeof window !== "undefined") {
  const q = new URLSearchParams(location.search);
  window.runSpike = runSpike;
  if (q.has("n")) {
    runSpike(+q.get("n"), +(q.get("block") || 1048576), +(q.get("mib") || 1024)).then((r) => {
      window.spikeResult = r;
      document.getElementById("out").textContent = JSON.stringify(r, null, 1);
    });
  }
}
