import importlib.util
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "sample_process_memory", Path(__file__).parents[1] / "sample-process-memory.py"
)
memory = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(memory)


class ProcessMemoryTests(unittest.TestCase):
    def process(self, root, pid, parent, start, rollup=True):
        entry = root / str(pid)
        entry.mkdir()
        fields = ["S", str(parent), *(["0"] * 17), str(start)]
        (entry / "stat").write_text(f"{pid} (name with ) spaces) {' '.join(fields)}")
        if rollup:
            (entry / "smaps_rollup").write_text(
                "Rss: 12 kB\nPss: 8 kB\nPss_Anon: 6 kB\n"
            )

    def test_tree_excludes_unrelated_process_and_handles_complex_comm(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.process(root, 10, 1, 100)
            self.process(root, 11, 10, 101)
            self.process(root, 12, 11, 102)
            self.process(root, 20, 1, 200)
            sample = memory.snapshot(10, 100, root)
            self.assertTrue(sample["enumerated_processes_readable"])
            self.assertEqual([p["pid"] for p in sample["processes"]], [10, 11, 12])
            self.assertEqual(sample["tree_bytes"]["Pss"], 3 * 8 * 1024)
            self.assertIsNone(sample["tree_bytes"]["Pss_File"])
            self.assertFalse(memory.snapshot(10, 999, root)["root_alive"])

    def test_missing_child_rollup_marks_partial_instead_of_zero_complete(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.process(root, 10, 1, 100)
            self.process(root, 11, 10, 101, rollup=False)
            sample = memory.snapshot(10, 100, root)
            self.assertFalse(sample["enumerated_processes_readable"])
            self.assertEqual(sample["unreadable_pids"], [11])
            self.assertEqual(sample["tree_bytes"]["Pss"], 8 * 1024)

    def test_rejects_missing_or_malformed_required_counters(self):
        for text in (
            "Rss: 1 kB\n",
            "Rss: -1 kB\nPss: 1 kB\n",
            "Rss: 1 MB\nPss: 1 kB\n",
        ):
            with self.subTest(text=text), self.assertRaises(ValueError):
                memory.memory_fields(text)
