// Host side of the core converter's hybrid spool: pooled RAM segments and
// sync-access-handle zones. The wasm side passes `Uint8Array` views into its own
// memory; they are only valid during the call, so bytes are always copied and
// views are never retained.

const CLASS_BYTES = 32 * 1024;

const classOf = (capacity) => Math.max(1, Math.ceil(capacity / CLASS_BYTES)) * CLASS_BYTES;

export function createSpoolHost({ zones = [] } = {}) {
    const segments = new Map(); // key -> ArrayBuffer (its length is its capacity class)
    const pools = new Map(); // capacity -> ArrayBuffer[]
    const counters = { ramBytes: 0, ramPeakBytes: 0, pooledBytes: 0, pageWriteCalls: 0, pageReadCalls: 0, zoneTruncates: 0 };

    const take = (capacity) => {
        const pool = pools.get(capacity);
        if (pool?.length) {
            counters.pooledBytes -= capacity;
            return pool.pop();
        }
        try {
            return new ArrayBuffer(capacity);
        } catch (cause) {
            throw new Error(`spool RAM segment of ${capacity} bytes could not be allocated (live ${counters.ramBytes}, pooled ${counters.pooledBytes}): ${cause?.message ?? cause}`);
        }
    };
    const give = (buffer) => {
        const pool = pools.get(buffer.byteLength) ?? [];
        pool.push(buffer);
        pools.set(buffer.byteLength, pool);
        counters.pooledBytes += buffer.byteLength;
    };
    const segment = (key) => {
        const buffer = segments.get(key);
        if (!buffer) throw new Error(`spool RAM segment ${key} does not exist`);
        return buffer;
    };
    const span = (buffer, at, length) => {
        if (!(at >= 0) || at + length > buffer.byteLength) {
            throw new RangeError(`spool RAM access [${at}, ${at + length}) exceeds segment capacity ${buffer.byteLength}`);
        }
    };
    const track = (delta) => {
        counters.ramBytes += delta;
        counters.ramPeakBytes = Math.max(counters.ramPeakBytes, counters.ramBytes);
    };
    const zoneAt = (zone) => {
        const handle = zones[zone];
        if (!handle) throw new Error(`spill zone ${zone} does not exist`);
        return handle;
    };

    return {
        get zoneCount() {
            return zones.length;
        },
        ramAlloc(key, capacity) {
            if (segments.has(key)) throw new Error(`spool RAM segment ${key} already exists`);
            const buffer = take(classOf(capacity));
            segments.set(key, buffer);
            track(buffer.byteLength);
        },
        ramWrite(key, at, view) {
            const buffer = segment(key);
            span(buffer, at, view.byteLength);
            new Uint8Array(buffer).set(view, at);
        },
        ramRead(key, at, view) {
            const buffer = segment(key);
            span(buffer, at, view.byteLength);
            view.set(new Uint8Array(buffer, at, view.byteLength));
        },
        ramShrink(key, capacity) {
            const old = segment(key);
            const target = classOf(capacity);
            if (target >= old.byteLength) return;
            const next = take(target);
            new Uint8Array(next).set(new Uint8Array(old, 0, target));
            segments.set(key, next);
            give(old);
            track(target - old.byteLength);
        },
        ramFree(key) {
            const buffer = segment(key);
            segments.delete(key);
            give(buffer);
            track(-buffer.byteLength);
        },
        pageWrite(zone, at, view) {
            counters.pageWriteCalls += 1;
            try {
                return zoneAt(zone).write(view, { at });
            } catch (error) {
                if (error?.name === "QuotaExceededError") return 0;
                throw error;
            }
        },
        pageRead(zone, at, view) {
            counters.pageReadCalls += 1;
            return zoneAt(zone).read(view, { at });
        },
        zoneRelease(zone) {
            const handle = zoneAt(zone);
            handle.truncate(0);
            handle.flush();
            counters.zoneTruncates += 1;
        },
        stats() {
            return { ...counters };
        },
        dispose() {
            pools.clear();
            counters.pooledBytes = 0;
        },
    };
}

// In-RAM look-alike of FileSystemSyncAccessHandle (Chrome `file://` output, no OPFS).
export function createMemorySyncHandle() {
    let bytes = new Uint8Array(0);
    let size = 0;
    const reserve = (length) => {
        if (length <= bytes.length) return;
        const next = new Uint8Array(Math.max(length, bytes.length * 2));
        next.set(bytes.subarray(0, size));
        bytes = next;
    };
    return {
        write(view, { at = 0 } = {}) {
            reserve(at + view.byteLength);
            bytes.set(view, at);
            size = Math.max(size, at + view.byteLength);
            return view.byteLength;
        },
        read(view, { at = 0 } = {}) {
            const count = Math.max(0, Math.min(view.byteLength, size - at));
            view.set(bytes.subarray(at, at + count));
            return count;
        },
        getSize: () => size,
        truncate(length) {
            reserve(length);
            if (length < size) bytes.fill(0, length, size); // later growth must read zeros
            size = length;
        },
        flush() {},
        close() {},
        toBlob: (type = "application/octet-stream") => new Blob([bytes.slice(0, size)], { type }),
    };
}

const RETRYABLE = new Set(["NoModificationAllowedError", "InvalidStateError"]);

// Chrome releases sync handles ~2 s after `terminate()`, so removal may need to wait.
// Returns the names that could not be removed (left for next-start cleanup).
export async function removeWithRetry(directory, names, { intervalMs = 50, timeoutMs = 10_000 } = {}) {
    const deadline = Date.now() + timeoutMs;
    const left = [];
    for (const name of names) {
        for (;;) {
            try {
                await directory.removeEntry(name);
                break;
            } catch (error) {
                if (error?.name === "NotFoundError") break;
                if (!RETRYABLE.has(error?.name)) throw error;
                if (Date.now() >= deadline) {
                    left.push(name);
                    break;
                }
                await new Promise((resolve) => setTimeout(resolve, intervalMs));
            }
        }
    }
    return left;
}
