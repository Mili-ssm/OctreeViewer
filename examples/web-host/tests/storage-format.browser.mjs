// Storage/Blob header and metadata probe; no GPU adapter or visual presentation required.
import init, { WasmVoxelViewer } from "../../../pkg/viewer_wasm.js";

function assert(condition, message) {
    if (!condition) throw new Error(message);
}

function crc32(bytes) {
    let crc = 0xffffffff;
    for (const byte of bytes) {
        crc ^= byte;
        for (let bit = 0; bit < 8; bit += 1) {
            crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
        }
    }
    return (crc ^ 0xffffffff) >>> 0;
}

self.onmessage = async ({ data = {} }) => {
    let viewer;
    try {
        await init();
        const bytes = await (data.coordinates ?? await fetch(new URL("../sample.oct", import.meta.url)).then(r => r.blob())).arrayBuffer();
        const attributes = data.attributes ?? await fetch(new URL("../sample.octattrs", import.meta.url)).then(r => r.blob());
        const header = new DataView(bytes, 0, 24);
        const major = header.getUint16(8, true);
        const revision = header.getUint32(16, true);
        const formatVersion = `v${major}${revision ? `.${revision}-pre` : ""}`;
        if (data.expectedVersion) assert(formatVersion === data.expectedVersion, "unexpected output version");
        viewer = new WasmVoxelViewer(0.6);
        const geometry = viewer.openFormatDataset(new Blob([bytes]), 64 * 1024 * 1024, undefined);
        assert(!geometry.hasAttributes, "primary unexpectedly requires attributes");
        const full = viewer.openFormatDataset(new Blob([bytes]), 64 * 1024 * 1024, attributes);
        assert(full.hasAttributes, "matching companion not attached");
        assert(full.nodeCount === geometry.nodeCount && full.nodeCount > 0
            && full.sourcePointCount === geometry.sourcePointCount, "companion changed hierarchy");
        assert(full.attributeSchema.length > 0, "missing typed schema");

        const legacy = new Uint8Array(bytes.slice(0));
        legacy.set(new TextEncoder().encode("OCTVWFRM"));
        new DataView(legacy.buffer).setUint32(20, crc32(legacy.subarray(0, 20)), true);
        const prerelease = new Uint8Array(bytes.slice(0));
        const view = new DataView(prerelease.buffer);
        view.setUint16(8, 6, true);
        view.setUint32(16, 4, true);
        view.setUint32(20, crc32(prerelease.subarray(0, 20)), true);
        const errors = [];
        for (const [input, expected] of [[legacy, "provisional"], [prerelease, "v6.4-pre"]]) {
            let error;
            try {
                viewer.openFormatDataset(new Blob([input]), 64 * 1024 * 1024, attributes);
            } catch (caught) {
                error = String(caught);
            }
            assert(error?.includes(expected), `missing explicit rejection: ${expected}`);
            errors.push(error);

        }
        let wrongSidecarRejected = false;
        try {
            viewer.openFormatDataset(new Blob([bytes]), 64 * 1024 * 1024, new Blob([bytes]));
        } catch {
            wrongSidecarRejected = true;
        }
        assert(wrongSidecarRejected, "primary accepted as its own companion");
        self.postMessage({ type: "probeResult", passed: true,
            geometryHasAttributes: geometry.hasAttributes, fullHasAttributes: full.hasAttributes,
            sourcePointCount: full.sourcePointCount, nodeCount: full.nodeCount,
            formatVersion, schemaColumns: full.attributeSchema.length, wrongSidecarRejected, errors });
    } catch (error) {
        self.postMessage({ type: "probeResult", passed: false, error: String(error) });
    } finally {
        viewer?.free();
    }
};
