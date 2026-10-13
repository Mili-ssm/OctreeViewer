// Bounded clear-only or fixed WASM-sample submissions, without retained frame history.
let device;
let context;
let viewer;
let submitted = 0;
function watchDevice() {
    device.lost.then(info => self.postMessage({ type: "failure", message: `device lost: ${info.message}` }));
    device.addEventListener("uncapturederror", event => {
        self.postMessage({ type: "failure", message: event.error.message });
    });
}
self.onmessage = async ({ data }) => {
    try {
        if (data.type === "init") {
            if (data.mode === "viewer") {
                const { default: init, WasmVoxelViewer } = await import("../../../pkg/viewer_wasm.js");
                await init();
                // wgpu 24 web queue completion is unimplemented. Capture the actual browser
                // device only in this diagnostic, then restore the prototype immediately.
                const requestDevice = GPUAdapter.prototype.requestDevice;
                GPUAdapter.prototype.requestDevice = async function (...args) {
                    device = await requestDevice.apply(this, args);
                    return device;
                };
                try {
                    viewer = await WasmVoxelViewer.createOffscreen(data.canvas, 0.6);
                } finally {
                    GPUAdapter.prototype.requestDevice = requestDevice;
                }
                if (!device) throw new Error("Browser device capture failed");
                watchDevice();
                const coordinates = await fetch(new URL("../sample.oct", import.meta.url)).then(response => response.blob());
                const attributes = await fetch(new URL("../sample.octattrs", import.meta.url)).then(response => response.blob());
                viewer.openFormatDataset(coordinates, 256 * 1024 * 1024, attributes);
                viewer.fitScene();
                let lod;
                for (let attempt = 0; attempt < 20; attempt++) {
                    lod = viewer.updateLod(4);
                    if (lod.coverageComplete && lod.pendingBlockIds.length === 0 && lod.lodStagePixels <= 4) break;
                }
                if (!lod.coverageComplete || lod.pendingBlockIds.length || lod.lodStagePixels > 4) {
                    throw new Error("Small sample target incomplete");
                }
                self.postMessage({ type: "ready", renderedVoxels: lod.renderedVoxels });
                return;
            }
            const adapter = await navigator.gpu.requestAdapter();
            if (!adapter) throw new Error("No WebGPU adapter");
            device = await adapter.requestDevice();
            watchDevice();
            context = data.canvas.getContext("webgpu");
            context.configure({ device, format: navigator.gpu.getPreferredCanvasFormat(), alphaMode: "opaque" });
            self.postMessage({ type: "ready" });
        } else if (data.type === "frame") {
            if (viewer) {
                const status = viewer.renderFrame();
                if (status !== "presented") throw new Error(`WASM frame did not present: ${status}`);
                self.postMessage({ type: "frame", submitted: ++submitted });
                return;
            }
            const texture = context.getCurrentTexture();
            const encoder = device.createCommandEncoder();
            const pass = encoder.beginRenderPass({ colorAttachments: [{
                view: texture.createView(), loadOp: "clear", storeOp: "store",
                clearValue: { r: 0.7, g: 0.1, b: 0.1, a: 1 },
            }] });
            pass.end();
            device.queue.submit([encoder.finish()]);
            self.postMessage({ type: "frame", submitted: ++submitted });
        } else if (data.type === "finish") {
            await device.queue.onSubmittedWorkDone();
            self.postMessage({ type: "idle", submitted, gpu: viewer?.gpuStats() ?? null });
        } else {
            throw new Error(`Unknown diagnostic command ${data.type}`);
        }
    } catch (error) {
        self.postMessage({ type: "failure", message: String(error) });
    }
};
