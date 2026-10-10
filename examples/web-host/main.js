import { CoalescedCommandQueue } from "./coalesced-command-queue.mjs";

let canvas = document.getElementById("viewer-canvas");
const reloadButton = document.getElementById("reload-scene");
const cancelButton = document.getElementById("cancel-load");
const sampleButton = document.getElementById("open-sample");
const fitButton = document.getElementById("fit-scene");
const fileInput = document.getElementById("dataset-file");
const statsRoot = document.getElementById("stats");
const statusRoot = document.getElementById("renderer-status");
const colorMode = document.getElementById("color-mode");
const voxelPixels = document.getElementById("voxel-pixels");
const voxelPixelsValue = document.getElementById("voxel-pixels-value");
const depthShading = document.getElementById("depth-shading");

let worker = null;
let canvasTransferred = false;
let viewportSize = null;
const commandQueue = new CoalescedCommandQueue((type, values) => {
    worker.postMessage({ type, ...values });
});
let animationFrame = 0;
let framePending = false;
let ready = false;
let frameFailed = false;
let lastFrameStatus = "";
let renderCallMs = null;
let activePointer = null;
let lastPointer = [0, 0];
let pointerMode = "orbit";
let cameraDelta = emptyCameraDelta();
let latestLodStats = null;
let datasetName = "Synthetic sample";
let loadingFile = false;
let sampleFetchController = null;

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

function emptyCameraDelta() {
    return { orbitX: 0, orbitY: 0, panX: 0, panY: 0, zoom: 0 };
}

function queueWorkerCommand(type, values = {}) {
    if (!ready || frameFailed) return;
    commandQueue.enqueue(type, values);
}

function resize() {
    const pixelRatio = window.devicePixelRatio || 1;
    const width = Math.max(0, Math.floor(canvas.clientWidth * pixelRatio));
    const height = Math.max(0, Math.floor(canvas.clientHeight * pixelRatio));

    if (!canvasTransferred && (canvas.width !== width || canvas.height !== height)) {
        canvas.width = width;
        canvas.height = height;
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
        `target voxel edge: ${lod?.targetVoxelPixels ?? voxelPixels.value} px`,
        `LOD source-point cap: ${lod?.pointBudget ?? "—"}`,
        `projected voxel edges: ${lod ? `${lod.minVoxelEdgePixels.toFixed(2)}–${lod.maxVoxelEdgePixels.toFixed(2)} px` : "—"}`,
        `voxels above target: ${lod?.oversizedVoxels ?? "—"}`,
        `visible points: ${points}`,
        `visible voxels: ${message.gpu.visibleInstances}`,
        `visible blocks: ${message.gpu.visibleBlocks}`,
        `deepest visible LOD level: ${lod?.maxVisibleLevel ?? "—"}`,
        `LOD coverage: ${coverage}`,
        `pending blocks: ${lod?.pendingBlockIds.length ?? 0}`,
        `decoded cache: ${cache}`,
        `uploaded instance bytes: ${message.gpu.uploadedBytes}`,
        `worker render call: ${renderCallMs === null ? "—" : `${renderCallMs.toFixed(2)} ms`}`,
        `viewport: ${viewportSize?.join(" x ") ?? "—"}`,
    ].join("\n");
}

function requestFrame() {
    if (!ready || frameFailed) return;
    if (!framePending) {
        framePending = true;
        const camera = cameraDelta;
        cameraDelta = emptyCameraDelta();
        worker.postMessage({ type: "frame", camera });
    }
    animationFrame = requestAnimationFrame(requestFrame);
}

function startRendering() {
    if (animationFrame === 0) animationFrame = requestAnimationFrame(requestFrame);
}

function stopRendering() {
    if (animationFrame !== 0) {
        cancelAnimationFrame(animationFrame);
        animationFrame = 0;
    }
}

