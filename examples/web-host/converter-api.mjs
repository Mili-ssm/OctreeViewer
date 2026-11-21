import { removeWithRetry } from "./spool-tiers.mjs";

const LEGACY_REMOVED = 'the legacy converter was removed; omit `engine` or use "core"';
// V8 is the only format and fixes these options: refused before anything reaches the worker.
const UNSUPPORTED_CONVERTER_OPTIONS = ["formatVersion", "rowOrder", "pointsPerBlock", "spatialDivisions"];
const PARTIAL_PATTERN = /^conversion-.*\.partial$/;

/**
 * Deletes `conversion-*.partial` files left by earlier sessions. Files held open by a live
 * conversion (another tab) refuse removal and are kept.
 */
async function removeOrphanPartials(storage, directoryName = "octree-viewer-conversions") {
    try {
        const root = await storage.getDirectory();
        const directory = await root.getDirectoryHandle(directoryName);
        const names = [];
        for await (const name of directory.keys()) if (PARTIAL_PATTERN.test(name)) names.push(name);
        await Promise.all(names.map((name) => directory.removeEntry(name).catch(() => {})));
    } catch {
        // No directory yet, or no OPFS: nothing to clean.
    }
}

// Chrome releases a terminated worker's sync access handles ~2 s later; names still locked after
// the retry window are left for the next start's orphan cleanup.
async function removePartials(storage, directoryName, names) {
    const directory = await (await storage.getDirectory()).getDirectoryHandle(directoryName);
    await removeWithRetry(directory, names);
}

/**
 * Creates a local LAS/LAZ converter backed by a dedicated worker and OPFS.
 * `options.converterOptions` are the converter options; they can be overridden per
 * `convert(file, { options })` call. `engine` is deprecated: "core" is accepted as a no-op and
 * "legacy" is rejected because that converter was removed.
 */
export function createConverter(options = {}) {
    if (typeof Worker === "undefined") throw new Error("Web Workers are required");
    const workerUrl = options.workerUrl
        ?? globalThis.__CONVERTER_WORKER_URL__
        ?? new URL("./converter-worker.js", import.meta.url);
    let worker = null;
    const listeners = new Map();
    let active = null;
    let disposed = false;
    let partial = null;
    const storage = options.storage ?? globalThis.navigator?.storage;
    // Sessions that were closed mid-conversion leave partial files behind.
    // `ready` settles once that cleanup is done; convert() waits for it so new files never race it.
    let readyDone = !storage?.getDirectory;
    const ready = readyDone
        ? Promise.resolve()
        : removeOrphanPartials(storage).then(() => { readyDone = true; });

    function emit(type, value) {
        for (const callback of listeners.get(type) ?? []) {
            try {
                callback(value);
            } catch (error) {
                globalThis.reportError?.(error);
            }
        }
    }
    function settle(error, value) {
        const request = active;
        if (!request) return;
        active = null;
        if (error) request.reject(error);
        else request.resolve(value);
    }
    // After a failure the WASM instance (e.g. post-trap) is untrusted: drop the worker, respawn lazily.
    function discardWorker() {
        const stale = worker;
        worker = null;
        stale?.terminate();
    }
    function onMessage(source, { data }) {
        if (source !== worker) return;
        if (data.type === "progress" || data.type === "status" || data.type === "warning") emit(data.type, data);
        else if (data.type === "partial-files") partial = { directory: data.directory, names: data.names };
        else if (data.type === "complete") {
            emit("complete", data);
            settle(null, {
                file: data.file,
                name: data.outputName,
                attributesFile: data.attributesFile,
                outputAttributesName: data.outputAttributesName,
                points: data.points,
                timings: data.timings,
                ...(data.spool === undefined ? {} : { spool: data.spool }),
            });
        } else if (data.type === "cancelled") {
            const error = new Error(data.message);
            error.name = "AbortError";
            emit("cancelled", error);
            settle(error);
        } else if (data.type === "error") {
            const error = new Error(data.message);
            discardWorker();
            emit("error", error);
            settle(error);
        }
    }
    function onWorkerError(source, event) {
        if (source !== worker) return;
        discardWorker();
        const error = new Error(event.message || "converter worker failed");
        emit("error", error);
        settle(error);
    }
    function ensureWorker() {
        if (worker) return worker;
        const created = new Worker(workerUrl, {
            type: "module",
        });
        worker = created;
        created.addEventListener("message", (event) => onMessage(created, event));
        created.addEventListener("error", (event) => onWorkerError(created, event));
        created.addEventListener("messageerror", (event) => onWorkerError(created, event));
        return created;
    }
    ensureWorker();

    return {
        convert(file, request = {}) {
            if (disposed) return Promise.reject(new Error("converter is disposed"));
            if (active) return Promise.reject(new Error("conversion is already in progress"));
            if (!(file instanceof Blob) || !/\.(las|laz)$/i.test(file.name ?? "")) {
                return Promise.reject(new TypeError("file must be a .las or .laz Blob"));
            }
            const entry = {};
            const result = new Promise((resolve, reject) => { entry.resolve = resolve; entry.reject = reject; });
            active = entry;
            const start = () => {
                try {
                    const engine = request.engine ?? options.engine ?? "core";
                    if (engine === "legacy") throw new TypeError(LEGACY_REMOVED);
                    if (engine !== "core") throw new TypeError(`unknown engine: ${engine}`);
                    const converterOptions = request.options ?? options.converterOptions ?? {};
                    for (const candidate of [options.converterOptions, request.options]) {
                        for (const name of UNSUPPORTED_CONVERTER_OPTIONS) {
                            if (candidate?.[name] !== undefined) {
                                throw new TypeError(`option ${name} was removed: V8 is the only format and fixes it`);
                            }
                        }
                    }
                    partial = null;
                    const message = {
                        type: "convert",
                        engine: "core",
                        file,
                        options: converterOptions,
                    };
                    entry.posted = true;
                    ensureWorker().postMessage(message);
                } catch (error) {
                    settle(error);
                }
            };
            if (readyDone) start();
            else ready.then(() => { if (active === entry && !disposed) start(); });
            return result;
        },
        cancel() {
            if (!active || disposed) return false;
            if (!active.posted) {
                // Still waiting for the orphan cleanup: nothing reached the worker yet.
                const error = new DOMException("Conversion cancelled.", "AbortError");
                emit("cancelled", error);
                settle(error);
                return true;
            }
            // The converter runs in one synchronous call that cannot observe a message: stop
            // the worker and delete its partial files; the next conversion respawns it.
            const files = partial;
            partial = null;
            discardWorker();
            const error = new DOMException("Conversion cancelled.", "AbortError");
            emit("cancelled", error);
            settle(error);
            if (files && storage?.getDirectory) {
                return removePartials(storage, files.directory, files.names).then(() => true, () => true);
            }
            return true;
        },
        ready,
        on(type, callback) {
            if (typeof callback !== "function") throw new TypeError("callback must be a function");
            const callbacks = listeners.get(type) ?? new Set();
            callbacks.add(callback);
            listeners.set(type, callbacks);
            return () => callbacks.delete(callback);
        },
        dispose() {
            if (disposed) return;
            disposed = true;
            discardWorker();
            settle(new Error("converter was disposed"));
            listeners.clear();
        },
    };
}
