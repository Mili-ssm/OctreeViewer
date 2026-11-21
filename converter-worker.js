import init, { WasmCoreConverter } from "./pkg/viewer_wasm-82416ae.js";
import { createMemorySyncHandle, createSpoolHost, removeWithRetry } from "./spool-tiers.mjs";

// One call site: package-local.py swaps it for the embedded-bytes initializer.
async function initWasm() {
    await init();
}

const LEGACY_REMOVED = 'the legacy converter was removed; omit `engine` or use "core"';

// Cancelling is the host's job: it terminates this worker (see converter-api.mjs).
self.addEventListener("message", ({ data }) => {
    if (data?.type !== "convert") return;
    if (data.engine === "legacy") self.postMessage({ type: "error", message: LEGACY_REMOVED });
    else void convertWithCore(data.file, data.options ?? {});
});

const PARTIAL_DIRECTORY = "octree-viewer-conversions";
const GB = 1e9;

async function availableSummary() {
    try {
        const { quota, usage } = await navigator.storage.estimate();
        if (Number.isFinite(quota)) return `${(Math.max(0, quota - (usage ?? 0)) / GB).toFixed(1)} GB`;
    } catch { /* the estimate API is optional */ }
    return "unknown";
}

async function quotaMessage(estimate) {
    const need = (estimate?.spilledPeakBytes ?? 0) + (estimate?.outputBytes ?? 0);
    return `Not enough browser storage: need about ${(need / GB).toFixed(1)} GB, available ${await availableSummary()}. `
        + "Free disk space or convert a smaller file.";
}

function closeAll(handles) {
    for (const handle of handles) {
        try { handle.close(); } catch { /* already closed by a failed finalization */ }
    }
}

const DEFAULT_RAM_BUDGET = 4 * GB;
const MAX_RAM_BUDGET = 8 * GB;
// Firefox without persistent storage stops writing at 10 GiB - 1 MiB (OPFS_QUOTA_2026-10-29).
const FIREFOX_TEMPORARY_LIMIT = 10 * 2 ** 30 - 2 ** 20;
const RAM_WARNING = "Running without browser storage: the conversion is limited to RAM. "
    + "Serve the page over http://localhost for large files.";

function ramBudgetOf(options) {
    const value = options?.ramBudget;
    if (value === undefined || value === null) return DEFAULT_RAM_BUDGET;
    if (!Number.isFinite(value) || value <= 0) throw new Error("option ramBudget must be a positive number of bytes");
    return Math.min(value, MAX_RAM_BUDGET);
}

async function requestPersistence() {
    try {
        return (await navigator.storage.persist?.()) === true;
    } catch {
        return false;
    }
}

/**
 * Storage preflight (R4, C11). Only a case that is certain to fail is rejected up front:
 *  - Firefox without persistent storage hard-stops at ~10 GiB (short writes) whatever `estimate()`
 *    says, so `need` is checked against min(reported quota, 10 GiB - 1 MiB) - usage.
 *  - Firefox with `persist()` granted allows far more (476 GiB measured) and reports it honestly,
 *    so the reported free space is used.
 *  - Every other browser is never rejected here: Chrome enforces a larger quota than it reports
 *    (40 GiB+ written with a 10 GiB estimate), so a conversion it would allow must not be refused.
 *    A real shortage still surfaces as a QuotaExceededError and becomes the same message.
 */
