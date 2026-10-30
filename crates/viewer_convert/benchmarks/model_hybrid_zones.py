#!/usr/bin/env python3
"""Model the zone-paged spill policy of the web hybrid spool store (stage 4, decision 5).

Builds on the S10 model (`model_hybrid_spools`) and the node-job arena model: the same node
events and the same variant-c semantics (a job creates its child spools when it starts, its LOD
spool when it ends, and frees its input spool when it ends; leaf and LOD spools are retained
until the block phase). Differences from S10, which counted a job's children in full at start:
here spools grow in 32 KiB flushes (the core's ChildSpool flush size), interleaved in proportion
to their final sizes, so the peak is a tighter bound than S10's. N = 1 worker, depth-first order (the native serial one), web layout (the
root partition reads the Blob, so it has no input spool).

RAM tier: a spool owns segments (`fixed_1mib`, `progressive` 32 KiB doubling to 1 MiB, or
`fixed_32kib`); held bytes are the sum of segment capacities, payload is what was appended. A
segment is not trimmed on close, except in the extra mode `progressive_trim` (the last segment
shrinks to its payload, e.g. `ArrayBuffer.prototype.transfer(length)` or a copy). After every append, while held > budget whole spools are
spilled to pages; a spilled spool keeps appending to pages.

Page tier: 1 MiB pages from a free list (lowest free index first, so zones fill in order),
zones of `zone_pages` pages (1 GiB), a zone that becomes empty is truncated. A spilled spool
owns ceil(bytes / page) pages (the last one partially filled). A spool read back from pages is
not promoted to RAM: the reader copies from the sync handle straight into wasm.

Re-reads count bytes read back from pages: `reread_input_bytes` is a consumer job whose input
was spilled (the whole input if spilled before the job starts, the unread share, proportional
to the job's output progress, if spilled during it); `reread_block_bytes` is the block phase
reading retained leaf/LOD spools that sit on pages (unavoidable for retained spools).

Spill policies (open spools are always last, largest first):
  oldest_closed  closed spools, oldest-closed first, no hints (the running job's input included).
  p3             S10 order: retained leaf/LOD largest first, then queued inputs (consumed
                 latest first), the running input never.
  hinted         retained leaf/LOD oldest-closed first, then the other closed spools
                 (including the running input) oldest-closed first.
"""

from __future__ import annotations

import argparse
import heapq
import json
import math
from pathlib import Path
import re
import sys
from typing import Any

import model_hybrid_spools as s10
from model_node_job_arena import POLICIES, parse_log

KIB = 1 << 10
MIB = 1 << 20
GIB = 1 << 30
FLUSH = 32 * KIB
PAGE = MIB
ZONE_PAGES = 1024  # 1 GiB zones
SEGMENTS = {"fixed_1mib": (MIB, MIB), "progressive": (32 * KIB, MIB), "fixed_32kib": (32 * KIB, 32 * KIB),
            "progressive_trim": (32 * KIB, MIB)}
BASE_SEGMENT = "progressive_trim"  # segment mode of the page-size, small-spool and order sweeps
# Ready-job orders: `dfs` is the native serial recursion (first child's subtree before the next
# child, `process_child_tasks`); S10's fifo/lifo/largest are kept as sensitivity.
ORDERS = ("dfs", *POLICIES)
SPILL_POLICIES = ("oldest_closed", "p3", "hinted")
# Size-aware handling of spools smaller than one page: spill them like any other, keep them in
# RAM, or pack closed ones together into shared pages (idealised: perfect compaction).
SMALL_SPOOLS = ("spill", "keep", "pack")


