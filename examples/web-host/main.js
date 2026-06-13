import init, { WasmVoxelViewer } from "../../pkg/viewer_wasm.js";

const canvas = document.getElementById("viewer-canvas");
const reloadButton = document.getElementById("reload-scene");
const statsRoot = document.getElementById("stats");
const context = canvas.getContext("2d");

if (!context) {
    throw new Error("2D canvas context is not available");
}

await init();

const viewer = new WasmVoxelViewer(0.6);

function createSamplePointCloud() {
    const positions = [];
    const colors = [];

    for (let x = -8; x <= 8; x += 1) {
        for (let z = -8; z <= 8; z += 1) {
            const height = Math.sin(x * 0.45) * Math.cos(z * 0.35) * 3.0;

            for (let layer = 0; layer < 4; layer += 1) {
                positions.push(x, height + layer * 0.35, z);

                const r = Math.round(120 + (x + 8) * 7);
                const g = Math.round(90 + (z + 8) * 7);
                const b = Math.round(180 + layer * 18);
                colors.push(r, g, b);
            }
        }
    }

    return {
        positions: new Float32Array(positions),
        colors: new Uint8Array(colors),
    };
}

function resize() {
    const pixelRatio = window.devicePixelRatio || 1;
    const width = Math.max(1, Math.floor(canvas.clientWidth * pixelRatio));
    const height = Math.max(1, Math.floor(canvas.clientHeight * pixelRatio));

    if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
    }

    viewer.resize(width, height);
}

function renderDebugProjection() {
    const scene = viewer.renderScene();
    const stats = viewer.stats();

    context.clearRect(0, 0, canvas.width, canvas.height);
    context.fillStyle = "#020617";
    context.fillRect(0, 0, canvas.width, canvas.height);

    const scale = Math.min(canvas.width, canvas.height) / 24;
    const halfWidth = canvas.width / 2;
    const halfHeight = canvas.height / 2;

    for (const instance of scene.instances) {
        const [ x, y, z ] = instance.center;
        const screenX = halfWidth + (x - z) * scale * 0.75;
        const screenY = halfHeight + (x + z) * scale * 0.3 - y * scale * 1.1;
        const size = Math.max(2, instance.size * scale * 0.6);

        const [ r, g, b ] = instance.color.map((component, index) =>
            index < 3 ? Math.round(component * 255) : component,
        );

        context.fillStyle = `rgb(${r}, ${g}, ${b})`;
        context.fillRect(screenX - size * 0.5, screenY - size * 0.5, size, size);
    }

    statsRoot.innerHTML = [
        `points: ${stats.point_count}`,
        `voxels: ${stats.voxel_count}`,
        `instances: ${stats.instance_count}`,
        `viewport: ${stats.viewport_size[ 0 ]} x ${stats.viewport_size[ 1 ]}`,
    ]
        .map((line) => `<div>${line}</div>`)
        .join("");
}

function loadScene() {
    const { positions, colors } = createSamplePointCloud();
    viewer.replacePoints(positions, colors);
    renderDebugProjection();
}

reloadButton.addEventListener("click", loadScene);
window.addEventListener("resize", () => {
    resize();
    renderDebugProjection();
});

resize();
loadScene();
