// R3 Task 11: browser smoke of the GPU cells path (cells + build worker, R3-Q24) on one dataset.
// Run from the repository root while a static server serves the worktree root (with pkg/), over http
// (see docs/records/benchmarks/data/r3-t10-workers-07-11-2026/command.sh for the build and server):
//   node examples/web-host/probe-cells-smoke.mjs DATASET.oct OUT.json [--poses=fitted,orbit,jump,leaf]
//        [--url=URL] [--px=4] [--timeout-s=120] [--rest-s=5] [--chrome-bin=PATH]
// Poses (in order): `fitted`; `orbit` (fitted turned 35 degrees about the vertical through the target);
// `jump` (dolly to 10% of the distance to the target: a cut that may fail with DescendantTooFine and
// keeps the previous one, R3-Q10); `leaf` (Majadas only: the T16 case majadas-inside-leaf-py-4px, the
// eye inside a leaf looking along +y). Default: fitted,orbit,jump plus leaf when the file name has "majadas".
// Per pose: "reached" is the plan's definition (R3-Q3): cutApply "applied", poolNodesWithoutLevel 0,
// poolCoarsened 0, cpuCoarsened 0 (ADR 0037), poolStages 0 and a repeated non-null cutDigestAtRest.
// "settled" is weaker: camera at the pose, cutApply "applied", a repeated non-null cutDigestAtRest and
// no node forgotten during the rest window, so a pose whose far nodes the CPU budget keeps coarse
// (cpuCoarsened > 0) can settle without being reached. Exit code 1 only if a pose does neither.
// Also records time to reached,
// RetainedPrevious streaks (samples every ~100 ms), forgotten nodes over a rest window (the camera
// is redrawn every sample, so a forget/rebuild loop would show), frame interval/render-call percentiles
// during arrival and the rest window, GTT delta over the idle baseline, console and page errors.
// Not a unit test and not part of scripts/check.sh.
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";

const args = process.argv.slice(2);
const positional = args.filter(arg => !arg.startsWith("--"));
const option = (name, fallback) => args.find(arg => arg.startsWith(`--${name}=`))?.split("=")[1] ?? fallback;
const [input, output] = positional;
if (!input || !output) {
    throw new Error("Usage: node examples/web-host/probe-cells-smoke.mjs DATASET.oct OUT.json [--poses=fitted,orbit,jump,leaf] [--url=URL] [--px=4] [--timeout-s=120] [--rest-s=5]");
}
const baseUrl = option("url", "http://127.0.0.1:8767/examples/web-host/index.html");
const targetPixels = Number(option("px", "4"));
const timeoutMs = Number(option("timeout-s", "120")) * 1000;
const restMs = Number(option("rest-s", "5")) * 1000;
const poseNames = option("poses", /majadas/i.test(path.basename(input)) ? "fitted,orbit,jump,leaf" : "fitted,orbit,jump").split(",");
const CHROME_BIN = option("chrome-bin", process.env.CHROME_BIN ?? "/usr/bin/google-chrome");
const PLAYWRIGHT_CORE = process.env.PLAYWRIGHT_CORE ?? "/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/";
const GTT_FILE = process.env.GTT_FILE ?? "/sys/class/drm/card1/device/mem_info_gtt_used";
const SAMPLE_MS = 100;
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
// Null (and gttAvailable false in the report) when the sysfs text is not a finite integer.
const readGtt = () => {
    try {
        const text = fs.readFileSync(GTT_FILE, "utf8").trim();
        return /^\d+$/.test(text) ? Number(text) : null;
    } catch { return null; }
};
// A worker that stops answering must not hang a poll loop past its pose timeout.
const CALL_TIMEOUT_MS = 10_000;
const timed = (promise, label) => {
    let timer;
    const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`${label} did not answer within ${CALL_TIMEOUT_MS} ms`)), CALL_TIMEOUT_MS); });
    return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
};

// The T16 case majadas-inside-leaf-py-4px (crates/viewer_render_wgpu/examples/coverage_spike.rs): the
// case `eye` is in dataset units relative to the coordinate origin; the camera API takes source
// coordinates (the fitted pose is centred on the metadata bounds), so the eye is origin + case eye
// and the target is one sixteenth of the root side ahead along +y (visual_scale only scales the
// internal camera). The root side is approximated by the largest extent of the metadata bounds.
const LEAF_CASE = { eye: [621, 619, 155], direction: [0, 1, 0] };

