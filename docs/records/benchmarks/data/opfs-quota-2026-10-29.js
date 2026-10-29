// Task 3 (S8, 29-10-2026): throwaway measurement of the real OPFS quota. Reports persisted(),
// persist() and estimate(), then appends 1 MiB blocks to OPFS files (fileMiB each) until
// QuotaExceededError or the safety cap (capGiB). Always deletes what it wrote (finally).
// The writer runs in a Blob worker (sync access handle) so that it also works from file://.

function writerMain() {
  self.onmessage = async (event) => {
    const { capBytes, fileBytes, runId } = event.data;
    const block = 1024 * 1024;
    const out = { written: 0, files: 0, error: null, errorName: null };
    let handle = null;
    try {
      const root = await navigator.storage.getDirectory();
      const dir = await root.getDirectoryHandle("quota-" + runId, { create: true });
      const buf = new Uint8Array(block);
      const view = new DataView(buf.buffer);
      for (let o = 0; o + 4 <= block; o += 4096) view.setUint32(o, o, true);
      let fileIdx = 0;
      let inFile = fileBytes; // force opening the first file
      for (let i = 0; out.written < capBytes; i++) {
        if (inFile >= fileBytes) {
          if (handle) { handle.flush(); handle.close(); handle = null; }
          const f = await dir.getFileHandle("spool-" + fileIdx++, { create: true });
          handle = await f.createSyncAccessHandle();
          out.files = fileIdx;
          inFile = 0;
        }
        view.setUint32(0, i, true);
        const n = handle.write(buf, { at: inFile });
        if (n !== block) throw new Error("short write " + n);
        inFile += n;
        out.written += n;
        if ((i & 1023) === 1023) self.postMessage({ progress: out.written });
      }
      out.capped = true;
    } catch (e) {
      out.errorName = e && e.name;
      out.error = String(e);
    }
    // Cleanup always runs, also after a failure.
    try { if (handle) handle.close(); } catch (e) { out.closeError = String(e); }
    try {
      const t = performance.now();
      const root = await navigator.storage.getDirectory();
      await root.removeEntry("quota-" + runId, { recursive: true });
      out.removeMs = performance.now() - t;
      out.removed = true;
    } catch (e) {
      out.removed = false;
      out.removeError = String(e);
    }
    self.postMessage({ done: out });
  };
}

const GiB = 1024 ** 3;
const MiB = 1024 ** 2;

async function main() {
  const q = new URLSearchParams(location.search);
  const callPersist = q.get("persist") === "1";
  const capGiB = Number(q.get("capGiB") || 40);
  const fileMiB = Number(q.get("fileMiB") || 1024);
  const result = {
    ua: navigator.userAgent, protocol: location.protocol, isSecureContext: window.isSecureContext,
    callPersist, capGiB, fileMiB,
  };
  const est = async () => {
    const e = await navigator.storage.estimate();
    return { quota: e.quota, usage: e.usage };
  };
  try {
    result.persistedBefore = await navigator.storage.persisted();
    if (callPersist) result.persistResult = await navigator.storage.persist();
    result.persistedAfter = await navigator.storage.persisted();
    result.estimateBefore = await est();
  } catch (e) {
    result.apiError = String(e);
  }
  const runId = Date.now().toString(36);
  try {
    const src = "(" + writerMain.toString() + ")()";
    const worker = new Worker(URL.createObjectURL(new Blob([src], { type: "text/javascript" })));
    const t0 = performance.now();
    result.writer = await new Promise((resolve, reject) => {
      worker.onerror = (ev) => reject(new Error("worker error: " + ev.message));
      worker.onmessage = (ev) => {
        if (ev.data.progress) document.getElementById("out").textContent = "escrito " + (ev.data.progress / GiB).toFixed(1) + " GiB";
        if (ev.data.done) resolve(ev.data.done);
      };
      worker.postMessage({ capBytes: capGiB * GiB, fileBytes: fileMiB * MiB, runId });
    });
    result.wallMs = performance.now() - t0;
    worker.terminate();
  } catch (e) {
    result.workerError = String(e);
  }
  try { result.estimateAfterCleanup = await est(); } catch (e) { result.estimateAfterError = String(e); }
  window.quotaResult = result;
  document.getElementById("out").textContent = JSON.stringify(result, null, 1);
}

main().catch((e) => {
  window.quotaResult = { fatal: String(e) };
  document.getElementById("out").textContent = "fatal " + e;
});
