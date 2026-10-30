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
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { closeSync, fstatSync, mkdirSync, openSync, readFileSync, readSync, readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { createSpoolHost } from "../../examples/web-host/spool-tiers.mjs";
import { openFsSyncHandle } from "../../examples/web-host/tests/support/fs-sync-handle.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const pkg = join(root, "target/convert_parity/pkg");
const nativeBin = join(root, "target/release/native_convert");
const workDir = join(root, "target/convert_parity/bindings");
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

const { initSync, WasmCoreConverter } = await import(pathToFileURL(join(pkg, "viewer_wasm.js")).href);
if (readdirSync(pkg).includes("snippets")) throw new Error("wasm-bindgen generated snippets");
initSync({ module: readFileSync(join(pkg, "viewer_wasm_bg.wasm")) });

const small = join(root, "data/samples/fs_e430_n4008_18.las");
const synthetic = join(root, "tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz");

function nativeConvert(name, input, ppb, divisions) {
  const output = join(workDir, `${name}.native.oct`);
  const run = spawnSync(nativeBin, [input, output, String(ppb), String(divisions)], { encoding: "utf8" });
  if (run.status !== 0) throw new Error(`native conversion failed: ${run.stderr}`);
  return { oct: sha256(readFileSync(output)), attrs: sha256(readFileSync(`${output}attrs`)) };
}

// Runs one binding conversion; `outputQuota` and `zoneQuota` (bytes) make the fakes write short.
function bindingConvert(name, input, ppb, divisions, { outputQuota = Infinity, zoneQuota = Infinity } = {}) {
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
      { pointsPerBlock: ppb, spatialDivisions: divisions, maxError: 0.01, spoolBudgetBytes: SPOOL_BUDGET_BYTES },
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
  { name: "small-default", input: small, ppb: 262144, divisions: 2 },
  { name: "synthetic-4096-d2", input: synthetic, ppb: 4096, divisions: 2 },
  { name: "synthetic-4096-d8", input: synthetic, ppb: 4096, divisions: 8 },
];
for (const { name, input, ppb, divisions } of cases) {
  const native = nativeConvert(name, input, ppb, divisions);
  const { summary, oct, attrs } = bindingConvert(name, input, ppb, divisions);
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

// A full spill zone or a full output surfaces as a `QuotaExceededError`.
const quotaCases = [
  { name: "zone-quota", options: { zoneQuota: 256 * 1024 } },
  { name: "output-quota", options: { outputQuota: 64 * 1024 } },
];
for (const { name, options } of quotaCases) {
  try {
    bindingConvert(name, synthetic, 4096, 8, options);
    report(false, `${name}: the conversion succeeded`);
  } catch (error) {
    report(error?.name === "QuotaExceededError", `${name}: ${error?.name ?? typeof error}: ${error?.message ?? error}`);
  }
}
process.exit(failed ? 1 : 0);
