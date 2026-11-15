// R3 Task 12 (B2.5): browser probe of the colour modes on the GPU cells path (cells + build worker).
// Run from the repository root while a static server serves the worktree root (with pkg/), over http:
//   node examples/web-host/probe-cells-colour.mjs DATASET.oct OUT.json [--shots=target/r3-b2] [--attributes=FILE|none]
//        [--url=URL] [--px=4] [--timeout-s=120] [--chrome-bin=PATH]
// Opens the dataset with cells at --px, settles in rgb, then selects each colour mode the dataset offers
// (the enabled options of #color-mode: intensity, elevation, classification and the first attribute:N:M, or --attribute=)
// and finally rgb again. Per mode it records: cellPath at every sample (anything but "enabled" is a fall
// back to the CPU path), the nodeCells received during the switch (0 for a shader mode, > 0 for an
// attribute mode: a new colour generation), the last plan's builtNodes / rebuiltTotal, whether the cut
// settled again (cutApply "applied" and a repeated non-null cutDigestAtRest at the unchanged camera),
// the digest, a hash of the canvas screenshot, and a PNG per mode in the shots directory.
// The final rgb must give the first rgb's cutDigestAtRest; the canvas hashes are reported, not enforced (a few pixels
// of a repeated rgb frame can differ between runs, so compare the PNGs when the hash differs). Console and page errors are
// listed. Not a unit test and not part of scripts/check.sh. Exit code 1 on any failed expectation.
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";

const args = process.argv.slice(2);
const positional = args.filter(arg => !arg.startsWith("--"));
const option = (name, fallback) => args.find(arg => arg.startsWith(`--${name}=`))?.split("=")[1] ?? fallback;
const [input, output] = positional;
if (!input || !output) {
    throw new Error("Usage: node examples/web-host/probe-cells-colour.mjs DATASET.oct OUT.json [--shots=DIR] [--attributes=FILE|none] [--attribute=attribute:N:M] [--url=URL] [--px=4] [--timeout-s=120]");
}
const baseUrl = option("url", "http://127.0.0.1:8767/examples/web-host/index.html");
const targetPixels = Number(option("px", "4"));
const timeoutMs = Number(option("timeout-s", "120")) * 1000;
const shotsDir = path.resolve(option("shots", "target/r3-b2"));
// The sidecar next to the dataset (DATASET.octattrs) is loaded with it unless --attributes=none or --attributes=FILE.
const attributesOption = option("attributes", input.replace(/\.oct$/, ".octattrs"));
const attributesFile = attributesOption !== "none" && fs.existsSync(attributesOption) ? path.resolve(attributesOption) : null;
const CHROME_BIN = option("chrome-bin", process.env.CHROME_BIN ?? "/usr/bin/google-chrome");
const PLAYWRIGHT_CORE = process.env.PLAYWRIGHT_CORE ?? "/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/";
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const CALL_TIMEOUT_MS = 10_000;
const timed = (promise, label) => {
    let timer;
    const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`${label} did not answer within ${CALL_TIMEOUT_MS} ms`)), CALL_TIMEOUT_MS); });
    return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
};
const baseName = path.basename(input).replace(/\.oct$/, "");

const { chromium } = createRequire(PLAYWRIGHT_CORE)("playwright-core");
const consoleMessages = [];
const responseErrors = [];
const report = { schemaVersion: 1, probe: "probe-cells-colour", startedAt: new Date().toISOString(), input: path.resolve(input),
    targetPixels, timeoutMs, consoleMessages, responseErrors, modes: [], failures: [] };
