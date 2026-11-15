// Compares the wasm32 serial conversion with the native CLI path, case by case (SHA-256 of the
// `.oct` and `.octattrs`). Build first:
//   cargo build --release -p convert_parity --target wasm32-unknown-unknown --lib
//   cargo build --release -p convert_parity --bin native_convert --bin native_hybrid
// Exits non-zero on any mismatch, failed conversion or single-block case that must be multi-block.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

const root = fileURLToPath(new URL("../../", import.meta.url));
const wasmPath = join(root, "target/wasm32-unknown-unknown/release/convert_parity.wasm");
const nativeBin = join(root, "target/release/native_convert");
const hybridBin = join(root, "target/release/native_hybrid");
const workDir = join(root, "target/convert_parity");
mkdirSync(workDir, { recursive: true });

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

function findAutzen() {
  const registry = join(process.env.CARGO_HOME ?? join(homedir(), ".cargo"), "registry/src");
  if (!existsSync(registry)) return null;
  for (const index of readdirSync(registry)) {
    const crates = readdirSync(join(registry, index)).filter((name) => name.startsWith("las-0.11"));
    for (const crate of crates) {
      const candidate = join(registry, index, crate, "tests/data/autzen.copc.laz");
      if (existsSync(candidate)) return candidate;
    }
  }
  return null;
}

// LAS 1.2, point format 1 with 4 Extra Bytes (u16 + u8 + i8, described by an Extra Bytes VLR) and
// every flag bit set somewhere: exercises the RF-24 columns of `v7.5-pre`. Fully deterministic.
function writeFlagsExtraLas(path) {
  const points = 6000;
  const recordLength = 32;
  const descriptor = (dataType, name) => {
    const d = Buffer.alloc(192);
    d.writeUInt8(dataType, 2);
    d.write(name, 4, "latin1");
    return d;
  };
  const vlrData = Buffer.concat([descriptor(3, "amplitude"), descriptor(1, "small"), descriptor(2, "signed")]);
  const vlr = Buffer.alloc(54);
  vlr.write("LASF_Spec", 2, "latin1");
  vlr.writeUInt16LE(4, 18);
  vlr.writeUInt16LE(vlrData.length, 20);
  const offsetToPoints = 227 + vlr.length + vlrData.length;
  const header = Buffer.alloc(227);
  header.write("LASF", 0, "latin1");
  header.writeUInt8(1, 24);
  header.writeUInt8(2, 25);
  header.writeUInt16LE(227, 94);
  header.writeUInt32LE(offsetToPoints, 96);
  header.writeUInt32LE(1, 100);
  header.writeUInt8(1, 104);
  header.writeUInt16LE(recordLength, 105);
  header.writeUInt32LE(points, 107);
  header.writeUInt32LE(points, 111);
  for (const [i, v] of [0.01, 0.01, 0.01, 0, 0, 0, 1000, 0, 1000, 0, 100, 0].entries()) {
    header.writeDoubleLE(v, 131 + 8 * i);
  }
  const body = Buffer.alloc(points * recordLength);
  for (let i = 0; i < points; i += 1) {
    const o = i * recordLength;
    body.writeInt32LE((i * 7919) % 100000, o);
    body.writeInt32LE((i * 104729) % 100000, o + 4);
    body.writeInt32LE((i * 31) % 10000, o + 8);
    body.writeUInt16LE((i * 17) % 65536, o + 12);
    body.writeUInt8(1 | (1 << 3) | ((i % 2) << 6) | ((i % 3 === 0 ? 1 : 0) << 7), o + 14);
    body.writeUInt8((i % 19) | ((i % 4 === 0 ? 1 : 0) << 5) | ((i % 5 === 0 ? 1 : 0) << 6) | ((i % 7 === 0 ? 1 : 0) << 7), o + 15);
    body.writeUInt16LE(i % 11, o + 18);
    body.writeDoubleLE(i * 0.5, o + 20);
    body.writeUInt16LE((i * 257) % 65536, o + 28);
    body.writeUInt8(i % 256, o + 30);
    body.writeInt8((i % 200) - 100, o + 31);
  }
  writeFileSync(path, Buffer.concat([header, vlr, vlrData, body]));
}
const flagsExtra = join(workDir, "flags-extra-input.las");
writeFlagsExtraLas(flagsExtra);

