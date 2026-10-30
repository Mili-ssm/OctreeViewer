"""Hand-computed checks of the zone-paged spill model (stage 4, task 2).

Tiny units: flush 10 B, segments 10 B (small) and 40 B (big), pages 40 B, zones of 2 pages.
FIVE (R writes A=60 inner and B=40 leaf, LOD 5; A writes leaves 35 and 25, LOD 3) is run with
LIFO order, which is irrelevant with a single ready job.
"""
import math
import unittest

import model_hybrid_zones as zones
from test_model_hybrid_spools import FIVE

UNITS = dict(flush=10, page=40, zone_pages=2, small=10, big=40)


def run(segment, policy, budget):
    return zones.simulate_zones(FIVE, segment, policy, budget, **UNITS)


class SegmentModeTests(unittest.TestCase):
    # Without a budget the peak is when A's LOD is appended, before A's input (60 B) is freed.
    # Payload then: 60 + 40 + 5 + 35 + 25 + 3 = 168 B.
    def test_fixed_big_segments(self):
        # caps: A 80 (2 x 40), B 40, LOD 40, leaves 40 + 40, LOD 40 = 280
        r = run("fixed_1mib", "p3", math.inf)
        self.assertEqual((r["peak_held_bytes"], r["peak_waste_bytes"]), (280, 112))
        self.assertEqual((r["spilled_bytes"], r["pages_hwm"], r["reread_bytes"]), (0, 0, 0))

    def test_progressive_segments(self):
        # segments 10, 20, 40: caps A 70, B 70, LOD 10, leaves 70 + 30, LOD 10 = 260
        r = run("progressive", "p3", math.inf)
        self.assertEqual((r["peak_held_bytes"], r["peak_waste_bytes"]), (260, 92))

    def test_fixed_small_segments(self):
        # caps A 60, B 40, LOD 10, leaves 40 + 30, LOD 10 = 190
        r = run("fixed_32kib", "p3", math.inf)
        self.assertEqual((r["peak_held_bytes"], r["peak_waste_bytes"]), (190, 22))


class SpillPolicyTests(unittest.TestCase):
    # Budget 100 with 10 B segments. R's writes reach 100 exactly; R's LOD (held 110) forces the
    # first spill. Then A's writes (order s3 s4 s3 s4 s3 s3 s4, then LOD) force more.
    def keys(self, result):
        return (result["peak_held_bytes"], result["spilled_bytes"], result["pages_hwm"],
                result["reread_input_bytes"], result["reread_block_bytes"])

    def test_oldest_closed_spills_the_input_before_it_is_read(self):
        # R's LOD: oldest closed is A (60 B -> pages 0,1). A's job re-reads all 60 B. Later
        # B (40 B, page 2) is spilled when the 4th leaf flush arrives; block phase re-reads B.
        r = run("fixed_32kib", "oldest_closed", 100)
        self.assertEqual(self.keys(r), (100, 100, 3, 60, 40))
        self.assertEqual((r["zones"], r["footprint_peak_bytes"]), (2, 120))
        self.assertEqual(r["open_spilled_bytes"], 0)

    def test_p3_order_never_spills_the_running_input(self):
        # R's LOD: retained B (40 B). A's writes: retained R-LOD (5 B), then the open leaf of
        # 30 B (+5 B appended to its page). No input re-read; block phase re-reads 40+5+35.
        r = run("fixed_32kib", "p3", 100)
        self.assertEqual(self.keys(r), (100, 80, 3, 0, 80))
        self.assertEqual(r["open_spilled_bytes"], 30)

    def test_pinned_input_is_never_spilled_while_it_is_read(self):
        # Hinted: B, then R-LOD, then the oldest closed spool left is the running input A (60 B).
        # A is pinned (its reader is partitioning it), so the open leaf of 30 B goes instead:
        # the outcome equals the p3 order and A is never re-read.
        r = run("fixed_32kib", "hinted", 100)
        self.assertEqual(self.keys(r), (100, 80, 3, 0, 80))
        self.assertEqual((r["open_spilled_bytes"], r["over_budget_bytes"]), (30, 0))

    def test_keep_small_spools_in_ram_can_break_the_budget(self):
        # Page 40 B: only B (40 B) is spillable in p3 order; R-LOD (5 B) and the leaves
        # (30, 20 B) are smaller than a page and stay. Held grows to 70 + 10 x 8 = 150 B.
        r = zones.simulate_zones(FIVE, "fixed_32kib", "p3", 100, small_spools="keep", **UNITS)
        self.assertEqual((r["peak_held_bytes"], r["over_budget_bytes"], r["spilled_bytes"],
                          r["pages_hwm"], r["small_spills"]), (150, 50, 40, 1, 0))

    def test_small_spills_are_counted(self):
        # p3 order spills R-LOD (5 B) and the open leaf (30 B), both below one 40 B page.
        r = run("fixed_32kib", "p3", 100)
        self.assertEqual((r["small_spills"], r["spill_freed_bytes"]), (2, 40 + 10 + 30))
        self.assertEqual(r["peak_pages_payload_bytes"], 80)

    def test_everything_is_released_at_the_end(self):
        for policy in zones.SPILL_POLICIES:
            r = run("fixed_32kib", policy, 100)
            self.assertEqual((r["final_held_bytes"], r["final_pages"]), (0, 0))

    def test_unlimited_budget_never_touches_pages(self):
        for policy in zones.SPILL_POLICIES:
            self.assertEqual(run("progressive", policy, math.inf)["pages_hwm"], 0)


