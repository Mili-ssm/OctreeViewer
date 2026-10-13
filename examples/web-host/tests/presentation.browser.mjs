// Worker diagnostic: independent WebGPU clear/readback versus the real WASM scene.
function assert(condition, message) {
    if (!condition) throw new Error(message);
}

async function clearProbe(canvas) {
    const adapter = await navigator.gpu.requestAdapter();
    assert(adapter, "no WebGPU adapter");
    const device = await adapter.requestDevice();
    device.lost.then(info => self.postMessage({ type: "deviceLost", reason: info.reason, message: info.message }));
    device.addEventListener("uncapturederror", event => {
        self.postMessage({ type: "gpuError", message: event.error.message });
    });
    const format = navigator.gpu.getPreferredCanvasFormat();
    const context = canvas.getContext("webgpu");
    context.configure({ device, format, alphaMode: "opaque", usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC });
    const texture = context.getCurrentTexture();
    const readback = device.createBuffer({ size: 256, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
    const encoder = device.createCommandEncoder();
    const pass = encoder.beginRenderPass({ colorAttachments: [{
        view: texture.createView(), loadOp: "clear", storeOp: "store", clearValue: { r: 1, g: 0, b: 0, a: 1 },
    }] });
    pass.end();
    encoder.copyTextureToBuffer({ texture }, { buffer: readback, bytesPerRow: 256 }, [1, 1, 1]);
    device.queue.submit([encoder.finish()]);
    await readback.mapAsync(GPUMapMode.READ);
    const pixel = [...new Uint8Array(readback.getMappedRange(), 0, 4)];
    readback.unmap();
    readback.destroy();
    const expected = format === "bgra8unorm" ? [0, 0, 255, 255] : [255, 0, 0, 255];
    assert(pixel.every((value, index) => value === expected[index]), `clear readback mismatch: ${pixel}`);
    // Keep references alive for the diagnostic lifetime; GPU readback alone does not prove presentation.
    self.probeResources = { device, context };
    return { format, pixel, adapter: { vendor: adapter.info?.vendor, architecture: adapter.info?.architecture } };
}

function checkResourceStats(gpu, width, height) {
    const memory = gpu.requestedResources;
    assert(memory && ["instanceBufferBytes", "fixedBufferBytes", "hzbBufferBytes",
        "depthTextureBytes", "hzbTextureBytes", "totalBytes"].every(key => /^\d+$/.test(memory[key])),
        "requested resource bytes must be decimal strings");
    const sum = ["instanceBufferBytes", "fixedBufferBytes", "hzbBufferBytes",
        "depthTextureBytes", "hzbTextureBytes"].reduce((total, key) => total + BigInt(memory[key]), 0n);
    assert(sum === BigInt(memory.totalBytes), "requested resource total mismatch");
    assert(BigInt(memory.depthTextureBytes) === BigInt(width) * BigInt(height) * 4n,
        "depth texture does not match framebuffer");
    assert(BigInt(memory.instanceBufferBytes) > 0n, "loaded scene has no instance allocation");
    return memory;
}

async function viewerProbe(canvas) {
    const { default: init, WasmVoxelViewer } = await import("../../../pkg/viewer_wasm.js");
    await init();
    const viewer = await WasmVoxelViewer.createOffscreen(canvas, 0.6);
    self.probeResources = viewer;
    const coordinates = await fetch(new URL("../sample.oct", import.meta.url)).then(response => response.blob());
    const attributes = await fetch(new URL("../sample.octattrs", import.meta.url)).then(response => response.blob());
    viewer.openFormatDataset(coordinates, 256 * 1024 * 1024, attributes);
    viewer.fitScene();
    let lod;
    for (let attempt = 0; attempt < 20; attempt += 1) {
        lod = viewer.updateLod(4);
        if (lod.coverageComplete && lod.pendingBlockIds.length === 0 && lod.lodStagePixels <= 4) break;
    }
    assert(lod.coverageComplete && lod.pendingBlockIds.length === 0 && lod.lodStagePixels <= 4, "sample did not complete");
    assert(viewer.renderFrame() === "presented", "direct frame failed");
    assert(viewer.renderFrame() === "presented", "indirect frame failed");
    const gpuBeforePick = viewer.gpuStats();
    const [width, height] = viewer.viewportSize();
    checkResourceStats(gpuBeforePick, width, height);
    const hit = await viewer.pick(width / 2, height / 2);
    assert(hit && hit.voxelCenter.every(Number.isFinite), "center pick missed the sample");
    const expectedCenter = [31.75390625, 31.75390625, 1.79296875];
    assert(hit.voxelCenter.every((value, index) => Math.abs(value - expectedCenter[index]) < 1e-5),
        "center pick returned incorrect dataset coordinates");
    assert(hit.position.every(Number.isFinite) && hit.voxelSize > 0 && hit.distance > 0,
        "pick returned invalid geometry");
    assert(await viewer.pick(-1, -1) === null, "out-of-viewport pick returned a hit");
    viewer.setOrbitTarget(new Float64Array(hit.voxelCenter));
    assert(viewer.renderFrame() === "presented", "recentered frame failed");
    const gpuAfterRecenter = viewer.gpuStats();
    viewer.resize(width / 2, height / 2);
    const gpuAfterResize = viewer.gpuStats();
    checkResourceStats(gpuAfterResize, width / 2, height / 2);
    assert(BigInt(gpuAfterResize.requestedResources.depthTextureBytes) * 4n
        === BigInt(gpuBeforePick.requestedResources.depthTextureBytes), "resize retained old depth extent");
    assert(gpuAfterResize.requestedResources.instanceBufferBytes
        === gpuBeforePick.requestedResources.instanceBufferBytes, "resize changed instance capacity");
    viewer.resize(width, height);
    assert(viewer.renderFrame() === "presented", "restored viewport frame failed");
    checkResourceStats(viewer.gpuStats(), width, height);
    return {
        renderedVoxels: lod.renderedVoxels,
        hit: { ...hit, position: [...hit.position], voxelCenter: [...hit.voxelCenter] },
        gpuBeforePick, gpuAfterRecenter, gpuAfterResize,
    };
}

self.onmessage = async ({ data }) => {
    try {
        const result = await (data.mode === "clear" ? clearProbe(data.canvas) : viewerProbe(data.canvas));
        self.postMessage({ type: "result", result });
    } catch (error) {
        self.postMessage({ type: "failure", message: String(error) });
    }
};
