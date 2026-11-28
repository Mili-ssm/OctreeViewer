// Opens a file over HTTP Range through the RED-REF server (red-ref-server.mjs) in headless Chrome
// and records first-frame time, time to the pixel target, and the requests and bytes the server
// saw (R5 P4, RQ-04, ASM-02). Serve the repository root (with pkg/) on --app first:
//   python3 -m http.server 8767
//   node examples/web-host/probes/probe-remote-open.mjs DATA_DIR NAME.oct OUTPUT.json [--app=http://127.0.0.1:8767]
//        [--px=4] [--rtt-ms=50] [--mbps=100] [--max-s=120] [--ignore-range] [--width=1280 --height=720]
// Times come from the page (`firstCoarseFrameMs`, `targetLodMs`, measured from the open); the
// request log is the server's, so it also counts requests made by both workers.
import fs from "node:fs";
import { createRequire } from "node:module";

import { startServer } from "./red-ref-server.mjs";

const args = process.argv.slice(2);
const [dataDir, name, output] = args.filter((arg) => !arg.startsWith("--"));
const option = (key, fallback) => args.find((arg) => arg.startsWith(`--${key}=`))?.split("=")[1] ?? fallback;
if (!dataDir || !name || !output) throw new Error("Usage: node probe-remote-open.mjs DATA_DIR NAME.oct OUTPUT.json [options]");
const app = option("app", "http://127.0.0.1:8767");
const px = Number(option("px", "4"));
const maxMs = Number(option("max-s", "120")) * 1000;
const width = Number(option("width", "1280"));
const height = Number(option("height", "720"));
const CHROME_BIN = process.env.CHROME_BIN ?? "/usr/bin/google-chrome";
const PLAYWRIGHT_CORE = process.env.PLAYWRIGHT_CORE ?? "/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/";
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

const server = await startServer({
  root: dataDir,
  rttMs: Number(option("rtt-ms", "50")),
  mbps: Number(option("mbps", "100")),
  ignoreRange: args.includes("--ignore-range"),
});
const { chromium } = createRequire(PLAYWRIGHT_CORE)("playwright-core");
const browser = await chromium.launch({
  executablePath: CHROME_BIN,
  args: ["--headless=new", "--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=vulkan"],
});
const page = await browser.newPage({ viewport: { width, height } });
const consoleErrors = [];
page.on("console", (message) => message.type() === "error" && consoleErrors.push(message.text()));
page.on("pageerror", (error) => consoleErrors.push(String(error)));
const result = { name, px, width, height, rttMs: Number(option("rtt-ms", "50")), mbps: Number(option("mbps", "100")) };
try {
  const dataUrl = `http://127.0.0.1:${server.port}/${name}`;
  await page.goto(`${app}/examples/web-host/index.html?url=${encodeURIComponent(dataUrl)}&px=${px}`);
  const startedAt = Date.now();
  let timings = null;
  let lod = null;
  while (Date.now() - startedAt < maxMs) {
    await sleep(250);
    timings = await page.evaluate(() => window.__renderBench?.openTimings?.() ?? null);
    lod = await page.evaluate(() => window.__renderBench?.apiCall("getStats").then((stats) => stats?.lod ?? null) ?? null);
    const status = await page.evaluate(() => document.querySelector("#renderer-status").textContent);
    result.status = status;
    if (/failed|error/i.test(status)) break;
    if (timings?.targetLodMs != null && !(lod?.pendingPageAnchors?.length)) break;
  }
  result.wallMs = Date.now() - startedAt;
  result.timings = timings;
  result.lod = lod && {
    pendingPageAnchors: lod.pendingPageAnchors?.length ?? null,
    pageBudgetExhausted: lod.pageBudgetExhausted ?? null,
    pendingBlockIds: lod.pendingBlockIds?.length ?? null,
  };
  result.pages = (await page.evaluate(() => window.__renderBench.workerMetrics()))?.buildWorker?.pages ?? null;
  const log = server.log;
  const first = log.find((entry) => entry.method === "GET")?.at ?? 0;
  const get = log.filter((entry) => entry.method === "GET");
  result.requests = {
    total: log.length,
    get: get.length,
    preflight: log.filter((entry) => entry.method === "OPTIONS").length,
    multiRange: log.filter((entry) => entry.multiRange).length,
    status200: get.filter((entry) => entry.status === 200).length,
    bytes: get.reduce((sum, entry) => sum + entry.bytes, 0),
    // The request count at the moment the first frame was drawn: requests started before the page's
    // `firstCoarseFrameMs` after the first GET (the open itself counts).
    untilFirstFrame: timings?.firstCoarseFrameMs == null
      ? null
      : get.filter((entry) => entry.at - first <= timings.firstCoarseFrameMs).length,
    log: log.map((entry) => ({ ...entry, at: Math.round(entry.at - first) })),
  };
} catch (error) {
  result.error = String(error?.stack ?? error);
}
result.consoleErrors = consoleErrors;
fs.writeFileSync(output, JSON.stringify(result, null, 2) + "\n");
await browser.close();
await server.close();
console.log(JSON.stringify({ ...result, requests: { ...result.requests, log: undefined } }));
