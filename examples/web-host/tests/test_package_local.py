import importlib.util
import json
import re
import subprocess
import tempfile
import unittest
from html.parser import HTMLParser
from pathlib import Path
from unittest.mock import patch


HOST = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("package_local", HOST / "package-local.py")
package_local = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(package_local)


class Scripts(HTMLParser):
    def __init__(self):
        super().__init__()
        self.scripts = []
        self.active = False

    def handle_starttag(self, tag, attrs):
        if tag == "script":
            self.scripts.append([dict(attrs), ""])
            self.active = True

    def handle_data(self, data):
        if self.active:
            self.scripts[-1][1] += data

    def handle_endtag(self, tag):
        if tag == "script":
            self.active = False


class LocalPackageTests(unittest.TestCase):
    def test_offline_package_has_no_external_modules_and_embeds_both_workers(self):
        # Minimal binding markers keep this packaging regression independent of a WASM build.
        glue = """export class WasmVoxelViewer {}
export class WasmCoreConverter {}
async function __wbg_init(module_or_path) {
    module_or_path = new URL('viewer_wasm_bg.wasm', import.meta.url);
}
function initSync() {}
export { initSync, __wbg_init as default };
"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pkg = root / "pkg"
            pkg.mkdir()
            (pkg / "viewer_wasm.js").write_text(glue)
            (pkg / "viewer_wasm_bg.wasm").write_bytes(b"\0asm\x01\0\0\0")
            output = root / "output"
            with patch.object(package_local, "ROOT", root), patch(
                "sys.argv", ["package-local.py", "--output", str(output)]
            ):
                package_local.main()
            html = (output / "index.html").read_text()
            parser = Scripts()
            parser.feed(html)
            self.assertEqual(len(parser.scripts), 1)
            attrs, runtime = parser.scripts[0]
            self.assertNotIn("src", attrs)
            self.assertNotEqual(attrs.get("type"), "module")
            self.assertNotIn("import.meta", runtime)
            self.assertNotIn('fetch(new URL("./sample.', runtime)
            self.assertIn("new Worker(window.__VIEWER_WORKER_URL__)", runtime)
            self.assertIn('type: "classic",', runtime)
            self.assertNotIn('type: "module",', runtime)
            self.assertIn("function validateConverterOptions(", runtime)
            self.assertNotIn("converter-ui.mjs", runtime)
            self.assertNotIn("WasmLasOctConverter", runtime)
            self.check_script(runtime)
            for name in ["WORKER_SOURCE", "CONVERTER_WORKER_SOURCE"]:
                match = re.search(rf"^const {name} = (.*);$", runtime, re.MULTILINE)
                self.assertIsNotNone(match)
                worker = json.loads(match[1])
                self.assertNotIn("import.meta", worker)
                self.assertNotRegex(worker, r"(?m)^import ")
                self.assertIn("Uint8Array.from(atob(", worker)
                self.check_script(worker)
                self.assertNotIn("WasmLasOctConverter", worker)
                if name == "CONVERTER_WORKER_SOURCE":
                    self.assertIn("function createSpoolHost(", worker)
            # The build worker is a nested Blob worker; it receives the WASM bytes from the viewer
            # worker instead of embedding a second copy.
            self.assertIn('const BUILD_WORKER = { url: window.__BUILD_WORKER_URL__, type: "classic" };', runtime)
            self.assertIn("window.__BUILD_WORKER_URL__ = URL.createObjectURL(", runtime)
            match = re.search(r"^const BUILD_WORKER_SOURCE = (.*);$", runtime, re.MULTILINE)
            self.assertIsNotNone(match)
            build_worker = json.loads(match[1])
            self.assertNotIn("import.meta", build_worker)
            self.assertNotRegex(build_worker, r"(?m)^import ")
            self.assertNotIn("atob(", build_worker)
            self.assertIn("await __wbg_init(", build_worker)
            self.check_script(build_worker)
            # The independently served embedding API remains modular.
            embed = output / "embed"
            self.assertIn('from "./viewer_wasm.js"', (embed / "build-worker.js").read_text())
            self.assertIn('type: "module",', (embed / "converter-api.mjs").read_text())
            self.assertIn('from "./viewer_wasm.js"', (embed / "viewer-worker.js").read_text())
            self.assertIn("export function createSpoolHost(", (embed / "spool-tiers.mjs").read_text())

    def test_inline_json_cannot_close_the_script_tag(self):
        source = 'const a = "</script><script>alert(1)</script>"; // <!-- x'
        literal = package_local.inline_json(source)
        self.assertNotIn("</", literal)
        self.assertNotIn("<!--", literal)
        html = f"<script>const S = {literal};</script>"
        self.assertEqual(html.count("</script>"), 1)
        result = subprocess.run(
            ["node", "-e", f"const S = {literal}; process.stdout.write(S);"],
            text=True, capture_output=True, check=True,
        )
        self.assertEqual(result.stdout, source)

    def check_script(self, source):
        result = subprocess.run(
            ["node", "--check"], input=source, text=True, capture_output=True, check=False
        )
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
