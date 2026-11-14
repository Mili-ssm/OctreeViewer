import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
const sleep = ms => new Promise(r => setTimeout(r, ms));
const LEAF_CASE = { eye: [621, 619, 155], direction: [0, 1, 0] };

function orbitStepPose(pose) {
    const angle = 35 * Math.PI / 180;
    const [px, py, pz] = pose.position;
    const [tx, ty, tz] = pose.target;
    const dx = px - tx, dy = py - ty;
    return {
        position: [tx + dx * Math.cos(angle) - dy * Math.sin(angle), ty + dx * Math.sin(angle) + dy * Math.cos(angle), pz],
        target: [tx, ty, tz],
    };
}
const jumpPose = pose => ({
    position: pose.target.map((t, axis) => t + 0.1 * (pose.position[axis] - t)),
    target: pose.target,
});
function leafPose(metadata) {
    const side = Math.max(...metadata.boundsMax.map((v, axis) => v - metadata.boundsMin[axis]));
    const eye = LEAF_CASE.eye.map((v, axis) => metadata.coordinateOrigin[axis] + v);
    return { position: eye, target: eye.map((v, axis) => v + side / 16 * LEAF_CASE.direction[axis]) };
}


const { chromium } = createRequire("/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/")("playwright-core");
const instance = await chromium.launch({ executablePath: "/usr/bin/google-chrome", headless: true,
  args: ["--headless=new","--enable-unsafe-webgpu","--enable-features=Vulkan","--use-angle=vulkan","--enable-dawn-features=allow_unsafe_apis","--enable-webgpu-developer-features"] });
const out = { poses: {}, console: [] };
try {
  const page = await (await instance.newContext({ viewport: { width: 1688, height: 768 }, deviceScaleFactor: 1 })).newPage();
  page.on("console", m => { if (["error","warning"].includes(m.type())) out.console.push(m.text()); });
  page.on("pageerror", e => out.console.push(String(e)));
  const ev = e => page.evaluate(e);
  const api = (op, extra) => ev(`(async()=>JSON.stringify(await window.__renderBench.apiCall(${JSON.stringify(op)}${extra?`,${JSON.stringify(extra)}`:""}),(k,v)=>ArrayBuffer.isView(v)?Array.from(v):v))()`).then(JSON.parse);
  await page.goto("http://127.0.0.1:8767/examples/web-host/index.html?cells=1&buildWorker=1", { waitUntil: "load" });
  while (await ev('document.querySelector("#dataset-file").disabled')) await sleep(300);
  await ev(`document.querySelector("canvas").style.width="1282px";document.querySelector("canvas").style.height="722px";window.dispatchEvent(new Event("resize"));window.__renderBench.start();true`);
  await sleep(500);
  await page.setInputFiles("#dataset-file", path.resolve("target/render-data/majadas-v75.oct"));
  const t0 = Date.now();
  while (!(await ev("window.__renderBench.latestLodCounters()"))) { if (Date.now()-t0>90000) throw new Error("open timeout"); await sleep(200); }
  await ev(`(()=>{const s=document.querySelector("#px-mode");s.value="dynamic";s.dispatchEvent(new Event("change"));return true})()`);
  const fitted = await api("getCamera"); const metadata = await api("getMetadata");
  for (const name of ["leaf","fitted"]) {
    const pose = name === "fitted" ? fitted : leafPose(metadata);
    await api("setCamera", { position: pose.position, target: pose.target });
    const trace = []; const start = Date.now();
    while (Date.now() - start < 20000) {
      await ev("window.__renderBench.wake(); true");
      const s = await api("getStats");
      trace.push({ t: Date.now()-start, px: s.px, lodTarget: s.lod?.targetVoxelPixels, stage: s.lod?.lodStagePixels, gpuP95: s.gpu?.gpuFrameMs?.p95 ?? null, n: s.gpu?.gpuFrameMs?.samples ?? null });
      await sleep(1000);
    }
    out.poses[name] = trace;
  }
  out.statsLine = await ev('document.querySelector("#stats").textContent.split("\\n").find(l=>l.startsWith("finest"))');
} finally { await instance.close(); }
fs.writeFileSync("target/r3-b4/trace.json", JSON.stringify(out, null, 1));
console.log(JSON.stringify(Object.fromEntries(Object.entries(out.poses).map(([k,v])=>[k,v.map(x=>`${x.t}:${x.px.targetPx}/${x.px.signalSource}/${x.px.signalMs?.toFixed?.(1)}`).join(" ")])), null, 1), out.statsLine, out.console.slice(0,5));
