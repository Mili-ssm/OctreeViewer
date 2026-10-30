// Driver of the OPFS zones measurement (Task 1 / S5 residual, 30-10-2026). Throwaway.
// Usage (from the repository root):
//   node docs/records/benchmarks/data/opfs-zones-2026-10-30-driver.mjs [cell ...] [--out=file.jsonl] [--rounds=3] [--tests=big,term,ram] [--gib=6] [--ramgib=4] [--trials=10]
// Tests: big, small, churn, term, ram, truncq (added after the first full pass, run on its own with --tests=truncq). A cell is "<chrome|firefox>:<http|file>"; no cells = chrome:http firefox:http firefox:file.
// Every run (cell x round x test [x segment size]) launches a fresh browser with a persistent
// profile under <repo>/target/s5-profiles (never tmpfs), serves HTTP with python3 -m http.server on
// 127.0.0.1, and deletes the profile in `finally`. At every parked step of the page it takes
// `df` (avail/used of /home) and `du -sk` of the profile after 2 s and after 12 s, plus the median
// PSS of the browser process tree (examples/web-host/sample-process-memory.py, 0.25 s) of the
// samples taken >= 0.6 s after the step. Records loadavg and /proc/pressure/cpu before and after.
// Aborts a run if /home has < 60 GiB free. Needs playwright-core (PLAYWRIGHT_CORE) and, for Firefox,
// the Playwright build at FIREFOX_BIN.
import { createRequire } from "module";
import { spawn, execSync } from "child_process";
import fs from "fs";
import path from "path";

const core = process.env.PLAYWRIGHT_CORE ?? "/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/";
const FIREFOX_BIN = process.env.FIREFOX_BIN ?? `${process.env.HOME}/.cache/ms-playwright/firefox-1555/firefox/firefox`;
const { chromium, firefox } = createRequire(core)("playwright-core");

const repo = process.cwd();
const args = process.argv.slice(2);
const opt = (k, d) => (args.find((a) => a.startsWith(`--${k}=`)) ?? `--${k}=${d}`).split("=")[1];
const outFile = opt("out", "");
const rounds = Number(opt("rounds", 3));
const tests = opt("tests", "big,small,churn,term,ram").split(",");
const bigGiB = Number(opt("gib", 6));
const ramGiB = Number(opt("ramgib", 4));
const trials = Number(opt("trials", 10));
const cells = args.filter((a) => !a.startsWith("--"));
const dataDir = "docs/records/benchmarks/data";
const port = 8796;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const emit = (rec) => {
  const line = JSON.stringify(rec);
  console.log(line);
  if (outFile) fs.appendFileSync(outFile, line + "\n");
};
const median = (v) => { const s = [...v].sort((a, b) => a - b); return s[Math.floor(s.length / 2)]; };
const freeKiB = () => Number(execSync("df -k --output=avail /home | tail -1").toString().trim());
const disk = (profile) => {
  const [used, avail] = execSync("df -k --output=used,avail /home | tail -1").toString().trim().split(/\s+/).map(Number);
  return { usedKiB: used, availKiB: avail, profileDuKiB: Number(execSync(`du -sk "${profile}" | cut -f1`).toString().trim()) };
};
const load = () => ({ loadavg: fs.readFileSync("/proc/loadavg", "utf8").trim(), cpuPressure: fs.readFileSync("/proc/pressure/cpu", "utf8").trim().split("\n")[0] });

function browserRootPid(profile) {
  const pids = execSync(`pgrep -f -- "${profile}" || true`).toString().split("\n").filter(Boolean).map(Number);
  const set = new Set(pids);
  for (const pid of pids) {
    const ppid = Number(fs.readFileSync(`/proc/${pid}/stat`, "utf8").replace(/^.*\) /, "").split(" ")[1]);
    if (!set.has(ppid)) return pid;
  }
  throw new Error("browser root pid not found");
}

function readSamples(file, sinceMs) {
  const rows = [];
  for (const line of fs.readFileSync(file, "utf8").split("\n")) {
    if (!line) continue;
    const r = JSON.parse(line);
    if (r.wall_time_ns / 1e6 >= sinceMs && r.root_alive && r.enumerated_processes_readable && r.tree_bytes?.Pss != null) rows.push(r);
  }
  return rows;
}

