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
