# Web viewer performance benchmark

Registro cronológico de pruebas del navegador. Los seguimientos «pendientes» dentro de resultados
fechados describen el siguiente paso de aquella ronda; la lista activa y priorizada está en el
[README del proyecto](../README.md).

This protocol measures the browser path (`viewer_wasm` + worker + WebGPU), separately from native
Rust render timings. The host's **Run 15 s FPS benchmark** and **Export run JSON** buttons record a
selected `.oct` file, viewport, voxel target, load milestones, scene/cache size, browser details, and
a steady-state frame sample.

## Metrics and interpretation

| Field | Meaning |
| --- | --- |
| `dataset.fileBytes`, `sourcePointCount`, `nodeCount` | Dataset scale as read from the selected file and its index. |
| `load.metadataOpenMs` | File selection to parsed index acknowledgement in the host; includes worker messaging and metadata access. |
| `load.firstCoarseFrameMs` | Worker open command to first coarse frame reply. |
| `load.targetLodMs` | Worker open command to complete coverage at the requested projected voxel edge. |
| `steadyState.meanFps` | Frame replies per second after the user starts the 15 s benchmark and a 5 s warm-up, over the next 10 s. This is worker frame submission cadence, not GPU completion or display scanout. |
| `steadyState.measurementWindowMs`, `frameReplySpanMs` | Elapsed observation window after warm-up (at most 10 s), and the span between its first/last sampled reply. FPS uses the latter; validity uses the former. Replies need not land on exact window boundaries. |
| `load.refinementWork` | Cumulative synchronous LOD work, also exposed by `lodStats().refinementWork`; see the phase definitions below. Older exports omit it. |
| `steadyState.frameReplyIntervalP50Ms/P95Ms` | Browser-observed spacing between returned frame messages; helps reveal stalls. |
| `steadyState.workerRenderCallP50Ms/P95Ms/P99Ms` | WASM/CPU `renderFrame()` percentiles during the stable sample; they exclude asynchronous GPU completion. `workerRenderCallDuringLoadMs` retains the worker's rolling percentiles from the whole load/run. |
| `scene.renderedVoxels`, cache and `environment.jsHeapUsedBytes` | Scene work and observable memory context. JS heap and GPU memory are incomplete proxies for total process/GPU memory. |
| `scene.voxelBuildBudgetBytes`, `voxelBuildEstimatedPeakBytes`, `voxelInstanceCapacityBytes`, `projectionCachePeakEntries` | Independent CPU construction budget and conservative model, final instance-vector capacity, and retained projection-cache peak. Byte fields are decimal strings. The model reserves cache and map/vector overlap; it does not measure WASM heap or process RSS. |
| `configuration.viewportFramebuffer`, `devicePixelRatio`, `effectivePixelRatio` | Actual bounded drawing-buffer size plus requested and effective display scale. The host caps it at 4,194,304 pixels and 4,096 pixels per side. |

The host renders continuously during progressive loading, then stops submitting
frames while the scene is idle. Input or renderer commands wake it again. A
15-second benchmark window runs only when requested with **Run 15 s FPS benchmark**.
This avoids spending GPU time repainting a static view or running an unrequested
benchmark. The framebuffer cap also bounds the HZB pyramid and depth target on
high-DPI displays; swapchain allocation remains browser/driver-owned and is not
included in the renderer's instance-buffer budget.

Runs with `cameraChanged`, `viewportChanged` or `settingsChanged` set to `true`, incomplete coverage,
or fewer than 10 s of stable frame replies are exploratory and should not be compared as steady state.
Keep GPU adapter, OS, browser version, power mode, color mode, depth shading, cache budget, and camera
fit fixed. The file cache of the OS is not reset by this page; record whether the browser was restarted
and treat repeated same-process reads as warm-cache runs.

## Refinement phase accounting

`lodStats().refinementWork` is a cumulative snapshot since dataset creation or the last
successful attribute/cache reset. The host exports the latest snapshot as
`load.refinementWork`; repeating only the FPS window does not reset it. Camera/LOD changes
also accumulate work, so reopen the dataset for an independent load comparison.

| Field | Measured work |
| --- | --- |
| `selectionMs` | Hierarchy LOD selection, including projected-error callbacks. |
| `planningMs` | Both transition-plan preparations and pending-block priority lookup. |
| `blockLoadMs` | Synchronous loader callback: file-range access, decompression, geometry/color/selected-attribute decoding. Includes read wait; excludes cache insertion and metadata opening. |
| `rangeReadMs`, `rangeReadCalls`, `rangeReadBytes` | Browser Blob-cache fills inside loader callbacks, shared across coordinates and attributes: slice, synchronous ArrayBuffer read, cache allocation/copy. Attempts include failed reads; decimal-string bytes count successful fills, including repeats. Metadata opening and attribute-range scans outside the callback are excluded. |
| `decodeAndOtherCpuMs` | `max(0, blockLoadMs - rangeReadMs)`: residual decoding, CRC, conversion, allocation, cache-hit copying, seeking and orchestration; not isolated decompression time. |
| `voxelBuildMs` | Building the whole visible voxel cut, including projection and cell aggregation. |
| `uploadMs` | `upload_voxel_cut` CPU preparation and enqueue; excludes GPU completion and the subsequent frame render. |
| `updateCalls`, `blockLoadCalls`, `loadedBlocks` | Update attempts, loader invocations, and blocks successfully inserted into the decoded cache. A loader may return no block or fail. |
| `sceneBuilds`, `voxelSourcePoints` | Whole-cut build attempts and sum of their input cut source-point counts, including repeated cuts. The point sum is a decimal string; it is neither unique points nor final rendered voxels. |

`rangeReadMs` and its residual subdivide `blockLoadMs`; do not add them again to the phase total.
The other intervals do not overlap and are elapsed synchronous wall time, not CPU-cycle accounting.
They omit cache insertion, plan commit/cloning, general orchestration, frame rendering and
messaging; their sum need not equal `targetLodMs`. Native uses `Instant`; browser uses
`performance.now()` (with `Date.now()` fallback if the global Performance object is absent).
Browser timer precision can make short intervals zero. The read residual is clamped at zero
when independent interval rounding would otherwise make it negative; cache-fill bytes do not
represent physical disk I/O or unique file coverage. Each completed phase records elapsed
time even if it returns an error; the JS stats API retains its last successful view snapshot
until a later successful update exposes accumulated work. Scene-build counters include failed
build attempts. Existing selection, budgets, cache behavior and render output are unchanged.

### Instrumentation smoke run — 12-10-2026

Release WASM rebuilt offline with the instrumentation, Firefox 157.0.1 headless and a fresh
profile, WebGPU enabled, HTTP memory/disk cache disabled, 1280×720 framebuffer, 4 px target,
fit camera, gray XYZ, depth shading and 256 MiB cache. OS cache was not flushed. No conversion
or compilation ran concurrently with the browser probe; GPU identity remains undisclosed.

Three valid exports using the **synthetic `examples/web-host/sample.oct`**, 4,096 source points,
41,247 B, one node, consistently report one loaded block, three update/build calls and
12,288 accumulated cut source points. All retain 4,096 rendered voxels and complete coverage.
This validates accounting across the 16/8/4 px stages, not an optimization or an Outer Banks
baseline. Median [min–max] phase times: selection 0 [0–0] ms, planning 0 [0–0] ms,
read/decode 1 [0–2] ms, voxel construction 75 [74–108] ms, upload CPU 1 [0–4] ms.
Zeros reflect timer precision. Existing 23 `viewer_wasm` tests, 25 host Node tests, four
host Python tests, native/WASM Clippy with warnings denied and formatting checks passed;
local packaged HTML/ZIP regenerated. Browser exports were checked for phase-counter
invariants, finite nonnegative timings, target coverage and the full 10 s stable window.

One subsequent **Majadas** load under the same settings completed target LOD in **162,900 ms**
(first coarse 69 ms). It is a single exploratory load, not three repeated measurements or a
before/after speedup. Complete 4 px coverage, zero pending blocks, 202 visible blocks,
7,015,449 selected source points, 6,397 rendered voxels, 102,958,469 unique bytes read and
112,290,672 decoded-cache bytes match the previous large-run scene counters.

| Majadas accumulated phase | Elapsed ms |
| --- | ---: |
| Selection | 149 |
| Transition planning and priority | 593 |
| Read/decode callback | 1,111 |
| Whole-cut voxel construction | 157,422 |
| Upload CPU | 115 |
| Measured phase sum | 159,390 |

239 update calls loaded 239 blocks and attempted 239 scene builds, with **522,040,584**
accumulated cut source points (~74.4× the final cut source count). Voxel construction accounts
for **98.8% of the measured phase sum**. This identifies the constructor as the dominant measured
phase in this run; it does not prove every repeated source visit is avoidable, since progressive
stages and visible cuts differ. Next compare rebuilds at unchanged cuts and profile cell/projection
work before introducing incremental caches; preserve integer-cell non-overlap and budget limits.
Read versus decode, the omitted orchestration costs, browser RSS and GPU completion remain open.

Raw exports, adapter/session details and first screenshots are under ignored
`target/web-browser-probe/refinement-work-runs/` (`sample-{1,2,3}.json`, `majadas-1.json`).
`dataset.name=sample.oct` records the synthetic input used for the three small runs.

### Majadas after terminal-cell reuse — 12-10-2026

The current release WASM produces **3/3 valid Majadas runs** at 1280×720, 4 px, gray XYZ in
Firefox 157.0.1 headless: target LOD median **59,696 [58,370–65,937] ms**, first coarse
**46 [45–76] ms**, accumulated voxel construction **53,422 [52,723–58,964] ms**, ~60 reply FPS.
All retain the previous scene/work counters; the first screenshot is byte-identical to the
previous Majadas screenshot. The old sessions are not paired with these browser runs, so
no browser speedup ratio is inferred. [Native paired benchmark, method and limits](VOXEL_BUILD_BENCH.md)
separate the cell-reuse effect from progressive web timing. The earlier BiDi failure remains
unexplained; it did not recur in these three attempts. Other large/browser/color cells stay open.

### Unchanged-cut instrumentation — 12-10-2026

`refinementWork.unchangedCutBuilds` counts build attempts with the same visible node/block IDs.
`reusableSceneBuilds` additionally requires the exact local `OrbitCamera` state, viewport and
voxel stage to match the last successfully uploaded scene. Replacing a visible decoded block
invalidates the saved context even if its ID is unchanged. Cache/dataset resets clear it.
`reusableVoxelBuildMs` is the subset of build elapsed time for those candidates;
`reusableVoxelSourcePoints` is their accumulated cut input count (decimal string).
These counters measure candidates; this revision still performs every build/upload.

One exploratory Majadas run under the preceding Firefox/gray/1280×720/4 px protocol records
239 loaded blocks/builds, **143 unchanged cuts**, **141 reusable scenes**, **355,318,762** reusable
cut source points and **40,972 ms** reusable construction out of **59,090 ms** voxel construction.
The two unchanged cuts that fail the input match coincide with progressive voxel-stage changes.
Target completes in 65,772 ms, with the same final 6,397 voxels and complete coverage. This is
one diagnostic run, not a repeated timing baseline or promised saving. Raw export:
`target/unchanged-cut-bench/measured/majadas-1.json`. 25 WASM-crate tests, native/WASM Clippy,
formatting and release WASM build passed. The next task is to skip only matched builds while
preserving planner/cache commits, payload invalidation, camera/viewport/stage changes and scene
identity, then measure again.

### Identical-scene reuse — 12-10-2026

Matched scenes now skip construction and CPU upload. Planner commits and cache protection still
run on every update. `sceneBuilds` and `voxelSourcePoints` count actual construction;
`reusedSceneUpdates` and decimal-string `reusedSourcePoints` count avoided work. The earlier
candidate-build counters remain available, but stay zero when all matching builds are skipped.
Camera, viewport, voxel stage and successful visible-payload changes invalidate reuse. Protected
visible cache entries reject replacement; a rejected insertion preserves the existing scene.

Three Firefox runs with the preceding XYZ-only Majadas protocol completed without failures:

| Run | Target LOD ms | Voxel construction ms | Builds / reused updates |
| --- | ---: | ---: | ---: |
| 1 | 30,827 | 19,564 | 98 / 141 |
| 2 | 21,677 | 16,847 | 98 / 141 |
| 3 | 29,488 | 17,453 | 98 / 141 |

Each loaded 239 blocks, constructed from 166,721,822 accumulated source points and avoided
355,318,762 repeated inputs. All ended with 6,397 voxels, 7,015,449 visible source points,
202 visible blocks and complete coverage; the stable observation window was 10,000 ms with
approximately 60 frame replies/s. Target median is 29,488 ms [21,677–30,827]. The preceding
sessions were not paired, so these times do not establish a controlled speedup ratio. Concrete
GPU identity, GPU timings and memory remain unmeasured. Raw exports/capture/session:
`target/unchanged-cut-bench/reused/`.

The dedicated-worker regression `examples/web-host/tests/lod-reuse.browser.mjs` fails with
“identical scene was rebuilt” before this change and passes afterwards. It checks identical view,
camera, viewport, stage, protected/replaced payload, attribute reset and dataset reopen. Serve
rebuilt `pkg/` and web-host fixtures over HTTP, create it as a module Worker, post an empty
message, and wait for `type: "probeResult"` (ignoring progress messages). WebGPU and
OffscreenCanvas are required; this is separate from the Node host suite. Validation also passed
83 native tests (two diagnostic tests ignored), native/WASM Clippy, release WASM and formatting.

### Browser range-read accounting smoke — 12-10-2026

One Majadas XYZ-only Firefox run with the same viewport/cache/4 px protocol records
**1,175 ms** block loading: **275 ms** range-cache fills and **900 ms** residual decoding/other
work. **542 fills**, **142,082,048 bytes** including rereads; this differs deliberately from
unique file-coverage bytes. Final scene remains 6,397 voxels, full coverage, 98 builds and
141 reused updates. Target: 34,558 ms. This single run validates observability, not instrumentation
overhead or a speedup. Metadata/sidecar opening and non-loader attribute scans are excluded by
before/after snapshots around each callback. Coordinates and optional attributes share counters.
Counters reset with dataset/cache resets; errors accumulate internally and the public API retains
one coherent last-successful snapshot until the next success. Conversion readers remain
uninstrumented and preserve the LAS reader's Send/Sync contract without unsafe implementations.

Firefox worker regression passes metadata exclusion, repeated-view no-read and reopen checks
alongside the seven reuse flows. Native 25 tests, native/WASM Clippy, release WASM, formatting
and diff checks pass. Raw diagnostic: `target/unchanged-cut-bench/read-metrics/majadas-1.json`;
probe: `target/unchanged-cut-bench/probe-read-metrics/result.json`. Repeated range/decode
measurements, attribute-sidecar cells, concrete GPU identity and browser memory remain pending.

### Albuera repeated gray load after reuse — 12-10-2026

Release WASM from `0c69c86`, Firefox 157.0.1, fresh WebGPU-enabled profile, XYZ-only
(default RGB mode therefore gray), fit camera, 1280×720, 4 px, depth shading, 256 MiB cache.
HTTP memory/disk cache disabled; OS cache not flushed. Three sequential loads in one browser
session; no compilation or other benchmark ran concurrently. All three exports completed:

| Run | First coarse frame ms | Target LOD ms | Voxel construction ms |
| --- | ---: | ---: | ---: |
| 1 | 89 | 151,399 | 103,626 |
| 2 | 61 | 119,886 | 96,539 |
| 3 | 55 | 140,347 | 99,333 |

