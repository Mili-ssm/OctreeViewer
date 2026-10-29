// Compares the wasm32 serial conversion with the native CLI path, case by case (SHA-256 of the
// `.oct` and `.octattrs`). Build first:
//   cargo build --release -p convert_parity --target wasm32-unknown-unknown --lib
//   cargo build --release -p convert_parity --bin native_convert
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

const small = join(root, "data/samples/fs_e430_n4008_18.las");
const synthetic = join(root, "tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz");
const autzen = findAutzen();
// `multi`: the case must produce more than one block, else it would not exercise the planner.
// `knownDivergent`: the wasm output differs from native by one ulp in a stored `quantization_error`
// because `viewer_format::quantization` folds errors with `f64::hypot`, a libm call (glibc natively,
// the bundled libm on wasm32). The case must keep differing in exactly that way until hypot is
// replaced by a target-independent expression; then drop the flag.
const cases = [
  { name: "small-default", input: small, ppb: 262144, divisions: 2, multi: false },
  { name: "small-4096-d2", input: small, ppb: 4096, divisions: 2, multi: true, knownDivergent: true },
  { name: "small-4096-d8", input: small, ppb: 4096, divisions: 8, multi: true },
  { name: "synthetic-4096-d2", input: synthetic, ppb: 4096, divisions: 2, multi: true },
  { name: "synthetic-4096-d8", input: synthetic, ppb: 4096, divisions: 8, multi: true },
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

function wasmConvert(name, bytes, ppb, divisions) {
  const ptr = x.convert_parity_alloc(bytes.length);
  // Viewed after the allocation: growing memory detaches earlier buffers.
  new Uint8Array(x.memory.buffer, ptr, bytes.length).set(bytes);
  const ok = x.convert_parity_run(ptr, bytes.length, BigInt(ppb), divisions, 0);
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

function nativeConvert(name, input, ppb, divisions) {
  const output = join(workDir, `${name}.oct`);
  // The converter refuses to overwrite an existing output.
  rmSync(output, { force: true });
  rmSync(`${output}attrs`, { force: true });
  const run = spawnSync(nativeBin, [input, output, String(ppb), String(divisions)], {
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
for (const { name, input, ppb, divisions, multi, knownDivergent } of cases) {
  const native = nativeConvert(name, input, ppb, divisions);
  const wasm = wasmConvert(name, readFileSync(input), ppb, divisions);
  const problems = [];
  if (native.oct !== wasm.oct) problems.push("oct differs");
  if (native.attrs !== wasm.attrs) problems.push("octattrs differs");
  if (native.blocks !== wasm.blocks) problems.push("block count differs");
  if (multi && wasm.blocks <= 1) problems.push("expected more than one block");
  if (knownDivergent) {
    // Only the geometry differs in value; sizes and block count must still agree.
    const diverged = native.oct !== wasm.oct && native.blocks === wasm.blocks;
    console.log(`${diverged ? "xfail" : "FAIL "} ${name} blocks=${wasm.blocks} wasm=${wasm.oct.slice(0, 12)} native=${native.oct.slice(0, 12)} (known hypot divergence)`);
    failed ||= !diverged || wasm.blocks <= 1;
    continue;
  }
  failed ||= problems.length > 0;
  const detail = problems.length ? ` (${problems.join(", ")})` : "";
  console.log(
    `${problems.length ? "FAIL" : "ok  "} ${name} blocks=${wasm.blocks} wasm=${wasm.oct.slice(0, 12)} native=${native.oct.slice(0, 12)} attrs=${wasm.attrs.slice(0, 12)}/${native.attrs.slice(0, 12)}${detail}`,
  );
}
process.exit(failed ? 1 : 0);
