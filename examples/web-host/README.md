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

## Export a local single-file viewer

To create a package that works offline and can be opened directly from `file://` without starting a
server, build the WASM module and its JS bindings, then run the packager:

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --offline -p viewer_wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/viewer_wasm.wasm
python3 examples/web-host/package-local.py
```

The output is `artifacts/visor-web-local/visor-web-local.zip`. Extract it and open `index.html` in a
browser with WebGPU and OffscreenCanvas support. The HTML embeds the runtime, WASM binary and sample,
so it does not fetch assets from the repository or the network.

### Cargar una nube

1. Espera a que el estado indique que el renderizador está listo.
2. Pulsa **Open project dataset** y selecciona un archivo de proyecto `.oct` (versiones 1, 2 o 3).
3. El visor muestra primero una vista gruesa y luego refina progresivamente. Ajusta **Finest voxel
   detail** para elegir el detalle mínimo que quieres intentar.

La nube se lee localmente en el navegador; el paquete no la sube a ningún servicio. No acepta LAS/LAZ
directamente en este selector: conviértelos primero al formato `.oct` del proyecto.

The target browsers are Chrome and Firefox builds that support WebGPU and `OffscreenCanvas`; visual
rendering has been checked in Chrome with converted Bunny and Outer Banks datasets. Firefox still
requires browser validation. The host transfers the canvas
with `transferControlToOffscreen()`, then the worker calls
`WasmVoxelViewer.createOffscreen`. Select a project-format v1/v2/v3 file to call `openFormatDataset` and
`updateLod`; the worker publishes a coarse visible cut first, then loads at most one missing block
per `renderFrame()` and refines through projected voxel stages until the requested detail is reached.
Pending blocks are prioritized against the current camera, and a camera change restarts selection from
the coarse frontier. The worker can process camera commands between block reads; each individual
`FileReaderSync` read is still synchronous, so a slow block can delay input until that read completes.
The host reports time to the first coarse frame and target LOD frame, plus p50/p95/p99 worker
`renderFrame()` times over a bounded window of up to 4,096 frames. Real-browser latency and frame-time
validation remain pending in the [project roadmap](../../README.md#plan-de-trabajo).
The camera fits dataset bounds and supports orbit, pan, zoom, and resize. Drag to orbit, use Shift+drag
to pan, and scroll to zoom; keyboard users can focus the canvas and use arrow keys, Shift+arrows,
`+`/`-`, and `Home`. The camera uses Z as the vertical axis. It requires a browser/GPU with WebGPU and
OffscreenCanvas support. The host recreates a transferred canvas by replacing it and starting a new
worker.

Use the keyboard-accessible **Open built-in sample** button to load the included v1/v2 file, or focus
the labeled file picker to choose another local dataset. The sample is generated from a fixed 64×64
wave grid and does not depend on external or licensed point-cloud data.

The demo exposes RGB, intensity, elevation, and classification color modes, plus one slider for the
finest projected voxel detail to try (0.5–16 framebuffer pixels). `updateLod(targetVoxelPixels)` selects
source detail progressively from 16 px toward the requested target, then aggregates it into one global
adaptive dyadic grid. Cells subdivide 2×2×2 according to their twelve projected edges; storage-node
depth and block size do not set rendered voxel size. The dyadic steps give approximately target/2 to
target pixels per maximum cell edge. Full-resolution leaf points can use arbitrarily finer cells up to
the precision limit (24 subdivisions). Internal representatives require sufficient source detail
before drawing a finer grid. The 8-million safety cap applies to generated render voxels, not source
points: framebuffer area/target² does not bound hidden surfaces or source points. Missing required
blocks or exhausted voxel capacity preserve the
previous scene and report an error. Cells share integer identities; there is no point-size multiplier.
Project-format files preserve their attributes for color modes; the synthetic sample only has RGB,
so missing intensity uses grayscale luminance and missing classification keeps RGB.
The stats also expose the projected maximum-edge range and the count of voxels above the target,
including precision-limited cells. The `renderedVoxels` count reflects the GPU-visible instance count
after asynchronous HZB readback when that path is active; `hzbActive` indicates that a valid static-view
depth pyramid is ready. Those measurements do not report rasterized fragments or pixel coverage of every face.
Depth shading is enabled by default and can be disabled in Rendering options. It uses face orientation
and camera depth in the existing voxel shader; it is a lightweight depth cue, not physical ambient
occlusion.

For opaque voxel scenes of at least 4,096 instances, the renderer can build a hierarchical Z buffer
(HZB) and cull fully hidden cubes on the GPU while the camera and scene remain unchanged. The first
frame after a camera, scene, or viewport change uses the complete scene and rebuilds the depth pyramid;
frames without adapter support, within a tight GPU budget, with transparent cells, or on the per-block
render path keep the direct renderer. The source instance buffer remains resident and all candidates
are checked by the compute shader, so HZB reduces draw/vertex/raster work but does not reduce scene
buffer memory or candidate traversal. This avoids stale-depth holes, but the GPU-time effect still
needs measurement in Chrome and Firefox with Albuera.

The stats panel reports visible source points, deduplicated voxels/blocks, deepest visible LOD level, LOD coverage,
pending block count, decoded CPU cache usage/budget, cumulative bytes written to instance buffers, and
the synchronous CPU time spent inside the worker's `renderFrame()`. This does not measure GPU
execution or display latency. File reads report unique
bytes fetched in 256 KiB ranges. This is not a completion percentage: the reader seeks through
metadata and selected blocks, so it may never read the full file. Cancel terminates the synchronous
worker, replaces its transferred canvas, and starts a fresh viewer; select the file again to retry.
The host keeps at most one worker command of each repeatable kind in flight and replaces its pending
value with the latest input. It also allows only one frame request in flight; file reads remain
synchronous inside the worker.

The browser host caps the render framebuffer at 4,194,304 pixels and 4,096 pixels
per dimension to limit swapchain, depth, and HZB allocation on high-DPI displays.
It renders continuously while loading/refining, then idles until input or a renderer
command needs another frame. Only the explicit 15-second FPS benchmark runs
continuously after loading. The benchmark JSON records both the browser DPR and
the effective framebuffer scale.

For web benchmarks, open one file at a fixed viewport/detail, wait until the target LOD is complete,
click **Run 15 s FPS benchmark**, keep the camera still until it completes, then export the JSON. It includes file/index open time, first
coarse frame, target-LOD time, a warmed 10-second worker-frame-reply rate, reply intervals and WASM/CPU
frame percentiles. Frame replies mean frames submitted by the worker; they are not GPU completion or
display FPS. The [benchmark protocol](../../docs/WEB_PERFORMANCE_BENCH.md) defines a dataset-size,
LOD-detail and viewport matrix, and `collect-benchmarks.py` aggregates repeated exports by median.

The generic WASM point APIs accept typed arrays and stage them in batches of 16,384 points into
Rust-owned storage. Full-scene inputs are limited by the renderer's visible-instance budget;
`readNext` is limited to 65,536 points and 8 MiB of source records per response. Use
`renderSceneBatch(start, count)` for diagnostic snapshots, with at most 16,384 instances per call.
These APIs copy across JS/WASM memory; the local-format viewing path reads selected blocks directly in
Rust and avoids materializing the full file in JS.

Run the host-side queue tests with `node --test examples/web-host/tests/*.test.mjs`. Browser interaction,
WebGPU presentation, and worker cancellation still need a real browser check.
