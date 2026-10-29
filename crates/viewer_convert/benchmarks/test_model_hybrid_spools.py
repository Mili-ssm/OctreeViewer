"""Hand-computed checks of the hybrid spool RAM model (S10)."""
import unittest

import model_hybrid_spools as model
import model_node_job_arena as arena


def event(node_id, parent_id, depth, input_bytes, children=(), lod=0, leaf=False):
    return {"node_id": node_id, "parent_id": parent_id, "depth": depth,
            "input_spool_bytes": input_bytes, "lod_bytes": lod, "is_leaf": leaf,
            "became_leaf_all_equal": False, "elapsed_ns": 1,
            "start_seq": node_id, "end_seq": 100 - node_id, "child_spool_bytes": list(children)}


# R reads the source (100 B) and writes A (60, inner) and B (40, leaf); A writes leaves D, E.
FIVE = [
    event(0, None, 0, 100, [60, 40], lod=5),
    event(1, 0, 1, 60, [35, 25], lod=3),
    event(2, 0, 1, 40, leaf=True),
    event(3, 1, 2, 35, leaf=True),
    event(4, 1, 2, 25, leaf=True),
]
# R writes inner A and B (50 each); both write two leaves. A is consumed first (FIFO).
FORK = [
    event(0, None, 0, 100, [50, 50]),
    event(1, 0, 1, 50, [30, 20]),
    event(2, 0, 1, 50, [25, 25]),
    event(3, 1, 2, 30, leaf=True),
    event(4, 1, 2, 20, leaf=True),
    event(5, 2, 2, 25, leaf=True),
    event(6, 2, 2, 25, leaf=True),
]
KEYS = ("peak_ram_bytes", "spilled_bytes", "files_created", "handles_opened")


def run(events, mode, budget, **kwargs):
    result = model.simulate_hybrid(events, 1, "fifo", mode, budget, **kwargs)
    return tuple(result[key] for key in KEYS)


class HybridModelTests(unittest.TestCase):
    def test_p0_keeps_everything_in_ram(self):
        # R: 100 children, +5 LOD = 105; A: +60 children = 165, +3 LOD = 168, then A's input dies.
        self.assertEqual(run(FIVE, "p0", 0), (168, 0, 0, 0))

    def test_p0_native_layout_equals_variant_c(self):
        for workers in (1, 4):
            for order in arena.POLICIES:
                hybrid = model.simulate_hybrid(FIVE, workers, order, "p0", 0, root_input_spool=True)
                self.assertEqual(hybrid["peak_ram_bytes"],
                                 arena.simulate(FIVE, workers, order, "c")["peak_bytes"])

    def test_p1_admits_by_worst_case_output(self):
        # B=110: R bound 100 + 5 LOD = 105 fits (RAM 105). A bound 60 + 3 = 63 > 5 free:
        # handles for D, E, LOD (3), 60 + 3 B to disk, 3 files.
        self.assertEqual(run(FIVE, "p1", 110), (105, 63, 3, 3))
        # B=104: R's worst case (105) does not fit -> R gets handles; A (63) then fits in RAM.
        self.assertEqual(run(FIVE, "p1", 104), (63, 105, 3, 3))
        # B=170: A's worst case 63 <= 65 free -> everything in RAM = p0.
        self.assertEqual(run(FIVE, "p1", 170), run(FIVE, "p0", 0))

    def test_p1_without_budget_equals_p2(self):
        # Every job opens len(children)+1 = 3 handles; every nonempty spool is a file:
        # R: A, B, LOD; A: D, E, LOD. 6 files, 100 + 5 + 60 + 3 bytes.
        self.assertEqual(run(FIVE, "p2", 0), (0, 168, 6, 6))
        self.assertEqual(run(FIVE, "p1", 0), run(FIVE, "p2", 0))
        self.assertEqual(model.simulate_hybrid(FIVE, 1, "fifo", "p2", 0)["max_open_handles"], 3)

    def test_nominal_divisions_count_empty_handles(self):
        # divisions=2 -> 2**3 + 1 = 9 handles per job; R uses 3 (A, B, LOD), A uses 3.
        result = model.simulate_hybrid(FIVE, 1, "fifo", "p2", 0, divisions=2)
        self.assertEqual((result["handles_opened"], result["empty_handles"], result["disk_jobs"]),
                         (18, 12, 2))

    def test_p1r_spills_for_room_instead_of_opening_handles(self):
        # B=125: A needs 63, RAM 105 -> spill leaf B (40) and R's LOD (5) -> RAM 60 -> A runs in
        # RAM: 60 + 60 + 3 LOD = 123 <= 125; 2 spills, 45 B.
        self.assertEqual(run(FIVE, "p1r", 125), (123, 45, 2, 2))
        # B=120: spilling everything spillable (45 B) still leaves 60 > 57 -> handles instead.
        self.assertEqual(run(FIVE, "p1r", 120), (105, 63, 3, 3))

    def test_p1r_spills_queued_inputs_consumed_latest(self):
        # B=110: R in RAM (100). A needs 50, free 10: spill queued B-input (50, consumed after A);
        # A then runs in RAM (50 + 50), B reads from disk and also fits.
        self.assertEqual(run(FORK, "p1r", 110), (100, 50, 1, 1))
        # Without room spilling A needs handles (2 for its leaves, no LOD, 50 B on disk); B then fits.
        self.assertEqual(run(FORK, "p1", 110), (100, 50, 2, 2))

    def test_p3_spills_unfinished_outputs_without_job_handles(self):
        # B=100: R reserves 100 (no spill). R's LOD -> 105: spill leaf B (40) -> 65. A starts: +60
        # = 125 -> spill R's LOD (5), then A's unfinished output D (35, largest) -> 85.
        # 3 spills, 80 B, one handle each, none opened by a job.
        result = model.simulate_hybrid(FIVE, 1, "fifo", "p3", 100)
        self.assertEqual([result[k] for k in KEYS], [100, 80, 3, 3])
        self.assertEqual((result["job_handles"], result["inprogress_spilled_bytes"]), (0, 35))

    def test_concurrency_counts_open_handles(self):
        result = model.simulate_hybrid(FORK, 2, "fifo", "p2", 0)
        self.assertEqual(result["max_open_handles"], 4)  # A and B run together, 2 handles each

    def test_cost_uses_s9_rates(self):
        self.assertEqual(model.cost_s(1000), {"firefox": [22.0, 40.0], "chrome": [2.9, 4.4]})
        # 513 handles at divisions 8: about 11.3 s (22 ms) to 20.5 s (40 ms) in Firefox.
        self.assertEqual(model.cost_s(513)["firefox"], [11.3, 20.5])
        self.assertEqual(model.cost_s(0, 1000)["firefox"], [25.0, 40.0])

    def test_ranged_root_handles(self):
        rows = model.ranged_root(FIVE, 2)  # root has 2 non-empty children of 8
        self.assertEqual((rows["w4"]["handles"], rows["w4"]["empty_lower_bound"]), (33, 24))

    def test_rejects_unknown_mode(self):
        with self.assertRaises(ValueError):
            model.simulate_hybrid(FIVE, 1, "fifo", "p9", 0)


if __name__ == "__main__":
    unittest.main()
