import init, { WasmLasOctConverter } from "../../pkg/viewer_wasm.js";

let cancelRequested = false;

self.addEventListener("message", ({ data }) => {
    if (data?.type === "cancel") {
        cancelRequested = true;
    } else if (data?.type === "convert") {
        cancelRequested = false;
        void convert(data.file, data.formatVersion ?? "v5");
    }
});

async function convert(file, formatVersion) {
    const conversionStartedAt = performance.now();
    let stage = "planning";
    let stageStartedAt = conversionStartedAt;
    const stageDurations = {};
    let accessHandle = null;
    let attributesAccessHandle = null;
    let converter = null;
    let directory = null;
    let temporaryName = null;
    let temporaryAttributesName = null;
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
        const conversionId = crypto.randomUUID();
        temporaryName = `conversion-${conversionId}.oct.partial`;
        temporaryAttributesName = `conversion-${conversionId}.octattrs.partial`;
        const output = await directory.getFileHandle(temporaryName, { create: true });
        const attributesOutput = await directory.getFileHandle(temporaryAttributesName, { create: true });
        accessHandle = await output.createSyncAccessHandle();
        attributesAccessHandle = await attributesOutput.createSyncAccessHandle();
        accessHandle.truncate(0);
        attributesAccessHandle.truncate(0);
        converter = formatVersion === "v5"
            ? new WasmLasOctConverter(file, accessHandle, attributesAccessHandle)
            : WasmLasOctConverter.createForVersion(file, accessHandle, attributesAccessHandle, formatVersion);
        self.postMessage({ type: "status", message: `Converting ${file.name} locally…` });

        let progress;
        let lastProgressAt = performance.now();
        do {
            if (cancelRequested) throw new DOMException("Conversion cancelled.", "AbortError");
            progress = converter.convertNext();
            const now = performance.now();
            if (progress.stage !== stage) {
                stageDurations[stage] = now - stageStartedAt;
                stage = progress.stage;
                stageStartedAt = now;
            }
            if (progress.done) stageDurations[stage] = now - stageStartedAt;
            if (progress.done || now - lastProgressAt >= 100) {
                self.postMessage({
                    type: "progress",
                    ...progress,
                    sourceName: file.name,
                    stageElapsedMs: now - stageStartedAt,
                });
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
        attributesAccessHandle.flush();
        attributesAccessHandle.close();
        attributesAccessHandle = null;
        const convertedFile = await output.getFile();
        const convertedAttributesFile = await attributesOutput.getFile();
        const outputName = file.name.replace(/\.(las|laz)$/i, ".oct");
        const outputAttributesName = outputName.replace(/\.oct$/i, ".octattrs");
        self.postMessage({
            type: "complete",
            file: convertedFile,
            attributesFile: convertedAttributesFile,
            outputName,
            outputAttributesName,
            points: progress.totalPoints,
            timings: {
                planningMs: Math.round(stageDurations.planning ?? 0),
                writingMs: Math.round(stageDurations.writing ?? 0),
                totalMs: Math.round(performance.now() - conversionStartedAt),
            },
        });
    } catch (error) {
        try {
            converter?.free();
        } catch {
            // After a WASM trap (`RuntimeError: unreachable`) the instance is untrusted and
            // free() may trap again; the error message below must still reach the host.
        }
        converter = null;
        try {
            accessHandle?.close();
        } catch {
            // The handle may already have been closed by a failed finalization.
        }
        accessHandle = null;
        try {
            attributesAccessHandle?.close();
        } catch {
            // The handle may already have been closed by a failed finalization.
        }
        attributesAccessHandle = null;
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
        if (temporaryAttributesName && directory) {
            try {
                await directory.removeEntry(temporaryAttributesName);
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
        try {
            attributesAccessHandle?.close();
        } catch {
            // The handle may already have been closed after a successful conversion.
        }
    }
}
