// Scripted load + orbit probe (render plan R0-T1). Run from the repository root while a static server
// serves the worktree root (with pkg/). Default backend: Chrome launched by playwright-core with the
// ANGLE-Vulkan WebGPU flags (hardware adapter). --browser=firefox instead drives a fresh Firefox
// with WebGPU enabled and --remote-debugging-port 9222 over raw WebDriver BiDi (software adapter on
// the reference machine, so functional only):
//   node examples/web-host/probe-orbit-load.mjs INPUT.oct OUTPUT.json [--url=URL] [--px=4]
//        [--max-load-s=120] [--orbit-s=10] [--pixels] [--force-timing] [--browser=chrome|firefox]
//        [--build-worker | --no-build-worker] [--cells=0|1] [--remote [--rtt-ms=50] [--mbps=100]] [--attrs]
//        [--pixels=final] [--pss-out=FILE.jsonl] [--jump-after-ms=N] [--color=rgb|intensity|classification|attribute:COLUMN]
// --color=MODE selects the demo's Color control once the dataset is open (the demo has no URL parameter for it):
// `attribute:COLUMN` picks the single-component sidecar column COLUMN (needs --attrs); default `rgb` changes nothing.
// OUTPUT.json `colorMode` records the request and the value read back from the select and the render worker.
// --jump-after-ms=N: N ms after the load starts (blocks still loading, use --px=1) the camera jumps to the
// fitted pose seen from 10x farther, so reads in flight become stale. With --remote the client counters
// (`remoteClient`: requests, bytes, aborted, abortedBytes per worker) sit next to the server's getRequests/bodyBytes.
// R3 B6 additions: --cells=N adds ?cells=N; --remote serves the dataset directory through the RED-REF
// server (red-ref-server.mjs, HTTP Range) and opens it with ?url= (sidecar off unless --attrs);
// --attrs also gives the local sidecar. --max-gap=BYTES and --remote-reads=N add ?maxGap= / ?remoteReads= (D2). Records Chrome process-tree PSS (sample-process-memory.py),
// GTT over idle, GPU frame time (timestamp queries), the T11 "reached" definition on the cells path
// and per-orbit LOD-stage / uncovered-node series from the worker LOD log (RQ-02).
// It loads the dataset, orbits the camera for a fixed time while loading and (if the target LOD
// was reached) again afterwards, and records frame-reply intervals, worker busy time, the LOD
// message log, deterministic counters, adapter evidence and the environment to OUTPUT.json.
// Timing is only valid in "timing" mode: the probe falls back to "functional" when a cargo/rustc
// build or another browser instance is running, or when no hardware WebGPU adapter is evidenced
// (software or unidentified adapter; gpu_busy_percent is recorded but not required). --pixels adds opt-in screenshots to count empty pixels.
import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { createRequire } from "node:module";

import { lodTargetReached } from "./benchmark-metrics.mjs";
import { startServer } from "./red-ref-server.mjs";
import { countEmptyPixels, convexHull, decodePng, projectBounds } from "./pixel-metrics.mjs";

