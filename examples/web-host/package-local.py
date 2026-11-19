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


def inline_json(value: str) -> str:
    """JSON string literal safe inside an inline <script> (cannot close the tag or open a comment)."""
    return json.dumps(value).replace("</", "<\\/").replace("<!--", "<\\!--")


def require_replace(source: str, old: str, new: str, label: str) -> str:
    if source.count(old) != 1:
        raise ValueError(f"expected exactly one {label} marker")
    return source.replace(old, new)


# Workers import the generated bindings they use; the exported names change with the API.
WASM_IMPORT = re.compile(r'^import init, \{[^}]*\} from "\.\./\.\./pkg/viewer_wasm\.js";\n', re.MULTILINE)


def wasm_import_line(source: str, label: str) -> str:
    lines = WASM_IMPORT.findall(source)
    if len(lines) != 1:
        raise ValueError(f"expected exactly one {label} marker")
    return lines[0]


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
    build_worker_js = (HOST / "build-worker.js").read_text(encoding="utf-8")
    queue_js = (HOST / "coalesced-command-queue.mjs").read_text(encoding="utf-8")
    metrics_js = (HOST / "benchmark-metrics.mjs").read_text(encoding="utf-8")
    rendering_budget_js = (HOST / "rendering-budget.mjs").read_text(encoding="utf-8")
    viewer_api_js = (HOST / "viewer-api.mjs").read_text(encoding="utf-8")
    canvas_replacement_js = (HOST / "canvas-replacement.mjs").read_text(encoding="utf-8")
    min_length_js = (HOST / "min-length.mjs").read_text(encoding="utf-8")
    converter_api_js = (HOST / "converter-api.mjs").read_text(encoding="utf-8")
    converter_worker_js = (HOST / "converter-worker.js").read_text(encoding="utf-8")
    spool_tiers_js = (HOST / "spool-tiers.mjs").read_text(encoding="utf-8")
    converter_ui_js = (HOST / "converter-ui.mjs").read_text(encoding="utf-8")
    range_source_js = (HOST / "http-range-source.mjs").read_text(encoding="utf-8")
    inline_range_source_js = re.sub(r"^export ", "", range_source_js, flags=re.MULTILINE)
    range_source_import = 'import { isAbortError, readConcurrency, resolveSource, sumRangeStats } from "./http-range-source.mjs";\n'
    worker_js = require_replace(worker_js, range_source_import, "", "range source import")
    build_worker_js = require_replace(build_worker_js, range_source_import, "", "build worker range source import")
    # The dynamic px controller is a small pure module: inline it in place of its import.
    px_controller_js = (HOST / "px-controller.mjs").read_text(encoding="utf-8")
    inline_px_controller_js = re.sub(r"^export ", "", px_controller_js, flags=re.MULTILINE)
    px_controller_import = 'import { WINDOW_MS as PX_WINDOW_MS, createPxController } from "./px-controller.mjs";\n'
    worker_js = require_replace(
        worker_js,
        px_controller_import,
        inline_px_controller_js + "\nconst PX_WINDOW_MS = WINDOW_MS;\n",
        "px controller import",
    )
    embed_worker_js = worker_js
    embed_build_worker_js = build_worker_js
    wasm_js = (ROOT / "pkg" / "viewer_wasm.js").read_text(encoding="utf-8")
    embed_wasm_js = wasm_js
    wasm_path = ROOT / "pkg" / "viewer_wasm_bg.wasm"
    sample_path = HOST / "sample.oct"

    wasm_bytes = base64.b64encode(wasm_path.read_bytes()).decode("ascii")
    sample_bytes = base64.b64encode(sample_path.read_bytes()).decode("ascii")
    attributes_bytes = base64.b64encode((HOST / "sample.octattrs").read_bytes()).decode("ascii")
    worker_js = require_replace(worker_js, wasm_import_line(worker_js, "WASM import"), "", "WASM import")
    worker_js = require_replace(
        worker_js,
        "wasmExports = await init();",
        "wasmExports = await __wbg_init(__VIEWER_WASM_BYTES__);",
        "WASM init",
    )

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
        + inline_range_source_js
        + "\n"
        + worker_js
    )
    # The build worker gets the WASM bytes from the viewer worker (init message), not a second copy.
    build_worker_js = require_replace(
        build_worker_js, wasm_import_line(build_worker_js, "build worker WASM import"), "", "build worker WASM import"
    )
    build_worker_js = require_replace(
        build_worker_js, "wasmExports = await init(", "wasmExports = await __wbg_init(", "build worker WASM init"
    )
    build_worker_source = wasm_js + "\n" + inline_range_source_js + "\n" + build_worker_js
    converter_worker_js = require_replace(
        converter_worker_js,
        wasm_import_line(converter_worker_js, "converter WASM import"),
        "",
        "converter WASM import",
    )
    converter_worker_js = require_replace(
        converter_worker_js,
        "await init();",
        "await __wbg_init(__CONVERTER_WASM_BYTES__);",
        "converter WASM init",
    )
    spool_tiers_import = 'import { createMemorySyncHandle, createSpoolHost, removeWithRetry } from "./spool-tiers.mjs";\n'
    converter_worker_js = require_replace(converter_worker_js, spool_tiers_import, "", "spool tiers import")
    inline_spool_tiers_js = re.sub(r"^export ", "", spool_tiers_js, flags=re.MULTILINE)
    # The page script also needs removeWithRetry (cancel cleanup), so converter-api's import goes too.
    converter_api_js = require_replace(
        converter_api_js, 'import { removeWithRetry } from "./spool-tiers.mjs";\n\n', "", "converter API spool tiers import"
    )
    converter_worker_source = (
        f'const __CONVERTER_WASM_BYTES__ = Uint8Array.from(atob("{wasm_bytes}"), '
        "(value) => value.charCodeAt(0));\n"
        + wasm_js
        + "\n"
        + inline_spool_tiers_js
        + "\n"
        + converter_worker_js
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
        'import {\n    FRAME_RATE_WINDOW_MS, FRAME_RATE_WARMUP_MS, lodCounters, lodStillLoading, lodTargetReached, measureFrameRate,\n    renderPathStatsLines, summarizeFrameReplies,\n} from "./benchmark-metrics.mjs";\n',
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
        'import { replaceCanvasElement } from "./canvas-replacement.mjs";\n',
        "",
        "canvas replacement import",
    )
    main_js = require_replace(
        main_js,
        'import { crsUnitPolicy, lengthToDatasetUnits, minLengthCommands, parseSpacingFactor } from "./min-length.mjs";\n',
        "",
        "min length import",
    )
    main_js = require_replace(
        main_js,
        'import { createConverter } from "./converter-api.mjs";\n',
        "",
        "converter API import",
    )
    converter_api_js = converter_api_js.replace("export function createConverter", "function createConverter")
    main_js = require_replace(
        main_js,
        'import {\n    FORMAT_VERSIONS, OPTION_DEFAULTS, fixedFrameDisplayValue, fixedFrameHint, isGlobalFrameFormat, progressText, spoolText, timingsText, validateConverterOptions,\n} from "./converter-ui.mjs";\n',
        "",
        "converter UI import",
    )
    inline_converter_ui_js = re.sub(r"^export ", "", converter_ui_js, flags=re.MULTILINE)
    converter_api_js = require_replace(
        converter_api_js,
        '?? new URL("./converter-worker.js", import.meta.url);',
        ";",
        "inline converter worker URL",
    )
    # The embedded worker has no imports; match the viewer's classic Blob worker on file://.
    converter_api_js = require_replace(
        converter_api_js,
        'type: "module",',
        'type: "classic",',
        "inline converter worker type",
    )
    main_js = require_replace(
        main_js,
        'const BUILD_WORKER = { url: new URL("./build-worker.js", import.meta.url).href, type: "module" };',
        'const BUILD_WORKER = { url: window.__BUILD_WORKER_URL__, type: "classic" };',
        "build worker URL",
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

    main_js = require_replace(main_js,
        'await fetch(new URL("./sample.octattrs", import.meta.url), { signal: controller.signal })',
        'new Response(window.__VIEWER_ATTRIBUTE_BYTES__.slice(), { status: 200 })',
        "attribute sample fetch")
    inline_runtime = (
        "<script>\n"
        + f"const WORKER_SOURCE = {inline_json(worker_source)};\n"
        + f'window.__VIEWER_SAMPLE_BYTES__ = Uint8Array.from(atob("{sample_bytes}"), '
        "(value) => value.charCodeAt(0));\n"
        + f'window.__VIEWER_ATTRIBUTE_BYTES__ = Uint8Array.from(atob("{attributes_bytes}"), '
        "(value) => value.charCodeAt(0));\n"
        + "window.__VIEWER_WORKER_URL__ = URL.createObjectURL("
        "new Blob([WORKER_SOURCE], { type: \"text/javascript\" }));\n"
        + f"const BUILD_WORKER_SOURCE = {inline_json(build_worker_source)};\n"
        + "window.__BUILD_WORKER_URL__ = URL.createObjectURL("
        "new Blob([BUILD_WORKER_SOURCE], { type: \"text/javascript\" }));\n"
        + f"const CONVERTER_WORKER_SOURCE = {inline_json(converter_worker_source)};\n"
        + "window.__CONVERTER_WORKER_URL__ = URL.createObjectURL("
        "new Blob([CONVERTER_WORKER_SOURCE], { type: \"text/javascript\" }));\n"
        + queue_js
        + "\n"
        + metrics_js.replace("export const ", "const ").replace("export function ", "function ")
        + "\n"
        + rendering_budget_js.replace("export const ", "const ").replace("export function ", "function ")
        + "\n"
        + canvas_replacement_js.replace("export function ", "function ")
        + "\n"
        + min_length_js.replace("export const ", "const ").replace("export function ", "function ")
        + "\n"
        + inline_spool_tiers_js
        + "\n"
        + converter_api_js
        + "\n"
        + inline_converter_ui_js
        + "\n"
        + main_js
        + "\n</script>"
    )
    html, bootstrap_replacements = re.subn(
        r'<script id="demo-bootstrap">[\s\S]*?</script>',
        lambda _: inline_runtime,
        html,
    )
    if bootstrap_replacements != 1:
        raise ValueError("expected exactly one host bootstrap script")
    html = html.replace('<html lang="en">', '<html lang="es">')
    html = html.replace("<title>viewer_wasm host demo</title>", "<title>Visor local de nubes de puntos</title>")
    html = html.replace("<h1>Embeddable voxel viewer</h1>", "<h1>Visor de nubes de puntos</h1>")

    readme = """VISOR WEB LOCAL\n\n1. Extrae este ZIP en cualquier carpeta.\n2. Abre index.html con doble clic en un navegador compatible con WebGPU.\n3. Espera a que el estado indique que el renderizador está listo.\n4. Pulsa Open project dataset, selecciona tu archivo .oct y espera a que aparezca\n   primero la vista gruesa y después el refinamiento progresivo.\n5. Para medir FPS, espera el LOD objetivo y pulsa Run 15 s FPS benchmark. Mantén\n   la cámara quieta; al terminar pulsa Export run JSON.\n\nEl visor limita el framebuffer a 4.194.304 píxeles y 4.096 por lado. Reduce el\ntrabajo de render mientras está quieto; solo ejecuta el benchmark si lo solicitas.\n\nEl selector acepta el formato del proyecto .oct (V7 o V5; .octattrs opcional para color y atributos). La muestra\nincluida también funciona sin conexión. Para cambiar detalle, usa Finest voxel detail.\n\nNo requiere instalar Rust ni iniciar un servidor. El HTML incluye WASM, worker,\nJavaScript y la muestra. Se necesita un navegador con WebGPU y OffscreenCanvas.\nEl render no sube el archivo a ningún servicio: lo lee localmente en el navegador.\n"""
    readme = readme.replace(
        "5. Para medir FPS,",
        "5. Para convertir, selecciona un LAS/LAZ en Convert a local LAS/LAZ file;\n"
        "   descarga los .oct y .octattrs generados o ábrelo directamente en el visor.\n"
        "6. Para medir FPS,",
    )
    readme = readme.replace(
        "Para cambiar detalle, usa Finest voxel detail.",
        "Para cambiar detalle, usa Finest voxel detail.\n\nCAMARA\n"
        "Observer: clic en un voxel fija el centro; arrastre izquierdo desplaza, derecho rota y la rueda ajusta el radio.\n"
        "Bird: arrastre desplaza, derecho rota, WASD/QE mueve y la rueda ajusta velocidad.\n\n"
        "La carpeta embed/ contiene la API modular para integrarla en otra pagina servida en localhost o HTTPS.",
    )
    index_path = output / "index.html"
    readme_path = output / "LEEME.txt"
    archive_path = output / "visor-web-local.zip"
    index_path.write_text(html, encoding="utf-8")
    readme_path.write_text(readme, encoding="utf-8")
    embed_dir = output / "embed"
    embed_dir.mkdir(parents=True, exist_ok=True)
    embed_import = wasm_import_line(embed_worker_js, "embeddable worker WASM import")
    embed_worker_js = require_replace(
        embed_worker_js,
        embed_import,
        embed_import.replace('"../../pkg/viewer_wasm.js"', '"./viewer_wasm.js"'),
        "embeddable worker WASM import",
    )
    embed_build_import = wasm_import_line(embed_build_worker_js, "embeddable build worker WASM import")
    embed_build_worker_js = require_replace(
        embed_build_worker_js,
        embed_build_import,
        embed_build_import.replace('"../../pkg/viewer_wasm.js"', '"./viewer_wasm.js"'),
        "embeddable build worker WASM import",
    )
    embed_converter_worker_js = (HOST / "converter-worker.js").read_text(encoding="utf-8")
    embed_converter_import = wasm_import_line(embed_converter_worker_js, "embeddable converter WASM import")
    embed_converter_worker_js = require_replace(
        embed_converter_worker_js,
        embed_converter_import,
        embed_converter_import.replace('"../../pkg/viewer_wasm.js"', '"./viewer_wasm.js"'),
        "embeddable converter WASM import",
    )
    (embed_dir / "viewer-api.mjs").write_text(viewer_api_js, encoding="utf-8")
    (embed_dir / "converter-api.mjs").write_text(
        (HOST / "converter-api.mjs").read_text(encoding="utf-8"), encoding="utf-8"
    )
    (embed_dir / "spool-tiers.mjs").write_text(spool_tiers_js, encoding="utf-8")
    (embed_dir / "converter-worker.js").write_text(embed_converter_worker_js, encoding="utf-8")
    (embed_dir / "viewer-worker.js").write_text(embed_worker_js, encoding="utf-8")
    (embed_dir / "build-worker.js").write_text(embed_build_worker_js, encoding="utf-8")
    (embed_dir / "rendering-budget.mjs").write_text(rendering_budget_js, encoding="utf-8")
    (embed_dir / "canvas-replacement.mjs").write_text(canvas_replacement_js, encoding="utf-8")
    (embed_dir / "min-length.mjs").write_text(min_length_js, encoding="utf-8")
    (embed_dir / "viewer_wasm.js").write_text(embed_wasm_js, encoding="utf-8")
    (embed_dir / "viewer_wasm_bg.wasm").write_bytes(wasm_path.read_bytes())
    (embed_dir / "README.txt").write_text(
        "Sirve la carpeta extraida en localhost o HTTPS y añade a tu pagina:\n\n"
        '<canvas id="cloud" style="width:100%;height:70vh"></canvas>\n'
        '<input id="file" type="file" accept=".oct">\n'
        '<input id="source" type="file" accept=".las,.laz">\n'
        '<script type="module">\n'
        'import { createViewer } from "./embed/viewer-api.mjs";\n'
        'import { createConverter } from "./embed/converter-api.mjs";\n'
        'const viewer = createViewer(document.querySelector("#cloud"));\n'
        'await viewer.ready;\n'
        'document.querySelector("#file").addEventListener("change", (event) => {\n'
        '  void viewer.load(event.target.files[0], { voxelPixels: 1 });\n'
        '});\n'
        'viewer.on("pick", (hit) => console.log(hit?.position, hit?.voxelCenter));\n'
        'const converter = createConverter();\n'
        'document.querySelector("#source").addEventListener("change", async (event) => {\n'
        '  const output = await converter.convert(event.target.files[0]);\n'
        '  console.log(output.file, output.name, output.points);\n'
        '});\n'
        'converter.dispose();\n'
        '</script>\n\n'
        "API: viewer load, setMode, getCamera, setCamera, pick, getMetadata, getStats, fit, on, dispose; converter convert, on, cancel, dispose.\n",
        encoding="utf-8",
    )
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.write(index_path, "index.html")
        archive.write(readme_path, "LEEME.txt")
        for embed_file in sorted(embed_dir.iterdir()):
            archive.write(embed_file, f"embed/{embed_file.name}")

    print(f"Created {index_path} ({index_path.stat().st_size:,} bytes)")
    print(f"Created {archive_path} ({archive_path.stat().st_size:,} bytes)")


if __name__ == "__main__":
    main()
