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
# Each command must succeed on its own and the digest must be non-empty: inside test "$(a)" = "$(b)"
# a failure of both sides would compare "" = "" and pass.
step "format parity wasm32" bash -c 'set -euo pipefail; cargo build -q --release -p format_parity --target wasm32-unknown-unknown --lib $0 && n=$(cargo run -q --release -p format_parity --bin native_digest $0) && w=$(node tools/format_parity/run-wasm.mjs) && [ -n "$n" ] && [ "$n" = "$w" ]' "$CARGO_MODE"
step "cargo doc" cargo doc --workspace --no-deps $CARGO_MODE
step "node tests" node --test examples/web-host/tests/*.test.mjs
step "python web-host tests" python3 -m unittest discover -s examples/web-host/tests -p 'test_*.py'
step "python benchmark tests" python3 -m unittest discover -s crates/viewer_convert/benchmarks -p 'test_*.py'
step "python scripts tests" python3 -m unittest discover -s scripts/tests -p 'test_*.py'
step "docs links" python3 scripts/check_docs_links.py
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