const args = process.argv.slice(2);
const positional = args.filter(arg => !arg.startsWith("--"));
const option = (name, fallback) => args.find(arg => arg.startsWith(`--${name}=`))?.split("=")[1] ?? fallback;
const [input, output] = positional;
// The build worker (render plan R2) is the demo default; --no-build-worker requests ?buildWorker=0
// (the R0 baseline arm). --build-worker is accepted and requests the default explicitly.
if (args.includes("--build-worker") && args.includes("--no-build-worker")) {
    throw new Error("--build-worker and --no-build-worker are mutually exclusive");
}
const cellsOption = option("cells", null);
const remote = args.includes("--remote");
const withAttrs = args.includes("--attrs");
const pixelsMode = args.includes("--pixels") ? "orbit" : (option("pixels", null) === "final" ? "final" : null);
const restFirst = args.includes("--rest-first");
const colorRequest = option("color", "rgb");
const jumpAfterMs = args.some(arg => arg.startsWith("--jump-after-ms=")) ? Number(option("jump-after-ms", "0")) : null;
const psssOut = option("pss-out", null); const buildWorker = !args.includes("--no-build-worker");
const url = (() => {
    const base = new URL(option("url", "http://127.0.0.1:8767/examples/web-host/index.html"));
    base.searchParams.set("buildWorker", buildWorker ? "1" : "0");
    if (cellsOption !== null) base.searchParams.set("cells", cellsOption);
    return base.href;
})();
const targetPixels = Number(option("px", "4"));
const maxLoadMs = Number(option("max-load-s", "120")) * 1000;
const orbitMs = Number(option("orbit-s", "10")) * 1000;
const capturePixels = pixelsMode === "orbit";
const browser = option("browser", "chrome");
const CHROME_BIN = process.env.CHROME_BIN ?? "/usr/bin/google-chrome";
const PLAYWRIGHT_CORE = process.env.PLAYWRIGHT_CORE ?? "/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/";
if (!["chrome", "firefox"].includes(browser) || !input || !output || !Number.isInteger(targetPixels * 2) || targetPixels < 0.5 || targetPixels > 16
    || !/^(rgb|intensity|classification|attribute:.+)$/.test(colorRequest)) {
    throw new Error("Usage: node examples/web-host/probe-orbit-load.mjs INPUT.oct OUTPUT.json [--url=URL] [--px=4] [--max-load-s=120] [--orbit-s=10] [--pixels] [--browser=chrome|firefox] [--build-worker|--no-build-worker] [--color=rgb|intensity|classification|attribute:COLUMN]");
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

// GTT_FILE overrides; otherwise the first DRM card that exposes mem_info_gtt_used (amdgpu).
const GTT_FILE = process.env.GTT_FILE ?? fs.readdirSync("/sys/class/drm").filter(name => /^card\d+$/.test(name)).sort()
    .map(name => `/sys/class/drm/${name}/device/mem_info_gtt_used`).find(file => fs.existsSync(file)) ?? null;
const readGtt = () => { if (!GTT_FILE) return null; try { const t = fs.readFileSync(GTT_FILE, "utf8").trim(); return /^\d+$/.test(t) ? Number(t) : null; } catch { return null; } };
const gttBaseline = readGtt();
let gttPeak = gttBaseline;
const gttTimer = setInterval(() => { const v = readGtt(); if (v !== null && (gttPeak === null || v > gttPeak)) gttPeak = v; }, 100);

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
        // allow_unsafe_apis + developer features expose `timestamp-query` unquantized (R3-Q31).
        args: ["--headless=new", "--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=vulkan",
            "--enable-dawn-features=allow_unsafe_apis", "--enable-webgpu-developer-features"],
    });
    const page = await (await instance.newContext({ viewport: { width: 1688, height: 768 }, deviceScaleFactor: 1 })).newPage();
    page.on("console", message => { if (message.type() === "error") errors.push({ text: message.text() }); });
    page.on("pageerror", error => errors.push({ text: String(error) }));
    return {
        evaluate: async expression => ({ value: await page.evaluate(expression) }),
        navigate: url => page.goto(url, { waitUntil: "load" }),
        setViewport: (width, height) => page.setViewportSize({ width, height }),
        setDatasetFile: file => page.setInputFiles("#dataset-file", file),
        setAttributesFile: file => page.setInputFiles("#attributes-file", file),
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
// Stop only what this probe started (its browser tree, sampler and RED-REF server) on a signal.
let remoteServer = null;
for (const signal of ["SIGINT", "SIGTERM"]) {
    process.on(signal, async () => {
        pssSampler?.kill("SIGTERM");
        await remoteServer?.close?.().catch?.(() => {});
        await backend.close();
        process.exit(130);
    });
}
const evaluate = expression => backend.evaluate(expression);

// Chrome process-tree PSS (R3 B6, RQ-05): the root is the chrome process whose parent is not chrome.
const ppidOf = pid => { try { return Number(fs.readFileSync(`/proc/${pid}/stat`, "utf8").split(")").pop().trim().split(" ")[1]); } catch { return null; } };
let pssSampler = null, pssFile = null, chromeRoot = null;
if (browser === "chrome") {
    const all = pids("chrome");
    chromeRoot = all.find(pid => !all.includes(ppidOf(pid))) ?? null;
    if (chromeRoot) {
        pssFile = psssOut ?? `${output}.pss.jsonl`;
        fs.mkdirSync(path.dirname(path.resolve(pssFile)), { recursive: true });
        fs.rmSync(pssFile, { force: true });
        pssSampler = spawn("python3", [path.resolve(path.dirname(new URL(import.meta.url).pathname), "sample-process-memory.py"), String(chromeRoot), pssFile, "--interval", "0.5", "--seconds", "1500"], { stdio: "ignore" });
    }
}
const processType = pid => { try { const c = fs.readFileSync(`/proc/${pid}/cmdline`, "utf8").split(/[\0 ]/); return c.find(a => a.startsWith("--type="))?.slice(7) ?? "browser"; } catch { return null; } };
function summarizePss() {
    if (!pssFile || !fs.existsSync(pssFile)) return null;
    const lines = fs.readFileSync(pssFile, "utf8").split("\n").filter(Boolean).map(l => JSON.parse(l));
    let treePeak = 0, treePeakAt = null; const peakByType = {}; const typeOf = new Map();
    for (const rec of lines) {
        const tree = rec.tree_bytes?.Pss ?? null;
        if (tree !== null && tree > treePeak) { treePeak = tree; treePeakAt = rec.elapsed_seconds; }
        const byType = {};
        for (const proc of rec.processes ?? []) {
            if (!typeOf.has(proc.pid)) typeOf.set(proc.pid, processType(proc.pid));
            const type = typeOf.get(proc.pid) ?? "exited";
            byType[type] = (byType[type] ?? 0) + proc.bytes.Pss;
        }
        for (const [type, bytes] of Object.entries(byType)) peakByType[type] = Math.max(peakByType[type] ?? 0, bytes);
    }
    const mib = b => Math.round(b / 1048576);
    return { types: Object.fromEntries(typeOf), samples: lines.length, treePeakMiB: mib(treePeak), treePeakAtS: treePeakAt, peakByTypeMiB: Object.fromEntries(Object.entries(peakByType).map(([k, v]) => [k, mib(v)])), file: pssFile, incomplete: lines.filter(r => !r.enumerated_processes_readable).length };
}
// Evaluates an async expression and returns its JSON value (typed arrays become plain arrays).
async function evalJson(expression) {
    const result = await evaluate(`(async () => JSON.stringify(await (${expression}),
        (key, value) => ArrayBuffer.isView(value) ? Array.from(value) : value))()`);
    return JSON.parse(result.value);
}
// T11 "reached" on the cells path (R3-Q3, ADR 0037/0040): cutApply applied, nothing coarsened or
// staged, a resting cut digest. The CPU path keeps the R0 definition (lodTargetReached).
const cellsReachedNow = lod => lod.cutApply === "applied" && lod.cutDigestAtRest != null
    && lod.poolNodesWithoutLevel === 0 && lod.poolCoarsened === 0 && lod.cpuCoarsened === 0
    && lod.gpuCoarsened === 0 && lod.poolStages === 0;
const isCellsLod = lod => lod?.cellPath === "enabled";
const pageEpoch = () => evalJson("performance.timeOrigin + performance.now()");

async function orbit(label, durationMs) {
    const startEpoch = await pageEpoch();
    const steps = Math.round(durationMs / ORBIT_STEP_MS);
    // Fixed-rate back-and-forth sweep: one pointer move per step for the whole duration.
    const moves = Array.from({ length: steps }, (_, i) => ({
        x: Math.round(640 + 300 * Math.sin(i * 0.05)),
        y: Math.round(360 + 80 * Math.sin(i * 0.031)),
    }));
    await evalJson("window.__renderBench.apiCall('resetGpuFrameStats')").catch(() => null);
    const gesture = backend.gesture(label, moves, durationMs);
    const captures = [];
    // Pixel-mode runs also sample the cells stats (proxy for the stage, which the cells path does not publish).
    const proxy = [];
    let proxyStop = false;
    const proxyLoop = capturePixels ? (async () => {
        while (!proxyStop) {
            const lod = (await evalJson("window.__renderBench.apiCall('getStats')").catch(() => null))?.lod;
            if (lod) proxy.push({ t: Date.now(), poolStages: lod.poolStages ?? null, poolCoarsened: lod.poolCoarsened ?? null, cpuCoarsened: lod.cpuCoarsened ?? null, gpuCoarsened: lod.gpuCoarsened ?? null, uncoveredNodes: lod.uncoveredNodes ?? null, stagePx: lod.lodStagePixels ?? null, cutApply: lod.cutApply ?? null });
            await sleep(250);
        }
    })() : null;
    if (capturePixels) {
        for (const fraction of [0.25, 0.5, 0.75]) {
            await sleep(Math.max(0, durationMs * fraction - (Date.now() - Date.parse(new Date(startEpoch).toISOString()))));
            captures.push(await captureEmptyPixels().catch(error => ({ error: String(error) })));
        }
    }
    await gesture;
    proxyStop = true;
    await proxyLoop;
    const gpuFrameMs = (await evalJson("window.__renderBench.apiCall('getStats')").catch(() => null))?.gpu?.gpuFrameMs ?? null;
    return { label, gpuFrameMs, startEpoch, endEpoch: await pageEpoch(), requestedMs: durationMs, stepMs: ORBIT_STEP_MS, emptyPixelCaptures: captures, statsSamples: proxy };
}

// Returns the camera to `pose` and waits until the scene stops changing: no pending blocks, the
// target stage reached and the same digest on two polls. Null digest means it never settled.
// Orbit input is only delivered to the worker with the next `frame` message, and a slow worker
// (long synchronous scene builds) can still hold some of it when the gesture ends. Setting the pose
// first would let that backlog move the camera afterwards, so frames are flushed before the pose is
// set and the pose is verified; the digest is only meaningful for the requested pose.
let restTrace = null;
const SAME_POSE_TOLERANCE = 1e-6;
async function sceneDigestAtRest(pose, timeoutMs = 120_000) {
    const until = Date.now() + timeoutMs;
    const frameCount = () => evalJson("window.__renderBench.frameReplies().length");
    const flushFrom = await frameCount();
    while (Date.now() < until && (await frameCount()) < flushFrom + 2) {
        await evaluate("window.__renderBench.wake(); true");
        await sleep(200);
    }
    const trace = { setCamera: await evalJson(`window.__renderBench.apiCall('setCamera', ${JSON.stringify({ position: Array.from(pose.position), target: Array.from(pose.target) })})`), poseRestored: false };
    restTrace = trace;
    let previous = null;
    while (Date.now() < until) {
        await evaluate("window.__renderBench.wake(); true");
        await sleep(500);
        const current = await evalJson("window.__renderBench.apiCall('getCamera')");
        trace.poseRestored = ["position", "target"].every(key => pose[key].every((value, axis) => Math.abs(value - current[key][axis]) < SAME_POSE_TOLERANCE));
        if (!trace.poseRestored) { previous = null; continue; }
        const lod = (await evalJson("window.__renderBench.apiCall('getStats')"))?.lod;
        const cells = isCellsLod(lod);
        const idle = Boolean(lod) && (cells ? cellsReachedNow(lod) : lodTargetReached({ ...lod, coverageComplete: true }));
        const digest = idle ? (cells ? lod.cutDigestAtRest : lod.sceneDigest) ?? null : null;
        if (digest !== null && digest === previous) return digest;
        previous = digest;
    }
    return null;
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
const result = { schemaVersion: 3, startedAt, probe: "probe-orbit-load", targetPixels, orbitMs, maxLoadMs, buildWorker };
try {
    result.browser = browser;
    let navigateUrl = url;
    if (remote) {
        remoteServer = await startServer({ root: path.dirname(datasetPath), rttMs: Number(option("rtt-ms", "50")), mbps: Number(option("mbps", "100")) });
        const target = new URL(url);
        target.searchParams.set("url", `http://127.0.0.1:${remoteServer.port}/${path.basename(datasetPath)}`);
        target.searchParams.set("px", String(targetPixels));
        if (!withAttrs) target.searchParams.set("attrs", "none");
        for (const [flag, param] of [["max-gap", "maxGap"], ["remote-reads", "remoteReads"]]) {
            if (option(flag, null) !== null) target.searchParams.set(param, option(flag, null));
        }
        navigateUrl = target.href;
        result.remote = { rttMs: Number(option("rtt-ms", "50")), mbps: Number(option("mbps", "100")), attrs: withAttrs, url: navigateUrl };
    }
    await backend.navigate(navigateUrl);
    await backend.setViewport(1688, 768);
    const setupPage = () => evaluate(`document.querySelector("canvas").style.width="1282px";
        document.querySelector("canvas").style.height="722px";
        window.dispatchEvent(new Event("resize"));
        document.querySelector("#voxel-pixels").value=${targetPixels};
        document.querySelector("#voxel-pixels").dispatchEvent(new Event("input"));
        window.__renderBench.start(); true`);
    // Remote: loading starts by itself when the worker is ready, so the page is set up before that.
    if (remote) await setupPage();
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
    if (!remote) await setupPage();
    await evalJson("window.__renderBench.apiCall('setGpuFrameTiming', { enabled: true })").catch(() => null);
    if (!remote) await sleep(500);
    result.gttIdleChromeBytes = readGtt();
    const idleGpu = gpuBusySamples.slice(-4).map(s => s.percent);
    const loadStartEpoch = await pageEpoch();
    if (!remote) {
        if (withAttrs && fs.existsSync(attributesPath)) await backend.setAttributesFile(attributesPath);
        await backend.setDatasetFile(datasetPath);
    }
    const loadStartedMs = Date.now();
    // Colour mode: the Color options (sidecar columns) exist only after the dataset event, so wait for them.
    // The worker confirms the mode in sceneInputs().colorMode; a mode that never shows there fails the run.
    const colorState = async () => ({
        select: (await evaluate('document.querySelector("#color-mode").value')).value,
        worker: (await evalJson("window.__renderBench.apiCall('sceneInputs')"))?.colorMode ?? null,
    });
    result.colorMode = { requested: colorRequest, value: null, select: null, worker: null, verified: false };
    if (colorRequest !== "rgb") {
        const wanted = JSON.stringify(colorRequest.startsWith("attribute:") ? `Attribute: ${colorRequest.slice(10)}` : colorRequest);
        const find = `(() => { const o = [...document.querySelector("#color-mode").options].find(o => !o.disabled
            && (o.dataset.attribute ? o.textContent === ${wanted} : o.value === ${wanted})); return o ? o.value : null; })()`;
        let value = null;
        while (value === null && Date.now() - loadStartedMs < 120_000) {
            value = await evalJson(find);
            if (value === null) await sleep(100);
        }
        if (value === null) throw new Error(`colour mode ${colorRequest} is not offered by the demo (columns missing or --attrs not set)`);
        result.colorMode.value = value;
        result.colorMode.appliedAtMs = Date.now() - loadStartedMs;
        await evaluate(`(() => { const s = document.querySelector("#color-mode"); s.value = ${JSON.stringify(value)}; s.dispatchEvent(new Event("change")); return true; })()`);
    } else result.colorMode.value = "rgb";
    // Orbit only once the first LOD message exists, so the gesture overlaps real loading.
    const statusText = async () => (await evaluate('document.querySelector("#renderer-status").textContent')).value;
    while (!(await evalJson("window.__renderBench.latestLodCounters()")) && Date.now() - loadStartedMs < 60_000) {
        if (/failed/.test(await statusText())) throw new Error(`dataset did not open: ${await statusText()}`);
        await sleep(200);
    }
    const phases = [];
    let lastCellsDigest = null;
    // The status line only says "target reached" until the next LOD message rewrites it, which a
    // small dataset (reached before the orbit starts) never gets back to; fall back to the stats.
    const isTarget = async () => {
        const state = await evalJson(`(() => {
            const status = document.querySelector("#renderer-status").textContent;
            const c = window.__renderBench.latestLodCounters();
            return { status, reached: /target reached/.test(status), counters: c };
        })()`);
        const lod = (await evalJson("window.__renderBench.apiCall('getStats')"))?.lod;
        state.pageBudgetExhausted = Boolean(lod?.pageBudgetExhausted);
        // A pending hierarchy page overrides a stale "target reached" status line.
        if (state.reached && lod?.pendingPageAnchors?.length) state.reached = false;
        if (isCellsLod(lod)) {
            // Cells path: T11 definition with a repeated resting digest on two polls.
            const digest = cellsReachedNow(lod) ? lod.cutDigestAtRest : null;
            state.reached = digest !== null && digest === lastCellsDigest;
            lastCellsDigest = digest;
        } else if (!state.reached) state.reached = lodTargetReached(lod);
        state.cells = isCellsLod(lod) ? { ...Object.fromEntries(["cutApply", "poolNodesWithoutLevel", "poolCoarsened", "cpuCoarsened", "gpuCoarsened", "poolStages", "cellsForgottenTotal", "cellCpuBytes", "uncoveredNodes"].map(k => [k, lod[k] ?? null])) } : null;
        return state;
    };
    result.vulkan = vulkanEvidence();
    // The fitted camera both runs start from; the scene at rest is compared at this pose.
    const initialPose = await evalJson("window.__renderBench.apiCall('getCamera')");
    result.sceneInputsAtStart = await evalJson("window.__renderBench.apiCall('sceneInputs')");
    result.firstLodMs = Date.now() - loadStartedMs;
    if (jumpAfterMs !== null) {
        await sleep(Math.max(0, jumpAfterMs - (Date.now() - loadStartedMs)));
        const [position, target] = [initialPose.position, initialPose.target];
        const far = position.map((value, axis) => target[axis] + (value - target[axis]) * 10);
        await evalJson(`window.__renderBench.apiCall('setCamera', ${JSON.stringify({ position: far, target })})`);
        result.jump = { afterMs: Date.now() - loadStartedMs, from: position, to: far, target };
    }
    // --rest-first: no orbit during loading; the time to "reached" at the fitted pose is measured instead.
    if (!restFirst) phases.push(await orbit("during-load", orbitMs));
    result.vulkanDuringRender = vulkanEvidence();
    let state = await isTarget();
    while (!state.reached && Date.now() - loadStartedMs < maxLoadMs && !/failed|paused/.test(state.status)) {
        await sleep(restFirst ? 250 : 500);
        state = await isTarget();
    }
    if (restFirst && state.reached) { result.timeToReachedMs = Date.now() - loadStartedMs; result.reachedEpoch = Date.now(); }
    result.targetReached = state.reached;
    result.pageBudgetExhausted = Boolean(state.pageBudgetExhausted);
    result.finalStatus = state.status;
    result.loadElapsedMs = Date.now() - loadStartedMs;
    result.lastCellsState = state.cells;
    result.openTimings = await evalJson("window.__renderBench.openTimings()");
    if (state.reached) {
        phases.push(await orbit("after-target", orbitMs));
        result.sceneDigestAtRest = await sceneDigestAtRest(initialPose);
        // Everything the scene at rest was built from, to diff the two modes when digests differ.
        result.restTrace = restTrace;
        result.initialPose = initialPose;
        result.sceneInputsAtRest =await evalJson("window.__renderBench.apiCall('sceneInputs')");
    }
    const endEpoch = await pageEpoch();
    result.loadStartEpoch = loadStartEpoch;
    result.endEpoch = endEpoch;
    result.phases = phases.map(phase => ({ ...phase, frames: undefined }));
    for (const phase of result.phases) {
        phase.summary = await evalJson(`window.__renderBench.summarize(${phase.startEpoch}, ${phase.endEpoch})`);
    }
    result.wholeRunFrames = await evalJson(`window.__renderBench.summarize(${loadStartEpoch}, ${endEpoch})`);
    result.workerMetrics = await evalJson("window.__renderBench.workerMetrics()");
    // RQ-02: published LOD stage and uncovered nodes per phase, from the worker's LOD message log.
    const lodLog = result.workerMetrics?.lodLog ?? [];
    for (const phase of result.phases) {
        const entries = lodLog.filter(e => e.time >= phase.startEpoch && e.time <= phase.endEpoch);
        const stages = entries.map(e => e.stagePx);
        const defined = stages.filter(Number.isFinite);
        let rises = 0, maxRise = 0;
        for (let i = 1; i < defined.length; i++) if (defined[i] > defined[i - 1]) { rises++; maxRise = Math.max(maxRise, defined[i] - defined[i - 1]); }
        const uncovered = entries.map(e => e.uncoveredNodes).filter(Number.isFinite);
        phase.lodLog = {
            entries: entries.length, stageDefined: defined.length, stageFirst: defined[0] ?? null, stageLast: defined.at(-1) ?? null,
            stageMax: defined.length ? Math.max(...defined) : null, stageRises: rises, stageMaxRise: maxRise,
            uncoveredMax: uncovered.length ? Math.max(...uncovered) : null, uncoveredLast: uncovered.at(-1) ?? null,
            uncoveredMean: uncovered.length ? uncovered.reduce((a, b) => a + b, 0) / uncovered.length : null,
            oversizedMax: Math.max(0, ...entries.map(e => e.oversizedVoxels).filter(Number.isFinite)),
        };
    }
    {
        // Proxy for "loading done": the first LOD message after which no message reports pending blocks
        // (and, on the CPU path, the stage is at or below the target).
        const loadEpoch = loadStartEpoch;
        let drained = null;
        for (let i = lodLog.length - 1; i >= 0; i--) {
            const e = lodLog[i];
            const idle = e.pendingBlocks === 0 && !(e.stagePx > e.targetPx);
            if (!idle) break;
            drained = e.time;
        }
        result.pendingDrainedMs = drained === null ? null : Math.round(drained - loadEpoch);
    }
    result.lodLogTotal = { entries: lodLog.length, dropped: result.workerMetrics?.lodLogDropped ?? null };
    // Frame-reply interval over the load before the first orbit and over the whole run (RQ-01 "during loading").
    delete result.workerMetrics.lodLog;
    // Build-worker fields (null when the option is off); busy time and async read/decode ms of the
    // build worker are under workerMetrics.buildWorker and workerMetrics.asyncBlocks.
    const metrics = result.workerMetrics ?? {};
    result.buildWorkerMetrics = metrics.buildWorkerEnabled ? {
        sceneAgeMs: metrics.sceneAgeMs, epochLag: metrics.epochLag, viewsSent: metrics.viewsSent,
        viewsCoalesced: metrics.viewsCoalesced, scenesApplied: metrics.scenesApplied,
        scenesDropped: metrics.scenesDropped, sceneApplyMs: metrics.sceneApplyMs,
        buildBusyByMessage: metrics.buildWorker?.busyByMessage ?? null,
        build: metrics.buildWorker?.build ?? null,
        asyncBlocks: metrics.asyncBlocks, wasmLinearMemoryBytes: metrics.wasmLinearMemoryBytes,
    } : null;
    result.remoteClient = { viewerWorker: metrics.remote ?? null, buildWorker: metrics.buildWorker?.remote ?? null };
    result.counters = await evalJson("window.__renderBench.latestLodCounters()");
    const finalStats = await evalJson("window.__renderBench.apiCall('getStats')");
    result.lodFinal = finalStats?.lod ? {
        refinementWork: finalStats.lod.refinementWork ?? null,
        pendingTargetEvictions: finalStats.lod.pendingTargetEvictions ?? null,
        cacheCycleDetected: finalStats.lod.cacheCycleDetected ?? null,
    } : null;
    result.metadata = await evalJson("window.__renderBench.apiCall('getMetadata')");
    result.metadata = { name: result.metadata?.name, bytes: result.metadata?.bytes, nodeCount: result.metadata?.nodeCount, sourcePointCount: result.metadata?.sourcePointCount };
    {
        const end = await colorState();
        Object.assign(result.colorMode, end, { verified: end.select === result.colorMode.value && end.worker === result.colorMode.value });
        if (!result.colorMode.verified) throw new Error(`colour mode ${colorRequest} not active: ${JSON.stringify(end)}`);
    }
    fs.writeFileSync(`${output}.png`, await backend.screenshot());
    if (pixelsMode) result.finalEmptyPixels = await captureEmptyPixels();
    result.idleGpuBusyPercent = idleGpu;
} catch (error) {
    result.error = String(error?.stack ?? error);
} finally {
    clearInterval(gpuTimer);
    clearInterval(gttTimer);
    result.gtt = { idleChromeDeltaMiB: gttBaseline !== null && result.gttIdleChromeBytes != null ? Math.round((result.gttIdleChromeBytes - gttBaseline) / 1048576) : null, baselineBytes: gttBaseline, peakBytes: gttPeak, peakDeltaMiB: gttBaseline !== null && gttPeak !== null ? Math.round((gttPeak - gttBaseline) / 1048576) : null,
        endBytes: readGtt(), endDeltaMiB: gttBaseline !== null && readGtt() !== null ? Math.round((readGtt() - gttBaseline) / 1048576) : null };
    try { result.pss = summarizePss(); } catch (error) { result.pss = { error: String(error) }; }
    pssSampler?.kill("SIGTERM");
    if (remoteServer) {
        const gets = remoteServer.log.filter(e => e.method === "GET");
        const first = remoteServer.log.find(e => e.method === "GET");
        // The server clock is performance.now() of this process: epoch = timeOrigin + at.
        const firstGetEpoch = first ? performance.timeOrigin + first.at : null;
        result.remote = { ...result.remote, firstGetEpoch, timeToReachedFromFirstGetMs: firstGetEpoch && result.reachedEpoch ? Math.round(result.reachedEpoch - firstGetEpoch) : null, getRequests: gets.length, bodyBytes: gets.reduce((sum, e) => sum + e.bytes, 0), multiRange: gets.filter(e => e.multiRange).length, statuses: [...new Set(gets.map(e => e.status))] };
    }
    await remoteServer?.close?.();
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
    // gpu_busy stays informative: the CPU-bound voxel rebuild keeps the iGPU mostly idle even on a
    // hardware adapter, so a low value does not mean a software one.
    if (result.gpuBusy.max !== null && !(result.gpuBusy.max > 10)) result.gpuBusyNote = "gpu_busy_percent never above 10";
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
