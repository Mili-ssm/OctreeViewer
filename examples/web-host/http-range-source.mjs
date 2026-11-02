// Read-only Blob stand-in over HTTP Range (R5 P4, ADR 0023). `WasmAsyncFormatReader` only calls
// `.size` and `.slice(start, end).arrayBuffer()`, so this object can replace a `Blob` in a worker.
//
// - Opens with one suffix request (`Range: bytes=-tailBytes`) that also gives the file size and
//   the ETag; later reads inside that tail (footer, metadata block, root page of a v7.5 file) are
//   served from memory. `headBytes` adds a concurrent prefix request (the header of an
//   `.octattrs`), cached the same way.
// - Every later request carries `If-Range: <etag>`: a changed file answers 200, which is an error.
// - One single-range request per group: reads made in the same tick whose ranges touch or lie
//   within `maxGap` bytes are merged into one request (sibling pages are contiguous in the file).
//   Multi-range requests are never sent: S3 rejects them.
// - A 200 answer to a Range request means the server ignores Range: the body is aborted, not read.

export class HttpRangeError extends Error {
  constructor(message) {
    super(message);
    this.name = "HttpRangeError";
  }
}

const parseContentRange = (header) => {
  const match = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(header ?? "");
  return match ? { start: Number(match[1]), end: Number(match[2]) + 1, total: Number(match[3]) } : null;
};

export class HttpRangeSource {
  #url;
  #fetch;
  #maxGap;
  #signal;
  #cached = [];
  #pending = [];
  #scheduled = false;
  size = 0;
  etag = null;
  /** `requests` counts HTTP requests sent; `bytes` the body bytes received. */
  stats = { requests: 0, bytes: 0, cachedReads: 0 };

  constructor(url, { fetchImpl = globalThis.fetch.bind(globalThis), maxGap = 0, signal } = {}) {
    this.#url = url;
    this.#fetch = fetchImpl;
    this.#maxGap = maxGap;
    this.#signal = signal;
  }

  static async open(url, { tailBytes = 1024 * 1024, headBytes = 0, ...options } = {}) {
    const source = new HttpRangeSource(url, options);
    // The prefix is requested in parallel (one round trip for both); its ETag must match the tail's.
    const [tail, head] = await Promise.all([
      source.#request(`bytes=-${tailBytes}`, null),
      headBytes > 0 ? source.#request(`bytes=0-${headBytes - 1}`, { start: 0 }) : null,
    ]);
    if (head && head.etag !== tail.etag) throw new HttpRangeError(`${url}: the file changed while opening`);
    source.size = tail.range.total;
    source.etag = tail.etag;
    source.#cached.push({ start: tail.range.start, bytes: tail.bytes });
    if (head && tail.range.start > 0) source.#cached.push({ start: 0, bytes: head.bytes });
    return source;
  }

  slice(start, end) {
    const from = Math.max(0, Math.min(start ?? 0, this.size));
    const to = Math.max(from, Math.min(end ?? this.size, this.size));
    return { arrayBuffer: () => this.#read(from, to) };
  }

  async #read(start, end) {
    if (end === start) return new ArrayBuffer(0);
    for (const { start: base, bytes } of this.#cached) {
      if (start >= base && end <= base + bytes.length) {
        this.stats.cachedReads += 1;
        return bytes.slice(start - base, end - base).buffer;
      }
    }
    return new Promise((resolve, reject) => {
      this.#pending.push({ start, end, resolve, reject });
      if (!this.#scheduled) {
        this.#scheduled = true;
        queueMicrotask(() => this.#flush());
      }
    });
  }

  #flush() {
    const batch = this.#pending.sort((a, b) => a.start - b.start || a.end - b.end);
    this.#pending = [];
    this.#scheduled = false;
    for (let index = 0; index < batch.length; ) {
      const group = [batch[index]];
      let end = batch[index].end;
      for (index += 1; index < batch.length && batch[index].start <= end + this.#maxGap; index += 1) {
        group.push(batch[index]);
        end = Math.max(end, batch[index].end);
      }
      const start = group[0].start;
      this.#fetchRange(start, end).then(
        (bytes) => {
          for (const read of group) {
            read.resolve(bytes.slice(read.start - start, read.end - start).buffer);
          }
        },
        (error) => {
          for (const read of group) read.reject(error);
        },
      );
    }
  }

  async #fetchRange(start, end) {
    const { bytes } = await this.#request(`bytes=${start}-${end - 1}`, { start, end });
    return bytes;
  }

  // `expected` is null for the opening suffix request, whose range is not known beforehand.
  async #request(range, expected) {
    const controller = new AbortController();
    const forward = () => controller.abort(this.#signal.reason);
    this.#signal?.addEventListener("abort", forward, { once: true });
    const headers = { Range: range };
    if (this.etag) headers["If-Range"] = this.etag;
    this.stats.requests += 1;
    try {
      const response = await this.#fetch(this.#url, { headers, signal: controller.signal });
      if (response.status === 200) {
        controller.abort();
        throw new HttpRangeError(
          this.etag
            ? `${this.#url}: 200 for a Range request (the server ignores Range or the file changed)`
            : `${this.#url}: the server ignores Range (200 for ${range})`,
        );
      }
      if (response.status !== 206) {
        controller.abort();
        throw new HttpRangeError(`${this.#url}: HTTP ${response.status} for ${range}`);
      }
      const etag = response.headers.get("ETag");
      if (this.etag && etag && etag !== this.etag) {
        controller.abort();
        throw new HttpRangeError(`${this.#url}: the file changed (ETag ${etag}, expected ${this.etag})`);
      }
      const contentRange = parseContentRange(response.headers.get("Content-Range"));
      if (!contentRange) {
        controller.abort();
        throw new HttpRangeError(`${this.#url}: 206 without a valid Content-Range`);
      }
      if (
        (expected && (contentRange.start !== expected.start || (expected.end !== undefined && contentRange.end !== expected.end)))
        || (this.size && contentRange.total !== this.size)
      ) {
        controller.abort();
        throw new HttpRangeError(
          `${this.#url}: Content-Range ${response.headers.get("Content-Range")} does not match ${range}`,
        );
      }
      const bytes = new Uint8Array(await response.arrayBuffer());
      if (bytes.length !== contentRange.end - contentRange.start) {
        throw new HttpRangeError(`${this.#url}: short body for ${range} (${bytes.length} B)`);
      }
      this.stats.bytes += bytes.length;
      return { bytes, range: contentRange, etag };
    } finally {
      this.#signal?.removeEventListener("abort", forward);
    }
  }
}

/** `{ url, tailBytes?, headBytes? }` becomes an `HttpRangeSource`; anything else (a `Blob`) is returned as is. */
export async function resolveSource(spec, options = {}) {
  if (typeof spec?.url !== "string" || typeof spec.slice === "function") return spec;
  return HttpRangeSource.open(spec.url, { ...options, ...spec });
}
