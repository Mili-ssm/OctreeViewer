// R3-Q26 / B5: browser probe of the WebGPU device-loss recovery on the cells path. Opens a dataset,
// lets it settle, crashes Chrome's GPU process, and checks that the viewer recovers without a page
// reload: a `deviceLost` event with a non-destroyed reason, the same pose, the same cutDigestAtRest
// after settling, no page errors. Run from the repository root with a static server on the worktree
// root (python3 -m http.server 8767 --bind 127.0.0.1):
//   node examples/web-host/probe-device-loss.mjs DATASET.oct OUT.json [--method=gpucrash|cdp|auto]
//        [--url=URL] [--px=4] [--timeout-s=120] [--chrome-bin=PATH]
// `--method`: `gpucrash` navigates a second tab to chrome://gpucrash, `cdp` calls Browser.crashGpuProcess
// on the browser session; `auto` tries them in that order and records which one produced a loss.
// Not a unit test and not part of scripts/check.sh.
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";

const args = process.argv.slice(2);
const positional = args.filter(arg => !arg.startsWith("--"));
const option = (name, fallback) => args.find(arg => arg.startsWith(`--${name}=`))?.split("=")[1] ?? fallback;
const [input, output] = positional;
if (!input || !output) throw new Error("Usage: node examples/web-host/probe-device-loss.mjs DATASET.oct OUT.json [--method=auto]");
const baseUrl = option("url", "http://127.0.0.1:8767/examples/web-host/index.html");
const targetPixels = Number(option("px", "4"));
const timeoutMs = Number(option("timeout-s", "120")) * 1000;
const methodOption = option("method", "auto");
const CHROME_BIN = option("chrome-bin", process.env.CHROME_BIN ?? "/usr/bin/google-chrome");
const PLAYWRIGHT_CORE = process.env.PLAYWRIGHT_CORE ?? "/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/";
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const CALL_TIMEOUT_MS = 10_000;
const timed = (promise, label) => {
    let timer;
    const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`${label} did not answer within ${CALL_TIMEOUT_MS} ms`)), CALL_TIMEOUT_MS); });
    return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
};
const statsSubset = lod => Object.fromEntries(["cutApply", "cutDigestAtRest", "cellsBusy", "poolResidentPages", "poolNodesWithoutLevel", "cellPath"].map(key => [key, lod?.[key] ?? null]));
const samePose = (a, b) => ["position", "target"].every(key => a[key].every((v, axis) => Math.abs(v - b[key][axis]) < 1e-6 * Math.max(1, Math.abs(v))));

