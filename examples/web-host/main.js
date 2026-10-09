import init, { WasmVoxelViewer } from "../../pkg/viewer_wasm.js";

const canvas = document.getElementById("viewer-canvas");
const reloadButton = document.getElementById("reload-scene");
const fitButton = document.getElementById("fit-scene");
const statsRoot = document.getElementById("stats");
const statusRoot = document.getElementById("renderer-status");
const colorMode = document.getElementById("color-mode");
const pointSize = document.getElementById("point-size");
const pointSizeValue = document.getElementById("point-size-value");

await init();

let viewer = null;
let animationFrame = 0;
let lastFrameStatus = "";
let frameCount = 0;
let lastRenderCallMs = null;
let activePointer = null;
let lastPointer = [0, 0];
let pointerMode = "orbit";

function createSamplePointCloud() {
    const positions = [];
    const colors = [];

    for (let x = -8; x <= 8; x += 1) {
        for (let z = -8; z <= 8; z += 1) {
            const height = Math.sin(x * 0.45) * Math.cos(z * 0.35) * 3.0;

            for (let layer = 0; layer < 4; layer += 1) {
                positions.push(x, z, height + layer * 0.35);

                colors.push(
                    Math.round(120 + (x + 8) * 7),
                    Math.round(90 + (z + 8) * 7),
                    Math.round(180 + layer * 18),
                );
            }
        }
    }

    return {
        positions: new Float32Array(positions),
        colors: new Uint8Array(colors),
    };
}

function resize() {
    if (!viewer) return;

    const pixelRatio = window.devicePixelRatio || 1;
    const width = Math.max(0, Math.floor(canvas.clientWidth * pixelRatio));
    const height = Math.max(0, Math.floor(canvas.clientHeight * pixelRatio));

    if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
    }

    viewer.resize(width, height);
}

function updateStats() {
    if (!viewer) return;

    const stats = viewer.stats();
    const gpuStats = viewer.gpuStats();
    statsRoot.textContent = [
        `points: ${stats.point_count}`,
        `visible instances: ${gpuStats.visibleInstances}`,
        `visible blocks: ${gpuStats.visibleBlocks}`,
        `total uploaded bytes: ${gpuStats.uploadedBytes}`,
        `render call: ${lastRenderCallMs === null ? "—" : `${lastRenderCallMs.toFixed(2)} ms`}`,
        `viewport: ${stats.viewport_size[0]} x ${stats.viewport_size[1]}`,
    ].join("\n");
}

function stopRendering() {
    if (animationFrame !== 0) {
        cancelAnimationFrame(animationFrame);
        animationFrame = 0;
    }
}

function renderFrame() {
    try {
        const startedAt = performance.now();
        const status = viewer.renderFrame();
        lastRenderCallMs = performance.now() - startedAt;
        if (status !== lastFrameStatus) {
            statusRoot.textContent = `Renderer: ${status}`;
            lastFrameStatus = status;
        }
        frameCount += 1;
        if (frameCount === 1 || frameCount % 60 === 0) updateStats();
        animationFrame = requestAnimationFrame(renderFrame);
    } catch (error) {
        stopRendering();
        statusRoot.textContent = `Renderer error: ${error}`;
        reloadButton.disabled = false;
        reloadButton.textContent = "Recreate viewer";
    }
}

async function createViewer() {
    stopRendering();
    viewer = null;
    reloadButton.disabled = true;
    statusRoot.textContent = "Initializing WebGPU…";

    try {
        viewer = await WasmVoxelViewer.create(canvas, 0.6);
        viewer.setColorMode(colorMode.value);
        viewer.setPointSize(Number(pointSize.value));
        resize();
        const { positions, colors } = createSamplePointCloud();
        viewer.replacePoints(positions, colors);
        viewer.fitScene();
        frameCount = 0;
        lastRenderCallMs = null;
        lastFrameStatus = "";
        updateStats();
        reloadButton.textContent = "Recreate viewer";
        renderFrame();
    } catch (error) {
        viewer = null;
        statusRoot.textContent = `WebGPU initialization failed: ${error}`;
        reloadButton.textContent = "Retry viewer";
    } finally {
        reloadButton.disabled = false;
    }
}

reloadButton.addEventListener("click", () => void createViewer());
colorMode.addEventListener("change", () => {
    if (viewer) viewer.setColorMode(colorMode.value);
});
pointSize.addEventListener("input", () => {
    const size = Number(pointSize.value);
    pointSizeValue.value = `${size.toFixed(1)}×`;
    if (viewer) viewer.setPointSize(size);
});
fitButton.addEventListener("click", () => {
    if (viewer) viewer.fitScene();
});
canvas.addEventListener("pointerdown", (event) => {
    if (!viewer || event.button !== 0) return;
    event.preventDefault();
    activePointer = event.pointerId;
    pointerMode = event.shiftKey ? "pan" : "orbit";
    lastPointer = [event.clientX, event.clientY];
    canvas.setPointerCapture(event.pointerId);
});
canvas.addEventListener("pointermove", (event) => {
    if (!viewer || event.pointerId !== activePointer) return;
    const width = Math.max(canvas.clientWidth, 1);
    const height = Math.max(canvas.clientHeight, 1);
    const deltaX = (event.clientX - lastPointer[0]) / width;
    const deltaY = (event.clientY - lastPointer[1]) / height;
    if (pointerMode === "pan") viewer.pan(deltaX, deltaY);
    else viewer.orbit(deltaX, deltaY);
    lastPointer = [event.clientX, event.clientY];
});
function endPointer(event) {
    if (event.pointerId === activePointer) activePointer = null;
}
canvas.addEventListener("pointerup", endPointer);
canvas.addEventListener("pointercancel", endPointer);
canvas.addEventListener("lostpointercapture", endPointer);
canvas.addEventListener("wheel", (event) => {
    if (!viewer) return;
    event.preventDefault();
    viewer.zoom(event.deltaY * 0.001);
}, { passive: false });
canvas.addEventListener("keydown", (event) => {
    if (!viewer) return;
    const step = event.shiftKey ? 0.025 : 0.04;
    if (event.key === "Home") {
        viewer.fitScene();
    } else if (event.key === "+" || event.key === "=") {
        viewer.zoom(-0.15);
    } else if (event.key === "-") {
        viewer.zoom(0.15);
    } else if (event.key === "ArrowLeft") {
        if (event.shiftKey) viewer.pan(-step, 0);
        else viewer.orbit(-step, 0);
    } else if (event.key === "ArrowRight") {
        if (event.shiftKey) viewer.pan(step, 0);
        else viewer.orbit(step, 0);
    } else if (event.key === "ArrowUp") {
        if (event.shiftKey) viewer.pan(0, -step);
        else viewer.orbit(0, -step);
    } else if (event.key === "ArrowDown") {
        if (event.shiftKey) viewer.pan(0, step);
        else viewer.orbit(0, step);
    } else {
        return;
    }
    event.preventDefault();
});
window.addEventListener("resize", resize);

await createViewer();
