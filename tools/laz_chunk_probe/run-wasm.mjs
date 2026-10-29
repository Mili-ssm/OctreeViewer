// wasm32 side of the S2 measurement; prints one JSON line shaped like the native `bench`.
// Build first: cargo build --release -p laz_chunk_probe --target wasm32-unknown-unknown --lib
// Usage: node run-wasm.mjs <fixture> [iterations]
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const path = fileURLToPath(
  new URL("../../target/wasm32-unknown-unknown/release/laz_chunk_probe.wasm", import.meta.url),
);
const fixture = readFileSync(process.argv[2]);
const iterations = Number(process.argv[3] ?? 3);
// `las` pulls `chrono` -> `js-sys`/`wasm-bindgen`, which leave wasm-bindgen placeholder imports
// in the module. The decode path never calls them, so each one is stubbed to throw if it does.
const module = await WebAssembly.compile(readFileSync(path));
const imports = {};
for (const { module: from, name, kind } of WebAssembly.Module.imports(module)) {
  if (kind !== "function") throw new Error(`unexpected ${kind} import ${from}.${name}`);
  imports[from] ??= {};
  imports[from][name] = () => {
    throw new Error(`wasm import called: ${from}.${name}`);
  };
}
const instance = await WebAssembly.instantiate(module, imports);
const x = instance.exports;
const ptr = x.laz_probe_alloc(fixture.length);
// Viewed after the allocation: growing memory detaches earlier buffers.
new Uint8Array(x.memory.buffer, ptr, fixture.length).set(fixture);
const chunks = x.laz_probe_load();
if (chunks === 0xffffffffffffffffn) throw new Error("fixture rejected");
const points = x.laz_probe_points();

const hex = (value) => BigInt.asUintN(64, value).toString(16).padStart(16, "0");
const time = (call) => {
  const start = performance.now();
  const result = call();
  return [performance.now() - start, result];
};
const batch = [];
const each = [];
const setup = [];
let batchHash = 0n;
let eachHash = 0n;
// Iteration 0 is a warm-up and is dropped.
for (let i = 0; i <= iterations; i++) {
  const [batchTime, bh] = time(() => x.laz_probe_decode_batch());
  const [eachTime, eh] = time(() => x.laz_probe_decode_each());
  const [setupTime] = time(() => x.laz_probe_setup_each());
  batchHash = bh;
  eachHash = eh;
  if (i > 0) {
    batch.push(batchTime);
    each.push(eachTime);
    setup.push(setupTime);
  }
}
console.log(
  JSON.stringify({
    target: "wasm32",
    chunks: Number(chunks),
    points: Number(points),
    batch_ms: batch,
    each_ms: each,
    setup_ms: setup,
    batch_hash: hex(batchHash),
    each_hash: hex(eachHash),
  }),
);
