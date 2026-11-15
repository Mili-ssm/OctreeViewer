// Driver of the D3c web Belvedere run (15-11-2026), adapted from the M3b driver
// (docs/records/benchmarks/data/m3b-belvedere-2026-10-30-driver.mjs). Throwaway.
// Usage (repo root, fresh pkg/): node <this file> --out=runs.jsonl [--rounds=4]
// One fresh Chrome headless + profile under target/d3c-web per run, own http.server on 127.0.0.1:8798,
// Belvedere through <input type=file>, engine "core", options formatVersion v5|v7 (no pointsPerBlock /
// spatialDivisions), spoolBudgetBytes 2 GiB, ramBudget 4 GB. Peak PSS of the browser tree sampled every 0.5 s
// with examples/web-host/sample-process-memory.py. Interleaved: v5,v7 on odd rounds, v7,v5 on even rounds.
import { createRequire } from "module";
import { spawn, execSync } from "child_process";
import { createHash } from "crypto";
import fs from "fs";
import path from "path";

const core = process.env.PLAYWRIGHT_CORE ?? "/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/";
const { chromium } = createRequire(core)("playwright-core");
const repo = process.cwd();
const args = process.argv.slice(2);
const opt = (k, d) => (args.find((a) => a.startsWith(`--${k}=`)) ?? `--${k}=${d}`).split("=")[1];
const outFile = opt("out", "");
const rounds = Number(opt("rounds", 4));
const port = 8798;
const GiB = 2 ** 30;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const emit = (rec) => { const l = JSON.stringify(rec); console.log(l); if (outFile) fs.appendFileSync(outFile, l + "\n"); };
const sha = (file) => new Promise((res, rej) => {
  const h = createHash("sha256");
  fs.createReadStream(file).on("data", (c) => h.update(c)).on("end", () => res(h.digest("hex"))).on("error", rej);
});
const BELV = "/home/mili/Documentos/Mili-ssm.github.io/Private/Rust/octree_viewer_v3/data/datasets/belv_2009_digital-aerial_pcd_copc.laz";
const PAGE = `http://127.0.0.1:${port}/docs/records/benchmarks/data/d3c-web-belvedere-15-11-2026/page.html`;
const load = () => ({ loadavg: fs.readFileSync("/proc/loadavg", "utf8").trim(), cpuPressure: fs.readFileSync("/proc/pressure/cpu", "utf8").split("\n")[0] });

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

async function runCell(variant, round, dl) {
  const id = `${variant}-r${round}`;
  const rec = { cell: id, variant, round };
  for (let w = 0; w < 120 && Number(load().loadavg.split(" ")[0]) >= 1.5; w++) await sleep(5000);
  rec.hostBefore = load();
  fs.mkdirSync(path.join(repo, "target/d3c-web/profiles"), { recursive: true });
  const profile = fs.mkdtempSync(path.join(repo, "target/d3c-web/profiles/profile-"));
  const samples = path.join(repo, `target/d3c-web/${id}-pss.jsonl`);
  fs.rmSync(samples, { force: true });
  let server = null, ctx = null, sampler = null;
  try {
    server = spawn("python3", ["-m", "http.server", String(port), "--bind", "127.0.0.1"], { cwd: repo, stdio: "ignore" });
    await sleep(800);
    ctx = await chromium.launchPersistentContext(profile, { executablePath: "/usr/bin/google-chrome", headless: true, acceptDownloads: true });
    rec.version = ctx.browser()?.version?.() ?? null;
    const pid = browserRootPid(profile);
    sampler = spawn("python3", ["examples/web-host/sample-process-memory.py", String(pid), samples, "--interval", "0.5", "--seconds", "1500"], { cwd: repo, stdio: "ignore" });
    await sleep(1500);
    const page = ctx.pages()[0] ?? (await ctx.newPage());
    const logs = [];
    page.on("console", (m) => { if (["error", "warning"].includes(m.type())) logs.push(`${m.type()}: ${m.text().slice(0, 300)}`); });
    page.on("pageerror", (e) => logs.push(`pageerror: ${e}`));
    await page.goto(PAGE);
    await page.waitForFunction(() => window.ready, null, { timeout: 60000 });
    await page.setInputFiles("#f", BELV);
    const options = { formatVersion: variant, spoolBudgetBytes: 2 * GiB, ramBudget: 4e9 };
    rec.options = options;
    const out = await page.evaluate((o) => window.convertCase(o), options);
    rec.result = out;
    rec.leftoverAfter = await page.evaluate(() => window.leftover());
    if (!out.error) {
      const save = async (which) => {
        const [d] = await Promise.all([page.waitForEvent("download", { timeout: 300000 }), page.evaluate((w) => window.downloadOf(w), which)]);
        const p = path.join(dl, `${id}-${which}`);
        await d.saveAs(p);
        const h = await sha(p);
        fs.rmSync(p, { force: true });
        return h;
      };
      rec.oct = await save("oct");
      rec.attrs = await save("attrs");
    }
    sampler.kill();
    rec.pss = pssWindow(samples, out.wallStart, out.wallEnd + 500);
    rec.wallMs = out.ms;
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
}

const dl = path.join(repo, "target/d3c-web/dl");
fs.mkdirSync(dl, { recursive: true });
for (let r = 1; r <= rounds; r++) {
  for (const v of r % 2 ? ["v5", "v7"] : ["v7", "v5"]) await runCell(v, r, dl);
}
