from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("profile_conversion.py")
SPEC = importlib.util.spec_from_file_location("profile_conversion", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
profile_conversion = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(profile_conversion)


class ProfileConversionTests(unittest.TestCase):
    def test_temporary_sizes_exclude_outputs_logs_and_symlinks_and_dedupe_hardlinks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            run_dir = root / "run"
            run_dir.mkdir()
            output = run_dir / "output.oct"
            output.write_bytes(b"final")
            output.with_suffix(".octattrs").write_bytes(b"attrs")
            (run_dir / "stdout.log").write_bytes(b"log")
            transient = run_dir / "geometry.123.1.tmp"
            transient.write_bytes(b"x" * 7)
            alias = run_dir / "alias.123.2.tmp"
            os.link(transient, alias)
            work = run_dir / "output.oct.123.1.work"
            work.mkdir()
            (work / "spool").write_bytes(b"y" * 11)
            try:
                (run_dir / "linked.123.3.tmp").symlink_to(transient)
            except (OSError, NotImplementedError):
                pass

            sizes = profile_conversion.regular_file_sizes(run_dir, output)

            self.assertEqual(sum(sizes.values()), 18)
            self.assertEqual(len(sizes), 2)

    def test_growth_counts_positive_deltas_after_deletion_and_creation(self) -> None:
        previous = {"old.tmp": 10, "shrinking.tmp": 12}
        current = {"shrinking.tmp": 3, "new.tmp": 7}
        growth = sum(max(size - previous.get(path, 0), 0) for path, size in current.items())
        self.assertEqual(growth, 7)

    def test_existing_run_directory_is_preserved(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            input_path = root / "input.las"
            input_path.write_bytes(b"input")
            run_dir = root / "existing"
            run_dir.mkdir()
            marker = run_dir / "keep"
            marker.write_text("preserved")

            with self.assertRaises(FileExistsError):
                profile_conversion.profile(input_path, run_dir, Path("unused"), 1, 2, 10)

            self.assertEqual(marker.read_text(), "preserved")

    def test_fake_success_and_failure_preserve_final_state(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            input_path = root / "input.las"
            input_path.write_bytes(b"input")
            fake = root / "fake-converter"
            fake.write_text(
                "#!/usr/bin/env python3\n"
                "import pathlib, sys, time\n"
                "output = pathlib.Path(sys.argv[3])\n"
                "output.write_bytes(b'final-oct')\n"
                "output.with_suffix('.octattrs').write_bytes(b'final-attrs')\n"
                "work = output.parent / 'sample.work'\n"
                "work.mkdir()\n"
                "(work / 'source.spool').write_bytes(b'x' * 1000)\n"
                "(output.parent / 'candidate.tmp').write_bytes(b't' * 20)\n"
                "time.sleep(0.08)\n"
                "raise SystemExit(7 if output.parent.name == 'run-7' else 0)\n"
            )
            fake.chmod(0o755)

            for requested_exit in (0, 7):
                run_dir = root / f"run-{requested_exit}"
                result = profile_conversion.profile(
                    input_path, run_dir, fake, 1, 2, 10
                )
                report = json.loads((run_dir / "report.json").read_text())

                self.assertEqual(result, 0 if requested_exit == 0 else 1)
                self.assertEqual(report["exit_code"], requested_exit)
                self.assertEqual(report["status"], "complete" if requested_exit == 0 else "incomplete")
                self.assertEqual(report["output_oct_bytes"], len(b"final-oct"))
                self.assertEqual(report["output_octattrs_bytes"], len(b"final-attrs"))
                self.assertTrue((run_dir / "stdout.log").is_file())
                self.assertTrue((run_dir / "stderr.log").is_file())
                self.assertTrue((run_dir / "samples.jsonl").is_file())

    def test_exit_zero_requires_both_nonempty_outputs(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            input_path = root / "input.las"
            input_path.write_bytes(b"input")
            fake = root / "fake-converter"
            fake.write_text(
                "#!/usr/bin/env python3\n"
                "import pathlib, sys\n"
                "output = pathlib.Path(sys.argv[3])\n"
                "output.write_bytes(b'coordinates')\n"
                "if output.parent.name == 'empty':\n"
                "    output.with_suffix('.octattrs').touch()\n"
            )
            fake.chmod(0o755)
            for case in ("missing", "empty"):
                run_dir = root / case
                result = profile_conversion.profile(input_path, run_dir, fake, 1, 2, 10)
                report = json.loads((run_dir / "report.json").read_text())
                self.assertEqual(result, 1)
                self.assertEqual(report["exit_code"], 0)
                self.assertEqual(report["status"], "incomplete")
                self.assertTrue(report["observation_errors"])
                self.assertEqual((run_dir / "output.oct").read_bytes(), b"coordinates")


if __name__ == "__main__":
    unittest.main()
