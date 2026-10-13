#!/usr/bin/env python3
"""Sample a Linux process tree; PSS/RSS are not JS/WASM heap or dedicated VRAM."""

from __future__ import annotations

import argparse
import json
import math
import time
from pathlib import Path

FIELDS = ("Rss", "Pss", "Pss_Anon", "Pss_File", "Pss_Shmem", "SwapPss")


def identity(path: Path) -> tuple[int, int]:
    # comm may contain spaces and parentheses; fields after its last ')' start at state.
    fields = (path / "stat").read_text().rsplit(")", 1)[1].split()
    return int(fields[1]), int(fields[19])  # parent PID, kernel starttime


def memory_fields(text: str) -> dict[str, int]:
    values = {}
    for line in text.splitlines():
        name, _, rest = line.partition(":")
        if name in FIELDS:
            value, unit = rest.split()
            if unit != "kB" or int(value) < 0:
                raise ValueError(f"invalid {name} memory counter")
            values[name] = int(value) * 1024
    if not {"Rss", "Pss"} <= values.keys():
        raise ValueError("missing Rss/Pss counters")
    return values


def snapshot(pid: int, starttime: int, proc: Path = Path("/proc")) -> dict:
    table = {}
    for entry in proc.iterdir():
        if entry.name.isdigit():
            try:
                table[int(entry.name)] = identity(entry)
            except (OSError, ValueError, IndexError):
                continue
    if pid not in table or table[pid][1] != starttime:
        return {
            "root_alive": False,
            "enumerated_processes_readable": False,
            "processes": [],
        }
    selected = {pid}
    while True:
        children = {child for child, (parent, _) in table.items() if parent in selected}
        if children <= selected:
            break
        selected.update(children)
    processes, unreadable = [], []
    for child in sorted(selected):
        entry = proc / str(child)
        try:
            values = memory_fields((entry / "smaps_rollup").read_text())
            if identity(entry) != table[child]:
                raise ValueError("process identity changed during sample")
            processes.append(
                {"pid": child, "starttime": table[child][1], "bytes": values}
            )
        except (OSError, ValueError, IndexError):
            unreadable.append(child)
    # Root may exit while descendants are being read. Do not call that a complete sample.
    try:
        root_alive = identity(proc / str(pid))[1] == starttime
    except (OSError, ValueError, IndexError):
        root_alive = False
    totals = {
        field: sum(process["bytes"][field] for process in processes)
        if processes and all(field in process["bytes"] for process in processes)
        else None
        for field in FIELDS
    }
    return {
        "root_alive": root_alive,
        "enumerated_processes_readable": root_alive and not unreadable,
        "enumerated_processes": len(selected),
        "unreadable_pids": unreadable,
        "tree_bytes": totals,
        "processes": processes,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", type=int)
    parser.add_argument("output", type=Path)
    parser.add_argument("--interval", type=float, default=1.0)
    parser.add_argument("--seconds", type=float, default=1200.0)
    args = parser.parse_args()
    if args.pid <= 0 or not all(
        math.isfinite(v) and v > 0 for v in (args.interval, args.seconds)
    ):
        parser.error("PID, interval and duration must be positive and finite")
    _, starttime = identity(Path("/proc") / str(args.pid))
    started = time.monotonic()
    # Exclusive creation prevents accidental mixing or overwriting of measurement series.
    with args.output.open("x", encoding="utf-8") as stream:
        while True:
            record = snapshot(args.pid, starttime)
            record.update(
                {
                    "schema_version": 1,
                    "root_pid": args.pid,
                    "root_starttime": starttime,
                    "wall_time_ns": time.time_ns(),
                    "elapsed_seconds": time.monotonic() - started,
                    "interval_seconds": args.interval,
                }
            )
            record["outcome"] = (
                "root_exited"
                if not record["root_alive"]
                else (
                    "duration_cap"
                    if record["elapsed_seconds"] >= args.seconds
                    else "sampling"
                )
            )
            stream.write(json.dumps(record) + "\n")
            stream.flush()
            if record["outcome"] != "sampling":
                break
            time.sleep(
                min(args.interval, max(0, args.seconds - (time.monotonic() - started)))
            )


if __name__ == "__main__":
    main()
