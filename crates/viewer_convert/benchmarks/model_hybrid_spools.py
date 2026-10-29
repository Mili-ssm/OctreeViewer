#!/usr/bin/env python3
"""Model the RAM peak of the web hybrid spool store (S10) on node-job event traces.

Spools live as JS ArrayBuffer segments in RAM and are spilled to ONE OPFS file per spool. A
wasm job runs synchronously, so a sync access handle must be opened (async) by the host
BEFORE the job starts; the root-partition job writes the whole dataset, so spilling only
between jobs cannot bound RAM by itself. Policies:

  p0   RAM only (upper bound; Chrome file:// case).
  p1   admission by input: a job whose `input_spool_bytes` does not fit in the free budget gets
       handles for all its outputs (children + LOD) and writes them to disk; otherwise RAM.
       Between jobs the host spills retained spools (leaves, LOD: read in the block phase)
       largest first until RAM is under budget.
  p1r  p1, but before falling back to handles the host spills retained and then queued input
       spools (consumed latest first) to make room for the job.
  p2   handles for every job (the S9 layout; everything on disk).
  p3   coordinator-only spills: workers never touch OPFS, they post transferred segments; the
       coordinator keeps the RAM ledger and, when over budget, opens handles asynchronously and
       spills spools (retained, queued inputs, then outputs still being written, in that order).
       A job never blocks on a handle. True RAM = ledger (<= budget) + in-flight segments
       (N workers x K unacknowledged segments x segment size), which backpressure bounds.

Accounting follows `model_node_job_arena` variant c: a job's child spools are counted in full
when it starts (upper bound), its LOD when it ends, its input spool when it ends. Admission
(p1, p1r) reserves the worst case, input_spool_bytes + lod_bytes. A disk-mode job pre-opens
`divisions**3 + 1` handles (the host cannot know which children stay empty; `divisions=None`
uses the number of non-empty outputs instead); handles of empty outputs are closed and removed.
"""

from __future__ import annotations

import argparse
import collections
import heapq
import json
import re
from pathlib import Path
import sys
from typing import Any

from model_node_job_arena import POLICIES, parse_log, source_bytes

MODES = ("p0", "p1", "p1r", "p2", "p3")
GIB = 1 << 30
# S9 cost per handle/file in ms (low, high): Firefox 22 (per handle) to 40 (per file); Chrome per file.
COST_MS = {"firefox": (22.0, 40.0), "chrome": (2.9, 4.4)}
# S9 removeEntry p50 (low, high) in ms: what an empty pre-opened handle costs on top of its open.
EMPTY_REMOVE_MS = {"firefox": (25.0, 40.0), "chrome": (0.5, 1.4)}


