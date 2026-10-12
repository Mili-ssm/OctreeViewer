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
| `blockLoadMs` | Synchronous loader callback: file-range access, decompression, geometry/color/selected-attribute decoding. Includes read wait; excludes cache insertion and metadata opening. Reading and decoding are not yet separated. |
| `voxelBuildMs` | Building the whole visible voxel cut, including projection and cell aggregation. |
| `uploadMs` | `upload_voxel_cut` CPU preparation and enqueue; excludes GPU completion and the subsequent frame render. |
| `updateCalls`, `blockLoadCalls`, `loadedBlocks` | Update attempts, loader invocations, and blocks successfully inserted into the decoded cache. A loader may return no block or fail. |
| `sceneBuilds`, `voxelSourcePoints` | Whole-cut build attempts and sum of their input cut source-point counts, including repeated cuts. The point sum is a decimal string; it is neither unique points nor final rendered voxels. |

Intervals do not overlap and are elapsed synchronous wall time, not CPU-cycle accounting.
They omit cache insertion, plan commit/cloning, general orchestration, frame rendering and
messaging; their sum need not equal `targetLodMs`. Native uses `Instant`; browser uses
`performance.now()` (with `Date.now()` fallback if the global Performance object is absent).
Browser timer precision can make short intervals zero. Each completed phase records elapsed
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
