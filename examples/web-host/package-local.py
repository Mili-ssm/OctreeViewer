#!/usr/bin/env python3
"""Create a self-contained file:// viewer and ZIP archive."""

from __future__ import annotations

import argparse
import base64
import json
import re
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
HOST = ROOT / "examples" / "web-host"


def require_replace(source: str, old: str, new: str, label: str) -> str:
    if source.count(old) != 1:
        raise ValueError(f"expected exactly one {label} marker")
    return source.replace(old, new)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "artifacts" / "visor-web-local",
        help="directory for index.html, instructions, and ZIP archive",
    )
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)

    html = (HOST / "index.html").read_text(encoding="utf-8")
    main_js = (HOST / "main.js").read_text(encoding="utf-8")
    worker_js = (HOST / "viewer-worker.js").read_text(encoding="utf-8")
    queue_js = (HOST / "coalesced-command-queue.mjs").read_text(encoding="utf-8")
    metrics_js = (HOST / "benchmark-metrics.mjs").read_text(encoding="utf-8")
    rendering_budget_js = (HOST / "rendering-budget.mjs").read_text(encoding="utf-8")
    wasm_js = (ROOT / "pkg" / "viewer_wasm.js").read_text(encoding="utf-8")
    wasm_path = ROOT / "pkg" / "viewer_wasm_bg.wasm"
    sample_path = HOST / "sample.oct"

    wasm_bytes = base64.b64encode(wasm_path.read_bytes()).decode("ascii")
    sample_bytes = base64.b64encode(sample_path.read_bytes()).decode("ascii")
    worker_js = require_replace(
        worker_js,
        'import init, { WasmVoxelViewer } from "../../pkg/viewer_wasm.js";\n',
        "",
        "WASM import",
    )
    worker_js = require_replace(worker_js, "await init();", "await __wbg_init(__VIEWER_WASM_BYTES__);", "WASM init")

    wasm_js = wasm_js.replace("export class ", "class ")
    wasm_js = require_replace(
        wasm_js,
        "module_or_path = new URL('viewer_wasm_bg.wasm', import.meta.url);",
        "module_or_path = undefined;",
        "WASM asset URL",
    )
    wasm_js = require_replace(
        wasm_js,
        "export { initSync, __wbg_init as default };",
        "",
        "WASM module exports",
    )
    worker_source = (
        f'const __VIEWER_WASM_BYTES__ = Uint8Array.from(atob("{wasm_bytes}"), '
        "(value) => value.charCodeAt(0));\n"
        + wasm_js
        + "\n"
        + worker_js
    )

    queue_js = queue_js.replace("export class ", "class ")
    main_js = require_replace(
        main_js,
        'import { CoalescedCommandQueue } from "./coalesced-command-queue.mjs";\n',
        "",
        "command queue import",
    )
    main_js = require_replace(
        main_js,
        'import { FRAME_RATE_WARMUP_MS, measureFrameRate } from "./benchmark-metrics.mjs";\n',
        "",
        "benchmark metrics import",
    )
    main_js = require_replace(
        main_js,
        'import { framebufferSize } from "./rendering-budget.mjs";\n',
        "",
        "rendering budget import",
    )
    main_js = require_replace(
        main_js,
        'new Worker(new URL("./viewer-worker.js", import.meta.url), { type: "module" })',
        "new Worker(window.__VIEWER_WORKER_URL__)",
        "worker module URL",
    )
    sample_fetch = re.compile(
        r'const response = await fetch\(new URL\("\./sample\.oct", import\.meta\.url\),\s*'
        r'\{\s*signal: controller\.signal,\s*\}\);'
    )
    main_js, sample_replacements = sample_fetch.subn(
        "const response = new Response(window.__VIEWER_SAMPLE_BYTES__.slice(), { status: 200 });",
        main_js,
    )
    if sample_replacements != 1:
        raise ValueError("expected exactly one sample file fetch")

    inline_runtime = (
        "<script>\n"
        + f"const WORKER_SOURCE = {json.dumps(worker_source)};\n"
        + f'window.__VIEWER_SAMPLE_BYTES__ = Uint8Array.from(atob("{sample_bytes}"), '
        "(value) => value.charCodeAt(0));\n"
        + "window.__VIEWER_WORKER_URL__ = URL.createObjectURL("
        "new Blob([WORKER_SOURCE], { type: \"text/javascript\" }));\n"
        + queue_js
        + "\n"
        + metrics_js.replace("export const ", "const ").replace("export function ", "function ")
        + "\n"
        + rendering_budget_js.replace("export const ", "const ").replace("export function ", "function ")
        + "\n"
        + main_js
        + "\n</script>"
    )
    html = require_replace(
        html,
        '<script type="module" src="./main.js"></script>',
        inline_runtime,
        "host module script",
    )
    html = html.replace('<html lang="en">', '<html lang="es">')
    html = html.replace("<title>viewer_wasm host demo</title>", "<title>Visor local de nubes de puntos</title>")
    html = html.replace("<h1>Embeddable voxel viewer</h1>", "<h1>Visor de nubes de puntos</h1>")

    readme = """VISOR WEB LOCAL\n\n1. Extrae este ZIP en cualquier carpeta.\n2. Abre index.html con doble clic en un navegador compatible con WebGPU.\n3. Espera a que el estado indique que el renderizador está listo.\n4. Pulsa Open project dataset, selecciona tu archivo .oct y espera a que aparezca\n   primero la vista gruesa y después el refinamiento progresivo.\n5. Para medir FPS, espera el LOD objetivo y pulsa Run 15 s FPS benchmark. Mantén\n   la cámara quieta; al terminar pulsa Export run JSON.\n\nEl visor limita el framebuffer a 4.194.304 píxeles y 4.096 por lado. Reduce el\ntrabajo de render mientras está quieto; solo ejecuta el benchmark si lo solicitas.\n\nEl selector acepta el formato del proyecto .oct (versiones 1, 2 y 3). La muestra\nincluida también funciona sin conexión. Para cambiar detalle, usa Finest voxel detail.\n\nNo requiere instalar Rust ni iniciar un servidor. El HTML incluye WASM, worker,\nJavaScript y la muestra. Se necesita un navegador con WebGPU y OffscreenCanvas.\nEl render no sube el archivo a ningún servicio: lo lee localmente en el navegador.\n"""
    index_path = output / "index.html"
    readme_path = output / "LEEME.txt"
    archive_path = output / "visor-web-local.zip"
    index_path.write_text(html, encoding="utf-8")
    readme_path.write_text(readme, encoding="utf-8")
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.write(index_path, "index.html")
        archive.write(readme_path, "LEEME.txt")

    print(f"Created {index_path} ({index_path.stat().st_size:,} bytes)")
    print(f"Created {archive_path} ({archive_path.stat().st_size:,} bytes)")


if __name__ == "__main__":
    main()