function orbitStepPose(pose) {
    const angle = 35 * Math.PI / 180;
    const [px, py, pz] = pose.position;
    const [tx, ty, tz] = pose.target;
    const dx = px - tx, dy = py - ty;
    return {
        position: [tx + dx * Math.cos(angle) - dy * Math.sin(angle), ty + dx * Math.sin(angle) + dy * Math.cos(angle), pz],
        target: [tx, ty, tz],
    };
}
const jumpPose = pose => ({
    position: pose.target.map((t, axis) => t + 0.1 * (pose.position[axis] - t)),
    target: pose.target,
});
function leafPose(metadata) {
    const side = Math.max(...metadata.boundsMax.map((v, axis) => v - metadata.boundsMin[axis]));
    const eye = LEAF_CASE.eye.map((v, axis) => metadata.coordinateOrigin[axis] + v);
    return { position: eye, target: eye.map((v, axis) => v + side / 16 * LEAF_CASE.direction[axis]) };
}

const samePose = (pose, current) => ["position", "target"].every(key => pose[key].every((v, axis) => Math.abs(v - current[key][axis]) < 1e-6 * Math.max(1, Math.abs(v))));
const settledNow = (lod, poseApplied = true) => poseApplied
    && lod.cutApply === "applied" && lod.cutDigestAtRest != null;
const reachedNow = (lod, poseApplied = true) => settledNow(lod, poseApplied)
    && lod.poolNodesWithoutLevel === 0 && lod.poolCoarsened === 0 && lod.cpuCoarsened === 0 && lod.poolStages === 0;
const statsOf = lod => Object.fromEntries(["cutApply", "cutDigestAtRest", "cellsBusy", "viewEpoch", "cutEpochSent", "cutEpochApplied",
    "poolStages", "poolNodesWithoutLevel", "poolCoarsened", "cpuCoarsened", "poolOverflow", "poolDescendantTooFine", "poolResidentPages",
    "cellsForgottenTotal", "cellCpuBytes", "wasmLinearMemoryBytes", "cellPath"].map(key => [key, lod?.[key] ?? null]));

// The whole LOD object for diagnosis, with arrays reduced to their length.
const rawOf = lod => Object.fromEntries(Object.entries(lod ?? {}).map(([key, value]) =>
    [key, Array.isArray(value) || ArrayBuffer.isView(value) ? { length: value.length }
        : value !== null && typeof value === "object" ? "[object]" : value]));

const { chromium } = createRequire(PLAYWRIGHT_CORE)("playwright-core");
const gttBaseline = readGtt();
const consoleMessages = [];
const report = { schemaVersion: 1, probe: "probe-cells-smoke", startedAt: new Date().toISOString(), input: path.resolve(input),
    targetPixels, timeoutMs, restMs, gttBaselineBytes: gttBaseline, gttAvailable: gttBaseline !== null, consoleMessages, poses: [] };
