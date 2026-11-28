// Runs the REAL `viewer_wasm` glue (`WasmCoreConverter`) under Node and compares its output with
// the native CLI path (SHA-256 of the `.oct` and `.octattrs`), converting with a tiny spool budget
// so the spill zones are exercised. The browser pieces are shimmed: `FileReaderSync` and the input
// Blob read an fd, outputs and zones are `fs`-backed sync-access-handle fakes. Build first:
//   cargo build --release -p viewer_wasm --target wasm32-unknown-unknown
//   wasm-bindgen --target web --out-dir target/convert_parity/pkg \
//     target/wasm32-unknown-unknown/release/viewer_wasm.wasm
//   cargo build --release -p convert_parity --bin native_convert
// Exits non-zero on any mismatch, unexpected failure, run that never spilled or a quota error
// that is not named `QuotaExceededError`.
//
// Single-input mode (the Node gate of the web converter, no native comparison, one JSON line):
//   node run-bindings.mjs --input FILE [--spool-budget-bytes B] [--work-dir DIR]
// V8 options only (V8 fixes the format, points per block and divisions). Defaults: B = 2 GiB, DIR = target/convert_parity/single. The zone count comes from
// `WasmCoreConverter.estimate`, like the web host. Prints the elapsed `convert()` time, the SHA-256
// of both outputs, `exports.memory.buffer.byteLength`, the spool counters and the process RSS.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { closeSync, createReadStream, fstatSync, mkdirSync, openSync, readFileSync, readSync, readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { createSpoolHost } from "../../examples/web-host/spool-tiers.mjs";
import { openFsSyncHandle } from "../../examples/web-host/tests/support/fs-sync-handle.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const pkg = join(root, "target/convert_parity/pkg");
const nativeBin = join(root, "target/release/native_convert");
const cli = (() => {
  const values = {};
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i].startsWith("--") || argv[i + 1] === undefined) throw new Error(`bad argument ${argv[i]}`);
    values[argv[i].slice(2)] = argv[i + 1];
  }
  return values;
})();
const singleInput = cli.input !== undefined;
const workDir = singleInput
  ? join(root, cli["work-dir"] ?? "target/convert_parity/single")
  : join(root, "target/convert_parity/bindings");
rmSync(workDir, { recursive: true, force: true });
mkdirSync(workDir, { recursive: true });

const SPOOL_BUDGET_BYTES = 64 * 1024;
const ZONES = 2;
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

