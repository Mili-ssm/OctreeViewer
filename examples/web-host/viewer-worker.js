import init, { WasmVoxelViewer } from "../../pkg/viewer_wasm.js";

let viewer = null;
let profile = "quality";
let renderedFrames = 0;
const coalescedCommands = new Set(["resize", "color", "pointSize", "depthShading", "profile", "fit"]);

function samplePointCloud() {
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

function postStats(frameStatus = null) {
    self.postMessage({
        type: "stats",
        frameStatus,
        gpu: viewer.gpuStats(),
        lod: viewer.lodStats(),
        viewer: viewer.stats(),
    });
}

async function handleMessage({ data }) {
    try {
        switch (data.type) {
            case "init": {
                await init();
                viewer = await WasmVoxelViewer.createOffscreen(data.canvas, 0.6);
                viewer.resize(data.width, data.height);
                viewer.setColorMode(data.colorMode);
                viewer.setPointSize(data.pointSize);
                viewer.setDepthShading(data.depthShading);
                const sample = samplePointCloud();
                viewer.replacePoints(sample.positions, sample.colors);
                viewer.fitScene();
                self.postMessage({ type: "ready" });
                break;
            }
            case "resize":
                viewer.resize(data.width, data.height);
                break;
            case "color":
                viewer.setColorMode(data.value);
                break;
            case "pointSize":
                viewer.setPointSize(data.value);
                break;
            case "depthShading":
                viewer.setDepthShading(data.value);
                break;
            case "profile": {
                profile = data.value;
                const lod = viewer.updateLod(profile);
                self.postMessage({ type: "lod", lod });
                break;
            }
            case "openFile": {
                profile = data.profile;
                self.postMessage({ type: "status", message: `Opening ${data.file.name}…` });
                const opened = viewer.openFormatDataset(data.file, data.cacheBytes);
                self.postMessage({ type: "datasetOpened", opened, name: data.file.name });
                viewer.fitScene();
                const lod = viewer.updateLod(profile);
                self.postMessage({ type: "dataset", opened, lod, name: data.file.name });
                break;
            }
            case "fit":
                viewer.fitScene();
                break;
            case "frame": {
                const camera = data.camera;
                if (camera.orbitX !== 0 || camera.orbitY !== 0) {
                    viewer.orbit(camera.orbitX, camera.orbitY);
                }
                if (camera.panX !== 0 || camera.panY !== 0) {
                    viewer.pan(camera.panX, camera.panY);
                }
                if (camera.zoom !== 0) viewer.zoom(camera.zoom);
                const startedAt = performance.now();
                const frameStatus = viewer.renderFrame();
                const renderCallMs = performance.now() - startedAt;
                renderedFrames += 1;
                if (renderedFrames % 30 === 0) {
                    postStats({ status: frameStatus, renderCallMs });
                } else {
                    self.postMessage({ type: "frame", frameStatus, renderCallMs });
                }
                break;
            }
            default:
                throw new Error(`unknown worker message: ${data.type}`);
        }
    } catch (error) {
        self.postMessage({ type: "error", operation: data.type, message: String(error) });
    } finally {
        if (coalescedCommands.has(data.type)) {
            self.postMessage({ type: "commandAck", command: data.type });
        }
    }
}

self.addEventListener("message", (event) => void handleMessage(event));
