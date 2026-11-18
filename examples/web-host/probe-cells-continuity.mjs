// R3 Task 13 (B3.2): browser probe of continuity (RF-05) on the GPU cells path (cells + build worker).
// Run from the repository root while a static server serves the worktree root (with pkg/), over http:
//   node examples/web-host/probe-cells-continuity.mjs DATASET.oct OUT.json [--shots=target/r3-b3] [--px=4]
//        [--poses=fitted,leaf] [--url=URL] [--timeout-s=120] [--chrome-bin=PATH]
// Per pose: settles literal (repeated non-null cutDigestAtRest), screenshots, turns continuity on through
// the demo's minimum-size selector (spacing mode), settles again, screenshots and records the effective minimum side, the nodes the
// render side received across the toggle (must be 0: cells hold every level) and the stats of both modes.
// Not a unit test and not part of scripts/check.sh.
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";

const args = process.argv.slice(2);
const positional = args.filter(arg => !arg.startsWith("--"));
const option = (name, fallback) => args.find(arg => arg.startsWith(`--${name}=`))?.split("=")[1] ?? fallback;
const [input, output] = positional;
if (!input || !output) throw new Error("Usage: node examples/web-host/probe-cells-continuity.mjs DATASET.oct OUT.json [--shots=DIR] [--px=4] [--poses=fitted,leaf]");
const baseUrl = option("url", "http://127.0.0.1:8767/examples/web-host/index.html");
const targetPixels = Number(option("px", "4"));
const timeoutMs = Number(option("timeout-s", "120")) * 1000;
const shots = option("shots", "target/r3-b3");
const poseNames = option("poses", "fitted").split(",");
const CHROME_BIN = option("chrome-bin", process.env.CHROME_BIN ?? "/usr/bin/google-chrome");
const PLAYWRIGHT_CORE = process.env.PLAYWRIGHT_CORE ?? "/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/";
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const CALL_TIMEOUT_MS = 10_000;
const timed = (promise, label) => {
    let timer;
    const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`${label} did not answer within ${CALL_TIMEOUT_MS} ms`)), CALL_TIMEOUT_MS); });
    return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
};
// The T16 case majadas-inside-leaf (see probe-cells-smoke.mjs).
const LEAF_CASE = { eye: [621, 619, 155], direction: [0, 1, 0] };
function leafPose(metadata) {
    const side = Math.max(...metadata.boundsMax.map((v, axis) => v - metadata.boundsMin[axis]));
    const eye = LEAF_CASE.eye.map((v, axis) => metadata.coordinateOrigin[axis] + v);
    return { position: eye, target: eye.map((v, axis) => v + side / 16 * LEAF_CASE.direction[axis]) };
}
const KEYS = ["cutApply", "cutDigestAtRest", "cellsBusy", "poolStages", "poolNodesWithoutLevel", "poolCoarsened", "cpuCoarsened", "gpuCoarsened",
    "poolOverflow", "poolResidentPages", "cellNodesReceivedTotal", "continuity", "continuityFactor", "continuityMinSide", "cellPath"];
const statsOf = lod => Object.fromEntries(KEYS.map(key => [key, lod?.[key] ?? null]));

const { chromium } = createRequire(PLAYWRIGHT_CORE)("playwright-core");
const consoleMessages = [];
const report = { schemaVersion: 1, probe: "probe-cells-continuity", startedAt: new Date().toISOString(), input: path.resolve(input), targetPixels, consoleMessages, poses: [] };
const instance = await chromium.launch({
    executablePath: CHROME_BIN, headless: true,
    args: ["--headless=new", "--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=vulkan"],
});
try {
    const url = new URL(baseUrl);
    url.searchParams.set("cells", "1");
    url.searchParams.set("buildWorker", "1");
    const page = await (await instance.newContext({ viewport: { width: 1688, height: 768 }, deviceScaleFactor: 1 })).newPage();
    page.on("console", m => { if (["error", "warning"].includes(m.type())) consoleMessages.push({ type: m.type(), text: m.text() }); });
    page.on("pageerror", error => consoleMessages.push({ type: "pageerror", text: String(error) }));
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
        await sleep(200);
    }
    const fitted = await api("getCamera");
    const metadata = await api("getMetadata");
    // Waits for a repeated non-null cutDigestAtRest; returns the stats and the ms it took, or null on timeout.
    const settle = async () => {
        const started = Date.now();
        let previous = null, last = {};
        while (Date.now() - started < timeoutMs) {
            await evaluate("window.__renderBench.wake(); true");
            last = await lodStats();
            if (last.cellsBusy === false && last.cutApply === "applied" && last.cutDigestAtRest != null && last.cutDigestAtRest === previous) return { stats: last, ms: Date.now() - started };
            previous = last.cutDigestAtRest ?? null;
            await sleep(150);
        }
        return { stats: last, ms: null };
    };
    const setContinuity = async on => {
        await evaluate(`(() => { const mode = document.querySelector("#min-length-mode"); mode.value = ${on} ? "spacing" : "length"; mode.dispatchEvent(new Event("change")); return true; })()`);
    };
    for (const name of poseNames) {
        const pose = name === "leaf" ? leafPose(metadata) : fitted;
        const entry = { label: name, pose };
        await setContinuity(false);
        if (name !== "fitted") await api("setCamera", { position: pose.position, target: pose.target });
        const literal = await settle();
        entry.literal = { settleMs: literal.ms, stats: statsOf(literal.stats) };
        await page.screenshot({ path: path.join(shots, `${path.basename(input, ".oct")}-${name}-${targetPixels}px-literal.png`) });
        const received = literal.stats.cellNodesReceivedTotal;
        await setContinuity(true);
        const on = await settle();
        entry.continuity = { settleMs: on.ms, stats: statsOf(on.stats) };
        await page.screenshot({ path: path.join(shots, `${path.basename(input, ".oct")}-${name}-${targetPixels}px-continuity.png`) });
        entry.buildsOnToggle = on.stats.cellNodesReceivedTotal - received;
        await setContinuity(false);
        const off = await settle();
        entry.backToLiteral = { settleMs: off.ms, sameDigest: off.stats.cutDigestAtRest === literal.stats.cutDigestAtRest, buildsAfterOff: off.stats.cellNodesReceivedTotal - received };
        report.poses.push(entry);
    }
} catch (error) {
    report.error = String(error?.stack ?? error);
    console.error(report.error.split("\n")[0]);
} finally {
    await instance.close().catch(() => {});
}
fs.mkdirSync(path.dirname(path.resolve(output)), { recursive: true });
fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n");
console.log(JSON.stringify(report.poses.map(p => ({ pose: p.label, literalMs: p.literal.settleMs, continuityMs: p.continuity.settleMs, minSide: p.continuity.stats.continuityMinSide, builds: p.buildsOnToggle, back: p.backToLiteral.sameDigest })))
    + ` errors ${consoleMessages.length}${report.error ? " ERROR" : ""}`);
if (report.error || report.poses.length < poseNames.length) process.exitCode = 1;