class PageStoreTests(unittest.TestCase):
    def test_free_list_reuses_the_lowest_page_and_truncates_empty_zones(self):
        store = zones.PageStore(2)
        pages = [store.alloc() for _ in range(5)]  # zones 0, 0, 1, 1, 2
        self.assertEqual((pages, store.max_zone, store.footprint_pages), ([0, 1, 2, 3, 4], 2, 5))
        store.release(3)
        store.release(1)
        self.assertEqual((store.truncations, store.footprint_pages), (0, 5))
        self.assertEqual(store.alloc(), 1)  # lowest free page first
        store.release(2)  # zone 1 empty -> truncated, its 2 pages leave the footprint
        self.assertEqual((store.truncations, store.footprint_pages), (1, 3))
        self.assertEqual(store.alloc(), 2)  # truncated zone is reusable
        self.assertEqual(store.in_use, 4)


class PackPoolTests(unittest.TestCase):
    def test_small_spools_share_pages_and_release_them(self):
        store = zones.PageStore(8)
        pool = zones.PackPool(store, 40)
        pool.change(30)
        pool.change(10)
        self.assertEqual(store.in_use, 1)  # 40 B fit one page
        pool.change(10)
        self.assertEqual(store.in_use, 2)
        pool.change(-30)  # 20 B left
        self.assertEqual(store.in_use, 1)

    def test_pack_never_needs_more_pages_than_spilling_each(self):
        for policy in zones.SPILL_POLICIES:
            plain = run("fixed_32kib", policy, 100)
            packed = zones.simulate_zones(FIVE, "fixed_32kib", policy, 100, small_spools="pack", **UNITS)
            self.assertLessEqual(packed["pages_hwm"], plain["pages_hwm"])


class EstimateTests(unittest.TestCase):
    # 100 M points x 40 B = 4 GB of source payload.
    def test_hand_computed_estimate(self):
        e = zones.estimate(100_000_000, 40, 2, 262_144, 2 * zones.GIB)
        self.assertAlmostEqual(e["live_spool_bytes"], 1.1 * 4e9 * 1.625)  # 7.15 GB
        self.assertEqual(e["nodes"], 1832)  # ceil(2.4 x 2 x 1e8 / 262144)
        self.assertEqual(e["opfs_bytes"], 7.15e9 + 1832 * 131072)
        self.assertEqual(e["zones"], 15)  # ceil(2 x 7.39 GB / 1 GiB) + 1
        self.assertAlmostEqual(e["output_bytes"], 2.4e9)

    def test_live_estimate_is_capped_at_twice_the_source(self):
        self.assertAlmostEqual(zones.estimate(100_000_000, 40, 1, 262_144, 0)["live_spool_bytes"], 8e9)

    def test_nothing_spills_when_the_budget_holds_the_live_peak(self):
        e = zones.estimate(100_000_000, 40, 8, 262_144, 64 * zones.GIB)
        self.assertAlmostEqual(e["live_spool_bytes"], 1.1 * 4e9 * (1 + 2.5 / 64))
        self.assertEqual((e["opfs_bytes"], e["zones"]), (0, 0))


if __name__ == "__main__":
    unittest.main()
