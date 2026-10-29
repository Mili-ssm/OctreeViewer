// Driver of the OPFS quota measurement (Task 3 / S8, 29-10-2026). Throwaway.
// Usage (from the repository root):
//   node docs/records/benchmarks/data/opfs-quota-2026-10-29-driver.mjs [cell ...] [--cap=40] [--out=file.jsonl]
// A cell is "<chrome|firefox>:<http|file>:<none|grant|deny>"; no cells = the whole matrix of the record.
// Needs playwright-core (PLAYWRIGHT_CORE env var = its directory) and, for Firefox, the Playwright
// build at FIREFOX_BIN. Profiles live under <repo>/target/q3-profiles (never tmpfs); each cell
// creates one, records `findmnt -T` and `df` before/after, serves HTTP with python3 -m http.server on
// 127.0.0.1, and in `finally` closes the browser, kills the server and deletes the profile.
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
const capGiB = Number(opt("cap", 40));
const outFile = opt("out", "");
const MATRIX = [
  "chrome:http:none", "chrome:http:grant", "chrome:http:deny", "chrome:file:none",
  "firefox:http:none", "firefox:http:deny", "firefox:http:grant",
  "firefox:file:none", "firefox:file:deny", "firefox:file:grant",
];
const cells = args.filter((a) => !a.startsWith("--"));
const pagePath = "docs/records/benchmarks/data/opfs-quota-2026-10-29.html";
const port = 8793;
const freeKiB = () => Number(execSync("df -k --output=avail /home | tail -1").toString().trim());

async function runCell(cell) {
  const [browserName, origin, persistMode] = cell.split(":");
  const record = { browser: browserName, origin, persistMode, capGiB };
  const freeBefore = freeKiB();
  record.freeBeforeKiB = freeBefore;
  record.freeBeforeGiB = Math.floor(freeBefore / 2 ** 20);
  // Safety: never let free space drop under 100 GiB because of the cap.
  if (freeBefore / 2 ** 20 < 100 + capGiB) { record.runnerError = "aborted: free space too low"; return record; }
  fs.mkdirSync(path.join(repo, "target/q3-profiles"), { recursive: true });
  const profile = fs.mkdtempSync(path.join(repo, "target/q3-profiles/profile-"));
  let server = null;
  let ctx = null;
  try {
    record.profileFs = execSync(`findmnt -T "${profile}" -no FSTYPE,TARGET`).toString().trim();
    if (record.profileFs.startsWith("tmpfs")) throw new Error("profile on tmpfs");
    let url;
    if (origin === "http") {
      server = spawn("python3", ["-m", "http.server", String(port), "--bind", "127.0.0.1"], { cwd: repo, stdio: "ignore" });
      await new Promise((r) => setTimeout(r, 800));
      url = `http://127.0.0.1:${port}/${pagePath}`;
    } else url = `file://${repo}/${pagePath}`;
    url += `?persist=${persistMode === "none" ? 0 : 1}&capGiB=${capGiB}`;

    if (browserName === "chrome") {
      ctx = await chromium.launchPersistentContext(profile, { executablePath: "/usr/bin/google-chrome", headless: true });
    } else {
      // persist() grant/deny without a dialog: Firefox test prefs.
      const prefs = persistMode === "none" ? {} : {
        "dom.storageManager.prompt.testing": true,
        "dom.storageManager.prompt.testing.allow": persistMode === "grant",
      };
      ctx = await firefox.launchPersistentContext(profile, { executablePath: FIREFOX_BIN, headless: true, firefoxUserPrefs: prefs });
    }
    record.version = ctx.browser()?.version?.() ?? null;
    const page = ctx.pages()[0] ?? (await ctx.newPage());
    if (browserName === "chrome" && origin === "http" && persistMode !== "none") {
      const cdp = await ctx.newCDPSession(page);
      await cdp.send("Browser.setPermission", {
        origin: new URL(url).origin, permission: { name: "persistent-storage" },
        setting: persistMode === "grant" ? "granted" : "denied",
      });
    }
    await page.goto(url);
    await page.waitForFunction(() => window.quotaResult, null, { timeout: 20 * 60 * 1000 });
    record.result = await page.evaluate(() => window.quotaResult);
  } catch (e) {
    record.runnerError = String(e);
  } finally {
    try { if (ctx) await ctx.close(); } catch {}
    if (server) server.kill();
    fs.rmSync(profile, { recursive: true, force: true });
    record.profileRemoved = !fs.existsSync(profile);
    await new Promise((r) => setTimeout(r, 1500));
    record.freeAfterKiB = freeKiB();
    record.freeAfterGiB = Math.floor(record.freeAfterKiB / 2 ** 20);
    const q = record.result?.estimateBefore?.quota;
    if (q) record.quotaFracOfFreeBefore = q / (freeBefore * 1024);
  }
  return record;
}

for (const cell of cells.length ? cells : MATRIX) {
  const line = JSON.stringify(await runCell(cell));
  console.log(line);
  if (outFile) fs.appendFileSync(outFile, line + "\n");
}