class PageStore:
    """Pages with a free list over zones; the lowest free index is allocated first."""

    def __init__(self, zone_pages: int) -> None:
        self.zone_pages = zone_pages
        self.free: list[int] = []
        self.next_new = 0
        self.used: dict[int, int] = {}
        self.high: dict[int, int] = {}  # per zone: pages from the start up to the highest written
        self.in_use = 0
        self.footprint_pages = 0
        self.truncations = 0
        self.max_zone = -1

    def alloc(self) -> int:
        if self.free:
            index = heapq.heappop(self.free)
        else:
            index = self.next_new
            self.next_new += 1
        zone, offset = divmod(index, self.zone_pages)
        self.used[zone] = self.used.get(zone, 0) + 1
        if offset + 1 > self.high.get(zone, 0):
            self.footprint_pages += offset + 1 - self.high.get(zone, 0)
            self.high[zone] = offset + 1
        self.max_zone = max(self.max_zone, zone)
        self.in_use += 1
        return index

    def release(self, index: int) -> None:
        zone = index // self.zone_pages
        heapq.heappush(self.free, index)
        self.used[zone] -= 1
        self.in_use -= 1
        if not self.used[zone]:  # empty zone: truncate(0) frees its quota
            self.footprint_pages -= self.high[zone]
            self.high[zone] = 0
            self.truncations += 1


