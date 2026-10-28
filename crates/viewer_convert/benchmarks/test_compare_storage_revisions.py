from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock


HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
SPEC = importlib.util.spec_from_file_location("compare_storage_revisions", HERE / "compare_storage_revisions.py")
compare = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(compare)


def recorded_reference() -> dict:
    """Minimal shape of docs/records/benchmarks/data/storage-v7-*-2026-10-25.json."""
    return {
        "kind": "real_dataset_v5_v7_corpus_repeated_comparison",
        "conversion_options": {"divisions": 2, "points_per_block": 262144, "workers": 1},
        "datasets": {
            "belvedere": {"input": {"path": "data/b.laz", "bytes": 1}, "logical_read": {"budget_mib": 512}},
            "majadas": {"input": {"path": "data/m.laz", "bytes": 2}, "logical_read": {"budget_mib": 256}},
        },
    }


class ParseReferenceTests(unittest.TestCase):
    def test_recorded_reference_yields_datasets_and_options(self):
        parsed = compare.parse_reference(recorded_reference())
        self.assertEqual([d["label"] for d in parsed], ["belvedere", "majadas"])
        self.assertEqual(parsed[1]["source"], Path("data/m.laz"))
        self.assertEqual(parsed[1]["options"], {"--workers": "1", "--divisions": "2",
                                                "--points-per-block": "262144", "--max-decoded-mib": "256"})

    def test_command_reference_keeps_its_meaning(self):
        command = ["storage_suite.py", "x", "src.laz", "--workers", "4", "--divisions", "3",
                   "--points-per-block", "1000", "--max-decoded-mib", "64"]
        command[1], command[2] = "ignored", "src.laz"
        parsed = compare.parse_reference({"datasets": [{"label": "a b", "runs": [{"command": command}]}]})
        self.assertEqual(parsed[0]["source"], Path("src.laz"))
        self.assertEqual(parsed[0]["options"]["--workers"], "4")
        self.assertEqual(parsed[0]["options"]["--max-decoded-mib"], "64")

    def test_missing_fields_are_named(self):
        reference = recorded_reference()
        del reference["conversion_options"]["divisions"]
        with self.assertRaisesRegex(ValueError, r"conversion_options\.divisions"):
            compare.parse_reference(reference)
        reference = recorded_reference()
        del reference["datasets"]["majadas"]["logical_read"]["budget_mib"]
        with self.assertRaisesRegex(ValueError, r"datasets\.majadas\.logical_read\.budget_mib"):
            compare.parse_reference(reference)
        with self.assertRaisesRegex(ValueError, r"datasets\[0\]\.runs\[0\]\.command"):
            compare.parse_reference({"datasets": [{"label": "a", "runs": [{}]}]})
        with self.assertRaisesRegex(ValueError, "datasets"):
            compare.parse_reference({})


class PathAndValidationTests(unittest.TestCase):
    def test_relative_source_resolved_from_root_regardless_of_cwd(self):
        root = Path("/some/repo")
        with tempfile.TemporaryDirectory() as tmp:
            previous = os.getcwd()
            os.chdir(tmp)
            try:
                parsed = compare.parse_reference(recorded_reference(), root)
            finally:
                os.chdir(previous)
        self.assertEqual(parsed[0]["source"], root / "data/b.laz")

    def test_trailing_flag_raises_value_error_naming_it(self):
        command = ["s.py", "x", "src.laz", "--divisions", "3", "--points-per-block", "1",
                   "--max-decoded-mib", "64", "--workers"]
        with self.assertRaisesRegex(ValueError, "--workers"):
            compare.parse_reference({"datasets": [{"label": "a", "runs": [{"command": command}]}]})

    def test_missing_source_fails_before_creating_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            reference = tmp / "ref.json"
            reference.write_text(json.dumps(recorded_reference()))
            output, report = tmp / "out", tmp / "sub" / "report.json"
            argv = ["prog", str(reference), str(output), str(report),
                    "--convert-binary", "c", "--read-binary", "r"]
            with mock.patch.object(sys, "argv", argv), self.assertRaisesRegex(FileNotFoundError, "b.laz"):
                compare.main()
            self.assertFalse(output.exists())
            self.assertFalse(report.parent.exists())


class ModesTests(unittest.TestCase):
    BASE = ["ref.json", "out", "report.json", "--convert-binary", "c", "--read-binary", "r"]

    def test_default_runs_all_modes(self):
        self.assertEqual(compare.parse_args(self.BASE).modes, list(compare.storage_suite.MODES))

    def test_subset_excludes_reconstruct(self):
        args = compare.parse_args([*self.BASE, "--modes", "hierarchy", "progressive"])
        self.assertEqual(args.modes, ["hierarchy", "progressive"])
        self.assertNotIn("reconstruct", args.modes)

    def test_invalid_mode_rejected(self):
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            compare.parse_args([*self.BASE, "--modes", "bogus"])


if __name__ == "__main__":
    unittest.main()
