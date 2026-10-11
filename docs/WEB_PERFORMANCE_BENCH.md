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

Chrome/Chromium is not installed. An exploratory Belvedere load did not reach the target
within 120 s (last reported unique range bytes: ~129.1 MiB). That attempt used cached old
host modules and is retained separately, not mixed into this corrected baseline. Its cause
is unresolved; it does not establish successful large-file browser navigation. Large datasets,
sidecar association, color changes, OPFS, cancellation, picking, actual GPU loss and headed
browser measurements remain pending.

Local raw exports and profile capabilities: `target/web-browser-probe/final-runs/`;
the one-off BiDi automation is `target/web-browser-probe/measure.mjs`. It selects local files,
uses the host's benchmark/export buttons and captures the emitted JSON Blob without changing
its payload. The server exposes only a staged copy of host modules, WASM and deterministic
sample, not the repository root or datasets; file inputs supply the local `.oct`.
Reproduce manually with the protocol above and run the collector on the three exported files.
Seven Node suites (25 tests), four Python tests and local packaging pass; the single-file
viewer was regenerated after the change. Automated tests do not certify the pending flows.
