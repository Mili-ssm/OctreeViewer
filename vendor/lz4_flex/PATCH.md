# Local patch to lz4_flex 0.14.0

Vendored from crates.io `lz4_flex` 0.14.0 (LICENSE kept). README, Cargo.lock and Cargo.toml.orig removed.

## What changed

Only `src/block/hashtable.rs`, `HashTable::get_hash_at` for `target_pointer_width = "32"`:

- Upstream: `hash(get_batch(input, pos))` (4-byte read, 32-bit multiplicative hash).
- Patched: `hash5_u64(get_batch_u64(input, pos))` (8-byte native-endian read, same constants and
  shifts as the 64-bit `hash5`, with explicit `u64` arithmetic).

The 64-bit path is untouched, so x86_64/aarch64 output is byte-identical to upstream.

## Why

Upstream picks a different hash on 32-bit targets, so blocks larger than 64 KiB (which use
`HashTable4K` instead of `HashTable4KU16`) compress to different, equally valid bytes on wasm32 and
x86_64. The browser converter must produce the same bytes as the CLI
(spec `docs/explanation/conversor-web-paralelo.md`, R1). Verified by `tools/format_parity`.

## How to revert

Remove the `[patch.crates-io]` entry and `exclude` for `vendor/lz4_flex` from the root `Cargo.toml`,
delete `vendor/lz4_flex/`, run `cargo update -p lz4_flex`, and drop the "format parity wasm32" step
from `scripts/check.sh` (or accept that digests differ).
