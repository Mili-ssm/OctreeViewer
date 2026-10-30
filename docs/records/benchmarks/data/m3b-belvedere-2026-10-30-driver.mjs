// Driver of the M3b Belvedere browser run (Task 6, 30-10-2026). Throwaway.
// Usage (repo root, after the viewer_wasm release build + wasm-bindgen into pkg/ and
// `python3 examples/web-host/package-local.py --output target/m3b/package`):
//   node docs/records/benchmarks/data/m3b-belvedere-2026-10-30-driver.mjs [--out=file.jsonl] [--only=cell,cell]
// Cells (run in this order, one fresh browser + profile per cell; profiles live under
// <repo>/target/m3b-profiles and are deleted in `finally`):
//   http cells  : m3b-belvedere-2026-10-30.html over `python3 -m http.server`, Belvedere through
//                 <input type=file> (disk-backed File), options spatialDivisions / spoolBudgetBytes /
//                 ramBudget; the page polls navigator.storage.estimate() and the OPFS directory.
//   file cells  : the packaged index.html (artifacts of package-local.py) driven through its own UI.
// Per cell: wall time, worker time, spool stats, OPFS usage peak, zones, peak PSS of the browser tree
// (examples/web-host/sample-process-memory.py, 0.5 s) between start and end of the conversion, and the
// SHA-256 of the saved download pair. The native reference is measured separately (native_hybrid).
// Stop rule: a SHA different from R1 or > 4x the native time stops the remaining cells of that browser.
import { createRequire } from "module";
import { spawn, execSync } from "child_process";
import { createHash } from "crypto";
import fs from "fs";
import path from "path";

const core = process.env.PLAYWRIGHT_CORE ?? "/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/";
const FIREFOX_BIN = process.env.FIREFOX_BIN ?? `${process.env.HOME}/.cache/ms-playwright/firefox-1555/firefox/firefox`;
const { chromium, firefox } = createRequire(core)("playwright-core");

const repo = process.cwd();
const args = process.argv.slice(2);
const opt = (k, d) => (args.find((a) => a.startsWith(`--${k}=`)) ?? `--${k}=${d}`).split("=")[1];
const outFile = opt("out", "");
const only = opt("only", "").split(",").filter(Boolean);
const port = 8797;
const GiB = 2 ** 30;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const emit = (rec) => {
  const line = JSON.stringify(rec);
  console.log(line);
  if (outFile) fs.appendFileSync(outFile, line + "\n");
};
const sha = (file) => new Promise((res, rej) => {
  const h = createHash("sha256");
  fs.createReadStream(file).on("data", (c) => h.update(c)).on("end", () => res(h.digest("hex"))).on("error", rej);
});

const BELV = "data/datasets/belv_2009_digital-aerial_pcd_copc.laz";
const SMALL = "data/samples/fs_e430_n4008_18.las";
const R1 = {
  d2: { oct: "ecdc4889778c082ee4ba67d8d0a2fc37182a9d3f2f35abd965a8293bec6fccad", attrs: "cd3cad4571b2474377b977fe2237b645eef221f97f68c6e1783c5fd4a2a8fe9a" },
  d8: { oct: "3aee289064d516170e20c727af5a92e39dbb153c83ee0a2dc0476bf147830f07", attrs: "816959b4d1aa34915109ae1417e9830759be348b7531864ab1daad8c643b19dc" },
  small: { oct: "176ee42de533ca13222398c6e1a5db589494512622f10d0a0230e27153af925e", attrs: "ee1e5daa735045db4b3191f688543697271efdc16909052bb6ef1354f76e171c" },
};
// Native native_hybrid elapsed (ms) measured in the same session; 4x limits derive from them.
const NATIVE_MS = { "d2-2": 43668, "d8-2": 34688, "d2-4": 43518 };

