// Load the actual paged viewer binding and compare its stored value before and after a graft.
// From the repository root (requires wasm32 target, matching wasm-bindgen CLI and playwright-cli):
// cargo run --offline -q -p format_parity --bin reference_spacing -- fuzz/corpus/fuzz_format_reader/seed-sample-paged-chain-k1
// The fixture must print `0.03125 1` (native spacing and unloaded anchor).
// cargo build --offline -p viewer_wasm --target wasm32-unknown-unknown --features test-reference-spacing
// wasm-bindgen --target web --out-dir target/reference-spacing/pkg target/wasm32-unknown-unknown/debug/viewer_wasm.wasm
// Run the browser probe in a subshell so its server and browser session are closed on exit:
// (
//   set -e
//   python3 -m http.server 18765 --bind 127.0.0.1 & server_pid=$!
//   trap 'playwright-cli -s=r3-spacing close; kill "$server_pid" 2>/dev/null || true' EXIT
//   sleep 1; kill -0 "$server_pid"
//   playwright-cli -s=r3-spacing open http://127.0.0.1:18765/
//   playwright-cli -s=r3-spacing eval "async () => { const m = await import('/examples/web-host/tests/reference-spacing.browser.mjs'); const b = await fetch('/fuzz/corpus/fuzz_format_reader/seed-sample-paged-chain-k1').then(r => r.blob()); return await m.probe(b, 0.03125, '1'); }"
// )
// A pass reports native, before and after all equal to 0.03125, with anchor "1".
import init, { WasmAsyncFormatReader, WasmVoxelViewer } from '../../../target/reference-spacing/pkg/viewer_wasm.js';

export async function probe(primary, expected, anchor) {
    await init();
    const reader = await WasmAsyncFormatReader.open(primary);
    const viewer = new WasmVoxelViewer(0.6);
    let before;
    let after;
    try {
        viewer.openPagedLodDataset(reader, 1024 * 1024);
        before = viewer.testReferenceSpacing();
        viewer.graftPage(await reader.loadPage(anchor));
        after = viewer.testReferenceSpacing();
    } finally {
        viewer.free();
        reader.free();
    }
    if (before !== expected || after !== expected) {
        throw new Error(`reference spacing mismatch: native=${expected}, before=${before}, after=${after}`);
    }
    return { native: expected, before, after, anchor };
}
