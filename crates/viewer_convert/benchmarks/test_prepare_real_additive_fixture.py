from __future__ import annotations

import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("prepare_real_additive_fixture.py")
SPEC = importlib.util.spec_from_file_location("prepare_real_additive_fixture", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
fixture = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixture)


class RealAdditiveFixtureTests(unittest.TestCase):
    def inputs(self):
        payload = b"test payload; real LAS headers are validated by viewer_convert"
        nodes = {"0-0-0-0": (2, hashlib.sha256(payload).hexdigest())}
        manifest = {
            "dataType": "laszip",
            "hierarchyType": "json",
            "points": 100,
            "bounds": [0, 0, 0, 8, 8, 8],
            "schema": [{"name": "X"}],
        }
        files = {
            "ept.json": json.dumps(manifest).encode(),
            "ept-hierarchy/0-0-0-0.json": b'{"0-0-0-0": 2, "1-0-0-0": 98}',
            "ept-data/0-0-0-0.laz": payload,
        }
        return nodes, manifest, files

    def test_subset_retains_payload_metadata_and_source_provenance(self):
        nodes, manifest, files = self.inputs()
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "fixture"
            with (
                patch.object(fixture, "NODES", nodes),
                patch.object(fixture, "download", side_effect=files.__getitem__),
            ):
                provenance = fixture.prepare(output)
            selected = json.loads((output / "ept.json").read_text())
            self.assertEqual(selected, {**manifest, "points": 2})
            self.assertEqual(
                json.loads((output / "ept-hierarchy/0-0-0-0.json").read_text()),
                {"0-0-0-0": 2},
            )
            self.assertEqual(
                (output / "ept-data/0-0-0-0.laz").read_bytes(),
                files["ept-data/0-0-0-0.laz"],
            )
            self.assertEqual(
                (output / "source-ept.json").read_bytes(), files["ept.json"]
            )
            self.assertTrue(provenance["partial_subset"])
            self.assertEqual(provenance["source_points"], 100)
            with patch.object(fixture, "download") as download:
                with self.assertRaises(FileExistsError):
                    fixture.prepare(output)
                download.assert_not_called()

    def test_changed_payload_or_hierarchy_never_publishes_output(self):
        for changed in ("ept-data/0-0-0-0.laz", "ept-hierarchy/0-0-0-0.json"):
            with (
                self.subTest(changed=changed),
                tempfile.TemporaryDirectory() as temporary,
            ):
                nodes, _, files = self.inputs()
                files[changed] = b"{}"
                output = Path(temporary) / "fixture"
                with (
                    patch.object(fixture, "NODES", nodes),
                    patch.object(fixture, "download", side_effect=files.__getitem__),
                ):
                    with self.assertRaises(ValueError):
                        fixture.prepare(output)
                self.assertEqual(list(Path(temporary).iterdir()), [])

    def test_oversized_download_is_rejected(self):
        with (
            patch.object(
                fixture.urllib.request, "urlopen", return_value=io.BytesIO(b"12345")
            ),
            patch.object(fixture, "MAX_FILE_BYTES", 4),
        ):
            with self.assertRaises(ValueError):
                fixture.download("ept.json")


if __name__ == "__main__":
    unittest.main()
