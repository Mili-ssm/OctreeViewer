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
    def test_internal_phase_samples_require_complete_non_overlapping_windows(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stdout, samples = root / "stdout.log", root / "samples.jsonl"
            stdout.write_text(
                "internal_spatial_planning_available=true\n"
                "internal_spatial_planning_elapsed_ns=100\n"
                "internal_spatial_planning_start_unix_ns=10\n"
                "internal_spatial_planning_end_unix_ns=20\n"
                "internal_block_stream_available=true\n"
                "internal_block_stream_elapsed_ns=50\n"
                "internal_block_stream_start_unix_ns=20\n"
                "internal_block_stream_end_unix_ns=30\n"
                "internal_output_validation_available=false\n"
            )
            samples.write_text("".join(json.dumps({
                "start_unix_ns": start, "end_unix_ns": end,
                "rss_bytes": rss, "temporary_bytes": temporary_bytes,
            }) + "\n" for start, end, rss, temporary_bytes in [
                (10, 12, 40, 70), (19, 21, 999, 999), (22, 23, 30, 50),
            ]))

            result = profile_conversion.internal_phase_peaks(stdout, samples)

            phases = result["phases"]
            self.assertEqual(result["unassigned_sample_count"], 1)
            self.assertEqual(phases[0]["rss_sampled_peak_bytes"], 40)
            self.assertEqual(phases[0]["temporary_sampled_peak_bytes"], 70)
            self.assertEqual(phases[1]["rss_sampled_peak_bytes"], 30)
            self.assertEqual(phases[2]["elapsed_ns"], None)
            self.assertEqual(phases[2]["sample_count"], 0)

    def test_internal_phase_attribution_rejects_missing_and_overlapping_windows(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stdout, samples = root / "stdout.log", root / "samples.jsonl"
            samples.write_text(json.dumps({
                "start_unix_ns": 15, "end_unix_ns": 16,
                "rss_bytes": 1, "temporary_bytes": 1,
            }) + "\n")
            stdout.write_text("")
            self.assertFalse(profile_conversion.internal_phase_peaks(stdout, samples)["available"])

            lines = []
            for name, start, end in (("spatial_planning", 10, 20), ("block_stream", 15, 25)):
                lines.extend((
                    f"internal_{name}_available=true",
                    f"internal_{name}_elapsed_ns=10",
                    f"internal_{name}_start_unix_ns={start}",
                    f"internal_{name}_end_unix_ns={end}",
                ))
            lines.append("internal_output_validation_available=false")
            stdout.write_text("\n".join(lines) + "\n")
            with self.assertRaises(ValueError):
                profile_conversion.internal_phase_peaks(stdout, samples)

    def test_phase_peaks_exclude_transitions_and_do_not_attribute_lifetime_hwm(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stdout = root / "stdout.log"
            samples = root / "samples.jsonl"
            stdout.write_text("noise=ignored\n" + "".join(
                f"phase_{index}_name={name}\nphase_{index}_start_unix_ns={start}\n"
                f"phase_{index}_end_unix_ns={end}\n"
                for index, (name, start, end) in enumerate([
                    ("Planning", 10, 20), ("Writing", 20, 30),
                    ("Planning", 30, 31), ("Complete", 31, 40),
                ])
            ))
            samples.write_text("".join(json.dumps({
                "start_unix_ns": start, "end_unix_ns": end,
                "rss_bytes": rss, "temporary_bytes": disk, "vmhwm_bytes": 9999,
            }) + "\n" for start, end, rss, disk in [
                (0, 5, 999, 999), (10, 12, 50, 100), (13, 19, 70, 90),
                (19, 21, 900, 900), (21, 25, 30, 200), (30, 32, 800, 800),
                (33, 35, None, 0), (40, 41, 999, 999),
            ]))
            result = profile_conversion.phase_peaks(stdout, samples)
            phases = result["phases"]
            self.assertEqual(result["phase_unassigned_sample_count"], 4)
            self.assertEqual([phase["sample_count"] for phase in phases], [2, 1, 0, 1])
            self.assertEqual([phase["rss_sampled_peak_bytes"] for phase in phases],
                             [70, 30, None, None])
            self.assertEqual([phase["temporary_sampled_peak_bytes"] for phase in phases],
                             [100, 200, None, 0])
            self.assertEqual([phase["ordinal"] for phase in phases], [0, 1, 2, 3])

    def test_phase_attribution_rejects_missing_duplicate_and_invalid_boundaries(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stdout, samples = root / "stdout.log", root / "samples.jsonl"
            samples.write_text("")
            valid = "phase_0_name=Planning\nphase_0_start_unix_ns=10\nphase_0_end_unix_ns=20\n"
            for content in ("", "phase_0_name=Planning\n", valid + "phase_0_name=Other\n",
                            valid.replace("=20", "=9"), valid.replace("phase_0", "phase_1"),
                            valid + valid.replace("phase_0", "phase_1")):
                stdout.write_text(content)
                with self.subTest(content=content), self.assertRaises(ValueError):
                    profile_conversion.phase_peaks(stdout, samples)
            stdout.write_text(valid)
            for intervals in ([(15, 14)], [(15, 16), (14, 17)]):
                samples.write_text("".join(json.dumps({
                    "start_unix_ns": start, "end_unix_ns": end,
                    "rss_bytes": 10, "temporary_bytes": 20,
                }) + "\n" for start, end in intervals))
                with self.subTest(intervals=intervals), self.assertRaises(ValueError):
                    profile_conversion.phase_peaks(stdout, samples)

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
                "start = time.time_ns()\n"
                "output = pathlib.Path(sys.argv[3])\n"
                "output.write_bytes(b'final-oct')\n"
                "output.with_suffix('.octattrs').write_bytes(b'final-attrs')\n"
                "work = output.parent / 'sample.work'\n"
                "work.mkdir()\n"
                "(work / 'source.spool').write_bytes(b'x' * 1000)\n"
                "(output.parent / 'candidate.tmp').write_bytes(b't' * 20)\n"
                "time.sleep(0.08)\n"
                "print('phase_0_name=Fake')\n"
                "print(f'phase_0_start_unix_ns={start}')\n"
                "print(f'phase_0_end_unix_ns={time.time_ns()}')\n"
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
                self.assertEqual(report["phases"][0]["name"], "Fake")

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

    def test_exit_zero_with_output_pair_but_no_phase_metadata_is_incomplete(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            input_path = root / "input.las"
            input_path.write_bytes(b"input")
            fake = root / "legacy-converter"
            fake.write_text(
                "#!/usr/bin/env python3\n"
                "import pathlib, sys\n"
                "output = pathlib.Path(sys.argv[3])\n"
                "output.write_bytes(b'coordinates')\n"
                "output.with_suffix('.octattrs').write_bytes(b'attributes')\n"
            )
            fake.chmod(0o755)
            run_dir = root / "run"
            self.assertEqual(profile_conversion.profile(input_path, run_dir, fake, 1, 2, 10), 1)
            report = json.loads((run_dir / "report.json").read_text())
            self.assertEqual(report["exit_code"], 0)
            self.assertEqual(report["status"], "incomplete")
            self.assertEqual(report["phases"], [])
            self.assertTrue(any("phase attribution" in error for error in report["observation_errors"]))


if __name__ == "__main__":
    unittest.main()
