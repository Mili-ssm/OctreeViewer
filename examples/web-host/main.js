import { CoalescedCommandQueue } from "./coalesced-command-queue.mjs";
import {
    FRAME_RATE_WINDOW_MS, FRAME_RATE_WARMUP_MS, lodCounters, lodStillLoading, lodTargetReached, measureFrameRate,
    summarizeFrameReplies,
} from "./benchmark-metrics.mjs";
import { replaceCanvasElement } from "./canvas-replacement.mjs";
import { framebufferSize } from "./rendering-budget.mjs";
import { createConverter } from "./converter-api.mjs";
import {
    FORMAT_VERSIONS, OPTION_DEFAULTS, fixedFrameDisplayValue, fixedFrameHint, isGlobalFrameFormat, progressText, spoolText, timingsText, validateConverterOptions,
} from "./converter-ui.mjs";

let canvas = document.getElementById("viewer-canvas");
const reloadButton = document.getElementById("reload-scene");
const cancelButton = document.getElementById("cancel-load");
const sampleButton = document.getElementById("open-sample");
const webgpuNotice = document.getElementById("webgpu-notice");
const fitButton = document.getElementById("fit-scene");
const fileInput = document.getElementById("dataset-file");
const attributesFileInput = document.getElementById("attributes-file");
const convertFileInput = document.getElementById("convert-file");
const convertButton = document.getElementById("convert-start");
const convertCancelButton = document.getElementById("convert-cancel");
const convertProgress = document.getElementById("convert-progress");
const convertStatus = document.getElementById("convert-status");
const convertDownloadButton = document.getElementById("convert-download");
const convertDownloadAttributesButton = document.getElementById("convert-download-attributes");
const convertOpenButton = document.getElementById("convert-open");
const convertWarning = document.getElementById("convert-warning");
const convertResult = document.getElementById("convert-result");
const convertDetails = document.getElementById("convert-details");
const convertOptionsFields = document.getElementById("convert-options-fields");
const optionFields = {
    formatVersion: document.getElementById("opt-format-version"),
    pointsPerBlock: document.getElementById("opt-points-per-block"),
    maxError: document.getElementById("opt-max-error"),
    spatialDivisions: document.getElementById("opt-spatial-divisions"),
    plannerBudgetMiB: document.getElementById("opt-planner-budget"),
    ramBudgetGB: document.getElementById("opt-ram-budget"),
};
const statsRoot = document.getElementById("stats");
const statusRoot = document.getElementById("renderer-status");
const startBenchmarkButton = document.getElementById("start-benchmark");
const downloadBenchmarkButton = document.getElementById("download-benchmark");
const colorMode = document.getElementById("color-mode");
const pxMode = document.getElementById("px-mode");
let latestPxStats = null;
const voxelPixels = document.getElementById("voxel-pixels");
const voxelPixelsValue = document.getElementById("voxel-pixels-value");
const depthShading = document.getElementById("depth-shading");
const continuity = document.getElementById("continuity");
const cameraMode = document.getElementById("camera-mode");
const cameraModeHelp = document.getElementById("camera-mode-help");

let worker = null;
let converterApi = null;
let convertedFile = null;
let currentProjectFile = null;
let currentProjectName = null;
// The dataset being opened, and the one on screen (set once the worker shows it). Only the shown
// one is reopened by "Recreate viewer", so a cancelled or failed open is never replayed.
let pendingProject = null;
let shownProject = null;
let reopenAfterRecreate = null;
let convertedAttributesFile = null;
let convertedOutputName = "converted.oct";
let convertedAttributesOutputName = "converted.octattrs";
let convertedDownloadUrl = null;
let convertedAttributesDownloadUrl = null;
let canvasTransferred = false;
let viewportSize = null;
const commandQueue = new CoalescedCommandQueue((type, values) => {
    worker.postMessage({ type, ...values });
});
let animationFrame = 0;
let continuousRendering = false;
let frameRequested = false;
let activityStopTimer = 0;
let benchmarkStopTimer = 0;
let framePending = false;
let frameRequestedAt = 0;
let ready = false;
let frameFailed = false;
let lastFrameStatus = "";
let renderCallMs = null;
let activePointer = null;
let lastPointer = [0, 0];
let pointerStart = [0, 0];
let pointerMoved = false;
let pickPending = false;
let pointerMode = "orbit";
let activeCameraMode = cameraMode.value;
let movementSpeed = 1;
let cameraDelta = emptyCameraDelta();
let latestLodStats = null;
let datasetName = "Synthetic sample";
let loadingFile = false;
let sampleFetchController = null;
let benchmarkRun = null;
let frameSamples = [];
let benchmarkArmed = false;
let requestedRefinementId = 0;
let observedRefinementId = 0;


function formatBytes(bytes) {
    if (!Number.isFinite(bytes)) return "unknown";
    const units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let value = bytes;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
        value /= 1024;
        unit += 1;
    }
    return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

function updateProgressiveLoadState(lod) {
    if (!lod) return;
    loadingFile = observedRefinementId !== requestedRefinementId
        || (!lod.lodError && !lod.pageBudgetExhausted && (benchmarkRun?.refinement?.targetLodMs === null
            || lodStillLoading(lod)));
    cancelButton.hidden = !loadingFile;
    if (loadingFile) startRendering();
    else if (!benchmarkStopTimer) stopRendering();
}

function formatFrameRenderMs(metrics) {
    if (!metrics || metrics.sampleCount === 0) return "—";
    return `p50/p95/p99 ${metrics.p50Ms.toFixed(2)}/${metrics.p95Ms.toFixed(2)}/${metrics.p99Ms.toFixed(2)} ms (${metrics.sampleCount} frames)`;
}

function recordFrameReply(renderCallMs) {
    const time = performance.now();
    const roundTripMs = time - frameRequestedAt;
    frameSamples.push({ time, renderCallMs, roundTripMs });
    if (frameSamples.length > 2_048) frameSamples.splice(0, 1_024);
    // Opt-in full log (probe-orbit-load.mjs): epoch-stamped to align with worker LOD messages.
    if (benchLog && benchLog.length < BENCH_LOG_CAPACITY) {
        benchLog.push({ time: performance.timeOrigin + time, renderCallMs, roundTripMs });
    }
}

