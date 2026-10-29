// Driver of the OPFS per-spool-files measurement (Task 5 / S9, 29-10-2026). Throwaway.
// Usage (from the repository root):
//   node docs/records/benchmarks/data/opfs-spool-files-2026-10-29-driver.mjs [cell ...] [--out=file.jsonl] [--total=7500]
// A cell is "<chrome|firefox>:<http|file>"; no cells = chrome:http firefox:http firefox:file.
// Per cell: S1 page (single arena, N x 1 GiB) and S9 page (d2/d8 x N 4/8 x block 32 KiB/1 MiB), in
// the same browser session, one run per combination. Needs playwright-core (PLAYWRIGHT_CORE env
// var = its directory) and, for Firefox, the Playwright build at FIREFOX_BIN. Profiles live under
// <repo>/target/s9-profiles (never tmpfs); each cell records `findmnt -T` and `df` before/after,
// serves HTTP with python3 -m http.server on 127.0.0.1, and in `finally` closes the browser, kills
// the server and deletes the profile. Aborts a run if /home has < 100 GiB free.
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
const totalMiB = Number(opt("total", 7500));
const cells = args.filter((a) => !a.startsWith("--"));
const dataDir = "docs/records/benchmarks/data";
const port = 8794;
const freeKiB = () => Number(execSync("df -k --output=avail /home | tail -1").toString().trim());
const emit = (rec) => {
  const line = JSON.stringify(rec);
  console.log(line);
  if (outFile) fs.appendFileSync(outFile, line + "\n");
};

const RUNS = [];
for (const n of [4, 8]) for (const block of [32768, 1048576]) RUNS.push({ kind: "s1", n, block, query: `n=${n}&block=${block}&mib=1024`, page: "opfs-arena-spike-2026-10-29.html", prop: "spikeResult" });
for (const scenario of ["d2", "d8"]) for (const n of [4, 8]) for (const block of [32768, 1048576]) RUNS.push({ kind: "s9", scenario, n, block, query: `scenario=${scenario}&n=${n}&block=${block}&totalMiB=${totalMiB}`, page: "opfs-spool-files-2026-10-29.html", prop: "s9Result" });

async function runCell(cell) {
  const [browserName, origin] = cell.split(":");
  const meta = { cell, browser: browserName, origin };
  fs.mkdirSync(path.join(repo, "target/s9-profiles"), { recursive: true });
  const profile = fs.mkdtempSync(path.join(repo, "target/s9-profiles/profile-"));
  let server = null;
  let ctx = null;
  try {
    meta.profileFs = execSync(`findmnt -T "${profile}" -no FSTYPE,TARGET`).toString().trim();
    if (meta.profileFs.startsWith("tmpfs")) throw new Error("profile on tmpfs");
    if (origin === "http") {
      server = spawn("python3", ["-m", "http.server", String(port), "--bind", "127.0.0.1"], { cwd: repo, stdio: "ignore" });
      await new Promise((r) => setTimeout(r, 800));
    }
    ctx = browserName === "chrome"
      ? await chromium.launchPersistentContext(profile, { executablePath: "/usr/bin/google-chrome", headless: true })
      : await firefox.launchPersistentContext(profile, { executablePath: FIREFOX_BIN, headless: true });
    meta.version = ctx.browser()?.version?.() ?? null;
    meta.freeBeforeKiB = freeKiB();
    emit({ ...meta, event: "cell-start" });
    for (const run of RUNS) {
      const rec = { ...meta, ...run, freeBeforeGiB: Math.floor(freeKiB() / 2 ** 20) };
      delete rec.event;
      if (freeKiB() / 2 ** 20 < 100) { rec.runnerError = "aborted: free space < 100 GiB"; emit(rec); continue; }
      let page = null;
      try {
        page = await ctx.newPage();
        const base = origin === "http" ? `http://127.0.0.1:${port}/${dataDir}/` : `file://${repo}/${dataDir}/`;
        await page.goto(`${base}${run.page}?${run.query}`);
        await page.waitForFunction((p) => window[p], run.prop, { timeout: 10 * 60 * 1000 });
        rec.result = await page.evaluate((p) => window[p], run.prop);
      } catch (e) {
        rec.runnerError = String(e);
      } finally {
        try { if (page) await page.close(); } catch {}
      }
      rec.freeAfterGiB = Math.floor(freeKiB() / 2 ** 20);
      emit(rec);
    }
  } catch (e) {
    meta.runnerError = String(e);
    emit(meta);
  } finally {
    try { if (ctx) await ctx.close(); } catch {}
    if (server) server.kill();
    fs.rmSync(profile, { recursive: true, force: true });
    await new Promise((r) => setTimeout(r, 1500));
    emit({ ...meta, event: "cell-end", profileRemoved: !fs.existsSync(profile), freeAfterKiB: freeKiB(), freeAfterGiB: Math.floor(freeKiB() / 2 ** 20) });
  }
}

for (const cell of cells.length ? cells : ["chrome:http", "firefox:http", "firefox:file"]) await runCell(cell);
