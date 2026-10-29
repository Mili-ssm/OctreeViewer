// Spike S10 (29-10-2026): throwaway measurement of the memory cost of retaining ArrayBuffer
// segments in a dedicated worker, and of transferring them worker -> main. No wasm.
// The worker retains `seg`-byte segments up to each step (MiB list) and stops; the Node driver
// samples the browser process tree (PSS) while the page is parked, then calls window.s10Go().
// After the last step the worker drops everything ("released", a PSS sample), then transfers
// `xferMiB` of fresh segments to the main thread as (a) one message with all of them in the
// transfer list and (b) one message per segment. Main keeps the received buffers ("held", a PSS
// sample) until the driver releases it. Timestamps are epoch ms (timeOrigin + now) on both sides.
// The worker body is a function so it can start from a Blob URL.

function workerMain() {
  const MiB = 1048576;
  const epoch = () => performance.timeOrigin + performance.now();
  let noise = null;
  const makeSegment = (size, serial) => {
    const buffer = new ArrayBuffer(size);
    const bytes = new Uint8Array(buffer);
    bytes.set(noise.subarray(0, size)); // touch every page with incompressible data
    new DataView(buffer).setUint32(0, serial >>> 0); // keep segments distinct
    return buffer;
  };
  let go = null;
  const waitGo = () => new Promise((resolve) => { go = resolve; });
  self.onmessage = (event) => {
    if (event.data.type === "go" && go) { const g = go; go = null; g(); }
    else if (event.data.type === "start") run(event.data).catch((e) => self.postMessage({ type: "error", error: String(e) }));
  };
  async function run(m) {
    let state = 0x9e3779b9;
    noise = new Uint8Array(MiB);
    const words = new Uint32Array(noise.buffer);
    for (let i = 0; i < words.length; i++) {
      state ^= state << 13; state >>>= 0; state ^= state >>> 17; state ^= state << 5; state >>>= 0;
      words[i] = state;
    }
    const seg = m.seg;
    let segments = [];
    let retained = 0;
    self.postMessage({ type: "step", label: "baseline", retained: 0, at: Date.now() });
    await waitGo();
    for (const stepMiB of m.steps) {
      const target = stepMiB * MiB;
      const t0 = performance.now();
      const before = retained;
      while (retained < target) { segments.push(makeSegment(seg, segments.length)); retained += seg; }
      self.postMessage({ type: "step", label: "retained", retained, allocMs: performance.now() - t0, allocatedBytes: retained - before, at: Date.now() });
      await waitGo();
    }
    segments = null;
    retained = 0;
    self.postMessage({ type: "step", label: "released", retained: 0, at: Date.now() });
    await waitGo();
    const count = Math.round((m.xferMiB * MiB) / seg);
    for (const mode of ["single", "per-segment"]) {
      const batch = [];
      for (let i = 0; i < count; i++) batch.push(makeSegment(seg, i));
      const t0 = epoch();
      if (mode === "single") {
        self.postMessage({ type: "batch", mode, count, bytes: count * seg, t0, buffers: batch }, batch);
        const sendMs = epoch() - t0;
        self.postMessage({ type: "sent", mode, sendMs });
      } else {
        for (let i = 0; i < count; i++) self.postMessage({ type: "segment", mode, buffer: batch[i] }, [batch[i]]);
        const sendMs = epoch() - t0;
        self.postMessage({ type: "batch", mode, count, bytes: count * seg, t0, sendMs, expectSegments: true });
      }
      await waitGo(); // main acknowledges after receiving; the driver samples "held" in between
    }
    self.postMessage({ type: "finished" });
  }
}

const params = new URLSearchParams(location.search);
const seg = Number(params.get("seg") ?? 32768);
const steps = (params.get("steps") ?? "256,512,1024,2048").split(",").map(Number);
const xferMiB = Number(params.get("xferMiB") ?? 256);
const out = document.getElementById("out");
const result = { seg, steps, xferMiB, retainedSteps: [], transfers: [], errors: [] };
window.s10State = { seq: 0 };
let held = [];
let pending = null;
const epoch = () => performance.timeOrigin + performance.now();
const publish = (state) => {
  window.s10State = { ...state, seq: window.s10State.seq + 1 };
  out.textContent = JSON.stringify(window.s10State);
};
const source = `(${workerMain.toString()})();`;
const worker = new Worker(URL.createObjectURL(new Blob([source], { type: "text/javascript" })));
window.s10Go = () => {
  held = []; // drop the buffers received in the previous transfer
  worker.postMessage({ type: "go" });
};
const finishTransfer = () => {
  result.transfers.push({ mode: pending.mode, count: pending.count, bytes: pending.bytes, sendMs: pending.sendMs,
    totalMs: pending.receivedAt - pending.t0, receivedSegments: pending.received });
  publish({ label: "held", retained: pending.bytes, mode: pending.mode, at: Date.now() });
};
worker.onerror = (e) => { result.errors.push(String(e.message)); window.s10Result = result; };
worker.onmessage = (event) => {
  const m = event.data;
  if (m.type === "step") {
    if (m.label === "retained") result.retainedSteps.push({ retained: m.retained, allocMs: m.allocMs, allocatedBytes: m.allocatedBytes });
    publish(m);
  } else if (m.type === "error") {
    result.errors.push(m.error);
    window.s10Result = result;
  } else if (m.type === "segment") { // mode (b): one message per segment, FIFO on the port
    const t = epoch();
    if (!pending || pending.mode !== "per-segment") pending = { mode: "per-segment", received: 0 };
    held.push(m.buffer);
    pending.received++;
    pending.receivedAt = t;
  } else if (m.type === "batch" && m.mode === "single") { // mode (a)
    held = m.buffers;
    pending = { mode: "single", count: m.count, bytes: m.bytes, t0: m.t0, receivedAt: epoch(), received: m.buffers.length };
  } else if (m.type === "sent") {
    pending.sendMs = m.sendMs;
    finishTransfer();
  } else if (m.type === "batch") { // end marker of mode (b), after all its segments
    Object.assign(pending, { count: m.count, bytes: m.bytes, t0: m.t0, sendMs: m.sendMs });
    finishTransfer();
  } else if (m.type === "finished") {
    window.s10Result = result;
  }
};
worker.postMessage({ type: "start", seg, steps, xferMiB });
