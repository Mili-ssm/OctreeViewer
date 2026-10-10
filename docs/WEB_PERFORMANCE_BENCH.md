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
| `steadyState.frameReplyIntervalP50Ms/P95Ms` | Browser-observed spacing between returned frame messages; helps reveal stalls. |
| `steadyState.workerRenderCallP50Ms/P95Ms/P99Ms` | WASM/CPU `renderFrame()` percentiles during the stable sample; they exclude asynchronous GPU completion. `workerRenderCallDuringLoadMs` retains the worker's rolling percentiles from the whole load/run. |
| `scene.renderedVoxels`, cache and `environment.jsHeapUsedBytes` | Scene work and observable memory context. JS heap and GPU memory are incomplete proxies for total process/GPU memory. |
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
| Cloud size | Bunny 40,256 points (413 KB); `fs_e430_n4008_18.oct` (2.25 MB); Albuera 508.8 M points (13.71 GB); Majadas 415.5 M points (7.54 GB) | 1280×720 | 4 px | Initial load and frame rate as file/point count grows. |
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
| Pending real-browser run | — | — | — | — | — | — | — | — | — | — | — | — | — |

Run the collector over exported files to form medians and valid-run counts:

```bash
python3 examples/web-host/collect-benchmarks.py artifacts/web-bench/*.json
python3 examples/web-host/collect-benchmarks.py --format csv artifacts/web-bench/*.json
```

No web-performance baseline is recorded until these values come from the browser benchmark export.
