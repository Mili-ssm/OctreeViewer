// THROWAWAY SPIKE CODE (R3 Task 5a): GPU timing probe for the cube-pool spike page.
//   node examples/web-host/probe-cube-pool.mjs OUTPUT.json CASE_DIR... [--rounds=3] [--warmup-s=2] [--frames=30]  [--diag]
//   T16a (compaction + Hi-Z, W16 only): add --t16 to time the T16 variant set instead of the T5 one, or --t16-check
//   for the untimed correctness check (compacted counts, bit-identical depth against w16-ranges); both are opt-in.
// Case directories (format: spike-cube-pool-format.md) must live inside the repository root, which is served
// over HTTP. Launches Chrome like probe-orbit-load.mjs (ANGLE-Vulkan, hardware adapter required) plus
// --enable-webgpu-developer-features, runs ROUNDS rounds over all cases and writes one JSON with the adapter,
// features, limits, power/clock/thermal state before and after each round and the per-frame series.
// Exit code is non-zero on any failure (software adapter, device error, empty render, missing timestamps).
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const args = process.argv.slice(2);
const positional = args.filter(arg => !arg.startsWith("--"));
const option = (name, fallback) => args.find(arg => arg.startsWith(`--${name}=`))?.split("=")[1] ?? fallback;
const [output, ...caseDirs] = positional;
const diag = args.includes("--diag"); // T5b: overdraw and visible-fraction diagnostics, one pass, no timing
const t16 = args.includes("--t16");
const t16Check = args.includes("--t16-check");
const rounds = diag || t16Check ? 1 : Number(option("rounds", "3"));
const warmupMs = Number(option("warmup-s", "2")) * 1000;
const frames = Number(option("frames", "30"));
if (!output || !caseDirs.length || !(rounds >= 1) || warmupMs < 0 || !(frames >= 1)) {
    throw new Error("Usage: node examples/web-host/probe-cube-pool.mjs OUTPUT.json CASE_DIR... [--rounds=3] [--warmup-s=2] [--frames=30]");
}
const CHROME_BIN = process.env.CHROME_BIN ?? "/usr/bin/google-chrome";
const PLAYWRIGHT_CORE = process.env.PLAYWRIGHT_CORE ?? "/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/";

const caseUrls = caseDirs.map(dir => {
    const rel = path.relative(root, path.resolve(dir));
    if (rel.startsWith("..") || path.isAbsolute(rel)) throw new Error(`${dir} is outside the served repository root ${root}`);
    return { dir: path.resolve(dir), url: `/${rel.split(path.sep).join("/")}` };
});

// ---- environment snapshots (sysfs) ----
const readText = file => { try { return fs.readFileSync(file, "utf8").trim(); } catch { return null; } };
const ls = dir => { try { return fs.readdirSync(dir); } catch { return []; } };
function snapshot() {
    const cards = ls("/sys/class/drm").filter(n => /^card\d+$/.test(n));
    return {
        t: Date.now(),
        acOnline: Object.fromEntries(ls("/sys/class/power_supply").map(n => [n, { type: readText(`/sys/class/power_supply/${n}/type`), online: readText(`/sys/class/power_supply/${n}/online`) }])
            .filter(([, v]) => v.online !== null)),
        powerProfile: (() => { try { return execFileSync("powerprofilesctl", ["get"], { encoding: "utf8" }).trim(); } catch { return null; } })(),
        gpu: Object.fromEntries(cards.map(card => {
            const dev = `/sys/class/drm/${card}/device`;
            const temps = {};
            for (const hw of ls(`${dev}/hwmon`)) {
                for (const f of ls(`${dev}/hwmon/${hw}`).filter(n => /^temp\d+_input$/.test(n))) {
                    temps[`${readText(`${dev}/hwmon/${hw}/name`)}/${f}`] = Number(readText(`${dev}/hwmon/${hw}/${f}`)) / 1000;
                }
            }
            return [card, { pp_dpm_sclk: readText(`${dev}/pp_dpm_sclk`), gpu_busy_percent: readText(`${dev}/gpu_busy_percent`), tempsC: temps }];
        }).filter(([, v]) => v.pp_dpm_sclk !== null || v.gpu_busy_percent !== null || Object.keys(v.tempsC).length)),
    };
}