def simulate_hybrid(events: list[dict], workers: int, order: str, mode: str, budget: int,
                    root_input_spool: bool = False, divisions: int | None = None) -> dict[str, int]:
    """Run `workers` concurrent node jobs under the hybrid store; `budget` in bytes of RAM spools.

    root_input_spool=False models the web layout (the root partition reads the Blob, so there is
    no root input spool); True is the native layout used to calibrate against variant c.
    """
    if mode not in MODES or order not in POLICIES:
        raise ValueError("unknown mode or order")
    if any(e["became_leaf_all_equal"] for e in events):
        raise ValueError("became_leaf_all_equal nodes are not modelled")
    by_id = {e["node_id"]: e for e in events}
    kids: dict[int | None, list[dict]] = collections.defaultdict(list)
    for event in sorted(events, key=lambda e: e["node_id"]):
        kids[event["parent_id"]].append(event)

    st = {"ram": 0, "disk": 0, "files": 0, "open": 0, "peak_ram": 0, "peak_disk": 0,
          "peak_files": 0, "peak_open": 0, "spilled": 0, "created": 0, "handles": 0,
          "disk_jobs": 0, "job_handles": 0, "empty_handles": 0, "inprogress_spilled": 0}
    running_outputs: dict[int, list[tuple[str, int]]] = {}
    # spool_id -> [bytes, "ram"|"disk", kind]; kind "inner" is freed when its job ends,
    # "leaf"/"lod" are retained until the block phase.
    spools: dict[tuple[str, int], list] = {}
    ready: list = []
    counter = 0

    def touch() -> None:
        for key, value in (("peak_ram", st["ram"]), ("peak_disk", st["disk"]),
                           ("peak_files", st["files"]), ("peak_open", st["open"])):
            st[key] = max(st[key], value)

    def add(spool_id: tuple[str, int], size: int, kind: str, where: str, new_file: bool) -> None:
        if not size:
            return
        spools[spool_id] = [size, where, kind]
        st[where] += size
        if where == "disk":
            st["files"] += 1
            if new_file:
                st["spilled"] += size
                st["created"] += 1

    def remove(spool_id: tuple[str, int]) -> None:
        size, where, _ = spools.pop(spool_id, (0, "ram", ""))
        st[where] -= size
        if size and where == "disk":
            st["files"] -= 1

    def spill(spool_id: tuple[str, int]) -> None:
        spool = spools[spool_id]
        st["ram"] -= spool[0]
        st["disk"] += spool[0]
        st["files"] += 1
        st["spilled"] += spool[0]
        st["created"] += 1
        st["handles"] += 1
        st["peak_open"] = max(st["peak_open"], st["open"] + 1)  # transient handle during the copy
        st["peak_disk"] = max(st["peak_disk"], st["disk"])
        st["peak_files"] = max(st["peak_files"], st["files"])
        spool[1] = "disk"

    def candidates(exclude: tuple[str, int] | None) -> list[tuple[str, int]]:
        writing = {i for ids in running_outputs.values() for i in ids}
        retained = sorted((i for i, s in spools.items()
                           if s[1] == "ram" and s[2] != "inner" and i not in writing),
                          key=lambda i: (-spools[i][0], i))
        queued = [("node", node_id) for _, node_id in sorted(ready, reverse=True)]
        inner = [i for i in queued if i != exclude and i in spools and spools[i][1] == "ram"]
        inner = [i for i in inner if i not in writing]
        if mode != "p3":
            return retained + inner
        # p3 only: outputs still being written (the coordinator owns the file from then on)
        unfinished = sorted((i for i in writing if i in spools and spools[i][1] == "ram"),
                            key=lambda i: (-spools[i][0], i))
        return retained + inner + unfinished

    def spill_until(target: int, exclude: tuple[str, int] | None = None) -> bool:
        """Spill candidates (latest consumed first) until RAM <= target; False if impossible."""
        picked = candidates(exclude)
        if st["ram"] - sum(spools[i][0] for i in picked) > target:
            return False
        for spool_id in picked:
            if st["ram"] <= target:
                break
            if spool_id in {i for ids in running_outputs.values() for i in ids}:
                st["inprogress_spilled"] += spools[spool_id][0]
            spill(spool_id)
        return True

    def push(event: dict) -> None:
        nonlocal counter
        counter += 1
        key = {"fifo": counter, "lifo": -counter,
               "largest": (-event["input_spool_bytes"], event["node_id"])}[order]
        heapq.heappush(ready, (key, event["node_id"]))

    def is_job(event: dict) -> bool:
        return not event["is_leaf"]

    disk_jobs: set[int] = set()
    job_open: dict[int, int] = {}

    def start(event: dict) -> None:
        children = [c for c in kids[event["node_id"]] if c["input_spool_bytes"]]
        own = ("node", event["node_id"])
        bound = (event["input_spool_bytes"] or sum(c["input_spool_bytes"] for c in children)) \
            + event["lod_bytes"]
        if mode in ("p0", "p3"):
            disk = False
        elif mode == "p2":
            disk = True
        else:
            disk = bound > budget - st["ram"]
            if disk and mode == "p1r":
                disk = not spill_until(budget - bound, exclude=own)
        if disk:
            disk_jobs.add(event["node_id"])
            used = len(children) + (1 if event["lod_bytes"] else 0)
            opened = divisions ** 3 + 1 if divisions else used
            st["handles"] += opened
            st["job_handles"] += opened
            st["empty_handles"] += opened - used
            st["disk_jobs"] += 1
            st["open"] += opened
            job_open[event["node_id"]] = opened
        running_outputs[event["node_id"]] = []
        for child in children:
            kind = "leaf" if child["is_leaf"] else "inner"
            add(("node", child["node_id"]), child["input_spool_bytes"], kind,
                "disk" if disk else "ram", True)
            running_outputs[event["node_id"]].append(("node", child["node_id"]))
        if mode == "p3":
            spill_until(budget)  # the coordinator spills before the in-flight segments pile up
        touch()

    def finish(event: dict) -> None:
        node_id = event["node_id"]
        disk = node_id in disk_jobs
        if disk:
            st["open"] -= job_open.pop(node_id)
        running_outputs.pop(node_id, None)
        add(("lod", node_id), event["lod_bytes"], "lod", "disk" if disk else "ram", True)
        if mode == "p3":
            spill_until(budget)
        touch()
        remove(("node", node_id))  # the job consumed its input spool (absent for the web root)
        for child in kids[node_id]:
            if is_job(child):
                push(child)
        if mode in ("p1", "p1r", "p3"):
            spill_until(budget)

    for root in kids[None]:
        if root["input_spool_bytes"] and root_input_spool:
            add(("node", root["node_id"]), root["input_spool_bytes"], "inner",
                "disk" if mode == "p2" else "ram", False)
        if is_job(root):
            push(root)
    touch()
    now = 0
    free_workers = workers
    running: list[tuple[int, int]] = []
    while ready or running:
        while free_workers and ready:
            _, node_id = heapq.heappop(ready)
            free_workers -= 1
            event = by_id[node_id]
            start(event)
            heapq.heappush(running, (now + max(1, event["elapsed_ns"]), node_id))
        now, node_id = heapq.heappop(running)
        free_workers += 1
        finish(by_id[node_id])
    return {"peak_ram_bytes": st["peak_ram"], "peak_disk_bytes": st["peak_disk"],
            "spilled_bytes": st["spilled"], "files_created": st["created"],
            "peak_live_files": st["peak_files"], "handles_opened": st["handles"],
            "max_open_handles": st["peak_open"], "makespan_ns": now,
            "disk_jobs": st["disk_jobs"], "job_handles": st["job_handles"],
            "empty_handles": st["empty_handles"], "inprogress_spilled_bytes": st["inprogress_spilled"],
            "final_ram_bytes": st["ram"], "final_disk_bytes": st["disk"]}


