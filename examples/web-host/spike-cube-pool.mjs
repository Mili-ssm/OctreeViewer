// THROWAWAY SPIKE CODE (R3 Task 5a). Not product code: the bit-exact instance decode belongs to
// Task 6 and the pool to Task 7. Plain WebGPU, no wasm. Reads one case directory in the format of
// spike-cube-pool-format.md and times five draw variants plus a moving-camera variant on an
// offscreen 1280x720 target. Driven by probe-cube-pool.mjs through window.__cubePool:
//   init() -> device info;  loadCase(baseUrl) -> summary;  runRound({warmupMs, frames, round}) -> results.

const WIDTH = 1280;
const HEIGHT = 720;
const COLOR_FORMAT = "rgba8unorm-srgb";
const DEPTH_FORMAT = "depth32float";

// Cube geometry copied from crates/viewer_render_wgpu/src/gpu.rs (CUBE_VERTICES / CUBE_INDICES).
const CUBE_VERTICES = new Float32Array([
    -0.5, -0.5, -0.5, -1, -1, -1,
    0.5, -0.5, -0.5, 1, -1, -1,
    0.5, 0.5, -0.5, 1, 1, -1,
    -0.5, 0.5, -0.5, -1, 1, -1,
    -0.5, -0.5, 0.5, -1, -1, 1,
    0.5, -0.5, 0.5, 1, -1, 1,
    0.5, 0.5, 0.5, 1, 1, 1,
    -0.5, 0.5, 0.5, -1, 1, 1,
]);
const CUBE_INDICES = new Uint16Array([
    0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 2, 3, 7, 2, 7, 6, 1, 2, 6, 1, 6, 5, 3, 0, 4, 3, 4, 7,
]);

// Fragment stage and VertexOutput copied from VOXEL_SHADER_WGSL (scene.rs); the lighting branch
// (intensity_range.z > 0.5) is always on. render_origin is kept only so the uniform has the same layout.
const COMMON_WGSL = `
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) depth: f32,
};
struct CameraUniform {
    view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    style: vec4<f32>,
    intensity_range: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: CameraUniform;

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    if camera.intensity_range.z > 0.5 {
        let light_direction = normalize(vec3<f32>(-0.35, -0.45, 0.82));
        let face_light = 0.68 + 0.32 * max(dot(normalize(input.normal), light_direction), 0.0);
        let depth_light = 1.0 - 0.12 * clamp(input.depth, 0.0, 1.0);
        return vec4<f32>(input.color.rgb * face_light * depth_light, input.color.a);
    }
    return input.color;
}
`;

const I40_VS_WGSL = `
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) instance_center: vec3<f32>,
    @location(3) instance_size: f32,
    @location(4) instance_color: vec4<f32>,
    @location(5) instance_intensity: f32,
    @location(6) instance_classification: f32,
};
@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let local_position = input.instance_center * camera.render_origin.w - camera.render_origin.xyz
        + input.position * input.instance_size * camera.style.y * camera.render_origin.w;
    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(local_position, 1.0);
    out.depth = out.clip_position.z / out.clip_position.w;
    out.normal = input.normal;
    out.color = input.instance_color;
    return out;
}
`;

