// R3 Task 10c2: cells path in a real browser. R3-Q24: cells run only with the build worker, so the
// no-build-worker arm is a gate check (CPU path, `cellPath: "needsBuildWorker"`), and parity compares
// cells + build worker against itself across two loads.
// Run from the repository root while a static server serves the worktree root (with pkg/), over http:
// the nested build worker disables itself on file://.
//   node examples/web-host/probes/probe-cells-parity.mjs INPUT.oct OUTPUT.json [--url=URL] [--px=4]
//        [--rest-timeout-s=180] [--chrome-bin=PATH]
// The report's `cellLod` is the constant "cell" (the only level selection); it keeps old and new reports comparable.
// Runs four arms on the same dataset and two fixed poses (the fitted pose, then a deterministic
// orbit step from it): cells + build worker, cells without it (gate: CPU path), the CPU path
// (cells off) as the frame-time reference, and cells + build worker again (parity). Cells arms wait for a non-null `cutDigestAtRest` that repeats over two
// polls (`cellsBusy:false` alone is not a settled signal); the CPU arm waits for `sceneDigest`.
// Records digest, time to rest, frame-reply percentiles (interval and render-call, p50/p95) while nodes arrive, cell counters and the
// wasm linear memory of both workers. Not a unit test and not part of scripts/check.sh.
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";

const args = process.argv.slice(2);
const positional = args.filter(arg => !arg.startsWith("--"));
const option = (name, fallback) => args.find(arg => arg.startsWith(`--${name}=`))?.split("=")[1] ?? fallback;
const [input, output] = positional;
if (!input || !output) {
    throw new Error("Usage: node examples/web-host/probes/probe-cells-parity.mjs INPUT.oct OUTPUT.json [--url=URL] [--px=4] [--rest-timeout-s=180]");
}
const baseUrl = option("url", "http://127.0.0.1:8767/examples/web-host/index.html");
const targetPixels = Number(option("px", "4"));
const restTimeoutMs = Number(option("rest-timeout-s", "180")) * 1000;
const CHROME_BIN = option("chrome-bin", process.env.CHROME_BIN ?? "/usr/bin/google-chrome");
const PLAYWRIGHT_CORE = process.env.PLAYWRIGHT_CORE ?? "/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/";
const POSE_TOLERANCE = 1e-6;
const POLL_MS = 1000;
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

const ARMS = [
    { name: "cells+buildWorker", cells: true, buildWorker: true },
    { name: "cells+noBuildWorker", cells: true, buildWorker: false },
    { name: "cpu+buildWorker", cells: false, buildWorker: true },
    { name: "cells+buildWorker#2", cells: true, buildWorker: true },
];
// The arm drawn by cells: the digest to wait for is `cutDigestAtRest`, not `sceneDigest`.
const drawsCells = arm => arm.cells && arm.buildWorker;

// A deterministic second pose: the fitted one turned 35 degrees about the vertical axis through the target.
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

