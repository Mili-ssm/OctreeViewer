// Conversion timing probe: one `WasmCoreConverter` conversion under Node (the same serial core and
// glue as the web worker, fs-backed zones) with wall time per progress stage, wasm memory sampled at
// progress callbacks (not a peak) and peak RSS.
// Build the bindings first (see run-bindings.mjs), then:
//   node tools/convert_parity/time-bindings.mjs --input FILE [--spool-budget-bytes B] [--work-dir DIR]
// Prints one JSON line. Defaults: B = 2 GiB (DEFAULT_SPOOL_BUDGET_BYTES, what the web app uses), DIR = target/convert_parity/timing;
// DIR must not exist or be empty, and is removed when the run ends (also on failure).
import { createHash } from "node:crypto";
import { closeSync, createReadStream, existsSync, fstatSync, mkdirSync, openSync, readFileSync, readSync, readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { createSpoolHost } from "../../examples/web-host/spool-tiers.mjs";
import { openFsSyncHandle } from "../../examples/web-host/tests/support/fs-sync-handle.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const pkg = join(root, "target/convert_parity/pkg");
const cli = {};
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i += 2) {
  if (!argv[i].startsWith("--") || argv[i + 1] === undefined) throw new Error(`bad argument ${argv[i]}`);
  cli[argv[i].slice(2)] = argv[i + 1];
}
if (cli.input === undefined) throw new Error("--input FILE is required");
const workDir = cli["work-dir"] ?? join(root, "target/convert_parity/timing");
if (existsSync(workDir) && readdirSync(workDir).length > 0) throw new Error(`work dir ${workDir} is not empty`);
mkdirSync(workDir, { recursive: true });

function fdBlob(fd, start = 0, end = fstatSync(fd).size) {
  return { size: end - start, fd, start, slice: (a, b) => fdBlob(fd, start + a, Math.min(start + b, end)) };
}
globalThis.FileReaderSync = class {
  readAsArrayBuffer(blob) {
    const buffer = new ArrayBuffer(blob.size);
    const view = new Uint8Array(buffer);
    let done = 0;
    while (done < view.length) {
      const n = readSync(blob.fd, view, done, view.length - done, blob.start + done);
      if (n === 0) throw new Error("unexpected end of input");
      done += n;
    }
    return buffer;
  }
};

const { initSync, WasmCoreConverter } = await import(pathToFileURL(join(pkg, "viewer_wasm.js")).href);
if (readdirSync(pkg).includes("snippets")) throw new Error("wasm-bindgen generated snippets");
const wasmExports = initSync({ module: readFileSync(join(pkg, "viewer_wasm_bg.wasm")) });

try {
  const options = { maxError: 0.01, spoolBudgetBytes: Number(cli["spool-budget-bytes"] ?? 2 * 1024 ** 3) };
  const fd = openSync(cli.input, "r");
  const paths = ["t.oct", "t.octattrs"].map((file) => join(workDir, file));
  const estimate = WasmCoreConverter.estimate(fdBlob(fd), options);
  const handles = paths.map((path) => openFsSyncHandle(path));
  const zones = Array.from({ length: estimate.zones }, (_, i) => openFsSyncHandle(join(workDir, `zone${i}`)));
  const kb = (key) => Number(readFileSync("/proc/self/status", "utf8").match(new RegExp(`${key}:\\s+(\\d+) kB`))[1]) * 1024;

  const stages = [];
  let sampledWasmBytes = 0;
  const started = performance.now();
  const onProgress = ({ stage }) => {
    const now = performance.now();
    if (stages.at(-1)?.stage !== stage) stages.push({ stage, startMs: now - started });
    sampledWasmBytes = Math.max(sampledWasmBytes, wasmExports.memory.buffer.byteLength);
    return true;
  };
  const converter = new WasmCoreConverter(fdBlob(fd), handles[0], handles[1], options, onProgress, createSpoolHost({ zones }));
  const summary = converter.convert();
  const elapsedMs = performance.now() - started;
  stages.forEach((s, i) => (s.ms = Math.round(((stages[i + 1]?.startMs ?? elapsedMs) - s.startMs) * 10) / 10));
  sampledWasmBytes = Math.max(sampledWasmBytes, wasmExports.memory.buffer.byteLength);
  converter.free();
  closeSync(fd);
  for (const handle of [...handles, ...zones]) handle.close();
  const sha256 = async (path) => {
    const hash = createHash("sha256");
    for await (const chunk of createReadStream(path)) hash.update(chunk);
    return hash.digest("hex");
  };
  console.log(
    JSON.stringify({
      elapsedMs: Math.round(elapsedMs * 10) / 10,
      stages: stages.map(({ stage, ms }) => ({ stage, ms })),
      blocks: summary.blockCount,
      pointCount: summary.pointCount,
      wasmMemorySampledBytes: sampledWasmBytes,
      peakRssBytes: kb("VmHWM"),
      zones: estimate.zones,
      spool: summary.spool,
      octSha256: await sha256(paths[0]),
      octattrsSha256: await sha256(paths[1]),
    }),
  );
} finally {
  rmSync(workDir, { recursive: true, force: true });
}
process.exit(0);
