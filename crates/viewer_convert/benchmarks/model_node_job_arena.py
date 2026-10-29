#!/usr/bin/env python3
"""Model the live-spool peak of per-node jobs from `real_dataset_bench convert-profile` logs.

Each internal node is a job: it reads its input spool and writes all its children into a
fresh arena (plus its own LOD sample). Leaf spools and LOD samples are read in the block
phase, so they stay alive until planning ends. An arena is freed only when ALL its extents
are dead. Job duration is the measured `elapsed_ns` of the node (relative weights only);
events carry no wall-clock claims.
"""

from __future__ import annotations

import argparse
import collections
import heapq
import json
from pathlib import Path
import sys
from typing import Any

EVENT_PREFIX = "internal_spatial_node_event="
SCALARS = {
    "internal_spatial_peak_live_spool_bytes": "native_peak_live_spool_bytes",
    "internal_spatial_spool_bytes_written": "spool_bytes_written",
}
POLICIES = ("fifo", "lifo", "largest")
# a: leaf/LOD extents share the arena that holds the children, so they pin it.
# b: leaf/LOD extents go to a retained arena; consumed sibling spools can be freed.
# c: like b, but every child spool is its own file, freed when the job that reads it ends.
# r: one arena per root branch, freed when the branch's subtree ends; leaves/LOD retained.
# own: no arenas, every spool freed at its job's end (pure concurrency baseline; the bytes of c).
VARIANTS = ("a", "b", "c", "r", "own")


def parse_log(text: str) -> dict[str, Any]:
    """Extract the native peak and the node events from one benchmark log."""
    result: dict[str, Any] = {"events": []}
    for line in text.splitlines():
        if line.startswith(EVENT_PREFIX):
            (node, parent, depth, inp, lod, leaf, equal, elapsed, start, end,
             children) = line[len(EVENT_PREFIX):].split(",")
            result["events"].append({
                "node_id": int(node), "parent_id": None if parent == "-" else int(parent),
                "depth": int(depth), "input_spool_bytes": int(inp), "lod_bytes": int(lod),
                "is_leaf": leaf == "1", "became_leaf_all_equal": equal == "1",
                "elapsed_ns": int(elapsed), "start_seq": int(start), "end_seq": int(end),
                "child_spool_bytes": [int(v) for v in children.split(";") if v],
            })
            continue
        key, _, value = line.partition("=")
        if key in SCALARS:
            result[SCALARS[key]] = int(value)
    missing = [name for name in [*SCALARS.values(), "events"] if not result.get(name)]
    if missing:
        raise ValueError(f"log lacks {missing}")
    return result


def source_bytes(events: list[dict]) -> int:
    """Payload of all source points: the root input, or the root's children if read from LAS."""
    root = next(e for e in events if e["parent_id"] is None)
    return root["input_spool_bytes"] or sum(root["child_spool_bytes"])


def critical_path_ns(events: list[dict]) -> int:
    """Longest chain of job durations with unlimited workers."""
    kids = collections.defaultdict(list)
    for event in events:
        kids[event["parent_id"]].append(event)
    best: dict[int, int] = {}
    for event in sorted(events, key=lambda e: -e["node_id"]):  # children have larger ids
        below = max((best[k["node_id"]] for k in kids[event["node_id"]]), default=0)
        is_job = not event["is_leaf"] or event["became_leaf_all_equal"]
        best[event["node_id"]] = (event["elapsed_ns"] if is_job else 0) + below
    return max(best[e["node_id"]] for e in kids[None])


