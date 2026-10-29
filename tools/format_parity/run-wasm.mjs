// Prints the wasm32 encoding digest in the same format as `native_digest`.
// Build first: cargo build --release -p format_parity --target wasm32-unknown-unknown --lib
// Optional argument: a `dump_blocks` file; its bytes are copied into wasm memory and digested.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const path = fileURLToPath(
  new URL("../../target/wasm32-unknown-unknown/release/format_parity.wasm", import.meta.url),
);
const { instance } = await WebAssembly.instantiate(readFileSync(path), {});
const { memory, format_parity_alloc, format_parity_digest_blocks, format_parity_digest } =
  instance.exports;
let digest;
if (process.argv[2]) {
  const bytes = readFileSync(process.argv[2]);
  const ptr = format_parity_alloc(bytes.length);
  // Viewed after the allocation: growing memory detaches earlier buffers.
  new Uint8Array(memory.buffer, ptr, bytes.length).set(bytes);
  digest = format_parity_digest_blocks(ptr, bytes.length);
} else {
  digest = format_parity_digest();
}
console.log(BigInt.asUintN(64, digest).toString(16).padStart(16, "0"));