// Test/benchmark hook. Frame-reply logging stays off until a probe calls start().
let benchLog = null;
const BENCH_LOG_CAPACITY = 100_000;
let pendingWorkerMetrics = [];
let nextBenchApiId = 1_000_000;
const pendingBenchApi = new Map();
window.__renderBench = {
    start() { benchLog = []; },
    frameReplies: () => (benchLog ?? []).slice(),
    summarize(from, to) { return summarizeFrameReplies(benchLog ?? [], from, to); },
    workerMetrics() {
        return new Promise((resolve) => {
            pendingWorkerMetrics.push(resolve);
            worker.postMessage({ type: "metrics" });
        });
    },
    resetWorkerMetrics() { worker.postMessage({ type: "metrics", reset: true }); },
    apiCall(operation, args = {}) {
        return new Promise((resolve) => {
            const requestId = nextBenchApiId++;
            pendingBenchApi.set(requestId, resolve);
            worker.postMessage({ type: "api", ...args, operation, requestId });
        });
    },
    // Runs frames for a moment, as camera input would, so an idle scene keeps refining.
    wake: () => wakeRendering(),
    latestLodCounters: () => lodCounters(latestLodStats),
    // Cells path: the resting digest of the applied cut (null until at rest) and the latest cell stats.
    cutDigestAtRest: () => latestLodStats?.cutDigestAtRest ?? null,
    cellStats: () => latestLodStats && {
        cellPath: latestLodStats.cellPath ?? null, cellsBusy: latestLodStats.cellsBusy ?? null,
        viewEpoch: latestLodStats.viewEpoch ?? null, cutEpochSent: latestLodStats.cutEpochSent ?? null,
        cutEpochApplied: latestLodStats.cutEpochApplied ?? null,
        continuity: latestLodStats.continuity ?? null, continuityMinSide: latestLodStats.continuityMinSide ?? null,
        wasmLinearMemoryBytes: latestLodStats.wasmLinearMemoryBytes ?? null,
    },
    openTimings: () => benchmarkRun && {
        firstCoarseFrameMs: benchmarkRun.firstCoarseFrameMs,
        targetLodMs: benchmarkRun.targetLodMs,
        uniqueBytesRead: benchmarkRun.uniqueBytesRead,
    },
};

