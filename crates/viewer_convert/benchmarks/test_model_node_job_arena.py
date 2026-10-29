"""Hand-computed checks of the per-node job arena model."""
import unittest

import model_node_job_arena as model


def event(node_id, parent_id, depth, input_bytes, children=(), lod=0, elapsed=1, leaf=False):
    return {"node_id": node_id, "parent_id": parent_id, "depth": depth,
            "input_spool_bytes": input_bytes, "lod_bytes": lod, "is_leaf": leaf,
            "became_leaf_all_equal": False, "elapsed_ns": elapsed,
            "start_seq": node_id, "end_seq": 100 - node_id, "child_spool_bytes": list(children)}


# Chain: LAS root -> X -> Y -> two leaves; sizes chosen so the three variants differ.
CHAIN = [
    event(0, None, 0, 0, [100], lod=10, elapsed=5),
    event(1, 0, 1, 100, [80], lod=5, elapsed=3),
    event(2, 1, 2, 80, [50, 30], lod=3, elapsed=2),
    event(3, 2, 3, 50, leaf=True),
    event(4, 2, 3, 30, leaf=True),
]


class ModelTests(unittest.TestCase):
    def test_variants_on_hand_computed_chain(self):
        runs = {variant: model.simulate(CHAIN, 4, "fifo", variant)
                for variant in ("a", "b", "c", "own")}
        # a: no arena is ever freed (LOD pins it); b, c and own free consumed spools.
        self.assertEqual({v: r["peak_bytes"] for v, r in runs.items()},
                         {"a": 278, "b": 195, "c": 195, "own": 195})
        # Live files: a = three arenas; b = retained + two arenas; c = retained + two spools.
        self.assertEqual([runs[v]["max_live_files"] for v in "abc"], [3, 3, 3])
        # r: the one branch (X and Y spools, 180 B) is freed only when Y ends, after the LOD of Y.
        branch = model.simulate(CHAIN, 4, "fifo", "r")
        self.assertEqual((branch["peak_bytes"], branch["max_live_files"]), (278, 2))

    def test_policies_and_metrics_on_chain(self):
        for policy in model.POLICIES:
            self.assertEqual(model.simulate(CHAIN, 8, policy, "b")["peak_bytes"], 195)
        self.assertEqual(model.simulate(CHAIN, 4, "fifo", "b")["makespan_ns"], 10)
        self.assertEqual(model.critical_path_ns(CHAIN), 10)
        self.assertEqual(model.source_bytes(CHAIN), 100)

    def test_parse_log_reads_events_and_scalars(self):
        text = "\n".join([
            "internal_spatial_peak_live_spool_bytes=7",
            "internal_spatial_spool_bytes_written=9",
            "internal_spatial_node_event=0,-,0,0,10,0,0,5,0,9,60;40",
            "internal_spatial_node_event=1,0,1,60,0,1,1,2,1,2,",
        ])
        parsed = model.parse_log(text)
        self.assertEqual(parsed["native_peak_live_spool_bytes"], 7)
        self.assertEqual(parsed["events"][0]["child_spool_bytes"], [60, 40])
        self.assertTrue(parsed["events"][1]["is_leaf"])
        self.assertIsNone(parsed["events"][0]["parent_id"])
        with self.assertRaises(ValueError):
            model.parse_log("nothing")


if __name__ == "__main__":
    unittest.main()
