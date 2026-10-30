// Scripted load + orbit probe (render plan R0-T1). Run from the repository root while a static server
// serves the worktree root (with pkg/). Default backend: Chrome launched by playwright-core with the
// ANGLE-Vulkan WebGPU flags (hardware adapter). --browser=firefox instead drives a fresh Firefox
// with WebGPU enabled and --remote-debugging-port 9222 over raw WebDriver BiDi (software adapter on
// the reference machine, so functional only):
//   node examples/web-host/probe-orbit-load.mjs INPUT.oct OUTPUT.json [--url=URL] [--px=4]
//        [--max-load-s=120] [--orbit-s=10] [--pixels] [--force-timing] [--browser=chrome|firefox]
// It loads the dataset, orbits the camera for a fixed time while loading and (if the target LOD
// was reached) again afterwards, and records frame-reply intervals, worker busy time, the LOD
// message log, deterministic counters, adapter evidence and the environment to OUTPUT.json.
// Timing is only valid in "timing" mode: the probe falls back to "functional" when a cargo/rustc
// build or another browser instance is running, or when no hardware WebGPU adapter is evidenced
// (software/unidentified adapter, or gpu_busy_percent never above 10). --pixels adds opt-in screenshots to count empty pixels.
import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { createRequire } from "node:module";

import { countEmptyPixels, convexHull, decodePng, projectBounds } from "./pixel-metrics.mjs";

const args = process.argv.slice(2);
const positional = args.filter(arg => !arg.startsWith("--"));
const option = (name, fallback) => args.find(arg => arg.startsWith(`--${name}=`))?.split("=")[1] ?? fallback;
const [input, output] = positional;
const url = option("url", "http://127.0.0.1:8767/examples/web-host/index.html");
const targetPixels = Number(option("px", "4"));
const maxLoadMs = Number(option("max-load-s", "120")) * 1000;
const orbitMs = Number(option("orbit-s", "10")) * 1000;
const capturePixels = args.includes("--pixels");
const browser = option("browser", "chrome");
const CHROME_BIN = process.env.CHROME_BIN ?? "/usr/bin/google-chrome";
const PLAYWRIGHT_CORE = process.env.PLAYWRIGHT_CORE ?? "/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/";
if (!["chrome", "firefox"].includes(browser) || !input || !output || !Number.isInteger(targetPixels * 2) || targetPixels < 0.5 || targetPixels > 16) {
    throw new Error("Usage: node examples/web-host/probe-orbit-load.mjs INPUT.oct OUTPUT.json [--url=URL] [--px=4] [--max-load-s=120] [--orbit-s=10] [--pixels] [--browser=chrome|firefox]");
}
const ORBIT_STEP_MS = 16;

const pids = (name) => {
    try { return execFileSync("pgrep", ["-x", name], { encoding: "utf8" }).split("\n").filter(Boolean).map(Number); }
    catch { return []; }
};
const functionalReasons = [];
if (pids("cargo").length || pids("rustc").length) functionalReasons.push("cargo/rustc running");
// The probe's own browser is started later, so anything already running is foreign load.
if (pids("firefox").length > (browser === "firefox" ? 1 : 0)) functionalReasons.push("other firefox instance running");
if (pids("chrome").length) functionalReasons.push("chrome already running");

function vulkanEvidence() {
    const processes = [];
    for (const pid of [...pids("firefox"), ...pids("chrome")]) {
        try {
            const maps = fs.readFileSync(`/proc/${pid}/maps`, "utf8");
            const dri = fs.readdirSync(`/proc/${pid}/fd`).filter((fd) => {
                try { return fs.readlinkSync(`/proc/${pid}/fd/${fd}`).startsWith("/dev/dri"); } catch { return false; }
            }).length;
            processes.push({
                pid,
                libvulkanRadeon: maps.includes("libvulkan_radeon"),
                libvulkanLvp: maps.includes("libvulkan_lvp"),
                libLLVM: maps.includes("libLLVM"),
                driFds: dri,
            });
        } catch { /* process exited */ }
    }
    return processes;
}
const gpuFiles = fs.readdirSync("/sys/class/drm").filter(name => /^card\d+$/.test(name))
    .map(name => `/sys/class/drm/${name}/device/gpu_busy_percent`).filter(file => fs.existsSync(file));
const gpuBusySamples = [];
const gpuTimer = setInterval(() => {
    for (const file of gpuFiles) {
        try { gpuBusySamples.push({ t: Date.now(), card: file.split("/")[4], percent: Number(fs.readFileSync(file, "utf8")) }); }
        catch { /* sysfs unavailable */ }
    }
}, 250);