// Vertex stage of the pooled variants, following the "Vertex-stage contract" of the format document.
function pooledVsWgsl(width) {
    const spp = width === "w10" ? 682 : 512;
    const decode = width === "w10" ? `
    let base = s * 3u;
    let w0 = inst[base];
    let empty = w0 == 0xFFFFFFFFu;
    let rel = vec3<u32>(w0 & 1023u, (w0 >> 10u) & 1023u, (w0 >> 20u) & 1023u);
    let rgba = inst[base + 1u];` : `
    let base = s * 4u;
    let w0 = inst[base];
    let w1 = inst[base + 1u];
    let empty = w0 == 0xFFFFFFFFu && w1 == 0xFFFFFFFFu;
    let rel = vec3<u32>(w0 & 0xFFFFu, w0 >> 16u, w1 & 0xFFFFu);
    let rgba = inst[base + 2u];`;
    return `
@group(1) @binding(0) var<storage, read> inst: array<u32>;
@group(1) @binding(1) var<storage, read> pages: array<u32>;
@group(1) @binding(2) var<storage, read> nodes: array<u32>;
const SLOTS_PER_PAGE: u32 = ${spp}u;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

@vertex
fn vs_main(@builtin(instance_index) s: u32, @location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    let page = s / SLOTS_PER_PAGE;
    let p = page * 12u;
${decode}
    if empty { return out; }
    let octant_bits = pages[p + 9u];
    let shift = bitcast<i32>(pages[p + 10u]);
    let cell = vec3<u32>(pages[p + 6u], pages[p + 7u], pages[p + 8u]) + rel;
    let m = select(cell << vec3<u32>(u32(-shift)), cell >> vec3<u32>(u32(shift)), shift >= 0);
    if any(m > vec3<u32>(7u)) { return out; }
    let octant = 4u * (m.x >> 2u) + 2u * (m.y >> 2u) + (m.z >> 2u);
    if ((octant_bits >> octant) & 1u) == 0u { return out; }
    let bit = m.x + 8u * m.y + 64u * m.z;
    if ((nodes[pages[p + 4u] * 16u + (bit >> 5u)] >> (bit & 31u)) & 1u) == 1u { return out; }
    let origin = vec3<f32>(bitcast<f32>(pages[p]), bitcast<f32>(pages[p + 1u]), bitcast<f32>(pages[p + 2u]));
    let side = bitcast<f32>(pages[p + 3u]);
    let center = origin + (vec3<f32>(rel) + vec3<f32>(0.5)) * side;
    out.clip_position = camera.view_proj * vec4<f32>(center + position * side, 1.0);
    out.depth = out.clip_position.z / out.clip_position.w;
    out.normal = normal;
    let c = unpack4x8unorm(rgba);
    out.color = vec4<f32>(srgb_to_linear(c.rgb), 1.0);
    return out;
}
`;
}

const gcd = (a, b) => { while (b) { [a, b] = [b, a % b]; } return a; };
const median = values => {
    const sorted = [...values].sort((a, b) => a - b);
    const mid = sorted.length >> 1;
    return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};
const stats = values => values.length
    ? { median: median(values), min: Math.min(...values), max: Math.max(...values), n: values.length }
    : null;

const state = { device: null, adapter: null, errors: [], gpu: null, kase: null };
const logElement = typeof document === "undefined" ? null : document.getElementById("log");
const log = text => { if (logElement) logElement.textContent += `\n${text}`; };

async function init() {
    if (!navigator.gpu) throw new Error("WebGPU unavailable (navigator.gpu missing)");
    const adapter = await navigator.gpu.requestAdapter({ powerPreference: "high-performance" });
    if (!adapter) throw new Error("requestAdapter returned null");
    if (!adapter.features.has("timestamp-query")) throw new Error("adapter lacks timestamp-query");
    const requiredLimits = {
        maxBufferSize: adapter.limits.maxBufferSize,
        maxStorageBufferBindingSize: adapter.limits.maxStorageBufferBindingSize,
    };
    if ("maxStorageBuffersInVertexStage" in adapter.limits) {
        requiredLimits.maxStorageBuffersInVertexStage = Math.min(4, adapter.limits.maxStorageBuffersInVertexStage);
    }
    const requiredFeatures = ["timestamp-query"];
    const device = await adapter.requestDevice({ requiredFeatures, requiredLimits });
    device.addEventListener("uncapturederror", event => state.errors.push(`uncaptured: ${event.error.message}`));
    device.lost.then(info => state.errors.push(`device lost: ${info.reason} ${info.message}`));
    state.device = device;
    state.adapter = adapter;
    const info = adapter.info ?? (await adapter.requestAdapterInfo?.()) ?? {};
    const toPlain = limits => Object.fromEntries(Object.keys(Object.getPrototypeOf(limits)).map(k => [k, limits[k]])
        .filter(([, v]) => typeof v === "number"));
    const result = {
        adapterInfo: { vendor: info.vendor, architecture: info.architecture, device: info.device,
            description: info.description, isFallbackAdapter: adapter.isFallbackAdapter ?? info.isFallbackAdapter },
        adapterFeatures: [...adapter.features],
        deviceFeatures: [...device.features],
        requestedLimits: requiredLimits,
        grantedLimits: toPlain(device.limits),
    };
    buildGpuResources(device);
    return result;
}

