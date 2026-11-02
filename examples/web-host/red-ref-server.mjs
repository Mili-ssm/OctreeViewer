// RED-REF harness server (docs/requirements.md, RED-REF: 100 Mbit/s, 50 ms RTT). Serves files
// with HTTP Range, CORS, a strong ETag + If-Range, and simulated latency and bandwidth, so the
// HTTP page/block source can be measured and tested without a real network.
//
//   node examples/web-host/red-ref-server.mjs ROOT [--port=8780] [--rtt-ms=50] [--mbps=100]
//        [--ignore-range]
//
// Latency: every response waits `rttMs` before its headers (one round trip per request, CORS
// preflights included). Bandwidth: all bodies share one link of `mbps`, so parallel requests
// split it. `--ignore-range` answers every GET with the full 200, like a server that does not
// implement Range; the client must detect that and abort. Multi-range requests are refused with
// 416 (S3 does not serve them either) and counted. GET /__stats returns the request log and
// POST /__reset clears it.
import { createServer } from "node:http";
import { open, stat } from "node:fs/promises";
import { resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const CHUNK = 32 * 1024;
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

/** Parses one `Range: bytes=...` header against `size`. Returns {start,end} (inclusive),
 * "multi", "unsatisfiable" or null (header not understood: serve the whole file). */
export function parseRange(header, size) {
  const match = /^bytes=(.*)$/.exec(header ?? "");
  if (!match) return null;
  if (match[1].includes(",")) return "multi";
  const single = /^(\d*)-(\d*)$/.exec(match[1].trim());
  if (!single || (single[1] === "" && single[2] === "")) return null;
  if (single[1] === "") {
    const suffix = Number(single[2]);
    if (suffix === 0) return "unsatisfiable";
    return { start: Math.max(0, size - suffix), end: size - 1 };
  }
  const start = Number(single[1]);
  if (start >= size) return "unsatisfiable";
  const end = single[2] === "" ? size - 1 : Math.min(Number(single[2]), size - 1);
  return end < start ? null : { start, end };
}

export async function startServer({ root, port = 0, rttMs = 0, mbps = 0, ignoreRange = false }) {
  const base = resolve(root);
  let log = [];
  let linkFreeAt = 0;
  // Reserves `bytes` on the shared link and resolves when they would have been delivered.
  const transmit = async (bytes) => {
    if (!mbps) return;
    const now = performance.now();
    linkFreeAt = Math.max(linkFreeAt, now) + (bytes * 8) / (mbps * 1000);
    await sleep(linkFreeAt - now);
  };
  const cors = {
    "Access-Control-Allow-Origin": "*",
    "Access-Control-Allow-Headers": "Range, If-Range",
    "Access-Control-Allow-Methods": "GET, HEAD, OPTIONS",
    "Access-Control-Expose-Headers": "Content-Range, Content-Length, ETag, Accept-Ranges",
    "Access-Control-Max-Age": "600",
  };

  const server = createServer(async (req, res) => {
    const url = new URL(req.url, "http://localhost");
    if (url.pathname === "/__stats") {
      res.writeHead(200, { ...cors, "Content-Type": "application/json" });
      res.end(JSON.stringify(log));
      return;
    }
    if (url.pathname === "/__reset") {
      log = [];
      res.writeHead(204, cors);
      res.end();
      return;
    }
    const entry = {
      method: req.method,
      path: url.pathname,
      range: req.headers.range ?? null,
      ifRange: req.headers["if-range"] ?? null,
      status: 0,
      bytes: 0,
      at: performance.now(),
    };
    log.push(entry);
    const finish = (status, headers, body) => {
      entry.status = status;
      res.writeHead(status, { ...cors, ...headers });
      res.end(body);
    };
    try {
      await sleep(rttMs);
      if (req.method === "OPTIONS") return finish(204, {});
      if (req.method !== "GET" && req.method !== "HEAD") return finish(405, {});
      const path = resolve(base, `.${decodeURIComponent(url.pathname)}`);
      if (path !== base && !path.startsWith(base + sep)) return finish(403, {});
      const info = await stat(path).catch(() => null);
      if (!info?.isFile()) return finish(404, {});
      const size = info.size;
      const etag = `"${size.toString(16)}-${Math.trunc(info.mtimeMs).toString(16)}"`;
      const common = { ETag: etag, "Accept-Ranges": ignoreRange ? "none" : "bytes" };
      let range = null;
      if (!ignoreRange && req.headers.range) {
        range = parseRange(req.headers.range, size);
        // A stale validator means the representation changed: answer with the whole new file.
        if (req.headers["if-range"] && req.headers["if-range"] !== etag) range = null;
        if (range === "multi") {
          entry.multiRange = true;
          return finish(416, { ...common, "Content-Range": `bytes */${size}` });
        }
        if (range === "unsatisfiable") return finish(416, { ...common, "Content-Range": `bytes */${size}` });
      }
      const [start, end] = range ? [range.start, range.end] : [0, size - 1];
      const length = size === 0 ? 0 : end - start + 1;
      const headers = {
        ...common,
        "Content-Type": "application/octet-stream",
        "Content-Length": String(length),
        ...(range ? { "Content-Range": `bytes ${start}-${end}/${size}` } : {}),
      };
      entry.status = range ? 206 : 200;
      res.writeHead(entry.status, { ...cors, ...headers });
      if (req.method === "HEAD" || length === 0) return res.end();
      const handle = await open(path, "r");
      try {
        const buffer = Buffer.allocUnsafe(Math.min(CHUNK, length));
        for (let sent = 0; sent < length && !res.destroyed; ) {
          const want = Math.min(CHUNK, length - sent);
          const { bytesRead } = await handle.read(buffer, 0, want, start + sent);
          await transmit(bytesRead);
          if (res.destroyed) break;
          res.write(Buffer.from(buffer.subarray(0, bytesRead)));
          sent += bytesRead;
          entry.bytes = sent;
        }
        res.end();
      } finally {
        await handle.close();
      }
    } catch (error) {
      if (!res.headersSent) finish(500, {}, String(error));
      else res.destroy();
    }
  });
  await new Promise((done) => server.listen(port, "127.0.0.1", done));
  return {
    port: server.address().port,
    get log() {
      return log;
    },
    reset: () => {
      log = [];
    },
    close: () => {
      server.closeAllConnections();
      return new Promise((done) => server.close(done));
    },
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const option = (name, fallback) => args.find((a) => a.startsWith(`--${name}=`))?.split("=")[1] ?? fallback;
  const root = args.find((a) => !a.startsWith("--"));
  if (!root) throw new Error("Usage: node red-ref-server.mjs ROOT [--port=8780] [--rtt-ms=50] [--mbps=100] [--ignore-range]");
  const running = await startServer({
    root,
    port: Number(option("port", "8780")),
    rttMs: Number(option("rtt-ms", "50")),
    mbps: Number(option("mbps", "100")),
    ignoreRange: args.includes("--ignore-range"),
  });
  console.log(`RED-REF server on http://127.0.0.1:${running.port}/ serving ${root}`);
}
