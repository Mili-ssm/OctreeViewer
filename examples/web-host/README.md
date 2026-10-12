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

The repository includes `sample.oct` and `sample.octattrs`, a deterministic 4,096-point dataset. Regenerate it from the
workspace root with:

```bash
cargo run --offline -p viewer_format --example create_web_sample -- examples/web-host/sample.oct
```

## Serve the demo

La demo de desarrollo usa módulos externos: **no abrir este `index.html` con doble clic**.
`file://` les asigna un origen local que provoca errores de CORS; el arranque detecta ese caso
y muestra un enlace al visor autocontenido. Para usar la demo de desarrollo, desde la raíz:

```bash
python3 -m http.server 8000 --bind 127.0.0.1
```

Abre **http://localhost:8000/examples/web-host/index.html**. Sirve la raíz completa, incluyendo
`pkg/`: servir solo `examples/web-host/` deja fuera los bindings y WASM. WebGPU y los workers
requieren un contexto seguro, como localhost o HTTPS; HTTP desde otra máquina puede no servir.

### Contrato de carga y prevención de regresiones CORS

- **Demo y API modular:** página, workers, módulos JS/MJS y WASM deben servirse desde el mismo
  origen (esquema, host y puerto), con sus rutas relativas intactas. Los módulos necesitan MIME
  JavaScript y el WASM `application/wasm`. Una respuesta HTML/404 en lugar de un módulo también
  rompe el arranque. No mezclar `localhost` con `127.0.0.1` en las URLs de recursos.
- **Doble clic:** abrir el `artifacts/visor-web-local/index.html` generado abajo. Su script,
  bindings, WASM y muestra están embebidos; ambos workers son Blob clásicos sin imports
  externos. No introducir módulos externos, `import.meta.url` ni fetch de assets en esta ruta.
  La API modular en `embed/` conserva su requisito de localhost/HTTPS.
- **Después de cambiar el host, workers o bindings:** regenerar el paquete; el HTML/ZIP son
  una instantánea, no se actualizan automáticamente. `package-local.py` exige encontrar sus
  marcadores y debe fallar si una refactorización rompe la transformación del runtime.
- No usar `fetch(..., { mode: "no-cors" })` ni desactivar la seguridad del navegador: no
  habilitan los módulos de `file://`. La demo en un único origen no necesita cabeceras CORS
  permisivas. [Referencia: módulos JavaScript y pruebas locales (MDN)](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Guide/Modules).

Regresiones automatizadas (desde la raíz, Python 3 y Node.js):

```bash
node examples/web-host/tests/demo-bootstrap.test.mjs
python3 -m unittest discover -s examples/web-host/tests -p 'test_*.py'
```

Cubren el bloqueo preventivo de cargas externas bajo `file://`, el arranque modular HTTP/HTTPS,
y la generación/sintaxis del runtime y ambos workers autocontenidos. No sustituyen una prueba
de renderizado WebGPU en Chrome/Firefox: abrir el paquete generado con doble clic, cargar la
muestra y comprobar la consola; comprobar también el arranque desde localhost.

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
También puedes abrir directamente `artifacts/visor-web-local/index.html` con doble clic; no es el
`index.html` de desarrollo de esta carpeta.

### Convertir y cargar una nube

1. En **Convert a local LAS/LAZ file**, selecciona el `.las` o `.laz` y pulsa **Convert to .oct**.
2. La conversión se ejecuta en un worker aparte y escribe bloques en OPFS; cancelar detiene el trabajo
   entre bloques. El archivo permanece en almacenamiento local del navegador y puede descargarse o
   abrirse en el visor. La cuota disponible puede interrumpir una conversión; los temporales incompletos
   se eliminan cuando el navegador permite cerrar el handle. Si cierras la pestaña abruptamente,
   puede quedar un temporal en el almacenamiento del sitio.
3. Esta ruta web conserva XYZ, intensidad, clasificación, retornos, ángulo, user data, point source ID,
   GPS time, RGB y NIR cuando están presentes. No conserva Extra Bytes ni CRS. Escribe hasta 4.096
   representantes voxelizados y organiza la fuente en ocho celdas espaciales tras dos pasadas.
   No sustituye la jerarquía adaptativa nativa para datasets grandes.
4. Para abrir un `.oct` ya convertido, espera a que el estado indique que el renderizador está listo y
   usa **Open project dataset**. El visor muestra primero una vista gruesa y luego refina progresivamente.
   Ajusta **Finest voxel
   detail** para elegir el detalle mínimo que quieres intentar.

