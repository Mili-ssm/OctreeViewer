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
