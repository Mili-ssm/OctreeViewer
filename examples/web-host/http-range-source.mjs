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
// - Network errors, 429 and 5xx are retried with backoff and jitter (`retryDelays`, `Retry-After`
//   honoured up to 10 s); every attempt has a `timeoutMs` limit that counts as a retryable failure.
//   Other 4xx, a changed file and aborts are final. Counters live in `stats`.
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
  #timeoutMs;
  #retryDelays;
  #random;
  #cached = [];
  #pending = [];
  #scheduled = false;
  size = 0;
  etag = null;
  /**
   * `requests` counts HTTP attempts sent; `bytes` the body bytes received; `retries` the attempts
   * repeated after a retryable failure; `timeouts` the attempts cut by `timeoutMs`; `aborted` the
   * attempts cancelled through the signal, `abortedBytes` the bytes they had asked for.
   */
  stats = { requests: 0, bytes: 0, cachedReads: 0, retries: 0, timeouts: 0, aborted: 0, abortedBytes: 0 };

  constructor(url, {
    fetchImpl = globalThis.fetch.bind(globalThis),
    maxGap = 0,
    signal,
    timeoutMs = 15_000,
    retryDelays = [200, 800, 2400],
    random = Math.random,
  } = {}) {
    this.#timeoutMs = timeoutMs;
    this.#retryDelays = retryDelays;
    this.#random = random;
    this.#url = url;
    this.#fetch = fetchImpl;
    this.#maxGap = maxGap;
    this.#signal = signal;
  }

  resetStats() {
    for (const key of Object.keys(this.stats)) this.stats[key] = 0;
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
  // Network errors, 429, 5xx and per-attempt timeouts are retried with backoff; the rest is final.
  async #request(range, expected) {
    const bytesHint = expected?.end !== undefined ? expected.end - expected.start : 0;
    for (let attempt = 0; ; attempt += 1) {
      try {
        return await this.#attempt(range, expected, bytesHint);
      } catch (error) {
        if (this.#signal?.aborted) throw abortError(this.#signal);
        if (!error.retryable || attempt >= this.#retryDelays.length) throw error;
        this.stats.retries += 1;
        const hinted = Math.min(error.retryAfterMs ?? 0, MAX_RETRY_AFTER_MS);
        const backoff = this.#retryDelays[attempt] * (0.75 + 0.5 * this.#random());
        await this.#wait(Math.max(backoff, hinted));
      }
    }
  }

  #wait(ms) {
    return new Promise((resolve, reject) => {
      const signal = this.#signal;
      const onAbort = () => {
        clearTimeout(timer);
        reject(abortError(signal));
      };
      const timer = setTimeout(() => {
        signal?.removeEventListener("abort", onAbort);
        resolve();
      }, ms);
      signal?.addEventListener("abort", onAbort, { once: true });
    });
  }

  async #attempt(range, expected, bytesHint) {
    const controller = new AbortController();
    const forward = () => controller.abort(this.#signal.reason);
    this.#signal?.addEventListener("abort", forward, { once: true });
    if (this.#signal?.aborted) forward();
    let timedOut = false;
    const timer = this.#timeoutMs > 0
      ? setTimeout(() => {
        timedOut = true;
        controller.abort();
      }, this.#timeoutMs)
      : null;
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
        const error = new HttpRangeError(`${this.#url}: HTTP ${response.status} for ${range}`);
        if (response.status === 429 || response.status >= 500) {
          error.retryable = true;
          error.retryAfterMs = parseRetryAfter(response.headers.get("Retry-After"));
        }
        throw error;
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
      let body;
      try {
        body = await response.arrayBuffer();
      } catch (error) {
        if (!controller.signal.aborted) error.retryable = true;
        throw error;
      }
      const bytes = new Uint8Array(body);
      if (bytes.length !== contentRange.end - contentRange.start) {
        throw new HttpRangeError(`${this.#url}: short body for ${range} (${bytes.length} B)`);
      }
      this.stats.bytes += bytes.length;
      return { bytes, range: contentRange, etag };
    } catch (error) {
      if (this.#signal?.aborted) {
        this.stats.aborted += 1;
        this.stats.abortedBytes += bytesHint;
      } else if (timedOut) {
        this.stats.timeouts += 1;
        const timeout = new HttpRangeError(`${this.#url}: timeout after ${this.#timeoutMs} ms for ${range}`);
        timeout.retryable = true;
        throw timeout;
      } else if (error?.retryable === undefined && !(error instanceof HttpRangeError)) {
        error.retryable = true; // fetch rejected: network error
      }
      throw error;
    } finally {
      clearTimeout(timer);
      this.#signal?.removeEventListener("abort", forward);
    }
  }
}

const MAX_RETRY_AFTER_MS = 10_000;

const abortError = (signal) =>
  signal?.reason?.name === "AbortError" ? signal.reason : new DOMException("The read was aborted", "AbortError");

const parseRetryAfter = (header) => {
  if (!header) return undefined;
  const seconds = Number(header);
  if (Number.isFinite(seconds)) return Math.max(0, seconds * 1000);
  const date = Date.parse(header);
  return Number.isNaN(date) ? undefined : Math.max(0, date - Date.now());
};

/** `{ url, tailBytes?, headBytes? }` becomes an `HttpRangeSource`; anything else (a `Blob`) is returned as is. */
export async function resolveSource(spec, options = {}) {
  if (typeof spec?.url !== "string" || typeof spec.slice === "function") return spec;
  return HttpRangeSource.open(spec.url, { ...options, ...spec });
}

/** Sums the `stats` of the range sources among `sources` (Blobs are skipped); null when there is none. */
export function sumRangeStats(sources) {
  const found = sources.filter((source) => source instanceof HttpRangeSource);
  if (found.length === 0) return null;
  const total = {};
  for (const { stats } of found) for (const [key, value] of Object.entries(stats)) total[key] = (total[key] ?? 0) + value;
  return total;
}
