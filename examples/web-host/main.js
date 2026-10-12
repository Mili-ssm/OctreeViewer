import { CoalescedCommandQueue } from "./coalesced-command-queue.mjs";
import { FRAME_RATE_WINDOW_MS, FRAME_RATE_WARMUP_MS, measureFrameRate } from "./benchmark-metrics.mjs";
import { framebufferSize } from "./rendering-budget.mjs";
import { createConverter } from "./converter-api.mjs";

let canvas = document.getElementById("viewer-canvas");
const reloadButton = document.getElementById("reload-scene");
const cancelButton = document.getElementById("cancel-load");
const sampleButton = document.getElementById("open-sample");
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
const statsRoot = document.getElementById("stats");
const statusRoot = document.getElementById("renderer-status");
const startBenchmarkButton = document.getElementById("start-benchmark");
const downloadBenchmarkButton = document.getElementById("download-benchmark");
const colorMode = document.getElementById("color-mode");
const voxelPixels = document.getElementById("voxel-pixels");
const voxelPixelsValue = document.getElementById("voxel-pixels-value");
const depthShading = document.getElementById("depth-shading");
const cameraMode = document.getElementById("camera-mode");
const cameraModeHelp = document.getElementById("camera-mode-help");

let worker = null;
let converterApi = null;
let convertedFile = null;
let currentProjectFile = null;
let currentProjectName = null;
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
        || (!lod.lodError && (benchmarkRun?.refinement?.targetLodMs === null
            || lod.pendingBlockIds.length > 0 || lod.lodStagePixels > lod.targetVoxelPixels));
    cancelButton.hidden = !loadingFile;
    if (loadingFile) startRendering();
    else if (!benchmarkStopTimer) stopRendering();
}

function formatFrameRenderMs(metrics) {
    if (!metrics || metrics.sampleCount === 0) return "—";
    return `p50/p95/p99 ${metrics.p50Ms.toFixed(2)}/${metrics.p95Ms.toFixed(2)}/${metrics.p99Ms.toFixed(2)} ms (${metrics.sampleCount} frames)`;
}

function recordFrameReply(renderCallMs) {
    frameSamples.push({ time: performance.now(), renderCallMs });
    if (frameSamples.length > 2_048) frameSamples.splice(0, 1_024);
}

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
        },
        steadyState: {
            warmupAfterBenchmarkStartMs: FRAME_RATE_WARMUP_MS,
            measurementWindowMs: fps.windowDurationMs,
            frameReplySpanMs: fps.durationMs,
            frameReplies: fps.sampleCount,
            meanFps: fps.meanFps,
            frameReplyIntervalP50Ms: fps.intervalP50Ms,
            frameReplyIntervalP95Ms: fps.intervalP95Ms,
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
        environment: {
            userAgent: navigator.userAgent,
            hardwareConcurrency: navigator.hardwareConcurrency ?? null,
            deviceMemoryGiB: navigator.deviceMemory ?? null,
            webGpuAvailable: Boolean(navigator.gpu),
            jsHeapUsedBytes: performance.memory?.usedJSHeapSize ?? null,
            gpuStats: gpu,
        },
        notes: [
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
        `finest voxel detail target: ${lod?.targetVoxelPixels ?? voxelPixels.value} px`,
        `LOD stage: ${lod?.lodStagePixels ?? "—"} px`,
        `rendered voxel limit: ${lod?.voxelBudget ?? "—"}`,
        `projected voxel edges: ${lod ? `${lod.minVoxelEdgePixels.toFixed(2)}–${lod.maxVoxelEdgePixels.toFixed(2)} px` : "—"}`,
        `voxels above target: ${lod?.oversizedVoxels ?? "—"}`,
        `visible points: ${points}`,
        `visible voxels: ${lod?.renderedVoxels ?? message.gpu.visibleInstances}`,
        `HZB occlusion: ${(lod?.hzbActive ?? message.gpu.hzbActive) ? "active" : "inactive"}`,
        `visible blocks: ${message.gpu.visibleBlocks}`,
        `deepest visible LOD level: ${lod?.maxVisibleLevel ?? "—"}`,
        `LOD coverage: ${coverage}`,
        `pending blocks: ${lod?.pendingBlockIds.length ?? 0} · evicted targets ${lod?.refinementWork?.evictedPendingBlocks ?? "—"}`,
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
    } else if (latestLodStats?.coverageComplete
        && latestLodStats.pendingBlockIds.length === 0
        && latestLodStats.lodStagePixels <= latestLodStats.targetVoxelPixels) {
        beginBenchmarkWindow();
    } else {
        benchmarkArmed = false;
        startBenchmarkButton.disabled = false;
        startBenchmarkButton.textContent = "Run 15 s FPS benchmark";
    }
}

