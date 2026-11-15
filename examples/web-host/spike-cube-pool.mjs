// THROWAWAY SPIKE CODE (R3 Task 5a). Not product code: the bit-exact instance decode belongs to
// Task 6 and the pool to Task 7. Plain WebGPU, no wasm. Reads one case directory in the format of
// spike-cube-pool-format.md and times five draw variants plus a moving-camera variant on an
// offscreen 1280x720 target. Driven by probe-cube-pool.mjs through window.__cubePool:
//   init() -> device info;  loadCase(baseUrl) -> summary;  runRound({warmupMs, frames, round, set}) -> results.
// set "t16" (R3 Task 16a) times the compaction + Hi-Z variants instead; runT16Check() is the untimed correctness check.

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
        // T16 compute passes bind 8 storage buffers; t16Global() fails with a clear message if the device grants fewer.
        maxStorageBuffersPerShaderStage: Math.min(10, adapter.limits.maxStorageBuffersPerShaderStage),
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
    const w16Pages = w16p.byteLength / 48;
    const w16Sides = Array.from({ length: w16Pages }, (_, i) => new Float32Array(w16p, i * 48 + 12, 1)[0]);
    state.kase = { base, meta, buffers, pool, i40Buffer, ranges, drawnCells, pageWorld, nodesBuffer,
        pageBytes: { w10: new Uint32Array(w10p.slice(0)), w16: new Uint32Array(w16p.slice(0)) },
        jitterAmp: median(sides) * 0.05,
        // T16 inputs: tight page boxes, pristine pose and median cell side for the moving path.
        tightBoxes: tightBoxes(w16i, w16Pages), uniformStatic: uniform.slice(), w16PagesPristine: new Uint8Array(w16p.slice(0)),
        t16CellSide: median(w16Sides) };
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