function exportBenchmarkRun() {
    if (!benchmarkRun) return;
    const fps = benchmarkRun.finalFps
        ?? measureFrameRate(frameSamples, benchmarkRun.measurementStartedAt);
    const lod = latestLodStats;
    const gpu = benchmarkRun.latestGpuStats;
    const payload = {
        schemaVersion: 1,
        measurement: "worker_frame_reply_rate",
        startedAt: benchmarkRun.startedAtIso,
        dataset: {
            name: benchmarkRun.datasetName,
            fileBytes: benchmarkRun.fileBytes,
            attributes: benchmarkRun.attributes,
            sourcePointCount: benchmarkRun.sourcePointCount,
            nodeCount: benchmarkRun.nodeCount,
        },
        configuration: {
            viewportFramebuffer: benchmarkRun.viewportFramebuffer,
            devicePixelRatio: benchmarkRun.devicePixelRatio,
            effectivePixelRatio: benchmarkRun.effectivePixelRatio,
            targetVoxelPixels: benchmarkRun.targetVoxelPixels,
            colorMode: colorMode.value,
            depthShading: depthShading.checked,
            cacheBudgetBytes: 256 * 1024 * 1024,
        },
        refinement: benchmarkRun.refinement,
        load: {
            colorMode: "rgb",
            targetVoxelPixels: benchmarkRun.initialTargetVoxelPixels,
            metadataOpenMs: benchmarkRun.metadataOpenMs,
            firstCoarseFrameMs: benchmarkRun.firstCoarseFrameMs,
            targetLodMs: benchmarkRun.targetLodMs,
            uniqueBytesRead: benchmarkRun.uniqueBytesRead,
            refinementWork: lod?.refinementWork ?? null,
            counters: lodCounters(lod),
        },
        steadyState: {
            warmupAfterBenchmarkStartMs: FRAME_RATE_WARMUP_MS,
            measurementWindowMs: fps.windowDurationMs,
            frameReplySpanMs: fps.durationMs,
            frameReplies: fps.sampleCount,
            meanFps: fps.meanFps,
            frameReplyIntervalP50Ms: fps.intervalP50Ms,
            frameReplyIntervalP95Ms: fps.intervalP95Ms,
            frameReplyIntervalMaxMs: fps.intervalMaxMs ?? null,
            workerRenderCallDuringLoadMs: benchmarkRun.frameRenderMs,
            workerRenderCallP50Ms: fps.renderCallP50Ms,
            workerRenderCallP95Ms: fps.renderCallP95Ms,
            workerRenderCallP99Ms: fps.renderCallP99Ms,
            cameraChanged: benchmarkRun.cameraChanged,
            viewportChanged: benchmarkRun.viewportChanged,
            settingsChanged: benchmarkRun.settingsChanged,
        },
        scene: lod ? {
            visibleSourcePoints: lod.visiblePoints,
            renderedVoxels: lod.renderedVoxels,
            visibleBlocks: lod.visibleBlocks,
            pendingBlocks: lod.pendingBlockIds.length,
            lodStagePixels: lod.lodStagePixels,
            coverageComplete: lod.coverageComplete,
            cacheBytes: lod.cacheBytes,
            cacheBudgetBytes: lod.cacheBudgetBytes,
            cacheActivity: lod.cacheActivity ?? null,
            voxelBudget: lod.voxelBudget,
            voxelBuildBudgetBytes: lod.voxelBuildBudgetBytes,
            voxelBuildEstimatedPeakBytes: lod.voxelBuildEstimatedPeakBytes,
            voxelInstanceCapacityBytes: lod.voxelInstanceCapacityBytes,
            projectionCachePeakEntries: lod.projectionCachePeakEntries,
            hzbActive: lod.hzbActive ?? gpu?.hzbActive ?? false,
        } : null,
        memory: {
            latestViewerWorkerSnapshot: benchmarkRun.latestMemoryStats,
            mainThreadJsHeapUsedBytes: performance.memory?.usedJSHeapSize ?? null,
        },
        environment: {
            userAgent: navigator.userAgent,
            hardwareConcurrency: navigator.hardwareConcurrency ?? null,
            deviceMemoryGiB: navigator.deviceMemory ?? null,
            webGpuAvailable: Boolean(navigator.gpu),
            jsHeapUsedBytes: performance.memory?.usedJSHeapSize ?? null,
            gpuStats: gpu,
        },
        notes: [
            "memory.latestViewerWorkerSnapshot is the latest stats/progress snapshot, not an export-time measurement or a peak. WASM linear capacity is not live allocations, JS heap, RSS/PSS or GPU memory.",
            "workerJsHeapUsedBytes is unknown (null). mainThreadJsHeapUsedBytes and legacy environment.jsHeapUsedBytes use nonstandard performance.memory on the main thread only; they do not measure the viewer worker. Do not sum these overlapping metrics.",
            "FPS counts worker frame replies (submitted frames), not GPU completion or display scanout.",
            "load timing describes initial RGB opening; refinement describes the latest dataset/color/detail request. Legacy workerRenderCallDuringLoadMs describes the active refinement.",
            "refinementWork accumulates synchronous LOD work since open or attribute-cache reset; uploadMs measures CPU preparation/enqueue, not GPU completion.",
            "Use only runs with cameraChanged=false, viewportChanged=false, settingsChanged=false, complete coverage, and a >=10 s stable window for comparisons.",
        ],
    };
    const blob = new Blob([JSON.stringify(payload, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    const safeName = benchmarkRun.datasetName.replace(/[^a-z0-9.-]+/gi, "-").replace(/^-|-$/g, "");
    link.href = url;
    link.download = `web-bench-${safeName || "dataset"}-${benchmarkRun.targetVoxelPixels}px-${benchmarkRun.viewportFramebuffer.join("x")}.json`;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 0);
}

function emptyCameraDelta() {
    return { orbitX: 0, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0 };
}

function queueWorkerCommand(type, values = {}) {
    if (!ready || frameFailed) return;
    commandQueue.enqueue(type, values);
    wakeRendering();
}

function resize() {
    const [width, height] = framebufferSize(
        canvas.clientWidth,
        canvas.clientHeight,
        window.devicePixelRatio || 1,
    );

    if (!canvasTransferred && (canvas.width !== width || canvas.height !== height)) {
        canvas.width = width;
        canvas.height = height;
    }
    if (benchmarkRun && (benchmarkRun.viewportFramebuffer[0] !== width
        || benchmarkRun.viewportFramebuffer[1] !== height)) {
        benchmarkRun.viewportChanged = true;
    }
    if (viewportSize?.[0] !== width || viewportSize?.[1] !== height) {
        viewportSize = [width, height];
        queueWorkerCommand("resize", { width, height });
    }
}

// The cells path carries no projected-edge range (the planner does not measure it): show "—".
function formatVoxelEdges(lod) {
    return Number.isFinite(lod?.minVoxelEdgePixels) && Number.isFinite(lod?.maxVoxelEdgePixels)
        ? `${lod.minVoxelEdgePixels.toFixed(2)}–${lod.maxVoxelEdgePixels.toFixed(2)} px` : "—";
}

function updateStats(message) {
    const lod = message.lod ?? latestLodStats;
    const points = lod?.visiblePoints ?? message.viewer.point_count;
    const coverage = lod
        ? `${lod.coverageComplete ? "complete" : "incomplete"} (${lod.uncoveredNodes} uncovered)`
        : "not using LOD";
    const cache = lod
        ? `${formatBytes(Number(lod.cacheBytes))} / ${formatBytes(Number(lod.cacheBudgetBytes))}`
        : "—";
    statsRoot.textContent = [
        `dataset: ${datasetName}`,
        `finest voxel detail target: ${lod?.targetVoxelPixels ?? voxelPixels.value} px`
            + (latestPxStats?.mode === "dynamic"
                ? ` · dynamic (${latestPxStats.signalSource ?? "no signal"}${latestPxStats.signalMs === null ? "" : ` p95 ${latestPxStats.signalMs.toFixed(1)} ms`})`
                : ""),
        `LOD stage: ${lod?.lodStagePixels ?? "—"} px`,
        `rendered voxel limit: ${lod?.voxelBudget ?? "—"}`,
        `projected voxel edges: ${formatVoxelEdges(lod)}`,
        `voxels above target: ${lod?.oversizedVoxels ?? "—"}`,
        `visible points: ${points}`,
        `visible voxels: ${lod?.renderedVoxels ?? message.gpu.visibleInstances}`,
        `HZB occlusion: ${(lod?.hzbActive ?? message.gpu.hzbActive) ? "active" : "inactive"}`,
        `visible blocks: ${message.gpu.visibleBlocks}`,
        `deepest visible LOD level: ${lod?.maxVisibleLevel ?? "—"}`,
        `LOD coverage: ${coverage}`,
        `pending blocks: ${lod?.pendingBlockIds.length ?? 0} · pending pages ${lod?.pendingPageAnchors?.length ?? 0}${lod?.pageBudgetExhausted ? " (page budget exhausted)" : ""} · evicted targets ${lod?.refinementWork?.evictedPendingBlocks ?? "—"}`,
        `decoded cache: ${cache} · evictions / rejects ${lod?.cacheActivity?.evictions ?? "—"} / ${lod?.cacheActivity ? Number(lod.cacheActivity.rejectedOversize) + Number(lod.cacheActivity.rejectedProtected) : "—"}`,
        `uploaded instance bytes: ${message.gpu.uploadedBytes}`,
        `worker render call: ${renderCallMs === null ? "—" : `${renderCallMs.toFixed(2)} ms`}`,
        `renderFrame p50/p95/p99: ${formatFrameRenderMs(message.frameRenderMs)}`,
        `worker frame replies: ${measureFrameRate(frameSamples, benchmarkRun?.measurementStartedAt).meanFps?.toFixed(1) ?? "warming up"} FPS (last stable window)`,
        `viewport: ${viewportSize?.join(" x ") ?? "—"}`,
    ].join("\n");
}

function requestFrame() {
    animationFrame = 0;
    if (!ready || frameFailed) return;
    if (framePending) return;
    framePending = true;
    const camera = cameraDelta;
    cameraDelta = emptyCameraDelta();
    if (benchmarkRun && Object.values(camera).some((value) => value !== 0)) {
        benchmarkRun.cameraChanged = true;
    }
    frameRequestedAt = performance.now();
    worker.postMessage({ type: "frame", camera });
    if (continuousRendering) animationFrame = requestAnimationFrame(requestFrame);
}

function requestFrameOnce() {
    if (framePending) {
        frameRequested = true;
        return;
    }
    if (animationFrame === 0) animationFrame = requestAnimationFrame(requestFrame);
}

function startRendering() {
    continuousRendering = true;
    requestFrameOnce();
}

function stopRendering() {
    continuousRendering = false;
    if (animationFrame !== 0) {
        cancelAnimationFrame(animationFrame);
        animationFrame = 0;
    }
}

function wakeRendering() {
    if (!ready || frameFailed) return;
    startRendering();
    clearTimeout(activityStopTimer);
    activityStopTimer = setTimeout(() => {
        activityStopTimer = 0;
        if (!loadingFile && !benchmarkStopTimer) stopRendering();
    }, 250);
}

function acknowledgeFrame() {
    framePending = false;
    if (continuousRendering || frameRequested) {
        frameRequested = false;
        requestFrameOnce();
    }
}

function finishBenchmarkWindow() {
    benchmarkStopTimer = 0;
    if (!benchmarkRun || loadingFile) return;
    const measured = measureFrameRate(frameSamples, benchmarkRun.measurementStartedAt);
    if (measured.windowDurationMs < FRAME_RATE_WINDOW_MS) {
        benchmarkStopTimer = setTimeout(finishBenchmarkWindow,
            Math.max(1, Math.ceil(FRAME_RATE_WINDOW_MS - measured.windowDurationMs)));
        return;
    }
    benchmarkRun.finalFps = measured;
    startBenchmarkButton.disabled = false;
    startBenchmarkButton.textContent = "Repeat 15 s benchmark";
    downloadBenchmarkButton.disabled = false;
    stopRendering();
}

function beginBenchmarkWindow() {
    if (!benchmarkRun || !benchmarkArmed || loadingFile) return;
    benchmarkArmed = false;
    benchmarkRun.measurementStartedAt = performance.now();
    benchmarkRun.finalFps = null;
    startBenchmarkButton.disabled = true;
    startBenchmarkButton.textContent = "Benchmark running…";
    clearTimeout(benchmarkStopTimer);
    startRendering();
    benchmarkStopTimer = setTimeout(finishBenchmarkWindow, FRAME_RATE_WARMUP_MS + FRAME_RATE_WINDOW_MS);
}

function startBenchmark() {
    if (!benchmarkRun || benchmarkStopTimer) return;
    frameSamples = [];
    benchmarkRun.cameraChanged = false;
    benchmarkRun.viewportChanged = false;
    benchmarkRun.settingsChanged = false;
    benchmarkRun.finalFps = null;
    benchmarkRun.measurementStartedAt = null;
    benchmarkRun.viewportFramebuffer = [canvas.width, canvas.height];
    benchmarkRun.devicePixelRatio = window.devicePixelRatio || 1;
    benchmarkRun.effectivePixelRatio = canvas.clientWidth > 0
        ? canvas.width / canvas.clientWidth
        : 0;
    startBenchmarkButton.disabled = true;
    startBenchmarkButton.textContent = loadingFile
        ? "Waiting for target LOD…"
        : "Benchmark running…";
    downloadBenchmarkButton.disabled = true;
    benchmarkArmed = true;
    if (loadingFile) {
        startRendering();
    } else if (lodTargetReached(latestLodStats)) {
        beginBenchmarkWindow();
    } else {
        benchmarkArmed = false;
        startBenchmarkButton.disabled = false;
        startBenchmarkButton.textContent = "Run 15 s FPS benchmark";
    }
}

// `?url=FILE.oct[&attrs=FILE.octattrs|none][&px=4]` opens a file over HTTP Range; the attributes default to
// the `.octattrs` next to it. The server must answer Range and send CORS headers.
// `&maxGap=BYTES` merges range reads separated by up to that many bytes (default 0: touching only);
// `&remoteReads=N` sets the concurrent LOD reads for this remote source (default 8).
let urlOpened = false;
let sampleAutoOpened = false;
function openUrlFromQuery() {
    const params = new URLSearchParams(location.search);
    const target = params.get("url");
    if (urlOpened || !target) return;
    urlOpened = true;
    if (params.get("px")) voxelPixels.value = params.get("px");
    const absolute = (value) => new URL(value, location.href).href;
    const url = absolute(target);
    const attrs = params.get("attrs");
    const readOptions = {};
    for (const name of ["maxGap", "remoteReads"]) {
        const value = Number(params.get(name));
        if (params.get(name) !== null && Number.isFinite(value) && value >= 0) readOptions[name] = Math.trunc(value);
    }
    const attributes = attrs === "none" ? undefined : { url: attrs ? absolute(attrs) : `${url}attrs`, ...readOptions };
    openProjectFile({ url, ...readOptions }, decodeURIComponent(new URL(url).pathname.split("/").pop()), attributes);
}

function handleWorkerMessage(event) {
    if (event.currentTarget !== worker) return;
    const { data } = event;
    if (data.type === "pxMode") {
        pxMode.value = data.mode;
    } else if (data.type === "ready") {
        ready = true;
        pxMode.disabled = false;
        fileInput.disabled = false;
        attributesFileInput.disabled = false;
        sampleButton.disabled = false;
        reloadButton.textContent = "Recreate viewer";
        if (viewportSize) {
            queueWorkerCommand("resize", {
                width: viewportSize[0],
                height: viewportSize[1],
            });
        }
        statusRoot.textContent = "Renderer ready · synthetic sample";
        // `?cellLod=cell|octant` picks the cells path's level selection (no UI control yet).
        const cellLod = new URLSearchParams(location.search).get("cellLod");
        if (cellLod === "octant" || cellLod === "cell") {
            worker.postMessage({ type: "setCellLod", mode: cellLod });
        } else if (cellLod !== null) {
            // A bad value would come back as a worker error and stop the render loop.
            console.warn(`ignoring ?cellLod=${cellLod}: expected "octant" or "cell"`);
        }
        requestFrameOnce();
        if (reopenAfterRecreate) {
            const { file, name, attributes } = reopenAfterRecreate;
            reopenAfterRecreate = null;
            openProjectFile(file, name, attributes);
            return;
        }
        openUrlFromQuery();
        // A visitor with no `?url=` sees something at once; the button stays for reopening it.
        // Automated browsers (probes, Playwright) skip it so their measurements start empty;
        // `?sample=1` forces it, `?sample=0` disables it.
        const sampleParam = new URLSearchParams(location.search).get("sample");
        const autoSample = sampleParam === "1" || (sampleParam !== "0" && !navigator.webdriver);
        if (autoSample && !urlOpened && !sampleAutoOpened) {
            sampleAutoOpened = true;
            sampleButton.click();
        }
    } else if (data.type === "commandAck") {
        commandQueue.acknowledge(data.command);
    } else if (data.type === "status") {
        statusRoot.textContent = data.message;
    } else if (data.type === "fileReadProgress") {
        const read = Number(data.uniqueBytes);
        const total = Number(data.fileBytes);
        loadingFile = true;
        if (benchmarkRun) benchmarkRun.uniqueBytesRead = String(data.uniqueBytes);
        cancelButton.hidden = false;
        statusRoot.textContent = `Read ranges: ${formatBytes(read)} of ${formatBytes(total)} (unique bytes)`;
    } else if (data.type === "dataset") {
        datasetName = data.name;
        if (pendingProject) shownProject = pendingProject;
        updateAttributeChoices(data.opened);
        latestLodStats = data.lod;
        updateProgressiveLoadState(data.lod);
        fileInput.disabled = false;
        attributesFileInput.disabled = false;
        sampleButton.disabled = false;
        voxelPixels.disabled = false;
        statusRoot.textContent = `${data.name}: showing coarse view · refining LOD`;
    } else if (data.type === "datasetOpened") {
        datasetName = data.name;
        if (benchmarkRun) {
            benchmarkRun.metadataOpenMs = performance.now() - benchmarkRun.startedAt;
            benchmarkRun.nodeCount = data.opened.nodeCount;
            benchmarkRun.sourcePointCount = data.opened.sourcePointCount ?? null;
        }
        statusRoot.textContent = `${data.name}: ${data.opened.nodeCount} nodes${data.opened.paged ? " (declared, paged)" : ""} · selecting visible blocks…`;
    } else if (data.type === "lod") {
        latestLodStats = data.lod;
        updateProgressiveLoadState(data.lod);
        statusRoot.textContent = `LOD stage ${data.lod.lodStagePixels} px · finest target ${data.lod.targetVoxelPixels} px`;
    } else if (data.type === "lodProgress") {
        if (data.refinement?.id !== requestedRefinementId) return;
        observedRefinementId = data.refinement.id;
        latestLodStats = data.lod;
        updateStats(data);
        if (benchmarkRun) {
            benchmarkRun.refinement = data.refinement;
            benchmarkRun.latestGpuStats = data.gpu;
            benchmarkRun.latestMemoryStats = data.memory ?? null;
            if (data.refinement.trigger === "dataset") {
                benchmarkRun.firstCoarseFrameMs = data.firstCoarseMs;
                benchmarkRun.targetLodMs = data.targetLodMs;
            }
            benchmarkRun.frameRenderMs = data.frameRenderMs;
        }
        updateProgressiveLoadState(data.lod);
        if (data.lod.lodError) {
            benchmarkArmed = false;
            startBenchmarkButton.disabled = false;
            startBenchmarkButton.textContent = "Run 15 s FPS benchmark";
            const action = data.lod.lodError.startsWith("refinement paused")
                ? "paused" : "failed";
            statusRoot.textContent = `LOD refinement ${action}: ${data.lod.lodError} · keeping the last visible cut`;
            return;
        }
        if (benchmarkArmed && data.targetLodMs !== null) beginBenchmarkWindow();
        const firstView = data.firstCoarseMs === null
            ? "coarse frame pending"
            : `first coarse frame ${data.firstCoarseMs.toFixed(0)} ms`;
        const targetView = data.lod.pageBudgetExhausted
            ? "page budget exhausted (target not reached)"
            : data.targetLodMs === null || !lodTargetReached(data.lod)
                ? "target refining"
                : `target reached ${data.targetLodMs.toFixed(0)} ms`;
        statusRoot.textContent = `${firstView} · ${targetView} · stage ${data.lod.lodStagePixels} px`;
    } else if (data.type === "stats") {
        latestPxStats = data.px ?? null;
        acknowledgeFrame();
        renderCallMs = data.frameStatus?.renderCallMs ?? renderCallMs;
        recordFrameReply(renderCallMs);
        if (data.frameStatus?.status !== lastFrameStatus) {
            lastFrameStatus = data.frameStatus?.status ?? "";
            if (lastFrameStatus) statusRoot.textContent = `Renderer: ${lastFrameStatus}`;
        }
        latestLodStats = data.lod ?? latestLodStats;
        if (benchmarkRun) {
            benchmarkRun.latestGpuStats = data.gpu;
            benchmarkRun.latestMemoryStats = data.memory ?? null;
            benchmarkRun.frameRenderMs = data.frameRenderMs;
        }
        updateProgressiveLoadState(latestLodStats);
        updateStats(data);
    } else if (data.type === "frame") {
        acknowledgeFrame();
        renderCallMs = data.renderCallMs;
        recordFrameReply(renderCallMs);
        updateProgressiveLoadState(latestLodStats);
        if (data.frameStatus !== lastFrameStatus) {
            lastFrameStatus = data.frameStatus;
            statusRoot.textContent = `Renderer: ${lastFrameStatus}`;
        }
    } else if (data.type === "apiResult") {
        pendingBenchApi.get(data.requestId)?.(data.value);
        pendingBenchApi.delete(data.requestId);
    } else if (data.type === "metrics") {
        const waiting = pendingWorkerMetrics;
        pendingWorkerMetrics = [];
        for (const resolve of waiting) resolve(data.workerMetrics);
    } else if (data.type === "pick") {
        pickPending = false;
        if (data.hit) {
            canvas.dispatchEvent(new CustomEvent("voxelviewer:pick", {
                detail: { ...data.hit, sceneGeneration: data.sceneGeneration },
            }));
        }
    } else if (data.type === "deviceLost") {
        if (data.reason !== "destroyed") swapCanvasForRecovery();
        statusRoot.textContent = `GPU device lost (${data.reason}): ${data.message}`;
    } else if (data.type === "deviceRecovery") {
        // R3-Q26: the worker recreates the viewer by itself; the page only shows it and draws again.
        statusRoot.textContent = data.state === "recovered"
            ? `GPU device recovered (attempt ${data.attempt})`
            : `GPU device lost: recovering (attempt ${data.attempt})…`;
        if (data.state === "recovered") wakeRendering();
    } else if (data.type === "error") {
        loadingFile = false;
        observedRefinementId = requestedRefinementId;
        benchmarkArmed = false;
        clearTimeout(benchmarkStopTimer);
        benchmarkStopTimer = 0;
        stopRendering();
        startBenchmarkButton.disabled = !benchmarkRun?.refinement;
        startBenchmarkButton.textContent = "Run 15 s FPS benchmark";
        cancelButton.hidden = true;
        fileInput.disabled = false;
        attributesFileInput.disabled = false;
        sampleButton.disabled = false;
        statusRoot.textContent = `${data.operation} failed: ${data.message}`;
        if (data.operation === "frame" || data.operation === "init") {
            frameFailed = true;
            stopRendering();
            worker?.terminate();
            worker = null;
            reloadButton.textContent = "Recreate viewer";
        }
    }
}

// On by default: scenes are built in build-worker.js instead of the render worker (render plan R2,
// ADR 0018). ?buildWorker=0 turns it off; ?buildWorker=1 is accepted and changes nothing.
const BUILD_WORKER = { url: new URL("./build-worker.js", import.meta.url).href, type: "module" };

function createWorkerViewer() {
    if (!navigator.gpu) {
        const message = "WebGPU is not available in this browser. This demo needs desktop Chrome or Edge with WebGPU enabled.";
        statusRoot.textContent = message;
        if (webgpuNotice) {
            webgpuNotice.textContent = message;
            webgpuNotice.hidden = false;
        }
        reloadButton.disabled = false;
        return;
    }
    statusRoot.textContent = "Starting renderer worker…";
    if (!HTMLCanvasElement.prototype.transferControlToOffscreen || !window.Worker) {
        statusRoot.textContent = "This browser does not support OffscreenCanvas workers.";
        reloadButton.disabled = false;
        return;
    }

    try {
        worker = new Worker(new URL("./viewer-worker.js", import.meta.url), { type: "module" });
        worker.addEventListener("message", handleWorkerMessage);
        const handleWorkerError = (event) => {
            if (event.currentTarget !== worker) return;
            frameFailed = true;
            loadingFile = false;
            cancelButton.hidden = true;
            stopRendering();
            statusRoot.textContent = `Renderer worker failed: ${event.message || "message error"}. Recreate the viewer to retry.`;
            worker.terminate();
            worker = null;
        };
        worker.addEventListener("error", handleWorkerError);
        worker.addEventListener("messageerror", handleWorkerError);
        const offscreen = canvas.transferControlToOffscreen();
        canvasTransferred = true;
        worker.postMessage({
            type: "init",
            canvas: offscreen,
            width: canvas.width,
            height: canvas.height,
            colorMode: colorMode.value,
            depthShading: depthShading.checked,
            buildWorker: new URLSearchParams(location.search).get("buildWorker") !== "0" ? BUILD_WORKER : undefined,
            // R3 cells path by default (R3-Q35); ?cells=0 keeps the CPU path.
            cells: new URLSearchParams(location.search).get("cells") !== "0",
            canvasReplacement: true,
        }, [offscreen]);
        reloadButton.disabled = false;
        reloadButton.textContent = "Recreate viewer";
    } catch (error) {
        loadingFile = false;
        cancelButton.hidden = true;
        statusRoot.textContent = `Renderer initialization failed: ${error}. Recreate the viewer to retry.`;
        reloadButton.disabled = false;
    }
}

// R3-Q34: a GPU-process crash leaves the old canvas blank, so the worker gets a fresh element.
function swapCanvasForRecovery() {
    const replaced = replaceCanvasElement(canvas);
    if (!replaced) {
        worker.postMessage({ type: "replaceCanvas", canvas: null });
        return;
    }
    canvas = replaced.canvas;
    bindCanvasInteractions();
    worker.postMessage({ type: "replaceCanvas", canvas: replaced.offscreen }, [replaced.offscreen]);
    canvas.dispatchEvent(new CustomEvent("voxelviewer:canvasReplaced", { detail: { canvas } }));
}

function recreateViewer({ reopen = false } = {}) {
    reopenAfterRecreate = reopen ? shownProject : null;
    pendingProject = null;
    shownProject = null;
    stopRendering();
    clearTimeout(activityStopTimer);
    clearTimeout(benchmarkStopTimer);
    activityStopTimer = 0;
    benchmarkStopTimer = 0;
    frameRequested = false;
    benchmarkRun = null;
    frameSamples = [];
    benchmarkArmed = false;
    startBenchmarkButton.disabled = true;
    startBenchmarkButton.textContent = "Run 15 s FPS benchmark";
    downloadBenchmarkButton.disabled = true;
    sampleFetchController?.abort();
    sampleFetchController = null;
    if (worker) worker.terminate();
    worker = null;
    ready = false;
    frameFailed = false;
    framePending = false;
    commandQueue.reset();
    loadingFile = false;
    cancelButton.hidden = true;
    fileInput.disabled = true;
    attributesFileInput.disabled = true;
    sampleButton.disabled = true;
    voxelPixels.disabled = true;
    latestLodStats = null;
    datasetName = "Synthetic sample";
    cameraDelta = emptyCameraDelta();
    const replaced = replaceCanvasElement(canvas, { transfer: false });
    if (replaced) canvas = replaced.canvas;
    canvasTransferred = false;
    viewportSize = null;
    bindCanvasInteractions();
    resize();
    createWorkerViewer();
}

function cancelFileLoad() {
    if (!loadingFile) return;
    recreateViewer();
    statusRoot.textContent = "File load cancelled · viewer restarted";
}

reloadButton.addEventListener("click", () => recreateViewer({ reopen: true }));
cancelButton.addEventListener("click", cancelFileLoad);
startBenchmarkButton.addEventListener("click", startBenchmark);
function resetBenchmarkWindow() {
    if (!benchmarkRun) return;
    benchmarkRun.settingsChanged = true;
    benchmarkRun.measurementStartedAt = null;
    benchmarkRun.finalFps = null;
    benchmarkArmed = false;
    clearTimeout(benchmarkStopTimer);
    benchmarkStopTimer = 0;
    startBenchmarkButton.disabled = false;
    startBenchmarkButton.textContent = "Run 15 s FPS benchmark";
    downloadBenchmarkButton.disabled = true;
}

function requestRefinement() {
    requestedRefinementId += 1;
    loadingFile = true;
    benchmarkRun.refinement = null;
    return requestedRefinementId;
}

function queueRefinement(trigger) {
    if (!ready || frameFailed) return;
    const refinementId = benchmarkRun ? requestRefinement() : undefined;
    queueWorkerCommand("refine", {
        trigger, refinementId, colorMode: colorMode.value,
        targetVoxelPixels: Number(voxelPixels.value),
    });
}

colorMode.addEventListener("change", () => {
    resetBenchmarkWindow();
    queueRefinement("color");
});
function updateLodControls() {
    voxelPixelsValue.value = `${voxelPixels.value} px`;
    if (ready && latestLodStats) {
        if (benchmarkRun && Number(voxelPixels.value) !== benchmarkRun.targetVoxelPixels) {
            benchmarkRun.targetVoxelPixels = Number(voxelPixels.value);
            resetBenchmarkWindow();
        }
        queueRefinement("detail");
    }
}

voxelPixels.addEventListener("input", updateLodControls);
pxMode.addEventListener("change", () => {
    if (benchmarkRun) benchmarkRun.settingsChanged = true;
    worker.postMessage({ type: "pxMode", mode: pxMode.value });
});
depthShading.addEventListener("change", () => {
    if (benchmarkRun) benchmarkRun.settingsChanged = true;
    queueWorkerCommand("depthShading", { value: depthShading.checked });
});
continuity.addEventListener("change", () => {
    resetBenchmarkWindow();
    queueWorkerCommand("continuity", { enabled: continuity.checked });
});
for (const version of FORMAT_VERSIONS) {
    optionFields.formatVersion.append(new Option(version, version, false, version === OPTION_DEFAULTS.formatVersion));
}

// Global-frame formats fix these two values: disable the controls and say why (aria-describedby).
const savedFixedFrameValues = {};
function syncFixedFrameControls() {
    const fixed = isGlobalFrameFormat(optionFields.formatVersion.value);
    for (const name of ["pointsPerBlock", "spatialDivisions"]) {
        const field = optionFields[name];
        // Rewrite the value only when the fixed state flips, so typing in an enabled field sticks.
        if (fixed !== field.disabled) {
            if (fixed) savedFixedFrameValues[name] = field.value;
            field.value = fixedFrameDisplayValue(name, optionFields.formatVersion.value, savedFixedFrameValues[name]);
            if (!fixed) delete savedFixedFrameValues[name];
        }
        field.disabled = fixed;
        const hint = document.getElementById(`${field.id}-fixed`);
        hint.textContent = fixedFrameHint(name);
        hint.hidden = !fixed;
    }
}

function readConverterOptions() {
    return validateConverterOptions(Object.fromEntries(
        Object.entries(optionFields).map(([name, element]) => [name, element.value]),
    ));
}

// Shows each field's message next to it (aria-describedby) and returns the options, or null if any is invalid.
function refreshConverterOptions() {
    syncFixedFrameControls();
    const { errors, options } = readConverterOptions();
    for (const [name, element] of Object.entries(optionFields)) {
        const message = document.getElementById(`${element.id}-error`);
        if (!message) continue;
        message.textContent = errors[name] ?? "";
        message.hidden = !errors[name];
        element.setAttribute("aria-invalid", String(Boolean(errors[name])));
    }
    const valid = Object.keys(errors).length === 0;
    convertButton.disabled = !valid || !convertFileInput.files?.[0] || Boolean(converterApi);
    return valid ? options : null;
}

function showConvertNotice(kind, message) {
    convertResult.className = `notice ${kind}`;
    convertResult.textContent = message;
    convertResult.hidden = false;
    convertResult.focus();
}

for (const element of Object.values(optionFields)) element.addEventListener("input", refreshConverterOptions);
convertFileInput.addEventListener("change", refreshConverterOptions);
refreshConverterOptions();
convertButton.addEventListener("click", () => {
    const file = convertFileInput.files?.[0];
    if (!file || converterApi) return;
    if (!/\.(las|laz)$/i.test(file.name)) {
        convertStatus.textContent = "Choose a .las or .laz file.";
        return;
    }
    const options = refreshConverterOptions();
    if (!options) return;
    convertedFile = null;
    convertedAttributesFile = null;
    if (convertedDownloadUrl) URL.revokeObjectURL(convertedDownloadUrl);
    if (convertedAttributesDownloadUrl) URL.revokeObjectURL(convertedAttributesDownloadUrl);
    convertedDownloadUrl = null;
    convertedAttributesDownloadUrl = null;
    convertDownloadButton.hidden = true;
    convertDownloadAttributesButton.hidden = true;
    convertOpenButton.hidden = true;
    convertWarning.hidden = true;
    convertResult.hidden = true;
    convertDetails.hidden = true;
    convertProgress.hidden = false;
    convertProgress.value = 0;
    convertProgress.max = 1;
    convertButton.disabled = true;
    convertFileInput.disabled = true;
    convertOptionsFields.disabled = true;
    convertCancelButton.hidden = false;
    convertStatus.textContent = `Preparing local conversion of ${file.name}… (Esc cancels)`;
    try {
        converterApi = createConverter();
        converterApi.on("status", ({ message }) => { convertStatus.textContent = message; });
        converterApi.on("warning", ({ message }) => {
            convertWarning.textContent = message;
            convertWarning.hidden = false;
        });
        converterApi.on("progress", ({ processedPoints, totalPoints, stage }) => {
            convertProgress.max = Number(totalPoints) || 1;
            convertProgress.value = Number(processedPoints);
            convertStatus.textContent = progressText(stage, processedPoints, totalPoints);
        });
        converterApi.convert(file, { options }).then(({ file: resultFile, name, attributesFile, outputAttributesName, points, timings, spool }) => {
            convertedFile = resultFile;
            convertedAttributesFile = attributesFile;
            convertedOutputName = name;
            convertedAttributesOutputName = outputAttributesName;
            convertProgress.value = convertProgress.max;
            convertStatus.textContent = "";
            const details = [timingsText(timings), spoolText(spool)].filter(Boolean).join(" · ");
            convertDetails.textContent = details;
            convertDetails.hidden = !details;
            convertDownloadButton.hidden = false;
            convertDownloadAttributesButton.hidden = !attributesFile;
            convertOpenButton.hidden = false;
            showConvertNotice("success", `Converted ${Number(points).toLocaleString("en-US")} points to ${name}.`);
        }).catch((error) => {
            convertStatus.textContent = "";
            if (error.name === "AbortError") showConvertNotice("warning", error.message);
            // The core engine's own message already starts with "conversion failed:".
            else showConvertNotice("error", `Conversion failed: ${error.message.replace(/^conversion failed:\s*/i, "")}`);
        }).finally(finishConverter);
    } catch (error) {
        showConvertNotice("error", `Conversion could not start: ${error.message}`);
        finishConverter();
    }
});
function requestConvertCancel() {
    if (!converterApi || convertCancelButton.disabled) return;
    converterApi.cancel();
    convertCancelButton.disabled = true;
    convertStatus.textContent = "Cancelling…";
}
convertCancelButton.addEventListener("click", requestConvertCancel);
document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && converterApi) requestConvertCancel();
});
convertDownloadButton.addEventListener("click", () => {
    if (!convertedFile) return;
    if (convertedDownloadUrl) URL.revokeObjectURL(convertedDownloadUrl);
    convertedDownloadUrl = URL.createObjectURL(convertedFile);
    const link = document.createElement("a");
    link.href = convertedDownloadUrl;
    link.download = convertedOutputName;
    link.click();
});
convertDownloadAttributesButton.addEventListener("click", () => {
    if (!convertedAttributesFile) return;
    if (convertedAttributesDownloadUrl) URL.revokeObjectURL(convertedAttributesDownloadUrl);
    convertedAttributesDownloadUrl = URL.createObjectURL(convertedAttributesFile);
    const link = document.createElement("a");
    link.href = convertedAttributesDownloadUrl;
    link.download = convertedAttributesOutputName;
    link.click();
});
convertOpenButton.addEventListener("click", () => {
    if (convertedFile) openProjectFile(convertedFile, convertedOutputName, convertedAttributesFile);
});