const small = join(root, "data/samples/fs_e430_n4008_18.las");
const synthetic = join(root, "tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz");
const autzen = findAutzen();
// `multi`: the case must produce more than one block, else it would not exercise the planner.
const cases = [
  { name: "small-default", input: small, ppb: 262144, divisions: 2, multi: false },
  { name: "small-4096-d2", input: small, ppb: 4096, divisions: 2, multi: true },
  { name: "small-4096-d8", input: small, ppb: 4096, divisions: 8, multi: true },
  { name: "synthetic-4096-d2", input: synthetic, ppb: 4096, divisions: 2, multi: true },
  { name: "synthetic-4096-d8", input: synthetic, ppb: 4096, divisions: 8, multi: true },
  { name: "small-v74", input: small, ppb: 262144, divisions: 2, multi: false, format: "v7.4-pre" },
  { name: "small-v75", input: small, ppb: 262144, divisions: 2, multi: false, format: "v7.5-pre" },
  { name: "flags-extra-v75", input: flagsExtra, ppb: 262144, divisions: 2, multi: false, format: "v7.5-pre" },
  { name: "synthetic-v75", input: synthetic, ppb: 262144, divisions: 2, multi: false, format: "v7.5-pre" },
  { name: "synthetic-v75-hybrid", input: synthetic, ppb: 262144, divisions: 2, multi: false, format: "v7.5-pre", rowOrder: "hybrid", sourceTwin: "synthetic-v75" },
  { name: "synthetic-v74", input: synthetic, ppb: 262144, divisions: 2, multi: false, format: "v7.4-pre" },
];
if (autzen) {
  cases.push({ name: "autzen-copc-4096-d2", input: autzen, ppb: 4096, divisions: 2, multi: false });
} else {
  console.log("skip autzen-copc: las-0.11 test data not found in the cargo registry");
}

// `las` pulls `chrono` -> `js-sys`/`wasm-bindgen`, which leave placeholder imports in the module.
// The conversion never calls them, so each one is stubbed to throw if it does.
const module = await WebAssembly.compile(readFileSync(wasmPath));
const imports = {};
for (const { module: from, name, kind } of WebAssembly.Module.imports(module)) {
  if (kind !== "function") throw new Error(`unexpected ${kind} import ${from}.${name}`);
  imports[from] ??= {};
  imports[from][name] = () => {
    throw new Error(`wasm import called: ${from}.${name}`);
  };
}
const x = (await WebAssembly.instantiate(module, imports)).exports;

const dumpDir = process.env.CONVERT_PARITY_DUMP;

// `convert_parity_run` format argument.
const FORMAT_CODES = { "": 0, "v7.4-pre": 1, "v7.5-pre": 2 };

function wasmConvert(name, bytes, ppb, divisions, format, rowOrder) {
  const ptr = x.convert_parity_alloc(bytes.length);
  // Viewed after the allocation: growing memory detaches earlier buffers.
  new Uint8Array(x.memory.buffer, ptr, bytes.length).set(bytes);
  const ok = x.convert_parity_run(ptr, bytes.length, BigInt(ppb), divisions, 0, FORMAT_CODES[format ?? ""], rowOrder === "hybrid" ? 1 : rowOrder === "source" ? 2 : 0);
  const view = (p, l) => new Uint8Array(x.memory.buffer, p, l);
  if (!ok) {
    const message = new TextDecoder().decode(
      view(x.convert_parity_error_ptr(), x.convert_parity_error_len()),
    );
    throw new Error(`wasm conversion failed: ${message}`);
  }
  const oct = view(x.convert_parity_geometry_ptr(), x.convert_parity_geometry_len());
  const attrs = view(x.convert_parity_attributes_ptr(), x.convert_parity_attributes_len());
  if (dumpDir) {
    // Debugging aid: CONVERT_PARITY_DUMP=<dir> keeps the wasm pair for comparison with the native one.
    mkdirSync(dumpDir, { recursive: true });
    writeFileSync(join(dumpDir, `${name}.oct`), oct);
    writeFileSync(join(dumpDir, `${name}.octattrs`), attrs);
  }
  return {
    blocks: Number(x.convert_parity_block_count()),
    oct: sha256(oct),
    attrs: sha256(attrs),
  };
}