async function runOnce(cell, round, run) {
  const [browserName, origin] = cell.split(":");
  const rec = { cell, browser: browserName, origin, round, test: run.test, query: run.query, steps: [], hostBefore: load() };
  // Isolation: wait (<= 150 s) until the 1-min loadavg is < 2 so that timing rounds are not run under load.
  for (let w = 0; w < 30 && Number(load().loadavg.split(" ")[0]) >= 2; w++) { await sleep(5000); rec.waitedForLoadS = (w + 1) * 5; }
  rec.hostBefore = load();
  if (freeKiB() / 2 ** 20 < 60) { rec.runnerError = "aborted: free space < 60 GiB"; emit(rec); return; }
  fs.mkdirSync(path.join(repo, "target/s5-profiles"), { recursive: true });
  fs.mkdirSync(path.join(repo, "target/s5-samples"), { recursive: true });
  const profile = fs.mkdtempSync(path.join(repo, "target/s5-profiles/profile-"));
  const samples = path.join(repo, `target/s5-samples/${path.basename(profile)}.jsonl`);
  let server = null, ctx = null, sampler = null;
  try {
    rec.profileFs = execSync(`findmnt -T "${profile}" -no FSTYPE,TARGET`).toString().trim();
    if (rec.profileFs.startsWith("tmpfs")) throw new Error("profile on tmpfs");
    if (origin === "http") {
      server = spawn("python3", ["-m", "http.server", String(port), "--bind", "127.0.0.1"], { cwd: repo, stdio: "ignore" });
      await sleep(800);
    }
    ctx = browserName === "chrome"
      ? await chromium.launchPersistentContext(profile, { executablePath: "/usr/bin/google-chrome", headless: true })
      : await firefox.launchPersistentContext(profile, { executablePath: FIREFOX_BIN, headless: true });
    rec.version = ctx.browser()?.version?.() ?? null;
    rec.diskStart = disk(profile);
    if (run.test !== "term") {
      const pid = browserRootPid(profile);
      sampler = spawn("python3", ["examples/web-host/sample-process-memory.py", String(pid), samples, "--interval", "0.25", "--seconds", "1500"], { cwd: repo, stdio: "ignore" });
      await sleep(1500);
    }
    const page = await ctx.newPage();
    const base = origin === "http" ? `http://127.0.0.1:${port}/${dataDir}/` : `file://${repo}/${dataDir}/`;
    await page.goto(`${base}opfs-zones-2026-10-30.html?${run.query}`);
    let seen = 0, prevAt = Date.now();
    const t0 = Date.now();
    for (;;) {
      const st = await page.evaluate(() => ({ s: window.zState, done: Boolean(window.zResult) }));
      if (st.done) { rec.result = await page.evaluate(() => window.zResult); break; }
      if (st.s.seq > seen) {
        seen = st.s.seq;
        const step = { label: st.s.label, retained: st.s.retained ?? null, size: st.s.size ?? null, estimate: st.s.estimate, failure: st.s.failure ?? null, allocMs: st.s.allocMs ?? null };
        await sleep(2000);
        step.disk2s = disk(profile);
        const rows = sampler ? readSamples(samples, st.s.at + 600) : [];
        const pss = rows.map((r) => r.tree_bytes.Pss);
        step.pssSamples = pss.length;
        const sinceRows = readSamples(samples, prevAt).filter((r) => r.wall_time_ns / 1e6 <= st.s.at);
        step.pssPeakSincePrevStep = sinceRows.length ? Math.max(...sinceRows.map((r) => r.tree_bytes.Pss)) : null;
        prevAt = st.s.at;
        if (st.s.label === "released") { await sleep(8000); const r2 = readSamples(samples, Date.now() - 2000).map((r) => r.tree_bytes.Pss); step.pssBytes8sLater = r2.length ? median(r2) : null; }
        step.pssBytes = pss.length ? median(pss) : null;
        step.pssMin = pss.length ? Math.min(...pss) : null;
        step.pssMax = pss.length ? Math.max(...pss) : null;
        step.rssBytes = pss.length ? median(rows.map((r) => r.tree_bytes.Rss)) : null;
        if ((run.test === "big" || run.test === "truncq") && /^trunc|removed|written|closed/.test(step.label)) {
          await sleep(10000);
          step.disk12s = disk(profile);
          step.estimate12s = await page.evaluate(async () => { const e = await navigator.storage.estimate(); return { usage: e.usage, quota: e.quota }; });
        }
        rec.steps.push(step);
        await page.evaluate(() => window.zGo());
      }
      if (Date.now() - t0 > 40 * 60 * 1000) throw new Error("run timeout");
      await sleep(100);
    }
  } catch (e) {
    rec.runnerError = String(e);
  } finally {
    try { if (sampler) sampler.kill(); } catch {}
    try { if (ctx) await ctx.close(); } catch {}
    if (server) server.kill();
    fs.rmSync(profile, { recursive: true, force: true });
    fs.rmSync(samples, { force: true });
    await sleep(1500);
    rec.profileRemoved = !fs.existsSync(profile);
    rec.hostAfter = load();
    rec.freeAfterGiB = Math.floor(freeKiB() / 2 ** 20);
  }
  emit(rec);
}

const plan = [];
if (tests.includes("truncq")) plan.push({ test: "truncq", query: "test=truncq&gib=2" });
if (tests.includes("small")) plan.push({ test: "small", query: "test=small&mib=1024" });
if (tests.includes("churn")) for (const pool of [0, 1]) plan.push({ test: "churn", query: `test=churn&seg=262144&pool=${pool}&gib=8` });
if (tests.includes("big")) plan.push({ test: "big", query: `test=big&gib=${bigGiB}` });
if (tests.includes("term")) plan.push({ test: "term", query: `test=term&trials=${trials}` });
if (tests.includes("ram")) for (const seg of [32768, 262144, 1048576]) plan.push({ test: "ram", query: `test=ram&seg=${seg}&gib=${ramGiB}` });
for (let round = 1; round <= rounds; round++) {
  for (const cell of cells.length ? cells : ["chrome:http", "firefox:http", "firefox:file"]) {
    for (const run of plan) await runOnce(cell, round, run);
  }
}
