// Driver of the hybrid-spool RAM measurement (Task 2 / S10, 29-10-2026). Throwaway.
// Usage (from the repository root):
//   node docs/records/benchmarks/data/hybrid-spool-ram-2026-10-29-driver.mjs [cell ...] [--out=file.jsonl] [--rounds=3]
// A cell is "<chrome|firefox>:<seg>" with seg in bytes; no cells = both browsers x 32768 and 1048576.
// Each run launches a fresh browser (persistent profile under <repo>/target/s10-profiles, never
// tmpfs), serves HTTP with python3 -m http.server on 127.0.0.1, and for every parked step of the
// page samples the browser process tree with examples/web-host/sample-process-memory.py (PSS,
// 0.25 s interval): the median of the samples taken >= 0.6 s after the step. Needs playwright-core
// (PLAYWRIGHT_CORE env var = its directory) and, for Firefox, the Playwright build at FIREFOX_BIN.
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
const cells = args.filter((a) => !a.startsWith("--"));
const dataDir = "docs/records/benchmarks/data";
const port = 8795;
const steps = "256,512,1024,2048";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const emit = (rec) => {
  const line = JSON.stringify(rec);
  console.log(line);
  if (outFile) fs.appendFileSync(outFile, line + "\n");
};
const median = (v) => { const s = [...v].sort((a, b) => a - b); return s[Math.floor(s.length / 2)]; };

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

const load = () => ({ loadavg: fs.readFileSync("/proc/loadavg", "utf8").trim(), cpuPressure: fs.readFileSync("/proc/pressure/cpu", "utf8").trim() });

async function runOnce(browserName, seg, round) {
  const rec = { browser: browserName, seg, round, steps: [], hostBefore: load() };
  fs.mkdirSync(path.join(repo, "target/s10-profiles"), { recursive: true });
  fs.mkdirSync(path.join(repo, "target/s10-samples"), { recursive: true });
  const profile = fs.mkdtempSync(path.join(repo, "target/s10-profiles/profile-"));
  const samples = path.join(repo, `target/s10-samples/${path.basename(profile)}.jsonl`);
  let server = null, ctx = null, sampler = null;
  try {
    rec.profileFs = execSync(`findmnt -T "${profile}" -no FSTYPE,TARGET`).toString().trim();
    if (rec.profileFs.startsWith("tmpfs")) throw new Error("profile on tmpfs");
    server = spawn("python3", ["-m", "http.server", String(port), "--bind", "127.0.0.1"], { cwd: repo, stdio: "ignore" });
    await sleep(800);
    ctx = browserName === "chrome"
      ? await chromium.launchPersistentContext(profile, { executablePath: "/usr/bin/google-chrome", headless: true })
      : await firefox.launchPersistentContext(profile, { executablePath: FIREFOX_BIN, headless: true });
    rec.version = ctx.browser()?.version?.() ?? null;
    const pid = browserRootPid(profile);
    sampler = spawn("python3", ["examples/web-host/sample-process-memory.py", String(pid), samples, "--interval", "0.25", "--seconds", "900"], { cwd: repo, stdio: "ignore" });
    await sleep(1500);
    const page = await ctx.newPage();
    await page.goto(`http://127.0.0.1:${port}/${dataDir}/hybrid-spool-ram-2026-10-29.html?seg=${seg}&steps=${steps}&xferMiB=256`);
    let seen = 0;
    for (;;) {
      const state = await page.evaluate(() => ({ s: window.s10State, done: Boolean(window.s10Result) }));
      if (state.done) { rec.result = await page.evaluate(() => window.s10Result); break; }
      if (state.s.seq > seen) {
        seen = state.s.seq;
        await sleep(2000);
        const rows = readSamples(samples, state.s.at + 600);
        const pss = rows.map((r) => r.tree_bytes.Pss);
        rec.steps.push({
          label: state.s.label, mode: state.s.mode ?? null, retained: state.s.retained, samples: pss.length,
          pssBytes: pss.length ? median(pss) : null, pssMin: pss.length ? Math.min(...pss) : null, pssMax: pss.length ? Math.max(...pss) : null,
          rssBytes: pss.length ? median(rows.map((r) => r.tree_bytes.Rss)) : null,
          pssAnonBytes: pss.length ? median(rows.map((r) => r.tree_bytes.Pss_Anon)) : null,
          processes: rows.length ? rows[rows.length - 1].enumerated_processes : null,
        });
        await page.evaluate(() => window.s10Go());
      }
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
    await sleep(1000);
    rec.profileRemoved = !fs.existsSync(profile);
    rec.hostAfter = load();
  }
  emit(rec);
}

const plan = cells.length ? cells : ["chrome:32768", "chrome:1048576", "firefox:32768", "firefox:1048576"];
for (let round = 1; round <= rounds; round++) {
  for (const cell of plan) {
    const [browserName, seg] = cell.split(":");
    await runOnce(browserName, Number(seg), round);
  }
}
