#!/usr/bin/env python3
"""Summarize exported web viewer benchmark JSON files as a Markdown or CSV table."""

from __future__ import annotations

import argparse
import csv
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any


def comparable(run: dict[str, Any]) -> bool:
    stable = run.get("steadyState", {})
    scene = run.get("scene") or {}
    return (
        not stable.get("cameraChanged", True)
        and not stable.get("viewportChanged", True)
        and not stable.get("settingsChanged", True)
        and scene.get("coverageComplete") is True
        and scene.get("pendingBlocks") == 0
        and stable.get("measurementWindowMs", 0) >= 10_000
        and stable.get("meanFps") is not None
    )


def median(values: list[float | int | None]) -> str:
    numbers = [float(value) for value in values if isinstance(value, (float, int))]
    if not numbers:
        return "—"
    result = f"{statistics.median(numbers):.2f}"
    if len(numbers) > 1:
        result += f" [{min(numbers):.2f}–{max(numbers):.2f}]"
    return result


def run_group_key(run: dict[str, Any]) -> tuple[Any, ...]:
    dataset = run.get("dataset", {})
    configuration = run.get("configuration", {})
    environment = run.get("environment", {})
    adapter = environment.get("gpuStats", {}).get("adapter", {}) or {}
    return (
        environment.get("userAgent", "unknown browser"),
        adapter.get("name", "unknown GPU"),
        dataset.get("name", "unknown dataset"),
        dataset.get("sourcePointCount"),
        dataset.get("fileBytes"),
        tuple(configuration.get("viewportFramebuffer", [])),
        configuration.get("targetVoxelPixels"),
        configuration.get("colorMode"),
        configuration.get("depthShading"),
    )


def collect(paths: list[Path]) -> list[list[str]]:
    groups: dict[tuple[Any, ...], list[dict[str, Any]]] = defaultdict(list)
    for path in paths:
        run = json.loads(path.read_text(encoding="utf-8"))
        if run.get("schemaVersion") != 1:
            raise ValueError(f"{path}: unsupported benchmark schema")
        groups[run_group_key(run)].append(run)

    rows: list[list[str]] = []
    for key, runs in sorted(groups.items(), key=lambda entry: str(entry[0])):
        valid = [run for run in runs if comparable(run)]
        if not valid:
            rows.append([key[0], key[1], str(key[2]), str(key[3] or "—"), str(key[4] or "—"), "×".join(map(str, key[5])) or "—", str(key[6] or "—"), f"0/{len(runs)}", *(["—"] * 8)])
            continue
        loads = [run.get("load", {}) for run in valid]
        steady = [run.get("steadyState", {}) for run in valid]
        scenes = [run.get("scene", {}) or {} for run in valid]
        environments = [run.get("environment", {}) for run in valid]
        rows.append([
            str(key[0]),
            str(key[1]),
            str(key[2]),
            str(key[3] or "—"),
            str(key[4] or "—"),
            "×".join(map(str, key[5])) or "—",
            str(key[6] or "—"),
            f"{len(valid)}/{len(runs)}",
            median([load.get("metadataOpenMs") for load in loads]),
            median([load.get("firstCoarseFrameMs") for load in loads]),
            median([load.get("targetLodMs") for load in loads]),
            median([steady.get("meanFps") for steady in steady]),
            median([steady.get("frameReplyIntervalP95Ms") for steady in steady]),
            median([steady.get("workerRenderCallP95Ms") for steady in steady]),
            median([scene.get("renderedVoxels") for scene in scenes]),
            median([environment.get("jsHeapUsedBytes") for environment in environments]),
        ])
    return rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("runs", nargs="+", type=Path, help="exported web-bench-*.json files")
    parser.add_argument("--format", choices=("markdown", "csv"), default="markdown")
    args = parser.parse_args()
    headers = [
        "Browser", "GPU adapter", "Dataset", "Points", "File bytes", "Viewport", "Target px",
        "Valid runs", "Metadata ms", "First coarse ms", "Target LOD ms", "FPS", "Reply p95 ms",
        "CPU frame p95 ms", "Voxels", "JS heap bytes",
    ]
    rows = collect(args.runs)
    if args.format == "csv":
        writer = csv.writer(sys.stdout)
        writer.writerow(headers)
        writer.writerows(rows)
        return
    print("| " + " | ".join(headers) + " |")
    print("| " + " | ".join("---" for _ in headers) + " |")
    for row in rows:
        print("| " + " | ".join(value.replace("|", "\\|") for value in row) + " |")


if __name__ == "__main__":
    main()
