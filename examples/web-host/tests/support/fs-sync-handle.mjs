// Node stand-in for FileSystemSyncAccessHandle backed by a sparse file, so tests
// can use offsets beyond 2^32 without allocating that much disk.
import fs from "node:fs";

export function openFsSyncHandle(path, { quotaBytes = Infinity, onQuota = "short" } = {}) {
    const fd = fs.openSync(path, "w+");
    return {
        write(view, { at = 0 } = {}) {
            let length = view.byteLength;
            if (at + length > quotaBytes) {
                if (onQuota === "throw") throw new DOMException("quota exceeded", "QuotaExceededError");
                length = Math.max(0, Math.min(length, quotaBytes - at));
            }
            let done = 0;
            while (done < length) {
                done += fs.writeSync(fd, view, done, length - done, at + done);
            }
            return done;
        },
        read(view, { at = 0 } = {}) {
            let done = 0;
            while (done < view.byteLength) {
                const n = fs.readSync(fd, view, done, view.byteLength - done, at + done);
                if (n === 0) break;
                done += n;
            }
            return done;
        },
        getSize: () => fs.fstatSync(fd).size,
        truncate: (length) => fs.ftruncateSync(fd, length),
        flush: () => fs.fsyncSync(fd),
        close: () => fs.closeSync(fd),
    };
}
