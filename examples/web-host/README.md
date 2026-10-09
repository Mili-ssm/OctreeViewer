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

The demo uses a fixed oblique matrix for its synthetic sample and retries renderer initialization or
recreates the viewer after a rendering error. It requires a browser and GPU with WebGPU support. Camera
navigation and local file loading are not implemented yet.
