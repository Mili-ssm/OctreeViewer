# web-host demo

## Quick start

Requirements: [rustup](https://rustup.rs) (it installs the toolchain pinned in `rust-toolchain.toml`
and its `wasm32-unknown-unknown` target; otherwise run `rustup target add wasm32-unknown-unknown`),
the `wasm-bindgen` CLI at the version in `Cargo.lock` (`cargo install wasm-bindgen-cli --version 0.2.129`),
Python 3, and desktop Chrome or Edge with WebGPU.

```bash
scripts/serve-demo.sh        # builds the wasm bindings, then serves the repository root
```

Open <http://127.0.0.1:8000/examples/web-host/> (`PORT=9000 scripts/serve-demo.sh` changes the port;
`--no-build` skips the build). The page opens the bundled sample (589,824 points) on load; the
**Open built-in sample** button reopens it. **Open real cloud · Belvedere Glacier** loads a real
photogrammetric RGB cloud (a 600 × 600 m crop, 1.32 M points, 21 MB; Ioli et al.,
[CC BY 4.0](https://doi.org/10.5281/zenodo.19265915)). It is built outside git by
`scripts/make-demo-belvedere.sh` into `target/demo-data/`, which `scripts/publish-demo.sh` copies to
the site's `datasets/`; the button stays hidden where neither location answers. To try your own cloud, use **Convert a local LAS/LAZ file**
in the side panel, press **Convert to .oct**, then open the result. Converted files default to the
`v8` format.

The built-in sample button opens `sample.oct`, a final `V8` file drawn by the cells path. The demo
sample always uses the latest stable format ([ADR 0047](../../docs/adr/0047-v5-heredado-hasta-v8.md)).
`sample-small.oct` (`V8`, 4,096 points) backs the `*.browser.mjs` tests.

`?url=FILE.oct` opens a file over HTTP and needs a server with **HTTP Range and CORS**; python's
`http.server` does not support Range. Locally use
`node examples/web-host/red-ref-server.mjs DIR --port=8780` (it also adds CORS; flags `--rtt-ms`,
`--mbps`, `--ignore-range`). The v7 format opens remotely too. Details in
[Open a file over HTTP Range](#open-a-file-over-http-range). Probe procedures: [PROBES.md](PROBES.md).

This folder demonstrates the embeddable contract for the viewer. The host owns the `canvas` and
controls a dedicated module worker. The worker owns the WASM viewer, `OffscreenCanvas`, format reader,
and WebGPU renderer. JavaScript does not copy the render instances back for drawing.

## Build the WASM package

```bash
rustup target add wasm32-unknown-unknown
scripts/build-wasm-bindings.sh   # cargo build --release --target wasm32-unknown-unknown + wasm-bindgen into pkg/
```

The repository includes `sample.oct` and `sample.octattrs`: a deterministic synthetic wave surface
over 64×64 m on a 768×768 grid (≈ 8.3 cm spacing, 589,824 points, about 3.0 MiB) with fine relief.
The example writes it as LAS and converts it with the converter defaults (`V8`). Regenerate it from the workspace root with:

```bash
cargo run --offline --release -p viewer_convert --example create_web_sample
```

The browser tests use `sample-small.oct` / `sample-small.octattrs`: the same surface on a 64×64 grid
(1 m spacing, 4,096 points, one block, about 22 KiB, paged `V8`). Regenerate it with:

```bash
cargo run --offline -p viewer_convert --example create_web_sample -- examples/web-host/sample-small.oct 64
```

## Serve the demo

The development demo uses external modules: **do not open this `index.html` by double click**.
`file://` gives them a local origin that causes CORS errors; startup detects that case and links
to the self-contained viewer. To use the development demo, from the repository root run
`scripts/serve-demo.sh` (builds the bindings, then serves on `http://127.0.0.1:8000/examples/web-host/`),
or by hand:

```bash
python3 -m http.server 8000 --bind 127.0.0.1
```

and open **http://localhost:8000/examples/web-host/index.html**. Serve the whole repository root,
including `pkg/`: serving only `examples/web-host/` leaves out the bindings and WASM. WebGPU and
workers need a secure context such as localhost or HTTPS; plain HTTP from another machine may not work.

### Loading contract and CORS regression prevention

- **Demo and modular API:** the page, workers, JS/MJS modules and WASM must be served from the same
  origin (scheme, host and port) with their relative paths intact. Modules need a JavaScript MIME
  type and the WASM `application/wasm`. An HTML/404 response in place of a module also breaks
  startup. Do not mix `localhost` and `127.0.0.1` in resource URLs.
- **Double click:** open the generated `artifacts/visor-web-local/index.html` (below). Its script,
  bindings, WASM and sample are embedded; both workers are classic Blob workers with no external
  imports. Do not introduce external modules, `import.meta.url` or asset fetches on this path.
  The modular API in `embed/` keeps its localhost/HTTPS requirement.
- **After changing the host, workers or bindings:** regenerate the package; the HTML/ZIP are a
  snapshot and do not update by themselves. `package-local.py` must find its markers and fails if a
  refactor breaks the runtime transformation.
- Do not use `fetch(..., { mode: "no-cors" })` or disable browser security: neither enables
  `file://` modules. The single-origin demo needs no permissive CORS headers.
  [Reference: JavaScript modules and local testing (MDN)](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Guide/Modules).

Automated regressions (from the root, Python 3 and Node.js):

```bash
node examples/web-host/tests/demo-bootstrap.test.mjs
python3 -m unittest discover -s examples/web-host/tests -p 'test_*.py'
```

They cover the preventive block of external loads under `file://`, the modular HTTP/HTTPS startup,
and the generation/syntax of the runtime and both self-contained workers. They do not replace a
WebGPU rendering test in Chrome/Firefox: open the generated package by double click, load the
sample and check the console; also check startup from localhost.

## Open a file over HTTP Range

`index.html?url=FILE.oct[&attrs=FILE.octattrs|none][&px=4]` opens a file from a server that answers
`Range` (single range, `206`) and sends CORS headers (`Access-Control-Allow-Headers: Range, If-Range`,
`Access-Control-Expose-Headers: Content-Range, ETag`). The workers read it with
`http-range-source.mjs`: one suffix request opens a `v7.5-pre` file, later reads are single ranges
pinned by `If-Range`, and a server that ignores `Range` (answers `200`) is an error.

Failures are handled per request: network errors, `429` and `5xx` are retried three times with
exponential backoff and jitter (about 200 ms, 800 ms and 2.4 s; `Retry-After` is honoured up to 10 s),
and every attempt has a 15 s timeout (`timeoutMs`) that counts as a retryable failure. Other `4xx`,
a changed file (ETag or `200` for a Range request) and aborts are never retried. The counters
`requests`, `retries`, `timeouts`, `aborted` and `abortedBytes` appear as `remote` in the worker
metrics (`workerMetrics.remote`, and `remote` in the build worker's metrics).

A ticket the scheduler drops is cancelled for real: each LOD read request gets a view of the source
(`HttpRangeSource.withSignal`, installed with `WasmAsyncLodReadRequest.setSource`) tied to an
`AbortController`. Aborting rejects that ticket's reads with an `AbortError` (a cancellation: not
reported, not retried, not a failed block) and cancels an HTTP request only when every read merged into
it is aborted, so a block still wanted by another ticket keeps its download.

Two tuning options travel with the remote source descriptor (`{ url, maxGap?, remoteReads? }`) and
the demo URL: `&maxGap=BYTES` merges reads of the same tick separated by up to that many bytes into
one request (default `0`: only touching ranges; the over-read is counted as `gapBytes` in `remote`),
and `&remoteReads=N` sets the concurrent LOD reads for that source: up to N block reads plus up to N
hierarchy page reads, each counted apart (default `8`; values below 1 or not a finite number use the
default, values above 64 are capped; local `Blob`s always use `4`). D2 measurements on RED-REF
(Majadas and Belvedere v7.5, CPU path): 8 reads cut the median time to reached by 15-30 % against 4
with no frame regression, 16 gives no further gain (and more bytes on Belvedere), and gap merging
(16-256 KiB) saved at most 3 of 31 requests for up to 6 % more bytes without changing the time, so
`maxGap` stays `0` (record `docs/records/benchmarks/D2_GAP_CONCURRENCY_14-11-2026.md`).
`probe-orbit-load.mjs --remote` takes `--max-gap=` and `--remote-reads=`.

To measure with the RED-REF network (100 Mbit/s, 50 ms RTT), serve the repository root (for example
`python3 -m http.server 8767`) and run `node examples/web-host/probe-remote-open.mjs DATA_DIR NAME.oct OUT.json`;
`red-ref-server.mjs` can also be started alone (`--ignore-range` simulates a server without Range).

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
You can also open `artifacts/visor-web-local/index.html` directly by double click; it is not the
development `index.html` of this folder.

### Convert and load a cloud

1. Under **Convert a local LAS/LAZ file**, select the `.las` or `.laz` and press **Convert to .oct**.
2. Conversion runs in a separate worker with the same pipeline as the command-line converter
   (`viewer_convert_core`); cancelling terminates the worker and deletes the temporaries. The file
   stays in the browser's local storage and can be downloaded or opened in the viewer. The available
   quota can interrupt a conversion; incomplete temporaries are removed when the browser lets the
   handle close. If you close the tab abruptly, a temporary may remain in site storage.
3. This web path keeps XYZ, intensity, classification, returns, angle, user data, point source ID,
   GPS time, RGB and NIR when present. It does not keep Extra Bytes or CRS.
4. To open an already converted `.oct`, wait until the status says the renderer is ready and use
   **Open project dataset**. The viewer shows a coarse view first and then refines progressively.
   Adjust **Finest voxel detail** to choose the minimum detail you want to try.

The cloud and the converted file are processed locally in the browser; nothing is uploaded.
Conversion needs WebAssembly, workers and OPFS in a secure context. Evidence (30-10-2026,
[M3b](../../docs/records/benchmarks/M3B_BELVEDERE_2026-10-30.md)): Belvedere converted with a SHA equal
to the CLI in Chrome 156.0.8078.73 and Firefox 157.0 (headless, one machine) in 77.8-107.5 s
(2.1-2.9 x native), with peak PSS of 3.4-4.5 GiB and 2 GiB spools. Albuera and larger sizes are not
validated. Support policy ([requirements](../../docs/requirements.md), CON-01): desktop Chrome/Edge
are the target; Firefox, best effort; Safari and mobile, out of scope. Planned (RF-28, not
implemented): the UI will announce "validated up to Majadas (~6 GB of LAZ)"; larger inputs are
converted with the CLI.

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
`WasmVoxelViewer.createOffscreen`. Select a project-format file to open it with
`WasmAsyncFormatReader` (asynchronous `Blob` range reads) and call `updateLod`; the worker publishes a
coarse visible cut first, then each `frame` command schedules reads for the pending blocks, at most 4
in flight (`WasmBlockRequestScheduler`) and within a shared byte budget (`WasmAsyncReadBudget`), and
refines through projected voxel stages until the requested detail is reached. Pending blocks are
prioritized against the current camera, and a camera change restarts selection from the coarse
frontier. Reads do not block the worker: camera commands run while reads are pending, and only
the insertion of each decoded block is queued with the other commands. Reads whose block is no longer
wanted are cancelled. With the build worker (default) the reads and scene building move to `build-worker.js`
(below). `openFormatDataset` in the wasm API is a synchronous reader that this worker does not use.
The host reports time to the first coarse frame and target LOD frame, plus p50/p95/p99 worker
`renderFrame()` times over a bounded window of up to 4,096 frames. Real-browser latency and frame-time
validation remain pending in the [project roadmap](../../docs/estado.md#prioridades-pendientes).
The camera fits dataset bounds and supports observer and bird navigation. Observer mode uses a voxel
click to set the orbit center, left drag to pan it, right drag to orbit, and the wheel to change radius. Bird mode uses
left drag to pan, right drag to orbit, and the wheel to change movement speed. In both modes, holding
WASD flies forward, back and sideways relative to the view (in the horizontal plane), Q/E move down and
up, and Shift is four times faster; the speed is 0.3 orbit radii per second times the movement speed,
so it suits any cloud size. The
camera uses Z as the vertical axis. It requires a browser/GPU with WebGPU and OffscreenCanvas support.
The host recreates a transferred canvas by replacing it and starting a new worker.

### Embedding from another page

Import `createViewer` from `viewer-api.mjs` and pass it a canvas owned by the host page:

```js
import { createViewer } from "./viewer-api.mjs";
import { sidecarUrl } from "./http-range-source.mjs";

const viewer = createViewer(document.querySelector("canvas"), { mode: "observer" });
await viewer.ready;
await viewer.load(file, { voxelPixels: 1, attributes: optionalAttributesFile });
console.log(await viewer.getMetadata());
console.log(await viewer.getCamera());

// A remote dataset over HTTP Range (same server requirements as `?url=` above). The sidecar is never
// derived: pass it explicitly, or leave it out to open without attributes.
const url = new URL("data/cloud.oct", location.href);
await viewer.load({ url, remoteReads: 8 }, { attributes: { url: sidecarUrl(url) } });
viewer.on("stats", (stats) => console.log(stats.lod?.renderedVoxels));
viewer.on("pick", (hit) => console.log(hit?.position, hit?.voxelCenter));
```

`load(source, options)` takes a `File`/`Blob` or a remote descriptor `{ url, maxGap?, remoteReads? }`
(see "Open a file over HTTP Range"; any other field is a `TypeError`). `url` is a string or `URL`, resolved
against the page and limited to `http:`/`https:` without embedded credentials; `attributes` is a `Blob`, a
descriptor, or absent/`null` (no sidecar). `sidecarUrl(url)` maps `a.oct?v=1` to `a.octattrs?v=1` (it drops a fragment and refuses
a path ending in `/` or `.octattrs`). Network and Range errors reject `load`; a server that never answers
takes about 63 s to fail (4 attempts of 15 s plus backoff), and `load` cannot be aborted yet.
The API also provides `setMode("bird" | "observer")`, `setCamera(position, target)`, `fit()`,
`pick(x, y)`, `getStats()`, `setMinLength(side)`, `setContinuity(enabled, factor)`, `on(type, callback)`, and `dispose()`.
`pick(x, y)` uses CSS pixels relative to the canvas and resolves to `null` or `{ position, voxelCenter, voxelSize, distance,
instanceIndex, sceneGeneration }`; observer clicks perform the same pick and move the orbit center.
An instance index is valid only for its returned LOD scene generation. The GPU readback copies at
most 65,536 candidate instances per pick and returns an error if that safety cap is exceeded.
Camera positions/targets returned as arrays by the API
are in source dataset coordinates, with the file's coordinate origin and visual scale applied.
`getCamera()` also returns the active mode, fixed Z-up vector, and bird movement speed.
The cells path always selects the drawn level per cell; there is no mode to choose. `?cellLod=` in the page URL
was removed: any value is ignored with a console warning.
The active-depth rule (which range depths a node activates) is always the contiguous resident run; the
`setCellActiveRule` API and the "Active depths" select were removed (OQ-11, issue #66), and `?cellActive=` in the
page URL is ignored with a console warning.
`setMinLength(side)` sets the length minimum of the drawn cubes in dataset units: `undefined` restores the
default 1e-4 (0.1 mm if the unit is metres), `0` turns the length filter off, and a negative or non-finite side
rejects. `setContinuity(enabled, factor)` turns on the cloud-spacing minimum (`factor` × the reference spacing,
default 1). The larger of both wins per cell (ADR 0049, 0054). The demo panel "Minimum cube size" offers
`Length` (value in mm, cm or m), `Cloud spacing` (factor, with the resulting side from `cellStats().continuityMinSide`)
and `Off · pixels only`. The open result carries `crsUnit` (`{ unit, metresPerUnit, geographic, assumed }`,
read from the top-level unit of the stored CRS, ADR 0049; the non-paged descriptor carries it too): the demo
converts the length into dataset units with `metresPerUnit`, disables `Length` for a geographic CRS (degrees), and
says when metres are assumed because the CRS declares no unit (`unrecognised`: it declares one this reader cannot
interpret). The stats report `minLength`.
The cells path never draws cubes smaller than a default length minimum of `1e-4` dataset units (0.1 mm when
the unit is metres); continuity (cloud spacing x factor) wins when larger.
The demo's default length (ADR 0059) is the file's p5 when it carries one: the open result has
`recommendedMinSize` (p5 of the nearest-neighbour distance in dataset units; `null` for a v7 file, a file
whose p5 was not measured, or the CPU path). The panel then shows it converted with `crsUnit` in mm, cm or m
(3 significant digits) and sends the same value; with `null`, or a geographic CRS, it shows 0.1 mm.
Both open paths (build worker and in-worker, `?buildWorker=0`) carry `recommendedMinSize`; only the cells path
knows it. Each opened file starts from its own default: a value the user typed for the previous file is
replaced (the selected mode is kept); changing the control afterwards works as before. "Recreate viewer"
reopens the file, so it also ends at that file's default (p5), not at 0.1 mm or at the typed value.
- In `Cloud spacing` mode the demo still sends the length with the spacing factor, and that length is the
  file's default (p5 when present): the larger side wins per cell (ADR 0054), so the effective minimum can
  be the p5 length rather than the spacing side.
- The shown value is rounded to 3 significant digits and the applied side is that rounded value, so it can
  differ from p5 by up to about 0.5 %.
The API's own default (above) does not change.
`renderedVoxels` reports submitted/current HZB-visible instances; it does not guarantee each voxel
contributes a final pixel. Metadata reports bounds and coordinate origin; a CRS is not inferred when
the file does not provide one. The canvas is transferred to a worker and cannot be reused after
`dispose()`; create a replacement canvas before creating another viewer in its place.
Fatal frame errors, worker errors and message decoding failures reject outstanding API requests and
terminate the worker immediately so the browser can release its dataset and GPU resources. To retry,
replace the transferred canvas and create a new viewer.

GPU device loss is recovered automatically (R3-Q26, R3-Q34), except after an explicit `destroy()`
(reason `destroyed`). The worker emits `deviceLost` (`{ reason, message, canvasReplaced }`), then
`deviceRecovery` (`{ state: "started" | "recovered", attempt }`, up to 3 attempts with backoff). It
recreates the renderer on a new adapter and device, keeps the camera and settings, and reattaches
the open dataset without reading the file again. After the last failed attempt one `error` event with
`operation: "deviceLost"` follows. `getStats().gpu.deviceLost` reports the loss (`null` when healthy).
After a GPU-process crash the old canvas stays blank, so the API **replaces the canvas element** with a
clone (same id, classes, attributes, inline style and size) and dispatches `voxelviewer:canvasReplaced`
on the new element (`detail.canvas`). A host holding the old element must switch to `event.detail.canvas`;
listeners the host added with `addEventListener` are not cloned and must be re-bound in that event.
Focus moves to the clone if the old element had it. A canvas outside the document is not replaced
(`canvasReplaced: false`): recovery then runs on the old one and may stay blank after a GPU crash.
Verified with `probe-device-loss.mjs` (Chrome `Browser.crashGpuProcess`).
The small direct-WASM pick/recenter path passes in Chrome and Firefox via the presentation
diagnostic above. Browser pick latency and CSS/API interaction still need validation.
Chrome 155 is now available and the small software-WebGPU flows pass, but pick remains
unverified. Firefox 157.0.1 headless can present with WebGPU enabled
in a temporary profile outside the command sandbox; the first small-file benchmark is recorded
in [web performance](../../docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md). This does not validate pick.

**`buildWorker` (render plan R2).** On by default (`createViewer` passes `buildWorker: { url, type }` in
the worker `init` message unless called with `buildWorker: false`; `buildWorkerUrl` overrides the URL).
The demo opts out with `?buildWorker=0` (`?buildWorker=1` is accepted and changes nothing). Scenes are built in `build-worker.js`
instead of the render worker, which keeps frame replies short during loading and orbit (Majadas and
Belvedere: p95 <= 17.6 ms, time to target -57 to -62 %, see the
[measurement](../../docs/records/benchmarks/RENDER_BUILD_WORKER_2026-10-30.md)). Default
since `feat/web-converter-stage4`
([ADR 0018](../../docs/adr/0018-worker-de-construccion-por-defecto.md)). If nested workers are
unavailable (a Blob worker inside a Blob worker on `file://` in Chrome), the option disables itself,
the worker posts `buildWorkerUnavailable` and scenes are built in the render worker. Color modes by
attribute are not covered in this mode yet.

**`cells` (render plan R3, default).** The dataset is drawn as GPU cells instead of CPU-built voxel
scenes (R3-Q35). To use the CPU path instead, pass `?cells=0` in the demo or `createViewer({ cells: false })`.
Both workers receive the flag (`init.cells` for the render worker, `open.cells` for the build worker). With a build worker the
builder sends `nodeCells` (transferred, ordered, never coalesced), `cellCut` and `cellLod`, and the
render worker answers with `poolAck`, `evict`, `poolReset` and `driverReplaced`; the protocol is
documented at the top of `build-worker.js`. Cells need the build worker (R3-Q24): without one (`buildWorker=0`, or nested workers unavailable,
e.g. on `file://`), the viewer uses the CPU path and the LOD stats report `cellPath: "needsBuildWorker"`.
Limits:
- Needs a format with a global frame and replacement semantics, and a device that supports cells;
  otherwise the dataset opens on the CPU path (`cellPath: "CellPathUnsupported"`).
- Frames keep running while `cellsBusy` is true, for at most 600 frames in a row without progress
  (cut applied, queue, upload): then the render worker reports `cellsBusy: false` and `cellsStuck: true`
  (`cellsStuckFrames` counts them) so a driver that never settles cannot keep the host rendering.
  Waiting for missing nodes or a camera move does not count as progress. `cellsBusy: false` is not
  a "settled" signal: wait for a non-null `cutDigestAtRest`, which the cells path only publishes when
  the cut for the current view is applied, nothing is queued or missing and the digest repeats.
  `cutDigestAtRest` (LOD stats, worker metrics and `window.__renderBench.cutDigestAtRest()`) is the
  digest of the applied cut once everything is at rest.
- A scene received in cells mode is a bug: it is not applied, and it is counted in
  `cellCounters.strayScenes` of the worker metrics.
- Not measured in a browser yet.

The converter is separately embeddable from `converter-api.mjs`. It accepts a local `.las` or `.laz`
`File`, runs the core converter (the same pipeline as the CLI) in another worker, and resolves to
`{ file, name, attributesFile, outputAttributesName, points, timings, spool? }`:

```js
import { createConverter } from "./converter-api.mjs";

const converter = createConverter({ converterOptions: { maxMemoryMiB: 256 } });
await converter.ready; // orphan `conversion-*.partial` files from earlier sessions are removed
converter.on("progress", ({ processedPoints, totalPoints }) => console.log(processedPoints, totalPoints));
converter.on("warning", ({ message }) => console.warn(message));
const result = await converter.convert(lasFile, { options: { maxError: 0.05 } });
console.log(result.name, result.points, result.file, result.timings);
converter.dispose();
```

| Item | Behaviour |
| --- | --- |
| Options | `converterOptions` (creation) or `convert(file, { options })` (per call): `maxError`, `maxMemoryMiB`, `spatialWorkers`, `ramBudget` (bytes, default 4 GB, capped at 8 GB; only used without OPFS). V8 is the only format and fixes the format, row order, points per block (64000) and spatial divisions (2): `formatVersion`, `rowOrder`, `pointsPerBlock` and `spatialDivisions` are refused with `option NAME was removed: V8 is the only format and fixes it` |
| `ready` | Promise that settles once stale partial files are deleted; `convert()` waits for it on its own, so awaiting it is optional |
| Events | `on("progress" \| "status" \| "warning" \| "complete" \| "cancelled" \| "error", callback)` returns an unsubscribe function |
| `warning` | Emitted when OPFS is unavailable (Chrome `file://`): the conversion runs RAM-only and is limited by `ramBudget`; serve the page over `http://localhost` for large files |
| Quota | Firefox without persistent storage stops at about 10 GiB, so a conversion that cannot fit is rejected up front; the worker asks for `persist()` first. Other browsers are not pre-checked (Chrome allows more than it reports); a `QuotaExceededError` becomes a "Not enough browser storage" error |
| `cancel()` | Terminates the worker, deletes its partial files and rejects with an `AbortError`; the next `convert()` respawns the worker |
| `result.spool` | Spool statistics (spilled bytes, peak pages) when the engine reports them |
| `engine` | **Deprecated.** `"core"` is accepted and does nothing; omitting it uses the core converter. `"legacy"` fails with ``the legacy converter was removed; omit `engine` or use "core"`` |

The host decides whether to download or open `result.file`. Extra Bytes and CRS are omitted.
`timings.totalMs` is a browser wall-clock measurement. Large-file behaviour and quotas per browser are
recorded in the [stage 4 plan](../../docs/records/archive/conversor-web-paralelo-etapa4.md).

Use the keyboard-accessible **Open built-in sample** button to load the included `v7` file, or focus
the labeled file picker to choose another local dataset. The sample is a synthetic wave surface
(see [Build the WASM package](#build-the-wasm-package)) and does not depend on external or licensed
point-cloud data.

The demo exposes RGB, intensity, elevation, and classification color modes (elevation spans the
data's Z extent and intensity the union of the levels loaded so far, so colours do not change with the
view), plus one slider for the
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
execution or display latency. Its `render path` line names the active path; on the cells path the
point, voxel, block, coverage and instance-byte lines are replaced by the applied cut, the resident
pool pages, the bytes uploaded to the pool in the last frame and the nodes still waiting for a level. File reads report unique
bytes fetched in 256 KiB ranges. This is not a completion percentage: the reader seeks through
metadata and selected blocks, so it may never read the full file. Cancel terminates the
worker, replaces its transferred canvas, and starts a fresh viewer; select the file again to retry.
The host keeps at most one worker command of each repeatable kind in flight and replaces its pending
value with the latest input. It also allows only one frame request in flight.

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
Browser interaction, real GPU timing and cancellation during a local file read still need
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

The default `sample.*` carries typed intensity (`u16`, 0–64,428) and classification (`u8`, 0–3).

Majadas RGB pairing completed three Firefox runs with the same final geometry counters and a
visibly colored capture. `tests/lod-attributes.browser.mjs` checks the typed `sample-small` fixture by default;
create it as a module Worker and post one message. Optionally supply coordinate/attribute Blobs,
`modes` and an `incompatibleAttributes` Blob. It validates complete refinement, matching geometry
counts, invalid-column rejection and retention after failed sidecar attachment. It requires
rebuilt WASM/WebGPU/OffscreenCanvas and is separate from the Node suite. Majadas passed intensity
and RGB-component selection plus rejection of Albuera's sidecar. Its sequential API loop timings
are integration diagnostics, not host load/FPS benchmarks. Repeated host column-change loads and
other browsers remain pending. See the
[measured matrix](../../docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md).

With the legacy 4,096-point sample, Firefox 157.0.1 headless also passed opening the local package directly through
`file://` with the HTTP server stopped and a fresh WebGPU-enabled profile. The embedded sample
and sidecar loaded; keyboard type-ahead selected Classification, complete 4,096-voxel refinement
and a 10,000 ms benchmark export succeeded. This validates the standalone load path and that
selector interaction, not OPFS/conversion, all keyboard controls or Chrome. Evidence:
`target/offline-package-smoke/classification.json` and `.png`.

The `lodStats().cacheActivity` diagnostic and the export's `scene.cacheActivity` keep counters of
insertions, evictions and rejections over the dataset's lifetime, even after payloads are emptied
when attributes change. They are decimal strings; reopening resets the cache.
`refinementWork.evictedPendingBlocks` counts evictions of blocks that the target plan requests again
immediately and resets with the other work counters. There can be pressure without rejections: the
visible parent is protected and still-incomplete children may evict each other. See the
[contract and test](../../docs/records/benchmarks/WEB_PERFORMANCE_BENCH.md#cache-pressure-diagnostics--12-10-2026).

Probe and benchmark procedures (Firefox cache/camera, orbit-load, memory, v7 Blob range, conversion revisions) live in [PROBES.md](PROBES.md).
