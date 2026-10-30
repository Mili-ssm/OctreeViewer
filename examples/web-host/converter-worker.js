import init, { WasmCoreConverter, WasmLasOctConverter } from "../../pkg/viewer_wasm.js";
import { createSpoolHost } from "./spool-tiers.mjs";

let cancelRequested = false;

// One call site: package-local.py swaps it for the embedded-bytes initializer.
async function initWasm() {
    await init();
}

self.addEventListener("message", ({ data }) => {
    if (data?.type === "cancel") {
        cancelRequested = true;
    } else if (data?.type === "convert") {
        cancelRequested = false;
        if (data.engine === "core") void convertWithCore(data.file, data.options ?? {});
        else void convert(data.file, data.formatVersion ?? "v5");
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
        await initWasm();
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

const PARTIAL_DIRECTORY = "octree-viewer-conversions";

/**
 * Engine "core": one synchronous call over the shared native-equivalent converter. Cancelling is
 * the host's job (it terminates this worker and deletes the partial files named in `partial-files`).
 */
async function convertWithCore(file, options) {
    const startedAt = performance.now();
    let geometryHandle = null;
    let attributesHandle = null;
    let converter = null;
    let directory = null;
    let names = null;
    try {
        if (!file || !/\.la[sz]$/i.test(file.name)) throw new Error("Choose a .las or .laz file.");
        if (!navigator.storage?.getDirectory) {
            throw new Error("This browser does not support the Origin Private File System (OPFS).");
        }
        await initWasm();
        const root = await navigator.storage.getDirectory();
        directory = await root.getDirectoryHandle(PARTIAL_DIRECTORY, { create: true });
        const id = crypto.randomUUID();
        names = [`conversion-${id}.oct.partial`, `conversion-${id}.octattrs.partial`];
        // Announced before the files exist so a cancel can always find what to delete.
        self.postMessage({ type: "partial-files", directory: PARTIAL_DIRECTORY, names });
        const geometryFile = await directory.getFileHandle(names[0], { create: true });
        const attributesFile = await directory.getFileHandle(names[1], { create: true });
        geometryHandle = await geometryFile.createSyncAccessHandle();
        attributesHandle = await attributesFile.createSyncAccessHandle();
        geometryHandle.truncate(0);
        attributesHandle.truncate(0);
        self.postMessage({ type: "status", message: `Converting ${file.name} locally…` });
        let lastProgressAt = -Infinity;
        const onProgress = (progress) => {
            const now = performance.now();
            if (now - lastProgressAt < 100) return true;
            lastProgressAt = now;
            self.postMessage({ type: "progress", ...progress, sourceName: file.name });
            return true;
        };
        // RAM-only spools until the worker opens spill zones (zones: []).
        converter = new WasmCoreConverter(
            file, geometryHandle, attributesHandle, options, onProgress, createSpoolHost({ zones: [] }),
        );
        const summary = converter.convert();
        converter.free();
        converter = null;
        geometryHandle.flush();
        geometryHandle.close();
        geometryHandle = null;
        attributesHandle.flush();
        attributesHandle.close();
        attributesHandle = null;
        const outputName = file.name.replace(/\.(las|laz)$/i, ".oct");
        self.postMessage({
            type: "complete",
            file: await geometryFile.getFile(),
            attributesFile: await attributesFile.getFile(),
            outputName,
            outputAttributesName: outputName.replace(/\.oct$/i, ".octattrs"),
            points: summary.pointCount,
            blocks: summary.blockCount,
            timings: { totalMs: Math.round(performance.now() - startedAt) },
        });
    } catch (error) {
        try { converter?.free(); } catch { /* an untrusted instance may trap again */ }
        for (const handle of [geometryHandle, attributesHandle]) {
            try { handle?.close(); } catch { /* already closed by a failed finalization */ }
        }
        self.postMessage({ type: "error", message: error instanceof Error ? error.message : String(error) });
        for (const name of names ?? []) {
            try { await directory.removeEntry(name); } catch { /* the entry may be locked briefly */ }
        }
    }
}
