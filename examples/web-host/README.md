# web-host demo

This folder demonstrates the embeddable contract for the viewer. The host owns the `canvas` and
controls a dedicated module worker. The worker owns the WASM viewer, `OffscreenCanvas`, format reader,
and WebGPU renderer. JavaScript does not copy the render instances back for drawing.

## Build the WASM package

```bash
rustup target add wasm32-unknown-unknown
cd crates/viewer_wasm
wasm-pack build --target web --out-dir ../../pkg
```

The repository includes `sample.oct`, a deterministic 4,096-point dataset. Regenerate it from the
workspace root with:

```bash
cargo run --offline -p viewer_format --example create_web_sample -- examples/web-host/sample.oct
```

## Serve the demo

Use any static file server rooted at the repository. Then open `examples/web-host/index.html`. WebGPU
and module workers require a secure context such as `localhost` or HTTPS.

The host transfers the canvas with `transferControlToOffscreen()`, then the worker calls
`WasmVoxelViewer.createOffscreen`. Select a project-format v1/v2 file to call `openFormatDataset` and
`updateLod`; requested blocks are read and decoded in the worker before each atomic visible-cut swap.
The camera fits dataset bounds and supports orbit, pan, zoom, and resize. Drag to orbit, use Shift+drag
to pan, and scroll to zoom; keyboard users can focus the canvas and use arrow keys, Shift+arrows,
`+`/`-`, and `Home`. The camera uses Z as the vertical axis. It requires a browser/GPU with WebGPU and
OffscreenCanvas support. The host recreates a transferred canvas by replacing it and starting a new
worker.

Use the keyboard-accessible **Open built-in sample** button to load the included v1/v2 file, or focus
the labeled file picker to choose another local dataset. The sample is generated from a fixed 64×64
wave grid and does not depend on external or licensed point-cloud data.

The demo exposes RGB, intensity, elevation, and classification color modes, plus a point-size
multiplier from `0.1` to `4`. The renderer API accepts `0.1` to `10`. Project-format files preserve
their attributes for color modes; the synthetic sample only has RGB, so missing intensity uses
grayscale luminance and missing classification keeps RGB. `quality` targets 1/16 px projected error
with a default 4 million point budget; `performance` targets 2 px and defaults to 1 million points.

The stats panel reports visible points/instances/blocks, LOD coverage, pending block count, cumulative
bytes written to instance buffers, and the synchronous CPU time spent inside the worker's
`renderFrame()`. This does not measure GPU execution or display latency. File reads report unique
bytes fetched in 256 KiB ranges. This is not a completion percentage: the reader seeks through
metadata and selected blocks, so it may never read the full file. Cancel terminates the synchronous
worker, replaces its transferred canvas, and starts a fresh viewer; select the file again to retry.
The host keeps at most one worker command of each repeatable kind in flight and replaces its pending
value with the latest input. It also allows only one frame request in flight; file reads remain
synchronous inside the worker.

The generic WASM point APIs accept typed arrays and stage them in batches of 16,384 points into
Rust-owned storage. Full-scene inputs are limited by the renderer's visible-instance budget;
`readNext` is limited to 65,536 points and 8 MiB of source records per response. Use
`renderSceneBatch(start, count)` for diagnostic snapshots, with at most 16,384 instances per call.
These APIs copy across JS/WASM memory; the local-format viewing path reads selected blocks directly in
Rust and avoids materializing the full file in JS.
