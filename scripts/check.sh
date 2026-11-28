#!/usr/bin/env bash
# Local CI: runs every check. Offline by default; CHECK_LOCKED=1 uses --locked.
set -euo pipefail
cd "$(dirname "$0")/.."

CARGO_MODE="--offline"
if [ "${CHECK_LOCKED:-0}" = "1" ]; then
  CARGO_MODE="--locked"
fi

steps=()
step() {
  local name="$1"
  shift
  echo
  echo "=== $name ==="
  "$@"
  steps+=("ok   $name")
}

export RUSTDOCFLAGS="-D warnings"

step "tests (default features)" cargo test --workspace --profile fast $CARGO_MODE
step "tests (all features)" cargo test --workspace --all-features --profile fast $CARGO_MODE
step "fmt" cargo fmt --all -- --check
step "clippy (default features)" cargo clippy --workspace --all-targets $CARGO_MODE -- -D warnings
step "clippy (all features)" cargo clippy --workspace --all-targets --all-features $CARGO_MODE -- -D warnings
step "clippy wasm32" cargo clippy -p viewer_wasm --target wasm32-unknown-unknown $CARGO_MODE -- -D warnings
step "core wasm32 check" cargo check -p viewer_convert_core --target wasm32-unknown-unknown $CARGO_MODE
step "core wasm32 clippy" cargo clippy -p viewer_convert_core --target wasm32-unknown-unknown $CARGO_MODE -- -D warnings
step "core has no native-only deps" bash -c '! cargo tree -p viewer_convert_core -e features --target wasm32-unknown-unknown $0 | grep -E "laz-parallel|rayon|ctrlc"' "$CARGO_MODE"
# Each command must succeed on its own and the digest must be non-empty: inside test "$(a)" = "$(b)"
# a failure of both sides would compare "" = "" and pass.
step "format parity wasm32" bash -c 'set -euo pipefail; cargo build -q --release -p format_parity --target wasm32-unknown-unknown --lib $0 && n=$(cargo run -q --release -p format_parity --bin native_digest $0) && w=$(node tools/format_parity/run-wasm.mjs) && [ -n "$n" ] && [ "$n" = "$w" ]' "$CARGO_MODE"
# Optional: needs the git-ignored Belvedere dataset (override the path with FORMAT_PARITY_REAL_INPUT).
# Plans all of Belvedere on every run (~31 s, ~16 GB of spools written).
# Dumps 3 large leaf blocks + 1 LOD block (pre-mortem of 29-10-2026: the first planned blocks are
# too small to reach lz4_flex's 64 KiB large-hash-table path) and compares native vs wasm32.
real_input="${FORMAT_PARITY_REAL_INPUT:-data/datasets/belv_2009_digital-aerial_pcd_copc.laz}"
if [ -f "$real_input" ]; then
  step "format parity real blocks" bash -c 'set -euo pipefail; dump=target/format_parity/belvedere.blocks; cargo build -q --release -p format_parity --target wasm32-unknown-unknown --lib $0 && cargo run -q --release -p format_parity --bin dump_blocks $0 -- "$1" "$dump" && n=$(cargo run -q --release -p format_parity --bin native_digest $0 -- "$dump") && w=$(node tools/format_parity/run-wasm.mjs "$dump") && [ -n "$n" ] && [ "$n" = "$w" ]' "$CARGO_MODE" "$real_input"
else
  echo
  echo "=== format parity real blocks: skipped ($real_input not found) ==="
  steps+=("skip format parity real blocks (no dataset)")
fi
# Converts fixtures with the serial core path on wasm32 (Node) and with the native CLI path and
# compares the SHA-256 of the .oct/.octattrs pair (M3a). Autzen COPC is used when the cargo registry has it.
step "convert parity wasm32" bash -c 'set -euo pipefail; cargo build -q --release -p convert_parity --target wasm32-unknown-unknown --lib $0 && cargo build -q --release -p convert_parity --bin native_convert --bin native_hybrid $0 && node tools/convert_parity/run.mjs' "$CARGO_MODE"
# Runs the real viewer_wasm glue (wasm-bindgen --target web) under Node against the native CLI path
# with a tiny spool budget. The CLI must match Cargo.lock's wasm-bindgen; CHECK_WASM_BINDINGS=0 skips it.
wanted_bindgen=$(awk '/^name = "wasm-bindgen"$/ {getline; gsub(/[^0-9.]/, "", $0); print; exit}' Cargo.lock)
have_bindgen=$(wasm-bindgen --version 2>/dev/null | awk '{print $2}' || true)
if [ "${CHECK_WASM_BINDINGS:-1}" = "0" ]; then
  echo
  echo "=== convert parity bindings: skipped (CHECK_WASM_BINDINGS=0) ==="
  steps+=("skip convert parity bindings (CHECK_WASM_BINDINGS=0)")
elif [ -z "$have_bindgen" ] || [ "$have_bindgen" != "$wanted_bindgen" ]; then
  echo
  echo "=== convert parity bindings: skipped (wasm-bindgen CLI '${have_bindgen:-missing}', Cargo.lock wants $wanted_bindgen) ==="
  steps+=("skip convert parity bindings (wasm-bindgen CLI mismatch)")
else
  step "convert parity bindings" bash -c 'set -euo pipefail; cargo build -q --release -p viewer_wasm --target wasm32-unknown-unknown $0 && rm -rf target/convert_parity/pkg && wasm-bindgen --target web --out-dir target/convert_parity/pkg target/wasm32-unknown-unknown/release/viewer_wasm.wasm && cargo build -q --release -p convert_parity --bin native_convert $0 && node tools/convert_parity/run-bindings.mjs' "$CARGO_MODE"
fi
step "cargo doc" cargo doc --workspace --no-deps $CARGO_MODE
step "node tests" node --test examples/web-host/tests/*.test.mjs
step "python web-host tests" python3 -m unittest discover -s examples/web-host/tests -p 'test_*.py'
step "python benchmark tests" python3 -m unittest discover -s crates/viewer_convert/benchmarks -p 'test_*.py'
step "python scripts tests" python3 -m unittest discover -s scripts/tests -p 'test_*.py'
step "docs links" python3 scripts/check_docs_links.py
step "docs index" python3 scripts/check_docs_index.py
step "layout" python3 scripts/check_layout.py
step "version literals" bash scripts/check_version_literals.sh
# The fuzz crate is outside the workspace; FUZZ_CHECK=0 skips both steps (no nightly/C++ toolchain).
if [ "${FUZZ_CHECK:-1}" = "0" ]; then
  echo
  echo "=== fuzz crate: skipped (FUZZ_CHECK=0) ==="
  steps+=("skip fuzz crate (FUZZ_CHECK=0)")
else
  step "fuzz fmt" cargo fmt --manifest-path fuzz/Cargo.toml -- --check
  step "fuzz check" cargo check --manifest-path fuzz/Cargo.toml --offline
fi
# Working tree + index vs HEAD (committed history is not re-checked).
step "git diff --check" git diff --check HEAD

echo
echo "=== summary ==="
printf '%s\n' "${steps[@]}"
echo "all ${#steps[@]} steps passed"
