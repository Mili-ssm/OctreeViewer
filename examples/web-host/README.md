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

The repository includes `sample.oct` and `sample.octattrs`, a deterministic 4,096-point dataset
regenerated with final v5 headers (`OCTVFRM1`, revision 0). Provisional v5 files are rejected;
reconvert local datasets and rebuild/repackage old embedded demos. Regenerate it from the
workspace root with:

```bash
cargo run --offline -p viewer_format --example create_web_sample -- examples/web-host/sample.oct
```

The split pair `sample-v7.oct` / `sample-v7.octattrs` uses `v7.1-pre` and backs the asynchronous
Blob-reader probe. Regenerate it with:

```bash
cargo run --offline -p viewer_format --example create_web_sample -- examples/web-host/sample-v7.oct v7.1-pre
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
2. La conversión se ejecuta en un worker aparte con el mismo pipeline que el conversor de línea de
   comandos (`viewer_convert_core`); cancelar termina el worker y borra los temporales. El archivo permanece en almacenamiento local del navegador y puede descargarse o
   abrirse en el visor. La cuota disponible puede interrumpir una conversión; los temporales incompletos
   se eliminan cuando el navegador permite cerrar el handle. Si cierras la pestaña abruptamente,
   puede quedar un temporal en el almacenamiento del sitio.
3. Esta ruta web conserva XYZ, intensidad, clasificación, retornos, ángulo, user data, point source ID,
   GPS time, RGB y NIR cuando están presentes. No conserva Extra Bytes ni CRS.
4. Para abrir un `.oct` ya convertido, espera a que el estado indique que el renderizador está listo y
   usa **Open project dataset**. El visor muestra primero una vista gruesa y luego refina progresivamente.
   Ajusta **Finest voxel
   detail** para elegir el detalle mínimo que quieres intentar.

La nube y el archivo convertido se procesan localmente en el navegador; no se suben a ningún servicio.
La conversión requiere WebAssembly, workers y OPFS en un contexto seguro. Los límites de tamaño,
memoria máxima y tiempo para archivos grandes aún no están validados con Albuera ni en varios navegadores.

The target browsers support WebGPU and `OffscreenCanvas`. Firefox 157.0.1 has current v5
XYZ/RGB/typed-attribute and visual evidence. Chrome 155 passes the small functional flows;
current headless captures still require investigation. Older Bunny/Outer Banks Chrome captures
do not certify the current build. See [web measurements](../../docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md).
For a small independent presentation/picking diagnostic, serve the repository and open
[`tests/presentation.html`](tests/presentation.html). The left canvas must be red and the right
must show the RGB sample. `window.presentationResults` records a one-pixel GPU readback,
direct/indirect frames, a center pick in dataset coordinates, an out-of-bounds miss and recentering.
Readback/counters alone do not certify presentation: inspect or capture both canvases. The clear
worker records device-loss/uncaptured errors; this does not test the host's device-loss recovery.
Both workers time out after 30 seconds if they produce no result. See the
[observed Chrome/Firefox split](../../docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md#independent-presentation-and-picking-diagnostic--13-10-2026).

The host transfers the canvas
with `transferControlToOffscreen()`, then the worker calls
`WasmVoxelViewer.createOffscreen`. Select a project-format v5 file to call `openFormatDataset` and
`updateLod`; the worker publishes a coarse visible cut first, then loads at most one missing block
per `renderFrame()` and refines through projected voxel stages until the requested detail is reached.
Pending blocks are prioritized against the current camera, and a camera change restarts selection from
the coarse frontier. The worker can process camera commands between block reads; each individual
`FileReaderSync` read is still synchronous, so a slow block can delay input until that read completes.
The host reports time to the first coarse frame and target LOD frame, plus p50/p95/p99 worker
`renderFrame()` times over a bounded window of up to 4,096 frames. Real-browser latency and frame-time
validation remain pending in the [project roadmap](../../README.md#prioridades-pendientes).
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
The small direct-WASM pick/recenter path passes in Chrome and Firefox via the presentation
diagnostic above. Browser pick latency, CSS/API interaction and the host device-loss path still
need validation.
Chrome 155 is now available and the small software-WebGPU flows pass, but pick/device loss remain
unverified. Firefox 157.0.1 headless can present with WebGPU enabled
in a temporary profile outside the command sandbox; the first small-file benchmark is recorded
in [web performance](../../docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md). This does not validate pick or device loss.

The converter is separately embeddable from `converter-api.mjs`. It accepts a local `.las` or `.laz`
`File`, runs the core converter (the same pipeline as the CLI) in another worker, and resolves to
`{ file, name, attributesFile, outputAttributesName, points, timings, spool? }`:

```js
import { createConverter } from "./converter-api.mjs";