const instance = await chromium.launch({
    executablePath: CHROME_BIN, headless: true,
    args: ["--headless=new", "--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=vulkan"],
});
let gttPeak = gttBaseline;
const gttNow = () => { const v = readGtt(); if (v !== null && (gttPeak === null || v > gttPeak)) gttPeak = v; return v; };
const delta = v => (v === null || gttBaseline === null ? null : v - gttBaseline);
try {
    const url = new URL(baseUrl);
    url.searchParams.set("cells", "1");
    url.searchParams.set("buildWorker", "1");
    report.url = url.href;
    const page = await (await instance.newContext({ viewport: { width: 1688, height: 768 }, deviceScaleFactor: 1 })).newPage();
    page.on("console", m => { if (["error", "warning"].includes(m.type())) consoleMessages.push({ type: m.type(), text: m.text() }); });
    page.on("pageerror", error => consoleMessages.push({ type: "pageerror", text: String(error) }));
    report.browserVersion = instance.version();
    const evaluate = expression => timed(page.evaluate(expression), "page.evaluate");
    const evalJson = async expression => JSON.parse(await evaluate(`(async () => JSON.stringify(await (${expression}),
        (key, value) => ArrayBuffer.isView(value) ? Array.from(value) : value))()`));
    const api = (op, extra) => evalJson(`window.__renderBench.apiCall(${JSON.stringify(op)}${extra ? `, ${JSON.stringify(extra)}` : ""})`);
    const lodStats = async () => (await api("getStats"))?.lod ?? {};
    await page.goto(url.href, { waitUntil: "load" });
    const readyUntil = Date.now() + 120_000;
    while (await evaluate('document.querySelector("#dataset-file").disabled')) {
        if (Date.now() > readyUntil) throw new Error("Viewer startup timeout");
        await sleep(300);
    }
    await evaluate(`document.querySelector("canvas").style.width="1282px";
        document.querySelector("canvas").style.height="722px";
        window.dispatchEvent(new Event("resize"));
        document.querySelector("#voxel-pixels").value=${targetPixels};
        document.querySelector("#voxel-pixels").dispatchEvent(new Event("input"));
        window.__renderBench.start(); true`);
    await sleep(500);
    await page.setInputFiles("#dataset-file", path.resolve(input));
    const loadStarted = Date.now();
    while (!(await evalJson("window.__renderBench.latestLodCounters()"))) {
        if (Date.now() - loadStarted > 60_000) throw new Error("dataset did not open within 60 s");
        gttNow();
        const status = await evaluate('document.querySelector("#renderer-status").textContent');
        if (/failed/.test(status)) throw new Error(`dataset did not open: ${status}`);
        await sleep(200);
    }
    const fitted = await api("getCamera");
    const metadata = await api("getMetadata");
    report.fittedPose = fitted;
    report.metadata = { name: metadata?.name, bytes: metadata?.bytes, nodeCount: metadata?.nodeCount, boundsMin: metadata?.boundsMin, boundsMax: metadata?.boundsMax, coordinateOrigin: metadata?.coordinateOrigin };

    for (const name of poseNames) {
        const pose = { fitted, orbit: orbitStepPose(fitted), jump: jumpPose(fitted), leaf: metadata && leafPose(metadata) }[name];
        if (!pose) throw new Error(`unknown pose ${name}`);
        // timeToReachedMs counts from the moment the pose is requested; for `fitted` that is the
        // dataset open (timeOrigin "open"), since the fit happens on load.
        const entry = { label: name, pose, timeOrigin: name === "fitted" ? "open" : "request" };
        const started = Date.now();
        if (name !== "fitted") await api("setCamera", { position: pose.position, target: pose.target });
        const poseStarted = name === "fitted" ? loadStarted : started;
        entry.poseApplied = true;
        let previous = null, previousSettled = null, reached = false, settledStreak = false, streak = null, last = null;
        const streaks = [];
        let samples = 0, retainedSamples = 0, lastTrace = 0;
        entry.trace = [];
        while (Date.now() - poseStarted < timeoutMs) {
            await evaluate("window.__renderBench.wake(); true");
            const now = Date.now();
            // The camera must be at the requested pose on the same sample (as the parity probe).
            const applied = samePose(pose, await api("getCamera"));
            entry.poseApplied &&= applied;
            last = await lodStats();
            gttNow();
            samples++;
            // A coarse time series of the raw stats, to see what keeps a pose from settling.
            if (now - lastTrace >= 5000) { entry.trace.push({ t: now - poseStarted, ...rawOf(last) }); lastTrace = now; }
            if (last.cutApply === "retained") {
                retainedSamples++;
                if (streak) { streak.samples++; streak.end = now; } else streak = { start: now, end: now, samples: 1 };
            } else if (streak) { streaks.push(streak); streak = null; }
            const ok = reachedNow(last, applied);
            if (ok && previous === last.cutDigestAtRest) { reached = true; break; }
            previous = ok ? last.cutDigestAtRest : null;
            // A pose the CPU budget keeps coarse is final: waiting longer would not reach it.
            const settledOk = settledNow(last, applied);
            if (settledOk && previousSettled === last.cutDigestAtRest && last.cpuCoarsened > 0) { settledStreak = true; break; }
            previousSettled = settledOk ? last.cutDigestAtRest : null;
            await sleep(SAMPLE_MS);
        }
        if (streak) streaks.push(streak);
        const reachedAt = Date.now();
        entry.reached = reached;
        entry.settled = false;
        entry.timeToReachedMs = reached ? reachedAt - poseStarted : null;
        entry.arrivalFrames = await evalJson(`window.__renderBench.summarize(${poseStarted}, ${reachedAt})`);
        entry.retained = {
            samples: retainedSamples, of: samples, streaks: streaks.length,
            durationsMs: streaks.map(s => s.end - s.start + SAMPLE_MS), totalMs: streaks.reduce((sum, s) => sum + s.end - s.start + SAMPLE_MS, 0),
        };
        entry.statsAtReached = statsOf(last);
        entry.gttAtReachedDeltaBytes = delta(gttNow());
        if (reached || settledStreak) {
            // The rest window keeps waking the render loop on purpose: forgetting only shows when frames run.
            const forgottenBefore = last.cellsForgottenTotal ?? 0;
            const restStarted = Date.now();
            let restLod = last;
            while (Date.now() - restStarted < restMs) {
                await evaluate("window.__renderBench.wake(); true");
                restLod = await lodStats();
                gttNow();
                await sleep(SAMPLE_MS);
            }
            entry.rest = {
                windowMs: Date.now() - restStarted,
                forgottenDelta: (restLod.cellsForgottenTotal ?? 0) - forgottenBefore,
                stillAtRest: (reached ? reachedNow : settledNow)(restLod, samePose(pose, await api("getCamera"))),
                frames: await evalJson(`window.__renderBench.summarize(${restStarted}, ${Date.now()})`),
                gttDeltaBytes: delta(gttNow()),
            };
            entry.statsAtRestEnd = statsOf(restLod);
            entry.settled = entry.rest.stillAtRest && entry.rest.forgottenDelta === 0;
        }
        entry.endedInRetainedStreak = last?.cutApply === "retained";
        if (!reached) { entry.lastStats = statsOf(last); entry.lastRaw = rawOf(last); }
        report.poses.push(entry);
    }
    report.finalLod = statsOf(await lodStats());
} catch (error) {
    report.error = String(error?.stack ?? error);
    console.error(report.error.split("\n")[0]);
} finally {
    await instance.close().catch(() => {});
}
report.gttPeakDeltaBytes = delta(gttPeak);
if (report.error || report.poses.length < poseNames.length || report.poses.some(p => !p.reached && !p.settled)) process.exitCode = 1;
fs.mkdirSync(path.dirname(path.resolve(output)), { recursive: true });
fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n");