Target median **140,347 ms [119,886–151,399]**; first coarse frame median 61 ms [55–89].
Every run loaded 746 blocks, built 233 scenes and reused 513: 503,187,945 accumulated inputs
constructed and 1,115,026,990 avoided. Final scene: 17,030 voxels, 12,673,149 source points,
568 visible blocks, zero pending, 4 px and full coverage. Cache: 203,065,440 bytes; unique
coordinate bytes read: 276,320,912. Stable 10 s windows recorded approximately 60 replies/s.
Selection took 5,955–6,342 ms, planning 8,138–8,703 ms and combined block loading
1,604–2,950 ms; voxel construction remains the largest measured phase. Earlier failed
sessions are preserved above; this is not a paired speedup comparison. GPU identity, GPU
execution time, JS/WASM/RSS memory and RGB/typed-attribute runs remain open. Raw exports,
first-run capture and session: `target/unchanged-cut-bench/albuera-reused/`.

### Belvedere RGB at 8 px — 12-10-2026

Release WASM, Firefox 157.0.1 headless with a fresh WebGPU-enabled profile, HTTP memory/disk caches
disabled, OS cache not flushed, 1280×720/DPR 1, 8 px, depth shading and 256 MiB decoded cache.
Three sequential page/worker/dataset loads used matching Belvedere `.oct` (981,063,326 B) and
`.octattrs` (610,586,859 B). No build or native conversion ran concurrently. All exports passed
framebuffer, target, complete-coverage, zero-pending and 10 s steady-window checks; the first
capture was visually inspected.

| Run | First coarse frame ms | Target LOD ms | Voxel build ms | Range read ms | Decode/other ms | FPS replies |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 32 | 122,576 | 94,227 | 1,466 | 1,338 | 60.002 |
| 2 | 20 | 102,805 | 94,676 | 403 | 1,206 | 60.002 |
| 3 | 39 | 124,256 | 100,504 | 1,387 | 1,392 | 60.006 |

Target median **122,576 ms [102,805–124,256]**; first coarse frame **32 ms [20–39]**. Each run
loaded 389 blocks, built 151 scenes and reused 238 updates. Final cut: 4,835 voxels, 13,238,581
visible source points, 320 blocks, full coverage, 0 pending, HZB active and 211,910,672 decoded
cache bytes. No cache eviction or rejected insertions occurred at this target. Each run recorded
402,391,040 range-fill bytes in 1,535 calls; about 60 worker frame replies/s over the steady window.

These are 8 px RGB measurements: they do not close the 4 px Belvedere target, which previously
reached a cache-cycle pause, and are not directly comparable to 4 px measurements of other datasets.
Browser JS heap and concrete GPU adapter remain unavailable; uploaded instance bytes are
16,709,480, not total GPU memory. Raw exports, schema and first capture:
`target/unchanged-cut-bench/belvedere-rgb-8px/`.

### Albuera RGB sidecar — 12-10-2026

Release WASM, Firefox 157.0.1 headless with fresh WebGPU-enabled profile, HTTP memory/disk cache
disabled, OS cache not flushed, 1280×720/DPR 1, 4 px, depth shading and 256 MiB decoded cache.
Three sequential page/worker/dataset loads used matching `albuera-uav.oct` (6,105,617,040 B) and
`.octattrs` (7,561,226,723 B). No native conversion or build ran concurrently. All three exports
passed the exact framebuffer, completed-coverage, zero-pending and 10 s observation-window checks;
the first capture was visually inspected and shows RGB color.

| Run | Metadata ms | First coarse frame ms | Target LOD ms | Range read ms | Decode/other ms | Voxel build ms | FPS replies |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 59 | 72 | 142,524 | 6,638 | 1,282 | 97,593 | 59.996 |
| 2 | 98 | 110 | 137,601 | 1,005 | 1,282 | 99,069 | 60.008 |
| 3 | 103 | 114 | 139,616 | 997 | 1,264 | 97,757 | 59.996 |

Target median **139,616 ms [137,601–142,524]**; first coarse frame **110 ms [72–114]**. Each
run loaded 746 blocks, built 233 scenes and reused 513; total voxelization consumed inputs from
503,187,945 source points and skipped repeated work over 1,115,026,990 inputs. Final cut is stable
across runs: 17,030 voxels, 12,673,149 visible source points, 568 blocks, full coverage, zero
pending, 203,065,440 cache bytes and HZB active. Unique bytes read: 276,320,912; range fills
including rereads: 575,143,936 across 2,194 calls. The first run's range-read time is an outlier;
read time includes cache fills, and is not a cold-storage or physical-I/O measurement. All three
steady windows recorded about 60 worker frame replies/s, not GPU/display FPS.

The browser does not expose its JS heap here (`jsHeapUsedBytes: null`), device-memory estimate or
concrete GPU adapter; uploaded instance bytes are 97,115,760, not total GPU memory. These are RGB
load measurements, not a controlled comparison with the separate XYZ-only sessions. Raw JSON,
first colored capture, schema choices and adapter fields:
`target/unchanged-cut-bench/albuera-rgb/`.

### Albuera intensity-column reload — 12-10-2026

Three valid Firefox 157.0.1 headless rounds open matching Albuera XYZ/sidecar in RGB, wait for the
complete target, then select `attribute:0:0` (intensity) and measure the resulting `color` refinement.
Fresh WebGPU-enabled profile for the retry; HTTP caches disabled and OS cache not flushed;
1280×720/DPR 1, 4 px, depth shading and 256 MiB. One third attempt failed before export when the
BiDi driver timed out on `script.evaluate`; it reported no viewer/worker error. A retry in a fresh
profile with a longer command timeout completed. Thus **3/3 valid exports, one failed attempt**;
the third result is a retry, not the third page in the first browser session.

| Valid run | Initial RGB target ms | Intensity first frame ms | Intensity target ms | Range read ms | Decode/other ms | Voxel build ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 149,789 | 9 | 136,702 | 858 | 1,348 | 96,666 |
| 2 | 124,117 | 18 | 119,098 | 877 | 1,375 | 95,432 |
| 3 (retry) | 147,380 | 10 | 137,661 | 836 | 1,342 | 96,485 |

Initial RGB target median **147,380 ms [124,117–149,789]**; intensity target median
**136,702 ms [119,098–137,661]**, with first frame **10 ms [9–18]** after the column change. All
valid exports selected intensity and retained the same final scene: 17,030 voxels, 12,673,149
visible source points, 568 visible blocks, full coverage, 0 pending and 203,065,440 cache bytes.
Each refinement read/loaded 746 blocks; paired initial + reload counters show 1,492 insertions,
with no eviction/rejection. The reload therefore still voxelizes about 96 s: immutable geometry
selections retain their counters and final scene, but selected intensity requires new decoding and
scene output. The 10 s windows record ~60 worker frame replies/s, not display/GPU FPS. The numeric
range/decode work is below 2.3 s combined; voxel construction dominates. No paired/controlled
comparison with RGB target time is claimed.

The renderer exports no browser JS heap or total GPU memory; adapter identity is unavailable. Raw
valid exports are in `target/column-reload-bench/albuera-intensity/` (runs 1–2) and
`target/column-reload-bench/albuera-intensity-retry/` (run 3). The failed attempt is recorded as
`albuera-3-failure.json` in the first directory.

### Majadas RGB sidecar — 12-10-2026

WASM from `980b1c5`, Firefox 157.0.1, new WebGPU-enabled profile, same fit-camera/1280×720/4 px/
256 MiB/depth-shading protocol. Three sequential loads with matching `majadas-tls.octattrs`
(3,319,094,608 bytes) and coordinates (4,214,687,109 bytes). HTTP caches disabled, OS cache
not flushed, no concurrent compilation/benchmark. All exports valid, colored capture inspected:

| Run | First coarse frame ms | Target LOD ms | Range read ms | Decode/other ms | Voxel build ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 65 | 31,335 | 675 | 758 | 16,285 |
| 2 | 79 | 28,860 | 362 | 742 | 17,773 |
| 3 | 43 | 20,458 | 225 | 672 | 15,795 |

Target median **28,860 ms [20,458–31,335]**; first coarse frame median **65 ms [43–79]**.
Every run: 239 loads, 98 builds, 141 reused updates; 851 range fills / 222,393,664 bytes from
both readers including repeated fills. Final geometry unchanged: 6,397 voxels, 7,015,449 source
points, 202 visible blocks, full coverage, zero pending and 112,290,672 cache bytes. Stable
10 s windows approximately 60 frame replies/s. The selector exposes eight schemas including
RGB components, intensity and classification; presence in the selector does not validate a
large selected-column load. These sessions are not paired against gray runs, so do not infer
an attribute overhead ratio. GPU identity, GPU duration and browser memory remain unknown.
Raw exports/capture/schema choices: `target/unchanged-cut-bench/majadas-rgb/`.

The export now records `dataset.attributes` as `{name, fileBytes}` or explicit `null`. Legacy
exports without the field mean unknown, not no sidecar. The collector groups these separately
and includes name/size, color and depth shading in its displayed rows. A regression initially
merged four unknown/absent/different-sidecar inputs into one row; it now produces four groups.
Names and byte counts are provenance hints, not content hashes. Schema version 1 is retained
for this additive field. Node host tests (25), Python host tests (5), JS/Python syntax and diff
checks pass. Large selected-column comparisons and browser mismatched-pair rejection are next.

### Typed-attribute integration probe — 12-10-2026

The old built-in pair had display RGB but no selectable numeric schema. Its initial typed probe
therefore failed the schema assertion; the fixture generator now emits intensity `u16` and
classification `u8`. Positions, display colors and 4,096 source points are unchanged. Native
fixture validation checks exact ranges [0,65,016] and [0,3] and intensity row count. The new
`lod-attributes.browser.mjs` passes generic intensity selection, classification and invalid-index
rejection; the seven-flow reuse/read probe still passes with the regenerated pair.

A separate Firefox worker integration run uses Majadas XYZ/sidecar and explicitly selects
`attribute:0:0` (intensity) and `attribute:7:1` (RGB component). Both complete with 6,397 voxels,
7,015,449 visible source points, 202 visible blocks and 112,290,672 cache bytes. Repeated range
fill bytes are respectively 202,119,152 and 235,149,296. Invalid-column selection is rejected;
attaching Albuera's sidecar to Majadas is rejected and retains the previous completed scene.
Raw result: `target/unchanged-cut-bench/probe-attributes-majadas/result.json`; small result:
`target/unchanged-cut-bench/probe-attributes-typed/result.json`.

This probe calls update/render sequentially, yielding between iterations. Its single reload
intervals (17,991/18,607 ms) are **not** host-loading benchmarks, FPS or repeated performance
baselines; counter equality is not a full spatial/color scene hash. At this point the host color command still retained the previous RGB `targetLodMs`; the
scoped-timing change below corrects that before repeated host column-change benchmarks. Browser memory/peak attribute decode and other
browsers remain open. Validation: 42 format tests, format Clippy, both Firefox probes, JS syntax,
formatting and diff checks pass. No storage/renderer algorithm change was needed.

### Scoped host refinement timing — 12-10-2026

A mocked worker regression against the preceding implementation observes 25 ms from the initial
load where the new color request should report its own 30 ms first-frame interval. The worker now
restarts its timing context only after a successful settings request. `refinement` exports request
ID, trigger (`dataset`, `color`, `detail`), color, `firstFrameMs` and `targetLodMs`; `load` retains
initial RGB timing and initial target pixels. IDs order UI responses, not storage/cache revisions.
Color and detail travel in one bounded coalesced settings command, so the last pending request
carries both current values. The host ignores older progress, cancels previous FPS windows on
settings/new-file changes, and waits for a rendered target frame for the newest request.

The collector uses current refinement first/target times, separates triggers and rejects explicit
pending/stale-color contexts. Legacy exports without `refinement` retain initial-load behavior.
`load.refinementWork` retains its cache-reset scope, which can span multiple detail requests;
the legacy `workerRenderCallDuringLoadMs` label reports active-refinement render samples.

Firefox desktop 1688×768 / framebuffer 1280×720 interaction check with the typed fixture passes:
initial RGB target 216 ms; selected intensity target 104 ms; rapid color/detail changes after an
intentional invalid selection end at classification / 3 px, target 254 ms and 4,096 voxels. All
three final exports have 10,000 ms observation windows and retain initial `load.targetLodMs=216`.
These are functional diagnostics, not repeated timing baselines. Screenshot inspected; no layout
change was needed. Artifacts: `target/color-refinement-bench/after4/`.

The first interaction attempt exposed a separate early timer completion: a **9,999 ms** export,
preserved as `target/color-refinement-bench/initial.json`. Completion now reschedules only the
missing interval rather than publishing an incomplete window. Intermediate scripts needed fixes
for expected-error polling and BiDi global-const reuse; those partial outputs remain under
`after/`, `after2/`, `after3/` and do not count as completed interaction checks or benchmark repeats.
Final checks: 26 Node tests, 6 Python tests, JS/Python syntax, diff checks, local HTML/ZIP packaging
and the Firefox success/error/recovery flow pass. The package builder's exact metrics-import
marker was updated alongside the new window constant. Next: repeat isolated large column reloads
from a stable RGB view, then complete the remaining large/browser/memory cells.

### Repeated Majadas intensity reload — 12-10-2026

Revision `ca30050`, Firefox 157.0.1 headless, new WebGPU-enabled profile, HTTP caches disabled,
OS cache not flushed, fit camera / 1280×720 / 4 px / depth shading / 256 MiB. Each of three page
loads opens matching Majadas XYZ/sidecar in RGB, waits for its complete target, selects
`attribute:0:0` (intensity), immediately arms FPS and waits for the new target plus 5 s warmup /
10 s observation. No concurrent compilation or benchmark. These are reloads from a resident RGB
view with retained reader/OS caches, not cold initial selected-column loads.

| Run | Initial RGB target ms | Reload first frame ms | Reload target ms | Read/copy ms | Decode/other ms | Voxel build ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 22,420 | 6 | 21,018 | 380 | 660 | 16,293 |
| 2 | 22,750 | 6 | 21,696 | 185 | 702 | 17,181 |
| 3 | 20,242 | 7 | 20,322 | 188 | 672 | 15,969 |

**3/3 valid**, reload median **21,018 ms [20,322–21,696]**, first frame **6 ms [6–7]**.
Every reload: 239 loaded blocks, 98 builds, 141 reuses, 773 range fills / 202,119,152 bytes
including repeated XYZ/selected-column fills. Final 6,397 voxels, 7,015,449 source points,
202 blocks, zero pending, complete 4 px coverage and 112,290,672 cache bytes. All observation
windows are 10,000 ms, approximately 60 frame replies/s. The collector reports `Timing phase=color`
and reload times; metadata time remains that of the initial opening. Voxel construction still
dominates measured work. Other components/datasets, cold cache, GPU time and browser memory
remain open; no speedup ratio is claimed. Raw exports, schema choices, session and capture:
`target/column-reload-bench/majadas-intensity/`.

### Standalone file-origin interaction check — 12-10-2026

The regenerated `artifacts/visor-web-local/index.html` was opened directly via `file://` in a
fresh Firefox 157.0.1 WebGPU-enabled headless profile, with the HTTP server stopped. The page
reports a secure file context, loads embedded coordinates/sidecar and completes refinement.
Focusing the color selector and pressing the `c` key selects Classification through native
keyboard type-ahead. The ensuing export has `refinement.trigger=color`, classification,
4,096 voxels, zero pending, full coverage and exactly 10,000 ms observation. Capture inspected.
Raw export/session/capture: `target/offline-package-smoke/`. The single 154 ms target interval
is a functional diagnostic, not a performance baseline. This does not validate conversion,
OPFS/quota, all keyboard interactions, device loss or Chrome. The modular development host
continues to require HTTP/HTTPS; this check applies to the deliberately packaged HTML/ZIP.