function buildGpuResources(device) {
    const colorTexture = device.createTexture({
        size: [WIDTH, HEIGHT], format: COLOR_FORMAT,
        usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC,
    });
    const depthTexture = device.createTexture({ size: [WIDTH, HEIGHT], format: DEPTH_FORMAT, usage: GPUTextureUsage.RENDER_ATTACHMENT });
    for (const t of [colorTexture, depthTexture]) {
        if (t.width !== WIDTH || t.height !== HEIGHT) throw new Error(`offscreen target is ${t.width}x${t.height}, expected ${WIDTH}x${HEIGHT}`);
    }
    const vertexBuffer = device.createBuffer({ size: CUBE_VERTICES.byteLength, usage: GPUBufferUsage.VERTEX, mappedAtCreation: true });
    new Float32Array(vertexBuffer.getMappedRange()).set(CUBE_VERTICES);
    vertexBuffer.unmap();
    const indexBuffer = device.createBuffer({ size: CUBE_INDICES.byteLength, usage: GPUBufferUsage.INDEX, mappedAtCreation: true });
    new Uint16Array(indexBuffer.getMappedRange()).set(CUBE_INDICES);
    indexBuffer.unmap();
    const uniformBuffer = device.createBuffer({ size: 28 * 4, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });

    const uniformLayout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, buffer: { type: "uniform" } },
    ] });
    const poolLayout = device.createBindGroupLayout({ entries: [0, 1, 2].map(binding => (
        { binding, visibility: GPUShaderStage.VERTEX, buffer: { type: "read-only-storage" } })) });
    const uniformGroup = device.createBindGroup({ layout: uniformLayout, entries: [{ binding: 0, resource: { buffer: uniformBuffer } }] });

    const cubeBuffer = { arrayStride: 24, stepMode: "vertex", attributes: [
        { shaderLocation: 0, offset: 0, format: "float32x3" }, { shaderLocation: 1, offset: 12, format: "float32x3" }] };
    const i40Buffer = { arrayStride: 40, stepMode: "instance", attributes: [
        { shaderLocation: 2, offset: 0, format: "float32x3" }, { shaderLocation: 3, offset: 12, format: "float32" },
        { shaderLocation: 4, offset: 16, format: "float32x4" }, { shaderLocation: 5, offset: 32, format: "float32" },
        { shaderLocation: 6, offset: 36, format: "float32" }] };
    const makePipeline = (label, vsSource, layouts, buffers) => {
        const module = device.createShaderModule({ label, code: vsSource + COMMON_WGSL });
        return device.createRenderPipeline({
            label,
            layout: device.createPipelineLayout({ bindGroupLayouts: layouts }),
            vertex: { module, entryPoint: "vs_main", buffers },
            primitive: { topology: "triangle-list", cullMode: "back" },
            depthStencil: { format: DEPTH_FORMAT, depthWriteEnabled: true, depthCompare: "less" },
            fragment: { module, entryPoint: "fs_main", targets: [{ format: COLOR_FORMAT, blend: {
                color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha", operation: "add" },
                alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" } } }] },
        });
    };
    const pipelines = {
        w10: makePipeline("w10", pooledVsWgsl("w10"), [uniformLayout, poolLayout], [cubeBuffer]),
        w16: makePipeline("w16", pooledVsWgsl("w16"), [uniformLayout, poolLayout], [cubeBuffer]),
        i40: makePipeline("i40", I40_VS_WGSL, [uniformLayout], [cubeBuffer, i40Buffer]),
    };
    const querySet = device.createQuerySet({ type: "timestamp", count: 2 });
    const resolveBuffer = device.createBuffer({ size: 16, usage: GPUBufferUsage.QUERY_RESOLVE | GPUBufferUsage.COPY_SRC });
    const readBuffer = device.createBuffer({ size: 16, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
    const pixelBuffer = device.createBuffer({ size: WIDTH * HEIGHT * 4, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
    state.gpu = { colorTexture, depthTexture, vertexBuffer, indexBuffer, uniformBuffer, uniformGroup, poolLayout, pipelines,
        uniformLayout, cubeBuffer, i40Buffer,
        querySet, resolveBuffer, readBuffer, pixelBuffer };
}

async function fetchBin(base, name, optional = false) {
    const response = await fetch(`${base}/${name}`);
    if (!response.ok) {
        if (optional) return null;
        throw new Error(`fetch ${base}/${name}: ${response.status}`);
    }
    return response.arrayBuffer();
}

function makeStorageBuffer(label, data) {
    const { device } = state;
    const size = Math.max(16, Math.ceil(data.byteLength / 4) * 4);
    const limits = device.limits;
    if (size > limits.maxBufferSize || size > limits.maxStorageBufferBindingSize) {
        throw new Error(`case does not fit: ${label} needs ${size} B, granted maxBufferSize=${limits.maxBufferSize} maxStorageBufferBindingSize=${limits.maxStorageBufferBindingSize}`);
    }
    const buffer = device.createBuffer({ label, size, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST });
    device.queue.writeBuffer(buffer, 0, data);
    return buffer;
}

async function loadCase(base) {
    const { device, gpu } = state;
    if (!device) throw new Error("call init() first");
    unloadCase();
    const meta = await (await fetch(`${base}/case.json`)).json();
    if (meta.viewport[0] !== WIDTH || meta.viewport[1] !== HEIGHT) throw new Error(`case viewport ${meta.viewport} is not ${WIDTH}x${HEIGHT}`);
    const [w10i, w16i, w10p, w16p, nodes, w10r, w16r, i40] = await Promise.all([
        "w10_instances.bin", "w16_instances.bin", "w10_pages.bin", "w16_pages.bin", "nodes.bin",
        "w10_ranges.bin", "w16_ranges.bin", "i40_instances.bin"].map(name => fetchBin(base, name)));
    device.pushErrorScope("out-of-memory");
    device.pushErrorScope("validation");
    const buffers = [];
    const storage = (label, ab) => { const b = makeStorageBuffer(label, new Uint8Array(ab)); buffers.push(b); return b; };
    const nodesBuffer = storage("nodes", nodes);
    const makePool = (width, instAb, pagesAb) => {
        const instBuffer = storage(`${width}-instances`, instAb);
        const pagesBuffer = storage(`${width}-pages`, pagesAb);
        return { instBuffer, pagesBuffer, group: device.createBindGroup({ layout: gpu.poolLayout, entries: [
            { binding: 0, resource: { buffer: instBuffer } }, { binding: 1, resource: { buffer: pagesBuffer } },
            { binding: 2, resource: { buffer: nodesBuffer } }] }) };
    };
    const i40Buffer = device.createBuffer({ label: "i40", size: Math.max(16, i40.byteLength), usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST });
    if (i40.byteLength > device.limits.maxBufferSize) throw new Error(`case does not fit: i40 needs ${i40.byteLength} B`);
    device.queue.writeBuffer(i40Buffer, 0, new Uint8Array(i40));
    buffers.push(i40Buffer);
    const pool = { w10: makePool("w10", w10i, w10p), w16: makePool("w16", w16i, w16p) };
    const validation = await device.popErrorScope();
    const oom = await device.popErrorScope();
    if (validation || oom) throw new Error(`case does not fit / resource error: ${(validation ?? oom).message}`);

    const ranges = { w10: new Uint32Array(w10r), w16: new Uint32Array(w16r) };
    const drawnCells = i40.byteLength / 40;
    if (drawnCells !== meta.drawn_cells) log(`warning: i40 cells ${drawnCells} != case.json drawn_cells ${meta.drawn_cells}`);
    // Page origins as f64 in the scaled dataset frame (origin relative to eye + eye), for the moving-camera variant.
    const pageWorld = {};
    for (const width of ["w10", "w16"]) {
        const pages = new Float32Array(width === "w10" ? w10p : w16p);
        const count = pages.length / 12;
        const world = new Float64Array(count * 3);
        for (let page = 0; page < count; page++) {
            for (let axis = 0; axis < 3; axis++) world[page * 3 + axis] = pages[page * 12 + axis] + meta.eye[axis] * meta.visual_scale;
        }
        pageWorld[width] = world;
    }
    const pagesF32 = new Float32Array(w10p);
    const sides = Array.from({ length: pagesF32.length / 12 }, (_, i) => pagesF32[i * 12 + 3]);
    const uniform = new Float32Array(28);
    uniform.set(meta.view_proj, 0);
    // render_origin (floats 16..19) = (0, 0, 0, 1): positions are already eye-relative and scaled;
    // style.y (21) = 1; intensity_range.z (26) = 1 turns the lighting on.
    uniform[19] = 1;
    uniform[21] = 1;
    uniform[26] = 1;
    device.queue.writeBuffer(gpu.uniformBuffer, 0, uniform);
    state.kase = { base, meta, buffers, pool, i40Buffer, ranges, drawnCells, pageWorld,
        pageBytes: { w10: new Uint32Array(w10p.slice(0)), w16: new Uint32Array(w16p.slice(0)) },
        jitterAmp: median(sides) * 0.05 };
    return {
        base, dataset: meta.dataset, pose: meta.pose, target_px: meta.target_px,
        slots_used: { w10: meta.variants.w10.slots_used, w16: meta.variants.w16.slots_used },
        ranges: { w10: ranges.w10.length / 2, w16: ranges.w16.length / 2 }, drawnCells,
        bytes: { w10_instances: w10i.byteLength, w16_instances: w16i.byteLength, i40: i40.byteLength, nodes: nodes.byteLength },
    };
}

function unloadCase() {
    if (!state.kase) return;
    for (const b of state.kase.buffers) b.destroy();
    state.kase = null;
}

function variantList() {
    const { meta, pool, ranges, i40Buffer, drawnCells } = state.kase;
    const { pipelines } = state.gpu;
    const pooled = (name, width, mode, moving = false) => ({
        name, width, mode, moving, pipeline: pipelines[width], group: pool[width].group,
        slots: meta.variants[width].slots_used, ranges: ranges[width],
        draws: mode === "ranges" ? ranges[width].length / 2 : 1,
    });
    return [
        pooled("w10-ranges", "w10", "ranges"),
        pooled("w10-single", "w10", "single"),
        pooled("w16-ranges", "w16", "ranges"),
        pooled("w16-single", "w16", "single"),
        { name: "i40-single", width: "i40", mode: "single", pipeline: pipelines.i40, instances: i40Buffer, count: drawnCells, draws: 1 },
        pooled("w10-ranges-moving", "w10", "ranges", true),
    ];
}

function updatePageTable(variant, frameIndex) {
    const { kase, device } = state;
    const world = kase.pageWorld[variant.width];
    const bytes = kase.pageBytes[variant.width];
    const f32 = new Float32Array(bytes.buffer);
    const scale = kase.meta.visual_scale;
    const angle = frameIndex * 0.3;
    const amp = kase.jitterAmp;
    const d = [amp * Math.cos(angle), amp * Math.sin(angle), amp * 0.5 * Math.sin(2 * angle)];
    const eye = kase.meta.eye;
    const pages = world.length / 3;
    const t0 = performance.now();
    for (let page = 0; page < pages; page++) {
        for (let axis = 0; axis < 3; axis++) {
            f32[page * 12 + axis] = world[page * 3 + axis] - (eye[axis] * scale + d[axis]);
        }
    }
    const t1 = performance.now();
    device.queue.writeBuffer(kase.pool[variant.width].pagesBuffer, 0, bytes);
    return { cpuRecomputeMs: t1 - t0, writeBufferMs: performance.now() - t1 };
}

function encodeFrame(variant) {
    const { device, kase } = state;
    const g = state.gpu;
    const encoder = device.createCommandEncoder();
    const pass = encoder.beginRenderPass({
        colorAttachments: [{ view: g.colorTexture.createView(), clearValue: { r: 0, g: 0, b: 0, a: 1 }, loadOp: "clear", storeOp: "store" }],
        depthStencilAttachment: { view: g.depthTexture.createView(), depthClearValue: 1, depthLoadOp: "clear", depthStoreOp: "store" },
        timestampWrites: { querySet: g.querySet, beginningOfPassWriteIndex: 0, endOfPassWriteIndex: 1 },
    });
    pass.setPipeline(variant.pipeline);
    pass.setBindGroup(0, g.uniformGroup);
    pass.setVertexBuffer(0, g.vertexBuffer);
    if (variant.width === "i40") pass.setVertexBuffer(1, variant.instances);
    else pass.setBindGroup(1, variant.group);
    pass.setIndexBuffer(g.indexBuffer, "uint16");
    const d0 = performance.now();
    if (variant.mode === "ranges") {
        const r = variant.ranges;
        for (let i = 0; i < r.length; i += 2) if (r[i + 1] > 0) pass.drawIndexed(36, r[i + 1], 0, 0, r[i]);
    } else {
        pass.drawIndexed(36, variant.width === "i40" ? variant.count : variant.slots, 0, 0, 0);
    }
    const drawLoopMs = performance.now() - d0;
    pass.end();
    encoder.resolveQuerySet(g.querySet, 0, 2, g.resolveBuffer, 0);
    encoder.copyBufferToBuffer(g.resolveBuffer, 0, g.readBuffer, 0, 16);
    return { encoder, drawLoopMs, kase };
}

async function runFrame(variant, frameIndex) {
    const { device } = state;
    const g = state.gpu;
    const rec = {};
    const t0 = performance.now();
    if (variant.moving) Object.assign(rec, updatePageTable(variant, frameIndex));
    const { encoder, drawLoopMs } = encodeFrame(variant);
    const commands = encoder.finish();
    const t1 = performance.now();
    device.queue.submit([commands]);
    const t2 = performance.now();
    await device.queue.onSubmittedWorkDone();
    const t3 = performance.now();
    await g.readBuffer.mapAsync(GPUMapMode.READ);
    const ticks = new BigUint64Array(g.readBuffer.getMappedRange().slice(0));
    g.readBuffer.unmap();
    rec.gpuTicks = Number(ticks[1] - ticks[0]);
    rec.gpuMs = rec.gpuTicks / 1e6; // timestamp-query values are nanoseconds
    rec.wallMs = t3 - t1;
    rec.encodeMs = t1 - t0 - (rec.writeBufferMs ?? 0) - (rec.cpuRecomputeMs ?? 0);
    rec.drawLoopMs = drawLoopMs;
    rec.submitMs = t2 - t1;
    return rec;
}

async function readbackNonBlack() {
    const { device } = state;
    const g = state.gpu;
    const encoder = device.createCommandEncoder();
    encoder.copyTextureToBuffer({ texture: g.colorTexture }, { buffer: g.pixelBuffer, bytesPerRow: WIDTH * 4 }, [WIDTH, HEIGHT]);
    device.queue.submit([encoder.finish()]);
    await g.pixelBuffer.mapAsync(GPUMapMode.READ);
    const pixels = new Uint8Array(g.pixelBuffer.getMappedRange());
    let count = 0;
    for (let i = 0; i < pixels.length; i += 4) if (pixels[i] | pixels[i + 1] | pixels[i + 2]) count++;
    g.pixelBuffer.unmap();
    return count;
}

async function runVariant(variant, { warmupMs, frames }) {
    const { device } = state;
    device.pushErrorScope("validation");
    const warmStart = performance.now();
    let frameIndex = 0;
    let warmFrames = 0;
    while (performance.now() - warmStart < warmupMs || warmFrames < 3) { await runFrame(variant, frameIndex++); warmFrames++; }
    const nonBlackPixels = await readbackNonBlack();
    const series = { gpuTicks: [], gpuMs: [], wallMs: [], encodeMs: [], drawLoopMs: [], submitMs: [] };
    if (variant.moving) Object.assign(series, { writeBufferMs: [], cpuRecomputeMs: [] });
    for (let i = 0; i < frames; i++) {
        const rec = await runFrame(variant, frameIndex++);
        for (const key of Object.keys(series)) series[key].push(rec[key]);
    }
    const error = await device.popErrorScope();
    if (error) state.errors.push(`${variant.name}: validation: ${error.message}`);
    const ticksNonZero = series.gpuTicks.filter(t => t > 0).map(BigInt);
    const tickGcd = ticksNonZero.length ? Number(ticksNonZero.reduce(gcd)) : null;
    return {
        name: variant.name, draws: variant.draws, warmFrames, nonBlackPixels, tickGcd,
        perDrawCpuUs: (median(series.drawLoopMs) * 1000) / variant.draws,
        summary: Object.fromEntries(Object.entries(series).map(([k, v]) => [k, stats(v)])),
        series,
    };
}

async function runRound({ warmupMs = 2000, frames = 30, round = 0 } = {}) {
    if (!state.kase) throw new Error("call loadCase() first");
    const all = variantList();
    // Rotate the starting variant each round so no variant is always first (thermal/boost order effects).
    const order = all.map((_, i) => all[(i + round) % all.length]);
    const results = [];
    for (const variant of order) {
        log(`round ${round}: ${variant.name}`);
        results.push(await runVariant(variant, { warmupMs, frames }));
    }
    return { round, order: order.map(v => v.name), errors: state.errors.splice(0), variants: results };
}


// ---- T5b diagnostics (opt-in through runDiag(); not part of the T5a timing path) ----
const DIAG_ID_COMMON_WGSL = `
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) depth: f32,
    @location(3) @interpolate(flat) id: u32,
};
struct CameraUniform {
    view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    style: vec4<f32>,
    intensity_range: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: CameraUniform;
@fragment
fn fs_main(input: VertexOutput) -> @location(0) u32 { return input.id; }
`;
const ID_FORMAT = "r32uint";
const ID_CLEAR = 0xFFFFFFFF;

function diagPipelines() {
    const { device } = state;
    const g = state.gpu;
    if (g.diag) return g.diag;
    const colorTarget = { format: COLOR_FORMAT, blend: {
        color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha", operation: "add" },
        alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" } } };
    const make = (label, vsSource, withId, layouts, buffers, depth, target) => {
        const module = device.createShaderModule({ label, code: withId ? vsSource + DIAG_ID_COMMON_WGSL : vsSource + COMMON_WGSL });
        return device.createRenderPipeline({
            label, layout: device.createPipelineLayout({ bindGroupLayouts: layouts }),
            vertex: { module, entryPoint: "vs_main", buffers },
            primitive: { topology: "triangle-list", cullMode: "back" },
            depthStencil: { format: DEPTH_FORMAT, ...depth },
            fragment: { module, entryPoint: "fs_main", targets: [target] },
        });
    };
    const lessWrite = { depthWriteEnabled: true, depthCompare: "less" };
    const always = { depthWriteEnabled: false, depthCompare: "always" };
    const idVs = {
        // The id is the instance_index (pooled: global slot incl. firstInstance) or the drawn-cell index (i40).
        w10: pooledVsWgsl("w10").replace("out.depth = out.clip_position.z / out.clip_position.w;",
            "out.depth = out.clip_position.z / out.clip_position.w;\n    out.id = s;"),
        i40: I40_VS_WGSL.replace("@location(6) instance_classification: f32,",
            "@location(6) instance_classification: f32,\n    @builtin(instance_index) iid: u32,")
            .replace("out.normal = input.normal;", "out.normal = input.normal;\n    out.id = input.iid;"),
    };
    // Pooled/i40 sources without id keep VertexOutput from COMMON_WGSL; the id variants add the flat id.
    const l = [g.uniformLayout, g.poolLayout], li = [g.uniformLayout];
    g.diag = {
        w10Always: make("w10-always", pooledVsWgsl("w10"), false, l, [g.cubeBuffer], always, colorTarget),
        i40Always: make("i40-always", I40_VS_WGSL, false, li, [g.cubeBuffer, g.i40Buffer], always, colorTarget),
        w10Id: make("w10-id", idVs.w10, true, l, [g.cubeBuffer], lessWrite, { format: ID_FORMAT }),
        i40Id: make("i40-id", idVs.i40, true, li, [g.cubeBuffer, g.i40Buffer], lessWrite, { format: ID_FORMAT }),
        idTexture: device.createTexture({ size: [WIDTH, HEIGHT], format: ID_FORMAT, usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC }),
        occlusion: device.createQuerySet({ type: "occlusion", count: 1 }),
        occResolve: device.createBuffer({ size: 8, usage: GPUBufferUsage.QUERY_RESOLVE | GPUBufferUsage.COPY_SRC }),
        occRead: device.createBuffer({ size: 8, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ }),
    };
    return g.diag;
}

// One pass over the variant's draws; `occlusion` wraps them in an occlusion query (samples passed).
async function diagPass(variant, pipeline, colorTexture, occlusion) {
    const { device } = state;
    const g = state.gpu;
    const d = diagPipelines();
    const encoder = device.createCommandEncoder();
    const pass = encoder.beginRenderPass({
        colorAttachments: [{ view: colorTexture.createView(), clearValue: colorTexture === d.idTexture ? { r: ID_CLEAR, g: 0, b: 0, a: 0 } : { r: 0, g: 0, b: 0, a: 1 }, loadOp: "clear", storeOp: "store" }],
        depthStencilAttachment: { view: g.depthTexture.createView(), depthClearValue: 1, depthLoadOp: "clear", depthStoreOp: "store" },
        ...(occlusion ? { occlusionQuerySet: d.occlusion } : {}),
    });
    pass.setPipeline(pipeline);
    pass.setBindGroup(0, g.uniformGroup);
    pass.setVertexBuffer(0, g.vertexBuffer);
    if (variant.width === "i40") pass.setVertexBuffer(1, variant.instances); else pass.setBindGroup(1, variant.group);
    pass.setIndexBuffer(g.indexBuffer, "uint16");
    if (occlusion) pass.beginOcclusionQuery(0);
    if (variant.mode === "ranges") {
        const r = variant.ranges;
        for (let i = 0; i < r.length; i += 2) if (r[i + 1] > 0) pass.drawIndexed(36, r[i + 1], 0, 0, r[i]);
    } else {
        pass.drawIndexed(36, variant.width === "i40" ? variant.count : variant.slots, 0, 0, 0);
    }
    if (occlusion) pass.endOcclusionQuery();
    pass.end();
    if (occlusion) {
        encoder.resolveQuerySet(d.occlusion, 0, 1, d.occResolve, 0);
        encoder.copyBufferToBuffer(d.occResolve, 0, d.occRead, 0, 8);
    }
    device.queue.submit([encoder.finish()]);
    if (!occlusion) { await device.queue.onSubmittedWorkDone(); return null; }
    await d.occRead.mapAsync(GPUMapMode.READ);
    const samples = Number(new BigUint64Array(d.occRead.getMappedRange().slice(0))[0]);
    d.occRead.unmap();
    return samples;
}

async function readIds() {
    const { device } = state;
    const g = state.gpu;
    const encoder = device.createCommandEncoder();
    encoder.copyTextureToBuffer({ texture: diagPipelines().idTexture }, { buffer: g.pixelBuffer, bytesPerRow: WIDTH * 4 }, [WIDTH, HEIGHT]);
    device.queue.submit([encoder.finish()]);
    await g.pixelBuffer.mapAsync(GPUMapMode.READ);
    const ids = new Uint32Array(g.pixelBuffer.getMappedRange().slice(0));
    g.pixelBuffer.unmap();
    return ids;
}

async function runDiag() {
    if (!state.kase) throw new Error("call loadCase() first");
    const { device, kase } = state;
    const g = state.gpu;
    const d = diagPipelines();
    const variants = Object.fromEntries(variantList().map(v => [v.name, v]));
    const out = { idCap: "at most 1280*720 = 921600 distinct ids; cubes smaller than a pixel can be lost, so visible_cells is a lower bound",
        drawOrder: "range order of w10_ranges.bin (Morton within a node, nodes in cut order); i40 in file order",
        drawnCells: kase.drawnCells, variants: {} };
    const ranges = kase.ranges.w10;
    out.rangeCount = ranges.length / 2;
    out.firstRanges = Array.from(ranges.slice(0, 16));
    for (const [name, always, idPipeline] of [["w10-ranges", d.w10Always, d.w10Id], ["i40-single", d.i40Always, d.i40Id]]) {
        const variant = variants[name];
        device.pushErrorScope("validation");
        const depthTested = await diagPass(variant, variant.pipeline, g.colorTexture, true);
        const covered = await readbackNonBlack();
        const all = await diagPass(variant, always, g.colorTexture, true);
        await diagPass(variant, idPipeline, d.idTexture, false);
        const ids = await readIds();
        const error = await device.popErrorScope();
        if (error) state.errors.push(`diag ${name}: ${error.message}`);
        const found = ids.filter(v => v !== ID_CLEAR).sort();
        const distinct = [];
        for (const v of found) if (distinct[distinct.length - 1] !== v) distinct.push(v);
        const rec = { samples_depth_tested: depthTested, samples_all: all, covered_px: covered,
            overdraw_all_over_covered: covered ? all / covered : null,
            depth_tested_over_covered: covered ? depthTested / covered : null,
            id_pixels: found.length, visible_cells: distinct.length, drawn_cells: kase.drawnCells,
            visible_fraction: distinct.length / kase.drawnCells };
        if (name === "w10-ranges") {
            // firstInstance check: every id must lie in a draw range; ids in ranges with first_slot > 0
            // can only appear if instance_index includes firstInstance.
            const rs = [];
            for (let i = 0; i < ranges.length; i += 2) if (ranges[i + 1] > 0) rs.push([ranges[i], ranges[i + 1]]);
            rs.sort((a, b) => a[0] - b[0]);
            const rangeOf = id => { let lo = 0, hi = rs.length - 1;
                while (lo <= hi) { const mid = (lo + hi) >> 1; if (rs[mid][0] > id) hi = mid - 1; else if (rs[mid][0] + rs[mid][1] <= id) lo = mid + 1; else return mid; }
                return -1; };
            const outside = distinct.filter(id => rangeOf(id) < 0).length;
            const inLaterRanges = distinct.filter(id => id >= rs[0][0] + rs[0][1]).length;
            const sample = distinct.find(id => id >= rs[0][0] + rs[0][1]) ?? null;
            rec.firstInstanceCheck = { ids_outside_ranges: outside, ids_beyond_first_range: inLaterRanges,
                sample_id: sample, sample_range: sample === null ? null : rs[rangeOf(sample)],
                ok: outside === 0 && (inLaterRanges > 0 || rs.length === 1) };
        } else {
            rec.firstInstanceCheck = { ids_outside_cells: distinct.filter(id => id >= kase.drawnCells).length };
            rec.firstInstanceCheck.ok = rec.firstInstanceCheck.ids_outside_cells === 0;
        }
        if (!rec.firstInstanceCheck.ok) state.errors.push(`diag ${name}: id check failed ${JSON.stringify(rec.firstInstanceCheck)}`);
        out.variants[name] = rec;
    }
    out.errors = state.errors.splice(0);
    return out;
}

window.__cubePool = { init, loadCase, runRound, runDiag, unloadCase, get errors() { return state.errors; } };

const params = new URLSearchParams(location.search);
if (params.has("case")) {
    (async () => {
        try {
            const info = await init();
            const summary = await loadCase(params.get("case"));
            const round = await runRound({ warmupMs: Number(params.get("warmupMs") ?? 2000), frames: Number(params.get("frames") ?? 30) });
            window.__cubePoolResult = { info, summary, round };
            log(JSON.stringify(round.variants.map(v => [v.name, v.summary.gpuMs.median, v.nonBlackPixels])));
        } catch (error) { log(`error: ${error.stack ?? error}`); }
    })();
}