function finishConverter() {
    converterApi?.dispose();
    converterApi = null;
    convertFileInput.disabled = false;
    convertOptionsFields.disabled = false;
    refreshConverterOptions();
    convertCancelButton.hidden = true;
    convertCancelButton.disabled = false;
}

// `file` is a File, or `{ url }` for a file read over HTTP Range (the workers open it; see
// http-range-source.mjs). `attributesFile` follows the same rule.
function openProjectFile(file, displayName = file?.name, attributesFile = undefined) {
    if (!ready || !file) return;
    currentProjectFile = file;
    currentProjectName = displayName;
    pendingProject = { file, name: displayName, attributes: attributesFile };
    shownProject = null;
    resetBenchmarkWindow();
    colorMode.value = "rgb";
    benchmarkRun = {
        datasetName: displayName,
        fileBytes: file.size ?? null,
        attributes: attributesFile ? { name: attributesFile.name ?? attributesFile.url, fileBytes: attributesFile.size ?? null } : null,
        sourcePointCount: null,
        nodeCount: null,
        startedAt: performance.now(),
        startedAtIso: new Date().toISOString(),
        refinement: null,
        initialTargetVoxelPixels: Number(voxelPixels.value),
        metadataOpenMs: null,
        firstCoarseFrameMs: null,
        targetLodMs: null,
        measurementStartedAt: null,
        finalFps: null,
        targetVoxelPixels: Number(voxelPixels.value),
        viewportFramebuffer: [canvas.width, canvas.height],
        devicePixelRatio: window.devicePixelRatio || 1,
        effectivePixelRatio: canvas.clientWidth > 0 ? canvas.width / canvas.clientWidth : 0,
        uniqueBytesRead: "0",
        frameRenderMs: null,
        latestGpuStats: null,
        latestMemoryStats: null,
        cameraChanged: false,
        viewportChanged: false,
        settingsChanged: false,
    };
    frameSamples = [];
    benchmarkArmed = false;
    startBenchmarkButton.disabled = false;
    startBenchmarkButton.textContent = "Run 15 s FPS benchmark";
    downloadBenchmarkButton.disabled = true;
    fileInput.disabled = true;
    attributesFileInput.disabled = true;
    sampleButton.disabled = true;
    fileInput.value = "";
    attributesFileInput.value = "";
    loadingFile = true;
    cancelButton.hidden = false;
    statusRoot.textContent = `Opening ${displayName}…`;
    startRendering();
    worker.postMessage({
        type: "openFile",
        refinementId: requestRefinement(),
        file,
        attributes: attributesFile,
        name: displayName,
        targetVoxelPixels: Number(voxelPixels.value),
        cacheBytes: 256 * 1024 * 1024,
    });
}

