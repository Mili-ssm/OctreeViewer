import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).parents[1] / "collect-benchmarks.py"
SPEC = importlib.util.spec_from_file_location("collect_benchmarks", MODULE_PATH)
collect_benchmarks = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(collect_benchmarks)


def run(*, fps=50, complete=True, pending=0, measurement_ms=10_000, camera_changed=False):
    return {
        "schemaVersion": 1,
        "dataset": {
            "name": "albuera.oct",
            "fileBytes": 1024,
            "sourcePointCount": 1234,
        },
        "configuration": {
            "viewportFramebuffer": [640, 480],
            "targetVoxelPixels": 4,
            "colorMode": "rgb",
            "depthShading": True,
        },
        "load": {
            "metadataOpenMs": 10,
            "firstCoarseFrameMs": 20,
            "targetLodMs": 30,
        },
        "steadyState": {
            "cameraChanged": camera_changed,
            "viewportChanged": False,
            "settingsChanged": False,
            "measurementWindowMs": measurement_ms,
            "meanFps": fps,
            "frameReplyIntervalP95Ms": 21,
            "workerRenderCallP95Ms": 7,
        },
        "scene": {
            "coverageComplete": complete,
            "pendingBlocks": pending,
            "renderedVoxels": 88,
        },
        "environment": {
            "userAgent": "test browser",
            "gpuStats": {"adapter": {"name": "test adapter"}},
            "jsHeapUsedBytes": 4096,
        },
    }


class BenchmarkCollectorTests(unittest.TestCase):
    def test_only_stable_complete_runs_are_comparable(self):
        self.assertTrue(collect_benchmarks.comparable(run()))
        self.assertFalse(collect_benchmarks.comparable(run(camera_changed=True)))
        self.assertFalse(collect_benchmarks.comparable(run(complete=False)))
        self.assertFalse(collect_benchmarks.comparable(run(pending=1)))
        self.assertFalse(collect_benchmarks.comparable(run(measurement_ms=9_999)))
        self.assertFalse(collect_benchmarks.comparable(run(fps=None)))

    def test_collect_reports_valid_count_and_aggregates_only_valid_runs(self):
        with tempfile.TemporaryDirectory() as directory:
            paths = []
            for index, sample in enumerate([run(fps=50), run(fps=60), run(fps=90, pending=1)]):
                path = Path(directory) / f"run-{index}.json"
                path.write_text(json.dumps(sample), encoding="utf-8")
                paths.append(path)

            rows = collect_benchmarks.collect(paths)

        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0][7], "2/3")
        self.assertEqual(rows[0][11], "55.00 [50.00–60.00]")
        self.assertEqual(rows[0][14], "88.00 [88.00–88.00]")

    def test_collect_separates_unknown_absent_and_different_sidecars(self):
        samples = [run() for _ in range(4)]
        samples[1]["dataset"]["attributes"] = None
        samples[2]["dataset"]["attributes"] = {"name": "cloud.octattrs", "fileBytes": 100}
        samples[3]["dataset"]["attributes"] = {"name": "cloud.octattrs", "fileBytes": 200}
        with tempfile.TemporaryDirectory() as directory:
            paths = []
            for index, sample in enumerate(samples):
                path = Path(directory) / f"run-{index}.json"
                path.write_text(json.dumps(sample), encoding="utf-8")
                paths.append(path)
            rows = collect_benchmarks.collect(paths)
        self.assertEqual(len(rows), 4)
        self.assertTrue(all(row[7] == "1/1" for row in rows))
        self.assertEqual(
            {row[-4] for row in rows},
            {"unknown (legacy)", "none", "cloud.octattrs (100 B)", "cloud.octattrs (200 B)"},
        )

    def test_collect_uses_reload_timing_and_rejects_pending_or_stale_refinement(self):
        sample = run()
        sample["refinement"] = {
            "trigger": "color", "colorMode": "rgb", "firstFrameMs": 40, "targetLodMs": 90,
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "reload.json"
            path.write_text(json.dumps(sample), encoding="utf-8")
            initial_path = Path(directory) / "initial.json"
            initial_path.write_text(json.dumps(run()), encoding="utf-8")
            rows = collect_benchmarks.collect([path, initial_path])
        self.assertEqual(len(rows), 2)
        rows = [row for row in rows if row[-1] == "color"]
        self.assertEqual(rows[0][9:11], ["40.00", "90.00"])
        self.assertEqual(rows[0][-1], "color")
        self.assertTrue(collect_benchmarks.comparable(sample))
        sample["refinement"]["colorMode"] = "intensity"
        self.assertFalse(collect_benchmarks.comparable(sample))
        sample["refinement"] = None
        self.assertFalse(collect_benchmarks.comparable(sample))

    def test_collect_rejects_unknown_schema_versions(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "unsupported.json"
            path.write_text(json.dumps({"schemaVersion": 2}), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "unsupported benchmark schema"):
                collect_benchmarks.collect([path])

    def test_csv_cell_neutralizes_formulas_but_keeps_numbers(self):
        cell = collect_benchmarks.csv_cell
        for text in ["=1+1", "+cmd", "-cmd", "@SUM(A1)", "\tx", "\rx"]:
            self.assertEqual(cell(text), "'" + text)
        self.assertEqual(cell("normal"), "normal")
        self.assertEqual(cell(-1.5), -1.5)
        self.assertEqual(cell("-1.50"), "-1.50")
        self.assertEqual(cell("-1.50 [-3.00–2.00]"), "-1.50 [-3.00–2.00]")
        self.assertEqual(cell("—"), "—")


if __name__ == "__main__":
    unittest.main()
