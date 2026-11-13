#!/usr/bin/env bash
# R3 Task 11 browser smoke. Run from the repository root; Chrome with WebGPU required.
set -euo pipefail
cargo build --release --offline -p viewer_wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/viewer_wasm.wasm
python3 -m http.server 8767 --bind 127.0.0.1 &
server=$!
trap 'kill $server' EXIT
mkdir -p target/r3-t11
for ds in belvedere majadas; do
  node examples/web-host/probe-cells-smoke.mjs target/render-data/$ds-v75.oct target/r3-t11/$ds-4px.json --px=4 --timeout-s=120 --rest-s=5
done
node examples/web-host/probe-cells-smoke.mjs target/render-data/majadas-v75.oct target/r3-t11/majadas-1px.json --px=1 --timeout-s=300 --rest-s=10
node examples/web-host/probe-cells-smoke.mjs target/render-data/majadas-v75.oct target/r3-t11/majadas-1px-leaf.json --px=1 --poses=fitted,leaf --timeout-s=300 --rest-s=10
