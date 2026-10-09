import init, { WasmVoxelViewer } from "../../pkg/viewer_wasm.js";

const canvas = document.getElementById("viewer-canvas");
const reloadButton = document.getElementById("reload-scene");
const statsRoot = document.getElementById("stats");
const statusRoot = document.getElementById("renderer-status");

await init();

let viewer = null;
let animationFrame = 0;
let lastFrameStatus = "";
let frameCount = 0;

function createSamplePointCloud() {
    const positions = [];
    const colors = [];

    for (let x = -8; x <= 8; x += 1) {
        for (let z = -8; z <= 8; z += 1) {
            const height = Math.sin(x * 0.45) * Math.cos(z * 0.35) * 3.0;

            for (let layer = 0; layer < 4; layer += 1) {
                positions.push(x, height + layer * 0.35, z);

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

function viewProjection() {
    const aspect = canvas.height === 0 ? 1 : canvas.width / canvas.height;
    const horizontalScale = 0.06 * Math.min(1, aspect);

    // Fixed oblique view for this synthetic sample; camera controls are a separate feature.
    return [
        horizontalScale, 0, 0.01, 0,
        0, -0.08, 0, 0,
        -horizontalScale, 0, 0.01, 0,
        0, 0, 0.5, 1,
    ];
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
        `instances: ${stats.instance_count}`,
        `uploaded bytes: ${gpuStats.uploadedBytes}`,
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
        const status = viewer.renderFrame(viewProjection());
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
        resize();
        const { positions, colors } = createSamplePointCloud();
        viewer.replacePoints(positions, colors);
        frameCount = 0;
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
window.addEventListener("resize", resize);

await createViewer();
