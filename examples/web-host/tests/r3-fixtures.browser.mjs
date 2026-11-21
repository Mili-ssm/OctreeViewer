// Generate the three fixtures and the root-page anchor files:
// R3_FIXTURE_DIR="$PWD/target/r3-fixtures" cargo test --offline -p viewer_format --test paged_v8 r3_small_fixtures_open_with_expected_relative_depths
// cargo build --offline -p viewer_wasm --target wasm32-unknown-unknown
// wasm-bindgen --target web --out-dir target/r3-fixtures/pkg target/wasm32-unknown-unknown/debug/viewer_wasm.wasm
// Serve the repository root, then import this module in a browser and call probe().
// No GPU adapter or rendering is required.
import init, { WasmAsyncFormatReader, WasmVoxelViewer } from '../../../target/r3-fixtures/pkg/viewer_wasm.js';

function assert(condition, message) {
    if (!condition) throw new Error(message);
}

async function fetchFile(name, kind) {
    const response = await fetch(`/target/r3-fixtures/${name}.${kind}`);
    assert(response.ok, `cannot fetch ${name}.${kind}: ${response.status}`);
    return response;
}

export async function probe() {
    await init();
    const results = [];
    for (const name of ['crs-root0', 'crs', 'local']) {
        const stem = `${name}-v8`;
        const primary = await (await fetchFile(stem, 'oct')).blob();
        const attributes = await (await fetchFile(stem, 'octattrs')).blob();
        const reader = await WasmAsyncFormatReader.open(primary, attributes);
        const viewer = new WasmVoxelViewer(0.6);
        try {
            const descriptor = reader.lodDatasetDescriptor(1024 * 1024);
            assert(descriptor.nodeCount === 3, `${stem}: wrong node count`);
            assert(descriptor.paged, `${stem}: wrong route`);
            assert((descriptor.crs === 'EPSG:32632') === (name !== 'local'), `${stem}: wrong CRS`);
            // The exported anchor is taken from the root page's child pointer, so it
            // identifies an unloaded page before this load and graft.
            assert(descriptor.maxLevel < 2, `${stem}: leaf already resident`);
            const opened = viewer.openPagedLodDataset(reader, 1024 * 1024);
            const anchor = (await (await fetchFile(stem, 'anchor')).text()).trim();
            assert(/^\d+$/.test(anchor), `${stem}: invalid page anchor`);
            const block = (await (await fetchFile(stem, 'block')).text()).trim();
            assert(/^\d+$/.test(block), `${stem}: invalid child block`);
            let unavailable = false;
            try {
                reader.planLodRead(BigInt(block), 'geometry');
            } catch {
                unavailable = true;
            }
            assert(unavailable, `${stem}: child block already resident`);
            const page = await reader.loadPage(anchor);
            viewer.graftPage(page);
            const request = reader.planLodRead(BigInt(block), 'geometry');
            assert(request, `${stem}: child block unavailable after page load`);
            const result = await request.read();
            assert(result.value.positions.length > 0, `${stem}: child block is empty`);
            result.release();
            assert(opened.status === 'metadata_ready', `${stem}: viewer did not open`);
            results.push({ stem, paged: descriptor.paged, anchor });
        } finally {
            viewer.free();
            reader.free();
        }
    }
    return results;
}