class PackPool:
    """Shared pages for small closed spools; models perfect compaction (optimistic)."""

    def __init__(self, store: PageStore, page: int) -> None:
        self.store, self.page, self.bytes, self.pages = store, page, 0, []

    def change(self, delta: int) -> None:
        self.bytes += delta
        need = -(-self.bytes // self.page)
        while len(self.pages) < need:
            self.pages.append(self.store.alloc())
        while len(self.pages) > need:
            self.store.release(self.pages.pop())


class Spool:
    __slots__ = ("sid", "kind", "payload", "cap", "nseg", "where", "pbytes", "pages", "closed",
                 "close_seq", "state", "packed")

    def __init__(self, sid: tuple[str, int], kind: str) -> None:
        self.sid, self.kind, self.payload, self.cap, self.nseg = sid, kind, 0, 0, 0
        self.where, self.pbytes, self.pages, self.closed, self.close_seq = "ram", 0, [], False, 0
        self.state, self.packed = "open", False


class ZoneSim:
    def __init__(self, segment: str, policy: str, budget: float, flush: int, page: int,
                 zone_pages: int, small: int | None = None, big: int | None = None,
                 small_spools: str = "spill") -> None:
        if segment not in SEGMENTS or policy not in SPILL_POLICIES or small_spools not in SMALL_SPOOLS:
            raise ValueError("unknown segment mode, spill policy or small-spool handling")
        self.small_spools = small_spools
        self.first, self.big = SEGMENTS[segment]
        if small is not None:  # tests use tiny sizes
            self.first, self.big = {"fixed_1mib": (big, big), "progressive": (small, big),
                                    "fixed_32kib": (small, small), "progressive_trim": (small, big)}[segment]
        self.trim = segment == "progressive_trim"
        self.policy, self.budget, self.flush, self.page = policy, budget, flush, page
        self.pages = PageStore(zone_pages)
        self.pool = PackPool(self.pages, page)
        self.peak_pages_payload = 0
        self.small_spills = self.small_freed = self.freed = self.over_appends = 0
        self.held = self.ram_payload = self.pages_payload = 0
        self.peak_held = self.peak_waste = self.max_waste = self.over_budget = 0
        self.peak_live = self.hwm = self.hwm_payload = self.footprint_peak = 0
        self.spilled = self.open_spilled = self.spool_spills = 0
        self.reread_input = self.reread_block = 0
        self.close_seq = 0
        self.h_closed: list = []
        self.h_ret: list = []
        self.h_other: list = []
        self.h_queued: list = []
        self.open_ram: set[Spool] = set()
        self.job_done = self.job_total = 0
        self.running: Spool | None = None

    # -- accounting ---------------------------------------------------------------------
    def _touch(self) -> None:
        over = self.held - self.budget
        if over > self.over_budget:
            self.over_budget = over
        waste = self.held - self.ram_payload
        if waste > self.max_waste:
            self.max_waste = waste
        if self.held > self.peak_held:
            self.peak_held, self.peak_waste = self.held, waste
        if over > 0:
            self.over_appends += 1
        live = self.ram_payload + self.pages_payload
        if live > self.peak_live:
            self.peak_live = live
        if self.pages_payload > self.peak_pages_payload:
            self.peak_pages_payload = self.pages_payload
        if self.pages.in_use > self.hwm:
            self.hwm, self.hwm_payload = self.pages.in_use, self.pages_payload
        if self.pages.footprint_pages > self.footprint_peak:
            self.footprint_peak = self.pages.footprint_pages

    def _ensure_pages(self, sp: Spool) -> None:
        need = -(-sp.pbytes // self.page)
        while len(sp.pages) < need:
            sp.pages.append(self.pages.alloc())

    def append(self, sp: Spool, n: int) -> None:
        sp.payload += n
        if sp.where == "ram":
            self.ram_payload += n
            while sp.payload > sp.cap:
                size = self.big if self.first == self.big else min(self.first << sp.nseg, self.big)
                sp.cap += size
                sp.nseg += 1
                self.held += size
            self._enforce()
        else:
            sp.pbytes += n
            self.pages_payload += n
            self.spilled += n
            self._ensure_pages(sp)
        self._touch()

    # -- spilling -----------------------------------------------------------------------
    def _pop_valid(self, heap: list, test) -> Spool | None:
        """Pop the first spillable spool. A reader's spool is pinned: skipped now, kept for later.
        Spools that can never qualify (dead, spilled, small in `keep` mode) are dropped."""
        pinned = []
        found = None
        while heap:
            sp = heap[0][-1]
            entry = heapq.heappop(heap)
            if sp.where != "ram" or not test(sp):
                continue
            if sp is self.running:
                pinned.append(entry)
                continue
            if self.small_spools == "keep" and sp.payload < self.page:
                continue
            found = sp
            break
        for entry in pinned:
            heapq.heappush(heap, entry)
        return found

    def _pick(self) -> Spool | None:
        closed = lambda sp: sp.closed and sp.state != "dead"
        if self.policy == "oldest_closed":
            sp = self._pop_valid(self.h_closed, closed)
        elif self.policy == "hinted":
            sp = self._pop_valid(self.h_ret, closed) or self._pop_valid(self.h_other, closed)
        else:
            sp = (self._pop_valid(self.h_ret, closed)
                  or self._pop_valid(self.h_queued, lambda s: s.state == "queued"))
        if sp is None and self.open_ram:
            keep = self.small_spools == "keep"
            pool = [s for s in self.open_ram if not keep or s.payload >= self.page]
            if pool:
                sp = max(pool, key=lambda s: (s.payload, s.sid))
        return sp

    def _enforce(self) -> None:
        while self.held > self.budget:
            sp = self._pick()
            if sp is None:
                return
            self._spill(sp)

    def _spill(self, sp: Spool) -> None:
        self.held -= sp.cap
        self.freed += sp.cap
        self.ram_payload -= sp.payload
        sp.where, sp.pbytes = "pages", sp.payload
        self.pages_payload += sp.payload
        self.spilled += sp.payload
        self.spool_spills += 1
        if sp.payload < self.page:
            self.small_spills += 1
            self.small_freed += sp.cap
        if not sp.closed:
            self.open_spilled += sp.payload
            self.open_ram.discard(sp)
        if self.small_spools == "pack" and sp.closed and sp.payload < self.page:
            sp.packed = True
            self.pool.change(sp.payload)
        else:
            self._ensure_pages(sp)

    def _delete(self, sp: Spool) -> None:
        if sp.where == "ram":
            self.held -= sp.cap
            self.ram_payload -= sp.payload
            self.open_ram.discard(sp)
        else:
            self.pages_payload -= sp.payload
            if sp.packed:
                self.pool.change(-sp.payload)
            for index in sp.pages:
                self.pages.release(index)
        sp.state = "dead"

    def _close(self, sp: Spool, queue_key: int | None = None) -> None:
        sp.closed = True
        self.close_seq += 1
        sp.close_seq = self.close_seq
        self.open_ram.discard(sp)
        if self.trim and sp.where == "ram":  # shrink the last segment to the payload on close
            self.held -= sp.cap - sp.payload
            sp.cap = sp.payload
        retained = sp.kind != "inner"
        sp.state = "retained" if retained else "queued"
        if self.policy == "oldest_closed":
            heapq.heappush(self.h_closed, (sp.close_seq, sp.sid, sp))
        elif retained:
            key = (-sp.payload, sp.sid) if self.policy == "p3" else (sp.close_seq, sp.sid)
            heapq.heappush(self.h_ret, (*key, sp))
        elif self.policy == "hinted":
            heapq.heappush(self.h_other, (sp.close_seq, sp.sid, sp))
        else:  # p3: consumed latest first = largest ready key first
            heapq.heappush(self.h_queued, (-queue_key, sp.sid, sp))

    def _new(self, sid: tuple[str, int], kind: str) -> Spool:
        sp = Spool(sid, kind)
        self.open_ram.add(sp)
        return sp

    def _write_lod(self, node_id: int, size: int) -> Spool | None:
        if not size:
            return None
        sp = self._new(("lod", node_id), "lod")
        left = size
        while left:
            n = min(self.flush, left)
            self.append(sp, n)
            left -= n
        self._close(sp)
        return sp

    # -- run ----------------------------------------------------------------------------
    def run(self, events: list[dict], order: str) -> dict[str, Any]:
        if order not in ORDERS:
            raise ValueError("unknown order")
        if any(e["became_leaf_all_equal"] for e in events):
            raise ValueError("became_leaf_all_equal nodes are not modelled")
        kids: dict[int | None, list[dict]] = {}
        for event in sorted(events, key=lambda e: e["node_id"]):
            kids.setdefault(event["parent_id"], []).append(event)
        by_id = {e["node_id"]: e for e in events}
        node_spools: dict[int, Spool] = {}
        retained: list[Spool] = []
        ready: list = []
        counter = 0

        def queue_key(event: dict) -> int:  # smaller = consumed earlier (S10 ready order)
            nonlocal counter
            counter += 1
            return {"fifo": counter, "lifo": -counter, "dfs": -counter,
                    "largest": (-event["input_spool_bytes"] << 32) | event["node_id"]}[order]

        for root in kids.get(None, []):
            if not root["is_leaf"]:
                heapq.heappush(ready, (queue_key(root), root["node_id"]))
        while ready:
            _, node_id = heapq.heappop(ready)
            event = by_id[node_id]
            inp = node_spools.get(node_id)
            self.running = inp
            children = [c for c in kids.get(node_id, []) if c["input_spool_bytes"]]
            self.job_total = sum(c["input_spool_bytes"] for c in children)
            self.job_done = 0
            if inp is not None:
                inp.state = "running"
                if inp.where == "pages":
                    self.reread_input += inp.payload
            outs, plan = [], []
            for index, child in enumerate(children):
                sp = self._new(("node", child["node_id"]), "leaf" if child["is_leaf"] else "inner")
                node_spools[child["node_id"]] = sp
                outs.append(sp)
                size = child["input_spool_bytes"]
                flushes = -(-size // self.flush)
                plan.extend(((k + 1) / flushes, index, min(self.flush, size - k * self.flush))
                            for k in range(flushes))
            plan.sort(key=lambda p: (p[0], p[1]))
            for _, index, n in plan:
                self.job_done += n
                self.append(outs[index], n)
            # dfs pops the last-pushed job first, so it pushes the children in reverse order
            keys = {}
            for child in (reversed(children) if order == "dfs" else children):
                if not child["is_leaf"]:
                    keys[child["node_id"]] = queue_key(child)
            for child, sp in zip(children, outs):
                if child["is_leaf"]:
                    self._close(sp)
                    retained.append(sp)
                else:
                    key = keys[child["node_id"]]
                    self._close(sp, key)
                    heapq.heappush(ready, (key, child["node_id"]))
            lod = self._write_lod(node_id, event["lod_bytes"])
            if lod is not None:
                retained.append(lod)
            if inp is not None:
                self._delete(inp)
            self.running = None
        for sp in sorted(retained, key=lambda s: s.sid[::-1]):  # block phase reads them all
            if sp.where == "pages":
                self.reread_block += sp.payload
            self._delete(sp)
        return self.result()

    def result(self) -> dict[str, Any]:
        pages = self.pages
        return {"peak_held_bytes": self.peak_held, "peak_waste_bytes": self.peak_waste,
                "max_waste_bytes": self.max_waste, "over_budget_bytes": self.over_budget,
                "peak_live_payload_bytes": self.peak_live,
                "spilled_bytes": self.spilled, "open_spilled_bytes": self.open_spilled,
                "spool_spills": self.spool_spills,
                "over_budget_appends": self.over_appends, "peak_pages_payload_bytes": self.peak_pages_payload,
                "spill_freed_bytes": self.freed, "small_spills": self.small_spills,
                "small_spill_freed_bytes": self.small_freed,
                "pages_hwm": self.hwm, "pages_hwm_payload_bytes": self.hwm_payload,
                "page_waste_bytes_at_hwm": self.hwm * self.page - self.hwm_payload,
                "opfs_peak_bytes": self.hwm * self.page,
                "footprint_peak_bytes": self.footprint_peak * self.page,
                "zones": pages.max_zone + 1, "zone_truncations": pages.truncations,
                "reread_input_bytes": self.reread_input, "reread_block_bytes": self.reread_block,
                "reread_bytes": self.reread_input + self.reread_block,
                "final_held_bytes": self.held, "final_pages": pages.in_use}


def simulate_zones(events: list[dict], segment: str, policy: str, budget: float, order: str = "dfs",
                   flush: int = FLUSH, page: int = PAGE, zone_pages: int = ZONE_PAGES,
                   small: int | None = None, big: int | None = None,
                   small_spools: str = "spill") -> dict[str, Any]:
    """N = 1 run; budget math.inf = never spill. `small`/`big` override segment sizes (tests)."""
    return ZoneSim(segment, policy, budget, flush, page, zone_pages, small, big,
                   small_spools).run(events, order)


def estimate(points: int, record_bytes: float, divisions: int, points_per_block: int, budget: int,
             page: int = 256 * KIB, zone_bytes: int = GIB) -> dict[str, Any]:
    """Pre-run sizing from the point count, record size, divisions and points per block.

    N = 1, web layout. Calibrated on three traces (Belvedere 2 and 8 divisions, Majadas 2): see
    HYBRID_POLICY_2026-10-30.md. `live_spool_bytes` is the peak of live spool payload (ceiling
    2 x source: a point lives in one spool except while its node's job runs). The OPFS peak is
    about the whole live peak, not live - budget, because spilled spools are never promoted back;
    it is 0 only if the live peak (plus 3 % of segment slack) fits the budget.
    """
    source = points * record_bytes
    live = min(2 * source, 1.1 * source * (1 + min(1.0, 2.5 / divisions ** 2)))
    nodes = math.ceil(2.4 * divisions * points / points_per_block)
    output = 0.6 * source
    opfs = 0.0 if live * 1.03 <= budget else live + nodes * page / 2
    return {"source_payload_bytes": source, "live_spool_bytes": live, "nodes": nodes,
            "output_bytes": output, "ram_only_bytes": live * 1.03 + output,
            "opfs_bytes": opfs, "zones": zones_formula(opfs, zone_bytes) if opfs else 0}


def zones_formula(opfs_peak_bytes: int, zone_bytes: int = GIB) -> int:
    """Zones to pre-open: twice the modelled OPFS peak plus one spare."""
    return -(-2 * opfs_peak_bytes // zone_bytes) + 1


def run_cell(events: list[dict], segment: str, policy: str, budget: float, order: str = "dfs",
             page: int = PAGE, small_spools: str = "spill") -> dict[str, Any]:
    run = simulate_zones(events, segment, policy, budget, order, page=page, zone_pages=GIB // page,
                         small_spools=small_spools)
    spilled_peak = run["peak_pages_payload_bytes"]
    run["opfs_over_live_spilled"] = round(run["opfs_peak_bytes"] / spilled_peak, 3) if spilled_peak else None
    run["kill_over_1_5x"] = bool(spilled_peak and run["opfs_peak_bytes"] > 1.5 * spilled_peak)
    run["zones_formula"] = zones_formula(run["opfs_peak_bytes"])
    run["zones_run_out"] = run["zones"] > run["zones_formula"]
    return run


def grid(events: list[dict]) -> list[dict[str, Any]]:
    rows = []

    def add(segment, policy, budget, order="dfs", page=PAGE, small_spools="spill"):
        run = run_cell(events, segment, policy, math.inf if budget is None else budget * GIB,
                       order, page, small_spools)
        rows.append({"segment": segment, "policy": policy, "budget_gib": budget, "order": order,
                     "page_kib": page // KIB, "small_spools": small_spools, **run})

    for segment in SEGMENTS:
        add(segment, "p3", None)  # no spill: RAM held against payload
        for policy in SPILL_POLICIES:
            for budget in (2, 4):
                add(segment, policy, budget)
    for page in (64 * KIB, 256 * KIB, PAGE):
        for small_spools in SMALL_SPOOLS:
            for policy in SPILL_POLICIES:
                for budget in (2, 4):
                    if (page, small_spools) != (PAGE, "spill"):
                        add(BASE_SEGMENT, policy, budget, page=page, small_spools=small_spools)
    for order in ("lifo", "fifo", "largest"):
        for policy in SPILL_POLICIES:
            for budget in (2, 4):
                add(BASE_SEGMENT, policy, budget, order=order)
    return rows


def summarize(report: dict[str, Any]) -> dict[str, Any]:
    """Cross-cell findings: pinning, kill criterion, zone formula, estimate vs simulation."""
    out: dict[str, Any] = {"cells": 0, "over_budget_cells": [], "kill_cells": {}, "zone_run_out_cells": 0,
                           "estimate": {}}
    for name, trace in report.items():
        for row in trace["runs"]:
            if row["budget_gib"] is None:
                continue
            out["cells"] += 1
            key = f"page{row['page_kib']}_{row['small_spools']}"
            out["kill_cells"].setdefault(key, [0, 0])
            out["kill_cells"][key][1] += 1
            out["kill_cells"][key][0] += row["kill_over_1_5x"]
            out["zone_run_out_cells"] += row["zones_run_out"]
            if row["over_budget_bytes"]:
                out["over_budget_cells"].append({"trace": name, "segment": row["segment"],
                                                 "policy": row["policy"], "budget_gib": row["budget_gib"],
                                                 "page_kib": row["page_kib"], "small_spools": row["small_spools"],
                                                 "over_budget_bytes": row["over_budget_bytes"],
                                                 "appends_over": row["over_budget_appends"]})
        live_sim = max(r["peak_live_payload_bytes"] for r in trace["runs"])
        est = {}
        for page in (256 * KIB, PAGE):
            for budget in (2, 4):
                e = estimate(trace["points"], trace["record_bytes"], trace["divisions"], 262_144,
                             budget * GIB, page)
                sims = [r for r in trace["runs"] if r["budget_gib"] == budget and r["page_kib"] == page // KIB
                        and r["segment"] == BASE_SEGMENT]
                opfs_sim = max((r["opfs_peak_bytes"] for r in sims), default=0)
                zones_sim = max((r["zones"] for r in sims), default=0)
                est[f"page{page // KIB}_b{budget}"] = {
                    "opfs_est_bytes": e["opfs_bytes"], "opfs_sim_max_bytes": opfs_sim,
                    "ratio": round(e["opfs_bytes"] / opfs_sim, 3) if opfs_sim else None,
                    "zones_est": e["zones"], "zones_sim_max": zones_sim}
        e = estimate(trace["points"], trace["record_bytes"], trace["divisions"], 262_144, 2 * GIB)
        out["estimate"][name] = {"live_est_bytes": e["live_spool_bytes"], "live_sim_max_bytes": live_sim,
                                 "live_ratio": round(e["live_spool_bytes"] / live_sim, 3),
                                 "nodes_est": e["nodes"], "nodes_actual": trace["nodes"],
                                 "ram_only_est_bytes": e["ram_only_bytes"], "cells": est}
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("logs", nargs="+", help="NAME=PATH@POINTS of convert-profile logs")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = {}
    for item in args.logs:
        name, _, rest = item.partition("=")
        path, _, points = rest.partition("@")
        parsed = parse_log(Path(path).read_text())
        events = parsed["events"]
        source = s10.source_bytes(events)
        report[name] = {"divisions": int(re.search(r"-d(\d+)", name).group(1)), "points": int(points),
                        "source_payload_bytes": source, "record_bytes": source / int(points),
                        "nodes": len(events), "leaf_nodes": sum(e["is_leaf"] for e in events),
                        "native_peak_live_spool_bytes": parsed["native_peak_live_spool_bytes"],
                        "runs": grid(events)}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({"traces": report, "summary": summarize(report)}, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