def cost_s(handles: int, empty: int = 0) -> dict[str, list[float]]:
    """Wall-equivalent seconds (low, high) per browser, from S9: opening plus removing empties."""
    return {name: [round((handles * COST_MS[name][i] + empty * EMPTY_REMOVE_MS[name][i]) / 1000, 1)
                   for i in (0, 1)] for name in COST_MS}


def ranged_root(events: list[dict], divisions: int) -> dict[str, Any]:
    """Root partition as W chunk-range jobs, each pre-opening divisions**3 child handles."""
    root = next(e for e in events if e["parent_id"] is None)
    nonempty = len(root["child_spool_bytes"])
    rows = {}
    for ranges in (1, 4, 8):
        handles = ranges * divisions ** 3 + 1
        # Lower bound: a child empty for the whole dataset is empty in every range; children
        # that are empty only in some ranges cannot be told apart in the trace.
        empty = ranges * (divisions ** 3 - nonempty)
        rows[f"w{ranges}"] = {"handles": handles, "empty_lower_bound": empty,
                              "cost_s": cost_s(handles, empty), "setup_s_per_job": cost_s(handles)}
    return rows


def model_trace(events: list[dict], native_peak: int | None = None, divisions: int = 2) -> dict[str, Any]:
    source = source_bytes(events)
    root = next(e for e in events if e["parent_id"] is None)
    rows: dict[str, Any] = {}
    for workers in (1, 4, 8):
        for order in POLICIES:
            for mode in MODES:
                budgets = (0.5, 1, 2, 3) if mode in ("p1", "p1r", "p3") else (None,)
                for gib in budgets:
                    run = simulate_hybrid(events, workers, order, mode,
                                          int(gib * GIB) if gib else 0, divisions=divisions)
                    run["handles_listed"] = run["handles_opened"] - run["empty_handles"]
                    run["handle_cost_s"] = cost_s(run["handles_opened"], run["empty_handles"])
                    jobs = run["disk_jobs"]
                    run["job_setup_s_per_disk_job"] = cost_s(run["job_handles"] // jobs)["firefox"] if jobs else None
                    run["spill_rate_mb_s"] = round(run["spilled_bytes"] / 1e6 / (run["makespan_ns"] / 1e9), 1)
                    run["spill_files_per_s"] = round(run["files_created"] / (run["makespan_ns"] / 1e9), 1)
                    rows[f"{mode}_b{gib if gib else 'na'}_n{workers}_{order}"] = run
    root_bytes = sum(c["input_spool_bytes"] for c in events if c["parent_id"] == root["node_id"])
    return {"native_peak_live_spool_bytes": native_peak, "source_payload_bytes": source,
            "divisions": divisions, "nodes": len(events),
            "leaf_nodes": sum(1 for e in events if e["is_leaf"]),
            "root_nonempty_children": len(root["child_spool_bytes"]),
            "root_output_bytes": root_bytes, "root_elapsed_ns": root["elapsed_ns"],
            "root_write_rate_mb_s": round(root_bytes / 1e6 / (root["elapsed_ns"] / 1e9), 1),
            "ranged_root": ranged_root(events, divisions), "modelled": rows}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("logs", nargs="+", help="NAME=PATH of convert-profile logs")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = {}
    for item in args.logs:
        name, _, path = item.partition("=")
        parsed = parse_log(Path(path).read_text())
        divisions = int(re.search(r"-d(\d+)", name).group(1))
        report[name] = model_trace(parsed["events"], parsed["native_peak_live_spool_bytes"], divisions)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
