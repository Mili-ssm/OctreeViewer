// Prints the wasm32 encoding digest in the same format as `native_digest`.
// Build first: cargo build --release -p format_parity --target wasm32-unknown-unknown --lib
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const path = fileURLToPath(
  new URL("../../target/wasm32-unknown-unknown/release/format_parity.wasm", import.meta.url),
);
const { instance } = await WebAssembly.instantiate(readFileSync(path), {});
const digest = instance.exports.format_parity_digest();
console.log(BigInt.asUintN(64, digest).toString(16).padStart(16, "0"));