function handleWorkerMessage(event) {
    if (event.currentTarget !== worker) return;
    const { data } = event;
    if (data.type === "ready") {
        ready = true;
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
        requestFrameOnce();
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
        statusRoot.textContent = `${data.name}: ${data.opened.nodeCount} nodes · selecting visible blocks…`;
    } else if (data.type === "lod") {
        latestLodStats = data.lod;
        updateProgressiveLoadState(data.lod);
        statusRoot.textContent = `LOD stage ${data.lod.lodStagePixels} px · finest target ${data.lod.targetVoxelPixels} px`;
    } else if (data.type === "lodProgress") {
        if (data.refinement?.id !== requestedRefinementId) return;
        observedRefinementId = data.refinement.id;
        latestLodStats = data.lod;
        if (benchmarkRun) {
            benchmarkRun.refinement = data.refinement;
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
        const targetView = data.targetLodMs === null
            ? "target refining"
            : `target reached ${data.targetLodMs.toFixed(0)} ms`;
        statusRoot.textContent = `${firstView} · ${targetView} · stage ${data.lod.lodStagePixels} px`;
    } else if (data.type === "stats") {
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
    } else if (data.type === "pick") {
        pickPending = false;
        if (data.hit) {
            canvas.dispatchEvent(new CustomEvent("voxelviewer:pick", {
                detail: { ...data.hit, sceneGeneration: data.sceneGeneration },
            }));
        }
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

function createWorkerViewer() {
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

function recreateViewer() {
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
    const replacement = canvas.cloneNode(false);
    replacement.width = canvas.width;
    replacement.height = canvas.height;
    canvas.replaceWith(replacement);
    canvas = replacement;
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

reloadButton.addEventListener("click", recreateViewer);
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
depthShading.addEventListener("change", () => {
    if (benchmarkRun) benchmarkRun.settingsChanged = true;
    queueWorkerCommand("depthShading", { value: depthShading.checked });
});
convertFileInput.addEventListener("change", () => {
    convertButton.disabled = !convertFileInput.files?.[0] || Boolean(converterApi);
});
convertButton.addEventListener("click", () => {
    const file = convertFileInput.files?.[0];
    if (!file || converterApi) return;
    if (!/\.(las|laz)$/i.test(file.name)) {
        convertStatus.textContent = "Choose a .las or .laz file.";
        return;
    }
    convertedFile = null;
    convertedAttributesFile = null;
    if (convertedDownloadUrl) URL.revokeObjectURL(convertedDownloadUrl);
    if (convertedAttributesDownloadUrl) URL.revokeObjectURL(convertedAttributesDownloadUrl);
    convertedDownloadUrl = null;
    convertedAttributesDownloadUrl = null;
    convertDownloadButton.hidden = true;
    convertDownloadAttributesButton.hidden = true;
    convertOpenButton.hidden = true;
    convertProgress.hidden = false;
    convertProgress.value = 0;
    convertProgress.max = 1;
    convertButton.disabled = true;
    convertFileInput.disabled = true;
    convertCancelButton.hidden = false;
    convertStatus.textContent = `Preparing local conversion of ${file.name}…`;
    try {
        converterApi = createConverter();
        converterApi.on("status", ({ message }) => { convertStatus.textContent = message; });
        converterApi.on("progress", ({ processedPoints, totalPoints, sourceName, stage }) => {
            const processed = Number(processedPoints);
            const total = Number(totalPoints);
            convertProgress.max = total || 1;
            convertProgress.value = processed;
            const phase = stage === "writing" ? "Writing spatial LOD" : "Planning spatial LOD";
            convertStatus.textContent = `${phase} · ${sourceName}: ${processed.toLocaleString()} / ${total.toLocaleString()} points`;
        });
        converterApi.convert(file).then(({ file: resultFile, name, attributesFile, outputAttributesName, points, timings }) => {
            convertedFile = resultFile;
            convertedAttributesFile = attributesFile;
            convertedOutputName = name;
            convertedAttributesOutputName = outputAttributesName;
            convertProgress.value = convertProgress.max;
            const planning = ((timings?.planningMs ?? 0) / 1_000).toFixed(1);
            const writing = ((timings?.writingMs ?? 0) / 1_000).toFixed(1);
            const total = ((timings?.totalMs ?? 0) / 1_000).toFixed(1);
            convertStatus.textContent = `Converted ${points} points to ${name} · planning ${planning}s · writing ${writing}s · total ${total}s.`;
            convertDownloadButton.hidden = false;
            convertDownloadAttributesButton.hidden = !attributesFile;
            convertOpenButton.hidden = false;
        }).catch((error) => {
            convertStatus.textContent = error.name === "AbortError"
                ? error.message : `Conversion failed: ${error.message}`;
        }).finally(finishConverter);
    } catch (error) {
        convertStatus.textContent = `Conversion could not start: ${error.message}`;
        finishConverter();
    }
});
convertCancelButton.addEventListener("click", () => {
    converterApi?.cancel();
    convertCancelButton.disabled = true;
    convertStatus.textContent = "Cancelling after the current bounded block…";
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
    convertButton.disabled = !convertFileInput.files?.[0];
    convertFileInput.disabled = false;
    convertCancelButton.hidden = true;
    convertCancelButton.disabled = false;
}

function openProjectFile(file, displayName = file?.name, attributesFile = undefined) {
    if (!ready || !file) return;
    currentProjectFile = file;
    currentProjectName = displayName;
    resetBenchmarkWindow();
    colorMode.value = "rgb";
    benchmarkRun = {
        datasetName: displayName,
        fileBytes: file.size,
        attributes: attributesFile ? { name: attributesFile.name, fileBytes: attributesFile.size } : null,
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
    if (activeCameraMode === "bird") queueWorkerCommand("birdView");
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