def simulate(events: list[dict], workers: int, policy: str, variant: str) -> dict[str, int]:
    """Peak live spool bytes and makespan of `workers` concurrent node jobs.

    variant "a"/"b"/"c": see VARIANTS. variant "own" is a calibration: every spool dies at its
    own job end (native behaviour); with workers=1 and "lifo" it replays the native order.
    A job's child arena is counted in full when the job starts (upper bound: the real arena
    grows while the job runs). Pure leaves are no jobs: their spool is final.
    """
    by_id = {e["node_id"]: e for e in events}
    kids: dict[int | None, list[dict]] = collections.defaultdict(list)
    for event in sorted(events, key=lambda e: e["node_id"]):
        kids[event["parent_id"]].append(event)

    def is_job(event: dict) -> bool:
        return not event["is_leaf"] or event["became_leaf_all_equal"]

    live = sum(root["input_spool_bytes"] for root in kids[None])
    peak = live
    # Live files: arenas (a, b) or child spools (c), the retained arena (b, c) and a root spool.
    files = sum(1 for root in kids[None] if root["input_spool_bytes"]) + (variant in ("b", "c", "r"))
    max_files = files
    arenas: dict[int, dict] = {}
    # r: branch (depth-1 ancestor) of every non-root node, jobs left per branch, bytes per branch.
    branch_of: dict[int, int] = {}
    jobs_left: dict[int, int] = collections.defaultdict(int)
    branch_bytes: dict[int, int] = collections.defaultdict(int)
    branch_pinned: set[int] = set()
    for event in sorted(events, key=lambda e: e["node_id"]):  # parents precede children
        if event["parent_id"] is not None:
            branch_of[event["node_id"]] = (event["node_id"] if event["depth"] == 1
                                           else branch_of[event["parent_id"]])
            jobs_left[branch_of[event["node_id"]]] += is_job(event)
    ready: list = []
    counter = 0

    def push(event: dict) -> None:
        nonlocal counter
        counter += 1
        key = {"fifo": counter, "lifo": -counter,
               "largest": (-event["input_spool_bytes"], event["node_id"])}[policy]
        heapq.heappush(ready, (key, event["node_id"]))

    def start(event: dict) -> None:
        nonlocal live, peak, files, max_files
        if event["is_leaf"]:  # became a leaf: its scratch child spools are deleted again
            live += sum(event["child_spool_bytes"])
        else:
            children = kids[event["node_id"]]
            size = sum(c["input_spool_bytes"] for c in children)
            if variant in ("own", "c"):
                live += size
                # c: every non-plain-leaf child spool is its own file (leaves share the retained one).
                files += sum(1 for c in children if not c["is_leaf"] or c["became_leaf_all_equal"])
            elif variant == "r":
                live += size  # leaf spools join the retained arena; the rest joins its branch
                inner = [c for c in children if not c["is_leaf"] or c["became_leaf_all_equal"]]
                for child in inner:
                    key = child["node_id"] if event["parent_id"] is None else branch_of[event["node_id"]]
                    branch_bytes[key] += child["input_spool_bytes"]
                    files += event["parent_id"] is None
                    if child["became_leaf_all_equal"]:
                        branch_pinned.add(key)
            else:
                plain_leaves = [c for c in children if c["is_leaf"] and not c["became_leaf_all_equal"]]
                if variant == "b":
                    live += sum(c["input_spool_bytes"] for c in plain_leaves)
                    size -= sum(c["input_spool_bytes"] for c in plain_leaves)
                live += size
                if size:
                    files += 1
                arenas[event["node_id"]] = {
                    "bytes": size, "freed": False,
                    "pinned": variant == "a" or any(c["became_leaf_all_equal"] for c in children),
                    "outstanding": sum(1 for c in children if not c["is_leaf"]),
                }
        peak = max(peak, live)
        max_files = max(max_files, files)

    def maybe_free(node_id: int) -> None:
        nonlocal live, files
        arena = arenas[node_id]
        if not arena["pinned"] and not arena["outstanding"] and not arena["freed"]:
            arena["freed"] = True
            live -= arena["bytes"]
            files -= 1 if arena["bytes"] else 0

    def finish(event: dict) -> None:
        nonlocal live, peak, files
        if event["is_leaf"]:
            live -= sum(event["child_spool_bytes"])
        else:
            live += event["lod_bytes"]
            peak = max(peak, live)
            if variant in ("own", "c") or event["parent_id"] is None:
                live -= event["input_spool_bytes"]
                if variant == "c" and event["input_spool_bytes"]:
                    files -= 1
            if variant not in ("own", "c") and event["parent_id"] is None and event["input_spool_bytes"]:
                files -= 1
        parent = event["parent_id"]
        if variant == "r" and parent is not None:
            key = branch_of[event["node_id"]]
            jobs_left[key] -= 1
            if not jobs_left[key] and key not in branch_pinned:
                live -= branch_bytes[key]
                files -= 1
        if parent is not None and variant in ("a", "b") and not event["is_leaf"]:
            arenas[parent]["outstanding"] -= 1
            maybe_free(parent)
        if not event["is_leaf"] and event["node_id"] in arenas:
            maybe_free(event["node_id"])
        for child in kids[event["node_id"]]:
            if is_job(child):
                push(child)

    for root in kids[None]:  # single root in practice; successors would wait
        if is_job(root):
            push(root)
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
    return {"peak_bytes": peak, "makespan_ns": now, "max_live_files": max_files}


def model_log(parsed: dict[str, Any]) -> dict[str, Any]:
    events = parsed["events"]
    native = parsed["native_peak_live_spool_bytes"]
    source = source_bytes(events)
    rows = {}
    for variant in VARIANTS:
        for workers in (4, 8):
            for policy in POLICIES:
                run = simulate(events, workers, policy, variant)
                rows[f"{variant}_n{workers}_{policy}"] = {
                    **run, "ratio_to_native_peak": round(run["peak_bytes"] / native, 4),
                    "peak_over_source_bytes": round(run["peak_bytes"] / source, 4)}
    calibration = simulate(events, 1, "lifo", "own")
    calibration["ratio_to_native_peak"] = round(calibration["peak_bytes"] / native, 4)
    return {
        "native_peak_live_spool_bytes": native,
        "spool_bytes_written": parsed["spool_bytes_written"],
        "source_payload_bytes": source,
        "nodes": len(events),
        "leaf_nodes": sum(1 for e in events if e["is_leaf"]),
        "became_leaf_all_equal_nodes": sum(1 for e in events if e["became_leaf_all_equal"]),
        "retained_leaf_and_lod_bytes": sum(
            e["lod_bytes"] + (e["input_spool_bytes"] if e["is_leaf"] else 0) for e in events),
        "critical_path_ns": critical_path_ns(events),
        "calibration_n1_own_free_lifo": calibration,
        "modelled": rows,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("logs", nargs="+", help="NAME=PATH of convert-profile logs")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = {}
    for item in args.logs:
        name, _, path = item.partition("=")
        report[name] = model_log(parse_log(Path(path).read_text()))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