const CELLS = [
  { id: "chrome-http-d2-2g-r1", browser: "chrome", origin: "http", file: BELV, div: 2, gib: 2, ram: 4e9, ref: "d2", nat: "d2-2" },
  { id: "chrome-http-d2-2g-r2", browser: "chrome", origin: "http", file: BELV, div: 2, gib: 2, ram: 4e9, ref: "d2", nat: "d2-2" },
  { id: "chrome-http-d2-4g", browser: "chrome", origin: "http", file: BELV, div: 2, gib: 4, ram: 8e9, ref: "d2", nat: "d2-4" },
  { id: "chrome-http-d8-2g", browser: "chrome", origin: "http", file: BELV, div: 8, gib: 2, ram: 4e9, ref: "d8", nat: "d8-2" },
  { id: "firefox-http-d2-2g", browser: "firefox", origin: "http", file: BELV, div: 2, gib: 2, ram: 4e9, ref: "d2", nat: "d2-2" },
  { id: "firefox-http-d2-4g", browser: "firefox", origin: "http", file: BELV, div: 2, gib: 4, ram: 8e9, ref: "d2", nat: "d2-4" },
  { id: "firefox-http-d8-2g", browser: "firefox", origin: "http", file: BELV, div: 8, gib: 2, ram: 4e9, ref: "d8", nat: "d8-2" },
  { id: "firefox-file-d2-2g", browser: "firefox", origin: "file", ui: true, file: BELV, div: 2, gib: 2, ram: 4e9, ref: "d2", nat: "d2-2" },
  { id: "chrome-file-small", browser: "chrome", origin: "file", ui: true, file: SMALL, div: 2, gib: 2, ram: 4e9, ref: "small" },
  { id: "chrome-file-belvedere-rejected", browser: "chrome", origin: "file", ui: true, file: BELV, div: 2, gib: 2, ram: 4e9, expectReject: true },
];

function browserRootPid(profile) {
  const pids = execSync(`pgrep -f -- "${profile}" || true`).toString().split("\n").filter(Boolean).map(Number);
  const set = new Set(pids);
  for (const pid of pids) {
    const ppid = Number(fs.readFileSync(`/proc/${pid}/stat`, "utf8").replace(/^.*\) /, "").split(" ")[1]);
    if (!set.has(ppid)) return pid;
  }
  throw new Error("browser root pid not found");
}
const pssWindow = (file, fromMs, toMs) => {
  const rows = fs.readFileSync(file, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l))
    .filter((r) => r.wall_time_ns / 1e6 >= fromMs && r.wall_time_ns / 1e6 <= toMs && r.root_alive && r.enumerated_processes_readable && r.tree_bytes?.Pss != null);
  if (!rows.length) return null;
  const pss = rows.map((r) => r.tree_bytes.Pss);
  const peak = rows[pss.indexOf(Math.max(...pss))];
  return { samples: rows.length, peakPssBytes: Math.max(...pss), peakRssBytes: Math.max(...rows.map((r) => r.tree_bytes.Rss)), processesAtPeak: peak.enumerated_processes };
};
const load = () => ({ loadavg: fs.readFileSync("/proc/loadavg", "utf8").trim(), cpuPressure: fs.readFileSync("/proc/pressure/cpu", "utf8").split("\n")[0] });

// In-page poller for file cells (the http page has its own).
const POLLER = `(() => {
  window.__trace = [];
  const sample = async () => {
    const rec = { t: Math.round(performance.now()) };
    try { rec.usage = (await navigator.storage.estimate()).usage; } catch {}
    try {
      const dir = await (await navigator.storage.getDirectory()).getDirectoryHandle("octree-viewer-conversions", { create: true });
      const names = []; for await (const n of dir.keys()) names.push(n);
      rec.files = names.length; rec.zones = names.filter((n) => n.includes(".zone-")).length;
    } catch (e) { rec.noOpfs = String(e.name); }
    window.__trace.push(rec);
  };
  window.__sample = sample; sample(); window.__poller = setInterval(sample, 1000);
})()`;

