from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import sys


SCRIPT = Path(__file__).with_name("storage_suite.py")
SPEC = importlib.util.spec_from_file_location("storage_suite", SCRIPT)
storage_suite = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(storage_suite)


class StorageSuiteTests(unittest.TestCase):
    @unittest.skipUnless(Path("/usr/bin/time").is_file(), "GNU time required for RSS")
    def test_optional_rss_keeps_stdout_metrics_and_failed_operation_status(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            record = storage_suite.run_operation(
                [sys.executable, "-c", "print('elapsed_ms=1.5')"], root, "ok",
                storage_suite.parse_conversion_metrics, True,
            )
            self.assertEqual(record["metrics"], {"elapsed_ms": 1.5})
            self.assertGreater(record["peak_rss_bytes"], 0)
            self.assertEqual(record["execution_command"][0], "/usr/bin/time")
            failed = storage_suite.run_operation(
                [sys.executable, "-c", "raise SystemExit(3)"], root, "failed",
                storage_suite.parse_conversion_metrics, True,
            )
            self.assertEqual(failed["exit_code"], 3)
            self.assertNotIn("peak_rss_bytes", failed)

    def test_parsers_read_single_and_multiple_key_value_fields(self):
        conversion = storage_suite.parse_conversion_metrics(
            "mode=convert\noutput_bytes=123\nelapsed_ms=12.5\n"
        )
        self.assertEqual(conversion, {"mode": "convert", "output_bytes": 123, "elapsed_ms": 12.5})
        reader = storage_suite.parse_reader_metrics(
            "rep=1 mode=progressive load_ms=2.5 points=17 checksum=00000000000000af\n"
            "rep=1 level=0 nodes=1 node_p50_ms=0.25 node_p95_ms=0.5\n"
        )
        self.assertEqual(len(reader), 2)
        self.assertEqual(reader[0]["load_ms"], 2.5)
        self.assertEqual(reader[0]["checksum"], "00000000000000af")
        self.assertEqual(reader[1]["node_p95_ms"], 0.5)

    def test_summary_uses_successful_operations_and_excludes_level_and_wall_timestamps(self):
        summary = storage_suite.numeric_summary(
            [
                {
                    "exit_code": 0,
                    "metrics": [
                        {"mode": "progressive", "full_ms": 10.0, "start_unix_ns": 100},
                        {"level": 0, "node_p50_ms": 1.0, "node_p95_ms": 2.0},
                        {"level": 1, "node_p50_ms": 3.0, "node_p95_ms": 4.0},
                    ],
                },
                {"exit_code": 9, "metrics": {"full_ms": 1000.0, "payload_bytes": 999}},
            ]
        )
        self.assertEqual(summary["full_ms"]["median"], 10.0)
        self.assertNotIn("start_unix_ns", summary)
        self.assertNotIn("node_p50_ms", summary)
        self.assertNotIn("payload_bytes", summary)

    def test_preflight_rejects_nonpositive_repetitions_and_missing_binary(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "input.laz"
            source.write_bytes(b"fixture")
            run_dir = root / "run"
            with self.assertRaises(SystemExit):
                storage_suite.parse_args(
                    [
                        str(source), "result.oct", str(run_dir), "--convert-binary", "missing-convert",
                        "--read-binary", "missing-read", "--repetitions", "0", "--cache-state", "cold",
                    ]
                )
            args = storage_suite.parse_args(
                [
                    str(source), "result.oct", str(run_dir), "--convert-binary", "missing-convert",
                    "--read-binary", "missing-read", "--cache-state", "cold",
                ]
            )
            self.assertEqual(args.points_per_block, 262144)
            with self.assertRaisesRegex(ValueError, "convert binary"):
                storage_suite.run_suite(args)
            self.assertFalse(run_dir.exists())

    def test_points_per_block_is_bounded_by_converter_limit(self):
        with self.assertRaises(SystemExit):
            storage_suite.parse_args(
                [
                    "input.laz", "result.oct", "run", "--convert-binary", "convert",
                    "--read-binary", "reader", "--cache-state", "cold", "--points-per-block", "1000001",
                ]
            )

    def test_preflight_rejects_non_executable_binary_before_creating_run_dir(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "input.laz"
            source.write_bytes(b"fixture")
            executable = root / "executable"
            executable.write_text("#!/bin/sh\n", encoding="utf-8")
            executable.chmod(0o755)
            not_executable = root / "not-executable"
            not_executable.write_text("data", encoding="utf-8")
            not_executable.chmod(0o644)
            run_dir = root / "run"
            args = storage_suite.parse_args(
                [
                    str(source), "result.oct", str(run_dir), "--convert-binary", str(executable),
                    "--read-binary", str(not_executable), "--cache-state", "cold",
                ]
            )
            with self.assertRaisesRegex(ValueError, "not executable"):
                storage_suite.run_suite(args)
            self.assertFalse(run_dir.exists())

    def test_subprocess_failure_keeps_partial_report_and_logs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "input.laz"
            source.write_bytes(b"fixture")
            converter = root / "converter"
            converter.write_text(
                "#!/usr/bin/env python3\n"
                "import pathlib, sys\n"
                "out = pathlib.Path(sys.argv[3])\n"
                "if 'run-002' in out.name:\n"
                "    print('mode=convert')\n"
                "    raise SystemExit(7)\n"
                "out.write_bytes(b'oct')\n"
                "out.with_suffix('.octattrs').write_bytes(b'attrs')\n"
                "print('mode=convert\\noutput_bytes=9\\nelapsed_ms=1.5')\n",
                encoding="utf-8",
            )
            reader = root / "reader"
            reader.write_text(
                "#!/usr/bin/env python3\n"
                "import json, pathlib, sys\n"
                "report = json.loads(pathlib.Path('report.json').read_text())\n"
                "assert report['conversions'][0]['artifacts']['oct']['sha256']\n"
                "expected = len(report['conversions']) - 1\n"
                "assert len(report['reads'][sys.argv[2]]) == expected\n"
                "print(f'rep=1 mode={sys.argv[2]} load_ms=2.0 bytes=12')\n",
                encoding="utf-8",
            )
            converter.chmod(0o755)
            reader.chmod(0o755)
            run_dir = root / "run"
            args = storage_suite.parse_args(
                [
                    str(source), "result.oct", str(run_dir),
                    "--convert-binary", str(converter), "--read-binary", str(reader),
                    "--repetitions", "2", "--cache-state", "warm after conversion", "--profile", "release-perf",
                    "--points-per-block", "10000", "--format-version", "v7.2-pre",
                ]
            )
            self.assertEqual(storage_suite.run_suite(args), 1)
            report = json.loads((run_dir / "report.json").read_text(encoding="utf-8"))
            self.assertEqual(report["status"], "failed")
            self.assertEqual(len(report["conversions"]), 2)
            self.assertEqual(report["conversions"][1]["exit_code"], 7)
            self.assertEqual(len(report["reads"]["hierarchy"]), 1)
            self.assertTrue((run_dir / "convert-002.stderr.log").exists())
            self.assertEqual(report["options"]["cache_state"], "warm after conversion")
            self.assertEqual(report["options"]["profile"], "release-perf")
            self.assertEqual(report["options"]["points_per_block"], 10000)
            self.assertEqual(report["options"]["format_version"], "v7.2-pre")
            self.assertEqual(report["conversions"][0]["command"][-2:], ["10000", "v7.2-pre"])
            self.assertEqual(report["input"]["bytes"], len(b"fixture"))
            self.assertGreater(report["conversions"][0]["artifacts"]["oct"]["bytes"], 0)
            self.assertEqual(len(report["conversions"][0]["artifacts"]["octattrs"]["sha256"]), 64)


if __name__ == "__main__":
    unittest.main()
