# Web viewer performance benchmark

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
