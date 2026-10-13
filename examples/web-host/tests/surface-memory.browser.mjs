// Independent surface submission loop: no WASM, dataset, readback or retained frame history.
let device;
let context;
let submitted = 0;
self.onmessage = async ({ data }) => {
    try {
        if (data.type === "init") {
            const adapter = await navigator.gpu.requestAdapter();
            if (!adapter) throw new Error("No WebGPU adapter");
            device = await adapter.requestDevice();
            device.lost.then(info => self.postMessage({ type: "failure", message: `device lost: ${info.message}` }));
            device.addEventListener("uncapturederror", event => {
                self.postMessage({ type: "failure", message: event.error.message });
            });
            context = data.canvas.getContext("webgpu");
            context.configure({ device, format: navigator.gpu.getPreferredCanvasFormat(), alphaMode: "opaque" });
            self.postMessage({ type: "ready" });
        } else if (data.type === "frame") {
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
            self.postMessage({ type: "idle", submitted });
        } else {
            throw new Error(`Unknown diagnostic command ${data.type}`);
        }
    } catch (error) {
        self.postMessage({ type: "failure", message: String(error) });
    }
};