## Comparison matrix

Use the same browser/device and three runs per cell; the collector reports median and min–max range.
Begin with one run
per cell to find failures, then repeat valid cells three times.

| Comparison | Dataset / scale | Viewport | Finest voxel target | Purpose |
| --- | --- | ---: | ---: | --- |
| Cloud size | Outer Banks 111,190 points (1,335,303 B XYZ + 907,255 B attributes); Albuera 508.8 M points (6,105,617,040 B XYZ + 7,561,226,723 B attributes); Majadas 415.5 M points (4,214,687,109 B XYZ + 3,319,094,608 B attributes) | 1280×720 | 4 px | Initial load and frame rate as file/point count grows; declare whether attributes are attached. |
| LOD cost | Albuera | 1280×720 | 4, 2, 1, 0.5 px | Effect of requested detail, selected points, voxels, blocks and cache. |
| Viewport cost | Albuera | 640×480, 1280×720, 1920×1080 | 2 px | Pixel workload at fixed dataset and LOD. |

The first pair of comparisons share the Albuera 1280×720, 4 px case. Run each cell only after the
previous dataset is closed/reopened; do not rotate or pan during the steady-state sample. On each run,
wait for `target reached`, click **Run 15 s FPS benchmark**, keep the camera still, then export JSON
when the button re-enables. The first 5 s are discarded; the next 10 s are used for frame cadence.

## Results

Keep one table per browser/device. Populate it from exported JSON; do not mix native CLI measurements
or different adapters in one aggregate.

| Browser / device | Dataset | Points | File (B) | Viewport | Target (px) | Metadata (ms) | First coarse (ms) | Target LOD (ms) | FPS | Reply p95 (ms) | CPU frame p95 (ms) | Voxels | JS heap (B) |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Firefox 157.0.1 headless / adapter undisclosed | Outer Banks, gray XYZ | 111,190 | 1,335,303 | 1280×720 | 4 | 3 [2–3] | 80 [63–113] | 251 [197–370] | 60.00 [60.00–60.01] | 18 [18–18] | 1 [1–1] | 2,410 | unavailable |

Run the collector over exported files to form medians and valid-run counts:

```bash
python3 examples/web-host/collect-benchmarks.py artifacts/web-bench/*.json
python3 examples/web-host/collect-benchmarks.py --format csv artifacts/web-bench/*.json
```

### First Firefox run — 12-10-2026

The table above contains medians [minimum–maximum] over **3/3 valid runs**, from the corrected
host's exports. It is the initial small-file gray baseline, not the full comparison matrix.

The first real exports reproduced a validity bug: `measurementWindowMs` held the span between
sampled replies (~9,982–9,984 ms), while the collector required at least 10,000 ms. A completed
15 s benchmark was therefore rejected. The fix reports the elapsed stable observation window
separately from `frameReplySpanMs`; the FPS calculation and the 5 s/10 s protocol are unchanged.
A regression with replies away from both boundaries fails before the fix and passes afterwards.
Old exports are retained as exploratory; their durations are not edited to make them valid.

Firefox **157.0.1**, Linux `7.2.9-200.fc44.x86_64`, AMD Ryzen AI 9 365, release WASM rebuilt
offline from this checkout. Headless browser with a separate temporary profile, localhost,
`dom.webgpu.enabled=true`, and HTTP memory/disk cache disabled. OS file cache is not flushed;
repeated reads are warm-cache observations, without guaranteed residency. Browser restarts
before the three runs; the page/worker is recreated between runs. No native conversions run
concurrently. Framebuffer **1280×720**, DPR 1, target **4 px**, default fit camera, RGB mode
without sidecar (gray), depth shading on, cache budget **256 MiB**.

With default Firefox preferences `navigator.gpu` was absent and renderer creation failed.
Enabling WebGPU in the temporary profile allowed presentation. Both WebGPU adapter info and
the renderer's adapter name/driver were empty; hardware versus software execution could not
be established. Treat these results as a separate headless environment, not as a named GPU
baseline or interactive Chrome/Firefox validation. JS heap memory is unavailable in these
Firefox exports; renderer resource estimates do not supply missing JS/WASM/process memory.

Chrome/Chromium is not installed. The first Belvedere waiter timed out after 120 s watching
status text (~129.1 MiB of unique range reads). That text can be overwritten by other messages,
and the attempt used cached old host modules: it does not establish whether the target was
reached. The corrected large-file experiments below supersede this inconclusive attempt.
Sidecar association, color changes, OPFS, cancellation, picking, actual GPU loss and headed
browser measurements remain pending.

Local raw exports and profile capabilities: `target/web-browser-probe/final-runs/`;
the one-off BiDi automation is `target/web-browser-probe/measure.mjs`. It selects local files,
uses the host's benchmark/export buttons and captures the emitted JSON Blob without changing
its payload. The server exposes only a staged copy of host modules, WASM and deterministic
sample, not the repository root or datasets; file inputs supply the local `.oct`.
Reproduce manually with the protocol above and run the collector on the three exported files.
Seven Node suites (25 tests), four Python tests and local packaging pass; the single-file
viewer was regenerated after the change. Automated tests do not certify the pending flows.

### Large-file experiments — 12-10-2026

Same Firefox/headless/WebGPU settings, release WASM, gray XYZ without sidecar, 1280×720,
4 px target and 256 MiB cache. No native conversions ran concurrently. The page and worker
are recreated for each attempt; OS cache is not reset or guaranteed resident. Adapter details
remain undisclosed, so these are not measurements attributable to a named GPU. Albuera uses
a fresh browser process/profile with the same preferences after the Majadas automation failure.

The revised automation arms the existing benchmark after selecting the file and waits for
the export button to enable. The host starts its 15 s window only once the target has complete
coverage, no pending blocks and the requested stage. Successful exports are checked again
for viewport, stage, coverage, pending blocks and a 10 s stable window. It no longer waits
for transient `renderer-status` text. Timeouts below bound **load plus benchmark completion**,
not just loading. After a failed cell, repetitions stop and the failure is preserved.

Majadas completed **two valid exports from three attempts**; the third lost BiDi
`script.evaluate` responsiveness for over 30 s, without an export or final DOM/screenshot.
The cause is unresolved. This is an exploratory series, not a completed three-valid-run
baseline. Median [minimum–maximum] over its two valid exports:

| Dataset | XYZ file (B) | Metadata (ms) | First coarse (ms) | Target 4 px (ms) | FPS | Reply p95 (ms) | Stable CPU p95 (ms) | Submitted/current HZB-visible voxels |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Majadas TLS | 4,214,687,109 | 57.5 [48–67] | 68.5 [61–76] | 157,218.5 [152,983–161,454] | 60.00 [60.00–60.00] | 18 [18–18] | 1 [1–1] | 6,397 [6,397–6,397] |

Both valid Majadas exports report **415,465,882 dataset points**, **7,015,449 selected source
points**, 202 visible blocks, zero pending blocks, complete coverage and HZB active.
Unique bytes read **102,958,469**; decoded cache **112,290,672 B**. CPU build peak estimate
**34,629,024 B**, final instance capacity **255,880 B** and projection-cache peak **11,917**
entries. These are resource counters/models, not JS/WASM/process RSS or total GPU memory.
The whole-load/run rolling worker-call p95 is **873/933 ms** and p99 **1,428/1,516 ms**;
those mixed-window values are distinct from the stable-window p95 of 1 ms. A quick coarse
view and ~60 reply FPS after loading do not imply quick refinement or responsive loading.
These FPS count replies; they do not measure GPU completion or visible pixel count.

Two other cells did not produce a completed benchmark. Their last **DOM observations** are
retained with screenshots, not converted into FPS or aggregate medians:

| Dataset / attempts | Completion limit | Current stage | Pending blocks | Current coverage | Selected source points | Current HZB-visible voxels | Last CPU render call | Decoded cache / budget |
| --- | ---: | ---: | ---: | --- | ---: | ---: | ---: | --- |
| Belvedere / 1 | 180 s | 8 px | 64 | complete, 0 uncovered | 6,985,426 | 5,140 | 1,983 ms | ~108.6 / 256 MiB |
| Albuera / 1, fresh browser | 300 s | 4 px | 187 | complete, 0 uncovered | 2,176,303 | 16,674 | 769 ms | ~33.5 / 256 MiB |

Both show a visible gray cloud in the inspected screenshot. Coverage of the **current cut**
does not establish completion of the requested refinement while blocks remain pending.
Albuera already reports stage 4 px but still has pending blocks; it is not a completed
target benchmark. Read progress was ~155.4 MiB of Belvedere's 981,063,326 B XYZ file and
~134.5 MiB of Albuera's 6,105,617,040 B XYZ file. Albuera's last rolling call p50/p95/p99
was 755/793/804 ms; Belvedere's was 265/1,225/1,764 ms. No rejection by the CPU/cache budget
was observed in those snapshots; no conclusion about the cause follows from their sizes.

Local evidence: `target/web-browser-probe/large-final-runs/` holds the two Majadas JSON exports,
its third-attempt failure and the Belvedere DOM/screenshot; `albuera-final-runs/` holds Albuera's
separate profile capabilities and failure DOM/screenshot. File-input selection keeps datasets
local; the staged HTTP server exposes only host/runtime assets. Driver logs include shared-texture
fallback warnings; these do not establish hardware/software execution or explain the failure.

The next measured work is to split block read/decode, LOD selection, voxel construction and
upload CPU time, retaining the last visible cut and the budgets. Test whether repeated work
during refinement dominates before changing caches or scene construction. Reproduce the
third-attempt browser/automation failure separately. Closing the large-file baseline requires
three valid comparable runs per cell with failed attempts counted, including Chrome and
the paired/color path; the current evidence does not close that baseline.

### Belvedere after scene reuse — 12-10-2026

One gray XYZ attempt in a fresh Firefox 157.0.1 headless profile, WebGPU enabled,
1280×720, 4 px target and 256 MiB decoded cache, with the scene-reuse and range-read
instrumentation. No native conversion or build ran concurrently. The driver stopped
after the first failed attempt; this is not a three-run baseline.

The 300 s completion limit expired without an enabled benchmark export. The final DOM
reported first coarse frame **24 ms**, stage **4 px**, **782 pending blocks**, complete
coverage (zero uncovered), **14,871,589 selected source points**, **20,977 current
HZB-visible voxels**, 339 visible blocks and decoded cache **255.8 / 256.0 MiB**.
The last CPU render call was 60 ms; rolling p50/p95/p99 were 64/71/2,034 ms over
2,338 frames. These mixed-window values are not stable FPS or target completion times.
The near-full cache is an observation, not evidence of a rejected insertion or the
cause of incomplete refinement. Diagnose cache rejections and actual block progress
before changing the budget or transition policy.

Local evidence: `target/belvedere-reuse-bench/session.json`,
`belvedere-1-failure.json` and `belvedere-1-failure.png`. The staged HTTP server exposed
only public host/runtime assets. OS cache was not flushed; adapter identity, JS/WASM
heap and process/GPU memory remain unmeasured.

### Cache pressure diagnostics — 12-10-2026

`lodStats().cacheActivity` exposes decimal-string counters `insertions`, `evictions`,
`rejectedOversize` and `rejectedProtected`, also exported as `scene.cacheActivity`.
They cover the lifetime of the dataset's cache, including attribute resets that
clear its payloads. Reopening a dataset starts new counters. Cache activity is live
even if the last accepted scene predates a failed insertion; other scene/work fields
retain their existing snapshot contract. Older exports omit these diagnostics.

`refinementWork.evictedPendingBlocks` counts blocks evicted by a successful LOD load
that are immediately pending in the subsequent target plan. It follows the work
counters' cache-reset scope, not the longer cache-activity scope. This is direct
evidence that some target work was displaced; one occurrence alone does not establish
a permanent stall. It does not count unique blocks, bytes or physical reads.

A deterministic core regression reproduces a possible cycle: protect visible parent
R, leave room for only one of target children A/B, then insert A→B→A. Each insertion
succeeds, evicts the other child after the first, and keeps R covering the view, with
one child always pending and zero rejections. Thus zero rejected insertions does not
prove that a requested replacement can fit alongside its fallback. No admission or
transition policy changes in this instrumentation.

Validation: core/WASM tests, native and WASM Clippy, release WASM, 26 Node tests, six
Python host tests, formatting and local packaging pass. The Firefox dedicated-worker
probe verifies initial insertion counters, a failed protected replacement visible
through `lodStats()`, unchanged-scene reuse and dataset-reopen counter reset. Local
evidence: `target/cache-pressure-probe/small/`. Confirm pressure over successive
fixed-camera Belvedere snapshots before selecting a mitigation.

### Belvedere target-block displacement — 12-10-2026

One diagnostic run on `3c3bc4d`, release WASM, fresh Firefox 157.0.1 headless profile,
WebGPU enabled, XYZ gray, 1280×720, target 4 px and 256 MiB cache. No build/native
benchmark ran concurrently. Six snapshots at 30 s intervals use the host export
handler directly; **these are incomplete diagnostic exports, not completed FPS
benchmarks**. OS cache was not flushed.

| Driver elapsed (s) | Stage (px) | Pending | Cache bytes | Loaded blocks | Evictions | Evicted target blocks | Scene builds | Scene reuses |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 30 | 8 | 98 | 78,222,096 | 291 | 0 | 0 | 95 | 196 |
| 60 | 8 | 53 | 131,925,632 | 336 | 0 | 0 | 121 | 215 |
| 90 | 8 | 21 | 175,674,864 | 368 | 0 | 0 | 136 | 232 |
| 120 | 8 | 3 | 207,399,376 | 386 | 0 | 0 | 149 | 237 |
| 150 | 4 | 782 | 267,888,736 | 443 | 0 | 0 | 156 | 287 |
| 180 | 4 | 785 | 267,705,040 | 848 | 481 | 408 | 156 | 692 |

All snapshots have complete current-cut coverage, no oversize/protected rejection
and no target completion time. The 150/180 s snapshots both retain **14,871,589 source
points**, **20,977 current HZB-visible voxels**, 339 blocks and 156 builds. First coarse
frame 29 ms. The final capture shows the gray cloud. DOM statistics update on their
own cadence: the final DOM had 459 evictions / 386 evicted targets, so use the exported
snapshots for the table, not a mixture of DOM/export counts.

This confirms successful cache insertions displacing still-requested blocks during
a fixed-view interval without scene progress. It does not prove that every eviction
is harmful or that no different transition policy could complete under the same
budget. It motivates an explicit bounded response to repeated displacement while
retaining the accepted cut. GPU/display FPS, heap/RSS and named adapter are not measured.

Evidence: `target/cache-pressure-probe/belvedere/{session.json,snapshot-1.json,…,snapshot-6.json,final-dom.json,final.png}`;
driver `target/cache-pressure-belvedere.mjs`. Pending: avoid unbounded repeated work,
test recovery via coarser detail/camera changes, and rerun the real diagnostic.

### Belvedere cache-cycle pause and recovery — 12-10-2026

Release WASM build `3c3bc4d` with the cache-cycle detector, fresh Firefox 157.0.1
headless WebGPU profile, gray XYZ, 1280×720, 4 px target and 256 MiB decoded
cache. An initial pressure diagnostic reached 150 s without evictions; a later clean
repeat with the detector paused at 180 s. No native benchmark or build ran during either
browser run. Cache snapshots are forced diagnostic
exports; no target/FPS benchmark was armed, and neither run is a valid large-file
performance benchmark.