const converter = createConverter({ converterOptions: { pointsPerBlock: 4096 } });
await converter.ready; // orphan `conversion-*.partial` files from earlier sessions are removed
converter.on("progress", ({ processedPoints, totalPoints }) => console.log(processedPoints, totalPoints));
converter.on("warning", ({ message }) => console.warn(message));
const result = await converter.convert(lasFile, { options: { spatialDivisions: 8 } });
console.log(result.name, result.points, result.file, result.timings);
converter.dispose();
```

| Item | Behaviour |
| --- | --- |
| Options | `converterOptions` (creation) or `convert(file, { options })` (per call): `formatVersion`, `pointsPerBlock`, `maxError`, `spatialDivisions`, `maxMemoryMiB`, `ramBudget` (bytes, default 4 GB, capped at 8 GB; only used without OPFS) |
| `ready` | Promise that settles once stale partial files are deleted; `convert()` waits for it on its own, so awaiting it is optional |
| Events | `on("progress" \| "status" \| "warning" \| "complete" \| "cancelled" \| "error", callback)` returns an unsubscribe function |
| `warning` | Emitted when OPFS is unavailable (Chrome `file://`): the conversion runs RAM-only and is limited by `ramBudget`; serve the page over `http://localhost` for large files |
| Quota | Firefox without persistent storage stops at about 10 GiB, so a conversion that cannot fit is rejected up front; the worker asks for `persist()` first. Other browsers are not pre-checked (Chrome allows more than it reports); a `QuotaExceededError` becomes a "Not enough browser storage" error |
| `cancel()` | Terminates the worker, deletes its partial files and rejects with an `AbortError`; the next `convert()` respawns the worker |
| `result.spool` | Spool statistics (spilled bytes, peak pages) when the engine reports them |
| `engine` | **Deprecated.** `"core"` is accepted and does nothing; omitting it uses the core converter. `"legacy"` fails with ``the legacy converter was removed; omit `engine` or use "core"`` |

The host decides whether to download or open `result.file`. Extra Bytes and CRS are omitted.
`timings.totalMs` is a browser wall-clock measurement. Large-file behaviour and quotas per browser are
recorded in the [stage 4 plan](../../docs/explanation/conversor-web-paralelo-etapa4.md).

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
display FPS. The [benchmark protocol](../../docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md) defines a dataset-size,
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

The native stream format also permits monolithic v5 files for integrations, but the web
converter produces pairs. Previous format versions require reconversion. The current range-progress events
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
[measured matrix](../../docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md).

Firefox 157.0.1 headless also passed opening the regenerated local package directly through
`file://` with the HTTP server stopped and a fresh WebGPU-enabled profile. The embedded sample
and sidecar loaded; keyboard type-ahead selected Classification, complete 4,096-voxel refinement
and a 10,000 ms benchmark export succeeded. This validates the standalone load path and that
selector interaction, not OPFS/conversion, all keyboard controls or Chrome. Evidence:
`target/offline-package-smoke/classification.json` and `.png`.