async function runRound({ warmupMs = 2000, frames = 30, round = 0, set = "t5" } = {}) {
    if (!state.kase) throw new Error("call loadCase() first");
    if (set === "t16") return runT16Round({ warmupMs, frames, round });
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

// ---- T16a: GPU compaction + two-phase Hi-Z occlusion (W16 only; opt-in through runRound({set:"t16"}) / runT16Check()) ----
// Per frame, variant k:   page pass -> finalize -> slot pass (indirect, one workgroup per surviving page) -> one drawIndexedIndirect.
// kp / kpc (two-phase):    phase 1 (what was visible last frame) -> Hi-Z from its depth -> phase 2 (all candidates vs Hi-Z).
// Bits (double-buffered in one `bits` buffer, `params.prev` selects the half read):
//   page word  = bits[q * P + page]             1 = page passed the phase-2 Hi-Z test
//   slot words = bits[2P + q * P * 16 + page * 16 + (local >> 5)]   (kpc only) 1 = slot passed base tests + Hi-Z
// A page is "already drawn in phase 1" iff prev page bit (and, in kpc, prev slot bit) is set; phase 2 uses that same predicate.
const HIZ_W = 2048; // padded so that every level ceil(WIDTH / 2^L) fits the default floor-sized mip chain
const HIZ_H = 1024;
const HIZ_MIPS = 12;
const T16_MAX_PASSES = 10;
const T16_MOVING_FRAMES = 120;
const T16_YAW_DEG_PER_FRAME = 0.5;
const T16_KINDS = {
    k: { mode: 0, two: false, frustum: 1 },
    knf: { mode: 0, two: false, frustum: 0 }, // check variant: k without frustum culling
    kp: { mode: 1, two: true, frustum: 1 },
    kpc: { mode: 2, two: true, frustum: 1 },
};

const T16_COMMON_WGSL = `
struct CameraUniform {
    view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    style: vec4<f32>,
    intensity_range: vec4<f32>,
};
struct Params { prev: u32, num_pages: u32, pad0: u32, pad1: u32 };
@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var hiz: texture_2d<f32>;
@group(1) @binding(0) var<storage, read> inst: array<u32>;
@group(1) @binding(1) var<storage, read> pages: array<u32>;
@group(1) @binding(2) var<storage, read> nodes: array<u32>;
@group(1) @binding(3) var<storage, read> boxes: array<u32>;
override PHASE: u32 = 0u;
override MODE: u32 = 0u;
override FRUSTUM: u32 = 1u;
const SCREEN: vec2<f32> = vec2<f32>(${WIDTH}.0, ${HEIGHT}.0);
const MAX_PX: vec2<i32> = vec2<i32>(${WIDTH - 1}, ${HEIGHT - 1});

fn box_corners(lo: vec3<f32>, size: vec3<f32>) -> array<vec4<f32>, 8> {
    let vp = camera.view_proj;
    let b = vp * vec4<f32>(lo, 1.0);
    let dx = vp[0] * size.x;
    let dy = vp[1] * size.y;
    let dz = vp[2] * size.z;
    var c: array<vec4<f32>, 8>;
    c[0] = b; c[1] = b + dx; c[2] = b + dy; c[3] = b + dx + dy;
    c[4] = b + dz; c[5] = b + dx + dz; c[6] = b + dy + dz; c[7] = b + dx + dy + dz;
    return c;
}

// Conservative: culled only if all 8 corners lie outside the same clip plane (with a small relative margin).
fn outside_frustum(cin: array<vec4<f32>, 8>) -> bool {
    var c = cin;
    var all_out = 63u;
    for (var i = 0u; i < 8u; i++) {
        let q = c[i];
        let m = 1.0e-4 * abs(q.w) + 1.0e-30;
        var bits = 0u;
        if q.x < -q.w - m { bits |= 1u; }
        if q.x > q.w + m { bits |= 2u; }
        if q.y < -q.w - m { bits |= 4u; }
        if q.y > q.w + m { bits |= 8u; }
        if q.z < -m { bits |= 16u; }
        if q.z > q.w + m { bits |= 32u; }
        all_out &= bits;
    }
    return all_out != 0u;
}

// Hi-Z texel (x, y) at level L holds the MAX depth of pixels [x << L, (x + 1) << L) x [y << L, (y + 1) << L) clipped to the screen.
fn occluded(cin: array<vec4<f32>, 8>) -> bool {
    var c = cin;
    var nearest = 3.0e38;
    var pmin = vec2<f32>(3.0e38);
    var pmax = vec2<f32>(-3.0e38);
    for (var i = 0u; i < 8u; i++) {
        let q = c[i];
        if q.w <= 1.0e-6 || q.z < 0.0 { return false; }
        let ndc = q.xyz / q.w;
        nearest = min(nearest, ndc.z);
        let px = vec2<f32>((ndc.x * 0.5 + 0.5) * SCREEN.x, (0.5 - ndc.y * 0.5) * SCREEN.y);
        pmin = min(pmin, px);
        pmax = max(pmax, px);
    }
    let lo = vec2<i32>(floor(clamp(pmin - vec2<f32>(0.5), vec2<f32>(-2.0), SCREEN + vec2<f32>(2.0))));
    let hi = vec2<i32>(floor(clamp(pmax + vec2<f32>(0.5), vec2<f32>(-2.0), SCREEN + vec2<f32>(2.0))));
    if hi.x < 0 || hi.y < 0 || lo.x > MAX_PX.x || lo.y > MAX_PX.y { return false; }
    let a = clamp(lo, vec2<i32>(0), MAX_PX);
    let b = clamp(hi, vec2<i32>(0), MAX_PX);
    var level = 0u;
    loop {
        let sh = vec2<u32>(level);
        let d = (b >> sh) - (a >> sh);
        if (d.x <= 1 && d.y <= 1) || level >= 11u { break; }
        level++;
    }
    let sh = vec2<u32>(level);
    let a2 = a >> sh;
    let b2 = b >> sh;
    let lv = i32(level);
    var h = textureLoad(hiz, vec2<i32>(a2.x, a2.y), lv).r;
    h = max(h, textureLoad(hiz, vec2<i32>(b2.x, a2.y), lv).r);
    h = max(h, textureLoad(hiz, vec2<i32>(a2.x, b2.y), lv).r);
    h = max(h, textureLoad(hiz, vec2<i32>(b2.x, b2.y), lv).r);
    let margin = bitcast<f32>(bitcast<u32>(h) + 4u); // a few ulps above the max
    return nearest > margin;
}

fn page_bit_base(q: u32) -> u32 { return q * params.num_pages; }
fn slot_bit_base(q: u32) -> u32 { return 2u * params.num_pages + q * params.num_pages * 16u; }
`;

const T16_PAGE_WGSL = `
@group(2) @binding(0) var<storage, read_write> page_list: array<atomic<u32>>;
@group(2) @binding(1) var<storage, read_write> bits: array<u32>;

@compute @workgroup_size(64)
fn page_main(@builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) lid: u32) {
    let p = (wid.y * nwg.x + wid.x) * 64u + lid;
    if p >= params.num_pages { return; }
    let b = p * 6u;
    let bmin = vec3<u32>(boxes[b], boxes[b + 1u], boxes[b + 2u]);
    let bmax = vec3<u32>(boxes[b + 3u], boxes[b + 4u], boxes[b + 5u]);
    var keep = all(bmin <= bmax);
    var prev_bit = 0u;
    if PHASE != 0u { prev_bit = bits[page_bit_base(params.prev) + p]; }
    if keep && PHASE == 1u { keep = prev_bit != 0u; }
    if keep {
        let q = p * 12u;
        let origin = vec3<f32>(bitcast<f32>(pages[q]), bitcast<f32>(pages[q + 1u]), bitcast<f32>(pages[q + 2u]));
        let side = bitcast<f32>(pages[q + 3u]);
        let lo = origin + vec3<f32>(bmin) * side;
        let size = vec3<f32>(bmax - bmin + vec3<u32>(1u)) * side;
        let c = box_corners(lo, size);
        if FRUSTUM == 1u && outside_frustum(c) { keep = false; }
        if keep && PHASE == 2u {
            keep = !occluded(c);
        }
    }
    if PHASE == 2u {
        bits[page_bit_base(1u - params.prev) + p] = select(0u, 1u, keep);
        if MODE == 1u { keep = keep && prev_bit == 0u; }
    }
    if keep {
        let i = atomicAdd(&page_list[0], 1u);
        atomicStore(&page_list[1u + i], p);
    }
}
`;

const T16_SLOT_WGSL = `
@group(2) @binding(0) var<storage, read> page_list: array<u32>;
@group(2) @binding(1) var<storage, read_write> visible: array<u32>;
@group(2) @binding(2) var<storage, read_write> draw_args: array<atomic<u32>>;
@group(2) @binding(3) var<storage, read_write> bits: array<u32>;

var<workgroup> scan: array<u32, 256>;
var<workgroup> words: array<atomic<u32>, 16>;
var<workgroup> wg_base: u32;

var<private> pg_valid: bool;
var<private> pg_index: u32;
var<private> pg_origin: vec3<f32>;
var<private> pg_side: f32;
var<private> pg_octants: u32;
var<private> pg_shift: i32;
var<private> pg_node: u32;
var<private> pg_cell: vec3<u32>;
var<private> pg_prev: u32;

// bit 0: draw this slot in this pass; bit 1: slot "visible" for the next frame (kpc phase 2).
fn eval_slot(local: u32) -> u32 {
    let s = pg_index * 512u + local;
    let base = s * 4u;
    let w0 = inst[base];
    let w1 = inst[base + 1u];
    if !pg_valid || (w0 == 0xFFFFFFFFu && w1 == 0xFFFFFFFFu) { return 0u; }
    let rel = vec3<u32>(w0 & 0xFFFFu, w0 >> 16u, w1 & 0xFFFFu);
    let cell = pg_cell + rel;
    let m = select(cell << vec3<u32>(u32(-pg_shift)), cell >> vec3<u32>(u32(pg_shift)), pg_shift >= 0);
    if any(m > vec3<u32>(7u)) { return 0u; }
    let octant = 4u * (m.x >> 2u) + 2u * (m.y >> 2u) + (m.z >> 2u);
    if ((pg_octants >> octant) & 1u) == 0u { return 0u; }
    let bit = m.x + 8u * m.y + 64u * m.z;
    if ((nodes[pg_node * 16u + (bit >> 5u)] >> (bit & 31u)) & 1u) == 1u { return 0u; }
    let lo = pg_origin + vec3<f32>(rel) * pg_side;
    let size = vec3<f32>(pg_side);
    if FRUSTUM == 1u && outside_frustum(box_corners(lo, size)) { return 0u; }
    var prev_slot = 0u;
    if MODE == 2u && PHASE != 0u {
        prev_slot = (bits[slot_bit_base(params.prev) + pg_index * 16u + (local >> 5u)] >> (local & 31u)) & 1u;
    }
    if PHASE == 1u {
        if MODE == 2u && prev_slot == 0u { return 0u; }
        return 1u;
    }
    if PHASE == 2u && MODE == 2u {
        if occluded(box_corners(lo, size)) { return 0u; }
        let already = pg_prev != 0u && prev_slot != 0u;
        return select(3u, 2u, already);
    }
    return 1u;
}

@compute @workgroup_size(256)
fn slot_main(@builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) lid: u32) {
    let li = wid.y * nwg.x + wid.x;
    pg_valid = li < page_list[0];
    pg_index = page_list[select(0u, 1u + li, pg_valid)];
    let q = pg_index * 12u;
    pg_origin = vec3<f32>(bitcast<f32>(pages[q]), bitcast<f32>(pages[q + 1u]), bitcast<f32>(pages[q + 2u]));
    pg_side = bitcast<f32>(pages[q + 3u]);
    pg_node = pages[q + 4u];
    pg_cell = vec3<u32>(pages[q + 6u], pages[q + 7u], pages[q + 8u]);
    pg_octants = pages[q + 9u];
    pg_shift = bitcast<i32>(pages[q + 10u]);
    pg_prev = 0u;
    if PHASE == 2u { pg_prev = bits[page_bit_base(params.prev) + pg_index]; }
    if lid < 16u { atomicStore(&words[lid], 0u); }
    workgroupBarrier();

    let l0 = lid * 2u;
    let l1 = l0 + 1u;
    let f0 = eval_slot(l0);
    let f1 = eval_slot(l1);
    if PHASE == 2u && MODE == 2u {
        if (f0 & 2u) != 0u { atomicOr(&words[l0 >> 5u], 1u << (l0 & 31u)); }
        if (f1 & 2u) != 0u { atomicOr(&words[l1 >> 5u], 1u << (l1 & 31u)); }
    }
    let c0 = f0 & 1u;
    let c1 = f1 & 1u;
    scan[lid] = c0 + c1;
    workgroupBarrier();
    for (var off = 1u; off < 256u; off = off << 1u) {
        var v = scan[lid];
        if lid >= off { v += scan[lid - off]; }
        workgroupBarrier();
        scan[lid] = v;
        workgroupBarrier();
    }
    let total = scan[255];
    let excl = scan[lid] - (c0 + c1);
    if lid == 0u && total > 0u { wg_base = atomicAdd(&draw_args[1], total); }
    workgroupBarrier();
    if total > 0u {
        let out = wg_base + excl;
        if c0 == 1u { visible[out] = pg_index * 512u + l0; }
        if c1 == 1u { visible[out + c0] = pg_index * 512u + l1; }
    }
    if PHASE == 2u && MODE == 2u && pg_valid && lid < 16u {
        bits[slot_bit_base(1u - params.prev) + pg_index * 16u + lid] = atomicLoad(&words[lid]);
    }
}
`;

const t16FinalizeWgsl = dim => `
@group(0) @binding(0) var<storage, read> page_list: array<u32>;
@group(0) @binding(1) var<storage, read_write> dispatch_args: array<u32>;
@group(0) @binding(2) var<storage, read_write> draw_args: array<u32>;
const DIM: u32 = ${dim}u;
@compute @workgroup_size(1)
fn fin_main() {
    let n = page_list[0];
    let x = min(n, DIM);
    dispatch_args[0] = x;
    dispatch_args[1] = select(1u, (n + DIM - 1u) / DIM, n > 0u);
    dispatch_args[2] = 1u;
    draw_args[0] = 36u; draw_args[1] = 0u; draw_args[2] = 0u; draw_args[3] = 0u; draw_args[4] = 0u;
}
`;

const T16_DRAW_VS_WGSL = `
@group(1) @binding(0) var<storage, read> inst: array<u32>;
@group(1) @binding(1) var<storage, read> pages: array<u32>;
@group(2) @binding(0) var<storage, read> visible: array<u32>;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

@vertex
fn vs_main(@builtin(instance_index) ii: u32, @location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> VertexOutput {
    var out: VertexOutput;
    let s = visible[ii];
    let page = s / 512u;
    let p = page * 12u;
    let base = s * 4u;
    let w0 = inst[base];
    let w1 = inst[base + 1u];
    let rel = vec3<u32>(w0 & 0xFFFFu, w0 >> 16u, w1 & 0xFFFFu);
    let rgba = inst[base + 2u];
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

const T16_HIZ_COPY_WGSL = `
@group(0) @binding(0) var src: texture_depth_2d;
@group(0) @binding(1) var dst: texture_storage_2d<r32float, write>;
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= ${WIDTH}u || id.y >= ${HEIGHT}u { return; }
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(textureLoad(src, vec2<i32>(id.xy), 0), 0.0, 0.0, 0.0));
}
`;
const T16_HIZ_REDUCE_WGSL = `
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<r32float, write>;
@group(0) @binding(2) var<uniform> dims: vec4<u32>; // src w, src h, dst w, dst h
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= dims.z || id.y >= dims.w { return; }
    let x0 = i32(min(id.x * 2u, dims.x - 1u));
    let x1 = i32(min(id.x * 2u + 1u, dims.x - 1u));
    let y0 = i32(min(id.y * 2u, dims.y - 1u));
    let y1 = i32(min(id.y * 2u + 1u, dims.y - 1u));
    let m = max(max(textureLoad(src, vec2<i32>(x0, y0), 0).r, textureLoad(src, vec2<i32>(x1, y0), 0).r),
        max(textureLoad(src, vec2<i32>(x0, y1), 0).r, textureLoad(src, vec2<i32>(x1, y1), 0).r));
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(m, 0.0, 0.0, 0.0));
}
`;

function t16Need(limits, name, min) {
    if (!(name in limits) || limits[name] < min) {
        throw new Error(`T16 needs ${name} >= ${min}, device granted ${limits[name]}`);
    }
}

// Global (case independent) T16 resources, created on first use.
function t16Global() {
    const { device, gpu: g } = state;
    if (g.t16) return g.t16;
    const lim = device.limits;
    t16Need(lim, "maxStorageBuffersPerShaderStage", 8);
    t16Need(lim, "maxComputeInvocationsPerWorkgroup", 256);
    t16Need(lim, "maxComputeWorkgroupSizeX", 256);
    t16Need(lim, "maxComputeWorkgroupStorageSize", 2048);
    t16Need(lim, "maxBindGroups", 3);
    t16Need(lim, "maxStorageTexturesPerShaderStage", 1);
    if ("maxStorageBuffersInVertexStage" in lim) t16Need(lim, "maxStorageBuffersInVertexStage", 3);
    const C = GPUShaderStage.COMPUTE, V = GPUShaderStage.VERTEX;
    const storage = (binding, type, visibility = C) => ({ binding, visibility, buffer: { type } });
    const layouts = {
        cam: device.createBindGroupLayout({ entries: [
            { binding: 0, visibility: C, buffer: { type: "uniform" } },
            { binding: 1, visibility: C, buffer: { type: "uniform" } },
            { binding: 2, visibility: C, texture: { sampleType: "unfilterable-float" } }] }),
        pool: device.createBindGroupLayout({ entries: [
            storage(0, "read-only-storage", V | C), storage(1, "read-only-storage", V | C),
            storage(2, "read-only-storage"), storage(3, "read-only-storage")] }),
        pageWork: device.createBindGroupLayout({ entries: [storage(0, "storage"), storage(1, "storage")] }),
        fin: device.createBindGroupLayout({ entries: [storage(0, "read-only-storage"), storage(1, "storage"), storage(2, "storage")] }),
        slotWork: device.createBindGroupLayout({ entries: [storage(0, "read-only-storage"), storage(1, "storage"), storage(2, "storage"), storage(3, "storage")] }),
        vis: device.createBindGroupLayout({ entries: [storage(0, "read-only-storage", V)] }),
    };
    const pipelineLayout = layouts_ => device.createPipelineLayout({ bindGroupLayouts: layouts_ });
    const modules = {
        page: device.createShaderModule({ label: "t16-page", code: T16_COMMON_WGSL + T16_PAGE_WGSL }),
        slot: device.createShaderModule({ label: "t16-slot", code: T16_COMMON_WGSL + T16_SLOT_WGSL }),
        fin: device.createShaderModule({ label: "t16-fin", code: t16FinalizeWgsl(lim.maxComputeWorkgroupsPerDimension) }),
    };
    const cache = new Map();
    const computePipe = (stage, phase, kind) => {
        const key = `${stage}/${phase}/${kind}`;
        if (!cache.has(key)) {
            const spec = T16_KINDS[kind];
            cache.set(key, device.createComputePipeline({
                label: key,
                layout: pipelineLayout([layouts.cam, layouts.pool, stage === "page" ? layouts.pageWork : layouts.slotWork]),
                compute: { module: modules[stage], entryPoint: `${stage}_main`, constants: { PHASE: phase, MODE: spec.mode, FRUSTUM: spec.frustum } },
            }));
        }
        return cache.get(key);
    };
    const finPipe = device.createComputePipeline({ label: "t16-fin", layout: pipelineLayout([layouts.fin]),
        compute: { module: modules.fin, entryPoint: "fin_main" } });

    const drawModule = device.createShaderModule({ label: "t16-draw", code: T16_DRAW_VS_WGSL + COMMON_WGSL });
    const drawPipe = device.createRenderPipeline({
        label: "t16-draw",
        layout: pipelineLayout([g.uniformLayout, layouts.pool, layouts.vis]),
        vertex: { module: drawModule, entryPoint: "vs_main", buffers: [g.cubeBuffer] },
        primitive: { topology: "triangle-list", cullMode: "back" },
        depthStencil: { format: DEPTH_FORMAT, depthWriteEnabled: true, depthCompare: "less" },
        fragment: { module: drawModule, entryPoint: "fs_main", targets: [{ format: COLOR_FORMAT, blend: {
            color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha", operation: "add" },
            alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" } } }] },
    });

    const color = device.createTexture({ size: [WIDTH, HEIGHT], format: COLOR_FORMAT, usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC });
    const depth = device.createTexture({ size: [WIDTH, HEIGHT], format: DEPTH_FORMAT,
        usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_SRC });
    const hiz = device.createTexture({ size: [HIZ_W, HIZ_H], format: "r32float", mipLevelCount: HIZ_MIPS,
        usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.STORAGE_BINDING });
    const hizCopy = device.createComputePipeline({ label: "t16-hiz-copy", layout: "auto",
        compute: { module: device.createShaderModule({ code: T16_HIZ_COPY_WGSL }), entryPoint: "main" } });
    const hizReduce = device.createComputePipeline({ label: "t16-hiz-reduce", layout: "auto",
        compute: { module: device.createShaderModule({ code: T16_HIZ_REDUCE_WGSL }), entryPoint: "main" } });
    const hizCopyGroup = device.createBindGroup({ layout: hizCopy.getBindGroupLayout(0), entries: [
        { binding: 0, resource: depth.createView({ aspect: "depth-only" }) },
        { binding: 1, resource: hiz.createView({ baseMipLevel: 0, mipLevelCount: 1 }) }] });
    const hizLevels = [];
    for (let level = 1; level < HIZ_MIPS; level++) {
        const dims = [Math.ceil(WIDTH / 2 ** (level - 1)), Math.ceil(HEIGHT / 2 ** (level - 1)), Math.ceil(WIDTH / 2 ** level), Math.ceil(HEIGHT / 2 ** level)];
        const buffer = device.createBuffer({ size: 16, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
        device.queue.writeBuffer(buffer, 0, new Uint32Array(dims));
        hizLevels.push({ dims, group: device.createBindGroup({ layout: hizReduce.getBindGroupLayout(0), entries: [
            { binding: 0, resource: hiz.createView({ baseMipLevel: level - 1, mipLevelCount: 1 }) },
            { binding: 1, resource: hiz.createView({ baseMipLevel: level, mipLevelCount: 1 }) },
            { binding: 2, resource: { buffer } }] }) });
    }
    const querySet = device.createQuerySet({ type: "timestamp", count: 2 * T16_MAX_PASSES });
    const resolveBuf = device.createBuffer({ size: 16 * T16_MAX_PASSES, usage: GPUBufferUsage.QUERY_RESOLVE | GPUBufferUsage.COPY_SRC });
    const readBuf = device.createBuffer({ size: 16 * T16_MAX_PASSES, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
    const depthRead = device.createBuffer({ size: WIDTH * HEIGHT * 4, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
    const colorRead = device.createBuffer({ size: WIDTH * HEIGHT * 4, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
    g.t16 = { layouts, computePipe, finPipe, drawPipe, color, depth, colorView: color.createView(), depthView: depth.createView(),
        hiz, hizView: hiz.createView(), hizCopy, hizReduce, hizCopyGroup, hizLevels, querySet, resolveBuf, readBuf, depthRead, colorRead };
    return g.t16;
}

// Tight per-page box over the used slots, computed on the CPU at load (production: fields of PageEntry).
function tightBoxes(instAb, pageCount) {
    const u = new Uint32Array(instAb);
    const slots = u.length / 4;
    const out = new Uint32Array(pageCount * 6);
    for (let p = 0; p < pageCount; p++) out.set([0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF, 0, 0, 0], p * 6);
    for (let s = 0; s < slots; s++) {
        const w0 = u[s * 4], w1 = u[s * 4 + 1];
        if (w0 === 0xFFFFFFFF && w1 === 0xFFFFFFFF) continue;
        const page = s >> 9;
        if (page >= pageCount) continue;
        const x = w0 & 0xFFFF, y = w0 >>> 16, z = w1 & 0xFFFF, o = page * 6;
        if (x < out[o]) out[o] = x;
        if (y < out[o + 1]) out[o + 1] = y;
        if (z < out[o + 2]) out[o + 2] = z;
        if (x > out[o + 3]) out[o + 3] = x;
        if (y > out[o + 4]) out[o + 4] = y;
        if (z > out[o + 5]) out[o + 5] = z;
    }
    return out;
}

// Per-case T16 buffers and bind groups, created on first use after loadCase().
function t16Case() {
    const { device, gpu: g, kase } = state;
    if (kase.t16) return kase.t16;
    const G = t16Global();
    const P = kase.pageBytes.w16.length / 12;
    if (kase.tightBoxes.length !== P * 6) throw new Error("tight boxes do not match the w16 page table");
    const slotsUsed = kase.meta.variants.w16.slots_used;
    const dim = device.limits.maxComputeWorkgroupsPerDimension;
    if (P > dim * dim) throw new Error(`${P} pages exceed even a 2D dispatch (${dim} x ${dim})`);
    const mk = (label, bytes, usage) => {
        const size = Math.max(16, Math.ceil(bytes / 4) * 4);
        if ((usage & GPUBufferUsage.STORAGE) && (size > device.limits.maxBufferSize || size > device.limits.maxStorageBufferBindingSize)) {
            throw new Error(`T16: ${label} needs ${size} B over the granted storage binding limit ${device.limits.maxStorageBufferBindingSize}`);
        }
        const b = device.createBuffer({ label, size, usage });
        kase.buffers.push(b);
        return b;
    };
    const S = GPUBufferUsage.STORAGE, CS = GPUBufferUsage.COPY_SRC, CD = GPUBufferUsage.COPY_DST;
    const boxes = mk("t16-boxes", P * 24, S | CD);
    device.queue.writeBuffer(boxes, 0, kase.tightBoxes);
    const pageList = [0, 1].map(i => mk(`t16-page-list-${i}`, (1 + P) * 4, S | CS | CD));
    const dispatchArgs = [0, 1].map(i => mk(`t16-dispatch-args-${i}`, 12, S | GPUBufferUsage.INDIRECT | CS));
    const drawArgs = [0, 1].map(i => mk(`t16-draw-args-${i}`, 20, S | GPUBufferUsage.INDIRECT | CS));
    const visible = mk("t16-visible", slotsUsed * 4, S);
    const bits = mk("t16-bits", (2 * P + 2 * P * 16) * 4, S | CD);
    const params = mk("t16-params", 16, GPUBufferUsage.UNIFORM | CD);
    const statsRead = mk("t16-stats", 40, GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ);
    const L = G.layouts;
    const bg = (layout, buffers) => device.createBindGroup({ layout, entries: buffers.map((buffer, binding) => ({ binding, resource: { buffer } })) });
    const pool = kase.pool.w16;
    const t16 = {
        pages: P, slotsUsed, prev: 0,
        boxes, pageList, dispatchArgs, drawArgs, visible, bits, params, statsRead,
        camGroup: device.createBindGroup({ layout: L.cam, entries: [
            { binding: 0, resource: { buffer: g.uniformBuffer } }, { binding: 1, resource: { buffer: params } },
            { binding: 2, resource: G.hizView }] }),
        poolGroup: bg(L.pool, [pool.instBuffer, pool.pagesBuffer, kase.nodesBuffer, boxes]),
        pageWork: [0, 1].map(i => bg(L.pageWork, [pageList[i], bits])),
        fin: [0, 1].map(i => bg(L.fin, [pageList[i], dispatchArgs[i], drawArgs[i]])),
        slotWork: [0, 1].map(i => bg(L.slotWork, [pageList[i], visible, drawArgs[i], bits])),
        visGroup: bg(L.vis, [visible]),
    };
    kase.t16 = t16;
    return t16;
}

function dispatchDims(groups) {
    const max = state.device.limits.maxComputeWorkgroupsPerDimension;
    if (groups <= max) return [groups, 1];
    const y = Math.ceil(groups / max);
    if (y > max) throw new Error(`dispatch of ${groups} groups exceeds ${max} x ${max}`);
    return [max, y];
}

function t16ResetBits() {
    const { device } = state;
    const T = t16Case();
    const encoder = device.createCommandEncoder();
    encoder.clearBuffer(T.bits);
    device.queue.submit([encoder.finish()]);
    T.prev = 0;
}

// Static pose: the exported view_proj and page table.
function t16ResetScene() {
    const { device, gpu: g, kase } = state;
    device.queue.writeBuffer(g.uniformBuffer, 0, kase.uniformStatic);
    device.queue.writeBuffer(kase.pool.w16.pagesBuffer, 0, kase.w16PagesPristine);
}

// Frame f of the moving path: yaw 0.5 deg/frame about the world z axis through the eye (view_proj * R) and
// a translation of one median page side per frame along the initial view direction (page-origin rewrite).
function t16SetMovingPose(frame) {
    const { device, gpu: g, kase } = state;
    const vp = kase.meta.view_proj;
    const th = frame * T16_YAW_DEG_PER_FRAME * Math.PI / 180;
    const c = Math.cos(th), s = Math.sin(th);
    const uniform = new Float32Array(kase.uniformStatic);
    for (let r = 0; r < 4; r++) {
        uniform[r] = c * vp[r] + s * vp[4 + r];
        uniform[4 + r] = -s * vp[r] + c * vp[4 + r];
    }
    device.queue.writeBuffer(g.uniformBuffer, 0, uniform);
    const fwd = [vp[3], vp[7], vp[11]];
    const norm = Math.hypot(...fwd);
    const step = frame * kase.t16CellSide / norm;
    const bytes = kase.pageBytes.w16;
    const f32 = new Float32Array(bytes.buffer);
    const world = kase.pageWorld.w16;
    const eye = kase.meta.eye, scale = kase.meta.visual_scale;
    for (let page = 0; page < world.length / 3; page++) {
        for (let axis = 0; axis < 3; axis++) f32[page * 12 + axis] = world[page * 3 + axis] - (eye[axis] * scale + fwd[axis] * step);
    }
    device.queue.writeBuffer(kase.pool.w16.pagesBuffer, 0, bytes);
}

function t16Variants(set) {
    const mk = (name, kind, moving = false) => ({ name, kind, moving });
    if (set !== "t16") throw new Error(`unknown variant set ${set}`);
    return [mk("w16-ranges", "ref"), mk("k", "k"), mk("kp", "kp"), mk("kpc", "kpc"),
        mk("w16-ranges-moving", "ref", true), mk("k-moving", "k", true), mk("kp-moving", "kp", true), mk("kpc-moving", "kpc", true)];
}

// Encodes one frame. Returns the pass names in timestamp order.
function t16EncodeFrame(encoder, v, withStats) {
    const { gpu: g, kase } = state;
    const G = g.t16, T = kase.t16;
    const names = [];
    const tw = name => {
        const i = names.length;
        names.push(name);
        return { querySet: G.querySet, beginningOfPassWriteIndex: 2 * i, endOfPassWriteIndex: 2 * i + 1 };
    };
    const renderPass = (name, load) => encoder.beginRenderPass({
        colorAttachments: [{ view: G.colorView, clearValue: { r: 0, g: 0, b: 0, a: 1 }, loadOp: load ? "load" : "clear", storeOp: "store" }],
        depthStencilAttachment: { view: G.depthView, depthClearValue: 1, depthLoadOp: load ? "load" : "clear", depthStoreOp: "store" },
        timestampWrites: tw(name),
    });
    if (v.kind === "ref") {
        const pass = renderPass("draw-ranges", false);
        pass.setPipeline(g.pipelines.w16);
        pass.setBindGroup(0, g.uniformGroup);
        pass.setVertexBuffer(0, g.vertexBuffer);
        pass.setBindGroup(1, kase.pool.w16.group);
        pass.setIndexBuffer(g.indexBuffer, "uint16");
        const r = kase.ranges.w16;
        for (let i = 0; i < r.length; i += 2) if (r[i + 1] > 0) pass.drawIndexed(36, r[i + 1], 0, 0, r[i]);
        pass.end();
    } else {
        const spec = T16_KINDS[v.kind];
        encoder.clearBuffer(T.pageList[0], 0, 4);
        if (spec.two) encoder.clearBuffer(T.pageList[1], 0, 4);
        const [gx, gy] = dispatchDims(Math.ceil(T.pages / 64));
        const phases = spec.two ? [1, 2] : [0];
        for (const phase of phases) {
            const set = phase === 2 ? 1 : 0;
            const tag = phase === 0 ? "" : `${phase}`;
            if (phase === 2) {
                const hp = encoder.beginComputePass({ label: "hiz", timestampWrites: tw("hiz") });
                hp.setPipeline(G.hizCopy);
                hp.setBindGroup(0, G.hizCopyGroup);
                hp.dispatchWorkgroups(Math.ceil(WIDTH / 8), Math.ceil(HEIGHT / 8));
                hp.setPipeline(G.hizReduce);
                for (const level of G.hizLevels) {
                    hp.setBindGroup(0, level.group);
                    hp.dispatchWorkgroups(Math.ceil(level.dims[2] / 8), Math.ceil(level.dims[3] / 8));
                }
                hp.end();
            }
            let cp = encoder.beginComputePass({ label: `page${tag}`, timestampWrites: tw(`page${tag}`) });
            cp.setPipeline(G.computePipe("page", phase, v.kind));
            cp.setBindGroup(0, T.camGroup);
            cp.setBindGroup(1, T.poolGroup);
            cp.setBindGroup(2, T.pageWork[set]);
            cp.dispatchWorkgroups(gx, gy);
            cp.end();
            cp = encoder.beginComputePass({ label: `fin${tag}`, timestampWrites: tw(`fin${tag}`) });
            cp.setPipeline(G.finPipe);
            cp.setBindGroup(0, T.fin[set]);
            cp.dispatchWorkgroups(1);
            cp.end();
            cp = encoder.beginComputePass({ label: `slot${tag}`, timestampWrites: tw(`slot${tag}`) });
            cp.setPipeline(G.computePipe("slot", phase, v.kind));
            cp.setBindGroup(0, T.camGroup);
            cp.setBindGroup(1, T.poolGroup);
            cp.setBindGroup(2, T.slotWork[set]);
            cp.dispatchWorkgroupsIndirect(T.dispatchArgs[set], 0);
            cp.end();
            const pass = renderPass(`draw${tag}`, phase === 2);
            pass.setPipeline(G.drawPipe);
            pass.setBindGroup(0, g.uniformGroup);
            pass.setBindGroup(1, T.poolGroup);
            pass.setBindGroup(2, T.visGroup);
            pass.setVertexBuffer(0, g.vertexBuffer);
            pass.setIndexBuffer(g.indexBuffer, "uint16");
            pass.drawIndexedIndirect(T.drawArgs[set], 0);
            pass.end();
        }
        if (withStats) {
            encoder.copyBufferToBuffer(T.pageList[0], 0, T.statsRead, 0, 4);
            encoder.copyBufferToBuffer(T.drawArgs[0], 4, T.statsRead, 4, 4);
            encoder.copyBufferToBuffer(T.dispatchArgs[0], 0, T.statsRead, 16, 12);
            if (spec.two) {
                encoder.copyBufferToBuffer(T.pageList[1], 0, T.statsRead, 8, 4);
                encoder.copyBufferToBuffer(T.drawArgs[1], 4, T.statsRead, 12, 4);
                encoder.copyBufferToBuffer(T.dispatchArgs[1], 0, T.statsRead, 28, 12);
            }
        }
    }
    if (names.length > T16_MAX_PASSES) throw new Error(`${names.length} passes exceed T16_MAX_PASSES`);
    encoder.resolveQuerySet(G.querySet, 0, 2 * names.length, G.resolveBuf, 0);
    encoder.copyBufferToBuffer(G.resolveBuf, 0, G.readBuf, 0, 16 * names.length);
    return names;
}

// One frame, submitted and awaited. `counts` also reads the GPU counters (use it only outside timed frames).
async function t16RunFrame(v, frame, counts = false) {
    const { device, gpu: g } = state;
    const G = g.t16, T = t16Case();
    const t0 = performance.now();
    if (v.moving) t16SetMovingPose(frame);
    device.queue.writeBuffer(T.params, 0, new Uint32Array([T.prev, T.pages, 0, 0]));
    const encoder = device.createCommandEncoder();
    const names = t16EncodeFrame(encoder, v, counts && v.kind !== "ref");
    const commands = encoder.finish();
    const t1 = performance.now();
    device.queue.submit([commands]);
    await device.queue.onSubmittedWorkDone();
    const t2 = performance.now();
    await G.readBuf.mapAsync(GPUMapMode.READ, 0, 16 * names.length);
    const ticks = new BigUint64Array(G.readBuf.getMappedRange(0, 16 * names.length).slice(0));
    G.readBuf.unmap();
    const rec = { names, passMs: names.map((_, i) => Number(ticks[2 * i + 1] - ticks[2 * i]) / 1e6),
        gpuTicks: ticks[2 * names.length - 1] - ticks[0],
        gpuMs: Number(ticks[2 * names.length - 1] - ticks[0]) / 1e6, wallMs: t2 - t1, encodeMs: t1 - t0 };
    if (counts && v.kind !== "ref") {
        await T.statsRead.mapAsync(GPUMapMode.READ);
        const s = new Uint32Array(T.statsRead.getMappedRange().slice(0));
        T.statsRead.unmap();
        const two = T16_KINDS[v.kind].two;
        rec.stats = { pages1: s[0], cells1: s[1], dispatch1: [s[4], s[5], s[6]],
            ...(two ? { pages2: s[2], cells2: s[3], dispatch2: [s[7], s[8], s[9]] } : {}) };
        const dim = device.limits.maxComputeWorkgroupsPerDimension;
        for (const [pagesN, d] of [[rec.stats.pages1, rec.stats.dispatch1], ...(two ? [[rec.stats.pages2, rec.stats.dispatch2]] : [])]) {
            if (d[0] > dim || d[1] > dim || d[0] * d[1] < pagesN || (pagesN > 0 && d[0] === 0)) {
                state.errors.push(`${v.name}: indirect dispatch ${d} does not cover ${pagesN} pages (limit ${dim})`);
            }
        }
    }
    // Only variants own the visibility bits; a reference frame interleaved in the check must not flip them.
    if (v.kind !== "ref") T.prev ^= 1;
    return rec;
}

async function t16ReadTargets() {
    const { device, gpu: g } = state;
    const G = g.t16;
    const encoder = device.createCommandEncoder();
    encoder.copyTextureToBuffer({ texture: G.depth, aspect: "depth-only" }, { buffer: G.depthRead, bytesPerRow: WIDTH * 4 }, [WIDTH, HEIGHT]);
    encoder.copyTextureToBuffer({ texture: G.color }, { buffer: G.colorRead, bytesPerRow: WIDTH * 4 }, [WIDTH, HEIGHT]);
    device.queue.submit([encoder.finish()]);
    await Promise.all([G.depthRead.mapAsync(GPUMapMode.READ), G.colorRead.mapAsync(GPUMapMode.READ)]);
    const out = { depth: new Uint32Array(G.depthRead.getMappedRange().slice(0)), color: new Uint32Array(G.colorRead.getMappedRange().slice(0)) };
    G.depthRead.unmap();
    G.colorRead.unmap();
    return out;
}

function t16Compare(ref, got) {
    let holes = 0, shallower = 0, colorDiff = 0, covered = 0;
    for (let i = 0; i < ref.depth.length; i++) {
        const a = ref.depth[i], b = got.depth[i];
        // depth32float values in [0, 1]: the unsigned bit pattern orders like the float
        if (b > a) holes++; else if (b < a) shallower++;
        if (ref.color[i] !== got.color[i]) colorDiff++;
        if (a !== 0x3F800000) covered++;
    }
    return { holes, shallower, colorDiffFraction: colorDiff / ref.depth.length, coveredPixels: covered };
}

function t16Summary(values) {
    const sorted = [...values].sort((a, b) => a - b);
    return { median: median(values), p95: sorted[Math.max(0, Math.ceil(0.95 * sorted.length) - 1)],
        min: sorted[0], max: sorted[sorted.length - 1], n: values.length };
}

async function t16NonBlack() {
    const { color } = await t16ReadTargets();
    let count = 0;
    for (let i = 0; i < color.length; i++) if (color[i] & 0x00FFFFFF) count++;
    return count;
}

async function runT16Variant(v, { warmupMs, frames }) {
    const { device } = state;
    t16Case();
    device.pushErrorScope("validation");
    t16ResetScene();
    t16ResetBits();
    let series = [], nonBlackPixels, counts;
    const rec = { name: v.name, kind: v.kind, moving: v.moving };
    if (!v.moving) {
        const start = performance.now();
        let warm = 0;
        while (performance.now() - start < warmupMs || warm < 3) { await t16RunFrame(v, warm, false); warm++; }
        nonBlackPixels = await t16NonBlack();
        for (let i = 0; i < frames; i++) series.push(await t16RunFrame(v, i));
        if (v.kind !== "ref") counts = (await t16RunFrame(v, frames, true)).stats;
    } else {
        for (let f = 0; f < 5; f++) await t16RunFrame(v, f); // compile and warm caches; not part of the series
        t16ResetBits();
        for (let f = 0; f < T16_MOVING_FRAMES; f++) series.push(await t16RunFrame(v, f));
        // The path may leave the scene (synthetic case), so the non-black check looks at frame 1, outside the series.
        t16ResetBits();
        await t16RunFrame(v, 0);
        await t16RunFrame(v, 1);
        nonBlackPixels = await t16NonBlack();
        if (v.kind !== "ref") {
            t16ResetBits();
            counts = [];
            for (let f = 0; f < T16_MOVING_FRAMES; f++) counts.push((await t16RunFrame(v, f, true)).stats);
        }
        const warmSeries = series.slice(1).map(r => r.gpuMs);
        rec.cold = { gpuMs: series[0].gpuMs, passMs: series[0].passMs };
        rec.warm = t16Summary(warmSeries); // frames 1..119
        if (counts && counts[0].cells2 !== undefined) {
            const tail = counts.slice(30).map(c => c.cells2).sort((a, b) => a - b);
            const ref = tail[tail.length >> 1];
            let stable = counts.length;
            while (stable > 0 && counts[stable - 1].cells2 <= 1.5 * ref + 16) stable--;
            rec.stableFrame = { rule: "first frame after which phase-2 compacted cells never exceed 1.5 x median(frames 30..119) + 16", frame: stable, reference: ref };
        }
    }
    const error = await device.popErrorScope();
    if (error) state.errors.push(`${v.name}: validation: ${error.message}`);
    t16ResetScene();
    const names = series[0].names;
    return { ...rec, passes: names.map((name, i) => ({ name, ms: t16Summary(series.map(r => r.passMs[i])) })),
        nonBlackPixels, counts,
        summary: { gpuMs: t16Summary(series.map(r => r.gpuMs)), wallMs: t16Summary(series.map(r => r.wallMs)),
            encodeMs: t16Summary(series.map(r => r.encodeMs)) },
        tickGcd: Number(series.map(r => r.gpuTicks).filter(t => t > 0n).reduce(gcd, 0n)),
        series: { gpuMs: series.map(r => r.gpuMs), gpuTicks: series.map(r => Number(r.gpuTicks)) } };
}

async function runT16Round({ warmupMs, frames, round }) {
    const all = t16Variants("t16");
    const order = all.map((_, i) => all[(i + round) % all.length]);
    const results = [];
    for (const variant of order) {
        log(`round ${round}: ${variant.name}`);
        results.push(await runT16Variant(variant, { warmupMs, frames }));
    }
    return { round, set: "t16", order: order.map(v => v.name), errors: state.errors.splice(0), variants: results };
}

// Untimed correctness check (probe --t16-check): counts and bit-identical depth against w16-ranges.
async function runT16Check() {
    if (!state.kase) throw new Error("call loadCase() first");
    const { device, kase } = state;
    t16Case();
    const out = { failures: [], count: {}, static: {}, moving: {} };
    const fail = message => out.failures.push(message);
    device.pushErrorScope("validation");
    const make = (name, kind, moving = false) => ({ name, kind, moving });
    t16ResetScene();

    // (a) k without frustum must compact exactly case.json drawn_cells.
    t16ResetBits();
    const knf = await t16RunFrame(make("k-nofrustum", "knf"), 0, true);
    t16ResetBits();
    const kFrustum = await t16RunFrame(make("k", "k"), 0, true);
    out.count = { drawn_cells: kase.meta.drawn_cells, k_nofrustum_compacted: knf.stats.cells1, k_nofrustum_pages: knf.stats.pages1,
        k_compacted: kFrustum.stats.cells1, k_pages: kFrustum.stats.pages1, total_pages: kase.t16.pages,
        ok: knf.stats.cells1 === kase.meta.drawn_cells };
    if (!out.count.ok) fail(`k-nofrustum compacted ${knf.stats.cells1} cells, case.json drawn_cells ${kase.meta.drawn_cells}`);

    // (b) static pose: depth bit-identical to w16-ranges, several frames per two-phase variant (cold start, then warm bits).
    await t16RunFrame(make("w16-ranges", "ref"), 0);
    const staticRef = await t16ReadTargets();
    const staticCovered = t16Compare(staticRef, staticRef).coveredPixels;
    if (!(staticCovered > 0)) fail(`static reference covers no pixel`);
    for (const [kind, nFrames] of [["knf", 1], ["k", 1], ["kp", 6], ["kpc", 6]]) {
        t16ResetBits();
        const frames = [];
        for (let f = 0; f < nFrames; f++) {
            const rec = await t16RunFrame(make(kind, kind), f, true);
            const cmp = t16Compare(staticRef, await t16ReadTargets());
            frames.push({ ...cmp, ...rec.stats });
            if (cmp.holes || cmp.shallower) fail(`static ${kind} frame ${f}: ${cmp.holes} holes, ${cmp.shallower} shallower pixels`);
        }
        out.static[kind] = { frames };
    }

    // (c) moving: every frame of the path against w16-ranges rendered with the same uniform and pages.
    for (const kind of ["k", "kp", "kpc"]) {
        t16ResetBits();
        const v = make(`${kind}-moving`, kind, true), ref = make("w16-ranges-moving", "ref", true);
        let holes = 0, shallower = 0, colorSum = 0, colorMax = 0, minPhase1 = Infinity, culledPagesMax = 0, emptyRefFrames = 0;
        const first = [];
        for (let f = 0; f < T16_MOVING_FRAMES; f++) {
            await t16RunFrame(ref, f);
            const r = await t16ReadTargets();
            const rec = await t16RunFrame(v, f, true);
            const cmp = t16Compare(r, await t16ReadTargets());
            if (!(cmp.coveredPixels > 0)) { emptyRefFrames++; if (f < 10) fail(`moving ${kind} frame ${f}: reference covers no pixel`); }
            holes += cmp.holes; shallower += cmp.shallower; colorSum += cmp.colorDiffFraction;
            colorMax = Math.max(colorMax, cmp.colorDiffFraction);
            culledPagesMax = Math.max(culledPagesMax, kase.t16.pages - (rec.stats.pages2 ?? rec.stats.pages1));
            // Frames whose reference is empty (the path left the data) say nothing about temporal reuse.
            if (rec.stats.cells1 !== undefined && f > 0 && cmp.coveredPixels > 0) minPhase1 = Math.min(minPhase1, rec.stats.cells1);
            if (f < 10) first.push({ frame: f, ...cmp, ...rec.stats });
            if (cmp.holes || cmp.shallower) fail(`moving ${kind} frame ${f}: ${cmp.holes} holes, ${cmp.shallower} shallower pixels`);
        }
        out.moving[kind] = { frames: T16_MOVING_FRAMES, holes, shallower, colorDiffFractionMean: colorSum / T16_MOVING_FRAMES,
            colorDiffFractionMax: colorMax, emptyRefFrames, minPhase1Cells: minPhase1, first10: first };
        if (T16_KINDS[kind].two && emptyRefFrames < T16_MOVING_FRAMES - 1 && !(minPhase1 > 0 && minPhase1 < Infinity)) fail(`moving ${kind}: phase 1 drew ${minPhase1} cells after frame 0, temporal reuse not exercised`);
    }
    t16ResetScene();
    const error = await device.popErrorScope();
    if (error) fail(`validation: ${error.message}`);
    out.errors = state.errors.splice(0);
    for (const e of out.errors) fail(e);
    return out;
}

window.__cubePool = { init, loadCase, runRound, runDiag, runT16Check, unloadCase, get errors() { return state.errors; } };

const params = new URLSearchParams(location.search);
if (params.has("case")) {
    (async () => {
        try {
            const info = await init();
            const summary = await loadCase(params.get("case"));
            const round = await runRound({ warmupMs: Number(params.get("warmupMs") ?? 2000), frames: Number(params.get("frames") ?? 30), set: params.get("set") ?? "t5" });
            window.__cubePoolResult = { info, summary, round };
            log(JSON.stringify(round.variants.map(v => [v.name, v.summary.gpuMs.median, v.nonBlackPixels])));
        } catch (error) { log(`error: ${error.stack ?? error}`); }
    })();
}