const errors = [];
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

// Browser backend: the probe logic below only uses this small surface.
//   evaluate(expr) -> { value }   navigate(url)   setViewport(w, h)   setDatasetFile(path)
//   gesture(label, moves, ms)     screenshot(clip?) -> PNG Buffer     close()
async function firefoxBackend() {
    const socket = new WebSocket("ws://127.0.0.1:9222/session");
    const pending = new Map();
    let nextId = 0;
    socket.onmessage = ({ data }) => {
        const message = JSON.parse(data);
        if (message.method === "log.entryAdded" && message.params.level === "error") errors.push(message.params);
        const request = pending.get(message.id);
        if (!request) return;
        clearTimeout(request.timer);
        pending.delete(message.id);
        if (message.type === "error") request.reject(new Error(JSON.stringify(message)));
        else request.resolve(message.result);
    };
    await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
    const send = (method, params, timeoutMs = 60_000) => new Promise((resolve, reject) => {
        const id = ++nextId;
        const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timeout`)); }, timeoutMs);
        pending.set(id, { resolve, reject, timer });
        socket.send(JSON.stringify({ id, method, params }));
    });
    await send("session.new", { capabilities: {} });
    const context = (await send("browsingContext.getTree", {})).contexts[0].context;
    await send("session.subscribe", { events: ["log.entryAdded"] });
    const evaluate = async (expression) => {
        const result = await send("script.evaluate", { expression, target: { context }, awaitPromise: true });
        if (result.type !== "success") throw new Error(JSON.stringify(result));
        return result.result;
    };
    return {
        evaluate,
        navigate: url => send("browsingContext.navigate", { context, url, wait: "complete" }),
        setViewport: (width, height) => send("browsingContext.setViewport", { context, viewport: { width, height }, devicePixelRatio: 1 }),
        async setDatasetFile(file) {
            const element = await evaluate('document.querySelector("#dataset-file")');
            await send("input.setFiles", { context, element: { sharedId: element.sharedId }, files: [file] });
        },
        gesture: (label, moves, durationMs) => send("input.performActions", { context, actions: [{
            type: "pointer", id: `orbit-${label}`, parameters: { pointerType: "mouse" },
            actions: [{ type: "pointerMove", x: 640, y: 360 }, { type: "pointerDown", button: 2 },
                ...moves.map(m => ({ type: "pointerMove", ...m, duration: ORBIT_STEP_MS })), { type: "pointerUp", button: 2 }],
        }] }, durationMs + 60_000),
        async screenshot(clip) {
            const shot = await send("browsingContext.captureScreenshot", { context, origin: "viewport", ...(clip ? { clip: { type: "box", ...clip } } : {}) });
            return Buffer.from(shot.data, "base64");
        },
        async close() { await send("browser.close", {}).catch(() => {}); socket.close(); },
    };
}

// Chrome headless with ANGLE on Vulkan exposes the GPU as a real WebGPU adapter.
async function chromeBackend() {
    const { chromium } = createRequire(PLAYWRIGHT_CORE)("playwright-core");
    const instance = await chromium.launch({
        executablePath: CHROME_BIN,
        headless: true,
        args: ["--headless=new", "--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=vulkan"],
    });
    const page = await (await instance.newContext({ viewport: { width: 1688, height: 768 }, deviceScaleFactor: 1 })).newPage();
    page.on("console", message => { if (message.type() === "error") errors.push({ text: message.text() }); });
    page.on("pageerror", error => errors.push({ text: String(error) }));
    return {
        evaluate: async expression => ({ value: await page.evaluate(expression) }),
        navigate: url => page.goto(url, { waitUntil: "load" }),
        setViewport: (width, height) => page.setViewportSize({ width, height }),
        setDatasetFile: file => page.setInputFiles("#dataset-file", file),
        async gesture(_label, moves) {
            // Same fixed cadence as the BiDi action chain: one move per step on an absolute schedule.
            await page.mouse.move(640, 360);
            await page.mouse.down({ button: "right" });
            const t0 = Date.now();
            for (const [i, move] of moves.entries()) {
                await page.mouse.move(move.x, move.y);
                await sleep(Math.max(0, t0 + (i + 1) * ORBIT_STEP_MS - Date.now()));
            }
            await page.mouse.up({ button: "right" });
        },
        screenshot: clip => page.screenshot({ type: "png", ...(clip ? { clip } : {}) }),
        close: () => instance.close().catch(() => {}),
    };
}
const backend = await (browser === "chrome" ? chromeBackend() : firefoxBackend());
const evaluate = expression => backend.evaluate(expression);
// Evaluates an async expression and returns its JSON value (typed arrays become plain arrays).
async function evalJson(expression) {
    const result = await evaluate(`(async () => JSON.stringify(await (${expression}),
        (key, value) => ArrayBuffer.isView(value) ? Array.from(value) : value))()`);
    return JSON.parse(result.value);
}
const pageEpoch = () => evalJson("performance.timeOrigin + performance.now()");

async function orbit(label, durationMs) {
    const startEpoch = await pageEpoch();
    const steps = Math.round(durationMs / ORBIT_STEP_MS);
    // Fixed-rate back-and-forth sweep: one pointer move per step for the whole duration.
    const moves = Array.from({ length: steps }, (_, i) => ({
        x: Math.round(640 + 300 * Math.sin(i * 0.05)),
        y: Math.round(360 + 80 * Math.sin(i * 0.031)),
    }));
    const gesture = backend.gesture(label, moves, durationMs);
    const captures = [];
    if (capturePixels) {
        for (const fraction of [0.25, 0.5, 0.75]) {
            await sleep(Math.max(0, durationMs * fraction - (Date.now() - Date.parse(new Date(startEpoch).toISOString()))));
            captures.push(await captureEmptyPixels().catch(error => ({ error: String(error) })));
        }
    }
    await gesture;
    return { label, startEpoch, endEpoch: await pageEpoch(), requestedMs: durationMs, stepMs: ORBIT_STEP_MS, emptyPixelCaptures: captures };
}

let boundsCache = null;
async function captureEmptyPixels() {
    boundsCache ??= await evalJson("window.__renderBench.apiCall('getMetadata')");
    if (!boundsCache?.boundsMin) { boundsCache = null; return { epoch: await pageEpoch(), error: "metadata not ready" }; }
    const pose = await evalJson("window.__renderBench.apiCall('getCamera')");
    const rect = await evalJson(`(() => { const r = document.querySelector("canvas").getBoundingClientRect(); return {x: r.x, y: r.y, width: r.width, height: r.height}; })()`);
    const image = decodePng(await backend.screenshot(rect));
    // Node bounds and camera poses are both in source dataset coordinates.
    const min = Array.from(Object.values(boundsCache.boundsMin));
    const max = Array.from(Object.values(boundsCache.boundsMax));
    const corners = projectBounds(pose.position, pose.target, min, max, image.width, image.height);
    const hull = corners ? convexHull(corners) : null;
    return {
        epoch: await pageEpoch(),
        image: [image.width, image.height],
        cornersBehindCamera: corners === null,
        camera: pose,
        worldBounds: [min, max],
        hullPx: hull,
        ...countEmptyPixels(image, hull),
        wholeImage: countEmptyPixels(image, null),
    };
}

const startedAt = new Date().toISOString();
const loadavgStart = os.loadavg();
const wasmUrl = new URL("/pkg/viewer_wasm_bg.wasm", url);
const wasmSha256 = crypto.createHash("sha256").update(Buffer.from(await (await fetch(wasmUrl)).arrayBuffer())).digest("hex");
const datasetPath = path.resolve(input);
const attributesPath = `${datasetPath}attrs`;
const result = { schemaVersion: 1, startedAt, probe: "probe-orbit-load", targetPixels, orbitMs, maxLoadMs };
try {
    result.browser = browser;
    await backend.navigate(url);
    await backend.setViewport(1688, 768);
    const readyUntil = Date.now() + 120_000;
    while ((await evaluate('document.querySelector("#dataset-file").disabled')).value) {
        if (Date.now() > readyUntil) throw new Error("Viewer startup timeout");
        await sleep(300);
    }
    result.adapter = await evalJson(`(async () => {
        const adapter = navigator.gpu ? await navigator.gpu.requestAdapter() : null;
        if (!adapter) return null;
        const { vendor, architecture, device, description } = adapter.info ?? {};
        return { vendor, architecture, device, description, isFallbackAdapter: adapter.isFallbackAdapter };
    })()`);
    await evaluate(`document.querySelector("canvas").style.width="1282px";
        document.querySelector("canvas").style.height="722px";
        window.dispatchEvent(new Event("resize"));
        document.querySelector("#voxel-pixels").value=${targetPixels};
        document.querySelector("#voxel-pixels").dispatchEvent(new Event("input"));
        window.__renderBench.start(); true`);
    await sleep(500);
    const idleGpu = gpuBusySamples.slice(-4).map(s => s.percent);
    const loadStartEpoch = await pageEpoch();
    await backend.setDatasetFile(datasetPath);
    const loadStartedMs = Date.now();
    // Orbit only once the first LOD message exists, so the gesture overlaps real loading.
    const statusText = async () => (await evaluate('document.querySelector("#renderer-status").textContent')).value;
    while (!(await evalJson("window.__renderBench.latestLodCounters()")) && Date.now() - loadStartedMs < 60_000) {
        if (/failed/.test(await statusText())) throw new Error(`dataset did not open: ${await statusText()}`);
        await sleep(200);
    }
    const phases = [];
    const isTarget = async () => evalJson(`(() => {
        const status = document.querySelector("#renderer-status").textContent;
        const c = window.__renderBench.latestLodCounters();
        return { status, reached: /target reached/.test(status), counters: c };
    })()`);
    result.vulkan = vulkanEvidence();
    phases.push(await orbit("during-load", orbitMs));
    result.vulkanDuringRender = vulkanEvidence();
    let state = await isTarget();
    while (!state.reached && Date.now() - loadStartedMs < maxLoadMs && !/failed|paused/.test(state.status)) {
        await sleep(2_000);
        state = await isTarget();
    }
    result.targetReached = state.reached;
    result.finalStatus = state.status;
    result.loadElapsedMs = Date.now() - loadStartedMs;
    if (state.reached) phases.push(await orbit("after-target", orbitMs));
    const endEpoch = await pageEpoch();
    result.loadStartEpoch = loadStartEpoch;
    result.endEpoch = endEpoch;
    result.phases = phases.map(phase => ({ ...phase, frames: undefined }));
    for (const phase of result.phases) {
        phase.summary = await evalJson(`window.__renderBench.summarize(${phase.startEpoch}, ${phase.endEpoch})`);
    }
    result.wholeRunFrames = await evalJson(`window.__renderBench.summarize(${loadStartEpoch}, ${endEpoch})`);
    result.workerMetrics = await evalJson("window.__renderBench.workerMetrics()");
    result.counters = await evalJson("window.__renderBench.latestLodCounters()");
    result.metadata = await evalJson("window.__renderBench.apiCall('getMetadata')");
    result.metadata = { name: result.metadata?.name, bytes: result.metadata?.bytes, nodeCount: result.metadata?.nodeCount, sourcePointCount: result.metadata?.sourcePointCount };
    fs.writeFileSync(`${output}.png`, await backend.screenshot());
    if (capturePixels) result.finalEmptyPixels = await captureEmptyPixels();
    result.idleGpuBusyPercent = idleGpu;
} catch (error) {
    result.error = String(error?.stack ?? error);
} finally {
    clearInterval(gpuTimer);
    const loadavgEnd = os.loadavg();
    const busy = gpuBusySamples.map(sample => sample.percent);
    result.gpuBusy = {
        files: gpuFiles,
        sampleCount: busy.length,
        max: busy.length ? Math.max(...busy) : null,
        mean: busy.length ? busy.reduce((a, b) => a + b, 0) / busy.length : null,
        samples: gpuBusySamples,
    };
    // Hardware evidence needed before any wall-clock number is trusted (plan E1).
    const { vendor, architecture } = result.adapter ?? {};
    if (!vendor || !architecture || /google|llvmpipe|swiftshader/i.test(`${vendor} ${architecture}`) || result.adapter.isFallbackAdapter) {
        functionalReasons.push("software or unidentified WebGPU adapter");
    }
    if (!(result.gpuBusy.max > 10)) functionalReasons.push("gpu_busy_percent never above 10 (hardware adapter not evidenced)");
    result.mode = functionalReasons.length && !args.includes("--force-timing") ? "functional" : "timing";
    result.functionalReasons = functionalReasons;
    result.environment = {
        loadavgStart, loadavgEnd, cpus: os.cpus().length, node: process.version,
        wasmUrl: wasmUrl.href, wasmSha256,
        dataset: { path: datasetPath, bytes: fs.statSync(datasetPath).size, attributesBytes: fs.existsSync(attributesPath) ? fs.statSync(attributesPath).size : null },
    };
    result.consoleErrors = errors;
    fs.mkdirSync(path.dirname(path.resolve(output)), { recursive: true });
    fs.writeFileSync(output, JSON.stringify(result, null, 2) + "\n");
    await backend.close();
    console.log(JSON.stringify({ mode: result.mode, reasons: functionalReasons, targetReached: result.targetReached, error: result.error ?? null }));
}
