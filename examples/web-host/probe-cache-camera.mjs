// Run from the repository root with a fresh Firefox --remote-debugging-port 9222.
// Diagnostic snapshots are incomplete benchmark windows, not valid FPS samples.
import fs from "node:fs";
import path from "node:path";

const [input, output, url = "http://127.0.0.1:8767/examples/web-host/index.html", targetArg = "4"]
    = process.argv.slice(2);
const targetPixels = Number(targetArg);
if (!input || !output || !Number.isFinite(targetPixels)
    || targetPixels < 0.5 || targetPixels > 16 || !Number.isInteger(targetPixels * 2)) {
    throw new Error("Usage: node examples/web-host/probe-cache-camera.mjs INPUT.oct OUTPUT_DIR [URL] [TARGET_PIXELS=4]");
}
fs.mkdirSync(output, { recursive: true });
const save = (name, value) => fs.writeFileSync(path.join(output, name), JSON.stringify(value, null, 2) + "\n");
const socket = new WebSocket("ws://127.0.0.1:9222/session");
const pending = new Map();
const errors = [];
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
function send(method, params) {
    return new Promise((resolve, reject) => {
        const id = ++nextId;
        const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timeout`)); }, 30_000);
        pending.set(id, { resolve, reject, timer });
        socket.send(JSON.stringify({ id, method, params }));
    });
}
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
let context;
async function evaluate(expression) {
    const result = await send("script.evaluate", { expression, target: { context }, awaitPromise: true });
    if (result.type !== "success") throw new Error(JSON.stringify(result));
    return result.result;
}
async function snapshot() {
    // Clear the previous promise, click, then await this exact blob before reading DOM stats.
    const startedWallTimeMs = Date.now();
    const result = await evaluate(`(async () => {
        window.__probeExport = null;
        document.querySelector("#download-benchmark").dispatchEvent(new Event("click"));
        if (!window.__probeExport) throw new Error("Fresh export unavailable");
        const exported = window.__probeExport;
        const status = document.querySelector("#renderer-status").textContent;
        const stats = document.querySelector("#stats").textContent;
        return JSON.stringify({data: JSON.parse(await exported), status, stats});
    })()`);
    const sample = JSON.parse(result.value);
    sample.captureWallTimeMs = [startedWallTimeMs, Date.now()];
    // Progress messages may update latestLodStats before the next rendered-frame panel.
    sample.domMatchesExport = sample.data.scene !== null
        && [["LOD stage", sample.data.scene.lodStagePixels],
            ["pending blocks", sample.data.scene.pendingBlocks]].every(([label, value]) => {
            const displayed = sample.stats.match(new RegExp(label + ": ([0-9.]+)"));
            return displayed && Number(displayed[1]) === Number(value);
        });
    sample.incompleteBenchmarkWindow = true;
    return sample;
}
function progressSignature(sample) {
    const { scene, load } = sample.data;
    return JSON.stringify([sample.status, scene?.pendingBlocks, scene?.lodStagePixels,
        scene?.renderedVoxels, load.refinementWork?.loadedBlocks,
        load.refinementWork?.sceneBuilds, load.refinementWork?.reusedSceneUpdates,
        scene?.cacheActivity]);
}
async function observe(phase, previous = null) {
    const started = Date.now();
    let lastProgress = started;
    let lastSignature = previous && progressSignature(previous);
    let workerAdvanced = !previous;
    for (let index = 0; Date.now() - started < 600_000; index++) {
        await sleep(10_000);
        const sample = await snapshot();
        sample.elapsedPhaseMs = Date.now() - started;
        const { scene, load, refinement } = sample.data;
        const signature = progressSignature(sample);
        if (signature !== lastSignature) { lastSignature = signature; lastProgress = Date.now(); workerAdvanced = true; }
        sample.workerAdvanced = workerAdvanced;
        sample.outcome = sample.status.includes("openFile failed") || sample.status.includes("LOD refinement failed") ? "failed"
            : workerAdvanced && sample.status.includes("refinement paused") ? "paused"
            : workerAdvanced && scene?.pendingBlocks === 0 && scene.coverageComplete && scene.lodStagePixels <= targetPixels && refinement?.targetLodMs != null ? "target_complete"
                : Date.now() - lastProgress >= 120_000 ? "no_progress_120s" : "progressing";
        save(`${phase}-${index}.json`, sample);
        console.log(JSON.stringify({ phase, elapsedMs: sample.elapsedPhaseMs, pending: scene?.pendingBlocks ?? null, loads: load.refinementWork?.loadedBlocks ?? null, outcome: sample.outcome }));
        if (sample.outcome !== "progressing") return sample;
    }
    const sample = await snapshot();
    sample.outcome = "observation_cap_600s";
    return sample;
}
try {
    save("session.json", await send("session.new", { capabilities: {} }));
    context = (await send("browsingContext.getTree", {})).contexts[0].context;
    await send("session.subscribe", { events: ["log.entryAdded"] });
    await send("browsingContext.navigate", { context, url, wait: "complete" });
    await send("browsingContext.setViewport", { context, viewport: { width: 1688, height: 768 }, devicePixelRatio: 1 });
    const readyUntil = Date.now() + 120_000;
    while ((await evaluate('document.querySelector("#dataset-file").disabled')).value) {
        if (Date.now() > readyUntil) throw new Error("Viewer startup timeout");
        await sleep(300);
    }
    await evaluate(`document.querySelector("canvas").style.width="1282px";
        document.querySelector("canvas").style.height="722px";
        window.dispatchEvent(new Event("resize"));
        document.querySelector("#voxel-pixels").value=${targetPixels};
        document.querySelector("#voxel-pixels").dispatchEvent(new Event("input"));
        const original = URL.createObjectURL.bind(URL);
        URL.createObjectURL = blob => { if (blob.type === "application/json") window.__probeExport = blob.text(); return original(blob); };
        HTMLAnchorElement.prototype.click = function() {}; true`);
    await sleep(500);
    const element = await evaluate('document.querySelector("#dataset-file")');
    await send("input.setFiles", { context, element: { sharedId: element.sharedId }, files: [path.resolve(input)] });
    const initial = await observe("initial");
    save("initial.json", initial);
    if (initial.outcome !== "target_complete") throw new Error(`Initial load: ${initial.outcome}: ${initial.status}`);
    // Keep enough frames flowing for the worker's every-30-frame stats to publish
    // scene-build counters after a camera move; this is not an FPS observation.
    await evaluate('document.querySelector("#start-benchmark").click()');
    await send("input.performActions", { context, actions: [{
        type: "pointer", id: "camera-pointer", parameters: { pointerType: "mouse" }, actions: [
            { type: "pointerMove", x: 900, y: 400 },
            { type: "pointerDown", button: 2 },
            { type: "pointerMove", x: 930, y: 400, duration: 200 },
            { type: "pointerUp", button: 2 },
        ],
    }] });
    const camera = await observe("camera", initial);
    if (camera.outcome !== "target_complete") throw new Error(`Camera update: ${camera.outcome}: ${camera.status}`);
    if (!camera.data.steadyState.cameraChanged) throw new Error("Camera input was not acknowledged");
    save("camera.json", camera);
    const shot = await send("browsingContext.captureScreenshot", { context, origin: "viewport" });
    fs.writeFileSync(path.join(output, "final.png"), Buffer.from(shot.data, "base64"));
    if (errors.length) throw new Error(`${errors.length} browser console errors`);
} finally {
    save("console-errors.json", errors);
    await send("browser.close", {}).catch(() => {});
    socket.close();
}