function handleWorkerMessage(event) {
    if (event.currentTarget !== worker) return;
    const { data } = event;
    if (data.type === "ready") {
        ready = true;
        fileInput.disabled = false;
        sampleButton.disabled = false;
        reloadButton.textContent = "Recreate viewer";
        if (viewportSize) {
            queueWorkerCommand("resize", {
                width: viewportSize[0],
                height: viewportSize[1],
            });
        }
        statusRoot.textContent = "Renderer ready · synthetic sample";
        startRendering();
    } else if (data.type === "commandAck") {
        commandQueue.acknowledge(data.command);
    } else if (data.type === "status") {
        statusRoot.textContent = data.message;
    } else if (data.type === "fileReadProgress") {
        const read = Number(data.uniqueBytes);
        const total = Number(data.fileBytes);
        loadingFile = true;
        cancelButton.hidden = false;
        statusRoot.textContent = `Read ranges: ${formatBytes(read)} of ${formatBytes(total)} (unique bytes)`;
    } else if (data.type === "dataset") {
        loadingFile = false;
        cancelButton.hidden = true;
        datasetName = data.name;
        latestLodStats = data.lod;
        fileInput.disabled = false;
        sampleButton.disabled = false;
        voxelPixels.disabled = false;
        statusRoot.textContent = `${data.name}: loaded ${data.opened.nodeCount} nodes`;
    } else if (data.type === "datasetOpened") {
        datasetName = data.name;
        statusRoot.textContent = `${data.name}: ${data.opened.nodeCount} nodes · selecting visible blocks…`;
    } else if (data.type === "lod") {
        latestLodStats = data.lod;
        statusRoot.textContent = `Target voxel edge: ${data.lod.targetVoxelPixels} px`;
    } else if (data.type === "stats") {
        loadingFile = false;
        cancelButton.hidden = true;
        framePending = false;
        renderCallMs = data.frameStatus?.renderCallMs ?? renderCallMs;
        if (data.frameStatus?.status !== lastFrameStatus) {
            lastFrameStatus = data.frameStatus?.status ?? "";
            if (lastFrameStatus) statusRoot.textContent = `Renderer: ${lastFrameStatus}`;
        }
        latestLodStats = data.lod ?? latestLodStats;
        updateStats(data);
    } else if (data.type === "frame") {
        loadingFile = false;
        cancelButton.hidden = true;
        framePending = false;
        renderCallMs = data.renderCallMs;
        if (data.frameStatus !== lastFrameStatus) {
            lastFrameStatus = data.frameStatus;
            statusRoot.textContent = `Renderer: ${lastFrameStatus}`;
        }
    } else if (data.type === "error") {
        loadingFile = false;
        cancelButton.hidden = true;
        fileInput.disabled = false;
        sampleButton.disabled = false;
        statusRoot.textContent = `${data.operation} failed: ${data.message}`;
        if (data.operation === "frame" || data.operation === "init") {
            frameFailed = true;
            stopRendering();
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
        worker.addEventListener("error", (event) => {
            if (event.currentTarget !== worker) return;
            frameFailed = true;
            loadingFile = false;
            cancelButton.hidden = true;
            stopRendering();
            statusRoot.textContent = `Renderer worker failed: ${event.message}. Recreate the viewer to retry.`;
        });
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
colorMode.addEventListener("change", () => {
    queueWorkerCommand("color", { value: colorMode.value });
});
function updateLodControls() {
    voxelPixelsValue.value = `${voxelPixels.value} px`;
    if (ready && latestLodStats) {
        queueWorkerCommand("voxelPixels", { value: Number(voxelPixels.value) });
    }
}

voxelPixels.addEventListener("input", updateLodControls);
depthShading.addEventListener("change", () => {
    queueWorkerCommand("depthShading", { value: depthShading.checked });
});
function openProjectFile(file) {
    if (!ready || !file) return;
    fileInput.disabled = true;
    sampleButton.disabled = true;
    fileInput.value = "";
    loadingFile = true;
    cancelButton.hidden = false;
    statusRoot.textContent = `Opening ${file.name}…`;
    worker.postMessage({
        type: "openFile",
        file,
        targetVoxelPixels: Number(voxelPixels.value),
        cacheBytes: 256 * 1024 * 1024,
    });
}

fileInput.addEventListener("change", () => {
    const [file] = fileInput.files ?? [];
    openProjectFile(file);
});
sampleButton.addEventListener("click", async () => {
    if (!ready) return;
    sampleButton.disabled = true;
    fileInput.disabled = true;
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
        sampleFetchController = null;
        openProjectFile(file);
    } catch (error) {
        if (sampleFetchController !== controller) return;
        sampleFetchController = null;
        loadingFile = false;
        cancelButton.hidden = true;
        fileInput.disabled = false;
        sampleButton.disabled = false;
        if (error.name !== "AbortError") {
            statusRoot.textContent = `Sample download failed: ${error}`;
        }
    }
});
fitButton.addEventListener("click", () => {
    queueWorkerCommand("fit");
});
function bindCanvasInteractions() {
canvas.addEventListener("pointerdown", (event) => {
    if (!ready || event.button !== 0) return;
    event.preventDefault();
    activePointer = event.pointerId;
    pointerMode = event.shiftKey ? "pan" : "orbit";
    lastPointer = [event.clientX, event.clientY];
    canvas.setPointerCapture(event.pointerId);
});
canvas.addEventListener("pointermove", (event) => {
    if (!ready || event.pointerId !== activePointer) return;
    const width = Math.max(canvas.clientWidth, 1);
    const height = Math.max(canvas.clientHeight, 1);
    const deltaX = (event.clientX - lastPointer[0]) / width;
    const deltaY = (event.clientY - lastPointer[1]) / height;
    if (pointerMode === "pan") {
        cameraDelta.panX += deltaX;
        cameraDelta.panY += deltaY;
    } else {
        cameraDelta.orbitX += deltaX;
        cameraDelta.orbitY += deltaY;
    }
    lastPointer = [event.clientX, event.clientY];
});
function endPointer(event) {
    if (event.pointerId === activePointer) activePointer = null;
}
canvas.addEventListener("pointerup", endPointer);
canvas.addEventListener("pointercancel", endPointer);
canvas.addEventListener("lostpointercapture", endPointer);
canvas.addEventListener("wheel", (event) => {
    if (!ready) return;
    event.preventDefault();
    cameraDelta.zoom += event.deltaY * 0.001;
}, { passive: false });
canvas.addEventListener("keydown", (event) => {
    if (!ready) return;
    const step = event.shiftKey ? 0.025 : 0.04;
    if (event.key === "Home") {
        queueWorkerCommand("fit");
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
});
}
window.addEventListener("resize", resize);

bindCanvasInteractions();
resize();
createWorkerViewer();
