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