async function checkStorageQuota(estimate, persisted) {
    if (!/\bFirefox\//.test(navigator.userAgent ?? "")) return;
    let quota, usage;
    try {
        ({ quota, usage } = await navigator.storage.estimate());
    } catch {
        return; // no estimate: let the write fail if it must
    }
    if (!Number.isFinite(quota)) return;
    const effective = persisted ? quota : Math.min(quota, FIREFOX_TEMPORARY_LIMIT);
    const free = Math.max(0, effective - (usage ?? 0));
    const need = (estimate.spilledPeakBytes ?? 0) + (estimate.outputBytes ?? 0);
    if (need > free) {
        throw new Error(
            `Not enough browser storage: need about ${(need / GB).toFixed(1)} GB, available ${(free / GB).toFixed(1)} GB. `
            + "Allow persistent storage for this site (it lifts Firefox's 10 GiB limit), free disk space or convert a smaller file.",
        );
    }
}

async function openOpfsFiles(directory, names, handles, files) {
    for (const name of names) {
        const entry = await directory.getFileHandle(name, { create: true });
        files.push(entry);
        const handle = await entry.createSyncAccessHandle();
        handles.push(handle);
        handle.truncate(0);
    }
}

/**
 * One synchronous call over the shared native-equivalent converter. Cancelling is
 * the host's job (it terminates this worker and deletes the partial files named in `partial-files`).
 * With OPFS, spill zones are sized from the header estimate and every file is announced before it
 * is created. Without OPFS (Chrome `file://`) the conversion runs RAM-only into memory handles.
 */
async function convertWithCore(file, options) {
    const startedAt = performance.now();
    const handles = [];
    let converter = null;
    let spoolHost = null;
    let directory = null;
    let names = [];
    let estimate = null;
    try {
        if (!file || !/\.la[sz]$/i.test(file.name)) throw new Error("Choose a .las or .laz file.");
        const ramBudget = ramBudgetOf(options);
        await initWasm();
        estimate = WasmCoreConverter.estimate(file, options);
        let root = null;
        try {
            root = (await navigator.storage?.getDirectory?.()) ?? null;
        } catch { /* Chrome file:// rejects: no OPFS */ }
        const ramOnly = root === null;
        const files = [];
        let geometryHandle, attributesHandle, zoneHandles = [], zoneNames = [];
        if (ramOnly) {
            if (estimate.ramOnlyBytes > ramBudget) {
                throw new Error(
                    `Not enough memory without browser storage: this file needs about ${(estimate.ramOnlyBytes / GB).toFixed(1)} GB `
                    + `of RAM but the limit is ${(ramBudget / GB).toFixed(1)} GB. Serve the page over http://localhost `
                    + "(for example python3 -m http.server) so the converter can use disk-backed storage.",
                );
            }
            self.postMessage({ type: "warning", message: RAM_WARNING });
            geometryHandle = createMemorySyncHandle();
            attributesHandle = createMemorySyncHandle();
            spoolHost = createSpoolHost({ zones: [] });
        } else {
            const persisted = await requestPersistence();
            await checkStorageQuota(estimate, persisted);
            directory = await root.getDirectoryHandle(PARTIAL_DIRECTORY, { create: true });
            const id = crypto.randomUUID();
            zoneNames = Array.from({ length: estimate.zones }, (_, i) => `conversion-${id}.zone-${i}.partial`);
            names = [`conversion-${id}.oct.partial`, `conversion-${id}.octattrs.partial`, ...zoneNames];
            // Announced before the files exist so a cancel can always find what to delete.
            self.postMessage({ type: "partial-files", directory: PARTIAL_DIRECTORY, names });
            await openOpfsFiles(directory, names, handles, files);
            [geometryHandle, attributesHandle, ...zoneHandles] = handles;
            spoolHost = createSpoolHost({ zones: zoneHandles });
        }
        self.postMessage({ type: "status", message: `Converting ${file.name} locally…` });
        let lastProgressAt = -Infinity;
        const onProgress = (progress) => {
            const now = performance.now();
            if (now - lastProgressAt < 100) return true;
            lastProgressAt = now;
            self.postMessage({ type: "progress", ...progress, sourceName: file.name });
            return true;
        };
        converter = new WasmCoreConverter(
            file, geometryHandle, attributesHandle, options, onProgress, spoolHost,
        );
        const summary = converter.convert();
        converter.free();
        converter = null;
        geometryHandle.flush();
        attributesHandle.flush();
        const outputName = file.name.replace(/\.(las|laz)$/i, ".oct");
        const outputAttributesName = outputName.replace(/\.oct$/i, ".octattrs");
        let outputFile, outputAttributes;
        if (ramOnly) {
            spoolHost.dispose();
            const asFile = (blob, name) => (typeof File === "function" ? new File([blob], name) : blob);
            outputFile = asFile(geometryHandle.toBlob(), outputName);
            outputAttributes = asFile(attributesHandle.toBlob(), outputAttributesName);
        } else {
            // Scratch zones go first: removing them releases quota before the output is handed over.
            closeAll(zoneHandles);
            spoolHost.dispose();
            await removeWithRetry(directory, zoneNames).catch(() => {});
            closeAll([geometryHandle, attributesHandle]);
            outputFile = await files[0].getFile();
            outputAttributes = await files[1].getFile();
        }
        self.postMessage({
            type: "complete",
            file: outputFile,
            attributesFile: outputAttributes,
            outputName,
            outputAttributesName,
            points: summary.pointCount,
            blocks: summary.blockCount,
            spool: summary.spool,
            timings: { totalMs: Math.round(performance.now() - startedAt) },
        });
    } catch (error) {
        try { converter?.free(); } catch { /* an untrusted instance may trap again */ }
        closeAll(handles);
        const message = error?.name === "QuotaExceededError"
            ? await quotaMessage(estimate)
            : error instanceof Error ? error.message : String(error);
        self.postMessage({ type: "error", message });
        if (directory) await removeWithRetry(directory, names).catch(() => {});
    }
}