function nativeConvert(name, input, ppb, divisions, format, rowOrder) {
  const output = join(workDir, `${name}.oct`);
  // The converter refuses to overwrite an existing output.
  rmSync(output, { force: true });
  rmSync(`${output}attrs`, { force: true });
  const run = spawnSync(nativeBin, [input, output, String(ppb), String(divisions), ...(format ? [format] : []), ...(rowOrder ? [rowOrder] : [])], {
    encoding: "utf8",
  });
  if (run.status !== 0) throw new Error(`native conversion failed: ${run.stderr}`);
  return {
    blocks: Number(run.stdout.trim()),
    oct: sha256(readFileSync(output)),
    attrs: sha256(readFileSync(`${output}attrs`)),
  };
}

let failed = false;
const results = new Map();
for (const { name, input, ppb, divisions, multi, format, rowOrder, sourceTwin } of cases) {
  const native = nativeConvert(name, input, ppb, divisions, format, rowOrder);
  const wasm = wasmConvert(name, readFileSync(input), ppb, divisions, format, rowOrder);
  results.set(name, native);
  const problems = [];
  // The option must reach both sides: the hybrid pair differs from its source-order twin.
  if (sourceTwin && results.get(sourceTwin).oct === native.oct) problems.push("hybrid equals source order");
  if (native.oct !== wasm.oct) problems.push("oct differs");
  if (native.attrs !== wasm.attrs) problems.push("octattrs differs");
  if (native.blocks !== wasm.blocks) problems.push("block count differs");
  if (multi && wasm.blocks <= 1) problems.push("expected more than one block");
  failed ||= problems.length > 0;
  const detail = problems.length ? ` (${problems.join(", ")})` : "";
  console.log(
    `${problems.length ? "FAIL" : "ok  "} ${name} blocks=${wasm.blocks} wasm=${wasm.oct.slice(0, 12)} native=${native.oct.slice(0, 12)} attrs=${wasm.attrs.slice(0, 12)}/${native.attrs.slice(0, 12)}${detail}`,
  );
}
// The native 4x harness (`native_hybrid`, file spill zones, one worker) must write the same pair as
// the CLI whether the budget forces spilling (64 KiB) or not (2 GiB).
const hybridCases = cases.filter(({ name }) => name === "small-default" || name === "synthetic-4096-d8");
for (const { name, input, ppb, divisions } of hybridCases) {
  const native = nativeConvert(name, input, ppb, divisions);
  for (const [label, budget, spills] of [["64k", 65536, true], ["2g", 2 ** 31, false]]) {
    const output = join(workDir, `${name}-hybrid-${label}.oct`);
    const zones = join(workDir, `${name}-zones-${label}`);
    rmSync(zones, { recursive: true, force: true });
    const args = [input, output, "--divisions", String(divisions), "--points-per-block", String(ppb),
      "--spool-budget-bytes", String(budget), "--zones-dir", zones];
    const run = spawnSync(hybridBin, args, { encoding: "utf8" });
    if (run.status !== 0) throw new Error(`native_hybrid failed: ${run.stderr}`);
    const report = JSON.parse(run.stdout);
    const problems = [];
    if (report.octSha256 !== native.oct) problems.push("oct differs");
    if (report.octattrsSha256 !== native.attrs) problems.push("octattrs differs");
    if (report.blocks !== native.blocks) problems.push("block count differs");
    if (spills !== report.spilledSpools > 0) problems.push(`spilledSpools=${report.spilledSpools}`);
    rmSync(zones, { recursive: true, force: true });
    failed ||= problems.length > 0;
    const detail = problems.length ? ` (${problems.join(", ")})` : "";
    console.log(
      `${problems.length ? "FAIL" : "ok  "} ${name} hybrid-${label} blocks=${report.blocks} spilled=${report.spilledSpools} zones=${report.estimate.zones} ms=${report.elapsedMs} oct=${report.octSha256.slice(0, 12)}${detail}`,
    );
  }
}
process.exit(failed ? 1 : 0);
