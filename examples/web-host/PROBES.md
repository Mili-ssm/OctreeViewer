# web-host probes and benchmark procedures

Browser probes and measurement procedures split from the [web-host README](README.md). They are developer tooling, not needed to run the demo.

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
  [--browser=chrome|firefox] [--px=4] [--max-load-s=120] [--orbit-s=10] [--pixels] \
  [--color=rgb|intensity|classification|attribute:COLUMN]
```

`--color` picks the demo's Color control once the dataset is open (default `rgb`, unchanged); `attribute:COLUMN` needs
`--attrs` and a single-component sidecar column. OUTPUT.json `colorMode` records the request and the value read back
from the select and the render worker; a mode that is not active fails the run.

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
is not filled by the asynchronous path, use `workerMetrics.asyncBlocks.blocks`), adapter info,
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

### Cells path parity

`probe-cells-parity.mjs` runs four arms (cells + build worker, cells without it, CPU path, cells + build
worker again) on two fixed poses and compares the settled digests. Per-cell level selection is the only
mode; the report's `cellLod` field is the constant `"cell"` so old and new reports stay comparable. Serve
the repository root on a free port and run it on one file:

```bash
PORT=8791 scripts/serve-demo.sh --no-build &
node examples/web-host/probe-cells-parity.mjs target/render-data/belvedere-v7.oct target/cells-parity.json \
  --url=http://127.0.0.1:8791/examples/web-host/index.html --rest-timeout-s=120
```

The probe needs a V7 file (the Belvedere one above): a V5 file such as `data/samples/fs_e430_n4008_18.oct`
(header `OCTVFRM1 0500`) does not reach the cells path. The probe
takes a file, not the built-in demo sample. Its final `gate`/`parity` summary is written to the report
(a lookup bug, `arm.name` instead of `arm.arm`, kept it out before). Kill the server and browsers when done.

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

The `getStats()` API and the stats/progress events include `memory.wasmLinearMemoryBytes`: the
linear-memory capacity of the worker instance. `memory.workerJsHeapUsedBytes` is `null` (not
measured). The benchmark export keeps the last worker sample in `memory.latestViewerWorkerSnapshot`
and records `memory.mainThreadJsHeapUsedBytes` separately; the legacy field
`environment.jsHeapUsedBytes` has the same main-thread scope. They are neither live memory nor a
joint RSS/PSS/GPU peak.

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
LOD publication and navigation measurements are still pending ([#72](https://github.com/Mili-ssm/OctreeViewer/issues/72)); context in the archived [V7 plan](../../docs/records/archive/format-v7-plan.md#3-reader-lógico-único-concurrencia-y-transporte-asíncrono).

### Opt-in conversion revisions

The converter writes final v7 by default. Callers may select another revision with the converter
option `formatVersion`, for example `converter.convert(file, { options: { formatVersion: "v7.3-pre" } })`.
Opening a Blob automatically selects final v5, final v7 or an implemented experimental identity: `v6.1-pre`–`v6.4-pre`
and `v7.1-pre`–`v7.3-pre`. The option accepts these exact strings;
paired files must have the exact same identity. Final `v6`, v7 revisions after `v7.5-pre` and v8 revisions other than the experimental `v8.1-pre` and `v8.2-pre` remain unsupported; final `v7` is supported and the default.
The demo converter panel defaults to `v7` and lists the other writable revisions in its format selector; for global-frame formats (`v7`, `v7.4-pre`, `v7.5-pre`, `v8.1-pre`, `v8.2-pre`) it disables **Points per block** and **Spatial divisions** (fixed by the format) and does not send them.

For the storage probe, pass `expectedVersion: "v7.3-pre"` with the converted Blobs and check the
returned `formatVersion`. Use fresh bindings and reload worker scripts after rebuilding (a cache
busting query on the worker/probe URL avoids a stale development module). Results are recorded
in [storage comparisons](../../docs/records/benchmarks/STORAGE_BENCH.md); GPU presentation remains separate.

Physical contracts and later Blob/OPFS checks: [format preparation](../../docs/records/benchmarks/STORAGE_V6_PREPARATION.md),
[pre3](../../docs/records/benchmarks/STORAGE_V6_PRE3_BENCH.md) and [pre4](../../docs/records/benchmarks/STORAGE_V6_PRE4_BENCH.md).
These storage checks do not certify WebGPU presentation. For the current cross-component
inventory and pending work, use the [project README](../../docs/estado.md#prioridades-pendientes).

### Regional Replacement regression

With fresh bindings, run `tests/lod-regions.browser.mjs` as a module worker and wait for
`type === "probeResult"`. It requires WebGPU enabled and a usable adapter. It checks a resident
parent fallback with an independently detailed sibling, resize rebuilding and child replacement.
The [regional LOD report](../../docs/explanation/REGIONAL_VOXEL_LOD.md) records the software adapter, native
comparison and outstanding visual validation; a `presented` status alone is not screenshot proof.

### Remote source through the embeddable API

Checks `load({ url })` (HTTP Range, D4) with and without the build worker. The data server must
answer Range and CORS; `red-ref-server.mjs` answers every file as `application/octet-stream`, so it
serves only the dataset and a second static server serves the page, the modules and `pkg/`
(the workers import `../../pkg/viewer_wasm.js`, so the app root is the repository root). Build the bindings
first with `scripts/build-wasm-bindings.sh` when `pkg/` is missing or stale (a fresh worktree has none).

```bash
python3 -m http.server 8791 --bind 127.0.0.1 &                                     # app: repository root
node examples/web-host/red-ref-server.mjs examples/web-host --port=8780 --rtt-ms=0 &  # data with Range
# error case: node examples/web-host/red-ref-server.mjs examples/web-host --port=8781 --ignore-range
```

Open, in a WebGPU-capable browser (headless Chrome with Playwright works; the result is also in `window.probeResult`):

```text
http://127.0.0.1:8791/examples/web-host/probe-api-remote.html?data=http://127.0.0.1:8780/sample.oct&buildWorker=0&attributes=derived
```

Vary `buildWorker=0|1` and `attributes=derived|none`. The page awaits up to 60 s per wait and prints
`{ name, bytes, hasAttributes, reachedTarget, metadata }`, or `{ error }` when `load` rejects.

Expected:

- `name` is `sample.oct`, `bytes` is the file size and `metadata` (node count, max level, semantics,
  point count) is identical with and without the build worker; `bytes` is not `null` in either mode
  (the build worker reports it as `fileBytes`).
- `attributes=derived` gives `hasAttributes: true` (explicit `{ url: sidecarUrl(url) }`); `none` gives `false`.
- `reachedTarget` is `true`.
- Against the `--ignore-range` server (`data=http://127.0.0.1:8781/sample.oct`) the result is an `error`
  about the server ignoring Range, with no hang. A server that never answers takes about 63 s to fail
  (4 attempts of 15 s plus waits), longer than this probe's wait.