El diagnóstico `lodStats().cacheActivity` y `scene.cacheActivity` del export conserva
contadores de inserciones, evicciones y rechazos durante la vida del dataset, incluso
tras vaciar payloads al cambiar atributos. Son strings decimales; reabrir reinicia la
caché. `refinementWork.evictedPendingBlocks` cuenta expulsiones de bloques que el
plan objetivo vuelve a pedir inmediatamente y se reinicia con los demás contadores
de trabajo. Puede haber presión sin rechazos: el padre visible está protegido y
los hijos aún incompletos pueden expulsarse entre sí. Ver el
[contrato y prueba](../../docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md#cache-pressure-diagnostics--12-10-2026).

## Reproduce a cache/camera diagnostic in Firefox

From the repository root, rebuild the release WASM and bindings as above, serve the repository
on port 8767, and start a dedicated Firefox profile with WebGPU enabled:

```bash
mkdir -p target/firefox-camera-probe
printf '%s\n' 'user_pref("dom.webgpu.enabled", true);' 'user_pref("dom.webgpu.service.enabled", true);' > target/firefox-camera-probe/user.js
firefox --headless --no-remote --profile "$PWD/target/firefox-camera-probe" --remote-debugging-port 9222 about:blank
```

In another terminal (Node 22 or newer, built-in `WebSocket`, no npm dependencies):

```bash
node examples/web-host/probe-cache-camera.mjs data/samples/fs_e432_n4002_18.oct target/cache-camera-probe
```

The optional third argument changes the host URL; the fourth selects a 0.5–16 px target
in 0.5 px steps (default 4). Use a current-format `.oct`; older provisional files fail
at open and the probe reports that error. The probe owns and closes that Firefox session;
use a dedicated instance. It loads coordinates without a sidecar, fits a 1282×722
framebuffer at the requested detail and host cache budget, then sends a trusted BiDi right-button
drag over the canvas after the initial observation. It keeps the 15-second animation window
active so the worker's 30-frame statistics include the camera update; this window is excluded
from any FPS claim. Each phase ends on a detector pause, target completion, 120 seconds
without counter changes, or a 600-second cap. An observation cap is an incomplete result.
A paused retained cut can have complete coverage while the requested target remains pending.

### Scripted load + orbit probe (render benchmark, R0)

Serve the repository root (with `pkg/`) on any port, pass it as `--url`, and pick the browser:

```bash
python3 -m http.server 8791 --bind 127.0.0.1 &
node examples/web-host/probe-orbit-load.mjs data/samples/fs_e430_n4008_18.oct target/orbit-load.json \
  --url=http://127.0.0.1:8791/examples/web-host/index.html \
  [--browser=chrome|firefox] [--px=4] [--max-load-s=120] [--orbit-s=10] [--pixels]
```

`--browser=chrome` (default) launches `/usr/bin/google-chrome` (`CHROME_BIN`) through playwright-core
(`PLAYWRIGHT_CORE` = its directory) with `--headless=new --enable-unsafe-webgpu
--enable-features=Vulkan --use-angle=vulkan`; on the reference machine that reports
`{vendor:"amd", architecture:"rdna-3"}`, a hardware adapter. `--browser=firefox` needs the Firefox
with WebGPU and `--remote-debugging-port 9222` described above; headless Firefox there gets a
software adapter (empty `adapter.info`), so its runs are always `functional`.

It starts loading, orbits the camera with one pointer move every 16 ms for `--orbit-s` seconds
during loading and, if the target LOD is reached, once more afterwards. `OUTPUT.json` holds: frame-reply
interval and round-trip p50/p95/max per phase (the primary latency metric; `renderCallMs` is
secondary), worker busy time and queue wait per message type, async read/decode/insert time,
the timestamped log of every `lod` message, deterministic counters (`sceneBuilds`,
`voxelSourcePoints`, `evictedPendingBlocks`, `uncoveredNodes`, `oversizedVoxels`; `loadedBlocks`
only counts the synchronous path, use `workerMetrics.asyncBlocks.blocks`), adapter info,
Vulkan libraries and DRI fds of the browser processes, `gpu_busy_percent` samples, the SHA-256 of
the served `.wasm`, dataset size and `loadavg`. `--pixels` (opt-in) screenshots the canvas
three times per orbit and counts background pixels inside the projected dataset bounds.
`mode` is `functional` (no wall-clock claims) when `cargo`/`rustc` or another browser is running,
the adapter is software or unidentified (`google`/`llvmpipe`/`swiftshader`, empty `architecture`), or
`gpu_busy_percent` never exceeds 10 %, so a software adapter cannot produce timings.
The page hook `window.__renderBench` (`start`, `summarize`, `workerMetrics`, `apiCall`) backs it;
frame logging is off until `start()`.

Each 10-second sample awaits the exact newly exported Blob and saves both its counters and
the visible panel. `captureWallTimeMs` brackets that export for comparison with the process
sampler's wall-clock timestamp; neither is an atomic joint memory snapshot.
`domMatchesExport` records whether stage/pending agree: progress can update
exported counters before the next rendered frame updates the panel. Camera completion/pause
requires changed worker counters after the input, not only the host's `cameraChanged` flag.
Outputs include session metadata, snapshots, console errors and a final screenshot. Exceptions
fail the command; a recorded cache pause or observation cap is a diagnostic outcome, not a
successful target or valid steady-state FPS benchmark. This probe does not measure GPU memory,
heap JS or a paired performance improvement. Compare equivalent runs separately.

### Progress statistics regression

LOD progress messages carry GPU/viewer counters from the same worker state; the host refreshes
both the panel and benchmark GPU snapshot without waiting for every-thirtieth-frame statistics.
Progress messages do not count as frame replies. Serve the repository on port 8767 and run
`tests/progress.browser.mjs` via Playwright MCP `browser_run_code_unsafe`'s `filename` argument.
It exercises the real host DOM/export with a deterministic Worker in desktop/mobile viewports,
with HTTP caching disabled. It does not exercise WebGPU rendering. Actual Firefox load/camera
validation and prior stale-panel captures are recorded in the web benchmark document.

### Isolate surface presentation memory

Open `tests/surface-memory.html?width=1280&height=720&frames=300` over HTTP; repeat with
640×360 in a fresh browser profile. This independent worker submits clear-only frames with
no WASM/dataset and leaves the device/context alive afterwards. `window.surfaceMemoryResults`
reports drawing/CSS sizes, submitted count and `idle` after queue completion. Capture parent
maps at idle and after 20 seconds using the sampler's identity-checked helper. Drawing dimensions
are limited to 1–2048 and frame count to 1–600. The CSS surface stays at most 640 pixels wide.
Add `&mode=viewer` for the fixed 4,096-point RGB WASM sample; `mode=clear` remains default.
Both modes wait for browser queue completion. Viewer mode temporarily captures the browser
requestDevice result during initialization, restores the prototype immediately, and uses that
queue reference only inside the diagnostic; no product API is changed. Read the
[WASM matrix and counterexamples](../../docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md#fixed-small-wasm-sample-surface-matrix--13-10-2026).
This is a diagnostic submission sequence, not an FPS benchmark or VRAM measurement.
See the [six-run observations](../../docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md#independent-clear-only-surface-memory-matrix--13-10-2026).

For ordered Linux parent-map captures, serve the repository on port 8767 and start a dedicated
WebGPU-enabled Firefox BiDi instance on port 9222. Then run, from the repository root:

```bash
node examples/web-host/probe-surface-memory.mjs target/surface-orbit-run-1 1280 720 orbit 300
```

The command owns and closes its Firefox session. It requires Node 22+ and Python 3, refuses an
existing output directory, records parent maps synchronously after queue completion and 20 s
later, then requests a screenshot. Modes are `clear`, `viewer`, and `orbit`; dimensions are
1–2048 and frame count is 1–600. Output includes the browser session/root identity, capture
metadata and raw maps, console errors, screenshot timing and screenshot. The browser-memory
diagnostic and its CPU build counters do not report physical VRAM.

### Capture a transient parent-process mapping event

For an intermittent parent-process PSS increase, the sampler can make one raw mapping capture:

```bash
python3 examples/web-host/sample-process-memory.py FIREFOX_PID target/firefox-event.jsonl --interval 0.1 --seconds 1300 --root-pss-capture-mib 1024
```

The first readable root PSS sample at or above the threshold triggers one attempt to save
`target/firefox-event.root-smaps.txt`. The JSONL trigger record contains `root_smaps_capture`
with outcome, capture start/end timestamps and threshold bytes. Root kernel starttime is checked
before and after reading; an identity change discards the capture. Existing companion files
are never overwritten. Failed attempts are recorded and not retried in that run. No threshold
means no mapping capture; no crossing means no capture event or companion file.

A mapping read follows its trigger sample and may already see the peak recede. It is not an
atomic snapshot with tree counters and can perturb the process/sampling cadence. Captured VMA
sizes/PSS can identify anonymous/file-backed mappings, but do not by themselves identify an
allocator owner, JS/WASM object, or physical GPU allocation. A 0.1 s sleep reduces the requested
sampling interval without guaranteeing capture of every short event. Compare actual elapsed
sample times, and retain runs without a threshold crossing as negative results.

### Requested GPU resource sizes

`gpuStats().requestedResources` reports decimal byte strings for `instanceBufferBytes`,
`fixedBufferBytes` (cube vertices/indices and camera uniform), `hzbBufferBytes` (params,
visible indices, indirect args and owned count readback), `depthTextureBytes`,
`hzbTextureBytes` and their `totalBytes`. Buffer sizes come from the owned WebGPU buffers;
textures report logical Depth32Float/R32Float texels across their mip levels. Texture views
and bind-group references do not duplicate the underlying resource in this accounting.

These are sizes of resources currently owned by the renderer, **not physical VRAM**, process
RSS, or a synchronized transition peak. Swapchain, queue staging, pending external picking
readbacks, pipelines/bind-group metadata, allocator/driver padding and resources retained by
in-flight commands after replacement are excluded. Existing budget estimates are unchanged.
`uploadedBytes` remains cumulative transfer traffic, not resident allocation. Benchmark JSON
includes these counters under `environment.gpuStats.requestedResources` without adding UI.

### Sample browser process memory on Linux

In a second terminal, attach to the PID of the dedicated Firefox parent launched above:

```bash
python3 examples/web-host/sample-process-memory.py FIREFOX_PID target/firefox-process-memory.jsonl --interval 0.5 --seconds 1300
```

Use a fresh output path for each run. The sampler never launches or terminates the browser;
exit the dedicated browser to end sampling, or let the duration cap stop the sampler. It pins
PID identity using Linux starttime and writes a terminal `root_exited` or `duration_cap` record.
The cap says nothing about viewer completion: inspect the probe's initial/camera outcomes and
console errors separately. Start sampling before loading the dataset to include refinement.

Each JSONL record includes wall-clock nanoseconds, elapsed time, per-process counters and
`tree_bytes` sums from `smaps_rollup`. PSS apportions shared pages; summed RSS counts shared
pages multiple times. `enumerated_processes_readable` means that every enumerated member was
readable with unchanged identity and the parent survived the sample. Unreadable PIDs mark a
partial sample; available partial sums must not be treated as full-tree totals. Missing optional
counters are null. Enumeration and reads are sequential: new processes can appear between them,
and even readable samples are not atomic snapshots. Sampling can miss short-lived processes and
peaks between samples; the configured interval is a minimum sleep cadence, not a guaranteed rate.
These counters do not separate JS/WASM heaps, allocator reservations or dedicated GPU memory.
Do not compare sampled timings to unsampled timings as a performance improvement.

For Playwright MCP with Chrome software WebGPU, launch channel `chrome` with
`--enable-unsafe-webgpu --use-angle=swiftshader`. The default headless launch may expose
`navigator.gpu` while returning no adapter. Chrome 155 functional coverage and the mapped
HZB-buffer regression are recorded in
[the web measurements](../../docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md#chrome-software-webgpu-hzb-buffer-fix--13-10-2026).
Software-adapter checks do not establish hardware performance.

La API `getStats()` y los eventos de estadísticas/progreso incluyen
`memory.wasmLinearMemoryBytes`: capacidad de memoria lineal de la instancia del worker.
`memory.workerJsHeapUsedBytes` es `null` (no medido). El export de benchmark conserva la
última muestra del worker en `memory.latestViewerWorkerSnapshot` y registra por separado
`memory.mainThreadJsHeapUsedBytes`; el campo legado `environment.jsHeapUsedBytes` tiene
el mismo alcance del hilo principal. No son memoria viva ni un pico RSS/PSS/GPU conjunto.

### Final v5 storage contract probe

With fresh bindings, run `tests/storage-format.browser.mjs` as a module Worker and wait for
`type === "probeResult"`, ignoring read-progress messages. It checks Blob opening with/without
the companion and explicit rejection of provisional files, unsupported v6 revisions and an
incorrect companion. It needs no GPU adapter. Recorded native/OPFS/browser evidence and the
remaining presentation limitation are in [storage benchmarks](../../docs/records/benchmarks/STORAGE_BENCH.md).

### V7 asynchronous Blob range probe

With fresh bindings, run `tests/async-format-reader.browser.mjs` as a module Worker and wait for
`type === "probeResult"`. Against the V7.1-pre fixture it checks bounded metadata ranges,
asynchronous `Blob.arrayBuffer()` payload reads, XYZ/RGB and typed U16 materialization, companion
identity and CRC rejection without a full-file read, plus shared read quota, concurrent calls
and scheduler tickets/retry. Equivalent V7.3 fixtures passed with fresh bindings ([evidence](../../docs/records/benchmarks/data/storage-v7-v73-integration-2026-10-25.json)). Visual
LOD publication and navigation measurements remain pending in the [V7 plan](../../docs/explanation/format-v7-plan.md#3-reader-lógico-único-concurrencia-y-transporte-asíncrono).

### Opt-in conversion revisions

Normal conversion stays on final v5. Programmatic callers may select a revision with the converter
option `formatVersion`, for example `converter.convert(file, { options: { formatVersion: "v7.3-pre" } })`.
Opening a Blob automatically selects final v5 or an implemented experimental identity: `v6.1-pre`–`v6.4-pre`
and `v7.1-pre`–`v7.3-pre`. The option accepts these exact strings;
paired files must have the exact same identity. Final `v6`, final `v7` and later revisions remain unsupported.
The demo defaults to v5; this programmatic selection does not add a format selector to its UI.

For the storage probe, pass `expectedVersion: "v7.3-pre"` with the converted Blobs and check the
returned `formatVersion`. Use fresh bindings and reload worker scripts after rebuilding (a cache
busting query on the worker/probe URL avoids a stale development module). Results are recorded
in [storage comparisons](../../docs/records/benchmarks/STORAGE_BENCH.md); GPU presentation remains separate.

Physical contracts and later Blob/OPFS checks: [format preparation](../../docs/records/benchmarks/STORAGE_V6_PREPARATION.md),
[pre3](../../docs/records/benchmarks/STORAGE_V6_PRE3_BENCH.md) and [pre4](../../docs/records/benchmarks/STORAGE_V6_PRE4_BENCH.md).
These storage checks do not certify WebGPU presentation. For the current cross-component
inventory and pending work, use the [project README](../../README.md#prioridades-pendientes).

### Regional Replacement regression

With fresh bindings, run `tests/lod-regions.browser.mjs` as a module worker and wait for
`type === "probeResult"`. It requires WebGPU enabled and a usable adapter. It checks a resident
parent fallback with an independently detailed sibling, resize rebuilding and child replacement.
The [regional LOD report](../../docs/explanation/REGIONAL_VOXEL_LOD.md) records the software adapter, native
comparison and outstanding visual validation; a `presented` status alone is not screenshot proof.