async function runArm(chromium, arm) {
    const url = new URL(baseUrl);
    url.searchParams.set("buildWorker", arm.buildWorker ? "1" : "0");
    // Cells are the default path (R3-Q35): the CPU arm must opt out explicitly.
    url.searchParams.set("cells", arm.cells ? "1" : "0");
    const consoleMessages = [];
    const instance = await chromium.launch({
        executablePath: CHROME_BIN,
        headless: true,
        args: ["--headless=new", "--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=vulkan"],
    });
    const result = { arm: arm.name, url: url.href, poses: [], consoleMessages };
    let page = null;
    try {
        page = await (await instance.newContext({ viewport: { width: 1688, height: 768 }, deviceScaleFactor: 1 })).newPage();
        page.on("console", m => { if (["error", "warning"].includes(m.type())) consoleMessages.push({ type: m.type(), text: m.text() }); });
        page.on("pageerror", error => consoleMessages.push({ type: "pageerror", text: String(error) }));
        result.browserVersion = instance.version();
        const evaluate = expression => page.evaluate(expression);
        const evalJson = async expression => JSON.parse(await evaluate(`(async () => JSON.stringify(await (${expression}),
            (key, value) => ArrayBuffer.isView(value) ? Array.from(value) : value))()`));
        const api = (op, extra) => evalJson(`window.__renderBench.apiCall(${JSON.stringify(op)}${extra ? `, ${JSON.stringify(extra)}` : ""})`);
        await page.goto(url.href, { waitUntil: "load" });
        const readyUntil = Date.now() + 120_000;
        while (await evaluate('document.querySelector("#dataset-file").disabled')) {
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
        const loadStartEpoch = await evalJson("performance.timeOrigin + performance.now()");
        await page.setInputFiles("#dataset-file", path.resolve(input));
        const loadStarted = Date.now();
        while (!(await evalJson("window.__renderBench.latestLodCounters()")) && Date.now() - loadStarted < 60_000) {
            const status = await evaluate('document.querySelector("#renderer-status").textContent');
            if (/failed/.test(status)) throw new Error(`dataset did not open: ${status}`);
            await sleep(200);
        }
        const fitted = await api("getCamera");
        result.fittedPose = fitted;
        result.metadata = await api("getMetadata").then(m => ({ name: m?.name, bytes: m?.bytes, nodeCount: m?.nodeCount }));

        const poses = [{ label: "fitted", pose: fitted }, { label: "orbit-step", pose: orbitStepPose(fitted) }];
        for (const [index, { label, pose }] of poses.entries()) {
            const poseStartEpoch = index === 0 ? loadStartEpoch : await evalJson("performance.timeOrigin + performance.now()");
            const poseStarted = index === 0 ? loadStarted : Date.now();
            if (index > 0) await api("setCamera", { position: pose.position, target: pose.target });
            let previous = null;
            let digest = null;
            let samePose = false;
            const trace = [];
            while (Date.now() - poseStarted < restTimeoutMs) {
                await evaluate("window.__renderBench.wake(); true");
                await sleep(POLL_MS);
                const current = await api("getCamera");
                samePose = ["position", "target"].every(key => pose[key].every((v, axis) => Math.abs(v - current[key][axis]) < POSE_TOLERANCE));
                const lod = (await api("getStats"))?.lod ?? null;
                const candidate = samePose && lod ? (drawsCells(arm) ? lod.cutDigestAtRest : lod.sceneDigest) ?? null : null;
                trace.push({ t: Date.now() - poseStarted, samePose, cellsBusy: lod?.cellsBusy ?? null, digest: candidate });
                if (trace.length > 400) trace.splice(0, 200);
                if (candidate !== null && candidate === previous) { digest = candidate; break; }
                previous = candidate;
            }
            const restMs = Date.now() - poseStarted;
            const endEpoch = await evalJson("performance.timeOrigin + performance.now()");
            const lod = (await api("getStats"))?.lod ?? {};
            const metrics = await evalJson("window.__renderBench.workerMetrics()");
            result.poses.push({
                label, pose, digest, settled: digest !== null, timeToRestMs: digest !== null ? restMs : null,
                frames: await evalJson(`window.__renderBench.summarize(${poseStartEpoch}, ${endEpoch})`),
                stats: {
                    cellsBusy: lod.cellsBusy ?? null, cellsStuck: lod.cellsStuck ?? null,
                    viewEpoch: lod.viewEpoch ?? null, cutEpochSent: lod.cutEpochSent ?? null, cutEpochApplied: lod.cutEpochApplied ?? null,
                    cellPath: lod.cellPath ?? null, cutDigestAtRest: lod.cutDigestAtRest ?? null, sceneDigest: lod.sceneDigest ?? null,
                    cellStats: metrics?.cellStats ?? null, cellCounters: metrics?.cellCounters ?? null,
                },
                wasmLinearMemoryBytes: metrics?.wasmLinearMemoryBytes ?? lod.wasmLinearMemoryBytes ?? null,
                traceTail: trace.slice(-6),
            });
            if (digest === null) break;
        }
        result.finalLod = await api("getStats").then(s => s?.lod ?? null);
    } catch (error) {
        result.error = String(error?.stack ?? error);
        result.statusText = await page?.evaluate('document.querySelector("#renderer-status")?.textContent').catch(() => null);
    } finally {
        await instance.close().catch(() => {});
    }
    return result;
}

const { chromium } = createRequire(PLAYWRIGHT_CORE)("playwright-core");
const report = { schemaVersion: 1, probe: "probe-cells-parity", startedAt: new Date().toISOString(), input: path.resolve(input), cellLod: "cell", targetPixels, restTimeoutMs, arms: [] };
// --arms=0,2 runs a subset (indices into ARMS); the gate check needs arm 1, parity arms 0 and 3.
const selected = option("arms", "0,1,2,3").split(",").map(Number);
for (const arm of ARMS.filter((_, index) => selected.includes(index))) {
    const armResult = await runArm(chromium, arm);
    report.arms.push(armResult);
    fs.mkdirSync(path.dirname(path.resolve(output)), { recursive: true });
    fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n");
    console.log(JSON.stringify({
        arm: arm.name, error: armResult.error ?? null,
        poses: armResult.poses.map(p => ({ label: p.label, digest: p.digest, restMs: p.timeToRestMs, interval: p.frames?.intervalMs, renderCall: p.frames?.renderCallMs, wasm: p.wasmLinearMemoryBytes })),
        console: armResult.consoleMessages.length,
    }));
}
const armByName = name => report.arms.find(arm => arm.arm === name);
const gated = armByName("cells+noBuildWorker");
if (gated) {
    // R3-Q24: cells requested without a build worker must run the CPU path and say why.
    report.gate = gated.poses.map(p => ({
        pose: p.label,
        cellPath: p.stats.cellPath,
        sceneDigest: p.stats.sceneDigest,
        ok: p.stats.cellPath === "needsBuildWorker" && p.stats.sceneDigest != null && p.digest !== null,
    }));
}
const first = armByName("cells+buildWorker");
const second = armByName("cells+buildWorker#2");
if (first && second) {
    report.parity = first.poses.map((p, i) => ({ pose: p.label, equal: p.digest !== null && p.digest === second.poses[i]?.digest }));
}
fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n");
console.log(JSON.stringify({ gate: report.gate, parity: report.parity }));