async function runCell(cell, dl) {
  const rec = { cell: cell.id, browser: cell.browser, origin: cell.origin, divisions: cell.div, spoolBudgetGiB: cell.gib, ramBudget: cell.ram, input: cell.file, hostBefore: load() };
  fs.mkdirSync(path.join(repo, "target/m3b-profiles"), { recursive: true });
  const profile = fs.mkdtempSync(path.join(repo, "target/m3b-profiles/profile-"));
  const samples = path.join(repo, `target/m3b/${cell.id}-pss.jsonl`);
  fs.rmSync(samples, { force: true });
  let server = null, ctx = null, sampler = null;
  try {
    if (cell.origin === "http") {
      server = spawn("python3", ["-m", "http.server", String(port), "--bind", "127.0.0.1"], { cwd: repo, stdio: "ignore" });
      await sleep(800);
    }
    // Firefox: grant persist() without a dialog (S8 prefs) so the 10 GiB temporary limit does not apply.
    const prefs = { "dom.storageManager.prompt.testing": true, "dom.storageManager.prompt.testing.allow": true };
    ctx = cell.browser === "chrome"
      ? await chromium.launchPersistentContext(profile, { executablePath: "/usr/bin/google-chrome", headless: true, acceptDownloads: true })
      : await firefox.launchPersistentContext(profile, { executablePath: FIREFOX_BIN, headless: true, firefoxUserPrefs: prefs, acceptDownloads: true });
    rec.version = ctx.browser()?.version?.() ?? null;
    const pid = browserRootPid(profile);
    sampler = spawn("python3", ["examples/web-host/sample-process-memory.py", String(pid), samples, "--interval", "0.5", "--seconds", "1500"], { cwd: repo, stdio: "ignore" });
    await sleep(1500);
    const page = ctx.pages()[0] ?? (await ctx.newPage());
    const logs = [];
    page.on("console", (m) => { if (["error", "warning"].includes(m.type())) logs.push(`${m.type()}: ${m.text().slice(0, 300)}`); });
    page.on("pageerror", (e) => logs.push(`pageerror: ${e}`));
    const abs = path.join(repo, cell.file);
    let wallStart, wallEnd, out;
    if (!cell.ui) {
      await page.goto(`http://127.0.0.1:${port}/docs/records/benchmarks/data/m3b-belvedere-2026-10-30.html`);
      await page.waitForFunction(() => window.ready, null, { timeout: 60000 });
      await page.setInputFiles("#f", abs);
      rec.persistedBefore = await page.evaluate(() => navigator.storage.persisted?.());
      const options = { spatialDivisions: cell.div, spoolBudgetBytes: cell.gib * GiB, ramBudget: cell.ram };
      rec.options = options;
      out = await page.evaluate((o) => window.convertCase(o), options);
      rec.result = out;
      wallStart = out.wallStart; wallEnd = out.wallEnd;
      rec.leftoverAfter = await page.evaluate(() => window.leftover());
      if (!out.error) {
        const save = async (which) => {
          const [d] = await Promise.all([page.waitForEvent("download", { timeout: 300000 }), page.evaluate((w) => window.downloadOf(w), which)]);
          const p = path.join(dl, `${cell.id}-${which}`);
          await d.saveAs(p);
          const h = await sha(p);
          fs.rmSync(p, { force: true });
          return h;
        };
        rec.oct = await save("oct");
        rec.attrs = await save("attrs");
      }
    } else {
      const url = cell.origin === "file" ? `file://${repo}/target/m3b/package/index.html` : null;
      await page.goto(url);
      await page.waitForFunction(() => typeof createConverter === "function" && document.getElementById("convert-file"), null, { timeout: 60000 });
      await page.setInputFiles("#convert-file", abs);
      await page.evaluate(() => { document.getElementById("convert-options").open = true; });
      await page.selectOption("#opt-spatial-divisions", String(cell.div));
      await page.evaluate(POLLER);
      wallStart = Date.now();
      await page.evaluate(() => {
        document.getElementById("convert-start").click();
        window.__complete = null;
        converterApi.on("complete", (d) => { window.__complete = { spool: d.spool, timings: d.timings, points: d.points, blocks: d.blocks }; });
      });
      await page.waitForFunction(() => !document.getElementById("convert-result").hidden, null, { timeout: 12 * 60 * 1000, polling: 250 });
      wallEnd = Date.now();
      await page.evaluate(async () => { clearInterval(window.__poller); await window.__sample(); });
      const ui = await page.evaluate(() => ({
        notice: document.getElementById("convert-result").textContent,
        noticeClass: document.getElementById("convert-result").className,
        warning: document.getElementById("convert-warning").hidden ? null : document.getElementById("convert-warning").textContent,
        details: document.getElementById("convert-details").hidden ? null : document.getElementById("convert-details").textContent,
        complete: window.__complete,
        trace: window.__trace,
      }));
      const trace = ui.trace;
      rec.result = {
        ms: wallEnd - wallStart, uiNotice: ui.notice, noticeClass: ui.noticeClass, warning: ui.warning, details: ui.details,
        workerTimings: ui.complete?.timings, spool: ui.complete?.spool, points: ui.complete?.points, blocks: ui.complete?.blocks,
        usagePeak: trace.some((r) => r.usage != null) ? Math.max(...trace.map((r) => r.usage ?? 0)) : null,
        baselineUsage: trace[0]?.usage ?? null,
        zonesPeak: trace.some((r) => r.zones != null) ? Math.max(...trace.map((r) => r.zones ?? 0)) : null,
        zonesAtEnd: trace[trace.length - 1]?.zones ?? null, opfsAvailable: !trace.some((r) => r.noOpfs), samples: trace.length,
        persisted: await page.evaluate(() => navigator.storage.persisted?.()).catch(() => null),
      };
      out = rec.result;
      if (!cell.expectReject && ui.noticeClass.includes("success")) {
        const save = async (button) => {
          const [d] = await Promise.all([page.waitForEvent("download", { timeout: 300000 }), page.click(button)]);
          const p = path.join(dl, `${cell.id}-${button.slice(1)}`);
          await d.saveAs(p);
          const h = await sha(p);
          fs.rmSync(p, { force: true });
          return h;
        };
        rec.oct = await save("#convert-download");
        rec.attrs = await save("#convert-download-attributes");
      }
    }
    sampler.kill();
    rec.pss = pssWindow(samples, wallStart, wallEnd + 500);
    rec.wallMs = wallEnd - wallStart;
    if (cell.ref) {
      const exp = R1[cell.ref];
      rec.shaMatch = rec.oct === exp.oct && rec.attrs === exp.attrs;
    }
    if (cell.nat) {
      rec.nativeMs = NATIVE_MS[cell.nat];
      rec.ratioPage = out.ms / rec.nativeMs;
      rec.ratioWorker = out.workerTimings?.totalMs ? out.workerTimings.totalMs / rec.nativeMs : null;
      rec.over4x = rec.ratioPage > 4;
    }
    if (logs.length) rec.consoleProblems = logs.slice(0, 10);
  } catch (e) {
    rec.runnerError = String(e);
  } finally {
    rec.hostAfter = load();
    try { sampler?.kill(); } catch {}
    try { await ctx?.close(); } catch {}
    try { server?.kill(); } catch {}
    fs.rmSync(profile, { recursive: true, force: true });
  }
  emit(rec);
  return rec;
}

const dl = path.join(repo, "target/m3b/dl");
fs.mkdirSync(dl, { recursive: true });
const stopped = new Set();
for (const cell of CELLS) {
  if (only.length && !only.includes(cell.id)) continue;
  if (stopped.has(cell.browser)) { emit({ cell: cell.id, skipped: "stop rule on this browser" }); continue; }
  for (let w = 0; w < 24 && Number(load().loadavg.split(" ")[0]) >= 1.5; w++) await sleep(5000);
  const rec = await runCell(cell, dl);
  if (rec.shaMatch === false || rec.over4x || (rec.runnerError && !cell.expectReject)) stopped.add(cell.browser);
}
