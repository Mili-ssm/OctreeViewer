# viewer_wasm

Orquestación del visor y bindings de navegador (workers, typed arrays, WebGPU, conversión OPFS).
Conversión web: `core_convert.rs` (`WasmCoreConverter`, sobre `viewer_convert_core`) y `browser_convert.rs`
(motor anterior, por defecto hasta M3b).
Entrada: `ViewerController` (escenas en memoria), `LodDataset` (bloques en streaming) y
`src/bindings/`. Compilación y API:
[host web](../../examples/web-host/README.md), [CONTRIBUTING](../../CONTRIBUTING.md#compilar-wasm).
