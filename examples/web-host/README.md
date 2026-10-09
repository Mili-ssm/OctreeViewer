# web-host demo

This folder demonstrates the embeddable contract for the viewer. The host owns the `canvas`,
instantiates the WASM module and renderer, and uploads raw point buffers. Rust draws the scene to the
canvas with WebGPU; JavaScript does not copy the render instances back for drawing.

## Build the WASM package

```bash
rustup target add wasm32-unknown-unknown
cd crates/viewer_wasm
wasm-pack build --target web --out-dir ../../pkg
```

## Serve the demo

Use any static file server rooted at the repository. Then open `examples/web-host/index.html`.

The Rust camera fits the loaded scene and supports orbit, pan, zoom, and resize. Drag to orbit, use
Shift+drag to pan, and scroll to zoom; keyboard users can focus the canvas and use the arrow keys,
Shift+arrows, `+`/`-`, and `Home`. The camera uses Z as the vertical axis. It requires a browser and
GPU with WebGPU support. Bounds currently come from loaded instances; local file loading is not
implemented yet. Renderer initialization and frame errors can be recovered by recreating the viewer.

The demo exposes RGB, intensity, elevation, and classification color modes, plus a point-size
multiplier from `0.1` to `4`. The renderer API accepts `0.1` to `10`. Intensity and classification
need scalar attributes from `PointBlock`; the current synthetic sample only has RGB, so missing
intensity uses grayscale luminance and missing classification keeps RGB. Hosts can call
`viewer.setColorMode("elevation")` and `viewer.setPointSize(1.2)`.

The stats panel reports visible instances/blocks, cumulative bytes written to instance buffers, and
the synchronous JS/WASM time spent inside `renderFrame()`. This call duration is CPU-side and does
not measure GPU execution or display latency.