La nube y el archivo convertido se procesan localmente en el navegador; no se suben a ningún servicio.
La conversión requiere WebAssembly, workers y OPFS en un contexto seguro. Los límites de tamaño,
memoria máxima y tiempo para archivos grandes aún no están validados con Albuera ni en varios navegadores.

The target browsers are Chrome and Firefox builds that support WebGPU and `OffscreenCanvas`; visual
rendering has been checked in Chrome with converted Bunny and Outer Banks datasets. Firefox still
requires browser validation. The host transfers the canvas
with `transferControlToOffscreen()`, then the worker calls
`WasmVoxelViewer.createOffscreen`. Select a project-format v5 file to call `openFormatDataset` and
`updateLod`; the worker publishes a coarse visible cut first, then loads at most one missing block
per `renderFrame()` and refines through projected voxel stages until the requested detail is reached.
Pending blocks are prioritized against the current camera, and a camera change restarts selection from
the coarse frontier. The worker can process camera commands between block reads; each individual
`FileReaderSync` read is still synchronous, so a slow block can delay input until that read completes.
The host reports time to the first coarse frame and target LOD frame, plus p50/p95/p99 worker
`renderFrame()` times over a bounded window of up to 4,096 frames. Real-browser latency and frame-time
validation remain pending in the [project roadmap](../../README.md#próximos-criterios-de-cierre).
The camera fits dataset bounds and supports observer and bird navigation. Observer mode uses a voxel
click to set the orbit center, left drag to pan it, right drag to orbit, and the wheel to change radius. Bird mode uses
left drag to pan, right drag to orbit, WASD/QE to move, and the wheel to change movement speed. The
camera uses Z as the vertical axis. It requires a browser/GPU with WebGPU and OffscreenCanvas support.
The host recreates a transferred canvas by replacing it and starting a new worker.

### Embedding from another page

Import `createViewer` from `viewer-api.mjs` and pass it a canvas owned by the host page:

```js
import { createViewer } from "./viewer-api.mjs";

const viewer = createViewer(document.querySelector("canvas"), { mode: "observer" });
await viewer.ready;
await viewer.load(file, { voxelPixels: 1, attributes: optionalAttributesFile });
console.log(await viewer.getMetadata());
console.log(await viewer.getCamera());
viewer.on("stats", (stats) => console.log(stats.lod?.renderedVoxels));
viewer.on("pick", (hit) => console.log(hit?.position, hit?.voxelCenter));
```

The API also provides `setMode("bird" | "observer")`, `setCamera(position, target)`, `fit()`,
`pick(x, y)`, `getStats()`, `on(type, callback)`, and `dispose()`. `pick(x, y)` uses CSS pixels
relative to the canvas and resolves to `null` or `{ position, voxelCenter, voxelSize, distance,
instanceIndex, sceneGeneration }`; observer clicks perform the same pick and move the orbit center.
An instance index is valid only for its returned LOD scene generation. The GPU readback copies at
most 65,536 candidate instances per pick and returns an error if that safety cap is exceeded.
Camera positions/targets returned as arrays by the API
are in source dataset coordinates, with the file's coordinate origin and visual scale applied.
`getCamera()` also returns the active mode, fixed Z-up vector, and bird movement speed.
`renderedVoxels` reports submitted/current HZB-visible instances; it does not guarantee each voxel
contributes a final pixel. Metadata reports bounds and coordinate origin; a CRS is not inferred when
the file does not provide one. The canvas is transferred to a worker and cannot be reused after
`dispose()`; create a replacement canvas before creating another viewer in its place.
Fatal frame errors, worker errors and message decoding failures reject outstanding API requests and
terminate the worker immediately so the browser can release its dataset and GPU resources. To retry,
replace the transferred canvas and create a new viewer; automatic device recreation is not available.
The real browser/WebGPU pick latency and device-loss path still need a Chrome or Firefox run; the
current environment has no Chromium. Firefox 157.0.1 headless can present with WebGPU enabled
in a temporary profile outside the command sandbox; the first small-file benchmark is recorded
in [web performance](../../docs/WEB_PERFORMANCE_BENCH.md). This does not validate pick or device loss.

The converter is separately embeddable from `converter-api.mjs`. It accepts a local `.las` or `.laz`
`File`, runs in another worker, and returns `{ file, name, attributesFile, outputAttributesName, points, timings }`; progress/status events and
`cancel()` are available while it runs:

```js
import { createConverter } from "./converter-api.mjs";

const converter = createConverter();
converter.on("progress", ({ processedPoints, totalPoints }) => console.log(processedPoints, totalPoints));
const result = await converter.convert(lasFile);
console.log(result.name, result.points, result.file, result.timings);
converter.dispose();
```

The host decides whether to download or open `result.file`. The worker requires OPFS and scans the
source in two bounded passes: one to plan eight spatial child nodes and representatives, then one to
write the output blocks. The root contains up to 4,096 representatives at voxel centers, with color
and RGB averaged per voxel and other attributes copied from the nearest source point. Extra Bytes and
CRS are omitted. Large-file scale and OPFS quota behavior have not been validated in Chrome/Firefox
with Albuera. The result includes `timings.planningMs`, `timings.writingMs`, and `timings.totalMs`; these
are browser wall-clock measurements and still need to be collected on Albuera before choosing whether
per-node OPFS spools would improve the tradeoff.

Use the keyboard-accessible **Open built-in sample** button to load the included v5 file, or focus
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
Use
`renderSceneBatch(start, count)` for diagnostic snapshots, with at most 16,384 instances per call.
These APIs copy across JS/WASM memory; the local-format viewing path reads selected blocks directly in
Rust and avoids materializing the full file in JS.

Run the host-side tests with `node --test examples/web-host/tests/*.test.mjs` and
`python3 -m unittest discover -s examples/web-host/tests -p 'test_*.py'`. They cover request/resource
caps, worker failure cleanup, benchmark-window metrics and filtering/aggregation of exported runs.
Browser interaction, real GPU timing, device loss and cancellation during a local file read still need
a real browser check; no native or mocked timing should be reported as browser FPS.

Benchmark exports retain initial RGB open times under `load` (including its initial detail), and
publish the latest `refinement` with request ID, `trigger` (`dataset`, `color`, `detail`), color,
first-frame and target times. Color/detail share one coalesced settings command. Starting FPS
immediately after a change waits for the newest request and a rendered target frame; changing
settings cancels an older FPS window. Opening another dataset cancels its old window as well.
The collector separates timing triggers and uses refinement times, retaining legacy exports.
`load.refinementWork` still follows its existing cumulative cache-reset scope, not the request
ID; `workerRenderCallDuringLoadMs` is a legacy label for the active refinement's render samples.
The window finisher waits for the full 10,000 ms even if the browser timer fires early.

## Coordinate and attribute files (v5)

Open `.oct` alone for gray geometry. Select its optional `.octattrs` before opening coordinates or
add it afterward using the companion picker (the host reopens the dataset and clears caches).
Display RGB is separate from XYZ. The color selector includes every available numeric column and
component, normalized to finite dataset ranges; constant/nonfinite values are gray. Incompatible
companions fail validation. Conversion returns two downloadable files and opens both together.
The worker closes/cleans both OPFS files on ordinary cancellation or failure. Abrupt termination
can leave partial entries. Successful output remains in site storage until removed by the user.

The native stream format also permits monolithic v5 files for integrations, but both converters
produce pairs. Previous format versions require reconversion. The current range-progress events
count unique reads from the coordinate file; they do not include companion I/O. In contrast,
`refinementWork.rangeReadBytes` includes repeated Blob-cache fills from both files inside LOD
callbacks. Exports identify the sidecar as `dataset.attributes: {name, fileBytes}` or `null`;
the collector separates absent, present and unknown legacy companions, and displays color/depth
settings. Names/sizes describe inputs, not cryptographic content identity.

The reproducible 4,096-point fixture now includes typed intensity (`u16`, 0–65,016) and
classification (`u8`, 0–3), with unchanged positions/display colors. Regenerate the pair with
`cargo run --offline -p viewer_format --example create_web_sample` and repackage the local demo.

Majadas RGB pairing completed three Firefox runs with the same final geometry counters and a
visibly colored capture. `tests/lod-attributes.browser.mjs` checks the typed sample by default;
create it as a module Worker and post one message. Optionally supply coordinate/attribute Blobs,
`modes` and an `incompatibleAttributes` Blob. It validates complete refinement, matching geometry
counts, invalid-column rejection and retention after failed sidecar attachment. It requires
rebuilt WASM/WebGPU/OffscreenCanvas and is separate from the Node suite. Majadas passed intensity
and RGB-component selection plus rejection of Albuera's sidecar. Its sequential API loop timings
are integration diagnostics, not host load/FPS benchmarks. Repeated host column-change loads and
other browsers remain pending. See the
[measured matrix](../../docs/WEB_PERFORMANCE_BENCH.md).