const mib = bytes => (bytes === null || bytes === undefined ? "n/a" : `${(bytes / 1048576).toFixed(0)} MiB`);
console.log(`dataset ${path.basename(input)} px ${targetPixels}${report.error ? ` ERROR ${report.error.split("\n")[0]}` : ""}`);
for (const p of report.poses) {
    console.log(`${p.label}: reached ${p.reached}, settled ${p.settled}${p.timeToReachedMs !== null ? ` in ${p.timeToReachedMs} ms` : ""}, retained ${p.retained.streaks} streaks `
        + `(${p.retained.totalMs} ms, longest ${Math.max(0, ...p.retained.durationsMs)} ms), forgotten at rest ${p.rest?.forgottenDelta ?? "n/a"}, `
        + `frame p95 arrival ${p.arrivalFrames?.intervalMs?.p95 ?? "n/a"} / rest ${p.rest?.frames?.intervalMs?.p95 ?? "n/a"} ms, `
        + `render-call p95 ${p.arrivalFrames?.renderCallMs?.p95 ?? "n/a"} / ${p.rest?.frames?.renderCallMs?.p95 ?? "n/a"} ms, GTT +${mib(p.rest?.gttDeltaBytes ?? p.gttAtReachedDeltaBytes)}`);
}
console.log(`GTT baseline ${mib(gttBaseline)}, peak +${mib(report.gttPeakDeltaBytes)}; console messages ${consoleMessages.length}`);
