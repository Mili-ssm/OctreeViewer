#!/usr/bin/env bash
# R3 Task 10 browser parity (T10c2). Run from the repository root; Chrome with WebGPU required.
# parity.json was produced before the R3-Q24 gate (three arms); the current probe turns the
# no-build-worker arm into the gate check (gate.json, run with --arms=1) and repeats the
# build-worker arm for parity.
set -euo pipefail
cargo build --release --offline -p viewer_wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/viewer_wasm.wasm
python3 -m http.server 8767 --bind 127.0.0.1 &
server=$!
trap 'kill $server' EXIT
mkdir -p target/r3-t10c2
node examples/web-host/probe-cells-parity.mjs target/render-data/belvedere-v75.oct target/r3-t10c2/parity-final.json