// The glue only calls `size` and `slice` on the input and `readAsArrayBuffer` on the reader.
function fdBlob(fd, start = 0, end = fstatSync(fd).size) {
  return {
    size: end - start,
    fd,
    start,
    slice: (a, b) => fdBlob(fd, start + a, Math.min(start + b, end)),
  };
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

const { initSync, WasmCoreConverter, WasmAsyncFormatReader } = await import(pathToFileURL(join(pkg, "viewer_wasm.js")).href);
if (readdirSync(pkg).includes("snippets")) throw new Error("wasm-bindgen generated snippets");
const wasmExports = initSync({ module: readFileSync(join(pkg, "viewer_wasm_bg.wasm")) });

async function sha256Stream(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

if (singleInput) {
  const budget = Number(cli["spool-budget-bytes"] ?? 2 * 1024 ** 3);
  const options = { maxError: 0.01, spoolBudgetBytes: budget };
  const fd = openSync(cli.input, "r");
  const paths = ["single.oct", "single.octattrs"].map((file) => join(workDir, file));
  const estimate = WasmCoreConverter.estimate(fdBlob(fd), options);
  const handles = paths.map((path) => openFsSyncHandle(path));
  const zones = Array.from({ length: estimate.zones }, (_, i) => openFsSyncHandle(join(workDir, `zone${i}`)));
  const converter = new WasmCoreConverter(fdBlob(fd), handles[0], handles[1], options, undefined, createSpoolHost({ zones }));
  const started = performance.now();
  const summary = converter.convert();
  const elapsedMs = performance.now() - started;
  const wasmMemoryBytes = wasmExports.memory.buffer.byteLength;
  converter.free();
  closeSync(fd);
  for (const handle of [...handles, ...zones]) handle.close();
  const status = readFileSync("/proc/self/status", "utf8");
  const kb = (key) => Number(status.match(new RegExp(`${key}:\\s+(\\d+) kB`))[1]) * 1024;
  console.log(
    JSON.stringify({
      side: "wasm",
      elapsedMs: Math.round(elapsedMs * 10) / 10,
      blocks: summary.blockCount,
      pointCount: summary.pointCount,
      wasmMemoryBytes,
      rssBytes: kb("VmRSS"),
      peakRssBytes: kb("VmHWM"),
      zones: estimate.zones,
      estimate,
      spool: summary.spool,
      octSha256: await sha256Stream(paths[0]),
      octattrsSha256: await sha256Stream(paths[1]),
    }),
  );
  process.exit(0);
}

const small = join(root, "data/samples/fs_e430_n4008_18.las");
const synthetic = join(root, "tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz");

function nativeConvert(name, input) {
  const output = join(workDir, `${name}.native.oct`);
  const run = spawnSync(nativeBin, [input, output], { encoding: "utf8" });
  if (run.status !== 0) throw new Error(`native conversion failed: ${run.stderr}`);
  return { oct: sha256(readFileSync(output)), attrs: sha256(readFileSync(`${output}attrs`)) };
}

// Runs one binding conversion; `outputQuota` and `zoneQuota` (bytes) make the fakes write short.
function bindingConvert(name, input, { outputQuota = Infinity, zoneQuota = Infinity, removed = {} } = {}) {
  const paths = [`${name}.oct`, `${name}.octattrs`].map((file) => join(workDir, file));
  const handles = [
    openFsSyncHandle(paths[0], { quotaBytes: outputQuota }),
    openFsSyncHandle(paths[1], { quotaBytes: outputQuota }),
  ];
  const zones = Array.from({ length: ZONES }, (_, i) =>
    openFsSyncHandle(join(workDir, `${name}.zone${i}`), { quotaBytes: zoneQuota }),
  );
  const fd = openSync(input, "r");
  let converter;
  try {
    converter = new WasmCoreConverter(
      fdBlob(fd),
      handles[0],
      handles[1],
      {
        maxError: 0.01,
        spoolBudgetBytes: SPOOL_BUDGET_BYTES,
        ...removed,
      },
      undefined,
      createSpoolHost({ zones }),
    );
    const summary = converter.convert();
    return { summary, oct: sha256(readFileSync(paths[0])), attrs: sha256(readFileSync(paths[1])) };
  } finally {
    converter?.free();
    closeSync(fd);
    for (const handle of [...handles, ...zones]) handle.close();
  }
}

let failed = false;
const report = (ok, line) => {
  failed ||= !ok;
  console.log(`${ok ? "ok  " : "FAIL"} ${line}`);
};

const cases = [
  { name: "small-v8", input: small },
  { name: "synthetic-v8", input: synthetic },
];
for (const { name, input } of cases) {
  const native = nativeConvert(name, input);
  const { summary, oct, attrs } = bindingConvert(name, input);
  const problems = [];
  if (oct !== native.oct) problems.push("oct differs");
  if (attrs !== native.attrs) problems.push("octattrs differs");
  if (!(summary.spool.spilledBytes > 0)) problems.push("never spilled");
  if (summary.spool.livePayloadBytes !== 0) problems.push("spools left alive");
  report(
    problems.length === 0,
    `${name} blocks=${summary.blockCount} bindings=${oct.slice(0, 12)} native=${native.oct.slice(0, 12)} ` +
      `attrs=${attrs.slice(0, 12)}/${native.attrs.slice(0, 12)} spilled=${summary.spool.spilledBytes}B` +
      (problems.length ? ` (${problems.join(", ")})` : ""),
  );
}

// V8 fixes the format, row order, points per block and divisions, and `attrCodec` was removed
// (ADR 0060): the bindings refuse each of them with a message that names it, before any work.
for (const [name, value] of [["formatVersion", "v8"], ["formatVersion", "v5"], ["rowOrder", "hybrid"], ["pointsPerBlock", 64000], ["spatialDivisions", 2], ["attrCodec", "lz4"], ["rowOrder", null]]) {
  let message = "";
  try {
    bindingConvert(`refused-${name}`, small, { removed: { [name]: value } });
  } catch (error) {
    message = String(error?.message ?? error);
  }
  const expected = name === "attrCodec" ? "attrCodec was removed (ADR 0060)" : `option ${name} was removed: V8 is the only format and fixes it`;
  report(message.includes(expected), `${name}=${value} is refused by the bindings (${message.slice(0, 70)})`);
}

// The browser keeps its cap of 8 `spatialWorkers` while the native CLI goes up to the cores: 4 is
// accepted and gives the native bytes, 9 is refused with the cap in the message.
{
  const native = nativeConvert("workers-native", small);
  const { oct, attrs } = bindingConvert("workers-4", small, { removed: { spatialWorkers: 4 } });
  report(oct === native.oct && attrs === native.attrs, `spatialWorkers=4 is accepted and matches native (${oct.slice(0, 12)})`);
  let message = "";
  try {
    bindingConvert("workers-9", small, { removed: { spatialWorkers: 9 } });
  } catch (error) {
    message = String(error?.message ?? error);
  }
  report(
    message.includes("spatial worker count must be between 1 and 8, got 9"),
    `spatialWorkers=9 is refused by the bindings (${message.slice(0, 70)})`,
  );
}

// Opening a file of a retired family names it and says to reconvert (A4 of the V8 freeze). The
// fixtures are the real header and footer of each retired writer; the async reader decides from
// the footer it already read, so a V8 open costs no extra request.
const retiredDir = join(root, "crates/viewer_format/tests/fixtures/retired");
for (const name of ["v5", "v6.1-pre", "v6.2-pre", "v6.3-pre", "v6.4-pre", "v7.1-pre", "v7.2-pre", "v7.3-pre", "v7.4-pre", "v7.5-pre", "v7", "v8.1-pre", "v8.2-pre"]) {
  const head = readFileSync(join(retiredDir, `${name}.head`));
  const foot = readFileSync(join(retiredDir, `${name}.foot`));
  const bytes = Buffer.concat([head, Buffer.alloc(200), foot]);
  let message = "";
  try {
    await WasmAsyncFormatReader.open(new Blob([bytes]), undefined, undefined);
  } catch (error) {
    message = String(error?.message ?? error);
  }
  report(
    message.includes(`this file is format ${name}, retired with V8`) && message.includes("reconvert"),
    `${name} is refused by the async reader as retired (${message.slice(0, 80)})`,
  );
}

// A full spill zone or a full output surfaces as a `QuotaExceededError`.
const quotaCases = [
  { name: "zone-quota", options: { zoneQuota: 256 * 1024 } },
  { name: "output-quota", options: { outputQuota: 64 * 1024 } },
];
for (const { name, options } of quotaCases) {
  try {
    bindingConvert(name, small, options);
    report(false, `${name}: the conversion succeeded`);
  } catch (error) {
    report(error?.name === "QuotaExceededError", `${name}: ${error?.name ?? typeof error}: ${error?.message ?? error}`);
  }
}
process.exit(failed ? 1 : 0);