| Repeat | Elapsed | Stage | Pending | Cache bytes | Insertions | Evictions | Evicted target blocks | Loaded blocks | Scene builds / reuses |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| First | 150 s | 4 px | 782 | 267,888,736 | 443 | 0 | 0 | 443 | 156 / 287 |
| First | 180 s | 4 px | 785 | 267,705,040 | 848 | 481 | 408 | 848 | 156 / 692 |
| Clean repeat | 150 s | 4 px | 809 | 240,448,400 | 416 | 0 | 0 | 416 | 153 / 263 |
| Clean repeat | 180 s | 4 px | 782 | 268,017,232 | 507 | 137 | 64 | 506 | 156 / 350 |

At each 180 s pause, the active cut stayed covered: **14,871,589 selected source points**,
**20,977 rendered voxels**, **339 blocks**, 0 uncovered nodes and no rejected insertions.
Evicted target blocks are a count of IDs from the eviction immediately missing from
the subsequent plan; they are not unique-block or byte counts. The second run paused
with the expected message and kept the screenshot's gray visible cloud. Its range reader
had filled **215.1 MiB of 935.6 MiB** of unique source bytes when the driver captured
the pause. This does not measure heap, RSS or physical I/O.

After changing detail from 4 to **8 px**, the same second run reached the coarser target
in **48,618 ms** with first coarse frame at **2,359 ms**, 0 pending blocks, complete
coverage, **13,238,581 source points**, 5,140 voxels, 320 visible blocks and HZB active.
A separate repeat then changed only the camera orbit with ArrowRight after the pause. The
worker resumed and loaded another block; pending work fell from 782 to 68 and the stage
restarted at 16 px. The new camera target was still incomplete in that capture, so this
confirms retry initiation, not completed recovery. Neither run armed the FPS benchmark.

The detector retains one normalized LRU checkpoint and Brent counters, compares block
ID/size/protection order exactly, and pauses only when that state repeats for the same
camera, viewport, voxel stage, selection error and target detail while the current scene
is reusable. Reader callbacks use the immutable opened v5 files. Changing camera/viewport,
selection/detail or successfully inserting a block clears the detector and permits retry.
This detects an exact recurring state for this deterministic read path; it does not call
a one-time eviction an impossible budget or assert that every cache cycle proves the
whole target cut can never fit.

Unit coverage checks longer cycles, changing LRU/order/protection/size, counter changes,
finite progress and external-insert reset. Core/WASM tests, native/WASM Clippy, release
WASM compilation, 26 Node tests, six Python host tests, formatting, package validation
and the Firefox worker probe pass. The small Firefox probe verifies activity reset on
reopen and live rejection counters while preserving the scene.

Local evidence is under `target/cache-pressure-probe/`:
`belvedere-initial-cycle/`, `belvedere-cache-cycle-final/` and
`belvedere-camera-recovery/` hold the diagnostic, pause/detail recovery and camera retry runs;
the latter also contains `recovery.json`, `recovery-status.json`, `final-dom.json` and
`final.png`. `final.json` exports are explicitly marked incomplete.

### Belvedere repeated camera recovery — 13-10-2026

Firefox 157.0.1 headless with the prepared WebGPU profile, Belvedere coordinates without a sidecar,
1280×720, 4 px target and 256 MiB decoded cache. The initial refinement paused after 180 s with
782 target blocks pending and 507 cumulative loads. Five successive `ArrowRight` camera changes
were allowed to progress until the detector paused again. Across those retries, cumulative loads reached 1,234
and `evictedPendingBlocks` reached 426; the final snapshot still had **805 target blocks pending**,
`targetLodMs: null` and the paused status. The renderer retained a complete visible cut, which is
distinct from completing the requested target cut. This run demonstrates renewed work after each
view change, but not recovery to the 4 px target under this cache budget. It does not identify
whether a larger cache would complete the target, and it does not measure detector overhead.

The earlier one-orbit probe only captured one additional block and was too short to characterize
recovery. The five-change run took about 336 s after the first pause; its incomplete snapshots and
final image are under `target/cache-pressure-probe/belvedere-camera-retry-settled/`. A separate
attempt with a default Firefox profile failed before data loading because WebGPU was disabled;
that attempt is excluded. Next: test a bounded cache increase to see whether the same changed-camera
target can complete.

### Brent cache-cycle detector microbenchmark — 13-10-2026

An ignored native Rust release test measures `CacheCycleDetector` with 500 synthetic resident
blocks, close to the 507 blocks loaded before the Belvedere pause. Three invocations each take five
timed rounds of 100,000 checks. Median per-invocation results were **4.45 μs** for an identical
checkpoint, **8.03 μs** for a mismatch at the final LRU entry (full comparison), and **8.89 μs** to
construct the detector/checkpoint. The three per-invocation medians ranged 4.44–4.56 μs, 8.02–8.23
μs and 6.74–9.02 μs respectively.

Run with `cargo test --offline --release -p viewer_wasm cache_cycle_detector_microbenchmark --
--ignored --nocapture`. The probe forces a long checkpoint interval to isolate equality-check cost;
it measures checkpoint allocation separately, but excludes occasional checkpoint refreshes, block
decode/cache insertion, and the full `update_lod`/WASM/browser path. These microseconds do not prove
an end-to-end frame improvement or quantify Firefox overhead. Source: the ignored diagnostic test
`cache_cycle_detector_microbenchmark` in `crates/viewer_wasm/src/lod_dataset.rs`.

### Belvedere 512 MiB decoded-cache capacity probe — 13-10-2026

A fresh Firefox WebGPU profile opens the same Belvedere coordinate file without a sidecar, at
1280×720 and a 4 px target. The staged host sets both its cache allocation and exported budget to
512 MiB; other application files are unchanged. No camera input is sent. The OS file cache is warm
from earlier runs, so this is a capacity/completion probe, not a timing comparison.

At 450 s the decoded cache held **511.3 MiB**, with 665 loaded blocks and no evictions. At 480 s,
the detector paused: **765 loaded blocks**, **510.9 MiB resident**, **211 evictions**, including 98
evicted target blocks, and **559 target blocks pending**. `targetLodMs` remained null. The retained
visible cut reported complete coverage; the requested 4 px cut did not complete. Doubling the
budget delayed pressure relative to the prior 256 MiB probe but did not remove the cycle in this
no-camera run. Do not infer a runtime speedup from these sessions.

Raw samples and final export are in `target/cache-pressure-probe/belvedere-cache512-followup/`;
the staged `main.js` and Firefox profile are under `target/web-browser-probe/site-cache512/` and
`target/firefox-cache512-followup-profile/`. The 1 GiB follow-up below tests the API's maximum;
quantify the requested working set and eviction sequence before deciding whether more memory is useful.

### Belvedere 1 GiB cache and camera observation — 13-10-2026

A fresh WebGPU Firefox profile used the same Belvedere coordinate file without a sidecar, 1280×720,
4 px target and staged host, this time setting the cache to the API maximum of 1 GiB. After **900 s**
the initial target still had **357 pending blocks**, with 868 loaded, **732.4 MiB resident**, no
evictions, and `targetLodMs: null`. Accumulated `voxelBuildMs` was **836,998 ms**; selection was
11,831 ms, planning 18,948 ms, block loading 5,478 ms and upload 176 ms. This cumulative synchronous
accounting points to voxel construction as the dominant measured work in this run, not cache
pressure. It does not establish a representative per-frame cost or a completed target.

An `ArrowRight` event after the 900 s cap set `cameraChanged=true`. The immediate export still
reported the earlier 8 px/zero-pending snapshot, while the host's final stats displayed the changed
4 px cut with 368 blocks pending and a 722 MiB cache. Its reported render-call p95 was 8,877 ms.
The post-camera sampler stopped after 10 seconds without a new export signature, too soon to resolve
that high-latency build. Treat the camera result as **inconclusive**; no full target completion or
pause was observed after the camera event.

Raw 30 s samples, camera snapshot and final DOM/export are in
`target/cache-pressure-probe/belvedere-cache1024-long/`; staged host and profile are under
`target/web-browser-probe/site-cache1024/` and `target/firefox-cache1024-long-profile/`. Next:
profile the voxel-build hot path at a representative native/WASM boundary, then repeat the browser
camera probe with a longer no-progress window and an export tied to the latest worker stats.


### External Firefox process-tree PSS — 12-10-2026

During three sequential Albuera RGB runs at 1280×720, 4 px and 256 MiB cache, an external Linux
monitor sampled `/proc` once per second. It followed the Firefox parent and descendants, summing
`Pss` from each `smaps_rollup`; PSS apportions shared pages across processes. The sample does not
include dedicated GPU memory, does not expose the JS/WASM heap separately and can miss sub-second
peaks. No build or native conversion ran concurrently.

| Load | Valid target export | Tree PSS sampled peak | Tree RSS sum peak | Minimum host `MemAvailable` |
| --- | --- | ---: | ---: | ---: |
| 1 | 152,401 ms, complete | 0.978 GiB | 1.651 GiB | 18.35 GiB |
| 2 | 120,234 ms, complete | 6.584 GiB | 7.268 GiB | 13.08 GiB |
| 3 | 137,271 ms, complete | 0.986 GiB | 1.663 GiB | 18.72 GiB |

One isolated second-load sample reached **6.584 GiB PSS**, mainly in the Firefox parent process,
about 38 ms before the export-complete marker. After navigating to the third run, PSS returned to
about **0.8–1.0 GiB**. This single-profile observation does not establish a persistent leak or its
cause. The lowest sampled host memory availability was 13.08 GiB and swap free remained at least
7.38 GiB. The RSS column sums per-process RSS and double-counts shared mappings; use tree PSS for
the process group. Frame timings from this memory run are not compared to unsampled runs.

A fresh-profile follow-up sampled Firefox `smaps_rollup` once per second over two sequential loads
and inspected the main process's largest mappings whenever tree PSS reached 2 GiB. Both exports
completed (147,510 and 135,438 ms), with full coverage, zero pending blocks, and the same 17,030
voxels/568 blocks. Sampled tree PSS peaked at **0.978/0.986 GiB**; its anonymous component peaked
at **0.798/0.805 GiB**, file-backed at **0.172/0.174 GiB**, and shared memory at **0.007 GiB**.
Host `MemAvailable` stayed above 18.5 GiB and swap free above 7.38 GiB. Neither load crossed the
2 GiB mapping-capture threshold, so this run did not reproduce the earlier 6.58 GiB event and has
no large-VMA snapshot to attribute. The spike is therefore intermittent under this protocol; its
owner and cause remain unknown. The one-second sampling interval can still miss shorter events.

Raw follow-up samples and valid exports are in `target/browser-memory/albuera-rgb-smaps/`; the
temporary capture script is `target/browser-memory/sample_firefox_smaps.py`. This bounded retry
narrows the next step to capturing a reproduced event with a profiler or higher-frequency sampler;
it does not justify changing application memory budgets.

Raw 1 s process-tree and host-memory samples, per-run completion markers, exports and driver logs
are in `target/browser-memory/albuera-rgb/`. Reproduction driver: `python3
target/browser-memory/sample_firefox_tree.py` while the root HTTP server is available on port 8765.

### Fresh-export camera recovery at 256 MiB — 13-10-2026

Release WASM rebuilt from `21615df`, Firefox 157.0.1 headless with WebGPU, 1280×720
framebuffer, DPR 1, fitted camera, 4 px target and the unchanged product cache budget of
256 MiB. Belvedere coordinates only, no sidecar; RGB control therefore renders gray.
The dedicated profile was previously used only for short driver setup attempts; OS cache
was warm and HTTP cache was not explicitly disabled. No native compilation or benchmark
ran during the Belvedere observation. This is one recovery diagnostic, not three timing
repetitions or a before/after browser speed comparison.

The committed `examples/web-host/probe-cache-camera.mjs` waits for the exact new exported
Blob instead of reading a previous asynchronous result. It saves the panel in the same JS
task as the export request. Stage/pending may differ while progressing because progress
messages update exported `latestLodStats` before a rendered frame updates the panel;
`domMatchesExport` makes that visible. Both terminal snapshots here match the panel.
The driver requires changed worker counters after `ArrowRight` before accepting a pause
or completion, and allows 120 seconds without progress instead of the earlier 10 seconds.

| Observation | Driver time in phase | Cumulative loads | Pending target blocks | Cache MiB | Evictions / evicted targets | Voxels |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Initial detector pause | 120.2 s | 507 | 782 | 255.60 | 137 / 64 | 20,977 |
| Pause after one ArrowRight | 50.1 s | 686 | 780 | 254.21 | 316 / 171 | 20,707 |

The camera restarts real work: **179 additional loads**, **74 additional scene builds** and
**104 reused scene updates**. The detector pauses again; `targetLodMs` remains null. The
retained cut reports coverage complete at stage 4 px, but the requested target remains
incomplete. This closes the narrow observation of camera resumption/re-pause at 256 MiB;
it does not close the previous 1 GiB scenario, solve cache capacity or establish a speedup.
Accumulated voxel work is 102,443 ms before the camera and 138,471 ms at the second pause.
Zero browser console errors; final screenshot retained. Heap JS and dedicated GPU memory
are unavailable. Playwright MCP could not start because its configured Chrome executable
is absent; Firefox BiDi supplied this browser evidence.