const instance = await chromium.launch({
    executablePath: CHROME_BIN, headless: true,
    args: ["--headless=new", "--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=vulkan"],
});
try {
    const url = new URL(baseUrl);
    if (!url.searchParams.has("cells")) url.searchParams.set("cells", "1");
    if (!url.searchParams.has("buildWorker")) url.searchParams.set("buildWorker", "1");
    report.url = url.href;
    report.attributesFile = attributesFile;
    const page = await (await instance.newContext({ viewport: { width: 1688, height: 768 }, deviceScaleFactor: 1 })).newPage();
    page.on("console", m => { if (["error", "warning"].includes(m.type())) consoleMessages.push({ type: m.type(), text: m.text() }); });
    page.on("response", response => { if (response.status() >= 400) responseErrors.push(`${response.status()} ${response.url()}`); });
    page.on("pageerror", error => consoleMessages.push({ type: "pageerror", text: String(error) }));
    const evaluate = expression => timed(page.evaluate(expression), "page.evaluate");
    const evalJson = async expression => JSON.parse(await evaluate(`(async () => JSON.stringify(await (${expression}),
        (key, value) => ArrayBuffer.isView(value) ? Array.from(value) : value))()`));
    const api = (op, extra) => evalJson(`window.__renderBench.apiCall(${JSON.stringify(op)}${extra ? `, ${JSON.stringify(extra)}` : ""})`);
    const lodStats = async () => (await api("getStats"))?.lod ?? {};
    const nodeCells = async () => (await evalJson("window.__renderBench.workerMetrics()"))?.cellCounters?.nodeCells ?? null;
    const strayScenes = async () => (await evalJson("window.__renderBench.workerMetrics()"))?.cellCounters?.strayScenes ?? null;

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
    // The attributes first: the dataset's change event opens it with the file already chosen.
    if (attributesFile) await page.setInputFiles("#attributes-file", attributesFile);
    await page.setInputFiles("#dataset-file", path.resolve(input));
    const loadStarted = Date.now();
    while (!(await evalJson("window.__renderBench.latestLodCounters()"))) {
        if (Date.now() - loadStarted > 60_000) throw new Error("dataset did not open within 60 s");
        const status = await evaluate('document.querySelector("#renderer-status").textContent');
        if (/failed/.test(status)) throw new Error(`dataset did not open: ${status}`);
        await sleep(200);
    }
    const fitted = await api("getCamera");
    report.attributeSchema = (await api("getMetadata"))?.attributeSchema ?? null;
    const available = await evaluate('JSON.stringify([...document.querySelector("#color-mode").options].filter(o => !o.disabled).map(o => o.value))').then(JSON.parse);
    const attribute = available.find(value => value === option("attribute", "") || (!option("attribute", "") && value.startsWith("attribute:")));
    const sequence = ["rgb", ...["intensity", "elevation", "classification"].filter(mode => available.includes(mode)), ...(attribute ? [attribute] : []), "rgb"];
    report.availableModes = available;
    report.sequence = sequence;
    fs.mkdirSync(shotsDir, { recursive: true });
    const canvasBox = async () => page.locator("canvas").boundingBox();

    const settle = async (mode, switched) => {
        const started = Date.now();
        const result = { cellPathSeen: new Set(), samples: 0, settled: false, digest: null, lod: null };
        let previous = null;
        while (Date.now() - started < timeoutMs) {
            await evaluate("window.__renderBench.wake(); true");
            const lod = await lodStats();
            const sameCamera = JSON.stringify(await api("getCamera")) === JSON.stringify(fitted);
            result.samples++;
            result.cellPathSeen.add(lod.cellPath ?? null);
            result.lod = lod;
            // A switch is only judged after the refine message had time to reach the workers.
            const waited = !switched || Date.now() - started >= 1500;
            const ok = waited && sameCamera && lod.cutApply === "applied" && lod.cutDigestAtRest != null && lod.cellsBusy !== true;
            if (ok && previous === lod.cutDigestAtRest) {
                result.settled = true;
                result.digest = lod.cutDigestAtRest;
                break;
            }
            previous = ok ? lod.cutDigestAtRest : null;
            await sleep(100);
        }
        result.settleMs = Date.now() - started;
        result.cellPathSeen = [...result.cellPathSeen];
        return result;
    };

    for (const [index, mode] of sequence.entries()) {
        const before = await nodeCells();
        const rebuiltBefore = (await lodStats()).rebuiltTotal ?? null;
        if (index > 0) await page.selectOption("#color-mode", mode);
        const result = await settle(mode, index > 0);
        const after = await nodeCells();
        // Let the colours of the new mode reach a frame, then hash and save the canvas.
        await evaluate("window.__renderBench.wake(); true");
        await sleep(500);
        const png = await page.screenshot({ clip: await canvasBox() });
        const file = path.join(shotsDir, `${baseName}-${index}-${mode.replace(/[^a-z0-9]+/gi, "_")}.png`);
        fs.writeFileSync(file, png);
        const entry = {
            mode, index, settled: result.settled, settleMs: result.settleMs, samples: result.samples,
            cellPathSeen: result.cellPathSeen, nodeCellsDuringSwitch: after - before,
            builtNodesLast: result.lod?.builtNodes ?? null, rebuiltTotalDelta: (result.lod?.rebuiltTotal ?? null) === null || rebuiltBefore === null ? null : result.lod.rebuiltTotal - rebuiltBefore,
            cutDigestAtRest: result.digest, canvasSha256: crypto.createHash("sha256").update(png).digest("hex"),
            attributeRange: result.lod?.attributeRange ?? null, strayScenes: await strayScenes(), screenshot: path.relative(process.cwd(), file),
        };
        report.modes.push(entry);
        const isAttribute = mode.startsWith("attribute:");
        if (!entry.settled) report.failures.push(`${mode}: did not settle`);
        if (entry.cellPathSeen.some(path => path !== "enabled")) report.failures.push(`${mode}: cellPath left "enabled": ${entry.cellPathSeen}`);
        if (index > 0 && !isAttribute && mode !== "rgb" && entry.nodeCellsDuringSwitch !== 0) report.failures.push(`${mode}: shader mode built ${entry.nodeCellsDuringSwitch} nodes`);
        if (isAttribute && entry.nodeCellsDuringSwitch === 0) report.failures.push(`${mode}: attribute mode built no nodes`);
        if (entry.strayScenes) report.failures.push(`${mode}: ${entry.strayScenes} stray scenes`);
    }
    const first = report.modes[0], last = report.modes.at(-1);
    report.backToRgb = { cutDigestAtRest: first.cutDigestAtRest === last.cutDigestAtRest, canvasSha256: first.canvasSha256 === last.canvasSha256 };
    if (!report.backToRgb.cutDigestAtRest) report.failures.push("final rgb cutDigestAtRest differs from the first rgb");
} catch (error) {
    report.error = String(error?.stack ?? error);
    report.failures.push(report.error.split("\n")[0]);
} finally {
    await instance.close().catch(() => {});
}
if (consoleMessages.length) report.failures.push(`${consoleMessages.length} console messages`);
if (report.failures.length) process.exitCode = 1;
fs.mkdirSync(path.dirname(path.resolve(output)), { recursive: true });
fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n");
console.log(`dataset ${path.basename(input)} px ${targetPixels}; modes ${report.sequence?.join(",")}`);
for (const m of report.modes) {
    console.log(`${m.mode}: settled ${m.settled} in ${m.settleMs} ms, cellPath ${m.cellPathSeen}, nodeCells ${m.nodeCellsDuringSwitch}, builtNodes ${m.builtNodesLast}, rebuilt+ ${m.rebuiltTotalDelta}, digest ${m.cutDigestAtRest}, png ${m.screenshot}`);
}
console.log(`back to rgb: ${JSON.stringify(report.backToRgb)}; console messages ${consoleMessages.length}; failed responses ${JSON.stringify(responseErrors)}; failures ${JSON.stringify(report.failures)}`);