// ---- static server for the repo root ----
const MIME = { ".html": "text/html", ".mjs": "text/javascript", ".js": "text/javascript", ".json": "application/json" };
const server = http.createServer((req, res) => {
    if (req.url === "/favicon.ico") { res.writeHead(204).end(); return; }
    const file = path.resolve(root, `.${decodeURIComponent(new URL(req.url, "http://x").pathname)}`);
    if (!file.startsWith(root + path.sep) || !fs.existsSync(file) || !fs.statSync(file).isFile()) { res.writeHead(404).end(); return; }
    // Cross-origin isolation gives performance.now() microsecond resolution (else 0.1 ms), needed for CPU encode times.
    res.writeHead(200, { "cross-origin-opener-policy": "same-origin", "cross-origin-embedder-policy": "require-corp", "content-type": MIME[path.extname(file)] ?? "application/octet-stream", "content-length": fs.statSync(file).size });
    fs.createReadStream(file).pipe(res);
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${server.address().port}`;

const failures = [];
const consoleErrors = [];
const { chromium } = createRequire(PLAYWRIGHT_CORE)("playwright-core");
const instance = await chromium.launch({
    executablePath: CHROME_BIN,
    headless: true,
    args: ["--headless=new", "--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=vulkan", "--enable-webgpu-developer-features"],
});
const result = { startedAt: new Date().toISOString(), args: { rounds, warmupMs, frames, caseDirs }, browserVersion: instance.version(), cases: {}, rounds: [] };
try {
    const page = await (await instance.newContext({ viewport: { width: 1280, height: 720 }, deviceScaleFactor: 1 })).newPage();
    page.on("console", message => { if (message.type() === "error") consoleErrors.push(message.text()); });
    page.on("pageerror", error => consoleErrors.push(String(error)));
    await page.goto(`${origin}/examples/web-host/spike-cube-pool.html`, { waitUntil: "load" });
    result.device = await page.evaluate(() => window.__cubePool.init());
    const { adapterInfo } = result.device;
    const software = !adapterInfo.vendor || adapterInfo.isFallbackAdapter
        || /google|llvmpipe|swiftshader/i.test(`${adapterInfo.vendor} ${adapterInfo.architecture} ${adapterInfo.description}`);
    if (software) throw new Error(`software or unidentified WebGPU adapter rejected: ${JSON.stringify(adapterInfo)}`);
    if (!result.device.deviceFeatures.includes("timestamp-query")) throw new Error("device lacks timestamp-query");

    for (const [index, { dir, url }] of caseUrls.entries()) {
        const name = path.basename(dir);
        result.cases[name] = { url, summary: await page.evaluate(base => window.__cubePool.loadCase(base), url), rounds: [] };
        if (t16Check) {
            const data = await page.evaluate(() => window.__cubePool.runT16Check());
            result.cases[name].t16Check = data;
            for (const message of data.failures) failures.push(`${name} t16-check: ${message}`);
            const c = data.count;
            console.log(`${name} count: drawn_cells ${c.drawn_cells} k-nofrustum ${c.k_nofrustum_compacted} (pages ${c.k_nofrustum_pages}/${c.total_pages}) k ${c.k_compacted} (pages ${c.k_pages})`);
            for (const [kind, r] of Object.entries(data.static)) {
                const last = r.frames[r.frames.length - 1];
                console.log(`${name} static ${kind}: ${r.frames.length} frames, holes ${r.frames.reduce((a, f) => a + f.holes, 0)}, shallower ${r.frames.reduce((a, f) => a + f.shallower, 0)}, colour-diff ${(last.colorDiffFraction * 100).toFixed(4)} % , covered px ${last.coveredPixels}, last pages1 ${last.pages1} pages2 ${last.pages2 ?? "-"} cells1 ${last.cells1} cells2 ${last.cells2 ?? "-"}`);
            }
            for (const [kind, r] of Object.entries(data.moving)) {
                console.log(`${name} moving ${kind}: ${r.frames} frames, holes ${r.holes}, shallower ${r.shallower}, colour-diff mean ${(r.colorDiffFractionMean * 100).toFixed(4)} % max ${(r.colorDiffFractionMax * 100).toFixed(4)} %`);
            }
            continue;
        }
        if (diag) {
            const data = await page.evaluate(() => window.__cubePool.runDiag());
            result.cases[name].diag = data;
            if (data.errors.length) failures.push(`${name} diag: ${data.errors.join("; ")}`);
            for (const [v, r] of Object.entries(data.variants)) {
                if (!(r.covered_px > 0)) failures.push(`${name} diag ${v}: no covered pixel`);
            }
            const a = data.variants["w10-ranges"], b = data.variants["i40-single"];
            console.log(`${name} covered ${a.covered_px} overdraw all/cov ${a.overdraw_all_over_covered?.toFixed(2)} (i40 ${b.overdraw_all_over_covered?.toFixed(2)}) tested/cov ${a.depth_tested_over_covered?.toFixed(2)} (i40 ${b.depth_tested_over_covered?.toFixed(2)}) visible w10 ${a.visible_fraction.toFixed(4)} i40 ${b.visible_fraction.toFixed(4)}`);
            continue;
        }
        for (let round = 0; round < rounds; round++) {
            const before = snapshot();
            const data = await page.evaluate(opts => window.__cubePool.runRound(opts), { warmupMs, frames, round, ...(t16 ? { set: "t16" } : {}) });
            const after = snapshot();
            result.cases[name].rounds.push({ before, after, ...data });
            if (data.errors.length) failures.push(`${name} round ${round}: ${data.errors.join("; ")}`);
            for (const v of t16 ? data.variants : []) {
                if (!(v.nonBlackPixels > 0)) failures.push(`${name} round ${round} ${v.name}: rendered no non-black pixel`);
                const s = v.summary.gpuMs;
                const count = Array.isArray(v.counts) ? v.counts[v.counts.length - 1] : v.counts;
                const extra = v.moving ? ` p95 ${s.p95.toFixed(3)} cold ${v.cold.gpuMs.toFixed(3)} stableFrame ${v.stableFrame?.frame ?? "-"}` : ` p95 ${s.p95.toFixed(3)}`;
                const cnt = count ? ` pages ${count.pages1}${count.pages2 !== undefined ? `/${count.pages2}` : ""} cells ${count.cells1}${count.cells2 !== undefined ? `/${count.cells2}` : ""}` : "";
                console.log(`${name} r${round} ${v.name.padEnd(18)} gpu median ${s.median.toFixed(3)} ms${extra}${cnt} | ${v.passes.map(p => `${p.name} ${p.ms.median.toFixed(3)}`).join(" ")}`);
            }
            for (const v of t16 ? [] : data.variants) {
                if (!(v.nonBlackPixels > 0)) failures.push(`${name} round ${round} ${v.name}: rendered no non-black pixel`);
                if (v.series.gpuMs.length !== frames) failures.push(`${name} ${v.name}: ${v.series.gpuMs.length} frames, expected ${frames}`);
                console.log(`${name} r${round} ${v.name.padEnd(18)} gpu median ${v.summary.gpuMs.median.toFixed(3)} ms  wall ${v.summary.wallMs.median.toFixed(3)}  encode ${v.summary.encodeMs.median.toFixed(3)}  tickGCD ${v.tickGcd}  px ${v.nonBlackPixels}`);
            }
        }
        void index;
    }
} catch (error) {
    failures.push(String(error.stack ?? error));
} finally {
    await instance.close().catch(() => {});
    server.close();
}
if (consoleErrors.length) failures.push(`console errors: ${consoleErrors.join(" | ")}`);
result.consoleErrors = consoleErrors;
result.failures = failures;
result.finishedAt = new Date().toISOString();
fs.mkdirSync(path.dirname(path.resolve(output)), { recursive: true });
fs.writeFileSync(output, JSON.stringify(result, null, 1));
if (failures.length) {
    console.error(`FAILED:\n${failures.join("\n")}`);
    process.exit(1);
}
console.log(`ok, wrote ${output}`);
