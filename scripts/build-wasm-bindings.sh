#!/usr/bin/env bash
# Builds the viewer_wasm bindings into pkg/ (repository root) for the web host.
# Uses cargo + wasm-bindgen, like scripts/check.sh; the wasm-bindgen CLI must match Cargo.lock.
set -euo pipefail
cd "$(dirname "$0")/.."

want=$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock)
if ! command -v wasm-bindgen >/dev/null; then
  echo "wasm-bindgen not found: cargo install wasm-bindgen-cli --version ${want}" >&2
  exit 1
fi
have=$(wasm-bindgen --version | awk '{ print $2 }')
if [ "$have" != "$want" ]; then
  echo "wasm-bindgen ${have} does not match Cargo.lock (${want}): cargo install wasm-bindgen-cli --version ${want}" >&2
  exit 1
fi

cargo build --release -p viewer_wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/viewer_wasm.wasm
echo "Built pkg/ (viewer_wasm.js, viewer_wasm_bg.wasm)"
