// Dedicated Firefox BiDi instance on 9222, repository HTTP server on 8767; Node 22+ and Python 3.
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const [output, widthArg, heightArg, mode = "clear", framesArg = "300"] = process.argv.slice(2);
const width = Number(widthArg), height = Number(heightArg), frames = Number(framesArg);
if (!output || ![width, height, frames].every(Number.isInteger)
    || width < 1 || height < 1 || width > 2048 || height > 2048 || frames < 1 || frames > 600
    || !["clear", "viewer", "orbit"].includes(mode)) {
    throw new Error("Usage: node probe-surface-memory.mjs NEW_OUTPUT_DIR WIDTH HEIGHT [clear|viewer|orbit] [FRAMES=300]");
}
fs.mkdirSync(path.dirname(output), { recursive: true });
fs.mkdirSync(output); // Refuse to mix measurement series.
const save = (name, value) => fs.writeFileSync(path.join(output, `${name}.json`), JSON.stringify(value, null, 2) + "\n", { flag: "wx" });
const sampler = fileURLToPath(new URL("./sample-process-memory.py", import.meta.url));
const socket = new WebSocket("ws://127.0.0.1:9222/session");
const pending = new Map();
const errors = [];
let nextId = 0;
socket.onmessage = ({ data }) => {
    const message = JSON.parse(data), request = pending.get(message.id);
    if (message.method === "log.entryAdded" && message.params.level === "error") errors.push(message.params);
    if (!request) return;
    clearTimeout(request.timer);
    pending.delete(message.id);
    if (message.type === "error") request.reject(new Error(JSON.stringify(message)));
    else request.resolve(message.result);
};
await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
function send(method, params) {
    return new Promise((resolve, reject) => {
        const id = ++nextId;
        const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timeout`)); }, 30_000);
        pending.set(id, { resolve, reject, timer });
        socket.send(JSON.stringify({ id, method, params }));
    });
}
let context, rootPid, starttime;
async function snapshot() {
    const result = await send("script.evaluate", {
        expression: "JSON.stringify(window.surfaceMemoryResults)", target: { context }, awaitPromise: true,
    });
    if (result.type !== "success") throw new Error(JSON.stringify(result));
    return JSON.parse(result.result.value);
}
function capture(phase, result) {
    save(phase, result);
    // Blocking helper completion orders the mapping read before idle sleep or screenshot.
    const captured = JSON.parse(execFileSync("python3", ["-c", `
import json, runpy, sys
from pathlib import Path
m = runpy.run_path(sys.argv[1])
print(json.dumps(m["capture_root_smaps"](int(sys.argv[2]), int(sys.argv[3]), Path(sys.argv[4]))))
`, sampler, String(rootPid), starttime, path.join(output, `${phase}.smaps.txt`)], { encoding: "utf8" }));
    save(`${phase}.capture`, captured);
    if (captured.outcome !== "captured") throw new Error(JSON.stringify(captured));
    console.log(JSON.stringify({ phase, rootPid, capture: captured }));
}
let ownsSession = false;
try {
    const session = await send("session.new", { capabilities: {} });
    ownsSession = true;
    await send("session.subscribe", { events: ["log.entryAdded"] });
    save("session", session);
    rootPid = session.capabilities["moz:processID"];
    if (!Number.isSafeInteger(rootPid) || rootPid <= 0) throw new Error("Firefox parent PID unavailable");
    starttime = execFileSync("python3", ["-c", `
import runpy, sys
from pathlib import Path
m = runpy.run_path(sys.argv[1])
print(m["identity"](Path("/proc") / sys.argv[2])[1])
`, sampler, String(rootPid)], { encoding: "utf8" }).trim();
    save("root", { pid: rootPid, starttime });
    context = (await send("browsingContext.getTree", {})).contexts[0].context;
    await send("browsingContext.setViewport", { context, viewport: { width: 1280, height: 900 }, devicePixelRatio: 1 });
    const url = new URL("http://127.0.0.1:8767/examples/web-host/tests/surface-memory.html");
    url.search = new URLSearchParams({ width, height, frames, mode }).toString();
    await send("browsingContext.navigate", { context, url: url.href, wait: "complete" });
    let result;
    const deadline = Date.now() + 90_000;
    while (Date.now() < deadline) {
        result = await snapshot();
        if (result?.state === "failed") throw new Error(result.message);
        if (result?.state === "idle") break;
        await new Promise(resolve => setTimeout(resolve, 200));
    }
    if (result?.state !== "idle" || result.submitted !== frames) throw new Error("Frame sequence did not complete");
    capture("submitted", result);
    await new Promise(resolve => setTimeout(resolve, 20_000));
    result = await snapshot();
    if (result.state !== "idle" || result.submitted !== frames) throw new Error("Device did not remain idle");
    capture("settled", result);
    const screenshotStartedAtUnixMs = Date.now();
    const shot = await send("browsingContext.captureScreenshot", { context, origin: "viewport" });
    fs.writeFileSync(path.join(output, "final.png"), Buffer.from(shot.data, "base64"), { flag: "wx" });
    save("screenshot", { startedAtUnixMs: screenshotStartedAtUnixMs, finishedAtUnixMs: Date.now() });
    if (errors.length) throw new Error(`${errors.length} browser console errors`);
} finally {
    save("console-errors", errors);
    if (ownsSession) await send("browser.close", {}).catch(() => {});
    socket.close();
}
