import init, { WasmLasOctConverter } from "../../pkg/viewer_wasm.js";

let cancelRequested = false;

self.addEventListener("message", ({ data }) => {
    if (data?.type === "cancel") {
        cancelRequested = true;
    } else if (data?.type === "convert") {
        cancelRequested = false;
        void convert(data.file);
    }
});

async function convert(file) {
    let accessHandle = null;
    let converter = null;
    let directory = null;
    let temporaryName = null;
    try {
        if (!file || !/\.la[sz]$/i.test(file.name)) {
            throw new Error("Choose a .las or .laz file.");
        }
        if (!navigator.storage?.getDirectory) {
            throw new Error("This browser does not support the Origin Private File System (OPFS).");
        }
        await init();
        const root = await navigator.storage.getDirectory();
        directory = await root.getDirectoryHandle("octree-viewer-conversions", { create: true });
        temporaryName = `conversion-${crypto.randomUUID()}.partial`;
        const output = await directory.getFileHandle(temporaryName, { create: true });
        accessHandle = await output.createSyncAccessHandle();
        accessHandle.truncate(0);
        converter = new WasmLasOctConverter(file, accessHandle);
        self.postMessage({ type: "status", message: `Converting ${file.name} locally…` });

        let progress;
        let lastProgressAt = performance.now();
        do {
            if (cancelRequested) throw new DOMException("Conversion cancelled.", "AbortError");
            progress = converter.convertNext();
            const now = performance.now();
            if (progress.done || now - lastProgressAt >= 100) {
                self.postMessage({ type: "progress", ...progress, sourceName: file.name });
                lastProgressAt = now;
            }
            if (!progress.done) await new Promise((resolve) => setTimeout(resolve, 0));
        } while (!progress.done);

        if (cancelRequested) throw new DOMException("Conversion cancelled.", "AbortError");
        converter.finish();
        converter.free();
        converter = null;
        accessHandle.flush();
        accessHandle.close();
        accessHandle = null;
        const convertedFile = await output.getFile();
        self.postMessage({
            type: "complete",
            file: convertedFile,
            outputName: file.name.replace(/\.(las|laz)$/i, ".oct"),
            points: progress.totalPoints,
        });
    } catch (error) {
        converter?.free();
        converter = null;
        try {
            accessHandle?.close();
        } catch {
            // The handle may already have been closed by a failed finalization.
        }
        accessHandle = null;
        self.postMessage({
            type: error?.name === "AbortError" ? "cancelled" : "error",
            message: error instanceof Error ? error.message : String(error),
        });
        if (temporaryName && directory) {
            try {
                await directory.removeEntry(temporaryName);
            } catch {
                // A browser may keep a failed OPFS entry locked briefly after close.
            }
        }
    } finally {
        converter?.free();
        try {
            accessHandle?.close();
        } catch {
            // The handle may already have been closed after a successful conversion.
        }
    }
}
