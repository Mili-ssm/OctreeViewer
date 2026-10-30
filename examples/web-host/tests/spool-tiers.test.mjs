import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { createMemorySyncHandle, createSpoolHost, removeWithRetry } from "../spool-tiers.mjs";
import { openFsSyncHandle } from "./support/fs-sync-handle.mjs";

const KIB = 1024;

function withHandle(options, body) {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "spool-tiers-"));
    const handle = openFsSyncHandle(path.join(dir, "zone"), options);
    try {
        return body(handle);
    } finally {
        handle.close();
        fs.rmSync(dir, { recursive: true, force: true });
    }
}

test("pages round trip at offsets beyond 2^32", () => {
    withHandle({}, (handle) => {
        const host = createSpoolHost({ zones: [handle] });
        const at = 5 * 2 ** 30 + 123;
        assert.equal(host.zoneCount, 1);
        assert.equal(host.pageWrite(0, at, Uint8Array.of(1, 2, 3, 4)), 4);
        const out = new Uint8Array(4);
        assert.equal(host.pageRead(0, at, out), 4);
        assert.deepEqual([...out], [1, 2, 3, 4]);
        assert.equal(handle.getSize(), at + 4);
        assert.equal(host.stats().pageWriteCalls, 1);
        assert.equal(host.stats().pageReadCalls, 1);
    });
});

test("short and throwing quota writes return the byte count", () => {
    withHandle({ quotaBytes: 10 }, (handle) => {
        const host = createSpoolHost({ zones: [handle] });
        assert.equal(host.pageWrite(0, 8, new Uint8Array(6)), 2);
        assert.equal(host.pageWrite(0, 20, new Uint8Array(6)), 0);
    });
    withHandle({ quotaBytes: 10, onQuota: "throw" }, (handle) => {
        const host = createSpoolHost({ zones: [handle] });
        assert.equal(host.pageWrite(0, 8, new Uint8Array(6)), 0);
        assert.equal(host.pageWrite(0, 0, new Uint8Array(6)), 6);
    });
});

test("zoneRelease truncates to zero", () => {
    withHandle({}, (handle) => {
        const host = createSpoolHost({ zones: [handle] });
        host.pageWrite(0, 0, new Uint8Array(100));
        host.zoneRelease(0);
        assert.equal(handle.getSize(), 0);
        assert.equal(host.stats().zoneTruncates, 1);
    });
});

test("RAM pool reuses the same buffer per capacity class", () => {
    const host = createSpoolHost();
    host.ramAlloc(1, 40 * KIB); // rounds up to 64 KiB
    host.ramWrite(1, 0, Uint8Array.of(9));
    host.ramFree(1);
    assert.equal(host.stats().ramBytes, 0);
    assert.equal(host.stats().pooledBytes, 64 * KIB);
    host.ramAlloc(2, 64 * KIB);
    assert.equal(host.stats().pooledBytes, 0);
    assert.equal(host.stats().ramPeakBytes, 64 * KIB);
    // Pooled buffers are not cleared, so the surviving byte proves identity reuse.
    const out = new Uint8Array(1);
    host.ramRead(2, 0, out);
    assert.equal(out[0], 9);
    host.ramFree(2);
    host.dispose();
    assert.equal(host.stats().pooledBytes, 0);
});

test("shrink copies the prefix and pools the old class", () => {
    const host = createSpoolHost();
    host.ramAlloc(1, 128 * KIB);
    host.ramWrite(1, 0, Uint8Array.of(1, 2, 3));
    host.ramWrite(1, 32 * KIB - 1, Uint8Array.of(8));
    host.ramShrink(1, 32 * KIB);
    assert.equal(host.stats().ramBytes, 32 * KIB);
    assert.equal(host.stats().pooledBytes, 128 * KIB);
    const out = new Uint8Array(3);
    host.ramRead(1, 0, out);
    assert.deepEqual([...out], [1, 2, 3]);
    const last = new Uint8Array(1);
    host.ramRead(1, 32 * KIB - 1, last);
    assert.equal(last[0], 8);
    assert.throws(() => host.ramRead(1, 32 * KIB, last), RangeError);
});

test("views are copied, never retained", () => {
    const host = createSpoolHost();
    host.ramAlloc(1, 32 * KIB);
    const source = Uint8Array.of(1, 2, 3);
    host.ramWrite(1, 0, source);
    source.fill(0xff);
    const out = new Uint8Array(3);
    host.ramRead(1, 0, out);
    assert.deepEqual([...out], [1, 2, 3]);
    out.fill(0xee);
    const again = new Uint8Array(3);
    host.ramRead(1, 0, again);
    assert.deepEqual([...again], [1, 2, 3]);
});

test("allocation failure carries a useful message", () => {
    const host = createSpoolHost();
    assert.throws(() => host.ramAlloc(1, 2 ** 60), /could not be allocated/);
});

test("memory sync handle round trips and exposes a Blob", async () => {
    const handle = createMemorySyncHandle();
    assert.equal(handle.write(Uint8Array.of(1, 2, 3), { at: 2 }), 3);
    assert.equal(handle.getSize(), 5);
    const out = new Uint8Array(8);
    assert.equal(handle.read(out, { at: 0 }), 5);
    assert.deepEqual([...out.subarray(0, 5)], [0, 0, 1, 2, 3]);
    handle.truncate(0);
    assert.equal(handle.getSize(), 0);
    handle.write(Uint8Array.of(4, 5), { at: 0 });
    handle.flush();
    const blob = handle.toBlob();
    assert.equal(blob.size, 2);
    assert.deepEqual([...new Uint8Array(await blob.arrayBuffer())], [4, 5]);
});

function fakeDirectory(failures, errorName = "NoModificationAllowedError") {
    const calls = [];
    return {
        calls,
        async removeEntry(name) {
            calls.push(name);
            if (calls.length <= failures) throw new DOMException("busy", errorName);
        },
    };
}

test("removeWithRetry retries until the handle is released", async () => {
    const dir = fakeDirectory(3);
    const left = await removeWithRetry(dir, ["a"], { intervalMs: 1, timeoutMs: 1000 });
    assert.deepEqual(left, []);
    assert.equal(dir.calls.length, 4);
});

test("removeWithRetry reports names it could not remove and treats NotFound as removed", async () => {
    const stuck = fakeDirectory(Infinity, "InvalidStateError");
    assert.deepEqual(await removeWithRetry(stuck, ["a", "b"], { intervalMs: 1, timeoutMs: 20 }), ["a", "b"]);
    const missing = fakeDirectory(Infinity, "NotFoundError");
    assert.deepEqual(await removeWithRetry(missing, ["a"], { intervalMs: 1, timeoutMs: 20 }), []);
    assert.equal(missing.calls.length, 1);
});