const { chromium } = createRequire(PLAYWRIGHT_CORE)("playwright-core");
const outDir = path.dirname(path.resolve(output));
fs.mkdirSync(outDir, { recursive: true });
const consoleMessages = [];
const report = { probe: "probe-device-loss", startedAt: new Date().toISOString(), input: path.resolve(input), methodsTried: [], consoleMessages };
const instance = await chromium.launch({
    executablePath: CHROME_BIN, headless: true,
    args: ["--headless=new", "--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=vulkan",
        "--enable-dawn-features=allow_unsafe_apis", "--enable-webgpu-developer-features"],
});
try {
    const url = new URL(baseUrl);
    url.searchParams.set("cells", "1");
    url.searchParams.set("buildWorker", "1");
    report.url = url.href;
    const context = await instance.newContext({ viewport: { width: 1280, height: 720 }, deviceScaleFactor: 1 });
    // Records the worker's device messages with their time, and counts page loads to prove no reload.
    await context.addInitScript(() => {
        window.__deviceEvents = [];
        window.__loadMarker = Math.random();
        const Original = window.Worker;
        window.Worker = class extends Original {
            constructor(...parameters) {
                super(...parameters);
                this.addEventListener("message", ({ data }) => {
                    if (data?.type === "deviceLost" || data?.type === "deviceRecovery" || (data?.type === "error" && data.operation === "deviceLost")) {
                        window.__deviceEvents.push({ ...data, at: performance.now() });
                    }
                });
            }
        };
    });
    const page = await context.newPage();
    page.on("console", m => { if (["error", "warning"].includes(m.type())) consoleMessages.push({ type: m.type(), text: m.text() }); });
    page.on("pageerror", error => consoleMessages.push({ type: "pageerror", text: String(error) }));
    report.browserVersion = instance.version();
    const evaluate = expression => timed(page.evaluate(expression), "page.evaluate");
    const evalJson = async expression => JSON.parse(await evaluate(`(async () => JSON.stringify(await (${expression}),
        (key, value) => ArrayBuffer.isView(value) ? Array.from(value) : value))()`));
    const api = (op, extra) => evalJson(`window.__renderBench.apiCall(${JSON.stringify(op)}${extra ? `, ${JSON.stringify(extra)}` : ""})`);
    const lodStats = async () => (await api("getStats"))?.lod ?? {};
    const events = () => evalJson("window.__deviceEvents");
    // Settled: cut applied with a non-null digest that repeats on three samples in a row.
    const settle = async (label, limitMs) => {
        const started = Date.now();
        let digest = null, repeats = 0, lod = {};
        while (Date.now() - started < limitMs) {
            await evaluate("window.__renderBench.wake(); true");
            lod = await lodStats();
            const ok = lod.cutApply === "applied" && lod.cutDigestAtRest != null && !lod.cellsBusy;
            repeats = ok && lod.cutDigestAtRest === digest ? repeats + 1 : 0;
            digest = ok ? lod.cutDigestAtRest : null;
            if (repeats >= 3) return { digest, lod, ms: Date.now() - started };
            await sleep(200);
        }
        throw new Error(`${label}: did not settle within ${limitMs} ms (last ${JSON.stringify({ cutApply: lod.cutApply, digest: lod.cutDigestAtRest, busy: lod.cellsBusy })})`);
    };
    await page.goto(url.href, { waitUntil: "load" });
    const readyUntil = Date.now() + 120_000;
    while (await evaluate('document.querySelector("#dataset-file").disabled')) {
        if (Date.now() > readyUntil) throw new Error("Viewer startup timeout");
        await sleep(300);
    }
    await evaluate(`document.querySelector("#voxel-pixels").value=${targetPixels};
        document.querySelector("#voxel-pixels").dispatchEvent(new Event("input"));
        window.__renderBench.start(); true`);
    await sleep(500);
    await page.setInputFiles("#dataset-file", path.resolve(input));
    const loadStarted = Date.now();
    while (!(await evalJson("window.__renderBench.latestLodCounters()"))) {
        if (Date.now() - loadStarted > 60_000) throw new Error("dataset did not open within 60 s");
        await sleep(200);
    }
    // A pose away from the fitted one, so a recovery that restarted the camera would show.
    const fitted = await api("getCamera");
    const pose = { position: fitted.position.map((v, axis) => fitted.target[axis] + 0.6 * (v - fitted.target[axis])), target: fitted.target };
    await api("setCamera", pose);
    const before = await settle("before the crash", timeoutMs);
    report.before = { digest: before.digest, settleMs: before.ms, pose: await api("getCamera"), cellPath: before.lod.cellPath, loadMarker: await evaluate("window.__loadMarker") };
    const canvasBox = await page.locator("canvas").boundingBox();
    const beforeShot = await page.screenshot({ path: path.join(outDir, "before.png"), clip: canvasBox });
    report.beforeStats = { gpu: (await api("getStats"))?.gpu, lod: statsSubset(before.lod) };
    const gpuStatsBefore = (await api("getStats"))?.gpu;
    report.adapter = gpuStatsBefore?.adapter ?? null;
    report.deviceLostBefore = gpuStatsBefore?.deviceLost ?? null;

    const methods = {
        async gpucrash() {
            const tab = await context.newPage();
            // The navigation itself may be refused or never finish; what counts is the loss that follows.
            await tab.goto("chrome://gpucrash", { timeout: 5000 }).catch(error => ({ navigationError: String(error).split("\n")[0] }));
            await sleep(500);
            await tab.close().catch(() => {});
        },
        async cdp() {
            const session = await instance.newBrowserCDPSession();
            await session.send("Browser.crashGpuProcess");
        },
    };
    const order = methodOption === "auto" ? ["gpucrash", "cdp"] : [methodOption];
    let crashedAt = null;
    for (const name of order) {
        const attempt = { method: name };
        report.methodsTried.push(attempt);
        const startedAt = Date.now();
        try { attempt.result = (await methods[name]()) ?? null; } catch (error) { attempt.error = String(error).split("\n")[0]; }
        const waitUntil = Date.now() + 15_000;
        while (Date.now() < waitUntil && !(await events()).some(e => e.type === "deviceLost")) {
            await evaluate("window.__renderBench.wake(); true").catch(() => {});
            await sleep(200);
        }
        attempt.lossObserved = (await events()).some(e => e.type === "deviceLost");
        if (attempt.lossObserved) { crashedAt = startedAt; report.crashMethod = name; break; }
    }
    if (crashedAt === null) throw new Error("no method produced a device loss in this Chrome");

    const recoverUntil = Date.now() + timeoutMs;
    while (Date.now() < recoverUntil) {
        const list = await events();
        if (list.some(e => e.type === "deviceRecovery" && e.state === "recovered" || e.operation === "deviceLost")) break;
        await sleep(200);
    }
    report.recoveredAfterMs = Date.now() - crashedAt;
    const after = await settle("after the recovery", timeoutMs);
    report.after = { digest: after.digest, settleMs: after.ms, pose: await api("getCamera"), loadMarker: await evaluate("window.__loadMarker") };
    // The page only draws while woken: keep drawing for a second so the screenshot shows the presented frame.
    for (let i = 0; i < 5; i++) { await evaluate("window.__renderBench.wake(); true"); await sleep(200); }
    const afterShot = await page.screenshot({ path: path.join(outDir, "after.png"), clip: canvasBox });
    // A painted cloud compresses far worse than the empty canvas background: a crude "something is visible" test.
    report.screenshotBytes = { before: beforeShot.length, after: afterShot.length };
    report.afterStats = { gpu: (await api("getStats"))?.gpu, lod: statsSubset(await lodStats()) };
    report.events = (await events()).map(({ at, ...rest }) => rest);
    report.deviceLostAfter = (await api("getStats"))?.gpu?.deviceLost ?? null;
    report.checks = {
        lossReported: report.events.some(e => e.type === "deviceLost" && e.reason !== "destroyed"),
        recovered: report.events.some(e => e.type === "deviceRecovery" && e.state === "recovered"),
        noReload: report.before.loadMarker === report.after.loadMarker,
        samePose: samePose(report.before.pose, report.after.pose),
        sameDigest: report.before.digest === report.after.digest,
        newDeviceHealthy: report.deviceLostAfter === null,
        canvasPainted: afterShot.length > 0.5 * beforeShot.length,
        noPageErrors: !consoleMessages.some(m => m.type === "pageerror"),
    };
} catch (error) {
    report.error = String(error?.stack ?? error);
    console.error(report.error.split("\n")[0]);
} finally {
    await instance.close().catch(() => {});
}
if (report.error || !report.checks || Object.values(report.checks).some(ok => !ok)) process.exitCode = 1;
fs.writeFileSync(path.resolve(output), JSON.stringify(report, null, 2) + "\n");
console.log(JSON.stringify({ crashMethod: report.crashMethod, checks: report.checks, recoveredAfterMs: report.recoveredAfterMs, before: report.before?.digest, after: report.after?.digest, console: consoleMessages.length, error: report.error?.split("\n")[0] }, null, 1));
