# web-host demo

This folder demonstrates the embeddable contract for the viewer. The host owns the `canvas`,
instantiates the WASM module, uploads raw point buffers, and asks Rust for a voxel scene snapshot.

## Build the WASM package

```bash
rustup target add wasm32-unknown-unknown
cd crates/viewer_wasm
wasm-pack build --target web --out-dir ../../pkg
```

## Serve the demo

Use any static file server rooted at the repository. Then open `examples/web-host/index.html`.

The current demo renders a host-side debug projection. The next iteration should replace that 2D
projection with the Rust-side `wgpu` renderer while keeping the same host contract.
