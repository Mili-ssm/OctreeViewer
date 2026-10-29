// Driver of the M3a browser parity run (29-10-2026). Throwaway.
// Usage (from the repository root, after `cargo build --release -p viewer_wasm --target
// wasm32-unknown-unknown` and `wasm-bindgen --target web --out-dir pkg ...`):
//   node docs/records/benchmarks/data/m3a-parity-2026-10-29-driver.mjs [--out=file.jsonl]
// For Chrome and Firefox it opens m3a-parity-2026-10-29.html over HTTP (python3 -m http.server on
// 127.0.0.1), converts with engine "core" in the page's worker, reads the OPFS outputs back and
// compares their SHA-256 with the CLI's (computed here with the native `native_convert` binary
// of tools/convert_parity, built with --release). It also cancels one conversion and reconverts
// to the same name. Needs PLAYWRIGHT_CORE (playwright-core directory) and, for Firefox, FIREFOX_BIN.
// Profiles live under <repo>/target/m3a-profiles and are deleted in `finally`.
import { createRequire } from "module";
import { spawn, execFileSync } from "child_process";
import { createHash } from "crypto";
import fs from "fs";
import path from "path";

const core = process.env.PLAYWRIGHT_CORE ?? "/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/";
const FIREFOX_BIN = process.env.FIREFOX_BIN ?? `${process.env.HOME}/.cache/ms-playwright/firefox-1555/firefox/firefox`;
const { chromium, firefox } = createRequire(core)("playwright-core");

const repo = process.cwd();
const outFile = (process.argv.find((a) => a.startsWith("--out=")) ?? "--out=").split("=")[1];
const port = 8795;
const page_ = "docs/records/benchmarks/data/m3a-parity-2026-10-29.html";
const emit = (rec) => {
  const line = JSON.stringify(rec);
  console.log(line);
  if (outFile) fs.appendFileSync(outFile, line + "\n");
};

const small = "data/samples/fs_e430_n4008_18.las";
const synthetic = "tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz";
const CASES = [
  { name: "small-default", file: small, ppb: 262144, divisions: 2, options: {} },
  { name: "synthetic-4096-d8", file: synthetic, ppb: 4096, divisions: 8, options: { pointsPerBlock: 4096, spatialDivisions: 8 } },
];

const sha = (file) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
function nativeSha(c) {
  const out = path.join(repo, "target/convert_parity", `m3a-${c.name}.oct`);
  fs.rmSync(out, { force: true });
  fs.rmSync(`${out}attrs`, { force: true });
  execFileSync(path.join(repo, "target/release/native_convert"), [c.file, out, String(c.ppb), String(c.divisions)]);
  return { oct: sha(out), octattrs: sha(`${out}attrs`) };
}

async function runBrowser(browserName) {
  fs.mkdirSync(path.join(repo, "target/m3a-profiles"), { recursive: true });
  const profile = fs.mkdtempSync(path.join(repo, "target/m3a-profiles/profile-"));
  const server = spawn("python3", ["-m", "http.server", String(port), "--bind", "127.0.0.1"], { cwd: repo, stdio: "ignore" });
  let ctx = null;
  try {
    await new Promise((r) => setTimeout(r, 800));
    ctx = browserName === "chrome"
      ? await chromium.launchPersistentContext(profile, { executablePath: "/usr/bin/google-chrome", headless: true })
      : await firefox.launchPersistentContext(profile, { executablePath: FIREFOX_BIN, headless: true });
    const version = ctx.browser()?.version?.() ?? null;
    const page = await ctx.newPage();
    const logs = [];
    page.on("console", (m) => { if (["error", "warning"].includes(m.type())) logs.push(`${m.type()}: ${m.text()}`); });
    page.on("pageerror", (e) => logs.push(`pageerror: ${e}`));
    await page.goto(`http://127.0.0.1:${port}/${page_}`);
    await page.waitForFunction(() => window.parityReady, null, { timeout: 60000 });
    const url = (f) => `/${f}`;
    for (const c of CASES) {
      const expected = nativeSha(c);
      const rec = { browser: browserName, version, case: c.name, expected };
      try {
        rec.result = await page.evaluate(([u, n, o]) => window.convertCase(u, n, o), [url(c.file), path.basename(c.file), c.options]);
        rec.match = rec.result.oct === expected.oct && rec.result.octattrs === expected.octattrs;
      } catch (e) {
        rec.runnerError = String(e);
      }
      emit(rec);
    }
    // Cancel, then reconvert the same file (same output name) and compare again.
    const c = CASES[1];
    const rec = { browser: browserName, version, case: `${c.name}-cancel-then-reconvert`, expected: nativeSha(c) };
    try {
      const args = [url(c.file), path.basename(c.file), c.options];
      rec.cancel = await page.evaluate((a) => window.cancelCase(...a), args);
      rec.result = await page.evaluate((a) => window.convertCase(...a), args);
      rec.match = rec.result.oct === rec.expected.oct && rec.result.octattrs === rec.expected.octattrs;
      rec.partialsAfterReconvert = await page.evaluate(() => window.partialNames());
    } catch (e) {
      rec.runnerError = String(e);
    }
    emit(rec);
    if (logs.length) emit({ browser: browserName, consoleProblems: logs });
  } finally {
    try { await ctx?.close(); } catch {}
    server.kill();
    fs.rmSync(profile, { recursive: true, force: true });
  }
}

for (const b of ["chrome", "firefox"]) {
  try { await runBrowser(b); } catch (e) { emit({ browser: b, runnerError: String(e) }); }
}