fileInput.addEventListener("change", () => {
    const [file] = fileInput.files ?? [];
    const [attributesFile] = attributesFileInput.files ?? [];
    openProjectFile(file, file?.name, attributesFile);
});
attributesFileInput.addEventListener("change", () => {
    const [attributesFile] = attributesFileInput.files ?? [];
    if (currentProjectFile) openProjectFile(currentProjectFile, currentProjectName, attributesFile);
});
function updateAttributeChoices(opened) {
    colorMode.querySelectorAll("option[data-attribute]").forEach((option) => option.remove());
    for (const option of colorMode.options) {
        option.disabled = ["intensity", "classification"].includes(option.value)
            && (!opened.hasAttributes || !(opened.attributeSchema ?? []).some((column) => column.name === option.value && column.componentCount === 1));
    }
    if (!opened.hasAttributes) return;
    for (const [index, descriptor] of (opened.attributeSchema ?? []).entries()) {
        for (let component = 0; component < descriptor.componentCount; component++) {
            const option = document.createElement("option");
            option.dataset.attribute = "true";
            option.value = `attribute:${index}:${component}`;
            option.textContent = descriptor.componentCount > 1 ? `Attribute: ${descriptor.name} [${component}]` : `Attribute: ${descriptor.name}`;
            colorMode.append(option);
        }
    }
}
downloadBenchmarkButton.addEventListener("click", exportBenchmarkRun);
sampleButton.addEventListener("click", async () => {
    if (!ready) return;
    sampleButton.disabled = true;
    fileInput.disabled = true;
    attributesFileInput.disabled = true;
    loadingFile = true;
    cancelButton.hidden = false;
    statusRoot.textContent = "Fetching the built-in sample file…";
    const controller = new AbortController();
    sampleFetchController = controller;
    try {
        const response = await fetch(new URL("./sample.oct", import.meta.url), {
            signal: controller.signal,
        });
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        const file = new File([await response.arrayBuffer()], "web-host-sample.oct", {
            type: "application/octet-stream",
        });
        if (sampleFetchController !== controller) return;
        const attrsResponse = await fetch(new URL("./sample.octattrs", import.meta.url), { signal: controller.signal });
        if (!attrsResponse.ok) throw new Error(`Attributes HTTP ${attrsResponse.status}`);
        const attrs = new File([await attrsResponse.arrayBuffer()], "sample.octattrs");
        if (sampleFetchController !== controller) return;
        sampleFetchController = null;
        openProjectFile(file, file.name, attrs);
    } catch (error) {
        if (sampleFetchController !== controller) return;
        sampleFetchController = null;
        loadingFile = false;
        cancelButton.hidden = true;
        fileInput.disabled = false;
        attributesFileInput.disabled = false;
        sampleButton.disabled = false;
        if (error.name !== "AbortError") {
            statusRoot.textContent = `Sample download failed: ${error}`;
        }
    }
});
fitButton.addEventListener("click", () => {
    queueWorkerCommand("fit");
});
cameraMode.addEventListener("change", () => {
    activeCameraMode = cameraMode.value;
    if (activeCameraMode === "bird") {
        queueWorkerCommand("birdView");
        canvas.focus({ preventScroll: true });
    }
    cameraModeHelp.textContent = activeCameraMode === "bird"
        ? `Bird: drag pans, right drag rotates, WASD/QE moves · speed ${movementSpeed.toFixed(2)} · wheel changes speed.`
        : "Observer: click a voxel to set the center, left drag pans, right drag rotates, wheel changes radius.";
});
function bindCanvasInteractions() {
canvas.addEventListener("pointerdown", (event) => {
    if (!ready || (event.button !== 0 && event.button !== 2)) return;
    event.preventDefault();
    activePointer = event.pointerId;
    pointerStart = [event.clientX, event.clientY];
    pointerMoved = false;
    pointerMode = event.button === 2 || (activeCameraMode === "observer" && event.shiftKey)
        ? "orbit"
        : "pan";
    lastPointer = [event.clientX, event.clientY];
    canvas.setPointerCapture(event.pointerId);
});
canvas.addEventListener("pointermove", (event) => {
    if (!ready || event.pointerId !== activePointer) return;
    const width = Math.max(canvas.clientWidth, 1);
    const height = Math.max(canvas.clientHeight, 1);
    const deltaX = (event.clientX - lastPointer[0]) / width;
    const deltaY = (event.clientY - lastPointer[1]) / height;
    if (Math.hypot(event.clientX - pointerStart[0], event.clientY - pointerStart[1]) >= 4) {
        pointerMoved = true;
    }
    if (pointerMode === "pan") {
        cameraDelta.panX += deltaX;
        cameraDelta.panY += deltaY;
    } else {
        cameraDelta.orbitX += deltaX;
        cameraDelta.orbitY += deltaY;
    }
    lastPointer = [event.clientX, event.clientY];
    wakeRendering();
});
function endPointer(event) {
    if (event.pointerId !== activePointer) return;
    if (event.type === "pointerup" && event.button === 0
        && activeCameraMode === "observer" && !pointerMoved && !pickPending) {
        const rect = canvas.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0 && canvas.width > 0 && canvas.height > 0) {
            pickPending = true;
            worker.postMessage({
                type: "pick",
                cssX: event.clientX - rect.left,
                cssY: event.clientY - rect.top,
                cssWidth: rect.width,
                cssHeight: rect.height,
                recenter: true,
            });
            wakeRendering();
        }
    }
    activePointer = null;
}
canvas.addEventListener("pointerup", endPointer);
canvas.addEventListener("pointercancel", endPointer);
canvas.addEventListener("lostpointercapture", endPointer);
canvas.addEventListener("contextmenu", (event) => event.preventDefault());
canvas.addEventListener("wheel", (event) => {
    if (!ready) return;
    event.preventDefault();
    const deltaUnit = event.deltaMode === WheelEvent.DOM_DELTA_LINE ? 16
        : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? Math.max(canvas.clientHeight, 1) : 1;
    const wheelDelta = event.deltaY * deltaUnit;
    if (activeCameraMode === "bird") {
        movementSpeed = Math.max(0.001, Math.min(1_000_000, movementSpeed * Math.exp(-wheelDelta * 0.001)));
        cameraModeHelp.textContent = `Bird: drag pans, right drag rotates, WASD/QE moves · speed ${movementSpeed.toFixed(2)} · wheel changes speed.`;
    } else {
        cameraDelta.zoom += wheelDelta * 0.001;
    }
    wakeRendering();
}, { passive: false });
canvas.addEventListener("keydown", (event) => {
    if (!ready) return;
    const step = event.shiftKey ? 0.025 : 0.04;
    if (event.key === "Home") {
        queueWorkerCommand("fit");
    } else if (activeCameraMode === "bird" && ["w", "a", "s", "d", "q", "e"].includes(event.key.toLowerCase())) {
        const distance = movementSpeed * 0.05;
        if (event.key.toLowerCase() === "w") cameraDelta.moveY += distance;
        if (event.key.toLowerCase() === "s") cameraDelta.moveY -= distance;
        if (event.key.toLowerCase() === "a") cameraDelta.moveX -= distance;
        if (event.key.toLowerCase() === "d") cameraDelta.moveX += distance;
        if (event.key.toLowerCase() === "q") cameraDelta.moveZ -= distance;
        if (event.key.toLowerCase() === "e") cameraDelta.moveZ += distance;
    } else if (event.key === "+" || event.key === "=") {
        cameraDelta.zoom -= 0.15;
    } else if (event.key === "-") {
        cameraDelta.zoom += 0.15;
    } else if (event.key === "ArrowLeft") {
        if (event.shiftKey) cameraDelta.panX -= step;
        else cameraDelta.orbitX -= step;
    } else if (event.key === "ArrowRight") {
        if (event.shiftKey) cameraDelta.panX += step;
        else cameraDelta.orbitX += step;
    } else if (event.key === "ArrowUp") {
        if (event.shiftKey) cameraDelta.panY -= step;
        else cameraDelta.orbitY -= step;
    } else if (event.key === "ArrowDown") {
        if (event.shiftKey) cameraDelta.panY += step;
        else cameraDelta.orbitY += step;
    } else {
        return;
    }
    event.preventDefault();
    wakeRendering();
});
}
window.addEventListener("resize", resize);

bindCanvasInteractions();
resize();
createWorkerViewer();
