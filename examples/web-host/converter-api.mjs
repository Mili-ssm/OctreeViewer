/** Creates a local LAS/LAZ converter backed by a dedicated worker and OPFS. */
export function createConverter(options = {}) {
    if (typeof Worker === "undefined") throw new Error("Web Workers are required");
    const workerUrl = options.workerUrl
        ?? globalThis.__CONVERTER_WORKER_URL__
        ?? new URL("./converter-worker.js", import.meta.url);
    const worker = new Worker(workerUrl, {
        type: "module",
    });
    const listeners = new Map();
    let active = null;
    let disposed = false;

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
    worker.addEventListener("message", ({ data }) => {
        if (data.type === "progress" || data.type === "status") emit(data.type, data);
        else if (data.type === "complete") {
            emit("complete", data);
            settle(null, {
                file: data.file,
                name: data.outputName,
                attributesFile: data.attributesFile,
                outputAttributesName: data.outputAttributesName,
                points: data.points,
                timings: data.timings,
            });
        } else if (data.type === "cancelled") {
            const error = new Error(data.message);
            error.name = "AbortError";
            emit("cancelled", error);
            settle(error);
        } else if (data.type === "error") {
            const error = new Error(data.message);
            emit("error", error);
            settle(error);
        }
    });
    const onWorkerError = (event) => {
        const error = new Error(event.message || "converter worker failed");
        emit("error", error);
        settle(error);
    };
    worker.addEventListener("error", onWorkerError);
    worker.addEventListener("messageerror", onWorkerError);

    return {
        convert(file) {
            if (disposed) return Promise.reject(new Error("converter is disposed"));
            if (active) return Promise.reject(new Error("conversion is already in progress"));
            if (!(file instanceof Blob) || !/\.(las|laz)$/i.test(file.name ?? "")) {
                return Promise.reject(new TypeError("file must be a .las or .laz Blob"));
            }
            const result = new Promise((resolve, reject) => { active = { resolve, reject }; });
            try {
                worker.postMessage({ type: "convert", file });
            } catch (error) {
                settle(error);
            }
            return result;
        },
        cancel() {
            if (!active || disposed) return false;
            worker.postMessage({ type: "cancel" });
            return true;
        },
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
            worker.terminate();
            settle(new Error("converter was disposed"));
            listeners.clear();
        },
    };
}