Reproduction commands and stop conditions are in
[the host README](../examples/web-host/README.md#reproduce-a-cachecamera-diagnostic-in-firefox).
Raw snapshots/session/console/screenshot: ignored
`target/cache-pressure-probe/belvedere-entry-camera-fresh/`.
WASM SHA-256: `4d996a69ad6b9189bb37401b0dd9a7897c5432af6ada9fcf0ea617f0d969f294`.
Earlier driver setup attempts stopped on disabled-button handling and on treating normal
panel lag as a fatal mismatch; they are excluded from timing/recovery conclusions.
The small coordinate-only sample also completes both phases with 4,096 voxels and no
console errors; worker scene-build counters increase from 3 to 6 after camera input. Its
statistics panel stays empty, recorded as `domMatchesExport: false`; completion is based on
fresh worker/export counters. Node syntax check and all 26 existing host tests pass.
Next: repeat the 1 GiB changed-camera case with this fresh-export driver and isolate
progressive accumulator reuse before changing product memory defaults.

### Chrome software WebGPU HZB buffer fix — 13-10-2026

After Chrome became available to Playwright MCP, default headless startup exposed
`navigator.gpu` but no compatible adapter. Launching channel `chrome` with
`--enable-unsafe-webgpu --use-angle=swiftshader` presented the initial scene. Chrome version
**155.0.8059.39**. These are software-adapter functional checks, not hardware performance.

Opening the built-in sample then failed consistently: initially a WASM `unreachable` panic,
followed by misleading `recursive use ... unsafe aliasing` errors after the trap. A temporary
instrumented worker traced the original failure to `GPUDevice.createBuffer` for
`octree-viewer-hzb-indirect-args`, **20 B**, usage 396, `mappedAtCreation: true`. Chrome threw
`RangeError` about the mapped allocation; wgpu's WebGPU backend unwrap then panicked.
No viewer-method reentry was observed; the existing serialized worker queue is retained.
Instrumented copies are under ignored `target/chrome-reentry/`.

The localized correction creates this buffer unmapped. `HzbResources::cull` already writes
all five indirect arguments via `queue.write_buffer` before every dispatch/draw, so initial
mapped contents were redundant. Buffer size, resource accounting and draw arguments remain
unchanged. The native GPU regression now reads back **all five arguments plus the visible
index**, expecting `[36, 1, 0, 0, 0, 1]` for the occlusion fixture. The existing browser
reuse probe explicitly presents the first two sample frames to exercise HZB creation/use.

Rebuilt release WASM and repeated the same Playwright opening: target complete with **4,096
voxels**, one visible block, zero pending and HZB active. Classification, ArrowRight, fit
and resize from a 1280×900 desktop viewport to 390×844 all retain complete target coverage.
The seven-flow worker probe passes identical view, camera, viewport, stage, payload,
attributes and reopen. No page exceptions; the unrelated `/favicon.ico` 404 remains in the
console. The default-launch adapter limitation is distinct from this corrected mapped-buffer
failure. The initial experimental wait-on-status driver was discarded because host status
can change to `Renderer: presented`; final verification reads fresh scene/refinement exports.

Validation: 85 renderer/WASM tests, three focused HZB tests including native GPU readback,
format check, native/WASM Clippy with warnings denied, and release WASM/bindings rebuild pass.
Screenshots saved as `target/chrome-hzb-desktop.png` and `target/chrome-hzb-mobile.png`
show a white canvas under this SwiftShader/Playwright setup despite the successful worker
exports; mobile canvas framebuffer is 364×547. Thus these checks certify functional
completion/counters, not correct Chrome visual output. Firefox visual captures remain
separate evidence.
This closes the small Chrome functional case only; visual Chrome output, large datasets, physical GPU behavior,
performance repetitions and remaining interaction matrix are pending.

Further Chrome launch diagnostics tried Vulkan/SwiftShader and Vulkan with a reported
AMD `rdna-3` adapter (`--enable-features=Vulkan --use-angle=vulkan --disable-vulkan-surface`,
plus `--enable-unsafe-webgpu`; the software variant additionally selects SwiftShader).
Both complete the small sample in exported counters but screenshots show no geometry,
rather than resolving visual validation. Captures: ignored `target/chrome-hzb-vulkan.png`
and `target/chrome-hzb-radv.png`. Cause remains unisolated; no extra renderer change is
justified by these setup-dependent screenshots alone. Native Vulkan reports RADV Radeon
880M and llvmpipe adapters; the focused HZB test exercises the fallback path.

### Fresh 1 GiB camera observation after lookup change — 13-10-2026

The same committed fresh-export probe opened Belvedere coordinates without a sidecar in
Firefox 157.0.1, 1280×720, DPR 1, 4 px. A temporary host copy changes exactly the two
256 MiB cache constants to 1 GiB; the product default and 256 MiB voxel-build estimate
limit remain unchanged. Staged WASM is frozen from `21615df`, SHA-256
`4d996a69ad6b9189bb37401b0dd9a7897c5432af6ada9fcf0ea617f0d969f294`.
The later HZB fix did not change this running session. Fresh dedicated Firefox profile,
OS cache warm, HTTP cache not explicitly disabled. Chrome diagnostics, a release WASM
build and focused checks ran concurrently during parts of this observation: **no timing
comparison or speedup is claimed**.

The probe reached its **600 s cap in each phase**, rather than detector pause or completion:

| Snapshot | Cumulative loaded blocks | Pending target blocks | Cache MiB | Scene builds / reused updates | Cumulative voxel CPU ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Before ArrowRight, 600 s cap | 796 | 429 | 658.08 | 215 / 581 | 529,750 |
| After ArrowRight, another 600 s cap | 1,015 | 207 | 889.98 | 262 / 755 | 1,109,381 |

Camera input is acknowledged and new worker counters progress: **219 additional loads**,
**47 additional scene builds**, **174 reused updates**. No cache evictions or insert rejections,
20,977 retained voxels, coverage complete for the visible cut, `targetLodMs: null` throughout.
This establishes continued activity after camera input with fresh exports, unlike the earlier
10-second no-progress window. It does not establish target completion, eventual cache
sufficiency, a stable terminal state or a paired detector/lookup runtime improvement.
Both final panels lag the exported progress counters (`domMatchesExport: false`); snapshots
retain both rather than treating an older displayed panel as the authoritative export.
Zero browser console errors; the final screenshot shows the retained gray cloud.

Raw session, 10-second samples, final exports and screenshot:
ignored `target/cache-pressure-probe/belvedere-entry1024-camera/`.
Staged host: `target/web-browser-probe/site-entry1024/`.
Next: quantify traversal-prefix opportunities and combined retained-builder memory before
prototyping more incremental work. If repeating this capacity observation to completion,
use a longer explicit cap and keep concurrent diagnostics/builds out of any timing series.

Post-fix Firefox release-WASM check also passes both initial/camera phases on the 4,096-voxel
sample with HZB active, zero pending blocks and zero console errors. Its final capture visibly
shows the gray voxel grid (`target/cache-pressure-probe/sample-hzb-after/final.png`), unlike
the unresolved Chrome captures. Current post-fix WASM SHA-256:
`04dd3438249a76f7071822ccbae843b4823ab9928255e508e41a9d86203057f3`.


### Independent presentation and picking diagnostic — 13-10-2026

New served fixture: `examples/web-host/tests/presentation.html`, backed by two dedicated workers
in `presentation.browser.mjs`. It separates an independent raw WebGPU red clear and one-pixel
readback from the WASM RGB sample. The latter completes 4 px, renders direct/indirect frames,
checks a center pick against known dataset coordinates, rejects an out-of-viewport pick and
renders after recentering. A 30-second deadline records stalled workers. The clear worker records
uncaptured errors and device loss. These are diagnostic events, not a test of host recovery.

Playwright MCP, Chrome 155, viewport 1280×900, launch arguments:
`--enable-gpu --enable-unsafe-webgpu --enable-features=Vulkan --use-angle=vulkan --disable-vulkan-surface`.
The raw adapter reports AMD `rdna-3`; red RGBA readback is `[255,0,0,255]`. The WASM sample reports
4,096 voxels and center hit index 2,080, voxel center `[31.75390625,31.75390625,1.79296875]`, size
0.507811963558197. The out-of-bounds miss and recentered frame pass. Both canvas regions remain
at the CSS gray background in `target/presentation-chrome.png`, despite zero page errors.
An earlier SwiftShader launch exposed `device.lost`, reason `unknown`, message
`A valid external Instance reference no longer exists.` via a temporary request-device hook.
The hooks were diagnostic only and are not added to production code. A headed launch could not
start because the Playwright service has no X server. These failures do not establish a visor
shader/HZB bug: an independent clear with no Rust fails presentation too.

Firefox 157.0.1, fresh WebGPU-enabled profile, same fixture and viewport, driven by BiDi:
BGRA red readback `[0,0,255,255]`, same 4,096 voxels and exact center hit. Screenshot
`target/presentation-firefox/final.png` visibly contains the red clear and colored voxel scene.
The fixture validates direct WASM picking/readback and recentering on this small sample;
CSS-to-framebuffer interaction, host API/event handling, large-cloud latency and GPU recovery
remain pending. Chrome visual validation stays open; no render fallback or shader changes were
introduced based on the environment-specific symptom.

Chromium documents `--enable-gpu` for headless hardware use and notes Linux backend/display
constraints: [headless GPU guidance](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/docs/gpu/using-gpu-hardware-in-headless-chrome.md).
The launch options follow [Chrome's WebGPU headless guidance](https://developer.chrome.com/blog/supercharge-web-ai-testing).
Those options are not a guarantee that worker canvases present in this environment.

Validation: both diagnostic result branches pass again after adding exact center-coordinate
assertions. Chrome reports HZB active before picking; the recentered frame rebuilds the view.
Playwright also checks the fixture at 390×844 with no horizontal overflow. There are zero page
errors in the fixture. All 26 existing Node tests pass, worker syntax and local Markdown targets
check successfully. No Rust production code changed, so the prior Rust evidence was not rerun.

### Reproducible progressive process-tree memory — 13-10-2026

The committed `examples/web-host/sample-process-memory.py` replaces the profile-name-specific
sampling scripts for new measurements. It attaches to an explicit Linux parent PID and pins its
kernel starttime, samples descendants' `smaps_rollup`, records missing processes/counters, and
stops on parent exit or its duration cap. It never controls the browser lifecycle. See the
[CLI and limitations](../examples/web-host/README.md#sample-browser-process-memory-on-linux).
Enumeration and counter reads are sequential, not an atomic tree snapshot; new/short-lived
processes and between-sample peaks may be missed. `enumerated_processes_readable` describes only
the enumerated set. Partial sums are retained but excluded from the table below.

Three fresh Firefox 157.0.1 headless profiles, WebGPU enabled, HTTP disk/memory cache disabled,
OS cache not flushed, fitted 1280×720 framebuffer, 4 px, 256 MiB decoded cache. Input is
Majadas coordinates only (no sidecar, gray fallback; configured color selector remains `rgb`).
The existing `probe-cache-camera.mjs` loads the dataset, observes completion, then sends
`ArrowRight` and observes worker advancement. PSS/RSS were sampled with requested 0.5 s sleeps;
actual cadence includes sequential `/proc` reads. No build, tests or conversion overlapped.
An independent idle Chrome instance remained resident and is outside the measured Firefox tree.
WASM uses the existing release binding artifact; this series measures the current build, not
a paired before/after improvement. All byte values below are decimal bytes.

| Round | Readable / total samples | Tree PSS sampled peak | Tree RSS sum sampled peak | Initial / camera final voxels |
| --- | ---: | ---: | ---: | ---: |
| 1 | 83 / 84 | 1,001,586,688 | 1,818,447,872 | 6,397 / 6,396 |
| 2 | 64 / 65 | 1,849,248,768 | 2,664,980,480 | 6,397 / 6,396 |
| 3 | 83 / 85 | 984,955,904 | 1,801,097,216 | 6,397 / 6,396 |

Six phase exports report complete 4 px coverage, zero pending blocks, no cache evictions or
rejections, and zero browser console errors. Each camera phase acknowledges input and advances
scene builds from 98 to 101; it needs no additional block loads. The three final screenshots
show the gray voxel cloud (third image inspected). All sampler runs end with `root_exited`;
non-readable samples occur during shutdown and are excluded from maxima.

Round 2's PSS maximum occurs at 31.20 s elapsed, with **1,451,728,896 B** in the parent and
**1,657,601,024 B** anonymous PSS across the tree. This attributes the observed excess to a
process/mapping category, not to JS, WASM, driver or application allocations. It does not
reproduce or explain the earlier Albuera 6.58 GiB event. PSS apportions shared resident pages;
RSS sums double-count shared pages. Neither is dedicated GPU memory. Exported `uploadedBytes`
is cumulative upload traffic and must not be interpreted as live VRAM.

The DOM panel is behind all six terminal exports (`domMatchesExport=false`); the camera export
records worker advancement but its target timer retains the initial load's value. These are
fresh incomplete-window diagnostic exports, not valid FPS samples, synchronized panel captures,
or independent camera target-latency measurements. Progressive CPU process memory is now
reproducibly sampled; JS/WASM heap separation, GPU allocations and sub-second peaks remain open.

Raw JSONL, per-phase exports, screenshots, profiles and summary: ignored
`target/progressive-process-memory/majadas-{1,2,3}/`. Reproduce with a fresh dedicated Firefox
profile per round using the documented camera probe; attach the sampler before starting the
probe, then exclude records with `enumerated_processes_readable=false` when computing full-tree
sampled maxima. Validation: nine Python tests pass (three new tests exercise tree exclusion,
PID identity, partial reads and malformed counters); Ruff check/format pass for the two new files.

### Requested GPU resources and resize accounting — 13-10-2026

`SurfaceRenderer::resource_stats()` and WASM `gpuStats().requestedResources` now expose current
owned buffer sizes and logical texture bytes separately from cumulative upload traffic. This
is application resource accounting, not measured VRAM or a transition peak. The existing GPU
budget model is unchanged. See [scope and exclusions](../examples/web-host/README.md#requested-gpu-resource-sizes).

Release WASM/bindings rebuilt offline, Firefox 157.0.1 headless, WebGPU enabled, presentation
fixture with RGB sidecar at 480×360. Existing rendering/picking checks pass together with new
assertions for decimal-string counts, category sum, exact depth extent, unchanged instance
capacity on resize and restoration of the original viewport. Observed bytes:

| Resource | Loaded sample, HZB active | After pick/recenter | Resize to 240×180 |
| --- | ---: | ---: | ---: |
| Instance buffers | 163,840 | 163,840 | 163,840 |
| Fixed buffers | 376 | 376 | 376 |
| HZB buffers + count readback | 16,440 | 36 | 36 |
| Logical depth texture | 691,200 | 691,200 | 172,800 |
| Logical HZB mip pyramid | 1,398,100 | 1,398,100 | 349,524 |
| Total requested | 2,269,956 | 2,253,552 | 686,576 |

Upload traffic is 374,680 / 421,680 / 421,680 B in those states: clearly a different quantity.
HZB accounting reads actual optional buffer sizes, including the minimum one-index allocation
for a zero-instance prepared scene and params retained after reset. The native HZB GPU test
checks creation, preparation, reset and zero-instance preparation without changing budgets.

Validation: renderer/WASM tests **86 passed, 3 ignored**, Node **26 passed**, Python **9 passed**,
Clippy native all-target/WASM with warnings denied, formatting, release WASM rebuild and local
package regeneration pass. Firefox BiDi presentation/picking/resource/resize assertions pass.
Playwright MCP Chrome with `--enable-unsafe-webgpu --use-angle=swiftshader` instead reports
`RuntimeError: unreachable`; independent JS clear also loses its device with
`A valid external Instance reference no longer exists`. Chrome therefore remains unvalidated
for these counters; that run does not isolate a cause or demonstrate a new renderer regression.

Raw Firefox JSON/screenshot and Chrome screenshot: ignored `target/gpu-resource-probe/`.
Next: collect these counters through progressive large-dataset loads and explicitly keep
resident CPU PSS, requested GPU bytes and unobservable physical VRAM separate. No combined
physical-memory total is inferred by adding them.

### Progress-only panel and GPU export synchronization — 13-10-2026

Reproducer: the preceding three Majadas loads/camera phases had complete fresh exports but
`domMatchesExport=false` in all six terminal captures. `lodProgress` updated `latestLodStats`
and benchmark refinement counters, while the panel and `latestGpuStats` waited for periodic
`stats` messages (once every 30 rendered frames). Stopping at target could leave those values
behind indefinitely. Fresh Blob capture alone did not synchronize the contained GPU counters.

The worker now includes its current `gpuStats()` and `stats()` with each emitted LOD progress
message. The host uses that same message to refresh the panel and stored benchmark GPU stats.
Progress notifications are not acknowledged/counted as frame replies; real frames retain their
existing accounting. Existing refinement-ID filtering still rejects stale refinement messages.
The worker progress key and periodic frame-stat cadence are unchanged.

A new worker regression fails before the change (`undefined !== '4'` for resource bytes), then
passes for opening and color refinement without a periodic stats frame. Playwright MCP tests
the real host DOM/export with a deterministic Worker at **1280×900** and **390×844**: a terminal
progress message replaces stale pending/upload/resource counters, export agrees, and progress
does not create FPS frame replies. HTTP cache is disabled for this test. A first persistent-
context attempt used old cached host code and failed; the uncached run passes with zero
console/page errors. This mock covers host synchronization, not Chrome GPU presentation.
The committed callable Playwright snippet is `examples/web-host/tests/progress.browser.mjs`;
run it through MCP `browser_run_code_unsafe` with that filename while serving on port 8767.

Three fresh Firefox profiles then repeat actual Majadas XYZ load and `ArrowRight` at 1280×720,
4 px, 256 MiB decoded cache, gray fallback, release WASM, HTTP caches disabled and OS cache not
flushed. Linux tree memory uses 0.5 s requested sampler sleeps. All six final exports now have
`domMatchesExport=true`, complete coverage, zero pending blocks, and zero console errors.
Initial/camera scenes remain **6,397 / 6,396** voxels; no additional camera block loads.

| Round | PSS sampled peak (B) | RSS sum sampled peak (B) | Readable / total samples |
| --- | ---: | ---: | ---: |
| 1 | 967,474,176 | 1,786,269,696 | 82 / 84 |
| 2 | 1,028,718,592 | 1,849,253,888 | 83 / 85 |
| 3 | 1,889,075,200 | 2,704,953,344 | 63 / 65 |

At every final initial export, requested GPU resources are **15,153,112 B**: instances
255,880; fixed buffers 376; HZB buffers/readback 25,644; depth 3,686,400; mip pyramid
11,184,812. Camera exports request **15,153,108 B**, with one fewer visible index. The maximum
among the 10-second export snapshots is 15,153,112 B in each round; this cadence cannot measure
transient replacement peaks. Round 3's process PSS peak at 31.24 s includes 1,491,428,352 B
in the Firefox parent. CPU PSS and requested GPU bytes are distinct scopes and times; do not add
them into a physical-memory peak. No sampled/unsampled or before/after performance ratio is
claimed, and the Chrome mock ran around this diagnostic series rather than under a controlled
benchmark-isolation protocol.

Raw series: ignored `target/progressive-resource-memory/majadas-{1,2,3}/`. Validation: **27 Node
and 9 Python tests pass**, JS syntax and diff checks pass; offline package regenerated. No Rust
changed in this correction; prior Rust/WASM validation applies to the same binding artifact.
Next: broaden GPU resource snapshots to larger cuts and isolate transient parent memory with
higher-frequency capture; physical VRAM/JS heap separation remains unavailable here.

### Albuera GPU snapshots and transient parent mapping capture — 13-10-2026

The process sampler now accepts `--root-pss-capture-mib`: its first readable parent PSS
sample crossing the threshold triggers one identity-checked raw `smaps` capture. The companion
file is created exclusively; failed attempts are recorded and not retried. Capture timestamps
are separate from the triggering tree sample. Default sampling remains unchanged, without
mapping capture. [Command and limits](../examples/web-host/README.md#capture-a-transient-parent-process-mapping-event).

Three fresh Firefox 157.0.1 headless profiles, WebGPU enabled, Albuera coordinates only
(gray fallback), fitted 1280×720, 4 px, 256 MiB decoded cache; HTTP disk/memory cache disabled,
OS cache not flushed. Existing release WASM and host at `1c0ef45`, plus the sampler change.
Requested sampler sleep 0.1 s; actual median sample spacing **0.125 / 0.141 / 0.127 s**.
Root capture threshold **1 GiB PSS**. Each round uses the existing load/camera probe; all six
exports report target completion, zero pending blocks, matching panel/export, 17,085 voxels,
zero cache evictions/rejections and zero console errors. The camera needs 12 additional loads
(746→758); cache accounting rises from 203,065,440 to 207,501,472 B.

| Round | Tree PSS sampled peak (B) | Tree RSS sum sampled peak (B) | Readable / total samples | Root mapping capture |
| --- | ---: | ---: | ---: | --- |
| 1 | 1,009,138,688 | 1,827,762,176 | 1,224 / 1,226 | No threshold crossing |
| 2 | 3,823,155,200 | 4,648,906,752 | 884 / 886 | Captured at 6.078 s |
| 3 | 1,005,690,880 | 1,827,336,192 | 1,127 / 1,132 | No threshold crossing |

All sampler runs end with `root_exited`. Requested GPU resources at every terminal initial
and camera export are **15,623,424 B**: instance buffer 683,440; fixed buffers 376; HZB
buffers/readback 68,396; depth texture 3,686,400; HZB mip pyramid 11,184,812. The largest value
among the ten-second export snapshots is also 15,623,424 B in each round. This is current
application-requested storage, not VRAM, a per-build maximum or a synchronized CPU/GPU peak.
The series is a diagnostic with active external sampling, not a before/after timing comparison.

Round 2 crosses the parent threshold early: at 6.078 s tree PSS is 1,411,553,280 B and parent
PSS 1,089,865,728 B. Its mapping read takes ~16.0 ms and sums **1,094,124,544 B PSS**, at a later
time than the trigger. It contains **2,574 VMAs**, including **1,675 unnamed anonymous maps**
with 985,608,192 B PSS. Notably **161 maps of 3,686,400 B** account for **593,510,400 B PSS**;
that map size equals `1280 × 720 × 4`. This is evidence consistent with framebuffer-sized
storage, not proof of allocation ownership, driver behavior or a renderer leak.
At 120.292 s tree PSS reaches 3,823,155,200 B, parent PSS **3,307,372,544 B**. The one-shot
capture was already consumed: it does **not** capture those later peak mappings or explain
the earlier historical Albuera 6.58 GiB event.

Local code audit: `SurfaceRenderer::render` obtains a surface texture, submits and presents
without keeping a frame history. The actual `wgpu 24.0.5` web backend calls browser
`get_current_texture`; its `present` and `texture_discard` are no-ops because browser presentation
is automatic (`wgpu-24.0.5/src/backend/webgpu.rs:3718–3755` in the local Cargo registry).
Native `wgpu-core` presentation/latency semantics do not establish what Firefox retains.
The requested-resource counters intentionally exclude swapchain and driver resources.
Next discriminating experiment: clear-only WebGPU with no WASM/dataset, fixed frame count at
1280×720 and 640×360, separate fresh processes and mapping snapshots. Check whether anonymous
map-size classes follow drawing dimensions before considering a renderer change.

Raw series, source/binary hashes, mapping summaries and profiles: ignored
`target/albuera-resource-memory/`. Validation: **11 Python tests**, Ruff check/format and
CLI smoke (one capture, companion file, duration cap) pass. Tests cover root identity changes
before/during reads and refusing to overwrite a mapping capture. No UI/Rust behavior changed.

### Independent clear-only surface memory matrix — 13-10-2026

New bounded diagnostic: `examples/web-host/tests/surface-memory.html` and its worker submit
**300 clear-only WebGPU frames**, with no WASM, dataset, readback or retained texture history.
The host schedules one frame per animation callback after the previous worker reply, then
waits for `queue.onSubmittedWorkDone()`. Device/context stay alive during a 20 s idle interval.
Dimensions and frame count are explicit URL parameters, validated before creating a worker.
The CSS canvas stays **640×360** while the drawing dimensions change.

Six fresh Firefox 157.0.1 headless profiles, WebGPU enabled, HTTP disk/memory cache disabled,
1280×900 host viewport, three runs at each drawing size. Linux process-tree sampler sleeps
0.1 s between sequential reads. Identity-checked parent `smaps` captures are taken after
submission completion and again after 20 s idle. `Rss`/`Pss` are resident process memory;
frame submission counters are not display FPS. No build/conversion overlaps the series.
All runs finish 300 submissions and remain idle without a reported device failure. First
1280×720 screenshot inspected: visible red clear. Browser logs include shared-texture/dma-buf
fallback warnings; adapter identity and physical GPU memory are unavailable.

Bytes below are decimal. Anonymous-class counts include only unnamed VMAs whose `Size`
exactly equals drawing width × height × 4; this is an observational filter, not ownership proof.

| Drawing size / round | Parent PSS mapping sum after submissions | After 20 s idle | Exact image-size unnamed maps, both captures | Tree PSS sampled peak |
| --- | ---: | ---: | ---: | ---: |
| 1280×720 / 1 | 475,248,640 | 378,998,784 | 3 | 784,788,480 |
| 1280×720 / 2 | 507,559,936 | 386,115,584 | 4 | 797,125,632 |
| 1280×720 / 3 | 509,406,208 | 388,168,704 | 5 | 802,817,024 |
| 640×360 / 1 | 497,801,216 | 377,666,560 | 0 | 802,546,688 |
| 640×360 / 2 | 531,995,648 | 370,039,808 | 0 | 815,455,232 |
| 640×360 / 3 | 481,779,712 | 379,010,048 | 0 | 775,620,608 |

The large-image maps contribute **11,059,200 / 14,745,600 / 18,432,000 B PSS**, respectively,
and remain through idle. The small-image filter searches for 921,600-B VMAs and finds none;
allocation rounding/coalescing can change map boundaries, so this does not show that small
surfaces allocate nothing. Total process memory does not scale linearly with framebuffer size.
The **161** large-image maps seen during Albuera are not reproduced by this isolated clear
protocol. It neither absolves nor implicates browser presentation under the viewer's different
workload. A later screenshot is included in each sampled process lifetime; parent mapping
captures, rather than full-lifetime maxima, are the controlled phase comparisons here.

Raw driver, launcher protocol, JSONL, phase markers/captures, summaries, profiles and screenshots
are under ignored `target/surface-memory-probe/`; no browser sampling code is added to product
flows. The original six-run series requested its screenshot before the separate mapping reader
finished, so its nominal idle maps may overlap screenshot activity; treat those phase values as
observations, not a strictly ordered comparison. Reproduce manually: serve the root on 8767, start a fresh WebGPU-enabled Firefox profile,
attach the process sampler, and open
`/examples/web-host/tests/surface-memory.html?width=1280&height=720&frames=300` (then 640/360 in
another fresh profile). Capture parent mappings at `window.surfaceMemoryResults.state ===
"idle"` and 20 s later using `capture_root_smaps` from the committed sampler. Keep drawing
and reported CSS dimensions, phase timestamps and partial samples in the results.

Validation: worker JS syntax and diff checks pass; Firefox executes all six real GPU sequences.
Playwright MCP checks invalid parameters in desktop/mobile viewports with zero console/page
errors, without creating a GPU worker. This validates the diagnostic error path, not Chrome
GPU rendering. Product renderer/WASM source and package are unchanged.
Next: repeat the same bounded frame protocol with the small WASM sample, separating viewer
rendering from large-data refinement before attempting a memory fix.

### Fixed small WASM sample surface matrix — 13-10-2026

The bounded surface diagnostic adds `mode=viewer`, keeping `mode=clear` as default. Viewer
mode loads only the committed 4,096-point sample and RGB sidecar, fits once, completes its
4 px cut, then renders the unchanged scene for 300 frames. There is no progressive large-data
loading or camera motion during the measured frame sequence. Both modes retain their device
and wait for the actual browser queue's `onSubmittedWorkDone()` before reporting idle.

The local `wgpu 24.0.5` web backend's `on_submitted_work_done` is unimplemented
(`src/backend/webgpu.rs:2591–2593`), so the diagnostic temporarily wraps browser
`GPUAdapter.prototype.requestDevice` while creating its WASM viewer, captures the returned
device, and restores the prototype in `finally`. This changes only the diagnostic worker,
not Rust, public viewer APIs or product behavior. Common device-loss/uncaptured-error listeners
are installed for both modes. No per-frame readback or texture history is retained.

Three fresh Firefox 157.0.1 profiles per drawing size, WebGPU enabled, HTTP caches disabled,
1280×900 host viewport, constant **640×360 CSS**, same frame/20 s idle protocol and external
0.1 s requested sampler sleeps as the preceding clear matrix. OS cache is not flushed; the
series is sequential, not an alternating before/after performance benchmark. All six cases
complete 300 `presented` frame calls, browser queue completion and idle with **4,096 voxels**.
Third large-case screenshot inspected: visible RGB sample. All sampler runs end on root exit.

| Drawing size / round | Parent PSS mapping sum after submissions (B) | After 20 s idle (B) | Exact image-size unnamed maps, both captures | Tree PSS sampled peak (B) |
| --- | ---: | ---: | ---: | ---: |
| 1280×720 / 1 | 503,224,320 | 400,617,472 | 5 | 809,208,832 |
| 1280×720 / 2 | 504,192,000 | 400,592,896 | 6 | 810,493,952 |
| 1280×720 / 3 | 522,736,640 | 398,774,272 | 6 | 827,791,360 |
| 640×360 / 1 | 410,411,008 | 379,948,032 | 0 | 780,064,768 |
| 640×360 / 2 | 742,398,976 | 646,740,992 | 0 | 1,008,772,096 |
| 640×360 / 3 | 541,259,776 | 385,409,024 | 0 | 831,104,000 |

Large-case exact map classes contribute 18,432,000 / 22,118,400 / 22,118,400 B PSS. As in
clear mode, the small-size exact filter finding zero is not absence of surface memory:
round 2 has an elevated total parent footprint without those exact maps. Requested renderer
GPU resources are **15,051,868 B** at 1280×720 and **3,898,460 B** at 640×360, a separate scope
from resident CPU PSS. The large-data **161-map / multi-GB** event is not reproduced by the
fixed small scene. This narrows the next experiment to camera/scene-update churn or the
large-data/progressive workload; it does not identify an owner or justify a renderer fix.
Full-lifetime process peaks include final screenshot activity; phase mapping captures remain
the controlled comparison. No memory or timing improvement ratio is inferred.

The initial six-run driver also requested screenshots before the mapping reader had finished,
so the reported post-idle phase could overlap screenshot activity. Those values remain useful
observations but are not a strictly ordered phase comparison. The orbit series below uses a
single driver that waits for each identity-checked map capture to finish before sleeping or
requesting the screenshot.

Raw driver/profiles/markers/JSONL/maps/summaries/screenshots: ignored `target/surface-wasm-memory/`.
Reproduce with the existing URL/protocol plus `&mode=viewer`. Validation: all six real Firefox
GPU runs, final three-frame smoke of both modes after sharing error listeners, **27 Node /
11 Python tests**, JS syntax and diff checks pass. Playwright MCP validates invalid mode in
desktop/mobile with zero console/page errors; that check does not validate Chrome GPU rendering.
Product WASM binary, renderer and offline package remain unchanged.
Next: keep the small input but move camera between fixed submissions to isolate scene-update
churn from large-data loading, using fresh profiles and the same phase captures.

### Small WASM camera-orbit surface memory matrix — 13-10-2026

The surface probe adds `mode=orbit`: with the same 4,096-point RGB sample, each of 300 submitted
frames first rotates the camera by 0.01 radians and rebuilds the completed 4 px target cut, then
renders. This deliberately measures repeated camera/scene updates. All six fresh Firefox 157.0.1
profiles (three per drawing size) complete 300 orbit calls and frames, 903 LOD build/scene-build
calls, 902 unchanged-cut builds, one loaded block, zero additional reads, and a complete 4 px cut
with 4,096 rendered voxels. This is a diagnostic CPU-work count, not evidence of a speedup or a
browser FPS result.

The reproducible `examples/web-host/probe-surface-memory.mjs` driver owns a dedicated Firefox
BiDi session on port 9222 and requires the repository server on port 8767, Node 22+, Python 3,
and the committed sampler. It captures parent maps synchronously after queue completion, sleeps
20 seconds, captures them again, then takes the screenshot. Each new output directory is
exclusive; logs, session/root identity, both raw maps and capture metadata, screenshot timing,
console errors, and screenshots are retained. All sampler runs end after Firefox exits. The six
profiles have empty browser error logs. A final 320×180 three-frame smoke of the final driver
also passes with ordered captures and an empty error log.

| Drawing size / round | Parent PSS after submissions (B) | Parent PSS after 20 s idle (B) | Exact image-size unnamed maps | Target-cut voxel build CPU time (ms) | Tree PSS sampled peak (B) |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1280×720 / 1 | 412,524,544 | 471,827,456 | 5 | 21,065 | 830,565,376 |
| 1280×720 / 2 | 413,454,336 | 466,115,584 | 5 | 20,617 | 894,105,600 |
| 1280×720 / 3 | 397,196,288 | 466,043,904 | 5 | 19,822 | 897,498,112 |
| 640×360 / 1 | 396,147,712 | 497,147,904 | 0 | 13,668 | 903,543,808 |
| 640×360 / 2 | 397,204,480 | 518,950,912 | 0 | 12,754 | 804,446,208 |
| 640×360 / 3 | 398,015,488 | 505,359,360 | 0 | 13,127 | 863,605,760 |

Large exact-size maps contribute 18,432,000 B PSS at both phase captures. The small-image
filter finds no 921,600-B mappings; this does not imply the surface has no memory. In every run,
parent PSS is higher after the idle interval than immediately after queue completion, even
though capture order is explicit. The cause is unknown. Full-lifetime tree peaks include later
screenshot activity and are not the phase comparison. No run reproduces Albuera's 161 exact-size
maps or the multi-GB event. Requested GPU bytes, CPU PSS, WASM linear memory, and physical VRAM
remain distinct or unavailable measurements.

Raw six-run series: ignored `target/surface-orbit-serial-memory/`. Earlier orbit runs under
`target/surface-orbit-memory/` are preliminary because their map read and screenshot could
overlap. An attempted stdin-handshake series is incomplete and excluded. The permanent driver
avoids that race/deadlock by performing each map read synchronously before its next action.
Playwright MCP exercises the orbit parameter with a deterministic worker at desktop and mobile
viewports; actual Firefox runs exercise WebGPU. Validation: **27 Node tests, 11 Python tests**,
JavaScript syntax, CLI rejection/overwrite checks, six complete Firefox runs and the final smoke
pass; `git diff --check` passes. Chrome GPU remains unvalidated.

Next: prototype a one-shot selection of the actual target cut from resident blocks during camera
invalidation, publishing it only if coverage and all budgets are complete; otherwise preserve
the existing progressive coarse-to-fine fallback. Measure paired camera runs before claiming
reduced construction or memory.

### Resident target cut on camera invalidation — 13-10-2026

When a camera change resets the progressive stage, the binding now probes the requested target
cut against the current decoded-block cache. It publishes that cut directly only when the plan has
no pending blocks, uncovered nodes, selection-budget omissions or transition-budget omissions.
The probe neither loads blocks nor builds/uploads a scene. If the target is not complete, the
existing coarse-to-fine loader path runs. A complete resident plan still goes through the normal
voxel builder and renderer budgets; a fine build error retries at the coarse stage. The accepted
stage is used for subsequent progression so a successful direct target does not schedule an
unnecessary intermediate stage. No cache, scene or format metadata was added.

Three new Firefox 157.0.1 profiles repeat the fixed 4,096-point RGB sample at 1280×720 drawing
size, 640×360 CSS size, target 4 px, 300 camera orbits/frames, and the queue-completion plus
20-second idle protocol. The previous six-run orbit matrix supplies three same-size baseline
rounds with the identical diagnostic. Every new run ends with 4,096 rendered voxels, complete
coverage, no pending blocks, one initial block load, zero camera-time reads and zero console
errors. All request **15,051,868 B** of steady renderer resources.

| 1280×720 run | Baseline scene builds | Direct-target scene builds | Baseline voxel-build CPU (ms) | Direct-target voxel-build CPU (ms) | Baseline cumulative uploads (B) | Direct-target cumulative uploads (B) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 903 | 303 | 21,065 | 8,147 | 147,115,800 | 49,643,520 |
| 2 | 903 | 303 | 20,617 | 11,543 | 147,115,800 | 49,643,520 |
| 3 | 903 | 303 | 19,822 | 9,253 | 147,115,800 | 49,643,520 |

Median measured voxel-build CPU time changes from **20,617 ms to 9,253 ms** (55.1% lower);
scene builds fall by two thirds. The new selection/planning counters include the resident-target
preflight. Cumulative upload traffic falls with the avoided full-scene publications; current
requested GPU allocation stays the same. These sequential profile runs use the same browser,
settings and input, but are not alternating paired runs and the OS cache is not flushed. They
support the narrow construction-count and cumulative-work observation, not a general FPS,
end-to-end latency or memory-resident improvement claim. GPU/CPU build-budget failure was not
fault-injected; coarse fallback on a newly loaded sample is exercised in all runs.

Raw before data remains under `target/surface-orbit-serial-memory/`; the three new profiles,
identity-checked parent-map snapshots and screenshots are under ignored
`target/surface-orbit-fastpath/`. Validation: **29 viewer_wasm tests pass, 1 ignored**;
workspace native and `viewer_wasm` WASM-target Clippy, WASM target check, release WASM rebuild,
bindings/package regeneration, and three actual Firefox WebGPU runs pass. Rust format and
`git diff --check` pass.

The resident-plan gate also has a synthetic multiblock regression using the real hierarchy
selector and transition planner. A complete Replacement child cut is accepted even when its
unused parent payload is absent. A Replacement cut with a cached parent fallback is rejected
when a target child is pending; an Additive cut with the same missing child is rejected with an
uncovered region. Further cases reject selection-limited cuts and planner plans that omit nodes
because cached payload point counts exceed the target budget. These tests validate the gate's
inputs, not GPU scene publication or failure recovery.

The voxel-count construction failure is now covered by the regression below. Estimated-memory
and GPU-upload failure recovery remain separate unverified cases. A browser-level partially resident multiblock camera
run should also verify cache protection and publication.

### Resident construction budget fallback — 13-10-2026

`WasmVoxelViewer.createOffscreenWithVoxelLimits(canvas, voxelSize, maxVoxels, maxEstimatedBytes)`
adds immutable CPU construction limits at creation. Both limits are positive `u32` values; bytes
are estimated construction memory, not RSS or GPU allocation. `createOffscreen` and host defaults
remain unchanged. Initialization is shared, and the effective voxel limit is still capped by the
renderer GPU instance budget. No live budget mutation or build-context invalidation was added.

The persistent worker probe is `examples/web-host/tests/lod-budget-fallback.browser.mjs`.
It creates a one-block external dataset with eight points in `[0,1]^3`:
`[0.51 + i * 0.004, 0.51, 0.51]`, `i=0..7`. Both viewers use a 320×240 OffscreenCanvas,
`voxelSize=0.6`, and fitted camera. The default viewer builds eight voxels at 0.5 px. A second
viewer has a one-voxel limit and 64 MiB estimated construction budget; its 16 px scene fits.
A resize to 321×240 invalidates the previous build context so fallback must construct a new
coarse scene rather than reuse the old one. Requesting 0.5 px then returns 16 px with two added
scene-build attempts: fine construction fails and coarse construction succeeds. Coverage remains
complete, all eight source points stay resident, and one voxel remains visible.

Further explicit refinement reaches `voxelized scene exceeds the 1-voxel instance limit`;
rendering still presents the previous cut. Requesting 16 px recovers. The cache has exactly one
insertion and no automatic block loads. Zero constructor limits are rejected before creating a
GPU surface. This external session intentionally requires caller-driven refinement; the probe
verifies the synchronous error, not automatic `lodError` reporting for a file-backed session.

Rebuild release WASM, regenerate `pkg` with wasm-bindgen and run the root HTTP server as documented
in `examples/web-host/README.md`. From a browser page on that origin, create a module Worker for
`/examples/web-host/tests/lod-budget-fallback.browser.mjs`, post `{}`, and await `probeResult`.
Playwright MCP ran the worker with Chrome 155.0.8059.39 and flags `--enable-unsafe-webgpu`,
`--use-angle=swiftshader`, `--enable-features=Vulkan`, `--disable-vulkan-surface`, and
`--disable-gpu-sandbox`. The probe passed with `fineVoxels=8`, `coarseVoxels=1`, `fallbackBuilds=2`.
Session, attribute selection and reuse probes also passed; no uncaught JavaScript errors.
An earlier Chrome launch without explicit Vulkan flags could not create a surface-compatible
adapter and is not counted as a pass.

This closes the resident fine-build fallback check for a real voxel-count error. It does not
validate physical GPU behavior, Firefox, estimated-memory or upload failure recovery, frame
latency, RSS or large partially resident multiblock cuts. Native/WASM Clippy and checks, format,
255 Rust tests (3 optional ignored), 27 Node tests, 11 Python tests and regenerated local package
passed. There are no performance claims from this functional probe.

### Majadas multiblock resident-camera observation — 13-10-2026

One follow-up Firefox 157.0.1 run uses Majadas coordinates only (4,214,687,109 B, 415,465,882
source points), 1280×720 framebuffer, target 4 px and the existing 256 MiB cache. The initial
completed cut has 239 loaded blocks, 202 visible blocks, 6,397 voxels and **112,290,672 B** of
decoded cache. A trusted 30 CSS-pixel right drag is acknowledged; the fresh camera export has
complete coverage, zero pending blocks, the same 202 visible blocks and 6,397 voxels, no new
block loads/range reads, no cache evictions, and `sceneBuilds` increases **98→100**. Thus this
cut was multiblock and smaller than the full dataset while all payloads required by the visible
target remained resident. The one observation checks functional camera publication on a real
dataset; it is not a before/after count comparison, memory sample or FPS benchmark.

The cache/camera driver now uses a trusted BiDi pointer drag. It starts the host's 15-second
continuous-render window before moving, because LOD progress suppresses unchanged-stage messages
and periodic worker counters otherwise wait for every 30th frame. The window exists only to flush
current counters; `steadyState.cameraChanged` makes its FPS unsuitable for comparison. Three
earlier tries without the sustained frame window are excluded: their host received input, but the
fresh export retained the initial worker counters. The final sustained run has zero browser
console errors. The committed driver syntax-checks; **27 Node and 11 Python tests pass**.

Raw valid capture: ignored `target/resident-camera-majadas/sustained/`. This leaves one key case
open: induce a fine scene build failure under a controlled renderer budget, confirm the previous
scene remains visible, then confirm the coarse retry succeeds. Repeatable before/after camera
counts on this multiblock dataset also remain open.

### Worker WASM linear-memory snapshots — 13-10-2026

Assessment: `environment.jsHeapUsedBytes` in exported runs was sampled on the **main thread**
using nonstandard `performance.memory`; it never represented the viewer worker. The native
RSS diagnostic cannot supply browser heap attribution. Add a separate worker snapshot without
changing renderer ownership, allocation limits or Rust bindings.

Implemented contract:

- Worker `stats`, `lodProgress` and API `getStats()` carry `memory` with
  `wasmLinearMemoryBytes` and `workerJsHeapUsedBytes`.
- The first value reads the instance returned by wasm-bindgen `init()` and its current
  `memory.buffer.byteLength` after the viewer stats calls. It is linear-memory **capacity**,
  including unused/free allocator space, stack and static data; it is not live Rust allocations.
  The worker reads the current buffer each time, so growth cannot leave a detached cached view.
- `workerJsHeapUsedBytes` is explicitly `null`: this change does not measure worker JS heap.
  See the [WebAssembly JS interface specification](https://webassembly.github.io/spec/js-api/#dom-memory-buffer)
  for the memory buffer contract and [MDN's performance.memory limitations](https://developer.mozilla.org/en-US/docs/Web/API/Performance/memory)
  for why a main-thread reading does not cover workers.
- Run JSON adds `memory.latestViewerWorkerSnapshot` and `memory.mainThreadJsHeapUsedBytes`.
  The snapshot is the latest accepted progress/periodic stats message, potentially older than
  export time, with no history retained. The legacy `environment.jsHeapUsedBytes` stays compatible
  and describes the same main-thread scope. None of these metrics may be added to PSS/RSS or GPU
  estimates to calculate a total; no continuous peak or allocator attribution is claimed.

Validation: Node tests grow a real `WebAssembly.Memory` from one to three then four pages and
verify API, progress and periodic stats report current capacity. All 28 JS tests and 11 Python
packaging/probe tests pass. Local package regenerated with the same retained init exports.

Playwright MCP: serve the repo with `python3 -m http.server 8000 --bind 127.0.0.1`, open
`/examples/web-host/`, open the built-in sample, run the 15 s benchmark, and export JSON.
Chrome 155.0.8059.39 with `--enable-unsafe-webgpu --enable-unsafe-swiftshader
--use-angle=swiftshader --enable-features=Vulkan --use-vulkan=swiftshader` passes:

- Desktop 1280×720: 4,096 voxels, complete coverage, worker WASM capacity 2,818,048 B.
- Main-thread heap at export 3,844,047 B, recorded separately; this is a nonstandard observation,
  not a stable memory baseline.
- A separate embeddable `createViewer` worker returns 1,310,720 B through `getStats()`.
- Resizing to 390×844 and exporting preserves the memory contract, with worker capacity
  2,818,048 B and main-thread heap 2,603,405 B. No uncaught page errors.

The regenerated offline `file://` package also completes sample load, the 15 s benchmark and
JSON export: 4,096 voxels, complete coverage and 2,818,048 B worker WASM capacity, with no
uncaught page errors. Its embedded WASM/base64 JS footprint is separate from the modular host.

The default MCP browser could not create a compatible GPU adapter; the explicit software
adapter run supplies functional evidence. These are contract checks, not evidence of visual
presentation, FPS gains, GPU memory or browser-wide peak. Remaining: live allocation attribution
and a real Additive dataset before considering accumulator persistence.

### Correlación acotada de memoria en la revisión actual — 24-10-2026

Se corrigió `probe-cache-camera.mjs` para aceptar exports anteriores a la primera escena,
terminar con el error real de apertura/refinamiento, registrar el intervalo de reloj de cada
export y admitir un objetivo de píxeles explícito. Antes, el `.oct` Additive histórico
`target/additive-real-v5.oct` (magia provisional) provocaba un `TypeError` al leer
`scene=null`; ahora el probe informa `openFile failed: provisional format...` y termina.
La fuente EPT local se reconvirtió a v5 vigente: 8.884 puntos y 3 bloques. El `.oct`
Majadas local también era provisional; se usó la pareja Belvedere vigente de 81.682.329
puntos y 1.343 bloques, a **8 px** porque 4 px no cerró con la caché documentada.

Dos perfiles nuevos Firefox 158.0.1 headless, WebGPU habilitado, caché HTTP deshabilitada,
sin compañero de atributos, host local y framebuffer 1282×722. El sampler Linux empezó antes
de cargar. Additive cerró a 4 px con 3 bloques y cero pendientes; Belvedere cerró a 8 px
tras cargar 389 bloques (320 visibles, 13.238.581 puntos del corte) y la cámara completó
sin nuevas lecturas. Ambos exports finales concordaron con el panel y las capturas mostraron
las nubes; el registro de consola BiDi quedó vacío. Son una ejecución por corpus, no una
distribución ni una comparación de rendimiento. Los campos de nombre/proveedor del adapter
salieron vacíos, así que tampoco identifican una GPU física.

| Corpus y fase | Caché decodificada B | Builder estimado B | Instancias capacidad B | WASM capacidad B | GPU solicitado B | PSS cercano B |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Additive inicial | 142.240 | 34.540.824 | 234.880 | 2.949.120 | 15.130.012 | 689.075.200 |
| Additive cámara | 142.240 | 34.539.312 | 234.520 | 2.949.120 | 15.129.976 | 772.230.144 |
| Belvedere inicial | 211.910.672 | 34.420.536 | 205.600 | 225.705.984 | 15.097.804 | 944.612.352 |
| Belvedere cámara | 211.910.672 | 34.420.536 | 205.600 | 225.705.984 | 15.097.804 | 1.019.762.688 |

Los PSS cercanos provienen de la muestra completa más próxima al export, a menos de 90 ms;
el intervalo de export está en `captureWallTimeMs` y cada muestra lleva `wall_time_ns`.
La tabla muestra el último corte aceptado, no un snapshot atómico de sus asignaciones
transitorias durante el build.
El pico PSS observado del árbol fue 781.960.192 B en Additive y 1.024.223.232 B en
Belvedere; el RSS sumado alcanzó 1.576.886.272 y 1.818.705.920 B, respectivamente.
En Belvedere el mayor PSS llegó **entre** los exports inicial y de cámara mientras caché,
capacidad WASM y recursos GPU solicitados permanecían iguales en esos dos exports. No se
puede atribuir ese aumento a JS, WASM, superficies, driver ni una reserva concreta. El
sampler recorre procesos secuencialmente y omite picos entre lecturas. Belvedere requirió
dos archivos de muestreo solapados porque la carga duró unos 260 s; la superposición añade
algo de coste al proceso y no es una segunda ronda independiente.
El RSS sumado cuenta páginas compartidas varias veces; PSS reparte ese coste.

El benchmark nativo actual, con la misma pareja de entrada y tres construcciones, observó
por separado el payload de la **escena anterior**: 234.880 B en Additive y 205.600 B en
Belvedere. Sus modelos `decoded + previous scene + builder` fueron 35.059.016 y
246.582.096 B tras la primera construcción; los RSS máximos muestreados del proceso
nativo fueron 5.382.144 y 225.562.624 B. Additive produjo 5.872 vóxeles y Belvedere
5.140, coincidiendo con los cortes iniciales del navegador; el viewport nativo es
1280×720, por lo que no se comparan sus modelos byte a byte con los del navegador.
El modelo es conservador y no equivale a RSS. WASM es capacidad lineal, incluye espacio libre
y ya forma parte de PSS; GPU son bytes de recursos solicitados, no VRAM física. No se suman.

Para repetir: preparar la fuente Additive como en
[la prueba EPT](VOXEL_BUILD_BENCH.md), reconvertirla
con `viewer_convert --format-version v5`, y usar la conversión Belvedere de
[LOD regional](REGIONAL_VOXEL_LOD.md). Tras reconstruir
WASM/bindings y servir la raíz en 8767, iniciar un perfil Firefox nuevo y
`sample-process-memory.py FIREFOX_PID OUTPUT.jsonl --interval 0.25 --seconds 300` antes del
probe. Ejecutar `probe-cache-camera.mjs INPUT.oct OUTPUT_DIR` para Additive; para Belvedere,
añadir `http://127.0.0.1:8767/examples/web-host/index.html 8` tras `OUTPUT_DIR`.
El benchmark nativo usa `voxel_build_bench INPUT.oct 3 256 TARGET_PIXELS 1`.
Salidas brutas ignoradas: `target/combined-memory-additive-timed/`,
`target/combined-memory-belvedere-8/` y `target/combined-memory-*-native.json`.
Al cerrar esa primera tanda quedaban pendientes rondas independientes, Additive ramificado
representativo, atribución del pico y medición de asignaciones transitorias del viewer/GPU.

### Repeticiones y comparación del pico conjunto — 24-10-2026

Se repitió la carga/cámara con perfiles Firefox 158.0.1 headless frescos, mismo `.oct`, host,
1282×722, 256 MiB de caché decodificada y muestreo de proceso pedido cada 250 ms. Las repeticiones
de Belvedere usaron Firefox WebGPU sin sidecar; la de control desactivó `browser.cache.disk.enable`
y `browser.cache.memory.enable`, igual que la primera ronda. Dos ejecuciones exploratorias previas
dejaron la caché HTTP en su valor por defecto. No hubo conversión ni compilación concurrente.
Los SHA-256 de las entradas `.oct` son `937ce38451c71e11864a51f1fa973c264a18f6ec4b6b561f8cea718b23d9b00b`
(Additive) y `6ca0f86d376c79d0702225011c0f746e16cb81be9d10fb847e821ec6c45f62ac` (Belvedere).
El A/B comparó WASM release de `14105d5`
(`094fe817527404f92eeb622ab46f9c31795d5fe9c3a8d90b13c4a085baa6df5a`) y `3bc5e3d`
(`0553b709a7bc34168ff8e76a03e17c8c396286b218f8f1494d23f542c83bc6ed`).

Belvedere cerró 8 px, con 389 bloques cargados, 320 visibles, 13.238.581 puntos fuente y cero
pendientes. Las tres repeticiones con el planner actual reportaron `targetLodMs` de 249.480,
250.631 y 251.544 ms. Cada export de cámara reconoció el cambio y conservó 320 bloques; la
captura revisada mostró la nube y no hubo errores de consola. Additive cerró 4 px en las dos
repeticiones (3 bloques, 8.884 puntos fuente y cero pendientes). Es una comprobación de estado,
no una comparación de rendimiento.

| Caso | Fase / cortes | Caché decodificada B | Modelo builder B | Capacidad WASM B | GPU solicitado B | PSS cercano a export B | Máximo árbol observado PSS / RSS B |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Additive, repetición 1 | inicial → cámara | 142.240 | 34.540.824 → 34.539.312 | 2.949.120 | 15.130.012 → 15.129.976 | 704.998.400 → 779.696.128 | 875.324.416 / 1.669.693.440 |
| Additive, repetición 2, caché HTTP desactivada | inicial → cámara | 142.240 | 34.540.824 → 34.539.312 | 2.949.120 | 15.130.012 → 15.129.976 | 708.431.872 → 778.717.184 | 840.981.504 / 1.637.081.088 |
| Belvedere, planner anterior a `3bc5e3d` | inicial → cámara | 211.910.672 | 34.420.536 | 225.705.984 | 15.097.804 | 2.411.982.848 → 3.559.677.952 | 3.559.677.952 / 4.380.217.344 |
| Belvedere, planner actual, 3 perfiles (2 caché HTTP predeterminada, 1 desactivada) | inicial → cámara | 211.910.672 | 34.420.536 | 223.608.832 | 15.097.804 | 2.351.428.608–2.382.524.416 → 3.472.961.536–3.488.402.432 | 3.472.961.536–3.488.402.432 / 4.282.552.320–4.302.405.632 |

En el export final todos los cortes de Belvedere mostraron 5.140 vóxeles antes de cámara y 4.752
después; el trabajo acumulado fue igual al del registro de una ejecución (389 bloques y
1.134.496.852 puntos fuente de construcción). Los valores `cache`, builder, instancias y recursos
GPU solicitados quedaron iguales. La cámara elevó el PSS próximo al export de unos 2,35–2,41 GB
a 3,47–3,56 GB; en Additive los dos exports estuvieron alrededor de 0,70–0,78 GB.

El contraste con la primera ejecución Belvedere archivada (PSS máximo 1.024.223.232 B, RSS
1.818.705.920 B) no se reprodujo: una ejecución nueva del **planner anterior** llegó a
3.559.677.952 B PSS, y tres del actual llegaron a 3.472.961.536–3.488.402.432 B. El A/B no
apoya que `3bc5e3d` explique por sí solo el salto. Los resultados siguen mostrando una variación
grande entre sesiones con el mismo corpus/configuración; su causa queda abierta.

Se capturó `smaps` del proceso raíz Firefox al cruzar 2.500 MiB PSS en dos repeticiones actuales
y en el A/B con el planner anterior. En las tres, entre 2.476.720 y 2.504.188 KiB RSS/PSS del
proceso raíz estaban en mapeos anónimos sin nombre semántico. Esto no permite repartirlos entre
JS, WASM, WebGPU, driver u otros componentes. El muestreador periódico puede perder picos breves y
el `smaps` puntual introduce coste propio; no es una medición del pico exacto ni de VRAM. El heap
JS del worker sigue sin estar disponible.

Las salidas ignoradas quedan en `target/combined-memory-additive-repeat-{20261024,2-20261024}/`,
`target/combined-memory-belvedere-8-repeat-{20261024,2-20261024,3-20261024}/` y
`target/combined-memory-belvedere-8-ab-before-20261024/`; incluyen exports, muestras de proceso,
capturas y, en tres casos, `process-memory.root-smaps.txt`. La medida de memoria no determina si
conviene mantener un acumulador persistente. Siguen abiertos Additive ramificado representativo,
atribución del PSS y instrumentación que capture asignaciones transitorias del viewer/GPU.

### Belvedere v5 a 4 px: refinamiento bajo caché — 24-10-2026

Se repitió con `target/lod-belvedere-20261024.oct` (SHA-256
`6ca0f86d376c79d0702225011c0f746e16cb81be9d10fb847e821ec6c45f62ac`), Firefox 158.0.1
headless, WebGPU habilitado, host local, framebuffer 1282×722, objetivo 4 px y caché de
bloques de 256 MiB. Se abrió solo el `.oct`, sin compañero de atributos. El muestreador del
árbol de Firefox corrió cada 250 ms durante 600 s y registró 2.178 muestras válidas.

El primer frame grueso llegó a **28 ms** y metadatos abrieron en 18 ms. El probe alcanzó la
etapa solicitada de 4 px, pero no completó el corte: al límite de observación aún había **783
bloques pendientes** y 456 bloques cargados; `targetLodMs` quedó nulo. El export marcó
`coverageComplete=true`, que describe cobertura del corte aceptado, no la terminación del
objetivo. Se leyeron 246.273.694 B únicos de los 981.063.326 B de XYZ; las lecturas de rangos
sumaron 328.728.576 B. Hubo 87 expulsiones de caché, sin rechazos por bloque demasiado grande
ni por protección. El comando terminó con `observation_cap_600s`, no con éxito del objetivo.

El pico muestreado del árbol Firefox fue **2.587.087.872 B PSS** y **3.387.068.416 B RSS
sumado**. Son máximos observados con muestreo periódico, no picos garantizados; RSS suma páginas
compartidas más de una vez. El refinamiento tampoco produjo una ventana estable de FPS. El PSS
no puede atribuirse a JavaScript, WASM, driver, superficies u otra reserva concreta. Aunque el
PSS supera la ronda previa a 8 px, los objetivos y la duración de carga difieren: esta ejecución
no es una comparación controlada de memoria.

La salida está en `target/combined-memory-belvedere-4-current/` y el muestreo en
`target/combined-memory-belvedere-4-current-process-retry.jsonl` (ignorados por Git). Reproducir
el host con Firefox WebGPU, iniciar el sampler antes de la carga y ejecutar
`node examples/web-host/probe-cache-camera.mjs INPUT.oct OUTPUT_DIR URL 4`; para el sampler usar
`--interval 0.25 --seconds 600`. Belvedere no cerró su objetivo de 4 px. Albuera y Majadas sí
completaron 4 px en las ejecuciones actuales descritas a continuación.

### Albuera v5 actual a 4 px — 24-10-2026

Se usó la pareja v5 de la conversión actual documentada en [el perfil de conversión](V5_MEMORY_BENCH.md#conversión-v5-actual-de-albuera-uav--24-10-2026),
solo coordenadas `.oct` (sin `.octattrs`), Firefox 158.0.1 headless con WebGPU habilitado,
host local, framebuffer 1282×722, objetivo 4 px y caché configurada de 256 MiB. El perfil de
proceso empezó antes de abrir el archivo, muestreó el árbol Firefox cada 250 ms y produjo 480
muestras hasta la salida. El canvas mostró la nube en gris; los dos exports coincidieron con el
panel y la consola no registró errores.

| Fase | Resultado |
| --- | --- |
| Apertura de metadatos | 82 ms |
| Primer frame grueso | 102 ms |
| Objetivo 4 px | Completado en 115.379 ms; 746 bloques cargados, 0 pendientes |
| Lectura única al completar | 276.320.912 B de 6.105.617.040 B |
| Caché decodificada | 203.065.440 B de 268.435.456 B; 0 expulsiones y 0 rechazos |
| Corte visible | 568 bloques, 12.673.149 puntos fuente, 17.085 vóxeles |
| Fase de cámara | Cambio reconocido; terminó tras 10 s con 750 bloques cargados y 0 pendientes |

El PSS máximo muestreado del árbol fue **4.453.557.248 B** y el RSS sumado
**5.254.148.096 B**. El muestreo no garantiza capturar el pico exacto; RSS cuenta páginas
compartidas varias veces y PSS las reparte. No atribuye esa memoria al runtime, WASM, driver,
superficies o GPU. Tampoco se toma la ventana breve del probe como benchmark de FPS. Es una
ejecución por dataset y configuración; no sustituye rondas independientes ni una comparación
controlada con Belvedere, aunque cierra el objetivo observado de Albuera a 4 px.

Salidas locales ignoradas: `target/combined-memory-albuera-4-current/` y
`target/combined-memory-albuera-4-current-process.jsonl`.

### Majadas v5 actual a 4 px — 24-10-2026

Se usó la pareja v5 actual de [la conversión medida](V5_MEMORY_BENCH.md#conversión-v5-actual-de-majadas-tls--24-10-2026),
solo coordenadas `.oct` (sin `.octattrs`), Firefox 158.0.1 headless con WebGPU habilitado,
host local, framebuffer 1282×722, objetivo 4 px y caché de 256 MiB. El sampler empezó antes de
la carga y tomó 298 muestras hasta cerrar Firefox. La nube gris quedó visible en la captura; los
exports coincidieron con el panel y la consola no registró errores.

| Fase | Resultado |
| --- | --- |
| Apertura de metadatos | 58 ms |
| Primer frame grueso | 71 ms |
| Objetivo 4 px | Completado en 61.811 ms; 239 bloques cargados, 0 pendientes |
| Lectura única al completar | 102.958.469 B de 4.214.687.109 B |
| Caché decodificada | 112.290.672 B de 268.435.456 B; 0 expulsiones y 0 rechazos |
| Corte visible | 202 bloques, 7.015.449 puntos fuente, 6.397 vóxeles |
| Fase de cámara | Cambio reconocido; terminó tras 10 s sin más bloques |

El PSS máximo muestreado del árbol fue **3.731.787.776 B** y el RSS sumado
**4.535.824.384 B**. Son máximos muestreados, no picos garantizados; RSS cuenta páginas
compartidas varias veces y PSS las reparte. No permiten atribuir memoria a JavaScript, WASM,
driver o GPU. La ventana corta del probe no se interpreta como FPS estable. Esta es una ejecución,
sin repetición para variabilidad ni comparación controlada de memoria con otros corpus.

Salidas locales ignoradas: `target/combined-memory-majadas-4-current/` y
`target/combined-memory-majadas-4-current-process.jsonl`.

### Repetición de Majadas v5 a 4 px — 24-10-2026

Se repitió el probe con la misma pareja `.oct`, Firefox 158.0.1 headless/WebGPU, host local,
framebuffer 1280×720 y caché de 256 MiB. La apertura fue 58 ms, el primer frame 72 ms y el corte
objetivo se completó en 54.002 ms: 239 bloques cargados, 0 pendientes, cobertura completa y
102.958.469 B leídos. La vista de cámara terminó sin cargar bloques adicionales. La captura muestra
la nube y `console-errors.json` está vacío.

El PSS/RSS máximo muestreado del árbol Firefox fue **965.480.448 / 1.770.516.480 B**, frente a
**3.731.787.776 / 4.535.824.384 B** en la ejecución anterior. La diferencia entre dos perfiles es
grande y su causa no está identificada; no se interpreta como mejora ni como memoria del renderer,
WASM o GPU. Los máximos son muestreados, no garantizados. El proceso completó y la repetición sirve
para documentar variabilidad, no para estimar un pico representativo. Datos locales ignorados por Git:
`target/combined-memory-majadas-4-repeat-20261024/`.
